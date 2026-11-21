//! 四个核心 trait。
//!
//! 方法名按语义命名（打开独占 / 定位写入 / 落盘 / 重读分区表 / 是否占用），
//! 不出现系统调用式的名字。

use super::device::{DeviceCandidate, DeviceId};
use super::error::Result;
use super::sink::LedgerEntry;

/// 设备枚举。
///
/// `enumerate` 用于界面首次列出候选盘；
/// `resolve` 只重新检查指定的 DeviceId（D3：绝不触碰已排除的盘，D4：写前重新比对身份）。
pub trait DeviceEnumerator: Send + Sync {
    fn enumerate(&self) -> Result<Vec<DeviceCandidate>>;

    fn resolve(&self, ids: &[DeviceId]) -> Result<Vec<DeviceCandidate>>;
}

/// 镜像源：顺序读取、可回绕（校验阶段重放）。
pub trait ImageSource: Send {
    fn name(&self) -> &str;

    /// 已知的镜像字节数（.xz 通过索引解析，不落地临时文件）。
    fn size(&self) -> u64;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;

    fn rewind(&mut self) -> Result<()>;
}

/// 单盘写入后端。
///
/// `open_exclusive` 在 [`BackendFactory`] 上；拿到句柄后：
/// 写入前清零 1 MiB（D6）由状态机统一完成。
pub trait WriteBackend: Send {
    fn capacity_bytes(&self) -> u64;

    /// 卷是否被其他进程占用。
    fn is_busy(&self) -> Result<bool>;

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;

    /// 无缓存回读（D5），用于逐字节校验。
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()>;

    fn flush(&mut self) -> Result<()>;

    /// 写入完成后让系统重读分区表。
    fn reread_partition_table(&mut self) -> Result<()>;

    /// 写入阶段结束，立即关闭可写句柄（D7）。
    fn close_for_write(&mut self) -> Result<()>;
}

/// 后端工厂：打开独占句柄。
pub trait BackendFactory: Send + Sync {
    fn open_exclusive(&self, device: &DeviceId) -> Result<Box<dyn WriteBackend>>;
}

/// 输出（台账）。
pub trait OutputSink: Send {
    fn emit(&mut self, entry: &LedgerEntry) -> Result<()>;

    fn flush(&mut self) -> Result<()>;
}
