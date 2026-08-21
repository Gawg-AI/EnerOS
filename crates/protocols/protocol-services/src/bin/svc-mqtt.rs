//! svc-mqtt — 协议 EL0 服务（46 WP-46.2 句柄移交 + 46.3 配置驱动装配 +
//! 47 WP-47.1 协议入站写：下行写命令 → 意图导出 → 转发 svc-control-dispatch）。
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::EmptyPointRegistry;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 47（WP-47.1）：写意图转发服务——插件 poll（含下行写解析）+ 意图
/// EIPC 上报 svc-control-dispatch（`DISP_SUBMIT_INTENT` + 意图线码 +
/// actor）。dispatch 端点未授予 → 意图不转发（fail-closed，串口登记）。
#[cfg(target_os = "none")]
struct MqttWriteIoService {
    io: eneros_protocol_services::service::PluginIoService<eneros_mqtt::plugin::MqttPlugin>,
    /// svc-control-dispatch EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT + 1`；
    /// 槽 0 = svc-config 协议配置）。
    dispatch: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 分块传输 epoch（单调）。
    dispatch_epoch: u64,
    /// 转发成功计数（回执 Committed/Failed/Indeterminate 均算已受理）。
    pub forwarded_total: u64,
    /// 转发被拒计数（服务侧拒绝/回执 DISP_ERR）。
    pub rejected_total: u64,
}

#[cfg(target_os = "none")]
impl MqttWriteIoService {
    fn forward_intents(&mut self) {
        let now = eneros_protocol_services::now_ms();
        let mut intents: alloc::vec::Vec<eneros_protocol_abstract::write::PointWriteIntent> =
            alloc::vec::Vec::new();
        self.io.drain_write_intents(&mut intents);
        if intents.is_empty() {
            return;
        }
        for intent in intents {
            // 请求布局：[DISP_SUBMIT_INTENT=2][intent wire 49B][actor 2=Ai]。
            let mut req: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(1 + 49 + 1);
            req.push(2);
            req.extend_from_slice(&eneros_protocol_abstract::write::encode_intent(&intent));
            req.push(2);
            let Some(client) = &mut self.dispatch else {
                eneros_runtime::println!(
                    "[svc-mqtt] INTENT-FWD dispatch EP NOT granted —— 意图未转发（fail-closed）"
                );
                self.rejected_total = self.rejected_total.saturating_add(1);
                continue;
            };
            let mut transfer =
                eneros_agent_sdk::ChunkedClient::new(client, self.dispatch_epoch, 5_000);
            self.dispatch_epoch = self.dispatch_epoch.saturating_add(1);
            match transfer.transfer("svc-control-dispatch", &req) {
                Ok(resp) if resp.len() >= 2 && resp[0] == 0 => {
                    self.forwarded_total = self.forwarded_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD protocol=mqtt device={} point={} power={:?} seq={} state={} total={}",
                        intent.device_id,
                        u32::from(intent.point_id),
                        intent.value,
                        intent.sequence,
                        resp[1],
                        self.forwarded_total
                    );
                }
                Ok(resp) => {
                    self.rejected_total = self.rejected_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD rejected {:02x?}",
                        &resp[..resp.len().min(16)]
                    );
                }
                Err(e) => {
                    self.rejected_total = self.rejected_total.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] INTENT-FWD eipc={e:?}");
                }
            }
        }
        let _ = now;
    }
}

#[cfg(target_os = "none")]
impl eneros_drv_svc::runtime::DriverService for MqttWriteIoService {
    fn capability(&self) -> Option<&'static str> {
        self.io.capability()
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_drv_svc::runtime::*;
        if req.opcode == OP_EXECUTE {
            self.io.handle(req, resp);
            // 47（WP-47.1）：本轮 poll 解析到的下行写意图 → dispatch 授权链。
            self.forward_intents();
        } else {
            resp.status = ST_INVALID;
        }
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_mqtt::plugin::MqttPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] mqtt plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 46.2/46.3 + 47：手柄移交 → 配置装配插件 → 意图转发服务。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        ) {
            Ok(stack) => {
                let mut factory =
                    eneros_protocol_services::net_transport::NetTransportFactory::new(stack);
                let mut plugin = eneros_mqtt::plugin::MqttPlugin::new();
                // 46.3：配置即 peer/app_id 唯一来源（protocol-mqtt）；不可得
                // → 能力壳（fail-closed，不猜默认）。
                match eneros_protocol_services::service_cfg::fetch_protocol_config(
                    "mqtt-config",
                    eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                    "protocol-mqtt",
                ) {
                    Some(cfg) => match plugin.start(&cfg, &mut factory, &mut EmptyPointRegistry) {
                        Ok(()) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] NET-TRANSPORT READY capability=eneros_mqtt::plugin::MqttPlugin"
                            );
                            // 47：dispatch 端点在槽 base+1（表序第 2 条授予）。
                            let dispatch = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                                "mqtt-dispatch",
                                eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1,
                                eneros_drv_svc::now_ns,
                            )
                            .ok();
                            if dispatch.is_none() {
                                eneros_runtime::println!(
                                    "[svc-mqtt] dispatch EIPC NOT granted —— 写意图路径 fail-closed"
                                );
                            }
                            let mut io = MqttWriteIoService {
                                io: eneros_protocol_services::service::PluginIoService::new(
                                    plugin,
                                    "eneros_mqtt::plugin::MqttPlugin",
                                ),
                                dispatch,
                                dispatch_epoch: 0,
                                forwarded_total: 0,
                                rejected_total: 0,
                            };
                            // 46.5：启动期自检轮询（post-connect 交换尽早完成）。
                            io.io.self_check_poll(150);
                            io.forward_intents();
                            Box::new(io)
                        }
                        Err(e) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] plugin start FAILED {:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                                e
                            );
                            Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                        }
                    },
                    None => {
                        eneros_runtime::println!(
                            "[svc-mqtt] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
                        );
                        Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                    }
                }
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-mqtt] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "mqtt",
        ready_ntfn,
        eneros_protocol_services::now_ms,
        eneros_protocol_services::instance_epoch(),
        &mut *service,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
