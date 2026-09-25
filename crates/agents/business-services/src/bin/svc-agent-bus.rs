//! svc-agent-bus — MQTT AgentBus EL0（蓝图 41 WP-41.8）。
//! EIPC 协议：magic "BUS1" ‖ cmd ‖ payload。
//! - 0x01 PUBLISH：topic(≤64)‖qos(1)‖payload(≤256) → ok/denied
//! - 0x02 SUBSCRIBE：topic(≤64) → ok
//! - 0x03 ACK：packet_id(2) → ok
//!
//! broker transport 未接通（外部 broker 缺）→ PUBLISH 显式 Unavailable。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]


#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
extern crate eneros_business_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_business_services::HeapAllocator =
    eneros_business_services::HeapAllocator::new();

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
    let acl = eneros_business_services::agent_bus::TopicAcl {
        allowed_prefixes: alloc::vec::Vec::new(), // 默认拒绝（41 装配期注入租户/设备前缀）
    };
    let state = if eipc_mask & 1 != 0 {
        match eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        ) {
            Some(io) => {
                let mut store = eneros_durable_state::RegionStateStore::new(
                    io,
                    AGENTBUS_STATE_OFFSET,
                    AGENTBUS_STATE_LEN,
                );
                match store.open() {
                    Ok(()) => {
                        eneros_runtime::println!(
                            "[svc-agent-bus] durable cursor store attached (svc-storage)"
                        );
                        eneros_business_services::target_durable::MaybeState::Real(store)
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-agent-bus] durable cursor store open FAILED {:?} —— 显式 Unavailable",
                            e
                        );
                        eneros_business_services::target_durable::MaybeState::Unavailable(
                            eneros_durable_state::UnavailableStateStore::new(),
                        )
                    }
                }
            }
            None => {
                eneros_runtime::println!(
                    "[svc-agent-bus] storage NOT granted -> cursor Unavailable"
                );
                eneros_business_services::target_durable::MaybeState::Unavailable(
                    eneros_durable_state::UnavailableStateStore::new(),
                )
            }
        }
    } else {
        eneros_runtime::println!("[svc-agent-bus] storage NOT granted -> cursor Unavailable");
        eneros_business_services::target_durable::MaybeState::Unavailable(
            eneros_durable_state::UnavailableStateStore::new(),
        )
    };
    let mut handler = BusHandler {
        bus: eneros_business_services::agent_bus::AgentBus::new(state, acl),
    };
    // WP-75.2.5（R16-R）：恢复失败显式可见（原 `.ok()` 全吞零日志）；
    // 行为不变——失败仍继续启动（cursor 保持初始态）。
    if let Err(e) = handler.bus.restore() {
        eneros_runtime::println!("[svc-agent-bus] BUS-RESTORE-FAILED {e:?}");
    }
    eneros_runtime::println!(
        "[svc-agent-bus] AgentBus up (broker transport not granted -> publish Unavailable)"
    );
    eneros_control_services::control_service_main(
        ep,
        "agent-bus",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
// 蓝图 70.2 WP-70.2.4：总线状态区坐标单源 = DataRegion::AgentBusState。
const AGENTBUS_STATE_OFFSET: u64 =
    eneros_ota_ab::ab_partition::DataRegion::AgentBusState.base_byte();
#[cfg(target_os = "none")]
const AGENTBUS_STATE_LEN: u64 = 1024;

#[cfg(target_os = "none")]
struct BusHandler {
    bus: eneros_business_services::agent_bus::AgentBus<
        eneros_business_services::target_durable::MaybeState<
            eneros_business_services::agent_bus::SessionCursor,
        >,
    >,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for BusHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        const MAGIC: &[u8; 4] = b"BUS1";
        if eneros_business_services::svc_wire::parse_envelope(MAGIC, req).is_none() {
            resp.extend_from_slice(eneros_business_services::svc_wire::ERR_PROTO);
            return false;
        }
        match req[4] {
            0x01 => {
                // broker 未接通：显式 Unavailable（不伪造 publish 成功）。
                resp.extend_from_slice(b"ERR:broker-transport-unavailable");
                false
            }
            0x02 => {
                let topic = core::str::from_utf8(&req[5..]).unwrap_or("");
                match self.bus.subscribe(topic) {
                    Ok(()) => {
                        resp.push(1);
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(e.as_bytes());
                        false
                    }
                }
            }
            0x03 => {
                if req.len() != 7 {
                    resp.extend_from_slice(eneros_business_services::svc_wire::ERR_LEN);
                    return false;
                }
                let pid = u16::from_le_bytes(req[5..7].try_into().unwrap());
                match self.bus.ack_publish(pid) {
                    Ok(()) => {
                        resp.push(1);
                        true
                    }
                    Err(_) => {
                        resp.extend_from_slice(b"ERR:ack");
                        false
                    }
                }
            }
            _ => {
                resp.extend_from_slice(eneros_business_services::svc_wire::ERR_CMD);
                false
            }
        }
    }
}

eneros_business_services::svc_panic!("svc-agent-bus");

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
