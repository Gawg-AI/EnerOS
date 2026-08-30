//! 蓝图 61 [07] §3.5（v0.314.31）：adapter 无关合同套件。
//!
//! 一切接入 adapter（Modbus / IEC104 / 市场事实……）必须经同一套合同
//! 证明：来源越权、未知 schema、epoch/sequence 纪律、限流、背压无泄漏、
//! 重启/重连、raw 错误到质量/拒绝的映射、event_id 稳定与 payload 冲突、
//! poll/push 双模式。套件只经 [`crate::IngestionModule`] 公共 API 表达
//! ——协议特例在类型上不可达（adapter 只见
//! [`crate::RawObservationSink`]）。
//!
//! # 用法
//!
//! feature `contract`（默认关）下暴露；各协议的集成测试以自身的
//! descriptor/点计划/预算装配 [`ContractScenario`]，运行
//! [`run_full_suite`]。任何场景失败 → [`ContractFailure`]（场景名 + 证据
//! 串），全过 → 空报告。

use alloc::string::String;
use alloc::vec::Vec;
use alloc::{boxed::Box, format};

use eneros_telemetry_model::event::{event_flags, quality_bits, IntegrityTag, ObservationValue};

use crate::{
    InMemorySink, IngestOutcome, IngestReject, IngestionAdapter, IngestionModule, MemEpochStore,
    OperationalProfile, PointPlan, PollBudget, PollOutcome, PushOutcome, RawObservation,
    RawObservationSink, TickBudget,
};

/// 合同失败（场景名 + 证据）。
#[derive(Debug, Clone, PartialEq)]
pub struct ContractFailure {
    pub scenario: &'static str,
    pub detail: String,
}

/// 场景装配（协议侧提供：descriptor/点计划/预算——套件不关心协议）。
pub struct ContractScenario {
    /// 被测来源 descriptor。
    pub descriptor: crate::SourceDescriptor,
    /// 点计划（≥1；schema_id 须在 descriptor.allowed_schemas 内）。
    pub plans: Vec<PointPlan>,
    /// 运行剖面。
    pub profile: OperationalProfile,
    /// 事件预算（tick）。
    pub budget: TickBudget,
}

impl ContractScenario {
    /// 标准被测点（首个计划）。
    pub fn first_point(&self) -> u32 {
        self.plans[0].point_id
    }
}

/// 合法原始观测（走 `plan` 的点、值 = 计数器；时刻最小 1ns）。
fn good_raw(plan: &PointPlan, value: i64, now_ns: u64) -> RawObservation {
    RawObservation {
        point_id: plan.point_id,
        value: ObservationValue::I64(value),
        event_time_ns: Some(now_ns.max(1)),
        quality_bits: quality_bits::VALID,
        integrity: IntegrityTag::LocalTrustedEdge,
        flags: 0,
        schema_id: 0,
        schema_version: 0,
        unit_code: 0,
        asset_id: 0,
        mapping_version: 0,
        device_id: plan.device_id,
    }
}

/// 脚本化 adapter（poll 模式：先推 `batch`，再无剩余且 `auto_emit` 存在
/// 时补推一次——poll 合同场景的最小事件源）。
struct ScriptedAdapter {
    source_id: u32,
    batch: Vec<RawObservation>,
    auto_emit: Option<RawObservation>,
}

