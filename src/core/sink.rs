//! 台账输出。`FileSink` 追加写入 JSON Lines，便于事后 audit。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::Serialize;

use super::error::{BurnError, Result};
use super::traits::OutputSink;

/// 单盘写入结果。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WriteReport {
    NotStarted,
    Written { bytes: u64 },
    /// 中途取消或未写完。
    Partial { bytes: u64, reason: String },
    Failed { reason: String },
}

/// 单盘校验结果。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VerifyReport {
    NotRun,
    Passed,
    Failed { offset: u64, reason: String },
    Cancelled { offset: u64 },
}

/// 台账条目。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerEntry {
    Batch {
        at: String,
        image_name: String,
        /// 镜像完整路径；旧台账没有该字段时为空串。
        image_path: String,
        image_sha256: String,
        image_size: u64,
        targets: Vec<String>,
        /// D10：排除项必须写进台账。
        excluded: Vec<String>,
    },
    Disk {
        at: String,
        image_name: String,
        image_sha256: String,
        device_id: String,
        /// 厂商 + 型号；取不到时为空串。
        device_model: String,
        /// 写入前已挂载的盘符（如 `F: G:`）；空串表示无盘符。
        letters: String,
        capacity_bytes: u64,
        write: WriteReport,
        verify: VerifyReport,
    },
}

/// 追加写入 JSONL 的台账。
pub struct FileSink {
    writer: BufWriter<File>,
}

impl FileSink {
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    BurnError::context(format!("创建台账目录 {} 失败", parent.display()), error)
                })?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                BurnError::context(format!("打开台账 {} 失败", path.display()), error)
            })?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }
}

impl OutputSink for FileSink {
    fn emit(&mut self, entry: &LedgerEntry) -> Result<()> {
        serde_json::to_writer(&mut self.writer, entry)
            .map_err(|error| BurnError::message(format!("台账序列化失败: {error}")))?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| BurnError::context("写入台账失败", error))
    }

    fn flush(&mut self) -> Result<()> {
        self.writer
            .flush()
            .map_err(|error| BurnError::context("刷新台账失败", error))
    }
}

/// 内存台账，便于单元测试与 CLI 预览。
#[derive(Default)]
pub struct MemorySink {
    pub entries: Vec<LedgerEntry>,
}

impl OutputSink for MemorySink {
    fn emit(&mut self, entry: &LedgerEntry) -> Result<()> {
        self.entries.push(entry.clone());
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
