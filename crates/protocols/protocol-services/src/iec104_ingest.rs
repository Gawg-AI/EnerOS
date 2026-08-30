//! 蓝图 61 v0.314.31（[07] §3.3）：IEC 104 接入 adapter。
//!
//! 与 [`crate::modbus_ingest`] 同构（[07] §3.1：不修改 IngestionModule 内核
//! ——证明 seam 覆盖"事件上送 + 总召 + 时标 + 品质"的轮询差异协议）：本
//! adapter 只负责协议状态机与原始值——总召/自发收包 drain、CP56Time2a 时
//! 标、QualityDescriptor 逐位映射、重复 ASDU 去重；mapping/工程量/单位/
//! 事件身份/死区/QoS/日志全部属于 [`eneros_ingestion_core::IngestionModule`]
//! （[07] §2.2 分界——raw 携带未富化原始值，身份字段留零由 module 从
//! PointPlan 富化）。
//!
//! # 收包路径（[07] §3.3）
//!
//! 每 poll 两步：
//! 1. `plugin.poll`（ProtocolPlugin trait）驱动 STARTDT/总召唤调度/收帧
//!    （内部时钟 = eneros_time 单调钟——46.5 修正；host 未 time_init 时
//!    单调钟恒 0，状态机冻结，故 host 测试直测 [`drain_latest`] 纯函数，
//!    不跑 poll 全链）；
//! 2. drain master 数据面（[`eneros_iec104_master::LatestValue`]）：查
//!    映射得 point_id/device_id → 去重 → 推 [`RawObservation`]。
//!
//! # 时标与品质（[07] §3.3 逐条）
//!
//! - CP56Time2a → Unix 纳秒（master 侧保留）→ `event_time_ns`；
//! - IV 位（时钟未同步）→ 回退传输接收时刻 `timestamp_ms×1e6` + 品质追加
//!   UNCERTAIN（时钟质量保留，不静默冒充可信时标）；
//! - 无时标（TI 13 等）→ 接收时刻，品质不追加位——无时标≠时钟坏，时钟域
//!   信任面由 descriptor `clock_id` 承担（消费方裁决可参与控制快照切面）；
//!   注意：带时标（对端墙钟域）与接收时刻（本地单调域）混流时回退检测
//!   （ClockRollback）显式拒绝——可观测，不静默；
//! - QualityDescriptor 逐位：invalid→INVALID、not_topical→UNCERTAIN、
//!   substituted→SUBSTITUTED、blocked→BLOCKED、overflow→OVERFLOW；全良好
//!   → VALID（ADR-0027：IEC 品质无独立 valid 位）。
//!
//! # 去重（[07] §3.3：重复 ASDU 依 event identity 去重）
//!
//! seen 表 `(common_addr, ioa) → (timestamp_ms, value_fingerprint)`（有界
//! = 映射点数）：同戳同指纹（值+品质折叠）→ 跳过（重复 ASDU/重复 drain
//! 不重复发射——电平转边沿）；变化或新见 → 发射。预费用尽未发射的点不更新
//! seen——余下轮次自然重排（不静默丢）。
//!
//! # 预算（[07] §2.3）
//!
//! 一次 poll = 1 个协议事务（状态机驱动 + 数据面 drain）；发射事件数受
//! `budget.max_events` 封顶，超出记 deferred（下轮重排）。

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use eneros_iec104_master::plugin::Iec104Plugin;
use eneros_iec104_master::{IoValue, LatestValue, QualityDescriptor};
use eneros_ingestion_core::{
    IngestionAdapter, PollBudget, PollOutcome, PushOutcome, RawObservation, RawObservationSink,
};
use eneros_protocol_abstract::plugin::WorkBudget;
// ProtocolPlugin trait 方法（plugin.poll——收包状态机驱动入口）。
use eneros_protocol_abstract::ProtocolPlugin as _;
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

/// 共享插件句柄（收包=adapter drain；自检=bin 启动期轮——单线程 EL0，RefCell）。
pub type SharedIec104Plugin = Rc<RefCell<Iec104Plugin>>;

