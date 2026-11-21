//! 三语界面文案：简体中文 / 繁體中文 / English。
//!
//! `Lang` 持久化在 settings.json；`texts()` 返回该语言的静态文案表，
//! 带插值的文案以 `Lang` 的方法给出（方法内用 `format!` 填充）。

use serde::{Deserialize, Serialize};

use multi_burn::{BurnState, DiskPhase, DiskState, PartitionTableKind, VerifyReport, WriteReport};

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "zh-cn")]
    ZhCn,
    #[serde(rename = "zh-tw")]
    ZhTw,
    #[serde(rename = "en")]
    En,
}

impl Default for Lang {
    fn default() -> Self {
        Self::ZhCn
    }
}

impl Lang {
    /// 该语言的静态文案表。
    pub fn texts(self) -> &'static Texts {
        match self {
            Self::ZhCn => &ZH_CN,
            Self::ZhTw => &ZH_TW,
            Self::En => &EN,
        }
    }

    // ------------------------------------------------------------ 带插值的文案

    /// 底部徽章：参与 / 总数。
    pub fn of_badge(self, participating: usize, total: usize) -> String {
        match self {
            Self::ZhCn => format!("参与 {participating} / 共 {total} 块"),
            Self::ZhTw => format!("參與 {participating} / 共 {total} 塊"),
            Self::En => format!("In batch: {participating} / {total}"),
        }
    }

    /// 设备卡：已用 / 总容量。
    pub fn used_of(self, used: &str, total: &str) -> String {
        match self {
            Self::ZhCn => format!("已用 {used} / 总 {total}"),
            Self::ZhTw => format!("已用 {used} / 總 {total}"),
            Self::En => format!("Used {used} / Total {total}"),
        }
    }

    /// 设备卡：无法判断时的分区表详情。
    pub fn detail_line(
        self,
        table: &str,
        partitions: usize,
        signature: &str,
        entropy: f64,
    ) -> String {
        match self {
            Self::ZhCn => format!(
                "分区表 {table} · 分区 {partitions} 个 · 文件系统签名 {signature} · 熵 {entropy:.2}"
            ),
            Self::ZhTw => format!(
                "分割表 {table} · 分割區 {partitions} 個 · 檔案系統簽章 {signature} · 熵 {entropy:.2}"
            ),
            Self::En => format!(
                "Table {table} · {partitions} partitions · FS signature {signature} · entropy {entropy:.2}"
            ),
        }
    }

    /// 进度视图：状态 + 完成块数。
    pub fn state_line(self, state: &str, done: usize, total: usize) -> String {
        match self {
            Self::ZhCn => format!("{state} · 完成 {done}/{total} 块"),
            Self::ZhTw => format!("{state} · 完成 {done}/{total} 塊"),
            Self::En => format!("{state} · {done}/{total} done"),
        }
    }

    /// 进度视图：失败块数。
    pub fn failed_n(self, failed: usize) -> String {
        match self {
            Self::ZhCn => format!("失败 {failed} 块"),
            Self::ZhTw => format!("失敗 {failed} 塊"),
            Self::En => format!("{failed} failed"),
        }
    }

    /// 总进度：已写入 / 总量。
    pub fn written_of(self, written: &str, total: &str) -> String {
        match self {
            Self::ZhCn => format!("已写入 {written} / {total}"),
            Self::ZhTw => format!("已寫入 {written} / {total}"),
            Self::En => format!("Written {written} / {total}"),
        }
    }

    /// 日志页标题右侧的条数。
    pub fn n_lines(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("{count} 条"),
            Self::ZhTw => format!("{count} 條"),
            Self::En => format!("{count} lines"),
        }
    }

    /// 历史页标题右侧的批次数。
    pub fn n_writes(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("{count} 次写入"),
            Self::ZhTw => format!("{count} 次寫入"),
            Self::En => format!("{count} batches"),
        }
    }

    /// 历史页：跳过的盘数。
    pub fn skipped_n(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("跳过 {count} 块"),
            Self::ZhTw => format!("跳過 {count} 塊"),
            Self::En => format!("Skipped {count}"),
        }
    }

    /// 历史页：批次用时。
    pub fn elapsed(self, duration: &str) -> String {
        match self {
            Self::ZhCn => format!("用时 {duration}"),
            Self::ZhTw => format!("用時 {duration}"),
            Self::En => format!("Elapsed {duration}"),
        }
    }

    /// 设置页：日志保留天数选项。
    pub fn days(self, count: u32) -> String {
        match self {
            Self::ZhCn => format!("{count} 天"),
            Self::ZhTw => format!("{count} 天"),
            Self::En => format!("{count} d"),
        }
    }

    /// 关于页：版本行。
    pub fn version_line(self, version: &str) -> String {
        match self {
            Self::ZhCn => format!("版本 {version} · 多 U 盘 / SD 卡批量烧录"),
            Self::ZhTw => format!("版本 {version} · 多 U 碟 / SD 卡批次燒錄"),
            Self::En => format!("Version {version} · Multi USB / SD card flasher"),
        }
    }

    /// 关于页：复制版本信息的剪贴板内容。
    pub fn issue_copy_text(self, version: &str) -> String {
        match self {
            Self::ZhCn => format!(
                "齐刻 Qike Flash {version}\n作者 youye-luna\n许可证 Apache-2.0\nhttps://github.com/youye-luna/qike-flash"
            ),
            Self::ZhTw => format!(
                "齊刻 Qike Flash {version}\n作者 youye-luna\n授權條款 Apache-2.0\nhttps://github.com/youye-luna/qike-flash"
            ),
            Self::En => format!(
                "Qike Flash {version}\nAuthor youye-luna\nLicense Apache-2.0\nhttps://github.com/youye-luna/qike-flash"
            ),
        }
    }

    // ------------------------------------------------------------ 状态翻译

    /// 批量写入状态机文案。
    pub fn burn_state(self, state: BurnState) -> &'static str {
        match self {
            Self::ZhCn => match state {
                BurnState::Idle => "空闲",
                BurnState::Preparing => "准备中",
                BurnState::Writing => "写入中",
                BurnState::Verifying => "校验中",
                BurnState::Completed => "已完成",
                BurnState::Aborted => "已中止",
            },
            Self::ZhTw => match state {
                BurnState::Idle => "閒置",
                BurnState::Preparing => "準備中",
                BurnState::Writing => "寫入中",
                BurnState::Verifying => "校驗中",
                BurnState::Completed => "已完成",
                BurnState::Aborted => "已中止",
            },
            Self::En => match state {
                BurnState::Idle => "Idle",
                BurnState::Preparing => "Preparing",
                BurnState::Writing => "Writing",
                BurnState::Verifying => "Verifying",
                BurnState::Completed => "Completed",
                BurnState::Aborted => "Aborted",
            },
        }
    }

    /// 逐盘阶段文案。
    pub fn disk_phase(self, phase: DiskPhase) -> &'static str {
        match self {
            Self::ZhCn => match phase {
                DiskPhase::Waiting => "等待中",
                DiskPhase::Writing => "写入中",
                DiskPhase::Verifying => "校验中",
                DiskPhase::Done => "已完成",
                DiskPhase::Failed => "失败",
            },
            Self::ZhTw => match phase {
                DiskPhase::Waiting => "等待中",
                DiskPhase::Writing => "寫入中",
                DiskPhase::Verifying => "校驗中",
                DiskPhase::Done => "已完成",
                DiskPhase::Failed => "失敗",
            },
            Self::En => match phase {
                DiskPhase::Waiting => "Waiting",
                DiskPhase::Writing => "Writing",
                DiskPhase::Verifying => "Verifying",
                DiskPhase::Done => "Done",
                DiskPhase::Failed => "Failed",
            },
        }
    }

    /// 盘状态文案。
    pub fn disk_state(self, state: DiskState) -> &'static str {
        match self {
            Self::ZhCn => match state {
                DiskState::HasContent => "有内容",
                DiskState::Empty => "空盘",
                DiskState::Unknown => "无法判断",
            },
            Self::ZhTw => match state {
                DiskState::HasContent => "有內容",
                DiskState::Empty => "空碟",
                DiskState::Unknown => "無法判斷",
            },
            Self::En => match state {
                DiskState::HasContent => "Has data",
                DiskState::Empty => "Empty",
                DiskState::Unknown => "Unknown",
            },
        }
    }

    /// 分区表类型文案。
    pub fn table_kind(self, kind: PartitionTableKind) -> &'static str {
        match self {
            Self::ZhCn | Self::ZhTw => match kind {
                PartitionTableKind::Mbr => "MBR",
                PartitionTableKind::Gpt => "GPT",
                PartitionTableKind::Unknown => "未知",
            },
            Self::En => match kind {
                PartitionTableKind::Mbr => "MBR",
                PartitionTableKind::Gpt => "GPT",
                PartitionTableKind::Unknown => "Unknown",
            },
        }
    }

    /// 写入结果文案。
    pub fn write_report(self, report: &WriteReport) -> String {
        match self {
            Self::ZhCn => match report {
                WriteReport::NotStarted => "未写入".to_owned(),
                WriteReport::Written { bytes } => format!("写入完成 {}", crate::ui::format_bytes(*bytes)),
                WriteReport::Partial { bytes, .. } => {
                    format!("写入中断 {}", crate::ui::format_bytes(*bytes))
                }
                WriteReport::Failed { .. } => "写入失败".to_owned(),
            },
            Self::ZhTw => match report {
                WriteReport::NotStarted => "未寫入".to_owned(),
                WriteReport::Written { bytes } => {
                    format!("寫入完成 {}", crate::ui::format_bytes(*bytes))
                }
                WriteReport::Partial { bytes, .. } => {
                    format!("寫入中斷 {}", crate::ui::format_bytes(*bytes))
                }
                WriteReport::Failed { .. } => "寫入失敗".to_owned(),
            },
            Self::En => match report {
                WriteReport::NotStarted => "Not written".to_owned(),
                WriteReport::Written { bytes } => {
                    format!("Write complete {}", crate::ui::format_bytes(*bytes))
                }
                WriteReport::Partial { bytes, .. } => {
                    format!("Write interrupted {}", crate::ui::format_bytes(*bytes))
                }
                WriteReport::Failed { .. } => "Write failed".to_owned(),
            },
        }
    }

    /// 校验结果文案。
    pub fn verify_report(self, report: &VerifyReport) -> String {
        match self {
            Self::ZhCn => match report {
                VerifyReport::NotRun => "未校验".to_owned(),
                VerifyReport::Passed => "校验通过".to_owned(),
                VerifyReport::Failed { offset, .. } => format!("校验失败 @ {offset}"),
                VerifyReport::Cancelled { offset } => format!("校验取消 @ {offset}"),
            },
            Self::ZhTw => match report {
                VerifyReport::NotRun => "未驗證".to_owned(),
                VerifyReport::Passed => "驗證通過".to_owned(),
                VerifyReport::Failed { offset, .. } => format!("驗證失敗 @ {offset}"),
                VerifyReport::Cancelled { offset } => format!("驗證取消 @ {offset}"),
            },
            Self::En => match report {
                VerifyReport::NotRun => "Not verified".to_owned(),
                VerifyReport::Passed => "Verify passed".to_owned(),
                VerifyReport::Failed { offset, .. } => format!("Verify failed @ {offset}"),
                VerifyReport::Cancelled { offset } => format!("Verify cancelled @ {offset}"),
            },
        }
    }

    /// 历史页：把台账里的 write/verify 状态翻译成本语言。
    /// `is_verify` 区分两个同为 `failed` 的枚举变体。
    /// 返回 (文本, 是否成功)；write 与 verify 的成功判定不同，调用方各取所需。
    pub fn describe_status(
        self,
        value: Option<&serde_json::Value>,
        is_verify: bool,
    ) -> (String, bool) {
        let Some(value) = value else {
            return match self {
                Self::ZhCn => ("未开始".to_owned(), false),
                Self::ZhTw => ("未開始".to_owned(), false),
                Self::En => ("Not started".to_owned(), false),
            };
        };
        let status = value.get("status").and_then(|item| item.as_str()).unwrap_or("");
        match status {
            "written" => {
                let bytes = value.get("bytes").and_then(|item| item.as_u64()).unwrap_or(0);
                let size = crate::ui::format_bytes(bytes);
                match self {
                    Self::ZhCn => (format!("写入 {size}"), true),
                    Self::ZhTw => (format!("寫入 {size}"), true),
                    Self::En => (format!("Wrote {size}"), true),
                }
            }
            "partial" => match self {
                Self::ZhCn => ("写入未完成".to_owned(), false),
                Self::ZhTw => ("寫入未完成".to_owned(), false),
                Self::En => ("Write incomplete".to_owned(), false),
            },
            "failed" => {
                let reason = value
                    .get("reason")
                    .and_then(|item| item.as_str())
                    .unwrap_or("");
                match self {
                    Self::ZhCn if is_verify => (format!("校验失败 {reason}"), false),
                    Self::ZhCn => (format!("写入失败 {reason}"), false),
                    Self::ZhTw if is_verify => (format!("驗證失敗 {reason}"), false),
                    Self::ZhTw => (format!("寫入失敗 {reason}"), false),
                    Self::En if is_verify => (format!("Verify failed {reason}"), false),
                    Self::En => (format!("Write failed {reason}"), false),
                }
            }
            "passed" => match self {
                Self::ZhCn => ("校验通过".to_owned(), true),
                Self::ZhTw => ("驗證通過".to_owned(), true),
                Self::En => ("Verify passed".to_owned(), true),
            },
            "not_run" => match self {
                Self::ZhCn => ("未校验".to_owned(), true),
                Self::ZhTw => ("未驗證".to_owned(), true),
                Self::En => ("Not verified".to_owned(), true),
            },
            "cancelled" => match self {
                Self::ZhCn => ("校验取消".to_owned(), false),
                Self::ZhTw => ("驗證取消".to_owned(), false),
                Self::En => ("Verify cancelled".to_owned(), false),
            },
            _ => ("—".to_owned(), false),
        }
    }

    /// 批次用时（RFC3339 起止）。
    pub fn format_duration(
        self,
        start: Option<chrono::DateTime<chrono::FixedOffset>>,
        end: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Option<String> {
        let seconds = (end? - start?).num_seconds();
        if seconds <= 0 {
            return None;
        }
        let text = match self {
            Self::ZhCn | Self::ZhTw => {
                let hour = if matches!(self, Self::ZhCn) { "时" } else { "時" };
                if seconds >= 3600 {
                    format!("{} {hour} {} 分", seconds / 3600, (seconds % 3600) / 60)
                } else if seconds >= 60 {
                    format!("{} 分 {} 秒", seconds / 60, seconds % 60)
                } else {
                    format!("{seconds} 秒")
                }
            }
            Self::En => {
                if seconds >= 3600 {
                    format!("{} h {} min", seconds / 3600, (seconds % 3600) / 60)
                } else if seconds >= 60 {
                    format!("{} min {} s", seconds / 60, seconds % 60)
                } else {
                    format!("{seconds} s")
                }
            }
        };
        Some(text)
    }

    // ------------------------------------------------------------ 日志 / 后台消息

    pub fn log_started(self) -> &'static str {
        match self {
            Self::ZhCn => "程序启动",
            Self::ZhTw => "程式啟動",
            Self::En => "App started",
        }
    }

    pub fn scan_none(self) -> &'static str {
        match self {
            Self::ZhCn => "扫描完成：未发现可写入的 USB / SD 卡",
            Self::ZhTw => "掃描完成：未發現可寫入的 USB / SD 卡",
            Self::En => "Scan finished: no writable USB / SD card found",
        }
    }

    pub fn scan_done(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("扫描完成：{count} 块候选盘"),
            Self::ZhTw => format!("掃描完成：{count} 塊候選碟"),
            Self::En => format!("Scan finished: {count} candidate drive(s)"),
        }
    }

    pub fn image_ready(self, name: &str, size: &str) -> String {
        match self {
            Self::ZhCn => format!("镜像就绪：{name} · {size}"),
            Self::ZhTw => format!("鏡像就緒：{name} · {size}"),
            Self::En => format!("Image ready: {name} · {size}"),
        }
    }

    pub fn image_failed_log(self, message: &str) -> String {
        match self {
            Self::ZhCn => format!("镜像读取失败：{message}"),
            Self::ZhTw => format!("鏡像讀取失敗：{message}"),
            Self::En => format!("Failed to read image: {message}"),
        }
    }

    pub fn task_accepted(self) -> &'static str {
        match self {
            Self::ZhCn => "任务已受理，开始预检",
            Self::ZhTw => "任務已受理，開始預檢",
            Self::En => "Task accepted, preflight started",
        }
    }

    pub fn batch_ended(self, state: &str) -> String {
        match self {
            Self::ZhCn => format!("批量结束：{state}"),
            Self::ZhTw => format!("批次結束：{state}"),
            Self::En => format!("Batch ended: {state}"),
        }
    }

    pub fn reason_line(self, error: &str) -> String {
        match self {
            Self::ZhCn | Self::ZhTw => format!("    原因：{error}"),
            Self::En => format!("    Reason: {error}"),
        }
    }

    pub fn ejected(self, letter: char) -> String {
        match self {
            Self::ZhCn => format!("已弹出 {letter}:"),
            Self::ZhTw => format!("已彈出 {letter}:"),
            Self::En => format!("Ejected {letter}:"),
        }
    }

    pub fn eject_failed(self, letter: char) -> String {
        match self {
            Self::ZhCn => format!("弹出 {letter}: 失败"),
            Self::ZhTw => format!("彈出 {letter}: 失敗"),
            Self::En => format!("Eject {letter}: failed"),
        }
    }

    pub fn batch_failed(self, message: &str) -> String {
        match self {
            Self::ZhCn => format!("批量失败：{message}"),
            Self::ZhTw => format!("批次失敗：{message}"),
            Self::En => format!("Batch failed: {message}"),
        }
    }

    pub fn rescan_log(self) -> &'static str {
        match self {
            Self::ZhCn => "重新扫描设备…",
            Self::ZhTw => "重新掃描裝置…",
            Self::En => "Rescanning devices…",
        }
    }

    pub fn pick_image_first(self) -> &'static str {
        match self {
            Self::ZhCn => "请选择镜像文件",
            Self::ZhTw => "請選擇鏡像文件",
            Self::En => "Select an image file",
        }
    }

    pub fn read_image_log(self, path: &str) -> String {
        match self {
            Self::ZhCn => format!("读取镜像：{path}"),
            Self::ZhTw => format!("讀取鏡像：{path}"),
            Self::En => format!("Reading image: {path}"),
        }
    }

    pub fn include_log(self, id: &str) -> String {
        match self {
            Self::ZhCn => format!("恢复参与：{id}"),
            Self::ZhTw => format!("恢復參與：{id}"),
            Self::En => format!("Included: {id}"),
        }
    }

    pub fn exclude_log(self, id: &str) -> String {
        match self {
            Self::ZhCn => format!("排除：{id}"),
            Self::ZhTw => format!("排除：{id}"),
            Self::En => format!("Excluded: {id}"),
        }
    }

    pub fn pin_wait(self) -> &'static str {
        match self {
            Self::ZhCn => "存在有数据/无法判断的盘，等待 PIN 确认",
            Self::ZhTw => "存在有資料/無法判斷的碟，等待 PIN 確認",
            Self::En => "Drives with data or unknown state present, waiting for PIN confirmation",
        }
    }

    pub fn all_empty(self) -> &'static str {
        match self {
            Self::ZhCn => "全部为空盘，跳过确认直接开始",
            Self::ZhTw => "全部為空碟，跳過確認直接開始",
            Self::En => "All drives are empty, starting without confirmation",
        }
    }

    pub fn no_targets(self) -> &'static str {
        match self {
            Self::ZhCn => "没有参与写入的盘",
            Self::ZhTw => "沒有參與寫入的碟",
            Self::En => "No participating drives",
        }
    }

    pub fn start_batch(self, targets: usize, excluded: usize) -> String {
        match self {
            Self::ZhCn => format!("开始批量写入：{targets} 块盘，排除 {excluded} 块"),
            Self::ZhTw => format!("開始批次寫入：{targets} 塊碟，排除 {excluded} 塊"),
            Self::En => format!("Batch started: {targets} drive(s), excluded {excluded}"),
        }
    }

    pub fn cancel_requested(self) -> &'static str {
        match self {
            Self::ZhCn => "用户请求中止整批",
            Self::ZhTw => "使用者要求中止整批",
            Self::En => "User requested batch abort",
        }
    }

    pub fn cancel_reason(self) -> &'static str {
        match self {
            Self::ZhCn => "用户取消确认",
            Self::ZhTw => "使用者取消確認",
            Self::En => "User cancelled confirmation",
        }
    }

    pub fn cancel_reason_esc(self) -> &'static str {
        match self {
            Self::ZhCn => "用户取消确认（Esc）",
            Self::ZhTw => "使用者取消確認（Esc）",
            Self::En => "User cancelled confirmation (Esc)",
        }
    }

    pub fn open_explorer_log(self, letter: &str) -> String {
        match self {
            Self::ZhCn => format!("在资源管理器中打开 {letter}"),
            Self::ZhTw => format!("在檔案總管中開啟 {letter}"),
            Self::En => format!("Opened {letter} in File Explorer"),
        }
    }

    pub fn logs_exported(self, path: &str) -> String {
        match self {
            Self::ZhCn => format!("日志已导出：{path}"),
            Self::ZhTw => format!("日誌已匯出：{path}"),
            Self::En => format!("Logs exported: {path}"),
        }
    }

    pub fn logs_export_failed(self, error: &str) -> String {
        match self {
            Self::ZhCn => format!("导出日志失败：{error}"),
            Self::ZhTw => format!("匯出日誌失敗：{error}"),
            Self::En => format!("Failed to export logs: {error}"),
        }
    }

    pub fn ledger_exported(self, path: &str) -> String {
        match self {
            Self::ZhCn => format!("台账已导出：{path}"),
            Self::ZhTw => format!("帳本已匯出：{path}"),
            Self::En => format!("Ledger exported: {path}"),
        }
    }

    pub fn ledger_export_failed(self, error: &str) -> String {
        match self {
            Self::ZhCn => format!("导出台账失败：{error}"),
            Self::ZhTw => format!("匯出帳本失敗：{error}"),
            Self::En => format!("Failed to export ledger: {error}"),
        }
    }

    pub fn busy_hashing(self) -> &'static str {
        match self {
            Self::ZhCn => "正在计算镜像哈希…",
            Self::ZhTw => "正在計算鏡像雜湊…",
            Self::En => "Computing image hash…",
        }
    }

    pub fn enumerate_failed(self, error: &str) -> String {
        match self {
            Self::ZhCn => format!("枚举设备失败：{error}"),
            Self::ZhTw => format!("列舉裝置失敗：{error}"),
            Self::En => format!("Failed to enumerate devices: {error}"),
        }
    }

    pub fn ledger_write_failed(self, path: &str, error: &str) -> String {
        match self {
            Self::ZhCn => format!("无法写台账 {path}：{error}"),
            Self::ZhTw => format!("無法寫帳本 {path}：{error}"),
            Self::En => format!("Failed to write ledger {path}: {error}"),
        }
    }

    pub fn ledger_locate_failed(self, error: &str) -> String {
        match self {
            Self::ZhCn => format!("无法定位程序目录：{error}"),
            Self::ZhTw => format!("無法定位程式目錄：{error}"),
            Self::En => format!("Failed to locate program directory: {error}"),
        }
    }

    pub fn ledger_locate_failed_plain(self) -> &'static str {
        match self {
            Self::ZhCn | Self::ZhTw => "无法定位程序目录",
            Self::En => "Failed to locate program directory",
        }
    }

    pub fn ledger_create_failed(self, path: &str, error: &str) -> String {
        match self {
            Self::ZhCn => format!("无法创建历史目录 {path}：{error}"),
            Self::ZhTw => format!("無法建立歷史目錄 {path}：{error}"),
            Self::En => format!("Failed to create history directory {path}: {error}"),
        }
    }

    /// 批次全部成功时的状态徽章文案。
    pub fn batch_ok(self, total: usize) -> String {
        match self {
            Self::ZhCn => format!("{total}/{total} 盘全部成功"),
            Self::ZhTw => format!("{total}/{total} 碟全部成功"),
            Self::En => format!("{total}/{total} disks OK"),
        }
    }

    /// 批次部分失败时的状态徽章文案，`failed` 为失败盘标签（如 `F:、G:`）。
    pub fn batch_partial(self, ok: usize, total: usize, failed: &str) -> String {
        match self {
            Self::ZhCn => format!("{ok}/{total} 盘成功（{failed} 失败）"),
            Self::ZhTw => format!("{ok}/{total} 碟成功（{failed} 失敗）"),
            Self::En => format!("{ok}/{total} disks OK ({failed} failed)"),
        }
    }

    pub fn avg_speed(self, speed: &str) -> String {
        match self {
            Self::ZhCn | Self::ZhTw => format!("均速 {speed}"),
            Self::En => format!("avg {speed}"),
        }
    }

    pub fn today_at(self, clock: &str) -> String {
        match self {
            Self::ZhCn | Self::ZhTw => format!("今天 {clock}"),
            Self::En => format!("Today {clock}"),
        }
    }

    pub fn yesterday_at(self, clock: &str) -> String {
        match self {
            Self::ZhCn | Self::ZhTw => format!("昨天 {clock}"),
            Self::En => format!("Yesterday {clock}"),
        }
    }

    pub fn selected_n(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("已选 {count} 项"),
            Self::ZhTw => format!("已選 {count} 項"),
            Self::En => format!("{count} selected"),
        }
    }

    pub fn delete_selected(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("删除所选 ({count})"),
            Self::ZhTw => format!("刪除所選 ({count})"),
            Self::En => format!("Delete selected ({count})"),
        }
    }

    pub fn export_selected(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("导出所选 ({count})"),
            Self::ZhTw => format!("匯出所選 ({count})"),
            Self::En => format!("Export selected ({count})"),
        }
    }

    pub fn records_deleted(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("已删除 {count} 条记录"),
            Self::ZhTw => format!("已刪除 {count} 筆記錄"),
            Self::En => format!("Deleted {count} records"),
        }
    }

    /// 重试：镜像已预填，等用户确认后开写。
    pub fn retry_prefilled(self, name: &str) -> String {
        match self {
            Self::ZhCn => format!("已预填上次镜像「{name}」，请确认目标盘后开始写入"),
            Self::ZhTw => format!("已預填上次鏡像「{name}」，請確認目標碟後開始寫入"),
            Self::En => format!("Reloaded \"{name}\"; confirm the targets and start writing"),
        }
    }
}

