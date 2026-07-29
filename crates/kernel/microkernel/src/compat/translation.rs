//! seL4 syscall 翻译层（v0.156.0 用户态兼容性验证）。
//!
//! 实现 [`SyscallTranslator`]：将 seL4 syscall 编号翻译为自研内核
//! [`SyscallNum`]，并处理参数翻译。翻译表为静态数组，O(1) 查找。
//!
//! 蓝图 phase3.md §v0.156.0 §4.2 接口定义原样实现。
//!
//! 翻译规则（蓝图 §5 技术交底）：
//! - seL4 树形 CSpace → 自研内核扁平能力表（由 [`super::cspace`] 映射）
//! - seL4 MR（消息寄存器）→ 自研内核连续缓冲区（翻译层做转换）
//! - seL4 Retype+TCB_Configure 两步 → 自研内核 ThreadCreate 单 syscall 聚合

use super::types::sel4_syscall_nums::*;
use super::types::CompatError;
use crate::error::KernelError;
use crate::syscall::SyscallNum;

/// syscall 翻译表条目：(seL4 编号, 自研内核编号)。
///
/// 未列出的 seL4 编号翻译为 `None`（不支持）。
const TRANSLATION_TABLE: [(u32, SyscallNum); 20] = [
    // IPC 类
    (SEL4_SEND, SyscallNum::IpcSend),
    (SEL4_RECV, SyscallNum::IpcRecv),
    (SEL4_CALL, SyscallNum::IpcCall),
    (SEL4_REPLY, SyscallNum::IpcSend),      // Reply 语义降级为 Send
    (SEL4_REPLY_RECV, SyscallNum::IpcRecv), // ReplyRecv 降级为 Recv
    (SEL4_NBSEND, SyscallNum::IpcSend),     // NBSend 降级为 Send
    (SEL4_NBRECV, SyscallNum::IpcRecv),     // NBRecv 降级为 Recv
    // 通知类
    (SEL4_WAIT, SyscallNum::WaitFor),
    (SEL4_SIGNAL, SyscallNum::Notify),
    // 调度类
    (SEL4_YIELD, SyscallNum::Yield),
    // 能力类
    (SEL4_MINT, SyscallNum::CapInsert), // Mint → CapInsert（派生新能力插入目标槽）
    (SEL4_MOVE, SyscallNum::CapMove),
    (SEL4_COPY, SyscallNum::CapInsert), // Copy → CapInsert（副本插入目标槽）
    (SEL4_DELETE, SyscallNum::CapDelete),
    // 线程类（聚合语义）
    (SEL4_TCB_CONFIGURE, SyscallNum::ThreadCreate), // Retype+Configure 聚合
    (SEL4_TCB_SET_PRIORITY, SyscallNum::TcbConfigure),
    (SEL4_TCB_RESUME, SyscallNum::ThreadCreate), // Resume 降级（骨架）
    (SEL4_TCB_SUSPEND, SyscallNum::ThreadDelete), // Suspend 降级（骨架）
    // 内存类（兼容层跳过，内核自动管理）
    (SEL4_RETYPE, SyscallNum::MapPages), // Retype → MapPages（骨架映射）
    (SEL4_UNTYPED_RETYPE, SyscallNum::MapPages), // 同上
];

/// seL4 syscall 翻译器。
///
/// 维护静态翻译表，将 seL4 syscall 编号翻译为自研内核
/// [`SyscallNum`]。翻译为 O(1) 线性扫描（表项 ≤32，缓存友好）。
///
/// 蓝图 §4.2：`table: [(u32, u32); 32]` 原样实现（类型安全化
/// 为 `SyscallNum` 枚举替代裸 u32）。
pub struct SyscallTranslator {
    /// 翻译表：(seL4 syscall 号, 自研内核 SyscallNum)。
    table: [(u32, SyscallNum); TABLE_SIZE],
    /// 有效条目数。
    count: usize,
}

impl SyscallTranslator {
    /// 创建翻译器，加载默认翻译表。
    pub fn new() -> Self {
        let mut table = [(0u32, SyscallNum::Yield); TABLE_SIZE];
        let count = TRANSLATION_TABLE.len();
        // 复制有效条目到定长表。
        let mut i = 0;
        while i < count {
            table[i] = TRANSLATION_TABLE[i];
            i += 1;
        }
        Self { table, count }
    }

    /// 将 seL4 syscall 编号翻译为自研内核 [`SyscallNum`]。
    ///
    /// 无对应关系返回 `None`（蓝图 §4.4：记录警告日志，返回 ENOSYS）。
    pub fn translate(&self, sel4_syscall: u32) -> Option<SyscallNum> {
        let mut i = 0;
        while i < self.count {
            if self.table[i].0 == sel4_syscall {
                return Some(self.table[i].1);
            }
            i += 1;
        }
        None
    }

    /// 翻译参数（seL4 参数布局 → 自研内核参数布局）。
    ///
    /// 当前为直通模式（参数顺序一致），后续可按 syscall 类型
    /// 做差异化翻译（如 MR ↔ buffer 转换）。
    pub fn translate_args(&self, args: &[usize]) -> Result<[usize; 6], CompatError> {
        let mut translated = [0usize; 6];
        let copy_len = if args.len() > 6 { 6 } else { args.len() };
        let mut i = 0;
        while i < copy_len {
            translated[i] = args[i];
            i += 1;
        }
        Ok(translated)
    }

