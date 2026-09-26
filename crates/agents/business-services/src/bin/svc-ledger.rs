//! svc-ledger — 双分录账本 EL0（蓝图 41 WP-41.5）。
//! EIPC 协议：magic "LDG1" ‖ cmd ‖ payload。
//! - 0x01 POST：tx_id(8)‖debit(4)‖credit(4)‖amount(8)‖time(8)‖receipt(32) → entry_id(8)
//! - 0x02 BALANCE：account(4) → debit(8)‖credit(8)‖net(8)
//! - 0x03 COUNT：→ entries(8)
//!
//! fail-closed：账本错误（重复/同账户/零额/无计量）→ 结构化错误响应。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
use alloc::string::ToString;

#[cfg(target_os = "none")]
use eneros_durable_state::DurableStateStore;
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
    // 41：账本经 EIPC 调 svc-storage 真落盘（RegionStateStore<LedgerSnapshot>）。
    // EIPC 授予表按客户端独立计数：svc-ledger 是首个 storage 客户端 →
    // 槽 EIPC_EP_BASE_SLOT+0、mask bit 0。
    let mut storage = if eipc_mask & 1 != 0 {
        eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        )
    } else {
        None
    };
    let mut ledger = eneros_business_services::Ledger::new();
    if let Some(io) = storage.as_mut() {
        let mut store: eneros_durable_state::RegionStateStore<
            eneros_business_services::ledger::LedgerSnapshot,
            _,
        > = eneros_durable_state::RegionStateStore::new(
            io,
            LEDGER_PERSIST_OFFSET,
            LEDGER_PERSIST_LEN,
        );
        if store.open().is_ok() {
            if let Ok(Some(v)) = store.load() {
                ledger = v.state.0;
                ledger.snapshot_generation = Some(v.generation);
                eneros_runtime::println!(
                    "[svc-ledger] durable ledger restored gen={} entries={}",
                    v.generation.0,
                    ledger.entries().len()
                );
            } else {
                eneros_runtime::println!("[svc-ledger] durable ledger fresh (no snapshot)");
            }
        } else {
            eneros_runtime::println!("[svc-ledger] durable ledger open FAILED —— 显式 Unavailable");
        }
        eneros_runtime::println!("[svc-ledger] double-entry ledger up (durable via svc-storage)");
    } else {
        eneros_runtime::println!(
            "[svc-ledger] double-entry ledger up (storage NOT granted -> persistence unavailable)"
        );
    }
    let mut handler = LedgerHandler { ledger, storage };
    eneros_control_services::control_service_main(
        ep,
        "ledger",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
// 蓝图 70.2 WP-70.2.4：账本状态区坐标单源 = DataRegion::LedgerState。
const LEDGER_PERSIST_OFFSET: u64 = eneros_ota_ab::ab_partition::DataRegion::LedgerState.base_byte();
#[cfg(target_os = "none")]
const LEDGER_PERSIST_LEN: u64 = 1024;

#[cfg(target_os = "none")]
struct LedgerHandler {
    ledger: eneros_business_services::ledger::Ledger,
    storage: Option<
        eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
}

#[cfg(target_os = "none")]
impl LedgerHandler {
    fn persist(&mut self) -> Result<(), alloc::string::String> {
        let io = self
            .storage
            .as_mut()
            .ok_or_else(|| "storage-unavailable".to_string())?;
        let mut store: eneros_durable_state::RegionStateStore<
            eneros_business_services::ledger::LedgerSnapshot,
            _,
        > = eneros_durable_state::RegionStateStore::new(
            io,
            LEDGER_PERSIST_OFFSET,
            LEDGER_PERSIST_LEN,
        );
        store.open().map_err(|e| alloc::format!("open:{e}"))?;
        let expected = self.ledger.snapshot_generation;
        let snap = eneros_business_services::ledger::LedgerSnapshot(self.ledger.clone());
        let gen = store
            .compare_and_swap(expected, &snap)
            .map_err(|e| alloc::format!("cas:{e}"))?;
        self.ledger.snapshot_generation = Some(gen);
        Ok(())
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for LedgerHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use alloc::string::ToString;

        use eneros_business_services::ledger::*;
        const MAGIC: &[u8; 4] = b"LDG1";
        if eneros_business_services::svc_wire::parse_envelope(MAGIC, req).is_none() {
            resp.extend_from_slice(eneros_business_services::svc_wire::ERR_PROTO);
            return false;
        }
        match req[4] {
            0x01 => {
                // WP-75.2.6（R16-C）：载荷 decode 下沉 ldg_wire（偏移
                // golden 锁定）——bin 只剩 match 臂。
                let (tx, debit, credit, amount, time, receipt) =
                    match eneros_business_services::ldg_wire::parse_post_req(req) {
                        Ok(f) => f,
                        Err(bytes) => {
                            resp.extend_from_slice(bytes);
                            return false;
                        }
                    };
                let result = self.ledger.post(tx, debit, credit, amount, time, receipt);
                match result {
                    Ok(id) => {
                        // 持久化账本快照；失败 → 返回错误（不伪装成功）。
                        if let Err(e) = self.persist() {
                            resp.extend_from_slice(b"ERR:persist:");
                            resp.extend_from_slice(e.as_bytes());
                            return false;
                        }
                        resp.extend_from_slice(&id.0.to_le_bytes());
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(e.to_string().as_bytes());
                        false
                    }
                }
            }
            0x02 => {
                if req.len() != 9 {
                    resp.extend_from_slice(eneros_business_services::svc_wire::ERR_LEN);
                    return false;
                }
                let account = AccountId(u32::from_le_bytes(req[5..9].try_into().unwrap()));
                let b = self.ledger.balance_of(account);
                resp.extend_from_slice(&b.debit_total.0.to_le_bytes());
                resp.extend_from_slice(&b.credit_total.0.to_le_bytes());
                resp.extend_from_slice(&b.net.0.to_le_bytes());
                true
            }
            0x03 => {
                resp.extend_from_slice(&(self.ledger.entries().len() as u64).to_le_bytes());
                true
            }
            _ => {
                resp.extend_from_slice(eneros_business_services::svc_wire::ERR_CMD);
                false
            }
        }
    }
}

eneros_runtime::svc_panic!("svc-ledger");

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
