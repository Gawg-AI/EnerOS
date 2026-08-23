//! svc-mgmt 领域逻辑：严格管理 RPC（蓝图 41 WP-41.11）。
//!
//! - 身份在 transport session（AuthenticatedSession：principal/roles/
//!   tenant_scope/channel_binding/expires_at），payload 不再自报 role；
//! - malformed/unknown action → 错误（不默认 Operator/StatusQuery）；
//! - 每个 action 有真实 handler；publish/执行错误传播，不固定 Ok；
//! - destructive action 需要 stronger auth + 幂等；
//! - response 区分 accepted/completed/failed；audit 完整。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 会话认证结果（由 transport 层在 decode action 之前产生）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSession {
    pub principal: String,
    pub roles: Vec<String>,
    pub tenant_scope: u32,
    pub channel_binding: String,
    pub expires_at_ms: u64,
}

impl AuthenticatedSession {
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms == 0 || now_ms <= self.expires_at_ms
    }
}

/// 管理动作（24 个 action 的严格枚举——未知字符串不再映射默认）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MgmtAction {
    StatusQuery,
    SloQuery,
    AgentList,
    AgentStart,
    AgentStop,
    AgentRestart,
    ConfigGet,
    ConfigSet,
    LogQuery,
    SupportBundleExport,
    CrashDumpSymbolize,
    UpdateCheck,
    UpdateApply,
    UpdateRollback,
    MaintenanceEnter,
    MaintenanceExit,
    BackupCreate,
    Restore,
    Revoke,
    FactoryReset,
    SecureErase,
}

impl MgmtAction {
    #[allow(clippy::result_unit_err)] // 44.2：CLI 解析错误面为 ()（协议常量型），错误类型化属后续卷
    pub fn parse(raw: &str) -> Result<MgmtAction, ()> {
        match raw {
            "status" => Ok(MgmtAction::StatusQuery),
            "slo" => Ok(MgmtAction::SloQuery),
            "agent-list" => Ok(MgmtAction::AgentList),
            "agent-start" => Ok(MgmtAction::AgentStart),
            "agent-stop" => Ok(MgmtAction::AgentStop),
            "agent-restart" => Ok(MgmtAction::AgentRestart),
            "config-get" => Ok(MgmtAction::ConfigGet),
            "config-set" => Ok(MgmtAction::ConfigSet),
            "log-query" => Ok(MgmtAction::LogQuery),
            "support-bundle" => Ok(MgmtAction::SupportBundleExport),
            "crash-symbolize" => Ok(MgmtAction::CrashDumpSymbolize),
            "update-check" => Ok(MgmtAction::UpdateCheck),
            "update-apply" => Ok(MgmtAction::UpdateApply),
            "update-rollback" => Ok(MgmtAction::UpdateRollback),
            "maintenance-enter" => Ok(MgmtAction::MaintenanceEnter),
            "maintenance-exit" => Ok(MgmtAction::MaintenanceExit),
            "backup-create" => Ok(MgmtAction::BackupCreate),
            "restore" => Ok(MgmtAction::Restore),
            "revoke" => Ok(MgmtAction::Revoke),
            "factory-reset" => Ok(MgmtAction::FactoryReset),
            "secure-erase" => Ok(MgmtAction::SecureErase),
            _ => Err(()),
        }
    }

    /// destructive action 需要 stronger auth（Admin+ 或 Superuser）。
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            MgmtAction::UpdateApply
                | MgmtAction::UpdateRollback
                | MgmtAction::MaintenanceEnter
                | MgmtAction::MaintenanceExit
                | MgmtAction::Restore
                | MgmtAction::Revoke
                | MgmtAction::FactoryReset
                | MgmtAction::SecureErase
                | MgmtAction::AgentStop
                | MgmtAction::AgentRestart
        )
    }

    /// 50（WP-50.2）：**每动作独立 RBAC 角色**（复用 mgmt-api 角色模型：
    /// viewer/operator/admin/superuser 层级）。只读查询 viewer；运维操作
    /// operator；受控重启/破坏性 admin。
    pub fn role_for(&self) -> &'static str {
        match self {
            // 只读查询（服务状态/日志查询）→ viewer。
            MgmtAction::StatusQuery | MgmtAction::SloQuery | MgmtAction::LogQuery => "viewer",
            // 只读诊断包/配置只读查询/崩溃符号化 → operator。
            MgmtAction::ConfigGet
            | MgmtAction::SupportBundleExport
            | MgmtAction::CrashDumpSymbolize => "operator",
            // 受控重启（经 root supervisor API）→ admin。
            MgmtAction::AgentRestart => "admin",
            // 其余破坏性 → admin。
            _ if self.is_destructive() => "admin",
            // 其余非破坏性（写类）→ operator。
            _ => "operator",
        }
    }

    /// 50（WP-50.2）：50 卷白名单动作（真实 executor 接线面）。白名单外
    /// 动作 → executor 返回 Failed "action-not-wired"（fail-closed，不空成功）。
    pub fn in_whitelist(&self) -> bool {
        matches!(
            self,
            MgmtAction::StatusQuery
                | MgmtAction::AgentRestart
                | MgmtAction::ConfigGet
                | MgmtAction::LogQuery
                | MgmtAction::SupportBundleExport
        )
    }
}

