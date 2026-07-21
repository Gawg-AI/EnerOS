//! UPA 点表数据库.
//!
//! [`PointDatabase`] 维护 `BTreeMap` 主存储与 device/type/name 三级索引，
//! 支持注册/更新/按 ID/设备/类型/名称查询/删除/计数/全量列举。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::point::{DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue};

/// 点表数据库（多索引）。
///
/// - `points`：主存储 `PointId -> DataPoint`
/// - `device_index`：`DeviceId -> Vec<PointId>`
/// - `type_index`：`PointType -> Vec<PointId>`
/// - `name_index`：`String -> PointId`
///
/// 不内置 `RwLock`（D2，no_std 单线程）；`next_id` 为普通 `u32` 自增字段（D3）。
#[derive(Debug)]
pub struct PointDatabase {
    points: BTreeMap<PointId, DataPoint>,
    device_index: BTreeMap<DeviceId, Vec<PointId>>,
    type_index: BTreeMap<PointType, Vec<PointId>>,
    name_index: BTreeMap<String, PointId>,
    next_id: u32,
}

impl PointDatabase {
    /// 创建空数据库。
    pub fn new() -> Self {
        Self {
            points: BTreeMap::new(),
            device_index: BTreeMap::new(),
            type_index: BTreeMap::new(),
            name_index: BTreeMap::new(),
            next_id: 0,
        }
    }

    /// 注册新点，返回分配的全局唯一 `PointId`（u32 自增）。
    ///
    /// 初始值 `PointValue::Null`，品质 `PointQuality::invalid()`，
    /// 来源 `DataSource::Internal`，时间戳取 `now_ms`（D1/D9）。
    /// 同名注册会覆盖 `name_index` 指向（旧点仍在主存储中保留）。
    pub fn register(
        &mut self,
        device_id: DeviceId,
        name: &str,
        point_type: PointType,
        now_ms: u64,
    ) -> PointId {
        let id = self.next_id;
        self.next_id += 1;
        let point = DataPoint {
            point_id: id,
            device_id,
            name: name.to_string(),
            description: None,
            point_type,
            value: PointValue::Null,
            quality: PointQuality::invalid(),
            timestamp_ms: now_ms,
            source: DataSource::Internal,
            unit: None,
        };
        self.device_index.entry(device_id).or_default().push(id);
        self.type_index.entry(point_type).or_default().push(id);
        self.name_index.insert(name.to_string(), id);
        self.points.insert(id, point);
        id
    }

    /// 更新点值/品质/时间戳。
    ///
    /// 仅修改 `value`/`quality`/`timestamp_ms`，不改 `point_id`/`device_id`/
    /// `name`/`point_type`。点不存在时返回 `false`（D9：`now_ms` 参数注入时间戳）。
    pub fn update(
        &mut self,
        point_id: PointId,
        value: PointValue,
        quality: PointQuality,
        now_ms: u64,
    ) -> bool {
        if let Some(point) = self.points.get_mut(&point_id) {
            point.value = value;
            point.quality = quality;
            point.timestamp_ms = now_ms;
            true
        } else {
            false
        }
    }

    /// 按 ID 查询。
    pub fn get_by_id(&self, point_id: PointId) -> Option<&DataPoint> {
        self.points.get(&point_id)
    }

    /// 按设备查询，返回该设备下所有点（按注册顺序）。
    pub fn get_by_device(&self, device_id: DeviceId) -> Vec<&DataPoint> {
        self.device_index
            .get(&device_id)
            .map(|ids| ids.iter().filter_map(|id| self.points.get(id)).collect())
            .unwrap_or_default()
    }

    /// 按类型查询，返回该类型所有点（按注册顺序）。
    pub fn get_by_type(&self, point_type: PointType) -> Vec<&DataPoint> {
        self.type_index
            .get(&point_type)
            .map(|ids| ids.iter().filter_map(|id| self.points.get(id)).collect())
            .unwrap_or_default()
    }

    /// 按名称查询（精确匹配）。
    pub fn get_by_name(&self, name: &str) -> Option<&DataPoint> {
        self.name_index.get(name).and_then(|id| self.points.get(id))
    }

    /// 删除点，同步清理主存储与所有索引。
    ///
    /// 返回是否删除成功。`name_index` 仅在仍指向该 `point_id` 时才移除
    /// （避免误删同名后注册覆盖的索引项）。
    pub fn remove(&mut self, point_id: PointId) -> bool {
        let point = match self.points.remove(&point_id) {
            Some(p) => p,
            None => return false,
        };
        let device_id = point.device_id;
        let point_type = point.point_type;
        let name = point.name;

        if let Some(ids) = self.device_index.get_mut(&device_id) {
            ids.retain(|&id| id != point_id);
        }
        if let Some(ids) = self.type_index.get_mut(&point_type) {
            ids.retain(|&id| id != point_id);
        }
        if self.name_index.get(&name) == Some(&point_id) {
            self.name_index.remove(&name);
        }
        true
    }

