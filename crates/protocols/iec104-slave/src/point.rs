//! IEC 104 点数据库（D2）.
//!
//! `PointDatabase` trait 抽象点存储，`InMemoryPointDatabase` 提供内存实现。
//! 返回值使用 owned 类型（`IoValue` 为 `Copy`），避免从站在总召唤流程中
//! `&self.point_db` 与 `&mut self`（发送响应）的借用冲突。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::asdu::{Dco, IoValue, QualityDescriptor, Sco, SinglePointValue};

/// 点数据库 trait（D2）
///
/// 应用可插入自定义存储实现；`InMemoryPointDatabase` 供测试参考。
pub trait PointDatabase {
    /// 获取指定 IOA 的值（owned，`IoValue` 为 `Copy`）。
    fn get_value(&self, ioa: u16) -> Option<IoValue>;
    /// 获取指定 IOA 的品质；不存在时返回 `good()`。
    fn get_quality(&self, ioa: u16) -> QualityDescriptor;
    /// 设置指定 IOA 的值。
    fn set_value(&mut self, ioa: u16, value: IoValue);
    /// 设置指定 IOA 的品质。
    fn set_quality(&mut self, ioa: u16, quality: QualityDescriptor);
    /// 获取全部点（IOA, 值, 品质），按 IOA 升序。
    fn get_all_points(&self) -> Vec<(u16, IoValue, QualityDescriptor)>;
    /// 执行单点遥控命令。
    #[allow(clippy::result_unit_err)]
    fn execute_single_command(&mut self, ioa: u16, sco: &Sco) -> Result<(), ()>;
    /// 执行双点遥控命令。
    #[allow(clippy::result_unit_err)]
    fn execute_double_command(&mut self, ioa: u16, dco: &Dco) -> Result<(), ()>;
}

/// 内存点数据库实现
pub struct InMemoryPointDatabase {
    values: BTreeMap<u16, IoValue>,
    qualities: BTreeMap<u16, QualityDescriptor>,
}

impl InMemoryPointDatabase {
    /// 创建空数据库。
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
            qualities: BTreeMap::new(),
        }
    }
}

impl Default for InMemoryPointDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl PointDatabase for InMemoryPointDatabase {
    fn get_value(&self, ioa: u16) -> Option<IoValue> {
        self.values.get(&ioa).copied()
    }

    fn get_quality(&self, ioa: u16) -> QualityDescriptor {
        self.qualities
            .get(&ioa)
            .copied()
            .unwrap_or_else(QualityDescriptor::good)
    }

    fn set_value(&mut self, ioa: u16, value: IoValue) {
        self.values.insert(ioa, value);
        // 若品质未设置，补一个默认 good
        self.qualities
            .entry(ioa)
            .or_insert_with(QualityDescriptor::good);
    }

    fn set_quality(&mut self, ioa: u16, quality: QualityDescriptor) {
        self.qualities.insert(ioa, quality);
    }

    fn get_all_points(&self) -> Vec<(u16, IoValue, QualityDescriptor)> {
        let mut result = Vec::new();
        for (ioa, value) in &self.values {
            let q = self
                .qualities
                .get(ioa)
                .copied()
                .unwrap_or_else(QualityDescriptor::good);
            result.push((*ioa, *value, q));
        }
        // BTreeMap 已按 key 升序遍历
        result
    }

    fn execute_single_command(&mut self, ioa: u16, sco: &Sco) -> Result<(), ()> {
        // 仅执行模式（select=false）才实际改变值
        if sco.select {
            return Ok(());
        }
        let v = if sco.value {
            SinglePointValue::On
        } else {
            SinglePointValue::Off
        };
        self.values.insert(ioa, IoValue::SinglePoint(v));
        Ok(())
    }

    fn execute_double_command(&mut self, ioa: u16, dco: &Dco) -> Result<(), ()> {
        if dco.select {
            return Ok(());
        }
        self.values.insert(ioa, IoValue::DoublePoint(dco.value));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::DoublePointValue;

    #[test]
    fn test_set_get_value() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(1, IoValue::Float(1.5));
        let v = db.get_value(1).expect("should exist");
        assert!(matches!(v, IoValue::Float(f) if (f - 1.5).abs() < 1e-6));
    }

    #[test]
    fn test_get_value_not_found() {
        let db = InMemoryPointDatabase::new();
        assert!(db.get_value(999).is_none());
    }

    #[test]
    fn test_get_quality_default_good() {
        let db = InMemoryPointDatabase::new();
        assert_eq!(db.get_quality(1), QualityDescriptor::good());
    }

    #[test]
    fn test_set_get_quality() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(1, IoValue::Normalized(0));
        db.set_quality(
            1,
            QualityDescriptor {
                invalid: true,
                ..Default::default()
            },
        );
        let q = db.get_quality(1);
        assert!(q.invalid);
    }

    #[test]
    fn test_get_all_points_sorted() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(3, IoValue::Normalized(30));
        db.set_value(1, IoValue::Normalized(10));
        db.set_value(2, IoValue::Normalized(20));
        let all = db.get_all_points();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, 1);
        assert_eq!(all[1].0, 2);
        assert_eq!(all[2].0, 3);
    }

    #[test]
    fn test_get_all_points_empty() {
        let db = InMemoryPointDatabase::new();
        assert_eq!(db.get_all_points().len(), 0);
    }

    #[test]
    fn test_execute_single_command_on() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let sco = Sco::new(true);
        db.execute_single_command(10, &sco).expect("execute ok");
        let v = db.get_value(10).expect("should exist");
        assert_eq!(v, IoValue::SinglePoint(SinglePointValue::On));
    }

    #[test]
    fn test_execute_single_command_off() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::On));
        let sco = Sco::new(false);
        db.execute_single_command(10, &sco).expect("execute ok");
        let v = db.get_value(10).expect("should exist");
        assert_eq!(v, IoValue::SinglePoint(SinglePointValue::Off));
    }

    #[test]
    fn test_execute_single_command_select_no_change() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let sco = Sco {
            value: true,
            qu: 0,
            select: true,
        };
        db.execute_single_command(10, &sco).expect("select ok");
        // 选择阶段不改变值
        let v = db.get_value(10).expect("should exist");
        assert_eq!(v, IoValue::SinglePoint(SinglePointValue::Off));
    }

    #[test]
    fn test_execute_double_command_on() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(20, IoValue::DoublePoint(DoublePointValue::Off));
        let dco = Dco::new(DoublePointValue::On);
        db.execute_double_command(20, &dco).expect("execute ok");
        let v = db.get_value(20).expect("should exist");
        assert_eq!(v, IoValue::DoublePoint(DoublePointValue::On));
    }

    #[test]
    fn test_execute_double_command_select_no_change() {
        let mut db = InMemoryPointDatabase::new();
        db.set_value(20, IoValue::DoublePoint(DoublePointValue::Off));
        let dco = Dco {
            value: DoublePointValue::On,
            qu: 0,
            select: true,
        };
        db.execute_double_command(20, &dco).expect("select ok");
        let v = db.get_value(20).expect("should exist");
        assert_eq!(v, IoValue::DoublePoint(DoublePointValue::Off));
    }

    #[test]
    fn test_default() {
        let db = InMemoryPointDatabase::default();
        assert_eq!(db.get_all_points().len(), 0);
    }

    #[test]
    fn test_trait_object() {
        let db: Box<dyn PointDatabase> = Box::new(InMemoryPointDatabase::new());
        assert!(db.get_value(1).is_none());
    }
}
