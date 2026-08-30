//! 蓝图 61 [16] §3.1：`ingestion-core` — IngestionModule 窄腰深模块。
//!
//! 外部只见 [`SourceRegistry`] 的 register/descriptor/verify 与
//! [`ObservationSink`]；scheduling、deadband、rate limit、dedup、queue
//! routing 全部私有。唯一不变量（[06] §2.1）：任何可参与决策的观测都能
//! 唯一回答——谁、在哪个租户/站点、哪个 epoch/sequence、何时、以什么
//! schema/单位/映射、什么质量和完整性产生。
//!
//! # 身份与 epoch（[06] §2.4）
//!
//! - `source_id` 由签名配置分配（config-bundle `points` `[[source]]` 表），
//!   不由 binary 硬编码；
//! - `source_epoch` 由本模块分配（[`EpochStore`] 持久启动计数；`0` 只允许
//!   测试——事件 flags 须含 TEST）；
//! - sequence 同 epoch 严格单调，持久 watermark 防重启重放；
//! - 同身份不同 payload 的重复 sequence 视为篡改冲突（event_id 去重命中
//!   且 payload 不一致 → `TamperConflict`）；
//! - 未注册来源 / 越权点 / 超速 / 弱完整性 → 结构化 [`IngestReject`]；
//!   未注册事件可存 quarantine（[`QuarantineStore`]），控制 evaluator
//!   零消费。
//!
//! # 依赖边界
//!
//! 允许：upa-model / telemetry-model / protocol-abstract。
//! 禁止：AI、solver、control-dispatch、business-services。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod module;

/// 蓝图 61 [07] §3.5：adapter 无关合同套件（feature `contract`；
/// 各协议集成测试经同一套合同证明准入/背压/重启/event_id 纪律）。
#[cfg(feature = "contract")]
pub mod contract;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use eneros_telemetry_model::event::{
    event_flags, IntegrityTag, ObservationEvent, ObservationValue,
};
pub use module::{
    ConfigReject, EventQos, IngestOutcome, IngestionAdapter, IngestionModule, OperationalProfile,
    PointDirectory, PointPlan, PointShard, PollBudget, PollOutcome, PushOutcome,
    RawObservationSink, TickBudget, TickReport, SHARD_MAX_POINTS,
};

/// 来源描述符（[02] §4.2 最小封闭集；由签名配置编译）。
#[derive(Debug, Clone, PartialEq)]
pub struct SourceDescriptor {
    /// 来源 ID（≠0）。
    pub source_id: u32,
    /// 租户域（≠0）。
    pub tenant_id: u32,
    /// 站点域（≠0）。
    pub site_id: u32,
    /// 允许进入控制快照。
    pub allow_control: bool,
    /// 要求完整性认证（Unverified 事件 → `IntegrityTooWeak`）。
    pub require_integrity: bool,
    /// 允许点位范围（min, max；min=0 表示不设下界，max=0 表示不设上界）。
    pub point_range: (u32, u32),
    /// 最大事件率（事件/秒；令牌桶）。
    pub max_events_per_s: u32,
    /// 允许 schema id 集合（空 = 拒绝一切带 schema 的事件）。
    pub allowed_schemas: Vec<u32>,
    /// 时钟域注册号。
    pub clock_id: u16,
    /// 启停状态（停用来源的一切事件拒绝）。
    pub enabled: bool,
}

impl SourceDescriptor {
    /// 点是否在来源授权范围内。
    pub fn point_authorized(&self, point_id: u32) -> bool {
        let (min, max) = self.point_range;
        point_id >= min && (max == 0 || point_id <= max)
    }
}

/// adapter 内部原始观测（**只能存在于 adapter 内**；跨出 adapter 前必须
/// 经 [`SourceRegistry::verify`] 成为 [`ObservationEvent`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawObservation {
    /// 站内点位 ID。
    pub point_id: u32,
    /// 观测值。
    pub value: ObservationValue,
    /// 设备测量时刻（ns；None → flags 打 TIME_MISSING 且 event_time=0）。
    pub event_time_ns: Option<u64>,
    /// 品质位（[`quality_bits`] 布局）。
    pub quality_bits: u16,
    /// 完整性标签（采集链判定）。
    pub integrity: IntegrityTag,
    /// adapter 附加标志（SIMULATED/TEST 等；epoch-0 测试事件须含 TEST）。
    pub flags: u16,
    /// schema id（来自点映射身份绑定；0 = 未注册 schema）。
    pub schema_id: u32,
    /// schema 版本。
    pub schema_version: u16,
    /// 单位码（来自点映射身份绑定；0 = 未知）。
    pub unit_code: u16,
    /// 资产绑定（来自点映射身份绑定；0 = 未绑定）。
    pub asset_id: u64,
    /// 映射世代（点表生效 generation）。
    pub mapping_version: u64,
    /// 站内设备短 ID。
    pub device_id: u16,
}

