//! svc-pi — EnerOS CPU 密集型（π 计算）参考 EL0 服务（K2.3：Endpoint IPC 入口）.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_services::allocator::HeapAllocator =
    eneros_services::allocator::HeapAllocator::new();

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_services;

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(ep: u64, ipc_buffer: u64) -> ! {
    // K2.3：设置 IPC buffer TLS（__sel4_ipc_buffer）——被 spawn 的服务无
    // bootinfo，libsel4 的 ipc_recv/get_mr 依赖该指针；不设置会在 NULL
    // buffer 上解引用（21-A 遗留缺陷：svc_ipc_loop 首轮 recv 卡死）。
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // 22 §2.W1.4：可回收堆初始化（首次分配前；println/put_char 无分配）。
    // SAFETY: _start 仅执行一次且早于任何 alloc 调用。
    unsafe { ALLOC.init() };
    // K2.3：π 计算由 IPC 请求驱动（pi=<n>），见 handle_request。
    eneros_runtime::init();
    eneros_services::panic::init_svc_panic_audit();
    eneros_services::svc_entry(ep, "pi")
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // 22 §2.W1.2：审计 + 挂起（supervisor 重启兜底）。
    eneros_services::panic::svc_panic_hook("pi", info)
}

// 22 §2.W1.1：OOM 不 abort——记录指标 + 打印 + 挂起（fail-closed，
// watchdog 兜底见 W1.2/W1.6）。各 bin 独立定义（lang item 每二进制一个）。
#[cfg(target_os = "none")]
#[alloc_error_handler]
fn oom(layout: core::alloc::Layout) -> ! {
    eneros_runtime::oom::oom_fail_safe(layout)
}

/// host 构建壳（bin 目标侧代码仅 target_os = "none" 可见；host 下无运行环境）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {
    eprintln!("svc-pi 仅用于 aarch64-unknown-none 目标构建");
}
