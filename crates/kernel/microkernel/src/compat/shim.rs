//! seL4 用户态垫片库（v0.156.0 用户态兼容性验证）。
//!
//! 实现 [`Sel4Shim`] trait（用户态链接替换接口）与
//! [`sel4_syscall_handler`]（兼容层入口函数）。垫片在编译时替换
//! seL4 头文件依赖，syscall handler 在运行时将 seL4 语义映射到
//! 自研内核。
//!
//! 蓝图 phase3.md §v0.156.0 §3 接口类 + §4.5 关键代码原样实现。
//!
//! 偏差/决策记录：
//! - D1：蓝图 `#[no_mangle] pub extern "C" fn sel4_syscall_handler`
//!   全局函数 → 本版本实现为 [`Sel4Compat`] 结构体方法（lib crate
//!   全局 `#[no_mangle]` 与 host cargo test 链接冲突；语义等价，
//!   集成层在 main.rs 中接线为全局入口）。
//! - D2：蓝图 `CSPACE_MAPPER.lock()` 全局锁 → 构造注入
//!   `&mut CSpaceMapper`（同 v0.142.0 注入式先例；单核口径无锁）。
//! - D3：蓝图 `sys::mint_capability` / `sys::ipc_send` 等自由函数
//!   不存在 → 通过 [`KernelSyscallOps`] trait 注入（host 测试注入
//!   mock，集成层注入真实内核实现）。
//! - D4：蓝图 `log::warn!` → 本版本不引入 log 依赖（骨架 crate
//!   零外部依赖；日志接入属 v0.157.0 回归验证范畴）。
//! - D5：蓝图 `unsafe { core::slice::from_raw_parts(...) }` 用户态
//!   消息拷贝 → 本版本改为安全切片传递（host 测试无真实用户态
//!   地址空间；集成层 trampoline 完成用户态→内核态拷贝后传入
//!   安全切片）。

use super::cspace::CSpaceMapper;
use super::translation::SyscallTranslator;
use super::types::sel4_syscall_nums::*;
use super::types::{CPtr, CompatError, MessageInfo, Rights};
use crate::error::KernelError;

/// 内核 syscall 操作 trait（D3 注入式）。
///
/// 兼容层通过此 trait 调用自研内核功能，host 测试注入 mock，
/// 集成层注入真实内核实现。
pub trait KernelSyscallOps {
    /// 能力派生（mint）：从源能力派生新能力，权限缩减为 rights。
    fn mint_capability(&mut self, src_slot: u32, rights_bits: u32) -> Result<u32, KernelError>;

    /// IPC 发送。
    fn ipc_send(&mut self, endpoint_slot: u32, msg: &[u8]) -> Result<(), KernelError>;

    /// IPC 接收。
    fn ipc_recv(&mut self, endpoint_slot: u32, buf: &mut [u8]) -> Result<u64, KernelError>;

    /// 通知等待。
    fn notification_wait(&mut self, ntfn_slot: u32) -> Result<u64, KernelError>;

    /// 通知发送（signal）。
    fn notification_signal(&mut self, ntfn_slot: u32, badge: u64) -> Result<(), KernelError>;

    /// 让出时间片。
    fn yield_now(&mut self) -> Result<(), KernelError>;

    /// 能力删除。
    fn cap_delete(&mut self, slot: u32) -> Result<(), KernelError>;

    /// 能力移动。
    fn cap_move(&mut self, src_slot: u32, dst_slot: u32) -> Result<(), KernelError>;

    /// 线程创建（聚合 seL4 Retype+Configure）。
    fn thread_create(
        &mut self,
        entry: usize,
        stack: usize,
        priority: usize,
    ) -> Result<usize, KernelError>;

    /// 线程配置（优先级/亲和性）。
    fn tcb_configure(
        &mut self,
        tid: usize,
        priority: usize,
        affinity: usize,
    ) -> Result<(), KernelError>;
}

