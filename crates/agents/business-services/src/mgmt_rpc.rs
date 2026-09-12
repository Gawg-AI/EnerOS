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

/// 管理动作（22 个 action 的严格枚举——未知字符串不再映射默认；
/// 计数随 SPEC-C HistoryQuery 增至 22，历史注释曾误记 24/21）。
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
    // SPEC-C（WP-C.2）：北向历史取回（参数在 arg：point u32 ‖ last_n u32）。
    HistoryQuery,
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
            "history-query" => Ok(MgmtAction::HistoryQuery),
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
            MgmtAction::StatusQuery
            | MgmtAction::SloQuery
            | MgmtAction::LogQuery
            | MgmtAction::HistoryQuery => "viewer",
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
                | MgmtAction::HistoryQuery
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
    /// SPEC-C：二进制扩展载荷（history-query 的 25B 行区等）。handler 于
    /// detail 之后追加 `[len u32 LE][bytes]`；旧文本消费方不受影响。
    pub payload: Option<Vec<u8>>,
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

    /// 70.6（WP-70.6.1）：在途操作观测步——StatusQuery("op:<id>") 轮询
    /// 异步操作时调用；返回 `Some((终态, detail))` = 账本写回终态；
    /// `None` = 仍在途。默认无观测（同步操作不产生 Running 记录）。
    fn observe_operation(
        &mut self,
        _record: &crate::mgmt_ops::OperationRecord,
        _now_ms: u64,
    ) -> Option<(crate::mgmt_ops::OpStatus, String)> {
        None
    }

    /// 70.6：当前生效配置代次（副作用记录绑定；不可得 = 0 如实）。
    fn current_config_generation(&mut self) -> u64 {
        0
    }
}

/// 管理 RPC 处理器。
///
/// 蓝图 70.1 WP-70.1.3（N02）：`audit` 与 `replay_seen` 由无界 `Vec` 追加
/// 收口为有界记录——超过 [`MGMT_AUDIT_MAX_RECORDS`] / [`MGMT_REPLAY_SEEN_MAX`]
/// 时丢最旧一条并计数（`audit_dropped_count`/`replay_dropped_count` 可观测），
/// 常驻字节与请求量解耦。语义边界如实声明：重放缓存是**易失防重放窗口**
/// （丢弃最旧后，超出窗口的旧 request_id 不再被判重），不是已执行操作账本；
/// 持久 operation 幂等记录归蓝图 70.6 WP-70.6.1 的操作模块。
pub struct ManagementRpc<E: ActionExecutor> {
    pub executor: E,
    pub audit: Vec<AuditRecord>,
    pub replay_seen: Vec<u64>,
    /// 70.6（WP-70.6.1）：可信操作账本（副作用幂等事实源；持久化由
    /// svc-mgmt 装配侧承担——本结构只拥有状态与规则）。
    pub ops: crate::mgmt_ops::OperationLedger,
    /// 因容量到顶被丢弃的审计记录数.
    audit_dropped: u64,
    /// 因容量到顶被挤出的重放缓存条目数.
    replay_dropped: u64,
}

/// 审计记录容量上限（128 条，AuditRecord 字段含 String 换算字节有界）。
pub const MGMT_AUDIT_MAX_RECORDS: usize = 128;

/// 重放防护窗口容量上限（request_id 环窗）。
pub const MGMT_REPLAY_SEEN_MAX: usize = 1024;

impl<E: ActionExecutor> ManagementRpc<E> {
    pub fn new(executor: E) -> Self {
        ManagementRpc {
            executor,
            audit: Vec::new(),
            replay_seen: Vec::new(),
            ops: crate::mgmt_ops::OperationLedger::new(),
            audit_dropped: 0,
            replay_dropped: 0,
        }
    }

    /// 有界审计追加：到顶丢最旧并计数（记录数与请求量解耦）.
    fn push_audit(&mut self, record: AuditRecord) {
        if self.audit.len() >= MGMT_AUDIT_MAX_RECORDS {
            self.audit.remove(0);
            self.audit_dropped += 1;
        }
        self.audit.push(record);
    }

    /// 有界重放窗口追加：到顶丢最旧并计数.
    fn push_replay_seen(&mut self, request_id: u64) {
        if self.replay_seen.len() >= MGMT_REPLAY_SEEN_MAX {
            self.replay_seen.remove(0);
            self.replay_dropped += 1;
        }
        self.replay_seen.push(request_id);
    }

