//! svc-observability — target 可观测队列 EL0（蓝图 41 WP-41.12）。
//! EIPC 协议：magic "OBS1" ‖ cmd ‖ payload。
//! - 0x01 ENQUEUE：class(1)‖service(16)‖schema(8)‖created(8)‖payload(≤512) → ok(1)
//! - 0x02 FLUSH：batch(4) → exported(8)
//! - 0x03 STATS：→ enqueued(8)‖dropped(8)‖redacted(8)‖exported(8)‖failed(8)
//!
//! exporter 未接通（外部 AgentBus peer 缺）→ flush 显式失败不丢数据。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

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
    let mut svc = eneros_business_services::observability::ObservabilityService::new(
        UnavailableExporter,
        eneros_business_services::observability::OverflowPolicy::DropLowPriority,
        64,
    );
    // 存储授予时挂接真 durable outbox（AppendOutbox over BlockIo）。
    if eipc_mask & 1 != 0 {
        if let Some(io) = eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        ) {
            let mut ob = eneros_durable_state::AppendOutbox::new(
                io,
                OBS_OUTBOX_OFFSET,
                OBS_OUTBOX_SLOT_LEN,
                OBS_OUTBOX_MAX_SLOTS,
            );
            match ob.open() {
                Ok(()) => {
                    eneros_runtime::println!(
                        "[svc-observability] durable telemetry outbox attached (svc-storage)"
                    );
                    svc.durable = Some(alloc::boxed::Box::new(
                        eneros_business_services::target_durable::MaybeOutbox::Real(ob),
                    ));
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-observability] durable outbox open FAILED {:?} —— 显式 Unavailable",
                        e
                    );
                }
            }
        } else {
            eneros_runtime::println!(
                "[svc-observability] storage NOT granted -> durable outbox Unavailable"
            );
        }
    } else {
        eneros_runtime::println!(
            "[svc-observability] storage NOT granted -> durable outbox Unavailable"
        );
    }
    eneros_runtime::println!("[svc-observability] target telemetry queue up");
    let mut handler = ObsHandler { svc };
    eneros_control_services::control_service_main(
        ep,
        "observability",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
const OBS_OUTBOX_OFFSET: u64 = 57344;
#[cfg(target_os = "none")]
const OBS_OUTBOX_SLOT_LEN: usize = 1024;
#[cfg(target_os = "none")]
const OBS_OUTBOX_MAX_SLOTS: usize = 4;

#[cfg(target_os = "none")]
struct UnavailableExporter;

#[cfg(target_os = "none")]
impl eneros_business_services::observability::TelemetryExporter for UnavailableExporter {
    fn export_batch(
        &mut self,
        _items: &[eneros_business_services::observability::TelemetryItem],
    ) -> Result<eneros_durable_state::PeerReceipt, &'static str> {
        Err("agentbus-exporter-unavailable")
    }
}

#[cfg(target_os = "none")]
struct ObsHandler {
    svc: eneros_business_services::observability::ObservabilityService<UnavailableExporter>,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for ObsHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::observability::*;
        const MAGIC: &[u8; 4] = b"OBS1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        match req[4] {
            0x01 => {
                if req.len() < 5 + 34 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let p = &req[5..];
                let class = match p[0] {
                    0 => TelemetryClass::SecurityAudit,
                    1 => TelemetryClass::Control,
                    2 => TelemetryClass::Alarm,
                    3 => TelemetryClass::Telemetry,
                    4 => TelemetryClass::Log,
                    5 => TelemetryClass::Debug,
                    _ => {
                        resp.extend_from_slice(b"ERR:class");
                        return false;
                    }
                };
                let mut service = [0u8; 16];
                service.copy_from_slice(&p[1..17]);
                let service = alloc::string::String::from_utf8_lossy(&service).into_owned();
                let mut schema = [0u8; 8];
                schema.copy_from_slice(&p[17..25]);
                let schema = alloc::string::String::from_utf8_lossy(&schema).into_owned();
                let created = u64::from_le_bytes(p[25..33].try_into().unwrap());
                let plen = p[33] as usize;
                if 5 + 34 + plen > req.len() {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let item = TelemetryItem {
                    class,
                    service,
                    schema,
                    payload: req[5 + 34..5 + 34 + plen].to_vec(),
                    created_ms: created,
                    boot_id: 1,
                    config_generation: 0,
                };
                match self.svc.enqueue(item) {
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
            0x02 => {
                if req.len() != 9 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let batch = u32::from_le_bytes(req[5..9].try_into().unwrap()) as usize;
                match self.svc.flush_batch(batch) {
                    Ok(n) => {
                        resp.extend_from_slice(&(n as u64).to_le_bytes());
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
                let s = &self.svc.stats;
                resp.extend_from_slice(&s.enqueued.to_le_bytes());
                resp.extend_from_slice(&s.dropped.values().sum::<u64>().to_le_bytes());
                resp.extend_from_slice(&s.redacted.to_le_bytes());
                resp.extend_from_slice(&s.exported.to_le_bytes());
                resp.extend_from_slice(&s.export_failed.to_le_bytes());
                let _ = now_ms;
                true
            }
            _ => {
                resp.extend_from_slice(b"ERR:cmd");
                false
            }
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
