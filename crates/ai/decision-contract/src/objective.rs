//! `IntentCandidate` / `ObjectiveSpec` / `DecisionRequest`（[02] §7 表最小
//! 合同形状；.36 objective-engine / .42 model-harness 落生产者——本卷
//! 建立类型与校验面）。

use alloc::vec::Vec;

use crate::error::ContractError;
use crate::ids::{ReasonCode, SnapshotId};
use crate::wire::MAX_CONSTRAINTS;

/// 约束种类（封闭枚举；.36 RuleBundle 编译源）。
///
/// 码位冻结（[06] §3.6）：`code()` 只能追加，禁止改义；`from_code`
/// 对未登记码 fail-closed（None）。1-6 为 Phase 1 原始六类；7-11 为
/// .36/.37 多资源规划扩充（承诺/终值/能量窗）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstraintKind {
    MinSoc,
    MaxSoc,
    MaxPowerKw,
    MinPowerKw,
    RampUpKwS,
    RampDownKwS,
    /// 终值 SOC（horizon 末端；0.0~1.0 比率）。
    SocFinal,
    /// 能量窗上限（kWh）。
    MaxEnergyKwh,
    /// 能量窗下限（kWh）。
    MinEnergyKwh,
    /// 市场时段能量承诺（kWh；硬约束）。
    EnergyCommitmentKwh,
    /// 备用容量（kW；硬约束）。
    ReserveCapacityKw,
}

impl ConstraintKind {
    /// 稳定数字码（wire/目录层使用；0 保留）。
    pub const fn code(self) -> u16 {
        match self {
            Self::MinSoc => 1,
            Self::MaxSoc => 2,
            Self::MaxPowerKw => 3,
            Self::MinPowerKw => 4,
            Self::RampUpKwS => 5,
            Self::RampDownKwS => 6,
            Self::SocFinal => 7,
            Self::MaxEnergyKwh => 8,
            Self::MinEnergyKwh => 9,
            Self::EnergyCommitmentKwh => 10,
            Self::ReserveCapacityKw => 11,
        }
    }

    /// 数字码 → 枚举；0 与未登记码 → None（fail-closed）。
    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(Self::MinSoc),
            2 => Some(Self::MaxSoc),
            3 => Some(Self::MaxPowerKw),
            4 => Some(Self::MinPowerKw),
            5 => Some(Self::RampUpKwS),
            6 => Some(Self::RampDownKwS),
            7 => Some(Self::SocFinal),
            8 => Some(Self::MaxEnergyKwh),
            9 => Some(Self::MinEnergyKwh),
            10 => Some(Self::EnergyCommitmentKwh),
            11 => Some(Self::ReserveCapacityKw),
            _ => None,
        }
    }

    /// 该种类的主量纲（文档/测试基准）。
    pub const fn required_dimension(self) -> eneros_telemetry_model::Dimension {
        use eneros_telemetry_model::Dimension;
        match self {
            Self::MinSoc | Self::MaxSoc | Self::SocFinal => Dimension::Ratio,
            Self::MaxPowerKw | Self::MinPowerKw | Self::ReserveCapacityKw => Dimension::Power,
            Self::RampUpKwS | Self::RampDownKwS => Dimension::Power,
            Self::MaxEnergyKwh | Self::MinEnergyKwh | Self::EnergyCommitmentKwh => {
                Dimension::Energy
            }
        }
    }

    /// 单位-kind 匹配裁决（[09] §2.3）：SOC 比率类同时接受 Ratio
    /// （%）与 None（0..1 无量纲分数）；其余按主量纲。
    pub fn accepts_dimension(self, d: eneros_telemetry_model::Dimension) -> bool {
        use eneros_telemetry_model::Dimension;
        match self {
            Self::MinSoc | Self::MaxSoc | Self::SocFinal => {
                matches!(d, Dimension::Ratio | Dimension::None)
            }
            other => d == other.required_dimension(),
        }
    }
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

/// ObjectiveSpec 语义组构造器（WP-64.3.1：constraints/objective/window
/// 三组；全部字段必填）。
pub struct ObjectiveSpecBuilder {
    snapshot_id: SnapshotId,
    constraints: Option<(Vec<Constraint>, Vec<Constraint>)>,
    objective: Option<(ObjectiveKind, u8, ObjectiveSource, ReasonCode)>,
    window: Option<(u64, u64)>,
}

impl ObjectiveSpecBuilder {
    /// 约束组（hard 非空；hard/soft 各 ≤[`MAX_CONSTRAINTS`]）。
    pub fn constraints(mut self, hard: Vec<Constraint>, soft: Vec<Constraint>) -> Self {
        self.constraints = Some((hard, soft));
        self
    }

