//! 蓝图 61 Phase 5 WP-5.2（[10] §3）：`BatchSaga` —— 命令批执行 Saga
//! 与 Reconciler。
//!
//! 唯一不变量（[10] §3.1）：命令批在任意部分成功、服务崩溃、设备超时或
//! 网络分区后，能恢复并收敛到 `AppliedConfirmed`、`Failed`、`Compensated`
//! 或 `ManualIntervention`；**不会把未知变成功，也不会重复已确认副作用**。
//!
//! 批状态（[10] §3.2）：
//! `Pending → Authorizing → Executing ⇄ Reconciling →
//! Completed/Failed/Compensating → Compensated/ManualIntervention`
//!
//! 每次转换 append [`SagaEvent`]（batch/command/attempt、expected previous
//! 双侧状态、授权/回执摘要、读回证据、reason/time——[10] §3.2 字段集）；
//! expected-previous 强制（非法跳跃拒绝）。恢复 = [`BatchSaga::restore`]
//! （事件日志重放 + 确定性重编译的意图注入；序断链/状态矛盾 → None
//! fail-closed，人工处置，不猜重建）。
//!
//! 执行算法（[10] §3.3）：DAG ready set 按 `max_parallel` 调度；命令失败
//! 阻断依赖后继（Blocked 传播）；Unknown（投递后超时/回执不可解）→ 批
//! 进 Reconciling，readback-first 对账（[10] §3.4：超时后先 readback，
//! 非幂等动作禁止自动重发）；`RecoveredApplied` 需**双重独立校验**
//! （连续两次 reconcile 均确认才记 Applied，[10] §3.5）。
//!
//! 补偿（[10] §3.3.9）= 新意图、新授权、新 journal（经
//! [`SagaExecutor::compensate`] 全新执行），不改写历史；不可逆动作
//! （补偿提示 None）不可自动补偿 → ManualIntervention。
//!
//! exactly-once 诚实语义（[10] §3.4）：at-least-once transport + 幂等键 +
//! fencing + readback；不宣称网络/设备端绝对 exactly-once。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::{CommandIntent, CompensationHint};

/// 事件日志上界（有界内存；超限 fail-closed 停止追加）。
pub const MAX_SAGA_EVENTS: usize = 4096;

/// 批级 Saga 状态（[10] §3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchState {
    Pending,
    Authorizing,
    Executing,
    Reconciling,
    Completed,
    Failed,
    Compensating,
    Compensated,
    ManualIntervention,
}

impl BatchState {
    /// 终态。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            BatchState::Completed
                | BatchState::Failed
                | BatchState::Compensated
                | BatchState::ManualIntervention
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BatchState::Pending => "pending",
            BatchState::Authorizing => "authorizing",
            BatchState::Executing => "executing",
            BatchState::Reconciling => "reconciling",
            BatchState::Completed => "completed",
            BatchState::Failed => "failed",
            BatchState::Compensating => "compensating",
            BatchState::Compensated => "compensated",
            BatchState::ManualIntervention => "manual-intervention",
        }
    }

    fn allowed(from: BatchState, to: BatchState) -> bool {
        use BatchState::*;
        matches!(
            (from, to),
            (Pending, Authorizing)
                | (Authorizing, Executing)
                | (Executing, Reconciling)
                | (Reconciling, Executing)
                | (Executing, Completed)
                | (Executing, Failed)
                | (Executing, Compensating)
                | (Executing, ManualIntervention)
                | (Reconciling, Completed)
                | (Reconciling, Failed)
                | (Reconciling, Compensating)
                | (Reconciling, ManualIntervention)
                | (Compensating, Compensated)
                | (Compensating, ManualIntervention)
        )
    }
}

/// 命令级 Saga 状态（批内视角；与 journal 九态映射在 engine adapter）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdState {
    /// 未开始。
    Pending,
    /// 已提交 executor（在途）。
    Executing,
    /// AppliedConfirmed（唯一成功）。
    Applied,
    /// 明确失败/被拒（终态）。
    Failed,
    /// 投递后未知（不猜成败）。
    Unknown,
    /// 依赖失败被阻断（终态；不投递）。
    Blocked,
    /// 对账中。
    Reconciling,
    /// 补偿执行中。
    Compensating,
    /// 补偿完成（终态）。
    Compensated,
}

