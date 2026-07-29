//! seL4 用户态 API 兼容层（v0.156.0 用户态兼容性验证）。
//!
//! 验证 seL4 深度定制配置变更后 Phase 1-2 用户态代码的兼容性，
//! 确保无需修改即可运行（方案 A 下 seL4 内核保留，用户态无需迁移）。
//!
//! ## 模块结构
//!
//! - [`types`]：seL4 ABI 兼容数据类型（MessageInfo/CPtr/Rights/CompatError）
//! - [`translation`]：syscall 翻译表（seL4 编号 → 自研内核 SyscallNum）
//! - [`cspace`]：CSpace 映射器（seL4 CPtr → 自研内核 CapSlot）
//! - [`shim`]：用户态垫片库（Sel4Shim trait + Sel4Compat 主结构体）
//!
//! ## 架构设计（蓝图 §5 技术交底）
//!
//! 采用"垫片库（shim）+ syscall 翻译"双层方案：
//! - 垫片在编译时替换 seL4 头文件依赖
//! - syscall 翻译在运行时将 seL4 语义映射到自研内核
//!
//! ## ADR-0001 定位
//!
//! 本版本原为自研内核迁移组件，已按方案 A 调整为 seL4 深度定制/
//! 信创认证路径。seL4 内核保留，用户态无需"迁移"，本模块改为
//! "seL4 深度定制配置变更后的用户态兼容性验证"。
//!
//! ## 偏差/决策记录
//!
//! - D1：蓝图路径 `kernel/src/compat/` → `crates/kernel/microkernel/
//!   src/compat/`（仓库规范 §2.3.1 强制子系统归口）。
//! - D2：蓝图 `userlib/src/sel4_shim.rs` 独立 crate → 合并为
//!   [`shim::Sel4Shim`] trait（本仓库无 userlib crate；用户态链接
//!   替换通过 feature flag `sel4-compat` 门控，集成层在 main.rs
//!   中接线）。
//! - D3：蓝图全局 `#[no_mangle] sel4_syscall_handler` → 结构体方法
//!   [`shim::Sel4Compat::handle_syscall`]（lib crate 全局 no_mangle
//!   与 host cargo test 链接冲突；语义等价）。
//! - D4：蓝图 `BTreeMap` → 定长数组（no_std 无 alloc，同仓库 D2
//!   先例链）。
//! - D5：v0.155.0 既有 [`Sel4CompatLayer`] trait 保留为契约接口
//!   （向后兼容），新增 [`shim::Sel4Compat`] 为完整实现。

pub mod cspace;
pub mod shim;
pub mod translation;
pub mod types;

// 重导出核心类型。
pub use cspace::{CSpaceMapper, MAX_CSPACE_ENTRIES};
pub use shim::{CompatStats, KernelSyscallOps, Sel4Compat, Sel4Shim};
pub use translation::SyscallTranslator;
pub use types::{CPtr, CompatError, MessageInfo, Rights};

use crate::error::KernelError;
use crate::syscall::SyscallNum;

/// seL4 用户态 API 兼容层契约 trait（v0.155.0 既有，保留向后兼容）。
///
/// 差异聚合语义：seL4 线程创建需 Retype(TCB) + TCB_Configure 两次 syscall，
/// 本内核 ThreadCreate 单 syscall 聚合完成；兼容层负责把 seL4 调用序列
/// 翻译/聚合为本内核 syscall。映射事实表见 `docs/kernel/sel4_compat.md`。
///
/// v0.156.0 起完整实现见 [`shim::Sel4Compat`]。
pub trait Sel4CompatLayer {
    /// 将 seL4 syscall 编号翻译为本内核 [`SyscallNum`]；无对应关系返回 `None`。
    fn translate_syscall(&self, sel4_num: u16, args: &[usize]) -> Option<SyscallNum>;

    /// 线程创建聚合 shim：seL4 Retype+Configure 两步 → 单 syscall。
    fn shim_thread_create(&self, name: &str, entry: usize) -> Result<usize, KernelError>;
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// 翻译命中/未命中双路径 mock：仅识别 seL4 编号 1（示意 TCB 创建序列），
    /// 其余编号一律返回 `None`。
    struct MockTranslateLayer;

    impl Sel4CompatLayer for MockTranslateLayer {
        fn translate_syscall(&self, sel4_num: u16, _args: &[usize]) -> Option<SyscallNum> {
            if sel4_num == 1 {
                Some(SyscallNum::ThreadCreate)
            } else {
                None
            }
        }

        fn shim_thread_create(&self, _name: &str, _entry: usize) -> Result<usize, KernelError> {
            Ok(7)
        }
    }

    /// 聚合 shim 失败路径 mock：固定返回 [`KernelError::NotSupported`]。
    struct MockUnsupportedLayer;

    impl Sel4CompatLayer for MockUnsupportedLayer {
        fn translate_syscall(&self, _sel4_num: u16, _args: &[usize]) -> Option<SyscallNum> {
            None
        }

        fn shim_thread_create(&self, _name: &str, _entry: usize) -> Result<usize, KernelError> {
            Err(KernelError::NotSupported)
        }
    }

    /// CPT1：translate_syscall 命中返回 Some(ThreadCreate)，未命中返回 None。
    #[test]
    fn cpt1_translate_hit_and_miss() {
        let layer = MockTranslateLayer;
        let args: [usize; 2] = [0x1000, 0x2000];

        // 命中路径：seL4 编号 1 映射为本内核 ThreadCreate。
        assert_eq!(
            layer.translate_syscall(1, &args),
            Some(SyscallNum::ThreadCreate)
        );

        // 未命中路径：无对应关系的编号返回 None。
        assert_eq!(layer.translate_syscall(0xFF, &args), None);
    }

    /// CPT2：shim_thread_create 成功路径返回 Ok(tid)。
    #[test]
    fn cpt2_shim_thread_create_ok() {
        let layer = MockTranslateLayer;
        let tid = layer.shim_thread_create("worker", 0x4008_0000);
        assert_eq!(tid, Ok(7));
    }

    /// CPT3：shim_thread_create 失败路径返回 Err(NotSupported)。
    #[test]
    fn cpt3_shim_thread_create_not_supported() {
        let layer = MockUnsupportedLayer;
        let ret = layer.shim_thread_create("worker", 0x4008_0000);
        assert_eq!(ret, Err(KernelError::NotSupported));
    }
}
