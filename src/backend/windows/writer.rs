//! 物理盘写入后端：锁卷 → 写 → 刷新 → 关句柄 → 无缓存回读。

use std::alloc::{alloc, dealloc, Layout};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, SetFilePointerEx, WriteFile, FILE_BEGIN,
};

use super::{
    CachedDevice, DeviceCatalog, close_handle, disk_path, open_path, query_device_number,
    query_geometry, win_error, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME,
    GENERIC_READ, GENERIC_WRITE, IOCTL_DISK_UPDATE_PROPERTIES, FILE_FLAG_NO_BUFFERING,
};
use crate::core::device::DeviceId;
use crate::core::error::{BurnError, Result};
use crate::core::traits::{BackendFactory, WriteBackend};

pub struct WindowsBackendFactory {
    catalog: Arc<DeviceCatalog>,
}

impl WindowsBackendFactory {
    pub(crate) fn from_catalog(catalog: Arc<DeviceCatalog>) -> Self {
        Self { catalog }
    }
}

impl BackendFactory for WindowsBackendFactory {
    fn open_exclusive(&self, device: &DeviceId) -> Result<Box<dyn WriteBackend>> {
        let cached = self.catalog.get(device.as_str()).ok_or_else(|| {
            BurnError::IdentityChanged {
                device: device.clone(),
            }
        })?;
        let writer = WindowsWriter::open(device, cached)?;
        Ok(Box::new(writer))
    }
}

struct WindowsWriter {
    physical_drive: u32,
    capacity: u64,
    sector: u32,
    write_handle: Option<HANDLE>,
    read_handle: Option<HANDLE>,
    volume_handles: Vec<HANDLE>,
    volume_names: Vec<String>,
}

// HANDLE 由本结构独占；移动到写入 / 校验线程是安全的。
unsafe impl Send for WindowsWriter {}

impl WindowsWriter {
    fn open(device: &DeviceId, cached: CachedDevice) -> Result<Self> {
        let handle = open_path(
            &disk_path(cached.physical_drive),
            GENERIC_READ | GENERIC_WRITE,
            0,
        )?;
        let mut writer = Self {
            physical_drive: cached.physical_drive,
            capacity: cached.capacity_bytes,
            sector: 512,
            write_handle: Some(handle),
            read_handle: None,
            volume_handles: Vec::new(),
            volume_names: cached.volume_names.clone(),
        };

        let number = unsafe { query_device_number(handle) };
        match number {
            Ok(value) if value == cached.physical_drive => {}
            _ => {
                writer.close_write_handle();
                return Err(BurnError::IdentityChanged {
                    device: device.clone(),
                });
            }
        }
        let geometry = unsafe { query_geometry(handle) };
        match geometry {
            Ok((capacity, sector)) if capacity == cached.capacity_bytes && capacity > 0 => {
                writer.capacity = capacity;
                writer.sector = sector.max(512);
            }
            _ => {
                writer.close_write_handle();
                return Err(BurnError::IdentityChanged {
                    device: device.clone(),
                });
            }
        }

        for name in &cached.volume_names {
            let volume = match open_path(name, GENERIC_READ | GENERIC_WRITE, 0)
                .or_else(|_| open_path(name, GENERIC_READ, 0))
            {
                Ok(value) => value,
                Err(_) => {
                    writer.release_locks();
                    return Err(BurnError::Busy {
                        device: device.clone(),
                        detail: format!("卷 {name} 无法打开，可能被其他程序占用"),
                    });
                }
            };

            let mut locked = false;
            for _ in 0..10 {
                if unsafe { super::ioctl(volume, FSCTL_LOCK_VOLUME, None, None) }.is_ok() {
                    locked = true;
                    break;
                }
                sleep(Duration::from_millis(100));
            }
            if !locked {
                unsafe {
                    let _ = CloseHandle(volume);
                }
                writer.release_locks();
                return Err(BurnError::Busy {
                    device: device.clone(),
                    detail: "卷被占用，请关闭资源管理器等正在使用该盘的窗口".to_string(),
                });
            }
            unsafe {
                // 卸载失败不致命，锁住后仍然可以直接写物理盘。
                let _ = super::ioctl(volume, FSCTL_DISMOUNT_VOLUME, None, None);
            }
            writer.volume_handles.push(volume);
        }

        Ok(writer)
    }

    fn close_write_handle(&mut self) {
        unsafe {
            close_handle(&mut self.write_handle);
        }
    }

    fn release_locks(&mut self) {
        unsafe {
            for volume in self.volume_handles.drain(..) {
                let _ = super::ioctl(volume, FSCTL_UNLOCK_VOLUME, None, None);
                let _ = CloseHandle(volume);
            }
            close_handle(&mut self.write_handle);
        }
    }

    fn read_handle(&mut self) -> Result<HANDLE> {
        if let Some(handle) = self.read_handle {
            return Ok(handle);
        }
        let handle = open_path(
            &disk_path(self.physical_drive),
            GENERIC_READ,
            FILE_FLAG_NO_BUFFERING,
        )?;
        self.read_handle = Some(handle);
        Ok(handle)
    }
}

