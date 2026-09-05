//! svc-energy-agent — 蓝图 40：编排服务（EL0）。
//!
//! 消息路由：svc-telemetry（可信遥测）→ svc-ai-worker（决策）→
//! svc-control-dispatch（唯一投递）。EIPC 客户端经 root spawn 授予的
//! 端点 cap（X3 bit0/1/2 → 槽 3/4/5）；未授予位 → 对应客户端显式失败
//! （fail-closed：零推理零新写，不伪造）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec;
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
    // WP-54.4：按目标名解析授予槽（agent-sdk 单一事实源）；SPEC-B：
    // 掩码位判定收编 eipc_granted_slot，不再手数偏移。
    let slot_of = |target: &'static str| -> Option<u64> {
        eneros_agent_sdk::eipc_granted_slot("svc-energy-agent", eipc_mask, target)
    };
    let telemetry: alloc::boxed::Box<
        dyn eneros_control_services::energy_agent_service::TelemetryClient,
    > = if let Some(slot) = slot_of("svc-telemetry") {
        match EipcClient::new(slot, "svc-telemetry") {
            Some(c) => alloc::boxed::Box::new(TelemetryEipc(c)),
            None => alloc::boxed::Box::new(UnboundTelemetryClient),
        }
    } else {
        alloc::boxed::Box::new(UnboundTelemetryClient)
    };
    let worker: alloc::boxed::Box<dyn eneros_control_services::energy_agent_service::WorkerClient> =
        if let Some(slot) = slot_of("svc-ai-worker") {
            match EipcClient::new(slot, "svc-ai-worker") {
                Some(c) => alloc::boxed::Box::new(WorkerEipc(c)),
                None => alloc::boxed::Box::new(UnboundWorkerClient),
            }
        } else {
            alloc::boxed::Box::new(UnboundWorkerClient)
        };
    let dispatch: alloc::boxed::Box<
        dyn eneros_control_services::energy_agent_service::DispatchClient,
    > = if let Some(slot) = slot_of("svc-control-dispatch") {
        match EipcClient::new(slot, "svc-control-dispatch") {
            Some(c) => alloc::boxed::Box::new(DispatchEipc(c)),
            None => alloc::boxed::Box::new(UnboundDispatchClient),
        }
    } else {
        alloc::boxed::Box::new(UnboundDispatchClient)
    };
    let bound = (eipc_mask & 7).count_ones();
    eneros_runtime::println!(
        "[svc-energy-agent] EIPC clients bound: {bound}/3 (X3=0x{eipc_mask:x})"
    );
    let mut orchestrator = eneros_control_services::energy_agent_service::EnergyAgentService::new(
        telemetry, worker, dispatch,
    );
    let mut handler = EnergyAgentHandler {
        svc: &mut orchestrator,
    };
    eneros_control_services::control_service_main(
        ep,
        "energy-agent",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

/// EIPC 客户端（分块传输 + 裸帧请求）。蓝图 67 WP-67.2.3：`ipc + epoch
/// + timeout_ms` 三字段收敛为 `ChunkedEndpoint`（bind_cap_at 起始
/// instance_epoch；递增纪律单点，超时 5_000 = 常量默认走 `call`）。
#[cfg(target_os = "none")]
struct EipcClient {
    endpoint:
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
}

#[cfg(target_os = "none")]
impl EipcClient {
    fn new(dest_cap: u64, name: &str) -> Option<Self> {
        match eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap_at(
            name,
            dest_cap,
            eneros_drv_svc::instance_epoch(),
        ) {
            Some(endpoint) => Some(Self { endpoint }),
            None => {
                eneros_runtime::println!("[svc-energy-agent] EIPC {name} bind FAILED");
                None
            }
        }
    }

    fn call(
        &mut self,
        target: &str,
        req: &[u8],
    ) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        self.endpoint
            .call(target, req)
            .map_err(|e| alloc::format!("eipc:{e:?}"))
    }
}

#[cfg(target_os = "none")]
struct TelemetryEipc(EipcClient);
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::TelemetryClient for TelemetryEipc {
    fn fetch_telemetry(&mut self) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        // 蓝图 67 WP-67.3：跨 crate 双名混用消除——直呼 wire 单源常量
        // （control-services 侧 re-export 别名保留供旧路径）。
        self.0.call(
            "svc-telemetry",
            &[eneros_agent_sdk::tel_wire::TEL_REQ_SNAPSHOT],
        )
    }
}

#[cfg(target_os = "none")]
struct WorkerEipc(EipcClient);
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::WorkerClient for WorkerEipc {
    fn propose(
        &mut self,
        telemetry_wire: &[u8],
    ) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        self.0.call("svc-ai-worker", telemetry_wire)
    }
}

#[cfg(target_os = "none")]
struct DispatchEipc(EipcClient);
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::DispatchClient for DispatchEipc {
    fn submit(&mut self, req: &[u8]) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        self.0.call("svc-control-dispatch", req)
    }
}

/// 未绑定客户端（端点 cap 未授予）——明确失败。
#[cfg(target_os = "none")]
struct UnboundTelemetryClient;
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::TelemetryClient for UnboundTelemetryClient {
    fn fetch_telemetry(&mut self) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        Err(alloc::string::String::from("telemetry-endpoint-unbound"))
    }
}
#[cfg(target_os = "none")]
struct UnboundWorkerClient;
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::WorkerClient for UnboundWorkerClient {
    fn propose(&mut self, _t: &[u8]) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        Err(alloc::string::String::from("worker-endpoint-unbound"))
    }
}
#[cfg(target_os = "none")]
struct UnboundDispatchClient;
#[cfg(target_os = "none")]
impl eneros_control_services::energy_agent_service::DispatchClient for UnboundDispatchClient {
    fn submit(&mut self, _r: &[u8]) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
        Err(alloc::string::String::from("dispatch-endpoint-unbound"))
    }
}

#[cfg(target_os = "none")]
struct EnergyAgentHandler<'a> {
    svc: &'a mut eneros_control_services::energy_agent_service::EnergyAgentService,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for EnergyAgentHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        if req.first() != Some(&1) {
            *resp = vec![0xFF];
            return true;
        }
        // 蓝图 57：now_ms 注入 tick——错误预算熔断冷却的时钟基准。
        *resp = self.svc.tick(now_ms);
        // 编排结果证据行（外部可观察：串口 = QEMU 外部观察面）。
        // 蓝图 66 WP-66.6.4：词形单源 = agent-sdk ea_wire::ea_display
        // （root :711 同源；4 臂手写 match 删除）。
        let status = resp
            .first()
            .map(|&c| eneros_agent_sdk::ea_wire::ea_display(c))
            .unwrap_or("INVALID");
        eneros_runtime::println!(
            "[svc-energy-agent] tick={} status={status}",
            self.svc.tick_count
        );
        true
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // 死亡必须可见（svc-device-agent 词形先例；svc-telemetry 同款订正）。
    eneros_runtime::println!("[svc-energy-agent] PANIC: {info}");
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