/// 响应状态（accepted/completed/failed 分层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcStatus {
    Accepted,
    Completed,
    Failed,
}

/// 动作执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResult {
    pub status: RpcStatus,
    pub detail: String,
}

/// 审计记录（principal/channel/request/result）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub principal: String,
    pub channel: String,
    pub action: String,
    pub result: String,
    pub at_ms: u64,
}

/// 真实动作 handler trait（每种动作有真实实现与授权）。
pub trait ActionExecutor {
    /// 执行动作；**不得**固定 Ok——真实失败必须返回 Failed。
    fn execute(
        &mut self,
        action: &MgmtAction,
        session: &AuthenticatedSession,
        arg: &[u8],
        now_ms: u64,
    ) -> ActionResult;
}

/// 管理 RPC 处理器。
pub struct ManagementRpc<E: ActionExecutor> {
    pub executor: E,
    pub audit: Vec<AuditRecord>,
    pub replay_seen: Vec<u64>,
}

impl<E: ActionExecutor> ManagementRpc<E> {
    pub fn new(executor: E) -> Self {
        ManagementRpc {
            executor,
            audit: Vec::new(),
            replay_seen: Vec::new(),
        }
    }

    /// 处理请求：认证会话（transport 层已产生）→ parse action（未知 →
    /// Failed，不默认）→ 授权（角色不足 → Failed）→ 执行 → 审计。
    /// `request_id` 用于重放防护（重复请求 id → Failed）。
    pub fn handle(
        &mut self,
        session: &AuthenticatedSession,
        request_id: u64,
        action_raw: &str,
        arg: &[u8],
        now_ms: u64,
    ) -> ActionResult {
        if !session.is_valid_at(now_ms) {
            return ActionResult {
                status: RpcStatus::Failed,
                detail: "session-expired".into(),
            };
        }
        if self.replay_seen.contains(&request_id) {
            return ActionResult {
                status: RpcStatus::Failed,
                detail: "replay-request".into(),
            };
        }
        self.replay_seen.push(request_id);
        let action = match MgmtAction::parse(action_raw) {
            Ok(a) => a,
            Err(()) => {
                let r = ActionResult {
                    status: RpcStatus::Failed,
                    detail: format!("unknown-action:{action_raw}"),
                };
                self.audit.push(AuditRecord {
                    principal: session.principal.clone(),
                    channel: session.channel_binding.clone(),
                    action: action_raw.into(),
                    result: "unknown-action".into(),
                    at_ms: now_ms,
                });
                return r;
            }
        };
        // payload 不再自报 role——授权只看 session roles（每动作独立角色，
        // 50 WP-50.2）。
        if !session.has_role(action.role_for()) && !session.has_role("superuser") {
            let r = ActionResult {
                status: RpcStatus::Failed,
                detail: format!("insufficient-role:{}", action.role_for()),
            };
            self.audit.push(AuditRecord {
                principal: session.principal.clone(),
                channel: session.channel_binding.clone(),
                action: action_raw.into(),
                result: "denied".into(),
                at_ms: now_ms,
            });
            return r;
        }
        let result = self.executor.execute(&action, session, arg, now_ms);
        self.audit.push(AuditRecord {
            principal: session.principal.clone(),
            channel: session.channel_binding.clone(),
            action: action_raw.into(),
            result: format!("{:?}", result.status),
            at_ms: now_ms,
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试执行器：真实实现（带失败注入）。
    struct TestExecutor {
        fail: bool,
        executed: Vec<String>,
    }
    impl ActionExecutor for TestExecutor {
        fn execute(
            &mut self,
            action: &MgmtAction,
            _session: &AuthenticatedSession,
            _arg: &[u8],
            _now_ms: u64,
        ) -> ActionResult {
            if self.fail {
                return ActionResult {
                    status: RpcStatus::Failed,
                    detail: "executor-down".into(),
                };
            }
            self.executed.push(format!("{action:?}"));
            ActionResult {
                status: RpcStatus::Completed,
                detail: "ok".into(),
            }
        }
    }

    fn admin(now: u64) -> AuthenticatedSession {
        AuthenticatedSession {
            principal: "ops-1".into(),
            // 50（WP-50.2）：admin = viewer+operator+admin（mgmt-api 角色层级
            // 继承：Admin 拥有 SystemRead+SystemWrite+SecurityAdmin）。
            roles: vec!["viewer".into(), "operator".into(), "admin".into()],
            tenant_scope: 1,
            channel_binding: "mtls:cn=ops-1".into(),
            expires_at_ms: now + 60_000,
        }
    }

    fn viewer() -> AuthenticatedSession {
        AuthenticatedSession {
            principal: "viewer-1".into(),
            roles: vec!["viewer".into()],
            tenant_scope: 1,
            channel_binding: "tls".into(),
            expires_at_ms: 0,
        }
    }

    #[test]
    fn positive_known_action_executes() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&admin(1000), 1, "status", b"", 1000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.executor.executed.len(), 1);
    }

    #[test]
    fn negative_unknown_action_not_defaulted() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&admin(1000), 1, "totally-unknown", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("unknown-action"));
        assert!(rpc.executor.executed.is_empty());
    }

    #[test]
    fn negative_insufficient_role_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&viewer(), 1, "factory-reset", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role"));
        assert!(rpc.executor.executed.is_empty());
    }

    #[test]
    fn negative_executor_failure_propagates() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: true,
            executed: vec![],
        });
        let r = rpc.handle(&admin(1000), 1, "config-get", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "executor-down");
    }

    #[test]
    fn negative_replay_rejected_and_session_expiry() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        rpc.handle(&admin(1000), 7, "status", b"", 1000);
        let r = rpc.handle(&admin(1000), 7, "status", b"", 2000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "replay-request");
        // 过期会话。
        let mut expired = admin(1000);
        expired.expires_at_ms = 500;
        let r = rpc.handle(&expired, 8, "status", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "session-expired");
    }

    // ================= 50（WP-50.2）每动作独立 RBAC =================

    fn operator(now: u64) -> AuthenticatedSession {
        AuthenticatedSession {
            principal: "ops-op".into(),
            roles: vec!["operator".into()],
            tenant_scope: 1,
            channel_binding: "mtls:cn=ops-op".into(),
            expires_at_ms: now + 60_000,
        }
    }

    #[test]
    fn role_for_per_action_mapping() {
        use MgmtAction::*;
        assert_eq!(StatusQuery.role_for(), "viewer");
        assert_eq!(LogQuery.role_for(), "viewer");
        assert_eq!(ConfigGet.role_for(), "operator");
        assert_eq!(SupportBundleExport.role_for(), "operator");
        assert_eq!(AgentRestart.role_for(), "admin");
        // 白名单面。
        for a in [
            StatusQuery,
            AgentRestart,
            ConfigGet,
            LogQuery,
            SupportBundleExport,
        ] {
            assert!(a.in_whitelist());
        }
        assert!(!UpdateApply.in_whitelist(), "更新链非 50.2 白名单");
    }

    /// 50：正例——viewer 可查询服务状态（只读）。
    #[test]
    fn positive_status_query_viewer_allowed() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&viewer(), 1, "status", b"", 1000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(rpc.executor.executed.len(), 1);
        // 审计入链。
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.audit[0].principal, "viewer-1");
        assert_eq!(rpc.audit[0].action, "status");
    }

    /// 50：越权负例——viewer 不能 ConfigGet（需 operator）。
    #[test]
    fn negative_config_get_viewer_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&viewer(), 1, "config-get", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role:operator"));
        assert!(rpc.executor.executed.is_empty());
        // 越权亦审计。
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.audit[0].result, "denied");
    }

    /// 50：越权负例——viewer 不能 SupportBundleExport（需 operator）。
    #[test]
    fn negative_support_bundle_viewer_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&viewer(), 1, "support-bundle", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role:operator"));
        assert!(rpc.executor.executed.is_empty());
    }

    /// 50：越权负例——operator 不能 AgentRestart（需 admin）。
    #[test]
    fn negative_agent_restart_operator_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&operator(1000), 1, "agent-restart", b"svc-ota", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role:admin"));
        assert!(rpc.executor.executed.is_empty());
    }

    /// 50：正例——admin 可受控重启（审计入链）。
    #[test]
    fn positive_agent_restart_admin_allowed() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&admin(1000), 1, "agent-restart", b"svc-ota", 1000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(rpc.executor.executed.len(), 1);
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.audit[0].result, "Completed");
    }
}
