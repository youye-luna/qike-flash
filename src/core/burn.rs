//! `BurnTask`：六态状态机 + 并发扇出写入 + 无缓存回读校验。
//!
//! 安全约束：
//! - 确认后按 DeviceId 重新检查（D4），已排除的盘不会被 resolve（D3）
//! - 任一盘打开失败 / 容量不足 → 整批不写（C9、C10、D2）
//! - 单盘写失败不影响其他盘，但最终状态一定为 Aborted（A5、D9）
//! - 写完立刻关闭可写句柄（D7），校验走后端无缓存读（D5）

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use chrono::Local;

use super::device::{DeviceCandidate, DeviceId};
use super::error::{BurnError, Result};
use super::sink::{LedgerEntry, VerifyReport, WriteReport};
use super::traits::{BackendFactory, DeviceEnumerator, ImageSource, OutputSink, WriteBackend};

const ZERO_BYTES: u64 = 1024 * 1024;

/// 六态，match 必须穷尽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnState {
    Idle,
    Preparing,
    Writing,
    Verifying,
    Completed,
    Aborted,
}

impl BurnState {
    pub fn label(self) -> &'static str {
        match self {
            BurnState::Idle => "空闲",
            BurnState::Preparing => "准备中",
            BurnState::Writing => "写入中",
            BurnState::Verifying => "校验中",
            BurnState::Completed => "已完成",
            BurnState::Aborted => "已中止",
        }
    }
}

/// 单块盘在批量过程中的阶段，供界面逐盘展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskPhase {
    Waiting,
    Writing,
    Verifying,
    Done,
    Failed,
}

impl DiskPhase {
    pub fn label(self) -> &'static str {
        match self {
            DiskPhase::Waiting => "等待中",
            DiskPhase::Writing => "写入中",
            DiskPhase::Verifying => "校验中",
            DiskPhase::Done => "已完成",
            DiskPhase::Failed => "失败",
        }
    }
}

impl DiskPhase {
    fn from_code(code: u8) -> Self {
        match code {
            1 => DiskPhase::Writing,
            2 => DiskPhase::Verifying,
            3 => DiskPhase::Done,
            4 => DiskPhase::Failed,
            _ => DiskPhase::Waiting,
        }
    }

    fn code(self) -> u8 {
        match self {
            DiskPhase::Waiting => 0,
            DiskPhase::Writing => 1,
            DiskPhase::Verifying => 2,
            DiskPhase::Done => 3,
            DiskPhase::Failed => 4,
        }
    }
}

/// 单块盘的实时进度快照。
#[derive(Debug, Clone)]
pub struct DiskProgress {
    pub device_id: String,
    pub written_bytes: u64,
    pub phase: DiskPhase,
}

#[derive(Default)]
struct DiskTrack {
    written_bytes: AtomicU64,
    phase: AtomicU8,
}

/// 供界面读取的状态快照。
#[derive(Debug, Clone, Copy)]
pub struct BurnSnapshot {
    pub state: BurnState,
    pub written_bytes: u64,
    pub total_bytes: u64,
    pub cancelled: bool,
}

/// 状态机共享控制块：UI 只发取消命令、只读快照。
pub struct BurnControl {
    state: Mutex<BurnState>,
    cancel: AtomicBool,
    written_bytes: AtomicU64,
    total_bytes: AtomicU64,
    tracks: Mutex<Vec<(String, Arc<DiskTrack>)>>,
}

impl BurnControl {
    fn new() -> Self {
        Self {
            state: Mutex::new(BurnState::Idle),
            cancel: AtomicBool::new(false),
            written_bytes: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            tracks: Mutex::new(Vec::new()),
        }
    }

    pub fn state(&self) -> BurnState {
        match self.state.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub fn snapshot(&self) -> BurnSnapshot {
        BurnSnapshot {
            state: self.state(),
            written_bytes: self.written_bytes.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            cancelled: self.cancel.load(Ordering::Relaxed),
        }
    }

    /// 请求中止整批（C5）。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn set_state(&self, state: BurnState) {
        match self.state.lock() {
            Ok(mut guard) => *guard = state,
            Err(poisoned) => *poisoned.into_inner() = state,
        }
    }