/// 关于页开源组件的用途条目。
#[derive(Debug, Clone, Copy)]
pub enum Usage {
    Font,
    Ui,
    Window,
    Storage,
    Sha,
    Xz,
    Time,
    Win32,
    Icon,
}

impl Lang {
    pub fn usage(self, usage: Usage) -> &'static str {
        match self {
            Self::ZhCn => match usage {
                Usage::Font => "界面字体",
                Usage::Ui => "图形界面",
                Usage::Window => "窗口与输入",
                Usage::Storage => "设置与台账",
                Usage::Sha => "镜像 SHA-256",
                Usage::Xz => ".img.xz 解压",
                Usage::Time => "时间与日志",
                Usage::Win32 => "Win32 接口",
                Usage::Icon => "界面图标",
            },
            Self::ZhTw => match usage {
                Usage::Font => "介面字型",
                Usage::Ui => "圖形介面",
                Usage::Window => "視窗與輸入",
                Usage::Storage => "設定與帳本",
                Usage::Sha => "鏡像 SHA-256",
                Usage::Xz => ".img.xz 解壓",
                Usage::Time => "時間與日誌",
                Usage::Win32 => "Win32 介面",
                Usage::Icon => "介面圖示",
            },
            Self::En => match usage {
                Usage::Font => "UI font",
                Usage::Ui => "GUI",
                Usage::Window => "Window & input",
                Usage::Storage => "Settings & ledger",
                Usage::Sha => "Image SHA-256",
                Usage::Xz => ".img.xz decompression",
                Usage::Time => "Time & logs",
                Usage::Win32 => "Win32 API",
                Usage::Icon => "UI icons",
            },
        }
    }
}

