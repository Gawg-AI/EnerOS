//! planner 集成测试（[09] §3.7/§3.8）：golden 手算、不可行、求解门
//! 负例、负荷入平衡、承诺行、确定性、热启动、差分、规模。

// 测试文件整体豁免 panic! 禁令（测试断言需要）。
#![allow(clippy::disallowed_macros)]

use eneros_decision_contract::objective::{
    Constraint, ConstraintKind, ObjectiveKind, ObjectiveSource,
};
use eneros_decision_contract::CandidatePlan;
use eneros_decision_contract::{
    DecisionRequest, ObjectiveSpec, ReasonCode, SnapshotId, SolveStatusKind,
};
use eneros_objective_engine::compiler::digest_compiled;
use eneros_objective_engine::CompiledObjective;
use eneros_planner::builder::{
    BatterySpec, DispatchSpec, GridSpec, LoadSpec, ModelBuilder, PlanInput, RenewableSpec,
};
use eneros_planner::tolerance::ToleranceProfile;
use eneros_planner::{AssetBinding, PlanError, Planner};
use eneros_resource_registry::capability::{Availability, CapabilityEnvelope, DeratingReason};
use eneros_solver_core::error::SolverError;
use eneros_solver_core::problem::LpProblem;
use eneros_solver_core::result::{SolveResult, SolveStatus};
use eneros_solver_core::solver::Solver;

const STEP_NS_HOUR: u64 = 3_600_000_000_000;

fn env_pcc() -> CapabilityEnvelope {
    CapabilityEnvelope {
        asset_id: 10,
        valid_from_ns: 0,
        valid_until_ns: u64::MAX,
        p_min_kw: -20.0,
        p_max_kw: 20.0,
        q_min_kvar: 0.0,
        q_max_kvar: 0.0,
        energy_min_kwh: 0.0,
        energy_max_kwh: 0.0,
        ramp_up_kw_s: 1000.0,
        ramp_down_kw_s: 1000.0,
        min_on_ns: 0,
        min_off_ns: 0,
        availability: Availability::Full,
        derating_reason: DeratingReason::None,
        source_snapshot_id: [1; 16],
        tightenings: Vec::new(),
        generation: 1,
    }
}

fn env_battery() -> CapabilityEnvelope {
    CapabilityEnvelope {
        asset_id: 1,
        p_min_kw: -5.0,
        p_max_kw: 5.0,
        energy_min_kwh: 0.0,
        energy_max_kwh: 10.0,
        ramp_up_kw_s: 1000.0,
        ramp_down_kw_s: 1000.0,
        ..env_pcc()
    }
}

fn spec(hard: Vec<Constraint>, objective: ObjectiveKind) -> ObjectiveSpec {
    ObjectiveSpec::new(
        SnapshotId([7; 16]),
        hard,
        Vec::new(),
        objective,
        1,
        ObjectiveSource::RuleBundle,
        ReasonCode::PriceArbitrage,
        0,
        u64::MAX,
    )
    .unwrap()
}

fn compiled(spec: ObjectiveSpec, prices: Vec<f64>, t: u32) -> CompiledObjective {
    let c = CompiledObjective {
        spec,
        prices_per_kwh: prices,
        min_on_steps: 0,
        min_off_steps: 0,
        registry_generation: 1,
        bundle_version: 1,
        bundle_digest: [9; 32],
        spec_digest: [0; 32],
        horizon_step_ns: STEP_NS_HOUR,
        horizon_step_count: t,
    };
    let d = digest_compiled(
        &c.spec,
        &c.prices_per_kwh,
        &c.bundle_digest,
        c.registry_generation,
        c.min_on_steps,
        c.min_off_steps,
        c.horizon_step_ns,
        c.horizon_step_count,
    );
    CompiledObjective {
        spec_digest: d,
        ..c
    }
}

fn soc_bounds() -> Vec<Constraint> {
    vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.1,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.9,
        },
    ]
}

/// 标准两步场景：储能(1) + 并网点(10)，价格自定。
fn scenario<'a>(
    prices: Vec<f64>,
    hard: Vec<Constraint>,
    kind: ObjectiveKind,
) -> (CompiledObjective, PlanInput<'a>) {
    let obj = compiled(spec(hard, kind), prices, 2);
    let leaked: &'a CompiledObjective = Box::leak(Box::new(obj.clone()));
    (obj, base_input(leaked))
}

