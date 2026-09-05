//! 蓝图 68 卷 68.4（WP-68.4.1/68.4.2）：svc-telemetry OBS 事件面 + 快照面。
//!
//! 链路（三项裁决见 68.4 执行卡「执行记录」）：EIPC 块命令面 →
//! [`eneros_storage::partition::CachedBlocks`]（4 块直接映射读缓存——
//! BlockEventStore 逐记录扫描的块命令成本收敛）→
//! [`eneros_storage::partition::RegionScope`]（DATA_EVENT_* 区段钳制，
//! 类型门）→ `BlockEventStore`（event-store 单源）→
//! `StoreSink`（SS 解冻：OBS 事件 Q0 写透追加，ACK 即 durable）。
//!
//! 快照面：SnapshotModule（SM 解冻）以 catalog（svc-config 惰性装载）、
//! 投影与水位构建 `TrustedSnapshot`；blob 经快照区段 LFS 卷持久化
//! （DATA_SNAP_* 区段，`/snaps/<id>.snap`，tmp+rename 原子写），
//! 保留最新 [`SNAP_KEEP_BLOBS`] 份。
//!
//! 诚实性纪律（与 telemetry_history 同律）：
//! - 任一环失败 → 显式降级（EVENT-STORE-UNAVAILABLE / SNAP-SKIP 词形），
//!   不伪造持久化；事件面失败**绝不反向影响 ingest ack**；
//! - 生产 boot **不整卷重放**（逐记录块命令 10³-10⁴ 级，68.3 停顿教训）
//!   ——水位 = 活性会话基线（快照 freshness 窗 ≥600s，boot 缺口不跨界）；
//!   重放→水位一致的机制锁在 t4 host 测试（open 扫描的
//!   `EVENT-REPLAY` 词形是**扫描恢复摘要**，非全量重放）；
//! - lazy-open：构造期零 I/O，首个 OBS 批次触发打开（REG-BOOT-SLOW-FINAL
//!   在案的 boot 预算不再翻倍）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::block_io::{BlockIo, EipcBlockDevice, EipcSend};
use eneros_event_store::block::BlockEventStore;
use eneros_event_store::ingest_bridge::StoreSink;
use eneros_event_store::scoped::open_region_scoped;
use eneros_event_store::{EventStore, StoreConfig, WatermarkReport};
use eneros_fs::{FileSystem, Lfs};
use eneros_ingestion_core::ObservationSink;
use eneros_ota_ab::ab_partition::DataRegion;
use eneros_semantic_catalog::{CatalogRepository, SemanticCatalog};
use eneros_storage::partition::{CachedBlocks, RegionScope};
use eneros_telemetry_model::event::ObservationEvent;

/// 事件面/事实面共用 StoreConfig（68.4 执行卡裁决 2：segment 16KiB =
/// 4×4KiB 块/段；128KiB 区段 → (32-3)/4 = 7 槽；Q0 写透批量 1）。
pub fn event_store_config() -> StoreConfig {
    StoreConfig {
        segment_bytes: 16_384,
        batch_records: 64,
        q0_batch_records: 1,
        reject_q2q3_at_95_percent: false,
        clock_unix_s: || 0,
    }
}

/// 快照构建窗口（OBS 批数；与 TSDB checkpoint 合并窗同节奏——窗口批
/// 已承担秒级磁盘成本，快照构建+落盘叠加其上，轻批保持零磁盘）。
pub const SNAP_EVERY_BATCHES: u64 = 8;

/// 快照 blob 保留份数（超出删最老 produced_at；.tmp 残留与坏 blob 一并清）。
pub const SNAP_KEEP_BLOBS: usize = 16;

/// 保留触发水位（全局 permille；≥850 删全部**完整段**——开放尾段由
/// store 语义保护绝不删，滚动窗 ≈ 开放段。DELIVERY 位点检查属调用方
/// 职责：生产本卷无 drain 消费者，见执行卡"RPO=0 语义分层"）。
const RETAIN_PERMILLE: u32 = 850;

/// 快照 purpose（本卷唯一政策；真实 per-purpose 政策集归 certifier 卷）。
pub const SNAP_PURPOSE_ID: u32 = 1;

/// 快照面 LFS 数据目录。
const SNAP_DIR: &str = "/snaps";

/// 事件面设备栈（CachedBlocks 垫在 RegionScope 之下——缓存键 = 盘绝对
/// 块号，顺序扫描每块只取一次）。
type FaceDev<C> = RegionScope<CachedBlocks<4, EipcBlockDevice<C>>>;
type FaceStore<C> = BlockEventStore<FaceDev<C>>;

