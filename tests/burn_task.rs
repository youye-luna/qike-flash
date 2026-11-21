//! `BurnTask` 集成测试：全部使用内存假后端，不接触真实 U 盘。
//!
//! 覆盖验收要求：并发写 5 盘、校验失败、中途取消、单盘失败不影响其他盘，
//! 以及容量预检、占用拒绝、身份变化（TOCTOU）、排除盘零接触、台账内容。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use multi_burn::{
    BackendFactory, BurnError, BurnOptions, BurnPlan, BurnState, DeviceCandidate, DeviceEnumerator,
    DeviceId, DiskState, FileSink, ImageSource, LedgerEntry, MemorySink, OutputSink,
    PartitionTableKind, Result, VerifyReport, WriteBackend, WriteReport,
};

const MIB: u64 = 1024 * 1024;

// ---------------------------------------------------------------- 假镜像

struct VecImage {
    data: Arc<Vec<u8>>,
    position: usize,
    rewind_fails: bool,
}

impl VecImage {
    fn boxed(size: usize) -> Box<dyn ImageSource> {
        Box::new(Self {
            data: Arc::new(pattern(size)),
            position: 0,
            rewind_fails: false,
        })
    }
}

impl ImageSource for VecImage {
    fn name(&self) -> &str {
        "fake.img"
    }

    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let remaining = &self.data[self.position..];
        let count = remaining.len().min(buffer.len());
        buffer[..count].copy_from_slice(&remaining[..count]);
        self.position += count;
        Ok(count)
    }

    fn rewind(&mut self) -> Result<()> {
        if self.rewind_fails {
            return Err(BurnError::message("模拟回绕失败"));
        }
        self.position = 0;
        Ok(())
    }
}

fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

// ---------------------------------------------------------------- 假设备

struct DiskSim {
    capacity: u64,
    media: Mutex<Vec<u8>>,
    writes: AtomicUsize,
    reads: AtomicUsize,
    /// 写入偏移达到该值时失败。
    fail_write_at: Option<u64>,
    /// 回读在该绝对偏移处返回被翻转的字节。
    corrupt_read_at: Option<u64>,
    busy: bool,
    /// 每次 write_at 的模拟耗时，用于制造“写入中途取消”的窗口。
    delay: Duration,
}