impl CmdState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CmdState::Applied | CmdState::Failed | CmdState::Blocked | CmdState::Compensated
        )
    }

    fn allowed(from: CmdState, to: CmdState) -> bool {
        use CmdState::*;
        matches!(
            (from, to),
            (Pending, Executing)
                | (Pending, Blocked)
                | (Executing, Applied)
                | (Executing, Failed)
                | (Executing, Unknown)
                | (Executing, Blocked)
                | (Unknown, Reconciling)
                | (Reconciling, Applied)
                | (Reconciling, Failed)
                | (Applied, Compensating)
                | (Compensating, Compensated)
        )
    }
}

/// 批失败裁决策略（[10] §3.3.8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPolicy {
    /// 保持已成功命令，批终态 Failed。
    KeepSucceeded,
    /// 对全部 Applied 命令逆序补偿（不可补偿 → Manual）。
    Compensate,
    /// 直接人工介入。
    Manual,
}

/// Saga 执行策略。
#[derive(Debug, Clone)]
pub struct SagaPolicy {
    pub batch: BatchPolicy,
    /// 跨设备最大并行度（同设备恒串行——依赖边保证）。
    pub max_parallel: u16,
}

impl Default for SagaPolicy {
    fn default() -> Self {
        Self {
            batch: BatchPolicy::KeepSucceeded,
            max_parallel: 1,
        }
    }
}

/// executor 单命令结果（引擎 12 步合核/测试执行器的映射产物）。
#[derive(Debug, Clone, PartialEq)]
pub enum CmdOutcome {
    /// AppliedConfirmed（回执摘要 + 读回值证据）。
    Applied {
        receipt_digest: [u8; 32],
        readback: Option<f64>,
    },
    /// 零投递拒绝（终态）。
    Rejected { reason: String },
    /// 明确失败（写失败/读回不符——终态）。
    Failed { reason: String },
    /// 投递后未知（必须对账，禁止盲发）。
    Unknown {
        request_digest: [u8; 32],
        reason: String,
    },
}

/// 对账结果（[10] §3.5 Reconciler 判据）。
#[derive(Debug, Clone, PartialEq)]
pub enum ReconcileOutcome {
    /// 设备状态确认 = 授权值。`double_verified == false` 时驱动器发起
    /// 第二次独立校验（RecoveredApplied 需双重校验）。
    ConfirmedApplied {
        readback: f64,
        double_verified: bool,
    },
    /// 确认未生效（读回与授权值明确不符）。
    FailedConfirmed { reason: String },
    /// 无法确定 → 人工。
    ResolvedManual { reason: String },
}

/// 执行器 seam：生产 = 12 步合核 adapter（engine）；测试 = 故障注入执行器。
pub trait SagaExecutor {
    /// 执行单命令（含 authorize/persist/deliver/ack/readback/journal）。
    fn execute(&mut self, intent: &CommandIntent, attempt: u32) -> CmdOutcome;
    /// readback-first 对账（[10] §3.4：超时后先 readback，不盲发）。
    fn reconcile(&mut self, intent: &CommandIntent) -> ReconcileOutcome;
    /// 补偿 = 新意图、新授权、新 journal（restore 值由调用方取自意图
    /// 补偿提示——本模块不发明数值）。
    fn compensate(&mut self, intent: &CommandIntent, restore_kw: f64) -> CmdOutcome;
}

/// 事件类别（批级或命令级；expected-previous 双侧入码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Batch {
        from: BatchState,
        to: BatchState,
    },
    Command {
        intent_id: [u8; 16],
        from: CmdState,
        to: CmdState,
    },
}