/// OBS 事件面接缝（svc-telemetry 持有；host 可测）。
pub trait ObsEventFace {
    /// 一轮 OBS 接受集交付（追加 + 投影 + 窗口快照；失败只打点）。
    fn on_obs_batch(&mut self, events: &[ObservationEvent], now_ms: u64);
    /// catalog 到达（ensure_source_identity 惰性装载）——构建
    /// SnapshotModule 与快照政策（重复调用以最新 catalog 为准；政策
    /// 版本/目录版本自 repo 单源，无外部 generation 入参）。
    fn set_snapshot_catalog(&mut self, repo: CatalogRepository, now_ms: u64);
}

/// 打开后的内面（sink + 可选模块 + 快照 LFS 卷）。
struct OpenFace<C: EipcSend + Clone + 'static> {
    sink: StoreSink<FaceStore<C>>,
    /// 快照 LFS 卷（None = 卷打开失败——SNAP-SKIP store，事件面不受累）。
    snap_fs: Option<Lfs>,
    module: Option<SnapshotCore>,
}

/// 模块 + 快照窗口状态（catalog 到达后才有——legacy bundle 如实降级）。
struct SnapshotCore {
    module: eneros_snapshot_service::SnapshotModule,
    /// 距上次快照构建的批数（窗口计数）。
    batches_since_snap: u64,
}

/// 事件面装配体（svc-telemetry bin 持有；构造零 I/O）。
pub struct EventFace<C: EipcSend + Clone + 'static> {
    /// 事件面/快照面块通道（未打开时 Some——打开消耗；两通道独立，
    /// 任一失败不影响另一环的显式降级词形）。
    io_event: Option<BlockIo<C>>,
    io_snap: Option<BlockIo<C>>,
    event_base_512: u64,
    event_blocks_512: u64,
    snap_base_512: u64,
    snap_blocks_512: u64,
    state: FaceState<C>,
    /// catalog 暂存（打开早于 catalog 到达时，打开完成后补建模块）。
    pending_catalog: Option<CatalogRepository>,
    // —— 计数（可观测，不静默）——
    /// 已追加（ACK 即 durable）事件数。
    pub appended: u64,
    /// 追加失败数（store Err / tracker 拒）。
    pub append_failures: u64,
    /// 保留删除的完整段数。
    pub pruned_segments: u64,
    /// 已构建并落盘的快照数。
    pub snap_stored: u64,
    /// 快照构建/落盘跳过数（SNAP-SKIP）。
    pub snap_skipped: u64,
}

enum FaceState<C: EipcSend + Clone + 'static> {
    /// 未打开（构造期/lazy）。
    Idle,
    /// 打开失败（显式降级；后续批次零开销直通）。
    Failed,
    /// Box：Open 面远大于 Idle/Failed（枚举差值不应常驻栈帧）。
    Open(alloc::boxed::Box<OpenFace<C>>),
}

