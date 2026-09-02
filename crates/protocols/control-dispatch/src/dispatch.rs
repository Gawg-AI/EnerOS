//! 蓝图 40 WP-40.6：`ControlDispatch` 深接口与 12 态状态机。
//!
//! 唯一控制写出口：`submit(proposal, telemetry, actor, deadline)` 经
//! validate → authorize → persist → deliver → ACK → readback → audit 全链，
//! 返回带 [`DispatchState`] 的 [`DispatchReceipt`]——调用方（svc-energy-agent）
//! 只以 `final_state == Committed` 作为成功判据（learning 门控）。
//!
//! [`DispatchState`] 定义与码表单源（蓝图 63 WP-63.1.1）：
//! 蓝图 62 之前，12 态码表在三个面各自手写逐臂重复（回执 wire encode/
//! decode、durable 持久化解码、root 显示）且类型本体定义在此——root-task
//! 不得链接 control-dispatch（依赖闭包拖入 dual-brain/serde_json），码表
//! 收口因此把 **定义 + 码表单源移到零依赖 crate `eneros-dispatch-state`**；
//! 本模块 re-export 保持 `eneros_control_dispatch::DispatchState` 全仓类型
//! 身份不变（集成测试/消费方零改动）。定义锁测试（往返/未知码/冻结词形）
//! 在 dispatch-state crate 内。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_gateway::envelope::ActorKind;
use eneros_dual_brain::ProposedControlPlan;
use eneros_safety_validator::TrustedTelemetry;

/// 认证调用方（WP-40.6 步骤 3：actor/capability/lease/fencing）。
///
/// EIPC 层 capability 由 seL4 endpoint badge 认证（服务装配期授予）；
/// 本类型承载业务身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticatedActor {
    /// 调用方角色（AI 决策链 = `Ai`）。
    pub kind: ActorKind,
    /// 调用方服务标识。
    pub service: [u8; 16],
    /// 会话 epoch（重启计数；旧会话拒绝）。
    pub session_epoch: u64,
}

/// 投递状态机（WP-40.6 12 态；`Committed` 是唯一成功终态）。
///
/// 蓝图 63 WP-63.1.1：定义与码表（to_code/from_code/display_name）在
/// `eneros-dispatch-state`（re-export 源）；本处 re-export 保持类型身份。
#[doc(inline)]
pub use eneros_dispatch_state::DispatchState;

/// 投递回执（`final_state` 是唯一成功判据；`state_path` 供审计/对账）。
#[derive(Debug, Clone)]
pub struct DispatchReceipt {
    /// 命令 ID（信封 command_id）。
    pub command_id: [u8; 16],
    /// 关联 ID（correlation）。
    pub correlation_id: [u8; 16],
    /// 终态。
    pub final_state: DispatchState,
    /// 状态路径（12 步链轨迹）。
    pub state_path: Vec<DispatchState>,
    /// 提交时刻（单调毫秒；非 Committed 为 None）。
    pub committed_at_ms: Option<u64>,
    /// 结构化说明（拒绝/失败/不确定原因；Committed 为 None）。
    pub note: Option<String>,
    /// 端到端延迟（ms）。
    pub latency_ms: u64,
}

/// 投递错误（**前置失败零投递**——校验/授权/预算/持久化失败）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// proposal 校验失败（schema/units/ranges/idempotency）。
    ProposalInvalid(String),
    /// telemetry 在执行时刻已过期（控制时域外）。
    TelemetryExpired,
    /// 授权失败（chain deny：gateway/lease/fencing/policy）。
    NotAuthorized(String),
    /// 安全校验拒绝（chain deny：unsafe）。
    SafetyRejected(String),
    /// 反馈不可信（chain deny：FeedbackUntrusted）。
    FeedbackUntrusted(String),
    /// 预算 reserve 失败（速率/能量）。
    BudgetExceeded(String),
    /// durable intent 写入失败。
    IntentPersistFailed(String),
    /// 幂等键相同但命令内容不同（61 v0.314.24：journal digest 冲突）——
    /// 拒绝且零投递，不得覆盖原记录。
    IdempotencyConflict(String),
    /// 投递通道不可达。
    DeliveryUnreachable,
    /// 投递超时（写可能已发生——调用方按 Indeterminate 处理）。
    DeliveryTimeout,
    /// 响应畸形。
    BadResponse(String),
    /// 内部错误（不应发生）。
    Internal(String),
}

/// 唯一控制写出口 trait（WP-40.6 深接口）。
pub trait ControlDispatch {
    /// 提交提议计划。
    ///
    /// `deadline_ms` = 端到端截止（相对 now 的毫秒预算；超时 → 明确错误，
    /// 不猜成功）。任何前置失败 → `Err`（零投递）；投递后结果 →
    /// `Ok(receipt)`（`final_state` 决定成功/失败/不确定）。
    fn submit(
        &mut self,
        proposal: &ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError>;

    /// 47（WP-47.1 协议入站写）：提交协议写意图（唯一协议→授权链入口）。
    ///
    /// 与 [`submit`](Self::submit) 同一 12 态链：意图校验（设备/点/值域）→
    /// 序号重放门 → actor/租约/策略/安全授权（ControlChain 九步链，含
    /// SBO）→ rate 预算 → durable intent → 投递 → ACK/独立 readback →
    /// Committed/Failed/Indeterminate。任何前置失败 → `Err`（零投递）。
    fn submit_intent(
        &mut self,
        intent: &eneros_protocol_abstract::write::PointWriteIntent,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError>;

    /// 蓝图 61 v0.314.29：typed 提交（CandidatePlan V1——决策 wire 唯一
    /// 形态）。内部经过渡认证证书 + CommandIntent/CommandBatch 进入同一
    /// 12 步链；执行仍只经 Gateway AuthorizedCommand。
    fn submit_typed(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        target_device: &str,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_ordering_and_terminal() {
        assert!(DispatchState::Proposed < DispatchState::Committed);
        assert!(DispatchState::Committed.is_committed());
        assert!(!DispatchState::Indeterminate.is_committed());
        assert!(!DispatchState::Failed.is_committed());
        assert!(!DispatchState::Rejected.is_committed());
        assert_eq!(DispatchState::Committed.as_str(), "committed");
    }

    #[test]
    fn actor_constructs() {
        let a = AuthenticatedActor {
            kind: ActorKind::Ai,
            service: [1u8; 16],
            session_epoch: 0,
        };
        assert_eq!(a.kind, ActorKind::Ai);
    }
}