/// Saga 事件（[10] §3.2 转换记录全集；追加式，不改写）。
#[derive(Debug, Clone, PartialEq)]
pub struct SagaEvent {
    pub seq: u64,
    pub batch_id: [u8; 16],
    pub kind: EventKind,
    pub attempt: u32,
    /// 授权/回执摘要（Applied = receipt digest；Unknown = request digest）。
    pub digest: [u8; 32],
    /// 读回证据。
    pub readback: Option<f64>,
    pub reason: String,
    pub at_ms: u64,
}

/// 命令批内单命令状态。
#[derive(Debug, Clone)]
struct SagaCommand {
    intent: CommandIntent,
    state: CmdState,
    attempts: u32,
    digest: [u8; 32],
    readback: Option<f64>,
}

/// 单命令批终态视图（报告/审计面——[10] §3.7 可观测判据）。
#[derive(Debug, Clone, PartialEq)]
pub struct CmdReport {
    pub intent_id: [u8; 16],
    pub state: CmdState,
    pub attempts: u32,
    pub readback: Option<f64>,
}

/// 驱动结果。
#[derive(Debug, Clone, PartialEq)]
pub struct DriveReport {
    pub batch_id: [u8; 16],
    pub final_state: BatchState,
    pub commands: Vec<CmdReport>,
    pub events: u64,
}

/// 命令批 Saga。
pub struct BatchSaga {
    batch_id: [u8; 16],
    /// 依赖边（意图索引对；编译器产出）。
    edges: Vec<(u32, u32)>,
    deadline_ms: u64,
    commands: Vec<SagaCommand>,
    state: BatchState,
    events: Vec<SagaEvent>,
    event_seq: u64,
    policy: SagaPolicy,
}

impl BatchSaga {
    /// 由编译批构造（Pending 态）。
    pub fn new(
        batch_id: [u8; 16],
        intents: &[CommandIntent],
        edges: &[(u32, u32)],
        deadline_ms: u64,
        policy: SagaPolicy,
    ) -> Self {
        Self {
            batch_id,
            edges: edges.to_vec(),
            deadline_ms,
            commands: intents
                .iter()
                .map(|i| SagaCommand {
                    intent: *i,
                    state: CmdState::Pending,
                    attempts: 0,
                    digest: [0; 32],
                    readback: None,
                })
                .collect(),
            state: BatchState::Pending,
            events: Vec::new(),
            event_seq: 0,
            policy,
        }
    }

    pub fn state(&self) -> BatchState {
        self.state
    }
    pub fn events(&self) -> &[SagaEvent] {
        &self.events
    }
    pub fn batch_id(&self) -> &[u8; 16] {
        &self.batch_id
    }

