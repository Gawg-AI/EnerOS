//! 蓝图 61 Phase 5 WP-5.3（[10] §4.1–4.3）：`CommandArbiter` —— 同一
//! 资源的一切控制意图先经唯一仲裁。
//!
//! 唯一不变量（[10] §4.1）：同一资源的所有自动、人工、市场、联邦、维护
//! 和紧急控制先经本 Arbiter；优先级不是简单整数覆盖——按资源控制模式、
//! actor 权限/租约、电气工艺安全、已确认承诺、intent 生效窗、可抢占/
//! 合并、当前 Saga 状态与 fencing 综合裁决（[02] §12.4 严格字典序）。
//! **ModelSuggestion 永远不能直接成为命令来源**（只能经 objective-engine
//! 变成类型化候选）。
//!
//! 冲突（[10] §4.3）：
//! - 同资源互斥 intent：高优先级胜出，败者记录在案（`losers`）；
//! - 可叠加服务：不在命令层相加裸数值——回退到 Objective/solver 联合
//!   求解（`Decision::MergeInSolver`）；
//! - 执行中抢占：未发送命令直接取消；已发送/未知命令先 reconcile 再执行
//!   新意图（`Decision::Preempt(ReconcileFirst)`）；
//! - 人工模式：自动命令一律拒绝；切回自动必须持 fresh snapshot/capability
//!   （`Arbiter::resume_auto`），否则维持拒绝。

use alloc::string::String;
use alloc::vec::Vec;

/// 命令来源（[10] §4.2 九类闭枚举；优先级降序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandSource {
    /// 设备保护/SIS/硬联锁（最高——最终否决权）。
    EmergencyLocalProtection,
    /// 现场紧急停机与安全处置。
    LocalSafetyController,
    /// 本地人工接管/检修隔离（有租约）。
    LocalOperator,
    /// 调度机构强制指令（已确认）。
    GridOperatorConfirmed,
    /// 工艺控制器（连续工艺约束）。
    ProcessController,
    /// 已接受的市场/合同承诺计划。
    CommittedMarketPlan,
    /// 站级安全运行与经济优化（本产品决策链的生产来源）。
    SiteOptimizer,
    /// 联邦目标（企业/联邦层下发的承诺边界）。
    FederationObjective,
    /// 模型建议——**永不直接成为命令来源**（只能经 objective-engine）。
    ModelSuggestion,
}

impl CommandSource {
    /// 严格字典序优先级（数值小 = 高；[02] §12.4）。
    pub fn precedence(self) -> u8 {
        match self {
            CommandSource::EmergencyLocalProtection => 1,
            CommandSource::LocalSafetyController => 2,
            CommandSource::LocalOperator => 3,
            CommandSource::GridOperatorConfirmed => 4,
            CommandSource::ProcessController => 5,
            CommandSource::CommittedMarketPlan => 6,
            CommandSource::SiteOptimizer => 7,
            CommandSource::FederationObjective => 8,
            CommandSource::ModelSuggestion => 9,
        }
    }

    /// 紧急/安全类（可抢占一切，但仍需安全合同、审计与 readback）。
    pub fn is_emergency(self) -> bool {
        matches!(
            self,
            CommandSource::EmergencyLocalProtection | CommandSource::LocalSafetyController
        )
    }
}

/// 资源控制模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMode {
    /// 自动（生产闭环）。
    Auto,
    /// 人工模式：自动命令一律拒绝。
    Manual,
    /// 检修隔离：一切非紧急控制拒绝。
    Maintenance,
}

/// 待仲裁意图摘要（确定性裁决输入）。
#[derive(Debug, Clone)]
pub struct ArbitrationRequest {
    pub source: CommandSource,
    /// 资源（device_id 迁移期身份；多设备面待 Saga 装配扩展）。
    pub device_id: u32,
    /// actor 权限/租约有效（gateway 前置自证）。
    pub lease_valid: bool,
    /// 意图生效窗（now_ms ∈ [not_before, expires_at]）。
    pub valid_now: bool,
    /// 是否可抢占当前在途执行（不可抢占意图 = 顺序等待）。
    pub preemptible_target: bool,
    /// 在途命令是否已发送/未知（须先 reconcile 才能执行新意图）。
    pub in_flight_sent_or_unknown: bool,
}

/// 仲裁裁决。
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// 允许执行（唯一放行面——仍须经唯一 Gateway 授权）。
    Proceed,
    /// 拒绝；`losers` 恒携带至少一个败者记录（[10] §4.3：记录 loser）。
    Reject(Vec<Loser>),
    /// 可叠加服务：不在命令层相加——回 solver 联合求解。
    MergeInSolver(Vec<Loser>),
    /// 抢占：先取消未发送命令 + reconcile 已发送/未知命令，再执行新意图。
    Preempt(ReconcileFirst, Vec<Loser>),
}

/// 抢占前置动作（[10] §4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileFirst;

/// 败者记录（审计/可观测）。
#[derive(Debug, Clone, PartialEq)]
pub struct Loser {
    pub source: CommandSource,
    pub reason: String,
}

/// 仲裁上下文（资源当前状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceContext {
    pub mode: ControlMode,
    /// 在途 Saga 是否有非终态命令。
    pub saga_in_flight: bool,
    /// 人工 → 自动 切换已持 fresh snapshot/capability（[10] §4.3）。
    pub fresh_snapshot_after_manual: bool,
}

