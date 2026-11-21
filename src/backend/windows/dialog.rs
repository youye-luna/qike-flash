//! 镜像文件选择对话框（原生 `GetOpenFileNameW`，不引入额外依赖）。

use std::mem::size_of;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
};

use super::wide;

/// 弹出“打开镜像”对话框；用户取消或对话框失败时返回 `None`。
pub fn pick_image_file() -> Option<String> {
    let filter: Vec<u16> = "镜像文件 (*.iso;*.img;*.xz)\0*.iso;*.img;*.xz\0所有文件 (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = wide("选择镜像文件");
    let mut buffer = vec![0u16; 32768];

    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let accepted = unsafe { GetOpenFileNameW(&mut dialog) };
    if !accepted.as_bool() {
        return None;
    }

    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// 弹出“另存为”对话框（导出日志用）；取消或失败返回 `None`。
pub fn pick_save_file(default_name: &str) -> Option<String> {
    let filter: Vec<u16> = "文本文件 (*.txt)\0*.txt\0所有文件 (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = wide("导出日志");
    let mut buffer: Vec<u16> = default_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    buffer.resize(32768, 0);

    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let accepted = unsafe { GetSaveFileNameW(&mut dialog) };
    if !accepted.as_bool() {
        return None;
    }

    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..end]))
}