    fn set_total(&self, total: u64) {
        self.total_bytes.store(total, Ordering::Relaxed);
    }

    fn set_written(&self, written: u64) {
        self.written_bytes.store(written, Ordering::Relaxed);
    }

    /// 逐盘进度：登记参与本批的设备（顺序与写入/校验索引一致）。
    fn register_disks(&self, ids: Vec<String>) {
        let tracks = ids
            .into_iter()
            .map(|id| (id, Arc::new(DiskTrack::default())))
            .collect();
        match self.tracks.lock() {
            Ok(mut guard) => *guard = tracks,
            Err(poisoned) => *poisoned.into_inner() = tracks,
        }
    }

    fn track(&self, index: usize) -> Option<Arc<DiskTrack>> {
        match self.tracks.lock() {
            Ok(guard) => guard.get(index).map(|(_, track)| Arc::clone(track)),
            Err(poisoned) => poisoned
                .into_inner()
                .get(index)
                .map(|(_, track)| Arc::clone(track)),
        }
    }

    /// 读取每块盘的实时进度快照。
    pub fn disk_progress(&self) -> Vec<DiskProgress> {
        let guard = match self.tracks.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .iter()
            .map(|(id, track)| DiskProgress {
                device_id: id.clone(),
                written_bytes: track.written_bytes.load(Ordering::Relaxed),
                phase: DiskPhase::from_code(track.phase.load(Ordering::Relaxed)),
            })
            .collect()
    }

    fn mark_phase(&self, index: usize, phase: DiskPhase) {
        if let Some(track) = self.track(index) {
            track.phase.store(phase.code(), Ordering::Relaxed);
        }
    }

    fn add_written(&self, index: usize, delta: u64) {
        if let Some(track) = self.track(index) {
            track.written_bytes.fetch_add(delta, Ordering::Relaxed);
        }
    }
}

/// 写入参数。
#[derive(Debug, Clone)]
pub struct BurnOptions {
    pub chunk_bytes: usize,
    /// 写入后是否回读校验（关闭时台账记为 not_run）。
    pub verify: bool,
}

impl Default for BurnOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 1024 * 1024,
            verify: true,
        }
    }
}

impl BurnOptions {
    fn normalized_chunk(&self) -> usize {
        let value = self.chunk_bytes.clamp(4096, 8 * 1024 * 1024);
        value / 4096 * 4096
    }
}

/// 一次批量写入计划。
pub struct BurnPlan {
    pub image: Box<dyn ImageSource>,
    pub image_name: String,
    /// 镜像的完整路径，写进台账用于「重试」。
    pub image_path: String,
    pub image_sha256: String,
    pub targets: Vec<DeviceId>,
    /// D10：排除项要写入台账。
    pub excluded: Vec<DeviceId>,
    pub options: BurnOptions,
}

impl BurnPlan {
    pub fn new(
        image: Box<dyn ImageSource>,
        image_name: String,
        image_path: String,
        image_sha256: String,
        targets: Vec<DeviceId>,
        excluded: Vec<DeviceId>,
        options: BurnOptions,
    ) -> Self {
        Self {
            image,
            image_name,
            image_path,
            image_sha256,
            targets,
            excluded,
            options,
        }
    }
}

/// 单盘最终结果。
#[derive(Debug, Clone)]
pub struct DiskOutcome {
    pub device: DeviceId,
    /// 厂商 + 型号；取不到时为空串。
    pub model: String,
    /// 写入前已挂载的盘符（如 `F: G:`）。
    pub letters: String,
    pub capacity_bytes: u64,
    pub write: WriteReport,
    pub verify: VerifyReport,
    pub error: Option<String>,
}

