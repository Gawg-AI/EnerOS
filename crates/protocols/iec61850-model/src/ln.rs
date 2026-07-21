//! IEC 61850 逻辑节点（LN，Logical Node）.
//!
//! LN 为功能语义单元（断路器 XCBR、测量 MMXU 等），聚合数据对象（DO）。

use alloc::string::String;
use alloc::vec::Vec;

use crate::do_da::DataObject;

/// LN 类别（IEC 61850-7-4 常用类 + 扩展 Other）。
#[derive(Debug, Clone, PartialEq)]
pub enum LnClass {
    /// 断路器。
    XCBR,
    /// 测量。
    MMXU,
    /// 保护跳闸。
    PTRC,
    /// 开关控制。
    CSWI,
    /// 通用 IO。
    GGIO,
    /// 其他/厂商自定义类（如 LLN0）。
    Other(String),
}

/// 逻辑节点（IEC 61850 LN）。
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalNode {
    /// LN 类别。
    pub ln_class: LnClass,
    /// 实例号（LLN0 固定为 0，D8）。
    pub ln_inst: u16,
    /// 前缀（可空）。
    pub ln_prefix: String,
    /// 数据对象列表。
    pub do_list: Vec<DataObject>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;
    use alloc::vec::Vec;

    use super::*;
    use crate::do_da::CommonDataClass;

    fn make_ln(class: LnClass, inst: u16, prefix: &str) -> LogicalNode {
        LogicalNode {
            ln_class: class,
            ln_inst: inst,
            ln_prefix: String::from(prefix),
            do_list: Vec::new(),
        }
    }

    fn make_do(name: &str) -> DataObject {
        DataObject {
            do_name: String::from(name),
            da_list: Vec::new(),
            cdc: CommonDataClass::DPS,
        }
    }

    // ===== LN5：LnClass 全部变体可构造且互异 =====
    #[test]
    fn test_ln5_ln_class_variants() {
        let classes = [
            LnClass::XCBR,
            LnClass::MMXU,
            LnClass::PTRC,
            LnClass::CSWI,
            LnClass::GGIO,
            LnClass::Other(String::from("LLN0")),
        ];
        assert_eq!(classes.len(), 6);
        assert_ne!(LnClass::XCBR, LnClass::MMXU);
        assert_ne!(LnClass::PTRC, LnClass::CSWI);
        assert_ne!(LnClass::GGIO, LnClass::Other(String::from("GGIO")));
    }

    // ===== LN6：LogicalNode 构造（字段正确）=====
    #[test]
    fn test_ln6_construction() {
        let ln = make_ln(LnClass::XCBR, 1, "");
        assert_eq!(ln.ln_class, LnClass::XCBR);
        assert_eq!(ln.ln_inst, 1);
        assert_eq!(ln.ln_prefix, "");
        assert!(ln.do_list.is_empty());
    }

    // ===== LN7：实例号 + 前缀 =====
    #[test]
    fn test_ln7_inst_and_prefix() {
        let ln = make_ln(LnClass::CSWI, 3, "Q");
        assert_eq!(ln.ln_inst, 3);
        assert_eq!(ln.ln_prefix, "Q");
    }

    // ===== LN8：do_list push 与 len =====
    #[test]
    fn test_ln8_do_list_push() {
        let mut ln = make_ln(LnClass::MMXU, 1, "");
        ln.do_list.push(make_do("W"));
        ln.do_list.push(make_do("V"));
        assert_eq!(ln.do_list.len(), 2);
        assert_eq!(ln.do_list[0].do_name, "W");
        assert_eq!(ln.do_list[1].do_name, "V");
    }

    // ===== LN9：Debug / Clone / PartialEq 派生 =====
    #[test]
    fn test_ln9_debug_clone_eq() {
        let mut ln = make_ln(LnClass::XCBR, 1, "");
        ln.do_list.push(make_do("Pos"));
        let cloned = ln.clone();
        assert_eq!(ln, cloned);
        let dbg = alloc::format!("{:?}", ln);
        assert!(dbg.contains("XCBR"));
        assert!(dbg.contains("Pos"));
        let other = make_ln(LnClass::XCBR, 2, "");
        assert_ne!(ln, other);
    }

    // ===== LN10：Other(String) 相等性 =====
    #[test]
    fn test_ln10_other_equality() {
        assert_eq!(
            LnClass::Other(String::from("LLN0")),
            LnClass::Other(String::from("LLN0"))
        );
        assert_ne!(
            LnClass::Other(String::from("A")),
            LnClass::Other(String::from("B"))
        );
        assert_ne!(LnClass::Other(String::from("XCBR")), LnClass::XCBR);
    }
}
