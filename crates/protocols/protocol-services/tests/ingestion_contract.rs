//! 蓝图 61 [07] §3.5/§3.7（v0.314.31）：Modbus TCP 接入合同合规。
//!
//! Modbus 形态（轮询 + 注册 schema + 有界点域）走完整 adapter 无关
//! 合同套件——与市场事实/IEC104 共用同一 suite 实现（[07] §3.7：
//! IngestionModule public API 未为某协议新增特例）。

use eneros_ingestion_core::contract::{run_full_suite, ContractScenario};
use eneros_ingestion_core::{EventQos, OperationalProfile, PointPlan, SourceDescriptor};

fn modbus_scenario() -> ContractScenario {
    ContractScenario {
        descriptor: SourceDescriptor {
            source_id: 2,
            tenant_id: 1,
            site_id: 1,
            allow_control: true,
            require_integrity: true,
            point_range: (1, 10),
            max_events_per_s: 1000,
            allowed_schemas: vec![1],
            clock_id: 1,
            enabled: true,
        },
        plans: (1u32..=8)
            .map(|i| PointPlan {
                source_id: 2,
                point_id: i,
                device_id: 2,
                period_ns: 1_000_000_000,
                phase_offset_ns: 0,
                deadline_ns: 2_000_000_000,
                deadband_abs: 0.1,
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
                scale: 0.1,
                offset: 0.0,
                func_code: 3,
                reg_addr: 0x0100 + (i - 1) as u16,
                slave_addr: 2,
            })
            .collect(),
        profile: OperationalProfile::reference(),
        budget: eneros_ingestion_core::TickBudget {
            max_events: 256,
            max_transactions: 16,
        },
    }
}

#[test]
fn modbus_ingest_passes_full_contract_suite() {
    let failures = run_full_suite(modbus_scenario);
    assert!(failures.is_empty(), "Modbus 接入合同失败：{failures:?}");
}

/// W3 行为等价锁（蓝图 62 卷 62.3：跨实例回退/重放拦截——InMemorySink
/// 实现面）：同源更低 epoch 拒（水位不降级）、同 epoch 旧 seq 拒、
/// 首见即钉扎。tracker 定义锁在 ingestion-core（watermark_tracker_tests）。
#[test]
fn contract_sink_blocks_cross_instance_epoch_regression() {
    use eneros_ingestion_core::{InMemorySink, IngestReject, ObservationSink};
    use eneros_telemetry_model::event::{IntegrityTag, ObservationEvent, ObservationValue};

    let mk = |epoch: u64, seq: u64, v: f64| ObservationEvent {
        tenant_id: 1,
        site_id: 1,
        source_id: 2,
        source_epoch: epoch,
        sequence: seq,
        point_id: 1,
        value: ObservationValue::F64(v),
        event_time_ns: 1_000,
        ingest_time_ns: 1_000,
        quality_bits: 0,
        integrity: IntegrityTag::LocalTrustedEdge,
        flags: 0,
        schema_id: 1,
        schema_version: 1,
        unit_code: 0,
        asset_id: 1,
        mapping_version: 1,
        device_id: 1,
        clock_id: 1,
    };

    let mut sink = InMemorySink::new(64);
    sink.append(&mk(5, 10, 1.0)).expect("首帧入流");
    // 同源更低 epoch（重放/回滚）→ EpochRegression，且水位不降级。
    assert_eq!(
        sink.append(&mk(4, 99, 2.0)).unwrap_err(),
        IngestReject::EpochRegression {
            epoch: 4,
            expected: 5
        }
    );
    assert_eq!(
        sink.append(&mk(3, 1, 3.0)).unwrap_err(),
        IngestReject::EpochRegression {
            epoch: 3,
            expected: 5
        }
    );
    // 同 epoch 旧/等 seq → SequenceReplay。
    assert_eq!(
        sink.append(&mk(5, 9, 4.0)).unwrap_err(),
        IngestReject::SequenceReplay {
            sequence: 9,
            last: 10
        }
    );
    // 首见即钉扎（watermark 读面 = tracker.pinned 单源）。
    assert_eq!(sink.watermark(2), Some((5, 10)));
    // 新 epoch（adapter 重启形态）放行，钉扎推进后旧 epoch 永拒。
    sink.append(&mk(6, 1, 5.0)).expect("新 epoch 入流");
    assert!(matches!(
        sink.append(&mk(5, 999, 6.0)).unwrap_err(),
        IngestReject::EpochRegression { .. }
    ));
}
