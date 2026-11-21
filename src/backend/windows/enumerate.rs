//! SetupAPI 枚举 + 分区表 / 文件系统签名 / 熵值探测。

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    ReadFile, SetFilePointerEx, FILE_BEGIN,
};
use windows::core::PCWSTR;

use super::{
    CachedDevice, DeviceCatalog, RawVolume, close_handle, disk_interface_guid, disk_path,
    enumerate_volumes, is_supported_bus, open_path, query_descriptor, query_device_number,
    query_geometry, system_disk_numbers, wide_to_string, GENERIC_READ,
};
use crate::core::device::{
    DeviceCandidate, DeviceId, DiskState, PartitionInfo, PartitionTableKind, VolumeInfo,
};
use crate::core::error::{BurnError, Result};
use crate::core::traits::DeviceEnumerator;

pub struct WindowsDeviceEnumerator {
    pub(crate) catalog: Arc<DeviceCatalog>,
}

impl WindowsDeviceEnumerator {
    pub(crate) fn from_catalog(catalog: Arc<DeviceCatalog>) -> Self {
        Self { catalog }
    }

    /// 轻量设备签名：只列设备接口路径，不打开磁盘、不读扇区。
    ///
    /// 用于自动检测插拔；只有签名变化后才做一次完整枚举。
    pub fn interface_signature(&self) -> Result<Vec<String>> {
        let mut paths = list_disk_interfaces()?;
        paths.sort();
        Ok(paths)
    }
}

impl DeviceEnumerator for WindowsDeviceEnumerator {
    fn enumerate(&self) -> Result<Vec<DeviceCandidate>> {
        self.catalog.release_temporary_mounts();

        let system = unsafe { system_disk_numbers() };
        let interfaces = list_disk_interfaces()?;
        let mut assigned_letters = HashSet::new();
        let mut volumes = enumerate_volumes(&self.catalog, None, &mut assigned_letters);
        let mut grouped: HashMap<u32, Vec<RawVolume>> = HashMap::new();
        for volume in volumes.drain(..) {
            grouped.entry(volume.disk_number).or_default().push(volume);
        }
        // 预先收集已用盘符，避免临时挂载冲突。
        for list in grouped.values() {
            for volume in list {
                if let Some(letter) = volume.info.letter {
                    assigned_letters.insert(letter);
                }
            }
        }

        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        let mut cache = Vec::new();
        for interface_path in interfaces {
            let (number, _) = match probe_interface(&interface_path) {
                Some(value) => value,
                None => continue,
            };
            if !seen.insert(number) {
                continue;
            }
            if let Some((candidate, cached)) = inspect_disk(
                &interface_path,
                &system,
                Some(number),
                grouped.remove(&number).unwrap_or_default(),
            ) {
                cache.push((candidate.id.as_str().to_string(), cached));
                candidates.push(candidate);
            }
        }

        self.catalog.replace(cache);
        Ok(candidates)
    }

    fn resolve(&self, ids: &[DeviceId]) -> Result<Vec<DeviceCandidate>> {
        let system = unsafe { system_disk_numbers() };
        let mut targets: Vec<(DeviceId, CachedDevice)> = Vec::with_capacity(ids.len());
        for id in ids {
            match self.catalog.get(id.as_str()) {
                Some(cached) => targets.push((id.clone(), cached)),
                None => {
                    return Err(BurnError::IdentityChanged { device: id.clone() });
                }
            }
        }

        let allowed: HashSet<u32> = targets.iter().map(|(_, cached)| cached.physical_drive).collect();
        let mut assigned_letters = HashSet::new();
        let mut volumes = enumerate_volumes(&self.catalog, Some(&allowed), &mut assigned_letters);
        let mut grouped: HashMap<u32, Vec<RawVolume>> = HashMap::new();
        for volume in volumes.drain(..) {
            grouped.entry(volume.disk_number).or_default().push(volume);
        }

        let mut result = Vec::with_capacity(targets.len());
        for (id, cached) in targets {
            let (candidate, refreshed) = match inspect_disk(
                &cached.interface_path,
                &system,
                Some(cached.physical_drive),
                grouped.remove(&cached.physical_drive).unwrap_or_default(),
            ) {
                Some(value) => value,
                None => {
                    return Err(BurnError::IdentityChanged { device: id });
                }
            };
            if candidate.id != id {
                return Err(BurnError::IdentityChanged { device: id });
            }
            self.catalog.update(id.as_str(), refreshed);
            result.push(candidate);
        }
        Ok(result)
    }
}