impl<C: EipcSend + Clone + 'static> EventFace<C> {
    /// 装配（块通道 ×2 + 两个区段角色——ota-ab `DataRegion` 句柄，基址/
    /// 长度经唯一 accessor 换算，消灭裸 u64 错位面）。构造期零 I/O。
    pub fn new(
        io_event: BlockIo<C>,
        io_snap: BlockIo<C>,
        event_region: DataRegion,
        snap_region: DataRegion,
    ) -> Self {
        Self {
            io_event: Some(io_event),
            io_snap: Some(io_snap),
            event_base_512: event_region.base_block_512(),
            event_blocks_512: event_region.blocks_512(),
            snap_base_512: snap_region.base_block_512(),
            snap_blocks_512: snap_region.blocks_512(),
            state: FaceState::Idle,
            pending_catalog: None,
            appended: 0,
            append_failures: 0,
            pruned_segments: 0,
            snap_stored: 0,
            snap_skipped: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, FaceState::Open(_))
    }

    /// 首个 OBS 批次触发：事件面 open-or-format + 快照卷 mount-or-format
    /// + 快照 blob 目录恢复。事件面失败 → Failed（一次性词形，不刷屏）。
    fn ensure_open(&mut self, now_ms: u64) {
        let (Some(io_event), Some(io_snap)) = (self.io_event.take(), self.io_snap.take()) else {
            self.state = FaceState::Failed;
            return;
        };
        eneros_runtime::println!(
            "[svc-telemetry] EVENT-STORE-OPEN base_block={} region_blocks_512={}",
            self.event_base_512,
            self.event_blocks_512
        );
        let face = EipcBlockDevice::new(
            io_event,
            eneros_event_store::format::BLOCK_SIZE,
            eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS / 8,
        );
        let base_4k = self.event_base_512 / 8;
        let blocks_4k = self.event_blocks_512 / 8;
        let cfg = event_store_config();
        let slots = (blocks_4k - 3) / (cfg.segment_bytes as u64 / 4096);
        let open = open_region_scoped(
            || CachedBlocks::<4, _>::new(face.clone()),
            base_4k,
            blocks_4k,
            cfg,
        );
        let (store, report) = match open {
            Ok(pair) => pair,
            Err(_) => {
                eneros_runtime::println!("[svc-telemetry] EVENT-STORE-UNAVAILABLE");
                self.state = FaceState::Failed;
                return;
            }
        };
        let consistent = report.bad_segments == 0 && report.offset_gaps == 0;
        eneros_runtime::println!(
            "[svc-telemetry] EVENT-REPLAY recovered={} consistent={} slots={}",
            report.valid_records,
            consistent,
            slots
        );
        // 快照卷：mount-or-format（telemetry_history 同律）。
        let mut snap_fs = open_snap_volume(io_snap, self.snap_base_512, self.snap_blocks_512);
        match snap_fs.as_mut() {
            Some(fs) => {
                let blob_count = count_snap_blobs(fs);
                eneros_runtime::println!("[svc-telemetry] SNAP-RECOVERED blobs={}", blob_count);
            }
            None => {
                eneros_runtime::println!("[svc-telemetry] SNAP-STORE-UNAVAILABLE");
            }
        }
        // 模块：catalog 已先到（少见——首个 OBS 前 set_catalog）则建，
        // 否则挂 pending（catalog 到达时建）。
        let module = match self.pending_catalog.take() {
            Some(repo) => {
                let (core, required) = build_module(repo, now_ms);
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-MODULE-READY purpose={} required={}",
                    SNAP_PURPOSE_ID,
                    required
                );
                Some(core)
            }
            None => None,
        };
        self.state = FaceState::Open(alloc::boxed::Box::new(OpenFace {
            sink: StoreSink::new(store),
            snap_fs,
            module,
        }));
    }
}

impl<C: EipcSend + Clone + 'static> ObsEventFace for EventFace<C> {
    fn on_obs_batch(&mut self, events: &[ObservationEvent], now_ms: u64) {
        if matches!(self.state, FaceState::Idle) {
            self.ensure_open(now_ms);
        }
        let FaceState::Open(face_box) = &mut self.state else {
            return; // Failed 或未装配 io——直通（绝不影响 ack）。
        };
        let face = &mut **face_box;
        let mut ok = 0u64;
        let mut fail = 0u64;
        for ev in events {
            match face.sink.append(ev) {
                Ok(_) => {
                    ok += 1;
                    self.appended += 1;
                    if let Some(core) = face.module.as_mut() {
                        core.module.apply_event(*ev);
                    }
                }
                Err(_) => {
                    fail += 1;
                    self.append_failures += 1;
                }
            }
        }
        if ok > 0 || fail > 0 {
            eneros_runtime::println!("[svc-telemetry] EVENT-STORE append n={} fail={}", ok, fail);
        }
        // 窗口判定（模块借用域最小化——构建与落盘分步借用）。
        let due = {
            let Some(core) = face.module.as_mut() else {
                return; // catalog 未到（legacy bundle）——如实降级，无快照。
            };
            core.batches_since_snap += 1;
            let due = core.batches_since_snap >= SNAP_EVERY_BATCHES;
            if due {
                core.batches_since_snap = 0;
            }
            due
        };
        if due {
            // 字段级拆借：face（self.state）与计数器字段不相交。
            Self::window_tasks(
                face,
                &mut self.pruned_segments,
                &mut self.snap_stored,
                &mut self.snap_skipped,
                now_ms,
            );
        }
    }

    fn set_snapshot_catalog(&mut self, repo: CatalogRepository, now_ms: u64) {
        match &mut self.state {
            FaceState::Open(face) => {
                let (core, required) = build_module(repo, now_ms);
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-MODULE-READY purpose={} required={}",
                    SNAP_PURPOSE_ID,
                    required
                );
                face.module = Some(core);
            }
            FaceState::Idle => {
                self.pending_catalog = Some(repo);
            }
            FaceState::Failed => {}
        }
    }
}

