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

/// 保留修剪单窗段数预算（REG-70.15-PRUNE-MEGAOP，2026-09-15）：删除突发
/// 受预算约束、多窗摊销——无界删除 = 数万块 IPC 阻塞 svc-telemetry 单线程
/// 循环 ⇒ 快照停摆 ⇒ 控制链停摆（实弹 40 万行 IPC/20 分钟）。
pub const PRUNE_MAX_SEGMENTS_PER_WINDOW: usize = 4;

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
    /// 当前**已冻结**快照输入（蓝图 70.8 WP-70.8.1：身份 + 由同一快照
    /// 派生的 144B 状态）。`None` = 尚无可用冻结快照（冷启动/窗口未到/
    /// catalog 未到）——调用方必须拒绝该轮决策，不得回退评估器伪造身份。
    fn current_input(&self) -> Option<eneros_agent_sdk::snap_input_wire::TrustedInput>;
    /// 按 ID 取回已冻结快照 blob（`encode_snapshot_v1` 字节；蓝图 70.8.2
    /// /70.9.1——certifier 独立读取可信输入的唯一通道）。ID 不匹配当前
    /// 冻结快照 → `None`（绝不返回"另一代"的字节）。
    fn snapshot_blob(&mut self, id: &[u8; 16]) -> Option<Vec<u8>>;
    /// 登记保留持有（蓝图 70.8.2：被证书/执行证据引用的计划输入不得先
    /// 删）。`offset` = 该分区仍需保留的最小 offset（含）。
    fn hold_offset(&mut self, partition: u32, offset: u64);
    /// 快照保留钉（ADR-0054:29–30）：被持有计划证书引用的冻结快照
    /// （完整观测 blob）在持有期内不得被剪枝回收。幂等；false = 容量
    /// 满/面不可用（调用方 fail-closed：拒绝新计划准入）。
    fn pin_snapshot(&mut self, id: &[u8; 16]) -> bool;
    /// 解除快照保留钉（计划终态且无未决后）。幂等；false = 未钉/面不可
    /// 用（释放是回收面不是安全面，可重试）。
    fn release_snapshot(&mut self, id: &[u8; 16]) -> bool;
    /// 保留背压：容量水位越线且受保护记录阻塞删除（消费落后）→ 调用方
    /// 必须停发新决策（不得继续发新证书）。
    fn retention_blocked(&self) -> bool;
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
    /// 目录合同版本（catalog 单源；快照身份段引用）。
    catalog_version: u64,
    /// 目录 digest 前 8B（代次证据）。
    catalog_digest8: [u8; 8],
    /// 最近一次成功构建的冻结快照输入（蓝图 70.8.1：决策输入引用的同一
    /// 冻结产物；身份 + 由该快照派生的 144B 状态，234B 定长零堆）。
    last_input: Option<eneros_agent_sdk::snap_input_wire::TrustedInput>,
    /// 已交付给快照模块的分区消费位置（蓝图 70.8.2：提交位点的**唯一**
    /// 依据——只记"真的进了快照模块的 offset"，不猜）。
    consumed: Vec<(u32, u64)>,
}

impl SnapshotCore {
    /// 记录一条事件已被快照模块吸收（分区内单调推进）。
    fn mark_consumed(&mut self, partition: u32, offset: u64) {
        match self.consumed.iter_mut().find(|(p, _)| *p == partition) {
            Some((_, o)) => {
                if offset > *o {
                    *o = offset;
                }
            }
            None => self.consumed.push((partition, offset)),
        }
    }
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
    // —— 蓝图 70.8.2：消费位点 / 保留持有 / 背压 ——
    /// 消费位点提交成功次数（每次成功快照构建后一次）。
    pub consumer_commits: u64,
    /// 保留压力路径执行次数（水位 ≥850‰ 的窗口数）。
    pub retention_prune_passes: u64,
    /// 最近一次修剪中「计划输入持有是约束方」的分区数（证据，不静默）。
    pub retention_hold_limited: u64,
    /// 保留背压标志（容量越线 + 本轮删除未释放任何段）。
    retention_blocked: bool,
    /// 计划输入保留持有（分区 → 需保留的最小 offset；`hold_offset` 登记）。
    holds: Vec<(u32, u64)>,
    /// 快照保留钉（ADR-0054:29–30：持有计划证书引用的冻结快照 ID 集合
    /// ——剪枝跳过；重启后由 dispatch 按非终态计划重派生）。
    snap_pins: Vec<[u8; 16]>,
}

