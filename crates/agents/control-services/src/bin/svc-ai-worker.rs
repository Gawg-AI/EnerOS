//! svc-ai-worker — 蓝图 40 WP-40.5：target AI worker（EL0）。
//!
//! 只经 EIPC 获取可信输入（TrustedTelemetry wire）并返回 proposal/拒绝。
//! Llama/HiGHS 为 host native 后端——target profile 不声明（WP-40.4）；
//! 决策路径为**静态启发式后端**（WP-40.4："静态 solver/heuristic 具
//! bounded work"）：SOC 死区规则，O(1) 有界工作，只产生 proposal。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    _eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 决策后端：静态启发式（target 产品路径——profile 不声明 Llama/HiGHS，
    // WP-40.4 允许的 bounded heuristic；无 native 库依赖）。
    eneros_runtime::println!("[svc-ai-worker] backend=heuristic (Llama/HiGHS not declared)");
    let mut worker = eneros_control_services::ai_worker::AiWorkerService::new(
        eneros_control_services::ai_worker::HeuristicBackend::new(),
        [0x41; 16],
    );
    let mut handler = AiWorkerHandler {
        worker: &mut worker,
    };
    eneros_control_services::control_service_main(
        ep,
        "ai-worker",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct AiWorkerHandler<'a> {
    worker: &'a mut eneros_control_services::ai_worker::AiWorkerService<
        eneros_control_services::ai_worker::HeuristicBackend,
    >,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for AiWorkerHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        *resp = self.worker.propose(req, now_ms);
        true
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