/// 静态文案表：不带插值的界面文案。
pub struct Texts {
    // 侧栏 / 导航
    pub nav_home: &'static str,
    pub nav_logs: &'static str,
    pub nav_history: &'static str,
    pub nav_settings: &'static str,
    pub nav_about: &'static str,
    /// 侧栏品牌主标题。
    pub brand: &'static str,
    // 日志筛选
    pub filter_all: &'static str,
    pub filter_info: &'static str,
    pub filter_warn: &'static str,
    pub filter_error: &'static str,
    // 主视图
    pub start_write: &'static str,
    pub ready: &'static str,
    pub browse: &'static str,
    pub load: &'static str,
    pub hint_image_path: &'static str,
    pub hint_image_ctrl: &'static str,
    pub drag_hint: &'static str,
    pub drop_to_load: &'static str,
    pub reading_sha: &'static str,
    pub enumerating: &'static str,
    pub no_devices: &'static str,
    pub step1: &'static str,
    pub step1_sub: &'static str,
    pub step2: &'static str,
    pub step2_sub: &'static str,
    pub step3: &'static str,
    pub step3_sub: &'static str,
    pub scan_devices: &'static str,
    pub scan_devices_tip: &'static str,
    pub reason_loading: &'static str,
    pub reason_no_image: &'static str,
    pub reason_scanning: &'static str,
    pub reason_no_disk: &'static str,
    pub reason_no_target: &'static str,
    // 进度视图
    pub cancel_batch: &'static str,
    pub cancel_batch_tip: &'static str,
    pub esc_abort_hint: &'static str,
    pub preparing: &'static str,
    // 完成视图
    pub back_rescan: &'static str,
    pub view_ledger: &'static str,
    pub no_result: &'static str,
    pub all_done: &'static str,
    pub aborted: &'static str,
    pub passed: &'static str,
    pub not_passed: &'static str,
    // 确认弹窗
    pub confirm_title: &'static str,
    pub confirm_warning: &'static str,
    pub pin_prompt: &'static str,
    pub pin_correct: &'static str,
    pub cancel: &'static str,
    pub confirm_write: &'static str,
    // 设备卡
    pub card_participate_tip: &'static str,
    pub card_open_tip: &'static str,
    pub no_letter: &'static str,
    pub no_label: &'static str,
    pub excluded: &'static str,
    pub no_signature: &'static str,
    // 日志页
    pub logs_title: &'static str,
    pub open_log_dir: &'static str,
    pub confirm_clear: &'static str,
    pub clear: &'static str,
    pub copy: &'static str,
    pub export: &'static str,
    pub search_logs_hint: &'static str,
    pub autoscroll: &'static str,
    pub empty_level: &'static str,
    pub no_match_logs: &'static str,
    pub logs_cleared: &'static str,
    pub logs_dir_failed: &'static str,
    // 历史页
    pub history_title: &'static str,
    pub refresh: &'static str,
    pub empty_ledger_title: &'static str,
    pub empty_ledger_hint: &'static str,
    pub no_disk_records: &'static str,
    pub ledger_cleared: &'static str,
    /// 历史页搜索框占位。
    pub ledger_search_hint: &'static str,
    pub filter_success: &'static str,
    pub filter_failed: &'static str,
    /// 统计条：成功批次 / 失败批次 / 总写入量。
    pub stat_ok: &'static str,
    pub stat_failed: &'static str,
    pub stat_written: &'static str,
    /// 日期分组。
    pub group_today: &'static str,
    pub group_yesterday: &'static str,
    pub retry: &'static str,
    pub view_full_log: &'static str,
    pub copy_image_name: &'static str,
    pub copy_error_text: &'static str,
    pub delete_record: &'static str,
    pub clear_records: &'static str,
    pub clear_logs_too: &'static str,
    pub export_csv: &'static str,
    pub export_json: &'static str,
    pub cancel_selection: &'static str,
    pub no_match_records: &'static str,
    /// 解释「跳过 n 块」是已排除的盘。
    pub skipped_tip: &'static str,
    /// 重试时找不到原镜像文件。
    pub retry_missing_image: &'static str,
    // 设置页
    pub settings_title: &'static str,
    pub group_device: &'static str,
    pub group_write: &'static str,
    pub group_log: &'static str,
    pub group_general: &'static str,
    pub auto_detect: &'static str,
    pub auto_detect_desc: &'static str,
    pub verify_after: &'static str,
    pub verify_after_desc: &'static str,
    pub eject_after: &'static str,
    pub eject_after_desc: &'static str,
    pub keep_days: &'static str,
    pub keep_days_desc: &'static str,
    pub forever: &'static str,
    pub settings_updated: &'static str,
    pub language: &'static str,
    pub language_desc: &'static str,
    // 语言选择按钮（三种语言下显示原样）
    pub lang_zh_cn: &'static str,
    pub lang_zh_tw: &'static str,
    pub lang_en: &'static str,
    // 关于页
    pub about_title: &'static str,
    pub author: &'static str,
    pub repo: &'static str,
    pub license_label: &'static str,
    pub copy_version: &'static str,
    pub copy_version_tip: &'static str,
    pub about_intro: &'static str,
    pub open_source: &'static str,
    pub open_source_hint: &'static str,
    pub license_note: &'static str,
}

