//! 统一错误类型。运行期禁止 unwrap / expect，错误一律通过 Result 传播。

use std::fmt;

use super::device::DeviceId;

pub type Result<T> = std::result::Result<T, BurnError>;

#[derive(Debug)]
pub enum BurnError {
    /// 底层 I/O 错误。
    Io(std::io::Error),
    /// 带上下文的业务错误。
    Message(String),
    /// 卷被占用（C9）。
    Busy { device: DeviceId, detail: String },
    /// 容量不足（C10）。
    Capacity {
        device: DeviceId,
        required: u64,
        available: u64,
    },
    /// 重新枚举时设备身份发生变化（D4）。
    IdentityChanged { device: DeviceId },
    /// 已取消。
    Cancelled,
}

impl BurnError {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    pub fn context(context: impl fmt::Display, source: std::io::Error) -> Self {
        Self::Message(format!("{context}: {source}"))
    }
}

impl fmt::Display for BurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BurnError::Io(error) => write!(f, "I/O 错误: {error}"),
            BurnError::Message(text) => f.write_str(text),
            BurnError::Busy { device, detail } => {
                write!(f, "设备 {device} 被占用，请关闭正在使用它的窗口：{detail}")
            }
            BurnError::Capacity {
                device,
                required,
                available,
            } => write!(
                f,
                "设备 {device} 容量不足：需要 {required} 字节，只有 {available} 字节"
            ),
            BurnError::IdentityChanged { device } => {
                write!(f, "设备 {device} 的身份在确认后发生变化，已中止整批写入")
            }
            BurnError::Cancelled => f.write_str("已取消"),
        }
    }
}

impl std::error::Error for BurnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BurnError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BurnError {
    fn from(value: std::io::Error) -> Self {
        BurnError::Io(value)
    }
}
