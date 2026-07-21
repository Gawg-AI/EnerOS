//! 安全默认值表.
//!
//! [`SafeDefaults`] 存储 `PointId → f64` 映射，在 `SafeDefault` 模式下
//! 由引擎遍历下发到协议层。

use alloc::collections::BTreeMap;

use eneros_upa_model::PointId;

/// 安全默认值表（PointId → 值）.
///
/// 使用 `BTreeMap` 保证遍历顺序确定（D4 有序），便于测试断言。
#[derive(Debug, Clone, Default)]
pub struct SafeDefaults {
    map: BTreeMap<PointId, f64>,
}

impl SafeDefaults {
    /// 创建空表.
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入/覆盖安全默认值.
    pub fn insert(&mut self, point_id: PointId, value: f64) {
        self.map.insert(point_id, value);
    }

    /// 查询指定点的安全默认值.
    pub fn get(&self, point_id: PointId) -> Option<f64> {
        self.map.get(&point_id).copied()
    }

    /// 遍历所有安全默认值（按 PointId 升序）.
    pub fn iter(&self) -> impl Iterator<Item = (PointId, f64)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 条目数.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}