impl DiskOutcome {
    fn not_started(
        device: DeviceId,
        model: String,
        letters: String,
        capacity_bytes: u64,
    ) -> Self {
        Self {
            device,
            model,
            letters,
            capacity_bytes,
            write: WriteReport::NotStarted,
            verify: VerifyReport::NotRun,
            error: None,
        }
    }

    fn fail(&mut self, reason: String, written_bytes: u64) {
        self.write = if written_bytes > 0 {
            WriteReport::Partial {
                bytes: written_bytes,
                reason: reason.clone(),
            }
        } else {
            WriteReport::Failed {
                reason: reason.clone(),
            }
        };
        self.error = Some(reason);
    }
}

/// 整批结果。
#[derive(Debug, Clone)]
pub struct BurnOutcome {
    pub state: BurnState,
    pub cancelled: bool,
    pub batch_error: Option<String>,
    pub disks: Vec<DiskOutcome>,
    pub started_at: String,
    pub finished_at: String,
}

/// 批量写入状态机。
pub struct BurnTask<'a> {
    plan: BurnPlan,
    enumerator: &'a dyn DeviceEnumerator,
    factory: &'a dyn BackendFactory,
    sink: &'a mut dyn OutputSink,
    control: Arc<BurnControl>,
}

impl<'a> BurnTask<'a> {
    pub fn new(
        plan: BurnPlan,
        enumerator: &'a dyn DeviceEnumerator,
        factory: &'a dyn BackendFactory,
        sink: &'a mut dyn OutputSink,
    ) -> Self {
        Self {
            plan,
            enumerator,
            factory,
            sink,
            control: Arc::new(BurnControl::new()),
        }
    }

    /// 共享控制块（取消 / 状态快照）。
    pub fn control(&self) -> Arc<BurnControl> {
        Arc::clone(&self.control)
    }