/// 唯一生产 Arbiter（[10] §4.7：全仓只有一个生产 Arbiter——本模块）。
#[derive(Debug, Default)]
pub struct CommandArbiter {
    /// 仲裁历史（败者记录；有界）。
    losers: Vec<(u64, Loser)>,
    seq: u64,
    cap: usize,
    /// 在途命令来源登记（device → 来源；执行终态即清除）。
    in_flight: alloc::collections::BTreeMap<u32, CommandSource>,
}

impl CommandArbiter {
    pub fn new(cap: usize) -> Self {
        Self {
            losers: Vec::new(),
            seq: 0,
            cap,
            in_flight: alloc::collections::BTreeMap::new(),
        }
    }

    /// 登记在途来源（执行面投递前调用）。
    pub fn register_in_flight(&mut self, device_id: u32, source: CommandSource) {
        self.in_flight.insert(device_id, source);
    }

    /// 清除在途登记（执行终态/对账收敛后调用）。
    pub fn clear_in_flight(&mut self, device_id: u32) {
        self.in_flight.remove(&device_id);
    }

    /// 裁决（[10] §4.2 全因素；确定性——同输入同裁决）。
    pub fn decide(&mut self, req: &ArbitrationRequest, ctx: &ResourceContext) -> Decision {
        // 0. ModelSuggestion 永不直接成命令（[10] §4.2 红线）。
        if req.source == CommandSource::ModelSuggestion {
            return self.reject(req, String::from("model suggestion 不能直接成为命令来源"));
        }
        // 1. 生效窗。
        if !req.valid_now {
            return self.reject(req, String::from("intent 不在生效窗"));
        }
        // 2. 控制模式。
        match ctx.mode {
            ControlMode::Maintenance => {
                if !req.source.is_emergency() {
                    return self.reject(req, String::from("检修隔离：仅紧急源可执行"));
                }
            }
            ControlMode::Manual => {
                if !req.source.is_emergency() {
                    return self.reject(req, String::from("人工模式：自动命令拒绝"));
                }
            }
            ControlMode::Auto => {}
        }
        // 3. 租约/权限（紧急源可无租约先行动——事后审计与 readback 必至）。
        if !req.lease_valid && !req.source.is_emergency() {
            return self.reject(req, String::from("租约/权限无效"));
        }
        // 4. 在途冲突（[10] §4.3：抢占/合并规则）。
        if ctx.saga_in_flight {
            let flying = self.in_flight_source(req.device_id);
            // 可叠加服务（站级优化 × 联邦目标）：不在命令层相加——回
            // solver 联合求解（[10] §4.3）。
            let flying_stackable = matches!(
                flying,
                Some(CommandSource::SiteOptimizer) | Some(CommandSource::FederationObjective)
            );
            let req_stackable = matches!(
                req.source,
                CommandSource::SiteOptimizer | CommandSource::FederationObjective
            );
            if flying_stackable && req_stackable {
                return Decision::MergeInSolver(self.take_losers(
                    req,
                    String::from("可叠加服务：回 Objective/solver 联合求解"),
                ));
            }
            match flying {
                Some(f) if f.precedence() <= req.source.precedence() => {
                    // 在途优先级更高/同级：不可抢占（同级互斥——先到先执行）。
                    return self.reject(
                        req,
                        alloc::format!("在途 {f:?} 优先级不低于新意图，等待其终态"),
                    );
                }
                Some(f) => {
                    // 高优先级抢占低优先级（[10] §4.3：未发送取消；
                    // 已发送/未知先 reconcile 再执行新意图）。
                    if !req.preemptible_target {
                        return self.reject(req, String::from("在途不可抢占"));
                    }
                    let reason = if req.in_flight_sent_or_unknown {
                        String::from("抢占：在途命令先 reconcile")
                    } else {
                        String::from("抢占：未发送命令取消")
                    };
                    let losers = alloc::vec![Loser {
                        source: f,
                        reason: reason.clone(),
                    }];
                    for l in &losers {
                        self.record(l.clone());
                    }
                    let _ = req;
                    return Decision::Preempt(ReconcileFirst, losers);
                }
                None => {}
            }
        }
        Decision::Proceed
    }

    /// 人工 → 自动切换（[10] §4.3：必须重新 Snapshot/Capability）。
    pub fn resume_auto(&mut self, fresh_snapshot: bool) -> Result<(), String> {
        if fresh_snapshot {
            Ok(())
        } else {
            Err(String::from("切回自动必须持 fresh snapshot/capability"))
        }
    }

    /// 在途命令来源（登记面真实查询；无登记 = 无在途）。
    pub(crate) fn in_flight_source(&self, device_id: u32) -> Option<CommandSource> {
        self.in_flight.get(&device_id).copied()
    }

    fn reject(&mut self, req: &ArbitrationRequest, reason: String) -> Decision {
        Decision::Reject(self.take_losers(req, reason))
    }

    fn take_losers(&mut self, req: &ArbitrationRequest, reason: String) -> Vec<Loser> {
        let loser = Loser {
            source: req.source,
            reason,
        };
        self.record(loser.clone());
        alloc::vec![loser]
    }

    fn record(&mut self, loser: Loser) {
        self.seq += 1;
        if self.losers.len() >= self.cap {
            self.losers.remove(0);
        }
        self.losers.push((self.seq, loser));
    }

    /// 败者记录（可观测面）。
    pub fn loser_log(&self) -> &[(u64, Loser)] {
        &self.losers
    }
}