impl DiskSim {
    fn new(capacity: u64) -> Self {
        Self {
            capacity,
            media: Mutex::new(vec![0xAA; capacity as usize]),
            writes: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            fail_write_at: None,
            corrupt_read_at: None,
            busy: false,
            delay: Duration::ZERO,
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        match self.media.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

struct FakeBackend {
    sim: Arc<DiskSim>,
}

impl WriteBackend for FakeBackend {
    fn capacity_bytes(&self) -> u64 {
        self.sim.capacity
    }

    fn is_busy(&self) -> Result<bool> {
        Ok(self.sim.busy)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        if let Some(limit) = self.sim.fail_write_at {
            if offset >= limit {
                return Err(BurnError::message("模拟写入失败"));
            }
        }
        let end = offset as usize + data.len();
        {
            let mut media = match self.sim.media.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if end > media.len() {
                return Err(BurnError::message("模拟设备越界写入"));
            }
            media[offset as usize..end].copy_from_slice(data);
        }
        self.sim.writes.fetch_add(1, Ordering::SeqCst);
        if !self.sim.delay.is_zero() {
            thread::sleep(self.sim.delay);
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let end = offset as usize + buffer.len();
        {
            let media = match self.sim.media.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if end > media.len() {
                return Err(BurnError::message("模拟设备越界读取"));
            }
            buffer.copy_from_slice(&media[offset as usize..end]);
        }
        if let Some(position) = self.sim.corrupt_read_at {
            let position = position as usize;
            if position >= offset as usize && position < end {
                buffer[position - offset as usize] ^= 0xFF;
            }
        }
        self.sim.reads.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    fn reread_partition_table(&mut self) -> Result<()> {
        Ok(())
    }

    fn close_for_write(&mut self) -> Result<()> {
        Ok(())
    }
}

struct FakeFactory {
    sims: HashMap<String, Arc<DiskSim>>,
    opened: Mutex<Vec<String>>,
}

impl FakeFactory {
    fn new(sims: &[(DeviceId, Arc<DiskSim>)]) -> Self {
        Self {
            sims: sims
                .iter()
                .map(|(id, sim)| (id.as_str().to_string(), Arc::clone(sim)))
                .collect(),
            opened: Mutex::new(Vec::new()),
        }
    }

    fn opened_ids(&self) -> Vec<String> {
        match self.opened.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl BackendFactory for FakeFactory {
    fn open_exclusive(&self, device: &DeviceId) -> Result<Box<dyn WriteBackend>> {
        let sim = self
            .sims
            .get(device.as_str())
            .ok_or_else(|| BurnError::message("未知设备"))?;
        match self.opened.lock() {
            Ok(mut guard) => guard.push(device.as_str().to_string()),
            Err(poisoned) => poisoned.into_inner().push(device.as_str().to_string()),
        }
        Ok(Box::new(FakeBackend {
            sim: Arc::clone(sim),
        }))
    }
}

struct FakeEnumerator {
    candidates: Vec<DeviceCandidate>,
    /// 模拟确认后消失的设备（D4）。
    missing: Mutex<HashSet<String>>,
    resolved: Mutex<Vec<String>>,
}

impl FakeEnumerator {
    fn new(candidates: Vec<DeviceCandidate>) -> Self {
        Self {
            candidates,
            missing: Mutex::new(HashSet::new()),
            resolved: Mutex::new(Vec::new()),
        }
    }

    fn resolved_ids(&self) -> Vec<String> {
        match self.resolved.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl DeviceEnumerator for FakeEnumerator {
    fn enumerate(&self) -> Result<Vec<DeviceCandidate>> {
        Ok(self.candidates.clone())
    }

    fn resolve(&self, ids: &[DeviceId]) -> Result<Vec<DeviceCandidate>> {
        let missing = match self.missing.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let mut resolved = match self.resolved.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for id in ids {
            resolved.push(id.as_str().to_string());
        }
        drop(resolved);

        Ok(self
            .candidates
            .iter()
            .filter(|candidate| {
                ids.iter().any(|id| id == &candidate.id)
                    && !missing.contains(candidate.id.as_str())
            })
            .cloned()
            .collect())
    }
}

// ---------------------------------------------------------------- 工具

fn disk(index: u32, capacity: u64) -> DeviceId {
    DeviceId::derive(
        &format!("SN-{index}"),
        capacity,
        &format!(r"\\?\usbstor#disk#{index}"),
    )
}

fn candidate(id: &DeviceId, capacity: u64) -> DeviceCandidate {
    DeviceCandidate {
        id: id.clone(),
        capacity_bytes: capacity,
        bus: "USB".to_string(),
        model: "Test Vendor Test Model".to_string(),
        removable: true,
        is_system: false,
        state: DiskState::Empty,
        table: PartitionTableKind::Unknown,
        partitions: Vec::new(),
        volumes: Vec::new(),
        entropy: Some(0.0),
    }
}

fn options() -> BurnOptions {
    BurnOptions {
        chunk_bytes: 64 * 1024,
        verify: true,
    }
}

fn plan(
    image_size: usize,
    targets: Vec<DeviceId>,
    excluded: Vec<DeviceId>,
) -> BurnPlan {
    BurnPlan::new(
        VecImage::boxed(image_size),
        "fake.img".to_string(),
        "C:\\images\\fake.img".to_string(),
        "00".repeat(32),
        targets,
        excluded,
        options(),
    )
}

/// 校验阶段需要回绕镜像，所以镜像尺寸要完整写完后才能进入校验。
fn assert_written_then_zeroed(media: &[u8], image: &[u8], capacity: u64) {
    assert_eq!(&media[..image.len()], image, "镜像内容不一致");
    let zero_end = (MIB as usize).min(capacity as usize);
    assert!(
        media[image.len()..zero_end].iter().all(|byte| *byte == 0),
        "前 1 MiB 未被清零"
    );
    assert!(
        media[zero_end..].iter().all(|byte| *byte == 0xAA),
        "清零范围越过 1 MiB"
    );
}

// ---------------------------------------------------------------- 测试

#[test]
fn concurrent_write_to_five_disks_completes_and_verifies() {
    let image_size = 640 * 1024;
    let image = pattern(image_size);
    let capacity = 2 * MIB;

    let mut ids = Vec::new();
    let mut sims = Vec::new();
    for index in 0..5u32 {
        let id = disk(index, capacity);
        sims.push((id.clone(), Arc::new(DiskSim::new(capacity))));
        ids.push(id);
    }
    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();

    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, ids.clone(), Vec::new()),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("批量写入应当返回结果")
    };

    assert_eq!(outcome.state, BurnState::Completed);
    assert!(!outcome.cancelled);
    assert!(outcome.batch_error.is_none());
    assert_eq!(outcome.disks.len(), 5);
    for (out_going, (id, sim)) in outcome.disks.iter().zip(&sims) {
        assert_eq!(&out_going.device, id);
        assert!(matches!(
            out_going.write,
            WriteReport::Written { bytes } if bytes == image_size as u64
        ));
        assert!(matches!(out_going.verify, VerifyReport::Passed));
        assert_written_then_zeroed(&sim.snapshot(), &image, capacity);
    }

    assert_eq!(sink.entries.len(), 6, "1 条批次 + 5 条单盘");
    match &sink.entries[0] {
        LedgerEntry::Batch {
            targets, excluded, ..
        } => {
            assert_eq!(targets.len(), 5);
            assert!(excluded.is_empty());
        }
        other => panic!("首条台账应为批次，实际 {other:?}"),
    }
}

#[test]
fn verify_mismatch_fails_that_disk_and_aborts_batch() {
    let image_size = 256 * 1024;
    let capacity = 2 * MIB;
    let ids: Vec<DeviceId> = (0..3u32).map(|index| disk(index, capacity)).collect();

    let mut sims = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let mut sim = DiskSim::new(capacity);
        if index == 1 {
            // 第 2 块盘的回读在偏移 1000 处返回错误字节。
            sim.corrupt_read_at = Some(1000);
        }
        sims.push((id.clone(), Arc::new(sim)));
    }

    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, ids.clone(), Vec::new()),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("批量写入应当返回结果")
    };

    assert_eq!(outcome.state, BurnState::Aborted);
    assert!(matches!(
        outcome.disks[1].verify,
        VerifyReport::Failed { offset: 1000, .. }
    ));
    assert!(outcome.disks[1].error.is_some());
    assert!(matches!(outcome.disks[0].verify, VerifyReport::Passed));
    assert!(matches!(outcome.disks[2].verify, VerifyReport::Passed));
}

#[test]
fn single_disk_write_failure_does_not_block_others() {
    let image_size = 512 * 1024;
    let capacity = 2 * MIB;
    let ids: Vec<DeviceId> = (0..4u32).map(|index| disk(index, capacity)).collect();

    let mut sims = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let mut sim = DiskSim::new(capacity);
        if index == 2 {
            sim.fail_write_at = Some(128 * 1024);
        }
        sims.push((id.clone(), Arc::new(sim)));
    }

    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, ids.clone(), Vec::new()),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("批量写入应当返回结果")
    };

    assert_eq!(outcome.state, BurnState::Aborted, "有盘失败时整批必须中止");
    assert!(
        matches!(
            outcome.disks[2].write,
            WriteReport::Failed { .. } | WriteReport::Partial { .. }
        ),
        "失败盘必须可见"
    );
    for (index, disk_outcome) in outcome.disks.iter().enumerate() {
        if index == 2 {
            continue;
        }
        assert!(matches!(disk_outcome.write, WriteReport::Written { .. }));
        assert!(matches!(disk_outcome.verify, VerifyReport::Passed));
        assert_written_then_zeroed(
            &sims[index].1.snapshot(),
            &pattern(image_size),
            capacity,
        );
    }
}

#[test]
fn cancel_midway_aborts_whole_batch() {
    let image_size = 2 * 1024 * 1024;
    let capacity = 4 * MIB;
    let ids: Vec<DeviceId> = (0..3u32).map(|index| disk(index, capacity)).collect();

    let mut sims = Vec::new();
    for id in &ids {
        let mut sim = DiskSim::new(capacity);
        sim.delay = Duration::from_millis(8);
        sims.push((id.clone(), Arc::new(sim)));
    }

    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let mut task = multi_burn::BurnTask::new(
        plan(image_size, ids.clone(), Vec::new()),
        &enumerator,
        &factory,
        &mut sink,
    );
    let control = task.control();

    let outcome = thread::scope(|scope| {
        let runner = scope.spawn(move || task.run());
        // 等到真正进入写入阶段再取消，避免在预检阶段提前中止。
        let deadline = Instant::now() + Duration::from_secs(5);
        while control.state() != BurnState::Writing && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        // 清零前 1 MiB 会先占用约 24ms，再等 100ms 确保处于写入中途。
        thread::sleep(Duration::from_millis(100));
        control.cancel();
        match runner.join() {
            Ok(result) => result.expect("取消后仍应返回结果"),
            Err(_) => panic!("写入线程异常"),
        }
    });

    assert!(outcome.cancelled);
    assert_eq!(outcome.state, BurnState::Aborted);
    assert!(outcome.batch_error.is_some());
    assert_eq!(outcome.disks.len(), 3);
    assert!(
        outcome
            .disks
            .iter()
            .any(|disk| matches!(disk.write, WriteReport::Partial { .. })),
        "应当有盘在写入中途被打断"
    );
    for disk_outcome in &outcome.disks {
        assert!(
            !matches!(disk_outcome.write, WriteReport::Written { .. }),
            "取消后不允许出现完成态：{:?}",
            disk_outcome.write
        );
        assert!(!matches!(disk_outcome.verify, VerifyReport::Passed));
    }
}

#[test]
fn capacity_shortage_rejects_whole_batch() {
    let image_size = 900 * 1024;
    let targets: Vec<DeviceId> = (0..4u32).map(|index| disk(index, 2 * MIB)).collect();
    let small = disk(9, 512 * 1024);
    let mut all = targets.clone();
    all.push(small.clone());

    let mut sims = Vec::new();
    for id in &targets {
        sims.push((id.clone(), Arc::new(DiskSim::new(2 * MIB))));
    }
    sims.push((small.clone(), Arc::new(DiskSim::new(512 * 1024))));

    let candidates = all
        .iter()
        .map(|id| candidate(id, if id == &small { 512 * 1024 } else { 2 * MIB }))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, all.clone(), Vec::new()),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("容量不足应返回结果而非崩溃")
    };

    assert_eq!(outcome.state, BurnState::Aborted);
    let batch_error = outcome.batch_error.clone().unwrap_or_default();
    assert!(batch_error.contains("容量不足"), "错误信息应说明容量不足");
    assert!(factory.opened_ids().is_empty(), "预检失败不得打开任何盘");
    for (id, sim) in &sims {
        let media = sim.snapshot();
        assert!(media.iter().all(|byte| *byte == 0xAA), "{id} 不应被写入");
        assert_eq!(sim.writes.load(Ordering::SeqCst), 0);
    }
    assert!(
        matches!(outcome.disks.last().map(|disk| &disk.write), Some(WriteReport::Failed { .. })),
        "容量不足的盘必须标记失败"
    );
}

