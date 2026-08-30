//! 蓝图 61 [07] §3.5/§3.7（v0.314.31）：市场事实 adapter 合同合规。
//!
//! `MarketEventAdapter` 的准入面（descriptor/点计划/module）与
//! Modbus/IEC104 走同一套 adapter 无关合同（[07] §3.7：IngestionModule
//! public API 未为某协议新增特例——本测试与 modbus/iec104 侧的合同
//! 测试共用同一 suite 实现）。

use eneros_ingestion_core::contract::{run_full_suite, ContractScenario};
use eneros_ingestion_core::{EventQos, OperationalProfile, PointPlan, SourceDescriptor};

fn market_scenario() -> ContractScenario {
    ContractScenario {
        descriptor: SourceDescriptor {
            source_id: 5,
            tenant_id: 1,
            site_id: 1,
            allow_control: false,
            require_integrity: true,
            point_range: (0, 0),
            max_events_per_s: 1000,
            allowed_schemas: vec![],
            clock_id: 3,
            enabled: true,
        },
        plans: vec![PointPlan {
            source_id: 5,
            point_id: 9001,
            device_id: 0,
            period_ns: 0, // 事件驱动（市场事实不轮询）
            phase_offset_ns: 0,
            deadline_ns: 0,
            deadband_abs: 0.0,
            deadband_rel: 0.0,
            qos: EventQos::Q2,
            suppressible: true,
            timeout_ns: 0,
            max_retries: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 1,
            scale: 1.0,
            offset: 0.0,
            func_code: 0,
            reg_addr: 0,
            slave_addr: 0,
        }],
        profile: OperationalProfile::reference(),
        budget: eneros_ingestion_core::TickBudget {
            max_events: 64,
            max_transactions: 64,
        },
    }
}

#[test]
fn market_ingest_passes_full_contract_suite() {
    let failures = run_full_suite(market_scenario);
    assert!(failures.is_empty(), "市场事实接入合同失败：{failures:?}");
}