impl<C: EipcSend + Clone + 'static> EventFace<C> {
    /// 窗口批任务：保留修剪 → 快照构建 → blob 落盘。失败逐级显式
    /// （SNAP-SKIP reason），不伪装成功。借用序：sink（修剪）→ module
    /// （构建，产出 owned 快照）→ snap_fs（落盘）。计数器以字段指针
    /// 传入（face 已借走 self.state——拆借同一 self 的不相交字段）。
    fn window_tasks(
        face: &mut OpenFace<C>,
        pruned_segments: &mut u64,
        snap_stored: &mut u64,
        snap_skipped: &mut u64,
        now_ms: u64,
    ) {
        // 1. 保留：全局水位 ≥850‰ → 删全部完整段。
        let wm: WatermarkReport = face.sink.store_mut().watermarks();
        if wm.used_permille >= RETAIN_PERMILLE {
            let partitions: Vec<_> = face.sink.store_mut().partitions().into_iter().collect();
            for p in partitions {
                match face
                    .sink
                    .store_mut()
                    .delete_full_segments_before(p, u64::MAX)
                {
                    Ok(deleted) if !deleted.is_empty() => {
                        *pruned_segments += deleted.len() as u64;
                        eneros_runtime::println!(
                            "[svc-telemetry] EVENT-PRUNE n={} partition={}",
                            deleted.len(),
                            p
                        );
                    }
                    Ok(_) => {}
                    Err(_) => {
                        eneros_runtime::println!(
                            "[svc-telemetry] EVENT-PRUNE-FAIL partition={}",
                            p
                        );
                    }
                }
            }
        }
        // 2. 快照构建（owned 产出后立刻释放 module 借用）。
        let request = eneros_snapshot_service::SnapshotRequest {
            tenant_id: 1,
            site_id: 1,
            purpose_id: SNAP_PURPOSE_ID,
            as_of_ns: now_ms.saturating_mul(1_000_000),
            max_wait_ns: 0,
            policy_version: 1,
        };
        let now_ns = now_ms.saturating_mul(1_000_000);
        let built = face
            .module
            .as_mut()
            .map(|core| core.module.build(request, now_ns));
        let snap = match built {
            Some(Ok(snap)) => Some(snap),
            Some(Err(e)) => {
                *snap_skipped += 1;
                eneros_runtime::println!("[svc-telemetry] SNAP-SKIP {}", e);
                None
            }
            None => None,
        };
        let Some(snap) = snap else { return };
        let bytes = eneros_snapshot_service::encode_snapshot_v1(&snap);
        eneros_runtime::println!(
            "[svc-telemetry] SNAP-BUILT pts={} bytes={}",
            snap.observations.len(),
            bytes.len()
        );
        match face.snap_fs.as_mut() {
            Some(fs) => match store_snap_blob(fs, &snap, &bytes) {
                Ok(kept) => {
                    *snap_stored += 1;
                    eneros_runtime::println!("[svc-telemetry] SNAP-STORED kept={}", kept);
                }
                Err(()) => {
                    *snap_skipped += 1;
                    eneros_runtime::println!("[svc-telemetry] SNAP-SKIP store-fail");
                }
            },
            None => {
                *snap_skipped += 1;
                eneros_runtime::println!("[svc-telemetry] SNAP-SKIP store-unavailable");
            }
        }
    }
}

// ============================================================================
// 快照卷（LFS）与 blob 存档
// ============================================================================

/// 打开（或首次格式化后打开）快照 LFS 卷（telemetry_history 的
/// mount-or-format 同律；卷几何 = min(区段 4KiB 块数, 64)——
/// BlockDeviceStorage BLOCK_COUNT=64 编译期上限）。
fn open_snap_volume<C: EipcSend + Clone + 'static>(
    io: BlockIo<C>,
    base_block_512: u64,
    region_blocks_512: u64,
) -> Option<Lfs> {
    const FS_BLOCK_SIZE: usize = 4096;
    assert!(
        base_block_512 % 8 == 0 && (region_blocks_512 * 512) % FS_BLOCK_SIZE as u64 == 0,
        "快照区段基址/长度必须与 LFS 4096B 几何对齐"
    );
    let face = EipcBlockDevice::new(
        io,
        FS_BLOCK_SIZE,
        eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS / 8,
    );
    let base_4k = base_block_512 / 8;
    let blocks_4k = (region_blocks_512 / 8).min(64);
    let scope = RegionScope::new(face.clone(), base_4k, blocks_4k).ok()?;
    match Lfs::mount(alloc::boxed::Box::new(scope)) {
        Ok(fs) => Some(fs),
        Err(eneros_fs::FsError::BadSuperBlock) => {
            // 首次上电：显式格式化一次（fresh face；仅让渡区段内）。
            let scope2 = RegionScope::new(face, base_4k, blocks_4k).ok()?;
            Lfs::format(alloc::boxed::Box::new(scope2)).ok()
        }
        Err(_) => None,
    }
}

/// 扫描快照目录：可名记 .snap 数（目录不存在 = 0）。
fn count_snap_blobs(fs: &mut Lfs) -> usize {
    let Ok(entries) = fs.readdir(SNAP_DIR) else {
        return 0;
    };
    entries
        .iter()
        .filter(|e| !e.is_dir && e.name.ends_with(".snap"))
        .count()
}

