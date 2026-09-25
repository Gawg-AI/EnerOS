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
    // 蓝图 68 卷 68.4（WP-68.4.1）：外部事实持久化面（SFS 解冻）。授予边
    // = 本服务对 svc-storage 既有边（eipc_granted_slot 单源）；Q0 写透
    // RPO=0；打开失败显式降级（FACT-FACE-UNAVAILABLE），不伪造持久化。
    // 事实 transport（market peer / sealed 信封通道）未部署 ⇒ sink 生产
    // 在位零消费（跨 bin 接线登记后续卷，68.4 执行卡 WP-68.4.1）。
    let fact_face = eneros_agent_sdk::eipc_granted_slot("svc-market", eipc_mask, "svc-storage")
        .and_then(|slot| eneros_agent_sdk::block_io::eipc_block_io(slot, 0, 5_000))
        .and_then(|io| {
            match eneros_business_services::fact_face::open_fact_face(
                io,
                eneros_ota_ab::ab_partition::DataRegion::FactFace,
            ) {
                Ok(face) => Some(face),
                Err(e) => {
                    // WP-75.2.5（R16-R）：降级词形两态分立——本词形仅
                    // 区段打开失败（授予缺失/绑定失败仍落
                    // FACT-FACE-UNAVAILABLE）；行为不变仍降级 None。
                    eneros_runtime::println!("[svc-market] FACT-FACE-OPEN-FAILED {e:?}");
                    None
                }
            }
        });
    let fact_sink = match fact_face {
        Some((mut sink, report)) => {
            let consistent = report.bad_segments == 0 && report.offset_gaps == 0;
            // 修订水位重建（重启后跨实例新修订仍单调；失败显式可见）。
            let wm_ok = sink.recover().is_ok();
            eneros_runtime::println!(
                "[svc-market] FACT-FACE-OPEN base_block={} region_blocks_512={} recovered={} consistent={} wm={}",
                eneros_ota_ab::ab_partition::DATA_FACT_BASE_BLOCK,
                eneros_ota_ab::ab_partition::DATA_FACT_BLOCKS,
                report.valid_records,
                consistent,
                wm_ok
            );
            Some(sink)
        }
        None => {
            eneros_runtime::println!("[svc-market] FACT-FACE-UNAVAILABLE");
            None
        }
    };
    // 真实 transport 依赖外部 market peer（QEMU 未提供）→ Unavailable
    // transport：任何发送显式失败（不伪造 peer 副作用）；outbox 在存储
    // 授予时使用 AppendOutbox 真落盘，未授予时显式 Unavailable。
    // WP-54.5：附接四件事（grant 探测/设备打开/open 分流/Unavailable 兜底+日志）
    // 收编 target_durable::attach 单实现；文案与收编前逐字一致。
    let outbox = eneros_business_services::target_durable::attach(
        eipc_mask & 1 != 0,
        "[svc-market]",
        eneros_agent_sdk::EIPC_EP_BASE_SLOT,
        MARKET_OUTBOX_OFFSET,
        MARKET_OUTBOX_SLOT_LEN,
        MARKET_OUTBOX_MAX_SLOTS,
    );
    eneros_runtime::println!("[svc-market] market adapter up (peer not deployed -> disabled)");
    let mut handler = MarketHandler {
        adapter: eneros_business_services::market::MarketAdapter::new(
            outbox,
            DisabledMarketTransport,
        ),
        // SFS 解冻面：事实 transport 到位前零消费（68.4 执行卡 WP-68.4.1
        // 登记）——持有即生产状态（已 recover、可观测），非死字段。
        #[allow(dead_code)]
        fact_sink,
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
// 蓝图 70.2 WP-70.2.4：事实 outbox 坐标单源 = DataRegion::MarketOutbox。
const MARKET_OUTBOX_OFFSET: u64 = eneros_ota_ab::ab_partition::DataRegion::MarketOutbox.base_byte();
#[cfg(target_os = "none")]
const MARKET_OUTBOX_SLOT_LEN: usize = 256;
#[cfg(target_os = "none")]
const MARKET_OUTBOX_MAX_SLOTS: usize = 8;

/// 市场对端未部署 → **显式 disabled**（[11] §5.9 退出 gate：若某外部
/// 系统未部署则服务明确 disabled，不宣称已接入——旧 UnavailableTransport
/// 语义收编为本状态）。
#[cfg(target_os = "none")]
#[derive(Debug, Clone, Copy)]
struct DisabledMarketTransport;

#[cfg(target_os = "none")]
impl eneros_business_services::market::QuoteTransport for DisabledMarketTransport {
    fn send_quote(
        &mut self,
        _quote: &eneros_business_services::market::QuoteMessage,
    ) -> Result<(), eneros_business_services::market::MarketError> {
        Err(eneros_business_services::market::MarketError::Transport(
            "market-peer-disabled",
        ))
    }
}

#[cfg(target_os = "none")]
struct MarketHandler {
    adapter: eneros_business_services::market::MarketAdapter<
        eneros_business_services::target_durable::MaybeOutbox<
            eneros_business_services::market::QuoteMessage,
        >,
        DisabledMarketTransport,
    >,
    /// 外部事实持久化 sink（68.4 SFS 解冻；`Option` = 未授予降级态）。
    #[allow(dead_code)]
    fact_sink: Option<
        eneros_business_services::fact_face::FactSink<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for MarketHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::market::*;
        const MAGIC: &[u8; 4] = b"MKT1";
        if eneros_business_services::svc_wire::parse_envelope(MAGIC, req).is_none() {
            resp.extend_from_slice(eneros_business_services::svc_wire::ERR_PROTO);
            return false;
        }
        match req[4] {
            0x01 => {
                // WP-75.2.6（R16-C）：载荷 decode 下沉 mkt_wire（偏移/枚举
                // 映射/错误序 golden 锁定）——bin 只剩 match 臂。
                let quote = match eneros_business_services::mkt_wire::parse_submit_req(req) {
                    Ok(q) => q,
                    Err(bytes) => {
                        resp.extend_from_slice(bytes);
                        return false;
                    }
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
                // peer 未部署（transport disabled）→ 无签名回执路径：
                // **不验签不回执**（[11] §5.2）；显式拒绝，不宣称已接入。
                resp.extend_from_slice(b"ERR:peer-disabled");
                false
            }
            0x04 => {
                // 外部系统接入状态（[11] §5.9：未部署 → 明确 disabled）。
                resp.push(0); // 0 = disabled（peer 未部署）
                resp.extend_from_slice(b"market-peer-disabled");
                true
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
                resp.extend_from_slice(eneros_business_services::svc_wire::ERR_CMD);
                false
            }
        }
    }
}

eneros_business_services::svc_panic!("svc-market");

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
