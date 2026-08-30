//! `IntentCandidate` / `ObjectiveSpec` / `DecisionRequest`（[02] §7 表最小
//! 合同形状；.36 objective-engine / .42 model-harness 落生产者——本卷
//! 建立类型与校验面）。

use alloc::vec::Vec;

use crate::error::ContractError;
use crate::ids::{ReasonCode, SnapshotId};
use crate::wire::MAX_CONSTRAINTS;

/// 约束种类（封闭枚举；.36 RuleBundle 编译源）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstraintKind {
    MinSoc,
    MaxSoc,
    MaxPowerKw,
    MinPowerKw,
    RampUpKwS,
    RampDownKwS,
}

/// 带界约束（f64 必须有限）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub bound: f64,
}

/// 目标种类（封闭）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveKind {
    MaxRevenue,
    MinCost,
    PeakShave,
    SelfConsume,
}

/// 目标来源（[02] §7：LLM 不能单独签发 ObjectiveSpec）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveSource {
    RuleBundle,
    Contract,
    OperatorManual,
}

/// 目标规格（ObjectiveModule 唯一生产者——本卷仅类型与校验）。
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveSpec {
    snapshot_id: SnapshotId,
    hard: Vec<Constraint>,
    soft: Vec<Constraint>,
    objective: ObjectiveKind,
    priority: u8,
    source: ObjectiveSource,
    reason: ReasonCode,
    valid_from_ms: u64,
    valid_until_ms: u64,
}

impl ObjectiveSpec {
    /// 校验构造（硬约束数、时间窗、有限值）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snapshot_id: SnapshotId,
        hard: Vec<Constraint>,
        soft: Vec<Constraint>,
        objective: ObjectiveKind,
        priority: u8,
        source: ObjectiveSource,
        reason: ReasonCode,
        valid_from_ms: u64,
        valid_until_ms: u64,
    ) -> Result<Self, ContractError> {
        if snapshot_id.is_unbound() {
            return Err(ContractError::MissingField("snapshot_id"));
        }
        if hard.is_empty() {
            return Err(ContractError::MissingField("hard"));
        }
        if hard.len() > MAX_CONSTRAINTS || soft.len() > MAX_CONSTRAINTS {
            return Err(ContractError::LimitExceeded("constraints"));
        }
        for c in hard.iter().chain(soft.iter()) {
            if !c.bound.is_finite() {
                return Err(ContractError::OutOfRange("bound"));
            }
        }
        if valid_until_ms <= valid_from_ms {
            return Err(ContractError::InvalidWindow);
        }
        Ok(Self {
            snapshot_id,
            hard,
            soft,
            objective,
            priority,
            source,
            reason,
            valid_from_ms,
            valid_until_ms,
        })
    }

    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }
    pub fn hard(&self) -> &[Constraint] {
        &self.hard
    }
    pub fn soft(&self) -> &[Constraint] {
        &self.soft
    }
    pub fn objective(&self) -> ObjectiveKind {
        self.objective
    }
    pub fn priority(&self) -> u8 {
        self.priority
    }
    pub fn source(&self) -> ObjectiveSource {
        self.source
    }
    pub fn reason(&self) -> ReasonCode {
        self.reason
    }
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms >= self.valid_from_ms && now_ms <= self.valid_until_ms
    }
}

/// 意图候选（ModelHarness/人工适配器唯一输出形状；[02] §7：不能含协议
/// 地址或授权命令——类型上只有摘要与置信度，无设备面字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct IntentCandidate {
    scope_snapshot: SnapshotId,
    objective_hint: ObjectiveKind,
    reason: ReasonCode,
    /// 置信度（基点 0..=10000 = 0..100%）。
    confidence_bp: u16,
    model_digest: [u8; 32],
    prompt_digest: [u8; 32],
    valid_until_ms: u64,
}

impl IntentCandidate {
    pub fn new(
        scope_snapshot: SnapshotId,
        objective_hint: ObjectiveKind,
        reason: ReasonCode,
        confidence_bp: u16,
        model_digest: [u8; 32],
        prompt_digest: [u8; 32],
        valid_until_ms: u64,
    ) -> Result<Self, ContractError> {
        if confidence_bp > 10_000 {
            return Err(ContractError::OutOfRange("confidence_bp"));
        }
        if model_digest == [0u8; 32] {
            return Err(ContractError::MissingField("model_digest"));
        }
        if valid_until_ms == 0 {
            return Err(ContractError::OutOfRange("valid_until_ms"));
        }
        Ok(Self {
            scope_snapshot,
            objective_hint,
            reason,
            confidence_bp,
            model_digest,
            prompt_digest,
            valid_until_ms,
        })
    }

