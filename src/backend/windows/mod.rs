//! Windows 后端。
//!
//! - 枚举：SetupAPI 设备接口 + 存储属性 + 几何信息（不用 GET_LENGTH_INFO）
//! - 权限：卷挂载 / 锁卷 / 读写原始磁盘
//! - 写入：锁卷 → 卸载卷 → 写物理盘 → 刷新 → 关可写句柄
//! - 校验：FILE_FLAG_NO_BUFFERING 回读
//!
//! 平台细节全部封装在本模块，core 不感知。

mod dialog;
mod enumerate;
mod writer;

pub use dialog::{pick_image_file, pick_save_file};
pub use enumerate::WindowsDeviceEnumerator;
pub use writer::WindowsBackendFactory;

/// 弹出可移动介质（设置里「完成后弹出设备」）。
/// 先尝试锁卷，再发 EJECT_MEDIA；锁卷失败也继续尝试。
pub fn eject_drive(letter: char) -> bool {
    const IOCTL_STORAGE_EJECT_MEDIA: u32 = 0x002D_4808;
    let path = format!(r"\\.\{}:", letter);
    let Ok(handle) = open_path(&path, GENERIC_READ | GENERIC_WRITE, 0) else {
        return false;
    };
    let mut owned = Some(handle);
    unsafe {
        let _ = ioctl(handle, FSCTL_LOCK_VOLUME, None, None);
        let result = ioctl(handle, IOCTL_STORAGE_EJECT_MEDIA, None, None);
        let _ = ioctl(handle, FSCTL_UNLOCK_VOLUME, None, None);
        close_handle(&mut owned);
        result.is_ok()
    }
}

/// 用资源管理器打开目录 / 文件 / 驱动器（ShellExecute，避免直接 spawn explorer.exe）。
pub fn open_path_external(path: &std::path::Path) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file = wide(&path.to_string_lossy());
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR::null(),
            PCWSTR::from_raw(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    result.0 as usize > 32
}

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, GetDiskFreeSpaceExW, GetVolumeInformationW,
    GetVolumePathNamesForVolumeNameW, SetVolumeMountPointW, FILE_ATTRIBUTE_NORMAL,
    FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
};
use windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, StorageDeviceProperty,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{GUID, PCWSTR};

use crate::core::error::BurnError;

pub(crate) const GENERIC_READ: u32 = 0x8000_0000;
pub(crate) const GENERIC_WRITE: u32 = 0x4000_0000;
pub(crate) const FILE_SHARE_ALL: u32 = 0x0000_0003;
pub(crate) const OPEN_EXISTING: u32 = 3;
pub(crate) const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
pub(crate) const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
pub(crate) const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
pub(crate) const FSCTL_UNLOCK_VOLUME: u32 = 0x0009_001C;
pub(crate) const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
pub(crate) const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
pub(crate) const SYSTEM_BUS_USB: i32 = 7;
pub(crate) const SYSTEM_BUS_SD: i32 = 12;
pub(crate) const SYSTEM_BUS_MMC: i32 = 13;

