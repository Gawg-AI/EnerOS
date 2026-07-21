//! GOOSE 数据集：条目（path + DaValue）容器.
//!
//! `set` 语义：已有 path 覆盖 value（不新增），新 path 追加（保序）；
//! `get` 按 path 查找返回条目引用。DaValue 复用 eneros-iec61850-model。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_iec61850_model::DaValue;

/// GOOSE 数据集（发布/订阅共用载体，插入序）。
#[derive(Debug, Clone, PartialEq)]
pub struct GooseDataset {
    /// 条目列表（保序）。
    pub entries: Vec<GooseEntry>,
}

/// GOOSE 数据集条目（路径 + 值）。
#[derive(Debug, Clone, PartialEq)]
pub struct GooseEntry {
    /// 路径（如 "IED1LD/LLN0.Pos.stVal"；rx 侧无路径语义，置空字符串）。
    pub path: String,
    /// 值。
    pub value: DaValue,
}

impl GooseDataset {
    /// 创建空数据集。
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 设置条目：已有 path 覆盖 value，新 path 追加（保序）。
    pub fn set(&mut self, path: &str, value: DaValue) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
            entry.value = value;
        } else {
            self.entries.push(GooseEntry {
                path: String::from(path),
                value,
            });
        }
    }

    /// 按 path 查找条目。
    pub fn get(&self, path: &str) -> Option<&GooseEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

impl Default for GooseDataset {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;

    use super::*;

    // ===== DS1：new 空数据集 =====
    #[test]
    fn test_ds1_new_empty_dataset() {
        let ds = GooseDataset::new();
        assert!(ds.entries.is_empty());
        let ds2 = GooseDataset::default();
        assert_eq!(ds, ds2);
    }

    // ===== DS2：set 追加新路径 =====
    #[test]
    fn test_ds2_set_appends_new_path() {
        let mut ds = GooseDataset::new();
        ds.set("IED1LD/LLN0.Pos.stVal", DaValue::Bool(true));
        assert_eq!(ds.entries.len(), 1);
        assert_eq!(ds.entries[0].path, "IED1LD/LLN0.Pos.stVal");
        assert_eq!(ds.entries[0].value, DaValue::Bool(true));
        ds.set("IED1LD/MMXU1.Hz.mag", DaValue::Float32(50.0));
        assert_eq!(ds.entries.len(), 2);
        assert_eq!(ds.entries[1].path, "IED1LD/MMXU1.Hz.mag");
    }

    // ===== DS3：set 覆盖已有路径（不新增）=====
    #[test]
    fn test_ds3_set_overwrites_existing_path() {
        let mut ds = GooseDataset::new();
        ds.set("A", DaValue::Int32(1));
        ds.set("B", DaValue::Int32(2));
        ds.set("A", DaValue::Int32(10));
        assert_eq!(ds.entries.len(), 2);
        assert_eq!(ds.entries[0].value, DaValue::Int32(10));
        assert_eq!(ds.entries[1].value, DaValue::Int32(2));
    }

    // ===== DS4：get 命中 =====
    #[test]
    fn test_ds4_get_hit() {
        let mut ds = GooseDataset::new();
        ds.set("XCBR1.Pos.stVal", DaValue::Bool(false));
        let entry = ds.get("XCBR1.Pos.stVal").unwrap();
        assert_eq!(entry.value, DaValue::Bool(false));
        assert_eq!(entry.path, "XCBR1.Pos.stVal");
    }

    // ===== DS5：get miss → None =====
    #[test]
    fn test_ds5_get_miss_returns_none() {
        let mut ds = GooseDataset::new();
        ds.set("A", DaValue::Int32(1));
        assert!(ds.get("B").is_none());
        assert!(GooseDataset::new().get("A").is_none());
    }

    // ===== DS6：多 DaValue 变体存储 + Clone/PartialEq =====
    #[test]
    fn test_ds6_multi_variants_clone_eq() {
        let mut ds = GooseDataset::new();
        ds.set("b", DaValue::Bool(true));
        ds.set("i", DaValue::Int32(-7));
        ds.set("f32", DaValue::Float32(1.5));
        ds.set("f64", DaValue::Float64(2.5));
        ds.set("e", DaValue::Enum(3));
        ds.set("s", DaValue::StringVal(String::from("go")));
        ds.set("t", DaValue::Timestamp(123_456));
        assert_eq!(ds.entries.len(), 7);
        let cloned = ds.clone();
        assert_eq!(ds, cloned);
        assert_eq!(ds.get("f32").unwrap().value, DaValue::Float32(1.5));
        assert_eq!(
            ds.get("s").unwrap().value,
            DaValue::StringVal(String::from("go"))
        );
        assert_eq!(ds.get("t").unwrap().value, DaValue::Timestamp(123_456));
    }
}
