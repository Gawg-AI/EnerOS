//! svc-pie — EnerOS 静态 PIE（ET_DYN）参考 EL0 服务（WP-37.4 PIE loader
//! target 证据载体）。
//!
//! 与 svc-pi 同构（Endpoint IPC 入口 + π 计算），但经 build.rs 以
//! `relocation-model=pic` + `-pie` 链接为 **ET_DYN**：root-task 的
//! `plan_spawn_dyn_aware` 走 `load_verified_elf`（bias + R_AARCH64_RELATIVE
//! 重定位 + entry 平移）。服务正常 Signal 就绪通知 + 回 IPC 即证明
//! 重定位与装载在 target 真实生效（重定位错误必然 crash）。
//!
//! bundle 中可声明两个实例（不同 service id）→ `plan_spawn_dyn_aware`
//! 按实例递增 bias → 双 bias 运行证据。

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
pub extern "C" fn _start(ep: u64, ipc_buffer: u64, ready_ntfn: u64) -> ! {
    // 与 svc-pi 同构：IPC buffer TLS → 堆 → 就绪 Signal → IPC 循环。
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: _start 仅执行一次且早于任何 alloc 调用。
    unsafe { ALLOC.init() };
    eneros_runtime::init();
    eneros_services::panic::init_svc_panic_audit();
    eneros_services::svc_entry(ep, "pie", ready_ntfn)
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eneros_services::panic::svc_panic_hook("pie", info)
}

// 22 §2.W1.1：OOM 不 abort——记录指标 + 打印 + 挂起（fail-closed）。
#[cfg(target_os = "none")]
#[alloc_error_handler]
fn oom(layout: core::alloc::Layout) -> ! {
    eneros_runtime::oom::oom_fail_safe(layout)
}

/// host 构建壳（bin 目标侧代码仅 target_os = "none" 可见）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {
    eprintln!("svc-pie 仅用于 aarch64-unknown-none 目标构建");
}

#[cfg(test)]
mod tests {
    /// 静态 PIE 契约锚点：本 crate 必须产出 ET_DYN（load_verified_elf
    /// 白名单契约）。运行期无法自检链接类型——由构建侧证据
    /// （readelf e_type=DYN + 仅 RELATIVE）与 QEMU 运行证据覆盖。
    #[allow(clippy::assertions_on_constants)] // 契约锚点占位（构建侧证据覆盖链接类型）
    #[test]
    fn svc_pie_contract_anchor() {
        assert!(true);
    }
}
