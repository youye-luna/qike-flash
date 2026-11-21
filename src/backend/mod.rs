//! 平台后端。core 只依赖 trait，平台调用全部隔离在这里。

#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
pub mod linux;