/// IEC 104 点映射：`(common_addr, ioa) → (point_id, device_id)`。
///
/// bin 装配时从点表 `ProtocolAddress::Iec104` 行构建并传入（插件不自造
/// 地址——46.5 纪律）；调度/身份（schema/unit/asset/世代）在 PointPlan，
/// 由 module 富化——本表只承载协议坐标到站内点的路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IecMapping {
    /// ASDU 公共地址（设备）。
    pub common_addr: u16,
    /// 信息对象地址（IOA）。
    pub ioa: u32,
    /// 站内点位 ID。
    pub point_id: u32,
    /// 站内设备短 ID。
    pub device_id: u16,
}

/// IoValue → ObservationValue（[07] §3.3：raw 原始值映射）。
///
/// Float→F64、归一化/标度化/计数/步位→I64、单点遥信→Bool、双点遥信→Enum、
/// 比特串→U64。命令/设点回显类型不进数据面（`master.handle_i_format` 不
/// 存）——防御性兜底按命令码原样保留（不伪造数值）。
fn observation_value(v: IoValue) -> ObservationValue {
    match v {
        IoValue::Float(f) => ObservationValue::F64(f64::from(f)),
        IoValue::Normalized(n) => ObservationValue::I64(i64::from(n)),
        IoValue::Scaled(s) => ObservationValue::I64(i64::from(s)),
        IoValue::Counter(c) => ObservationValue::I64(i64::from(c)),
        IoValue::SinglePoint(sp) => {
            ObservationValue::Bool(sp == eneros_iec104_master::SinglePointValue::On)
        }
        IoValue::DoublePoint(d) => ObservationValue::Enum(u64::from(d as u8)),
        IoValue::StepPosition(v) => ObservationValue::I64(i64::from(v)),
        IoValue::Bitstring(b) => ObservationValue::U64(u64::from(b)),
        IoValue::SingleCommand(sco) => ObservationValue::Enum(u64::from(sco.encode())),
        IoValue::DoubleCommand(dco) => ObservationValue::Enum(u64::from(dco.encode())),
        IoValue::SetpointNormalized(n) => ObservationValue::I64(i64::from(n)),
        IoValue::SetpointFloat(f) => ObservationValue::F64(f64::from(f)),
    }
}

/// QualityDescriptor → quality_bits 逐位映射（[07] §3.3；全良好 → VALID）。
fn quality_bits_of(q: &QualityDescriptor) -> u16 {
    let mut bits = 0u16;
    if q.invalid {
        bits |= quality_bits::INVALID;
    }
    if q.not_topical {
        bits |= quality_bits::UNCERTAIN;
    }
    if q.substituted {
        bits |= quality_bits::SUBSTITUTED;
    }
    if q.blocked {
        bits |= quality_bits::BLOCKED;
    }
    if q.overflow {
        bits |= quality_bits::OVERFLOW;
    }
    if bits == 0 {
        // ADR-0027：IEC 品质描述符无独立 valid 位——全部坏位为 false 即有效。
        bits = quality_bits::VALID;
    }
    bits
}

/// 去重指纹：品质位（高 2B）折叠进值位形——品质变化不与同值重复合并
/// （[07] §2.3：品质变化永不被抑制的 adapter 侧对偶）。
fn fingerprint(value: ObservationValue, qbits: u16) -> [u8; 16] {
    let mut fp = [0u8; 16];
    fp[..2].copy_from_slice(&qbits.to_le_bytes());
    let (_, bits) = value.encode();
    fp[2..].copy_from_slice(&bits[..14]);
    fp
}

/// 映射索引（`(ca, ioa) → (point_id, device_id)`；装配期一次构建）。
fn mapping_index(mappings: &[IecMapping]) -> BTreeMap<(u16, u32), (u32, u16)> {
    mappings
        .iter()
        .map(|m| ((m.common_addr, m.ioa), (m.point_id, m.device_id)))
        .collect()
}

