//! 集成测试共享 DTO：最小资源注册表视图（绑定来自 profile 常数——与
//! 迁移期 `ProfileResourceView` 同构；生产装配注入 resource-registry
//! `PlannerView`，见 [08] §4.8）+ WP-5.1 合同编译视图。

use eneros_control_dispatch::compiler::{MapContractView, PlanContract};
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

// ---------------------------------------------------------------------------
// WP-64.3.3.1 · DispatchConfig 测试夹具单源
// ---------------------------------------------------------------------------

use eneros_control_dispatch::intent::IntentStore;
use eneros_control_dispatch::{DeliveryChannel, DispatchConfig};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::ControlProfile;

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

/// DispatchConfig 测试夹具单源——蓝图 69.4 WP-69.4.4：主体委托 src 侧
/// [`eneros_control_dispatch::ConfigSeed`] 装配器（安全默认单源，由
/// support_defaults.rs 变异靶锁锁定）；本函数只补测试视图派生
/// （resources/contracts 由 seed.profile 派生——与各套件原字面同构：
/// asset = profile.device_id、catalog_version 1、合同可逆）。
pub fn test_config(seed: TestConfigSeed) -> DispatchConfig {
    let profile = seed.profile;
    eneros_control_dispatch::ConfigSeed {
        chain: seed.chain,
        key: seed.key,
        fencing_token: seed.fencing_token,
        issuer_service: seed.issuer_service,
        now_fn: seed.now_fn,
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
        profile,
    }
    .assemble()
}