/// 简体中文。
const ZH_CN: Texts = Texts {
    nav_home: "首页",
    nav_logs: "日志",
    nav_history: "历史",
    nav_settings: "设置",
    nav_about: "关于",
    brand: "齐刻",
    filter_all: "全部",
    filter_info: "信息",
    filter_warn: "警告",
    filter_error: "错误",
    start_write: "开始写入",
    ready: "就绪",
    browse: "浏览…",
    load: "加载",
    hint_image_path: "选择镜像文件…",
    hint_image_ctrl: "选择镜像文件 (Ctrl+O)",
    drag_hint: "支持 ISO / IMG / IMG.XZ · 可把文件拖到窗口里",
    drop_to_load: "松开鼠标以加载镜像",
    reading_sha: "正在读取镜像并计算 SHA-256…",
    enumerating: "正在枚举可移动磁盘…",
    no_devices: "未发现可写入设备",
    step1: "选择镜像",
    step1_sub: "ISO / IMG / IMG.XZ",
    step2: "插入 U 盘",
    step2_sub: "支持多块同时写入",
    step3: "开始写入",
    step3_sub: "校验通过才算完成",
    scan_devices: "扫描设备",
    scan_devices_tip: "重新扫描设备 (F5)",
    reason_loading: "正在读取镜像…",
    reason_no_image: "请先选择镜像文件",
    reason_scanning: "正在扫描设备…",
    reason_no_disk: "请插入 U 盘 / SD 卡",
    reason_no_target: "请至少保留一块参与的盘",
    cancel_batch: "中止整批",
    cancel_batch_tip: "中止整批写入 (Esc)",
    esc_abort_hint: "Esc 也可中止",
    preparing: "准备中：校验设备与镜像…",
    back_rescan: "返回并重新扫描",
    view_ledger: "查看台账",
    no_result: "没有结果",
    all_done: "全部完成",
    aborted: "已中止",
    passed: "通过",
    not_passed: "未通过",
    confirm_title: "确认写入",
    confirm_warning: "以下磁盘将被覆盖，原有数据不可恢复：",
    pin_prompt: "请输入下方 6 位数字以确认（不支持粘贴）：",
    pin_correct: "PIN 正确",
    cancel: "取消",
    confirm_write: "确认写入",
    card_participate_tip: "参与本次批量写入",
    card_open_tip: "在资源管理器中打开",
    no_letter: "无盘符",
    no_label: "无卷标",
    excluded: "已排除",
    no_signature: "无",
    logs_title: "日志",
    open_log_dir: "打开日志目录",
    confirm_clear: "确认清空？",
    clear: "清空",
    copy: "复制",
    export: "导出",
    search_logs_hint: "搜索日志…",
    autoscroll: "自动滚动",
    empty_level: "该级别暂无日志",
    no_match_logs: "没有匹配的日志",
    logs_cleared: "已清空实时日志",
    logs_dir_failed: "无法打开 logs 文件夹",
    history_title: "历史",
    refresh: "刷新",
    empty_ledger_title: "暂无写入记录",
    empty_ledger_hint: "完成一次写入后，这里会按时间倒序列出台账",
    no_disk_records: "该批次没有逐盘记录",
    ledger_cleared: "已清空台账记录",
    ledger_search_hint: "搜索镜像名或盘符…",
    filter_success: "成功",
    filter_failed: "失败",
    stat_ok: "成功批次",
    stat_failed: "失败批次",
    stat_written: "总写入量",
    group_today: "今天",
    group_yesterday: "昨天",
    retry: "重试",
    view_full_log: "查看完整日志",
    copy_image_name: "复制镜像名",
    copy_error_text: "复制错误信息",
    delete_record: "删除此条",
    clear_records: "清空记录",
    clear_logs_too: "同时清空日志",
    export_csv: "导出 CSV",
    export_json: "导出 JSON",
    cancel_selection: "取消选择",
    no_match_records: "没有匹配的记录",
    skipped_tip: "本批次已排除、未参与写入的盘",
    retry_missing_image: "找不到上次使用的镜像文件，请重新选择",
    settings_title: "设置",
    group_device: "设备",
    group_write: "写入",
    group_log: "日志",
    group_general: "通用",
    auto_detect: "插入后自动检测",
    auto_detect_desc: "插入 U 盘 / SD 卡后自动刷新设备列表",
    verify_after: "写入后校验",
    verify_after_desc: "写完回读逐块比对；关闭后只写不验，台账记 not_run",
    eject_after: "完成后弹出设备",
    eject_after_desc: "整批完成后自动弹出参与写入的 U 盘",
    keep_days: "日志保留天数",
    keep_days_desc: "启动时清理更早的日志行",
    forever: "永久",
    settings_updated: "设置已更新",
    language: "语言",
    language_desc: "切换后立即生效",
    lang_zh_cn: "简体中文",
    lang_zh_tw: "繁體中文",
    lang_en: "English",
    about_title: "关于",
    author: "作者",
    repo: "仓库",
    license_label: "许可",
    copy_version: "复制版本信息",
    copy_version_tip: "复制版本、作者、许可与仓库地址，方便提 issue",
    about_intro: "一次选择镜像，同时写入多块 U 盘 / SD 卡，每块盘独立校验、独立报告结果。未选中的设备不会被读取，确认范围之外的盘也不会被写入。",
    open_source: "开源软件",
    open_source_hint: "点击任一项可在浏览器中打开项目主页。",
    license_note: "齐刻 Qike Flash 以 Apache-2.0 许可发布；以上第三方组件遵循其各自的许可。",
};