impl Drop for WindowsWriter {
    fn drop(&mut self) {
        unsafe {
            close_handle(&mut self.read_handle);
            self.release_locks();
        }
    }
}

impl WriteBackend for WindowsWriter {
    fn capacity_bytes(&self) -> u64 {
        self.capacity
    }

    fn is_busy(&self) -> Result<bool> {
        if self.write_handle.is_some() {
            return Ok(false);
        }
        for name in &self.volume_names {
            let volume = match open_path(name, GENERIC_READ | GENERIC_WRITE, 0)
                .or_else(|_| open_path(name, GENERIC_READ, 0))
            {
                Ok(value) => value,
                Err(_) => return Ok(true),
            };
            let locked = unsafe { super::ioctl(volume, FSCTL_LOCK_VOLUME, None, None) }.is_ok();
            unsafe {
                if locked {
                    let _ = super::ioctl(volume, FSCTL_UNLOCK_VOLUME, None, None);
                }
                let _ = CloseHandle(volume);
            }
            if !locked {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let handle = self.write_handle.ok_or_else(|| {
            BurnError::message("可写句柄已关闭，拒绝继续写入")
        })?;
        unsafe {
            SetFilePointerEx(handle, offset as i64, None, FILE_BEGIN)
                .map_err(|error| win_error("定位写入偏移失败", error))?;
        }
        let mut written_total = 0usize;
        while written_total < data.len() {
            let mut written: u32 = 0;
            let result = unsafe {
                WriteFile(
                    handle,
                    Some(&data[written_total..]),
                    Some(&mut written),
                    None,
                )
            };
            if let Err(error) = result {
                return Err(win_error("写入设备失败", error));
            }
            if written == 0 {
                return Err(BurnError::message("写入设备返回 0 字节"));
            }
            written_total += written as usize;
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        if buffer.is_empty() {
            return Ok(());
        }
        let sector = self.sector.max(512) as u64;
        if offset % sector != 0 {
            return Err(BurnError::message(format!(
                "回读偏移 {offset} 未按扇区对齐（{sector} 字节）"
            )));
        }
        let available = self.capacity.saturating_sub(offset);
        if (buffer.len() as u64) > available {
            return Err(BurnError::message("回读范围超出设备容量"));
        }
        let rounded = (buffer.len() as u64 + sector - 1) / sector * sector;
        let read_len = rounded.min(available) as usize;
        if read_len < buffer.len() {
            return Err(BurnError::message("回读长度不足"));
        }

        let mut aligned = AlignedBuffer::new(read_len.max(sector as usize), 4096)?;
        let handle = self.read_handle()?;
        unsafe {
            SetFilePointerEx(handle, offset as i64, None, FILE_BEGIN)
                .map_err(|error| win_error("定位回读偏移失败", error))?;
        }
        let mut read: u32 = 0;
        let result = unsafe {
            ReadFile(
                handle,
                Some(aligned.as_mut_slice()),
                Some(&mut read),
                None,
            )
        };
        if let Err(error) = result {
            return Err(win_error("回读设备失败", error));
        }
        if (read as usize) < buffer.len() {
            return Err(BurnError::message(format!(
                "回读字节数不足：期望 {}，实际 {read}",
                buffer.len()
            )));
        }
        buffer.copy_from_slice(&aligned.as_slice()[..buffer.len()]);
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        let handle = self
            .write_handle
            .ok_or_else(|| BurnError::message("可写句柄已关闭，无法刷新"))?;
        unsafe { FlushFileBuffers(handle) }.map_err(|error| win_error("刷新设备缓冲失败", error))
    }

    fn reread_partition_table(&mut self) -> Result<()> {
        let handle = self
            .write_handle
            .ok_or_else(|| BurnError::message("可写句柄已关闭，无法重读分区表"))?;
        unsafe { super::ioctl(handle, IOCTL_DISK_UPDATE_PROPERTIES, None, None) }?;
        Ok(())
    }

    fn close_for_write(&mut self) -> Result<()> {
        self.release_locks();
        Ok(())
    }
}

/// 4096 字节对齐缓冲区，满足 FILE_FLAG_NO_BUFFERING 的对齐要求。
struct AlignedBuffer {
    pointer: *mut u8,
    layout: Layout,
    length: usize,
}

impl AlignedBuffer {
    fn new(length: usize, alignment: usize) -> Result<Self> {
        let layout = Layout::from_size_align(length.max(1), alignment)
            .map_err(|error| BurnError::message(format!("分配对齐缓冲区失败: {error}")))?;
        let pointer = unsafe { alloc(layout) };
        if pointer.is_null() {
            return Err(BurnError::message("分配对齐缓冲区失败"));
        }
        Ok(Self {
            pointer,
            layout,
            length,
        })
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.pointer, self.length) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.pointer, self.length) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { dealloc(self.pointer, self.layout) }
    }
}