    /// 返回点总数。
    pub fn count(&self) -> usize {
        self.points.len()
    }

    /// 返回所有点（按 `PointId` 升序，由 `BTreeMap` 保证）。
    pub fn list_all(&self) -> Vec<&DataPoint> {
        self.points.values().collect()
    }
}

impl Default for PointDatabase {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point::{PointQuality, PointType, PointValue};

    #[test]
    fn test_new_empty() {
        let db = PointDatabase::new();
        assert_eq!(db.count(), 0);
        assert!(db.list_all().is_empty());
    }

    #[test]
    fn test_register_and_get_by_id() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "temp", PointType::Analog, 1000);
        assert_eq!(id, 0);
        let p = db.get_by_id(id).expect("should exist");
        assert_eq!(p.device_id, 1);
        assert_eq!(p.name, "temp");
        assert_eq!(p.point_type, PointType::Analog);
        assert_eq!(p.value, PointValue::Null);
        assert_eq!(p.quality, PointQuality::invalid());
        assert_eq!(p.timestamp_ms, 1000);
        assert_eq!(p.source, DataSource::Internal);
        assert!(p.description.is_none());
        assert!(p.unit.is_none());
    }

    #[test]
    fn test_update_existing_and_missing() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "v", PointType::Analog, 100);
        assert!(db.update(id, PointValue::Float(1.5), PointQuality::good(), 200));
        let p = db.get_by_id(id).expect("exists");
        assert_eq!(p.value, PointValue::Float(1.5));
        assert_eq!(p.quality, PointQuality::good());
        assert_eq!(p.timestamp_ms, 200);
        // immutable fields unchanged
        assert_eq!(p.point_id, id);
        assert_eq!(p.name, "v");
        // missing point
        assert!(!db.update(999, PointValue::Null, PointQuality::good(), 300));
    }

    #[test]
    fn test_get_by_device_multiple() {
        let mut db = PointDatabase::new();
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        db.register(2, "c", PointType::Analog, 0);
        assert_eq!(db.get_by_device(1).len(), 2);
        assert_eq!(db.get_by_device(2).len(), 1);
        assert_eq!(db.get_by_device(3).len(), 0);
    }

    #[test]
    fn test_get_by_type_filter() {
        let mut db = PointDatabase::new();
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        db.register(1, "c", PointType::Analog, 0);
        assert_eq!(db.get_by_type(PointType::Analog).len(), 2);
        assert_eq!(db.get_by_type(PointType::Digital).len(), 1);
        assert_eq!(db.get_by_type(PointType::Control).len(), 0);
    }

    #[test]
    fn test_get_by_name_exact() {
        let mut db = PointDatabase::new();
        db.register(1, "temp", PointType::Analog, 0);
        assert!(db.get_by_name("temp").is_some());
        assert!(db.get_by_name("missing").is_none());
    }

    #[test]
    fn test_remove_cleans_indices() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "x", PointType::Analog, 0);
        assert!(db.remove(id));
        assert!(db.get_by_id(id).is_none());
        assert!(db.get_by_device(1).is_empty());
        assert!(db.get_by_type(PointType::Analog).is_empty());
        assert!(db.get_by_name("x").is_none());
    }

    #[test]
    fn test_remove_missing() {
        let mut db = PointDatabase::new();
        assert!(!db.remove(999));
    }

    #[test]
    fn test_count_and_list_all() {
        let mut db = PointDatabase::new();
        assert_eq!(db.count(), 0);
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        assert_eq!(db.count(), 2);
        let all = db.list_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].point_id, 0);
        assert_eq!(all[1].point_id, 1);
    }

    #[test]
    fn test_default() {
        let db = PointDatabase::default();
        assert_eq!(db.count(), 0);
    }

    #[test]
    fn test_duplicate_name_overwrites_index() {
        let mut db = PointDatabase::new();
        let id0 = db.register(1, "dup", PointType::Analog, 0);
        let id1 = db.register(2, "dup", PointType::Digital, 0);
        // name_index points to latest registration
        assert_eq!(db.get_by_name("dup").map(|p| p.point_id), Some(id1));
        // both points still in main storage
        assert!(db.get_by_id(id0).is_some());
        assert!(db.get_by_id(id1).is_some());
        assert_eq!(db.count(), 2);
        // removing the latest should clear name_index (it pointed to id1)
        assert!(db.remove(id1));
        assert!(db.get_by_name("dup").is_none());
        // old point still exists
        assert!(db.get_by_id(id0).is_some());
        // removing the old (name_index no longer points to it) should not panic
        assert!(db.remove(id0));
        assert!(db.get_by_name("dup").is_none());
    }
}