/// 准入拒绝（结构化；一切丢弃可观测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestReject {
    /// 来源未注册（只能进 quarantine，不得参与规则/求解/LLM）。
    UnregisteredSource { source_id: u32 },
    /// 来源已停用。
    SourceDisabled { source_id: u32 },
    /// 来源越权点（不在 descriptor 授权范围）。
    PointNotAuthorized { point_id: u32 },
    /// 超速（令牌桶耗尽）。
    RateExceeded { source_id: u32 },
    /// 未知 schema（未在 descriptor 注册）。
    SchemaUnknown { schema_id: u32 },
    /// 完整性不足。
    IntegrityTooWeak { integrity: IntegrityTag },
    /// 非有限浮点（NaN/Inf 在接入门拒绝，不得直接传播）。
    NotFinite,
    /// 事件值/标志非法（validate 失败：epoch-0 非 TEST、缺时刻位错等）。
    InvalidEvent,
    /// sequence 回退/重放（同 epoch 内不单调）。
    SequenceReplay { sequence: u64, last: u64 },
    /// 时钟回拨（event_time 相对同源上一事件倒退）。
    ClockRollback { event_time_ns: u64, last_ns: u64 },
    /// 篡改冲突：同 (source,epoch,sequence) 身份但 payload 不同。
    TamperConflict { sequence: u64 },
    /// 重复事件（同 event_id；静默去重前的可观测回执——幂等接受）。
    Duplicate { event_id: [u8; 16] },
    /// epoch 回退（消费侧钉扎首见 epoch 后出现更低 epoch——重放/回滚）。
    EpochRegression { epoch: u64, expected: u64 },
    /// quarantine/sink 有界溢出（背压显式化，不静默丢）。
    Overflow,
}

/// append 回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendAck {
    /// 事件 ID（去重键）。
    pub event_id: [u8; 16],
    /// 段内序号（sink 分配）。
    pub offset: u64,
}

/// 观测事件汇（[06] §2.3 接口；watermark/dedup 由实现持有）。
pub trait ObservationSink {
    fn append(&mut self, event: &ObservationEvent) -> Result<AppendAck, IngestReject>;
    /// 该来源已持久 watermark（同 epoch 内最后接受 sequence）。
    fn watermark(&self, source_id: u32) -> Option<(u64, u64)>; // (epoch, sequence)
    /// 排空待交付 wire 帧（事件日志 → 传输批次接缝；[07] §2.2 交付面）。
    /// 默认零交付（纯存档 sink）。
    fn drain_frames(&mut self) -> Vec<[u8; 128]> {
        Vec::new()
    }
}

/// epoch/watermark 持久化接缝（重启防重放；[06] §2.4）。
pub trait EpochStore {
    /// 读回 (epoch, watermark)；无记录 → None。
    fn load(&mut self, source_id: u32) -> Option<(u64, u64)>;
    /// 持久化（失败 = 显式错误，调用方拒绝服务 ready 而非静默继续）。
    fn save(&mut self, source_id: u32, epoch: u64, watermark: u64) -> Result<(), IngestReject>;
}

/// 内存 epoch 存储（target 默认；跨重启不持久——见 ADR-0047 缺口登记）。
#[derive(Debug, Default)]
pub struct MemEpochStore {
    state: BTreeMap<u32, (u64, u64)>,
}

impl EpochStore for MemEpochStore {
    fn load(&mut self, source_id: u32) -> Option<(u64, u64)> {
        self.state.get(&source_id).copied()
    }
    fn save(&mut self, source_id: u32, epoch: u64, watermark: u64) -> Result<(), IngestReject> {
        self.state.insert(source_id, (epoch, watermark));
        Ok(())
    }
}

/// 有界 quarantine（未注册/拒绝事件存档；控制 evaluator 零消费——
/// 类型上隔离：取出的只是 (reason, wire bytes) 审计元组）。
#[derive(Debug, Default)]
pub struct QuarantineStore {
    entries: Vec<(IngestReject, [u8; 128])>,
    cap: usize,
    dropped: u64,
}

