//! svc-ota — 系统 A/B OTA EL0（蓝图 41 WP-41.6）。
//! EIPC 协议：magic "OTA1" ‖ cmd ‖ payload。
//! - 0x01 CHECK：target_version(32)‖sku(16)‖secver(4) → verdict(1)
//! - 0x02 DOWNLOAD+VERIFY：meta（定长）→ step(1)
//! - 0x03 TRIAL：→ step(1)
//! - 0x04 COMMIT / 0x05 REGRESSION：→ step(1)
//! - 0x06 PERSIST-TRIAL：remaining(4)‖version(16) → gen(8)（trial 状态真
//!   落盘——经 EIPC 调 svc-storage 块命令面；存储未授予 → Unavailable）
//! - 0x07 LOAD-TRIAL：→ found(1)‖remaining(4)‖version(16)（重启恢复读回）
//!
//! 41（WP-41.3 落盘）：EIPC 授予表 `svc-ota → svc-storage`（X3 bit 2 =
//! 槽 `EIPC_EP_BASE_SLOT+2`）→ `BlockIo`（PersistIo over 块命令面）→
//! `RegionStateStore<TrialState>` 固定区域（数据盘 8 KiB 偏移，避开首块
//! 引导区）。未授予 → 全部存储步骤显式 Unavailable（不伪造持久化）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
use alloc::string::ToString;
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
    // 41（WP-41.3 落盘）：存储句柄移交——EIPC 授予表 `svc-ota →
    // svc-storage`（X3 bit 2 = 槽 EIPC_EP_BASE_SLOT+2）。授予 → 经块命令
    // 面真落盘（RegionStateStore<TrialState>）；未授予 → 显式 Unavailable
    // （不伪造持久化）。
    // EIPC 授予表序：svc-ota 是首个（唯一）storage 客户端 → 槽
    // EIPC_EP_BASE_SLOT+0、mask bit 0（eipc_grants_for 按服务独立计数）。
    let storage = if eipc_mask & 1 != 0 {
        eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0, // epoch 未知——首个响应吸收（svc-storage 实例 epoch）
            5_000,
        )
    } else {
        None
    };
    if storage.is_some() {
        eneros_runtime::println!(
            "[svc-ota] block storage GRANTED (EIPC slot {}) —— trial 状态真落盘",
            eneros_agent_sdk::EIPC_EP_BASE_SLOT
        );
    } else {
        eneros_runtime::println!(
            "[svc-ota] block storage NOT granted —— 持久化显式 Unavailable（不伪造）"
        );
    }
    // 41（OTA A/B 地基）：存储授予时构造真实 AbPartitionOta（经 EipcBlockDevice
    // 写数据盘 A/B 槽）。root_key 使用当前开发公钥（config-bundle-pubkey）；
    // 生产由 KMS 注入替换。
    let ota = if eipc_mask & 1 != 0 {
        match eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        ) {
            Some(io) => {
                // 69.5（WP-69.5.2）：生产块大小单源 = ota-ab
                // OTA_PROD_BLOCK_SIZE（512）——原手抄字面与 ota.toml ⑧
                // 的 chunk_size 心算双源分歧（生产实际单槽容量 1 MiB）即
                // 出于此；容量口径锁 production_slot_capacity_matches_
                // documented_baseline 与本调用同取一常量。盘上块号语义按
                // 512B 块绑定（DATA_DISK_BLOCKS 同源），Z15 不得改值。
                let dev = eneros_agent_sdk::block_io::EipcBlockDevice::new(
                    io,
                    eneros_ota_ab::ab_partition::OTA_PROD_BLOCK_SIZE,
                    eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS,
                );
                let mut key65 = [0u8; 65];
                key65[0] = 0x04;
                key65[1..].copy_from_slice(&ENEROS_OTA_ROOT_KEY);
                match eneros_crypto::Sm2PublicKey::from_bytes(&key65) {
                    Ok(pk) => {
                        let mgr = eneros_ota_ab::ab_partition::AbPartitionOta::new(
                            dev,
                            pk,
                            eneros_business_services::now_ms(),
                        );
                        eneros_runtime::println!(
                            "[svc-ota] real AbPartitionOta attached (A/B via svc-storage)"
                        );
                        Some(eneros_business_services::ota_service::OtaService::new(
                            mgr,
                            ENEROS_OTA_POLICY.clone(),
                            "0.0.0",
                            "qemu",
                            1,
                            1,
                        ))
                    }
                    Err(_) => {
                        eneros_runtime::println!(
                            "[svc-ota] OTA root key invalid —— A/B unavailable"
                        );
                        None
                    }
                }
            }
            None => None,
        }
    } else {
        None
    };
    let mut handler = OtaHandler { storage, ota };
    eneros_control_services::control_service_main(
        ep,
        "ota",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

/// 数据盘持久化区域（字节偏移；避开首块引导区/GPT 保护区）。
#[cfg(target_os = "none")]
const OTA_PERSIST_OFFSET: u64 = 8192;
#[cfg(target_os = "none")]
const OTA_PERSIST_LEN: u64 = 128;

#[cfg(target_os = "none")]
struct OtaHandler {
    storage: Option<
        eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    ota: Option<
        eneros_business_services::ota_service::OtaService<
            eneros_ota_ab::ab_partition::AbPartitionOta<
                eneros_agent_sdk::block_io::EipcBlockDevice<
                    eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
                >,
            >,
        >,
    >,
}

#[cfg(target_os = "none")]
impl OtaHandler {
    /// 打开 trial 状态存储（RegionIo 从介质读回；fail-closed：任何 IO
    /// 错误 → None → 显式 Unavailable）。
    fn trial_store(
        &mut self,
    ) -> Option<
        eneros_durable_state::RegionStateStore<
            eneros_business_services::ota_service::TrialState,
            &mut eneros_business_services::block_io::BlockIo<
                eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
            >,
        >,
    > {
        let io = self.storage.as_mut()?;
        let mut store =
            eneros_durable_state::RegionStateStore::new(io, OTA_PERSIST_OFFSET, OTA_PERSIST_LEN);
        store.open().ok()?;
        Some(store)
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for OtaHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_business_services::ota_service::*;
        use eneros_durable_state::DurableStateStore;
        const MAGIC: &[u8; 4] = b"OTA1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        // 0x01-0x05 由可测的纯逻辑函数处理（ota_service::handle_ota_ab_command）。
        if (0x01..=0x05).contains(&req[4]) {
            return handle_ota_ab_command(self.ota.as_mut(), req, resp);
        }
        match req[4] {
            // 41：trial 状态真落盘（RegionStateStore CAS；首次写入
            // expected=None → gen 1；重启后 LOAD 从介质读回）。
            0x06 => {
                // 蓝图 58：请求解析走 ota_wire 单源（与 root 探针同型）。
                let (remaining, version) =
                    match eneros_agent_sdk::ota_wire::parse_persist_trial_req(req) {
                        Ok(v) => v,
                        Err(_) => {
                            resp.extend_from_slice(b"ERR:len");
                            return false;
                        }
                    };
                let state = TrialState::new(remaining, &version);
                let mut store = match self.trial_store() {
                    Some(s) => s,
                    None => {
                        resp.extend_from_slice(b"ERR:store-unavailable");
                        return false;
                    }
                };
                // CAS：期望当前代次（load 已知）——不存在则 None（首次）。
                let expected = match store.load() {
                    Ok(Some(v)) => Some(v.generation),
                    _ => None,
                };
                match store.compare_and_swap(expected, &state) {
                    Ok(gen) => {
                        resp.extend_from_slice(&gen.0.to_le_bytes());
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:persist:");
                        resp.extend_from_slice(e.to_string().as_bytes());
                        false
                    }
                }
            }
            0x07 => {
                let mut store = match self.trial_store() {
                    Some(s) => s,
                    None => {
                        resp.extend_from_slice(b"ERR:store-unavailable");
                        return false;
                    }
                };
                match store.load() {
                    Ok(Some(v)) => {
                        resp.push(1); // found
                        resp.extend_from_slice(&v.state.remaining.to_le_bytes());
                        let vb = v.state.trial_version.as_bytes();
                        let mut name = [0u8; 16];
                        let n = core::cmp::min(vb.len(), 16);
                        name[..n].copy_from_slice(&vb[..n]);
                        resp.extend_from_slice(&name);
                        true
                    }
                    Ok(None) => {
                        resp.push(0); // not found
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:load:");
                        resp.extend_from_slice(e.to_string().as_bytes());
                        false
                    }
                }
            }
            _ => {
                resp.extend_from_slice(b"ERR:cmd");
                false
            }
        }
    }
}

#[cfg(target_os = "none")]
static ENEROS_OTA_ROOT_KEY: [u8; 64] = [
    0x1f, 0x58, 0x39, 0x66, 0x96, 0xde, 0xec, 0x97, 0xa0, 0x09, 0x0f, 0xa6, 0x12, 0xeb, 0x5c, 0x47,
    0x84, 0xf0, 0xb1, 0x14, 0x69, 0x36, 0x96, 0x91, 0x6c, 0x01, 0xe9, 0xcf, 0xf8, 0xeb, 0xc5, 0xc7,
    0xec, 0x0c, 0x18, 0xca, 0xf0, 0x2a, 0x74, 0x78, 0xc7, 0x3e, 0x54, 0xbf, 0x13, 0xa0, 0x6f, 0xb1,
    0x16, 0x24, 0xfd, 0x1a, 0x23, 0x3a, 0xb5, 0xe4, 0x12, 0x17, 0xaa, 0x7b, 0xb3, 0x80, 0x3e, 0x04,
];

#[cfg(target_os = "none")]
static ENEROS_OTA_POLICY: eneros_business_services::ota_service::OtaPolicy =
    eneros_business_services::ota_service::OtaPolicy {
        partition_blocks: 2048,
        chunk_size: 65536,
        max_boot_attempts: 3,
        root_key: ENEROS_OTA_ROOT_KEY, // 开发公钥；生产 KMS 注入替换
        signature_mandatory: true,
    };

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
