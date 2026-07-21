//! IEC 61850 逻辑设备（LD，Logical Device）.
//!
//! LD 为 IED 内的逻辑设备分组单元，聚合若干逻辑节点（LN）。

use alloc::string::String;
use alloc::vec::Vec;

use crate::ln::LogicalNode;

/// 逻辑设备（IEC 61850 LD）。
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalDevice {
    /// LD 名（`{ied_name}_{ld_inst}`，如 "IED1_LD0"）。
    pub ld_name: String,
    /// 引用名（与 `ld_name` 一致）。
    pub ref_name: String,
    /// 逻辑节点列表（D3：`Vec` 替代 SlotMap）。
    pub lns: Vec<LogicalNode>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;
    use alloc::vec::Vec;

    use super::*;
    use crate::ln::LnClass;

    fn make_ln(class: LnClass, inst: u16) -> LogicalNode {
        LogicalNode {
            ln_class: class,
            ln_inst: inst,
            ln_prefix: String::new(),
            do_list: Vec::new(),
        }
    }

    fn make_ld(name: &str) -> LogicalDevice {
        LogicalDevice {
            ld_name: String::from(name),
            ref_name: String::from(name),
            lns: Vec::new(),
        }
    }

    // ===== LD1：LogicalDevice 构造（字段正确）=====
    #[test]
    fn test_ld1_construction() {
        let ld = make_ld("IED1_LD0");
        assert_eq!(ld.ld_name, "IED1_LD0");
        assert_eq!(ld.ref_name, "IED1_LD0");
        assert!(ld.lns.is_empty());
    }

    // ===== LD2：lns push 与 len =====
    #[test]
    fn test_ld2_lns_push_len() {
        let mut ld = make_ld("IED1_LD0");
        ld.lns.push(make_ln(LnClass::XCBR, 1));
        ld.lns.push(make_ln(LnClass::MMXU, 2));
        assert_eq!(ld.lns.len(), 2);
        assert_eq!(ld.lns[0].ln_class, LnClass::XCBR);
        assert_eq!(ld.lns[1].ln_class, LnClass::MMXU);
        assert_eq!(ld.lns[1].ln_inst, 2);
    }

    // ===== LD3：Debug / Clone 派生 =====
    #[test]
    fn test_ld3_debug_clone() {
        let mut ld = make_ld("IED1_LD0");
        ld.lns.push(make_ln(LnClass::PTRC, 1));
        let cloned = ld.clone();
        assert_eq!(ld, cloned);
        let dbg = alloc::format!("{:?}", ld);
        assert!(dbg.contains("IED1_LD0"));
        assert!(dbg.contains("PTRC"));
    }

    // ===== LD4：空 lns 边界 =====
    #[test]
    fn test_ld4_empty_lns() {
        let ld = make_ld("E");
        assert!(ld.lns.is_empty());
    }
}