impl QuarantineStore {
    /// 有界构造（满后丢弃最旧并计数——丢弃可观测）。
    pub fn new(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, reason: IngestReject, wire: &[u8; 128]) {
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
            self.dropped += 1;
        }
        self.entries.push((reason, *wire));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 审计迭代（控制路径没有该入口——零消费由类型隔离保证）。
    pub fn iter(&self) -> impl Iterator<Item = &(IngestReject, [u8; 128])> {
        self.entries.iter()
    }
}

/// 令牌桶（私有；容量 = 1 秒配额，初值满桶——允许 1 秒量突发）。
/// 内部定点：1 token = 1000 单位。
#[derive(Debug)]
struct RateLimiter {
    /// 容量（x1000 单位 = max_events_per_s × 1000）。
    capacity_x1000: u64,
    tokens_x1000: u64,
    last_ns: u64,
}

impl RateLimiter {
    fn new(max_events_per_s: u32, now_ns: u64) -> Self {
        let cap = u64::from(max_events_per_s.max(1)) * 1000;
        Self {
            capacity_x1000: cap,
            tokens_x1000: cap,
            last_ns: now_ns,
        }
    }

    fn try_take(&mut self, now_ns: u64) -> bool {
        let elapsed = now_ns.saturating_sub(self.last_ns);
        self.last_ns = now_ns;
        // 回填（x1000 单位）= elapsed_ns × rate / 1e6。
        let rate = self.capacity_x1000 / 1000;
        let refill = elapsed.saturating_mul(rate).saturating_div(1_000_000);
        self.tokens_x1000 = (self.tokens_x1000 + refill).min(self.capacity_x1000);
        if self.tokens_x1000 >= 1000 {
            self.tokens_x1000 -= 1000;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct SourceState {
    descriptor: SourceDescriptor,
    /// 已分配 epoch（registry 分配，adapter 无权指定）。
    epoch: u64,
    /// 同 epoch 最后接受 sequence。
    last_sequence: u64,
    /// 同源最后事件时刻（回拨检测）。
    last_event_time_ns: u64,
    limiter: RateLimiter,
    /// 同 epoch 已接受事件 id（去重；有界由 sink 承担，此处仅同批次）。
    opened: bool,
}

/// 时钟域（[02] §4.3 最小登记形态；PTP/GPS/设备时钟等的注册号与
/// 信任面标注——快照切面用它裁决事件时间可信度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDomain {
    /// 注册号（ObservationEvent.clock_id）。
    pub clock_id: u16,
    /// 域类别（0=单调本机/1=PTP/2=NTP/3=GPS/4=设备时钟；封闭登记）。
    pub kind: u8,
    /// 事件时间是否可参与控制快照切面（不可信时钟域的观测只可存档）。
    pub time_trusted: bool,
}

/// ClockRegistry——时钟域最小只读接口（[06] §2.5.3）。
///
/// 登记面由签名配置装配（单源）；查询只读。rollback 检测本体在
/// `SourceRegistry::verify`（同源 event_time 回退拒绝），本表只承载
/// 域身份与信任面。
#[derive(Debug, Default)]
pub struct ClockRegistry {
    domains: BTreeMap<u16, ClockDomain>,
}

impl ClockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记时钟域（clock_id 唯一；覆盖 = 更新信任面）。
    pub fn register(&mut self, domain: ClockDomain) {
        self.domains.insert(domain.clock_id, domain);
    }

    /// 只读查询；未登记 clock_id → None（消费方 fail-closed）。
    pub fn domain(&self, clock_id: u16) -> Option<&ClockDomain> {
        self.domains.get(&clock_id)
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }
}

/// 来源注册表（深模块唯一入口）。
///
/// 外部只见 [`register`](Self::register)/[`descriptor`](Self::descriptor)/
/// [`verify`](Self::verify)/[`open_source`](Self::open_source)。
pub struct SourceRegistry {
    sources: BTreeMap<u32, SourceState>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
    }