fn base_input(objective: &CompiledObjective) -> PlanInput<'_> {
    PlanInput {
        objective,
        snapshot_id: [7; 16],
        provenance_root: [0xAB; 32],
        registry_generation: 1,
        now_ms: 1_000,
        site_node_id: 1,
        envelopes: vec![(1, env_battery()), (10, env_pcc())],
        batteries: vec![BatterySpec {
            asset_id: 1,
            capacity_kwh: 10.0,
            eta_c: 1.0,
            eta_d: 1.0,
            initial_kwh: 5.0,
        }],
        renewables: vec![],
        loads: vec![],
        grid: Some(GridSpec { asset_id: 10 }),
        dispatchable: vec![],
        bindings: vec![
            AssetBinding {
                asset_id: 1,
                device_id: 2,
                point_id: 6,
                unit_code: 18,
            },
            AssetBinding {
                asset_id: 10,
                device_id: 3,
                point_id: 1,
                unit_code: 6,
            },
        ],
    }
}

fn request(obj_spec: &ObjectiveSpec) -> DecisionRequest {
    DecisionRequest::new([1u8; 16], obj_spec.clone(), 1, 10_000, 1).unwrap()
}

fn highs() -> eneros_solver_core::highs::HighsSolver {
    eneros_solver_core::highs::HighsSolver::new().expect("Highs create")
}

/// 重放型 scripted backend（gate 负例；等价 E1/E2 用法）。
struct ScriptedSolver {
    result: Option<SolveResult>,
    err: Option<SolverError>,
    warm_starts: Vec<Vec<f64>>,
}

impl ScriptedSolver {
    fn replay(r: SolveResult) -> Self {
        Self {
            result: Some(r),
            err: None,
            warm_starts: Vec::new(),
        }
    }
    fn error(e: SolverError) -> Self {
        Self {
            result: None,
            err: Some(e),
            warm_starts: Vec::new(),
        }
    }
}

impl Solver for ScriptedSolver {
    fn solve(&mut self, _p: &LpProblem, _now: u64) -> Result<SolveResult, SolverError> {
        if let Some(e) = self.err.take() {
            return Err(e);
        }
        Ok(self.result.as_ref().expect("scripted result").clone())
    }
    fn name(&self) -> &'static str {
        "Scripted"
    }
    fn version(&self) -> &'static str {
        "0"
    }
    fn set_param(&mut self, _k: &str, _v: &str) -> Result<(), SolverError> {
        Ok(())
    }
    fn status(&self) -> eneros_solver_core::solver::SolverStatus {
        eneros_solver_core::solver::SolverStatus::Idle
    }
    fn set_warm_start(&mut self, s: &[f64]) -> Result<(), SolverError> {
        self.warm_starts.push(Vec::from(s));
        Ok(())
    }
}

fn bat_assignments(plan: &CandidatePlan) -> Vec<(u32, f64)> {
    plan.assignments()
        .iter()
        .filter(|a| a.asset_id == 1)
        .map(|a| (a.step_index, a.power_kw))
        .collect()
}

// ---------------------------------------------------------------------------
// golden：手算两步套利（[09] §3.8 可手算小模型）
// ---------------------------------------------------------------------------

#[test]
fn golden_two_step_battery_arbitrage() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    // 手算最优：终值 SOC ≥ 1kWh → 初始 5kWh 中 4kWh 免费；
    // t1 放 5 需 soc1 ≥ 6 → t0 充 1（价 1）。obj = 50 − 1 = 49。
    assert!(
        (plan.objective_value() - 49.0).abs() < 1e-6,
        "obj={}",
        plan.objective_value()
    );
    assert_eq!(plan.solver_status(), SolveStatusKind::Optimal);
    assert!(plan.relative_gap() < 1e-9);
    assert!(plan.max_primal_residual() < 1e-6);
    let bat = bat_assignments(&plan);
    let t0 = bat.iter().find(|(t, _)| *t == 1).unwrap();
    assert!((t0.1 + 1.0).abs() < 1e-6, "t0 净充电 −1：{bat:?}");
    let t1 = bat.iter().find(|(t, _)| *t == 2).unwrap();
    assert!((t1.1 - 5.0).abs() < 1e-6, "t1 净放电 +5");
    // grid 净 = [4, −5]。
    let grid: Vec<(u32, f64)> = plan
        .assignments()
        .iter()
        .filter(|a| a.asset_id == 10)
        .map(|a| (a.step_index, a.power_kw))
        .collect();
    let g1 = grid.iter().find(|(t, _)| *t == 1).unwrap();
    assert!((g1.1 - 1.0).abs() < 1e-6);
    let g2 = grid.iter().find(|(t, _)| *t == 2).unwrap();
    assert!((g2.1 + 5.0).abs() < 1e-6);
    // 证据链：input_digest = provenance；objective_digest = spec_digest。
    assert_eq!(plan.input_digest(), &[0xAB; 32]);
    assert_eq!(plan.objective_digest(), &obj.spec_digest);
}

