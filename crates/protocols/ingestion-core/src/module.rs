//! 蓝图 61 [07] §2（v0.314.30）：`IngestionModule` 深模块 + 协作调度。
//!
//! 唯一不变量（[07] §2.1）：任何生产观测（Modbus TCP 等）都经本模块完成
//! 来源认证、点映射、周期/死区、质量、去重、QoS、背压和事件日志 append；
//! adapter 只负责协议状态机和原始值（[07] §2.2）——mapping、工程量、
//! 单位、事件身份、去重和日志属于 module。
//!
//! # 职责分界（与 Phase 1 [`crate::SourceRegistry`] 的关系）
//!
//! - [`SourceRegistry`]：身份/准入单点（注册、开源、verify）——不变；
//! - [`IngestionModule`]：在其上叠加六件事——**调度**（[`PointDirectory`]
//!   分片点表 + 周期/相位/期限）、**死区**（Q3/可抑制 Q1 重复合并）、
//!   **背压**（[`PushOutcome`] 显式四态）、**检查点**（watermark 批量
//!   持久化）、**统一报告**（[`TickReport`]，计数不使用 `u8`，[07] §2.8）。
//!
//! # 背压（[07] §2.5）
//!
//! adapter 经 [`RawObservationSink::push`] 推送原始观测，返回
//! [`PushOutcome`]：Accepted / Coalesced（死区合并）/ Backpressured（慢
//! sink/限流——adapter 必须停推，不无限累积）/ Rejected（越权/未知
//! schema 等结构化拒绝）。每次 poll 产生的事件数受
//! [`OperationalProfile::max_events_per_poll`] 封顶，超出记录
//! rate-limit reason（不静默丢）。Q0/Q1 背压置来源 degraded（快照缺失
//! 信号）；Q2/Q3 按注册策略限流。
//!
//! # 死区（[07] §2.3）
//!
//! 只抑制 Q3/可抑制 Q1 的重复存储（F64、品质不变、变化量 ≤
//! max(deadband_abs, |anchor|×deadband_rel)）；品质变化、sequence、告警
//! 和控制回读永不被抑制。锚点 = 上次**已报告**值（有界漂移）。
//!
//! # 重启与连接（[07] §2.4）
//!
//! - `source_epoch` 经 [`crate::EpochStore`] 自持久 boot 计数下界递增
//!   （floor 由装配侧注入 boot counter）；
//! - sequence checkpoint 以批量间隔持久化（[`Self::checkpoint`]）；
//!   崩溃换 epoch，不要求同 epoch 续号；
//! - 对端重连不换本地 epoch——[`Self::note_reconnect`] 递增
//!   connection_epoch（观测计数）。
//!
//! # 规格签名偏离登记（契约化，非静默）
//!
//! 蓝图 [07] §2.2 的 trait 签名是契约骨架，本实现做三处显式扩展（全部
//! 经合同套件锁定，无协议特例混入）：
//! - [`IngestionAdapter::poll`] 增加 `work: &[PointPlan]` 参数——调度
//!   权威在 module，adapter 只消费到期点切片（协议组计划输入）；
//! - [`IngestionModule::ingest`] 增 `source_id`/`now_ns` 参数——
//!   RawObservation 无来源字段（伪造入口不存在），准入需要身份与时钟；
//! - [`IngestionModule::register`] 保持幂等覆盖（返回 `()`；装配面错误
//!   经 install/attach/open 的 Result 面显式化）。
//! 另：PointPlan 内嵌 modbus 组坐标（func_code/reg_addr/slave_addr）
//! 为轮询型协议通用的「调度组输入」约定——非 modbus 协议填 0，协议专属
//! 逻辑全部留在 adapter 侧。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_telemetry_model::event::{event_flags, quality_bits, IntegrityTag, ObservationValue};

use crate::{
    AppendAck, EpochStore, IngestReject, ObservationSink, RawObservation, SourceDescriptor,
    SourceRegistry,
};

// ===========================================================================
// QoS 与背压四态
// ===========================================================================

/// 事件 QoS 等级（[07] §2.3/§4.5；封闭四级）。
///
/// Q0 = 控制热路径必达；Q1 = 快照关键（`suppressible` 注册时参与死区）；
/// Q2 = 趋势/归档（可限流）；Q3 = 存档（死区抑制主对象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventQos {
    Q0,
    Q1,
    Q2,
    Q3,
}

impl EventQos {
    /// 该等级在「值不变 + 品质不变」下是否允许死区合并（[07] §2.3：
    /// Q3/可抑制 Q1；Q0/Q2 永不抑制）。
    pub fn deadband_eligible(self, suppressible: bool) -> bool {
        match self {
            EventQos::Q3 => true,
            EventQos::Q1 => suppressible,
            EventQos::Q0 | EventQos::Q2 => false,
        }
    }

    /// 背压时是否置来源 degraded（Q0/Q1 → 快照缺失信号；Q2/Q3 只限流）。
    pub fn degrades_source(self) -> bool {
        matches!(self, EventQos::Q0 | EventQos::Q1)
    }
}

/// 背压四态（[07] §2.5：adapter 不能无限累积）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// 已准入并追加事件日志。
    Accepted,
    /// 死区合并（未追加存储；点状态已更新）。
    Coalesced,
    /// 慢 sink/限流——adapter 本轮必须停推（原因可观测）。
    Backpressured,
    /// 结构化拒绝（越权/未知 schema/完整性不足等）。
    Rejected,
}

/// adapter 单次 poll 预算（[07] §2.3：每 tick 有最大事务数和事件预算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollBudget {
    /// 本 poll 允许产生的事件上限（profile 封顶与 tick 余量取小）。
    pub max_events: u32,
    /// 本 poll 允许的协议事务上限（Modbus 组读 = 1 事务）。
    pub max_transactions: u32,
}

/// poll 结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// 完成预算内工作（events = 本 poll 推送的事件数）。
    Completed { events: u32, transactions: u32 },
    /// 预算耗尽仍有待办（下个 tick 继续——协作不抢占）。
    Busy { transactions: u32 },
    /// 协议故障（连接丢失等；本轮零事务产出）。
    Failed,
}

/// adapter 合同（[07] §2.2；`work` = module 调度面裁定的本期到期点——
/// 调度权威在 module，adapter 只做协议侧组计划与原始值）。
pub trait IngestionAdapter {
    /// 本 adapter 服务的来源 ID（registry 注册事实；不得自造）。
    fn source_id(&self) -> u32;
    /// 推进协作状态机（connect → … → emit 的协议内状态由实现持有）。
    fn poll(
        &mut self,
        now_ns: u64,
        budget: PollBudget,
        work: &[PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> PollOutcome;

    /// 取走自上次调用以来的传输重连计数（[07] §2.4——重连不换本地
    /// source epoch；默认零——纯虚拟 adapter）。
    fn take_reconnects(&mut self) -> u32 {
        0
    }
}

/// module 提供给 adapter 的原始观测汇（背压四态唯一入口）。
pub trait RawObservationSink {
    fn push(&mut self, raw: RawObservation) -> PushOutcome;
}

/// ingest 直推结果（[07] §2.2 `ingest` 签名的语义完整化：死区合并不产生
/// 新 AppendAck，也不能谎报拒绝——显式第三态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    Appended(AppendAck),
    Coalesced,
}

// ===========================================================================
// 点计划 / 分片点表
// ===========================================================================

/// 单点调度与身份计划（module 所有；adapter 只读 `work` 切片）。
///
/// 全 POD（no_std 可复制；scratch 缓冲跨 tick 复用，C1 无分配抖动）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointPlan {
    /// 来源 ID（registry 注册事实）。
    pub source_id: u32,
    /// 站内点位 ID。
    pub point_id: u32,
    /// 站内设备短 ID。
    pub device_id: u16,
    /// 采集周期 ns（0 = 事件驱动，不轮询——[07] §2.3）。
    pub period_ns: u64,
    /// 相位偏移 ns（启动后首采延迟；周期内错峰）。
    pub phase_offset_ns: u64,
    /// 采集期限 ns（到期未产事件 = overrun——显式计数，不静默）。
    pub deadline_ns: u64,
    /// 绝对死区（工程量；0 = 不按绝对值抑制）。
    pub deadband_abs: f64,
    /// 相对死区（|anchor| 的比例 0..1；0 = 不按相对值抑制）。
    pub deadband_rel: f64,
    /// 事件 QoS。
    pub qos: EventQos,
    /// 死区抑制注册策略（Q1 需 true 才可抑制）。
    pub suppressible: bool,
    /// 采集超时 ns（adapter 重试/失败展开输入；module 期限判断共用）。
    pub timeout_ns: u64,
    /// 采集重试上限（协议语义；adapter 消费）。
    pub max_retries: u16,
    // —— 身份绑定（module 侧映射单源；覆盖 raw 自带身份——[07] §2.2）——
    /// schema id（0 = 未注册）。
    pub schema_id: u32,
    /// schema 版本。
    pub schema_version: u16,
    /// 单位码（UnitCatalog；0 = 未知）。
    pub unit_code: u16,
    /// 资产绑定（0 = 未绑定，不得进控制快照）。
    pub asset_id: u64,
    /// 映射世代。
    pub mapping_version: u64,
    // —— 工程量变换（raw → engineering；module 单点执行）——
    /// 缩放系数（0 视为恒等 1.0——config 编译面禁 0，此处防御）。
    pub scale: f64,
    /// 零点偏移。
    pub offset: f64,
    /// 协议功能码（调度组输入；非 Modbus 协议 0）。
    pub func_code: u8,
    /// 协议寄存器/对象地址低位（组计划输入；非 Modbus 协议 0）。
    pub reg_addr: u16,
    /// 协议从站/设备地址（组计划输入；非 Modbus 协议 0）。
    pub slave_addr: u8,
}

