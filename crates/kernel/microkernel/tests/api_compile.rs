//! 接口定义编译检查 + 类型契约断言（蓝图 §3 测试类交付物）。
//!
//! 本文件为集成测试视角：与 src 内嵌单元测试互补，从 crate 外部仅以
//! lib.rs 重导出的公开 API 验证接口契约，确保对外暴露的签名、判别值
//! 与 ABI 语义稳定。集成测试在 host 上编译运行，可使用 std。

use std::boxed::Box;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use eneros_microkernel::{
    BootInfo, KernelError, KernelState, KernelSubsystem, SubsystemRegistry, SyscallHandler,
    SyscallNum, KERNEL_VERSION,
};

/// 测试用 mock 子系统：记录 init 调用次序。
struct MockSubsystem {
    /// 子系统标识：init 被调用时追加进共享日志。
    id: usize,
    /// 共享调用日志，用于断言初始化次序。
    log: Arc<Mutex<Vec<usize>>>,
    /// init 是否被调用过（含收到的 BootInfo 校验）。
    initialized: Arc<Mutex<bool>>,
}

impl KernelSubsystem for MockSubsystem {
    fn name(&self) -> &'static str {
        "api-mock"
    }

    fn init(&mut self, _boot_info: &BootInfo) -> Result<(), KernelError> {
        self.log.lock().expect("测试日志锁中毒").push(self.id);
        *self.initialized.lock().expect("初始化标志锁中毒") = true;
        Ok(())
    }

    fn shutdown(&self) -> Result<(), KernelError> {
        Ok(())
    }
}

/// T1：mock 子系统实现 KernelSubsystem 三方法，Box::leak 构造
/// `&'static mut`，注册槽位 0/1，init_all 返回 Ok(())，两个 mock
/// 均被初始化且次序正确。
#[test]
fn t1_registry_with_mock_subsystems() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let init0 = Arc::new(Mutex::new(false));
    let init1 = Arc::new(Mutex::new(false));

    let sub0 = Box::leak(Box::new(MockSubsystem {
        id: 0,
        log: Arc::clone(&log),
        initialized: Arc::clone(&init0),
    }));
    let sub1 = Box::leak(Box::new(MockSubsystem {
        id: 1,
        log: Arc::clone(&log),
        initialized: Arc::clone(&init1),
    }));

    let boot_info = BootInfo {
        kernel_entry: 0x4008_0000,
        dtb_phys: 0x4400_0000,
        ram_region: (0x4000_0000, 0x0800_0000),
        hart_id: 0,
    };

    let mut reg = SubsystemRegistry::new();
    assert_eq!(reg.register(0, sub0), Ok(()));
    assert_eq!(reg.register(1, sub1), Ok(()));
    assert_eq!(reg.count(), 2);

    assert_eq!(reg.init_all(&boot_info), Ok(()));
    // 两个 mock 均被初始化，且按槽位升序调用。
    assert!(*init0.lock().expect("初始化标志锁中毒"));
    assert!(*init1.lock().expect("初始化标志锁中毒"));
    assert_eq!(*log.lock().expect("测试日志锁中毒"), std::vec![0, 1]);
}

/// T2：BootInfo 全字段构造并逐字段断言读回。
#[test]
fn t2_boot_info_full_field_construction() {
    let boot_info = BootInfo {
        kernel_entry: 0x4008_0000,
        dtb_phys: 0x4400_0000,
        ram_region: (0x4000_0000, 0x0800_0000),
        hart_id: 0,
    };
    assert_eq!(boot_info.kernel_entry, 0x4008_0000);
    assert_eq!(boot_info.dtb_phys, 0x4400_0000);
    assert_eq!(boot_info.ram_region, (0x4000_0000, 0x0800_0000));
    assert_eq!(boot_info.hart_id, 0);
}