    pub fn run(&mut self) -> Result<BurnOutcome> {
        let started_at = now_rfc3339();
        self.control.set_state(BurnState::Preparing);
        self.control.set_total(self.plan.image.size());

        if self.plan.targets.is_empty() {
            self.control.set_state(BurnState::Aborted);
            return Err(BurnError::message("没有选择要写入的目标盘"));
        }
        let mut seen = std::collections::HashSet::new();
        for id in &self.plan.targets {
            if !seen.insert(id.as_str()) {
                self.control.set_state(BurnState::Aborted);
                return Err(BurnError::message(format!("目标盘重复: {id}")));
            }
        }

        // D4：按 DeviceId 重新检查；D3：只检查目标，不触碰排除项。
        let resolved = match self.enumerator.resolve(&self.plan.targets) {
            Ok(value) => value,
            Err(error) => {
                self.control.set_state(BurnState::Aborted);
                return Err(error);
            }
        };
        if resolved.len() != self.plan.targets.len() {
            self.control.set_state(BurnState::Aborted);
            return Err(BurnError::message(format!(
                "重新检查设备时数量不一致：期望 {}，实际 {}",
                self.plan.targets.len(),
                resolved.len()
            )));
        }

        let by_id: HashMap<&str, &DeviceCandidate> = resolved
            .iter()
            .map(|candidate| (candidate.id.as_str(), candidate))
            .collect();
        let mut ordered: Vec<&DeviceCandidate> = Vec::with_capacity(self.plan.targets.len());
        for target in &self.plan.targets {
            match by_id.get(target.as_str()) {
                Some(candidate) => ordered.push(candidate),
                None => {
                    self.control.set_state(BurnState::Aborted);
                    return Err(BurnError::IdentityChanged {
                        device: target.clone(),
                    });
                }
            }
        }

        self.emit_batch(&started_at)?;

        let image_size = self.plan.image.size();
        let mut disks: Vec<DiskOutcome> = ordered
            .iter()
            .map(|candidate| {
                DiskOutcome::not_started(
                    candidate.id.clone(),
                    candidate.model.clone(),
                    candidate.letters_display(),
                    candidate.capacity_bytes,
                )
            })
            .collect();
        self.control
            .register_disks(ordered.iter().map(|c| c.id.to_string()).collect());

        // C10 / B2：容量不足或系统盘 → 整批不写。
        let mut preflight_errors: Vec<String> = Vec::new();
        for (index, (outcome, candidate)) in disks.iter_mut().zip(&ordered).enumerate() {
            let reason = if candidate.is_system {
                Some(format!("{} 是系统盘 / 启动盘，禁止写入", outcome.device))
            } else if candidate.capacity_bytes < image_size {
                Some(format!(
                    "{} 容量不足：镜像 {} 字节，设备 {} 字节",
                    outcome.device, image_size, candidate.capacity_bytes
                ))
            } else {
                None
            };
            if let Some(text) = reason {
                outcome.fail(text.clone(), 0);
                self.control.mark_phase(index, DiskPhase::Failed);
                preflight_errors.push(text);
            }
        }
        if !preflight_errors.is_empty() {
            let summary = preflight_errors.join("；");
            return self.finish(disks, started_at, Some(summary), false);
        }

        // 全部打开成功才允许写入；任一盘占用 / 打开失败 → 整批中止（C9、D2）。
        let mut backends: Vec<Option<Box<dyn WriteBackend>>> = Vec::with_capacity(ordered.len());
        let mut open_errors: Vec<String> = Vec::new();
        for (index, candidate) in ordered.iter().enumerate() {
            match self.factory.open_exclusive(&candidate.id) {
                Ok(backend) => backends.push(Some(backend)),
                Err(error) => {
                    let reason = format!("{} 打开失败：{error}", candidate.id);
                    disks[index].fail(reason.clone(), 0);
                    self.control.mark_phase(index, DiskPhase::Failed);
                    open_errors.push(reason);
                    backends.push(None);
                }
            }
        }
        if !open_errors.is_empty() {
            return self.finish(disks, started_at, Some(open_errors.join("；")), false);
        }

        // 占用自检（后端已独占打开；这里保留显式检查，失败同样整批中止）。
        for (index, backend) in backends.iter().enumerate() {
            let busy = match backend {
                Some(value) => value.is_busy().unwrap_or(false),
                None => false,
            };
            if busy {
                let reason = format!("{} 仍被占用，请关闭正在使用它的窗口", ordered[index].id);
                disks[index].fail(reason, 0);
                self.control.mark_phase(index, DiskPhase::Failed);
                return self.finish(disks, started_at, Some("存在被占用的盘".into()), false);
            }
        }

        self.control.set_state(BurnState::Writing);

        // D6：写入前清零前 1 MiB，清掉残留 MBR / GPT 签名。
        let zero_buffer = vec![0u8; ZERO_BYTES as usize];
        for (index, backend) in backends.iter_mut().enumerate() {
            if let Some(value) = backend {
                let length = value.capacity_bytes().min(ZERO_BYTES) as usize;
                if length > 0 {
                    if let Err(error) = value.write_at(0, &zero_buffer[..length]) {
                        let reason = format!("{} 清零前 1 MiB 失败：{error}", ordered[index].id);
                        disks[index].fail(reason, 0);
                        self.control.mark_phase(index, DiskPhase::Failed);
                        *backend = None;
                    }
                }
            }
        }

        if self.control.is_cancelled() {
            return self.finish(
                disks,
                started_at,
                Some("写入前已取消".into()),
                true,
            );
        }

        // 并发写。
        let chunk_bytes = self.plan.options.normalized_chunk();
        let control = Arc::clone(&self.control);
        let WritePhase {
            backends,
            results: write_results,
            cancelled,
            complete,
            error: write_error,
        } = write_phase(self.plan.image.as_mut(), backends, chunk_bytes, control);

        // 汇总写入结果。
        let mut verify_backends: Vec<Option<Box<dyn WriteBackend>>> = backends;
        for (index, result) in write_results.into_iter().enumerate() {
            let result = match result {
                Some(value) => value,
                None => continue,
            };
            match result {
                Ok(bytes) => {
                    if complete && !cancelled {
                        disks[index].write = WriteReport::Written { bytes };
                    } else {
                        let reason = if cancelled {
                            "已取消".to_string()
                        } else {
                            write_error
                                .clone()
                                .unwrap_or_else(|| "镜像读取中断".to_string())
                        };
                        disks[index].write = WriteReport::Partial {
                            bytes,
                            reason: reason.clone(),
                        };
                        disks[index].error = Some(reason);
                        self.control.mark_phase(index, DiskPhase::Failed);
                        verify_backends[index] = None;
                    }
                }
                Err(error) => {
                    let reason = error.to_string();
                    disks[index].fail(reason, 0);
                    self.control.mark_phase(index, DiskPhase::Failed);
                    verify_backends[index] = None;
                }
            }
        }

        let mut batch_error = write_error;
        if cancelled {
            batch_error = Some("用户取消，整批已中止".into());
            return self.finish(disks, started_at, batch_error, true);
        }
        if !complete {
            if batch_error.is_none() {
                batch_error = Some("镜像未完整写入".into());
            }
            return self.finish(disks, started_at, batch_error, false);
        }

        // 只有完整写完的盘才进入校验。
        let written_indices: Vec<usize> = disks
            .iter()
            .enumerate()
            .filter(|(_, disk)| matches!(disk.write, WriteReport::Written { .. }))
            .map(|(index, _)| index)
            .collect();
        if written_indices.is_empty() {
            return self.finish(
                disks,
                started_at,
                Some("没有任何盘完成写入".into()),
                false,
            );
        }

        self.control.set_state(BurnState::Verifying);
        for index in &written_indices {
            self.control.mark_phase(*index, DiskPhase::Verifying);
        }
        // 设置里关掉「写入后校验」时直接判定完成，台账记 not_run。
        if !self.plan.options.verify {
            drop(verify_backends);
            for index in &written_indices {
                self.control.mark_phase(*index, DiskPhase::Done);
            }
            return self.finish(disks, started_at, None, false);
        }
        let control = Arc::clone(&self.control);
        let (verify_results, verify_cancelled, verify_error) = verify_phase(
            self.plan.image.as_mut(),
            verify_backends,
            image_size,
            chunk_bytes,
            control,
        );

        for (index, outcome) in verify_results.into_iter().enumerate() {
            let outcome = match outcome {
                Some(value) => value,
                None => continue,
            };
            match outcome {
                VerifyOutcome::Passed => {
                    disks[index].verify = VerifyReport::Passed;
                    self.control.mark_phase(index, DiskPhase::Done);
                }
                VerifyOutcome::Failed { offset, reason } => {
                    disks[index].verify = VerifyReport::Failed {
                        offset,
                        reason: reason.clone(),
                    };
                    disks[index].error = Some(reason);
                    self.control.mark_phase(index, DiskPhase::Failed);
                }
                VerifyOutcome::Cancelled { offset } => {
                    disks[index].verify = VerifyReport::Cancelled { offset };
                    self.control.mark_phase(index, DiskPhase::Failed);
                }
            }
        }

        if verify_cancelled {
            batch_error = Some("校验阶段被取消，整批已中止".into());
        } else if verify_error.is_some() {
            batch_error = verify_error;
        }

        self.finish(disks, started_at, batch_error, verify_cancelled)
    }

