//! 蓝图 61 v0.314.31（[07] §3.5）：IEC104 接入 adapter 合同套件。
//!
//! adapter 无关合同（[`eneros_ingestion_core::contract::run_full_suite`]）
//! 以 IEC104 形态的 descriptor/PointPlan 装配：事件驱动型协议同过
//! 来源越权/未知 schema/epoch/sequence/限流/背压/重启重连/错误映射/
//! event_id/poll+push 全套判据——IngestionModule public API 未为协议
//! 新增特例（[07] §3.7：seam 泛化性证明）。
//!
//! 合同套件经 [`ScriptedAdapter`] 脚本化推 raw（与 modbus 合同测试同形
//! ——真实收包路径的协议面由 `iec104_ingest` 单元测试与 E5 host 互操作
//! 覆盖；本套件锁的是 module 合同在 IEC104 形态下的纪律）。

use eneros_ingestion_core::contract::{run_full_suite, ContractScenario};
use eneros_ingestion_core::{
    EventQos, IngestionAdapter, OperationalProfile, PointPlan, PollBudget, PollOutcome,
    PushOutcome, RawObservation, RawObservationSink, TickBudget,
};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

/// IEC104 形态 descriptor（[02] §4.2 最小封闭集；source_id/point_range/
/// schema 与 46 L2 闭环量级一致）。
fn iec104_descriptor() -> eneros_ingestion_core::SourceDescriptor {
    eneros_ingestion_core::SourceDescriptor {
        source_id: 3,
        tenant_id: 1,
        site_id: 1,
        allow_control: true,
        require_integrity: true,
        point_range: (1, 100),
        max_events_per_s: 100_000,
        allowed_schemas: vec![1],
        clock_id: 1,
        enabled: true,
    }
}

/// IEC104 形态点计划（事件驱动形态以最小周期表达——见 iec104_ingest
/// 模块注：module 调度面只驱动 period>0 点；协议本体事件驱动）。
fn iec104_plan(point_id: u32) -> PointPlan {
    PointPlan {
        source_id: 3,
        point_id,
        device_id: 1,
        period_ns: 1_000_000_000,
        phase_offset_ns: 0,
        deadline_ns: 2_000_000_000,
        deadband_abs: 0.0,
        deadband_rel: 0.0,
        qos: EventQos::Q1,
        suppressible: false,
        timeout_ns: 1_000_000_000,
        max_retries: 2,
        schema_id: 1,
        schema_version: 1,
        unit_code: 0,
        asset_id: 1,
        mapping_version: 1,
        scale: 1.0,
        offset: 0.0,
        // 协议组计划字段仅 Modbus 消费——iec104 形态填 0。
        func_code: 0,
        reg_addr: 0,
        slave_addr: 0,
    }
}

/// 脚本化 IEC104 形态 adapter（每轮推一批原始观测；模拟收包 drain 产物
/// ——品质/时标路径已在 iec104_ingest 单元测试锁定，此处验证 module 合同）。
struct ScriptedAdapter {
    source_id: u32,
    batch: std::vec::Vec<RawObservation>,
}

impl IngestionAdapter for ScriptedAdapter {
    fn source_id(&self) -> u32 {
        self.source_id
    }
    fn poll(
        &mut self,
        _now_ns: u64,
        _budget: PollBudget,
        _work: &[PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> PollOutcome {
        let mut events = 0u32;
        for r in self.batch.drain(..) {
            match out.push(r) {
                PushOutcome::Backpressured | PushOutcome::Rejected => break,
                _ => events += 1,
            }
        }
        PollOutcome::Completed {
            events,
            transactions: 1,
        }
    }
}

/// 合同场景工厂（每场景独立装配——互不污染）。
fn scenario() -> ContractScenario {
    ContractScenario {
        descriptor: iec104_descriptor(),
        plans: vec![iec104_plan(1), iec104_plan(2), iec104_plan(3)],
        profile: OperationalProfile::reference(),
        budget: TickBudget {
            max_events: 64,
            max_transactions: 16,
        },
    }
}

/// [07] §3.5 全清单：合同套件空报告（全过）。
#[test]
fn iec104_adapter_passes_full_contract_suite() {
    let failures = run_full_suite(scenario);
    assert!(
        failures.is_empty(),
        "IEC104 形态合同套件必须全过：{failures:#?}"
    );
}

/// 补充判据：poll 模式下脚本 adapter 的 IEC104 形态观测经 module 富化
/// 进入事件日志（三遥测点全落，accepted ≥ 3）。
#[test]
fn iec104_poll_mode_enriches_and_appends() {
    // 独立装配（不复用套件模块——场景内聚）。
    use eneros_ingestion_core::{InMemorySink, IngestionModule, MemEpochStore};
    let s = scenario();
    let source = s.descriptor.source_id;
    let mut m = IngestionModule::new(
        Box::new(InMemorySink::new(64)),
        Box::new(MemEpochStore::default()),
        s.profile,
    );
    m.register(s.descriptor.clone());
    m.open(source, 0).unwrap();
    m.install_points(1, s.plans.clone()).unwrap();
    let batch: std::vec::Vec<RawObservation> = (1..=3)
        .map(|p| RawObservation {
            point_id: p,
            value: ObservationValue::F64(f64::from(p) * 10.0),
            event_time_ns: Some(1_000_000_000),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 0,
            device_id: 1,
        })
        .collect();
    m.attach(Box::new(ScriptedAdapter {
        source_id: source,
        batch,
    }))
    .unwrap();
    let report = m.tick(
        9_000_000_000,
        TickBudget {
            max_events: 64,
            max_transactions: 16,
        },
    );
    assert_eq!(report.accepted, 3, "三遥测点全进事件日志");
    assert_eq!(report.rejected, 0);
    assert_eq!(report.backpressured, 0);
}
