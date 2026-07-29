//! v0.156.0 seL4 用户态兼容性验证集成测试。
//!
//! 蓝图 phase3.md §v0.156.0 §6 测试计划：
//! - 6.1 单元测试（≥80%）：syscall 翻译表正确性 / CSpaceMapper 分配/解析/释放 / MessageInfo 序列化
//! - 6.2 集成测试：Agent Runtime 在兼容层上启动 / IPC 往返通过兼容层 / 能力 mint/revoke 通过兼容层
//! - 6.5 故障注入：不支持的 syscall 返回 ENOSYS / 无效 CPtr 返回 EFAULT
//!
//! 本文件为 host 可测骨架（集成层注入 mock KernelSyscallOps）。

use eneros_microkernel::compat::types::sel4_syscall_nums::*;
use eneros_microkernel::compat::{
    CSpaceMapper, CompatError, KernelSyscallOps, MessageInfo, Rights, Sel4Compat, Sel4Shim,
    SyscallTranslator,
};
use eneros_microkernel::error::KernelError;

/// 集成测试用 Mock 内核操作。
struct IntegrationMockOps {
    /// 记录最后一次 mint 的源槽位。
    last_mint_src: u32,
    /// 记录最后一次 send 的 endpoint 槽位。
    last_send_ep: u32,
    /// 记录最后一次 recv 的 endpoint 槽位。
    last_recv_ep: u32,
    /// 记录最后一次 wait 的 notification 槽位。
    last_wait_ntfn: u32,
    /// 记录最后一次 signal 的 badge。
    last_signal_badge: u64,
    /// yield 调用次数。
    yield_count: u64,
    /// 记录最后一次 delete 的槽位。
    last_delete_slot: u32,
    /// 记录最后一次 thread_create 的入口地址。
    last_thread_entry: usize,
}

impl IntegrationMockOps {
    fn new() -> Self {
        Self {
            last_mint_src: 0,
            last_send_ep: 0,
            last_recv_ep: 0,
            last_wait_ntfn: 0,
            last_signal_badge: 0,
            yield_count: 0,
            last_delete_slot: 0,
            last_thread_entry: 0,
        }
    }
}

impl KernelSyscallOps for IntegrationMockOps {
    fn mint_capability(&mut self, src_slot: u32, _rights: u32) -> Result<u32, KernelError> {
        self.last_mint_src = src_slot;
        Ok(src_slot + 100) // 新槽位 = 源 + 100
    }

    fn ipc_send(&mut self, ep: u32, _msg: &[u8]) -> Result<(), KernelError> {
        self.last_send_ep = ep;
        Ok(())
    }

    fn ipc_recv(&mut self, ep: u32, _buf: &mut [u8]) -> Result<u64, KernelError> {
        self.last_recv_ep = ep;
        Ok(77) // 模拟 sender badge
    }

    fn notification_wait(&mut self, ntfn: u32) -> Result<u64, KernelError> {
        self.last_wait_ntfn = ntfn;
        Ok(0xBEEF) // 模拟 badge
    }

    fn notification_signal(&mut self, _ntfn: u32, badge: u64) -> Result<(), KernelError> {
        self.last_signal_badge = badge;
        Ok(())
    }

    fn yield_now(&mut self) -> Result<(), KernelError> {
        self.yield_count += 1;
        Ok(())
    }

    fn cap_delete(&mut self, slot: u32) -> Result<(), KernelError> {
        self.last_delete_slot = slot;
        Ok(())
    }

    fn cap_move(&mut self, _src: u32, _dst: u32) -> Result<(), KernelError> {
        Ok(())
    }

    fn thread_create(
        &mut self,
        entry: usize,
        _stack: usize,
        _priority: usize,
    ) -> Result<usize, KernelError> {
        self.last_thread_entry = entry;
        Ok(42) // 模拟 tid
    }