    /// 目标组（kind/priority/source/reason）。
    pub fn objective(
        mut self,
        kind: ObjectiveKind,
        priority: u8,
        source: ObjectiveSource,
        reason: ReasonCode,
    ) -> Self {
        self.objective = Some((kind, priority, source, reason));
        self
    }

    /// 有效窗组。
    pub fn window(mut self, valid_from_ms: u64, valid_until_ms: u64) -> Self {
        self.window = Some((valid_from_ms, valid_until_ms));
        self
    }

    /// 组装并校验（硬约束数、时间窗、有限值）。
    pub fn build(self) -> Result<ObjectiveSpec, ContractError> {
        let (hard, soft) = self
            .constraints
            .ok_or(ContractError::MissingField("hard"))?;
        let (objective, priority, source, reason) = self
            .objective
            .ok_or(ContractError::MissingField("objective"))?;
        let (valid_from_ms, valid_until_ms) = self.window.ok_or(ContractError::InvalidWindow)?;
        if self.snapshot_id.is_unbound() {
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
        Ok(ObjectiveSpec {
            snapshot_id: self.snapshot_id,
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
}

impl ObjectiveSpec {
    /// 构造器入口（WP-64.3.1：三语义组 builder）。
    pub fn builder(snapshot_id: SnapshotId) -> ObjectiveSpecBuilder {
        ObjectiveSpecBuilder {
            snapshot_id,
            constraints: None,
            objective: None,
            window: None,
        }
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
    pub fn valid_from_ms(&self) -> u64 {
        self.valid_from_ms
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
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

    /// 码位冻结（[06] §3.6 同纪律）：1-6 Phase 1 原六类，7-11 .36 扩充。
    #[test]
    fn constraint_kinds_stable_and_unknown_fail_closed() {
        for (code, expected) in [
            (1u16, ConstraintKind::MinSoc),
            (2, ConstraintKind::MaxSoc),
            (3, ConstraintKind::MaxPowerKw),
            (4, ConstraintKind::MinPowerKw),
            (5, ConstraintKind::RampUpKwS),
            (6, ConstraintKind::RampDownKwS),
            (7, ConstraintKind::SocFinal),
            (8, ConstraintKind::MaxEnergyKwh),
            (9, ConstraintKind::MinEnergyKwh),
            (10, ConstraintKind::EnergyCommitmentKwh),
            (11, ConstraintKind::ReserveCapacityKw),
        ] {
            assert_eq!(ConstraintKind::from_code(code), Some(expected));
            assert_eq!(expected.code(), code);
        }
        assert_eq!(ConstraintKind::from_code(0), None, "0 保留");
        assert_eq!(ConstraintKind::from_code(12), None, "未登记码 fail-closed");
        assert_eq!(ConstraintKind::from_code(u16::MAX), None);
        // 量纲匹配面：SOC 系比率、功率系 Power、能量系 Energy。
        assert_eq!(
            ConstraintKind::MinSoc.required_dimension(),
            eneros_telemetry_model::Dimension::Ratio
        );
        assert_eq!(
            ConstraintKind::MaxPowerKw.required_dimension(),
            eneros_telemetry_model::Dimension::Power
        );
        assert_eq!(
            ConstraintKind::EnergyCommitmentKwh.required_dimension(),
            eneros_telemetry_model::Dimension::Energy
        );
    }

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
            ObjectiveSpec::builder(SnapshotId([0u8; 16]))
                .constraints(
                    vec![Constraint {
                        kind: ConstraintKind::MinSoc,
                        bound: 0.1
                    }],
                    vec![],
                )
                .objective(
                    ObjectiveKind::PeakShave,
                    1,
                    ObjectiveSource::RuleBundle,
                    ReasonCode::PeakShave
                )
                .window(0, 1)
                .build(),
            Err(ContractError::MissingField("snapshot_id"))
        );
        assert_eq!(
            ObjectiveSpec::builder(SnapshotId([1u8; 16]))
                .constraints(vec![], vec![])
                .objective(
                    ObjectiveKind::PeakShave,
                    1,
                    ObjectiveSource::RuleBundle,
                    ReasonCode::PeakShave
                )
                .window(0, 1)
                .build(),
            Err(ContractError::MissingField("hard"))
        );
    }

    #[test]
    fn decision_request_negatives() {
        let spec = ObjectiveSpec::builder(SnapshotId([1u8; 16]))
            .constraints(
                vec![Constraint {
                    kind: ConstraintKind::MaxSoc,
                    bound: 0.9,
                }],
                vec![],
            )
            .objective(
                ObjectiveKind::SelfConsume,
                1,
                ObjectiveSource::RuleBundle,
                ReasonCode::SocBalance,
            )
            .window(0, 1)
            .build()
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
