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
    // 独立计数：svc-mgmt 首个 grant = 槽 EIPC_EP_BASE_SLOT+0、mask bit 0。
    // SPEC-B：硬编码掩码位（`& 1`）收编 eipc_granted_slot（按名解析）。
    let config_ep = eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-config")
        .and_then(|slot| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "mgmt-config",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
    if config_ep.is_some() {
        eneros_runtime::println!("[svc-mgmt] config EIPC bound (svc-config)");
    } else {
        eneros_runtime::println!("[svc-mgmt] config EIPC NOT granted -> ConfigGet unavailable");
    }
    eneros_runtime::println!(
        "[svc-mgmt] management RPC up (session-authenticated; real actions only)"
    );
    // SPEC-C（WP-C.2）：history-query 取数通道——svc-mgmt 第 2 授予边
    // （SPEC-B：硬编码掩码位 `& 2` 收编 eipc_granted_slot 按名解析）；
    // 未授予 → 执行器回 "telemetry-not-bound"（fail-closed）。
    let telemetry_slot =
        eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-telemetry");
    if telemetry_slot.is_some() {
        eneros_runtime::println!("[svc-mgmt] history EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!(
            "[svc-mgmt] history EIPC NOT granted -> history-query unavailable"
        );
    }
    let mut handler = MgmtHandler {
        rpc: eneros_business_services::mgmt_rpc::ManagementRpc::new(TargetExecutor {
            config_ep,
            telemetry_slot,
            epoch: 0,
            history_binds: 0,
            requests_seen: 0,
            bundle_seq: 0,
            restart_requests: 0,
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
    /// SPEC-C：svc-telemetry 授予槽（按值重建客户端的先例模式——
    /// 避免跨请求可变借用纠缠）。
    telemetry_slot: Option<u64>,
    /// SPEC-C：TELR 通道构建计数（观测）。
    history_binds: u64,
    epoch: u64,
    /// 50（WP-50.2）：请求计数 / 诊断包序号 / 重启请求数（真实观测）。
    requests_seen: u64,
    bundle_seq: u64,
    restart_requests: u64,
}

#[cfg(target_os = "none")]
impl eneros_business_services::mgmt_rpc::ActionExecutor for TargetExecutor {
    fn execute(
        &mut self,
        action: &eneros_business_services::mgmt_rpc::MgmtAction,
        _session: &eneros_business_services::mgmt_rpc::AuthenticatedSession,
        arg: &[u8],
        now_ms: u64,
    ) -> eneros_business_services::mgmt_rpc::ActionResult {
        use eneros_business_services::mgmt_rpc::*;
        match action {
            // 50（WP-50.2）：服务状态查询（真实：uptime/epoch/请求计数）。
            MgmtAction::StatusQuery => ActionResult {
                status: RpcStatus::Completed,
                detail: alloc::format!("status:ok:up@{} req={}", now_ms, self.requests_seen),
                payload: None,
            },
            MgmtAction::ConfigGet => {
                // 配置只读查询（arg 指定 config_id；空 → fail-closed 拒绝）。
                let id = core::str::from_utf8(arg).unwrap_or("");
                if id.is_empty() {
                    self.requests_seen = self.requests_seen.saturating_add(1);
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-get:empty-id".into(),
                        payload: None,
                    };
                }
                let Some(client) = &mut self.config_ep else {
                    self.requests_seen = self.requests_seen.saturating_add(1);
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                        payload: None,
                    };
                };
                // SPEC-B：CFG1 wire 单源 = agent-sdk config_client；epoch
                // 透传保持页序单调（多管理请求不撞帧）。
                let resp =
                    eneros_agent_sdk::config_client::get_entry(client, id, self.epoch, 5_000);
                self.epoch = self.epoch.saturating_add(1);
                self.requests_seen = self.requests_seen.saturating_add(1);
                match resp {
                    Ok(raw) => ActionResult {
                        status: RpcStatus::Completed,
                        detail: alloc::format!("config-get:{}:{}B", id, raw.len()),
                        payload: None,
                    },
                    Err(e) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: alloc::format!("config-get-eipc:{e:?}"),
                        payload: None,
                    },
                }
            }
            // 50：日志级别查询（只读，经 svc-config logging 配置）。
            MgmtAction::LogQuery => {
                let Some(client) = &mut self.config_ep else {
                    self.requests_seen = self.requests_seen.saturating_add(1);
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                        payload: None,
                    };
                };
                let resp = eneros_agent_sdk::config_client::get_entry(
                    client, "logging", self.epoch, 5_000,
                );
                self.epoch = self.epoch.saturating_add(1);
                self.requests_seen = self.requests_seen.saturating_add(1);
                match resp {
                    Ok(raw) => ActionResult {
                        status: RpcStatus::Completed,
                        detail: alloc::format!("log-level:query:{}B", raw.len()),
                        payload: None,
                    },
                    Err(e) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: alloc::format!("log-query-eipc:{e:?}"),
                        payload: None,
                    },
                }
            }
            // 50：只读诊断包触发（汇总本服务真实运行计数 + 可达配置查询）。
            MgmtAction::SupportBundleExport => {
                self.bundle_seq = self.bundle_seq.saturating_add(1);
                self.requests_seen = self.requests_seen.saturating_add(1);
                ActionResult {
                    status: RpcStatus::Completed,
                    detail: alloc::format!(
                        "support-bundle:seq={} req={} cfg={}",
                        self.bundle_seq,
                        self.requests_seen,
                        if self.config_ep.is_some() {
                            "bound"
                        } else {
                            "none"
                        }
                    ),
                    payload: None,
                }
            }
            // 50：受控重启（经 root supervisor API）——记录并审计重启请求；
            // 实际重启由 root supervisor 心跳/故障路径执行（service→root
            // 重启用 RPC seam 登记 v1.x）。白名单外的 action → fail-closed。
            MgmtAction::AgentRestart => {
                self.restart_requests = self.restart_requests.saturating_add(1);
                self.requests_seen = self.requests_seen.saturating_add(1);
                let target = core::str::from_utf8(arg).unwrap_or("");
                if target.is_empty() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "restart:empty-target".into(),
                        payload: None,
                    };
                }
                ActionResult {
                    status: RpcStatus::Accepted,
                    detail: alloc::format!(
                        "restart-request-logged:service={} n={}（root supervisor 执行）",
                        target,
                        self.restart_requests
                    ),
                    payload: None,
                }
            }
            // SPEC-C（WP-C.2）：北向历史取回——分页拉取 + 二进制 payload。
            // arg 布局：point u32 ‖ last_n u32（LE）。能力边界写明：
            // 仅最后 N 条、非全量导出。
            MgmtAction::HistoryQuery => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                if arg.len() < 8 {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "history:bad-arg(point‖last_n 需 8B)".into(),
                        payload: None,
                    };
                }
                let point = u32::from_le_bytes(arg[0..4].try_into().unwrap_or([0; 4]));
                let last_n = u32::from_le_bytes(arg[4..8].try_into().unwrap_or([0; 4]));
                let Some(slot) = self.telemetry_slot else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "telemetry-not-bound".into(),
                        payload: None,
                    };
                };
                // 每请求按槽重建客户端（先例：eipc_block_io/协议服务重建）；
                // 失败 → 显式不可达。
                let Ok(client) = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                    "mgmt-history",
                    slot,
                    eneros_drv_svc::now_ns,
                ) else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "history:eipc-bind-failed".into(),
                        payload: None,
                    };
                };
                self.history_binds = self.history_binds.saturating_add(1);
                match eneros_business_services::mgmt_history::fetch_history(
                    client,
                    "svc-telemetry",
                    point,
                    last_n,
                    5_000,
                ) {
                    Ok(f) => {
                        let mut detail = f.summary.clone();
                        if f.clamped {
                            detail.push_str(" clamp-hint:last_n>512-capped");
                        }
                        ActionResult {
                            status: RpcStatus::Completed,
                            detail,
                            payload: Some(f.rows_bin),
                        }
                    }
                    Err(e) => {
                        let reason = match e {
                            eneros_business_services::mgmt_history::HistoryError::NoSuchPoint => {
                                alloc::format!("history:no-such-point:{}", point)
                            }
                            eneros_business_services::mgmt_history::HistoryError::Transport => {
                                "history:eipc-transport".into()
                            }
                            eneros_business_services::mgmt_history::HistoryError::BadFrame => {
                                "history:bad-frame".into()
                            }
                        };
                        ActionResult {
                            status: RpcStatus::Failed,
                            detail: reason,
                            payload: None,
                        }
                    }
                }
            }
            MgmtAction::UpdateCheck | MgmtAction::UpdateApply | MgmtAction::UpdateRollback => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                ActionResult {
                    status: RpcStatus::Failed,
                    detail: "ota-service-not-bound".into(),
                    payload: None,
                }
            }
            _ => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                ActionResult {
                    status: RpcStatus::Failed,
                    detail: alloc::format!("action-not-wired:{action:?}"),
                    payload: None,
                }
            }
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
        // 50（WP-50.2）：4B 角色字段 = **mgmt-api 角色码**（Role::to_u8 LE
        // 首字节、余位补零——字段装不下完整角色名，run8 实弹根因）。兼容
        // 旧字符串面（首字节非 {0..3} 码 → 按 ASCII 解析；全零 = 旧缺省
        // viewer 同语义）。
        let role = match eneros_mgmt_api::Role::from_u8(roles[0]) {
            Some(r) if roles[1] == 0 && roles[2] == 0 && roles[3] == 0 => {
                alloc::string::String::from(r.name())
            }
            _ => {
                let legacy = alloc::string::String::from_utf8_lossy(&roles).into_owned();
                legacy.trim_end_matches('\0').to_string()
            }
        };
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
        // SPEC-C：二进制载荷扩展段（history-query 行区等）。旧消费方按
        // detail 文本前缀解析不受影响；新消费方读尾部 [len][bytes]。
        if let Some(p) = &result.payload {
            if !p.is_empty() {
                resp.extend_from_slice(&(p.len() as u32).to_le_bytes());
                resp.extend_from_slice(p);
            }
        }
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