    fn emit_batch(&mut self, started_at: &str) -> Result<()> {
        let entry = LedgerEntry::Batch {
            at: started_at.to_string(),
            image_name: self.plan.image_name.clone(),
            image_path: self.plan.image_path.clone(),
            image_sha256: self.plan.image_sha256.clone(),
            image_size: self.plan.image.size(),
            targets: self
                .plan
                .targets
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            excluded: self
                .plan
                .excluded
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
        };
        self.sink.emit(&entry)
    }

    fn emit_disks(&mut self, disks: &[DiskOutcome]) -> Result<()> {
        let at = now_rfc3339();
        for disk in disks {
            let entry = LedgerEntry::Disk {
                at: at.clone(),
                image_name: self.plan.image_name.clone(),
                image_sha256: self.plan.image_sha256.clone(),
                device_id: disk.device.as_str().to_string(),
                device_model: disk.model.clone(),
                letters: disk.letters.clone(),
                capacity_bytes: disk.capacity_bytes,
                write: disk.write.clone(),
                verify: disk.verify.clone(),
            };
            self.sink.emit(&entry)?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        disks: Vec<DiskOutcome>,
        started_at: String,
        batch_error: Option<String>,
        cancelled: bool,
    ) -> Result<BurnOutcome> {
        let failed = disks.iter().any(|disk| {
            matches!(
                disk.write,
                WriteReport::Failed { .. } | WriteReport::Partial { .. }
            ) || matches!(disk.verify, VerifyReport::Failed { .. })
        });
        let all_verified = disks
            .iter()
            .all(|disk| matches!(disk.verify, VerifyReport::Passed | VerifyReport::NotRun));

        let state = if cancelled
            || failed
            || batch_error.is_some()
            || !all_verified
        {
            BurnState::Aborted
        } else {
            BurnState::Completed
        };
        self.control.set_state(state);
        self.emit_disks(&disks)?;
        self.sink.flush()?;

        Ok(BurnOutcome {
            state,
            cancelled,
            batch_error,
            disks,
            started_at,
            finished_at: now_rfc3339(),
        })
    }
}

struct WritePhase {
    backends: Vec<Option<Box<dyn WriteBackend>>>,
    results: Vec<Option<Result<u64>>>,
    cancelled: bool,
    complete: bool,
    error: Option<String>,
}

fn write_phase(
    image: &mut dyn ImageSource,
    backends: Vec<Option<Box<dyn WriteBackend>>>,
    chunk_bytes: usize,
    control: Arc<BurnControl>,
) -> WritePhase {
    let mut results: Vec<Option<Result<u64>>> = backends.iter().map(|_| None).collect();
    let mut idle_backends: Vec<Option<Box<dyn WriteBackend>>> = backends;
    let mut senders: Vec<Option<mpsc::SyncSender<Arc<Vec<u8>>>>> = Vec::new();
    let mut cancelled = false;
    let mut complete = false;
    let mut error: Option<String> = None;

    let mut returned: Vec<Option<Box<dyn WriteBackend>>> =
        idle_backends.iter().map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles: Vec<(usize, thread::ScopedJoinHandle<'_, WriterReturn>)> = Vec::new();
        for (index, slot) in idle_backends.iter_mut().enumerate() {
            match slot.take() {
                Some(backend) => {
                    let (sender, receiver) = mpsc::sync_channel::<Arc<Vec<u8>>>(4);
                    senders.push(Some(sender));
                    let worker_control = Arc::clone(&control);
                    handles.push((
                        index,
                        scope.spawn(move || {
                            writer_worker(index, backend, receiver, worker_control)
                        }),
                    ));
                }
                None => senders.push(None),
            }
        }

        if handles.is_empty() {
            complete = false;
            error = Some("没有可写的设备".into());
            return;
        }

        let mut buffer = vec![0u8; chunk_bytes];
        let mut dispatched: u64 = 0;
        let mut live = senders.iter().filter(|slot| slot.is_some()).count();

        loop {
            if control.is_cancelled() {
                cancelled = true;
                break;
            }
            match image.read(&mut buffer) {
                Ok(0) => {
                    complete = true;
                    break;
                }
                Ok(read) => {
                    dispatched += read as u64;
                    control.set_written(dispatched);
                    let chunk = Arc::new(buffer[..read].to_vec());
                    for slot in senders.iter_mut() {
                        if let Some(sender) = slot {
                            if sender.send(Arc::clone(&chunk)).is_err() {
                                *slot = None;
                                live -= 1;
                            }
                        }
                    }
                    if live == 0 {
                        error = Some("所有盘的写入通道都已断开".into());
                        break;
                    }
                }
                Err(read_error) => {
                    error = Some(format!("读取镜像失败：{read_error}"));
                    break;
                }
            }
        }

        drop(senders);

        for (index, handle) in handles {
            match handle.join() {
                Ok(WriterReturn {
                    index: returned_index,
                    result,
                    backend,
                }) => {
                    results[returned_index] = Some(result);
                    returned[returned_index] = Some(backend);
                    let _ = index;
                }
                Err(_) => {
                    results[index] = Some(Err(BurnError::message("写入线程异常终止")));
                }
            }
        }
    });