/// blob 落盘：tmp 写 + rename 原子替换（tsdb 同款），随后按 produced_at
/// 保留最新 [`SNAP_KEEP_BLOBS`] 份。返回保留后的 blob 总数。
fn store_snap_blob(
    fs: &mut Lfs,
    snap: &eneros_snapshot_service::TrustedSnapshot,
    bytes: &[u8],
) -> Result<usize, ()> {
    let _ = fs.mkdir(SNAP_DIR);
    let name = snapshot_file_stem(&snap.snapshot_id);
    let tmp = format!("{}/{}.tmp", SNAP_DIR, name);
    let path = format!("{}/{}.snap", SNAP_DIR, name);
    {
        let mut f = fs
            .create(&tmp, eneros_fs::types::FileMode::default_file())
            .map_err(|_| ())?;
        f.write(fs, bytes).map_err(|_| ())?;
        let _ = f.close();
    }
    fs.rename(&tmp, &path).map_err(|_| ())?;
    prune_snap_blobs(fs)
}

/// 按 produced_at 保留最新 [`SNAP_KEEP_BLOBS`] 份 .snap（解码失败文件
/// 一并清除——半成品不可见；.tmp 残留清场，tsdb cleanup 同律）。
fn prune_snap_blobs(fs: &mut Lfs) -> Result<usize, ()> {
    let entries = fs.readdir(SNAP_DIR).map_err(|_| ())?;
    let mut metas: Vec<(String, u64)> = Vec::new();
    for e in entries.iter().filter(|e| !e.is_dir) {
        let path = format!("{}/{}", SNAP_DIR, e.name);
        if e.name.ends_with(".tmp") {
            let _ = fs.remove(&path);
            continue;
        }
        if !e.name.ends_with(".snap") {
            continue;
        }
        let produced = read_snap_file(fs, &path).ok().and_then(|b| {
            eneros_snapshot_service::decode_snapshot_v1(&b)
                .ok()
                .map(|s| s.produced_at_ns)
        });
        match produced {
            Some(produced) => metas.push((path, produced)),
            None => {
                let _ = fs.remove(&path);
            }
        }
    }
    if metas.len() <= SNAP_KEEP_BLOBS {
        return Ok(metas.len());
    }
    metas.sort_by_key(|(_, produced)| core::cmp::Reverse(*produced));
    for (path, _) in metas[SNAP_KEEP_BLOBS..].iter() {
        let _ = fs.remove(path);
    }
    Ok(SNAP_KEEP_BLOBS)
}

/// 读整个文件（stat 定长 → open READ 循环读满；tsdb 同法）。
fn read_snap_file(fs: &mut Lfs, path: &str) -> Result<Vec<u8>, ()> {
    let stat = fs.stat(path).map_err(|_| ())?;
    let mut buf = alloc::vec![0u8; stat.size as usize];
    let mut f = fs
        .open(path, eneros_fs::types::OpenFlags::READ)
        .map_err(|_| ())?;
    let mut total = 0usize;
    while total < buf.len() {
        let n = f.read(fs, &mut buf[total..]).map_err(|_| ())?;
        if n == 0 {
            break;
        }
        total += n;
    }
    buf.truncate(total);
    Ok(buf)
}