/// 快照钉容量（= 持有计划容量 MAX_PLANS(2) + 换代余量 2；超出 = 调用方
/// fail-closed 拒绝新准入——不留无限增长面）。
pub const MAX_SNAP_PINS: usize = 4;

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
            consumer_commits: 0,
            retention_prune_passes: 0,
            retention_hold_limited: 0,
            retention_blocked: false,
            holds: Vec::new(),
            snap_pins: Vec::new(),
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
                Ok(ack) => {
                    ok += 1;
                    self.appended += 1;
                    if let Some(core) = face.module.as_mut() {
                        core.module.apply_event(*ev);
                        // 蓝图 70.8.2：消费位置只记「真的进了快照模块」的
                        // offset（apply_event 之后）——提交位点即由此派生。
                        core.mark_consumed(
                            eneros_event_store::ingest_bridge::partition_of(ev),
                            ack.offset,
                        );
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
                &mut self.consumer_commits,
                &mut self.retention_prune_passes,
                &mut self.retention_hold_limited,
                &mut self.retention_blocked,
                &self.holds,
                &self.snap_pins,
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

    fn current_input(&self) -> Option<eneros_agent_sdk::snap_input_wire::TrustedInput> {
        let FaceState::Open(face) = &self.state else {
            return None;
        };
        face.module.as_ref().and_then(|c| c.last_input)
    }

    fn snapshot_blob(&mut self, id: &[u8; 16]) -> Option<Vec<u8>> {
        let FaceState::Open(face) = &mut self.state else {
            return None;
        };
        let fs = face.snap_fs.as_mut()?;
        // 蓝图 70.8.2：**按 ID 服务保留代次**（`SNAP_KEEP_BLOBS` 份）——
        // 在本卷 = 已接纳计划/在途认证引用的那一代，不是"最新一代"。安全
        // 性来自**身份复核**（解码后 snapshot_id 必须等于请求 ID）：绝不
        // 返回另一代的字节，也绝不返回解不开的字节。当前冻结代次由
        // `current_input` 单独承担（opcode 5 只服务最新一代）。
        let path = alloc::format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(id));
        let bytes = read_snap_file(fs, &path).ok()?;
        match eneros_snapshot_service::decode_snapshot_v1(&bytes) {
            Ok(snap) if snap.snapshot_id == *id => Some(bytes),
            _ => {
                eneros_runtime::println!("[svc-telemetry] SNAP-BLOB-REJECT id-mismatch-or-corrupt");
                None
            }
        }
    }

    fn hold_offset(&mut self, partition: u32, offset: u64) {
        match self.holds.iter_mut().find(|(p, _)| *p == partition) {
            Some((_, o)) => {
                // 持有只收紧（保留更强的一侧）——登记更小 offset 才生效。
                if offset < *o {
                    *o = offset;
                }
            }
            None => self.holds.push((partition, offset)),
        }
    }

    fn pin_snapshot(&mut self, id: &[u8; 16]) -> bool {
        if self.snap_pins.contains(id) {
            return true;
        }
        if self.snap_pins.len() >= MAX_SNAP_PINS {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-REJECT reason=pin-capacity pins={}",
                self.snap_pins.len()
            );
            return false;
        }
        self.snap_pins.push(*id);
        eneros_runtime::println!(
            "[svc-telemetry] SNAP-PIN-HELD id={:02x}{:02x} pins={}",
            id[0],
            id[1],
            self.snap_pins.len()
        );
        true
    }

    fn release_snapshot(&mut self, id: &[u8; 16]) -> bool {
        match self.snap_pins.iter().position(|p| p == id) {
            Some(idx) => {
                self.snap_pins.swap_remove(idx);
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-PIN-RELEASED id={:02x}{:02x} pins={}",
                    id[0],
                    id[1],
                    self.snap_pins.len()
                );
                true
            }
            None => false,
        }
    }

    fn retention_blocked(&self) -> bool {
        self.retention_blocked
    }
}