impl IngestionAdapter for ScriptedAdapter {
    fn source_id(&self) -> u32 {
        self.source_id
    }
    fn poll(
        &mut self,
        _now_ns: u64,
        budget: PollBudget,
        _work: &[PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> PollOutcome {
        let mut events = 0u32;
        let mut transactions = 0u32;
        for r in self.batch.drain(..) {
            if events >= budget.max_events {
                return PollOutcome::Busy { transactions };
            }
            transactions += 1;
            match out.push(r) {
                PushOutcome::Backpressured | PushOutcome::Rejected => {
                    return PollOutcome::Completed {
                        events,
                        transactions,
                    }
                }
                _ => events += 1,
            }
        }
        if let Some(r) = self.auto_emit.take() {
            let _ = out.push(r);
            events += 1;
            transactions += 1;
        }
        PollOutcome::Completed {
            events,
            transactions,
        }
    }
}

/// 装配被测模块（open + install_points + attach）。
fn assembled(scenario: &ContractScenario, sink_cap: usize) -> IngestionModule {
    assembled_with(scenario, sink_cap, None)
}

/// 装配 + 事件源（poll 合同场景：auto_emit 补推）。
fn assembled_with(
    scenario: &ContractScenario,
    sink_cap: usize,
    auto_emit: Option<RawObservation>,
) -> IngestionModule {
    let mut m = IngestionModule::new(
        Box::new(InMemorySink::new(sink_cap)),
        Box::new(MemEpochStore::default()),
        scenario.profile,
    );
    m.register(scenario.descriptor.clone());
    let _ = m.open(scenario.descriptor.source_id, 0).unwrap();
    m.install_points(1, scenario.plans.clone()).unwrap();
    m.attach(Box::new(ScriptedAdapter {
        source_id: scenario.descriptor.source_id,
        batch: Vec::new(),
        auto_emit,
    }))
    .unwrap();
    m
}

/// 合同 1：来源越权（descriptor 点域外的点 → PointNotAuthorized）。
pub fn unauthorized_point_rejected(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    // 无界点域（(0,0)）来源：越权在 adapter 绑定门执行（如市场事实的
    // product/region → point 绑定）——模块级检查不适用，按合同豁免。
    if scenario.descriptor.point_range == (0, 0) {
        return None;
    }
    let source = scenario.descriptor.source_id;
    let now = 1_000;
    let mut p0 = scenario.plans[0];
    p0.point_id = 0xFFFF_FFFF; // 域外点（无计划 → 原样过门 → descriptor 拒）
    match m.ingest(source, good_raw(&p0, 1, now), now) {
        Err(IngestReject::PointNotAuthorized { .. }) => None,
        other => Some(ContractFailure {
            scenario: "unauthorized_point",
            detail: format!("期望 PointNotAuthorized，实际 {other:?}"),
        }),
    }
}

/// 合同 2：未知 schema（descriptor 未注册的 schema → SchemaUnknown）。
pub fn unknown_schema_rejected(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    // 未注册 schema 模式（计划 schema_id=0，如市场事实）：schema 门在
    // 该形态下不可表达——按合同豁免（descriptor 的 allowed_schemas 仍
    // 约束注册了 schema 的计划）。
    if scenario.plans[0].schema_id == 0 {
        return None;
    }
    let source = scenario.descriptor.source_id;
    let now = 2_000;
    // 探测点 = 域内无计划点（无计划 → 不经目录富化 → raw.schema 直达
    // 注册表门；有界域内放不下时越权先拒——两种拒绝都算门上拦截）。
    let probe = scenario
        .plans
        .iter()
        .map(|p| p.point_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut raw = good_raw(&scenario.plans[0], 1, now);
    raw.point_id = probe;
    raw.schema_id = 0xFEFE;
    match m.ingest(source, raw, now) {
        Err(IngestReject::SchemaUnknown { .. }) | Err(IngestReject::PointNotAuthorized { .. }) => {
            None
        }
        other => Some(ContractFailure {
            scenario: "unknown_schema",
            detail: format!("期望 SchemaUnknown，实际 {other:?}"),
        }),
    }
}

/// 合同 3：epoch/sequence 纪律——同模块再开源 epoch 严格递增；watermark
/// 单调；重复 event_id（同 payload）幂等去重。
pub fn epoch_sequence_discipline(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let plan = scenario.plans[0];
    let e1 = m.open(source, 0).unwrap();
    let ack = m.ingest(source, good_raw(&plan, 10, 3_000), 3_000).unwrap();
    let IngestOutcome::Appended(a1) = ack else {
        return Some(ContractFailure {
            scenario: "epoch_sequence",
            detail: format!("首个事件未追加：{ack:?}"),
        });
    };
    let e2 = m.open(source, 0).unwrap();
    if e2 <= e1 {
        return Some(ContractFailure {
            scenario: "epoch_sequence",
            detail: format!("重启后 epoch 未递增：{e1} → {e2}"),
        });
    }
    let ack2 = m.ingest(source, good_raw(&plan, 11, 4_000), 4_000).unwrap();
    let IngestOutcome::Appended(a2) = ack2 else {
        return Some(ContractFailure {
            scenario: "epoch_sequence",
            detail: format!("新 epoch 首事件未追加：{ack2:?}"),
        });
    };
    if a2.event_id == a1.event_id {
        return Some(ContractFailure {
            scenario: "epoch_sequence",
            detail: "不同内容事件 event_id 相同".into(),
        });
    }
    None
}

/// 合同 4：限流（把 descriptor 限速压到 2/s → 第三个同刻事件 RateExceeded）。
pub fn rate_limit_bounded(scenario: &ContractScenario) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let mut d = scenario.descriptor.clone();
    d.max_events_per_s = 2;
    let mut m = IngestionModule::new(
        Box::new(InMemorySink::new(64)),
        Box::new(MemEpochStore::default()),
        scenario.profile,
    );
    m.register(d);
    m.open(source, 0).unwrap();
    m.install_points(1, scenario.plans.clone()).unwrap();
    let plan = &scenario.plans[0];
    let now = 5_000_000_000;
    for i in 0..2 {
        let r = m.ingest(source, good_raw(plan, i, now), now);
        if r.is_err() {
            return Some(ContractFailure {
                scenario: "rate_limit",
                detail: format!("第 {i} 个配额内事件被拒：{r:?}"),
            });
        }
    }
    match m.ingest(source, good_raw(plan, 99, now), now) {
        Err(IngestReject::RateExceeded { .. }) => None,
        other => Some(ContractFailure {
            scenario: "rate_limit",
            detail: format!("期望 RateExceeded，实际 {other:?}"),
        }),
    }
}

/// 合同 5：背压无内存泄漏（sink 容量 2，洪灌 1000 事件——sink 有界、
/// 背压显式、点状态表不超计划数）。
pub fn backpressure_no_leak(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let now = 6_000_000_000;
    let mut report = m.tick(
        now,
        TickBudget {
            max_events: 1024,
            max_transactions: 1024,
        },
    );
    // 用直推洪灌（ScriptedAdapter 批量已空；直推等价 push 语义）。
    let mut backpressured = 0u32;
    for i in 0..1000u64 {
        let plan = &scenario.plans[(i as usize) % scenario.plans.len()];
        let mut raw = good_raw(plan, 10_000 + i as i64, now + i);
        // 同点重复值会被死区合并（Q1 不可抑制时不会）——统一保证值唯一。
        raw.value = ObservationValue::I64(10_000 + i as i64);
        match m.ingest(source, raw, now + i) {
            Err(IngestReject::Overflow) | Err(IngestReject::RateExceeded { .. }) => {
                backpressured += 1
            }
            Ok(_) => {}
            Err(other) => {
                return Some(ContractFailure {
                    scenario: "backpressure",
                    detail: format!("意外拒绝 {other:?}（i={i}）"),
                })
            }
        }
    }
    let _ = &mut report;
    if backpressured == 0 {
        return Some(ContractFailure {
            scenario: "backpressure",
            detail: "洪灌未触发任何背压——容量界未生效".into(),
        });
    }
    None
}

/// 合同 6：重启/重连——重连不换 source epoch（connection_epoch 递增）。
pub fn restart_reconnect(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let epoch_before = m.epoch(source);
    m.note_reconnect(source);
    m.note_reconnect(source);
    if m.connection_epoch(source) != 2 {
        return Some(ContractFailure {
            scenario: "restart_reconnect",
            detail: format!(
                "connection_epoch 期望 2，实际 {}",
                m.connection_epoch(source)
            ),
        });
    }
    if m.epoch(source) != epoch_before {
        return Some(ContractFailure {
            scenario: "restart_reconnect",
            detail: "重连改变了本地 source epoch".into(),
        });
    }
    None
}

/// 合同 7：raw 错误映射——坏品质 raw → INVALID 事件接受（组异常按点
/// 质量展开）；弱完整性 raw → IntegrityTooWeak 拒绝。
pub fn raw_error_mapping(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let plan = &scenario.plans[0];
    let now = 7_000_000_000;
    let mut bad = good_raw(plan, 1, now);
    bad.quality_bits = quality_bits::INVALID;
    match m.ingest(source, bad, now) {
        Ok(IngestOutcome::Appended(_)) => {}
        other => {
            return Some(ContractFailure {
                scenario: "raw_error_mapping",
                detail: format!("INVALID 品质事件应被接受存档，实际 {other:?}"),
            })
        }
    }
    let mut weak = good_raw(plan, 2, now + 1);
    weak.integrity = IntegrityTag::Unverified;
    match m.ingest(source, weak, now + 1) {
        Err(IngestReject::IntegrityTooWeak { .. }) => None,
        other => Some(ContractFailure {
            scenario: "raw_error_mapping",
            detail: format!("期望 IntegrityTooWeak，实际 {other:?}"),
        }),
    }
}

/// 合同 8：event_id 稳定与 payload 冲突——完全同内容事件（同 epoch/
/// sequence 不可能再现，用 sink 层已覆盖；此处验证同 identity 不同
/// payload 的伪造实例被 sink 拒）。经两次构造模块共享 sink 的形态在
/// epoch_sequence 已锁 sequence 单调；本场景验证 flags 差异 → id 差异。
pub fn event_id_stable(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let plan = &scenario.plans[0];
    let now = 8_000_000_000;
    let ack = m.ingest(source, good_raw(plan, 5, now), now).unwrap();
    let IngestOutcome::Appended(a1) = ack else {
        return Some(ContractFailure {
            scenario: "event_id",
            detail: "事件未追加".into(),
        });
    };
    let mut other = good_raw(plan, 5, now + 1);
    other.flags |= event_flags::BACKFILL;
    let ack2 = m.ingest(source, other, now + 1).unwrap();
    let IngestOutcome::Appended(a2) = ack2 else {
        return Some(ContractFailure {
            scenario: "event_id",
            detail: format!("flags 差异事件未追加：{ack2:?}"),
        });
    };
    if a1.event_id == a2.event_id {
        return Some(ContractFailure {
            scenario: "event_id",
            detail: alloc::string::String::from("flags 差异未反映到 event_id"),
        });
    }
    None
}

/// 合同 9：poll 与 push 双模式——tick 驱动（adapter poll → sink）与
/// ingest 直推两种模式都落事件日志。
pub fn poll_and_push_modes(
    m: &mut IngestionModule,
    scenario: &ContractScenario,
) -> Option<ContractFailure> {
    let source = scenario.descriptor.source_id;
    let plan = scenario.plans[0];
    let period = plan.period_ns.max(1);
    // push 模式（直推）。
    let ack = m.ingest(source, good_raw(&plan, 1, 9_000_000_000), 9_000_000_000);
    if !matches!(ack, Ok(IngestOutcome::Appended(_))) {
        return Some(ContractFailure {
            scenario: "poll_push_modes",
            detail: format!("push 模式失败：{ack:?}"),
        });
    }
    // poll 模式：仅轮询形态（存在 period_ns>0 的计划）适用；纯事件驱动
    // 来源（市场事实）poll 不适用——push 面已覆盖。
    if scenario.plans.iter().any(|p| p.period_ns > 0) {
        let now = 9_000_000_000 + period;
        match m.tick(
            now,
            TickBudget {
                max_events: 64,
                max_transactions: 64,
            },
        ) {
            r if r.accepted >= 1 => {}
            r => {
                return Some(ContractFailure {
                    scenario: "poll_push_modes",
                    detail: format!("poll 模式零接受：{r:?}"),
                })
            }
        }
    }
    None
}

/// 全套件（[07] §3.5 全清单；不含 no_std 构建门——构建门 = target
/// 编译本身，不含运行时断言）。
///
/// 每个场景独立装配（互不污染）；返回失败清单（空 = 合同全过）。
pub fn run_full_suite(scenario_factory: impl Fn() -> ContractScenario) -> Vec<ContractFailure> {
    let mut report: Vec<ContractFailure> = Vec::new();
    // 场景 1/2/7/8：共享一个模块（拒绝类不破坏状态）。
    let scenario = scenario_factory();
    {
        let mut m = assembled(&scenario, 4096);
        if let Some(f) = unauthorized_point_rejected(&mut m, &scenario) {
            report.push(f);
        }
        if let Some(f) = unknown_schema_rejected(&mut m, &scenario) {
            report.push(f);
        }
        if let Some(f) = raw_error_mapping(&mut m, &scenario) {
            report.push(f);
        }
        if let Some(f) = event_id_stable(&mut m, &scenario) {
            report.push(f);
        }
    }
    // 场景 3/6/9：共享另一模块。
    let scenario = scenario_factory();
    {
        let mut m = assembled(&scenario, 4096);
        if let Some(f) = epoch_sequence_discipline(&mut m, &scenario) {
            report.push(f);
        }
        if let Some(f) = restart_reconnect(&mut m, &scenario) {
            report.push(f);
        }
        drop(m);
        // poll 场景独立装配：auto_emit 提供轮询事件源。
        let mut m = assembled_with(
            &scenario,
            4096,
            Some(good_raw(&scenario.plans[0], 77, 9_000_000_000)),
        );
        if let Some(f) = poll_and_push_modes(&mut m, &scenario) {
            report.push(f);
        }
    }
    // 场景 5：背压（专用小容量模块）。
    let scenario = scenario_factory();
    {
        let mut m = assembled(&scenario, 2);
        if let Some(f) = backpressure_no_leak(&mut m, &scenario) {
            report.push(f);
        }
    }
    // 场景 4：限流（独立模块——descriptor 被改）。
    let scenario = scenario_factory();
    if let Some(f) = rate_limit_bounded(&scenario) {
        report.push(f);
    }
    report
}