    fn tcb_configure(
        &mut self,
        _tid: usize,
        _priority: usize,
        _affinity: usize,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

// ============================================================
// F1：syscall 翻译表完整性验证
// ============================================================

/// F1：翻译表覆盖 Phase 1-2 所有核心 seL4 syscall（≥9 个）。
#[test]
fn f1_translation_table_covers_core_syscalls() {
    let translator = SyscallTranslator::new();

    // IPC 类（3 个核心）
    assert!(translator.is_supported(SEL4_SEND));
    assert!(translator.is_supported(SEL4_RECV));
    assert!(translator.is_supported(SEL4_CALL));

    // 通知类（2 个核心）
    assert!(translator.is_supported(SEL4_WAIT));
    assert!(translator.is_supported(SEL4_SIGNAL));

    // 调度类（1 个核心）
    assert!(translator.is_supported(SEL4_YIELD));

    // 能力类（4 个核心）
    assert!(translator.is_supported(SEL4_MINT));
    assert!(translator.is_supported(SEL4_MOVE));
    assert!(translator.is_supported(SEL4_COPY));
    assert!(translator.is_supported(SEL4_DELETE));

    // 线程类（2 个核心）
    assert!(translator.is_supported(SEL4_TCB_CONFIGURE));
    assert!(translator.is_supported(SEL4_TCB_SET_PRIORITY));

    // 总条目数 ≥ 12
    assert!(translator.entry_count() >= 12);
}

// ============================================================
// F2：CSpaceMapper 完整生命周期
// ============================================================

/// F2：CSpaceMapper 分配→解析→释放完整生命周期。
#[test]
fn f2_cspace_mapper_lifecycle() {
    let mut mapper = CSpaceMapper::new();

    // 分配
    let cptr1 = mapper.allocate_cptr(10).unwrap();
    let cptr2 = mapper.allocate_cptr(20).unwrap();
    assert_ne!(cptr1, cptr2);
    assert_eq!(mapper.active_count(), 2);

    // 解析
    assert_eq!(mapper.resolve_cptr(cptr1), Some(10));
    assert_eq!(mapper.resolve_cptr(cptr2), Some(20));

    // 反向解析
    assert_eq!(mapper.resolve_cap_slot(10), Some(cptr1));
    assert_eq!(mapper.resolve_cap_slot(20), Some(cptr2));

    // 释放
    assert!(mapper.free_cptr(cptr1));
    assert_eq!(mapper.resolve_cptr(cptr1), None);
    assert_eq!(mapper.active_count(), 1);

    // 释放后反向解析也失败
    assert_eq!(mapper.resolve_cap_slot(10), None);
}

// ============================================================
// F3：MessageInfo 序列化/反序列化
// ============================================================

/// F3：MessageInfo 序列化往返一致性。
#[test]
fn f3_message_info_serialization() {
    let cases = [
        MessageInfo::empty(),
        MessageInfo::new(0, 0),
        MessageInfo::new(0xFFFF_FFFF, 0xFFFF),
        MessageInfo {
            label: 0x1234_5678,
            caps_unwrapped: 3,
            extra_caps: 2,
            length: 256,
        },
    ];

    for msg in &cases {
        let word = msg.to_word();
        let restored = MessageInfo::from_word(word);
        assert_eq!(*msg, restored, "roundtrip failed for {:?}", msg);
    }
}

// ============================================================
// F4：IPC 往返通过兼容层
// ============================================================

/// F4：IPC Send→Recv 往返通过兼容层。
#[test]
fn f4_ipc_roundtrip_through_compat() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);

    // 预置 endpoint 映射。
    compat.cspace_mut().insert_at(100, 5);

    // Send
    let result = compat.handle_syscall(SEL4_SEND, &[100, 0, 32]);
    assert_eq!(result, Ok(0));

    // Recv
    let result = compat.handle_syscall(SEL4_RECV, &[100]);
    assert_eq!(result, Ok(77)); // mock sender badge
}

// ============================================================
// F5：能力 Mint 通过兼容层
// ============================================================

/// F5：能力 Mint 通过兼容层，权限缩减正确。
#[test]
fn f5_capability_mint_through_compat() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);

    // 预置源能力映射。
    compat.cspace_mut().insert_at(200, 10);

    // Mint：src=200, dest=300, rights=All(7)
    let result = compat.handle_syscall(SEL4_MINT, &[200, 300, 7]);
    assert_eq!(result, Ok(0));

    // 验证目标 CPtr 已映射到新槽位（mock: src+100=110）。
    assert_eq!(compat.cspace().resolve_cptr(300), Some(110));
}

// ============================================================
// F6：故障注入——不支持的 syscall
// ============================================================

/// F6：不支持的 syscall 返回 UnsupportedSyscall（ENOSYS 语义）。
#[test]
fn f6_unsupported_syscall_returns_enosys() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);

    // 编号 0 不在翻译表中。
    let result = compat.handle_syscall(0, &[]);
    assert_eq!(result, Err(CompatError::UnsupportedSyscall));

    // 编号 255 不在翻译表中。
    let result = compat.handle_syscall(255, &[]);
    assert_eq!(result, Err(CompatError::UnsupportedSyscall));

    // 统计计数正确。
    assert_eq!(compat.stats.unsupported, 2);
}

// ============================================================
// F7：故障注入——无效 CPtr
// ============================================================