    WritePhase {
        backends: returned,
        results,
        cancelled,
        complete,
        error,
    }
}

struct WriterReturn {
    index: usize,
    result: Result<u64>,
    backend: Box<dyn WriteBackend>,
}

fn writer_worker(
    index: usize,
    mut backend: Box<dyn WriteBackend>,
    receiver: mpsc::Receiver<Arc<Vec<u8>>>,
    control: Arc<BurnControl>,
) -> WriterReturn {
    control.mark_phase(index, DiskPhase::Writing);
    let mut offset: u64 = 0;
    let result = loop {
        let chunk = match receiver.recv() {
            Ok(value) => value,
            Err(_) => break Ok(offset),
        };
        if let Err(error) = backend.write_at(offset, &chunk) {
            break Err(error);
        }
        offset += chunk.len() as u64;
        control.add_written(index, chunk.len() as u64);
    };

    let result = match result {
        Ok(bytes) => match backend.flush() {
            Ok(()) => match backend.reread_partition_table() {
                Ok(()) => match backend.close_for_write() {
                    Ok(()) => Ok(bytes),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };

    WriterReturn {
        index,
        result,
        backend,
    }
}

/// 校验阶段的单盘结果。
#[derive(Debug)]
enum VerifyOutcome {
    Passed,
    Failed { offset: u64, reason: String },
    Cancelled { offset: u64 },
}

enum VerifyFailure {
    Mismatch { offset: u64, detail: String },
    Error { offset: u64, error: BurnError },
}

fn verify_phase(
    image: &mut dyn ImageSource,
    backends: Vec<Option<Box<dyn WriteBackend>>>,
    image_size: u64,
    chunk_bytes: usize,
    control: Arc<BurnControl>,
) -> (Vec<Option<VerifyOutcome>>, bool, Option<String>) {
    let mut outcomes: Vec<Option<VerifyOutcome>> = backends.iter().map(|_| None).collect();
    let mut senders: Vec<Option<mpsc::SyncSender<Arc<VerifyChunk>>>> = Vec::new();
    let mut cancelled = false;
    let mut error: Option<String> = None;
    let mut started = false;

    if let Err(rewind_error) = image.rewind() {
        return (
            outcomes,
            false,
            Some(format!("校验前回绕镜像失败：{rewind_error}")),
        );
    }

    thread::scope(|scope| {
        let mut handles: Vec<(usize, thread::ScopedJoinHandle<'_, VerifierReturn>)> = Vec::new();
        let mut owned: Vec<Option<Box<dyn WriteBackend>>> = backends;
        for (index, slot) in owned.iter_mut().enumerate() {
            match slot.take() {
                Some(backend) => {
                    started = true;
                    let (sender, receiver) = mpsc::sync_channel::<Arc<VerifyChunk>>(4);
                    senders.push(Some(sender));
                    handles.push((
                        index,
                        scope.spawn(move || verifier_worker(index, backend, receiver)),
                    ));
                }
                None => senders.push(None),
            }
        }

        if !started {
            return;
        }

        let mut buffer = vec![0u8; chunk_bytes];
        let mut offset: u64 = 0;
        let mut live = senders.iter().filter(|slot| slot.is_some()).count();

        loop {
            if control.is_cancelled() {
                cancelled = true;
                break;
            }
            let read = match image.read(&mut buffer) {
                Ok(0) => break,
                Ok(value) => value,
                Err(read_error) => {
                    error = Some(format!("校验时读取镜像失败：{read_error}"));
                    break;
                }
            };

            let chunk = Arc::new(VerifyChunk {
                offset,
                data: buffer[..read].to_vec(),
            });
            for slot in senders.iter_mut() {
                if let Some(sender) = slot {
                    if sender.send(Arc::clone(&chunk)).is_err() {
                        *slot = None;
                        live -= 1;
                    }
                }
            }
            if live == 0 {
                error = Some("所有盘的校验通道都已断开".into());
                break;
            }
            offset += read as u64;
        }

        drop(senders);

        for (index, handle) in handles {
            match handle.join() {
                Ok(VerifierReturn {
                    index: returned_index,
                    result,
                    verified_bytes,
                }) => {
                    outcomes[returned_index] = Some(match result {
                        Ok(()) => {
                            if verified_bytes >= image_size {
                                VerifyOutcome::Passed
                            } else if cancelled {
                                VerifyOutcome::Cancelled {
                                    offset: verified_bytes,
                                }
                            } else {
                                VerifyOutcome::Failed {
                                    offset: verified_bytes,
                                    reason: "校验未读完镜像".into(),
                                }
                            }
                        }
                        Err(VerifyFailure::Mismatch { offset, detail }) => {
                            VerifyOutcome::Failed { offset, reason: detail }
                        }
                        Err(VerifyFailure::Error { offset, error }) => VerifyOutcome::Failed {
                            offset,
                            reason: format!("回读失败：{error}"),
                        },
                    });
                    let _ = index;
                }
                Err(_) => {
                    outcomes[index] = Some(VerifyOutcome::Failed {
                        offset: 0,
                        reason: "校验线程异常终止".into(),
                    });
                }
            }
        }
    });

    (outcomes, cancelled, error)
}

struct VerifyChunk {
    offset: u64,
    data: Vec<u8>,
}

struct VerifierReturn {
    index: usize,
    result: std::result::Result<(), VerifyFailure>,
    verified_bytes: u64,
}

fn verifier_worker(
    index: usize,
    mut backend: Box<dyn WriteBackend>,
    receiver: mpsc::Receiver<Arc<VerifyChunk>>,
) -> VerifierReturn {
    let mut verified: u64 = 0;
    let mut failure: Option<VerifyFailure> = None;

    while let Ok(chunk) = receiver.recv() {
        let mut buffer = vec![0u8; chunk.data.len()];
        if let Err(error) = backend.read_at(chunk.offset, &mut buffer) {
            failure = Some(VerifyFailure::Error {
                offset: chunk.offset,
                error,
            });
            break;
        }
        if buffer != chunk.data {
            let position = buffer
                .iter()
                .zip(chunk.data.iter())
                .position(|(expected, actual)| expected != actual)
                .unwrap_or(0) as u64;
            let expected = buffer.get(position as usize).copied().unwrap_or(0);
            let actual = chunk.data.get(position as usize).copied().unwrap_or(0);
            failure = Some(VerifyFailure::Mismatch {
                offset: chunk.offset + position,
                detail: format!(
                    "回读校验失败：偏移 {} 处镜像为 0x{:02X}，盘上为 0x{:02X}",
                    chunk.offset + position,
                    actual,
                    expected
                ),
            });
            break;
        }
        verified += chunk.data.len() as u64;
    }

    VerifierReturn {
        index,
        result: match failure {
            None => Ok(()),
            Some(value) => Err(value),
        },
        verified_bytes: verified,
    }
}

fn now_rfc3339() -> String {
    let now = SystemTime::now();
    let datetime: chrono::DateTime<Local> = now.into();
    datetime.to_rfc3339()
}
