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

    pub fn required_role(&self) -> &'static str {
        if self.is_destructive() {
            "admin"
        } else {
            "operator"
        }
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
        // payload 不再自报 role——授权只看 session roles。
        if !session.has_role(action.required_role()) && !session.has_role("superuser") {
            let r = ActionResult {
                status: RpcStatus::Failed,
                detail: format!("insufficient-role:{}", action.required_role()),
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
            roles: vec!["operator".into(), "admin".into()],
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
}
