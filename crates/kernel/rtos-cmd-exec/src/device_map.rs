//! DevicePointMap — controlbus::DeviceId → upa_model::PointId 映射（D4）.

use alloc::collections::BTreeMap;

use eneros_controlbus::DeviceId;
use eneros_upa_model::PointId;

/// 设备→点映射表.
///
/// 蓝图 `cmd.to_point_writes()` 不存在（D4）；本结构将控制总线的
/// `DeviceId`（u32 newtype）映射到统一点表的 `PointId`（u32），
/// 供 [`crate::executor::CommandExecutor`] 下发写入.
#[derive(Debug, Clone, Default)]
pub struct DevicePointMap {
    map: BTreeMap<u32, PointId>,
}

impl DevicePointMap {
    /// 创建空映射表.
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入/覆盖设备→点映射.
    pub fn insert(&mut self, device_id: DeviceId, point_id: PointId) {
        self.map.insert(device_id.0, point_id);
    }

    /// 查询设备对应的点 ID.
    pub fn get(&self, device_id: DeviceId) -> Option<PointId> {
        self.map.get(&device_id.0).copied()
    }

    /// 遍历所有设备→点映射（v0.57.0 新增，供降级引擎遍历下发）.
    pub fn iter(&self) -> impl Iterator<Item = (DeviceId, PointId)> + '_ {
        self.map.iter().map(|(&k, &v)| (DeviceId(k), v))
    }
}