    /// 从事件日志恢复（[10] §3.5 启动扫描的批级形态）。
    ///
    /// 意图由调用方注入（WP-5.1 确定性重编译保证同批同意图）；事件序
    /// 断链、batch_id 不符、状态转换非法、意图集不匹配 → `None`
    /// （fail-closed，人工处置，不猜重建）。
    pub fn restore(
        events: &[SagaEvent],
        intents: &[CommandIntent],
        edges: &[(u32, u32)],
        deadline_ms: u64,
        policy: SagaPolicy,
    ) -> Option<Self> {
        let first = events.first()?.clone();
        let mut saga = Self::new(first.batch_id, intents, edges, deadline_ms, policy);
        for e in events {
            if e.batch_id != saga.batch_id || e.seq != saga.event_seq + 1 {
                return None;
            }
            match e.kind {
                EventKind::Batch { from, to } => {
                    if from != saga.state || !BatchState::allowed(from, to) {
                        return None;
                    }
                    saga.state = to;
                }
                EventKind::Command {
                    intent_id,
                    from,
                    to,
                } => {
                    let idx = saga
                        .commands
                        .iter()
                        .position(|c| c.intent.intent_id().0 == intent_id)?;
                    let c = &mut saga.commands[idx];
                    if c.state != from || !CmdState::allowed(from, to) {
                        return None;
                    }
                    c.state = to;
                    c.attempts = c.attempts.max(e.attempt);
                    if e.digest != [0u8; 32] {
                        c.digest = e.digest;
                    }
                    if e.readback.is_some() {
                        c.readback = e.readback;
                    }
                }
            }
            saga.event_seq = e.seq;
            saga.events.push(e.clone());
        }
        // 补偿执行中途崩溃（至少一条命令已进入 Compensating——补偿结果
        // 未知）：fail-closed 交人工处置（[10] §3.3.9 补偿未知 →
        // ManualIntervention，不发明效果）——本模块无自动再补偿通道，
        // 恢复出 Compensating 命令只会让 drive() 停在不可推进态。
        if saga
            .commands
            .iter()
            .any(|c| c.state == CmdState::Compensating)
        {
            return None;
        }
        // 批已转 Compensating 但尚无命令开始补偿（截断点在批转型与首条
        // 补偿记录之间）：物理状态已知（Applied 集合未变）——恢复安全：
        // 回 Executing 让 settle 重入补偿（补偿键独立派生、幂等）。
        if saga.state == BatchState::Compensating {
            saga.state = BatchState::Executing;
        }
        if saga.state.is_terminal() {
            return Some(saga);
        }
        // 恢复语义（[10] §3.4 at-least-once + 幂等键）：崩溃时在途的命令
        // （Executing/Unknown/Reconciling）重置为 Pending——重启后重提交，
        // 双重效果由 executor 幂等键/日志兜底；批 Reconciling → Executing。
        for c in &mut saga.commands {
            if matches!(
                c.state,
                CmdState::Executing | CmdState::Unknown | CmdState::Reconciling
            ) {
                c.state = CmdState::Pending;
            }
        }
        if saga.state == BatchState::Reconciling {
            saga.state = BatchState::Executing;
        }
        Some(saga)
    }

    /// 批级转换（expected-previous 强制；append 事件）。
    fn transition(&mut self, to: BatchState, reason: &str, at_ms: u64) -> Result<(), String> {
        let from = self.state;
        if !BatchState::allowed(from, to) {
            return Err(alloc::format!("illegal batch transition {from:?}->{to:?}"));
        }
        self.append_event(
            EventKind::Batch { from, to },
            0,
            [0; 32],
            None,
            reason,
            at_ms,
        )?;
        self.state = to;
        Ok(())
    }

    /// 命令级事件 append（含 expected-previous 校验）。
    #[allow(clippy::too_many_arguments)]
    fn record_command(
        &mut self,
        idx: usize,
        to: CmdState,
        attempt: u32,
        digest: [u8; 32],
        readback: Option<f64>,
        reason: &str,
        at_ms: u64,
    ) -> Result<(), String> {
        let intent_id = self.commands[idx].intent.intent_id().0;
        let from = self.commands[idx].state;
        if !CmdState::allowed(from, to) {
            return Err(alloc::format!("illegal cmd transition {from:?}->{to:?}"));
        }
        self.append_event(
            EventKind::Command {
                intent_id,
                from,
                to,
            },
            attempt,
            digest,
            readback,
            reason,
            at_ms,
        )?;
        let c = &mut self.commands[idx];
        c.state = to;
        c.digest = digest;
        if readback.is_some() {
            c.readback = readback;
        }
        Ok(())
    }

    fn append_event(
        &mut self,
        kind: EventKind,
        attempt: u32,
        digest: [u8; 32],
        readback: Option<f64>,
        reason: &str,
        at_ms: u64,
    ) -> Result<(), String> {
        if self.events.len() >= MAX_SAGA_EVENTS {
            return Err(String::from("saga event log full"));
        }
        self.event_seq += 1;
        self.events.push(SagaEvent {
            seq: self.event_seq,
            batch_id: self.batch_id,
            kind,
            attempt,
            digest,
            readback,
            reason: reason.into(),
            at_ms,
        });
        Ok(())
    }

