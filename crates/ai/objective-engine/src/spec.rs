//! 规则表示（[09] §2.3）：ConstraintSpec / ObjectiveTerm / ScopeRef /
//! HorizonPlan / FactSet。全部字段可校验构造；规则在 host 编译，target
//! 不执行任意脚本。

use alloc::vec::Vec;

use crate::compiler::CompileError;

/// 作用域引用（[09] §2.3 ScopeRef）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopeRef {
    /// 资产 ID；0 = 站级（全站资源）。
    pub asset_id: u64,
    /// 拓扑节点 ID；0 = 不限节点。
    pub topology_node_id: u64,
}

impl ScopeRef {
    /// 站级作用域。
    pub const SITE: ScopeRef = ScopeRef {
        asset_id: 0,
        topology_node_id: 0,
    };
}

/// 规划地平线（由 RuleBundle 携带——地平线本身是规则输入的一部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HorizonPlan {
    /// 步长（纳秒；≥1s）。
    pub step_ns: u64,
    /// 步数（1..=96*4）。
    pub step_count: u32,
}

/// 步长上限（24h——域上界；与 step_count ≤ 384 联合保证 span_ns 乘法
/// 在 u64 内不溢出：8.64e13 × 384 ≈ 3.3e16 ≪ u64::MAX）。
const MAX_STEP_NS: u64 = 86_400_000_000_000;

impl HorizonPlan {
    pub fn new(step_ns: u64, step_count: u32) -> Result<Self, CompileError> {
        if step_ns < 1_000_000_000 {
            return Err(CompileError::InvalidRule("step_ns < 1s"));
        }
        if step_ns > MAX_STEP_NS {
            // 饱和 cast 链（如迁移端 period_hours=Inf → u64::MAX）在此
            // 拒绝，不进 span_ns 乘法。
            return Err(CompileError::InvalidRule("step_ns > 24h"));
        }
        if step_count == 0 || step_count > 384 {
            return Err(CompileError::InvalidRule("step_count"));
        }
        Ok(Self {
            step_ns,
            step_count,
        })
    }

    /// 地平线总时长（ns；构造界保证乘法不溢出）。
    pub fn span_ns(&self) -> u64 {
        self.step_ns * self.step_count as u64
    }
}

/// 单条约束规则（[09] §2.3 ConstraintSpec）。
///
/// 绑定界语义（按 kind）：`Max*` 取 `upper`；`Min*` 取 `lower`；
/// `EnergyCommitmentKwh`/`SocFinal` 要求 `lower == upper`（精确值）；
/// `ReserveCapacityKw` 取 `lower`（必须保留的容量）；`Ramp*` 取 `upper`
/// （跨规则取最严最小值）。
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintSpec {
    pub constraint_id: u64,
    /// [`eneros_decision_contract::objective::ConstraintKind::code()`] 码。
    pub kind: u16,
    /// 0 = 硬约束；1 = 软约束。
    pub hardness: u8,
    /// 优先级层 1..=7（crate::layer）。
    pub priority: u16,
    pub scope: ScopeRef,
    pub unit_code: u16,
    pub lower: f64,
    pub upper: f64,
    pub active_from_ns: u64,
    pub active_until_ns: u64,
    /// 来源事件（[09] §2.1：一切约束有来源）。
    pub source_event_id: [u8; 16],
    /// 规则内容摘要（构建时重算校验；防篡改锚）。
    pub rule_digest: [u8; 32],
}

