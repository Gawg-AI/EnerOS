//! IEC 61850 数据对象（DO，Data Object）与数据属性（DA，Data Attribute）.
//!
//! DO 为 LN 内的数据分组（如 "Pos"/"Beh"/"W"），DA 为叶子属性（如 "stVal"/"q"/"t"）。

use alloc::string::String;
use alloc::vec::Vec;

/// 数据对象（IEC 61850 DO）。
#[derive(Debug, Clone, PartialEq)]
pub struct DataObject {
    /// DO 名（如 "Pos"、"Beh"、"StVal"）。
    pub do_name: String,
    /// 数据属性列表。
    pub da_list: Vec<DataAttribute>,
    /// 公共数据类。
    pub cdc: CommonDataClass,
}

/// 公共数据类（CDC，Common Data Class）。
#[derive(Debug, Clone, PartialEq)]
pub enum CommonDataClass {
    /// 单点状态。
    SPS,
    /// 双点状态。
    DPS,
    /// 测量值。
    MV,
    /// 枚举状态。
    ENS,
    /// 保护激活。
    ACT,
    /// 设定值。
    ASG,
    /// 其他/未识别 CDC。
    Other(String),
}

/// 数据属性（IEC 61850 DA）。
#[derive(Debug, Clone, PartialEq)]
pub struct DataAttribute {
    /// DA 名（如 "stVal"、"q"、"t"）。
    pub da_name: String,
    /// 功能约束。
    pub fc: FunctionalConstraint,
    /// 当前值。
    pub value: DaValue,
    /// 品质标志。
    pub quality: Quality,
    /// 时间戳（毫秒，参数注入，与 v0.50.0 D1 一致）。
    pub timestamp: u64,
}

/// 功能约束（FC，Functional Constraint，D7：未知 FC 解析期报错）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FunctionalConstraint {
    /// 状态。
    ST,
    /// 测量。
    MX,
    /// 控制。
    CO,
    /// 设定值。
    SP,
    /// 设定组。
    SG,
    /// 服务响应。
    SE,
    /// 缓存报告。
    BR,
    /// 对象引用。
    OR,
}

/// DA 值（D11：仅派生 `PartialEq`，不派生 `Eq`，因含 `f32`/`f64`）。
#[derive(Debug, Clone, PartialEq)]
pub enum DaValue {
    /// 布尔值。
    Bool(bool),
    /// 32 位整数。
    Int32(i32),
    /// 32 位浮点。
    Float32(f32),
    /// 64 位浮点。
    Float64(f64),
    /// 枚举值。
    Enum(u16),
    /// 字符串。
    StringVal(String),
    /// 时间戳。
    Timestamp(u64),
}

/// 品质标志（IEC 61850 Quality 子集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    /// 有效性。
    pub validity: Validity,
    /// 来源。
    pub source: Source,
    /// 测试标志。
    pub test: bool,
    /// 操作员闭锁。
    pub operator_blocked: bool,
}

/// 品质有效性。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Validity {
    /// 有效。
    Good,
    /// 无效。
    Invalid,
    /// 保留。
    Reserved,
    /// 可疑。
    Questionable,
}

