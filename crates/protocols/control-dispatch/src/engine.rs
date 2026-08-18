//! 蓝图 40 WP-40.6/40.7/40.12：`DispatchEngine`——唯一 ControlDispatch 实现。
//!
//! 12 步链（WP-40.6 顺序）：
//!
//! 1. proposal schema/units/ranges/idempotency 校验；
//! 2. telemetry 执行时刻新鲜度（`valid_until_ms`）；
//! 3. actor/capability/lease/fencing 验证（`ControlChain` gateway +
//!    lease + policy——唯一 AuthorizedCommand 授权路径，WP-40.7）；
//! 4. safety catalog/constraint validator（chain 的 CTL-010 反馈调和 +
//!    CTL-008 独立安全校验，反馈来自受信任遥测）；
//! 5. rate/energy 预算 reserve（[`DecisionGate`]——**不 record**）；
//! 6. durable intent 写入（[`IntentStore`]）；
//! 7. DeliveryChannel EIPC 投递（`ControlEnvelope`，WP-40.8）；
//! 8. device-agent AuthorizedWriter 二次验证（对端职责）；
//! 9. 协议/设备 transport ACK（对端回执帧）；
//! 10. 独立 readback（对端；读回不符 → `Failed`，不可得 → `Indeterminate`）；
//! 11. commit 预算（`gate.record`——**仅 confirmed dispatch**）+ 审计；
//! 12. 回执反馈给 Agent（learning 只消费 `Committed`）。
//!
//! 任何前置失败零投递（`Err`）；投递后超时/结果不明 → `Indeterminate`
//! 对账，不盲重试非幂等命令（WP-40.12）。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_gateway::chain::{ChainOutcome, ControlChain, FeedbackBundle};
use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
use eneros_control_gateway::wire::{ControlEnvelope, OutcomeCode, OutcomeFrame};
use eneros_control_gateway::ChainDenyReason;
use eneros_dual_brain::audit::{DecisionAudit, ExecOutcome, SafetyOutcome};
use eneros_dual_brain::chain_sink::deny_reason_str;
use eneros_dual_brain::gate::DecisionGate;
use eneros_dual_brain::ProposedControlPlan;
use eneros_safety_validator::TrustedTelemetry;
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};

use crate::channel::DeliveryChannel;
use crate::dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchError, DispatchReceipt, DispatchState,
};
use crate::feedback::TrustedFeedbackBuilder;
use crate::intent::{IntentRecord, IntentStore, IntentStoreError};

/// 调度窗口（小时）——能量预算估算（与 dual-brain 同系数）。
pub const SCHEDULE_WINDOW_HOURS: f64 = eneros_dual_brain::coordinator::SCHEDULE_WINDOW_HOURS;

/// 单次调度能量估算（首时段功率 × 窗口；空调度 = 0）。
pub fn schedule_energy_kwh(s: &eneros_energy_lp_model::result::ScheduleResult) -> f64 {
    s.schedule
        .first()
        .map(|e| e.net_power_kw.abs() * SCHEDULE_WINDOW_HOURS)
        .unwrap_or(0.0)
}

/// 装配配置。
pub struct DispatchConfig {
    /// 授权链（gateway/lease/idempotency/policy/safety/SBO——default-deny）。
    pub chain: ControlChain,
    /// 信封签名密钥（与 chain gateway 同源）。
    pub key: Vec<u8>,
    /// AI 租约 ID（装配期由 chain.lease_service 签发）。
    pub lease_id: [u8; 16],
    /// 当前 fencing token。
    pub fencing_token: u64,
    /// 签发服务标识。
    pub issuer_service: [u8; 16],
    /// 签发服务 epoch（重启计数）。
    pub issuer_epoch: u64,
    /// 策略代次。
    pub policy_version: u64,
    /// 目标设备 ID（信封 device_id；registry 映射见 WP-40.10，本卷装配配置）。
    pub device_id: u32,
    /// 目标点 ID（信封 point_id）。
    pub point_id: u32,
    /// 投递目标端点（EIPC 服务名）。
    pub target_endpoint: String,
    /// 功率上限（kW）——proposal 校验。
    pub max_power_kw: f64,
    /// 速率/能量预算闸（check 在 submit；record 仅在 confirmed dispatch）。
    pub gate: DecisionGate,
    /// 决策审计（可选）。
    pub audit: Option<DecisionAudit>,
    /// durable intent 存储（WP-40.12 对账）。
    pub intent_log: Box<dyn IntentStore>,
    /// 投递通道（生产：Sel4EipcChannel；测试：内存通道）。
    pub delivery: Box<dyn DeliveryChannel>,
    /// 反馈点映射（受信任遥测 → FeedbackBundle）。
    pub feedback_mapping: crate::feedback::FeedbackPointMapping,
    /// 单调时钟（毫秒）。
    pub now_fn: fn() -> u64,
}