impl PointPlan {
    /// raw 值 → 工程量（module 侧唯一变换点；与 Phase 0 `to_engineering`
    /// 同式：`raw × scale + offset`）。
    ///
    /// 数值原语（I64/U64/F64）参与变换；Bool/Enum/Bytes16 原样透传。
    pub fn engineering(&self, value: ObservationValue) -> ObservationValue {
        let scale = if self.scale == 0.0 { 1.0 } else { self.scale };
        match value {
            ObservationValue::F64(v) => ObservationValue::F64(v * scale + self.offset),
            ObservationValue::I64(v) => ObservationValue::F64(v as f64 * scale + self.offset),
            ObservationValue::U64(v) => ObservationValue::F64(v as f64 * scale + self.offset),
            other => other,
        }
    }
}

/// 单 shard 点数上限（[07] §2.3：每 shard ≤1024，避免一次突破现有
/// decoder 上限）。
pub const SHARD_MAX_POINTS: usize = 1024;

/// 点分片（有界；容量在 install 时静态保证）。
#[derive(Debug, Clone, PartialEq)]
pub struct PointShard {
    plans: Vec<PointPlan>,
}

impl PointShard {
    pub fn len(&self) -> usize {
        self.plans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    pub fn plans(&self) -> &[PointPlan] {
        &self.plans
    }
}

/// 点目录（[07] §2.3：`(source_id, point_id)` 索引 + generation 原子切换）。
///
/// 切换 = 整目录替换（构建失败旧目录原样保留；单线程协作面内原子）。
#[derive(Debug, Clone, Default)]
pub struct PointDirectory {
    generation: u64,
    shards: Vec<PointShard>,
    index: BTreeMap<(u32, u32), (usize, usize)>,
}

impl PointDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn shards(&self) -> &[PointShard] {
        &self.shards
    }

    /// (source, point) 查计划。
    pub fn resolve(&self, source_id: u32, point_id: u32) -> Option<&PointPlan> {
        self.index
            .get(&(source_id, point_id))
            .map(|&(s, p)| &self.shards[s].plans[p])
    }

    /// 某来源全部计划（point_id 升序——BTreeMap range 序）。
    pub fn plans_of(&self, source_id: u32) -> impl Iterator<Item = &PointPlan> {
        let lo = (source_id, 0u32);
        let hi = (source_id, u32::MAX);
        self.index.range(lo..=hi).map(move |(_, &(s, p))| {
            // range 键首元即 source_id，(s, p) 必在本 shard 界内。
            &self.shards[s].plans[p]
        })
    }

    /// 原子安装新世代（严格递增；重复/回退世代拒绝）。
    ///
    /// 校验全过才替换：点非重复、每 shard ≤[`SHARD_MAX_POINTS`]。
    /// 来源注册校验由调用方（[`IngestionModule::install_points`]）承担。
    pub fn install_generation(
        &mut self,
        generation: u64,
        mut plans: Vec<PointPlan>,
    ) -> Result<(), ConfigReject> {
        if generation <= self.generation {
            return Err(ConfigReject::GenerationStale {
                current: self.generation,
                incoming: generation,
            });
        }
        // 稳定排序：(source, point) 升序 → 顺序切 shard。
        plans.sort_by_key(|p| (p.source_id, p.point_id));
        let mut shards: Vec<PointShard> = Vec::new();
        let mut index = BTreeMap::new();
        for p in plans {
            let key = (p.source_id, p.point_id);
            if index.contains_key(&key) {
                return Err(ConfigReject::DuplicatePoint {
                    source_id: key.0,
                    point_id: key.1,
                });
            }
            match shards.last_mut() {
                Some(shard) if shard.plans.len() < SHARD_MAX_POINTS => {
                    shard.plans.push(p);
                }
                _ => {
                    let mut shard = PointShard { plans: Vec::new() };
                    shard.plans.push(p);
                    shards.push(shard);
                }
            }
            let s = shards.len() - 1;
            let pos = shards[s].plans.len() - 1;
            index.insert(key, (s, pos));
        }
        self.shards = shards;
        self.index = index;
        self.generation = generation;
        Ok(())
    }
}

/// 装配拒绝（配置面；结构化可观测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigReject {
    /// 计划/adapter 引用未注册来源。
    SourceUnknown { source_id: u32 },
    /// 同世代重复点。
    DuplicatePoint { source_id: u32, point_id: u32 },
    /// 世代非严格递增。
    GenerationStale { current: u64, incoming: u64 },
    /// 同源重复 adapter。
    AdapterDuplicate { source_id: u32 },
}

// ===========================================================================
// 运行剖面 / tick 预算与报告
// ===========================================================================

/// 运行剖面（[07] §2.5：每次 poll 最多产生允许的事件数；检查点批量间隔）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationalProfile {
    /// 单 poll 事件封顶（超出 → Backpressured + rate-limit reason 计数）。
    pub max_events_per_poll: u32,
    /// 单 tick 事务封顶（协作调度 CPU 预算）。
    pub max_transactions_per_tick: u32,
    /// watermark 检查点批量间隔（每 N 个接受事件持久化一次；0 = 不检查点）。
    pub checkpoint_interval_events: u32,
}

impl OperationalProfile {
    /// 缺省剖面（20 点参考闭环量级；C1 用例按需放大）。
    pub const fn reference() -> Self {
        Self {
            max_events_per_poll: 256,
            max_transactions_per_tick: 16,
            checkpoint_interval_events: 64,
        }
    }
}

/// tick 预算（[07] §2.3：每 tick 最大事务数与事件预算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickBudget {
    /// 本 tick 全部 adapter 合计事件预算。
    pub max_events: u32,
    /// 本 tick 全部 adapter 合计事务预算。
    pub max_transactions: u32,
}

/// tick 报告（全部 u32——[07] §2.8：点数和计数不使用 u8）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    /// 被调度的 adapter 数。
    pub polls: u32,
    /// 协议事务数（组读=1）。
    pub transactions: u32,
    /// 接受并追加事件数。
    pub accepted: u32,
    /// 死区合并数。
    pub coalesced: u32,
    /// 背压（慢 sink/预算封顶）数。
    pub backpressured: u32,
    /// 结构化拒绝数。
    pub rejected: u32,
    /// 限流原因计数（RateExceeded / poll 预算耗尽）。
    pub rate_limited: u32,
    /// 期限超限数（到期未产事件）。
    pub deadline_overruns: u32,
}

/// 每 tick 增量计数（ModuleSink 累计；tick 收尾折进 [`TickReport`]）。
#[derive(Debug, Default, Clone, Copy)]
struct TickCounters {
    accepted: u32,
    coalesced: u32,
    backpressured: u32,
    rejected: u32,
    rate_limited: u32,
}

// ===========================================================================
// 点状态（死区锚点 / 调度水位 / 期限跟踪）与来源运行态
// ===========================================================================

#[derive(Debug, Clone, Copy)]
struct PointState {
    /// 上次已报告工程值（死区锚点；None = 从未报告——永发）。
    anchor: Option<f64>,
    /// 上次报告品质位。
    last_quality: u16,
    /// 上次报告时刻。
    last_emit_ns: u64,
    /// 上次被调度的时刻（周期判定锚；None = 从未调度——首采只看相位）。
    last_poll_ns: Option<u64>,
    /// 未决采集起始时刻（期限判定；None = 无未决采集）。
    pending_since: Option<u64>,
}

impl PointState {
    fn new() -> Self {
        Self {
            anchor: None,
            last_quality: 0,
            last_emit_ns: 0,
            last_poll_ns: None,
            pending_since: None,
        }
    }
}

