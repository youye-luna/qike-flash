//! 设备模型。字段设计参照既有实现，但完全重新实现，不移植代码。

use std::fmt;

use sha2::{Digest, Sha256};

/// 不透明的设备身份。
///
/// 由 `序列号 + 容量 + 端口路径` 派生，只暴露哈希摘要；
/// 界面、日志、台账中都不会出现 `PhysicalDriveN`。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// 派生不透明 ID（32 位十六进制摘要）。
    pub fn derive(serial: &str, capacity_bytes: u64, port_path: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"multi-burn/device-id/v1\0");
        hasher.update(serial.as_bytes());
        hasher.update([0u8]);
        hasher.update(capacity_bytes.to_le_bytes());
        hasher.update([0u8]);
        hasher.update(port_path.as_bytes());

        let digest = hasher.finalize();
        let mut hex = String::with_capacity(32);
        for byte in digest.iter().take(16) {
            hex.push(nibble_to_hex(byte >> 4));
            hex.push(nibble_to_hex(byte & 0x0F));
        }
        Self(hex)
    }

    /// 摘要文本，可直接写入台账。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dev:{}", self.0)
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.0)
    }
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

/// 盘的三态判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// 有可识别的分区 / 文件系统。
    HasContent,
    /// 空盘（无分区表签名、无分区）。
    Empty,
    /// 无法判断（RAW、分区表损坏、文件系统未知）。
    Unknown,
}

impl DiskState {
    pub fn label(self) -> &'static str {
        match self {
            DiskState::HasContent => "有内容",
            DiskState::Empty => "空盘",
            DiskState::Unknown => "无法判断",
        }
    }
}

/// 分区表类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionTableKind {
    Mbr,
    Gpt,
    Unknown,
}

impl PartitionTableKind {
    pub fn label(self) -> &'static str {
        match self {
            PartitionTableKind::Mbr => "MBR",
            PartitionTableKind::Gpt => "GPT",
            PartitionTableKind::Unknown => "未知",
        }
    }
}

/// 分区信息（B6 用：即使文件系统不认识也要能展示）。
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    pub index: u32,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    /// Windows 认识的文件系统名（NTFS / FAT32 ...）；未知时为 None。
    pub file_system: Option<String>,
    /// 自行探测到的文件系统签名（如 `ext-family`、`0xEF53`）。
    pub signature: Option<String>,
    pub drive_letter: Option<char>,
}

/// 卷信息。`volume_name` 是 `\\?\Volume{...}\`，不含物理盘号。
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub volume_name: String,
    pub letter: Option<char>,
    pub label: String,
    pub file_system: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub start_offset: u64,
    /// 是否为程序临时挂载的盘符（B5）。
    pub temporary_letter: bool,
}

/// 一个候选可移动盘。
#[derive(Debug, Clone)]
pub struct DeviceCandidate {
    pub id: DeviceId,
    pub capacity_bytes: u64,
    /// `USB` / `SD` / `MMC`。
    pub bus: String,
    /// 厂商 + 型号（来自设备描述符）；取不到时为空串。
    pub model: String,
    pub removable: bool,
    /// 系统盘 / 启动盘，绝不允许出现在写入目标里。
    pub is_system: bool,
    pub state: DiskState,
    pub table: PartitionTableKind,
    pub partitions: Vec<PartitionInfo>,
    pub volumes: Vec<VolumeInfo>,
    /// 前 1 MiB 的香农熵（0.0 ~ 8.0），用于 B6 展示。
    pub entropy: Option<f32>,
}

impl DeviceCandidate {
    /// 已用容量（所有已知卷的已用字节之和）。
    pub fn used_bytes(&self) -> u64 {
        self.volumes.iter().map(|v| v.used_bytes).sum()
    }

    /// 所有已挂载盘符，如 `E: F:`。
    pub fn letters_display(&self) -> String {
        let mut parts: Vec<String> = self
            .partitions
            .iter()
            .filter_map(|p| p.drive_letter.map(|c| format!("{c}:")))
            .collect();
        for volume in &self.volumes {
            if let Some(letter) = volume.letter {
                let text = format!("{letter}:");
                if !parts.contains(&text) {
                    parts.push(text);
                }
            }
        }
        parts.join(" ")
    }

    /// 主卷标（第一个非空卷标）。
    pub fn primary_label(&self) -> &str {
        self.volumes
            .iter()
            .find(|v| !v.label.is_empty())
            .map(|v| v.label.as_str())
            .unwrap_or("")
    }

    /// 主文件系统名。
    pub fn primary_file_system(&self) -> &str {
        self.volumes
            .iter()
            .find(|v| !v.file_system.is_empty())
            .map(|v| v.file_system.as_str())
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_stable_opaque_id() {
        let a = DeviceId::derive("SN-1", 64_000_000_000, r"\\?\usbstor#disk#1");
        let b = DeviceId::derive("SN-1", 64_000_000_000, r"\\?\usbstor#disk#1");
        let c = DeviceId::derive("SN-2", 64_000_000_000, r"\\?\usbstor#disk#1");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.as_str().len(), 32);
        assert!(a.as_str().chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn device_id_never_exposes_physical_drive_name() {
        let id = DeviceId::derive("SN", 1, r"\\.\PhysicalDrive3");
        let text = id.to_string();
        assert!(!text.contains("PhysicalDrive"));
        assert!(!format!("{id:?}").contains("PhysicalDrive"));
    }
}