/// 唯一 ControlDispatch 实现（12 步链）。
pub struct DispatchEngine {
    chain: ControlChain,
    key: Vec<u8>,
    lease_id: [u8; 16],
    fencing_token: u64,
    issuer_service: [u8; 16],
    issuer_epoch: u64,
    policy_version: u64,
    device_id: u32,
    point_id: u32,
    target_endpoint: String,
    max_power_kw: f64,
    /// 速率/能量预算闸（观测：used_kwh/rejected 计数）。
    pub gate: DecisionGate,
    /// 决策审计（观测：链校验/条目）。
    pub audit: Option<DecisionAudit>,
    /// durable intent 存储（观测：find/pending 对账）。
    pub intent_log: Box<dyn IntentStore>,
    delivery: Box<dyn DeliveryChannel>,
    feedback_builder: TrustedFeedbackBuilder,
    now_fn: fn() -> u64,
    seq: u64,
    /// 已提交（Committed）次数（观测/审计）。
    pub committed_count: u64,
    /// 审计丢弃计数。
    pub audit_drop_count: u64,
}

impl DispatchEngine {
    /// 构造。
    pub fn new(config: DispatchConfig) -> Self {
        Self {
            chain: config.chain,
            key: config.key,
            lease_id: config.lease_id,
            fencing_token: config.fencing_token,
            issuer_service: config.issuer_service,
            issuer_epoch: config.issuer_epoch,
            policy_version: config.policy_version,
            device_id: config.device_id,
            point_id: config.point_id,
            target_endpoint: config.target_endpoint,
            max_power_kw: config.max_power_kw,
            gate: config.gate,
            audit: config.audit,
            intent_log: config.intent_log,
            delivery: config.delivery,
            feedback_builder: TrustedFeedbackBuilder::new(config.feedback_mapping),
            now_fn: config.now_fn,
            seq: 0,
            committed_count: 0,
            audit_drop_count: 0,
        }
    }

    /// 更新 fencing token（租约续期后）。
    pub fn update_fencing_token(&mut self, token: u64) {
        self.fencing_token = token;
    }

    fn audit(&mut self, exec: ExecOutcome, safety: SafetyOutcome, detail: &str, now_ms: u64) {
        let Some(audit) = self.audit.as_mut() else {
            return;
        };
        let rec = eneros_dual_brain::audit::DecisionRecord {
            actor: "control-dispatch",
            target: &self.target_endpoint,
            intent: "dispatch",
            lp_solution: detail,
            safety,
            exec,
            deviation: 0.0,
            now_ms,
        };
        if audit.log_decision(&rec).is_err() {
            self.audit_drop_count += 1;
        }
    }

    /// 从 proposal 派生 correlation（plan_id 前 16 字节，定长）。
    fn correlation_of(plan_id: &str) -> [u8; 16] {
        let mut id = [0u8; 16];
        let bytes = plan_id.as_bytes();
        let n = core::cmp::min(16, bytes.len());
        id[..n].copy_from_slice(&bytes[..n]);
        id
    }

    /// 步骤 1：proposal 校验（schema/units/ranges/idempotency）。
    fn validate_proposal(&self, p: &ProposedControlPlan) -> Result<(), DispatchError> {
        if p.power_kw.is_nan() || p.power_kw.is_infinite() {
            return Err(DispatchError::ProposalInvalid("power_kw 非有限".into()));
        }
        if p.power_kw.abs() > self.max_power_kw {
            return Err(DispatchError::ProposalInvalid(format!(
                "power_kw {} 超上限 {}",
                p.power_kw, self.max_power_kw
            )));
        }
        if p.ttl_secs == 0 {
            return Err(DispatchError::ProposalInvalid(
                "ttl_secs 为 0（防零有效期）".into(),
            ));
        }
        if p.schedule.schedule.is_empty() {
            return Err(DispatchError::ProposalInvalid("空调度不可投递".into()));
        }
        Ok(())
    }