impl ConstraintSpec {
    /// 校验构造（[09] §2.3：值有限、lower≤upper、单位与 kind 匹配、
    /// 有效期非倒、来源非零）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        constraint_id: u64,
        kind: u16,
        hardness: u8,
        priority: u16,
        scope: ScopeRef,
        unit_code: u16,
        lower: f64,
        upper: f64,
        active_from_ns: u64,
        active_until_ns: u64,
        source_event_id: [u8; 16],
    ) -> Result<Self, CompileError> {
        if constraint_id == 0 {
            return Err(CompileError::InvalidRule("constraint_id=0"));
        }
        if eneros_decision_contract::objective::ConstraintKind::from_code(kind).is_none() {
            return Err(CompileError::UnknownKind(kind));
        }
        if hardness > 1 {
            return Err(CompileError::InvalidRule("hardness∉{0,1}"));
        }
        if !(1..=crate::layer::UNAPPROVED_ADVICE).contains(&priority) {
            return Err(CompileError::InvalidRule("priority∉1..=7"));
        }
        // 单位-kind 匹配（[09] §2.3）：量纲必须一致；0 = 未知单位拒绝。
        let unit = eneros_telemetry_model::unit_catalog::from_code(unit_code)
            .ok_or(CompileError::UnknownUnit(unit_code))?;
        let kind_enum = eneros_decision_contract::objective::ConstraintKind::from_code(kind)
            .ok_or(CompileError::UnknownKind(kind))?;
        if !kind_enum.accepts_dimension(unit.dimension()) {
            return Err(CompileError::UnitKindMismatch { kind, unit_code });
        }
        if !lower.is_finite() || !upper.is_finite() {
            return Err(CompileError::NonFinite("bound"));
        }
        if lower > upper {
            return Err(CompileError::InvalidRule("lower>upper"));
        }
        // 精确值种类必须退化为点。
        if matches!(
            kind_enum,
            eneros_decision_contract::objective::ConstraintKind::EnergyCommitmentKwh
                | eneros_decision_contract::objective::ConstraintKind::SocFinal
        ) && lower != upper
        {
            return Err(CompileError::InvalidRule("exact-kind lower!=upper"));
        }
        if active_until_ns <= active_from_ns {
            return Err(CompileError::InvalidRule("active window"));
        }
        if source_event_id == [0u8; 16] {
            return Err(CompileError::InvalidRule("source_event_id=0"));
        }
        let mut spec = Self {
            constraint_id,
            kind,
            hardness,
            priority,
            scope,
            unit_code,
            lower,
            upper,
            active_from_ns,
            active_until_ns,
            source_event_id,
            rule_digest: [0u8; 32],
        };
        spec.rule_digest = spec.compute_digest();
        Ok(spec)
    }

    /// 规则内容摘要（全部字段参与；防篡改锚）。
    pub fn compute_digest(&self) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.rule.v1");
        h.update(self.constraint_id.to_le_bytes());
        h.update(self.kind.to_le_bytes());
        h.update([self.hardness]);
        h.update(self.priority.to_le_bytes());
        h.update(self.scope.asset_id.to_le_bytes());
        h.update(self.scope.topology_node_id.to_le_bytes());
        h.update(self.unit_code.to_le_bytes());
        h.update(self.lower.to_le_bytes());
        h.update(self.upper.to_le_bytes());
        h.update(self.active_from_ns.to_le_bytes());
        h.update(self.active_until_ns.to_le_bytes());
        h.update(self.source_event_id);
        h.finalize().into()
    }

    /// 绑定界（按 kind 语义提取）。
    pub fn binding(&self) -> f64 {
        use eneros_decision_contract::objective::ConstraintKind;
        match ConstraintKind::from_code(self.kind) {
            Some(ConstraintKind::MinSoc)
            | Some(ConstraintKind::MinPowerKw)
            | Some(ConstraintKind::MinEnergyKwh)
            | Some(ConstraintKind::ReserveCapacityKw) => self.lower,
            Some(ConstraintKind::EnergyCommitmentKwh) | Some(ConstraintKind::SocFinal) => {
                self.lower // 构造已保证 lower==upper
            }
            _ => self.upper,
        }
    }

    /// 规则同类分组键（同层冲突检测的"同类"：约束的量类）。
    pub fn group_class(&self) -> u16 {
        use eneros_decision_contract::objective::ConstraintKind;
        match ConstraintKind::from_code(self.kind) {
            Some(ConstraintKind::MinSoc) | Some(ConstraintKind::MaxSoc) => 1, // SOC 范围
            Some(ConstraintKind::SocFinal) => 2,                              // 终值 SOC
            Some(ConstraintKind::MinPowerKw) | Some(ConstraintKind::MaxPowerKw) => 3, // 功率范围
            Some(ConstraintKind::MinEnergyKwh) | Some(ConstraintKind::MaxEnergyKwh) => 4, // 能量范围
            Some(ConstraintKind::EnergyCommitmentKwh) => 5, // 承诺能量
            Some(ConstraintKind::RampUpKwS) => 6,           // 上爬坡界
            Some(ConstraintKind::RampDownKwS) => 7,         // 下爬坡界
            Some(ConstraintKind::ReserveCapacityKw) => 8,   // 备用容量
            None => u16::MAX,
        }
    }
}