#[test]
fn busy_device_blocks_batch_before_writing() {
    let image_size = 256 * 1024;
    let capacity = 2 * MIB;
    let ids: Vec<DeviceId> = (0..3u32).map(|index| disk(index, capacity)).collect();

    let mut sims = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let mut sim = DiskSim::new(capacity);
        sim.busy = index == 0;
        sims.push((id.clone(), Arc::new(sim)));
    }

    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, ids.clone(), Vec::new()),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("占用应当返回结果而非崩溃")
    };

    assert_eq!(outcome.state, BurnState::Aborted);
    for (_, sim) in &sims {
        assert_eq!(sim.writes.load(Ordering::SeqCst), 0, "占用时不得写入任何盘");
    }
}

#[test]
fn identity_change_after_confirmation_is_rejected() {
    let image_size = 256 * 1024;
    let capacity = 2 * MIB;
    let ids: Vec<DeviceId> = (0..3u32).map(|index| disk(index, capacity)).collect();

    let mut sims = Vec::new();
    for id in &ids {
        sims.push((id.clone(), Arc::new(DiskSim::new(capacity))));
    }

    let candidates = ids
        .iter()
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    match enumerator.missing.lock() {
        Ok(mut guard) => {
            guard.insert(ids[1].as_str().to_string());
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(ids[1].as_str().to_string());
        }
    }
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let mut task = multi_burn::BurnTask::new(
        plan(image_size, ids.clone(), Vec::new()),
        &enumerator,
        &factory,
        &mut sink,
    );
    let result = task.run();

    assert!(result.is_err(), "设备消失后必须中止");
    assert_eq!(task.control().state(), BurnState::Aborted);
    assert!(factory.opened_ids().is_empty(), "身份变化时不得打开设备");
}