    /// 执行完整 12 步链。
    fn run(
        &mut self,
        proposal: &ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        let mut states: Vec<DispatchState> = Vec::with_capacity(12);
        states.push(DispatchState::Proposed);

        // 步骤 1：proposal 校验。
        self.validate_proposal(proposal)?;
        states.push(DispatchState::Validated);

        // 步骤 2：telemetry 执行时刻新鲜度。
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }

        // 步骤 3：actor/capability/lease/fencing（+ 步骤 4：safety catalog）。
        // 只允许 AI 决策链角色（产品闭环唯一决策方）。
        if actor.kind != ActorKind::Ai {
            return Err(DispatchError::NotAuthorized(format!(
                "actor kind {:?} 不是 Ai",
                actor.kind
            )));
        }
        self.seq += 1;
        let now_secs = started / 1000;
        let mut command_id = [0u8; 16];
        command_id[..8].copy_from_slice(&self.seq.to_le_bytes());
        let correlation = Self::correlation_of(&proposal.plan_id);
        let ttl_secs = proposal.ttl_secs.max(1);
        let single = if proposal.power_kw > 0.0 {
            SingleCommand::On // 放电/合
        } else {
            SingleCommand::Off // 充电/分
        };
        let buf = EnvelopeBuilder::new(
            command_id,
            self.device_id,
            self.point_id,
            ControlCommand::Single(single),
            now_secs,
            now_secs.saturating_add(ttl_secs as u64),
            self.issuer_service,
        )
        .lease(self.lease_id, self.fencing_token)
        .policy_version(self.policy_version)
        .sequence(self.seq)
        .actor_kind(ActorKind::Ai)
        // WP-40.12：幂等键 = correlation（plan_id 派生）——同计划重放
        // 返回原结果，不重复写。
        .idempotency_key(correlation)
        .build(&self.key)
        .encode();
        // SBO：AI 写命令语义 = 选择+执行原子。
        self.chain.sbo_select(self.point_id, now_secs);
        let fb: FeedbackBundle = self.feedback_builder.build(telemetry, now_secs);
        let chain_result = self.chain.execute(&buf, now_secs, &fb);
        match chain_result {
            ChainOutcome::Executed { .. } => {}
            ChainOutcome::IdempotentHit(_) => {
                // 幂等重放：返回原结果，不重复写（WP-40.12 duplicate）。
                states.push(DispatchState::Committed);
                return Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Committed,
                    state_path: states,
                    committed_at_ms: Some(started),
                    note: Some(String::from("idempotent-hit（原结果，不重复写）")),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                });
            }
            ChainOutcome::DryRun { .. } => {
                let note = String::from("dry-run-obligation（策略要求不写设备）");
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                return Err(DispatchError::NotAuthorized(note));
            }
            ChainOutcome::Denied(reason) => {
                let s = deny_reason_str(&reason);
                let (exec, safety) = if matches!(reason, ChainDenyReason::Unsafe(_)) {
                    (ExecOutcome::Failed, SafetyOutcome::Blocked)
                } else {
                    (ExecOutcome::Failed, SafetyOutcome::Passed)
                };
                self.audit(
                    exec,
                    safety,
                    &format!("states={:?};deny={}", states, s),
                    started,
                );
                let err = match reason {
                    ChainDenyReason::Unsafe(d) => DispatchError::SafetyRejected(d),
                    ChainDenyReason::FeedbackUntrusted(d) => DispatchError::FeedbackUntrusted(d),
                    _ => DispatchError::NotAuthorized(s),
                };
                return Err(err);
            }
        }
        states.push(DispatchState::Authorized);

        // 步骤 5：rate/energy 预算 reserve（不 record）。
        let energy = schedule_energy_kwh(&proposal.schedule);
        if self.gate.check_rate(started) != eneros_dual_brain::gate::GateDecision::Allow {
            return Err(DispatchError::BudgetExceeded("rate".into()));
        }
        if self.gate.check_energy(started, energy) != eneros_dual_brain::gate::GateDecision::Allow {
            return Err(DispatchError::BudgetExceeded("energy".into()));
        }
        states.push(DispatchState::Reserved);

        // 步骤 6：durable intent 写入。
        let expires_ms = started.saturating_add(ttl_secs as u64 * 1000);
        let intent = IntentRecord {
            command_id,
            correlation_id: correlation,
            issued_at_ms: started,
            expires_at_ms: expires_ms,
            target_device: proposal.target_device.clone(),
            power_kw: proposal.power_kw,
            input_digest: proposal.input_digest,
            state: DispatchState::Persisted,
        };
        self.intent_log
            .append(&intent)
            .map_err(|e: IntentStoreError| DispatchError::IntentPersistFailed(format!("{e:?}")))?;
        states.push(DispatchState::Persisted);

        // 步骤 7：DeliveryChannel 投递（ControlEnvelope）。
        let envelope = ControlEnvelope::new(
            correlation,
            self.issuer_service,
            self.issuer_epoch,
            started,
            expires_ms,
            buf,
            proposal.power_kw,
        );
        let payload = envelope.encode();
        let timeout = if deadline_ms == 0 {
            u64::MAX
        } else {
            deadline_ms
        };
        let response = match self
            .delivery
            .deliver(&self.target_endpoint, &payload, timeout)
        {
            Ok(r) => r,
            Err(crate::ChannelError::Timeout) => {
                // 投递后超时：写可能已发生——Indeterminate 对账，不猜成功。
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）",
                    started,
                );
                return Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Indeterminate,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(String::from("delivery-timeout：写可能已发生，需对账")),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                });
            }
            Err(crate::ChannelError::Unreachable) => {
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    "delivery-unreachable",
                    started,
                );
                return Err(DispatchError::DeliveryUnreachable);
            }
            Err(e) => {
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("delivery: {e:?}"),
                    started,
                );
                return Err(DispatchError::BadResponse(format!("{e:?}")));
            }
        };
        states.push(DispatchState::Delivered);

        // 步骤 8-10：device-agent 回执（AuthorizedWriter 二次验证 + transport
        // ACK + 独立 readback 已在对端完成；结果码分层见 WP-40.11）。
        let frame = OutcomeFrame::decode(&response)
            .map_err(|e| DispatchError::BadResponse(format!("{e:?}")))?;
        states.push(DispatchState::TransportAcked);
        let outcome = frame
            .outcome()
            .ok_or_else(|| DispatchError::BadResponse("未知结果码".into()))?;
        match outcome {
            OutcomeCode::Applied => {
                // 对端已独立 readback 验证（读回不得回显刚写 payload——
                // device-agent 侧执行点，见 WP-40.11）。
                states.push(DispatchState::ReadbackVerified);
                // 步骤 11：confirmed dispatch 才记账 + 审计。
                self.gate.record(started, energy);
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Committed);
                states.push(DispatchState::Committed);
                self.committed_count += 1;
                self.audit(
                    ExecOutcome::Succeeded,
                    SafetyOutcome::Passed,
                    &format!("states={:?}", states),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Committed,
                    state_path: states,
                    committed_at_ms: Some((self.now_fn)()),
                    note: None,
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::IdempotentHit => {
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Committed);
                states.push(DispatchState::Committed);
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Committed,
                    state_path: states,
                    committed_at_ms: Some((self.now_fn)()),
                    note: Some(String::from("idempotent-hit（对端）")),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::DryRun => {
                let note = String::from("对端 dry-run（未写设备）");
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Rejected,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::Denied => {
                let note = format!("device-denied class={}", frame.class);
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::AppliedReadbackUnavailable => {
                // 写可能已发生但物理状态无法确认 → Indeterminate。
                let note = String::from("readback-unavailable：物理状态未确认，需对账");
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Indeterminate,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::AppliedReadbackMismatch => {
                let note = String::from("readback-mismatch：ACK 成功但物理状态未达目标");
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Blocked,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::Failed => {
                let note = String::from("device-exec-failed");
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
        }
    }
}

impl ControlDispatch for DispatchEngine {
    fn submit(
        &mut self,
        proposal: &ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        self.run(proposal, telemetry, actor, deadline_ms)
    }
}

// 集成测试在 tests/ 目录（需要装配完整 chain + 内存通道 + 对端 ingress）。
#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn schedule_energy_estimate() {
        let s = eneros_energy_lp_model::result::ScheduleResult {
            schedule: alloc::vec![eneros_energy_lp_model::result::ScheduleEntry {
                period: 0,
                charge_power_kw: 0.0,
                discharge_power_kw: 5.0,
                net_power_kw: 5.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            }],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: eneros_solver_core::result::SolveStatus::Optimal,
        };
        assert!((schedule_energy_kwh(&s) - 1.25).abs() < 1e-9);
    }
}
