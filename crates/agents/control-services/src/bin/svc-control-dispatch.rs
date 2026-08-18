//! svc-control-dispatch — 蓝图 40 WP-40.6：唯一 ControlDispatch 服务（EL0）。
//!
//! 装配（40 收尾）：产品链（policy/lease/safety 同源配置）+ EIPC 投递
//! 通道（root spawn 授予 svc-device-agent 端点 → X3 bit0 → 槽 3）。
//! 未授予（X3 bit0=0）→ UnboundChannel 显式失败（fail-closed）。

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
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    let mut handler = DispatchHandler::new(eipc_mask);
    eneros_control_services::control_service_main(
        ep,
        "control-dispatch",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct DispatchHandler {
    engine: eneros_control_dispatch::DispatchEngine,
}

#[cfg(target_os = "none")]
impl DispatchHandler {
    fn new(eipc_mask: u64) -> Self {
        // 装配（40 收尾）：产品链 + EIPC 投递通道。
        use eneros_control_dispatch::{DispatchConfig, FeedbackPointMapping, InMemoryIntentStore};
        use eneros_control_services::dispatch_service::{
            prod_chain, PROD_EPOCH_SEED, PROD_KEY, PROD_LEASE_ID, PROD_POLICY_VERSION,
        };
        let now = eneros_drv_svc::now_ms();
        let chain = prod_chain(now);
        // 投递通道：X3 bit0 = 槽 3（svc-device-agent ep，root spawn mint）。
        let delivery: alloc::boxed::Box<dyn eneros_control_dispatch::DeliveryChannel> = if eipc_mask
            & 1
            != 0
        {
            match eneros_control_dispatch::Sel4EipcChannel::new(
                "control-dispatch",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                eneros_drv_svc::instance_epoch(),
                5_000,
                eneros_drv_svc::now_ns,
            ) {
                Ok(ch) => {
                    eneros_runtime::println!(
                            "[svc-control-dispatch] EIPC delivery bound: slot={} target=svc-device-agent",
                            eneros_agent_sdk::EIPC_EP_BASE_SLOT
                        );
                    alloc::boxed::Box::new(ch)
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] EIPC bind FAILED: {e:?} — Unbound (fail-closed)"
                    );
                    alloc::boxed::Box::new(UnboundChannel)
                }
            }
        } else {
            eneros_runtime::println!(
                    "[svc-control-dispatch] EIPC cap not granted (X3=0x{eipc_mask:x}) — Unbound (fail-closed)"
                );
            alloc::boxed::Box::new(UnboundChannel)
        };
        let config = DispatchConfig {
            chain,
            key: PROD_KEY.to_vec(),
            lease_id: PROD_LEASE_ID,
            fencing_token: PROD_EPOCH_SEED + 1,
            issuer_service: [0x41; 16],
            issuer_epoch: 0,
            policy_version: PROD_POLICY_VERSION,
            device_id: eneros_control_services::dispatch_service::PROD_DEVICE_ID,
            point_id: eneros_control_services::dispatch_service::PROD_POINT_ID,
            target_endpoint: alloc::string::String::from("svc-device-agent"),
            max_power_kw: 500.0,
            // QEMU demo profile：rate gate 放宽（60/h → 100000/h——60/h
            // 在演示驱动频率下 6 分钟虚拟即撞限，round 61 Rejected；
            // 产品值由 41 RuntimeConfigBundle 注入）。
            gate: eneros_dual_brain::gate::DecisionGate::new(100_000, 1000.0, 3_600_000),
            audit: None,
            intent_log: alloc::boxed::Box::new(InMemoryIntentStore::new(1024)),
            delivery,
            feedback_mapping: FeedbackPointMapping {
                grid_up: 5,
                soc: 4,
                dc_bus_v: 1,
                overcurrent: 6,
                overtemp: 6,
            },
            now_fn: eneros_drv_svc::now_ms,
        };
        Self {
            engine: eneros_control_dispatch::DispatchEngine::new(config),
        }
    }
}

/// 未绑定端点（无 EIPC cap 授予）——deliver 明确失败。
#[cfg(target_os = "none")]
struct UnboundChannel;

#[cfg(target_os = "none")]
impl eneros_control_dispatch::DeliveryChannel for UnboundChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<alloc::vec::Vec<u8>, eneros_control_dispatch::ChannelError> {
        Err(eneros_control_dispatch::ChannelError::Unreachable)
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for DispatchHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        *resp = eneros_control_services::dispatch_service::handle_submit(&mut self.engine, req);
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
