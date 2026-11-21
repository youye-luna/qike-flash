//! 多 U 盘 / SD 卡批量烧录工具内核。
//!
//! M0 范围：
//! - 四个核心 trait：设备枚举、写入后端、镜像源、输出
//! - `FileSink` 台账
//! - `BurnTask` 六态状态机
//! - Windows 后端（SetupAPI 枚举 + 锁卷写入 + 无缓存回读）
//!
//! 平台调用只允许出现在 [`backend`] 内；[`core`] 不依赖 windows / nix crate。

pub mod backend;
pub mod core;

pub use crate::core::burn::{
    BurnControl, BurnOptions, BurnOutcome, BurnPlan, BurnSnapshot, BurnState, BurnTask, DiskOutcome,
    DiskPhase, DiskProgress,
};
pub use crate::core::device::{
    DeviceCandidate, DeviceId, DiskState, PartitionInfo, PartitionTableKind, VolumeInfo,
};
pub use crate::core::error::{BurnError, Result};
pub use crate::core::image::ImageInfo;
pub use crate::core::sink::{FileSink, LedgerEntry, MemorySink, VerifyReport, WriteReport};
pub use crate::core::traits::{
    BackendFactory, DeviceEnumerator, ImageSource, OutputSink, WriteBackend,
};
