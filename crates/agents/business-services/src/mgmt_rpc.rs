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
                // 70.7.5：update-check（只读投影）/ update-rollback（取消
                // 未确认 trial）/ update-apply（无暂存包诚实拒绝）经
                // MGT2 root 中继接实——词形退役 ota-channel-unbound。
                | MgmtAction::UpdateCheck
                | MgmtAction::UpdateApply
                | MgmtAction::UpdateRollback
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

impl RpcStatus {
    /// MGT1 响应首字节（状态字节映射**单源**——蓝图 76 卷二 WP-76.2.6）。
    ///
    /// svc-mgmt 的 v2 验签臂与 v1 无签臂曾逐字抄两份同样的 match（漂移即
    /// 探针判据分叉）。值冻结：`Completed=1 / Accepted=2 / Failed=0`——
    /// root 探针与 agent-sdk 消费方依赖，改动是破坏性 wire 变更。
    ///
    /// `RpcStatus` 刻意留在本模块（它带 exec 环与 `ActionResult` 语义，不
    /// 是 mgmt-api 的契约面；卷二 §10 登记不修项）。
    pub fn wire_byte(self) -> u8 {
        match self {
            RpcStatus::Completed => 1u8,
            RpcStatus::Accepted => 2,
            RpcStatus::Failed => 0,
        }
    }
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

    /// F70-03（S2）：操作账本持久面可用性（无授权/坏介质/读取失败 = false）。
    ///
    /// **fail-closed 默认 = false**：需要幂等保障的副作用动作在存储不可
    /// 用时必须拒绝（不能"存储不可用但 executor 照跑"）。宿主测试替身
    /// 显式 opt-in true；生产 TargetExecutor 按启动期装载结果如实回答。
    fn ops_storage_available(&mut self) -> bool {
        false
    }

