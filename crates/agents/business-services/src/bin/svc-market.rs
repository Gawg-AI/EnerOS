//! svc-market — 市场接入 EL0（蓝图 41 WP-41.4）。
//! EIPC 协议：magic "MKT1" ‖ cmd ‖ payload。
//! - 0x01 SUBMIT：external_id(8)‖idem(16)‖deadline(8)‖market(1)‖tenant(4)‖product(4)‖
//!   start(8)‖end(8)‖volume(8)‖price(8)‖amount(8) → status(1)‖outbox_id(8)
//! - 0x02 ACK：outbox_id(8)‖receipt_id(8) → status(1)
//! - 0x03 RECONCILE：→ resent 数(8)
//!
//! 分层成功语义：Queued=0 / Sent=1 / PeerAcked=2 / Rejected=3。

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
    // 真实 transport 依赖外部 market peer（QEMU 未提供）→ Unavailable
    // transport：任何发送显式失败（不伪造 peer 副作用）；outbox 在存储
    // 授予时使用 AppendOutbox 真落盘，未授予时显式 Unavailable。
    let outbox = if eipc_mask & 1 != 0 {
        match eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        ) {
            Some(io) => {
                let mut ob = eneros_durable_state::AppendOutbox::new(
                    io,
                    MARKET_OUTBOX_OFFSET,
                    MARKET_OUTBOX_SLOT_LEN,
                    MARKET_OUTBOX_MAX_SLOTS,
                );
                match ob.open() {
                    Ok(()) => {
                        eneros_runtime::println!(
                            "[svc-market] durable outbox attached (svc-storage)"
                        );
                        eneros_business_services::target_durable::MaybeOutbox::Real(ob)
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-market] durable outbox open FAILED {:?} —— 显式 Unavailable",
                            e
                        );
                        eneros_business_services::target_durable::MaybeOutbox::Unavailable(
                            eneros_durable_state::UnavailableOutbox::new(),
                        )
                    }
                }
            }
            None => {
                eneros_runtime::println!("[svc-market] storage NOT granted -> outbox Unavailable");
                eneros_business_services::target_durable::MaybeOutbox::Unavailable(
                    eneros_durable_state::UnavailableOutbox::new(),
                )
            }
        }
    } else {
        eneros_runtime::println!("[svc-market] storage NOT granted -> outbox Unavailable");
        eneros_business_services::target_durable::MaybeOutbox::Unavailable(
            eneros_durable_state::UnavailableOutbox::new(),
        )
    };
    eneros_runtime::println!("[svc-market] market adapter up");
    let mut handler = MarketHandler {
        adapter: eneros_business_services::market::MarketAdapter::new(outbox, UnavailableTransport),
    };
    eneros_control_services::control_service_main(
        ep,
        "market",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
const MARKET_OUTBOX_OFFSET: u64 = 32768;
#[cfg(target_os = "none")]
const MARKET_OUTBOX_SLOT_LEN: usize = 256;
#[cfg(target_os = "none")]
const MARKET_OUTBOX_MAX_SLOTS: usize = 8;

#[cfg(target_os = "none")]
struct UnavailableTransport;

#[cfg(target_os = "none")]
impl eneros_business_services::market::QuoteTransport for UnavailableTransport {
    fn send_quote(
        &mut self,
        _quote: &eneros_business_services::market::QuoteMessage,
    ) -> Result<(), eneros_business_services::market::MarketError> {
        Err(eneros_business_services::market::MarketError::Transport(
            "market-peer-unavailable",
        ))
    }
}

#[cfg(target_os = "none")]
struct MarketHandler {
    adapter: eneros_business_services::market::MarketAdapter<
        eneros_business_services::target_durable::MaybeOutbox<
            eneros_business_services::market::QuoteMessage,
        >,
        UnavailableTransport,
    >,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for MarketHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::market::*;
        use eneros_durable_state::{IdempotencyKey, OutboxId, PeerReceipt};
        use eneros_market_types::{MilliYuan, TimeSlot, YuanPerMwh};
        const MAGIC: &[u8; 4] = b"MKT1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        match req[4] {
            0x01 => {
                if req.len() != 5 + 93 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let p = &req[5..];
                let market = match p[32] {
                    1 => eneros_market_types::MarketType::DayAhead,
                    2 => eneros_market_types::MarketType::IntraDay,
                    3 => eneros_market_types::MarketType::Realtime,
                    4 => eneros_market_types::MarketType::Spot,
                    5 => eneros_market_types::MarketType::AncillaryService,
                    6 => eneros_market_types::MarketType::DemandResponse,
                    _ => {
                        resp.extend_from_slice(b"ERR:market");
                        return false;
                    }
                };
                let quote = QuoteMessage {
                    quote_ref: QuoteRef {
                        external_id: u64::from_le_bytes(p[0..8].try_into().unwrap()),
                        idempotency: IdempotencyKey(p[8..24].try_into().unwrap()),
                        deadline_ms: u64::from_le_bytes(p[24..32].try_into().unwrap()),
                        market,
                        tenant: u32::from_le_bytes(p[33..37].try_into().unwrap()),
                        product: u32::from_le_bytes(p[37..41].try_into().unwrap()),
                    },
                    slot: TimeSlot {
                        start: u64::from_le_bytes(p[41..49].try_into().unwrap()),
                        end: u64::from_le_bytes(p[49..57].try_into().unwrap()),
                    },
                    volume_kwh: u64::from_le_bytes(p[57..65].try_into().unwrap()),
                    price: YuanPerMwh(f64::from_le_bytes(p[65..73].try_into().unwrap())),
                    amount: MilliYuan(i64::from_le_bytes(p[73..81].try_into().unwrap())),
                };
                match self.adapter.submit_quote(quote, now_ms) {
                    Ok(outcome) => {
                        let status = match outcome.status {
                            SubmitStatus::Queued => 0u8,
                            SubmitStatus::Sent => 1,
                            SubmitStatus::PeerAcked => 2,
                            SubmitStatus::Rejected => 3,
                        };
                        resp.push(status);
                        resp.extend_from_slice(&outcome.outbox_id.0.to_le_bytes());
                        true
                    }
                    Err(_) => {
                        resp.extend_from_slice(b"ERR:submit");
                        false
                    }
                }
            }
            0x02 => {
                if req.len() != 5 + 16 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let p = &req[5..];
                let id = OutboxId(u64::from_le_bytes(p[0..8].try_into().unwrap()));
                let receipt = PeerReceipt {
                    peer: "market-peer".into(),
                    receipt_id: u64::from_le_bytes(p[8..16].try_into().unwrap()),
                    at_ms: now_ms,
                };
                match self.adapter.handle_peer_ack(id, receipt) {
                    Ok(()) => {
                        resp.push(1);
                        true
                    }
                    Err(_) => {
                        resp.push(0);
                        false
                    }
                }
            }
            0x03 => match self.adapter.reconcile_pending(now_ms) {
                Ok(resent) => {
                    resp.extend_from_slice(&(resent.len() as u64).to_le_bytes());
                    true
                }
                Err(_) => {
                    resp.extend_from_slice(b"ERR:reconcile");
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