    pub fn scope_snapshot(&self) -> SnapshotId {
        self.scope_snapshot
    }
    pub fn objective_hint(&self) -> ObjectiveKind {
        self.objective_hint
    }
    pub fn reason(&self) -> ReasonCode {
        self.reason
    }
    pub fn confidence_bp(&self) -> u16 {
        self.confidence_bp
    }
    pub fn model_digest(&self) -> &[u8; 32] {
        &self.model_digest
    }
    pub fn prompt_digest(&self) -> &[u8; 32] {
        &self.prompt_digest
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }
}

/// 决策请求（DecisionOrchestrator 唯一生产者；不携带未验证原始文本——
/// 类型上只有规格与预算）。
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRequest {
    request_id: [u8; 16],
    objective: ObjectiveSpec,
    resource_catalog_version: u64,
    budget_ms: u32,
    policy_version: u64,
}

impl DecisionRequest {
    pub fn new(
        request_id: [u8; 16],
        objective: ObjectiveSpec,
        resource_catalog_version: u64,
        budget_ms: u32,
        policy_version: u64,
    ) -> Result<Self, ContractError> {
        if request_id == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        if budget_ms == 0 {
            return Err(ContractError::OutOfRange("budget_ms"));
        }
        if resource_catalog_version == 0 {
            return Err(ContractError::MissingField("resource_catalog_version"));
        }
        Ok(Self {
            request_id,
            objective,
            resource_catalog_version,
            budget_ms,
            policy_version,
        })
    }

    pub fn request_id(&self) -> &[u8; 16] {
        &self.request_id
    }
    pub fn objective(&self) -> &ObjectiveSpec {
        &self.objective
    }
    pub fn resource_catalog_version(&self) -> u64 {
        self.resource_catalog_version
    }
    pub fn budget_ms(&self) -> u32 {
        self.budget_ms
    }
    pub fn policy_version(&self) -> u64 {
        self.policy_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_candidate_negatives() {
        assert_eq!(
            IntentCandidate::new(
                SnapshotId([1u8; 16]),
                ObjectiveKind::PeakShave,
                ReasonCode::PeakShave,
                10_001,
                [1; 32],
                [1; 32],
                1,
            ),
            Err(ContractError::OutOfRange("confidence_bp"))
        );
        assert_eq!(
            IntentCandidate::new(
                SnapshotId([1u8; 16]),
                ObjectiveKind::PeakShave,
                ReasonCode::PeakShave,
                5_000,
                [0; 32],
                [1; 32],
                1,
            ),
            Err(ContractError::MissingField("model_digest"))
        );
    }

    #[test]
    fn objective_spec_negatives() {
        assert_eq!(
            ObjectiveSpec::new(
                SnapshotId([0u8; 16]),
                vec![Constraint {
                    kind: ConstraintKind::MinSoc,
                    bound: 0.1
                }],
                vec![],
                ObjectiveKind::PeakShave,
                1,
                ObjectiveSource::RuleBundle,
                ReasonCode::PeakShave,
                0,
                1,
            ),
            Err(ContractError::MissingField("snapshot_id"))
        );
        assert_eq!(
            ObjectiveSpec::new(
                SnapshotId([1u8; 16]),
                vec![],
                vec![],
                ObjectiveKind::PeakShave,
                1,
                ObjectiveSource::RuleBundle,
                ReasonCode::PeakShave,
                0,
                1,
            ),
            Err(ContractError::MissingField("hard"))
        );
    }

    #[test]
    fn decision_request_negatives() {
        let spec = ObjectiveSpec::new(
            SnapshotId([1u8; 16]),
            vec![Constraint {
                kind: ConstraintKind::MaxSoc,
                bound: 0.9,
            }],
            vec![],
            ObjectiveKind::SelfConsume,
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::SocBalance,
            0,
            1,
        )
        .unwrap();
        assert_eq!(
            DecisionRequest::new([0u8; 16], spec.clone(), 1, 100, 1),
            Err(ContractError::ZeroId)
        );
        assert_eq!(
            DecisionRequest::new([1u8; 16], spec, 0, 100, 1),
            Err(ContractError::MissingField("resource_catalog_version"))
        );
    }
}