/// 目标项（[09] §2.3 ObjectiveTerm）。
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveTerm {
    pub term_id: u64,
    /// crate::term_kind 码。
    pub kind: u16,
    /// 权重（≥0，有限）。
    pub weight: f64,
    pub unit_code: u16,
    /// crate::normalization id。
    pub normalization_id: u32,
    /// 优先级层 1..=7。
    pub priority: u16,
    pub source_event_id: [u8; 16],
}

impl ObjectiveTerm {
    pub fn new(
        term_id: u64,
        kind: u16,
        weight: f64,
        unit_code: u16,
        normalization_id: u32,
        priority: u16,
        source_event_id: [u8; 16],
    ) -> Result<Self, CompileError> {
        if term_id == 0 {
            return Err(CompileError::InvalidRule("term_id=0"));
        }
        if !crate::term_kind::is_known(kind) {
            return Err(CompileError::UnknownKind(kind));
        }
        if !weight.is_finite() || weight < 0.0 {
            return Err(CompileError::NonFinite("weight"));
        }
        if eneros_telemetry_model::unit_catalog::from_code(unit_code).is_none() {
            return Err(CompileError::UnknownUnit(unit_code));
        }
        if !crate::normalization::is_known(normalization_id) {
            return Err(CompileError::UnknownNormalization(normalization_id));
        }
        if !(1..=crate::layer::UNAPPROVED_ADVICE).contains(&priority) {
            return Err(CompileError::InvalidRule("priority∉1..=7"));
        }
        if source_event_id == [0u8; 16] {
            return Err(CompileError::InvalidRule("source_event_id=0"));
        }
        Ok(Self {
            term_id,
            kind,
            weight,
            unit_code,
            normalization_id,
            priority,
            source_event_id,
        })
    }
}

/// 已结构化市场事实（[09] §2.2 输入；类型面无自由文本）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MarketFacts {
    /// 每步电价（元/kWh；长度 0 = 无价格 → 经济项不可用）。
    pub prices_per_kwh: Vec<f64>,
    /// 事实来源事件（非零）。
    pub source_event_id: [u8; 16],
}

/// 已结构化工艺事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessFacts {
    /// 最小运行步数（0 = 不约束）。
    pub min_on_steps: u32,
    /// 最小停机步数（0 = 不约束）。
    pub min_off_steps: u32,
}

/// 已结构化企业事实（维护窗等）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnterpriseFacts {
    /// 维护窗口（ns 区间；与地平线相交 → 编译拒绝）。
    pub maintenance_windows_ns: Vec<(u64, u64)>,
    /// 人工越权收紧范围（比率 0..1；None = 无）。
    pub operator_override_soc: Option<(f64, f64)>,
}

/// 决策事实集（[09] §2.2：已结构化的 market/process/enterprise facts）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FactSet {
    /// 事实切面时刻（过期校验基准）。
    pub as_of_ns: u64,
    pub market: MarketFacts,
    pub process: ProcessFacts,
    pub enterprise: EnterpriseFacts,
}