/// snapshot_id → 文件名干（32 hex）。
fn snapshot_file_stem(id: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in id {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// ============================================================================
// 模块构建（catalog → 政策 → SnapshotModule）
// ============================================================================

/// 快照政策（68.4 执行卡裁决与判据对齐）：
/// - required = 生产安全覆盖点（`safety_coverage()` 单源——catalog parity
///   测试保证目录覆盖与之相等）；
/// - require_integrity = **false**：builder integrity 门只认
///   `Authenticated`，生产采集轨 integrity = `LocalTrustedEdge`（本机
///   可信边，modbus/iec104 ingest 唯一赋值点）——政策取生产事实，不放宽
///   也不伪标；Authenticated 提升归 certifier 接电卷；
/// - catalog_version = repo.version_set().contract_version（目录单源）；
/// - freshness/lateness 600s：覆盖 OBS 批距（实测 ~46s）与快照窗
///   （8 批 ≈ 6min）的诚实余量；per-purpose 语义收紧归 certifier 卷。
fn build_policy(
    repo: &CatalogRepository,
    coverage_points: Vec<u32>,
) -> eneros_snapshot_service::SnapshotPolicy {
    eneros_snapshot_service::SnapshotPolicy {
        purpose_id: SNAP_PURPOSE_ID,
        policy_version: 1,
        required: coverage_points,
        optional: Vec::new(),
        allowed_lateness_ns: 600_000_000_000,
        max_age_ns: 600_000_000_000,
        min_quality_bits_ok: true,
        require_integrity: false,
        degraded_ok: false,
        allowed_sources: None,
        substitution: None,
        catalog_version: repo.version_set().contract_version,
    }
}

/// catalog → (模块核, required 点数)。投影容量 64 点 ×4 事件（内存审计
/// 表 68.4 执行卡：SnapshotModule 上界 ~48KiB 的依据）。
fn build_module(repo: CatalogRepository, now_ms: u64) -> (SnapshotCore, usize) {
    use eneros_control_dispatch::feedback::safety_coverage;
    let coverage_points: Vec<u32> = safety_coverage().points.iter().map(|p| p.point).collect();
    let n = coverage_points.len();
    let mut policies = eneros_snapshot_service::SnapshotPolicyRegistry::new();
    policies.register(build_policy(&repo, coverage_points));
    let module = eneros_snapshot_service::module_from_catalog(
        repo,
        eneros_snapshot_service::ProjectorCapacity {
            max_points: 64,
            max_events_per_point: 4,
        },
        policies,
        eneros_snapshot_service::ConflictResolver::new(
            eneros_snapshot_service::conflict::ConflictRule::PrimaryBackup,
        ),
        now_ms.saturating_mul(1_000_000),
    );
    (
        SnapshotCore {
            module,
            batches_since_snap: 0,
        },
        n,
    )
}

// ============================================================================
// host 测试（FakeStorage 全链——telemetry_history 同构测试面）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeStorage;

    fn make_io(storage: &FakeStorage) -> BlockIo<FakeStorage> {
        BlockIo::new(storage.clone(), "svc-storage", 0, 5_000)
    }

    const REGION_BLOCKS_512: u64 = eneros_ota_ab::ab_partition::DATA_EVENT_BLOCKS;
    const SNAP_BLOCKS_512: u64 = eneros_ota_ab::ab_partition::DATA_SNAP_BLOCKS;
    /// 测试盘 = 事件面 + 间隙 + 快照面（验证 RegionScope 基址平移）。
    const EVENT_BASE: u64 = eneros_ota_ab::ab_partition::DATA_EVENT_BASE_BLOCK;
    const SNAP_BASE: u64 = eneros_ota_ab::ab_partition::DATA_SNAP_BASE_BLOCK;
    const DISK_END: u64 = SNAP_BASE + SNAP_BLOCKS_512;

    fn make_face(storage: &FakeStorage) -> EventFace<FakeStorage> {
        EventFace::new(
            make_io(storage),
            make_io(storage),
            DataRegion::EventFace,
            DataRegion::SnapshotFace,
        )
    }

    fn prod_module_catalog() -> CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled =
            eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile catalog");
        eneros_semantic_catalog::open(&compiled.payload).expect("decode catalog")
    }

    fn obs(point: u32, seq: u64, now_ms: u64, value: f64) -> ObservationEvent {
        use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};
        ObservationEvent {
            tenant_id: 1,
            site_id: 1,
            source_id: 1,
            source_epoch: 1,
            sequence: seq,
            event_time_ns: now_ms.saturating_mul(1_000_000),
            ingest_time_ns: now_ms.saturating_mul(1_000_000),
            asset_id: 1,
            device_id: 2,
            unit_code: 0,
            point_id: point,
            schema_id: 1,
            schema_version: 1,
            quality_bits: quality_bits::VALID,
            mapping_version: 1,
            value: ObservationValue::F64(value),
            integrity: IntegrityTag::LocalTrustedEdge,
            clock_id: 1,
            flags: 0,
        }
    }

    fn face_dev(storage: &FakeStorage) -> EipcBlockDevice<FakeStorage> {
        EipcBlockDevice::new(
            make_io(storage),
            eneros_event_store::format::BLOCK_SIZE,
            eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS / 8,
        )
    }

    /// t2（bin 装配存在性，E-DEF-01 教训的测试化）：svc-telemetry bin
    /// 必须装配 EventFace 并接事件面接缝——"改回 None"回归在此红。
    #[test]
    fn t2_bin_wiring_present() {
        let bin = include_str!("bin/svc-telemetry.rs");
        assert!(
            bin.contains("EventFace::new"),
            "svc-telemetry 未装配 EventFace（68.4 事件面回退回归）"
        );
        assert!(
            bin.contains("set_event_face"),
            "svc-telemetry 未接 OBS 事件面接缝"
        );
        assert!(
            bin.contains("DATA_EVENT_BASE_BLOCK") && bin.contains("DATA_SNAP_BASE_BLOCK"),
            "svc-telemetry 未以 ota-ab 区段常量装配（区段表单源破坏）"
        );
    }

    /// t3：越区段零逃逸——全部块命令字节偏移落在事件面/快照面让渡区段
    /// 内（含基址平移下界与上界）；变异（去 RegionScope 钳制）必红。
    #[test]
    fn t3_region_scoped_no_escape() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let events: Vec<ObservationEvent> = (1u32..=7)
            .map(|p| obs(p, p as u64, 2_000, p as f64 * 10.0))
            .collect();
        face.on_obs_batch(&events, 2_000);
        assert!(face.is_open(), "事件面必须已打开");
        assert_eq!(face.appended, 7, "接受集全量追加");
        let event_range =
            (EVENT_BASE * 512) as usize..((EVENT_BASE + REGION_BLOCKS_512) * 512) as usize;
        let snap_range = (SNAP_BASE * 512) as usize..((SNAP_BASE + SNAP_BLOCKS_512) * 512) as usize;
        let cmds = storage.cmds();
        assert!(!cmds.is_empty(), "块命令必须发生");
        for (cmd, off, len) in cmds.iter() {
            let (off, len) = (*off as usize, *len);
            let end = off + len;
            let within = (off >= event_range.start && end <= event_range.end)
                || (off >= snap_range.start && end <= snap_range.end);
            assert!(
                within,
                "块命令越区段：cmd={} off={} len={}（事件面 {:?} / 快照面 {:?}）",
                cmd, off, len, event_range, snap_range
            );
        }
    }

    /// open-or-format + 掉电重开不丢（scoped 全链；格式化不可静默重置）。
    #[test]
    fn t3b_open_or_format_remount_preserves() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let events: Vec<ObservationEvent> = (1u32..=7)
            .map(|p| obs(p, p as u64, 2_000, p as f64))
            .collect();
        face.on_obs_batch(&events, 2_000);
        drop(face);
        // 新面重开：事件保持（recovered = 7）。
        let dev = face_dev(&storage);
        let (_store, report) = open_region_scoped(
            || CachedBlocks::<4, _>::new(dev.clone()),
            EVENT_BASE / 8,
            REGION_BLOCKS_512 / 8,
            event_store_config(),
        )
        .expect("remount");
        assert_eq!(report.valid_records, 7);
        assert_eq!(report.bad_segments, 0);
    }

    /// t4：快照构建→落盘→（掉电）重启恢复→水位一致。重放机制：
    /// 同介质重开 store → 全量读 → 新模块 apply_events → 构建快照的
    /// source_watermarks 与落盘 blob 内水印逐字段相等（四维语义）。
    #[test]
    fn t4_snapshot_roundtrip_and_watermark_consistency() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut seq = 0u64;
        // —— 面甲：写满一个快照窗（8 批 × 7 点）——
        {
            let mut face = make_face(&storage);
            face.set_snapshot_catalog(prod_module_catalog(), 1_000);
            for round in 0..SNAP_EVERY_BATCHES {
                let now = 2_000 + round * 46_000;
                let events: Vec<ObservationEvent> = (1u32..=7)
                    .map(|p| {
                        seq += 1;
                        obs(p, seq, now, p as f64 * 10.0 + round as f64)
                    })
                    .collect();
                face.on_obs_batch(&events, now);
            }
            assert_eq!(face.snap_stored, 1, "窗满批必须产出 1 份快照");
            assert_eq!(face.appended, 7 * SNAP_EVERY_BATCHES);
        }
        // —— 掉电：整面丢弃，介质留存 ——
        // —— 上电：重开事件面（mount 路径）——
        let dev = face_dev(&storage);
        let (store, report) = open_region_scoped(
            || CachedBlocks::<4, _>::new(dev.clone()),
            EVENT_BASE / 8,
            REGION_BLOCKS_512 / 8,
            event_store_config(),
        )
        .expect("重开事件面");
        assert!(
            report.valid_records == 7 * SNAP_EVERY_BATCHES,
            "已 ACK 记录全在（recovered={}）",
            report.valid_records
        );
        // 重放 → 新模块 → 水位重建。
        let mut replayed: Vec<ObservationEvent> = Vec::new();
        for p in store.partitions() {
            let mut c = FrameCollector {
                frames: &mut replayed,
            };
            store.read(p, 0, &mut c).expect("replay read");
        }
        assert_eq!(
            replayed.len(),
            7 * SNAP_EVERY_BATCHES as usize,
            "重放零丢失零乱序"
        );
        let (mut module2, _) = build_module(prod_module_catalog(), 0);
        module2.module.apply_events(&replayed);
        // 落盘 blob 读回 → 水位一致（四维逐字段）。
        let mut snap_fs =
            open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("重开快照卷");
        let entries = snap_fs.readdir(SNAP_DIR).expect("snap dir");
        let snaps: Vec<_> = entries
            .iter()
            .filter(|e| e.name.ends_with(".snap"))
            .collect();
        assert_eq!(snaps.len(), 1, "恰好 1 份快照 blob");
        let bytes = read_snap_file(&mut snap_fs, &format!("{}/{}", SNAP_DIR, snaps[0].name))
            .expect("blob read");
        let stored = eneros_snapshot_service::decode_snapshot_v1(&bytes).expect("decode");
        // 重建模块以落盘快照的 cut 为 as_of 构建——水位逐字段一致。
        let request = eneros_snapshot_service::SnapshotRequest {
            tenant_id: 1,
            site_id: 1,
            purpose_id: SNAP_PURPOSE_ID,
            as_of_ns: stored.cut_time_ns,
            max_wait_ns: 0,
            policy_version: 1,
        };
        let rebuilt = module2
            .module
            .build(request, stored.cut_time_ns)
            .expect("重建模块可构建（水位完整）");
        assert_eq!(
            rebuilt.source_watermarks, stored.source_watermarks,
            "重启恢复后水位与落盘快照逐字段一致（四维语义）"
        );
        assert_eq!(
            rebuilt.provenance_root, stored.provenance_root,
            "重放确定性：同输入同 root"
        );
        // 生产路径重开：SNAP 恢复计数可见 + 事件续接（offset 单调）。
        let mut face2 = make_face(&storage);
        face2.set_snapshot_catalog(prod_module_catalog(), 900_000);
        let events: Vec<ObservationEvent> = (1u32..=7)
            .map(|p| {
                seq += 1;
                obs(p, seq, 900_000, p as f64)
            })
            .collect();
        face2.on_obs_batch(&events, 900_000);
        assert!(face2.is_open());
        assert_eq!(face2.appended, 7, "重启后事件面继续追加");
    }

    struct FrameCollector<'a> {
        frames: &'a mut Vec<ObservationEvent>,
    }

    impl eneros_event_store::RecordVisitor for FrameCollector<'_> {
        fn record(&mut self, r: eneros_event_store::RecordRef<'_>) -> bool {
            if let Ok(ev) = ObservationEvent::decode_v1(r.payload) {
                self.frames.push(ev);
            }
            true
        }
    }

    /// 保留策略：低水位不修剪；高水位（≥850‰）删完整段、开放段保持。
    /// 以小盘逼近水位（128KiB 区段 → ~580 事件触发）。
    #[test]
    fn t5_retention_prunes_full_segments_only() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 几何实算：段尾块被 trailer 占用 → 每段数据区 = 16KiB-64B 头
        // -4KiB trailer 块 = 12,224B → 74 记录/段（164B/条）；7 槽容量
        // 518。8 批 × 64 = 512 ≤ 518（零 Full），7 槽全占 → used =
        // (12,288 + 7×16,384)/131,072 = 968‰ ≥ 850‰——窗口批必须修剪
        // 且零丢失。
        let mut seq = 0u64;
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = (0..64u64)
                .map(|i| {
                    seq += 1;
                    obs(((i % 7) as u32) + 100, seq, now, i as f64)
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        assert!(
            face.append_failures == 0,
            "滚动保留下不得出现静默丢失（fail={}）",
            face.append_failures
        );
        assert!(face.pruned_segments >= 1, "高水位必须发生修剪");
    }

    /// 栈诊断（telemetry_history diag 同法）：打开+追加全链在服务栈供给
    /// 候选值（64KiB，宿主 debug 帧 ≥ target release 同路径）内存活——
    /// 此测过只说明余量存在，target 结论仍以 QEMU 实弹为准（E 卷教训）。
    #[test]
    fn diag_stack_open_append_survives() {
        let stack: usize = std::env::var("STACK_DIAG_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64 * 1024);
        // Rc 介面不可跨线程——FakeStorage 在线程内构造（无捕获即 Send）。
        let h = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || {
                let pad: [u8; 5120] = [0u8; 5120];
                core::hint::black_box(&pad);
                let storage = FakeStorage::new((DISK_END * 512) as usize);
                let mut face = make_face(&storage);
                face.set_snapshot_catalog(prod_module_catalog(), 1_000);
                let events: Vec<ObservationEvent> = (1u32..=7)
                    .map(|p| obs(p, p as u64, 2_000, p as f64))
                    .collect();
                face.on_obs_batch(&events, 2_000);
                assert!(face.is_open());
            })
            .expect("spawn");
        h.join()
            .expect("open+append path must survive at provisioned stack");
    }
}
