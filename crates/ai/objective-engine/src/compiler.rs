//! ObjectiveCompiler（[16] §3.7 深模块）：签名 RuleBundle + 快照 + 事实
//! → CompiledObjective（合同 ObjectiveSpec + 求解经济输入 + 编译证据）。
//!
//! 优先级/冲突/归一化/规则过滤全部本模块私有（[09] §2.4/§2.5）：
//! - 层内同类范围求交，空交 → [`ObjectiveConflict`]；任何冲突 → **不产出
//!   ObjectiveSpec**（冲突时零 solver、零控制）。
//! - 跨层合并永远取交集（低层只能收紧，不能放宽高层）。
//! - 第 7 层（未批准建议）规则/目标项一律拒绝。
//! - 必备约束清单缺一即拒（删除安全约束在编译期被发现）。

use alloc::vec;
use alloc::vec::Vec;

use eneros_decision_contract::objective::{Constraint, ConstraintKind, ObjectiveSource};
use eneros_decision_contract::{ObjectiveSpec, ReasonCode, SnapshotId};
use eneros_snapshot_service::TrustedSnapshot;

use crate::bundle::RuleBundle;
/// 固定优先级层（重导出语义名；[09] §2.4）。
pub use crate::layer;
use crate::spec::{ConstraintSpec, FactSet, ScopeRef};

/// 编译配置。
pub struct ObjectiveCompiler {
    /// 事实最大年龄（超出 → StaleFacts）。
    pub max_facts_age_ns: u64,
    /// 最低 bundle 版本（回滚拒绝）。
    pub min_bundle_version: u64,
    /// 是否强制签名 bundle（true = 无签名拒绝；生产态）。
    pub require_signature: bool,
}

/// 签名验证材料（key_id → key 由调用方持有；crate 不存 key）。
pub struct SignatureVerifier<'k> {
    pub key_id: u16,
    pub key: &'k [u8],
}

/// 编译输入（[09] §2.2）。
pub struct CompileInputs<'a, 'k> {
    pub snapshot: &'a TrustedSnapshot,
    /// ResourceRegistry generation（证据绑定；.36 不读注册表本体）。
    pub registry_generation: u64,
    pub bundle: &'a RuleBundle,
    pub facts: &'a FactSet,
    /// 本版本生产禁用（[09] §2.2：可选 IntentCandidate，传入即拒）。
    pub intent: Option<&'a eneros_decision_contract::IntentCandidate>,
    /// bundle 签名验证材料。
    pub verify: Option<SignatureVerifier<'k>>,
}

/// 冲突原因（闭集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// 同层同类范围交集为空。
    EmptyIntersection,
    /// 同层同类精确值矛盾（承诺/终值）。
    ContradictoryExactValues,
    /// 跨层矛盾（低层与高层联合交集为空）。
    CrossLayerContradiction,
}

/// 冲突报告（[09] §2.4 ObjectiveConflict）。
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveConflict {
    pub layer: u16,
    /// 类码（SOC/功率/能量/…；见 group_class）。
    pub class: u16,
    pub scope: ScopeRef,
    pub unit_code: u16,
    /// 参与冲突的规则 id（排序稳定）。
    pub constraint_ids: Vec<u64>,
    /// 求交结果（空范围）。
    pub lower: f64,
    pub upper: f64,
    pub reason: ConflictReason,
}

