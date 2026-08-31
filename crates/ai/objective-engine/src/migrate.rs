//! 迁移与 shadow（[09] §2.6 步骤 3/6）：现役单储能固定参数
//! （energy-lp-model::ScheduleConfig）→ RuleBundle；编译产物与旧 LP 输入
//! **shadow 对比**——只对比，不改旧执行路径。

use eneros_energy_lp_model::config::ScheduleConfig;

use crate::bundle::RuleBundle;
use crate::compiler::SignatureVerifier;
use crate::spec::{ConstraintSpec, FactSet, HorizonPlan, MarketFacts, ObjectiveTerm, ScopeRef};

/// 规则 id 段（迁移规则固定段位；golden 测试锁定）。
pub mod ids {
    pub const SOC_MIN: u64 = 101;
    pub const SOC_MAX: u64 = 102;
    pub const PCS_MAX_POWER: u64 = 103;
    pub const CHARGE_RAMP: u64 = 104;
    pub const DISCHARGE_RAMP: u64 = 105;
}

/// 迁移束签名 key_id（站内规则签发；host 装配注入真 key）。
pub const LEGACY_KEY_ID: u16 = 1;

/// legacy ScheduleConfig → 签名 RuleBundle（[09] §2.6 步骤 3）。
///
/// 语义映射（等价性由 shadow 测试锁定）：
/// - `soc_min/soc_max` → MinSoc/MaxSoc（比率，层 1 设备安全）
/// - `pcs_power_kw` → MaxPowerKw（kW，层 2 电气边界；充电/放电共用 PCS 界）
/// - `charge_ramp_kw/period` → RampUpKwS；`discharge_ramp_kw/period` →
///   RampDownKwS（kW/步 ÷ 步长秒 → kW/s，层 2）
/// - `period_hours/num_periods` → HorizonPlan（步长 ns + 步数）
/// - `price` → FactSet.prices_per_kwh（经济输入随编译输出携带）
pub fn legacy_rule_bundle(cfg: &ScheduleConfig, issuer: [u8; 16], version: u64) -> RuleBundle {
    use eneros_decision_contract::objective::ConstraintKind as K;
    let step_ns = (cfg.period_hours * 3.600_000_000e12) as u64;
    let step_s = step_ns / 1_000_000_000;
    let unit_dim = eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Dimensionless,
    );
    let unit_kw = eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Kilowatts,
    );
    let rule = |id: u64, k: u16, layer: u16, unit: u16, lo: f64, hi: f64| {
        ConstraintSpec::new(
            id,
            k,
            0,
            layer,
            ScopeRef::SITE,
            unit,
            lo,
            hi,
            0,
            u64::MAX,
            [1; 16],
        )
        .expect("legacy rule valid")
    };
    let mut bundle = RuleBundle::builder(issuer, version)
        .validity_window(0, u64::MAX)
        .horizon(HorizonPlan::new(step_ns, cfg.num_periods.min(384) as u32).expect("horizon"))
        .site_rating_kw(cfg.pcs_power_kw)
        .add_constraint(rule(
            ids::SOC_MIN,
            K::MinSoc.code(),
            crate::layer::DEVICE_SAFETY,
            unit_dim,
            cfg.soc_min,
            cfg.soc_min,
        ))
        .add_constraint(rule(
            ids::SOC_MAX,
            K::MaxSoc.code(),
            crate::layer::DEVICE_SAFETY,
            unit_dim,
            cfg.soc_max,
            cfg.soc_max,
        ))
        .add_constraint(rule(
            ids::PCS_MAX_POWER,
            K::MaxPowerKw.code(),
            crate::layer::GRID_CODE,
            unit_kw,
            -cfg.pcs_power_kw,
            cfg.pcs_power_kw,
        ))
        .add_constraint(rule(
            ids::CHARGE_RAMP,
            K::RampUpKwS.code(),
            crate::layer::GRID_CODE,
            unit_kw,
            0.0,
            cfg.charge_ramp_kw / step_s as f64,
        ))
        .add_constraint(rule(
            ids::DISCHARGE_RAMP,
            K::RampDownKwS.code(),
            crate::layer::GRID_CODE,
            unit_kw,
            0.0,
            cfg.discharge_ramp_kw / step_s as f64,
        ))
        .required(&[ids::SOC_MIN, ids::SOC_MAX]);
    // 经济目标项（价格驱动收益最大化——legacy 模型唯一目标）。
    if !cfg.price.is_empty() {
        let unit_dim = eneros_telemetry_model::unit_catalog::code_of(
            eneros_telemetry_model::PhysicalUnit::Dimensionless,
        );
        bundle = bundle.add_term(
            ObjectiveTerm::new(
                201,
                crate::term_kind::ECONOMIC,
                1.0,
                unit_dim,
                crate::normalization::PRICE_SCALED,
                crate::layer::ECONOMIC,
                [1; 16],
            )
            .expect("legacy term valid"),
        );
    }
    let mut b = bundle.build().expect("legacy bundle valid");
    b.seal(LEGACY_KEY_ID, legacy_signing_key());
    b
}

/// legacy 事实集（价格透传；[09] §2.6 步骤 5 输入侧）。
pub fn legacy_fact_set(cfg: &ScheduleConfig, as_of_ns: u64) -> FactSet {
    FactSet {
        as_of_ns,
        market: MarketFacts {
            prices_per_kwh: cfg.price.clone(),
            source_event_id: [1; 16],
        },
        process: crate::spec::ProcessFacts::default(),
        enterprise: crate::spec::EnterpriseFacts::default(),
    }
}

/// 迁移签发 key（host 内嵌；真密钥装配随 Phase 6 业务接入替换）。
pub fn legacy_signing_key() -> &'static [u8] {
    b"eneros.legacy-rule-bundle.v1"
}