    /// 翻译返回值（自研内核返回值 → seL4 返回值格式）。
    ///
    /// 当前为直通模式，后续可按需做错误码映射。
    pub fn translate_ret(&self, ret: Result<usize, KernelError>) -> Result<usize, CompatError> {
        match ret {
            Ok(v) => Ok(v),
            Err(KernelError::InvalidCapability) => Err(CompatError::InvalidCapability),
            Err(KernelError::Timeout) => Err(CompatError::Timeout),
            Err(KernelError::NotSupported) => Err(CompatError::UnsupportedSyscall),
            // 其余错误统一映射为 TranslationFailed。
            Err(_) => Err(CompatError::TranslationFailed),
        }
    }

    /// 判定指定 seL4 syscall 是否被兼容层支持。
    pub fn is_supported(&self, sel4_syscall: u32) -> bool {
        self.translate(sel4_syscall).is_some()
    }

    /// 获取翻译表有效条目数。
    pub const fn entry_count(&self) -> usize {
        self.count
    }
}

impl Default for SyscallTranslator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// TR1：IPC 类 syscall 翻译正确。
    #[test]
    fn tr1_ipc_translation() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate(SEL4_SEND), Some(SyscallNum::IpcSend));
        assert_eq!(t.translate(SEL4_RECV), Some(SyscallNum::IpcRecv));
        assert_eq!(t.translate(SEL4_CALL), Some(SyscallNum::IpcCall));
    }

    /// TR2：通知类 syscall 翻译正确。
    #[test]
    fn tr2_notification_translation() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate(SEL4_WAIT), Some(SyscallNum::WaitFor));
        assert_eq!(t.translate(SEL4_SIGNAL), Some(SyscallNum::Notify));
    }

    /// TR3：能力类 syscall 翻译正确。
    #[test]
    fn tr3_capability_translation() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate(SEL4_MINT), Some(SyscallNum::CapInsert));
        assert_eq!(t.translate(SEL4_MOVE), Some(SyscallNum::CapMove));
        assert_eq!(t.translate(SEL4_COPY), Some(SyscallNum::CapInsert));
        assert_eq!(t.translate(SEL4_DELETE), Some(SyscallNum::CapDelete));
    }

    /// TR4：调度类 syscall 翻译正确。
    #[test]
    fn tr4_schedule_translation() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate(SEL4_YIELD), Some(SyscallNum::Yield));
    }

    /// TR5：线程类 syscall 聚合翻译正确。
    #[test]
    fn tr5_thread_translation() {
        let t = SyscallTranslator::new();
        assert_eq!(
            t.translate(SEL4_TCB_CONFIGURE),
            Some(SyscallNum::ThreadCreate)
        );
        assert_eq!(
            t.translate(SEL4_TCB_SET_PRIORITY),
            Some(SyscallNum::TcbConfigure)
        );
    }

    /// TR6：不支持的 syscall 返回 None。
    #[test]
    fn tr6_unsupported_returns_none() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate(0), None);
        assert_eq!(t.translate(99), None);
        assert_eq!(t.translate(0xFFFF), None);
    }

    /// TR7：translate_args 直通模式正确。
    #[test]
    fn tr7_translate_args_passthrough() {
        let t = SyscallTranslator::new();
        let args = [1usize, 2, 3, 4, 5, 6, 7, 8];
        let result = t.translate_args(&args).unwrap();
        // 仅取前 6 个。
        assert_eq!(result, [1, 2, 3, 4, 5, 6]);
    }

    /// TR8：translate_args 短参数补零。
    #[test]
    fn tr8_translate_args_short() {
        let t = SyscallTranslator::new();
        let args = [42usize, 99];
        let result = t.translate_args(&args).unwrap();
        assert_eq!(result, [42, 99, 0, 0, 0, 0]);
    }

    /// TR9：translate_ret 成功直通。
    #[test]
    fn tr9_translate_ret_ok() {
        let t = SyscallTranslator::new();
        assert_eq!(t.translate_ret(Ok(7)), Ok(7));
    }

    /// TR10：translate_ret 错误映射。
    #[test]
    fn tr10_translate_ret_error_mapping() {
        let t = SyscallTranslator::new();
        assert_eq!(
            t.translate_ret(Err(KernelError::InvalidCapability)),
            Err(CompatError::InvalidCapability)
        );
        assert_eq!(
            t.translate_ret(Err(KernelError::Timeout)),
            Err(CompatError::Timeout)
        );
        assert_eq!(
            t.translate_ret(Err(KernelError::NotSupported)),
            Err(CompatError::UnsupportedSyscall)
        );
        assert_eq!(
            t.translate_ret(Err(KernelError::OutOfMemory)),
            Err(CompatError::TranslationFailed)
        );
    }

    /// TR11：is_supported 判定正确。
    #[test]
    fn tr11_is_supported() {
        let t = SyscallTranslator::new();
        assert!(t.is_supported(SEL4_SEND));
        assert!(t.is_supported(SEL4_YIELD));
        assert!(!t.is_supported(0));
        assert!(!t.is_supported(100));
    }

    /// TR12：entry_count 返回有效条目数。
    #[test]
    fn tr12_entry_count() {
        let t = SyscallTranslator::new();
        assert_eq!(t.entry_count(), TRANSLATION_TABLE.len());
    }
}