/// 品质来源。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// 过程值。
    Process,
    /// 替代值。
    Substituted,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;
    use alloc::vec::Vec;

    use super::*;

    fn good_quality() -> Quality {
        Quality {
            validity: Validity::Good,
            source: Source::Process,
            test: false,
            operator_blocked: false,
        }
    }

    // ===== DD11：DaValue 7 种变体构造 =====
    #[test]
    fn test_dd11_da_value_variants() {
        let vals = [
            DaValue::Bool(true),
            DaValue::Int32(-5),
            DaValue::Float32(1.5),
            DaValue::Float64(2.5),
            DaValue::Enum(3),
            DaValue::StringVal(String::from("s")),
            DaValue::Timestamp(123_456),
        ];
        assert_eq!(vals.len(), 7);
        assert_eq!(vals[0], DaValue::Bool(true));
        assert_eq!(vals[1], DaValue::Int32(-5));
        assert_eq!(vals[2], DaValue::Float32(1.5));
        assert_eq!(vals[3], DaValue::Float64(2.5));
        assert_eq!(vals[4], DaValue::Enum(3));
        assert_eq!(vals[5], DaValue::StringVal(String::from("s")));
        assert_eq!(vals[6], DaValue::Timestamp(123_456));
    }

    // ===== DD12：PartialEq — 同变体同值相等 / 同变体异值不等 =====
    #[test]
    fn test_dd12_partial_eq_same_variant() {
        assert_eq!(DaValue::Bool(true), DaValue::Bool(true));
        assert_ne!(DaValue::Bool(true), DaValue::Bool(false));
        assert_eq!(DaValue::Int32(1), DaValue::Int32(1));
        assert_ne!(DaValue::Int32(1), DaValue::Int32(2));
        assert_eq!(DaValue::Float64(2.5), DaValue::Float64(2.5));
        assert_ne!(DaValue::Float32(1.0), DaValue::Float32(2.0));
        assert_eq!(
            DaValue::StringVal(String::from("a")),
            DaValue::StringVal(String::from("a"))
        );
    }

    // ===== DD13：PartialEq — 不同变体不等 =====
    #[test]
    fn test_dd13_partial_eq_different_variants() {
        assert_ne!(DaValue::Int32(1), DaValue::Bool(true));
        assert_ne!(DaValue::Enum(1), DaValue::Int32(1));
        assert_ne!(DaValue::Timestamp(1), DaValue::Int32(1));
        assert_ne!(DaValue::Float32(1.0), DaValue::Float64(1.0));
        assert_ne!(DaValue::StringVal(String::from("1")), DaValue::Int32(1));
    }

    // ===== DD14：Quality 字段 =====
    #[test]
    fn test_dd14_quality_fields() {
        let q = Quality {
            validity: Validity::Invalid,
            source: Source::Substituted,
            test: true,
            operator_blocked: true,
        };
        assert_eq!(q.validity, Validity::Invalid);
        assert_eq!(q.source, Source::Substituted);
        assert!(q.test);
        assert!(q.operator_blocked);
        let g = good_quality();
        assert_eq!(g.validity, Validity::Good);
        assert!(!g.test);
        assert!(!g.operator_blocked);
    }

    // ===== DD15：Validity 4 变体互异 =====
    #[test]
    fn test_dd15_validity_variants() {
        let vs = [
            Validity::Good,
            Validity::Invalid,
            Validity::Reserved,
            Validity::Questionable,
        ];
        assert_eq!(vs.len(), 4);
        for (i, a) in vs.iter().enumerate() {
            for (j, b) in vs.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    // ===== DD16：Source 2 变体互异 =====
    #[test]
    fn test_dd16_source_variants() {
        assert_eq!(Source::Process, Source::Process);
        assert_eq!(Source::Substituted, Source::Substituted);
        assert_ne!(Source::Process, Source::Substituted);
    }

    // ===== DD17：FunctionalConstraint 8 变体（Copy 语义）=====
    #[test]
    fn test_dd17_fc_variants() {
        let fcs = [
            FunctionalConstraint::ST,
            FunctionalConstraint::MX,
            FunctionalConstraint::CO,
            FunctionalConstraint::SP,
            FunctionalConstraint::SG,
            FunctionalConstraint::SE,
            FunctionalConstraint::BR,
            FunctionalConstraint::OR,
        ];
        assert_eq!(fcs.len(), 8);
        let a = FunctionalConstraint::ST;
        let b = a; // Copy
        assert_eq!(a, b);
        for (i, x) in fcs.iter().enumerate() {
            for (j, y) in fcs.iter().enumerate() {
                if i == j {
                    assert_eq!(x, y);
                } else {
                    assert_ne!(x, y);
                }
            }
        }
    }

    // ===== DD18：CommonDataClass 变体 =====
    #[test]
    fn test_dd18_cdc_variants() {
        let cdcs = [
            CommonDataClass::SPS,
            CommonDataClass::DPS,
            CommonDataClass::MV,
            CommonDataClass::ENS,
            CommonDataClass::ACT,
            CommonDataClass::ASG,
            CommonDataClass::Other(String::from("X")),
        ];
        assert_eq!(cdcs.len(), 7);
        assert_ne!(CommonDataClass::SPS, CommonDataClass::DPS);
        assert_eq!(
            CommonDataClass::Other(String::from("X")),
            CommonDataClass::Other(String::from("X"))
        );
        assert_ne!(
            CommonDataClass::Other(String::from("SPS")),
            CommonDataClass::SPS
        );
    }

    // ===== DD19：DataObject 构造 =====
    #[test]
    fn test_dd19_data_object_construction() {
        let mut data_obj = DataObject {
            do_name: String::from("Pos"),
            da_list: Vec::new(),
            cdc: CommonDataClass::DPS,
        };
        assert_eq!(data_obj.do_name, "Pos");
        assert_eq!(data_obj.cdc, CommonDataClass::DPS);
        assert!(data_obj.da_list.is_empty());
        data_obj.da_list.push(DataAttribute {
            da_name: String::from("stVal"),
            fc: FunctionalConstraint::ST,
            value: DaValue::Bool(true),
            quality: good_quality(),
            timestamp: 0,
        });
        assert_eq!(data_obj.da_list.len(), 1);
        assert_eq!(data_obj.da_list[0].da_name, "stVal");
    }

    // ===== DD20：DataAttribute 构造（含时间戳）+ Debug/Clone =====
    #[test]
    fn test_dd20_data_attribute_construction() {
        let da = DataAttribute {
            da_name: String::from("stVal"),
            fc: FunctionalConstraint::ST,
            value: DaValue::Int32(1),
            quality: good_quality(),
            timestamp: 1_700_000_000_000,
        };
        assert_eq!(da.da_name, "stVal");
        assert_eq!(da.fc, FunctionalConstraint::ST);
        assert_eq!(da.value, DaValue::Int32(1));
        assert_eq!(da.quality, good_quality());
        assert_eq!(da.timestamp, 1_700_000_000_000);
        let cloned = da.clone();
        assert_eq!(da, cloned);
        let dbg = alloc::format!("{:?}", da);
        assert!(dbg.contains("stVal"));
        assert!(dbg.contains("Int32"));
    }
}