/// 列出磁盘设备接口路径。
fn list_disk_interfaces() -> Result<Vec<String>> {
    let guid = disk_interface_guid();
    unsafe {
        let devices = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
        .map_err(|error| BurnError::message(format!("枚举磁盘设备失败: {error}")))?;
        let owned = DeviceInfoSet(devices);

        let mut paths = Vec::new();
        let mut index = 0u32;
        loop {
            let mut data = SP_DEVICE_INTERFACE_DATA::default();
            data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(owned.0, None, &guid, index, &mut data).is_err() {
                break;
            }
            index += 1;

            let mut needed = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                owned.0,
                &data,
                None,
                0,
                Some(&mut needed),
                None,
            );
            if needed == 0 {
                continue;
            }
            let words = (needed as usize + 3) / 4;
            let mut buffer = vec![0u32; words];
            let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                owned.0,
                &data,
                Some(detail),
                needed,
                None,
                None,
            )
            .is_err()
            {
                continue;
            }
            let path = wide_to_string((*detail).DevicePath.as_ptr());
            if !path.is_empty() {
                paths.push(path);
            }
        }
        Ok(paths)
    }
}

struct DeviceInfoSet(windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// 打开接口并读取物理盘号 / 容量 / 描述。
fn probe_interface(path: &str) -> Option<(u32, DeviceProbe)> {
    let handle = open_path(path, 0, 0)
        .or_else(|_| open_path(path, GENERIC_READ, 0))
        .ok()?;
    let mut owned = Some(handle);
    let number = unsafe { query_device_number(handle) }.ok();
    let descriptor = unsafe { query_descriptor(handle) }.ok();
    let geometry = unsafe { query_geometry(handle) }.ok();
    unsafe { close_handle(&mut owned) };

    let number = number?;
    let descriptor = descriptor?;
    let (capacity, sector) = geometry?;
    Some((
        number,
        DeviceProbe {
            descriptor,
            capacity,
            sector,
        },
    ))
}

struct DeviceProbe {
    descriptor: super::StorageDescriptor,
    capacity: u64,
    sector: u32,
}

/// 完整检查一个接口；`expected_drive` 用于 resolve 时校验物理盘号未变。
fn inspect_disk(
    interface_path: &str,
    system: &HashSet<u32>,
    expected_drive: Option<u32>,
    volumes: Vec<RawVolume>,
) -> Option<(DeviceCandidate, CachedDevice)> {
    let (number, probe) = probe_interface(interface_path)?;
    if let Some(expected) = expected_drive {
        if number != expected {
            return None;
        }
    }
    if system.contains(&number) {
        return None;
    }
    if !probe.descriptor.removable || !is_supported_bus(probe.descriptor.bus_type) {
        return None;
    }
    if probe.capacity == 0 || probe.sector == 0 {
        return None;
    }

    let id = DeviceId::derive(
        &probe.descriptor.serial,
        probe.capacity,
        interface_path,
    );

    let layout = unsafe { probe_layout(number, probe.capacity, probe.sector, &volumes) };
    let recognized = !volumes.is_empty() && volumes.iter().any(|volume| volume.recognized);
    let has_partition_fs = layout.partitions.iter().any(|part| part.file_system.is_some());
    let state = if recognized || has_partition_fs {
        DiskState::HasContent
    } else if layout.corrupt || !layout.readable {
        DiskState::Unknown
    } else if layout.partitions.is_empty() {
        DiskState::Empty
    } else {
        DiskState::Unknown
    };

    let bus = match probe.descriptor.bus_type {
        super::SYSTEM_BUS_USB => "USB",
        super::SYSTEM_BUS_SD => "SD",
        super::SYSTEM_BUS_MMC => "MMC",
        _ => "Other",
    };

    let volume_infos: Vec<VolumeInfo> = volumes.iter().map(|volume| volume.info.clone()).collect();
    let volume_names = volume_infos
        .iter()
        .map(|volume| volume.volume_name.clone())
        .collect();
    let model = format!(
        "{} {}",
        probe.descriptor.vendor, probe.descriptor.product
    )
    .trim()
    .to_string();

    let candidate = DeviceCandidate {
        id: id.clone(),
        capacity_bytes: probe.capacity,
        bus: bus.to_string(),
        model,
        removable: probe.descriptor.removable,
        is_system: false,
        state,
        table: layout.table,
        partitions: layout.partitions,
        volumes: volume_infos,
        entropy: layout.entropy,
    };
    let cached = CachedDevice {
        interface_path: interface_path.to_string(),
        physical_drive: number,
        capacity_bytes: probe.capacity,
        volume_names,
    };
    Some((candidate, cached))
}

struct LayoutProbe {
    table: PartitionTableKind,
    partitions: Vec<PartitionInfo>,
    entropy: Option<f32>,
    readable: bool,
    corrupt: bool,
}

/// 读取分区表、引导扇区签名与熵值。
unsafe fn probe_layout(
    number: u32,
    capacity: u64,
    sector: u32,
    volumes: &[RawVolume],
) -> LayoutProbe {
    let mut empty = LayoutProbe {
        table: PartitionTableKind::Unknown,
        partitions: Vec::new(),
        entropy: None,
        readable: false,
        corrupt: false,
    };

    let handle = match open_path(&disk_path(number), GENERIC_READ, 0) {
        Ok(value) => value,
        Err(_) => return empty,
    };
    let mut owned = Some(handle);
    let _ = &mut owned;

    let mut head = vec![0u8; 1024 * 1024];
    let read = read_upto(handle, 0, &mut head);
    if read == 0 {
        return empty;
    }
    head.truncate(read);
    empty.readable = true;
    empty.entropy = Some(entropy(&head));
    if head.len() < 512 {
        return empty;
    }

    let signature_ok = head[510] == 0x55 && head[511] == 0xAA;
    if !signature_ok {
        return empty;
    }

    let parse = |offset: u64, size: u64, index: u32| -> PartitionInfo {
        let (file_system, letter) = volume_for_offset(volumes, offset, size);
        let detected = detect_partition_signature(handle, offset, size, sector);
        let signature = detected.or_else(|| file_system.clone());
        PartitionInfo {
            index,
            offset_bytes: offset,
            size_bytes: size,
            file_system,
            signature,
            drive_letter: letter,
        }
    };

    let protective = head[446..510]
        .chunks_exact(16)
        .any(|entry| entry[4] == 0xEE);
    if protective {
        empty.table = PartitionTableKind::Gpt;
        match read_gpt(handle, capacity, sector) {
            Some(entries) => {
                for (index, (offset, size)) in entries.into_iter().enumerate() {
                    empty
                        .partitions
                        .push(parse(offset, size, index as u32 + 1));
                }
            }
            None => {
                empty.corrupt = true;
            }
        }
        return empty;
    }

    let mut valid = false;
    for (index, entry) in head[446..510].chunks_exact(16).enumerate() {
        let partition_type = entry[4];
        if partition_type == 0 {
            continue;
        }
        valid = true;
        let start_lba = le_u32(entry, 8).unwrap_or(0) as u64;
        let sectors = le_u32(entry, 12).unwrap_or(0) as u64;
        if sectors == 0 {
            empty.corrupt = true;
            continue;
        }
        let offset = start_lba * 512;
        let size = sectors * 512;
        if offset.saturating_add(size) > capacity {
            empty.corrupt = true;
            continue;
        }
        empty
            .partitions
            .push(parse(offset, size, index as u32 + 1));
    }
    empty.table = if valid || !empty.partitions.is_empty() {
        PartitionTableKind::Mbr
    } else {
        PartitionTableKind::Unknown
    };
    empty
}

fn volume_for_offset(
    volumes: &[RawVolume],
    offset: u64,
    size: u64,
) -> (Option<String>, Option<char>) {
    for volume in volumes {
        if volume.recognized
            && volume.info.start_offset >= offset
            && volume.info.start_offset < offset.saturating_add(size.max(1))
        {
            return (
                Some(volume.info.file_system.clone()),
                volume.info.letter,
            );
        }
    }
    (None, None)
}

/// 读取 GPT 分区项（偏移, 大小）。
unsafe fn read_gpt(handle: windows::Win32::Foundation::HANDLE, capacity: u64, sector: u32) -> Option<Vec<(u64, u64)>> {
    let sector_size = sector.max(512) as u64;
    let mut header = vec![0u8; sector_size as usize];
    if read_exact(handle, sector_size, &mut header).is_none() {
        return None;
    }
    if header.get(0..8) != Some(b"EFI PART") {
        return None;
    }
    let entry_lba = le_u64(&header, 72)?;
    let count = le_u32(&header, 80)?;
    let entry_size = le_u32(&header, 84)?;
    if count == 0 || entry_size < 128 || count > 1024 {
        return None;
    }
    let total = count as u64 * entry_size as u64;
    if total > 4 * 1024 * 1024 {
        return None;
    }
    let offset = entry_lba.checked_mul(sector_size)?;
    if offset.saturating_add(total) > capacity {
        return None;
    }
    let mut entries = vec![0u8; total as usize];
    if read_exact(handle, offset, &mut entries).is_none() {
        return None;
    }

    let mut result = Vec::new();
    for index in 0..count as usize {
        let start = index * entry_size as usize;
        if start + entry_size as usize > entries.len() {
            break;
        }
        let entry = &entries[start..start + entry_size as usize];
        if entry[0..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first = le_u64(entry, 32)?;
        let last = le_u64(entry, 40)?;
        if last < first {
            continue;
        }
        let offset = first.checked_mul(sector_size)?;
        let size = (last - first + 1).checked_mul(sector_size)?;
        if offset.saturating_add(size) > capacity {
            continue;
        }
        result.push((offset, size));
    }
    Some(result)
}

/// 读取分区引导扇区并识别文件系统签名。
unsafe fn detect_partition_signature(
    handle: windows::Win32::Foundation::HANDLE,
    offset: u64,
    size: u64,
    sector: u32,
) -> Option<String> {
    let sector_size = sector.max(512) as u64;
    let read_len = (40 * 1024).min(size).max(sector_size);
    let read_len = (read_len / sector_size) * sector_size;
    if read_len == 0 || read_len > size {
        return None;
    }
    let mut buffer = vec![0u8; read_len as usize];
    if read_exact(handle, offset, &mut buffer).is_none() {
        return None;
    }
    detect_signature(&buffer)
}

fn detect_signature(buffer: &[u8]) -> Option<String> {
    let has = |range: std::ops::Range<usize>, text: &[u8]| -> bool {
        buffer.get(range) == Some(text)
    };
    if has(3..11, b"NTFS    ") {
        return Some("NTFS".into());
    }
    if has(3..11, b"EXFAT   ") {
        return Some("exFAT".into());
    }
    if has(82..90, b"FAT32   ") {
        return Some("FAT32".into());
    }
    if has(54..62, b"FAT16   ") {
        return Some("FAT16".into());
    }
    if has(54..62, b"FAT12   ") {
        return Some("FAT12".into());
    }
    if buffer.len() >= 1082 {
        let magic = u16::from_le_bytes([buffer[1080], buffer[1081]]);
        if magic == 0x53EF {
            return Some("ext-family".into());
        }
    }
    if has(32769..32774, b"CD001") {
        return Some("ISO9660".into());
    }
    if has(1024..1026, b"H+") || has(1024..1026, b"BD") {
        return Some("HFS".into());
    }
    if has(32..36, b"NXSB") {
        return Some("APFS".into());
    }
    None
}

unsafe fn read_exact(handle: windows::Win32::Foundation::HANDLE, offset: u64, buffer: &mut [u8]) -> Option<()> {
    if SetFilePointerEx(handle, offset as i64, None, FILE_BEGIN).is_err() {
        return None;
    }
    let mut read: u32 = 0;
    if ReadFile(handle, Some(buffer), Some(&mut read), None).is_err() {
        return None;
    }
    if read as usize != buffer.len() {
        return None;
    }
    Some(())
}

unsafe fn read_upto(handle: windows::Win32::Foundation::HANDLE, offset: u64, buffer: &mut [u8]) -> usize {
    if SetFilePointerEx(handle, offset as i64, None, FILE_BEGIN).is_err() {
        return 0;
    }
    let mut read: u32 = 0;
    if ReadFile(handle, Some(buffer), Some(&mut read), None).is_err() {
        return 0;
    }
    read as usize
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn entropy(bytes: &[u8]) -> f32 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut histogram = [0u64; 256];
    for byte in bytes {
        histogram[*byte as usize] += 1;
    }
    let total = bytes.len() as f64;
    let mut value = 0.0f64;
    for count in histogram {
        if count == 0 {
            continue;
        }
        let probability = count as f64 / total;
        value -= probability * probability.log2();
    }
    value as f32
}