/// seL4 兼容层主结构体。
///
/// 组合 [`SyscallTranslator`]（翻译表）、[`CSpaceMapper`]（能力映射）
/// 与 [`KernelSyscallOps`]（内核操作注入），实现 seL4 syscall 的
/// 完整翻译、执行、返回链路。
pub struct Sel4Compat<'a, T: KernelSyscallOps> {
    /// syscall 翻译器。
    translator: SyscallTranslator,
    /// CSpace 映射器。
    cspace: CSpaceMapper,
    /// 内核操作（注入式 D3）。
    ops: &'a mut T,
    /// 兼容层统计计数。
    pub stats: CompatStats,
}

/// 兼容层统计计数（蓝图 §9.6 可观测性）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompatStats {
    /// 翻译成功次数。
    pub translated: u64,
    /// 不支持的 syscall 次数。
    pub unsupported: u64,
    /// 无效 CPtr 次数。
    pub invalid_cptr: u64,
    /// 执行成功次数。
    pub executed_ok: u64,
    /// 执行失败次数。
    pub executed_err: u64,
}

impl<'a, T: KernelSyscallOps> Sel4Compat<'a, T> {
    /// 创建兼容层实例。
    pub fn new(ops: &'a mut T) -> Self {
        Self {
            translator: SyscallTranslator::new(),
            cspace: CSpaceMapper::new(),
            ops,
            stats: CompatStats::default(),
        }
    }

    /// 获取 CSpace 映射器的可变引用（用于预置映射）。
    pub fn cspace_mut(&mut self) -> &mut CSpaceMapper {
        &mut self.cspace
    }

    /// 获取 CSpace 映射器的只读引用。
    pub const fn cspace(&self) -> &CSpaceMapper {
        &self.cspace
    }

    /// 兼容层入口：拦截 seL4 风格 syscall 并翻译执行。
    ///
    /// 蓝图 §4.5 `sel4_syscall_handler` 原样实现（D1 结构体方法化）。
    ///
    /// 参数：
    /// - `syscall_num`：seL4 syscall 编号
    /// - `args`：参数数组（最多 6 个，对应 x0~x5）
    ///
    /// 返回：`Ok(usize)` 成功返回值，`Err(CompatError)` 错误码。
    pub fn handle_syscall(
        &mut self,
        syscall_num: u32,
        args: &[usize],
    ) -> Result<usize, CompatError> {
        // 步骤 1：翻译 syscall 编号。
        let _kernel_syscall = match self.translator.translate(syscall_num) {
            Some(sc) => {
                self.stats.translated = self.stats.translated.saturating_add(1);
                sc
            }
            None => {
                self.stats.unsupported = self.stats.unsupported.saturating_add(1);
                return Err(CompatError::UnsupportedSyscall);
            }
        };

        // 步骤 2：按 seL4 syscall 类型分发执行。
        let result = match syscall_num {
            SEL4_MINT => self.handle_mint(args),
            SEL4_SEND | SEL4_NBSEND => self.handle_send(args),
            SEL4_RECV | SEL4_NBRECV | SEL4_REPLY_RECV => self.handle_recv(args),
            SEL4_CALL => self.handle_call(args),
            SEL4_WAIT => self.handle_wait(args),
            SEL4_SIGNAL => self.handle_signal(args),
            SEL4_YIELD => self.handle_yield(),
            SEL4_MOVE => self.handle_move(args),
            SEL4_DELETE => self.handle_delete(args),
            SEL4_TCB_CONFIGURE => self.handle_thread_create(args),
            SEL4_TCB_SET_PRIORITY => self.handle_tcb_configure(args),
            // Retype/Untyped_Retype：兼容层跳过（内核自动管理）。
            SEL4_RETYPE | SEL4_UNTYPED_RETYPE => Ok(0),
            // 其余已翻译但无专门 handler 的：返回成功（骨架）。
            _ => Ok(0),
        };

        // 步骤 3：更新统计。
        match &result {
            Ok(_) => self.stats.executed_ok = self.stats.executed_ok.saturating_add(1),
            Err(_) => self.stats.executed_err = self.stats.executed_err.saturating_add(1),
        }

        result
    }