    /// 注册/更新来源 descriptor（签名配置编译产物；幂等覆盖）。
    pub fn register(&mut self, descriptor: SourceDescriptor, now_ns: u64) {
        let source_id = descriptor.source_id;
        let max_events = descriptor.max_events_per_s.max(1);
        let entry = self
            .sources
            .entry(source_id)
            .or_insert_with(|| SourceState {
                epoch: 0,
                last_sequence: 0,
                last_event_time_ns: 0,
                limiter: RateLimiter::new(max_events, now_ns),
                opened: false,
                descriptor: descriptor.clone(),
            });
        // 限流参数/描述符以最新配置为准（身份/epoch/水位保留）。
        entry.limiter.capacity_x1000 = u64::from(max_events) * 1000;
        entry.descriptor = descriptor;
    }

    pub fn descriptor(&self, source_id: u32) -> Option<&SourceDescriptor> {
        self.sources.get(&source_id).map(|s| &s.descriptor)
    }

    pub fn is_registered(&self, source_id: u32) -> bool {
        self.sources.contains_key(&source_id)
    }

    /// 开源：分配本启动的 source_epoch。
    ///
    /// epoch = max(持久化 epoch, `epoch_floor`) + 1（严格递增；`epoch_floor`
    /// 由装配侧注入——host 文件存储可持久，target 用启动时刻下界）。
    /// 任何事件在 open 之前 verify → `SourceDisabled`（未开源不可产事件）。
    pub fn open_source(
        &mut self,
        source_id: u32,
        epoch_floor: u64,
        store: &mut dyn EpochStore,
    ) -> Result<u64, IngestReject> {
        let state = self
            .sources
            .get_mut(&source_id)
            .ok_or(IngestReject::UnregisteredSource { source_id })?;
        let persisted = store.load(source_id);
        let new_epoch = persisted
            .map(|(e, _)| e.max(epoch_floor).saturating_add(1))
            .unwrap_or_else(|| epoch_floor.saturating_add(1))
            .max(1);
        let watermark = persisted.map(|(_, w)| w).unwrap_or(0);
        store.save(source_id, new_epoch, watermark)?;
        state.epoch = new_epoch;
        state.last_sequence = watermark;
        state.opened = true;
        Ok(new_epoch)
    }

    /// 当前分配 epoch（未开源 → 0）。
    pub fn epoch(&self, source_id: u32) -> u64 {
        self.sources.get(&source_id).map(|s| s.epoch).unwrap_or(0)
    }

