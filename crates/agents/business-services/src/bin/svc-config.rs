//! svc-config — 签名配置服务 EL0（蓝图 41 WP-41.1/41.2）。
//!
//! 嵌入 xtask `config compile` 产出的签名 RuntimeConfigBundle（build.rs）；
//! 启动时验签装载；EIPC 查询面：GET_ENTRY / GET_DIGEST / GET_GENERATION。
//! bundle 缺失/验签失败 → 拒绝一切查询（fail-closed）。

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
    _eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };

    let mut service = eneros_business_services::ConfigService::new();
    // 生产路径：嵌入的签名 bundle + 验签公钥（无 → fail-closed）。
    match service.load_embedded(
        eneros_business_services::CONFIG_BUNDLE,
        eneros_business_services::CONFIG_BUNDLE_PUBKEY_HEX,
    ) {
        Ok(()) => {
            eneros_runtime::println!(
                "[svc-config] bundle loaded gen={} entries={}",
                service.generation(),
                service
                    .bundle
                    .as_ref()
                    .map(|b| b.entries.len())
                    .unwrap_or(0)
            );
        }
        Err(e) => {
            eneros_runtime::println!("[svc-config] FAIL-CLOSED no valid bundle: {e}");
        }
    }

    let mut handler = ConfigHandler { service };
    eneros_control_services::control_service_main(
        ep,
        "config",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct ConfigHandler {
    service: eneros_business_services::ConfigService,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for ConfigHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_business_services::config_service::*;
        // 协议：magic(4) ‖ cmd(1) ‖ arg。
        if req.len() < 5 || &req[..4] != PROTO_MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        let cmd = req[4];
        let arg = &req[5..];
        match self.service.handle_query(cmd, arg) {
            Ok(data) => {
                resp.extend_from_slice(&(data.len() as u32).to_le_bytes());
                resp.extend_from_slice(&data);
                true
            }
            Err(e) => {
                resp.extend_from_slice(b"ERR:");
                resp.extend_from_slice(e.as_bytes());
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

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