#[test]
fn excluded_device_is_never_probed_or_opened() {
    let image_size = 256 * 1024;
    let capacity = 2 * MIB;
    let targets: Vec<DeviceId> = (0..3u32).map(|index| disk(index, capacity)).collect();
    let excluded = disk(7, capacity);

    let mut sims = Vec::new();
    for id in targets.iter().chain(std::iter::once(&excluded)) {
        sims.push((id.clone(), Arc::new(DiskSim::new(capacity))));
    }

    let candidates = targets
        .iter()
        .chain(std::iter::once(&excluded))
        .map(|id| candidate(id, capacity))
        .collect::<Vec<_>>();
    let enumerator = FakeEnumerator::new(candidates);
    let factory = FakeFactory::new(&sims);
    let mut sink = MemorySink::default();

    let outcome = {
        let mut task = multi_burn::BurnTask::new(
            plan(image_size, targets.clone(), vec![excluded.clone()]),
            &enumerator,
            &factory,
            &mut sink,
        );
        task.run().expect("排除后仍应正常写入其余盘")
    };

    assert_eq!(outcome.state, BurnState::Completed);
    assert!(
        !enumerator
            .resolved_ids()
            .contains(&excluded.as_str().to_string()),
        "D3：已排除的盘不能被 resolve"
    );
    assert!(
        !factory
            .opened_ids()
            .contains(&excluded.as_str().to_string()),
        "D3：已排除的盘不能被打开"
    );
    if let Some((_, sim)) = sims.last() {
        assert_eq!(sim.writes.load(Ordering::SeqCst), 0);
        assert_eq!(sim.reads.load(Ordering::SeqCst), 0);
        assert!(sim.snapshot().iter().all(|byte| *byte == 0xAA));
    }

    match &sink.entries[0] {
        LedgerEntry::Batch { excluded: logged, .. } => {
            assert_eq!(logged, &vec![excluded.as_str().to_string()], "D10：排除项必须进台账");
        }
        other => panic!("首条台账应为批次，实际 {other:?}"),
    }
}

#[test]
fn file_sink_appends_one_json_object_per_line() {
    let base = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
        .join("multi-burn-ledger-test");
    let path = base.join("ledger.jsonl");
    let _ = std::fs::remove_file(&path);

    {
        let mut sink = FileSink::create(&path).expect("创建台账失败");
        let entry = LedgerEntry::Batch {
            at: "2026-01-01T00:00:00+08:00".to_string(),
            image_name: "fake.img".to_string(),
            image_path: "C:\\images\\fake.img".to_string(),
            image_sha256: "00".repeat(32),
            image_size: 1024,
            targets: vec!["dev:aaaa".to_string()],
            excluded: vec!["dev:bbbb".to_string()],
        };
        sink.emit(&entry).expect("写台账失败");
        sink.flush().expect("刷新台账失败");
    }

    let text = std::fs::read_to_string(&path).expect("读取台账失败");
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    let value: serde_json::Value = serde_json::from_str(lines[0]).expect("台账必须是合法 JSON");
    assert_eq!(value["kind"], "batch");
    assert_eq!(value["excluded"][0], "dev:bbbb");
    let _ = std::fs::remove_file(&path);
}