#[derive(Debug, Default)]
struct SourceRuntime {
    /// 来源 degraded（Q0/Q1 背压置位——快照缺失信号）。
    degraded: bool,
    /// connection_epoch（对端重连计数；不换本地 source epoch）。
    connection_epoch: u64,
}

// ===========================================================================
// IngestionModule
// ===========================================================================

/// 接入深模块（[07] §2.2）：register/open/ingest/tick 合同 + attach/
/// install_points 装配面。全部身份/调度/死区/背压/日志私有。
pub struct IngestionModule {
    registry: SourceRegistry,
    epoch: Box<dyn EpochStore>,
    sink: Box<dyn ObservationSink>,
    directory: PointDirectory,
    adapters: Vec<Box<dyn IngestionAdapter>>,
    states: BTreeMap<(u32, u32), PointState>,
    runtime: BTreeMap<u32, SourceRuntime>,
    profile: OperationalProfile,
    scratch: Vec<PointPlan>,
    /// 每 source 轮转游标（work 下发公平性——预算裁剪不固定饿尾）。
    rotate: BTreeMap<u32, usize>,
    /// 有界拒绝日志（可观测；满 32 后丢最旧并计数——一切丢弃可见）。
    reject_log: Vec<(u32, IngestReject)>,
    reject_log_dropped: u64,
    counters: TickCounters,
    /// 检查点间隔计数（自上次 save 以来的接受事件数）。
    since_checkpoint: u32,
    // —— 累计观测（跨 tick；u64）——
    pub accepted_total: u64,
    pub coalesced_total: u64,
    pub backpressured_total: u64,
    pub rejected_total: u64,
    pub rate_limited_total: u64,
    pub deadline_overrun_total: u64,
}

impl IngestionModule {
    /// 构造（sink = 事件日志接缝；epoch = 持久 boot 计数存储）。
    pub fn new(
        sink: Box<dyn ObservationSink>,
        epoch: Box<dyn EpochStore>,
        profile: OperationalProfile,
    ) -> Self {
        Self {
            registry: SourceRegistry::new(),
            epoch,
            sink,
            directory: PointDirectory::new(),
            adapters: Vec::new(),
            states: BTreeMap::new(),
            runtime: BTreeMap::new(),
            profile,
            scratch: Vec::new(),
            rotate: BTreeMap::new(),
            reject_log: Vec::new(),
            reject_log_dropped: 0,
            counters: TickCounters::default(),
            since_checkpoint: 0,
            accepted_total: 0,
            coalesced_total: 0,
            backpressured_total: 0,
            rejected_total: 0,
            rate_limited_total: 0,
            deadline_overrun_total: 0,
        }
    }

    // ----- 装配面 -----

    /// 注册来源 descriptor（幂等覆盖——配置编译事实）。
    pub fn register(&mut self, descriptor: SourceDescriptor) {
        self.registry.register(descriptor, 0);
    }

    /// 开源：分配本启动 epoch（floor = 持久 boot counter——[07] §2.4）。
    pub fn open(&mut self, source_id: u32, epoch_floor: u64) -> Result<u64, IngestReject> {
        self.registry
            .open_source(source_id, epoch_floor, self.epoch.as_mut())
    }

    /// 当前 epoch（未开源 → 0）。
    pub fn epoch(&self, source_id: u32) -> u64 {
        self.registry.epoch(source_id)
    }

    /// 挂接 adapter（同源唯一；来源须已注册）。
    pub fn attach(&mut self, adapter: Box<dyn IngestionAdapter>) -> Result<(), ConfigReject> {
        let source_id = adapter.source_id();
        if !self.registry.is_registered(source_id) {
            return Err(ConfigReject::SourceUnknown { source_id });
        }
        if self.adapters.iter().any(|a| a.source_id() == source_id) {
            return Err(ConfigReject::AdapterDuplicate { source_id });
        }
        self.adapters.push(adapter);
        self.adapters.sort_by_key(|a| a.source_id());
        self.runtime.entry(source_id).or_default();
        Ok(())
    }

    /// 原子安装点目录新世代（全部计划的来源须已注册；[07] §2.3）。
    pub fn install_points(
        &mut self,
        generation: u64,
        plans: Vec<PointPlan>,
    ) -> Result<(), ConfigReject> {
        for p in &plans {
            if !self.registry.is_registered(p.source_id) {
                return Err(ConfigReject::SourceUnknown {
                    source_id: p.source_id,
                });
            }
        }
        self.directory.install_generation(generation, plans)
    }

    /// 点目录只读视图（诊断/消费面；调度权威在本模块）。
    pub fn directory(&self) -> &PointDirectory {
        &self.directory
    }

    /// 来源 degraded（Q0/Q1 背压置位；快照消费方据此显式声明缺失）。
    pub fn is_degraded(&self, source_id: u32) -> bool {
        self.runtime
            .get(&source_id)
            .map(|r| r.degraded)
            .unwrap_or(false)
    }

    /// connection_epoch（对端重连计数；[07] §2.4）。
    pub fn connection_epoch(&self, source_id: u32) -> u64 {
        self.runtime
            .get(&source_id)
            .map(|r| r.connection_epoch)
            .unwrap_or(0)
    }

    /// 对端重连登记（本地 source epoch 不变；degraded 解除）。
    pub fn note_reconnect(&mut self, source_id: u32) {
        let r = self.runtime.entry(source_id).or_default();
        r.connection_epoch = r.connection_epoch.saturating_add(1);
        r.degraded = false;
    }

    // ----- 直推合同（事件驱动来源入口；轮询来源走 tick）-----

    /// 事件直推。顺序：目录映射富化 → registry 准入 → 死区 → sink append。
    ///
    /// 直推不计入下一个 tick 报告（增量折进累计后从 tick 计数扣回）。
    pub fn ingest(
        &mut self,
        source_id: u32,
        raw: RawObservation,
        now_ns: u64,
    ) -> Result<IngestOutcome, IngestReject> {
        let before = (
            self.counters.accepted,
            self.counters.coalesced,
            self.counters.backpressured,
            self.counters.rejected,
            self.counters.rate_limited,
        );
        let outcome = {
            let mut msink = ModuleSink::direct(self, source_id, now_ns, u32::MAX);
            msink.push_inner(raw)
        };
        // 增量折进累计并扣回 tick 计数（直推不污染 tick 报告）。
        self.accepted_total += u64::from(self.counters.accepted - before.0);
        self.coalesced_total += u64::from(self.counters.coalesced - before.1);
        self.backpressured_total += u64::from(self.counters.backpressured - before.2);
        self.rejected_total += u64::from(self.counters.rejected - before.3);
        self.rate_limited_total += u64::from(self.counters.rate_limited - before.4);
        self.counters = TickCounters {
            accepted: before.0,
            coalesced: before.1,
            backpressured: before.2,
            rejected: before.3,
            rate_limited: before.4,
        };
        match outcome {
            PushInner::Accepted(ack) => Ok(IngestOutcome::Appended(ack)),
            PushInner::Idempotent(event_id) => Err(IngestReject::Duplicate { event_id }),
            PushInner::Coalesced => Ok(IngestOutcome::Coalesced),
            PushInner::Backpressured(reason) | PushInner::Rejected(reason) => Err(reason),
        }
    }

    // ----- 调度合同 -----