/// 设备目录：DeviceId → 内部定位信息。UI / 台账永远看不到物理盘号。
pub(crate) struct DeviceCatalog {
    devices: Mutex<std::collections::HashMap<String, CachedDevice>>,
    temporary_mounts: Mutex<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct CachedDevice {
    pub interface_path: String,
    pub physical_drive: u32,
    pub capacity_bytes: u64,
    pub volume_names: Vec<String>,
}

impl DeviceCatalog {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(std::collections::HashMap::new()),
            temporary_mounts: Mutex::new(Vec::new()),
        }
    }

    pub fn replace(&self, entries: Vec<(String, CachedDevice)>) {
        match self.devices.lock() {
            Ok(mut guard) => {
                guard.clear();
                for (key, value) in entries {
                    guard.insert(key, value);
                }
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.clear();
                for (key, value) in entries {
                    guard.insert(key, value);
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<CachedDevice> {
        match self.devices.lock() {
            Ok(guard) => guard.get(id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(id).cloned(),
        }
    }

    pub fn update(&self, id: &str, entry: CachedDevice) {
        match self.devices.lock() {
            Ok(mut guard) => {
                guard.insert(id.to_string(), entry);
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(id.to_string(), entry);
            }
        }
    }

    fn add_temporary_mount(&self, mount_point: String) {
        match self.temporary_mounts.lock() {
            Ok(mut guard) => guard.push(mount_point),
            Err(poisoned) => poisoned.into_inner().push(mount_point),
        }
    }

    /// 移除程序临时挂上的盘符（B5 的“临时”语义）。
    pub fn release_temporary_mounts(&self) {
        let mounts = match self.temporary_mounts.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        for mount in mounts {
            let _ = remove_mount_point(&mount);
        }
    }
}

impl Drop for DeviceCatalog {
    fn drop(&mut self) {
        self.release_temporary_mounts();
    }
}

fn remove_mount_point(mount: &str) -> bool {
    use windows::Win32::Storage::FileSystem::DeleteVolumeMountPointW;
    let wide_mount = wide(mount);
    unsafe { DeleteVolumeMountPointW(PCWSTR::from_raw(wide_mount.as_ptr())).is_ok() }
}

/// Windows 平台入口：共享同一份设备目录。
pub struct WindowsPlatform {
    catalog: Arc<DeviceCatalog>,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self {
            catalog: Arc::new(DeviceCatalog::new()),
        }
    }

    pub fn enumerator(&self) -> WindowsDeviceEnumerator {
        WindowsDeviceEnumerator::from_catalog(Arc::clone(&self.catalog))
    }

    pub fn factory(&self) -> WindowsBackendFactory {
        WindowsBackendFactory::from_catalog(Arc::clone(&self.catalog))
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读取以 NUL 结尾的宽字符串。
pub(crate) unsafe fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 4096 && *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

/// 在固定长度缓冲区内读取宽字符串。
pub(crate) unsafe fn wide_to_string_bounded(ptr: *const u16, max_units: usize) -> String {
    if ptr.is_null() || max_units == 0 {
        return String::new();
    }
    let mut len = 0usize;
    while len < max_units && *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

pub(crate) fn trim_nul(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|value| *value == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len]).trim().to_string()
}

pub(crate) fn open_path(path: &str, access: u32, flags: u32) -> std::result::Result<HANDLE, BurnError> {
    let wide_path = wide(path);
    unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide_path.as_ptr()),
            access,
            FILE_SHARE_MODE(FILE_SHARE_ALL),
            None,
            FILE_CREATION_DISPOSITION(OPEN_EXISTING),
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0 | flags),
            HANDLE::default(),
        )
    }
    .map_err(|error| BurnError::message(format!("打开设备失败: {error}")))
}

pub(crate) unsafe fn close_handle(handle: &mut Option<HANDLE>) {
    if let Some(value) = handle.take() {
        let _ = CloseHandle(value);
    }
}

pub(crate) fn win_error(context: &str, error: windows::core::Error) -> BurnError {
    BurnError::message(format!("{context}: {error}"))
}

pub(crate) unsafe fn ioctl(
    handle: HANDLE,
    code: u32,
    input: Option<&[u8]>,
    output: Option<&mut [u8]>,
) -> std::result::Result<u32, BurnError> {
    let mut returned: u32 = 0;
    let input_ptr = input.map(|bytes| bytes.as_ptr() as *const c_void);
    let (output_ptr, output_len) = match output {
        Some(bytes) => (Some(bytes.as_mut_ptr() as *mut c_void), bytes.len()),
        None => (None, 0),
    };
    DeviceIoControl(
        handle,
        code,
        input_ptr,
        input.map(|bytes| bytes.len() as u32).unwrap_or(0),
        output_ptr,
        output_len as u32,
        Some(&mut returned),
        None,
    )
    .map_err(|error| win_error("设备控制调用失败", error))?;
    Ok(returned)
}