    /// 依赖失败链传播（Blocked 标记；[10] §3.3.7）。
    fn propagate_blocked(&mut self) {
        loop {
            let mut changed = false;
            for &(a, b) in &self.edges {
                let sa = self.commands[a as usize].state;
                let sb = self.commands[b as usize].state;
                if matches!(sa, CmdState::Failed | CmdState::Blocked) && sb == CmdState::Pending {
                    self.commands[b as usize].state = CmdState::Blocked;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// ready 判据：依赖全 Applied。
    fn ready(&self, idx: usize) -> bool {
        self.edges.iter().all(|&(a, b)| {
            b as usize != idx || self.commands[a as usize].state == CmdState::Applied
        })
    }

    /// 驱动至终态（[10] §3.3 执行算法）。`now_ms`：单调毫秒时钟。
    pub fn drive<E: SagaExecutor>(
        &mut self,
        ex: &mut E,
        now_ms: u64,
    ) -> Result<DriveReport, String> {
        match self.state {
            BatchState::Pending => {
                self.transition(BatchState::Authorizing, "saga start", now_ms)?;
                // 逐命令 re-authorize 在 executor 内（生产 = 唯一 Gateway，
                // [10] §3.3.3）。
                self.transition(BatchState::Executing, "authorized (chain)", now_ms)?;
            }
            BatchState::Authorizing => {
                self.transition(BatchState::Executing, "resumed (authorized)", now_ms)?;
            }
            BatchState::Executing => {}
            other => return Err(alloc::format!("drive on non-resumable state {other:?}")),
        }

        loop {
            self.propagate_blocked();
            // ready set（[10] §3.3.2：DAG ready + max_parallel + 故障域；
            // not_before [10] §2.3：未到期命令不进 ready——留待到期 tick，
            // 不让其反复占位导致空转）。
            let ready: Vec<usize> = self
                .commands
                .iter()
                .enumerate()
                .filter(|(_, c)| c.state == CmdState::Pending)
                .map(|(i, _)| i)
                .filter(|&i| self.ready(i) && now_ms >= self.commands[i].intent.valid_from_ms())
                .collect();
            if ready.is_empty() {
                break;
            }
            let take = ready.len().min(self.policy.max_parallel.max(1) as usize);
            for &idx in ready.iter().take(take) {
                // 批过期：停止执行剩余命令（[10] §3.5：旧计划过期不继续）。
                if now_ms > self.deadline_ms {
                    self.record_command(
                        idx,
                        CmdState::Blocked,
                        0,
                        [0; 32],
                        None,
                        "batch expired",
                        now_ms,
                    )?;
                    continue;
                }
                let intent = self.commands[idx].intent;
                let attempt = self.commands[idx].attempts + 1;
                self.commands[idx].attempts = attempt;
                // [10] §3.2：投递前 append（Pending → Executing）。
                self.record_command(
                    idx,
                    CmdState::Executing,
                    attempt,
                    [0; 32],
                    None,
                    "submitted",
                    now_ms,
                )?;
                let outcome = ex.execute(&intent, attempt);
                match outcome {
                    CmdOutcome::Applied {
                        receipt_digest,
                        readback,
                    } => {
                        self.record_command(
                            idx,
                            CmdState::Applied,
                            attempt,
                            receipt_digest,
                            readback,
                            "applied",
                            now_ms,
                        )?;
                    }
                    CmdOutcome::Rejected { reason } => {
                        self.record_command(
                            idx,
                            CmdState::Failed,
                            attempt,
                            [0; 32],
                            None,
                            &reason,
                            now_ms,
                        )?;
                    }
                    CmdOutcome::Failed { reason } => {
                        self.record_command(
                            idx,
                            CmdState::Failed,
                            attempt,
                            [0; 32],
                            None,
                            &reason,
                            now_ms,
                        )?;
                    }
                    CmdOutcome::Unknown {
                        request_digest,
                        reason,
                    } => {
                        self.record_command(
                            idx,
                            CmdState::Unknown,
                            attempt,
                            request_digest,
                            None,
                            &reason,
                            now_ms,
                        )?;
                        // 批 → Reconciling：readback-first 对账（§3.4）。
                        self.transition(BatchState::Reconciling, "unknown outcome", now_ms)?;
                        self.reconcile_one(ex, idx, now_ms)?;
                        if self.state == BatchState::Reconciling {
                            self.transition(BatchState::Executing, "reconciled", now_ms)?;
                        }
                    }
                }
            }
            if self.state != BatchState::Executing {
                break;
            }
        }

        // 终局裁决（[10] §3.3.8）。
        self.settle(ex, now_ms)?;
        Ok(self.report())
    }

    /// 单命令对账（[10] §3.4/§3.5）：readback-first；RecoveredApplied 需
    /// 双重独立校验。
    fn reconcile_one<E: SagaExecutor>(
        &mut self,
        ex: &mut E,
        idx: usize,
        now_ms: u64,
    ) -> Result<(), String> {
        self.record_command(
            idx,
            CmdState::Reconciling,
            self.commands[idx].attempts,
            [0; 32],
            None,
            "reconcile start",
            now_ms,
        )?;
        let intent = self.commands[idx].intent;
        match ex.reconcile(&intent) {
            ReconcileOutcome::ConfirmedApplied {
                readback,
                double_verified,
            } => {
                if double_verified {
                    let attempt = self.commands[idx].attempts;
                    self.record_command(
                        idx,
                        CmdState::Applied,
                        attempt,
                        [0; 32],
                        Some(readback),
                        "recovered-applied (double-verified)",
                        now_ms,
                    )?;
                } else {
                    // 第二次独立校验（[10] §3.5 双重校验判据）。
                    match ex.reconcile(&intent) {
                        ReconcileOutcome::ConfirmedApplied { readback: rb2, .. } => {
                            let attempt = self.commands[idx].attempts;
                            self.record_command(
                                idx,
                                CmdState::Applied,
                                attempt,
                                [0; 32],
                                Some(rb2),
                                "recovered-applied (2nd verify)",
                                now_ms,
                            )?;
                        }
                        _ => {
                            let attempt = self.commands[idx].attempts;
                            self.record_command(
                                idx,
                                CmdState::Failed,
                                attempt,
                                [0; 32],
                                None,
                                "double-verify failed",
                                now_ms,
                            )?;
                        }
                    }
                }
                Ok(())
            }
            ReconcileOutcome::FailedConfirmed { reason } => {
                let attempt = self.commands[idx].attempts;
                self.record_command(
                    idx,
                    CmdState::Failed,
                    attempt,
                    [0; 32],
                    None,
                    &reason,
                    now_ms,
                )?;
                Ok(())
            }
            ReconcileOutcome::ResolvedManual { reason } => {
                let attempt = self.commands[idx].attempts;
                self.record_command(
                    idx,
                    CmdState::Failed,
                    attempt,
                    [0; 32],
                    None,
                    &reason,
                    now_ms,
                )?;
                self.transition(
                    BatchState::ManualIntervention,
                    "reconcile unresolved",
                    now_ms,
                )
            }
        }
    }

    /// 终局：全 Applied → Completed；有失败按策略（[10] §3.3.8/9）。
    fn settle<E: SagaExecutor>(&mut self, ex: &mut E, now_ms: u64) -> Result<(), String> {
        self.propagate_blocked();
        if self.state.is_terminal() {
            return Ok(()); // 对账已判人工
        }
        let all_applied = !self.commands.is_empty()
            && self
                .commands
                .iter()
                .all(|c| c.state == CmdState::Applied || c.state == CmdState::Compensated);
        if all_applied {
            return match self.state {
                BatchState::Executing => {
                    self.transition(BatchState::Completed, "all applied", now_ms)
                }
                BatchState::Reconciling => self.transition(
                    BatchState::Completed,
                    "all applied (post-reconcile)",
                    now_ms,
                ),
                _ => Ok(()),
            };
        }
        // 未到期等待窗口（not_before，[10] §2.3）：剩余命令全为
        // Pending 且 valid_from 尚在未来——批保持 Executing 不裁决
        // （不得按「失败」处置未来步；到期 tick 再驱动）。任何已失败/
        // 阻断/未知命令存在时仍按策略裁决。
        if self
            .commands
            .iter()
            .any(|c| c.state == CmdState::Pending && c.intent.valid_from_ms() > now_ms)
            && self
                .commands
                .iter()
                .all(|c| matches!(c.state, CmdState::Applied | CmdState::Pending))
        {
            return Ok(());
        }
        match self.state {
            BatchState::Executing | BatchState::Reconciling => {}
            _ => return Ok(()),
        }
        match self.policy.batch {
            BatchPolicy::KeepSucceeded => {
                self.transition(BatchState::Failed, "failure (keep-succeeded)", now_ms)
            }
            BatchPolicy::Manual => {
                self.transition(BatchState::ManualIntervention, "policy manual", now_ms)
            }
            BatchPolicy::Compensate => {
                self.transition(BatchState::Compensating, "compensating", now_ms)?;
                // 逆序补偿全部 Applied（新意图、新授权、新 journal）。
                let applied: Vec<usize> = self
                    .commands
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.state == CmdState::Applied)
                    .map(|(i, _)| i)
                    .collect();
                for idx in applied.iter().rev() {
                    let intent = self.commands[*idx].intent;
                    let restore = match intent.compensation() {
                        CompensationHint::RestorePowerKw(v) => *v,
                        CompensationHint::None => {
                            // 不可逆动作：不可自动补偿 → 人工。
                            return self.transition(
                                BatchState::ManualIntervention,
                                "irreversible applied cmd cannot auto-compensate",
                                now_ms,
                            );
                        }
                    };
                    let attempt = self.commands[*idx].attempts + 1;
                    self.commands[*idx].attempts = attempt;
                    self.record_command(
                        *idx,
                        CmdState::Compensating,
                        attempt,
                        [0; 32],
                        None,
                        "compensation (new intent)",
                        now_ms,
                    )?;
                    let outcome = ex.compensate(&intent, restore);
                    match outcome {
                        CmdOutcome::Applied {
                            receipt_digest,
                            readback,
                        } => {
                            self.record_command(
                                *idx,
                                CmdState::Compensated,
                                attempt,
                                receipt_digest,
                                readback,
                                "compensated",
                                now_ms,
                            )?;
                        }
                        CmdOutcome::Rejected { reason } | CmdOutcome::Failed { reason } => {
                            self.transition(
                                BatchState::ManualIntervention,
                                &alloc::format!("compensation: {reason}"),
                                now_ms,
                            )?;
                            return Ok(());
                        }
                        CmdOutcome::Unknown { reason, .. } => {
                            // 补偿效果未知不猜、不再自动施加新效果。
                            self.transition(
                                BatchState::ManualIntervention,
                                &alloc::format!("compensation unknown: {reason}"),
                                now_ms,
                            )?;
                            return Ok(());
                        }
                    }
                }
                self.transition(BatchState::Compensated, "all applied compensated", now_ms)
            }
        }
    }

    /// 报告（[10] §3.7：每命令真实状态 + 批整体语义）。
    pub fn report(&self) -> DriveReport {
        DriveReport {
            batch_id: self.batch_id,
            final_state: self.state,
            commands: self
                .commands
                .iter()
                .map(|c| CmdReport {
                    intent_id: c.intent.intent_id().0,
                    state: c.state,
                    attempts: c.attempts,
                    readback: c.readback,
                })
                .collect(),
            events: self.event_seq,
        }
    }
}

/// 启动 Reconciler（[10] §3.5）判据：非终态批需要收敛扫描；过期批不继续
/// 执行剩余命令（由 drive 的 deadline 阻断路径落实）。
pub fn needs_reconcile(state: BatchState, deadline_ms: u64, now_ms: u64) -> bool {
    if state.is_terminal() {
        return false;
    }
    match state {
        // Pending/Authorizing 从未投递：过期即可安全丢弃。
        BatchState::Pending | BatchState::Authorizing => now_ms <= deadline_ms,
        _ => true,
    }
}