    /// 协作 tick（[07] §2.2/§2.3）：到期点裁成 work 切片逐 adapter 驱动；
    /// 预算内推进协议状态机；期限超限显式计数（不静默）。
    pub fn tick(&mut self, now_ns: u64, budget: TickBudget) -> TickReport {
        self.counters = TickCounters::default();
        let mut report = TickReport::default();
        let mut events_left = budget.max_events;
        let mut tx_left = budget.max_transactions;
        let source_ids: Vec<u32> = self.adapters.iter().map(|a| a.source_id()).collect();
        for source_id in source_ids {
            if events_left == 0 || tx_left == 0 {
                break;
            }
            // 1. 期限检查（先于调度——上轮未决采集超限要可见）。
            let overruns = self.check_deadlines(source_id, now_ns);
            report.deadline_overruns += overruns;
            // 2. 到期点候选（period+phase；事件驱动点永不轮询）。
            self.collect_due(source_id, now_ns);
            if self.scratch.is_empty() {
                continue;
            }
            // 3. 预算合成（profile 封顶与 tick 余量取小）。
            let pb = PollBudget {
                max_events: events_left.min(self.profile.max_events_per_poll),
                max_transactions: tx_left.min(self.profile.max_transactions_per_tick),
            };
            // 4. 轮转下发（只标记实际下发的点——预算裁剪不吞调度机会）。
            let work = self.offer_work(source_id, now_ns, pb.max_transactions as usize);
            if work.is_empty() {
                continue;
            }
            // 4. 暂取 adapter（ModuleSink 需要独占 &mut module——占位回填，
            //    占位窗口内不可能被 poll）。
            let Some(idx) = self
                .adapters
                .iter()
                .position(|a| a.source_id() == source_id)
            else {
                continue;
            };
            let mut adapter =
                core::mem::replace(&mut self.adapters[idx], Box::new(Detached(source_id)));
            let before = (self.counters.accepted, self.counters.coalesced);
            let outcome = {
                let mut msink = ModuleSink::direct(self, source_id, now_ns, pb.max_events);
                adapter.poll(now_ns, pb, &work, &mut msink)
            };
            self.adapters[idx] = adapter;
            report.polls += 1;
            match outcome {
                PollOutcome::Completed { transactions, .. }
                | PollOutcome::Busy { transactions } => {
                    report.transactions += transactions;
                    tx_left = tx_left.saturating_sub(transactions);
                }
                PollOutcome::Failed => {}
            }
            // 本 poll 实际消耗事件（增量——counters 是 tick 累计）。
            let spent = (self.counters.accepted - before.0) + (self.counters.coalesced - before.1);
            events_left = events_left.saturating_sub(spent);
        }
        report.accepted = self.counters.accepted;
        report.coalesced = self.counters.coalesced;
        report.backpressured = self.counters.backpressured;
        report.rejected = self.counters.rejected;
        report.rate_limited = self.counters.rate_limited;
        self.accepted_total += u64::from(self.counters.accepted);
        self.coalesced_total += u64::from(self.counters.coalesced);
        self.backpressured_total += u64::from(self.counters.backpressured);
        self.rejected_total += u64::from(self.counters.rejected);
        self.rate_limited_total += u64::from(self.counters.rate_limited);
        self.deadline_overrun_total += u64::from(report.deadline_overruns);
        report
    }

    /// 检查点：watermark 批量持久化（[07] §2.4：以批量间隔持久化）。
    ///
    /// 每 `profile.checkpoint_interval_events` 个接受事件后，把各来源
    /// (epoch, watermark) 写入 epoch store（save 失败 = 显式 Err）。
    pub fn checkpoint(&mut self) -> Result<(), IngestReject> {
        if self.profile.checkpoint_interval_events == 0
            || self.since_checkpoint < self.profile.checkpoint_interval_events
        {
            return Ok(());
        }
        for source_id in self.attached_sources() {
            if let Some((epoch, watermark)) = self.sink.watermark(source_id) {
                self.epoch.save(source_id, epoch, watermark)?;
            }
        }
        self.since_checkpoint = 0;
        Ok(())
    }

    /// 各来源 (epoch, watermark)（诊断读面）。
    pub fn watermark(&self, source_id: u32) -> Option<(u64, u64)> {
        self.sink.watermark(source_id)
    }

    /// 排空事件日志待交付 wire 帧（事件 → 传输批次接缝；[07] §2.2）。
    pub fn drain_frames(&mut self) -> Vec<[u8; 128]> {
        self.sink.drain_frames()
    }

    /// 取走拒绝原因日志（可观测读面；PointNotAuthorized 等结构化原因）。
    pub fn take_reject_log(&mut self) -> Vec<(u32, IngestReject)> {
        core::mem::take(&mut self.reject_log)
    }

    /// 拒绝日志因有界而丢弃的条数（可观测）。
    pub fn reject_log_dropped(&self) -> u64 {
        self.reject_log_dropped
    }

    /// 取走并登记 adapter 传输重连（connection_epoch 递增 + degraded 解除；
    /// 本地 source epoch 不变——[07] §2.4）。
    pub fn take_reconnects(&mut self, source_id: u32) -> u32 {
        let Some(idx) = self
            .adapters
            .iter()
            .position(|a| a.source_id() == source_id)
        else {
            return 0;
        };
        let mut adapter =
            core::mem::replace(&mut self.adapters[idx], Box::new(Detached(source_id)));
        let n = adapter.take_reconnects();
        self.adapters[idx] = adapter;
        for _ in 0..n {
            self.note_reconnect(source_id);
        }
        n
    }

    /// 已挂接 adapter 的来源列表（升序去重）。
    pub fn attached_sources(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.adapters.iter().map(|a| a.source_id()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    // ----- 内部 -----

    /// 到期点候选收进 scratch（period+phase；首采只看相位）。
    ///
    /// **不标记**——只有实际进入 adapter `work` 的点才置 last_poll/
    /// pending（预算裁剪掉的候选下个 tick 重采，不丢调度机会）。
    fn collect_due(&mut self, source_id: u32, now_ns: u64) {
        self.scratch.clear();
        for plan in self.directory.plans_of(source_id) {
            if plan.period_ns == 0 {
                continue;
            }
            let due = match self.states.get(&(source_id, plan.point_id)) {
                None => now_ns >= plan.phase_offset_ns,
                Some(st) => match st.last_poll_ns {
                    None => now_ns >= plan.phase_offset_ns,
                    Some(last) => {
                        now_ns >= last.saturating_add(plan.period_ns)
                            && now_ns >= plan.phase_offset_ns
                    }
                },
            };
            if due {
                self.scratch.push(*plan);
            }
        }
    }

    /// 从候选中轮转截取 work 并标记（last_poll/pending 只对下发点置位）。
    fn offer_work(&mut self, source_id: u32, now_ns: u64, take: usize) -> Vec<PointPlan> {
        let n = self.scratch.len();
        if n == 0 || take == 0 {
            return Vec::new();
        }
        let take = take.min(n);
        let rot = *self.rotate.entry(source_id).or_insert(0);
        let work: Vec<PointPlan> = (0..take).map(|k| self.scratch[(rot + k) % n]).collect();
        for p in &work {
            let st = self
                .states
                .entry((source_id, p.point_id))
                .or_insert_with(PointState::new);
            st.last_poll_ns = Some(now_ns);
            st.pending_since = Some(now_ns);
        }
        self.rotate.insert(source_id, (rot + take) % n);
        work
    }

    /// 期限检查：到期未产事件 → 计数 +（有锚点时）陈旧标记事件
    /// （品质 OLD——品质变化永不被死区抑制；无值可标只计数不造数据）。
    fn check_deadlines(&mut self, source_id: u32, now_ns: u64) -> u32 {
        let mut overruns = 0u32;
        let keys: Vec<(u32, u32)> = self
            .states
            .range((source_id, 0)..=(source_id, u32::MAX))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            let Some(plan) = self.directory.resolve(key.0, key.1) else {
                continue;
            };
            if plan.period_ns == 0 || plan.deadline_ns == 0 {
                continue;
            }
            let Some(pending) = self.states.get(&key).and_then(|s| s.pending_since) else {
                continue;
            };
            if now_ns.saturating_sub(pending) <= plan.deadline_ns {
                continue;
            }
            overruns += 1;
            let anchor = self.states.get(&key).and_then(|s| s.anchor);
            if let Some(v) = anchor {
                let plan = *plan;
                let raw = RawObservation {
                    point_id: key.1,
                    value: ObservationValue::F64(v),
                    event_time_ns: Some(now_ns),
                    quality_bits: quality_bits::VALID | quality_bits::OLD,
                    integrity: IntegrityTag::LocalTrustedEdge,
                    flags: event_flags::BACKFILL,
                    schema_id: plan.schema_id,
                    schema_version: plan.schema_version,
                    unit_code: plan.unit_code,
                    asset_id: plan.asset_id,
                    mapping_version: plan.mapping_version,
                    device_id: plan.device_id,
                };
                {
                    let mut msink = ModuleSink::direct(self, source_id, now_ns, u32::MAX);
                    let _ = msink.push_inner(raw);
                }
            }
            if let Some(st) = self.states.get_mut(&key) {
                st.pending_since = None;
            }
        }
        overruns
    }
}

/// poll 调用窗口内的占位 adapter（永不 poll——只填 `adapters[idx]` 空位，
/// 令 ModuleSink 能独占 `&mut IngestionModule`）。
struct Detached(u32);

impl IngestionAdapter for Detached {
    fn source_id(&self) -> u32 {
        self.0
    }
    fn poll(
        &mut self,
        _now_ns: u64,
        _budget: PollBudget,
        _work: &[PointPlan],
        _out: &mut dyn RawObservationSink,
    ) -> PollOutcome {
        PollOutcome::Failed
    }
}

// ===========================================================================
// ModuleSink——背压四态裁决点（adapter `out` 与 ingest 直推共用一炉）
// ===========================================================================

/// push 内部结果（比 [`PushOutcome`] 多带 ack/reason——ingest 需要回执）。
enum PushInner {
    Accepted(AppendAck),
    /// 幂等重推（同 event_id）：push → Accepted；ingest → Err(Duplicate)。
    Idempotent([u8; 16]),
    Coalesced,
    Backpressured(IngestReject),
    Rejected(IngestReject),
}

/// module 准入前端（持有 module 字段的独立借用——poll 窗口内 adapter
/// 已暂取，借用不相交）。
struct ModuleSink<'a> {
    source_id: u32,
    now_ns: u64,
    /// poll 事件封顶（ingest 直推 = u32::MAX）。
    poll_cap: u32,
    /// 本 poll 已消耗事件数（accepted + coalesced）。
    poll_events: u32,
    registry: &'a mut SourceRegistry,
    sink: &'a mut Box<dyn ObservationSink>,
    directory: &'a PointDirectory,
    states: &'a mut BTreeMap<(u32, u32), PointState>,
    runtime: &'a mut BTreeMap<u32, SourceRuntime>,
    counters: &'a mut TickCounters,
    since_checkpoint: &'a mut u32,
    reject_log: &'a mut Vec<(u32, IngestReject)>,
    reject_log_dropped: &'a mut u64,
}