/// drain 统计（bin ACQ/诊断与测试判据）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainStats {
    /// 发射（push 进入 sink）事件数。
    pub pushed: u32,
    /// 预算截断未发射数（seen 未更新——下轮自然重排）。
    pub deferred: u32,
    /// 同戳同值去重跳过数。
    pub skipped: u32,
    /// 无映射跳过数（对端上报点表外 (ca, ioa)——不进事件面）。
    pub unmapped: u32,
    /// sink 接受/合并数（ACQ good）。
    pub good: u32,
    /// sink 背压/拒绝数（ACQ bad）。
    pub bad: u32,
}

/// drain 纯函数（[07] §3.3 收包→事件面的全部裁决单点；host 测试不经
/// TCP 直测——构造 `LatestValue` fixture 注入）。
///
/// 遍历 master 数据面 latest 值：查映射 → 去重（同戳同指纹跳过）→ 预算
/// 截断 → 时标/品质映射 → push。**只有实际进入 sink 的点才更新 seen**
/// （预算截断/背压的观测下轮重排——不静默丢）。
fn drain_latest<'a, I>(
    max_events: u32,
    index: &BTreeMap<(u16, u32), (u32, u16)>,
    seen: &mut BTreeMap<(u16, u32), (u64, [u8; 16])>,
    latest: I,
    out: &mut dyn RawObservationSink,
) -> DrainStats
where
    I: Iterator<Item = (u16, u32, &'a LatestValue)>,
{
    let mut stats = DrainStats::default();
    for (ca, ioa, lv) in latest {
        let Some(&(point_id, device_id)) = index.get(&(ca, ioa)) else {
            stats.unmapped += 1;
            continue;
        };
        let qbits = quality_bits_of(&lv.quality);
        let fp = fingerprint(observation_value(lv.value), qbits);
        let key = (ca, ioa);
        // 去重（[07] §3.3：重复 ASDU 去重；同戳同值+同品质 = 事实未变）。
        if seen.get(&key) == Some(&(lv.timestamp_ms, fp)) {
            stats.skipped += 1;
            continue;
        }
        // 预算封顶（[07] §2.3：一次 poll 最多 budget 事件；超出停排，
        // seen 未更新 → 余下轮次自然重排）。
        if stats.pushed >= max_events {
            stats.deferred += 1;
            continue;
        }
        // 时标路径（[07] §3.3：CP56 → event_time，保留时钟质量）。
        let fallback_ns = lv.timestamp_ms.saturating_mul(1_000_000).max(1);
        let (event_time_ns, quality) = match (lv.time_tag_ns, lv.time_tag_iv) {
            // 可信对端时标：直接采用。
            (Some(ns), false) => (ns.max(1), qbits),
            // IV 位（时钟未同步）：回退接收时刻 + UNCERTAIN（不冒充可信）。
            (Some(_), true) => (fallback_ns, qbits | quality_bits::UNCERTAIN),
            // 无时标：接收时刻；时钟域信任面由 descriptor clock_id 承担
            // （无时标≠时钟坏，不追加品质位）。
            (None, _) => (fallback_ns, qbits),
        };
        let pushed = out.push(RawObservation {
            point_id,
            value: observation_value(lv.value),
            event_time_ns: Some(event_time_ns),
            quality_bits: quality,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            // [07] §2.2 分界：身份（schema/unit/asset/世代）由 module 从
            // PointPlan 富化——raw 留零（与 modbus adapter 一致）。
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 0,
            device_id,
        });
        match pushed {
            PushOutcome::Backpressured | PushOutcome::Rejected => {
                // 未入流：seen 不更新——下轮重发（背压不吞事件）。
                stats.bad += 1;
            }
            _ => {
                stats.good += 1;
                seen.insert(key, (lv.timestamp_ms, fp));
            }
        }
        stats.pushed += 1;
    }
    stats
}

/// IEC 104 接入 adapter（bin 装配；传输/状态机在
/// [`eneros_iec104_master::plugin::Iec104Plugin`]）。
pub struct Iec104IngestAdapter {
    source_id: u32,
    plugin: SharedIec104Plugin,
    /// `(ca, ioa) → (point_id, device_id)` 路由索引（装配期构建）。
    index: BTreeMap<(u16, u32), (u32, u16)>,
    /// 去重表（有界 = 映射点数；(timestamp_ms, value_fingerprint)）。
    seen: BTreeMap<(u16, u32), (u64, [u8; 16])>,
    /// 自上次成功 poll 以来累计重连计数（poll Err→Ok 转换——同 modbus
    /// 形态；bin 侧折进 module.note_reconnect，[07] §2.4）。
    reconnects: u32,
    last_poll_ok: bool,
    /// 本轮采集证据行计数（ACQ good/bad——bin 打印聚合）。
    pub acq_good: u32,
    pub acq_bad: u32,
}

impl Iec104IngestAdapter {
    /// 构造（mappings 从点表 `ProtocolAddress::Iec104` 行构建——bin 装配）。
    pub fn new(source_id: u32, plugin: SharedIec104Plugin, mappings: Vec<IecMapping>) -> Self {
        Self {
            source_id,
            plugin,
            index: mapping_index(&mappings),
            seen: BTreeMap::new(),
            reconnects: 0,
            last_poll_ok: true,
            acq_good: 0,
            acq_bad: 0,
        }
    }
}

impl IngestionAdapter for Iec104IngestAdapter {
    fn source_id(&self) -> u32 {
        self.source_id
    }

    fn poll(
        &mut self,
        now_ns: u64,
        budget: PollBudget,
        _work: &[eneros_ingestion_core::PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> PollOutcome {
        if budget.max_transactions == 0 {
            return PollOutcome::Busy { transactions: 0 };
        }
        // 1. 驱动协议状态机（STARTDT/总召唤调度/收帧）。插件内部时钟 =
        //    eneros_time 单调钟（46.5 修正：调度域与 connect 同源）——调用
        //    侧 now_ms 仅作健康观测。host 未 time_init 时单调钟恒 0，状态
        //    机冻结——host 测试直测 drain_latest 纯函数（不经 poll 全链）。
        let now_ms = now_ns / 1_000_000;
        let work_budget = WorkBudget {
            max_work_ms: 50,
            max_polls: 8,
        };
        match self.plugin.borrow_mut().poll(now_ms, work_budget) {
            Ok(_) => {
                if !self.last_poll_ok {
                    // Err→Ok 转换 = 传输重连（同 modbus ok-after-fail 形态）。
                    self.reconnects = self.reconnects.saturating_add(1);
                    self.last_poll_ok = true;
                }
            }
            Err(_) => self.last_poll_ok = false,
        }
        // 2. drain 数据面（纯函数 seam；映射/去重/时标/品质单点）。
        //    guard 绑定须活过 drain——latest 迭代器借用插件数据面（作用域
        //    结束时 latest 先于 guard 析构）。
        let guard = self.plugin.borrow();
        let mut latest = guard.latest_values();
        let stats = drain_latest(
            budget.max_events,
            &self.index,
            &mut self.seen,
            &mut latest,
            out,
        );
        self.acq_good = self.acq_good.saturating_add(stats.good);
        self.acq_bad = self.acq_bad.saturating_add(stats.bad);
        PollOutcome::Completed {
            events: stats.pushed,
            transactions: 1,
        }
    }

    fn take_reconnects(&mut self) -> u32 {
        core::mem::take(&mut self.reconnects)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::rc::Rc;

    use eneros_iec104_master::{DoublePointValue, QualityDescriptor, SinglePointValue};

    use super::*;

    /// 构造 LatestValue fixture（不经真实 TCP——drain 纯函数注入面）。
    fn lv(value: IoValue, quality: QualityDescriptor, ts_ms: u64) -> LatestValue {
        LatestValue {
            value,
            quality,
            timestamp_ms: ts_ms,
            time_tag_ns: None,
            time_tag_iv: false,
        }
    }

    fn good() -> QualityDescriptor {
        QualityDescriptor::good()
    }

    /// 单点映射表（ca=1, ioa=10 → point=1, device=2）。
    fn one_mapping() -> Vec<IecMapping> {
        alloc::vec![IecMapping {
            common_addr: 1,
            ioa: 10,
            point_id: 1,
            device_id: 2,
        }]
    }

    /// 从映射表直接构建（绕过 Rc——纯函数测试面）。
    fn idx(mappings: &[IecMapping]) -> BTreeMap<(u16, u32), (u32, u16)> {
        mapping_index(mappings)
    }

    #[derive(Default)]
    struct CaptureSink {
        pushes: Vec<RawObservation>,
        /// push 回执策略（默认 Accepted；可注入背压/拒绝形态）。
        verdict: Vec<PushOutcome>,
    }

    impl RawObservationSink for CaptureSink {
        fn push(&mut self, raw: RawObservation) -> PushOutcome {
            let v = self.verdict.pop().unwrap_or(PushOutcome::Accepted);
            self.pushes.push(raw);
            v
        }
    }

    /// fixture → 迭代器（drain_latest 泛型注入面——与 plugin 数据面
    /// `Box<dyn Iterator>` 同 Item 形）。
    fn fixtures(
        pairs: &[(u16, u32, LatestValue)],
    ) -> impl Iterator<Item = (u16, u32, &LatestValue)> {
        pairs.iter().map(|(c, i, v)| (*c, *i, v))
    }

    // ===== 映射 + 去重：同值同戳跳过、变化发射 =====

    #[test]
    fn drain_dedups_same_value_and_stamp_emits_change() {
        let mappings = one_mapping();
        let index = idx(&mappings);
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        // 首见：Float 50.5 @ ts=1000 → 发射。
        let f1 = alloc::vec![(1u16, 10u32, lv(IoValue::Float(50.5), good(), 1_000))];
        let s = drain_latest(16, &index, &mut seen, fixtures(&f1), &mut sink);
        assert_eq!(s.pushed, 1);
        assert_eq!(s.skipped, 0);
        assert_eq!(sink.pushes[0].point_id, 1);
        assert_eq!(sink.pushes[0].device_id, 2);
        assert_eq!(sink.pushes[0].value, ObservationValue::F64(50.5));
        assert_eq!(sink.pushes[0].quality_bits, quality_bits::VALID);
        assert_eq!(sink.pushes[0].event_time_ns, Some(1_000_000_000));
        // 重复 drain（同戳同值——重复 ASDU/重复总召形态）→ 跳过。
        let s = drain_latest(16, &index, &mut seen, fixtures(&f1), &mut sink);
        assert_eq!(s.pushed, 0);
        assert_eq!(s.skipped, 1);
        // 值变化（同戳）→ 发射。
        let f2 = alloc::vec![(1u16, 10u32, lv(IoValue::Float(51.5), good(), 1_000))];
        let s = drain_latest(16, &index, &mut seen, fixtures(&f2), &mut sink);
        assert_eq!(s.pushed, 1, "值变化必发射");
        assert_eq!(
            sink.pushes.last().unwrap().value,
            ObservationValue::F64(51.5)
        );
        // 品质变化（同值同戳）→ 发射（品质变化永不合并）。
        let mut bad_q = QualityDescriptor::good();
        bad_q.invalid = true;
        let f3 = alloc::vec![(1u16, 10u32, lv(IoValue::Float(51.5), bad_q, 1_000))];
        let s = drain_latest(16, &index, &mut seen, fixtures(&f3), &mut sink);
        assert_eq!(s.pushed, 1, "品质变化必发射（指纹折叠品质）");
        assert_eq!(
            sink.pushes.last().unwrap().quality_bits,
            quality_bits::INVALID
        );
    }

    // ===== 品质映射：invalid→INVALID；not_topical→UNCERTAIN；全良好→VALID =====

    #[test]
    fn quality_descriptor_maps_bitwise() {
        let mappings = one_mapping();
        let index = idx(&mappings);
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        let mut q = QualityDescriptor::good();
        q.not_topical = true;
        q.overflow = true;
        let f = alloc::vec![(1u16, 10u32, lv(IoValue::Float(1.0), q, 5_000))];
        drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        let bits = sink.pushes[0].quality_bits;
        assert_eq!(
            bits,
            quality_bits::UNCERTAIN | quality_bits::OVERFLOW,
            "not_topical→UNCERTAIN、overflow→OVERFLOW；全坏位=false 才 VALID"
        );
        // substituted / blocked 位。
        let mut seen2 = BTreeMap::new();
        let mut sink2 = CaptureSink::default();
        let mut q2 = QualityDescriptor::good();
        q2.substituted = true;
        q2.blocked = true;
        let f2 = alloc::vec![(1u16, 10u32, lv(IoValue::Float(2.0), q2, 5_000))];
        drain_latest(16, &index, &mut seen2, fixtures(&f2), &mut sink2);
        assert_eq!(
            sink2.pushes[0].quality_bits,
            quality_bits::SUBSTITUTED | quality_bits::BLOCKED
        );
    }

    // ===== 时标路径：可信 CP56 / IV 变体 / 无时标 =====

    #[test]
    fn trusted_time_tag_used_as_event_time() {
        let index = idx(&one_mapping());
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        let mut v = lv(IoValue::Float(61.5), good(), 9_000);
        v.time_tag_ns = Some(1_788_091_200_500_000_000); // 2026-08-30 12:00:00.500Z
        v.time_tag_iv = false;
        let f = alloc::vec![(1u16, 10u32, v)];
        drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(
            sink.pushes[0].event_time_ns,
            Some(1_788_091_200_500_000_000),
            "可信 CP56 时标 → event_time"
        );
        assert_eq!(
            sink.pushes[0].quality_bits,
            quality_bits::VALID,
            "iv=false 不追加 UNCERTAIN"
        );
    }

    #[test]
    fn iv_time_tag_falls_back_with_uncertain() {
        let index = idx(&one_mapping());
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        let mut v = lv(IoValue::Float(61.5), good(), 9_000);
        v.time_tag_ns = Some(1_788_091_200_500_000_000);
        v.time_tag_iv = true; // 时钟未同步
        let f = alloc::vec![(1u16, 10u32, v)];
        drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(
            sink.pushes[0].event_time_ns,
            Some(9_000_000_000),
            "IV=true → 回退传输接收时刻（ts×1e6）"
        );
        assert_eq!(
            sink.pushes[0].quality_bits,
            quality_bits::VALID | quality_bits::UNCERTAIN,
            "IV=true → 品质追加 UNCERTAIN（时钟质量保留）"
        );
    }

    #[test]
    fn no_time_tag_uses_receive_time_without_flags() {
        let index = idx(&one_mapping());
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        // time_tag_ns=None（TI=13 无时标形态）。
        let f = alloc::vec![(1u16, 10u32, lv(IoValue::Float(3.0), good(), 7_500))];
        drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(
            sink.pushes[0].event_time_ns,
            Some(7_500_000_000),
            "无时标 → 接收时刻（时钟域信任面由 clock_id 承担）"
        );
        assert_eq!(
            sink.pushes[0].quality_bits,
            quality_bits::VALID,
            "无时标≠时钟坏——不追加品质位"
        );
    }

    // ===== IoValue → ObservationValue 映射 =====

    #[test]
    fn iovalue_maps_to_observation_value() {
        assert_eq!(
            observation_value(IoValue::Float(1.5)),
            ObservationValue::F64(1.5)
        );
        assert_eq!(
            observation_value(IoValue::Normalized(-3)),
            ObservationValue::I64(-3)
        );
        assert_eq!(
            observation_value(IoValue::Scaled(320)),
            ObservationValue::I64(320)
        );
        assert_eq!(
            observation_value(IoValue::Counter(99)),
            ObservationValue::I64(99)
        );
        assert_eq!(
            observation_value(IoValue::SinglePoint(SinglePointValue::On)),
            ObservationValue::Bool(true)
        );
        assert_eq!(
            observation_value(IoValue::SinglePoint(SinglePointValue::Off)),
            ObservationValue::Bool(false)
        );
        assert_eq!(
            observation_value(IoValue::DoublePoint(DoublePointValue::On)),
            ObservationValue::Enum(2)
        );
        assert_eq!(
            observation_value(IoValue::StepPosition(5)),
            ObservationValue::I64(5)
        );
        assert_eq!(
            observation_value(IoValue::Bitstring(0x0F0F)),
            ObservationValue::U64(0x0F0F)
        );
    }

    // ===== 预算截断：超出停排，seen 未更新下轮自然重排 =====

    #[test]
    fn budget_truncates_and_deferred_reemits_next_round() {
        let mappings = alloc::vec![
            IecMapping {
                common_addr: 1,
                ioa: 10,
                point_id: 1,
                device_id: 2
            },
            IecMapping {
                common_addr: 1,
                ioa: 11,
                point_id: 2,
                device_id: 2
            },
        ];
        let index = idx(&mappings);
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        let f = alloc::vec![
            (1u16, 10u32, lv(IoValue::Float(1.0), good(), 1_000)),
            (1u16, 11u32, lv(IoValue::Float(2.0), good(), 1_000)),
        ];
        // 预算 1 → 首点发射、次点截断。
        let s = drain_latest(1, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(s.pushed, 1);
        assert_eq!(s.deferred, 1);
        assert_eq!(seen.len(), 1, "截断点不得入 seen");
        // 下轮预算放开 → 首点去重跳过、截断点重排发射。
        let s = drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(s.pushed, 1);
        assert_eq!(s.skipped, 1);
        assert_eq!(s.deferred, 0);
        assert_eq!(sink.pushes.len(), 2);
        assert_eq!(sink.pushes[1].point_id, 2);
    }

    // ===== 背压：未入流不更新 seen（下轮重发——不吞事件）=====

    #[test]
    fn backpressured_push_keeps_seen_stale_for_retry() {
        let index = idx(&one_mapping());
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink {
            verdict: alloc::vec![PushOutcome::Backpressured],
            pushes: alloc::vec![],
        };
        let f = alloc::vec![(1u16, 10u32, lv(IoValue::Float(9.0), good(), 2_000))];
        let s = drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(s.pushed, 1);
        assert_eq!(s.bad, 1);
        assert!(seen.is_empty(), "背压未入流——seen 不得更新");
        // 恢复接受 → 同一观测重发成功。
        let mut sink2 = CaptureSink::default();
        let s = drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink2);
        assert_eq!(s.good, 1);
        assert_eq!(seen.len(), 1);
    }

    // ===== 无映射点：跳过（不进事件面）=====

    #[test]
    fn unmapped_points_are_counted_and_skipped() {
        let index = idx(&one_mapping());
        let mut seen = BTreeMap::new();
        let mut sink = CaptureSink::default();
        let f = alloc::vec![
            (1u16, 10u32, lv(IoValue::Float(1.0), good(), 1_000)),
            (9u16, 99u32, lv(IoValue::Float(2.0), good(), 1_000)), // 点表外
        ];
        let s = drain_latest(16, &index, &mut seen, fixtures(&f), &mut sink);
        assert_eq!(s.pushed, 1);
        assert_eq!(s.unmapped, 1);
    }

    // ===== adapter 级：未启动插件（host 形态）→ poll 全链零事件不 panic =====

    #[test]
    fn poll_with_unstarted_plugin_is_safe_and_counts_reconnects() {
        let plugin: SharedIec104Plugin = Rc::new(RefCell::new(Iec104Plugin::new()));
        let mut a = Iec104IngestAdapter::new(2, plugin, one_mapping());
        let mut sink = CaptureSink::default();
        // 未 start → plugin.poll Err → last_poll_ok=false；drain 空数据面。
        let out = a.poll(
            1_000,
            PollBudget {
                max_events: 16,
                max_transactions: 16,
            },
            &[],
            &mut sink,
        );
        assert_eq!(
            out,
            PollOutcome::Completed {
                events: 0,
                transactions: 1
            }
        );
        assert!(a.take_reconnects() == 0, "未 Ok 过不记重连");
        // 零事务预算 → Busy（不驱动状态机）。
        let out = a.poll(
            1_000,
            PollBudget {
                max_events: 16,
                max_transactions: 0,
            },
            &[],
            &mut sink,
        );
        assert_eq!(out, PollOutcome::Busy { transactions: 0 });
    }
}
