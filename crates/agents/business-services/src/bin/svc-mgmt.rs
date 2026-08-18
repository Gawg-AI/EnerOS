//! svc-mgmt — 严格管理 RPC EL0（蓝图 41 WP-41.11）。
//! EIPC 协议：magic "MGT1" ‖ session(principal 16B‖roles 4B‖tenant 4B‖channel 16B‖expiry 8B) ‖
//! request_id(8) ‖ action(≤16) ‖ arg(≤128)。
//! 真实动作经注入 executor；未接线动作 → Failed（不空成功）。

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
use alloc::vec;
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
    // 41（WP-41.11）：ConfigGet 经 EIPC 查 svc-config（授予表按客户端
    // 独立计数：svc-mgmt 首个 grant = 槽 EIPC_EP_BASE_SLOT+0、mask bit 0）。
    let config_ep = if eipc_mask & 1 != 0 {
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "mgmt-config",
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            eneros_drv_svc::now_ns,
        )
        .ok()
    } else {
        None
    };
    if config_ep.is_some() {
        eneros_runtime::println!("[svc-mgmt] config EIPC bound (svc-config)");
    } else {
        eneros_runtime::println!("[svc-mgmt] config EIPC NOT granted -> ConfigGet unavailable");
    }
    eneros_runtime::println!(
        "[svc-mgmt] management RPC up (session-authenticated; real actions only)"
    );
    let mut handler = MgmtHandler {
        rpc: eneros_business_services::mgmt_rpc::ManagementRpc::new(TargetExecutor {
            config_ep,
            epoch: 0,
        }),
    };
    eneros_control_services::control_service_main(
        ep,
        "mgmt",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct TargetExecutor {
    config_ep: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    epoch: u64,
}

#[cfg(target_os = "none")]
impl eneros_business_services::mgmt_rpc::ActionExecutor for TargetExecutor {
    fn execute(
        &mut self,
        action: &eneros_business_services::mgmt_rpc::MgmtAction,
        _session: &eneros_business_services::mgmt_rpc::AuthenticatedSession,
        _arg: &[u8],
        now_ms: u64,
    ) -> eneros_business_services::mgmt_rpc::ActionResult {
        use eneros_business_services::mgmt_rpc::*;
        match action {
            MgmtAction::StatusQuery => ActionResult {
                status: RpcStatus::Completed,
                detail: alloc::format!("status:ok:up@{}", now_ms),
            },
            MgmtAction::ConfigGet => {
                let Some(client) = &mut self.config_ep else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                    };
                };
                // CFG1 GET_ENTRY：config_id = "ota"（可扩展为 arg 指定）。
                let mut req: alloc::vec::Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
                req.extend_from_slice(b"ota");
                let mut transfer = eneros_agent_sdk::ChunkedClient::new(client, self.epoch, 5_000);
                self.epoch = self.epoch.saturating_add(1);
                match transfer.transfer("svc-config", &req) {
                    Ok(resp) => ActionResult {
                        status: RpcStatus::Completed,
                        detail: alloc::format!("config-get:{}B", resp.len()),
                    },
                    Err(e) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: alloc::format!("config-get-eipc:{e:?}"),
                    },
                }
            }
            MgmtAction::UpdateCheck | MgmtAction::UpdateApply | MgmtAction::UpdateRollback => {
                ActionResult {
                    status: RpcStatus::Failed,
                    detail: "ota-service-not-bound".into(),
                }
            }
            _ => ActionResult {
                status: RpcStatus::Failed,
                detail: alloc::format!("action-not-wired:{action:?}"),
            },
        }
    }
}

#[cfg(target_os = "none")]
struct MgmtHandler {
    rpc: eneros_business_services::mgmt_rpc::ManagementRpc<TargetExecutor>,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for MgmtHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::mgmt_rpc::*;
        const SESSION_LEN: usize = 16 + 4 + 4 + 16 + 8;
        if req.len() < 5 + SESSION_LEN + 8 + 1 {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        let p = &req[5..];
        let mut principal = [0u8; 16];
        principal.copy_from_slice(&p[0..16]);
        let mut roles = [0u8; 4];
        roles.copy_from_slice(&p[16..20]);
        let role = alloc::string::String::from_utf8_lossy(&roles).into_owned();
        let role = role.trim_end_matches('\0').to_string();
        let tenant = u32::from_le_bytes(p[20..24].try_into().unwrap());
        let mut channel = [0u8; 16];
        channel.copy_from_slice(&p[24..40]);
        let expiry = u64::from_le_bytes(p[40..48].try_into().unwrap());
        let request_id = u64::from_le_bytes(p[48..56].try_into().unwrap());
        let action_len = req[5 + SESSION_LEN + 8] as usize;
        let action_start = 5 + SESSION_LEN + 9;
        if action_start + action_len > req.len() {
            resp.extend_from_slice(b"ERR:len");
            return false;
        }
        let action = core::str::from_utf8(&req[action_start..action_start + action_len])
            .unwrap_or("")
            .to_string();
        let arg = &req[action_start + action_len..];

        let session = AuthenticatedSession {
            principal: alloc::string::String::from_utf8_lossy(&principal)
                .trim_end_matches('\0')
                .to_string(),
            roles: vec![if role.is_empty() {
                "viewer".into()
            } else {
                role
            }],
            tenant_scope: tenant,
            channel_binding: alloc::string::String::from_utf8_lossy(&channel)
                .trim_end_matches('\0')
                .to_string(),
            expires_at_ms: expiry,
        };
        let result = self.rpc.handle(&session, request_id, &action, arg, now_ms);
        resp.push(match result.status {
            RpcStatus::Completed => 1u8,
            RpcStatus::Accepted => 2,
            RpcStatus::Failed => 0,
        });
        resp.extend_from_slice(result.detail.as_bytes());
        true
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
