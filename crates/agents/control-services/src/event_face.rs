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
use eneros_fs::lfs::{RegionGeometry, LFS_BLOCK_SIZE};
use eneros_fs::{FileSystem, FsError, Lfs};
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

/// 落盘工件种类（蓝图 78 WP-78.1.1 故障注入缝的定位标签；生产逻辑不读）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapArtifact {
    Blob,
    Meta,
}

/// 窄写单点（蓝图 78 WP-78.1.1）：blob/meta 落盘的一切 `File::write`
/// 调用收拢于此——`File::write` 返回**实际写入字节数**（file.rs:95-118），
/// 短写/失败由调用方裁决。主机故障注入面只在本函数（cfg(test)）：
/// 生产构造零开销直通，测试按工件种类注入短写/写失败，物理介质承载
/// 真实残迹（写前缀 = 文件里真的只有前缀——不伪造上层「store 成功」）。
#[allow(unused_variables)] // which 仅测试注入面消费
fn snap_file_write(
    fs: &mut Lfs,
    f: &mut eneros_fs::File,
    which: SnapArtifact,
    bytes: &[u8],
) -> Result<usize, ()> {
    #[cfg(test)]
    if let Some(outcome) = write_fault::intercept(which) {
        match outcome {
            write_fault::Intercept::Fail => return Err(()),
            write_fault::Intercept::Short(n) => {
                let n = n.min(bytes.len());
                // 物理写入前缀：介质真相 = 文件确实只含 n 字节。
                f.write(fs, &bytes[..n]).map_err(|_| ())?;
                return Ok(n);
            }
        }
    }
    f.write(fs, bytes).map_err(|_| ())
}

/// 主机故障注入登记表（cfg(test) 专用；生产构建零存在）。thread_local：
/// 测试线程各自独立注入态——并行测试零互扰（共享静态曾污染非注入测试）。
#[cfg(test)]
mod write_fault {
    use core::cell::RefCell;

    #[derive(Default)]
    struct State {
        blob_short: Option<usize>,
        meta_short: Option<usize>,
        blob_fail: usize,
        meta_fail: usize,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
    }

    pub enum Intercept {
        Fail,
        /// 短写：只写 n 字节（n=0 = Ok(0) 零进展形态）。
        Short(usize),
    }

    /// 取走一次注入（短写优先于失败；未登记 = 不注入）。
    pub fn intercept(which: super::SnapArtifact) -> Option<Intercept> {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let State {
                blob_short,
                meta_short,
                blob_fail,
                meta_fail,
            } = &mut *s;
            let (short, fail) = match which {
                super::SnapArtifact::Blob => (blob_short, blob_fail),
                super::SnapArtifact::Meta => (meta_short, meta_fail),
            };
            if let Some(n) = short.take() {
                // usize::MAX 哨兵 = Ok(0) 零进展形态（复查轮 F3：thread_local
                // 重写曾丢此映射——t21 的注入退化成整长写入 no-op）。
                let n = if n == usize::MAX { 0 } else { n };
                return Some(Intercept::Short(n));
            }
            if *fail > 0 {
                if *fail != usize::MAX {
                    *fail -= 1;
                }
                return Some(Intercept::Fail);
            }
            None
        })
    }

    pub fn arm_blob_short(n: usize) {
        STATE.with(|s| s.borrow_mut().blob_short = Some(n));
    }
    pub fn arm_meta_short(n: usize) {
        STATE.with(|s| s.borrow_mut().meta_short = Some(n));
    }
    pub fn arm_meta_fail(n: usize) {
        STATE.with(|s| s.borrow_mut().meta_fail = n);
    }
    pub fn arm_meta_fail_forever() {
        arm_meta_fail(usize::MAX);
    }
}

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
    /// 快照保留钉（ADR-0054:29–30）：被持有计划证书引用的冻结快照
    /// （完整观测 blob）在持有期内不得被剪枝回收。幂等；false = 容量
    /// 满/面不可用（调用方 fail-closed：拒绝新计划准入）。
    fn pin_snapshot(&mut self, id: &[u8; 16]) -> bool;
    /// 解除快照保留钉（计划终态且无未决后）。幂等；false = 未钉/面不可
    /// 用（释放是回收面不是安全面，可重试）。
    fn release_snapshot(&mut self, id: &[u8; 16]) -> bool;
    /// 完整活跃钉集同步（蓝图 77 卷一 WP-77.1.3 进程世代屏障）：原子
    /// 替换本进程钉集为 `ids`（逐 ID 验证现存 blob；任一失败 ⇒ 整体
    /// 拒绝）。成功前本进程不做可能删除旧 blob/事件的修剪。空集 = 显式
    /// 「无活跃持有」，同样解锁屏障。
    // 错误类型留空刻意为之（与 store_snap_blob 同族——失败面二元，细节走
    // 证据行）。
    #[allow(clippy::result_unit_err)]
    fn sync_snap_pins(&mut self, ids: &[[u8; 16]]) -> Result<(), ()>;
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
    /// 蓝图 78 卷一 WP-78.1.2：保留状态唯一所有者（钉集/区间缓存/世代
    /// 屏障——旧三条平行字段的直接改写面退役）。
    retention: SnapshotRetention,
}

/// 快照钉容量（单源 = agent-sdk `tel_wire::SNAP_SYNC_MAX_IDS`：持有计划
/// 容量 MAX_PLANS(2) + 换代余量 2；超出 = 调用方 fail-closed 拒绝新准入
/// ——不留无限增长面）。
pub const MAX_SNAP_PINS: usize = eneros_agent_sdk::tel_wire::SNAP_SYNC_MAX_IDS;

/// 快照 ID。
pub type SnapId = [u8; 16];
/// 来源区间（每分区最早仍需事件 offset）——`None` = 旧格式 blob 无 meta
/// （fail-closed 全保护）。
pub type SnapRanges = Option<Vec<(u32, u64)>>;
/// 蓝图 78 卷一 WP-78.1.2：快照保留状态私有深 module。同一保留承诺的
/// 验证、同步、发布与删除资格收进同一所有者——`pin/sync/release` 的
/// 状态迁移、事件保护界（[`Self::floor_for`]）、剪枝保护集合全部由此
/// 派生；`ObsEventFace` 仍是外部 interface，`Lfs`/`EventStore` 仍是现有
/// adapter（fs/store 按需分字段借入）。删除本 module 会使资格判据散回
/// 多个调用点（删除测试锁此性质）。
#[derive(Default)]
struct SnapshotRetention {
    /// 存活钉集（id → 来源区间缓存；缓存 `None` = 遗留全保护条目）。
    /// 唯一事实源仍是 dispatch 的持久计划——本集合是远端 RAM 派生态，
    /// 由保留同步协议维护（蓝图 77 卷一 WP-77.1.3）。
    pins: Vec<PinnedSnapshot>,
    /// 进程世代同步屏障：未收到本世代完整活跃集全量确认前不做可能删除
    /// 旧 blob/事件的修剪。
    synced: bool,
}

/// 钉住条目（ID + 来源区间缓存；私有深 module 的所有权形态）。
#[derive(Clone)]
struct PinnedSnapshot {
    id: SnapId,
    ranges: SnapRanges,
}

/// 新钉准入的 meta/来源核验结论（[`EventFace::verify_admission_meta`]）。
enum AdmissionMeta {
    /// 区间缓存入账（保护界由此重算）。
    Ranges(SnapRanges),
    /// 真遗留形态：接受钉 + 全保护条目（`None`）。
    LegacyFull(SnapRanges),
    /// 拒绝（可 grep 的 reason 词形：meta-corrupt / meta-io-fault /
    /// source-events-gone）。
    Reject(alloc::string::String),
}

impl SnapshotRetention {
    fn contains(&self, id: &SnapId) -> bool {
        self.pins.iter().any(|p| p.id == *id)
    }

    fn len(&self) -> usize {
        self.pins.len()
    }

    /// 存活钉集 ID（剪枝保护集合的来源）。
    fn pin_ids(&self) -> Vec<SnapId> {
        self.pins.iter().map(|p| p.id).collect()
    }

    /// 进程世代屏障（true = 已确认，可修剪）。
    fn synced(&self) -> bool {
        self.synced
    }

    /// 存活钉集的每分区事件保护界（蓝图 77 卷一 WP-77.1.4：由**当前**
    /// 钉集重算——释放后上移；遗留全保护条目在场 ⇒ 0；无钉 = `u64::MAX`
    /// = 不约束（退化为消费位点判据））。
    fn floor_for(&self, partition: u32) -> u64 {
        let mut floor = u64::MAX;
        for pin in &self.pins {
            match &pin.ranges {
                None => return 0,
                Some(rs) => {
                    for &(p, start) in rs.iter() {
                        if p == partition && start < floor {
                            floor = start;
                        }
                    }
                }
            }
        }
        floor
    }

    /// 解除快照保留钉（幂等；false = 未钉）。按同一 ID 同位去钉与来源
    /// 区间（t16：两侧严格同位配对，非末位释放不错删）。
    fn release(&mut self, id: &SnapId) -> bool {
        self.pins
            .iter()
            .position(|p| p.id == *id)
            .is_some_and(|idx| {
                // 同位删除（n≤4，移位成本可忽略）：swap_remove 会使 ID 与
                // 区间在非末位释放后错位，第二次释放按现位索引错删他人
                // 区间（蓝图 77 复查轮 R4 同律）。
                self.pins.remove(idx);
                true
            })
    }
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
            consumer_commits: 0,
            retention_prune_passes: 0,
            retention_hold_limited: 0,
            retention_blocked: false,
            retention: SnapshotRetention::default(),
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
        // R16-M（WP-75.3.3）：512B→4K 几何折算单源（RegionGeometry——
        // 原 `/ 8` 算术惯用法退役）。
        let disk_4k =
            RegionGeometry::from_blocks_512(0, eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS)
                .expect("数据盘 512B→4K 几何");
        let event_geo = RegionGeometry::from_blocks_512(self.event_base_512, self.event_blocks_512)
            .expect("事件区段 512B→4K 几何对齐");
        // R16-L（WP-75.3.2）：attach 期 health 探针构造（+1 EIPC 往返）。
        let face = EipcBlockDevice::new_probed(
            io_event,
            eneros_event_store::format::BLOCK_SIZE,
            disk_4k.blocks,
        );
        let base_4k = event_geo.base_block;
        let blocks_4k = event_geo.blocks;
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
                    let part = eneros_event_store::ingest_bridge::partition_of(ev);
                    if let Some(core) = face.module.as_mut() {
                        // WP-77.1.4：带日志落点流入（投影落点镜像 = 快照
                        // 来源区间的事实源）。
                        core.module.apply_event_at(*ev, part, ack.offset);
                        // 蓝图 70.8.2：消费位置只记「真的进了快照模块」的
                        // offset（apply_event 之后）——提交位点即由此派生。
                        core.mark_consumed(part, ack.offset);
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
                &self.retention,
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
        read_and_verify_snap(fs, id)
    }

