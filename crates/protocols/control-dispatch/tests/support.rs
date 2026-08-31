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