/// 编译错误（结构化；一切拒绝可观测）。
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    /// IntentCandidate 生产禁用（本版本）。
    IntentDisabled,
    /// bundle 内容摘要复算不符（篡改）。
    BundleTampered,
    /// 需要签名但未提供/未签。
    BundleSignatureRequired,
    /// 签名验证失败。
    BundleSignatureInvalid,
    /// bundle 版本回滚。
    BundleRolledBack {
        found: u64,
        min: u64,
    },
    /// 有效期未覆盖地平线（bundle/规则/维护窗）。
    WindowNotCovered(&'static str),
    /// 事实过期。
    StaleFacts,
    UnknownKind(u16),
    UnknownUnit(u16),
    UnitKindMismatch {
        kind: u16,
        unit_code: u16,
    },
    UnknownNormalization(u32),
    NonFinite(&'static str),
    /// 规则内容非法（构造校验失败透传）。
    InvalidRule(&'static str),
    NoHardConstraints,
    /// 必备约束缺失（安全底线 manifest 不满足）。
    MissingRequiredConstraint(u64),
    /// 第 7 层未批准建议进入生产编译。
    UnapprovedLayer,
    /// 硬约束冲突（零产出）。
    Conflicted(Vec<ObjectiveConflict>),
    LimitExceeded(&'static str),
}

/// 编译字段袋（WP-64.3.2.1）：`compile()` 与 `test-util` fixture 缝的
/// 共用装配形态；`spec_digest` **不在袋中**——只算不传。
pub struct CompiledObjectiveParts {
    pub spec: ObjectiveSpec,
    pub prices_per_kwh: Vec<f64>,
    pub min_on_steps: u32,
    pub min_off_steps: u32,
    pub registry_generation: u64,
    pub bundle_version: u64,
    pub bundle_digest: [u8; 32],
    pub horizon_step_ns: u64,
    pub horizon_step_count: u32,
}

/// 编译产物（[09] §2.2 输出 ObjectiveSpec + 求解经济输入 + 证据）。
///
/// WP-64.3.2.1 密封构造：字段 crate 私有，公开构造唯一出口 =
/// [`ObjectiveCompiler::compile`]（`spec_digest` 构造时由
/// [`digest_compiled`] 算出——**只算不传**，外部手调镜像函数即编译
/// 失败）；crate 内测试装配走 `test-util` feature 的 fixture 缝。
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledObjective {
    /// 合同形状 ObjectiveSpec（唯一 solver 面种类）。
    pub(crate) spec: ObjectiveSpec,
    /// 每步电价（元/kWh；长度 = step_count 或 0）。
    pub(crate) prices_per_kwh: Vec<f64>,
    /// 工艺事实透传（.37 min on/off 用）。
    pub(crate) min_on_steps: u32,
    pub(crate) min_off_steps: u32,
    pub(crate) registry_generation: u64,
    pub(crate) bundle_version: u64,
    pub(crate) bundle_digest: [u8; 32],
    /// 规格摘要（CandidatePlan.objective_digest 来源；覆盖全部内容）。
    pub(crate) spec_digest: [u8; 32],
    pub(crate) horizon_step_ns: u64,
    pub(crate) horizon_step_count: u32,
}

impl CompiledObjective {
    /// crate 内组装（digest 唯一计算点；外部不可达）。
    pub(crate) fn from_parts(parts: CompiledObjectiveParts) -> Self {
        let spec_digest = digest_compiled(&parts);
        Self {
            spec: parts.spec,
            prices_per_kwh: parts.prices_per_kwh,
            min_on_steps: parts.min_on_steps,
            min_off_steps: parts.min_off_steps,
            registry_generation: parts.registry_generation,
            bundle_version: parts.bundle_version,
            bundle_digest: parts.bundle_digest,
            spec_digest,
            horizon_step_ns: parts.horizon_step_ns,
            horizon_step_count: parts.horizon_step_count,
        }
    }

    /// crate 内测试装配缝（WP-64.3.2.1）：planner/certifier 集成测试经
    /// `test-util` feature 构造 fixture——digest 仍由 [`digest_compiled`]
    /// 算出，测试面与生产面同构造纪律（`#[cfg(test)]` 不跨 crate，
    /// 故以 feature 门替代，执行卡登记）。
    #[cfg(feature = "test-util")]
    pub fn compile_fixture(parts: CompiledObjectiveParts) -> Self {
        Self::from_parts(parts)
    }

    pub fn spec(&self) -> &ObjectiveSpec {
        &self.spec
    }
    pub fn prices_per_kwh(&self) -> &[f64] {
        &self.prices_per_kwh
    }
    /// 规格摘要（CandidatePlan.objective_digest 来源）。
    pub fn spec_digest(&self) -> &[u8; 32] {
        &self.spec_digest
    }
    pub fn horizon_step_ns(&self) -> u64 {
        self.horizon_step_ns
    }
    pub fn horizon_step_count(&self) -> u32 {
        self.horizon_step_count
    }
}

impl ObjectiveCompiler {
    pub fn new(max_facts_age_ns: u64, min_bundle_version: u64, require_signature: bool) -> Self {
        Self {
            max_facts_age_ns,
            min_bundle_version,
            require_signature,
        }
    }

    /// 唯一编译入口（[16] §3.7 公开面）。
    pub fn compile(&self, input: CompileInputs<'_, '_>) -> Result<CompiledObjective, CompileError> {
        // 0. IntentCandidate 生产禁用。
        if input.intent.is_some() {
            return Err(CompileError::IntentDisabled);
        }
        let bundle = input.bundle;
        let snapshot = input.snapshot;

        // 1. bundle 完整性 + 签名 + 版本。
        if !bundle.verify_digest() {
            return Err(CompileError::BundleTampered);
        }
        if bundle.signature().is_some() {
            let v = input
                .verify
                .as_ref()
                .ok_or(CompileError::BundleSignatureRequired)?;
            bundle.verify_signature(v.key_id, v.key)?;
        } else if self.require_signature {
            return Err(CompileError::BundleSignatureRequired);
        }
        if bundle.version() < self.min_bundle_version {
            return Err(CompileError::BundleRolledBack {
                found: bundle.version(),
                min: self.min_bundle_version,
            });
        }

        // 2. 地平线与覆盖（地平线锚定快照切面）。
        let horizon = bundle.horizon();
        let cut_ns = snapshot.cut_time_ns;
        let horizon_end_ns = cut_ns.saturating_add(horizon.span_ns());
        if cut_ns < bundle.valid_from_ns() || horizon_end_ns > bundle.valid_until_ns() {
            return Err(CompileError::WindowNotCovered("bundle"));
        }

        // 3. 事实新鲜度 + 来源。
        let facts = input.facts;
        if facts.as_of_ns.saturating_add(self.max_facts_age_ns) < cut_ns {
            return Err(CompileError::StaleFacts);
        }
        if !facts.market.prices_per_kwh.is_empty() && facts.market.source_event_id == [0u8; 16] {
            return Err(CompileError::InvalidRule("facts source_event_id=0"));
        }

        // 4. 维护窗与地平线相交 → 拒绝（资源在窗内不可用，保守不规划）。
        for (lo, hi) in &facts.enterprise.maintenance_windows_ns {
            if *lo < horizon_end_ns && cut_ns < *hi {
                return Err(CompileError::WindowNotCovered("maintenance"));
            }
        }

        // 5. 规则激活过滤（有效期必须全覆盖地平线）。
        let mut active: Vec<&ConstraintSpec> = Vec::new();
        for rule in bundle.constraints() {
            if rule.priority == layer::UNAPPROVED_ADVICE {
                return Err(CompileError::UnapprovedLayer);
            }
            if rule.active_from_ns <= cut_ns && rule.active_until_ns >= horizon_end_ns {
                active.push(rule);
            }
        }
        // 6. 必备清单（安全底线）：manifest 成员必须以**硬约束**激活
        // ——id 存在但被翻成软约束同样击穿安全底线（一位翻转不可绕过）。
        for id in bundle.required_constraints() {
            let ok = active
                .iter()
                .any(|r| &r.constraint_id == id && r.hardness == 0);
            if !ok {
                return Err(CompileError::MissingRequiredConstraint(*id));
            }
        }

        // 7. 逐 (scope, class) 求交（层内空交=冲突；跨层联合空交=冲突）。
        let conflicts = self.resolve_conflicts(&active);
        if !conflicts.is_empty() {
            return Err(CompileError::Conflicted(conflicts));
        }

        // 8. 有效范围 → 合同硬约束（one-sided 拆分；单位归一到类基准）。
        let mut hard: Vec<Constraint> = Vec::new();
        let mut soft: Vec<Constraint> = Vec::new();
        self.emit_constraints(
            &active,
            &mut hard,
            &mut soft,
            bundle.site_rating_kw(),
            cut_ns,
            horizon_end_ns,
        )?;
        // 人工越权收紧（企业事实；只收紧 SOC）。
        if let Some((ov_lo, ov_hi)) = facts.enterprise.operator_override_soc {
            if !(0.0..=1.0).contains(&ov_lo) || !(0.0..=1.0).contains(&ov_hi) || ov_lo > ov_hi {
                return Err(CompileError::InvalidRule("operator_override_soc"));
            }
            tighten_soc(&mut hard, ov_lo, ov_hi);
            // 收紧后复检窗口非空：override 与既有约束矛盾时拒绝（产出
            // MinSoc > MaxSoc 的不可行 spec 违背"冲突零产出"不变量）。
            let mut lo = None;
            let mut hi = None;
            for c in hard.iter() {
                match c.kind {
                    ConstraintKind::MinSoc => {
                        lo = Some(lo.map_or(c.bound, |v: f64| v.max(c.bound)))
                    }
                    ConstraintKind::MaxSoc => {
                        hi = Some(hi.map_or(c.bound, |v: f64| v.min(c.bound)))
                    }
                    _ => {}
                }
            }
            if let (Some(l), Some(h)) = (lo, hi) {
                if l > h {
                    return Err(CompileError::InvalidRule(
                        "operator_override_soc inverts soc window",
                    ));
                }
            }
        }
        // 跨类一致性（类间矛盾同样零产出）：终值 SOC 点必须落在 [MinSoc,
        // MaxSoc] 窗内——求交只裁决同类，点类(2)与范围类(1)分组不同，
        // 不可行组合在此拦截而非流给 solver。
        {
            let mut w_lo = None;
            let mut w_hi = None;
            let mut final_bound = None;
            for c in hard.iter() {
                match c.kind {
                    ConstraintKind::MinSoc => {
                        w_lo = Some(w_lo.map_or(c.bound, |v: f64| v.max(c.bound)))
                    }
                    ConstraintKind::MaxSoc => {
                        w_hi = Some(w_hi.map_or(c.bound, |v: f64| v.min(c.bound)))
                    }
                    ConstraintKind::SocFinal => final_bound = Some(c.bound),
                    _ => {}
                }
            }
            if let Some(f) = final_bound {
                if w_lo.is_some_and(|lo| f < lo) {
                    return Err(CompileError::InvalidRule("soc_final below soc window"));
                }
                if w_hi.is_some_and(|hi| f > hi) {
                    return Err(CompileError::InvalidRule("soc_final above soc window"));
                }
            }
        }
        if hard.is_empty() {
            return Err(CompileError::NoHardConstraints);
        }
        if hard.len() > eneros_decision_contract::wire::MAX_CONSTRAINTS
            || soft.len() > eneros_decision_contract::wire::MAX_CONSTRAINTS
        {
            return Err(CompileError::LimitExceeded("constraints"));
        }

        // 9. 目标项：第 7 层拒绝；主目标 = 合同可映射的最低层项。
        if bundle
            .terms()
            .iter()
            .any(|t| t.priority == layer::UNAPPROVED_ADVICE)
        {
            return Err(CompileError::UnapprovedLayer);
        }
        let mut primary: Option<(u16, eneros_decision_contract::objective::ObjectiveKind)> = None;
        for t in bundle.terms() {
            if let Some(kind) = crate::term_kind::to_contract(t.kind) {
                match primary {
                    Some((cur_layer, _)) if cur_layer <= t.priority => {}
                    _ => primary = Some((t.priority, kind)),
                }
            }
        }
        let Some((primary_layer, objective_kind)) = primary else {
            return Err(CompileError::InvalidRule(
                "no contract-mappable objective term",
            ));
        };
        let reason = match objective_kind {
            eneros_decision_contract::objective::ObjectiveKind::MaxRevenue => {
                ReasonCode::PriceArbitrage
            }
            eneros_decision_contract::objective::ObjectiveKind::PeakShave => ReasonCode::PeakShave,
            eneros_decision_contract::objective::ObjectiveKind::SelfConsume => {
                ReasonCode::RenewableCurtail
            }
            eneros_decision_contract::objective::ObjectiveKind::MinCost => {
                ReasonCode::PriceArbitrage
            }
        };

        // 10. 价格序列（0 = 无 / 1 = 常数广播 / step_count = 逐步）。
        let raw_prices = &facts.market.prices_per_kwh;
        for p in raw_prices {
            if !p.is_finite() {
                return Err(CompileError::NonFinite("price"));
            }
        }
        // 无价格 = 经济项不可用（[09] §2.2）：价格驱动的目标在空价格
        // 下编译会产出"任意调度皆最优"的退化指令（fail-open）。
        if raw_prices.is_empty()
            && matches!(
                objective_kind,
                eneros_decision_contract::objective::ObjectiveKind::MaxRevenue
                    | eneros_decision_contract::objective::ObjectiveKind::MinCost
            )
        {
            return Err(CompileError::InvalidRule(
                "economic objective requires prices",
            ));
        }
        let prices: Vec<f64> = match raw_prices.len() {
            0 => vec![],
            1 => vec![raw_prices[0]; horizon.step_count as usize],
            n if n == horizon.step_count as usize => raw_prices.clone(),
            _ => return Err(CompileError::InvalidRule("price length")),
        };

        // 11. 合同 ObjectiveSpec（ms 窗）。
        let valid_from_ms = cut_ns / 1_000_000;
        let valid_until_ms = horizon_end_ns / 1_000_000;
        let spec = ObjectiveSpec::builder(SnapshotId(snapshot.snapshot_id))
            .constraints(hard, soft)
            .objective(
                objective_kind,
                primary_layer as u8,
                ObjectiveSource::RuleBundle,
                reason,
            )
            .window(valid_from_ms, valid_until_ms)
            .build()
            .map_err(|_| CompileError::InvalidRule("spec"))?;

        Ok(CompiledObjective::from_parts(CompiledObjectiveParts {
            spec,
            prices_per_kwh: prices,
            min_on_steps: facts.process.min_on_steps,
            min_off_steps: facts.process.min_off_steps,
            registry_generation: input.registry_generation,
            bundle_version: bundle.version(),
            bundle_digest: *bundle.digest(),
            horizon_step_ns: horizon.step_ns,
            horizon_step_count: horizon.step_count,
        }))
    }

    /// 层内/跨层冲突检测（同 (scope, class) 分组求交）。
    fn resolve_conflicts(&self, active: &[&ConstraintSpec]) -> Vec<ObjectiveConflict> {
        use alloc::collections::BTreeMap;
        let mut groups: BTreeMap<(ScopeRef, u16), Vec<&ConstraintSpec>> = BTreeMap::new();
        for r in active {
            if r.hardness != 0 {
                continue; // 软约束不做冲突裁决
            }
            groups
                .entry((r.scope, r.group_class()))
                .or_default()
                .push(r);
        }
        let mut out = Vec::new();
        for ((scope, class), rules) in groups {
            // 层内求交：同层内范围/精确值矛盾直接冲突。
            let mut by_layer: BTreeMap<u16, Vec<&ConstraintSpec>> = BTreeMap::new();
            for r in &rules {
                by_layer.entry(r.priority).or_default().push(r);
            }
            for lr in by_layer.values() {
                if let Some(c) = intersect_one_layer(scope, class, lr) {
                    out.push(c);
                }
            }
            // 跨层联合求交：低层不能放宽高层；联合空 = 矛盾。
            if out.is_empty() {
                let mut lo = f64::MIN;
                let mut hi = f64::MAX;
                let mut lo_id = 0u64;
                let mut hi_id = 0u64;
                let mut any = false;
                for r in &rules {
                    let (r_lo, r_hi) = contribution(r, class);
                    if r_lo > lo {
                        lo = r_lo;
                        lo_id = r.constraint_id;
                    }
                    if r_hi < hi {
                        hi = r_hi;
                        hi_id = r.constraint_id;
                    }
                    any = true;
                }
                if any && lo > hi {
                    let mut ids = vec![lo_id, hi_id];
                    ids.sort_unstable();
                    ids.dedup();
                    out.push(ObjectiveConflict {
                        layer: rules.iter().map(|r| r.priority).min().unwrap_or(0),
                        class,
                        scope,
                        unit_code: rules[0].unit_code,
                        constraint_ids: ids,
                        lower: lo,
                        upper: hi,
                        reason: ConflictReason::CrossLayerContradiction,
                    });
                }
            }
        }
        out
    }

    /// 有效范围 → 合同约束拆分（单位归一类基准）。
    #[allow(clippy::too_many_arguments)]
    fn emit_constraints(
        &self,
        active: &[&ConstraintSpec],
        hard: &mut Vec<Constraint>,
        soft: &mut Vec<Constraint>,
        site_rating_kw: f64,
        _cut_ns: u64,
        _end_ns: u64,
    ) -> Result<(), CompileError> {
        use alloc::collections::BTreeMap;
        // (scope, class, hardness) → 合并范围。
        /// (scope, class, hardness) → 合并范围 (lo, hi)。
        type RangeMap = BTreeMap<(ScopeRef, u16, u8), (Option<f64>, Option<f64>)>;
        let mut ranges: RangeMap = BTreeMap::new();
        let mut exacts: BTreeMap<(ScopeRef, u16, u8), (f64, u16)> = BTreeMap::new();
        for r in active {
            let class = r.group_class();
            let canonical = to_canonical(r)?;
            match class {
                // 精确值类。
                2 | 5 => {
                    exacts.insert((r.scope, class, r.hardness), (canonical, r.unit_code));
                }
                // 单界类（Ramp/Reserve：无冲突，取最严）。
                6..=8 => {
                    let key = (r.scope, class, r.hardness);
                    let e = ranges.entry(key).or_insert((None, None));
                    match class {
                        8 => e.0 = Some(e.0.map_or(canonical, |v: f64| v.max(canonical))),
                        _ => e.1 = Some(e.1.map_or(canonical, |v: f64| v.min(canonical))),
                    }
                }
                // 范围类。
                _ => {
                    let key = (r.scope, class, r.hardness);
                    let e = ranges.entry(key).or_insert((None, None));
                    // Min* 贡献 lower，Max* 贡献 upper。
                    let kind = ConstraintKind::from_code(r.kind)
                        .ok_or(CompileError::UnknownKind(r.kind))?;
                    match kind {
                        ConstraintKind::MinSoc
                        | ConstraintKind::MinPowerKw
                        | ConstraintKind::MinEnergyKwh => {
                            e.0 = Some(e.0.map_or(canonical, |v: f64| v.max(canonical)))
                        }
                        ConstraintKind::MaxSoc
                        | ConstraintKind::MaxPowerKw
                        | ConstraintKind::MaxEnergyKwh => {
                            e.1 = Some(e.1.map_or(canonical, |v: f64| v.min(canonical)))
                        }
                        _ => return Err(CompileError::InvalidRule("range class")),
                    }
                }
            }
        }
        // 范围拆分（site 级 SOC 用比率；功率 kW；能量 kWh——类基准即合同单位）。
        for ((scope, class, h), (lo, hi)) in &ranges {
            let _ = scope; // .36 站级汇总；资产级拆分随 .37 消费方扩展
            let _ = site_rating_kw;
            let (lo, hi) = (*lo, *hi);
            match class {
                1 => {
                    // SOC 范围（比率）。
                    if let Some(lo) = lo {
                        push(hard, soft, *h, ConstraintKind::MinSoc, lo);
                    }
                    if let Some(hi) = hi {
                        push(hard, soft, *h, ConstraintKind::MaxSoc, hi);
                    }
                }
                3 => {
                    if let Some(lo) = lo {
                        push(hard, soft, *h, ConstraintKind::MinPowerKw, lo);
                    }
                    if let Some(hi) = hi {
                        push(hard, soft, *h, ConstraintKind::MaxPowerKw, hi);
                    }
                }
                4 => {
                    if let Some(lo) = lo {
                        push(hard, soft, *h, ConstraintKind::MinEnergyKwh, lo);
                    }
                    if let Some(hi) = hi {
                        push(hard, soft, *h, ConstraintKind::MaxEnergyKwh, hi);
                    }
                }
                6 => {
                    if let Some(hi) = hi {
                        push(hard, soft, *h, ConstraintKind::RampUpKwS, hi);
                    }
                }
                7 => {
                    if let Some(hi) = hi {
                        push(hard, soft, *h, ConstraintKind::RampDownKwS, hi);
                    }
                }
                8 => {
                    if let Some(lo) = lo {
                        push(hard, soft, *h, ConstraintKind::ReserveCapacityKw, lo);
                    }
                }
                _ => return Err(CompileError::InvalidRule("class")),
            }
        }
        for ((_scope, class, h), (v, _unit)) in &exacts {
            match class {
                2 => push(hard, soft, *h, ConstraintKind::SocFinal, *v),
                5 => push(hard, soft, *h, ConstraintKind::EnergyCommitmentKwh, *v),
                _ => return Err(CompileError::InvalidRule("exact class")),
            }
        }
        Ok(())
    }
}

/// 规则对同类组的范围贡献（[lo, hi] 语义，未绑定侧为 ±∞；值已归一
/// 到类基准——不同单位规则求交必须同基准）：Min* 只贡献下界（上界视为
/// +∞）；Max* 只贡献上界；精确类（2/5）贡献点；Ramp（6/7）取 upper
/// 最小；Reserve（8）取 lower 最大。
fn contribution(r: &ConstraintSpec, class: u16) -> (f64, f64) {
    // 构造期已保证单位/kind 已知，to_canonical 不可失败；unwrap_or 仅
    // 防御性回退（绝不放大范围：回退值仍是被校验过的原值）。
    let bind = to_canonical(r).unwrap_or(r.binding());
    match class {
        6 | 7 => (f64::MIN, bind),
        8 => (bind, f64::MAX),
        2 | 5 => (bind, bind),
        _ => match ConstraintKind::from_code(r.kind) {
            Some(ConstraintKind::MinSoc)
            | Some(ConstraintKind::MinPowerKw)
            | Some(ConstraintKind::MinEnergyKwh) => (bind, f64::MAX),
            Some(ConstraintKind::MaxSoc)
            | Some(ConstraintKind::MaxPowerKw)
            | Some(ConstraintKind::MaxEnergyKwh) => (f64::MIN, bind),
            _ => (bind, bind),
        },
    }
}

/// 层内同类求交（空交 → 冲突报告）。
fn intersect_one_layer(
    scope: ScopeRef,
    class: u16,
    rules: &[&ConstraintSpec],
) -> Option<ObjectiveConflict> {
    let mut lo = f64::MIN;
    let mut hi = f64::MAX;
    let mut ids: Vec<u64> = Vec::new();
    for r in rules {
        let (r_lo, r_hi) = contribution(r, class);
        lo = lo.max(r_lo);
        hi = hi.min(r_hi);
        ids.push(r.constraint_id);
    }
    if lo > hi {
        ids.sort_unstable();
        let reason = if matches!(class, 2 | 5) {
            ConflictReason::ContradictoryExactValues
        } else {
            ConflictReason::EmptyIntersection
        };
        Some(ObjectiveConflict {
            layer: rules[0].priority,
            class,
            scope,
            unit_code: rules[0].unit_code,
            constraint_ids: ids,
            lower: lo,
            upper: hi,
            reason,
        })
    } else {
        None
    }
}

/// 类基准单位归一（Power→kW；Energy→kWh；Ratio→0..1 比率；Ramp→kW/s）。
fn to_canonical(r: &ConstraintSpec) -> Result<f64, CompileError> {
    let unit = eneros_telemetry_model::unit_catalog::from_code(r.unit_code)
        .ok_or(CompileError::UnknownUnit(r.unit_code))?;
    let kind = ConstraintKind::from_code(r.kind).ok_or(CompileError::UnknownKind(r.kind))?;
    let canonical = match kind.required_dimension() {
        eneros_telemetry_model::Dimension::Power => eneros_telemetry_model::PhysicalUnit::Kilowatts,
        eneros_telemetry_model::Dimension::Energy => {
            eneros_telemetry_model::PhysicalUnit::KilowattHour
        }
        eneros_telemetry_model::Dimension::Ratio => {
            eneros_telemetry_model::PhysicalUnit::Dimensionless
        }
        // 其余维度（频率/温度…）保留 SI 基准。
        _ => return Ok(r.binding() * unit.si_scale_factor()),
    };
    Ok(r.binding() * unit.si_scale_factor() / canonical.si_scale_factor())
}

/// 附加约束（hard/soft 分派）。
fn push(
    hard: &mut Vec<Constraint>,
    soft: &mut Vec<Constraint>,
    hardness: u8,
    kind: ConstraintKind,
    bound: f64,
) {
    let c = Constraint { kind, bound };
    if hardness == 0 {
        hard.push(c);
    } else {
        soft.push(c);
    }
}

/// 人工越权 SOC 收紧（只收紧）。
fn tighten_soc(hard: &mut [Constraint], lo: f64, hi: f64) {
    for c in hard.iter_mut() {
        match c.kind {
            ConstraintKind::MinSoc => c.bound = c.bound.max(lo),
            ConstraintKind::MaxSoc => c.bound = c.bound.min(hi),
            _ => {}
        }
    }
}

/// 编译产物摘要（CandidatePlan.objective_digest 的单源）。
///
/// 覆盖 CompiledObjective 全部内容字段（含 min on/off 透传与地平线——
/// 价格为空/常数广播时 spec+prices 不绑定地平线，缺地平线域 = 可替换
/// 地平线而不改摘要）。
pub(crate) fn digest_compiled(parts: &CompiledObjectiveParts) -> [u8; 32] {
    let CompiledObjectiveParts {
        ref spec,
        ref prices_per_kwh,
        min_on_steps,
        min_off_steps,
        registry_generation,
        // bundle_version 显式出局（弃权而非 `..`）：版本已在 bundle_digest
        // 输入内（bundle 编码含版本域）——入 preimage 即双计。蓝图 65
        // WP-65.5 去弃权：穷尽解构后未来 Parts 增字段 = 编译错，静默出
        // preimage 不再可能；摘要值逐字节不变（known-vector 锁）。
        bundle_version: _,
        bundle_digest,
        horizon_step_ns,
        horizon_step_count,
    } = *parts;
    let prices: &[f64] = prices_per_kwh;
    let bundle_digest: &[u8; 32] = &bundle_digest;
    use eneros_decision_contract::objective::ObjectiveKind;
    use sha2::Digest;
    let kind_code = |k: ObjectiveKind| match k {
        ObjectiveKind::MaxRevenue => 1u8,
        ObjectiveKind::MinCost => 2,
        ObjectiveKind::PeakShave => 3,
        ObjectiveKind::SelfConsume => 4,
    };
    let mut h = sha2::Sha256::new();
    h.update(b"eneros.compiled-objective.v2");
    h.update(spec.snapshot_id().0);
    h.update([kind_code(spec.objective())]);
    h.update([spec.priority()]);
    h.update([spec.reason().code()]);
    h.update(spec.valid_from_ms().to_le_bytes());
    h.update(spec.valid_until_ms().to_le_bytes());
    h.update((spec.hard().len() as u32).to_le_bytes());
    for c in spec.hard() {
        h.update(c.kind.code().to_le_bytes());
        h.update(c.bound.to_le_bytes());
    }
    h.update((spec.soft().len() as u32).to_le_bytes());
    for c in spec.soft() {
        h.update(c.kind.code().to_le_bytes());
        h.update(c.bound.to_le_bytes());
    }
    h.update((prices.len() as u32).to_le_bytes());
    for p in prices {
        h.update(p.to_le_bytes());
    }
    h.update(min_on_steps.to_le_bytes());
    h.update(min_off_steps.to_le_bytes());
    h.update(horizon_step_ns.to_le_bytes());
    h.update(horizon_step_count.to_le_bytes());
    h.update(bundle_digest);
    h.update(registry_generation.to_le_bytes());
    h.finalize().into()
}

// ---------------------------------------------------------------------------
// tests — [09] §2.6 红测 + §2.7 判据
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use alloc::{vec, vec::Vec};

    /// WP-64.3.2.1 known-vector（64.3 密封前现值快照）。
    const SPEC_DIGEST_KNOWN_VECTOR: &str =
        "55f8dc19738b6683478c8bfa672503df933677e9deb75afd049db2dfc9114ffc";

    use eneros_decision_contract::objective::ConstraintKind as K;
    use eneros_snapshot_service::TrustedSnapshot;

    use super::*;

    /// WP-64.3.2.1 golden：固定输入的 spec_digest 快照（64.3 密封前现值
    /// ——同输入同摘要；摘要公式/域串/字段序任何漂移即红）。
    #[test]
    fn spec_digest_known_vector() {
        let obj = CompiledObjective::from_parts(CompiledObjectiveParts {
            spec: {
                let mut sid = [0u8; 16];
                sid[..8].copy_from_slice(&0xA5A5u64.to_le_bytes());
                eneros_decision_contract::ObjectiveSpec::builder(
                    eneros_decision_contract::SnapshotId(sid),
                )
                .constraints(
                    vec![eneros_decision_contract::objective::Constraint {
                        kind: K::MinSoc,
                        bound: 0.1,
                    }],
                    vec![],
                )
                .objective(
                    eneros_decision_contract::objective::ObjectiveKind::PeakShave,
                    1,
                    eneros_decision_contract::objective::ObjectiveSource::RuleBundle,
                    eneros_decision_contract::ReasonCode::PeakShave,
                )
                .window(1_000, 61_000)
                .build()
                .unwrap()
            },
            prices_per_kwh: vec![420.0],
            min_on_steps: 2,
            min_off_steps: 1,
            registry_generation: 7,
            bundle_version: 1,
            bundle_digest: [9; 32],
            horizon_step_ns: 900_000_000,
            horizon_step_count: 4,
        });
        let hex: alloc::string::String = obj
            .spec_digest()
            .iter()
            .map(|b| alloc::format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex, SPEC_DIGEST_KNOWN_VECTOR,
            "spec_digest 已知向量漂移——digest_compiled 公式/域串/字段序变更"
        );
    }
    use crate::bundle::RuleBundle;
    use crate::layer;
    use crate::normalization;
    use crate::spec::{ConstraintSpec, FactSet, HorizonPlan, MarketFacts, ScopeRef};

    const CUT: u64 = 1_000_000_000_000; // 1e12 ns

    fn snapshot() -> TrustedSnapshot {
        TrustedSnapshot {
            snapshot_id: [7; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: CUT,
            produced_at_ns: CUT,
            policy_version: 1,
            version_set: eneros_semantic_catalog::VersionSet::ZERO,
            observations: vec![],
            source_watermarks: vec![],
            completeness: eneros_snapshot_service::CompletenessReport {
                required: vec![],
                accepted: vec![],
                missing: vec![],
                stale: vec![],
                bad: vec![],
                conflict: vec![],
                substituted: vec![],
            },
            conflicts: vec![],
            derived: vec![],
            provenance_root: [3; 32],
        }
    }

    fn factset(as_of_ns: u64) -> FactSet {
        FactSet {
            as_of_ns,
            market: MarketFacts {
                prices_per_kwh: vec![0.5],
                source_event_id: [1; 16],
            },
            ..Default::default()
        }
    }

    fn rule(
        id: u64,
        kind: K,
        hardness: u8,
        layer_p: u16,
        unit: u16,
        lo: f64,
        hi: f64,
    ) -> ConstraintSpec {
        ConstraintSpec::new(
            id,
            kind.code(),
            hardness,
            layer_p,
            ScopeRef::SITE,
            unit,
            lo,
            hi,
            0,
            u64::MAX,
            [1; 16],
        )
        .unwrap()
    }

    fn dim(u: eneros_telemetry_model::PhysicalUnit) -> u16 {
        eneros_telemetry_model::unit_catalog::code_of(u)
    }

    fn econ_term(id: u64, layer_p: u16) -> crate::spec::ObjectiveTerm {
        crate::spec::ObjectiveTerm::new(
            id,
            crate::term_kind::ECONOMIC,
            1.0,
            dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            normalization::PRICE_SCALED,
            layer_p,
            [1; 16],
        )
        .unwrap()
    }

    fn signed_bundle(
        constraints: Vec<ConstraintSpec>,
        terms: Vec<crate::spec::ObjectiveTerm>,
    ) -> RuleBundle {
        let mut b = RuleBundle::builder([9; 16], 5)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0);
        for c in constraints {
            b = b.add_constraint(c);
        }
        for t in terms {
            b = b.add_term(t);
        }
        let mut bundle = b.build().unwrap();
        bundle.seal(1, b"test-key");
        bundle
    }

    fn compiler() -> ObjectiveCompiler {
        ObjectiveCompiler::new(60_000_000_000, 1, true)
    }

    fn compile(bundle: &RuleBundle, facts: &FactSet) -> Result<CompiledObjective, CompileError> {
        compiler().compile(CompileInputs {
            snapshot: &snapshot(),
            registry_generation: 4,
            bundle,
            facts,
            intent: None,
            verify: Some(SignatureVerifier {
                key_id: 1,
                key: b"test-key",
            }),
        })
    }

    fn base_rules() -> Vec<ConstraintSpec> {
        vec![
            rule(
                1,
                K::MinSoc,
                0,
                layer::DEVICE_SAFETY,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.1,
                0.1,
            ),
            rule(
                2,
                K::MaxSoc,
                0,
                layer::DEVICE_SAFETY,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.9,
                0.9,
            ),
            rule(
                3,
                K::MaxPowerKw,
                0,
                layer::GRID_CODE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                -100.0,
                100.0,
            ),
        ]
    }

    #[test]
    fn compile_happy_path_and_digest_stability() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let out = compile(&b, &factset(CUT)).expect("compile");
        // MinSoc(0.1)+MaxSoc(0.9)+MaxPowerKw(100)——Max* 规则只贡献上界，
        // 无 Min 功率规则则不产生 MinPowerKw（包络下界归 .37 供给面）。
        assert_eq!(out.spec.hard().len(), 3);
        assert_eq!(
            out.spec.objective(),
            eneros_decision_contract::objective::ObjectiveKind::MaxRevenue
        );
        assert_eq!(out.prices_per_kwh.len(), 96);
        assert_eq!(out.registry_generation, 4);
        assert!(out.spec.is_valid_at(CUT / 1_000_000 + 1));
        // 顺序无关：同内容乱序重建 → spec_digest 一致。
        let mut rev = base_rules();
        rev.reverse();
        let b2 = signed_bundle(rev, vec![econ_term(10, layer::ECONOMIC)]);
        let out2 = compile(&b2, &factset(CUT)).unwrap();
        assert_eq!(out.spec_digest, out2.spec_digest, "顺序无关");
        // 重复编译 → 稳定。
        let out3 = compile(&b, &factset(CUT)).unwrap();
        assert_eq!(out.spec_digest, out3.spec_digest);
    }

    /// 红测：IntentCandidate 生产禁用。
    #[test]
    fn intent_candidate_rejected() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let intent = eneros_decision_contract::IntentCandidate::new(
            SnapshotId([7; 16]),
            eneros_decision_contract::objective::ObjectiveKind::PeakShave,
            ReasonCode::PeakShave,
            9_000,
            [1; 32],
            [1; 32],
            u64::MAX,
        )
        .unwrap();
        let err = compiler().compile(CompileInputs {
            snapshot: &snapshot(),
            registry_generation: 1,
            bundle: &b,
            facts: &factset(CUT),
            intent: Some(&intent),
            verify: Some(SignatureVerifier {
                key_id: 1,
                key: b"test-key",
            }),
        });
        assert_eq!(err, Err(CompileError::IntentDisabled));
    }

    /// 红测：冲突硬约束 → Conflicted 且**零产出**（不产出 spec）。
    #[test]
    fn conflicting_hard_constraints_zero_output() {
        // 同层（层 2）同向 Max 只收紧不冲突；矛盾须反向：Min 200 vs Max 100。
        let rules = vec![
            base_rules()[0].clone(),
            base_rules()[1].clone(),
            rule(
                3,
                K::MaxPowerKw,
                0,
                layer::GRID_CODE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                f64::MIN,
                100.0,
            ),
            rule(
                4,
                K::MinPowerKw,
                0,
                layer::GRID_CODE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                200.0,
                f64::MAX,
            ),
        ];
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        let err = compile(&b, &factset(CUT)).unwrap_err();
        match err {
            CompileError::Conflicted(conflicts) => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].reason, ConflictReason::EmptyIntersection);
                assert_eq!(conflicts[0].class, 3);
                assert_eq!(conflicts[0].constraint_ids, vec![3, 4]);
                assert!((conflicts[0].lower - 200.0).abs() < 1e-12);
                assert!((conflicts[0].upper - 100.0).abs() < 1e-12);
            }
            other => panic!("expected conflicted, got {other:?}"),
        }
        // 精确值矛盾（两个不同承诺）。
        let rules = vec![
            base_rules()[0].clone(),
            base_rules()[1].clone(),
            base_rules()[2].clone(),
            rule(
                5,
                K::EnergyCommitmentKwh,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::KilowattHour),
                50.0,
                50.0,
            ),
            rule(
                6,
                K::EnergyCommitmentKwh,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::KilowattHour),
                80.0,
                80.0,
            ),
        ];
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        match compile(&b, &factset(CUT)) {
            Err(CompileError::Conflicted(c)) => {
                assert_eq!(c[0].reason, ConflictReason::ContradictoryExactValues)
            }
            other => panic!("expected exact conflict, got {other:?}"),
        }
    }

    /// 跨层：低层只能收紧高层，不能放宽；联合矛盾 → 冲突。
    #[test]
    fn cross_layer_tighten_and_contradiction() {
        // 层 1 SOC [0.1,0.9]；层 5 收紧到 [0.2,0.8] → 生效 [0.2,0.8]。
        let rules = vec![
            base_rules()[0].clone(),
            base_rules()[1].clone(),
            base_rules()[2].clone(),
            rule(
                5,
                K::MinSoc,
                0,
                layer::OPERATIONAL,
                dim(eneros_telemetry_model::PhysicalUnit::Percent),
                20.0,
                20.0,
            ),
            rule(
                6,
                K::MaxSoc,
                0,
                layer::OPERATIONAL,
                dim(eneros_telemetry_model::PhysicalUnit::Percent),
                80.0,
                80.0,
            ),
        ];
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        let out = compile(&b, &factset(CUT)).unwrap();
        let kind = K::MinSoc;
        let soc_min = out
            .spec
            .hard()
            .iter()
            .find(|c| c.kind.code() == kind.code())
            .unwrap();
        assert!((soc_min.bound - 0.2).abs() < 1e-12, "Percent 归一 + 收紧");
        // 跨层矛盾：层 1 max 100kW 与层 4 min 200kW → 冲突。
        let rules = vec![
            base_rules()[0].clone(),
            base_rules()[1].clone(),
            base_rules()[2].clone(),
            rule(
                7,
                K::MinPowerKw,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                200.0,
                200.0,
            ),
        ];
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        match compile(&b, &factset(CUT)) {
            Err(CompileError::Conflicted(c)) => {
                assert_eq!(c[0].reason, ConflictReason::CrossLayerContradiction)
            }
            other => panic!("expected cross-layer conflict, got {other:?}"),
        }
    }

    /// 红测：过期事实、未知单位、未知 kind、量纲错配、非有限值。
    #[test]
    fn input_validation_rejections() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        // 过期事实（as_of 远早于 cut）。
        assert_eq!(
            compile(&b, &factset(CUT - 61_000_000_000)),
            Err(CompileError::StaleFacts)
        );
        // 未知单位。
        assert!(matches!(
            ConstraintSpec::new(
                9,
                K::MinSoc.code(),
                0,
                1,
                ScopeRef::SITE,
                999,
                0.0,
                1.0,
                0,
                u64::MAX,
                [1; 16]
            ),
            Err(CompileError::UnknownUnit(999))
        ));
        // 未知 kind。
        assert!(matches!(
            ConstraintSpec::new(
                9,
                99,
                0,
                1,
                ScopeRef::SITE,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.0,
                1.0,
                0,
                u64::MAX,
                [1; 16]
            ),
            Err(CompileError::UnknownKind(99))
        ));
        // 量纲错配（SOC 规则用 kW）。
        assert!(matches!(
            ConstraintSpec::new(
                9,
                K::MinSoc.code(),
                0,
                1,
                ScopeRef::SITE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                0.0,
                1.0,
                0,
                u64::MAX,
                [1; 16]
            ),
            Err(CompileError::UnitKindMismatch { .. })
        ));
        // 非有限界。
        assert_eq!(
            ConstraintSpec::new(
                9,
                K::MaxSoc.code(),
                0,
                1,
                ScopeRef::SITE,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.0,
                f64::NAN,
                0,
                u64::MAX,
                [1; 16]
            ),
            Err(CompileError::NonFinite("bound"))
        );
        // 未知归一化。
        assert!(matches!(
            crate::spec::ObjectiveTerm::new(
                1,
                crate::term_kind::ECONOMIC,
                1.0,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                77,
                6,
                [1; 16]
            ),
            Err(CompileError::UnknownNormalization(77))
        ));
    }

    /// 篡改/回滚/签名缺失。
    #[test]
    fn bundle_tamper_rollback_signature_gates() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        // 篡改：复算 digest 不符。
        let mut tampered = b.clone();
        tampered.corrupt_digest_for_test();
        let err = compile(&tampered, &factset(CUT));
        assert_eq!(err, Err(CompileError::BundleTampered));
        // 回滚：version 5 < min 6。
        let c = ObjectiveCompiler::new(60_000_000_000, 6, true);
        let err = c.compile(CompileInputs {
            snapshot: &snapshot(),
            registry_generation: 1,
            bundle: &b,
            facts: &factset(CUT),
            intent: None,
            verify: Some(SignatureVerifier {
                key_id: 1,
                key: b"test-key",
            }),
        });
        assert_eq!(
            err,
            Err(CompileError::BundleRolledBack { found: 5, min: 6 })
        );
        // 签名缺失（require_signature=true）。
        let unsigned = {
            let mut ub = RuleBundle::builder([9; 16], 5)
                .validity_window(0, u64::MAX)
                .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
                .site_rating_kw(100.0);
            for r in base_rules() {
                ub = ub.add_constraint(r);
            }
            ub = ub.add_term(econ_term(10, layer::ECONOMIC));
            ub.build().unwrap()
        };
        assert_eq!(
            compile(&unsigned, &factset(CUT)),
            Err(CompileError::BundleSignatureRequired)
        );
        // 错 key → 签名无效。
        let err = compile(&b, &factset(CUT));
        let _ = err; // 正确 key 已在主路径验证
        let bad_verify = ObjectiveCompiler::new(60_000_000_000, 1, true);
        let r = bad_verify.compile(CompileInputs {
            snapshot: &snapshot(),
            registry_generation: 1,
            bundle: &b,
            facts: &factset(CUT),
            intent: None,
            verify: Some(SignatureVerifier {
                key_id: 1,
                key: b"wrong",
            }),
        });
        assert_eq!(r, Err(CompileError::BundleSignatureInvalid));
    }

    /// 变异：删除安全约束（层 1）→ 必备清单拦截 + 摘要必变。
    #[test]
    fn mutation_removed_safety_constraint_detected() {
        let full = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let out_full = compile(&full, &factset(CUT)).unwrap();
        // 变异 1：删 MinSoc（id 1）但必备清单仍要求 → builder 拒绝
        // （manifest 引用不存在的 id）。
        let rebuilt = RuleBundle::builder([9; 16], 5)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(base_rules()[1].clone())
            .add_constraint(base_rules()[2].clone())
            .add_term(econ_term(10, layer::ECONOMIC))
            .required(&[1, 2])
            .build();
        assert!(rebuilt.is_err(), "必备清单引用不存在的 id 必须被拒绝");
        // 变异 2：manifest 同步删除（连带变异）→ digest 必变 + 缺 MinSoc
        // 可由消费者检测（spec 无 MinSoc 硬约束）。
        let mutated_bundle = RuleBundle::builder([9; 16], 5)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(base_rules()[1].clone())
            .add_constraint(base_rules()[2].clone())
            .add_term(econ_term(10, layer::ECONOMIC))
            .required(&[2])
            .build()
            .unwrap();
        let mut m = mutated_bundle;
        m.seal(1, b"test-key");
        let out_mut = compile(&m, &factset(CUT)).unwrap();
        assert_ne!(
            out_full.spec_digest, out_mut.spec_digest,
            "删除安全约束必须改变编译摘要"
        );
        assert!(!out_mut
            .spec
            .hard()
            .iter()
            .any(|c| c.kind.code() == K::MinSoc.code()));
    }

    /// 第 7 层（未批准建议）规则/目标项一律拒绝。
    #[test]
    fn layer7_unapproved_rejected() {
        let rules = base_rules();
        let rules_with_l7 = {
            let mut v = rules.clone();
            v.push(rule(
                9,
                K::MaxPowerKw,
                0,
                layer::UNAPPROVED_ADVICE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                -200.0,
                200.0,
            ));
            v
        };
        let b = signed_bundle(rules_with_l7, vec![econ_term(10, layer::ECONOMIC)]);
        assert_eq!(
            compile(&b, &factset(CUT)),
            Err(CompileError::UnapprovedLayer)
        );
        // 目标项层 7。
        let b = signed_bundle(rules, vec![econ_term(10, layer::UNAPPROVED_ADVICE)]);
        assert_eq!(
            compile(&b, &factset(CUT)),
            Err(CompileError::UnapprovedLayer)
        );
    }

    /// 规则过期（active window 不覆盖地平线）→ 规则不激活；
    /// 必备清单缺失 → MissingRequiredConstraint。
    #[test]
    fn expired_rules_and_required_manifest() {
        // MaxPower 规则只活到 cut（地平线 96 步 → 未覆盖）→ 激活集少一条；
        // 非 manifest 成员 → 只是约束变少（仍编译成功但无 MaxPowerKw）。
        let mut rules = base_rules();
        rules.push(
            ConstraintSpec::new(
                3,
                K::MaxPowerKw.code(),
                0,
                layer::GRID_CODE,
                ScopeRef::SITE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                -100.0,
                100.0,
                0,
                CUT, // 恰好到 cut，不覆盖地平线
                [1; 16],
            )
            .unwrap(),
        );
        // 同 id 两条 → builder 拒绝重复；改成不同 id：上面 push 与 base 里 id=3 冲突，
        // 因此改用独立集合：
        let rules = vec![
            rules[0].clone(),
            rules[1].clone(),
            ConstraintSpec::new(
                8,
                K::MaxPowerKw.code(),
                0,
                layer::GRID_CODE,
                ScopeRef::SITE,
                dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
                -100.0,
                100.0,
                0,
                CUT,
                [1; 16],
            )
            .unwrap(),
        ];
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        let out = compile(&b, &factset(CUT)).unwrap();
        assert!(
            !out.spec
                .hard()
                .iter()
                .any(|c| c.kind.code() == K::MaxPowerKw.code()),
            "过期规则不得进入 spec"
        );
        // 必备规则过期 → MissingRequiredConstraint。
        let b = RuleBundle::builder([9; 16], 5)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(
                ConstraintSpec::new(
                    101,
                    K::MinSoc.code(),
                    0,
                    layer::DEVICE_SAFETY,
                    ScopeRef::SITE,
                    dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                    0.1,
                    0.1,
                    0,
                    CUT, // 过期
                    [1; 16],
                )
                .unwrap(),
            )
            .add_constraint(base_rules()[1].clone())
            .add_term(econ_term(10, layer::ECONOMIC))
            .required(&[101])
            .build()
            .unwrap();
        let mut b = b;
        b.seal(1, b"test-key");
        assert_eq!(
            compile(&b, &factset(CUT)),
            Err(CompileError::MissingRequiredConstraint(101))
        );
    }

    /// 维护窗与地平线相交 → 拒绝编译（保守不规划）。
    #[test]
    fn maintenance_window_blocks_horizon() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let mut f = factset(CUT);
        f.enterprise.maintenance_windows_ns = vec![(CUT + 1, CUT + 2)];
        assert_eq!(
            compile(&b, &f),
            Err(CompileError::WindowNotCovered("maintenance"))
        );
        // 不相交（地平线之前结束）→ 通过。
        let mut f = factset(CUT);
        f.enterprise.maintenance_windows_ns = vec![(0, CUT - 1)];
        assert!(compile(&b, &f).is_ok());
    }

    /// 软目标归一化 + 软约束随编译输出（权重不动硬约束——类型分离）。
    #[test]
    fn soft_terms_and_constraints_flow() {
        let mut rules = base_rules();
        // 软功率上限 80kW（层 6 经济软项）。
        rules.push(rule(
            20,
            K::MaxPowerKw,
            1,
            layer::ECONOMIC,
            dim(eneros_telemetry_model::PhysicalUnit::Kilowatts),
            -80.0,
            80.0,
        ));
        let b = signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)]);
        let out = compile(&b, &factset(CUT)).unwrap();
        assert_eq!(out.spec.soft().len(), 1);
        assert!((out.spec.soft()[0].bound - 80.0).abs() < 1e-12);
        // 硬 MaxPower 100 不受软项影响。
        let hard_max = out
            .spec
            .hard()
            .iter()
            .find(|c| c.kind.code() == K::MaxPowerKw.code())
            .unwrap();
        assert!((hard_max.bound - 100.0).abs() < 1e-12, "软项不买掉硬约束");
    }

    /// 无合同可映射主目标 → 拒绝（不可静默选目标）。
    #[test]
    fn no_mappable_objective_rejected() {
        let b = signed_bundle(base_rules(), vec![]);
        assert_eq!(
            compile(&b, &factset(CUT)),
            Err(CompileError::InvalidRule(
                "no contract-mappable objective term"
            ))
        );
    }

    /// 红测：manifest 成员被翻转成软约束 → 编译期拦截（id 存在不足，
    /// 硬度必须为硬——安全底线不可一位翻转降级）。
    #[test]
    fn manifest_member_softened_rejected() {
        // 层 1 MinSoc 保持 id=1，但 hardness=1（软）。
        let rules = vec![
            rule(
                1,
                K::MinSoc,
                1,
                layer::DEVICE_SAFETY,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.1,
                0.1,
            ),
            base_rules()[1].clone(),
            base_rules()[2].clone(),
        ];
        let mut bb = RuleBundle::builder([9; 16], 5)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0);
        for c in rules {
            bb = bb.add_constraint(c);
        }
        let mut b = bb
            .add_term(econ_term(10, layer::ECONOMIC))
            .required(&[1])
            .build()
            .unwrap();
        b.seal(1, b"test-key");
        assert_eq!(
            compile(&b, &factset(CUT)),
            Err(CompileError::MissingRequiredConstraint(1)),
            "manifest 成员翻软必须被拦（不是只查 id 存在）"
        );
    }

    /// 红测：空价格 + 经济目标 → 拒绝（无价格 = 经济项不可用）。
    #[test]
    fn economic_objective_without_prices_rejected() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let mut f = factset(CUT);
        f.market.prices_per_kwh = vec![];
        assert_eq!(
            compile(&b, &f),
            Err(CompileError::InvalidRule(
                "economic objective requires prices"
            )),
            "空价格编译出 MaxRevenue = 任意调度皆最优（fail-open）"
        );
    }

    /// 红测：override 与既有 SOC 窗矛盾 → 拒绝（不可产出 MinSoc > MaxSoc
    /// 的不可行 spec）。
    #[test]
    fn operator_override_inverting_window_rejected() {
        let b = signed_bundle(base_rules(), vec![econ_term(10, layer::ECONOMIC)]);
        let mut f = factset(CUT);
        // 既有窗 [0.1, 0.9]；override (0.95, 1.0) → MinSoc 收紧 0.95 > 0.9。
        f.enterprise.operator_override_soc = Some((0.95, 1.0));
        assert_eq!(
            compile(&b, &f),
            Err(CompileError::InvalidRule(
                "operator_override_soc inverts soc window"
            ))
        );
        // 合法收紧（不倒窗）仍通过。
        let mut f = factset(CUT);
        f.enterprise.operator_override_soc = Some((0.2, 0.8));
        assert!(compile(&b, &f).is_ok());
    }

    /// 跨类一致性：终值 SOC 点落在窗外 → 拒绝（点类(2) vs 范围类(1)
    /// 分组不同，求交不覆盖——不可行组合零产出而非流给 solver）。
    #[test]
    fn soc_final_outside_window_rejected() {
        let below = {
            let mut rules = base_rules();
            rules.push(rule(
                9,
                K::SocFinal,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.05,
                0.05,
            ));
            signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)])
        };
        assert_eq!(
            compile(&below, &factset(CUT)),
            Err(CompileError::InvalidRule("soc_final below soc window")),
            "SocFinal 0.05 < MinSoc 0.1 必须编译期拦截"
        );
        // 窗内（0.5 ∈ [0.1, 0.9]）仍通过。
        let inside = {
            let mut rules = base_rules();
            rules.push(rule(
                9,
                K::SocFinal,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.5,
                0.5,
            ));
            signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)])
        };
        assert!(compile(&inside, &factset(CUT)).is_ok());
        // 窗上方同理。
        let above = {
            let mut rules = base_rules();
            rules.push(rule(
                9,
                K::SocFinal,
                0,
                layer::COMMITMENT,
                dim(eneros_telemetry_model::PhysicalUnit::Dimensionless),
                0.95,
                0.95,
            ));
            signed_bundle(rules, vec![econ_term(10, layer::ECONOMIC)])
        };
        assert_eq!(
            compile(&above, &factset(CUT)),
            Err(CompileError::InvalidRule("soc_final above soc window"))
        );
    }
}
