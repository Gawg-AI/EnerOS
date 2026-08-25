//! svc-device-agent — 蓝图 40 WP-40.9：设备执行服务（EL0）。
//!
//! 收分块 ControlEnvelope（经 EIPC）→ 复验/认证/路由 → AuthorizedWriter
//! 执行 → 独立 readback → 结构化回执（OutcomeFrame）。registry 装配期
//! 注册（持久化加载见 persist API——存储句柄移交后接线）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]
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
    eneros_runtime::println!("[svc-device-agent] _start entry ipc_buffer={ipc_buffer:x}");
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    eneros_runtime::println!("[svc-device-agent] allocator initialized");
    eneros_runtime::println!("[svc-device-agent] constructing handler");
    let mut handler = DeviceAgentHandler::new(eipc_mask);
    eneros_runtime::println!("[svc-device-agent] handler constructed");
    eneros_control_services::control_service_main(
        ep,
        "device-agent",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct DeviceAgentHandler {
    ingress: eneros_device_agent::EnvelopeIngress,
    devices: eneros_device_agent::DeviceRegistry,
}

#[cfg(target_os = "none")]
impl DeviceAgentHandler {
    fn new(eipc_mask: u64) -> Self {
        // 装配（40 收尾 + 47 WP-47.2 + 蓝图 55）：产品配置与 svc-control-dispatch
        // 同源——两侧经同一 ControlProfile（dispatch_service::prod_profile）
        // 派生 key 之外的档案事实，不同源互相拒绝。
        // 写目标（设备/点）由 45 卷点表 Control 点绑定（svc-config）；
        // 不可得 → 0/0 fail-closed（default-deny）。
        use alloc::collections::BTreeMap;

        use eneros_control_services::dispatch_service::{prod_profile, ASSEMBLY_TEST_KEY};
        let now = eneros_drv_svc::now_ms();
        let mut config_client = if eipc_mask & 1 != 0 {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "device-agent",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                eneros_drv_svc::now_ns,
            )
            .ok()
        } else {
            None
        };
        let binding = match &mut config_client {
            Some(c) => eneros_control_services::cfg_bindings::fetch_control_binding(c),
            None => {
                eneros_runtime::println!(
                    "[svc-device-agent] config EP NOT granted —— 控制绑定缺失（fail-closed）"
                );
                None
            }
        };
        let (device_id, point_id) = binding.unwrap_or((0, 0));
        let profile = prod_profile(device_id, point_id);
        let ingress =
            eneros_device_agent::EnvelopeIngress::from_profile(ASSEMBLY_TEST_KEY, &profile, now);
        eneros_runtime::println!(
            "[svc-device-agent] ingress: constructed (lease_ok={})",
            ingress.lease_ok()
        );
        // 虚拟 PCS 设备（profile 声明 virtual-pcs 时装配；写/读回为真实
        // 寄存器语义——串口 VIRTUAL-PCS 行为外部观察证据）。
        let mut devices = eneros_device_agent::DeviceRegistry::new();
        let dev = eneros_device_agent::VirtualPcsDevice::new(
            eneros_device_agent::VIRTUAL_PCS_DEVICE_ID,
            eneros_drv_svc::now_ms,
        );
        let mut pm = BTreeMap::new();
        for (name, point) in eneros_device_agent::VIRTUAL_PCS_POINTS {
            pm.insert(alloc::string::String::from(*name), *point);
        }
        devices.register(
            "pcs",
            eneros_device_agent::DeviceType::Pcs,
            eneros_device_agent::VIRTUAL_PCS_DEVICE_ID,
            alloc::boxed::Box::new(dev),
            pm,
        );
        eneros_runtime::println!("[svc-device-agent] virtual-pcs registered (device=1 point=10)");
        Self { ingress, devices }
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for DeviceAgentHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        // 请求 = ControlEnvelope 字节 → ingress 处理 → OutcomeFrame。
        let out = self.ingress.process(&mut self.devices, req, now_ms);
        eneros_runtime::println!("[svc-device-agent] INGRESS: {:?}", out);
        *resp = eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec();
        true
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eneros_runtime::println!("[svc-device-agent] PANIC: {info}");
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_os = "none")]
#[alloc_error_handler]
fn oom(layout: core::alloc::Layout) -> ! {
    eneros_runtime::println!(
        "[svc-device-agent] OOM: size={} align={}",
        layout.size(),
        layout.align()
    );
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