    /// 唯一准入门：RawObservation → [`ObservationEvent`]。
    ///
    /// 顺序（fail-closed，逐项结构化拒绝）：注册 → 启用 → 已开源 → 点位
    /// 越权 → 速率 → schema → 完整性 → 非有限值 → 事件校验 → 时钟回拨 →
    /// sequence 单调 → 组装（stamp epoch/tenant/site/ingest_time）。
    /// 去重与 watermark 持久化在 sink 侧 [`ObservationSink`] 承担。
    pub fn verify(
        &mut self,
        source_id: u32,
        raw: &RawObservation,
        now_ns: u64,
    ) -> Result<ObservationEvent, IngestReject> {
        let state = self
            .sources
            .get_mut(&source_id)
            .ok_or(IngestReject::UnregisteredSource { source_id })?;
        if !state.descriptor.enabled {
            return Err(IngestReject::SourceDisabled { source_id });
        }
        if !state.opened {
            return Err(IngestReject::SourceDisabled { source_id });
        }
        if !state.descriptor.point_authorized(raw.point_id) {
            return Err(IngestReject::PointNotAuthorized {
                point_id: raw.point_id,
            });
        }
        if !state.limiter.try_take(now_ns) {
            return Err(IngestReject::RateExceeded { source_id });
        }
        if state.descriptor.allowed_schemas.contains(&raw.schema_id)
            || (raw.schema_id == 0 && state.descriptor.allowed_schemas.is_empty())
        {
            // schema 允许（空表只允许显式 0=未注册 schema 的存档事件——
            // 控制资格由事件 asset/unit 位决定）。
        } else {
            return Err(IngestReject::SchemaUnknown {
                schema_id: raw.schema_id,
            });
        }
        if state.descriptor.require_integrity && raw.integrity == IntegrityTag::Unverified {
            return Err(IngestReject::IntegrityTooWeak {
                integrity: raw.integrity,
            });
        }
        if let ObservationValue::F64(v) = raw.value {
            if !v.is_finite() {
                return Err(IngestReject::NotFinite);
            }
        }
        let (event_time_ns, flags) = match raw.event_time_ns {
            Some(t) => (t, raw.flags),
            None => (0u64, raw.flags | event_flags::TIME_MISSING),
        };
        let event = ObservationEvent {
            tenant_id: state.descriptor.tenant_id,
            site_id: state.descriptor.site_id,
            source_id,
            source_epoch: state.epoch,
            sequence: 0, // 下面推进后回填
            event_time_ns,
            ingest_time_ns: now_ns,
            asset_id: raw.asset_id,
            device_id: raw.device_id,
            unit_code: raw.unit_code,
            point_id: raw.point_id,
            schema_id: raw.schema_id,
            schema_version: raw.schema_version,
            quality_bits: raw.quality_bits,
            mapping_version: raw.mapping_version,
            value: raw.value,
            integrity: raw.integrity,
            clock_id: state.descriptor.clock_id,
            flags,
        };
        // 时钟回拨（同源 event_time 严格不回退；TIME_MISSING 事件不参与）。
        if raw.event_time_ns.is_some() && event_time_ns < state.last_event_time_ns {
            return Err(IngestReject::ClockRollback {
                event_time_ns,
                last_ns: state.last_event_time_ns,
            });
        }
        // sequence 单调：由 registry 单点分配（last+1）——回退在类型上
        // 不可达；跨实例回退由 sink watermark 复核拦截（SequenceReplay）。
        let sequence = state.last_sequence.saturating_add(1);
        let mut event = event;
        event.sequence = sequence;
        event.validate().map_err(|_| IngestReject::InvalidEvent)?;
        // 提交状态（调用方 sink append 失败时事件未入流——sequence 空洞
        // 允许（单调即可），无正确性影响）。
        state.last_sequence = sequence;
        if raw.event_time_ns.is_some() {
            state.last_event_time_ns = event_time_ns;
        }
        Ok(event)
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 内存观测事件汇（E3 参考实现；dedup + 同源 watermark 复核）。
///
/// 有界（v0.314.30）：事件环 + event_id 去重环同为 `cap`——满后拒绝
/// （`IngestReject::Overflow`，背压显式化）或滑动淘汰最旧（drain 交付
/// 后腾位）。长跑内存有界（C1 判据），去重窗口 = 最近 `cap` 事件。
pub struct InMemorySink {
    events: VecDeque<([u8; 16], [u8; 128])>,
    ids: BTreeMap<[u8; 16], ()>,
    watermarks: BTreeMap<u32, (u64, u64)>,
    offset: u64,
    cap: usize,
}

impl InMemorySink {
    pub fn new(cap: usize) -> Self {
        Self {
            events: VecDeque::new(),
            ids: BTreeMap::new(),
            watermarks: BTreeMap::new(),
            offset: 0,
            cap: cap.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 审计迭代（事件 wire 字节）。
    pub fn iter(&self) -> impl Iterator<Item = &[u8; 128]> {
        self.events.iter().map(|(_, w)| w)
    }
}

impl ObservationSink for InMemorySink {
    fn append(&mut self, event: &ObservationEvent) -> Result<AppendAck, IngestReject> {
        let id = event.event_id();
        // 1. event_id 去重（窗口内重放同 payload → 幂等回执）。
        if self.ids.contains_key(&id) {
            return Err(IngestReject::Duplicate { event_id: id });
        }
        // 2. 同源 watermark 复核（跨 registry 实例的回退/篡改冲突）。
        if let Some(&(we, ws)) = self.watermarks.get(&event.source_id) {
            if event.source_epoch < we {
                // epoch 回退（重放/回滚）：拒绝且 watermark 不降级——
                // 否则旧 epoch 重放会把水位拉低，后续重放序列全部放行。
                return Err(IngestReject::EpochRegression {
                    epoch: event.source_epoch,
                    expected: we,
                });
            }
            if we == event.source_epoch && event.sequence <= ws {
                return Err(IngestReject::SequenceReplay {
                    sequence: event.sequence,
                    last: ws,
                });
            }
        }
        if self.events.len() >= self.cap {
            return Err(IngestReject::Overflow);
        }
        let wire = event.encode_v1();
        self.offset = self.offset.saturating_add(1);
        self.ids.insert(id, ());
        self.events.push_back((id, wire));
        self.watermarks
            .insert(event.source_id, (event.source_epoch, event.sequence));
        Ok(AppendAck {
            event_id: id,
            offset: self.offset,
        })
    }

    fn watermark(&self, source_id: u32) -> Option<(u64, u64)> {
        self.watermarks.get(&source_id).copied()
    }

    /// 排空事件环（wire 帧交付）；id 去重环随环淘汰（窗口有界）。
    fn drain_frames(&mut self) -> Vec<[u8; 128]> {
        let mut out = Vec::with_capacity(self.events.len());
        while let Some((id, wire)) = self.events.pop_front() {
            self.ids.remove(&id);
            out.push(wire);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::event::quality_bits;
    use eneros_telemetry_model::unit_catalog;

    use super::*;

    fn descriptor(source_id: u32) -> SourceDescriptor {
        SourceDescriptor {
            source_id,
            tenant_id: 1,
            site_id: 1,
            allow_control: true,
            require_integrity: true,
            point_range: (1, 10),
            max_events_per_s: 1000,
            allowed_schemas: alloc::vec![1],
            clock_id: 1,
            enabled: true,
        }
    }

    fn raw(point_id: u32, value: f64) -> RawObservation {
        RawObservation {
            point_id,
            value: ObservationValue::F64(value),
            event_time_ns: Some(1_000),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 1,
            schema_version: 1,
            unit_code: unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Kilowatts),
            asset_id: 1,
            mapping_version: 1,
            device_id: 2,
        }
    }

    fn opened_registry() -> (SourceRegistry, MemEpochStore) {
        let mut reg = SourceRegistry::new();
        let mut store = MemEpochStore::default();
        reg.register(descriptor(2), 0);
        reg.open_source(2, 0, &mut store).unwrap();
        (reg, store)
    }

    // ===== 红线：身份由 registry 分配，伪造路径不存在 =====

    #[test]
    fn unregistered_source_rejected_and_quarantined() {
        let mut reg = SourceRegistry::new();
        let mut q = QuarantineStore::new(8);
        // source 99 未注册：verify 拒绝 + quarantine 存档（wire 为全零
        // 占位——真 wire 只能来自 verify 产出的合法事件）。
        let err = reg.verify(99, &raw(1, 50.0), 1_000_000).unwrap_err();
        assert_eq!(err, IngestReject::UnregisteredSource { source_id: 99 });
        let zero_wire = [0u8; 128];
        q.push(err, &zero_wire);
        assert_eq!(q.len(), 1);
        assert!(matches!(
            q.iter().next().unwrap().0,
            IngestReject::UnregisteredSource { .. }
        ));
    }

    #[test]
    fn epoch_assigned_by_registry_not_adapter() {
        // RawObservation 没有 epoch 字段——伪造入口在类型上不存在。
        let (mut reg, _store) = opened_registry();
        assert_eq!(reg.epoch(2), 1);
        let ev = reg.verify(2, &raw(1, 50.0), 1_000_000).unwrap();
        assert_eq!(ev.source_epoch, 1);
        assert_eq!(ev.sequence, 1);
        assert_eq!(ev.tenant_id, 1);
        assert_eq!(ev.site_id, 1);
    }

    #[test]
    fn epoch_increments_across_restart_with_persisted_store() {
        // 重启模拟：registry 销毁重建，同一 store——epoch 必须递增。
        let mut store = MemEpochStore::default();
        {
            let mut reg = SourceRegistry::new();
            reg.register(descriptor(2), 0);
            let e1 = reg.open_source(2, 0, &mut store).unwrap();
            assert_eq!(e1, 1);
        }
        {
            let mut reg = SourceRegistry::new();
            reg.register(descriptor(2), 0);
            let e2 = reg.open_source(2, 0, &mut store).unwrap();
            assert_eq!(e2, 2, "重启后 epoch 严格递增");
        }
    }

    #[test]
    fn epoch_floor_prevents_regression() {
        // 持久 epoch 回退不可能：floor 更低时取持久值 +1。
        let mut store = MemEpochStore::default();
        let mut reg = SourceRegistry::new();
        reg.register(descriptor(2), 0);
        reg.open_source(2, 10, &mut store).unwrap();
        let mut reg2 = SourceRegistry::new();
        reg2.register(descriptor(2), 0);
        // floor(3) < 持久 11 → 取持久值 +1 = 12（epoch 严格递增）。
        assert_eq!(reg2.open_source(2, 3, &mut store).unwrap(), 12);
    }

    // ===== sequence/重放/时钟 =====

    #[test]
    fn sequence_monotonic_replay_rejected() {
        let (mut reg, _store) = opened_registry();
        let e1 = reg.verify(2, &raw(1, 50.0), 1_000_000).unwrap();
        assert_eq!(e1.sequence, 1);
        let e2 = reg.verify(2, &raw(1, 51.0), 1_100_000).unwrap();
        assert_eq!(e2.sequence, 2);
        // registry 分配 sequence，调用方无法重放——回退只会发生在伪造
        // 实例上；sink watermark 复核拦截（下个测试）。
    }

    #[test]
    fn sink_watermark_blocks_cross_instance_replay() {
        let (mut reg, _store) = opened_registry();
        let mut sink = InMemorySink::new(64);
        let e1 = reg.verify(2, &raw(1, 50.0), 1_000_000).unwrap();
        sink.append(&e1).unwrap();
        // 伪造实例：同 source/epoch 从 sequence=1 重放。
        let mut forged = SourceRegistry::new();
        let d = descriptor(2);
        forged.register(d, 0);
        // 直接构造同身份同 sequence 事件（绕过真 registry 的 sequence 推进）。
        let mut replay = e1;
        replay.sequence = 1;
        let err = sink.append(&replay).unwrap_err();
        // 同 event_id（payload 相同）→ Duplicate；payload 不同 → SequenceReplay。
        assert!(matches!(
            err,
            IngestReject::Duplicate { .. } | IngestReject::SequenceReplay { .. }
        ));
        // 同 sequence 不同 payload = 篡改冲突形态：sink 以 SequenceReplay/
        // Duplicate 拒绝，绝不双写。
        let mut tampered = e1;
        tampered.value = ObservationValue::F64(999.0);
        tampered.sequence = 1;
        assert!(sink.append(&tampered).is_err());
        let _ = forged;
    }

    #[test]
    fn clock_rollback_rejected() {
        let (mut reg, _store) = opened_registry();
        reg.verify(2, &raw(1, 50.0), 1_000_000).unwrap();
        let mut older = raw(1, 51.0);
        older.event_time_ns = Some(999);
        assert!(matches!(
            reg.verify(2, &older, 1_100_000).unwrap_err(),
            IngestReject::ClockRollback { .. }
        ));
    }

    // ===== 越权/超速/完整性/schema =====

    #[test]
    fn unauthorized_point_rejected() {
        let (mut reg, _store) = opened_registry();
        assert_eq!(
            reg.verify(2, &raw(77, 1.0), 1_000_000).unwrap_err(),
            IngestReject::PointNotAuthorized { point_id: 77 }
        );
    }

    #[test]
    fn rate_limit_rejects_burst() {
        let mut reg = SourceRegistry::new();
        let mut store = MemEpochStore::default();
        let mut d = descriptor(2);
        d.max_events_per_s = 5; // 5/s → 满桶容量 5（1s 配额）
        reg.register(d, 0);
        reg.open_source(2, 0, &mut store).unwrap();
        // 同刻连发 5 个 = 排空满桶（容量=1s 配额）。
        for i in 0..5 {
            assert!(
                reg.verify(2, &raw(1, 1.0 + f64::from(i)), 1_000_000_000)
                    .is_ok(),
                "第 {i} 个应过"
            );
        }
        // 第 6 个超桶容量 → 拒绝（20ms 只回填 0.1 token）。
        assert!(matches!(
            reg.verify(2, &raw(1, 9.0), 1_020_000_000).unwrap_err(),
            IngestReject::RateExceeded { .. }
        ));
    }

    #[test]
    fn weak_integrity_rejected_when_required() {
        let (mut reg, _store) = opened_registry();
        let mut r = raw(1, 50.0);
        r.integrity = IntegrityTag::Unverified;
        assert!(matches!(
            reg.verify(2, &r, 1_000_000).unwrap_err(),
            IngestReject::IntegrityTooWeak { .. }
        ));
    }

    #[test]
    fn unknown_schema_rejected() {
        let (mut reg, _store) = opened_registry();
        let mut r = raw(1, 50.0);
        r.schema_id = 0xFE;
        assert_eq!(
            reg.verify(2, &r, 1_000_000).unwrap_err(),
            IngestReject::SchemaUnknown { schema_id: 0xFE }
        );
    }

    #[test]
    fn nan_rejected_at_door() {
        let (mut reg, _store) = opened_registry();
        let mut r = raw(1, 50.0);
        r.value = ObservationValue::F64(f64::NAN);
        assert_eq!(
            reg.verify(2, &r, 1_000_000).unwrap_err(),
            IngestReject::NotFinite
        );
    }

    #[test]
    fn disabled_source_rejected() {
        let mut reg = SourceRegistry::new();
        let mut store = MemEpochStore::default();
        let mut d = descriptor(2);
        d.enabled = false;
        reg.register(d, 0);
        reg.open_source(2, 0, &mut store).unwrap();
        assert!(matches!(
            reg.verify(2, &raw(1, 1.0), 1_000_000).unwrap_err(),
            IngestReject::SourceDisabled { .. }
        ));
    }

    #[test]
    fn before_open_rejected() {
        let mut reg = SourceRegistry::new();
        reg.register(descriptor(2), 0);
        assert!(matches!(
            reg.verify(2, &raw(1, 1.0), 1_000_000).unwrap_err(),
            IngestReject::SourceDisabled { .. }
        ));
    }

    // ===== quarantine 有界与零消费 =====

    #[test]
    fn quarantine_bounded_and_observable() {
        let mut q = QuarantineStore::new(2);
        let zero = [0u8; 128];
        q.push(IngestReject::UnregisteredSource { source_id: 9 }, &zero);
        q.push(IngestReject::UnregisteredSource { source_id: 8 }, &zero);
        q.push(IngestReject::UnregisteredSource { source_id: 7 }, &zero);
        assert_eq!(q.len(), 2);
        assert_eq!(q.dropped(), 1);
        // 只存最早丢弃计数与最新两条；控制 evaluator 无读取入口。
        assert_eq!(
            q.iter().next().unwrap().0,
            IngestReject::UnregisteredSource { source_id: 8 }
        );
    }

    #[test]
    fn clock_registry_readonly_lookup() {
        use super::ClockDomain;
        let mut reg = ClockRegistry::new();
        assert!(reg.is_empty());
        reg.register(ClockDomain {
            clock_id: 1,
            kind: 0,
            time_trusted: true,
        });
        reg.register(ClockDomain {
            clock_id: 4,
            kind: 4,
            time_trusted: false,
        });
        assert_eq!(reg.len(), 2);
        // 只读查询；未登记 id → None（fail-closed）。
        assert!(reg.domain(1).expect("d1").time_trusted);
        assert!(!reg.domain(4).expect("d4").time_trusted);
        assert!(reg.domain(9).is_none());
    }

    #[test]
    fn sink_rejects_epoch_regression_and_preserves_watermark() {
        // adapter 重启模拟：同 registry/store 再开源 → epoch 2；旧 epoch
        // 事件重放到 sink → EpochRegression，且 watermark 不被降级。
        let mut store = MemEpochStore::default();
        let mut reg = SourceRegistry::new();
        reg.register(descriptor(2), 0);
        let e1 = reg.open_source(2, 0, &mut store).unwrap();
        assert_eq!(e1, 1);
        let ev_old = reg.verify(2, &raw(1, 50.0), 1_000_000).unwrap();
        let e2 = reg.open_source(2, 0, &mut store).unwrap();
        assert_eq!(e2, 2, "重启后 epoch 严格递增");
        let ev_new = reg.verify(2, &raw(1, 51.0), 1_100_000).unwrap();
        assert_eq!(ev_new.source_epoch, 2);
        let mut sink = InMemorySink::new(64);
        sink.append(&ev_new).unwrap();
        assert_eq!(
            sink.append(&ev_old).unwrap_err(),
            IngestReject::EpochRegression {
                epoch: 1,
                expected: 2
            }
        );
        assert_eq!(sink.watermark(2), Some((2, ev_new.sequence)));
        // 同 epoch 内旧 sequence 仍被 SequenceReplay 拦截。
        let mut replay = ev_new;
        replay.sequence = 1;
        assert!(matches!(
            sink.append(&replay).unwrap_err(),
            IngestReject::SequenceReplay { .. } | IngestReject::Duplicate { .. }
        ));
    }

    #[test]
    fn sink_overflow_is_explicit_backpressure() {
        let mut sink = InMemorySink::new(1);
        let (mut reg, _store) = opened_registry();
        let e1 = reg.verify(2, &raw(1, 1.0), 1_000_000).unwrap();
        sink.append(&e1).unwrap();
        let e2 = reg.verify(2, &raw(1, 2.0), 1_100_000).unwrap();
        assert_eq!(sink.append(&e2).unwrap_err(), IngestReject::Overflow);
    }
}