/// 查询物理盘号与容量。
pub(crate) unsafe fn query_device_number(handle: HANDLE) -> std::result::Result<u32, BurnError> {
    let mut output = [0u8; 12];
    let returned = ioctl(handle, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, Some(&mut output))?;
    if returned < 12 {
        return Err(BurnError::message("查询设备号返回长度不足"));
    }
    Ok(u32::from_ne_bytes([output[4], output[5], output[6], output[7]]))
}

/// 查询容量（IOCTL_DISK_GET_DRIVE_GEOMETRY_EX，参考实现已验证）。
pub(crate) unsafe fn query_geometry(handle: HANDLE) -> std::result::Result<(u64, u32), BurnError> {
    let mut output = vec![0u64; 64];
    let bytes = std::slice::from_raw_parts_mut(
        output.as_mut_ptr() as *mut u8,
        output.len() * std::mem::size_of::<u64>(),
    );
    let returned = ioctl(handle, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, None, Some(bytes))?;
    if (returned as usize) < std::mem::size_of::<DISK_GEOMETRY_EX>() {
        return Err(BurnError::message("查询磁盘几何信息返回长度不足"));
    }
    let geometry = &*(output.as_ptr() as *const DISK_GEOMETRY_EX);
    if geometry.DiskSize <= 0 {
        return Err(BurnError::message("查询磁盘容量失败"));
    }
    let sector = geometry.Geometry.BytesPerSector.max(512);
    Ok((geometry.DiskSize as u64, sector))
}

pub(crate) struct StorageDescriptor {
    pub bus_type: i32,
    pub removable: bool,
    pub serial: String,
    /// 厂商名（描述符里的 VendorId 字符串）。
    pub vendor: String,
    /// 型号（描述符里的 ProductId 字符串）。
    pub product: String,
}

/// 查询 STORAGE_DEVICE_DESCRIPTOR（总线类型 / 可移动 / 序列号）。
pub(crate) unsafe fn query_descriptor(handle: HANDLE) -> std::result::Result<StorageDescriptor, BurnError> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let input = std::slice::from_raw_parts(
        &query as *const STORAGE_PROPERTY_QUERY as *const u8,
        std::mem::size_of::<STORAGE_PROPERTY_QUERY>(),
    );
    let mut output = vec![0u32; 1024];
    let bytes = std::slice::from_raw_parts_mut(
        output.as_mut_ptr() as *mut u8,
        output.len() * std::mem::size_of::<u32>(),
    );
    let returned = ioctl(handle, IOCTL_STORAGE_QUERY_PROPERTY, Some(input), Some(bytes))?;
    if returned < 28 {
        return Err(BurnError::message("查询设备属性返回长度不足"));
    }
    let descriptor = &*(output.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR);
    // 描述符里的字符串都以偏移量指向同一块缓冲区，读到 NUL 为止。
    let field = |offset: u32| -> String {
        if offset == 0 || offset >= returned {
            return String::new();
        }
        let pointer =
            (output.as_ptr() as *const u8).add(offset as usize) as *const u16;
        wide_to_string_bounded(pointer, ((returned - offset) / 2) as usize)
            .trim()
            .to_string()
    };
    let serial = field(descriptor.SerialNumberOffset);
    Ok(StorageDescriptor {
        bus_type: descriptor.BusType.0,
        removable: descriptor.RemovableMedia.0 != 0,
        serial,
        vendor: field(descriptor.VendorIdOffset),
        product: field(descriptor.ProductIdOffset),
    })
}

pub(crate) fn is_supported_bus(bus_type: i32) -> bool {
    matches!(bus_type, SYSTEM_BUS_USB | SYSTEM_BUS_SD | SYSTEM_BUS_MMC)
}

