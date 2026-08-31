//! svc-cloud-sync — CloudSync durable outbox EL0（蓝图 41 WP-41.10）。
//! EIPC 协议：magic "CLD1" ‖ cmd ‖ payload。
//! - 0x01 UPLOAD：tenant(4)‖class(1)‖schema(8)‖seq(8)‖hash(32)‖len(2)‖payload → outbox_id(8)
//! - 0x02 FLUSH：batch(4) → cursor(8)
//! - 0x03 RECONCILE：→ resent(8)
//!
//! 外部云端 peer 缺 → transport Unavailable → flush 显式失败（数据不丢）。

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
    // WP-54.5：附接四件事收编 target_durable::attach 单实现；文案逐字一致。
    let outbox = eneros_business_services::target_durable::attach(
        eipc_mask & 1 != 0,
        "[svc-cloud-sync]",
        eneros_agent_sdk::EIPC_EP_BASE_SLOT,
        CLOUD_OUTBOX_OFFSET,
        CLOUD_OUTBOX_SLOT_LEN,
        CLOUD_OUTBOX_MAX_SLOTS,
    );
    eneros_runtime::println!(
        "[svc-cloud-sync] durable outbox up (cloud peer unavailable -> flush fails closed)"
    );
    let mut handler = CloudHandler {
        sync: eneros_business_services::cloud_sync::CloudSync::new(outbox, DisabledCloudTransport),
    };
    eneros_control_services::control_service_main(
        ep,
        "cloud-sync",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
const CLOUD_OUTBOX_OFFSET: u64 = 40960;
#[cfg(target_os = "none")]
const CLOUD_OUTBOX_SLOT_LEN: usize = 256;
#[cfg(target_os = "none")]
const CLOUD_OUTBOX_MAX_SLOTS: usize = 8;

/// 云对端未部署 → 显式 disabled（[11] §5.9 退出 gate：若某外部系统未
/// 部署则服务明确 disabled，不宣称已接入；旧名 UnavailableTransport 的
/// 行为与文案保持一致，仅状态语义显式化）。
#[cfg(target_os = "none")]
struct DisabledCloudTransport;

#[cfg(target_os = "none")]
impl eneros_business_services::cloud_sync::UploadTransport for DisabledCloudTransport {
    fn upload_batch(
        &mut self,
        _items: &[eneros_business_services::cloud_sync::UploadItem],
    ) -> Result<u64, &'static str> {
        Err("cloud-peer-unavailable (disabled)")
    }
}

#[cfg(target_os = "none")]
struct CloudHandler {
    sync: eneros_business_services::cloud_sync::CloudSync<
        eneros_business_services::target_durable::MaybeOutbox<
            eneros_business_services::cloud_sync::UploadItem,
        >,
        UnavailableTransport,
    >,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for CloudHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_business_services::cloud_sync::*;
        const MAGIC: &[u8; 4] = b"CLD1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        match req[4] {
            0x01 => {
                if req.len() < 5 + 56 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let p = &req[5..];
                let mut schema = [0u8; 8];
                schema.copy_from_slice(&p[5..13]);
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&p[21..53]);
                let plen = u16::from_le_bytes(p[53..55].try_into().unwrap()) as usize;
                if 5 + 55 + plen > req.len() {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let item = UploadItem {
                    tenant: u32::from_le_bytes(p[0..4].try_into().unwrap()),
                    data_class: p[4],
                    schema: alloc::string::String::from_utf8_lossy(&schema).into_owned(),
                    sequence: u64::from_le_bytes(p[13..21].try_into().unwrap()),
                    hash,
                    payload: req[5 + 55..5 + 55 + plen].to_vec(),
                };
                match self.sync.upload(item) {
                    Ok(id) => {
                        resp.extend_from_slice(&id.0.to_le_bytes());
                        true
                    }
                    Err(_) => {
                        resp.extend_from_slice(b"ERR:upload");
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
                match self.sync.flush(batch) {
                    Ok(cursor) => {
                        resp.extend_from_slice(&cursor.to_le_bytes());
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(e.as_bytes());
                        false
                    }
                }
            }
            0x03 => match self.sync.reconcile() {
                Ok(resent) => {
                    resp.extend_from_slice(&(resent.len() as u64).to_le_bytes());
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            },
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