    /// F70-03（S2）：把账本状态提交到持久介质（`Ok(true)` = **已确认落盘**）。
    ///
    /// 合同（§6.3）：`Running` 意图必须在 executor 调用**之前**经本接缝
    /// 确认落盘；终态在回复 Completed 之前落盘。false = 提交失败或结果
    /// 不确定（调用方按 fail-closed 处理：意图阶段不执行副作用，终态
    /// 阶段不回 Completed 并保留周期重试）。默认 false（同上）。
    fn persist_ops_state(&mut self, _state: &crate::mgmt_ops::MgmtOpsState) -> bool {
        false
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
    /// F70-03（§6.3.4）：终态已观察但持久提交未成功 ⇒ 置位；由周期驱动
    /// （root MgmtPersistTick Send 面，§RS2）重试，不依赖用户再发新请求
    /// 触发。装配侧确认提交成功后调 [`Self::note_persist`] 清零。
    pending_persist: bool,
    /// recheck RS1：已在内存写成终态、但该终态**尚未确认落盘**的 op 集合
    /// （有界 ≤ [`MGMT_OPS_MAX`]；持久化以整个账本快照为单位，任一次成功
    /// 提交都使全集合收敛 ⇒ 清空）。回放/回查对集合内 op 不得返回
    /// Completed——盘上事实仍是 Running，冒称完成即违反 §6.3 表格
    /// 「终态持久成功……才可回复 Completed」。
    undurable_terminals: Vec<u64>,
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
            pending_persist: false,
            undurable_terminals: Vec::new(),
            audit_dropped: 0,
            replay_dropped: 0,
        }
    }

    /// F70-03：是否存在待重试的持久化（终态未确认落盘）。
    pub fn pending_persist(&self) -> bool {
        self.pending_persist
    }

    /// recheck RS1：op 的终态是否已确认落盘（未观察/未终态 = true——
    /// 该集合只收「已写终态但未确认持久」的 op）。
    fn terminal_persist_confirmed(&self, op_id: u64) -> bool {
        !self.undurable_terminals.contains(&op_id)
    }

    /// recheck RS1：把 op 记入「终态未确认落盘」集合（有界；重复标记
    /// 幂等）。任一次成功的整账本提交都会清空全集合。
    fn mark_undurable_terminal(&mut self, op_id: u64) {
        if !self.undurable_terminals.contains(&op_id)
            && self.undurable_terminals.len() < crate::mgmt_ops::MGMT_OPS_MAX
        {
            self.undurable_terminals.push(op_id);
        }
    }

    /// F70-03 + recheck RS1/RS2：装配侧持久化结果回填。true = 已确认 ⇒
    /// 清除待重试旗与未持久终态集合；**false = 提交失败 ⇒ 必须保留待
    /// 处理状态**（pending 置位——旧实现只在成功时动旗，异步路径失败后
    /// 无任何标志可触发重试，违反 §6.3.4「终态提交失败保留待处理状态」）。
    pub fn note_persist(&mut self, committed: bool) {
        if committed {
            self.pending_persist = false;
            self.undurable_terminals.clear();
        } else {
            self.pending_persist = true;
        }
    }

    /// F70-03：把当前账本提交到持久介质（装配侧周期兜底与 RPC 内部
    /// 意图/终态提交共用本口）。返回值 = 是否已确认落盘；**成功 ⇒ 整个
    /// 账本快照确认落盘**（pending 与未持久终态集合一并收敛清零）。
    fn persist_now(&mut self) -> bool {
        let state = crate::mgmt_ops::MgmtOpsState {
            ledger: self.ops.clone(),
        };
        if self.executor.persist_ops_state(&state) {
            self.pending_persist = false;
            self.undurable_terminals.clear();
            true
        } else {
            false
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
                    // F70-03（S2 §6.3）：持久意图**先于副作用**。
                    // ① 存储不可用（无授权/坏介质/读取失败）⇒ 撤回本次
                    //    新记录、executor 零调用（不能"存储不可用但
                    //    executor 照跑"）；只读诊断不受影响。
                    if !self.executor.ops_storage_available() {
                        self.ops.abort_new(op_id);
                        self.push_audit(AuditRecord {
                            principal: session.principal.clone(),
                            channel: session.channel_binding.clone(),
                            action: action_raw.into(),
                            result: "ops-store-unavailable".into(),
                            at_ms: now_ms,
                        });
                        return ActionResult {
                            status: RpcStatus::Failed,
                            detail: "ops-store-unavailable:side-effect-requires-ledger".into(),
                            payload: None,
                        };
                    }
                    // ② Running 意图确认落盘（提交失败/结果不确定 ⇒ 同样
                    //    撤回并拒绝——崩溃后既无记录也无执行，重放按新
                    //    请求受理，不丢幂等账）。
                    if !self.persist_now() {
                        self.ops.abort_new(op_id);
                        self.push_audit(AuditRecord {
                            principal: session.principal.clone(),
                            channel: session.channel_binding.clone(),
                            action: action_raw.into(),
                            result: "ops-intent-persist-failed".into(),
                            at_ms: now_ms,
                        });
                        return ActionResult {
                            status: RpcStatus::Failed,
                            detail: "ops-intent-persist-failed:executor-not-called".into(),
                            payload: None,
                        };
                    }
                    // ③ 意图已持久 ⇒ 才允许真实副作用。
                    let mut result = self.executor.execute(&action, session, arg, now_ms);
                    match result.status {
                        RpcStatus::Accepted => {
                            // 在途：已落盘的 Running 即恢复锚点（重启 →
                            // Unknown 对账，observe_operation 轮询收口）。
                        }
                        RpcStatus::Completed => {
                            let d = result.detail.clone();
                            self.ops.finish(
                                op_id,
                                crate::mgmt_ops::OpStatus::Completed,
                                &d,
                                now_ms,
                            );
                            // ④ 终态确认落盘：失败 ⇒ **不回 Completed**
                            //（§6.3：效果可能已发生但账本未确认——回
                            // 可查询的 Failed + 保留待重试，由周期面兜底）。
                            // recheck RS1：该 op 同时记入未持久终态集合——
                            // 在补写成功之前，重放/回查也不得回 Completed。
                            if !self.persist_now() {
                                self.pending_persist = true;
                                self.mark_undurable_terminal(op_id);
                                result.status = RpcStatus::Failed;
                                result.detail = alloc::format!(
                                    "op={op_id} terminal-persist-pending;requery-op"
                                );
                            }
                        }
                        RpcStatus::Failed => {
                            let d = result.detail.clone();
                            self.ops
                                .finish(op_id, crate::mgmt_ops::OpStatus::Failed, &d, now_ms);
                            if !self.persist_now() {
                                self.pending_persist = true;
                                self.mark_undurable_terminal(op_id);
                                result.detail = alloc::format!(
                                    "op={op_id} {};terminal-persist-pending",
                                    result.detail
                                );
                            }
                        }
                    }
                    if !result.detail.starts_with("op=") {
                        result.detail = alloc::format!("op={op_id} {}", result.detail);
                    }
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
                    // recheck RS1：内存终态若尚未确认落盘（未持久终态集合
                    // 在册），盘上事实仍是 Running——回放不得回 Completed，
                    // 返回可查询的 pending 词形（补写成功后自动收敛）。
                    let record = self.ops.get(op_id).expect("Replayed 必有记录");
                    let undurable = !self.terminal_persist_confirmed(op_id);
                    let r = match record.status {
                        crate::mgmt_ops::OpStatus::Running => ActionResult {
                            status: RpcStatus::Accepted,
                            detail: alloc::format!("op={op_id} in-progress (idempotent-replay)"),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Completed if undurable => ActionResult {
                            status: RpcStatus::Failed,
                            detail: alloc::format!(
                                "op={op_id} replay: terminal-persist-pending;requery-op"
                            ),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Completed => ActionResult {
                            status: RpcStatus::Completed,
                            detail: alloc::format!("op={op_id} replay: {}", record.detail),
                            payload: None,
                        },
                        crate::mgmt_ops::OpStatus::Failed if undurable => ActionResult {
                            status: RpcStatus::Failed,
                            detail: alloc::format!(
                                "op={op_id} replay: {};terminal-persist-pending;requery-op",
                                record.detail
                            ),
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
                    let mut line = alloc::format!(
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
                    // recheck RS1：内存 Completed 若未确认落盘（盘上仍是
                    // Running），查询不得回 Completed——保留可查询状态与
                    // operation_id，补写成功后自动收敛。
                    let status = if status == RpcStatus::Completed
                        && !self.terminal_persist_confirmed(op_id)
                    {
                        line.push_str(" terminal-persist-pending;requery-op");
                        RpcStatus::Failed
                    } else {
                        status
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
                        // recheck RS1：异步收口的终态必须在**本次回复之前**
                        // 确认落盘（§6.3「终态持久成功……才可回复 Completed」
                        // ——旧实现先回 Completed 再由外层兜底补写，崩溃后
                        // 承诺与持久账本不一致）。失败 ⇒ 回可查询的
                        // pending 词形并保留周期重试。
                        if !self.persist_now() {
                            self.pending_persist = true;
                            self.mark_undurable_terminal(op_id);
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: alloc::format!(
                                    "op={op_id} terminal-persist-pending;requery-op"
                                ),
                                payload: None,
                            };
                        }
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
        /// F70-03：持久面注入（存储可用性/提交成败；计数器供断言）。
        storage_ok: bool,
        persist_ok: bool,
        /// 前 N 次提交的既定结果（崩溃断点注入：意图成功/终态失败等）；
        /// 耗尽后回落 `persist_ok`。
        persist_plan: Vec<bool>,
        persist_calls: u32,
        /// recheck RS1：异步观测计划——首次 observe_operation 消费并返回
        /// （None = 仍在途）。模拟 agent-restart 的实例代次收口。
        observe: Option<(crate::mgmt_ops::OpStatus, String)>,
    }
    impl Default for TestExecutor {
        fn default() -> Self {
            TestExecutor {
                fail: false,
                executed: Vec::new(),
                storage_ok: true,
                persist_ok: true,
                persist_plan: Vec::new(),
                persist_calls: 0,
                observe: None,
            }
        }
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

        fn ops_storage_available(&mut self) -> bool {
            self.storage_ok
        }

        fn observe_operation(
            &mut self,
            _record: &crate::mgmt_ops::OperationRecord,
            _now_ms: u64,
        ) -> Option<(crate::mgmt_ops::OpStatus, String)> {
            self.observe.take()
        }

        fn persist_ops_state(&mut self, _state: &crate::mgmt_ops::MgmtOpsState) -> bool {
            let i = self.persist_calls as usize;
            self.persist_calls += 1;
            self.persist_plan.get(i).copied().unwrap_or(self.persist_ok)
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
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r = rpc.handle(&admin(1000), 1, "status", b"", 1000);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(rpc.audit.len(), 1);
        assert_eq!(rpc.executor.executed.len(), 1);
    }

    #[test]
    fn negative_unknown_action_not_defaulted() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r = rpc.handle(&admin(1000), 1, "totally-unknown", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("unknown-action"));
        assert!(rpc.executor.executed.is_empty());
    }

    #[test]
    fn negative_insufficient_role_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r = rpc.handle(&viewer(), 1, "factory-reset", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role"));
        assert!(rpc.executor.executed.is_empty());
    }

    #[test]
    fn negative_executor_failure_propagates() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            fail: true,
            ..TestExecutor::default()
        });
        let r = rpc.handle(&admin(1000), 1, "config-get", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "executor-down");
    }

    #[test]
    fn negative_replay_rejected_and_session_expiry() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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
        // 白名单面（70.7.5 起 OTA 三动作接实入列）。
        for a in [
            StatusQuery,
            AgentRestart,
            ConfigGet,
            LogQuery,
            SupportBundleExport,
            UpdateCheck,
            UpdateApply,
            UpdateRollback,
        ] {
            assert!(a.in_whitelist());
        }
    }

    /// 50：正例——viewer 可查询服务状态（只读）。
    #[test]
    fn positive_status_query_viewer_allowed() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r = rpc.handle(&viewer(), 1, "support-bundle", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role:operator"));
        assert!(rpc.executor.executed.is_empty());
    }

    /// 50：越权负例——operator 不能 AgentRestart（需 admin）。
    #[test]
    fn negative_agent_restart_operator_denied() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r = rpc.handle(&operator(1000), 1, "agent-restart", b"svc-ota", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("insufficient-role:admin"));
        assert!(rpc.executor.executed.is_empty());
    }

    /// 50：正例——admin 可受控重启（审计入链；70.6 起副作用动作入账本，
    /// 审计词形 `Completed#<op_id>`，操作可回查）。
    #[test]
    fn positive_agent_restart_admin_allowed() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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
        let mut rpc = ManagementRpc::new(TestExecutor::default());
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

    // ============================================================
    // F70-03（S2/S3）：管理意图持久化与完整请求幂等（M01～M08 host 面）
    // ============================================================

    /// M01/S3 红转绿：同 principal/request_id、空参数，先后
    /// `maintenance-enter` / `maintenance-exit`——第二个必须 Conflict，
    /// 只执行 enter，exit 不得拿 enter 的 Completed 充当结果。
    #[test]
    fn m01_same_id_different_action_conflicts_not_replay() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let enter = rpc.handle(&admin(1000), 123, "maintenance-enter", b"", 1000);
        assert_eq!(enter.status, RpcStatus::Completed, "{}", enter.detail);
        let exit = rpc.handle(&admin(1100), 123, "maintenance-exit", b"", 1100);
        assert_eq!(
            exit.status,
            RpcStatus::Failed,
            "不同动作同 ID 必须 Conflict"
        );
        assert!(
            exit.detail.contains("idempotency-conflict"),
            "{}",
            exit.detail
        );
        assert_eq!(rpc.executor.executed.len(), 1, "只执行 enter");
        assert!(
            rpc.executor.executed[0].contains("MaintenanceEnter"),
            "{:?}",
            rpc.executor.executed
        );
    }

    /// M02：完全相同请求在 Completed / Running（在途）/ 恢复后重放 ⇒
    /// 同 operation_id，执行计数不增加，状态如实。
    #[test]
    fn m02_identical_replay_same_op_no_double_execution() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        let r1 = rpc.handle(&admin(1000), 7, "maintenance-enter", b"cfg=1", 1000);
        assert_eq!(r1.status, RpcStatus::Completed);
        assert!(r1.detail.contains("op=1"));
        let r2 = rpc.handle(&admin(1200), 7, "maintenance-enter", b"cfg=1", 1200);
        assert_eq!(r2.status, RpcStatus::Completed, "{}", r2.detail);
        assert!(r2.detail.contains("op=1"), "同 operation_id：{}", r2.detail);
        assert!(r2.detail.contains("replay"), "{}", r2.detail);
        assert_eq!(rpc.executor.executed.len(), 1, "重放不重复执行");

        // 恢复后重放：从持久态重建（模拟重启——mark_unknown_inflight 只对
        // 在途；Completed 记录保持终态）。
        let mut restored = rpc.ops.clone();
        restored.mark_unknown_inflight(2000);
        let mut rpc2 = ManagementRpc::new(TestExecutor::default());
        rpc2.ops = restored;
        let r3 = rpc2.handle(&admin(2100), 7, "maintenance-enter", b"cfg=1", 2100);
        assert_eq!(r3.status, RpcStatus::Completed, "已终态记录的重放原样返回");
        assert!(r3.detail.contains("op=1"));
        assert_eq!(rpc2.executor.executed.len(), 0, "恢复后重放仍不重复执行");
    }

    /// M03：同键不同参数 = Conflict；未授权角色 = 拒绝且副作用 0 次。
    #[test]
    fn m03_same_key_different_arg_or_bad_identity_rejected() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        assert_eq!(
            rpc.handle(&admin(1000), 9, "config-set", b"aaa", 1000)
                .status,
            RpcStatus::Completed
        );
        let r = rpc.handle(&admin(1100), 9, "config-set", b"bbb", 1100);
        assert_eq!(r.status, RpcStatus::Failed);
        assert!(r.detail.contains("idempotency-conflict"));
        assert_eq!(rpc.executor.executed.len(), 1);

        // 非法身份（角色不足）：副作用 0 次（授权先于账本/执行）。
        let mut rpc2 = ManagementRpc::new(TestExecutor::default());
        let r2 = rpc2.handle(&viewer(), 10, "maintenance-enter", b"", 1000);
        assert_eq!(r2.status, RpcStatus::Failed);
        assert!(r2.detail.contains("insufficient-role"));
        assert!(rpc2.executor.executed.is_empty());
        assert!(rpc2.ops.records().is_empty(), "授权拒绝不入账本");
    }

    /// M04（host 面）：存储面不可用（无授权/坏介质/装载失败）⇒ 需幂等
    /// 保障的副作用拒绝、executor 0 次；只读诊断不受影响。
    #[test]
    fn m04_storage_unavailable_rejects_side_effects_only() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            storage_ok: false,
            ..TestExecutor::default()
        });
        let r = rpc.handle(&admin(1000), 3, "maintenance-enter", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(
            r.detail,
            "ops-store-unavailable:side-effect-requires-ledger"
        );
        assert!(rpc.executor.executed.is_empty());
        assert!(rpc.ops.records().is_empty(), "拒绝不留幻影在途记录");
        // 只读诊断照常。
        let q = rpc.handle(&admin(1100), 4, "update-check", b"", 1100);
        assert_eq!(q.status, RpcStatus::Completed, "只读不受存储面影响");
    }

    /// M05：意图提交失败（写/flush/CAS 失败或结果不确定）⇒ executor
    /// 调用 0 次、账本回到受理前形态；之后同请求重试按**新请求**受理
    ///（盘上从未有过该记录，不丢幂等账）。
    #[test]
    fn m05_intent_persist_failure_blocks_executor_and_rolls_back() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            persist_ok: false,
            ..TestExecutor::default()
        });
        let r = rpc.handle(&admin(1000), 5, "maintenance-enter", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "ops-intent-persist-failed:executor-not-called");
        assert!(rpc.executor.executed.is_empty());
        assert!(rpc.ops.records().is_empty(), "abort_new 撤回受理记录");
        assert_eq!(rpc.executor.persist_calls, 1, "意图提交恰一次（失败即停）");

        // 存储恢复后同一请求重试 ⇒ 新请求受理并成功（无残留冲突）。
        rpc.executor.persist_ok = true;
        let r2 = rpc.handle(&admin(2000), 5, "maintenance-enter", b"", 2000);
        assert_eq!(r2.status, RpcStatus::Completed, "{}", r2.detail);
        assert_eq!(rpc.executor.executed.len(), 1);
        assert!(
            r2.detail.contains("op=1"),
            "撤回后 op 号不跳空：{}",
            r2.detail
        );
    }

    /// M06：真实副作用已发生、终态持久提交失败 ⇒ **不回 Completed**；
    /// 状态可查询（Failed + terminal-persist-pending）；周期兜底提交成功
    /// 后收敛（pending 清零、账本 Completed、重放返回 Completed）。
    #[test]
    fn m06_terminal_persist_failure_keeps_queryable_state_and_converges() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            // 意图（第 1 次）成功，终态（第 2 次）失败——真实副作用已发生。
            persist_plan: vec![true, false],
            ..TestExecutor::default()
        });
        let r = rpc.handle(&admin(1000), 11, "maintenance-enter", b"", 1000);
        assert_eq!(rpc.executor.executed.len(), 1, "副作用已真实发生");
        assert_eq!(
            r.status,
            RpcStatus::Failed,
            "终态未确认落盘不得回 Completed"
        );
        assert!(
            r.detail.contains("terminal-persist-pending"),
            "{}",
            r.detail
        );
        assert!(r.detail.contains("op=1"));
        assert!(rpc.pending_persist(), "保留待重试旗");
        assert_eq!(
            rpc.ops.get(1).map(|x| x.status),
            Some(crate::mgmt_ops::OpStatus::Completed),
            "内存终态如实（可查询）"
        );

        // 周期兜底（消息驱动）：下一条消息触发重试 ⇒ 收敛。
        rpc.executor.persist_ok = true;
        let committed = rpc
            .executor
            .persist_ops_state(&crate::mgmt_ops::MgmtOpsState {
                ledger: rpc.ops.clone(),
            });
        rpc.note_persist(committed);
        assert!(committed);
        assert!(!rpc.pending_persist());
        // 重放返回已收敛的 Completed（不再 pending 词形）。
        let r2 = rpc.handle(&admin(1500), 11, "maintenance-enter", b"", 1500);
        assert_eq!(r2.status, RpcStatus::Completed, "{}", r2.detail);
        assert_eq!(rpc.executor.executed.len(), 1, "收敛后重放不重复执行");
    }

    /// M07：四个崩溃断点分别按**持久事实**恢复；可能已执行的动作不自动
    /// 重做（host 模拟：以"最后一次成功提交的账本快照"为盘上事实）。
    #[test]
    fn m07_crash_breakpoints_recover_from_persisted_facts() {
        // 断点 1：意图落盘前崩溃（提交失败即未执行）⇒ 盘上无记录。
        {
            let mut rpc = ManagementRpc::new(TestExecutor {
                persist_ok: false,
                ..TestExecutor::default()
            });
            let _ = rpc.handle(&admin(1000), 21, "maintenance-enter", b"", 1000);
            assert!(rpc.executor.executed.is_empty());
            // 盘 = 空；恢复后同请求 = 新请求（可受理，非冲突）。
            let mut rpc2 = ManagementRpc::new(TestExecutor::default());
            let r = rpc2.handle(&admin(2000), 21, "maintenance-enter", b"", 2000);
            assert_eq!(r.status, RpcStatus::Completed);
        }
        // 断点 2/3：意图已落盘、执行前/执行后终态前崩溃 ⇒ 盘上 Running，
        // 恢复转 Unknown；重放返回 Unknown（不自动重做——副作用可能已
        // 发生，物理 exactly-once 不可证，诚实 Unknown 可对账）。
        {
            // 盘上事实 = 只有 Running 意图（经 begin 构造 = 提交成功的
            // 持久形态）。
            let mut disk = crate::mgmt_ops::OperationLedger::new();
            let digest = crate::mgmt_ops::arg_digest(b"");
            assert!(matches!(
                disk.begin(22, "ops-1", "maintenance-enter", digest, 1, 1000),
                crate::mgmt_ops::OpGate::New(1)
            ));
            let mut rpc2 = ManagementRpc::new(TestExecutor::default());
            let mut restored = disk;
            restored.mark_unknown_inflight(3000);
            rpc2.ops = restored;
            let r = rpc2.handle(&admin(3100), 22, "maintenance-enter", b"", 3100);
            assert_eq!(r.status, RpcStatus::Failed, "Unknown 不得冒称成功");
            assert!(r.detail.contains("op=1"));
            assert!(
                rpc2.executor.executed.is_empty(),
                "可能已执行的动作不自动重做"
            );
        }
        // 断点 4：终态落盘后、回应前崩溃 ⇒ 盘上 Completed；重放返回
        // Completed 且不再执行。
        {
            let mut rpc = ManagementRpc::new(TestExecutor::default());
            let _ = rpc.handle(&admin(1000), 23, "maintenance-enter", b"", 1000);
            let disk = rpc.ops.clone();
            let mut rpc2 = ManagementRpc::new(TestExecutor::default());
            let mut restored = disk;
            restored.mark_unknown_inflight(4000);
            rpc2.ops = restored;
            let r = rpc2.handle(&admin(4100), 23, "maintenance-enter", b"", 4100);
            assert_eq!(r.status, RpcStatus::Completed, "{}", r.detail);
            assert!(rpc2.executor.executed.is_empty(), "终态在盘 ⇒ 零重执行");
        }
    }

    /// M08：账本满 + 在途 = 背压拒绝（不驱逐未决；已有既有锁，此处锁
    /// RPC 层词形与执行计数）。
    #[test]
    fn m08_full_ledger_backpressure_at_rpc_layer() {
        use crate::mgmt_ops::{OpStatus, MGMT_OPS_MAX};
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        // 直填账本到满（全 Running = 在途占位）。
        for i in 0..MGMT_OPS_MAX as u64 {
            let digest = crate::mgmt_ops::arg_digest(&i.to_le_bytes());
            assert!(matches!(
                rpc.ops
                    .begin(1000 + i, "ops-1", "agent-restart", digest, 1, i),
                crate::mgmt_ops::OpGate::New(_)
            ));
        }
        assert_eq!(rpc.ops.records().len(), MGMT_OPS_MAX);
        let r = rpc.handle(&admin(1000), 999, "maintenance-enter", b"", 1000);
        assert_eq!(r.status, RpcStatus::Failed);
        assert_eq!(r.detail, "ops-backpressure:ledger-full-with-inflight");
        assert!(rpc.executor.executed.is_empty());
        // 未决不被驱逐。
        assert_eq!(rpc.ops.records().len(), MGMT_OPS_MAX);
        assert!(rpc
            .ops
            .records()
            .iter()
            .all(|x| x.status == OpStatus::Running));
    }

    // ============================================================
    // recheck RS1/RS2（2026-09-19 复查缺陷）：终态持久确认边界——
    // 同步/异步/幂等重放/状态查询全部出口共用「终态落盘前不得回
    // Completed」的确认边界；提交失败保留待重试状态。
    // ============================================================

    /// RS1 同步面：真实副作用已发生、终态持久失败 ⇒ 首答/重放/查询三
    /// 出口都不得回 Completed；周期补写成功后收敛（重放回 Completed，
    /// 执行次数恒 1）。
    #[test]
    fn rs1_sync_terminal_undurable_replay_and_query_not_completed() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            // 意图（第 1 次）成功，终态（第 2 次）失败。
            persist_plan: vec![true, false],
            ..TestExecutor::default()
        });
        let first = rpc.handle(&admin(1000), 31, "maintenance-enter", b"", 1000);
        assert_eq!(rpc.executor.executed.len(), 1, "副作用已真实发生");
        assert_eq!(first.status, RpcStatus::Failed);
        assert!(
            first.detail.contains("terminal-persist-pending"),
            "{}",
            first.detail
        );

        // 幂等重放：盘上事实仍是 Running——不得回 Completed。
        let replay = rpc.handle(&admin(1100), 31, "maintenance-enter", b"", 1100);
        assert_eq!(replay.status, RpcStatus::Failed, "{}", replay.detail);
        assert!(
            replay.detail.contains("terminal-persist-pending"),
            "{}",
            replay.detail
        );
        assert_eq!(rpc.executor.executed.len(), 1, "重放零执行");

        // 状态查询 op:1：同样不得回 Completed。
        let query = rpc.handle(&admin(1200), 32, "status", b"op:1", 1200);
        assert_eq!(query.status, RpcStatus::Failed, "{}", query.detail);
        assert!(
            query.detail.contains("terminal-persist-pending"),
            "{}",
            query.detail
        );

        // 周期补写成功（装配侧 persist_ops + note_persist）⇒ 收敛。
        rpc.executor.persist_ok = true;
        let committed = rpc
            .executor
            .persist_ops_state(&crate::mgmt_ops::MgmtOpsState {
                ledger: rpc.ops.clone(),
            });
        rpc.note_persist(committed);
        assert!(committed);
        assert!(!rpc.pending_persist());
        let converged = rpc.handle(&admin(1300), 31, "maintenance-enter", b"", 1300);
        assert_eq!(
            converged.status,
            RpcStatus::Completed,
            "{}",
            converged.detail
        );
        assert_eq!(rpc.executor.executed.len(), 1, "收敛后仍零重复执行");
    }

    /// RS1 异步面：observe_operation 收口出终态时，终态必须在**本次回复
    /// 之前**确认落盘——提交失败 ⇒ 回复非 Completed + 保留待重试；补写
    /// 成功后查询收敛 Completed。
    #[test]
    fn rs1_async_observe_persists_terminal_before_reply() {
        let mut rpc = ManagementRpc::new(TestExecutor {
            observe: Some((
                crate::mgmt_ops::OpStatus::Completed,
                alloc::string::String::from("restart-complete:gen=2"),
            )),
            // 异步收口的终态提交失败（意图不经过 persist_now——Running
            // 由 begin 直建，模拟盘上已有意图）。
            persist_plan: vec![false],
            ..TestExecutor::default()
        });
        let digest = crate::mgmt_ops::arg_digest(b"svc-observability");
        assert!(matches!(
            rpc.ops.begin(40, "ops-1", "agent-restart", digest, 1, 1000),
            crate::mgmt_ops::OpGate::New(1)
        ));
        let poll = rpc.handle(&admin(1100), 41, "status", b"op:1", 1100);
        assert_eq!(
            poll.status,
            RpcStatus::Failed,
            "终态未落盘不得回 Completed：{}",
            poll.detail
        );
        assert!(
            poll.detail.contains("terminal-persist-pending"),
            "{}",
            poll.detail
        );
        assert!(rpc.pending_persist(), "保留待重试旗");
        assert_eq!(rpc.executor.persist_calls, 1, "收口步恰一次提交尝试");
        // 内存终态如实（可查询、不丢观测事实）。
        assert_eq!(
            rpc.ops.get(1).map(|r| r.status),
            Some(crate::mgmt_ops::OpStatus::Completed)
        );
        // 后续查询（同 op 再轮询）也不得回 Completed。
        let again = rpc.handle(&admin(1150), 42, "status", b"op:1", 1150);
        assert_eq!(again.status, RpcStatus::Failed, "{}", again.detail);
        assert!(again.detail.contains("terminal-persist-pending"));
        // 补写成功 ⇒ 收敛。
        rpc.executor.persist_ok = true;
        let committed = rpc
            .executor
            .persist_ops_state(&crate::mgmt_ops::MgmtOpsState {
                ledger: rpc.ops.clone(),
            });
        rpc.note_persist(committed);
        let done = rpc.handle(&admin(1200), 43, "status", b"op:1", 1200);
        assert_eq!(done.status, RpcStatus::Completed, "{}", done.detail);
        // 成功路径：下一次异步收口提交成功 ⇒ 直接回 Completed。
        let mut rpc2 = ManagementRpc::new(TestExecutor {
            observe: Some((
                crate::mgmt_ops::OpStatus::Completed,
                alloc::string::String::from("restart-complete:gen=5"),
            )),
            persist_plan: vec![true],
            ..TestExecutor::default()
        });
        assert!(matches!(
            rpc2.ops
                .begin(50, "ops-1", "agent-restart", digest, 1, 1000),
            crate::mgmt_ops::OpGate::New(1)
        ));
        let ok = rpc2.handle(&admin(2100), 51, "status", b"op:1", 2100);
        assert_eq!(ok.status, RpcStatus::Completed, "{}", ok.detail);
        assert!(!rpc2.pending_persist());
    }

    /// RS2：装配侧提交失败必须保留待重试旗（旧 note_persist(false) 无
    /// 操作——异步路径失败后无任何标志可触发重试）。
    #[test]
    fn rs2_note_persist_failure_retains_retry_flag() {
        let mut rpc = ManagementRpc::new(TestExecutor::default());
        assert!(!rpc.pending_persist());
        rpc.note_persist(false);
        assert!(
            rpc.pending_persist(),
            "提交失败 ⇒ pending 置位（§6.3.4 终态提交失败保留待处理状态）"
        );
        rpc.note_persist(true);
        assert!(!rpc.pending_persist(), "确认提交 ⇒ 清旗");
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

    /// 蓝图 76 卷二 WP-76.2.6：MGT1 响应首字节映射冻结（svc-mgmt 两臂从此
    /// 共用 [`RpcStatus::wire_byte`]——值改动即破坏 root 探针/agent-sdk 判据）。
    #[test]
    fn rpc_status_wire_byte_is_frozen() {
        assert_eq!(RpcStatus::Completed.wire_byte(), 1u8);
        assert_eq!(RpcStatus::Accepted.wire_byte(), 2u8);
        assert_eq!(RpcStatus::Failed.wire_byte(), 0u8);
    }
}
