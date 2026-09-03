//! 集成测试共享 DTO：最小资源注册表视图（绑定来自 profile 常数——与
//! 迁移期 `ProfileResourceView` 同构；生产装配注入 resource-registry
//! `PlannerView`，见 [08] §4.8）+ WP-5.1 合同编译视图。

use eneros_control_dispatch::compiler::{CompilerPolicy, MapContractView, PlanContract};
use eneros_decision_contract::ResourceRegistry;

/// 测试视图（asset=设备位、device/point/unit 由调用方给）。
pub struct TestResourceView {
    pub asset_id: u64,
    pub device_id: u16,
    pub point_id: u32,
    pub unit_code: u16,
    pub catalog_version: u64,
}

impl ResourceRegistry for TestResourceView {
    fn asset_exists(&self, asset_id: u64) -> bool {
        asset_id == self.asset_id
    }

    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
        if asset_id == self.asset_id {
            Some((self.device_id, self.point_id, self.unit_code))
        } else {
            None
        }
    }

    fn catalog_version(&self) -> u64 {
        self.catalog_version
    }
}

/// 测试合同视图（WP-5.1）：单资产 setpoint 合同（类 2 = setpoint、
/// 值域 ±500、可逆、回读点 1001——与 configs/catalog.toml contract 1
/// 同形）。`reversible=0` 变体供不可逆语义用例。
pub fn test_contract_view(asset_id: u64, reversible: u8) -> MapContractView {
    let mut v = MapContractView::new();
    v.insert(
        asset_id,
        PlanContract {
            class: 2,
            readback_point_id: 1001,
            reversible,
            min: -500.0,
            max: 500.0,
            settle_ms: 0,
            compensable: false,
        },
    );
    v
}

/// 测试编译策略（默认：串行、不合并、最短窗 1s）。
pub fn test_compiler_policy() -> CompilerPolicy {
    CompilerPolicy::default()
}

// ---------------------------------------------------------------------------
// WP-64.3.3.1 · DispatchConfig 测试夹具单源
// ---------------------------------------------------------------------------

use eneros_control_dispatch::arbiter::ControlMode;
use eneros_control_dispatch::intent::IntentStore;
use eneros_control_dispatch::{DeliveryChannel, DispatchConfig, FeedbackPointMapping};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::ControlProfile;
use eneros_dual_brain::gate::DecisionGate;

/// 套件身份组：每套件**刻意隔离**的本地事实（KEY/ISSUER/chain/profile/
/// clock 各文件本地常量）+ 两个注入面。其余字段由 [`test_config`] 安全
/// 默认填充；套件差异经结构更新语法只声明所变字段。
pub struct TestConfigSeed {
    pub chain: ControlChain,
    pub key: Vec<u8>,
    pub profile: ControlProfile,
    pub issuer_service: [u8; 16],
    pub fencing_token: u64,
    pub now_fn: fn() -> u64,
    pub intent_log: Box<dyn IntentStore>,
    pub delivery: Box<dyn DeliveryChannel>,
}

/// DispatchConfig 测试夹具单源——安全默认 = settlement_lock base_config
/// 公共值（护栏窗/爬坡/限功率/gate/feedback_mapping/target_endpoint）；
/// resources/contracts 由 seed.profile 派生（与各套件原字面同构：
/// asset = profile.device_id、catalog_version 1、合同可逆）。
pub fn test_config(seed: TestConfigSeed) -> DispatchConfig {
    let profile = seed.profile;
    DispatchConfig {
        chain: seed.chain,
        key: seed.key,
        profile,
        fencing_token: seed.fencing_token,
        issuer_service: seed.issuer_service,
        issuer_epoch: 0,
        target_endpoint: String::from("svc-device-agent"),
        max_power_kw: 500.0,
        safety_voltage_window: (340.0, 420.0),
        safety_freq_window: (49.5, 50.5),
        safety_ramp_kw_per_min: 200.0,
        gate: DecisionGate::new(100, 1000.0, 3_600_000),
        audit: None,
        control_audit: None,
        intent_log: seed.intent_log,
        delivery: seed.delivery,
        resources: Box::new(TestResourceView {
            asset_id: u64::from(profile.device_id),
            device_id: profile.device_id as u16,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        contracts: Box::new(test_contract_view(u64::from(profile.device_id), 1)),
        compiler_policy: test_compiler_policy(),
        control_mode: ControlMode::Auto,
        feedback_mapping: FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 6,
        },
        now_fn: seed.now_fn,
    }
}