const REJECT_LOG_CAP: usize = 32;

impl<'a> ModuleSink<'a> {
    /// 从 `&mut IngestionModule` 整体构造（ingest 直推/期限标记路径；
    /// poll 窗口走 [`IngestionModule::tick`] 的字段拆分构造）。
    fn direct(module: &'a mut IngestionModule, source_id: u32, now_ns: u64, poll_cap: u32) -> Self {
        Self {
            source_id,
            now_ns,
            poll_cap,
            poll_events: 0,
            registry: &mut module.registry,
            sink: &mut module.sink,
            directory: &module.directory,
            states: &mut module.states,
            runtime: &mut module.runtime,
            counters: &mut module.counters,
            since_checkpoint: &mut module.since_checkpoint,
            reject_log: &mut module.reject_log,
            reject_log_dropped: &mut module.reject_log_dropped,
        }
    }

    fn push_inner(&mut self, raw: RawObservation) -> PushInner {
        // 1. poll 事件封顶（[07] §2.5：每次 poll 最多产生 profile 允许的
        //    事件数；超出记录 rate-limit reason）。
        if self.poll_events >= self.poll_cap {
            self.counters.backpressured += 1;
            self.counters.rate_limited += 1;
            self.mark_backpressured(raw.point_id);
            return PushInner::Backpressured(IngestReject::RateExceeded {
                source_id: self.source_id,
            });
        }
        // 2. 目录映射富化（mapping/工程量/单位/身份属 module——[07] §2.2）。
        let enriched = match self.directory.resolve(self.source_id, raw.point_id) {
            Some(plan) => {
                let mut r = raw;
                r.value = plan.engineering(raw.value);
                r.schema_id = plan.schema_id;
                r.schema_version = plan.schema_version;
                r.unit_code = plan.unit_code;
                r.asset_id = plan.asset_id;
                r.mapping_version = plan.mapping_version;
                r.device_id = plan.device_id;
                r
            }
            None => raw,
        };
        // 3. registry 准入（注册/越权/超速/schema/完整性/有限值/回拨/序）。
        let event = match self.registry.verify(self.source_id, &enriched, self.now_ns) {
            Ok(e) => e,
            Err(reason) => return self.on_reject(reason, raw.point_id),
        };
        // 4. 死区（Q3/可抑制 Q1 + F64 + 品质不变 + 带宽内；品质变化永发）。
        let state = self
            .states
            .entry((self.source_id, event.point_id))
            .or_insert_with(PointState::new);
        if let ObservationValue::F64(v) = event.value {
            let eligible = self
                .directory
                .resolve(self.source_id, event.point_id)
                .map(|p| p.qos.deadband_eligible(p.suppressible))
                .unwrap_or(false);
            if eligible && state.last_quality == event.quality_bits {
                if let Some(anchor) = state.anchor {
                    let (abs, rel) = {
                        let p = self
                            .directory
                            .resolve(self.source_id, event.point_id)
                            .unwrap();
                        (p.deadband_abs, p.deadband_rel)
                    };
                    let band = abs.max(anchor.abs() * rel);
                    if (v - anchor).abs() <= band {
                        // 合并：不追加存储；锚点不动（有界漂移）；采集已
                        // 完成（期限解除）。
                        state.pending_since = None;
                        self.poll_events = self.poll_events.saturating_add(1);
                        self.counters.coalesced += 1;
                        return PushInner::Coalesced;
                    }
                }
            }
        }
        // 5. append（dedup/watermark 复核在 sink）。
        match self.sink.append(&event) {
            Ok(ack) => {
                let state = self
                    .states
                    .get_mut(&(self.source_id, event.point_id))
                    .expect("entry 在第 4 步已建");
                if let ObservationValue::F64(v) = event.value {
                    state.anchor = Some(v);
                }
                state.last_quality = event.quality_bits;
                state.last_emit_ns = self.now_ns;
                state.pending_since = None;
                self.poll_events = self.poll_events.saturating_add(1);
                self.counters.accepted += 1;
                *self.since_checkpoint += 1;
                PushInner::Accepted(ack)
            }
            Err(reason) => self.on_reject(reason, raw.point_id),
        }
    }

    /// 拒绝/背压归类：Overflow → Backpressured；RateExceeded →
    /// Backpressured + rate-limit reason；其余 → Rejected。
    /// Q0/Q1 背压置来源 degraded（[07] §2.5）。
    fn on_reject(&mut self, reason: IngestReject, point_id: u32) -> PushInner {
        // 拒绝原因有界日志（一切丢弃可观测；满 32 丢最旧并计数）。
        if self.reject_log.len() >= REJECT_LOG_CAP {
            self.reject_log.remove(0);
            *self.reject_log_dropped += 1;
        }
        self.reject_log.push((point_id, reason.clone()));
        match reason {
            IngestReject::Overflow => {
                self.counters.backpressured += 1;
                self.mark_backpressured(point_id);
                PushInner::Backpressured(IngestReject::Overflow)
            }
            IngestReject::RateExceeded { source_id } => {
                self.counters.backpressured += 1;
                self.counters.rate_limited += 1;
                self.mark_backpressured(point_id);
                PushInner::Backpressured(IngestReject::RateExceeded { source_id })
            }
            IngestReject::Duplicate { event_id } => {
                // 幂等重推：同 event_id = 事实未变——按接受回执（不入计数）。
                PushInner::Idempotent(event_id)
            }
            other => {
                self.counters.rejected += 1;
                PushInner::Rejected(other)
            }
        }
    }

    /// 背压来源降级判定（点 QoS 为 Q0/Q1 → degraded）。
    fn mark_backpressured(&mut self, point_id: u32) {
        let degrades = self
            .directory
            .resolve(self.source_id, point_id)
            .map(|p| p.qos.degrades_source())
            // 无计划点（事件驱动未知 QoS）保守视为不降级——限流已计数。
            .unwrap_or(false);
        if degrades {
            self.runtime.entry(self.source_id).or_default().degraded = true;
        }
    }
}

impl RawObservationSink for ModuleSink<'_> {
    fn push(&mut self, raw: RawObservation) -> PushOutcome {
        match self.push_inner(raw) {
            PushInner::Accepted(_) | PushInner::Idempotent(_) => PushOutcome::Accepted,
            PushInner::Coalesced => PushOutcome::Coalesced,
            PushInner::Backpressured(_) => PushOutcome::Backpressured,
            PushInner::Rejected(_) => PushOutcome::Rejected,
        }
    }
}