    /// 因容量到顶被丢弃的审计记录数.
    pub fn audit_dropped_count(&self) -> u64 {
        self.audit_dropped
    }

    /// 因容量到顶被挤出的重放缓存条目数.
    pub fn replay_dropped_count(&self) -> u64 {
        self.replay_dropped
    }

    /// 处理请求：认证会话（transport 层已产生）→ parse action（未知 →
    /// Failed，不默认）→ 授权（角色不足 → Failed）→ **能力表分派**
    ///（70.6：副作用动作经操作账本幂等门；只读/流式走易失重放窗口）→
    /// 执行 → 账本/审计。
    ///
    /// `request_id` 幂等语义（WP-70.6.1）：
    /// - 副作用动作：持久账本单源——同 (principal, request_id, arg_digest)
    ///   回放已记录终态（不二次执行）；同 request_id 异参数 = 冲突拒绝；
    ///   表满且有在途 = 背压拒绝。
    /// - 只读/未知动作：易失 replay 窗口（不变）。
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
                payload: None,
            };
        }
        let action = match MgmtAction::parse(action_raw) {
            Ok(a) => a,
            Err(()) => {
                let r = ActionResult {
                    status: RpcStatus::Failed,
                    detail: format!("unknown-action:{action_raw}"),
                    payload: None,
                };
                if self.replay_seen.contains(&request_id) {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "replay-request".into(),
                        payload: None,
                    };
                }
                self.push_replay_seen(request_id);
                self.push_audit(AuditRecord {
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
                payload: None,
            };
            self.push_audit(AuditRecord {
                principal: session.principal.clone(),
                channel: session.channel_binding.clone(),
                action: action_raw.into(),
                result: "denied".into(),
                at_ms: now_ms,
            });
            return r;
        }
        // ===== 70.6：StatusQuery 扩展面（操作回查在授权之后、执行之前）=====
        if matches!(action, MgmtAction::StatusQuery) && !arg.is_empty() {
            return self.handle_status_arg(session, arg, now_ms);
        }
        let capability = crate::mgmt_ops::action_capability(&action).capability;
        if capability == crate::mgmt_ops::ActionCapability::SideEffect {
            // 副作用动作：持久账本幂等门（不经易失重放窗口——窗口判重
            // 会把合法回放误判为 replay-request，回放必须返回记录终态）。
            let digest = crate::mgmt_ops::arg_digest(arg);
            let config_generation = self.executor.current_config_generation();
            match self.ops.begin(
                request_id,
                &session.principal,
                action_raw,
                digest,
                config_generation,
                now_ms,
            ) {
                crate::mgmt_ops::OpGate::New(op_id) => {
                    let mut result = self.executor.execute(&action, session, arg, now_ms);
                    match result.status {
                        RpcStatus::Accepted => {
                            // 在途：账本保持 Running（observe_operation 轮询收口）。
                        }
                        RpcStatus::Completed => {
                            let d = result.detail.clone();
                            self.ops.finish(
                                op_id,
                                crate::mgmt_ops::OpStatus::Completed,
                                &d,
                                now_ms,
                            );
                        }
                        RpcStatus::Failed => {
                            let d = result.detail.clone();
                            self.ops
                                .finish(op_id, crate::mgmt_ops::OpStatus::Failed, &d, now_ms);
                        }
                    }
                    result.detail = alloc::format!("op={op_id} {}", result.detail);
                    self.push_audit(AuditRecord {
                        principal: session.principal.clone(),
                        channel: session.channel_binding.clone(),
                        action: action_raw.into(),
                        result: format!("{:?}#{}", result.status, op_id),
                        at_ms: now_ms,
                    });
                    return result;
                }
                crate::mgmt_ops::OpGate::Replayed(op_id) => {
                    // 幂等回放：同一操作只执行一次——返回已记录状态。
                    let record = self.ops.get(op_id).expect("Replayed 必有记录");
                    let r = match record.status {
                        crate::mgmt_ops::OpStatus::Running => ActionResult {
                            status: RpcStatus::Accepted,
                            detail: alloc::format!("op={op_id} in-progress (idempotent-replay)"),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Completed => ActionResult {
                            status: RpcStatus::Completed,
                            detail: alloc::format!("op={op_id} replay: {}", record.detail),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Failed => ActionResult {
                            status: RpcStatus::Failed,
                            detail: alloc::format!("op={op_id} replay: {}", record.detail),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Unknown => ActionResult {
                            status: RpcStatus::Failed,
                            detail: alloc::format!("op={op_id} replay: {}", record.detail),
                            payload: None,
                        },
                    };
                    self.push_audit(AuditRecord {
                        principal: session.principal.clone(),
                        channel: session.channel_binding.clone(),
                        action: action_raw.into(),
                        result: alloc::format!("replayed#{}", op_id),
                        at_ms: now_ms,
                    });
                    return r;
                }
                crate::mgmt_ops::OpGate::Conflict => {
                    self.push_audit(AuditRecord {
                        principal: session.principal.clone(),
                        channel: session.channel_binding.clone(),
                        action: action_raw.into(),
                        result: "idempotency-conflict".into(),
                        at_ms: now_ms,
                    });
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "idempotency-conflict:same-request-id-different-arg".into(),
                        payload: None,
                    };
                }
                crate::mgmt_ops::OpGate::Full => {
                    self.push_audit(AuditRecord {
                        principal: session.principal.clone(),
                        channel: session.channel_binding.clone(),
                        action: action_raw.into(),
                        result: "ops-backpressure".into(),
                        at_ms: now_ms,
                    });
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "ops-backpressure:ledger-full-with-inflight".into(),
                        payload: None,
                    };
                }
            }
        }
        // 只读/流式：易失重放窗口（既有语义不变）。
        if self.replay_seen.contains(&request_id) {
            return ActionResult {
                status: RpcStatus::Failed,
                detail: "replay-request".into(),
                payload: None,
            };
        }
        self.push_replay_seen(request_id);
        let result = self.executor.execute(&action, session, arg, now_ms);
        self.push_audit(AuditRecord {
            principal: session.principal.clone(),
            channel: session.channel_binding.clone(),
            action: action_raw.into(),
            result: format!("{:?}", result.status),
            at_ms: now_ms,
        });
        result
    }

    /// StatusQuery 扩展参数（WP-70.6.1 操作回查面；viewer 只读。回查
    /// 不判重放——轮询语义允同 id 重复查询）。
    /// - `op:<id>`：单操作终态回查；在途 → observe_operation 驱动收口。
    /// - `ops`：账本摘要。
    fn handle_status_arg(
        &mut self,
        _session: &AuthenticatedSession,
        arg: &[u8],
        now_ms: u64,
    ) -> ActionResult {
        let text = core::str::from_utf8(arg).unwrap_or("");
        if let Some(id_text) = text.strip_prefix("op:") {
            let op_id: u64 = match id_text.parse() {
                Ok(v) => v,
                Err(_) => {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "op:bad-id".into(),
                        payload: None,
                    }
                }
            };
            let unknown = {
                let record = match self.ops.get(op_id) {
                    Some(r) => r.clone(),
                    None => {
                        return ActionResult {
                            status: RpcStatus::Failed,
                            detail: alloc::format!("op={op_id} no-such-operation"),
                            payload: None,
                        }
                    }
                };
                if record.status.is_terminal() {
                    let line = alloc::format!(
                        "op={} {} {} gen={} created={} updated={} {}",
                        record.op_id,
                        record.action,
                        match record.status {
                            crate::mgmt_ops::OpStatus::Completed => "completed",
                            crate::mgmt_ops::OpStatus::Failed => "failed",
                            crate::mgmt_ops::OpStatus::Unknown => "unknown",
                            crate::mgmt_ops::OpStatus::Running => "running",
                        },
                        record.config_generation,
                        record.created_ms,
                        record.updated_ms,
                        record.detail
                    );
                    let status = match record.status {
                        crate::mgmt_ops::OpStatus::Completed => RpcStatus::Completed,
                        _ => RpcStatus::Failed,
                    };
                    return ActionResult {
                        status,
                        detail: line,
                        payload: None,
                    };
                }
                // 在途：执行器观测步驱动收口（重启轮询路径）。
                match self.executor.observe_operation(&record, now_ms) {
                    Some((final_status, detail)) => {
                        self.ops.finish(op_id, final_status, &detail, now_ms);
                        None
                    }
                    None => Some(record.clone()),
                }
            };
            return match unknown {
                None => {
                    let record = self.ops.get(op_id).expect("刚写回");
                    let line = alloc::format!(
                        "op={} {} {} gen={} {}",
                        record.op_id,
                        record.action,
                        match record.status {
                            crate::mgmt_ops::OpStatus::Completed => "completed",
                            crate::mgmt_ops::OpStatus::Failed => "failed",
                            crate::mgmt_ops::OpStatus::Unknown => "unknown",
                            crate::mgmt_ops::OpStatus::Running => "running",
                        },
                        record.config_generation,
                        record.detail
                    );
                    ActionResult {
                        status: match record.status {
                            crate::mgmt_ops::OpStatus::Completed => RpcStatus::Completed,
                            _ => RpcStatus::Failed,
                        },
                        detail: line,
                        payload: None,
                    }
                }
                Some(record) => ActionResult {
                    status: RpcStatus::Accepted,
                    detail: alloc::format!(
                        "op={} running since={} ({}); poll again",
                        record.op_id,
                        record.created_ms,
                        record.action
                    ),
                    payload: None,
                },
            };
        }
        if text == "ops" {
            let (mut n_run, mut n_done, mut n_fail, mut n_unk) = (0u8, 0u8, 0u8, 0u8);
            for r in self.ops.records() {
                match r.status {
                    crate::mgmt_ops::OpStatus::Running => n_run += 1,
                    crate::mgmt_ops::OpStatus::Completed => n_done += 1,
                    crate::mgmt_ops::OpStatus::Failed => n_fail += 1,
                    crate::mgmt_ops::OpStatus::Unknown => n_unk += 1,
                }
            }
            return ActionResult {
                status: RpcStatus::Completed,
                detail: alloc::format!(
                    "ops:total={} running={n_run} completed={n_done} failed={n_fail} unknown={n_unk} dropped={}",
                    self.ops.records().len(),
                    self.ops.dropped_count()
                ),
                payload: None,
            };
        }
        ActionResult {
            status: RpcStatus::Failed,
            detail: alloc::format!("status:bad-arg ({text:?})"),
            payload: None,
        }
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
                    payload: None,
                };
            }
            self.executed.push(format!("{action:?}"));
            ActionResult {
                status: RpcStatus::Completed,
                detail: "ok".into(),
                payload: None,
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

    /// 50：正例——admin 可受控重启（审计入链；70.6 起副作用动作入账本，
    /// 审计词形 `Completed#<op_id>`，操作可回查）。
    #[test]
    fn positive_agent_restart_admin_allowed() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let r = rpc.handle(&admin(1000), 1, "agent-restart", b"svc-ota", 1000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert!(r.detail.starts_with("op=1 "), "{}", r.detail);
        assert_eq!(rpc.executor.executed.len(), 1);
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.audit[0].result, "Completed#1");
        assert_eq!(
            rpc.ops.get(1).unwrap().status,
            crate::mgmt_ops::OpStatus::Completed
        );
        // 幂等回放：同 request_id 同参数重发 = 记录终态，不二次执行。
        let r2 = rpc.handle(&admin(1000), 1, "agent-restart", b"svc-ota", 1100);
        assert_eq!(r2.status, RpcStatus::Completed);
        assert!(r2.detail.contains("replay:"), "{}", r2.detail);
        assert_eq!(rpc.executor.executed.len(), 1, "回放不得二次执行");
        // 幂等冲突：同 request_id 异参数拒绝。
        let r3 = rpc.handle(&admin(1000), 1, "agent-restart", b"svc-mqtt", 1200);
        assert_eq!(r3.status, RpcStatus::Failed);
        assert!(r3.detail.starts_with("idempotency-conflict"));
    }
    // ================= 蓝图 70.1 WP-70.1.3：有界账本 =================

    /// 审计账有界：超过 [`MGMT_AUDIT_MAX_RECORDS`] 丢最旧并计数——
    /// 常驻字节与请求量解耦（N02 无界 Vec 收口），窗口内记录完整。
    #[test]
    fn bounded70_audit_ring_drops_oldest_and_counts() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        let total = MGMT_AUDIT_MAX_RECORDS + 8;
        for i in 0..total {
            let r = rpc.handle(&admin(1000), 100 + i as u64, "status", b"", 1000);
            assert_eq!(r.status, RpcStatus::Completed);
        }
        assert_eq!(rpc.audit.len(), MGMT_AUDIT_MAX_RECORDS, "账本封顶");
        assert_eq!(rpc.audit[0].action, "status", "窗口内记录类型不变");
        // 首条应是被挤出的第 9 条请求之后的那条（旧 0..8 已逐出）。
        assert_eq!(rpc.audit_dropped_count(), 8, "逐出计数 = 总数-容量");
        // 窗口语义：仍在窗口内的 request_id 判重，已逐出的不再判重。
        let r = rpc.handle(
            &admin(1000),
            100 + MGMT_AUDIT_MAX_RECORDS as u64,
            "status",
            b"",
            1001,
        );
        assert_eq!(r.detail, "replay-request", "窗口内 id 判重");
    }

    /// 重放窗口有界：超过 [`MGMT_REPLAY_SEEN_MAX`] 逐出最旧 request_id
    /// 并计数；逐出后同 id 不再判重（窗口语义如实——非持久操作账）。
    #[test]
    fn bounded70_replay_window_evicts_oldest() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: false,
            executed: vec![],
        });
        // 填满窗口（首个 id = 1）。
        for i in 1..=(MGMT_REPLAY_SEEN_MAX as u64) {
            let r = rpc.handle(&admin(1000), i, "status", b"", 1000);
            assert_eq!(r.status, RpcStatus::Completed);
        }
        assert_eq!(rpc.replay_seen.len(), MGMT_REPLAY_SEEN_MAX);
        // 再来一条 → 最旧（id=1）被逐出。
        let r = rpc.handle(
            &admin(1000),
            MGMT_REPLAY_SEEN_MAX as u64 + 1,
            "status",
            b"",
            1000,
        );
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(rpc.replay_dropped_count(), 1);
        assert_eq!(rpc.replay_seen.len(), MGMT_REPLAY_SEEN_MAX);
        // id=1 已逐出：重发不再判 replay-request（窗口外如实放行）。
        let r = rpc.handle(&admin(1000), 1, "status", b"", 1002);
        assert_ne!(r.detail, "replay-request", "窗口外 id 不得谎判重放");
        // 窗口内 id（如 500）仍判重。
        let r = rpc.handle(&admin(1000), 500, "status", b"", 1003);
        assert_eq!(r.detail, "replay-request");
    }
}