/// 繁體中文。
const ZH_TW: Texts = Texts {
    nav_home: "首頁",
    nav_logs: "日誌",
    nav_history: "歷史",
    nav_settings: "設定",
    nav_about: "關於",
    brand: "齊刻",
    filter_all: "全部",
    filter_info: "資訊",
    filter_warn: "警告",
    filter_error: "錯誤",
    start_write: "開始寫入",
    ready: "就緒",
    browse: "瀏覽…",
    load: "載入",
    hint_image_path: "選擇鏡像文件…",
    hint_image_ctrl: "選擇鏡像文件 (Ctrl+O)",
    drag_hint: "支援 ISO / IMG / IMG.XZ · 可把檔案拖到視窗裡",
    drop_to_load: "放開滑鼠以載入鏡像",
    reading_sha: "正在讀取鏡像並計算 SHA-256…",
    enumerating: "正在列舉可抽取式磁碟…",
    no_devices: "未發現可寫入裝置",
    step1: "選擇鏡像",
    step1_sub: "ISO / IMG / IMG.XZ",
    step2: "插入 U 碟",
    step2_sub: "支援多碟同時寫入",
    step3: "開始寫入",
    step3_sub: "校驗通過才算完成",
    scan_devices: "掃描裝置",
    scan_devices_tip: "重新掃描裝置 (F5)",
    reason_loading: "正在讀取鏡像…",
    reason_no_image: "請先選擇鏡像文件",
    reason_scanning: "正在掃描裝置…",
    reason_no_disk: "請插入 U 碟 / SD 卡",
    reason_no_target: "請至少保留一塊參與的碟",
    cancel_batch: "中止整批",
    cancel_batch_tip: "中止整批寫入 (Esc)",
    esc_abort_hint: "Esc 也可中止",
    preparing: "準備中：校驗裝置與鏡像…",
    back_rescan: "返回並重新掃描",
    view_ledger: "檢視帳本",
    no_result: "沒有結果",
    all_done: "全部完成",
    aborted: "已中止",
    passed: "通過",
    not_passed: "未通過",
    confirm_title: "確認寫入",
    confirm_warning: "以下磁碟將被覆寫，原有資料無法復原：",
    pin_prompt: "請輸入下方 6 位數字以確認（不支援貼上）：",
    pin_correct: "PIN 正確",
    cancel: "取消",
    confirm_write: "確認寫入",
    card_participate_tip: "參與本次批次寫入",
    card_open_tip: "在檔案總管中開啟",
    no_letter: "無盤符",
    no_label: "無卷標",
    excluded: "已排除",
    no_signature: "無",
    logs_title: "日誌",
    open_log_dir: "開啟日誌目錄",
    confirm_clear: "確認清空？",
    clear: "清空",
    copy: "複製",
    export: "匯出",
    search_logs_hint: "搜尋日誌…",
    autoscroll: "自動捲動",
    empty_level: "該級別暫無日誌",
    no_match_logs: "沒有符合的日誌",
    logs_cleared: "已清空即時日誌",
    logs_dir_failed: "無法開啟 logs 資料夾",
    history_title: "歷史",
    refresh: "重新整理",
    empty_ledger_title: "暫無寫入紀錄",
    empty_ledger_hint: "完成一次寫入後，這裡會按時間倒序列出帳本",
    no_disk_records: "該批次沒有逐碟紀錄",
    ledger_cleared: "已清空帳本紀錄",
    ledger_search_hint: "搜尋鏡像名稱或磁碟機代號…",
    filter_success: "成功",
    filter_failed: "失敗",
    stat_ok: "成功批次",
    stat_failed: "失敗批次",
    stat_written: "總寫入量",
    group_today: "今天",
    group_yesterday: "昨天",
    retry: "重試",
    view_full_log: "檢視完整日誌",
    copy_image_name: "複製鏡像名稱",
    copy_error_text: "複製錯誤資訊",
    delete_record: "刪除此筆",
    clear_records: "清空記錄",
    clear_logs_too: "同時清空日誌",
    export_csv: "匯出 CSV",
    export_json: "匯出 JSON",
    cancel_selection: "取消選取",
    no_match_records: "沒有符合的記錄",
    skipped_tip: "本批次已排除、未參與寫入的碟",
    retry_missing_image: "找不到上次使用的鏡像檔案，請重新選擇",
    settings_title: "設定",
    group_device: "裝置",
    group_write: "寫入",
    group_log: "日誌",
    group_general: "一般",
    auto_detect: "插入後自動偵測",
    auto_detect_desc: "插入 U 碟 / SD 卡後自動重新整理裝置清單",
    verify_after: "寫入後校驗",
    verify_after_desc: "寫完回讀逐塊比對；關閉後只寫不驗，帳本記 not_run",
    eject_after: "完成後彈出裝置",
    eject_after_desc: "整批完成後自動彈出參與寫入的 U 碟",
    keep_days: "日誌保留天數",
    keep_days_desc: "啟動時清理更早的日誌行",
    forever: "永久",
    settings_updated: "設定已更新",
    language: "語言",
    language_desc: "切換後立即生效",
    lang_zh_cn: "简体中文",
    lang_zh_tw: "繁體中文",
    lang_en: "English",
    about_title: "關於",
    author: "作者",
    repo: "儲存庫",
    license_label: "授權",
    copy_version: "複製版本資訊",
    copy_version_tip: "複製版本、作者、授權與儲存庫地址，方便提 issue",
    about_intro: "一次選擇鏡像，同時寫入多塊 U 碟 / SD 卡，每塊碟獨立校驗、獨立回報結果。未選取的裝置不會被讀取，確認範圍之外的碟也不會被寫入。",
    open_source: "開源軟體",
    open_source_hint: "點擊任一項可在瀏覽器中開啟專案主頁。",
    license_note: "齊刻 Qike Flash 以 Apache-2.0 授權發布；以上第三方元件遵循其各自的授權。",
};