/// 系统盘 / 启动盘所在的物理盘号。
pub(crate) unsafe fn system_disk_numbers() -> std::collections::HashSet<u32> {
    let mut result = std::collections::HashSet::new();
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(drive) = std::env::var("SystemDrive") {
        candidates.push(format!(r"\\.\{}", drive));
    }
    if let Ok(root) = std::env::var("SystemRoot") {
        if let Some(letter) = root.chars().next() {
            candidates.push(format!(r"\\.\{}:", letter));
        }
    }
    for path in candidates {
        if let Ok(handle) = open_path(&path, 0, 0) {
            if let Ok((disk, _)) = volume_extent(handle) {
                result.insert(disk);
            }
            let mut owned = Some(handle);
            close_handle(&mut owned);
        }
    }
    result
}

/// 卷所在物理盘号与起始偏移。
pub(crate) unsafe fn volume_extent(handle: HANDLE) -> std::result::Result<(u32, u64), BurnError> {
    let mut output = vec![0u64; 128];
    let bytes = std::slice::from_raw_parts_mut(
        output.as_mut_ptr() as *mut u8,
        output.len() * std::mem::size_of::<u64>(),
    );
    let returned = ioctl(
        handle,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        None,
        Some(bytes),
    )?;
    if returned < 8 || (returned as usize) < std::mem::size_of::<VOLUME_DISK_EXTENTS>() {
        return Err(BurnError::message("查询卷磁盘范围失败"));
    }
    let extents = &*(output.as_ptr() as *const VOLUME_DISK_EXTENTS);
    if extents.NumberOfDiskExtents == 0 {
        return Err(BurnError::message("卷没有磁盘范围"));
    }
    Ok((
        extents.Extents[0].DiskNumber,
        extents.Extents[0].StartingOffset.max(0) as u64,
    ))
}

pub(crate) fn disk_path(number: u32) -> String {
    format!(r"\\.\PhysicalDrive{number}")
}

/// 设备接口 GUID（磁盘类）。
pub(crate) fn disk_interface_guid() -> GUID {
    windows::Win32::System::Ioctl::GUID_DEVINTERFACE_DISK
}

/// 释放 HANDLE 的小工具。
pub(crate) struct OwnedHandle(pub Option<HANDLE>);

impl OwnedHandle {
    pub fn new(handle: HANDLE) -> Self {
        Self(Some(handle))
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { close_handle(&mut self.0) }
    }
}

/// 读取卷信息，供枚举模块使用。
pub(crate) struct RawVolume {
    pub disk_number: u32,
    pub info: crate::core::device::VolumeInfo,
    pub recognized: bool,
}

/// 枚举所有卷。`assign_disks` 为 None 时给所有“有已知文件系统但无盘符”的卷临时挂盘符；
/// 为 Some 时只处理集合内的盘（D3：不碰已排除的盘）。
pub(crate) fn enumerate_volumes(
    catalog: &DeviceCatalog,
    assign_disks: Option<&std::collections::HashSet<u32>>,
    assigned_letters: &mut std::collections::HashSet<char>,
) -> Vec<RawVolume> {
    let mut volumes = Vec::new();
    let mut buffer = vec![0u16; 1024];
    unsafe {
        let find = match FindFirstVolumeW(&mut buffer) {
            Ok(value) => value,
            Err(_) => return volumes,
        };
        let mut find_owned = Some(find);
        loop {
            let name = wide_to_string(buffer.as_ptr());
            if let Some(volume) = read_volume(&name, catalog, assign_disks, assigned_letters) {
                volumes.push(volume);
            }
            if FindNextVolumeW(find, &mut buffer).is_err() {
                break;
            }
        }
        close_handle(&mut find_owned);
    }
    volumes
}