#[test]
fn infeasible_spec_rejected() {
    // MinSoc 1.0 → SOC 窗冻结在 10kWh，初值 5 → 不可行。
    let hard = vec![Constraint {
        kind: ConstraintKind::MinSoc,
        bound: 1.0,
    }];
    let (obj, input) = scenario(vec![1.0, 10.0], hard, ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    match p.plan(&mut hb, &req, &input) {
        Err(PlanError::Rejected(m)) => assert!(m.contains("infeasible"), "{m}"),
        other => panic!("expected rejected, got {other:?}"),
    }
}

#[test]
fn load_demand_enters_balance() {
    // SOC 冻结在初值（Min=Max=0.5）→ 负荷全部由电网买。
    let hard = vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.5,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.5,
        },
    ];
    let obj = compiled(spec(hard, ObjectiveKind::MinCost), vec![2.0; 2], 2);
    let mut input = base_input(&obj);
    // 负荷资产同样需要包络（certifier 对轨迹内每个资产都要求）。
    let mut load_env = env_pcc();
    load_env.asset_id = 20;
    input.envelopes.push((20, load_env));
    input.loads = vec![LoadSpec {
        asset_id: 20,
        baseline_kw: vec![10.0, 10.0],
        shed_max_kw: 0.0,
    }];
    input.bindings.push(AssetBinding {
        asset_id: 20,
        device_id: 4,
        point_id: 2,
        unit_code: 6,
    });
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    // MinCost = 2 元/kWh × 10kW × 1h × 2 步 = 40。
    assert!(
        (plan.objective_value() - 40.0).abs() < 1e-6,
        "obj={}",
        plan.objective_value()
    );
    let buy = plan
        .assignments()
        .iter()
        .find(|a| a.asset_id == 10 && a.step_index == 1)
        .unwrap();
    assert!((buy.power_kw - 10.0).abs() < 1e-6, "负荷实际进入平衡行");
}