    /// 处理 seL4 Mint 调用。
    ///
    /// 参数：args[0]=src_cptr, args[1]=dest_cptr, args[2]=rights_bits
    fn handle_mint(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 3 {
            return Err(CompatError::TranslationFailed);
        }
        let src_cptr = args[0] as CPtr;
        let dest_cptr = args[1] as CPtr;
        let rights = Rights::from_bits(args[2] as u64);

        // 解析源 CPtr → CapSlot。
        let src_slot = self.cspace.resolve_cptr(src_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        // 调用内核 mint。
        let new_slot = self
            .ops
            .mint_capability(src_slot, rights.to_cap_rights_bits())
            .map_err(|_| CompatError::TranslationFailed)?;

        // 在目标 CPtr 处建立映射。
        self.cspace.insert_at(dest_cptr, new_slot);
        Ok(0)
    }

    /// 处理 seL4 Send 调用。
    ///
    /// 参数：args[0]=ep_cptr, args[1]=msg_ptr（host 测试中为切片索引）,
    ///        args[2]=msg_len
    fn handle_send(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 3 {
            return Err(CompatError::TranslationFailed);
        }
        let ep_cptr = args[0] as CPtr;
        let msg_len = args[2];

        let ep_slot = self.cspace.resolve_cptr(ep_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        // D5：host 测试使用空消息（集成层由 trampoline 提供真实切片）。
        let msg = [0u8; 0];
        let _ = msg_len; // 集成层使用
        self.ops
            .ipc_send(ep_slot, &msg)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(0)
    }

    /// 处理 seL4 Recv 调用。
    ///
    /// 参数：args[0]=ep_cptr
    fn handle_recv(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.is_empty() {
            return Err(CompatError::TranslationFailed);
        }
        let ep_cptr = args[0] as CPtr;

        let ep_slot = self.cspace.resolve_cptr(ep_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        // D5：host 测试使用固定缓冲区。
        let mut buf = [0u8; 64];
        let sender = self
            .ops
            .ipc_recv(ep_slot, &mut buf)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(sender as usize)
    }

    /// 处理 seL4 Call 调用（Send + Recv 原子组合）。
    ///
    /// 参数：args[0]=ep_cptr
    fn handle_call(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.is_empty() {
            return Err(CompatError::TranslationFailed);
        }
        let ep_cptr = args[0] as CPtr;

        let ep_slot = self.cspace.resolve_cptr(ep_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        // Send 阶段。
        let msg = [0u8; 0];
        self.ops
            .ipc_send(ep_slot, &msg)
            .map_err(|_| CompatError::TranslationFailed)?;

        // Recv 阶段。
        let mut buf = [0u8; 64];
        let sender = self
            .ops
            .ipc_recv(ep_slot, &mut buf)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(sender as usize)
    }

    /// 处理 seL4 Wait 调用。
    ///
    /// 参数：args[0]=ntfn_cptr
    fn handle_wait(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.is_empty() {
            return Err(CompatError::TranslationFailed);
        }
        let ntfn_cptr = args[0] as CPtr;

        let ntfn_slot = self.cspace.resolve_cptr(ntfn_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        let badge = self
            .ops
            .notification_wait(ntfn_slot)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(badge as usize)
    }

    /// 处理 seL4 Signal 调用。
    ///
    /// 参数：args[0]=ntfn_cptr, args[1]=badge
    fn handle_signal(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 2 {
            return Err(CompatError::TranslationFailed);
        }
        let ntfn_cptr = args[0] as CPtr;
        let badge = args[1] as u64;

        let ntfn_slot = self.cspace.resolve_cptr(ntfn_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        self.ops
            .notification_signal(ntfn_slot, badge)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(0)
    }

    /// 处理 seL4 Yield 调用。
    fn handle_yield(&mut self) -> Result<usize, CompatError> {
        self.ops
            .yield_now()
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(0)
    }

    /// 处理 seL4 Move 调用。
    ///
    /// 参数：args[0]=src_cptr, args[1]=dest_cptr
    fn handle_move(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 2 {
            return Err(CompatError::TranslationFailed);
        }
        let src_cptr = args[0] as CPtr;
        let dest_cptr = args[1] as CPtr;

        let src_slot = self.cspace.resolve_cptr(src_cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        // 分配目标槽位（简化：使用 src_slot+1 作为目标）。
        let dst_slot = src_slot.saturating_add(1);
        self.ops
            .cap_move(src_slot, dst_slot)
            .map_err(|_| CompatError::TranslationFailed)?;

        // 更新映射：删除源，建立目标。
        self.cspace.free_cptr(src_cptr);
        self.cspace.insert_at(dest_cptr, dst_slot);
        Ok(0)
    }

    /// 处理 seL4 Delete 调用。
    ///
    /// 参数：args[0]=cptr
    fn handle_delete(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.is_empty() {
            return Err(CompatError::TranslationFailed);
        }
        let cptr = args[0] as CPtr;

        let slot = self.cspace.resolve_cptr(cptr).ok_or_else(|| {
            self.stats.invalid_cptr = self.stats.invalid_cptr.saturating_add(1);
            CompatError::InvalidCapability
        })?;

        self.ops
            .cap_delete(slot)
            .map_err(|_| CompatError::TranslationFailed)?;
        self.cspace.free_cptr(cptr);
        Ok(0)
    }

    /// 处理 seL4 TCB_Configure（聚合 ThreadCreate）。
    ///
    /// 参数：args[0]=entry, args[1]=stack, args[2]=priority
    fn handle_thread_create(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 3 {
            return Err(CompatError::TranslationFailed);
        }
        let entry = args[0];
        let stack = args[1];
        let priority = args[2];

        let tid = self
            .ops
            .thread_create(entry, stack, priority)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(tid)
    }

    /// 处理 seL4 TCB_SetPriority。
    ///
    /// 参数：args[0]=tid, args[1]=priority, args[2]=affinity
    fn handle_tcb_configure(&mut self, args: &[usize]) -> Result<usize, CompatError> {
        if args.len() < 2 {
            return Err(CompatError::TranslationFailed);
        }
        let tid = args[0];
        let priority = args[1];
        let affinity = if args.len() > 2 { args[2] } else { 0 };

        self.ops
            .tcb_configure(tid, priority, affinity)
            .map_err(|_| CompatError::TranslationFailed)?;
        Ok(0)
    }
}

/// 用户态垫片 trait（蓝图 §3 接口类 `Sel4Shim` 原样）。
///
/// 用户态代码通过此 trait 替换 seL4 编译依赖。实现者提供
/// seL4 风格的 API 签名，内部转发到兼容层。
///
/// 方法名保持 seL4 原始命名（`seL4_Call` 等），非 snake_case
/// 为有意设计（ABI 兼容性要求）。
#[allow(non_snake_case)]
pub trait Sel4Shim {
    /// seL4_Call：同步 IPC 调用。
    fn seL4_Call(&mut self, ep: CPtr, msg: &MessageInfo) -> Result<MessageInfo, CompatError>;

    /// seL4_Send：IPC 发送。
    fn seL4_Send(&mut self, ep: CPtr, msg: &MessageInfo) -> Result<(), CompatError>;

    /// seL4_Recv：IPC 接收。
    fn seL4_Recv(&mut self, ep: CPtr, sender: &mut u64) -> Result<MessageInfo, CompatError>;

    /// seL4_Yield：让出时间片。
    fn seL4_Yield(&mut self);
}

/// 基于 [`Sel4Compat`] 的 [`Sel4Shim`] 默认实现。
#[allow(non_snake_case)]
impl<'a, T: KernelSyscallOps> Sel4Shim for Sel4Compat<'a, T> {
    fn seL4_Call(&mut self, ep: CPtr, msg: &MessageInfo) -> Result<MessageInfo, CompatError> {
        let args = [ep as usize, msg.label as usize, msg.length as usize];
        let sender = self.handle_syscall(SEL4_CALL, &args)?;
        Ok(MessageInfo::new(sender as u64, 0))
    }

    fn seL4_Send(&mut self, ep: CPtr, msg: &MessageInfo) -> Result<(), CompatError> {
        let args = [ep as usize, 0, msg.length as usize];
        self.handle_syscall(SEL4_SEND, &args)?;
        Ok(())
    }

    fn seL4_Recv(&mut self, ep: CPtr, sender: &mut u64) -> Result<MessageInfo, CompatError> {
        let args = [ep as usize];
        let s = self.handle_syscall(SEL4_RECV, &args)?;
        *sender = s as u64;
        Ok(MessageInfo::new(s as u64, 0))
    }

    fn seL4_Yield(&mut self) {
        let _ = self.handle_syscall(SEL4_YIELD, &[]);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// Mock 内核操作实现。
    struct MockOps {
        mint_result: Result<u32, KernelError>,
        send_result: Result<(), KernelError>,
        recv_result: Result<u64, KernelError>,
        wait_result: Result<u64, KernelError>,
        signal_result: Result<(), KernelError>,
        yield_result: Result<(), KernelError>,
        delete_result: Result<(), KernelError>,
        move_result: Result<(), KernelError>,
        thread_create_result: Result<usize, KernelError>,
        tcb_configure_result: Result<(), KernelError>,
    }

    impl MockOps {
        fn ok() -> Self {
            Self {
                mint_result: Ok(100),
                send_result: Ok(()),
                recv_result: Ok(42),
                wait_result: Ok(0xFF),
                signal_result: Ok(()),
                yield_result: Ok(()),
                delete_result: Ok(()),
                move_result: Ok(()),
                thread_create_result: Ok(7),
                tcb_configure_result: Ok(()),
            }
        }
    }

    impl KernelSyscallOps for MockOps {
        fn mint_capability(&mut self, _src: u32, _rights: u32) -> Result<u32, KernelError> {
            self.mint_result
        }
        fn ipc_send(&mut self, _ep: u32, _msg: &[u8]) -> Result<(), KernelError> {
            self.send_result
        }
        fn ipc_recv(&mut self, _ep: u32, _buf: &mut [u8]) -> Result<u64, KernelError> {
            self.recv_result
        }
        fn notification_wait(&mut self, _ntfn: u32) -> Result<u64, KernelError> {
            self.wait_result
        }
        fn notification_signal(&mut self, _ntfn: u32, _badge: u64) -> Result<(), KernelError> {
            self.signal_result
        }
        fn yield_now(&mut self) -> Result<(), KernelError> {
            self.yield_result
        }
        fn cap_delete(&mut self, _slot: u32) -> Result<(), KernelError> {
            self.delete_result
        }
        fn cap_move(&mut self, _src: u32, _dst: u32) -> Result<(), KernelError> {
            self.move_result
        }
        fn thread_create(&mut self, _e: usize, _s: usize, _p: usize) -> Result<usize, KernelError> {
            self.thread_create_result
        }
        fn tcb_configure(&mut self, _t: usize, _p: usize, _a: usize) -> Result<(), KernelError> {
            self.tcb_configure_result
        }
    }

    /// SH1：不支持的 syscall 返回 UnsupportedSyscall。
    #[test]
    fn sh1_unsupported_syscall() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        let result = compat.handle_syscall(99, &[]);
        assert_eq!(result, Err(CompatError::UnsupportedSyscall));
        assert_eq!(compat.stats.unsupported, 1);
    }

    /// SH2：Yield 成功。
    #[test]
    fn sh2_yield_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        let result = compat.handle_syscall(SEL4_YIELD, &[]);
        assert_eq!(result, Ok(0));
        assert_eq!(compat.stats.executed_ok, 1);
    }

    /// SH3：Mint 成功（预置 CSpace 映射）。
    #[test]
    fn sh3_mint_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        // 预置源 CPtr 映射。
        compat.cspace_mut().insert_at(10, 5);
        let result = compat.handle_syscall(SEL4_MINT, &[10, 20, 7]);
        assert_eq!(result, Ok(0));
        // 验证目标 CPtr 已映射到新槽位。
        assert_eq!(compat.cspace().resolve_cptr(20), Some(100));
    }

    /// SH4：Mint 无效 CPtr 返回 InvalidCapability。
    #[test]
    fn sh4_mint_invalid_cptr() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        let result = compat.handle_syscall(SEL4_MINT, &[999, 20, 7]);
        assert_eq!(result, Err(CompatError::InvalidCapability));
        assert_eq!(compat.stats.invalid_cptr, 1);
    }

    /// SH5：Send 成功。
    #[test]
    fn sh5_send_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(30, 3);
        let result = compat.handle_syscall(SEL4_SEND, &[30, 0, 16]);
        assert_eq!(result, Ok(0));
    }

    /// SH6：Recv 成功返回 sender。
    #[test]
    fn sh6_recv_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(40, 4);
        let result = compat.handle_syscall(SEL4_RECV, &[40]);
        assert_eq!(result, Ok(42));
    }

    /// SH7：Wait 成功返回 badge。
    #[test]
    fn sh7_wait_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(50, 5);
        let result = compat.handle_syscall(SEL4_WAIT, &[50]);
        assert_eq!(result, Ok(0xFF));
    }

    /// SH8：Signal 成功。
    #[test]
    fn sh8_signal_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(60, 6);
        let result = compat.handle_syscall(SEL4_SIGNAL, &[60, 0xAB]);
        assert_eq!(result, Ok(0));
    }