#[cfg(test)]
mod spec_c_history_query_tests {
    use super::*;

    /// SPEC-C：history-query 解析/RBAC/白名单直证。
    #[test]
    fn spec_c_history_query_parse_role_whitelist() {
        assert_eq!(
            MgmtAction::parse("history-query"),
            Ok(MgmtAction::HistoryQuery)
        );
        assert_eq!(MgmtAction::HistoryQuery.role_for(), "viewer");
        assert!(MgmtAction::HistoryQuery.in_whitelist());
        assert!(!MgmtAction::HistoryQuery.is_destructive());
        // 负例：kebab 变体拼写不宽容。
        assert!(MgmtAction::parse("history_query").is_err());
        assert!(MgmtAction::parse("historyQuery").is_err());
    }

    /// SPEC-C：viewer 可执行 history-query 且审计入链；
    /// 二进制 payload 经 handle 原样回传。
    #[test]
    fn spec_c_history_query_viewer_allowed_and_payload_passes_through() {
        struct PayloadExec;
        impl ActionExecutor for PayloadExec {
            fn execute(
                &mut self,
                _a: &MgmtAction,
                _s: &AuthenticatedSession,
                _arg: &[u8],
                _now: u64,
            ) -> ActionResult {
                ActionResult {
                    status: RpcStatus::Completed,
                    detail: alloc::string::String::from("history-query:point=1 total=3"),
                    payload: Some(alloc::vec![0xAA, 0xBB, 0xCC]),
                }
            }
        }
        let mut rpc = ManagementRpc::new(PayloadExec);
        let viewer = AuthenticatedSession {
            principal: alloc::string::String::from("scada"),
            roles: vec![alloc::string::String::from("viewer")],
            tenant_scope: 1,
            channel_binding: alloc::string::String::from("tls"),
            expires_at_ms: 2_000,
        };
        // arg = point u32 ‖ last_n u32。
        let arg: Vec<u8> = [1u32.to_le_bytes(), 16u32.to_le_bytes()].concat();
        let r = rpc.handle(&viewer, 9, "history-query", &arg, 1_000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(r.payload.as_deref(), Some(&[0xAAu8, 0xBB, 0xCC][..]));
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.audit[0].action, "history-query");
    }
}