#[test]
fn energy_commitment_forces_sell() {
    let mut hard = soc_bounds();
    hard.push(Constraint {
        kind: ConstraintKind::EnergyCommitmentKwh,
        bound: 4.0,
    });
    let (obj, input) = scenario(vec![1.0, 10.0], hard, ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    let sold: f64 = plan
        .assignments()
        .iter()
        .filter(|a| a.asset_id == 10)
        .map(|a| -a.power_kw.min(0.0))
        .sum();
    assert!((sold - 4.0).abs() < 1e-6, "承诺 4kWh，实际 {sold}");
}

#[test]
fn renewable_curtailment_self_consume() {
    let obj = compiled(spec(soc_bounds(), ObjectiveKind::SelfConsume), vec![], 2);
    let mut input = base_input(&obj);
    let mut pv_env = env_pcc();
    pv_env.asset_id = 30;
    pv_env.p_min_kw = 0.0;
    input.envelopes.push((30, pv_env));
    input.renewables = vec![RenewableSpec {
        asset_id: 30,
        available_kw: vec![8.0, 8.0],
    }];
    input.bindings.push(AssetBinding {
        asset_id: 30,
        device_id: 5,
        point_id: 3,
        unit_code: 6,
    });
    // 负荷 3 + 电池充 5 → 全消纳（curt=0）。
    let mut load_env = env_pcc();
    load_env.asset_id = 20;
    input.envelopes.push((20, load_env));
    input.loads = vec![LoadSpec {
        asset_id: 20,
        baseline_kw: vec![3.0, 3.0],
        shed_max_kw: 0.0,
    }];
    input.bindings.push(AssetBinding {
        asset_id: 20,
        device_id: 4,
        point_id: 2,
        unit_code: 6,
    });
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    // SelfConsume = min Σ curtail·dt → 0（全部消纳：t0 充 5 + 负 3 + 售/储 0）。
    assert!(
        (plan.objective_value() - 0.0).abs() < 1e-6,
        "obj={}",
        plan.objective_value()
    );
    let pv: Vec<(u32, f64)> = plan
        .assignments()
        .iter()
        .filter(|a| a.asset_id == 30)
        .map(|a| (a.step_index, a.power_kw))
        .collect();
    let p1 = pv.iter().find(|(t, _)| *t == 1).unwrap();
    assert!((p1.1 - 8.0).abs() < 1e-6, "PV 全额注入（无弃电）");
}

// ---------------------------------------------------------------------------
// 确定性（[09] §3.8：顺序无关、解不漂移）
// ---------------------------------------------------------------------------

#[test]
fn determinism_shuffled_inputs_same_model() {
    let (obj, input_a) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut input_b = PlanInput {
        objective: &obj,
        envelopes: {
            let mut e = input_a.envelopes.clone();
            e.reverse();
            e
        },
        bindings: {
            let mut b = input_a.bindings.clone();
            b.reverse();
            b
        },
        ..base_input(&obj)
    };
    input_b.envelopes = {
        let mut e = input_a.envelopes.clone();
        e.reverse();
        e
    };
    input_b.bindings = {
        let mut b = input_a.bindings.clone();
        b.reverse();
        b
    };
    let built_a = ModelBuilder::build(&input_a).expect("build a");
    let built_b = ModelBuilder::build(&input_b).expect("build b");
    assert_eq!(built_a.model_digest, built_b.model_digest, "输入顺序无关");
    assert_eq!(built_a.catalog, built_b.catalog);
    let mut hb = highs();
    let r1 = hb.solve(&built_a.problem, 0).unwrap();
    let r2 = hb.solve(&built_b.problem, 0).unwrap();
    assert!((r1.objective_value - r2.objective_value).abs() < 1e-9);
    for (a, b) in r1.solution.iter().zip(&r2.solution) {
        assert!((a - b).abs() < 1e-6, "解漂移");
    }
}

// ---------------------------------------------------------------------------
// 热启动（[09] §3.6：同 digest 才注入）
// ---------------------------------------------------------------------------

#[test]
fn warm_start_injected_on_same_digest() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let built = ModelBuilder::build(&input).unwrap();
    let mut hb = highs();
    let real = hb.solve(&built.problem, 0).unwrap();
    let mut scripted = ScriptedSolver::replay(real);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let _ = p.plan(&mut scripted, &req, &input).unwrap();
    assert!(scripted.warm_starts.is_empty(), "首次无热启动");
    let _ = p.plan(&mut scripted, &req, &input).unwrap();
    assert_eq!(scripted.warm_starts.len(), 1, "同 digest 注入热启动");
    assert_eq!(scripted.warm_starts[0].len(), built.problem.variables.len());
}

// ---------------------------------------------------------------------------
// 求解门负例（红测 [09] §3.7 步骤 2）
// ---------------------------------------------------------------------------

#[test]
fn gate_rejects_short_and_nan_solutions() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let built = ModelBuilder::build(&input).unwrap();
    let mut hb = highs();
    let real = hb.solve(&built.problem, 0).unwrap();
    let n = real.solution.len();

    let mut short = real.clone();
    short.solution.truncate(n - 1);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    match p.plan(&mut ScriptedSolver::replay(short), &req, &input) {
        Err(PlanError::MalformedSolution("length mismatch")) => {}
        other => panic!("short: {other:?}"),
    }

    let mut nan = real;
    nan.solution[0] = f64::NAN;
    match p.plan(&mut ScriptedSolver::replay(nan), &req, &input) {
        Err(PlanError::MalformedSolution("non-finite value")) => {}
        other => panic!("nan: {other:?}"),
    }
}

#[test]
fn gate_rejects_backend_failures() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    match p.plan(
        &mut ScriptedSolver::error(SolverError::RunFailed(-7)),
        &req,
        &input,
    ) {
        Err(PlanError::Backend(_)) => {}
        other => panic!("backend: {other:?}"),
    }
    let mut s = ScriptedSolver::replay(SolveResult::optimal(0.0, vec![]));
    s.result.as_mut().unwrap().status = SolveStatus::Infeasible;
    match p.plan(&mut s, &req, &input) {
        Err(PlanError::Rejected(m)) => assert!(m.contains("infeasible")),
        other => panic!("infeasible: {other:?}"),
    }
    let mut s = ScriptedSolver::replay(SolveResult::optimal(0.0, vec![]));
    s.result.as_mut().unwrap().status = SolveStatus::Timeout;
    match p.plan(&mut s, &req, &input) {
        Err(PlanError::Rejected(m)) => assert!(m.contains("timeout")),
        other => panic!("timeout: {other:?}"),
    }
}