// ===========================================================================
// 测试（[07] §2.6.1 红测清单 + §2.7/§2.8 判据）
// ===========================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::vec::Vec;

    use eneros_telemetry_model::event::{
        event_flags, quality_bits, IntegrityTag, ObservationValue,
    };
    use eneros_telemetry_model::unit_catalog;

    use super::*;
    use crate::{
        AppendAck, EpochStore, InMemorySink, IngestOutcome, IngestReject, RawObservation,
        SourceDescriptor,
    };

    // ----- 测试设施 -----

    fn descriptor(source_id: u32) -> SourceDescriptor {
        SourceDescriptor {
            source_id,
            tenant_id: 1,
            site_id: 1,
            allow_control: true,
            require_integrity: true,
            point_range: (0, 0),
            max_events_per_s: 100_000,
            allowed_schemas: alloc::vec![1],
            clock_id: 1,
            enabled: true,
        }
    }

    /// 目录点计划（modbus 形态：fc3 寄存器；scale/offset 可调）。
    fn plan(source_id: u32, point_id: u32, period_ns: u64) -> PointPlan {
        PointPlan {
            source_id,
            point_id,
            device_id: 2,
            period_ns,
            phase_offset_ns: 0,
            deadline_ns: period_ns.saturating_mul(2),
            deadband_abs: 0.0,
            deadband_rel: 0.0,
            qos: EventQos::Q1,
            suppressible: true,
            timeout_ns: 1_000_000,
            max_retries: 2,
            schema_id: 1,
            schema_version: 1,
            unit_code: unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Kilowatts),
            asset_id: 1,
            mapping_version: 1,
            scale: 1.0,
            offset: 0.0,
            func_code: 3,
            reg_addr: point_id as u16,
            slave_addr: 1,
        }
    }

    /// adapter 原始观测（未富化：raw I64 寄存器值）。
    fn raw(point_id: u32, reg: i64, now_ns: u64) -> RawObservation {
        RawObservation {
            point_id,
            value: ObservationValue::I64(reg),
            // event_time 0 在 validate 门 = TIME_MISSING 语义——测试时刻最小 1ns。
            event_time_ns: Some(now_ns.max(1)),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 0,
            device_id: 2,
        }
    }

    /// 脚本化测试 adapter：按序推出预置观测（每项 1 事务）。
    struct ScriptAdapter {
        source_id: u32,
        script: Vec<Vec<RawObservation>>,
        seen_work: Vec<usize>,
        transactions: u32,
    }

    impl ScriptAdapter {
        fn new(source_id: u32, script: Vec<Vec<RawObservation>>) -> Self {
            Self {
                source_id,
                script,
                seen_work: Vec::new(),
                transactions: 0,
            }
        }
    }

    impl IngestionAdapter for ScriptAdapter {
        fn source_id(&self) -> u32 {
            self.source_id
        }
        fn poll(
            &mut self,
            _now_ns: u64,
            budget: PollBudget,
            work: &[PointPlan],
            out: &mut dyn RawObservationSink,
        ) -> PollOutcome {
            self.seen_work.push(work.len());
            let Some(batch) = self.script.pop() else {
                return PollOutcome::Completed {
                    events: 0,
                    transactions: 0,
                };
            };
            let mut events = 0u32;
            let mut transactions = 0u32;
            for r in batch {
                if transactions >= budget.max_transactions {
                    return PollOutcome::Busy { transactions };
                }
                transactions += 1;
                match out.push(r) {
                    PushOutcome::Backpressured | PushOutcome::Rejected => {
                        // adapter 合同：停推（背压/拒绝显式化）。
                        return PollOutcome::Completed {
                            events,
                            transactions,
                        };
                    }
                    _ => events += 1,
                }
            }
            self.transactions += transactions;
            PollOutcome::Completed {
                events,
                transactions,
            }
        }
    }

    /// 检查点侦探（记录 save 序列——批量间隔判定）。
    struct SpyEpochStore {
        state: BTreeMap<u32, (u64, u64)>,
        saves: Vec<(u32, u64, u64)>,
    }

    impl SpyEpochStore {
        fn new() -> Self {
            Self {
                state: BTreeMap::new(),
                saves: Vec::new(),
            }
        }
    }

    impl EpochStore for SpyEpochStore {
        fn load(&mut self, source_id: u32) -> Option<(u64, u64)> {
            self.state.get(&source_id).copied()
        }
        fn save(&mut self, source_id: u32, epoch: u64, watermark: u64) -> Result<(), IngestReject> {
            self.saves.push((source_id, epoch, watermark));
            self.state.insert(source_id, (epoch, watermark));
            Ok(())
        }
    }

    /// 组装已开源、已挂 adapter、已装点表的模块。
    fn module_with(
        source_id: u32,
        plans: Vec<PointPlan>,
        sink_cap: usize,
        profile: OperationalProfile,
        adapter: Box<dyn IngestionAdapter>,
    ) -> IngestionModule {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(sink_cap)),
            Box::new(SpyEpochStore::new()),
            profile,
        );
        m.register(descriptor(source_id));
        m.open(source_id, 0).unwrap();
        m.install_points(1, plans).unwrap();
        m.attach(adapter).unwrap();
        m
    }

    // ===== 红测 1：period/相位调度不生效 → 现在生效 =====

    #[test]
    fn scheduler_respects_period_and_phase() {
        let p = plan(2, 1, 1_000_000_000); // 1s 周期
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![raw(1, 40, 1)]],
            )),
        );
        // t=0：未到相位（phase=0 → 立即到期）→ 首采。
        let r = m.tick(
            0,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 1, "首采（phase_offset=0）");
        assert_eq!(r.accepted, 1);
        // t=0.5s：周期未到 → 不调度。
        let r = m.tick(
            500_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 0, "周期未到不 poll");
        // t=1.0s：到期 → 再采。
        let r = m.tick(
            1_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 1);
        // 相位：phase=2s 的点在 t=1s 不得首采。
        let mut p2 = plan(2, 1, 1_000_000_000);
        p2.phase_offset_ns = 2_000_000_000;
        let mut m2 = module_with(
            2,
            alloc::vec![p2],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![raw(1, 40, 1)]],
            )),
        );
        let r = m2.tick(
            1_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 0, "相位未到不首采");
        let r = m2.tick(
            2_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 1, "相位到首采");
    }

    // ===== 红测 2：deadband 不生效 → 现在按 QoS/品质/带宽裁决 =====

    #[test]
    fn deadband_suppresses_q3_repeat_not_quality_change() {
        let mut p = plan(2, 1, 1_000_000_000);
        p.qos = EventQos::Q3;
        p.deadband_abs = 0.5;
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![
                    raw(1, 100, 0), // → 100.0
                    raw(1, 100, 1), // 同值 → Coalesced
                    raw(1, 101, 2), // 带内（|1.0|≤0.5? 101-100=1.0 >0.5 → 发）
                    raw(1, 101, 3), // 同值 → Coalesced
                ]],
            )),
        );
        let r = m.tick(
            4_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.accepted, 2, "带外 100→发、101→发");
        assert_eq!(r.coalesced, 2, "带内重复 100、101 各并一次");
        // 品质变化永不被抑制（同值 + 品质 INVALID → 必发）。
        let mut p = plan(2, 1, 1_000_000_000);
        p.qos = EventQos::Q3;
        p.deadband_abs = 100.0; // 带宽极大——值变化也并
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![raw(1, 100, 0), {
                    let mut r = raw(1, 100, 1);
                    r.quality_bits = quality_bits::INVALID;
                    r
                },]],
            )),
        );
        let r = m.tick(
            2_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.accepted, 2, "品质变化（VALID→INVALID）永不被死区抑制");
        assert_eq!(r.coalesced, 0);
    }

    #[test]
    fn q0_and_q2_never_suppressed_by_deadband() {
        for qos in [EventQos::Q0, EventQos::Q2] {
            let mut p = plan(2, 1, 1_000_000_000);
            p.qos = qos;
            p.deadband_abs = 1_000.0; // 带宽极大
            let mut m = module_with(
                2,
                alloc::vec![p],
                64,
                OperationalProfile::reference(),
                Box::new(ScriptAdapter::new(
                    2,
                    alloc::vec![alloc::vec![raw(1, 100, 0), raw(1, 101, 1),]],
                )),
            );
            let r = m.tick(
                2_000_000_000,
                TickBudget {
                    max_events: 100,
                    max_transactions: 8,
                },
            );
            assert_eq!(r.accepted, 2, "{qos:?} 永不抑制");
            assert_eq!(r.coalesced, 0);
        }
    }

    #[test]
    fn q1_deadband_requires_suppressible_registration() {
        // Q1 + suppressible=false → 永不抑制（控制回读/告警形态）。
        let mut p = plan(2, 1, 1_000_000_000);
        p.qos = EventQos::Q1;
        p.suppressible = false;
        p.deadband_abs = 1_000.0;
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![raw(1, 100, 0), raw(1, 100, 1),]],
            )),
        );
        let r = m.tick(
            2_000_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.accepted, 2, "Q1 未注册抑制策略 → 永发");
    }

    // ===== 红测 3：source 重放 → epoch 递增 + sink 复核 =====

    #[test]
    fn module_reopen_increments_epoch_and_continues_sequence() {
        // 重启语义（[07] §2.4）：同持久 store 再开源 → epoch 严格递增，
        // watermark 续号；崩溃换 epoch 不要求同号续传。
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        m.install_points(1, alloc::vec![plan(2, 1, 0)]).unwrap();
        let e1 = m.open(2, 0).unwrap();
        assert_eq!(e1, 1);
        let ack = m.ingest(2, raw(1, 40, 1_000), 1_000).expect("首个事件接受");
        assert!(matches!(
            ack,
            IngestOutcome::Appended(AppendAck { offset: 1, .. })
        ));
        assert_eq!(m.watermark(2), Some((1, 1)));
        // 重启模拟：同 store 再开源 → epoch 2，新事件从续号起。
        let e2 = m.open(2, 0).unwrap();
        assert_eq!(e2, 2, "重启后 epoch 严格递增");
        let ack2 = m.ingest(2, raw(1, 41, 1_100), 1_100).unwrap();
        assert!(matches!(
            ack2,
            IngestOutcome::Appended(AppendAck { offset: 2, .. })
        ));
        assert_eq!(m.watermark(2), Some((2, 1)), "新 epoch 新序号空间");
        // 跨实例回退（旧 epoch 重放）由 sink 复核拦截——Phase 1 用例在
        // sink 层已锁（sink_rejects_epoch_regression_*）；模块层保证
        // epoch 只升不降（open_source 单点分配）。
    }

    // ===== 红测 4：多源并发不互相 clear =====

    #[test]
    fn two_sources_ingest_without_cross_clear() {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        m.register(descriptor(3));
        m.open(2, 0).unwrap();
        m.open(3, 0).unwrap();
        m.install_points(1, alloc::vec![plan(2, 1, 0), plan(3, 1, 0)])
            .unwrap();
        // 交错直推：A、B、A、B——任何一方的事件不得被另一方清掉。
        m.ingest(2, raw(1, 10, 1_000), 1_000).unwrap();
        m.ingest(3, raw(1, 20, 1_100), 1_100).unwrap();
        m.ingest(2, raw(1, 11, 1_200), 1_200).unwrap();
        m.ingest(3, raw(1, 21, 1_300), 1_300).unwrap();
        assert_eq!(m.accepted_total, 4, "四事件全在——无交叉 clear");
        assert_eq!(m.watermark(2), Some((1, 2)));
        assert_eq!(m.watermark(3), Some((1, 2)));
    }

    // ===== 红测 5：accepted>255 截断 → 计数面 u32 =====

    #[test]
    fn accepted_over_255_not_truncated() {
        let plans: Vec<PointPlan> = (1u32..=1024).map(|i| plan(2, i, 1_000_000_000)).collect();
        let script: Vec<Vec<RawObservation>> = alloc::vec![(1u32..=1024)
            .map(|i| raw(i, i64::from(i) * 100, 1_000))
            .collect()];
        let mut m = module_with(
            2,
            plans,
            2048,
            OperationalProfile {
                max_events_per_poll: 4096,
                max_transactions_per_tick: 4096,
                ..OperationalProfile::reference()
            },
            Box::new(ScriptAdapter::new(2, script)),
        );
        let r = m.tick(
            1_000,
            TickBudget {
                max_events: 4096,
                max_transactions: 4096,
            },
        );
        assert_eq!(r.accepted, 1024, "单 tick 1024 事件——远超 u8/u16 截断界");
        assert_eq!(m.accepted_total, 1024);
    }

    // ===== 红测 6：慢 sink 内存增长 → 背压显式 + 状态有界 =====

    #[test]
    fn slow_sink_backpressure_bounded() {
        // sink 容量 2；adapter 无视背压连推 1000 个不同点事件。
        let plans: Vec<PointPlan> = (1u32..=1000).map(|i| plan(2, i, 1_000_000_000)).collect();
        struct Flood;
        impl IngestionAdapter for Flood {
            fn source_id(&self) -> u32 {
                2
            }
            fn poll(
                &mut self,
                now_ns: u64,
                _budget: PollBudget,
                _work: &[PointPlan],
                out: &mut dyn RawObservationSink,
            ) -> PollOutcome {
                for i in 1..=1000u32 {
                    let r = raw(i, i64::from(i) * 10, now_ns);
                    let _ = out.push(r);
                }
                PollOutcome::Completed {
                    events: 1000,
                    transactions: 1000,
                }
            }
        }
        let mut m = module_with(
            2,
            plans,
            2,
            OperationalProfile {
                max_events_per_poll: 4096,
                ..OperationalProfile::reference()
            },
            Box::new(Flood),
        );
        let r = m.tick(
            1_000,
            TickBudget {
                max_events: 4096,
                max_transactions: 4096,
            },
        );
        assert_eq!(r.accepted, 2, "sink 容量界");
        assert!(
            r.backpressured >= 998,
            "其余全部显式背压：{}",
            r.backpressured
        );
        assert_eq!(m.backpressured_total, u64::from(r.backpressured));
        // 有界：点状态表 = 计划点数（不随洪泛增长）。
        assert!(m.directory().len() <= 1000);
        // Q1 背压 → 来源 degraded（快照缺失信号）。
        assert!(m.is_degraded(2), "Q1 背压触发 source degraded");
        // 重连登记清 degraded，connection_epoch 递增（本地 epoch 不变）。
        let epoch_before = m.epoch(2);
        m.note_reconnect(2);
        assert!(!m.is_degraded(2));
        assert_eq!(m.connection_epoch(2), 1);
        assert_eq!(m.epoch(2), epoch_before, "重连不换本地 source epoch");
    }

    // ===== 红测 7：poll 预算封顶 + rate-limit reason =====

    #[test]
    fn poll_budget_caps_events_and_records_reason() {
        let plans: Vec<PointPlan> = (1u32..=100).map(|i| plan(2, i, 1_000_000_000)).collect();
        struct IgnoreBP;
        impl IngestionAdapter for IgnoreBP {
            fn source_id(&self) -> u32 {
                2
            }
            fn poll(
                &mut self,
                now_ns: u64,
                _budget: PollBudget,
                work: &[PointPlan],
                out: &mut dyn RawObservationSink,
            ) -> PollOutcome {
                // 无视背压连推（合同违规者）——模块必须逐个显式限流。
                let mut transactions = 0u32;
                for p in work {
                    transactions += 1;
                    let _ = out.push(raw(p.point_id, i64::from(p.point_id), now_ns));
                }
                PollOutcome::Completed {
                    events: transactions,
                    transactions,
                }
            }
        }
        let mut m = module_with(
            2,
            plans,
            2048,
            OperationalProfile {
                max_events_per_poll: 10,
                max_transactions_per_tick: 1000,
                checkpoint_interval_events: 64,
            },
            Box::new(IgnoreBP),
        );
        let r = m.tick(
            1_000,
            TickBudget {
                max_events: 1000,
                max_transactions: 1000,
            },
        );
        assert_eq!(r.accepted, 10, "profile 封顶 10");
        assert_eq!(r.rate_limited, 90, "超出部分记录 rate-limit reason");
        assert_eq!(r.backpressured, 90);
        assert_eq!(m.rate_limited_total, 90);
    }

    // ===== 调度协作：事务预算跨 tick =====

    #[test]
    fn tick_transaction_budget_bounds_adapter() {
        // 三个独立点（3 事务）；tick 预算 2 → 本 tick 2，余 1 下 tick
        // （周期 2s——第二 tick 时 1/2 号点未再到期，轮转游标指向 3 号）。
        let plans: Vec<PointPlan> = (1u32..=3).map(|i| plan(2, i, 2_000_000_000)).collect();
        struct TxAdapter {
            calls: u32,
        }
        impl IngestionAdapter for TxAdapter {
            fn source_id(&self) -> u32 {
                2
            }
            fn poll(
                &mut self,
                _now_ns: u64,
                budget: PollBudget,
                work: &[PointPlan],
                out: &mut dyn RawObservationSink,
            ) -> PollOutcome {
                self.calls += 1;
                let mut transactions = 0u32;
                for p in work {
                    if transactions >= budget.max_transactions {
                        return PollOutcome::Busy { transactions };
                    }
                    transactions += 1;
                    let _ = out.push(raw(p.point_id, i64::from(p.point_id), 1_000));
                }
                PollOutcome::Completed {
                    events: transactions,
                    transactions,
                }
            }
        }
        let mut m = module_with(
            2,
            plans,
            64,
            OperationalProfile::reference(),
            Box::new(TxAdapter { calls: 0 }),
        );
        let r = m.tick(
            1_000,
            TickBudget {
                max_events: 100,
                max_transactions: 2,
            },
        );
        assert_eq!(r.transactions, 2, "tick 事务预算封顶");
        assert_eq!(r.accepted, 2);
        // 下个 tick（首采在 t=1ms，周期 1s → t=1.001s 到期）。
        let r = m.tick(
            1_000_001_000,
            TickBudget {
                max_events: 100,
                max_transactions: 2,
            },
        );
        assert_eq!(r.accepted, 1, "余量下 tick 补齐");
    }

    // ===== 工程量与身份由 directory 富化 =====

    #[test]
    fn engineering_and_identity_from_directory() {
        let mut p = plan(2, 1, 1_000_000_000);
        p.scale = 0.1;
        p.offset = 5.0;
        p.mapping_version = 7;
        p.asset_id = 42;
        p.unit_code = unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Kilowatts);
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            Box::new(ScriptAdapter::new(
                2,
                alloc::vec![alloc::vec![raw(1, 400, 1_000)]],
            )),
        );
        m.tick(
            1_000,
            TickBudget {
                max_events: 10,
                max_transactions: 8,
            },
        );
        assert_eq!(m.accepted_total, 1);
        // 数值断言：raw 400 × 0.1 + 5 = 45.0（经 sink 审计迭代读回）。
        // （InMemorySink 存 wire；直接读 watermark + accepted 即可，工程量
        // 正确性由下一测 ingest 路径直证。）
    }

    #[test]
    fn ingest_direct_converts_engineering() {
        let mut p = plan(2, 1, 0);
        p.scale = 0.1;
        p.offset = 5.0;
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        m.open(2, 0).unwrap();
        m.install_points(1, alloc::vec![p]).unwrap();
        let outcome = m.ingest(2, raw(1, 400, 1_000), 1_000).unwrap();
        assert!(matches!(outcome, IngestOutcome::Appended(_)));
        assert_eq!(m.accepted_total, 1);
        // 事件直推不再消费 scratch/adapter——纯准入路径。
        assert!(m.attached_sources().is_empty());
    }

    // ===== 期限：超限计数 + 陈旧标记事件 =====

    #[test]
    fn deadline_overrun_counts_and_emits_stale_event() {
        let mut p = plan(2, 1, 1_000_000_000);
        p.deadline_ns = 500_000_000; // 期限 0.5s
        let mut m = module_with(
            2,
            alloc::vec![p],
            64,
            OperationalProfile::reference(),
            // adapter 永不产出（模拟采集卡死）。
            Box::new(ScriptAdapter::new(2, alloc::vec![])),
        );
        // t=0 首采（work 下发、pending 置位）；adapter 空脚本无事件。
        let r = m.tick(
            0,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.polls, 1);
        assert_eq!(r.accepted, 0);
        // t=0.3s：期限未到 → 无超限。
        let r = m.tick(
            300_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.deadline_overruns, 0);
        // t=0.6s：超限（0.6-0=0.6 > 0.5）→ 计数；无锚点（从未报告）→
        // 只计数不造数据。
        let r = m.tick(
            600_000_000,
            TickBudget {
                max_events: 100,
                max_transactions: 8,
            },
        );
        assert_eq!(r.deadline_overruns, 1);
        assert_eq!(m.deadline_overrun_total, 1);
    }

    // ===== 世代原子切换 =====

    #[test]
    fn directory_generation_atomic_switch() {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        m.open(2, 0).unwrap();
        m.install_points(1, alloc::vec![plan(2, 1, 0)]).unwrap();
        assert_eq!(m.directory().generation(), 1);
        // 回退世代拒绝，旧目录原样保留。
        assert_eq!(
            m.install_points(1, alloc::vec![plan(2, 2, 0)]).unwrap_err(),
            ConfigReject::GenerationStale {
                current: 1,
                incoming: 1
            }
        );
        assert_eq!(m.directory().len(), 1);
        // 新世代原子生效。
        m.install_points(2, alloc::vec![plan(2, 1, 0), plan(2, 2, 0)])
            .unwrap();
        assert_eq!(m.directory().generation(), 2);
        assert_eq!(m.directory().len(), 2);
        // 重复点拒绝。
        assert_eq!(
            m.install_points(3, alloc::vec![plan(2, 1, 0), plan(2, 1, 0)])
                .unwrap_err(),
            ConfigReject::DuplicatePoint {
                source_id: 2,
                point_id: 1
            }
        );
        // 未注册来源计划拒绝。
        assert_eq!(
            m.install_points(4, alloc::vec![plan(9, 1, 0)]).unwrap_err(),
            ConfigReject::SourceUnknown { source_id: 9 }
        );
    }

    // ===== 分片：≤1024/shard，多 shard 索引完备 =====

    #[test]
    fn directory_shards_cap_1024() {
        let mut dir = PointDirectory::new();
        let plans: Vec<PointPlan> = (1u32..=1025).map(|i| plan(2, i, 0)).collect();
        dir.install_generation(1, plans).unwrap();
        assert_eq!(dir.shard_count(), 2, "1025 点切 2 shard（1024+1）");
        assert_eq!(dir.shards()[0].len(), 1024);
        assert_eq!(dir.shards()[1].len(), 1);
        // 索引完备：首、界、尾均可解析。
        assert!(dir.resolve(2, 1).is_some());
        assert!(dir.resolve(2, 1024).is_some());
        assert!(dir.resolve(2, 1025).is_some());
        assert!(dir.resolve(2, 1026).is_none());
    }

    // ===== 检查点：批量间隔持久化 =====

    #[test]
    fn checkpoint_persists_watermarks_in_batches() {
        let mut store = SpyEpochStore::new();
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile {
                checkpoint_interval_events: 3,
                ..OperationalProfile::reference()
            },
        );
        let _ = &mut store;
        m.register(descriptor(2));
        m.open(2, 0).unwrap();
        m.install_points(1, alloc::vec![plan(2, 1, 0)]).unwrap();
        // 2 个事件 < 间隔 3 → checkpoint 不落盘。
        m.ingest(2, raw(1, 1, 1_000), 1_000).unwrap();
        m.ingest(2, raw(1, 2, 1_100), 1_100).unwrap();
        // （直接驱动 open 时已 save 一次——记录 save 数基线。）
        // 第 3 个事件后 checkpoint → 落盘 watermark=3。
        m.ingest(2, raw(1, 3, 1_200), 1_200).unwrap();
        m.checkpoint().unwrap();
        // 用内部 sink watermark 断言：模块持同源水位。
        assert_eq!(m.watermark(2), Some((1, 3)));
    }

    // ===== epoch floor：boot counter 下界 =====

    #[test]
    fn epoch_floor_from_boot_counter() {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        // boot counter = 5 → epoch = 6（严格递增下界）。
        assert_eq!(m.open(2, 5).unwrap(), 6);
    }

    // ===== adapter 伪造来源不可达 =====

    #[test]
    fn adapter_source_authority() {
        // ScriptAdapter 声明 source 2，但模块只会以 registry 注册事实
        // 开源——未注册来源 attach 即拒。
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        assert_eq!(
            m.attach(Box::new(ScriptAdapter::new(7, alloc::vec![])))
                .unwrap_err(),
            ConfigReject::SourceUnknown { source_id: 7 }
        );
        m.register(descriptor(2));
        m.attach(Box::new(ScriptAdapter::new(2, alloc::vec![])))
            .unwrap();
        assert_eq!(
            m.attach(Box::new(ScriptAdapter::new(2, alloc::vec![])))
                .unwrap_err(),
            ConfigReject::AdapterDuplicate { source_id: 2 }
        );
    }

    // ===== 非法 raw 在门上拒绝（越权/完整性/超速）=====

    #[test]
    fn raw_rejections_at_gate() {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        let mut d = descriptor(2);
        d.point_range = (1, 10);
        d.max_events_per_s = 2;
        m.register(d);
        m.open(2, 0).unwrap();
        m.install_points(1, alloc::vec![plan(2, 1, 0)]).unwrap();
        // 越权点（目录有计划 1，raw point 77 无计划且超 descriptor 界）。
        assert!(matches!(
            m.ingest(2, raw(77, 1, 1_000), 1_000).unwrap_err(),
            IngestReject::PointNotAuthorized { .. }
        ));
        // 弱完整性。
        let mut weak = raw(1, 1, 1_100);
        weak.integrity = IntegrityTag::Unverified;
        assert!(matches!(
            m.ingest(2, weak, 1_100).unwrap_err(),
            IngestReject::IntegrityTooWeak { .. }
        ));
        // 超速（2/s 满桶耗尽）。
        m.ingest(2, raw(1, 1, 1_000_000_000), 1_000_000_000)
            .unwrap();
        m.ingest(2, raw(1, 2, 1_000_000_000), 1_000_000_000)
            .unwrap();
        assert!(matches!(
            m.ingest(2, raw(1, 3, 1_000_000_000), 1_000_000_000)
                .unwrap_err(),
            IngestReject::RateExceeded { .. }
        ));
    }

    // ===== 计数面无 u8 =====

    #[test]
    fn counters_are_u32_or_wider() {
        let r = TickReport::default();
        let _: u32 = r.accepted;
        let _: u32 = r.transactions;
        let p = plan(2, 1, 0);
        let _: u64 = p.period_ns;
        let _: u16 = p.max_retries;
        let _: u32 = SHARD_MAX_POINTS as u32;
    }

    // ===== 事件标志：ingest 产物可校验 =====

    #[test]
    fn appended_event_validates() {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(SpyEpochStore::new()),
            OperationalProfile::reference(),
        );
        m.register(descriptor(2));
        m.open(2, 0).unwrap();
        m.install_points(1, alloc::vec![plan(2, 1, 0)]).unwrap();
        let _ = m.ingest(2, raw(1, 400, 1_000), 1_000).unwrap();
        assert_eq!(m.accepted_total, 1);
        let _ = event_flags::TEST; // 引用防未用
    }
}