/// English。
const EN: Texts = Texts {
    nav_home: "Home",
    nav_logs: "Logs",
    nav_history: "History",
    nav_settings: "Settings",
    nav_about: "About",
    brand: "Qike",
    filter_all: "All",
    filter_info: "Info",
    filter_warn: "Warn",
    filter_error: "Error",
    start_write: "Start writing",
    ready: "Ready",
    browse: "Browse…",
    load: "Load",
    hint_image_path: "Select an image file…",
    hint_image_ctrl: "Select an image file (Ctrl+O)",
    drag_hint: "Supports ISO / IMG / IMG.XZ · or drop a file into the window",
    drop_to_load: "Drop to load the image",
    reading_sha: "Reading image and computing SHA-256…",
    enumerating: "Enumerating removable drives…",
    no_devices: "No writable drives found",
    step1: "Select image",
    step1_sub: "ISO / IMG / IMG.XZ",
    step2: "Insert drives",
    step2_sub: "Multiple drives at once",
    step3: "Start writing",
    step3_sub: "Verified before done",
    scan_devices: "Scan devices",
    scan_devices_tip: "Rescan devices (F5)",
    reason_loading: "Reading image…",
    reason_no_image: "Select an image file first",
    reason_scanning: "Scanning devices…",
    reason_no_disk: "Insert a USB / SD card",
    reason_no_target: "Keep at least one participating drive",
    cancel_batch: "Abort batch",
    cancel_batch_tip: "Abort the whole batch (Esc)",
    esc_abort_hint: "Esc also aborts",
    preparing: "Preparing: checking drives and image…",
    back_rescan: "Back and rescan",
    view_ledger: "View ledger",
    no_result: "No result",
    all_done: "All done",
    aborted: "Aborted",
    passed: "Passed",
    not_passed: "Failed",
    confirm_title: "Confirm writing",
    confirm_warning: "The drives below will be overwritten; existing data cannot be recovered:",
    pin_prompt: "Type the 6-digit number below to confirm (paste disabled):",
    pin_correct: "PIN correct",
    cancel: "Cancel",
    confirm_write: "Confirm",
    card_participate_tip: "Include in this batch",
    card_open_tip: "Open in File Explorer",
    no_letter: "No letter",
    no_label: "No label",
    excluded: "Excluded",
    no_signature: "None",
    logs_title: "Logs",
    open_log_dir: "Open log folder",
    confirm_clear: "Confirm clear?",
    clear: "Clear",
    copy: "Copy",
    export: "Export",
    search_logs_hint: "Search logs…",
    autoscroll: "Auto-scroll",
    empty_level: "No logs for this level",
    no_match_logs: "No matching logs",
    logs_cleared: "Live logs cleared",
    logs_dir_failed: "Failed to open the logs folder",
    history_title: "History",
    refresh: "Refresh",
    empty_ledger_title: "No records yet",
    empty_ledger_hint: "After a batch finishes, records are listed here, newest first",
    no_disk_records: "No per-drive records in this batch",
    ledger_cleared: "Ledger cleared",
    ledger_search_hint: "Search image name or drive…",
    filter_success: "Success",
    filter_failed: "Failed",
    stat_ok: "Successful",
    stat_failed: "Failed",
    stat_written: "Total written",
    group_today: "Today",
    group_yesterday: "Yesterday",
    retry: "Retry",
    view_full_log: "View full log",
    copy_image_name: "Copy image name",
    copy_error_text: "Copy error",
    delete_record: "Delete record",
    clear_records: "Clear records",
    clear_logs_too: "Also clear logs",
    export_csv: "Export CSV",
    export_json: "Export JSON",
    cancel_selection: "Clear selection",
    no_match_records: "No matching records",
    skipped_tip: "Disks excluded from this batch",
    retry_missing_image: "The previous image file is missing; choose it again",
    settings_title: "Settings",
    group_device: "Devices",
    group_write: "Writing",
    group_log: "Logs",
    group_general: "General",
    auto_detect: "Auto-detect on insert",
    auto_detect_desc: "Refresh the device list automatically when a USB / SD card is plugged in",
    verify_after: "Verify after writing",
    verify_after_desc: "Read back and compare block by block; when off, verification is skipped and logged as not_run",
    eject_after: "Eject when finished",
    eject_after_desc: "Eject the participating drives automatically after the batch finishes",
    keep_days: "Log retention",
    keep_days_desc: "Remove older log lines on startup",
    forever: "Forever",
    settings_updated: "Settings updated",
    language: "Language",
    language_desc: "Takes effect immediately",
    lang_zh_cn: "简体中文",
    lang_zh_tw: "繁體中文",
    lang_en: "English",
    about_title: "About",
    author: "Author",
    repo: "Repository",
    license_label: "License",
    copy_version: "Copy version info",
    copy_version_tip: "Copy version, author, license and repository URL for filing issues",
    about_intro: "Flash one image to multiple USB drives or SD cards at once, with per-drive verification and per-drive results. Unselected devices are never read, and no drive outside the confirmed set is ever written.",
    open_source: "Open-source software",
    open_source_hint: "Click any entry to open its project page in a browser.",
    license_note: "Qike Flash is released under the Apache-2.0 license; the third-party components above keep their own licenses.",
};