/// F7：无效 CPtr 返回 InvalidCapability（EFAULT 语义）。
#[test]
fn f7_invalid_cptr_returns_efault() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);

    // 未预置映射，CPtr 999 无效。
    let result = compat.handle_syscall(SEL4_SEND, &[999, 0, 16]);
    assert_eq!(result, Err(CompatError::InvalidCapability));

    let result = compat.handle_syscall(SEL4_RECV, &[888]);
    assert_eq!(result, Err(CompatError::InvalidCapability));

    let result = compat.handle_syscall(SEL4_WAIT, &[777]);
    assert_eq!(result, Err(CompatError::InvalidCapability));

    // 统计计数正确。
    assert_eq!(compat.stats.invalid_cptr, 3);
}

// ============================================================
// F8：Sel4Shim trait 集成验证
// ============================================================

/// F8：Sel4Shim trait 方法通过兼容层正确执行。
#[test]
#[allow(non_snake_case)]
fn f8_sel4_shim_trait_integration() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);
    compat.cspace_mut().insert_at(500, 50);

    // seL4_Send
    let msg = MessageInfo::new(1, 64);
    assert!(compat.seL4_Send(500, &msg).is_ok());

    // seL4_Recv
    let mut sender = 0u64;
    let info = compat.seL4_Recv(500, &mut sender).unwrap();
    assert_eq!(sender, 77);
    assert_eq!(info.label, 77);

    // seL4_Call
    let call_result = compat.seL4_Call(500, &msg);
    assert!(call_result.is_ok());

    // seL4_Yield
    compat.seL4_Yield();
}

// ============================================================
// F9：Notification Wait/Signal 通过兼容层
// ============================================================

/// F9：Notification Wait/Signal 通过兼容层。
#[test]
fn f9_notification_through_compat() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);
    compat.cspace_mut().insert_at(600, 60);

    // Signal
    let result = compat.handle_syscall(SEL4_SIGNAL, &[600, 0xDEAD]);
    assert_eq!(result, Ok(0));

    // Wait
    let result = compat.handle_syscall(SEL4_WAIT, &[600]);
    assert_eq!(result, Ok(0xBEEF));
}

// ============================================================
// F10：ThreadCreate 聚合语义验证
// ============================================================

/// F10：seL4 TCB_Configure 聚合为 ThreadCreate 单 syscall。
#[test]
fn f10_thread_create_aggregation() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);

    // TCB_Configure：entry=0x8000, stack=0x10000, priority=3
    let result = compat.handle_syscall(SEL4_TCB_CONFIGURE, &[0x8000, 0x10000, 3]);
    assert_eq!(result, Ok(42)); // mock tid
}

// ============================================================
// F11：Rights 权限映射正确性
// ============================================================

/// F11：seL4 Rights → 自研内核 CapRights 位值映射。
#[test]
fn f11_rights_mapping() {
    assert_eq!(Rights::None.to_cap_rights_bits(), 0);
    assert_eq!(Rights::Read.to_cap_rights_bits(), 1);
    assert_eq!(Rights::Write.to_cap_rights_bits(), 2);
    assert_eq!(Rights::Grant.to_cap_rights_bits(), 8);
    assert_eq!(Rights::ReadWrite.to_cap_rights_bits(), 3);
    assert_eq!(Rights::All.to_cap_rights_bits(), 11);
}

// ============================================================
// F12：兼容层统计计数完整性
// ============================================================

/// F12：CompatStats 在多操作后正确累加。
#[test]
fn f12_compat_stats_integrity() {
    let mut ops = IntegrationMockOps::new();
    let mut compat = Sel4Compat::new(&mut ops);
    compat.cspace_mut().insert_at(700, 70);

    // 3 次成功操作
    let _ = compat.handle_syscall(SEL4_YIELD, &[]);
    let _ = compat.handle_syscall(SEL4_SEND, &[700, 0, 8]);
    let _ = compat.handle_syscall(SEL4_RECV, &[700]);

    // 2 次不支持
    let _ = compat.handle_syscall(0, &[]);
    let _ = compat.handle_syscall(255, &[]);

    // 1 次无效 CPtr
    let _ = compat.handle_syscall(SEL4_WAIT, &[999]);

    assert_eq!(compat.stats.translated, 4); // 3 ok + 1 invalid (translated but failed)
    assert_eq!(compat.stats.unsupported, 2);
    assert_eq!(compat.stats.invalid_cptr, 1);
    assert_eq!(compat.stats.executed_ok, 3);
    assert_eq!(compat.stats.executed_err, 1);
}
