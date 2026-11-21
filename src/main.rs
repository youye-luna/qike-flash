//! 程序入口：默认启动图形界面；带参数时走无人值守的命令行。
//!
//! 写入流程由库内 `BurnTask` 提供，界面只发命令、只读状态快照。
//!
//! GUI 子系统编译：双击启动不再附带黑色控制台窗口；
//! 命令行模式（--list / --help）运行时重新挂回父终端，输出照常可见。

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

mod i18n;
mod icons;
mod ui;

use multi_burn::backend::windows::WindowsPlatform;
use multi_burn::{DeviceCandidate, DeviceEnumerator};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 命令行模式：挂回启动它的终端，让 stdout 有去处。
#[cfg(windows)]
fn attach_console_for_cli() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// GUI 启动失败时用消息框提示（此时没有控制台可写）。
#[cfg(windows)]
fn show_error(text: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let to_wide = |value: &str| -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let body = to_wide(text);
    let title = to_wide("齐刻 Qike Flash");
    unsafe {
        let _ = MessageBoxW(
            HWND(std::ptr::null_mut()),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if !args.is_empty() {
        attach_console_for_cli();
    }
    match args.first().map(String::as_str) {
        None => match ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let text = format!("界面启动失败：{error}");
                #[cfg(windows)]
                show_error(&text);
                #[cfg(not(windows))]
                eprintln!("{text}");
                ExitCode::FAILURE
            }
        },
        Some("--list") | Some("-l") => match list() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("错误：{error}");
                ExitCode::FAILURE
            }
        },
        Some("--help") | Some("-h") => {
            usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("未知参数：{other}\n");
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!("齐刻 Qike Flash {VERSION} — 多 U 盘 / SD 卡批量烧录");
    println!();
    println!("用法：");
    println!("  multi-burn            启动图形界面");
    println!("  multi-burn --list     列出可写入的 USB / SD 卡候选");
    println!("  multi-burn --help     显示本帮助");
}

fn list() -> multi_burn::Result<()> {
    let platform = WindowsPlatform::new();
    let enumerator = platform.enumerator();
    let devices = enumerator.enumerate()?;

    if devices.is_empty() {
        println!("未发现可写入的 USB / SD 卡。");
        return Ok(());
    }

    println!("共 {} 个候选设备：", devices.len());
    for (index, device) in devices.iter().enumerate() {
        println!("{}", describe(index + 1, device));
    }
    Ok(())
}

fn describe(index: usize, device: &DeviceCandidate) -> String {
    let mut line = format!(
        "[{index}] {}  {}  {}  {}",
        device.id,
        device.bus,
        format_bytes(device.capacity_bytes),
        device.state.label(),
    );

    let letters = device.letters_display();
    if !letters.is_empty() {
        line.push_str(&format!("  盘符 {letters}"));
    }
    let label = device.primary_label();
    if !label.is_empty() {
        line.push_str(&format!("  卷标 {label}"));
    }
    let file_system = device.primary_file_system();
    if !file_system.is_empty() {
        line.push_str(&format!("  {file_system}"));
    }
    if device.state != multi_burn::DiskState::Empty {
        line.push_str(&format!("  分区表 {}", device.table.label()));
    }
    if let Some(entropy) = device.entropy {
        if device.state == multi_burn::DiskState::Unknown {
            line.push_str(&format!("  熵 {entropy:.2}"));
        }
    }
    line
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