/// 迁移验签材料。
pub fn legacy_verifier() -> SignatureVerifier<'static> {
    SignatureVerifier {
        key_id: LEGACY_KEY_ID,
        key: legacy_signing_key(),
    }
}

/// shadow 对比结论（只对比，不执行）。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowReport {
    /// SOC 界一致。
    pub soc_bounds_match: bool,
    /// 功率上界一致（kW）。
    pub power_cap_match: bool,
    /// 爬坡一致（kW/步换算后）。
    pub ramp_match: bool,
    /// 目标方向一致（收益最大化）。
    pub objective_match: bool,
    /// 旧模型等效硬约束集合（kW/比率/每步），供诊断。
    pub legacy_effective: LegacyEffective,
}

/// 旧模型等效参数（从 ScheduleConfig 直接推导——对比基准）。
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyEffective {
    pub soc_min: f64,
    pub soc_max: f64,
    pub max_power_kw: f64,
    pub ramp_up_kw_per_step: f64,
    pub ramp_down_kw_per_step: f64,
}

impl LegacyEffective {
    pub fn from_config(cfg: &ScheduleConfig) -> Self {
        Self {
            soc_min: cfg.soc_min,
            soc_max: cfg.soc_max,
            max_power_kw: cfg.pcs_power_kw,
            ramp_up_kw_per_step: cfg.charge_ramp_kw,
            ramp_down_kw_per_step: cfg.discharge_ramp_kw,
        }
    }
}

/// shadow 对比：编译产物 vs 旧模型参数（[09] §2.6 步骤 6；不执行）。
pub fn shadow_compare(
    compiled: &crate::compiler::CompiledObjective,
    cfg: &ScheduleConfig,
) -> ShadowReport {
    let legacy = LegacyEffective::from_config(cfg);
    let step_s = compiled.horizon_step_ns / 1_000_000_000;
    use eneros_decision_contract::objective::ConstraintKind as K;
    let get = |code: u16| {
        compiled
            .spec
            .hard()
            .iter()
            .find(|c| c.kind.code() == code)
            .map(|c| c.bound)
    };
    let soc_min = get(K::MinSoc.code());
    let soc_max = get(K::MaxSoc.code());
    let power = get(K::MaxPowerKw.code());
    let ramp_up = get(K::RampUpKwS.code());
    let ramp_down = get(K::RampDownKwS.code());
    ShadowReport {
        soc_bounds_match: soc_min == Some(legacy.soc_min) && soc_max == Some(legacy.soc_max),
        power_cap_match: power == Some(legacy.max_power_kw),
        ramp_match: ramp_up
            .is_some_and(|v| (v * step_s as f64 - legacy.ramp_up_kw_per_step).abs() < 1e-9)
            && ramp_down
                .is_some_and(|v| (v * step_s as f64 - legacy.ramp_down_kw_per_step).abs() < 1e-9),
        objective_match: compiled.spec.objective()
            == eneros_decision_contract::objective::ObjectiveKind::MaxRevenue,
        legacy_effective: legacy,
    }
}

/// shadow 判定（全 true = 迁移等价）。
pub fn shadow_equivalent(report: &ShadowReport) -> bool {
    report.soc_bounds_match && report.power_cap_match && report.ramp_match && report.objective_match
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use alloc::vec;

    use eneros_snapshot_service::TrustedSnapshot;

    use super::*;
    use crate::compiler::{CompileInputs, ObjectiveCompiler};

    fn snapshot(cut_ns: u64) -> TrustedSnapshot {
        TrustedSnapshot {
            snapshot_id: [7; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: cut_ns,
            produced_at_ns: cut_ns,
            policy_version: 1,
            version_set: eneros_semantic_catalog::VersionSet {
                generation: 1,
                mapping_version: 1,
                topology_version: 1,
                asset_version: 1,
                contract_version: 1,
                safety_version: 1,
            },
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

    /// 迁移编译 → shadow 全等（[09] §2.6 步骤 3/6 判据）。
    #[test]
    fn legacy_migration_shadow_equivalent() {
        let cfg = ScheduleConfig::default();
        let bundle = legacy_rule_bundle(&cfg, [9; 16], 1);
        let compiler = ObjectiveCompiler::new(60_000_000_000, 1, true);
        let cut = 1_000_000_000_000u64;
        let out = compiler
            .compile(CompileInputs {
                snapshot: &snapshot(cut),
                registry_generation: 1,
                bundle: &bundle,
                facts: &legacy_fact_set(&cfg, cut),
                intent: None,
                verify: Some(legacy_verifier()),
            })
            .expect("compile legacy");
        let report = shadow_compare(&out, &cfg);
        assert!(shadow_equivalent(&report), "{report:?}");
        // 必备安全约束在产物中（MinSoc/MaxSoc 硬约束存在）。
        use eneros_decision_contract::objective::ConstraintKind as K;
        assert!(out
            .spec
            .hard()
            .iter()
            .any(|c| c.kind.code() == K::MinSoc.code()));
        // 价格广播（常数价 0.5 × 96 步）。
        assert_eq!(out.prices_per_kwh.len(), 96);
        assert!(out.prices_per_kwh.iter().all(|&p| (p - 0.5).abs() < 1e-12));
        // spec_digest 稳定（同输入重复编译一致）。
        let out2 = compiler
            .compile(CompileInputs {
                snapshot: &snapshot(cut),
                registry_generation: 1,
                bundle: &bundle,
                facts: &legacy_fact_set(&cfg, cut),
                intent: None,
                verify: Some(legacy_verifier()),
            })
            .unwrap();
        assert_eq!(out.spec_digest, out2.spec_digest);
    }
}