#[test]
fn l2_rejects_suboptimal_l3_accepts_within_gap() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let built = ModelBuilder::build(&input).unwrap();
    let mut hb = highs();
    let real = hb.solve(&built.problem, 0).unwrap();
    let mut sub = real.clone();
    sub.status = SolveStatus::Suboptimal;
    sub.mip_gap = Some(0.001);

    let mut p2 = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    match p2.plan(&mut ScriptedSolver::replay(sub.clone()), &req, &input) {
        Err(PlanError::GapExceeded { .. }) => {}
        other => panic!("l2 suboptimal: {other:?}"),
    }
    let mut p3 = Planner::new(ToleranceProfile::l3(), 100);
    let plan = p3
        .plan(&mut ScriptedSolver::replay(sub), &req, &input)
        .expect("l3");
    assert_eq!(plan.solver_status(), SolveStatusKind::Suboptimal);
    let mut bad = real;
    bad.status = SolveStatus::Suboptimal;
    bad.mip_gap = Some(0.01);
    match p3.plan(&mut ScriptedSolver::replay(bad), &req, &input) {
        Err(PlanError::GapExceeded { gap, limit }) => {
            assert!((gap - 0.01).abs() < 1e-12);
            assert!((limit - 0.005).abs() < 1e-12);
        }
        other => panic!("l3 gap: {other:?}"),
    }
}

#[test]
fn request_binding_enforced() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = DecisionRequest::new([1u8; 16], obj.spec.clone(), 99, 10_000, 1).unwrap();
    let mut hb = highs();
    match p.plan(&mut hb, &req, &input) {
        Err(PlanError::RequestMismatch("registry_generation")) => {}
        other => panic!("{other:?}"),
    }
    let mut input2 = PlanInput {
        snapshot_id: [8; 16],
        ..base_input(&obj)
    };
    let req = request(&obj.spec);
    match p.plan(&mut hb, &req, &input2) {
        Err(PlanError::RequestMismatch("snapshot_id")) => {}
        other => panic!("{other:?}"),
    }
    let _ = &mut input2;
}

// ---------------------------------------------------------------------------
// 差分（[09] §3.8：与当前单储能模型差分，发现而不是继承旧错误）
// ---------------------------------------------------------------------------

#[test]
fn differential_vs_legacy_energy_lp() {
    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_energy_lp_model::model::EnergyScheduleModel;
    let cfg = ScheduleConfig {
        num_periods: 2,
        period_hours: 1.0,
        pcs_power_kw: 5.0,
        battery_capacity_kwh: 10.0,
        soc_min: 0.1,
        soc_max: 0.9,
        soc_init: 0.5,
        soc_final: None,
        charge_ramp_kw: 50.0,
        discharge_ramp_kw: 50.0,
        charge_efficiency: 1.0,
        discharge_efficiency: 1.0,
        price: vec![1.0, 10.0],
        load_demand: None,
    };
    let legacy = EnergyScheduleModel::new(cfg);
    let lp = legacy.compile().unwrap();
    let mut hb = highs();
    let legacy_obj = hb.solve(&lp, 0).unwrap().objective_value;

    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    assert!(
        (plan.objective_value() - legacy_obj).abs() < 1e-6,
        "planner={} legacy={}",
        plan.objective_value(),
        legacy_obj
    );
}

// ---------------------------------------------------------------------------
// 规模（C1/C2）
// ---------------------------------------------------------------------------