    /// 快照保留钉（ADR-0054:29–30）：被持有计划证书引用的冻结快照
    /// （完整观测 blob）在持有期内不得被剪枝回收。幂等；false = 容量
    /// 满/面不可用/验证失败（调用方 fail-closed：拒绝新计划准入）。
    fn pin_snapshot(&mut self, id: &[u8; 16]) -> bool {
        // 蓝图 77 卷一 WP-77.1.3：入钉前**验证现存文件**（同 snapshot_blob
        // 的读、解码、ID 对比语义）——「认证已拿 blob → 中间剪枝 → hold」
        // 不再产出幽灵钉。校验与入集在同一请求内完成（telemetry 单线程
        // 服务：请求间不可插队，本服务的剪枝不会落在校验与入集之间）。
        let FaceState::Open(face) = &mut self.state else {
            eneros_runtime::println!("[svc-telemetry] SNAP-PIN-REJECT reason=face-not-open");
            return false;
        };
        let Some(fs) = face.snap_fs.as_mut() else {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-REJECT reason=snap-volume-unavailable"
            );
            return false;
        };
        if read_and_verify_snap(fs, id).is_none() {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-REJECT reason=blob-missing-or-corrupt"
            );
            return false;
        }
        // 蓝图 78 §2.2：重复 pin 走**同一** blob/meta/来源核验，且不得把
        // 缓存的 Some(ranges) 降为 None（meta 消失/读错 = 身份不可证 ⇒
        // 拒新准入，旧条目与缓存保护界保留待排障）。
        if self.retention.contains(id) {
            let verdict = Self::reverify_pinned(fs, face.sink.store_mut(), id, &self.retention);
            if !verdict {
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-PIN-REJECT reason=repin-unverified pins={}",
                    self.retention.len()
                );
            }
            return verdict;
        }
        if self.retention.len() >= MAX_SNAP_PINS {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-REJECT reason=pin-capacity pins={}",
                self.retention.len()
            );
            return false;
        }
        match Self::verify_admission_meta(fs, face.sink.store_mut(), id) {
            AdmissionMeta::Ranges(r) | AdmissionMeta::LegacyFull(r) => {
                self.retention
                    .pins
                    .push(PinnedSnapshot { id: *id, ranges: r });
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-PIN-HELD id={:02x}{:02x} pins={}",
                    id[0],
                    id[1],
                    self.retention.len()
                );
                true
            }
            AdmissionMeta::Reject(reason) => {
                eneros_runtime::println!("[svc-telemetry] SNAP-PIN-REJECT reason={reason}");
                false
            }
        }
    }

    fn sync_snap_pins(&mut self, ids: &[[u8; 16]]) -> Result<(), ()> {
        // 蓝图 77 卷一 WP-77.1.3：进程世代屏障的唯一解锁面——原子替换
        // 钉集。蓝图 78 §2.2：单趟核验（blob + meta 三态 + 来源事件段在
        // 位），任一失败 ⇒ 整体拒绝、旧集与 `synced` 不变（过度保留安
        // 全，误清不安全）；成功（含空集）= 显式解锁屏障。
        let mut deduped: Vec<[u8; 16]> = Vec::with_capacity(ids.len());
        for id in ids {
            if !deduped.contains(id) {
                deduped.push(*id);
            }
        }
        // 服务端自守容量门（与 pin_snapshot 对称；t17）：有界性不得只依赖
        // wire 解帧门——trait 直调传超容集必须整体拒绝、钉集保持原样。
        if deduped.len() > MAX_SNAP_PINS {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-SYNC-REJECT reason=pin-capacity ids={}",
                deduped.len()
            );
            return Err(());
        }
        let mut new_pins: Vec<PinnedSnapshot> = Vec::with_capacity(deduped.len());
        {
            let FaceState::Open(face) = &mut self.state else {
                return Err(());
            };
            let Some(fs) = face.snap_fs.as_mut() else {
                return Err(());
            };
            for id in &deduped {
                if read_and_verify_snap(fs, id).is_none() {
                    eneros_runtime::println!(
                        "[svc-telemetry] SNAP-PIN-SYNC-REJECT reason=blob-missing-or-corrupt ids={}",
                        ids.len()
                    );
                    return Err(());
                }
                // 蓝图 78 §2.2（复查轮 F5 对称化）：旧集内缓存 Some(ranges)
                // 的 ID 在 meta **真消失**（确凿 NotFound）时，不得被全量
                // 同步降级成遗留全保护接纳——与 pin 的 reverify_pinned 同
                // 判据（整体拒绝、旧集保持待排障）。方向上降级=过度保留
                // （安全），但 §2.2 字面禁降级，两入口必须同律。
                if self.retention.contains(id)
                    && self
                        .retention
                        .pins
                        .iter()
                        .find(|p| p.id == *id)
                        .map(|p| p.ranges.is_some())
                        .unwrap_or(false)
                    && matches!(probe_snap_meta(fs, id), MetaProbe::LegacyAbsent)
                {
                    eneros_runtime::println!(
                        "[svc-telemetry] SNAP-PIN-SYNC-REJECT reason=meta-vanished-while-pinned ids={}",
                        ids.len()
                    );
                    return Err(());
                }
                match Self::verify_admission_meta(fs, face.sink.store_mut(), id) {
                    AdmissionMeta::Ranges(r) | AdmissionMeta::LegacyFull(r) => {
                        new_pins.push(PinnedSnapshot { id: *id, ranges: r });
                    }
                    AdmissionMeta::Reject(reason) => {
                        eneros_runtime::println!(
                            "[svc-telemetry] SNAP-PIN-SYNC-REJECT reason={reason} ids={}",
                            ids.len()
                        );
                        return Err(());
                    }
                }
            }
        }
        self.retention.pins = new_pins;
        self.retention.synced = true;
        eneros_runtime::println!(
            "[svc-telemetry] SNAP-PIN-SYNC n={} pins={}",
            ids.len(),
            self.retention.len()
        );
        Ok(())
    }

    fn release_snapshot(&mut self, id: &[u8; 16]) -> bool {
        if self.retention.release(id) {
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-RELEASED id={:02x}{:02x} pins={}",
                id[0],
                id[1],
                self.retention.len()
            );
            true
        } else {
            // F70-04（§7.2.5）幂等确认：钉不在场 = "确认已无本次持有"
            // ——按成功收敛（远端已释放/回执丢失后的重试不得把 absent
            // 永远当未知失败）。不触碰任何其他钉（新持有人不受影响）。
            eneros_runtime::println!(
                "[svc-telemetry] SNAP-PIN-RELEASE-CONFIRMED-ABSENT id={:02x}{:02x}",
                id[0],
                id[1]
            );
            true
        }
    }

    fn retention_blocked(&self) -> bool {
        self.retention_blocked
    }
}

