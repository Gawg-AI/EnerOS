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

/// 蓝图60 WP-60.2：OTA 三动作在 mgmt 侧的降级词形——「服务未绑定」语义
/// 保持（fail-closed 不空成功），补 EIPC 授予边缺失原因，与 13 个
/// `action-not-wired` 裸词形区分层级（svc-mgmt→svc-ota 授予边未建立；
/// 升格接通属独立侦察项，蓝图60 §5 默认不做）。宿主测试面经
/// `cfg(any(target_os = "none", test))` 双侧可用。
#[cfg(any(target_os = "none", test))]
fn ota_channel_unbound_detail() -> &'static str {
    "ota-channel-unbound:mgmt→ota-grant-missing"
}

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
    // 蓝图 67 WP-67.2.3：config 传输面 = ChunkedEndpoint（epoch 单点
    // 纪律——原 self.epoch 手递增 ×2 面删除）。
    let config_ep = eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-config")
        .and_then(|slot| {
            eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap("mgmt-config", slot)
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
    // 蓝图 67 WP-67.2.3：取数端点启动期一次装配持有（跨请求 epoch 单调
    // 延续——原"每请求重建客户端恒 0"面删除；bind 失败词形区分保留）。
    let telemetry_slot =
        eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-telemetry");
    let history_ep = telemetry_slot.and_then(|slot| {
        eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap("mgmt-history", slot)
    });
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
            history_ep,
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
    // 蓝图 67 WP-67.2.3：config/history 传输面 = ChunkedEndpoint（epoch
    // 单点递增，跨请求单调延续；原 epoch 字段 + 手递增 ×2 删除）。
    config_ep: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// SPEC-C：svc-telemetry 授予槽（bind 失败/未授予两态词形区分——
    /// history_ep 为 None 且本槽在场 = 建端失败词形）。
    telemetry_slot: Option<u64>,
    /// SPEC-C：TELR 取数端点（启动期一次装配）。
    history_ep: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
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
                // SPEC-B：CFG1 wire 单源 = agent-sdk config_client；蓝图 67
                // WP-67.2.3：endpoint 面（原 epoch 手递增删除——endpoint
                // 单点递增，跨管理请求单调）。
                let resp = eneros_agent_sdk::config_client::get_entry(client, id);
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
                let resp = eneros_agent_sdk::config_client::get_entry(client, "logging");
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
                // 蓝图 67 WP-67.2.3：端点启动期一次装配持有（跨请求 epoch
                // 单调延续）。两态失败词形逐字节保留：槽未授予 =
                // telemetry-not-bound；槽在场但建端失败 = eipc-bind-failed。
                if self.telemetry_slot.is_none() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "telemetry-not-bound".into(),
                        payload: None,
                    };
                }
                let Some(ep) = self.history_ep.as_mut() else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "history:eipc-bind-failed".into(),
                        payload: None,
                    };
                };
                match eneros_business_services::mgmt_history::fetch_history(
                    ep,
                    "svc-telemetry",
                    point,
                    last_n,
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
                    detail: ota_channel_unbound_detail().into(),
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

#[cfg(test)]
mod ota_word_form_tests {
    use eneros_business_services::mgmt_rpc::MgmtAction;

    use crate::ota_channel_unbound_detail;

    /// 蓝图60 D60.2.2：OTA 三动作降级词形钉住——词形自带 EIPC 授予边
    /// 缺失原因（mgmt→ota-grant-missing），与 `action-not-wired` 裸词形分层。
    #[test]
    fn ota_actions_report_channel_unbound_reason() {
        let d = ota_channel_unbound_detail();
        assert_eq!(d, "ota-channel-unbound:mgmt→ota-grant-missing");
        assert!(d.starts_with("ota-channel-unbound:"), "{d}");
        assert!(d.contains("mgmt→ota-grant-missing"), "{d}");
        assert!(!d.contains("action-not-wired"), "{d}");
        // 三动作可解析且不在接实白名单（保证路由进 OTA 降级分支而非
        // 通用 action-not-wired 臂）。
        for raw in ["update-check", "update-apply", "update-rollback"] {
            let a = MgmtAction::parse(raw).expect("ota action must parse");
            assert!(!a.in_whitelist(), "{raw} 不应在 svc-mgmt 接实白名单");
        }
    }

    /// 蓝图60 D60.2.3 披露清单计数钉住：22 动作 = 6 接实 + 3 OTA 未绑定
    /// 以及 13 个 `action-not-wired` 裸词形未接（README「管理面动作接线
    /// 现状」小节的数字以本测试为单一事实源）。
    #[test]
    fn action_surface_counts_match_disclosure() {
        let all = [
            "status",
            "slo",
            "agent-list",
            "agent-start",
            "agent-stop",
            "agent-restart",
            "config-get",
            "config-set",
            "log-query",
            "support-bundle",
            "crash-symbolize",
            "update-check",
            "update-apply",
            "update-rollback",
            "maintenance-enter",
            "maintenance-exit",
            "backup-create",
            "restore",
            "revoke",
            "factory-reset",
            "secure-erase",
            "history-query",
        ];
        assert_eq!(all.len(), 22);
        let ota = ["update-check", "update-apply", "update-rollback"];
        let mut n_wired = 0usize;
        let mut n_ota = 0usize;
        for raw in all {
            let a = MgmtAction::parse(raw).expect("action must parse");
            if a.in_whitelist() {
                n_wired += 1;
            }
            if ota.contains(&raw) {
                n_ota += 1;
            }
        }
        assert_eq!(n_wired, 6, "接实白名单面");
        assert_eq!(n_ota, 3, "OTA 三动作（ota-channel-unbound 词形）");
        assert_eq!(22 - n_wired - n_ota, 13, "action-not-wired 裸词形面");
        // 穷尽性哨兵（code-review 加固）：MgmtAction 增删变体时本 match
        // 编译失败，强制同步上方名册与 README 披露清单，防止新增动作
        // 静默落在披露面之外。
        let _exhaustive_roster_guard = |a: &MgmtAction| match a {
            MgmtAction::StatusQuery
            | MgmtAction::SloQuery
            | MgmtAction::AgentList
            | MgmtAction::AgentStart
            | MgmtAction::AgentStop
            | MgmtAction::AgentRestart
            | MgmtAction::ConfigGet
            | MgmtAction::ConfigSet
            | MgmtAction::LogQuery
            | MgmtAction::SupportBundleExport
            | MgmtAction::CrashDumpSymbolize
            | MgmtAction::UpdateCheck
            | MgmtAction::UpdateApply
            | MgmtAction::UpdateRollback
            | MgmtAction::MaintenanceEnter
            | MgmtAction::MaintenanceExit
            | MgmtAction::BackupCreate
            | MgmtAction::Restore
            | MgmtAction::Revoke
            | MgmtAction::FactoryReset
            | MgmtAction::SecureErase
            | MgmtAction::HistoryQuery => (),
        };
    }
}