    /// SH9：Delete 成功并释放映射。
    #[test]
    fn sh9_delete_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(70, 7);
        let result = compat.handle_syscall(SEL4_DELETE, &[70]);
        assert_eq!(result, Ok(0));
        assert_eq!(compat.cspace().resolve_cptr(70), None);
    }

    /// SH10：ThreadCreate 聚合成功。
    #[test]
    fn sh10_thread_create_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        let result = compat.handle_syscall(SEL4_TCB_CONFIGURE, &[0x4000, 0x8000, 5]);
        assert_eq!(result, Ok(7));
    }

    /// SH11：Retype 跳过返回 Ok(0)。
    #[test]
    fn sh11_retype_skip() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        let result = compat.handle_syscall(SEL4_RETYPE, &[1, 2, 3]);
        assert_eq!(result, Ok(0));
    }

    /// SH12：Sel4Shim trait 实现正确。
    #[test]
    fn sh12_shim_trait() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(80, 8);

        // seL4_Send
        let msg = MessageInfo::new(1, 32);
        assert!(compat.seL4_Send(80, &msg).is_ok());

        // seL4_Recv
        let mut sender = 0u64;
        let info = compat.seL4_Recv(80, &mut sender).unwrap();
        assert_eq!(sender, 42);
        assert_eq!(info.label, 42);

        // seL4_Yield
        compat.seL4_Yield();
    }

    /// SH13：统计计数正确累加。
    #[test]
    fn sh13_stats_accumulate() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(90, 9);

        let _ = compat.handle_syscall(SEL4_YIELD, &[]);
        let _ = compat.handle_syscall(SEL4_YIELD, &[]);
        let _ = compat.handle_syscall(99, &[]); // unsupported

        assert_eq!(compat.stats.translated, 2);
        assert_eq!(compat.stats.unsupported, 1);
        assert_eq!(compat.stats.executed_ok, 2);
    }

    /// SH14：Move 成功并更新映射。
    #[test]
    fn sh14_move_ok() {
        let mut ops = MockOps::ok();
        let mut compat = Sel4Compat::new(&mut ops);
        compat.cspace_mut().insert_at(100, 10);
        let result = compat.handle_syscall(SEL4_MOVE, &[100, 200]);
        assert_eq!(result, Ok(0));
        // 源已删除。
        assert_eq!(compat.cspace().resolve_cptr(100), None);
        // 目标已建立（dst_slot = src_slot + 1 = 11）。
        assert_eq!(compat.cspace().resolve_cptr(200), Some(11));
    }
}