unsafe fn read_volume(
    name: &str,
    catalog: &DeviceCatalog,
    assign_disks: Option<&std::collections::HashSet<u32>>,
    assigned_letters: &mut std::collections::HashSet<char>,
) -> Option<RawVolume> {
    if name.is_empty() {
        return None;
    }
    let handle = match open_path(name, 0, 0) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let mut owned = OwnedHandle::new(handle);
    let extent = volume_extent(handle);
    let mut disk_number = 0u32;
    let mut start_offset = 0u64;
    if let Ok((disk, offset)) = extent {
        disk_number = disk;
        start_offset = offset;
    }
    let _ = &mut owned;

    let mut paths = vec![0u16; 4096];
    let mut needed: u32 = 0;
    let mut letters: Vec<char> = Vec::new();
    if GetVolumePathNamesForVolumeNameW(
        PCWSTR::from_raw(wide(name).as_ptr()),
        Some(&mut paths),
        &mut needed,
    )
    .is_ok()
    {
        let mut index = 0usize;
        while index < paths.len() {
            let start = index;
            while index < paths.len() && paths[index] != 0 {
                index += 1;
            }
            if index == start {
                break;
            }
            let path = String::from_utf16_lossy(&paths[start..index]);
            if let Some(letter) = path_to_letter(&path) {
                if !letters.contains(&letter) {
                    letters.push(letter);
                }
            }
            index += 1;
        }
    }

    let mut label_buffer = [0u16; 256];
    let mut fs_buffer = [0u16; 64];
    let mut recognized = false;
    let mut label = String::new();
    let mut file_system = String::new();
    if GetVolumeInformationW(
        PCWSTR::from_raw(wide(name).as_ptr()),
        Some(&mut label_buffer),
        None,
        None,
        None,
        Some(&mut fs_buffer),
    )
    .is_ok()
    {
        recognized = true;
        label = trim_nul(&label_buffer);
        file_system = trim_nul(&fs_buffer);
    }

    let mut free_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free: u64 = 0;
    let mut total = 0u64;
    let mut used = 0u64;
    if GetDiskFreeSpaceExW(
        PCWSTR::from_raw(wide(name).as_ptr()),
        Some(&mut free_bytes),
        Some(&mut total_bytes),
        Some(&mut total_free),
    )
    .is_ok()
    {
        total = total_bytes;
        used = total_bytes.saturating_sub(total_free);
    }

    let _ = free_bytes;
    let mut letter = letters.first().copied();
    let mut temporary = false;
    let allow_assign = match assign_disks {
        None => true,
        Some(set) => set.contains(&disk_number),
    };
    if recognized && letter.is_none() && allow_assign {
        if let Some((assigned, mount_point)) = assign_letter(name, assigned_letters) {
            letter = Some(assigned);
            temporary = true;
            catalog.add_temporary_mount(mount_point);
        }
    }

    Some(RawVolume {
        disk_number,
        recognized,
        info: crate::core::device::VolumeInfo {
            volume_name: name.to_string(),
            letter,
            label,
            file_system,
            used_bytes: used,
            total_bytes: total,
            start_offset,
            temporary_letter: temporary,
        },
    })
}

fn path_to_letter(path: &str) -> Option<char> {
    let trimmed = path.trim_end_matches('\\');
    let mut chars = trimmed.chars();
    let letter = chars.next()?;
    let colon = chars.next()?;
    if colon == ':' && chars.next().is_none() && letter.is_ascii_alphabetic() {
        Some(letter.to_ascii_uppercase())
    } else {
        None
    }
}

/// 只给 Windows 认识的文件系统挂临时盘符（D1）。
unsafe fn assign_letter(
    volume_name: &str,
    assigned_letters: &mut std::collections::HashSet<char>,
) -> Option<(char, String)> {
    for byte in (b'D'..=b'Z').rev() {
        let letter = byte as char;
        if assigned_letters.contains(&letter) {
            continue;
        }
        let mount = format!("{letter}:\\");
        if std::path::Path::new(&mount).exists() {
            continue;
        }
        let mount_wide = wide(&mount);
        let volume_wide = wide(volume_name);
        if SetVolumeMountPointW(
            PCWSTR::from_raw(mount_wide.as_ptr()),
            PCWSTR::from_raw(volume_wide.as_ptr()),
        )
        .is_ok()
        {
            assigned_letters.insert(letter);
            return Some((letter, mount));
        }
    }
    None
}