impl<C: EipcSend + Clone + 'static> EventFace<C> {
    /// 窗口批任务：快照构建 → 消费位点提交 → 受保护保留修剪 → blob 落盘。
    ///
    /// 蓝图 70.8.2 顺序理由：保留判据必须用**本窗口刚提交**的消费位点
    /// （先删后建会用一代前的位点，把仍被本轮快照引用的记录提前删掉）。
    /// 失败逐级显式（SNAP-SKIP reason），不伪装成功。借用序：module
    /// （构建，产出 owned 快照）→ sink（位点提交 + 修剪）→ snap_fs（落盘）。
    /// 计数器/持有以字段指针传入（face 已借走 self.state——拆借同一 self
    /// 的不相交字段）。
    #[allow(clippy::too_many_arguments)] // 计数器/持有以字段指针拆借传入
                                         //（face 已借走 self.state——同一 self 的不相交字段无法成束借用）。
    fn window_tasks(
        face: &mut OpenFace<C>,
        pruned_segments: &mut u64,
        snap_stored: &mut u64,
        snap_skipped: &mut u64,
        consumer_commits: &mut u64,
        retention_prune_passes: &mut u64,
        retention_hold_limited: &mut u64,
        retention_blocked: &mut bool,
        holds: &[(u32, u64)],
        snap_pins: &[[u8; 16]],
        now_ms: u64,
    ) {
        // 1. 快照构建（owned 产出后立刻释放 module 借用）。
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
        let (snap, consumed) = match built {
            Some(Ok(snap)) => {
                // 身份 + 已消费位置（快照成功 = 这批事件真的进了冻结产物）。
                let core = face.module.as_ref().expect("module present（上一步 Ok）");
                let input = crate::telemetry::input_from_snapshot(
                    &snap,
                    core.catalog_version,
                    core.catalog_digest8,
                    crate::telemetry::DEFAULT_CONTROL_HORIZON_MS,
                );
                let consumed = core.consumed.clone();
                (Some((snap, input)), consumed)
            }
            Some(Err(e)) => {
                *snap_skipped += 1;
                *retention_blocked = false; // 无新快照 → 不得因此冻结保留
                eneros_runtime::println!("[svc-telemetry] SNAP-SKIP {}", e);
                (None, Vec::new())
            }
            None => (None, Vec::new()),
        };
        // 2. blob 落盘（**先于**发布与位点提交）。顺序即不变式：冻结输入
        // 可用 ⇒ 其工件可取回；消费位点前进 ⇒ 被吸收的记录必有可检索的
        // 冻结产物。落盘失败 ⇒ 本窗口不发布输入、不提交位点（保留判据
        // 因此不会越过"无工件"的记录——蓝图 70.8.2 失败语义）。
        let stored = match &snap {
            Some((snap, _)) => {
                let bytes = eneros_snapshot_service::encode_snapshot_v1(snap);
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-BUILT pts={} bytes={}",
                    snap.observations.len(),
                    bytes.len()
                );
                match face.snap_fs.as_mut() {
                    Some(fs) => match store_snap_blob(fs, snap, &bytes, snap_pins) {
                        Ok(kept) => {
                            *snap_stored += 1;
                            eneros_runtime::println!("[svc-telemetry] SNAP-STORED kept={}", kept);
                            true
                        }
                        Err(()) => {
                            *snap_skipped += 1;
                            eneros_runtime::println!("[svc-telemetry] SNAP-SKIP store-fail");
                            false
                        }
                    },
                    None => {
                        *snap_skipped += 1;
                        eneros_runtime::println!("[svc-telemetry] SNAP-SKIP store-unavailable");
                        false
                    }
                }
            }
            None => false,
        };
        // 3. 发布冻结输入 + 4. 消费位点提交（**仅当工件已落盘**——位点即
        // "已被可检索的冻结产物吸收"）。
        if stored {
            let (_, input) = snap.expect("stored ⇒ snap 存在");
            if let Some(core) = face.module.as_mut() {
                core.last_input = Some(input);
            }
            let facts = input.facts();
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-INPUT-BOUND id={:02x}{:02x}{:02x}{:02x} pts={} complete={} catalog={}",
                facts.snapshot_id[0],
                facts.snapshot_id[1],
                facts.snapshot_id[2],
                facts.snapshot_id[3],
                facts.accepted,
                facts.is_complete(),
                facts.catalog_version
            );
            if !consumed.is_empty() {
                let offsets: Vec<eneros_event_store::PartitionOffset> = consumed
                    .iter()
                    .map(|(p, o)| eneros_event_store::PartitionOffset::new(*p, o.saturating_add(1)))
                    .collect();
                match face.sink.store_mut().commit_consumer(
                    eneros_snapshot_service::consumer::SNAPSHOT_CONSUMER_GROUP,
                    &offsets,
                ) {
                    Ok(()) => {
                        *consumer_commits += 1;
                    }
                    Err(_) => {
                        // 位点提交失败 = 保留判据无依据 → 下一轮全量保护
                        // （绝不因提交失败而删掉未吸收的记录）。
                        eneros_runtime::println!("[svc-telemetry] EVENT-CONSUMER-COMMIT-FAIL");
                    }
                }
            }
        }
        // 5. 受保护保留修剪：keep_after_offset = min(消费位点, 计划输入持有)。
        let wm_before: WatermarkReport = face.sink.store_mut().watermarks();
        if wm_before.used_permille >= RETAIN_PERMILLE {
            *retention_prune_passes += 1;
            // 冷启动/复用盘保护：本会话尚未物化任何冻结工件时**零删除**——
            // 复用盘上恢复出的消费位点指向的是上一会话的工件，不构成本次
            // 删除的依据（蓝图 70.8.2："不凭 0/最大值编造已消费"；此处同理
            // "不凭上一会话的位点删除本会话尚无工件覆盖的记录"）。
            let materialized = face.module.as_ref().and_then(|c| c.last_input).is_some();
            let partitions: Vec<_> = face.sink.store_mut().partitions().into_iter().collect();
            let mut deleted_total = 0u64;
            let mut hold_limited = 0u64;
            for p in partitions {
                // 界 = min(已提交消费位点, 计划输入持有的最小 offset)。
                // 无位点、无持有 → 0 = 全保护（不凭 0/最大值编造"已消费"）。
                let committed = if materialized {
                    face.sink
                        .store()
                        .committed_offset(
                            eneros_snapshot_service::consumer::SNAPSHOT_CONSUMER_GROUP,
                            p,
                        )
                        .unwrap_or(0)
                } else {
                    0
                };
                let hold = holds
                    .iter()
                    .filter(|(hp, _)| *hp == p)
                    .map(|(_, o)| *o)
                    .min()
                    .unwrap_or(u64::MAX);
                let keep = core::cmp::min(committed, hold);
                if hold < committed {
                    hold_limited += 1; // 计划输入持有是此刻的约束方（证据）
                }
                match face.sink.store_mut().delete_full_segments_before(
                    p,
                    keep,
                    PRUNE_MAX_SEGMENTS_PER_WINDOW,
                ) {
                    Ok(deleted) if !deleted.is_empty() => {
                        deleted_total += deleted.len() as u64;
                        *pruned_segments += deleted.len() as u64;
                        eneros_runtime::println!(
                            "[svc-telemetry] EVENT-PRUNE n={} partition={} keep={}",
                            deleted.len(),
                            p,
                            keep
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
            *retention_hold_limited = hold_limited;
            // 背压：水位仍越线且本轮一个段都没能释放 → 保留受保护记录
            // 阻塞容量（消费落后/持有未释放）——调用方必须停发新决策。
            let wm_after: WatermarkReport = face.sink.store_mut().watermarks();
            let blocked = wm_after.used_permille >= RETAIN_PERMILLE && deleted_total == 0;
            if blocked != *retention_blocked {
                eneros_runtime::println!(
                    "[svc-telemetry] EVENT-RETENTION blocked={} permille={} keep_limited_by_hold={}",
                    blocked,
                    wm_after.used_permille,
                    hold_limited
                );
            }
            *retention_blocked = blocked;
        } else if *retention_blocked {
            *retention_blocked = false;
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
    let scope =
        RegionScope::new(CachedBlocks::<4, _>::new(face.clone()), base_4k, blocks_4k).ok()?;
    match Lfs::mount(alloc::boxed::Box::new(scope)) {
        Ok(fs) => Some(fs),
        Err(eneros_fs::FsError::BadSuperBlock) => {
            // 首次上电：显式格式化一次（fresh face；仅让渡区段内）。
            let scope2 =
                RegionScope::new(CachedBlocks::<4, _>::new(face), base_4k, blocks_4k).ok()?;
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
    pins: &[[u8; 16]],
) -> Result<usize, ()> {
    let _ = fs.mkdir(SNAP_DIR);
    let name = snapshot_file_stem(&snap.snapshot_id);
    let tmp = format!("{}/{}.tmp", SNAP_DIR, name);
    let path = format!("{}/{}.snap", SNAP_DIR, name);

    // —— 先腾位，再写 ——
    //
    // 2026-09-14（70.15 长窗实弹）：旧实现是"先写后剪"，于是**区段一旦写不下
    // 一份新 blob 就永远写不下**——littlefs 的 create/rename（目录元数据提交）
    // 本身需要空闲块，满区时新写入必失败，而唯一的释放动作在**失败之后**的
    // 剪枝里，永远轮不到 ⇒ 冻结输入停止发布 ⇒ EA 恒 `snapshot-stale` ⇒ 控制链
    // 静默饿死。实测：`SNAP-BUILT=25 / SNAP-STORED=6 / SNAP-SKIP store-fail=19`，
    // 提交停在 52 次不再增长（同一窗内 rounds 仍在涨——进程活着但不再控制）。
    //
    // 修法（保持"半成品不可见"与"保留最新 N 份"语义）：
    //   1) 写前先剪到 `SNAP_KEEP_BLOBS - 1`，为新文件留出确定性空间；
    //   2) 仍失败 ⇒ **对半砍保留数并重试**（15 → 7 → 3 → 1），直到写得下
    //      或砍到 0 为止——自恢复，不把"区段紧张"升级成"控制链永久停摆"。
    //      为什么必须"按容量自适应"而不是固定砍一次：实测该区段（64×4KiB
    //      LFS）**实际只装得下约 6 份 1.2KiB blob**（littlefs 元数据/CTZ
    //      开销），远低于配置的 `SNAP_KEEP_BLOBS=16` ⇒ 固定阈值剪枝腾不出
    //      空间（见 `REG-70.15-SNAPBLOB-CAPACITY-VS-RETENTION`）。
    let attempt = |fs: &mut Lfs| -> Result<(), ()> {
        let mut f = fs
            .create(&tmp, eneros_fs::types::FileMode::default_file())
            .map_err(|_| ())?;
        f.write(fs, bytes).map_err(|_| ())?;
        let _ = f.close();
        fs.rename(&tmp, &path).map_err(|_| ())
    };
    let mut keep = SNAP_KEEP_BLOBS.saturating_sub(1);
    loop {
        let _ = prune_snap_blobs_keep(fs, keep, pins);
        if attempt(fs).is_ok() {
            break;
        }
        // 半成品不可见：失败即清 tmp（其名称不在 `.snap` 集合内，但会占块）。
        let _ = fs.remove(&tmp);
        if keep == 0 {
            return Err(());
        }
        keep /= 2;
    }
    prune_snap_blobs_keep(fs, SNAP_KEEP_BLOBS, pins)
}

/// 保留上限参数化版本：写前腾位需要"比最终保留数少一份"的目标，
/// 重试路径需要"砍到一半"。语义与 [`SNAP_KEEP_BLOBS`] 版逐字相同。
fn prune_snap_blobs_keep(fs: &mut Lfs, keep: usize, pins: &[[u8; 16]]) -> Result<usize, ()> {
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
    // ADR-0054:29–30：持有计划证书引用的冻结快照（钉集）不参与剪枝
    // ——钉住者越过 keep 线也保留（回收面的最终裁决在 release）。
    let pinned_paths: Vec<String> = pins
        .iter()
        .map(|id| format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(id)))
        .collect();
    if metas.len() <= keep {
        return Ok(metas.len());
    }
    metas.sort_by_key(|(_, produced)| core::cmp::Reverse(*produced));
    for (path, _) in metas[keep..].iter() {
        if pinned_paths.contains(path) {
            continue;
        }
        let _ = fs.remove(path);
    }
    Ok(keep)
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
/// - optional = **控制合同回读点 − required**（蓝图 70 R10 多资产：认证器
///   `readback_ok` 为 presence 判定——合同回读点必须在快照受理观测里；
///   不在安全覆盖集的回读点（如 PCS-2 的 1005）经 optional 进快照——
///   缺席不影响完整性，不把快照健康耦合到每台 PCS 的遥测）；
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
    let mut optional: Vec<u32> = repo
        .control_contracts()
        .map(|c| c.readback_point_id)
        .chain(
            repo.asset_ids()
                .into_iter()
                .filter_map(|id| repo.asset_soc_point(id)),
        )
        .filter(|&p| p != 0 && !coverage_points.contains(&p))
        .collect();
    optional.sort_unstable();
    optional.dedup();
    eneros_snapshot_service::SnapshotPolicy {
        purpose_id: SNAP_PURPOSE_ID,
        policy_version: 1,
        required: coverage_points,
        optional,
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
/// 表 68.4 执行卡：SnapshotModule 上界 ~48KiB 的依据）。目录版本/digest
/// 前 8B 在本点捕获（repo 随后被 module 消费——身份事实不留悬空引用）。
fn build_module(repo: CatalogRepository, now_ms: u64) -> (SnapshotCore, usize) {
    use eneros_control_dispatch::feedback::safety_coverage;
    let coverage_points: Vec<u32> = safety_coverage().points.iter().map(|p| p.point).collect();
    let n = coverage_points.len();
    let catalog_version = repo.version_set().contract_version;
    let repo_digest = repo.digest();
    let mut catalog_digest8 = [0u8; 8];
    catalog_digest8.copy_from_slice(&repo_digest[..8]);
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
            catalog_version,
            catalog_digest8,
            last_input: None,
            consumed: Vec::new(),
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

    /// 快照必收点集（生产 = `control_dispatch::feedback::safety_coverage()`；
    /// 夹具与之同集：1..=7 + **控制合同功率回读点 1001**，2026-09-14 起）。
    fn coverage_fixture_points() -> impl Iterator<Item = u32> {
        (1u32..=7).chain(core::iter::once(1001u32))
    }

    /// 夹具取值：点 1001（kW，覆盖窗 ±500）必须落在窗内；其余保持原伪值。
    fn fixture_value(point: u32, round: usize) -> f64 {
        if point == 1001 {
            12.0 + round as f64
        } else {
            point as f64 * 10.0 + round as f64
        }
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
        let events: Vec<ObservationEvent> = coverage_fixture_points()
            .map(|p| obs(p, p as u64, 2_000, fixture_value(p, 0)))
            .collect();
        face.on_obs_batch(&events, 2_000);
        assert!(face.is_open(), "事件面必须已打开");
        assert_eq!(
            face.appended, 8,
            "接受集全量追加（1..=7 + 合同回读点 1001）"
        );
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
        let events: Vec<ObservationEvent> = coverage_fixture_points()
            .map(|p| obs(p, p as u64, 2_000, fixture_value(p, 0)))
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
        assert_eq!(report.valid_records, 8);
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
        // —— 面甲：写满一个快照窗（8 批 × 8 点；含合同回读点 1001）——
        {
            let mut face = make_face(&storage);
            face.set_snapshot_catalog(prod_module_catalog(), 1_000);
            for round in 0..SNAP_EVERY_BATCHES {
                let now = 2_000 + round * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        seq += 1;
                        obs(p, seq, now, fixture_value(p, round as usize))
                    })
                    .collect();
                face.on_obs_batch(&events, now);
            }
            assert_eq!(face.snap_stored, 1, "窗满批必须产出 1 份快照");
            assert_eq!(face.appended, 8 * SNAP_EVERY_BATCHES);
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
            report.valid_records == 8 * SNAP_EVERY_BATCHES,
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
            8 * SNAP_EVERY_BATCHES as usize,
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
        let events: Vec<ObservationEvent> = coverage_fixture_points()
            .map(|p| {
                seq += 1;
                obs(p, seq, 900_000, fixture_value(p, 0))
            })
            .collect();
        face2.on_obs_batch(&events, 900_000);
        assert!(face2.is_open());
        assert_eq!(face2.appended, 8, "重启后事件面继续追加");
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

    /// 保留策略：低水位不修剪；高水位（≥850‰）**且已物化到冻结快照**
    /// 的完整段可删、开放段保持（蓝图 70.8.2 新判据）。
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
        // 且零丢失。每批含 **8 个必需点**（1..=7 + 合同回读点 1001，快照
        // 可物化）+ **56 个填充点**（填充数随之 −1，保持"每批 64 条 / 总
        // 512 ≤ 518"这一几何前提不变——填充数是适配量，几何才是判据）。
        let mut seq = 0u64;
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let mut events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            for i in 0..56u64 {
                seq += 1;
                events.push(obs(100 + (i % 4) as u32, seq, now, i as f64));
            }
            face.on_obs_batch(&events, now);
        }
        assert!(
            face.append_failures == 0,
            "滚动保留下不得出现静默丢失（fail={}）",
            face.append_failures
        );
        assert!(
            face.snap_stored >= 1,
            "窗口必须产出冻结快照（删除判据前提）"
        );
        assert!(face.consumer_commits >= 1, "快照物化后必须提交消费位点");
        assert!(
            face.pruned_segments >= 1,
            "高水位 + 已物化必须发生修剪（pruned={}）",
            face.pruned_segments
        );
    }

    /// 2026-09-14（70.15 长窗）：**快照 blob 落盘在 30 个窗内恒成功**的
    /// 稳态回归锁。
    ///
    /// 实弹现象（`SNAP-BUILT=25 / SNAP-STORED=6 / SNAP-SKIP store-fail=19`，
    /// 提交停在 52 次而 rounds 仍在涨 = "进程活着但不再控制"）的**真根因是
    /// 块层**：`[svc-storage] blk read FAILED … err=HardwareFault { detail:
    /// "virtqueue full" }` ⇒ 快照写路径拿不到块 ⇒ store-fail（登记
    /// `REG-70.15-VIRTQ-FULL-UNDER-LOAD`）。**本夹具不含 virtio**，故本测试
    /// 在"先写后剪"的旧实现下同样为绿——它不是那条缺陷的红测，只锁住
    /// "稳态下每窗都出可检索产物"这一性质（防止后续改动引入新的落盘丢失）。
    ///
    /// 与之配套的**真修复**是 `store_snap_blob` 的"先剪后写 + 按容量对半
    /// 重试"（区段真紧张时降保留而非停链）；其红测需带块层的夹具，属后续卷。
    #[test]
    fn t7_snapshot_blob_store_never_starves_on_tight_region() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        // 30 个快照窗（窗 = SNAP_EVERY_BATCHES 批）> 区段实际容量（≈6 份）
        // 与配置保留（16 份）——旧实现必在第 7 个窗起连续 store-fail。
        let windows = 30u64;
        for batch in 0..(windows * SNAP_EVERY_BATCHES) {
            let now = 2_000 + batch * 46_000;
            let events: alloc::vec::Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, batch as usize))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        assert_eq!(
            face.snap_skipped, 0,
            "快照落盘不得因区段紧张而失败（skipped={}，stored={}）",
            face.snap_skipped, face.snap_stored
        );
        assert!(
            face.snap_stored >= windows,
            "每个快照窗都必须产出可检索的冻结产物（stored={} 窗={windows}）",
            face.snap_stored
        );
        assert!(
            face.current_input().is_some(),
            "最后一窗之后仍必须有可用的冻结输入（否则控制链饿死）"
        );
    }

    /// ADR-0054:29–30（2026-09-18 R10 复查补全）：被持有计划证书引用的
    /// 冻结快照（钉）在存储压力剪枝下**必须保留**（按 ID 仍可取回完整
    /// 观测）；解除后回收面真实（后续压力剪枝可回收）。
    /// 变异靶：prune 跳过钉集的条件删除 → `snapshot_blob(&id1)` 变 None
    /// → 本测红；release 后不回收 → 尾断言红。
    #[test]
    fn t8_pinned_snapshot_survives_prune_and_release_recycles() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let run_windows = |face: &mut EventFace<FakeStorage>, from: u64, n: u64, seq: &mut u64| {
            for batch in from..(from + n * SNAP_EVERY_BATCHES) {
                let now = 2_000 + batch * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        *seq += 1;
                        obs(p, *seq, now, fixture_value(p, batch as usize))
                    })
                    .collect();
                face.on_obs_batch(&events, now);
            }
        };
        // 窗 1：产出快照 id1 并钉住（模拟 dispatch 持有计划准入）。
        run_windows(&mut face, 0, 1, &mut seq);
        let id1 = face
            .current_input()
            .expect("窗 1 后必有冻结输入")
            .facts()
            .snapshot_id;
        assert!(face.pin_snapshot(&id1), "钉面容量内必须成功");
        // 30 窗存储压力（区段实际容量 ≈6 份 > 配置保留 16——压力剪枝必发生）。
        run_windows(&mut face, 8, 30, &mut seq);
        assert_eq!(face.snap_skipped, 0, "钉住单份不得造成落盘停摆");
        assert!(
            face.snapshot_blob(&id1).is_some(),
            "被钉住的冻结输入必须可在持有期内按 ID 取回（ADR-0054:29–30）"
        );
        // 解除（计划终态）→ 后续压力剪枝可回收。
        assert!(face.release_snapshot(&id1));
        run_windows(&mut face, 8 + 30 * 8, 12, &mut seq);
        assert!(
            face.snapshot_blob(&id1).is_none(),
            "解除后的旧快照必须可被回收（释放面真实——只收紧不释放即违约）"
        );
    }

    /// 70.8.1：冻结快照身份可被决策侧读取，且与落盘 blob 同源
    /// （snapshot_id / provenance_root 一致）；冷启动（无 catalog/无窗）
    /// 为空——绝不伪造身份。
    #[test]
    fn t6_frozen_snapshot_input_identity() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        // 冷启动：未装配 catalog → 无冻结快照。
        assert!(face.current_input().is_none(), "冷启动不得有冻结快照");
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        for round in 0..SNAP_EVERY_BATCHES {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        let input = face.current_input().expect("窗满批必须有冻结快照输入");
        assert!(!input.facts().is_unbound(), "身份绑定（snapshot_id 非零）");
        assert!(input.facts().is_complete(), "必需点齐 → 完整");
        assert_eq!(
            input.facts().accepted,
            8,
            "快照观测 = 必需点集（含合同回读点 1001）"
        );
        assert!(input.catalog_version() > 0, "目录合同版本随身份携带");
        // 载荷派生自同一快照：电压 = 点 1 的值（10.0 ≤ v ≤ 70.0 区间内）。
        let v = input
            .telemetry_f64(eneros_agent_sdk::snap_input_wire::TELEMETRY_OFF_V)
            .expect("payload");
        assert!((10.0..=70.0).contains(&v), "v={v}");
        // blob 读回：身份一致（同源）。
        let bytes = face
            .snapshot_blob(&input.facts().snapshot_id)
            .expect("blob");
        let stored = eneros_snapshot_service::decode_snapshot_v1(&bytes).expect("decode");
        assert_eq!(stored.snapshot_id, input.facts().snapshot_id);
        assert_eq!(stored.provenance_root, input.facts().provenance_root);
        // 身份不匹配 → 拒绝（绝不返回另一代的字节）。
        assert!(face.snapshot_blob(&[0xEE; 16]).is_none());
    }

    /// 蓝图 70.8.2 保留代次可达性（t6 追加牙）：blob 通道按 ID 服务
    /// **保留代次**（`SNAP_KEEP_BLOBS` 份），不是只有最新一代——已接纳
    /// 计划/在途认证引用的那一代必须可取回（70.11 计划持有的前置）。
    /// 未知/不存在的 ID 仍拒绝（绝不返回另一代字节，也绝不返回解不开的
    /// 字节）。
    ///
    /// 变异靶：改回"只服务当前冻结快照" → 本测第一代取回断言红。
    #[test]
    fn t6b_blob_serves_retained_generations_by_id() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let mut ids: Vec<[u8; 16]> = Vec::new();
        // 两个快照窗（各 8 批）→ 两代冻结快照，皆在保留窗内。
        for round in 0..(SNAP_EVERY_BATCHES * 2) {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            face.on_obs_batch(&events, now);
            if let Some(input) = face.current_input() {
                let id = *input.snapshot_id();
                if ids.last() != Some(&id) {
                    ids.push(id);
                }
            }
        }
        assert_eq!(ids.len(), 2, "两个窗必须产出两代不同的冻结快照");
        // 当前输入 = 最新一代；两代 blob 都必须按 ID 可取回。
        let current = *face.current_input().expect("current").snapshot_id();
        assert_eq!(current, ids[1], "current_input 只服务最新一代");
        for id in &ids {
            let bytes = face.snapshot_blob(id).expect("保留代次必须按 ID 可取回");
            let decoded = eneros_snapshot_service::decode_snapshot_v1(&bytes).expect("decode");
            assert_eq!(decoded.snapshot_id, *id, "取回的必须是所请求的那一代");
        }
        assert!(face.snapshot_blob(&[0xEE; 16]).is_none(), "未知 ID 拒绝");
    }

    /// 70.8.2：未物化（无有效快照）时保留不释放——记录留在盘上；
    /// 计划输入持有同样阻塞删除；两者都不得造成"删了还声称可追溯"。
    #[test]
    fn t7_retention_blocked_without_materialization_and_by_hold() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 全部为非法点（100+）：快照无法物化（缺必需点）→ 消费位点永不提交。
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
        assert_eq!(face.snap_stored, 0, "非法点集不得产出快照");
        assert_eq!(face.consumer_commits, 0, "无物化 → 零消费位点提交");
        assert_eq!(face.pruned_segments, 0, "无物化 → 零删除（全保护）");
        assert!(
            face.retention_blocked(),
            "高水位 + 无物化 → 背压（调用方必须停发新决策）"
        );

        // —— 持有阻塞：物化路径下登记更早的持有 → 删除界收紧 ——
        let storage2 = FakeStorage::new(disk_len);
        let mut face2 = make_face(&storage2);
        face2.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 持有该分区 offset 0（保护一切）——即使快照物化也不得删。
        // 分区键 = event-store 单源 `partition_of`（tenant/site/source 散列）。
        let part = eneros_event_store::ingest_bridge::partition_of(&obs(1, 1, 2_000, 0.0));
        face2.hold_offset(part, 0);
        let mut seq = 0u64;
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let mut events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            for i in 0..57u64 {
                seq += 1;
                events.push(obs(100 + (i % 4) as u32, seq, now, i as f64));
            }
            face2.on_obs_batch(&events, now);
        }
        assert!(
            face2.consumer_commits >= 1,
            "物化仍然发生（持有只影响保留）"
        );
        assert!(
            face2.retention_hold_limited >= 1,
            "持有必须是此刻的约束方（证据计数）"
        );
        // 持有 → 删除界收紧的**耦合**断言（复查补口：原测只断言计数，
        // 忽略持有仍可全绿）：持有 offset 0 = 保护一切 → 零删除。
        //
        // 变异靶：`let keep = committed;`（持有不参与 min）→ face2 修剪出段
        // → 本断言红。
        assert_eq!(
            face2.pruned_segments, 0,
            "持有 offset=0 时必须零删除（持有必须真的进入 min 判据）"
        );
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
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| obs(p, p as u64, 2_000, fixture_value(p, 0)))
                    .collect();
                face.on_obs_batch(&events, 2_000);
                assert!(face.is_open());
            })
            .expect("spawn");
        h.join()
            .expect("open+append path must survive at provisioned stack");
    }
    #[test]
    fn snapshot_reads_have_bounded_block_roundtrips() {
        let storage = FakeStorage::new((DISK_END * 512) as usize);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0;
        for round in 1..=SNAP_EVERY_BATCHES {
            let events: Vec<_> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 1_000 + round, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 1_000 + round);
        }
        let id = face
            .current_input()
            .expect("frozen input")
            .facts()
            .snapshot_id;
        let expected = face.snapshot_blob(&id).unwrap();
        let start = storage.cmds().len();
        for _ in 0..4 {
            assert_eq!(face.snapshot_blob(&id).unwrap(), expected);
        }
        let calls = storage.cmds().len() - start;
        // Four blob fetches share an unchanged tiny volume. At most four full
        // 4 KiB reads (10 EIPC fragments each) per fetch is the storage budget.
        assert!(
            calls <= 160,
            "repeated snapshot reads used {calls} block RPCs"
        );
        drop(face);
        let mut reopened = open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).unwrap();
        let path = alloc::format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(&id));
        assert_eq!(read_snap_file(&mut reopened, &path).unwrap(), expected);
    }
}