#[test]
fn scale_build_1000_resources_96_steps() {
    let obj = compiled(
        spec(soc_bounds(), ObjectiveKind::MinCost),
        vec![1.0; 96],
        96,
    );
    let mut envs = vec![(10, env_pcc())];
    let mut disp = Vec::new();
    let mut binds = vec![AssetBinding {
        asset_id: 10,
        device_id: 3,
        point_id: 1,
        unit_code: 6,
    }];
    for id in 100..1100u64 {
        let mut e = env_pcc();
        e.asset_id = id;
        e.p_min_kw = -1.0;
        e.p_max_kw = 2.0;
        envs.push((id, e));
        disp.push(DispatchSpec { asset_id: id });
        binds.push(AssetBinding {
            asset_id: id,
            device_id: 5,
            point_id: 3,
            unit_code: 6,
        });
    }
    let input = PlanInput {
        objective: &obj,
        envelopes: envs,
        dispatchable: disp,
        bindings: binds,
        batteries: vec![],
        ..base_input(&obj)
    };
    let built = ModelBuilder::build(&input).expect("build");
    assert_eq!(
        built.catalog.len(),
        1000 * 96 + 3 * 96,
        "资源列 + 网格 3 列/步"
    );
    let again = ModelBuilder::build(&input).unwrap();
    assert_eq!(built.model_digest, again.model_digest);
}

#[test]
fn scale_solve_200x24_real_highs() {
    let obj = compiled(
        spec(soc_bounds(), ObjectiveKind::MinCost),
        vec![1.0; 24],
        24,
    );
    let mut envs = vec![(10, env_pcc())];
    let mut disp = Vec::new();
    let mut binds = vec![AssetBinding {
        asset_id: 10,
        device_id: 3,
        point_id: 1,
        unit_code: 6,
    }];
    for id in 100..300u64 {
        let mut e = env_pcc();
        e.asset_id = id;
        e.p_min_kw = -1.0;
        e.p_max_kw = 2.0;
        envs.push((id, e));
        disp.push(DispatchSpec { asset_id: id });
        binds.push(AssetBinding {
            asset_id: id,
            device_id: 5,
            point_id: 3,
            unit_code: 6,
        });
    }
    let input = PlanInput {
        objective: &obj,
        envelopes: {
            let mut e = envs;
            let mut load_env = env_pcc();
            load_env.asset_id = 20;
            // 净负荷 −100kW 需包络下界覆盖。
            load_env.p_min_kw = -150.0;
            load_env.p_max_kw = 150.0;
            e.push((20, load_env));
            e
        },
        dispatchable: disp,
        batteries: vec![],
        loads: vec![LoadSpec {
            asset_id: 20,
            baseline_kw: vec![100.0; 24],
            shed_max_kw: 0.0,
        }],
        bindings: {
            let mut b = binds;
            b.push(AssetBinding {
                asset_id: 20,
                device_id: 4,
                point_id: 2,
                unit_code: 6,
            });
            b
        },
        ..base_input(&obj)
    };
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let out = p.solve_document(&mut hb, &req, &input).expect("solve");
    assert_eq!(out.solver_status, SolveStatusKind::Optimal);
    // MinCost：buy=0 可达（资源可覆盖负荷）→ 目标 0。
    assert!(
        out.objective_value.abs() < 1e-4,
        "obj={}",
        out.objective_value
    );
}

// ---------------------------------------------------------------------------
// 复查修复：无界 / 数值病态（[09] §3.8 负例补全）
// ---------------------------------------------------------------------------

/// Unbounded：无并网点 + MaxRevenue 无价格锚 → 目标随放电无界增长。
/// 注意：储能放电受 SOC 守恒界——真正无界需要无守恒变量；此处用
/// ScriptedSolver 直接上报 Unbounded 状态，验证 gate 拒绝路径。
#[test]
fn gate_rejects_unbounded_status() {
    let (obj, input) = scenario(vec![1.0, 10.0], soc_bounds(), ObjectiveKind::MaxRevenue);
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut s = ScriptedSolver::replay(SolveResult::optimal(0.0, vec![]));
    s.result.as_mut().unwrap().status = SolveStatus::Unbounded;
    match p.plan(&mut s, &req, &input) {
        Err(PlanError::Rejected(m)) => assert!(m.contains("unbounded")),
        other => panic!("unbounded: {other:?}"),
    }
}

