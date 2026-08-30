//! 集成测试共享 DTO：最小资源注册表视图（绑定来自 profile 常数——与
//! 迁移期 `ProfileResourceView` 同构；生产装配注入 resource-registry
//! `PlannerView`，见 [08] §4.8）。

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