impl<C: EipcSend + Clone + 'static> EventFace<C> {
    /// 新钉准入的 meta/来源核验（蓝图 78 §2.2/§2.3）：meta 三态判别 +
    /// 来源事件段在位。`Ranges` = 区间缓存；`LegacyFull` = 真遗留（无
    /// meta，全保护条目 `None`——仅确凿 `NotFound` 可达）；`Reject` =
    /// 形态坏/读故障/来源事件已失（reason = 可 grep 词形）。
    fn verify_admission_meta(
        fs: &mut Lfs,
        store: &mut FaceStore<C>,
        id: &[u8; 16],
    ) -> AdmissionMeta {
        match probe_snap_meta(fs, id) {
            MetaProbe::Ranges(r) => {
                // 「缺少范围或相应事件已不存在时拒绝新持有，而非承诺一个
                // 空 offset」（WP-77.1.4 判据保持）。
                if !ranges_present_in_store(store, &r) {
                    return AdmissionMeta::Reject(alloc::string::String::from(
                        "source-events-gone",
                    ));
                }
                AdmissionMeta::Ranges(Some(r))
            }
            // 真遗留（确凿 NotFound）：接受钉 + 全保护（REG-77-F 双版本
            // 兼容——重启后没有可恢复的格式证据时只能按真遗留处理）。
            MetaProbe::LegacyAbsent => AdmissionMeta::LegacyFull(None),
            MetaProbe::Corrupt => {
                AdmissionMeta::Reject(alloc::string::String::from("meta-corrupt"))
            }
            // I/O/坏块/不确定 = 状态未知：拒绝（故障不得冒充遗留格式），
            // 旧钉/旧 meta/旧事件保留 + 重试机会（蓝图 78 §2.3）。
            MetaProbe::IoFault => {
                AdmissionMeta::Reject(alloc::string::String::from("meta-io-fault"))
            }
        }
    }

    /// 已钉条目的同 ID 再核验（蓝图 78 §2.2）：身份可证才放行新准入；
    /// 任何降级形态（缓存有区间而 meta 消失/读错/形态坏）= 拒新准入，
    /// 旧条目与缓存保护界原样保留（本函数不触碰钉集）。
    fn reverify_pinned(
        fs: &mut Lfs,
        store: &mut FaceStore<C>,
        id: &[u8; 16],
        retention: &SnapshotRetention,
    ) -> bool {
        let cached_ranges = retention
            .pins
            .iter()
            .find(|p| p.id == *id)
            .and_then(|p| p.ranges.clone());
        match (&cached_ranges, probe_snap_meta(fs, id)) {
            // 遗留条目幂等（缓存 None + meta 仍缺席）。
            (None, MetaProbe::LegacyAbsent) => true,
            // 有区间缓存：meta 仍须在场可解 + 来源事件仍在位。
            (Some(_), MetaProbe::Ranges(_)) | (None, MetaProbe::Ranges(_)) => {
                matches!(
                    Self::verify_admission_meta(fs, store, id),
                    AdmissionMeta::Ranges(_)
                )
            }
            // 缓存 Some(ranges) 而 meta 消失 = 不得降级成遗留全保护。
            (Some(_), MetaProbe::LegacyAbsent) => false,
            // 读故障/形态坏 = 身份不可证 = 拒新准入。
            (_, MetaProbe::Corrupt) | (_, MetaProbe::IoFault) => false,
        }
    }

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
        retention: &SnapshotRetention,
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
                // 蓝图 77 卷一 WP-77.1.4：本快照的来源区间（每分区最早仍需
                // 事件 offset）——出自在驻事件落点镜像，非消费位点。
                let floors: Vec<(u32, u64)> = core.module.provenance_floors().into_iter().collect();
                (Some((snap, input, floors)), consumed)
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
            Some((snap, _, floors)) => {
                let bytes = eneros_snapshot_service::encode_snapshot_v1(snap);
                eneros_runtime::println!(
                    "[svc-telemetry] SNAP-BUILT pts={} bytes={}",
                    snap.observations.len(),
                    bytes.len()
                );
                match face.snap_fs.as_mut() {
                    Some(fs) => {
                        // WP-77.1.4 发布次序：blob → meta → 发布/位点提交——
                        // 「可被 pin 的 ID 必有可查来源区间」；meta 失败 =
                        // 本窗不发布（与 blob store-fail 同一失败语义）。
                        // end = 构建时每分区已消费位点（真实取证边界）。
                        let ends: alloc::collections::BTreeMap<u32, u64> =
                            consumed.iter().copied().collect();
                        let meta_ranges: Vec<(u32, u64, u64)> = floors
                            .iter()
                            .map(|&(p, start)| (p, start, ends.get(&p).copied().unwrap_or(start)))
                            .collect();
                        // 蓝图 78 §2.5：写前腾位的保护集合 = 存活钉集 ∪
                        // **当前已发布 ID**（旧发布工件在新工件可发布前不
                        // 可删——可发布性不变量的临时资格）。
                        let mut protected = retention.pin_ids();
                        if let Some(cur) = face.module.as_ref().and_then(|c| c.last_input) {
                            let pid = *cur.snapshot_id();
                            if !protected.contains(&pid) {
                                protected.push(pid);
                            }
                        }
                        match store_snap_blob(fs, snap, &bytes, &protected, retention.synced())
                            .and_then(|kept| {
                                store_snap_meta(fs, &snap.snapshot_id, &meta_ranges).map(|_| kept)
                            }) {
                            Ok(kept) => {
                                *snap_stored += 1;
                                eneros_runtime::println!(
                                    "[svc-telemetry] SNAP-STORED kept={}",
                                    kept
                                );
                                true
                            }
                            Err(()) => {
                                *snap_skipped += 1;
                                eneros_runtime::println!("[svc-telemetry] SNAP-SKIP store-fail");
                                false
                            }
                        }
                    }
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
            let (_, input, _) = snap.expect("stored ⇒ snap 存在");
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
        // 蓝图 77 卷一 WP-77.1.3：进程世代同步屏障——本进程未收到完整活跃
        // 钉集的全量确认前，不做可能删除旧事件段的修剪（钉集未知 ⇒ 任何
        // 删除都不安全）；水位越线时显式背压（调用方停发新决策），不以
        // "钉集可能没丢"为赌注。
        let wm_before: WatermarkReport = face.sink.store_mut().watermarks();
        if wm_before.used_permille >= RETAIN_PERMILLE && !retention.synced() {
            *retention_prune_passes += 1;
            if !*retention_blocked {
                eneros_runtime::println!(
                    "[svc-telemetry] EVENT-RETENTION blocked=1 reason=pin-sync-pending permille={}",
                    wm_before.used_permille
                );
            }
            *retention_blocked = true;
            return;
        }
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
                // WP-77.1.4：事件保留界 = 当前存活钉集来源区间的每分区
                // 最早 offset（**重算**——释放后上移；不再复用只紧不松的
                // hold_offset）。旧格式 blob（无 meta）钉在场 ⇒ 全保护
                // （fail-closed 过度保留，可恢复）。
                let hold = retention.floor_for(p);
                let keep = core::cmp::min(committed, hold);
                if hold < committed {
                    hold_limited += 1; // 计划来源持有是此刻的约束方（证据）
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
    // R16-M（WP-75.3.3）：几何折算单源（原本地 FS_BLOCK_SIZE const +
    // `% 8`/`/ 8` assert + 折算全部退役；expect 保持原 assert 的 panic
    // 力度，行为等价）。
    let snap_geo = RegionGeometry::from_blocks_512(base_block_512, region_blocks_512)
        .expect("快照区段基址/长度必须与 LFS 4096B 几何对齐");
    let disk_4k = RegionGeometry::from_blocks_512(0, eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS)
        .expect("数据盘 512B→4K 几何");
    // R16-L（WP-75.3.2）：attach 期 health 探针构造。
    let face = EipcBlockDevice::new_probed(io, LFS_BLOCK_SIZE, disk_4k.blocks);
    // 卷几何 = min(区段 4KiB 块数, 64)——BlockDeviceStorage BLOCK_COUNT=64
    // 编译期上限（钳制是策略，留调用方）。
    let base_4k = snap_geo.base_block;
    let blocks_4k = snap_geo.blocks.min(64);
    // mount-or-format 单源（2026-09-24 收口 C13）：缓存深度 4（区段直映射
    // 读缓存——本面快照读为主）与区段几何显式传入。
    eneros_fs::mount_or_format(|| {
        RegionScope::new(CachedBlocks::<4, _>::new(face.clone()), base_4k, blocks_4k).ok()
    })
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
///
/// 蓝图 77 卷一 WP-77.1.3：`synced = false`（进程世代屏障未解锁）时
/// **跳过一切剪枝删除**（腾位剪枝与终剪）——钉集未知 ⇒ 任何 blob 删除
/// 都可能吃掉 dispatch 在途计划的冻结输入；代价 = 满卷时新 blob 写失败
/// （SNAP-SKIP store-fail，控制链安全饿等同步），符合"安全条件优先于
/// 可用性"。
fn store_snap_blob(
    fs: &mut Lfs,
    snap: &eneros_snapshot_service::TrustedSnapshot,
    bytes: &[u8],
    protected: &[SnapId],
    synced: bool,
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
        // 蓝图 78 §2.4 完整写入：短写补全/零进展失败/close、rename 逐环
        // 裁决 + **写后取回核验**（rename 成功 ≠ 工件完整）。
        write_snap_file_all(fs, SnapArtifact::Blob, &tmp, &path, bytes)?;
        if read_and_verify_snap(fs, &snap.snapshot_id).is_none() {
            // 取回不可解 = 残迹已上线 ⇒ 撤下（失败文件不可发布）。
            let _ = fs.remove(&path);
            return Err(());
        }
        Ok(())
    };
    let mut keep = SNAP_KEEP_BLOBS.saturating_sub(1);
    loop {
        if synced {
            let _ = prune_snap_blobs_keep(fs, keep, protected);
        }
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
    if !synced {
        // 屏障未解锁：不删除，返回现存计数（kept 仅作证据打印）。
        return Ok(count_snap_blobs(fs));
    }
    let out = prune_snap_blobs_keep(fs, SNAP_KEEP_BLOBS, protected);
    if out.unknown > 0 {
        // 删除面不确定 = kept 证据不许撒谎（蓝图 78 §2.4：可区别词形）。
        eneros_runtime::println!("[svc-telemetry] SNAP-PRUNE-UNKNOWN n={}", out.unknown);
    }
    Ok(out.kept)
}

/// 修剪结果账（蓝图 78 §2.4/验收矩阵「诊断与容量」行）：删除面区分
/// **已删（确凿）/未知（remove 失败，状态不明）**——kept 只计确凿事实，
/// 不许把未知折叠成达标。
struct PruneOutcome {
    /// 修剪后仍在场的 blob 数（含保护条目；确凿）。
    kept: usize,
    /// 确凿删除的 **blob** 数（kept 的唯一减项——meta 删除单独分账，
    /// 防混入下溢：孤儿 meta 的对端 blob 不在 snap_total 内，复查轮
    /// F1 实证 debug 下溢 panic / release 回绕虚假 kept）。
    removed: usize,
    /// 确凿删除的 meta 数（证据，不参与 kept 算术）。
    meta_removed: usize,
    /// remove 返回错误 = 状态未知（文件可能仍在）——过度保留，可诊断。
    unknown: usize,
}

/// 保留上限参数化版本：写前腾位需要"比最终保留数少一份"的目标，
/// 重试路径需要"砍到一半"。语义与 [`SNAP_KEEP_BLOBS`] 版逐字相同。
fn prune_snap_blobs_keep(fs: &mut Lfs, keep: usize, protected: &[SnapId]) -> PruneOutcome {
    let mut out = PruneOutcome {
        kept: 0,
        removed: 0,
        meta_removed: 0,
        unknown: 0,
    };
    let Ok(entries) = fs.readdir(SNAP_DIR) else {
        return out; // 目录读失败 = 不删任何东西（过度保留）。
    };
    // 保护集合（蓝图 78 §2.2）：存活钉集 ∪ 当前已发布 ID——路径面。
    let protected_paths: Vec<String> = protected
        .iter()
        .map(|id| format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(id)))
        .collect();
    let mut metas: Vec<(String, u64)> = Vec::new();
    // WP-77.1.4：meta 与 blob 同生命周期——被删 blob 的 meta 一并删除，
    // 孤儿 meta（blob 已不在，如掉电于两步删除之间）随 prune 清理，
    // 否则快照卷被无形积累撑满（SNAP-SKIP store-fail 重现）。
    let mut orphan_meta_paths: Vec<String> = Vec::new();
    let mut snap_total = 0usize;
    for e in entries.iter().filter(|e| !e.is_dir) {
        let path = format!("{}/{}", SNAP_DIR, e.name);
        if e.name.ends_with(".tmp") {
            let _ = fs.remove(&path);
            continue;
        }
        if e.name.ends_with(".meta") {
            // 先记候选：blob 侧扫完后按**文件存在性真相**裁决。
            orphan_meta_paths.push(path);
            continue;
        }
        if !e.name.ends_with(".snap") {
            continue;
        }
        snap_total += 1;
        let produced = read_snap_file(fs, &path).ok().and_then(|b| {
            eneros_snapshot_service::decode_snapshot_v1(&b)
                .ok()
                .map(|s| s.produced_at_ns)
        });
        match produced {
            Some(produced) => metas.push((path, produced)),
            None => {
                // 复查轮 F5：不可读 blob 先于钉守卫被删 = 瞬态读失败即可
                // 毁掉持有计划的冻结输入（ADR-0054 违约， prune 恰在高载
                // 高故障相关期运行）。保护集合（钉 ∪ 已发布）的 blob 一律
                // 保留（过度保留安全；真损坏 blob 的回收裁决在 release
                // 之后）。
                if protected_paths.contains(&path) {
                    continue;
                }
                if fs.remove(&path).is_ok() {
                    out.removed += 1;
                } else {
                    out.unknown += 1;
                }
            }
        }
    }
    // 孤儿判定（蓝图 78 §2.2/§2.3，t22 红测锁）：meta 的对端 blob 用
    // **文件存在性真相**裁决——确凿 `NotFound` 且不在保护集才删 meta；
    // blob 在场（哪怕暂不可读）或 stat 不确定 = 过度保留。旧实现按
    // 「可读 blob 干集」判定 ⇒ 已钉 blob 暂不可读时其 meta 被误删
    //（同 ID 从有来源区间退化成遗留无 meta 全保护——丢取证界）。
    for mpath in orphan_meta_paths {
        let stem = mpath.trim_end_matches(SNAP_META_PATH_SUFFIX);
        let blob_path = alloc::format!("{stem}.snap");
        if let Err(FsError::NotFound { .. }) = fs.stat(&blob_path) {
            let protected_blob = protected_paths.iter().any(|p| p.as_str() == blob_path);
            if !protected_blob && fs.remove(&mpath).is_ok() {
                out.meta_removed += 1;
            } else if !protected_blob {
                out.unknown += 1;
            }
        }
        // 其余臂（在场/不确定）⇒ meta 过度保留，不删。
    }
    // ADR-0054:29–30 + 蓝图 78 §2.5：保护集合（钉 ∪ 已发布）不参与剪枝
    // ——越过 keep 线也保留（回收面的最终裁决在 release/新发布之后）。
    if metas.len() > keep {
        metas.sort_by_key(|(_, produced)| core::cmp::Reverse(*produced));
        for (path, _) in metas[keep..].iter() {
            if protected_paths.contains(path) {
                continue;
            }
            // blob 删除必须确凿才联动删 meta——remove 未知时 meta 保留
            // （孤儿清扫下一轮按存在性真相收敛）。
            if fs.remove(path).is_ok() {
                out.removed += 1;
                // 复查轮 F1：联动删名字必须与 store_snap_meta 的落点同构
                //（`<stem>.meta`，不是 `<stem>.snap.meta`）。meta 删除独立
                // 分账（不进 kept 减项）。
                let meta_path = format!("{}.meta", path.trim_end_matches(".snap"));
                if fs.remove(&meta_path).is_ok() {
                    out.meta_removed += 1;
                } else {
                    out.unknown += 1;
                }
            } else {
                out.unknown += 1;
            }
        }
    }
    out.kept = snap_total - out.removed;
    out
}

/// 读整个文件（stat 定长 → open READ 循环读满；tsdb 同法）。蓝图 78
/// §2.3：**错误保持 `FsError` 分类原样透出**——`NotFound` 与 I/O/坏块的
/// 判别权在调用方（meta 三态），本函数不再压平为 `()`。
fn read_snap_file(fs: &mut Lfs, path: &str) -> Result<Vec<u8>, FsError> {
    let stat = fs.stat(path)?;
    let mut buf = alloc::vec![0u8; stat.size as usize];
    let mut f = fs.open(path, eneros_fs::types::OpenFlags::READ)?;
    let mut total = 0usize;
    while total < buf.len() {
        let n = f.read(fs, &mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    buf.truncate(total);
    Ok(buf)
}

/// 来源区间元数据 magic/版本（蓝图 77 卷一 WP-77.1.4；格式变更须升版
/// 并做双版本读取判定）。
const SNAP_META_MAGIC: &[u8; 4] = b"SRM1";
const SNAP_META_PATH_SUFFIX: &str = ".meta";

/// 来源区间元数据编码：`"SRM1" | count u8 | {partition u32 LE, start u64
/// LE, end u64 LE} ×count`——start = 每分区最早仍需事件 offset（provenance
/// 下界），end = 构建时已消费位点（取证边界）。小端定长，无变长域。
fn encode_snap_meta(ranges: &[(u32, u64, u64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 1 + ranges.len() * 20);
    out.extend_from_slice(SNAP_META_MAGIC);
    out.push(ranges.len() as u8);
    for (p, start, end) in ranges {
        out.extend_from_slice(&p.to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
    }
    out
}

/// 来源区间元数据解码：形态不符 → `None`（调用方按损坏拒绝）。
fn decode_snap_meta(bytes: &[u8]) -> Option<Vec<(u32, u64, u64)>> {
    if bytes.len() < 5 || &bytes[..4] != SNAP_META_MAGIC {
        return None;
    }
    let count = bytes[4] as usize;
    if bytes.len() != 5 + count * 20 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = 5 + i * 20;
        let p = u32::from_le_bytes(bytes[at..at + 4].try_into().ok()?);
        let start = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().ok()?);
        let end = u64::from_le_bytes(bytes[at + 12..at + 20].try_into().ok()?);
        if start > end {
            return None;
        }
        out.push((p, start, end));
    }
    Some(out)
}

/// meta 落盘（tmp + rename 原子替换 + 写后取回核验，blob 同律）。失败 =
/// 本窗不发布（「可被 pin 的 ID 必有可查来源区间」的发布次序保障）。
/// 蓝图 78 §2.4：短写补全/零进展/close、rename 逐环裁决；取回必须解回
/// 同一区间——rename 成功不冒充工件完整。
fn store_snap_meta(fs: &mut Lfs, id: &[u8; 16], ranges: &[(u32, u64, u64)]) -> Result<(), ()> {
    let _ = fs.mkdir(SNAP_DIR);
    let name = snapshot_file_stem(id);
    let tmp = format!("{}/{}{}.tmp", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
    let path = format!("{}/{}{}", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
    let bytes = encode_snap_meta(ranges);
    write_snap_file_all(fs, SnapArtifact::Meta, &tmp, &path, &bytes)?;
    match read_snap_file(fs, &path) {
        Ok(read_back) if decode_snap_meta(&read_back).as_deref() == Some(ranges) => Ok(()),
        _ => {
            // 取回不可解/区间不符 = 残迹已上线 ⇒ 撤下（失败文件不可发布）。
            let _ = fs.remove(&path);
            Err(())
        }
    }
}

/// 完整写入窄 helper（蓝图 78 §2.4）：create → 循环写满（短写补全；
/// `Ok(0)` = 零进展即失败，有界不清盘）→ close（错误 = 工件不可信）→
/// rename。任何一环失败都清临时文件——半成品永不可见。
fn write_snap_file_all(
    fs: &mut Lfs,
    which: SnapArtifact,
    tmp: &str,
    path: &str,
    bytes: &[u8],
) -> Result<(), ()> {
    let mut f = fs
        .create(tmp, eneros_fs::types::FileMode::default_file())
        .map_err(|_| ())?;
    let mut off = 0usize;
    while off < bytes.len() {
        // 写失败同样清 tmp（复查轮 F2：泄漏面与自身文档对齐——meta 侧
        // 调用方无兜底，屏障未解锁时每窗泄漏一份无界积累）。
        let n = match snap_file_write(fs, &mut f, which, &bytes[off..]) {
            Ok(n) => n,
            Err(()) => {
                let _ = f.close();
                let _ = fs.remove(tmp);
                return Err(());
            }
        };
        if n == 0 {
            let _ = f.close();
            let _ = fs.remove(tmp);
            return Err(());
        }
        off += n;
    }
    if f.close().is_err() {
        let _ = fs.remove(tmp);
        return Err(());
    }
    fs.rename(tmp, path).map_err(|_| {
        // rename 失败：tmp 仍占块（半成品不可见），就地清。
        let _ = fs.remove(tmp);
    })
}

/// meta 探针（pin/sync 时消费）。蓝图 78 §2.3 三态：仅确凿 `NotFound`
/// 可表示遗留 `LegacyAbsent`；I/O/坏块/短读/不确定 = `IoFault`（fail-
/// closed，不得冒充遗留）；在场不可解 = `Corrupt`。
#[derive(Debug)]
enum MetaProbe {
    /// 每分区最早仍需事件 offset。
    Ranges(Vec<(u32, u64)>),
    /// 在场但不可解 = 身份不可证（拒绝）。
    Corrupt,
    /// 元数据读故障/不确定（瞬态 I/O、坏块）= 状态未知（拒绝，留旧状态）。
    IoFault,
    /// 确凿 `NotFound`（真遗留格式，与读故障判别；REG-77-F 全保护兼容）。
    LegacyAbsent,
}

/// 按 ID 读 meta（`/snaps/<stem>.meta`）。蓝图 78 §2.3 三态判别：仅确凿
/// `NotFound` = `LegacyAbsent`（REG-77-F 遗留形态唯一来源）；I/O/坏块/
/// 短读截尾等一切不确定 = `IoFault`（fail-closed，故障不得冒充遗留）；
/// 在场不可解 = `Corrupt`（身份不可证）。
fn probe_snap_meta(fs: &mut Lfs, id: &[u8; 16]) -> MetaProbe {
    let path = format!(
        "{}/{}{}",
        SNAP_DIR,
        snapshot_file_stem(id),
        SNAP_META_PATH_SUFFIX
    );
    let bytes = match read_snap_file(fs, &path) {
        Ok(b) => b,
        Err(FsError::NotFound { .. }) => return MetaProbe::LegacyAbsent,
        Err(_) => return MetaProbe::IoFault,
    };
    match decode_snap_meta(&bytes) {
        Some(ranges) => MetaProbe::Ranges(ranges.into_iter().map(|(p, s, _)| (p, s)).collect()),
        None => MetaProbe::Corrupt,
    }
}

/// 来源区间在位性核验（蓝图 77 卷一 WP-77.1.4 复查轮补口）：每个
/// (partition, floor) 位点必须仍可读——`OffsetGone`（保留删除吃掉）/
/// `OffsetOutOfRange`（位点从未写入 = 身份不可证）/ 任何 I/O 错误一律
/// 视为「相应事件已不存在」⇒ 拒绝新持有。读到首条记录即停（存在性
/// 判定，非重放）。
fn ranges_present_in_store<D: eneros_event_store::EventStore>(
    store: &mut D,
    ranges: &[(u32, u64)],
) -> bool {
    // 在位判据 = **visitor 本地计数 ≥ 1**（复查轮 F4：floor=0 + 分区不
    // 存在/事件区重整时 read 返回 Ok 空读——空读不得视为在位。判据不可
    // 用 ReadReport：早停报告中「当前这条」不计入 records_read、next_offset
    // 在早停时也不推进（block.rs 两者都在 visitor 返回后才更新）——只有
    // visitor 自身的调用计数可靠）。首条即停，单记录读成本。
    struct StopFirst {
        seen: u64,
    }
    impl eneros_event_store::RecordVisitor for StopFirst {
        fn record(&mut self, _: eneros_event_store::RecordRef<'_>) -> bool {
            self.seen += 1;
            false // 存在性判定，读到首条即停
        }
    }
    for &(partition, floor) in ranges {
        let mut visitor = StopFirst { seen: 0 };
        match store.read(partition, floor, &mut visitor) {
            Ok(_) if visitor.seen >= 1 => {}
            _ => return false,
        }
    }
    true
}

/// snapshot_id → 文件名干（32 hex）。
fn snapshot_file_stem(id: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in id {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 按 ID 读回并**验证**现存 blob（蓝图 77 卷一 WP-77.1.3 单源半边）：
/// 文件读取 + `decode_snapshot_v1` 解码 + snapshot_id 逐字节一致。任一
/// 失败 → `None`（证据行一次）——`snapshot_blob` 取回、`pin_snapshot`
/// 入钉前确认、`sync_snap_pins` 全集验证共用本判据。
fn read_and_verify_snap(fs: &mut Lfs, id: &[u8; 16]) -> Option<Vec<u8>> {
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
            RegionGeometry::from_blocks_512(0, eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS)
                .expect("数据盘 512B→4K 几何")
                .blocks,
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
            RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                .expect("事件区段 512B→4K 几何")
                .base_block,
            RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                .expect("事件区段 512B→4K 几何")
                .blocks,
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
            RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                .expect("事件区段 512B→4K 几何")
                .base_block,
            RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                .expect("事件区段 512B→4K 几何")
                .blocks,
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
            if round == 0 {
                // WP-77.1.3 协议：dispatch 每 held tick 全量同步（此处无钉
                // = 空集）——屏障解锁后保留判据才生效（首批已触发 open）。
                face.sync_snap_pins(&[]).expect("空集同步");
            }
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
        // 首批触发 open 后立刻空集同步（WP-77.1.3 协议）——解锁腾位剪枝。
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 2_000, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 2_000);
            face.sync_snap_pins(&[]).expect("空集同步");
        }
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
        // WP-77.1.3 协议：dispatch 以完整活跃集同步（此处 = {id1}）。
        face.sync_snap_pins(&[id1]).expect("活跃集同步");
        // 持有期压力（4 窗）：钉住的 blob 必须按 ID 可取，落盘不停摆。
        // （WP-77.1.4 起持有同时保护来源事件段——前缀删除模型下长持
        // 有 + 持续摄入会渐进冻结事件区（蓝图验收：满卷只背压），
        // 故压力窗数有界；释放后恢复有界剪枝由本测尾段证明。）
        run_windows(&mut face, 8, 4, &mut seq);
        assert_eq!(face.snap_skipped, 0, "钉住单份不得造成落盘停摆");
        assert!(
            face.snapshot_blob(&id1).is_some(),
            "被钉住的冻结输入必须可在持有期内按 ID 取回（ADR-0054:29–30）"
        );
        // 解除（计划终态）→ 同步集更新为空（dispatch 侧集合重算）→
        // 后续压力剪枝可回收（blob 与来源事件段同律）。
        assert!(face.release_snapshot(&id1));
        face.sync_snap_pins(&[]).expect("空集同步");
        run_windows(&mut face, 8 + 4 * 8, 12, &mut seq);
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

    /// 蓝图 77 卷一 WP-77.1.3：`pin_snapshot` **入钉前必须验证现存文件**
    /// （同 [`Self::snapshot_blob`] 的读、解码、ID 对比语义）——旧实现只查
    /// 重复 ID 与容量（`event_face.rs:447-465`），「认证已拿 blob → 中间
    /// 剪枝 → hold」产出**幽灵钉**（ADR-0054 持有承诺落空）。文件缺失、
    /// 解码失败、ID 不符一律拒绝；重复 ID 也重新确认现存文件。
    /// 变异靶：恢复"只按 ID 入 Vec"→ 本测红。
    #[test]
    fn t9_pin_verifies_existing_blob_no_ghost_pins() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 冷启动（卷未打开/无文件）：未知 ID 不得入钉。
        assert!(
            !face.pin_snapshot(&[0xEE; 16]),
            "文件缺失的 hold 必须拒绝（幽灵钉）"
        );
        // 产出一代冻结快照 → 真实 ID 可钉。
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
        let id = face.current_input().expect("冻结输入").facts().snapshot_id;
        assert!(face.pin_snapshot(&id), "现存文件的 hold 必须成功");
        // 重复 ID：重新确认现存文件（仍在）→ 仍成功。
        assert!(face.pin_snapshot(&id), "重复 ID 且文件在 ⇒ 幂等成功");
    }

    /// 蓝图 77 卷一 WP-77.1.3：**进程世代同步屏障**——未收到完整活跃集
    /// 全量同步前，高水位下零删除 + 显式背压；含未知 ID 的同步被整体
    /// 拒绝（原子性，屏障不解除）；合法全量同步解锁；被钉 blob 在恢复
    /// 剪枝后仍按 ID 可取。变异靶：删屏障分支/同步改逐 ID 部分接受 →
    /// 本测红。
    #[test]
    fn t10_generation_barrier_blocks_prune_until_full_set_sync() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        // 压力窗（不同步）：屏障下高水位零删除 + 背压。
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
        let id1 = face
            .current_input()
            .expect("窗满批必有冻结输入")
            .facts()
            .snapshot_id;
        assert_eq!(face.pruned_segments, 0, "未同步（屏障）⇒ 高水位也零删除");
        assert!(
            face.retention_blocked(),
            "屏障 + 高水位 ⇒ 背压（调用方停发新决策）"
        );
        // 混入未知 ID 的同步 = 整体拒绝（原子替换——不得部分接受后解锁）。
        assert!(
            face.sync_snap_pins(&[id1, [0xEE; 16]]).is_err(),
            "含未知 ID 的全量同步必须整体拒绝"
        );
        assert!(
            face.retention_blocked(),
            "同步失败 ⇒ 屏障保持（背压不解除）"
        );
        // 合法全量同步（{id1}）⇒ 解锁；恢复剪枝后被钉 blob 仍按 ID 可取。
        face.sync_snap_pins(&[id1]).expect("合法全量同步");
        // 再压 8 窗：水位维持高位，删除恢复。
        for round in 8..16u64 {
            let now = 2_000 + round * 46_000;
            let mut events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            for i in 0..56u64 {
                seq += 1;
                events.push(obs(100 + (i % 4) as u32, seq, now, i as f64));
            }
            face.on_obs_batch(&events, now);
        }
        assert!(
            face.pruned_segments >= 1,
            "同步解锁后修剪恢复（pruned={}）",
            face.pruned_segments
        );
        assert!(!face.retention_blocked(), "删除恢复 ⇒ 背压解除");
        assert!(
            face.snapshot_blob(&id1).is_some(),
            "被同步钉住的冻结输入在恢复剪枝后必须按 ID 可取（ADR-0054）"
        );
    }

    /// 蓝图 77 卷一 WP-77.1.4：**计划钉住快照的来源事件段在持有期受
    /// 保护**——保护界出自快照 meta 的来源区间（在驻事件落点镜像），
    /// 不是消费位点。取证链 = blob + 来源事件段 + meta 三件在持有期
    /// 同 ID 可查；释放后剪枝恢复。
    /// 旧代码红：keep = 消费位点（对来源区间一无所知）→ 压力下来源段
    /// 被删（重放读缺失）+ meta 无从谈起。
    #[test]
    fn t11_pinned_snapshot_source_events_survive_pressure() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        // 窗 1：8 批覆盖点 → 快照 id1（provenance 选中事件 = 最末批的
        // 在驻事件，offset ≈ 56..63）。
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        let id1 = face
            .current_input()
            .expect("窗 1 必有冻结输入")
            .facts()
            .snapshot_id;
        let batch7_time_ns: u64 = (2_000u64 + 7 * 46_000).saturating_mul(1_000_000);
        // 钉住 + 全量同步（dispatch 协议同形）。
        assert!(face.pin_snapshot(&id1), "id1 可钉（blob + meta 齐备）");
        face.sync_snap_pins(&[id1]).expect("活跃集同步");
        // meta 落盘事实：可被 pin 的 ID 必有可查来源区间（发布次序保障）。
        {
            let mut snap_fs =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("snap vol");
            let path = format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(&id1),
                SNAP_META_PATH_SUFFIX
            );
            let bytes = read_snap_file(&mut snap_fs, &path).expect("meta file");
            let ranges = decode_snap_meta(&bytes).expect("meta decodes");
            assert!(!ranges.is_empty(), "来源区间非空（非承诺空 offset）");
        }
        // 持有期压力（56 批 = 7 窗；总量逼近满卷 ≥850‰ ⇒ 修剪 pass 真实
        // 发生）：删除界被钉住 floor 收紧 ⇒ 零删除（非真空通过）。
        for round in 8..64u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        assert_eq!(
            face.pruned_segments, 0,
            "持有期：provenance floor 是约束方 ⇒ 事件段零删除（旧代码按消费位点删除 → 本断言前已红）"
        );
        // 取证读：来源事件（末批 event_time）仍可重放。
        {
            let dev = face_dev(&storage);
            let (store, _report) = open_region_scoped(
                || CachedBlocks::<4, _>::new(dev.clone()),
                RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                    .expect("事件区段几何")
                    .base_block,
                RegionGeometry::from_blocks_512(EVENT_BASE, REGION_BLOCKS_512)
                    .expect("事件区段几何")
                    .blocks,
                event_store_config(),
            )
            .expect("重开事件面");
            let mut replayed: Vec<ObservationEvent> = Vec::new();
            for p in store.partitions() {
                let mut c = FrameCollector {
                    frames: &mut replayed,
                };
                let _ = store.read(p, 0, &mut c);
            }
            assert!(
                replayed.iter().any(|e| e.event_time_ns == batch7_time_ns),
                "被钉快照的来源事件必须仍在（取证重放链）：replayed={}",
                replayed.len()
            );
        }
        // 释放 + 空集同步（保护界重算上移）→ 剪枝恢复（末窗提交位点
        // 前进 + 水位高位 ⇒ 删除判据全开）。
        assert!(face.release_snapshot(&id1));
        face.sync_snap_pins(&[]).expect("空集同步");
        for round in 64..72u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        assert!(
            face.pruned_segments >= 1,
            "释放后剪枝恢复（pruned={}）",
            face.pruned_segments
        );
    }

    /// 蓝图 77 卷一 WP-77.1.4 复查轮补口：「缺少范围**或相应事件已不
    /// 存在**时拒绝新持有，而非承诺一个空 offset」——meta 区间位点不可
    /// 读（保留删除吃掉/位点从未写入）⇒ pin 拒绝、sync 整体拒绝。
    /// 变异靶：删 `ranges_present_in_store` 核验 → 本测红（幽灵区间钉）。
    /// 注：face 持有自己的 Lfs 实例（元数据在实例内），测试注入 meta
    /// 必须「写盘 → 丢弃面 → 重开」才能被 face 的实例看见。
    #[test]
    fn t12_pin_rejects_when_source_events_gone() {
        let write_meta = |storage: &FakeStorage, id: &[u8; 16], part: u32, floor: u64| {
            let mut fs =
                open_snap_volume(make_io(storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let name = snapshot_file_stem(id);
            let tmp = format!("{}/{}{}.tmp", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
            let path = format!("{}/{}{}", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
            let bytes = encode_snap_meta(&[(part, floor, floor)]);
            let mut f = fs
                .create(&tmp, eneros_fs::types::FileMode::default_file())
                .expect("create tmp meta");
            f.write(&mut fs, &bytes).expect("write meta");
            let _ = f.close();
            fs.rename(&tmp, &path).expect("publish meta");
        };
        let run_window1 = |storage: &FakeStorage| -> [u8; 16] {
            let mut face = make_face(storage);
            face.set_snapshot_catalog(prod_module_catalog(), 1_000);
            let mut seq = 0u64;
            for round in 0..8u64 {
                let now = 2_000 + round * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        seq += 1;
                        obs(p, seq, now, fixture_value(p, 0))
                    })
                    .collect();
                face.on_obs_batch(&events, now);
            }
            face.current_input()
                .expect("窗 1 必有冻结输入")
                .facts()
                .snapshot_id
        };
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let part = eneros_event_store::ingest_bridge::partition_of(&obs(1, 1, 2_000, 0.0));
        // —— 负臂：blob 在、来源事件位点不在 ⇒ pin/sync 拒绝 ——
        let id1 = run_window1(&storage);
        write_meta(&storage, &id1, part, 10_000_000);
        let mut face2 = make_face(&storage);
        face2.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 一批触发 lazy-open（事件继续追加，不影响 blob/meta 断言）。
        let warm: Vec<ObservationEvent> = coverage_fixture_points()
            .map(|p| obs(p, 9_999, 9_000, fixture_value(p, 0)))
            .collect();
        face2.on_obs_batch(&warm, 9_000);
        assert!(face2.is_open(), "暖批后事件面必须已打开");
        assert!(
            !face2.pin_snapshot(&id1),
            "来源事件不在 ⇒ 必须拒绝新持有（不承诺空 offset）"
        );
        assert!(
            face2.sync_snap_pins(&[id1]).is_err(),
            "同步面同判据：来源事件不在 ⇒ 整体拒绝"
        );
        drop(face2);
        // —— 正臂：真实区间（0..63 在位）⇒ 核验通过、钉住成功 ——
        write_meta(&storage, &id1, part, 0);
        let mut face3 = make_face(&storage);
        face3.set_snapshot_catalog(prod_module_catalog(), 1_000);
        face3.on_obs_batch(&warm, 9_100);
        assert!(face3.is_open());
        assert!(face3.pin_snapshot(&id1), "来源事件在位 ⇒ 钉住成功");
    }

    /// WP-77.1.4 复查轮补口（全局对抗 F4）：存在性核验对「空读」不得
    /// 空洞通过——floor=0 且分区不存在（事件区重整/清空）时 read 返回
    /// Ok(0 records)，核验必须以「至少一条记录可读」为准，否则承诺的
    /// 来源区间实为空。变异靶：Ok 空读当在位 → 本测红。
    #[test]
    fn t14_pin_rejects_vacuous_presence_floor_zero_missing_partition() {
        let write_meta = |storage: &FakeStorage, id: &[u8; 16], part: u32, floor: u64| {
            let mut fs =
                open_snap_volume(make_io(storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let name = snapshot_file_stem(id);
            let tmp = format!("{}/{}{}.tmp", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
            let path = format!("{}/{}{}", SNAP_DIR, name, SNAP_META_PATH_SUFFIX);
            let bytes = encode_snap_meta(&[(part, floor, floor)]);
            let mut f = fs
                .create(&tmp, eneros_fs::types::FileMode::default_file())
                .expect("create tmp meta");
            f.write(&mut fs, &bytes).expect("write meta");
            let _ = f.close();
            fs.rename(&tmp, &path).expect("publish meta");
        };
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let id1 = {
            let mut face = make_face(&storage);
            face.set_snapshot_catalog(prod_module_catalog(), 1_000);
            let mut seq = 0u64;
            for round in 0..8u64 {
                let now = 2_000 + round * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        seq += 1;
                        obs(p, seq, now, fixture_value(p, 0))
                    })
                    .collect();
                face.on_obs_batch(&events, now);
            }
            face.current_input().expect("冻结输入").facts().snapshot_id
        };
        // 伪造 meta：分区号不存在于事件区 + floor=0（read 返回 Ok 空读的
        // 形态）。blob 真实存在——只有修好的在位核验能拒。
        write_meta(&storage, &id1, 0xF0F0_F0F0, 0);
        let mut face2 = make_face(&storage);
        face2.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let warm: Vec<ObservationEvent> = coverage_fixture_points()
            .map(|p| obs(p, 9_999, 9_000, fixture_value(p, 0)))
            .collect();
        face2.on_obs_batch(&warm, 9_000);
        assert!(
            !face2.pin_snapshot(&id1),
            "空读不得视为在位——承诺的来源区间实为空（不承诺空 offset）"
        );
    }

    /// 蓝图 77 卷一复查轮（全局对抗）：meta 与 blob 同生命周期——被钉
    /// 快照的 meta 必须在任何 prune pass（含写前腾位剪）后仍可读。
    /// 变异靶：孤儿清扫误删（counterpart 代数错）/ 联动删名字错拼 →
    /// 本测红（meta 先于 blob 死亡，取证链断）。
    #[test]
    fn t13_meta_survives_prune_while_pinned() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        let id1 = face
            .current_input()
            .expect("窗 1 必有冻结输入")
            .facts()
            .snapshot_id;
        assert!(face.pin_snapshot(&id1));
        face.sync_snap_pins(&[id1]).expect("同步");
        let meta_path_of = |id: &[u8; 16]| {
            format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(id),
                SNAP_META_PATH_SUFFIX
            )
        };
        // 窗 2：任一 store_snap_blob 都会跑腾位/终剪——meta 不得被误删。
        for round in 8..16u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        drop(face);
        let mut vol = open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
        let meta_bytes = read_snap_file(&mut vol, &meta_path_of(&id1))
            .expect("被钉快照的 meta 必须在 prune 后仍可读（同生命周期）");
        let ranges = decode_snap_meta(&meta_bytes).expect("meta 可解");
        assert!(!ranges.is_empty());
        // blob 本体同样在场（钉守卫）。
        let blob_path = format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(&id1));
        assert!(
            read_snap_file(&mut vol, &blob_path).is_ok(),
            "被钉 blob 在场"
        );
    }

    /// 蓝图 77 卷一复查轮（全局对抗 C2+C3 存活变异补牙）：
    /// ① **遗留 blob 无 meta**（REG-77-F 双版本兼容）——pin 成功 + 全保护
    ///    （floor=0：存活钉中含无区间条目 ⇒ 零删除；变异 C2「None 分支
    ///    跳过」→ 本测红）；
    /// ② **释放后不 sync**（C3：release 的区间缓存删除被全量同步掩蔽）
    ///    ——释放后保护界必须**立即**上移（删除恢复），证明 pins 与
    ///    pin_ranges 严格配对。变异 C3「release 删 pin_ranges 缺失」→ 红。
    #[test]
    fn t15_legacy_pin_full_protection_and_release_resumes_prune() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        // 窗 1 → id1；随后**删掉其 meta**（制造遗留形态）。
        for round in 0..8u64 {
            let now = 2_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, now);
        }
        let id1 = face
            .current_input()
            .expect("窗 1 必有冻结输入")
            .facts()
            .snapshot_id;
        drop(face);
        {
            let mut vol =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let path = format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(&id1),
                SNAP_META_PATH_SUFFIX
            );
            vol.remove(&path).expect("删除 meta（制造遗留 blob）");
        }
        // 重开 + pin：无 meta = 遗留形态 → 接受钉（全保护条目）。
        // （时间轴单调递增——水印防回退会拒绝倒退事件，那是正确防线。）
        let mut face2 = make_face(&storage);
        face2.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 序列逐事件递增（同源同刻同序列 = 水印判重放拒绝——正确防线）。
        let warm: Vec<ObservationEvent> = coverage_fixture_points()
            .enumerate()
            .map(|(i, p)| obs(p, 65 + i as u64, 400_000, fixture_value(p, 0)))
            .collect();
        face2.on_obs_batch(&warm, 400_000);
        assert!(
            face2.pin_snapshot(&id1),
            "遗留 blob（无 meta）必须接受钉（REG-77-F 双版本兼容）"
        );
        assert!(face2.sync_snap_pins(&[id1]).is_ok());
        // 压力窗：遗留钉全保护（floor=0）⇒ 零删除。
        for round in 0..64u64 {
            let now = 400_000 + round * 46_000;
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
            face2.on_obs_batch(&events, now);
        }
        assert_eq!(
            face2.pruned_segments, 0,
            "遗留钉（无区间）必须全保护：零删除（C2 变异在此红）"
        );
        // 释放（**不 sync**）⇒ 保护界立即上移 ⇒ 删除恢复（C3 变异在此红：
        // 区间缓存未随 release 删除 → floor 恒 0 → 恒零删除）。
        assert!(face2.release_snapshot(&id1));
        for round in 64..72u64 {
            let now = 400_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, now, fixture_value(p, round as usize))
                })
                .collect();
            face2.on_obs_batch(&events, now);
        }
        assert!(
            face2.pruned_segments >= 1,
            "释放后（无 sync）保护界必须上移：删除恢复（pruned={}）",
            face2.pruned_segments
        );
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

        // —— 钉住阻塞：物化路径下钉住早期快照（其来源区间覆盖低
        // offset）→ 删除界收紧 ——（WP-77.1.4 重写：hold_offset 只紧不松
        // 的旧 API 退役，保护界改由存活钉集的来源区间重算。）
        let storage2 = FakeStorage::new(disk_len);
        let mut face2 = make_face(&storage2);
        face2.set_snapshot_catalog(prod_module_catalog(), 1_000);
        // 首窗产出快照 id_early：批 0 = 覆盖点（offset 0..7，此后不再
        // 有同点新事件 ⇒ 其落点始终在驻）→ 快照来源区间起点 = 0。
        let mut seq2 = 0u64;
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq2 += 1;
                    obs(p, seq2, 2_000, fixture_value(p, 0))
                })
                .collect();
            face2.on_obs_batch(&events, 2_000);
            for round in 1..8u64 {
                let now = 2_000 + round * 46_000;
                let fillers: Vec<ObservationEvent> = (0..56u64)
                    .map(|i| {
                        seq2 += 1;
                        obs(100 + (i % 4) as u32, seq2, now, i as f64)
                    })
                    .collect();
                face2.on_obs_batch(&fillers, now);
            }
        }
        let id_early = face2
            .current_input()
            .expect("窗满批必有冻结输入")
            .facts()
            .snapshot_id;
        assert!(face2.pin_snapshot(&id_early), "早期快照可钉");
        // WP-77.1.3 协议：以存活集 {id_early} 全量同步解锁屏障。
        face2.sync_snap_pins(&[id_early]).expect("活跃集同步");
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

    /// 蓝图 77 卷一复查轮（≥3 钉中途释放配对锁）：`snap_pins` 与
    /// `pin_ranges` 在**非末位释放**后必须仍同位配对。旧实现
    /// `snap_pins.swap_remove(idx)` × `pin_ranges.remove(idx)` 不对称——
    /// 首次非末位释放后两数组错位（集合尚各自完整），**第二次**释放按
    /// pin 现位索引删 ranges = 错删他人区间：`event_floor` 只对集合取
    /// min，错删低 floor 条目可把保护界抬高到越界剪删仍持有计划的来源
    /// 事件段（ADR-0054 违约），错删 None/全保护条目同理。单钉测试对此
    /// 天然免疫（末位 swap_remove == remove）。
    /// 变异靶：恢复 `swap_remove` → 本测红。
    #[test]
    fn t16_multi_pin_interior_release_keeps_pin_range_pairing() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let mut ids: Vec<[u8; 16]> = Vec::new();
        // 三个快照窗 → 三代冻结快照（保留窗 16 份，三代皆在、皆带 meta）。
        for round in 0..(SNAP_EVERY_BATCHES * 3) {
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
        assert_eq!(ids.len(), 3, "三窗必须产出三代不同冻结快照");
        // dispatch 全量同步三钉（wire 序 = [id1, id2, id3]）。
        face.sync_snap_pins(&ids).expect("三钉同步");
        // 非末位释放 id1（错位源），再释放 id2——错位后按 id2 现位索引
        // 删 ranges 会删掉 id3 的区间条目。
        assert!(face.release_snapshot(&ids[0]));
        assert!(face.release_snapshot(&ids[1]));
        let pins: Vec<SnapId> = face.retention.pins.iter().map(|p| p.id).collect();
        assert_eq!(pins, vec![ids[2]], "剩余钉集必须只剩第三代");
        let range_ids: alloc::vec::Vec<SnapId> = face
            .retention
            .pins
            .iter()
            .filter(|p| p.ranges.is_some())
            .map(|p| p.id)
            .collect();
        assert_eq!(
            range_ids,
            vec![ids[2]],
            "剩余区间条目必须仍是第三代本人——错留第二代的区间 = 保护界据错误集合重算"
        );
    }

    /// 蓝图 77 卷一复查轮（纵深）：`sync_snap_pins` 服务端自守容量门——
    /// 有界性不得只依赖 wire 解帧门（count ≤ SNAP_SYNC_MAX_IDS）与
    /// dispatch 侧编译期断言，trait 直调（新测试/未来调用方）传超容集
    /// 必须整体拒绝且钉集保持原样（与 [`Self::pin_snapshot`] 的容量门
    /// 对称；过度保留安全，越容接纳破坏容量语义）。
    /// 变异靶：删容量门 → 本测红。
    #[test]
    fn t17_sync_rejects_over_capacity_id_set_atomically() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let mut ids: Vec<[u8; 16]> = Vec::new();
        for round in 0..(SNAP_EVERY_BATCHES * 5) {
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
        assert_eq!(ids.len(), 5, "五窗必须产出五代不同冻结快照");
        // 先以合法两钉建立基线钉集；超容同步必须整体拒绝、基线保持。
        face.sync_snap_pins(&ids[..2]).expect("容量内同步");
        assert!(
            face.sync_snap_pins(&ids).is_err(),
            "超容集（5 > MAX_SNAP_PINS）必须拒绝"
        );
        let pins: Vec<SnapId> = face.retention.pins.iter().map(|p| p.id).collect();
        assert_eq!(pins, ids[..2], "拒绝后钉集必须保持原样");
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

    // ========================================================================
    // 蓝图 78 卷一 WP-78.1.1：故障注入红测（四组 + 负例 + 慢周期证明）。
    // 判据先于修复：本节测试在 WP-78.1.2 修复前的旧逻辑上必须按注释标明的
    // 「旧代码」臂变红。注入面 = write_fault 静态（窄写单点）+ 真实介质
    // 真相（目录占位/字节损坏——不用块层定时注入，确定性优先）。
    // ========================================================================

    /// 快窗/慢窗共用：跑 n 个快照窗，返回最后一批时刻。（batch 间隔 =
    /// `interval_ms`；首批后空集同步解锁剪枝屏障——WP-77.1.3 协议由调用
    /// 方负责。）`light = true` 时每批只喂 1 个填充点（快窗批量场景用：
    /// 压低事件区水位，让 blob 侧满卷腾位成为唯一压力源）。
    fn run_windows(
        face: &mut EventFace<FakeStorage>,
        start_batch: usize,
        windows: usize,
        interval_ms: u64,
        seq: &mut u64,
    ) -> u64 {
        run_windows_opt(face, start_batch, windows, interval_ms, seq, false)
    }

    fn run_windows_opt(
        face: &mut EventFace<FakeStorage>,
        start_batch: usize,
        windows: usize,
        interval_ms: u64,
        seq: &mut u64,
        light: bool,
    ) -> u64 {
        for batch in start_batch..(start_batch + windows * SNAP_EVERY_BATCHES as usize) {
            let now = 2_000 + batch as u64 * interval_ms;
            let events: Vec<ObservationEvent> = if light {
                vec![obs(
                    100,
                    {
                        *seq += 1;
                        *seq
                    },
                    now,
                    1.0,
                )]
            } else {
                coverage_fixture_points()
                    .map(|p| {
                        *seq += 1;
                        obs(p, *seq, now, fixture_value(p, batch))
                    })
                    .collect()
            };
            face.on_obs_batch(&events, now);
        }
        2_000 + (start_batch + windows * SNAP_EVERY_BATCHES as usize - 1) as u64 * interval_ms
    }

    /// ①组（元数据三态 + 再钉降级负例，蓝图 78 §2.2/§2.3，face 级）：
    /// 非遗留形态的 meta 异常不得接纳新持有；**已钉条目**同 ID 再 pin 走
    /// 同一核验——缓存 Some(ranges) 而 meta 真消失（确凿 NotFound）⇒ 拒
    /// 新准入、不得降级成遗留全保护，旧条目与保护界保留待排障。
    /// 红臂（旧代码）：重复 pin 短路 `contains ⇒ true`——meta 已消失仍放
    /// 行新准入（变异靶：恢复「contains 即 true」→ 降级臂红）。
    /// 故障形态 = 介质真相（目录占位/字节破坏/真删除）+「写盘 → 丢弃面
    /// → 重开」（LFS 实例元数据不互通——t12/t15 同律；纯读故障三态判别
    /// 在 t18b 的 probe 缝锁定——CachedBlocks 下 face 级 meta 探针读命中
    /// 刚暖好的缓存，块层瞬态故障无法定向）。
    #[test]
    fn t18_meta_read_fault_is_not_legacy_admission_rejected_pins_kept() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let mut ids: Vec<[u8; 16]> = Vec::new();
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
        assert_eq!(ids.len(), 2, "两窗产出两代快照");
        drop(face);
        // 介质真相：id2 的 meta 路径换成同名目录（读为空 → 形态坏 Corrupt）。
        {
            let mut vol =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let path = format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(&ids[1]),
                SNAP_META_PATH_SUFFIX
            );
            vol.remove(&path).expect("remove meta file");
            vol.mkdir(&path).expect("occupy meta path with directory");
        }
        // 重开（新 LFS 实例见真介质）+ 建立合法基线（id1 钉 + 全量同步）。
        let mut face2 = make_face(&storage);
        face2.set_snapshot_catalog(prod_module_catalog(), 100_000);
        let warm: Vec<ObservationEvent> = coverage_fixture_points()
            .enumerate()
            .map(|(i, p)| obs(p, 9_000 + i as u64, 100_000, fixture_value(p, 0)))
            .collect();
        face2.on_obs_batch(&warm, 100_000);
        assert!(face2.pin_snapshot(&ids[0]), "基线钉（meta 齐备）必须成功");
        assert!(face2.sync_snap_pins(&[ids[0]]).is_ok(), "基线全量同步");
        // —— 臂 1（sanity）：形态坏 meta ⇒ 新 pin 拒绝。实测口径：78-A
        // 修复前的旧 probe 把读错压成 Absent，但 open(dir,READ) 返回空读
        // 走 decode-fail=Corrupt 臂 ⇒ 旧代码同拒（六红名单无此臂）；本臂
        // 锁「非遗留形态不接纳」——纯读故障的 Absent 误判由 t18b 锁定 ——
        assert!(
            !face2.pin_snapshot(&ids[1]),
            "meta 异常（Corrupt 形态）不得接纳新持有"
        );
        // —— 臂 2：含异常 ID 的全量 sync 整体拒绝、旧钉不动（原子性）——
        assert!(
            face2.sync_snap_pins(&[ids[0], ids[1]]).is_err(),
            "任一 ID 元数据异常 ⇒ 全量同步整体拒绝"
        );
        let pins_after_sync_reject: Vec<SnapId> =
            face2.retention.pins.iter().map(|p| p.id).collect();
        assert_eq!(pins_after_sync_reject, vec![ids[0]], "拒绝后旧钉集保持原样");
        // —— 臂 3（红测主体，降级负例）：id1 的 meta **真消失**（确凿
        // NotFound）+ 新面 RAM 钉态缓存 Some(ranges) ⇒ 同 ID 再 pin 拒新
        // 准入（旧代码 contains 短路 true = RED）且旧条目原样保留 ——
        drop(face2);
        {
            let mut vol =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let path = format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(&ids[0]),
                SNAP_META_PATH_SUFFIX
            );
            vol.remove(&path).expect("真删除 id1 meta（确凿 NotFound）");
        }
        let mut face3 = make_face(&storage);
        face3.set_snapshot_catalog(prod_module_catalog(), 200_000);
        let warm3: Vec<ObservationEvent> = coverage_fixture_points()
            .enumerate()
            .map(|(i, p)| obs(p, 19_000 + i as u64, 200_000, fixture_value(p, 0)))
            .collect();
        face3.on_obs_batch(&warm3, 200_000);
        // RAM 钉态（进程内旧钉缓存：来自上一世代的 Some(ranges) 条目）。
        // 手工灌入的理由：LFS 实例元数据缓存墙——外删 meta 对已开 face
        // 不可见（t12/t15 同律），「同进程再 pin」只能以新面 + 显式钉态
        // 构造；判定路径（pin_snapshot→reverify_pinned）仍走真接口。
        face3.retention.pins.push(PinnedSnapshot {
            id: ids[0],
            ranges: Some(vec![(7_759_173, 56)]),
        });
        assert!(
            !face3.pin_snapshot(&ids[0]),
            "缓存有区间而 meta 真消失 ⇒ 不得降级成遗留全保护接纳新准入"
        );
        assert!(
            face3.retention.contains(&ids[0]),
            "拒新准入但旧钉与缓存保护界保留"
        );
        // —— 臂 5（复查轮 F5 对称化）：同态走全量同步 ⇒ 同样整体拒绝
        //（旧代码 sync 直调 admission 判别 → LegacyFull 降级接纳 = RED）
        // 且旧集保持 ——
        assert!(
            face3.sync_snap_pins(&[ids[0]]).is_err(),
            "缓存 Some(ranges) 而 meta 真消失 ⇒ 全量同步不得降级接纳（与 pin 同律）"
        );
        assert!(face3.retention.contains(&ids[0]), "sync 拒绝后旧集保持原样");
        // —— 对照臂：字节破坏（Corrupt）双代码同拒（sanity）——
        let id3 = {
            let mut face4 = make_face(&storage);
            face4.set_snapshot_catalog(prod_module_catalog(), 300_000);
            let mut seq4 = 29_000u64;
            for round in 0..SNAP_EVERY_BATCHES {
                let now = 300_000 + round * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        seq4 += 1;
                        obs(p, seq4, now, fixture_value(p, round as usize))
                    })
                    .collect();
                face4.on_obs_batch(&events, now);
            }
            face4.current_input().expect("第三窗").facts().snapshot_id
        };
        {
            let mut vol =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let path = format!(
                "{}/{}{}",
                SNAP_DIR,
                snapshot_file_stem(&id3),
                SNAP_META_PATH_SUFFIX
            );
            let mut f = vol
                .open(&path, eneros_fs::types::OpenFlags::WRITE)
                .expect("open meta for corrupt");
            f.write(&mut vol, b"bad").expect("write garbage meta");
            let _ = f.close();
        }
        let mut face5 = make_face(&storage);
        face5.set_snapshot_catalog(prod_module_catalog(), 600_000);
        let mut seq5 = 59_000u64;
        for round in 0..SNAP_EVERY_BATCHES {
            let now = 600_000 + round * 46_000;
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq5 += 1;
                    obs(p, seq5, now, fixture_value(p, round as usize))
                })
                .collect();
            face5.on_obs_batch(&events, now);
        }
        assert!(
            !face5.pin_snapshot(&id3),
            "形态坏 meta ⇒ 拒绝（Corrupt 与 IoFault 同属不可冒充遗留）"
        );
    }

    /// ①组红测（三态判别边界，probe 缝）：**纯读故障 ≠ 遗留格式**。真实
    /// 故障块设备（全部 BLK_CMD_READ → Err，写面真实透传）+ 真实 LFS 卷
    /// + 真实 `probe_snap_meta`——stat/open/read 的瞬态 I/O 故障必须落
    /// `IoFault` 臂；确凿 `NotFound` 才是 `LegacyAbsent`（REG-77-F 遗留
    /// 形态唯一来源）；在场可解 = `Ranges`。
    /// 旧代码：任意读错压成 `Absent` → 按 REG-77-F 全保护接纳 = 把故障
    /// 伪装成旧格式（本测红）。
    /// 缝位说明：CachedBlocks(4) 下 face 级 pin 的 meta 探针读命中 blob
    /// 刚暖好的目录缓存，块层瞬态故障无法定向到 meta 探针——三态判别
    /// 只能在 probe 缝直接锁定（同一真实 LFS/块链，非替身）。
    /// 变异靶：probe 恢复「任意读错 = Absent」→ 读故障臂红。
    struct FaultReads {
        inner: FakeStorage,
        arm: std::rc::Rc<core::cell::Cell<bool>>,
    }
    impl Clone for FaultReads {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
                arm: self.arm.clone(),
            }
        }
    }
    impl eneros_agent_sdk::block_io::EipcSend for FaultReads {
        fn drv_call(&mut self, req: &[u8], timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()> {
            if self.arm.get() {
                use eneros_drv_svc::block_cmd::BLK_CMD_READ;
                use eneros_drv_svc::runtime::{wire, OP_EXECUTE};
                if let Some(r) = wire::decode_request(req) {
                    if r.opcode == OP_EXECUTE && r.payload.first() == Some(&BLK_CMD_READ) {
                        return Err(()); // 瞬态 I/O 故障（非 NotFound 语义）
                    }
                }
            }
            self.inner.drv_call(req, timeout_ns)
        }
    }

    #[test]
    fn t18b_probe_classifies_read_fault_not_found_and_ranges() {
        use eneros_agent_sdk::block_io::BlockIo;
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut ids: Vec<[u8; 16]> = Vec::new();
        {
            let mut face = make_face(&storage);
            face.set_snapshot_catalog(prod_module_catalog(), 1_000);
            let mut seq = 0u64;
            for round in 0..(SNAP_EVERY_BATCHES * 5) {
                let now = 2_000 + round * 46_000;
                let events: Vec<ObservationEvent> = coverage_fixture_points()
                    .map(|p| {
                        seq += 1;
                        obs(p, seq, now, fixture_value(p, round as usize))
                    })
                    .collect();
                face.on_obs_batch(&events, now);
                if let Some(input) = face.current_input() {
                    let sid = *input.snapshot_id();
                    if ids.last() != Some(&sid) {
                        ids.push(sid);
                    }
                }
            }
        }
        assert!(ids.len() >= 5, "五代快照（读驱逐的素材）");
        let id = ids[ids.len() - 1];
        let arm = std::rc::Rc::new(core::cell::Cell::new(false));
        let faulty_io = |arm: &std::rc::Rc<core::cell::Cell<bool>>| {
            BlockIo::new(
                FaultReads {
                    inner: storage.clone(),
                    arm: arm.clone(),
                },
                "svc-storage",
                0,
                5_000,
            )
        };
        // 单实例续用（mount 本身需要读——故障臂只能在卷打开后武装）。
        let mut vol = open_snap_volume(faulty_io(&arm), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
        let meta_path = format!(
            "{}/{}{}",
            SNAP_DIR,
            snapshot_file_stem(&id),
            SNAP_META_PATH_SUFFIX
        );
        // 臂 1：无故障 → 在场可解 = Ranges（卷内真相）。
        assert!(
            matches!(probe_snap_meta(&mut vol, &id), MetaProbe::Ranges(_)),
            "齐备 meta 必须判 Ranges"
        );
        // 缓存驱逐：读五代 blob 数据块（工作集 > CachedBlocks 的 4 槽）
        // ⇒ 目录元数据块被挤出 ⇒ 后续 meta 探针必须发真实块命令——注入
        // 面才可达（实例内写经缓存自洽，仅靠新增文件逼不出未命中）。
        for sid in ids.iter() {
            let p = alloc::format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(sid));
            let _ = read_snap_file(&mut vol, &p);
        }
        // 臂 2（红测主体）：瞬态读故障 → IoFault（旧代码 = Absent = 冒充遗留）。
        arm.set(true);
        let probe = probe_snap_meta(&mut vol, &id);
        arm.set(false);
        assert!(
            matches!(probe, MetaProbe::IoFault),
            "读故障必须落 IoFault 臂（不得冒充遗留 Absent）：{probe:?}"
        );
        drop(vol); // 故障探针会污染 littlefs 实例内存缓存——臂 3 用新卷。
                   // 臂 3：真删除（确凿 NotFound）→ LegacyAbsent（兼容臂保持）。
        {
            let mut vol =
                open_snap_volume(faulty_io(&arm), SNAP_BASE, SNAP_BLOCKS_512).expect("vol arm3");
            vol.remove(&meta_path).expect("remove meta");
            assert!(
                matches!(probe_snap_meta(&mut vol, &id), MetaProbe::LegacyAbsent),
                "确凿 NotFound 才是遗留 Absent"
            );
        }
    }

    /// ②组（完整写入）：blob 短写不得发布残迹。注入 = 窄写单点返回
    /// Ok(13)（物理介质只含前缀 13 字节——真实短写形态）。
    /// 旧代码：忽略 Ok(n) 计数 → rename 成功 → last_input 发布 →
    /// snapshot_blob 解码失败 = 已发布 ID 不可取（RED）。
    /// 新代码：write_all 补写剩余 → 工件完整 → 写后取回核验通过 → 发布。
    #[test]
    fn t19_short_blob_write_publishes_only_complete_artifact() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 2_000, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 2_000);
            face.sync_snap_pins(&[]).expect("空集同步解锁屏障");
        }
        write_fault::arm_blob_short(13);
        let _ = run_windows(&mut face, 1, 1, 46_000, &mut seq);
        let input = face.current_input().expect("窗口必须产出冻结输入");
        let id = *input.snapshot_id();
        let blob = face
            .snapshot_blob(&id)
            .expect("已发布 ID 必须可取回完整工件");
        let decoded = eneros_snapshot_service::decode_snapshot_v1(&blob).expect("blob 可解");
        assert_eq!(decoded.snapshot_id, id, "取回 = 所发布那一代");
    }

    /// ②组（meta 短写）：meta 残迹不得作为「写成功」发布。
    /// 旧代码：Ok(5) 当完整 → rename 发布截断 meta → 当前 ID 的 pin 因
    /// meta Corrupt 被拒（已发布输入不可钉 = 发布原子性违约，RED）。
    /// 新代码：write_all 补全 → 写后取回与预期区间逐项相等 → 可钉。
    #[test]
    fn t20_short_meta_write_still_yields_verifiable_meta() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 2_000, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 2_000);
            face.sync_snap_pins(&[]).expect("空集同步解锁屏障");
        }
        write_fault::arm_meta_short(5);
        let _ = run_windows(&mut face, 1, 1, 46_000, &mut seq);
        let id = face.current_input().expect("冻结输入").facts().snapshot_id;
        assert!(
            face.pin_snapshot(&id),
            "已发布输入的 meta 必须完整可核（短写补全后写后取回）"
        );
    }

    /// 复查轮 F1 红测（kept 下溢）：孤儿 meta 删除曾计入 `removed`（与
    /// kept 的 blob 算术混账）——目录 = 0 blob + 1 孤儿 meta + keep=0 +
    /// 无保护集：旧代码 `0 - 1` debug 整数下溢 panic（panic=abort 即进程
    /// 崩溃）/ release 回绕虚假 kept。新代码 meta 删除独立分账。
    #[test]
    fn t26_prune_outcome_kept_never_underflows_on_orphan_meta() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut vol = open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
        let _ = vol.mkdir(SNAP_DIR);
        // 介质真相：仅一份孤儿 meta（对端 blob 不存在）。
        let path = format!("{}/orphan1234{}", SNAP_DIR, SNAP_META_PATH_SUFFIX);
        let bytes = encode_snap_meta(&[(1, 0, 1)]);
        let mut f = vol
            .create(&path, eneros_fs::types::FileMode::default_file())
            .expect("create orphan meta");
        f.write(&mut vol, &bytes).expect("write orphan meta");
        let _ = f.close();
        let out = prune_snap_blobs_keep(&mut vol, 0, &[]);
        assert_eq!(out.kept, 0, "零 blob ⇒ kept=0（不得回绕）");
        assert_eq!(out.meta_removed, 1, "孤儿 meta 确凿删除（独立分账）");
    }

    /// 复查轮 F2 红测（tmp 泄漏）：meta 写失败路径曾不清临时文件——
    /// meta 侧调用方无兜底，世代屏障未解锁时每快照窗泄漏一份无界积累
    /// （满卷恰是最怕额外占块的形态）。注入 = 一次 meta 写失败；窗后
    /// 卷内不得残留任何 .tmp。
    #[test]
    fn t25_meta_write_failure_leaves_no_tmp_litter() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 2_000, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 2_000);
            face.sync_snap_pins(&[]).expect("空集同步解锁屏障");
        }
        write_fault::arm_meta_fail(1);
        let _ = run_windows(&mut face, 1, 1, 46_000, &mut seq);
        drop(face);
        let mut vol = open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
        let entries = vol.readdir(SNAP_DIR).expect("readdir");
        let tmp_litter: Vec<_> = entries
            .iter()
            .filter(|e| e.name.ends_with(".tmp"))
            .collect();
        assert!(
            tmp_litter.is_empty(),
            "meta 写失败不得泄漏临时文件：{:?}",
            tmp_litter
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// ②组（零进展）：Ok(0) 不得当成功发布空工件（不变量式——一次性
    /// 零进展故障被自适应重试收敛是正确行为；断言 = **发布面与介质
    /// 一致**：凡发布的 ID 必可取回完整工件；旧代码把 Ok(0) 当完整 →
    /// 空文件上线 + last_input 发布 → 同 ID 取回为空（RED））。
    /// 附自恢复锁：故障清除后下一窗正常发布（回到先写后剪的满卷饥饿
    /// 不得借机复活）。
    #[test]
    fn t21_zero_progress_write_fails_without_publish() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        {
            let events: Vec<ObservationEvent> = coverage_fixture_points()
                .map(|p| {
                    seq += 1;
                    obs(p, seq, 2_000, fixture_value(p, 0))
                })
                .collect();
            face.on_obs_batch(&events, 2_000);
            face.sync_snap_pins(&[]).expect("空集同步解锁屏障");
        }
        assert!(face.current_input().is_none(), "首窗前无冻结输入");
        write_fault::arm_blob_short(usize::MAX);
        let _ = run_windows(&mut face, 1, 1, 46_000, &mut seq);
        // 不变量：凡发布者必完整可取（空工件绝不上线）。
        if let Some(input) = face.current_input() {
            let id = *input.snapshot_id();
            let blob = face
                .snapshot_blob(&id)
                .expect("已发布 ID 必须可取回（Ok(0) 不得当成功发布空文件）");
            let decoded = eneros_snapshot_service::decode_snapshot_v1(&blob).expect("工件可解");
            assert_eq!(decoded.snapshot_id, id);
        }
        // 自恢复：注入消退后下一窗正常发布完整工件。
        let _ = run_windows(&mut face, 9, 1, 46_000, &mut seq);
        let input = face.current_input().expect("故障清除后必须恢复发布");
        let id = *input.snapshot_id();
        assert!(
            face.snapshot_blob(&id).is_some(),
            "恢复窗的发布工件必须完整可取"
        );
    }

    /// ③组（孤儿清扫）：已钉 blob 暂不可读时 meta 不得被当孤儿删除。
    /// 故障形态 = 介质上改写 blob 首字节（decode 失败——prune 的
    /// produced=None 臂，真实介质真相）。**可达性前提**：先跑 5 窗把
    /// id1 的数据块挤出 CachedBlocks(4)（实例内写经缓存自洽——不驱逐
    /// 则 prune 永远读到缓存旧值，缺陷路径不可达，t22 曾因此空转——
    /// M4 变异探针实证），损坏后 prune 才真正读到坏 blob。
    /// 旧代码：prune 读不出 produced_at ⇒ 钉住 blob 跳过删除，但 meta
    /// 不在「可读干集」⇒ 孤儿清扫误删 meta（RED）——读故障恢复后同 ID
    /// 从「有来源区间」退化成「遗留无 meta 全保护」，丢取证界。
    /// 新代码：孤儿判定用文件存在性真相——blob 在场（哪怕暂不可读）⇒
    /// meta 过度保留。
    #[test]
    fn t22_pinned_unreadable_blob_keeps_meta_through_orphan_sweep() {
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let _ = run_windows(&mut face, 0, 1, 46_000, &mut seq);
        let id1 = face
            .current_input()
            .expect("窗 1 必有冻结输入")
            .facts()
            .snapshot_id;
        assert!(face.pin_snapshot(&id1));
        face.sync_snap_pins(&[id1]).expect("活跃集同步");
        // 5 个后续窗：每窗 store（blob+meta+读后核验）的块触碰必然把
        // id1 的数据块挤出 4 槽缓存——后续 prune 读 id1 必发真实块命令。
        let _ = run_windows(&mut face, 8, 5, 46_000, &mut seq);
        // 介质真相：损坏 blob 首字节（magic 破坏 → decode 失败）。
        {
            let mut vol =
                open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
            let path = format!("{}/{}.snap", SNAP_DIR, snapshot_file_stem(&id1));
            let mut f = vol
                .open(&path, eneros_fs::types::OpenFlags::WRITE)
                .expect("open blob for corrupt");
            f.write(&mut vol, &[0u8; 4]).expect("clobber magic");
            let _ = f.close();
        }
        // 再跑两窗：每窗 store_snap_blob 的腾位/终剪都会跑孤儿清扫。
        let _ = run_windows(&mut face, 8 + 5 * 8, 2, 46_000, &mut seq);
        drop(face);
        let mut vol = open_snap_volume(make_io(&storage), SNAP_BASE, SNAP_BLOCKS_512).expect("vol");
        let meta_path = format!(
            "{}/{}{}",
            SNAP_DIR,
            snapshot_file_stem(&id1),
            SNAP_META_PATH_SUFFIX
        );
        let bytes = read_snap_file(&mut vol, &meta_path)
            .expect("已钉 blob 暂不可读 ⇒ meta 不得被孤儿清扫误删");
        assert!(
            decode_snap_meta(&bytes).is_some(),
            "meta 内容仍可解（取证界保留）"
        );
    }

    /// ④组（发布原子性，条件性缺口——快窗 + 满卷 + meta 持续写失败）：
    /// 快窗（每批 250ms，窗口 < 30s 控制窗）下过期门不挡，八批窗口接连
    /// 发布；meta 写持续失败 ⇒ 每窗 stored=false、last_input 冻结在旧 ID；
    /// 满卷腾位剪枝把旧 ID 的 blob（未钉、旧代码无「已发布」保护资格）
    /// 删除 ⇒ `TelemetryService::snapshot_input` 仍给出旧 ID 而
    /// `snapshot_blob` 取不到（RED——可发布性不变量违约）。
    /// 新代码：旧发布 ID 进写前腾位临时保护 ⇒ 拒新发布而保留旧输入 ⇒
    /// 同 ID 恒可取回完整工件。
    /// 注入 = 窄写单点 META_FAIL_LEFT=usize::MAX（meta 写持续失败；
    /// blob 写真实成功——不伪造上层 store 成功）。
    #[test]
    fn t23_fast_window_publish_atomicity_keeps_old_id_retrievable() {
        use eneros_safety_validator::{EvaluatorConfig, RequiredPointCoverage, StateBindings};

        use crate::telemetry::TelemetryService;
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        // 窗 1（正常）：产出 id_old 并确认已发布。
        let _ = run_windows(&mut face, 0, 1, 250, &mut seq);
        face.sync_snap_pins(&[]).expect("空集同步解锁屏障");
        let id_old = face
            .current_input()
            .expect("窗 1 冻结输入")
            .facts()
            .snapshot_id;
        // meta 写持续失败：此后每窗 blob 存成功、meta 失败 ⇒ 不发布。
        write_fault::arm_meta_fail_forever();
        // 快窗连跑 30 窗（每批 80ms ⇒ 总量 ≈20s < 30s 控制窗），每批 1 个
        // 填充点（压低事件区水位——满卷压力只来自 blob 侧）。30 份 blob
        // 远超区段实际容量（t7 实测 ≈6 份）⇒ 满卷腾位自适应剪枝真实发生。
        let _ = run_windows_opt(&mut face, 8, 30, 80, &mut seq, true);
        let final_now = 2_000 + (8 + 30 * SNAP_EVERY_BATCHES as usize - 1) as u64 * 80;
        // 真实服务 interface：snapshot_input → snapshot_blob（不用私有
        // helper 替身）。
        let mut svc = TelemetryService::new(EvaluatorConfig {
            coverage: RequiredPointCoverage { points: Vec::new() },
            bindings: StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1_000,
            allowed_sources: vec![1],
            require_integrity: false,
            control_horizon_ms: 30_000,
        });
        svc.set_event_face(alloc::boxed::Box::new(face));
        let input = svc
            .snapshot_input(final_now)
            .expect("快窗内旧输入仍新鲜、无背压 ⇒ 可发布性承诺成立");
        assert_eq!(
            *input.snapshot_id(),
            id_old,
            "meta 持续失败 ⇒ 冻结输入仍是旧发布 ID"
        );
        let blob = svc
            .snapshot_blob(&id_old)
            .expect("已发布 ID 必须在同一可观察窗口内可取回（可发布性不变量）");
        let decoded = eneros_snapshot_service::decode_snapshot_v1(&blob).expect("取回工件可解");
        assert_eq!(decoded.snapshot_id, id_old);
    }

    /// ④组的条件性证明（慢周期对照）：普通 46s 窗距下，旧 input 会被
    /// 30s 新鲜度门拒绝——证明 ④ 组是「快窗特有」的条件性缺口而非普遍
    /// 故障（不把条件性缺口写成常态事故声明）。
    #[test]
    fn t24_slow_cycle_stale_gate_rejects_old_input() {
        use eneros_safety_validator::{EvaluatorConfig, RequiredPointCoverage, StateBindings};

        use crate::telemetry::TelemetryService;
        let disk_len = (DISK_END * 512) as usize;
        let storage = FakeStorage::new(disk_len);
        let mut face = make_face(&storage);
        face.set_snapshot_catalog(prod_module_catalog(), 1_000);
        let mut seq = 0u64;
        let last_now = run_windows(&mut face, 0, 1, 46_000, &mut seq);
        face.sync_snap_pins(&[]).expect("空集同步");
        let id = face.current_input().expect("冻结输入").facts().snapshot_id;
        let produced_ms = face
            .current_input()
            .expect("冻结输入")
            .facts()
            .produced_at_ns
            / 1_000_000;
        let mut svc = TelemetryService::new(EvaluatorConfig {
            coverage: RequiredPointCoverage { points: Vec::new() },
            bindings: StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1_000,
            allowed_sources: vec![1],
            require_integrity: false,
            control_horizon_ms: 30_000,
        });
        svc.set_event_face(alloc::boxed::Box::new(face));
        // 窗内（age < 30s）⇒ Ok；慢周期下一轮决策时刻（age > 30s）⇒ Stale。
        assert!(svc.snapshot_input(last_now).is_ok(), "窗内输入必须可用");
        match svc.snapshot_input(produced_ms + 31_000) {
            Err(crate::telemetry::SnapshotInputReject::Stale { .. }) => {}
            other => panic!("慢周期旧输入必须被新鲜度门拒绝：{other:?}（id={id:02x?}）"),
        }
    }
}