/// 数值病态：价格系数超 1e12 → 建模期阻断（[09] §3.6 condition 检查）。
#[test]
fn ill_conditioned_prices_rejected() {
    let obj = compiled(
        spec(soc_bounds(), ObjectiveKind::MaxRevenue),
        vec![1e13, 1e13],
        2,
    );
    let input = base_input(&obj);
    match ModelBuilder::build(&input) {
        Err(eneros_planner::builder::ModelError::IllConditioned(_)) => {}
        other => panic!("ill-conditioned: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 复查收编：新能源/负荷包络 ramp（planner 行与 certifier 第 3 节同语义）
// ---------------------------------------------------------------------------

/// PV 可用出力断崖（云切）快于包络 ramp：规划须提前 curtail 压出力。
#[test]
fn renewable_ramp_forces_preemptive_curtailment() {
    let obj = compiled(spec(soc_bounds(), ObjectiveKind::SelfConsume), vec![], 2);
    let mut input = base_input(&obj);
    let mut pv_env = env_pcc();
    pv_env.asset_id = 30;
    pv_env.p_min_kw = 0.0;
    pv_env.ramp_up_kw_s = 0.001; // 3.6 kW/步（dt=1h）
    pv_env.ramp_down_kw_s = 0.001;
    input.envelopes.push((30, pv_env));
    input.renewables = vec![RenewableSpec {
        asset_id: 30,
        available_kw: vec![8.0, 0.0], // t1 断崖到 0
    }];
    input.bindings.push(AssetBinding {
        asset_id: 30,
        device_id: 5,
        point_id: 3,
        unit_code: 6,
    });
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    // SelfConsume = min Σ curt·dt：ramp 迫使 curt_0 ≥ 8 − 3.6 = 4.4。
    assert!(
        (plan.objective_value() - 4.4).abs() < 1e-6,
        "obj={}",
        plan.objective_value()
    );
    let pv: Vec<(u32, f64)> = plan
        .assignments()
        .iter()
        .filter(|a| a.asset_id == 30)
        .map(|a| (a.step_index, a.power_kw))
        .collect();
    let p1 = pv.iter().find(|(t, _)| *t == 1).unwrap().1;
    let p2 = pv.iter().find(|(t, _)| *t == 2).unwrap().1;
    assert!(p2.abs() < 1e-6, "t1 可用 0 → 净 0");
    assert!(p1 <= 3.6 + 1e-6, "t0 净出力须提前压坡（{p1}）");
}

/// 固定负荷基线跳变超包络 ramp：建模期拒绝（任何计划都不可认证）。
#[test]
fn fixed_load_baseline_ramp_violation_rejected() {
    let obj = compiled(spec(soc_bounds(), ObjectiveKind::MinCost), vec![1.0; 2], 2);
    let mut input = base_input(&obj);
    let mut load_env = env_pcc();
    load_env.asset_id = 20;
    load_env.ramp_down_kw_s = 0.001; // 3.6 kW/步
    input.envelopes.push((20, load_env));
    input.loads = vec![LoadSpec {
        asset_id: 20,
        baseline_kw: vec![3.0, 20.0], // 跳 17 ≫ 3.6
        shed_max_kw: 0.0,
    }];
    input.bindings.push(AssetBinding {
        asset_id: 20,
        device_id: 4,
        point_id: 2,
        unit_code: 6,
    });
    match ModelBuilder::build(&input) {
        Err(eneros_planner::builder::ModelError::InvalidInput(
            "load baseline violates envelope ramp",
        )) => {}
        other => panic!("{other:?}"),
    }
}

/// 可削减负荷：ramp=0 冻结净负荷 → 削减量各步相等。
#[test]
fn shed_load_ramp_rows_freeze_net() {
    let obj = compiled(spec(soc_bounds(), ObjectiveKind::MinCost), vec![10.0; 2], 2);
    let mut input = base_input(&obj);
    let mut load_env = env_pcc();
    load_env.asset_id = 20;
    load_env.ramp_up_kw_s = 0.0; // 冻结
    load_env.ramp_down_kw_s = 0.0;
    input.envelopes.push((20, load_env));
    input.loads = vec![LoadSpec {
        asset_id: 20,
        baseline_kw: vec![10.0, 10.0],
        shed_max_kw: 5.0,
    }];
    input.bindings.push(AssetBinding {
        asset_id: 20,
        device_id: 4,
        point_id: 2,
        unit_code: 6,
    });
    let mut p = Planner::new(ToleranceProfile::l2(), 100);
    let req = request(&obj.spec);
    let mut hb = highs();
    let plan = p.plan(&mut hb, &req, &input).expect("plan");
    let shed: Vec<f64> = plan
        .assignments()
        .iter()
        .filter(|a| a.asset_id == 20)
        .map(|a| a.power_kw)
        .collect();
    assert_eq!(shed.len(), 2);
    assert!(
        (shed[0] - shed[1]).abs() < 1e-6,
        "ramp=0 冻结净负荷：{shed:?}"
    );
}