/// T3：SyscallHandler 函数指针接收 mock handler（校验 IpcCall 契约签名），
/// 以 IpcCall + 8 元素 msg 调用断言 Ok(8)。
#[test]
fn t3_syscall_handler_fn_pointer_ipc_call() {
    fn mock_handler(num: SyscallNum, args: &[usize]) -> Result<usize, KernelError> {
        // IpcCall 契约：编号为 IpcCall，msg 为 8 元素切片，返回收到的 word 数。
        assert_eq!(num, SyscallNum::IpcCall);
        assert_eq!(args.len(), 8);
        Ok(8)
    }

    let handler: SyscallHandler = mock_handler;
    let msg: [usize; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(handler(SyscallNum::IpcCall, &msg), Ok(8));
}

/// T4：SyscallNum 16 编号断言 + 尺寸为 2 字节（repr(u16)）。
#[test]
fn t4_syscall_num_discriminants_and_size() {
    assert_eq!(SyscallNum::Yield as u16, 0);
    assert_eq!(SyscallNum::ThreadCreate as u16, 1);
    assert_eq!(SyscallNum::ThreadDelete as u16, 2);
    assert_eq!(SyscallNum::IpcSend as u16, 3);
    assert_eq!(SyscallNum::IpcRecv as u16, 4);
    assert_eq!(SyscallNum::IpcCall as u16, 5);
    assert_eq!(SyscallNum::MapPages as u16, 6);
    assert_eq!(SyscallNum::UnmapPages as u16, 7);
    assert_eq!(SyscallNum::CapInsert as u16, 8);
    assert_eq!(SyscallNum::CapDelete as u16, 9);
    assert_eq!(SyscallNum::CapMove as u16, 10);
    assert_eq!(SyscallNum::Notify as u16, 11);
    assert_eq!(SyscallNum::WaitFor as u16, 12);
    assert_eq!(SyscallNum::Schedule as u16, 13);
    assert_eq!(SyscallNum::TcbConfigure as u16, 14);
    assert_eq!(SyscallNum::VSpaceMap as u16, 15);
    assert_eq!(core::mem::size_of::<SyscallNum>(), 2);
}

/// T5：KERNEL_VERSION 字段断言 + KernelState::new() 可构造（v0.155.0）。
#[test]
fn t5_kernel_version_and_state() {
    assert_eq!(KERNEL_VERSION.major, 0);
    assert_eq!(KERNEL_VERSION.minor, 155);
    assert_eq!(KERNEL_VERSION.patch, 0);
    assert!(!KERNEL_VERSION.commit.is_empty());
    assert!(!KERNEL_VERSION.build_time.is_empty());

    let state = KernelState::new();
    let _ = &state.scheduler;
    let _ = &state.memory;
    let _ = &state.caps;
    let _ = &state.ipc;
    let _ = &state.hal;
    let _ = &state.time;
    let _ = &state.bootstrap;
}

/// T6：KernelError 十一边体 match 穷尽性（编译期穷尽检查即契约），
/// 并断言十一边体 as u16 覆盖 1~9 与 11~12 互异（10 留空：v0.142.0
/// spec InvalidCapability=10 语义已由既有 =3 覆盖，见 error.rs 头注）。
#[test]
fn t6_kernel_error_exhaustive_classify() {
    /// 十一边体全列的穷尽 match：缺任一变体则编译失败，即穷尽性契约。
    fn classify(e: KernelError) -> u16 {
        match e {
            KernelError::InvalidArgument => 1,
            KernelError::OutOfMemory => 2,
            KernelError::InvalidCapability => 3,
            KernelError::PermissionDenied => 4,
            KernelError::WouldBlock => 5,
            KernelError::Timeout => 6,
            KernelError::BadState => 7,
            KernelError::NotSupported => 8,
            KernelError::AlreadyMapped => 9,
            KernelError::ConstraintViolation => 11,
            KernelError::CapExpired => 12,
        }
    }

    let all = [
        KernelError::InvalidArgument,
        KernelError::OutOfMemory,
        KernelError::InvalidCapability,
        KernelError::PermissionDenied,
        KernelError::WouldBlock,
        KernelError::Timeout,
        KernelError::BadState,
        KernelError::NotSupported,
        KernelError::AlreadyMapped,
        KernelError::ConstraintViolation,
        KernelError::CapExpired,
    ];
    let mut vals: Vec<u16> = all.iter().map(|e| *e as u16).collect();
    vals.sort_unstable();
    assert_eq!(vals, std::vec![1u16, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12]);
    // classify 与 as u16 判别值一致。
    for e in all {
        assert_eq!(classify(e), e as u16);
    }
}
