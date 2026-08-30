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
