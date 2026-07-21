//! EnerOS v0.88.0 多目标优化器.
//!
//! 在 v0.87.0 单目标（损耗最小）LP 调度基础上扩展为 4 目标（经济 / 电池寿命 / 安全 /
//! 碳排）优化：各目标成本系数归一化后按权重线性组合为单一 LP 目标并求解（`weighted`）；
//! 或通过确定性权重采样生成多个调度方案、评估各目标取值并过滤被支配解，得到
//! Pareto 前沿（`pareto`）。Solver 失败一律回退 `equal_split` 平均分配兜底（沿用 v0.87.0）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | sync `weighted/pareto(&mut self, ...)` — no_std 无 async runtime；`&mut` 因 Solver::solve 需 &mut + last_setpoints 更新 |
//! | **D2** | `Objective` 4 目标枚举（经济/寿命/安全/碳排），Economy 默认；LP 目标为各目标归一化成本的加权和 |
//! | **D3** | `BTreeMap<Objective, f32>` 权重/目标值表 — no_std 合规且迭代有序确定性（沿用 device_pool D3） |
//! | **D8** | 复用 v0.87.0 `DispatchError` 2 变体（EmptyPool/InvalidTarget）；Solver 失败为回退非错误 |
//! | **D9** | 爬坡约束 `prev - ramp <= p <= prev + ramp`（沿用 v0.87.0 D9） |
//! | **D10** | SOC 过滤 `soc <= 0.0` → 跳过（沿用 v0.87.0 D10） |
//! | **D11** | `now_ms: u64` 参数注入，no_std 无 Instant::now() |
//! | **D13** | `total_power = Σ setpoints`（clamp 后实际值，沿用 v0.87.0 D13） |
//! | **D14** | Pareto 前沿为确定性权重采样 + O(n²) 支配过滤（严格支配移除；完全相同向量保留先者），非随机 MOEA |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_solver_core::{
    problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType},
    result::SolveStatus,
    solver::Solver,
};

use crate::device_pool::{DeviceCapability, DeviceMode, DevicePool};
use crate::multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan};

/// 4 目标固定迭代顺序（声明顺序即 Ord 排序）.
const OBJECTIVES: [Objective; 4] = [
    Objective::Economy,
    Objective::BatteryLife,
    Objective::Safety,
    Objective::Carbon,
];

/// 优化目标.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Objective {
    /// 经济性（损耗成本最小，默认）.
    #[default]
    Economy,
    /// 电池寿命（充放深度最小）.
    BatteryLife,
    /// 安全性（爬坡裕度最大）.
    Safety,
    /// 碳排放（等效损耗最小）.
    Carbon,
}

/// 加权目标权重表.
#[derive(Debug, Clone, Default)]
pub struct WeightedSum {
    /// 目标 → 权重.
    pub weights: BTreeMap<Objective, f32>,
}

impl WeightedSum {
    /// 创建空权重表.
    pub fn new() -> Self {
        Self {
            weights: BTreeMap::new(),
        }
    }

    /// 设置目标权重（重复设置覆盖）.
    pub fn set(&mut self, obj: Objective, w: f32) {
        self.weights.insert(obj, w);
    }

    /// 获取目标权重（缺失返回 0.0）.
    pub fn get(&self, obj: Objective) -> f32 {
        self.weights.get(&obj).copied().unwrap_or(0.0)
    }

    /// 归一化权重（返回值含全部 4 目标键）.
    ///
    /// 任一权重非有限/负值，或总和非正/非有限 → 返回 4 目标各 0.25 均权；
    /// 否则每项除以总和.
    pub fn normalized(&self) -> BTreeMap<Objective, f32> {
        let mut sum = 0.0f32;
        let mut valid = true;
        for obj in OBJECTIVES.iter() {
            let w = self.get(*obj);
            if !w.is_finite() || w < 0.0 {
                valid = false;
                break;
            }
            sum += w;
        }
        if !valid || !sum.is_finite() || sum <= 0.0 {
            let mut out = BTreeMap::new();
            for obj in OBJECTIVES.iter() {
                out.insert(*obj, 0.25);
            }
            return out;
        }
        let mut out = BTreeMap::new();
        for obj in OBJECTIVES.iter() {
            out.insert(*obj, self.get(*obj) / sum);
        }
        out
    }
}

/// Pareto 解（一组目标评估值 + 对应调度计划）.
#[derive(Debug, Clone)]
pub struct ParetoSolution {
    /// 各目标评估值（含全部 4 目标键）.
    pub objectives: BTreeMap<Objective, f32>,
    /// 调度计划.
    pub plan: DispatchPlan,
}

/// Pareto 前沿（非支配解集合）.
#[derive(Debug, Clone, Default)]
pub struct ParetoFront {
    /// 非支配解列表.
    pub solutions: Vec<ParetoSolution>,
}

/// 计算单目标的各设备成本系数（f64，LP 目标构建用）.
pub fn objective_costs(obj: Objective, caps: &[(u64, DeviceCapability)]) -> Vec<f64> {
    let mut costs = Vec::with_capacity(caps.len());
    for (_, cap) in caps.iter() {
        let c = match obj {
            Objective::Economy | Objective::Carbon => 1.0 - cap.efficiency as f64,
            Objective::BatteryLife => {
                if cap.p_max <= 0.0 {
                    1.0
                } else {
                    1.0 / cap.p_max as f64
                }
            }
            Objective::Safety => {
                if cap.ramp_rate <= 0.0 {
                    1.0
                } else {
                    1.0 / cap.ramp_rate as f64
                }
            }
        };
        costs.push(c);
    }
    costs
}

/// 原地归一化成本系数：有限值最大值缩放到 1.0；无有效有限值 → 全部置 0.0.
pub fn normalize_costs(costs: &mut [f64]) {
    let mut max = 0.0f64;
    for c in costs.iter() {
        if c.is_finite() && *c > max {
            max = *c;
        }
    }
    if max <= 0.0 {
        for c in costs.iter_mut() {
            *c = 0.0;
        }
        return;
    }
    for c in costs.iter_mut() {
        if c.is_finite() {
            *c /= max;
        } else {
            *c = 0.0;
        }
    }
}

/// 确定性生成第 i 个采样权重（共 samples 个采样点）.
///
/// samples == 0 为防御分支（`pareto` 已先行返回），返回全 1.0 权重避免取模除零.
pub fn generate_weight_sample(i: u32, samples: u32) -> WeightedSum {
    let mut w = WeightedSum::new();
    if samples == 0 {
        for obj in OBJECTIVES.iter() {
            w.set(*obj, 1.0);
        }
        return w;
    }
    for (j, obj) in OBJECTIVES.iter().enumerate() {
        let v = ((i as u64 * (j as u64 + 1)) % samples as u64 + 1) as f32;
        w.set(*obj, v);
    }
    w
}

/// 评估调度计划在 4 目标上的取值（Σ cost_obj * setpoint，返回含全部 4 键）.
pub fn eval_plan_objectives(plan: &DispatchPlan, pool: &DevicePool) -> BTreeMap<Objective, f32> {
    let mut economy = 0.0f32;
    let mut life = 0.0f32;
    let mut safety = 0.0f32;
    let mut carbon = 0.0f32;
    for a in plan.assignments.iter() {
        if let Some(cap) = pool.get(a.device_id) {
            let sp = a.setpoint;
            economy += (1.0 - cap.efficiency) * sp;
            life += (if cap.p_max <= 0.0 {
                1.0
            } else {
                1.0 / cap.p_max
            }) * sp;
            safety += (if cap.ramp_rate <= 0.0 {
                1.0
            } else {
                1.0 / cap.ramp_rate
            }) * sp;
            carbon += (1.0 - cap.efficiency) * sp;
        }
    }
    let mut out = BTreeMap::new();
    out.insert(Objective::Economy, economy);
    out.insert(Objective::BatteryLife, life);
    out.insert(Objective::Safety, safety);
    out.insert(Objective::Carbon, carbon);
    out
}

/// 过滤被支配解（最小化语义，O(n²)，保持原相对顺序）.
///
/// A 支配 B ⟺ 4 目标全部 A<=B 且至少一个 A<B（缺失键按 0.0 处理）；
/// 完全相同向量保留先出现者.
/// NaN 防御：非有限目标值（NaN/±∞）一律视为 f32::INFINITY（最小化语义下的最差值），
/// 避免 IEEE-754 下 NaN 与任何值比较均返回 false 造成的支配误判.
pub fn filter_dominated(solutions: Vec<ParetoSolution>) -> Vec<ParetoSolution> {
    fn objective_vector(s: &ParetoSolution) -> [f32; 4] {
        // 非有限值 → +∞（最小化最差值）；缺失键仍按 0.0 处理
        let sanitize = |v: f32| {
            if v.is_finite() {
                v
            } else {
                f32::INFINITY
            }
        };
        [
            sanitize(
                s.objectives
                    .get(&Objective::Economy)
                    .copied()
                    .unwrap_or(0.0),
            ),
            sanitize(
                s.objectives
                    .get(&Objective::BatteryLife)
                    .copied()
                    .unwrap_or(0.0),
            ),
            sanitize(s.objectives.get(&Objective::Safety).copied().unwrap_or(0.0)),
            sanitize(s.objectives.get(&Objective::Carbon).copied().unwrap_or(0.0)),
        ]
    }
    let n = solutions.len();
    let vectors: Vec<[f32; 4]> = solutions.iter().map(objective_vector).collect();
    let mut dominated = alloc::vec![false; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let mut all_le = true;
            let mut any_lt = false;
            for (av, bv) in vectors[j].iter().zip(vectors[i].iter()) {
                if av > bv {
                    all_le = false;
                    break;
                }
                if av < bv {
                    any_lt = true;
                }
            }
            // 严格支配；或完全相同向量但 j 先出现（保留先者）
            if all_le && (any_lt || j < i) {
                dominated[i] = true;
                break;
            }
        }
    }
    let mut out = Vec::new();
    for (i, s) in solutions.into_iter().enumerate() {
        if !dominated[i] {
            out.push(s);
        }
    }
    out
}

/// 多目标优化器.
pub struct MultiObjectiveOptimizer {
    /// 设备池.
    pub pool: DevicePool,
    /// 求解器.
    pub solver: Box<dyn Solver>,
    /// 上次设定点（设备 ID → 功率）.
    pub last_setpoints: BTreeMap<u64, f32>,
}

impl MultiObjectiveOptimizer {
    /// 创建多目标优化器.
    pub fn new(pool: DevicePool, solver: Box<dyn Solver>) -> Self {
        Self {
            pool,
            solver,
            last_setpoints: BTreeMap::new(),
        }
    }

    /// 加权和优化：4 目标成本归一化后按权重线性组合为 LP 目标并求解.
    pub fn weighted(
        &mut self,
        target: f32,
        socs: &BTreeMap<u64, f32>,
        w: &WeightedSum,
        now_ms: u64,
    ) -> Result<DispatchPlan, DispatchError> {
        // 1. 目标校验
        if !target.is_finite() {
            return Err(DispatchError::InvalidTarget);
        }

        // 2. 陈旧清理
        self.last_setpoints
            .retain(|id, _| self.pool.devices.contains_key(id));

        // 3. SOC 过滤（D10）
        let mut eligible: Vec<(u64, DeviceCapability)> = Vec::new();
        for (id, cap) in self.pool.devices.iter() {
            if let Some(soc) = socs.get(id) {
                if *soc <= 0.0 {
                    continue;
                }
            }
            eligible.push((*id, *cap));
        }

        // 4. 空池校验
        if eligible.is_empty() {
            return Err(DispatchError::EmptyPool);
        }

        // 5. 加权目标构建（D2）：各目标成本归一化后按权重线性组合
        let wn = w.normalized();
        let mut combined = alloc::vec![0.0f64; eligible.len()];
        for obj in OBJECTIVES.iter() {
            let mut costs = objective_costs(*obj, &eligible);
            normalize_costs(&mut costs);
            let weight = wn.get(obj).copied().unwrap_or(0.0) as f64;
            for (i, c) in costs.iter().enumerate() {
                combined[i] += weight * c;
            }
        }

        // 6. 构建 LP
        let problem = build_weighted_lp(&eligible, target, &self.last_setpoints, combined);
        let n = eligible.len();

        // 7. 求解（Optimal 且解长度匹配 → 采用；其余全部回退平均分配）
        let (assignments, objective_value) = match self.solver.solve(&problem, now_ms) {
            Ok(result) if result.status == SolveStatus::Optimal && result.solution.len() == n => {
                let mut assignments = Vec::with_capacity(n);
                for (i, (id, cap)) in eligible.iter().enumerate() {
                    let sp = result.solution[i]
                        .max(cap.p_min as f64)
                        .min(cap.p_max as f64) as f32;
                    assignments.push(DeviceAssignment {
                        device_id: *id,
                        setpoint: sp,
                        mode: DeviceMode::Auto,
                    });
                }
                (assignments, result.objective_value as f32)
            }
            _ => (equal_split(target, &eligible), 0.0),
        };

        // 8. 更新 last_setpoints
        for a in &assignments {
            self.last_setpoints.insert(a.device_id, a.setpoint);
        }

        // 9. 返回（D13：total_power 为 clamp 后实际设定值之和）
        let total_power: f32 = assignments.iter().map(|a| a.setpoint).sum();
        Ok(DispatchPlan {
            timestamp: now_ms,
            assignments,
            total_power,
            objective_value,
        })
    }

    /// Pareto 前沿采样：确定性权重采样生成 samples 个解，评估目标并过滤被支配解.
    pub fn pareto(
        &mut self,
        target: f32,
        socs: &BTreeMap<u64, f32>,
        samples: u32,
        now_ms: u64,
    ) -> Result<ParetoFront, DispatchError> {
        if samples == 0 {
            return Ok(ParetoFront {
                solutions: Vec::new(),
            });
        }
        let mut solutions = Vec::new();
        for i in 0..samples {
            let w = generate_weight_sample(i, samples);
            let plan = self.weighted(target, socs, &w, now_ms)?;
            let objectives = eval_plan_objectives(&plan, &self.pool);
            solutions.push(ParetoSolution { objectives, plan });
        }
        Ok(ParetoFront {
            solutions: filter_dominated(solutions),
        })
    }
}

// 构建加权目标 LP（与 v0.87.0 build_lp_problem 同构，objective 外部传入）.
fn build_weighted_lp(
    eligible: &[(u64, DeviceCapability)],
    target: f32,
    last_setpoints: &BTreeMap<u64, f32>,
    objective: Vec<f64>,
) -> LpProblem {
    let n = eligible.len();
    let mut variables = Vec::with_capacity(n);
    let mut lower_bounds = Vec::with_capacity(n);
    let mut upper_bounds = Vec::with_capacity(n);
    let mut var_types = Vec::with_capacity(n);

    for (id, cap) in eligible.iter() {
        variables.push(alloc::format!("p_{}", id));
        lower_bounds.push(cap.p_min as f64);
        upper_bounds.push(cap.p_max as f64);
        var_types.push(VarType::Continuous);
    }

    // 平衡行 + 每个有上次设定点的设备 1 条爬坡行
    let num_ramp = eligible
        .iter()
        .filter(|(id, _)| last_setpoints.contains_key(id))
        .count();
    let num_rows = 1 + num_ramp;
    let num_nz = n + num_ramp;

    let mut row_start = Vec::with_capacity(num_rows + 1);
    let mut col_index = Vec::with_capacity(num_nz);
    let mut values = Vec::with_capacity(num_nz);
    let mut rhs_lower = Vec::with_capacity(num_rows);
    let mut rhs_upper = Vec::with_capacity(num_rows);

    // 平衡行：Σ p_i = target
    row_start.push(0);
    for i in 0..n {
        col_index.push(i as i32);
        values.push(1.0);
    }
    row_start.push(n as i32);
    rhs_lower.push(target as f64);
    rhs_upper.push(target as f64);

    // 爬坡行（D9）：prev - ramp <= p_i <= prev + ramp，每行 1 个非零
    for (i, (id, cap)) in eligible.iter().enumerate() {
        if let Some(prev) = last_setpoints.get(id) {
            col_index.push(i as i32);
            values.push(1.0);
            row_start.push(col_index.len() as i32);
            rhs_lower.push((*prev as f64) - (cap.ramp_rate as f64));
            rhs_upper.push((*prev as f64) + (cap.ramp_rate as f64));
        }
    }

    LpProblem {
        variables,
        lower_bounds,
        upper_bounds,
        var_types,
        objective,
        sense: ObjectiveSense::Minimize,
        constraints: ConstraintMatrix::new(num_rows, num_nz, row_start, col_index, values),
        rhs_lower,
        rhs_upper,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_solver_core::{error::SolverError, result::SolveResult};

    use super::*;

    // ===== FixedSolver 测试辅助 =====
    //
    // LP 结构验证直接调用同模块私有 `build_weighted_lp`（T149/T150），
    // 因此本桩仅提供固定结果 / 失败两种行为，不记录问题。
    struct FixedSolver {
        result: Option<SolveResult>,
        fail: bool,
    }

    impl FixedSolver {
        fn new() -> Self {
            Self {
                result: None,
                fail: false,
            }
        }
        fn with_result(result: SolveResult) -> Self {
            Self {
                result: Some(result),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                result: None,
                fail: true,
            }
        }
    }

    impl Solver for FixedSolver {
        fn solve(
            &mut self,
            _problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<SolveResult, SolverError> {
            if self.fail {
                return Err(SolverError::RunFailed(-1));
            }
            match &self.result {
                Some(r) => Ok(r.clone()),
                None => Ok(SolveResult::optimal(0.0, vec![])),
            }
        }
        fn name(&self) -> &'static str {
            "FixedSolver"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn set_param(&mut self, _key: &str, _value: &str) -> Result<(), SolverError> {
            Ok(())
        }
        fn status(&self) -> eneros_solver_core::solver::SolverStatus {
            eneros_solver_core::solver::SolverStatus::Idle
        }
    }

    /// 辅助：构造标准设备（p_min=0, p_max, ramp_rate=1.0, efficiency）.
    fn cap(p_max: f32, efficiency: f32) -> DeviceCapability {
        DeviceCapability {
            p_min: 0.0,
            p_max,
            ramp_rate: 1.0,
            efficiency,
        }
    }

    /// 辅助：构造 2 设备池（ID 1/2）+ 对应 SOC 表.
    fn two_device_setup() -> (DevicePool, BTreeMap<u64, f32>) {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(5.0, 0.9));
        pool.add_device(2, cap(5.0, 0.8));
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        socs.insert(2u64, 0.5f32);
        (pool, socs)
    }

    /// 辅助：构造仅含 Economy/BatteryLife 2 键的 Pareto 解（total_power 作为标识）.
    fn sol2(econ: f32, life: f32, tag: f32) -> ParetoSolution {
        let mut objectives = BTreeMap::new();
        objectives.insert(Objective::Economy, econ);
        objectives.insert(Objective::BatteryLife, life);
        ParetoSolution {
            objectives,
            plan: DispatchPlan {
                timestamp: 0,
                assignments: Vec::new(),
                total_power: tag,
                objective_value: 0.0,
            },
        }
    }

    // ===== T121~T124：Objective 枚举 =====
    #[test]
    fn t121_objective_default_and_debug() {
        assert_eq!(Objective::default(), Objective::Economy);
        assert!(!format!("{:?}", Objective::Economy).is_empty());
        assert!(!format!("{:?}", Objective::BatteryLife).is_empty());
        assert!(!format!("{:?}", Objective::Safety).is_empty());
        assert!(!format!("{:?}", Objective::Carbon).is_empty());
    }

    #[test]
    fn t122_objective_variants_distinct() {
        let objs = [
            Objective::Economy,
            Objective::BatteryLife,
            Objective::Safety,
            Objective::Carbon,
        ];
        for (i, a) in objs.iter().enumerate() {
            for b in objs.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn t123_objective_btreemap_key_order() {
        let mut m = BTreeMap::new();
        m.insert(Objective::Carbon, 1.0f32);
        m.insert(Objective::Safety, 2.0f32);
        m.insert(Objective::BatteryLife, 3.0f32);
        m.insert(Objective::Economy, 4.0f32);
        let keys: Vec<Objective> = m.keys().copied().collect();
        assert_eq!(
            keys,
            vec![
                Objective::Economy,
                Objective::BatteryLife,
                Objective::Safety,
                Objective::Carbon
            ]
        );
    }

    #[test]
    fn t124_objective_copy() {
        let obj = Objective::Safety;
        let b = obj;
        assert_eq!(obj, b);
    }

    // ===== T125~T131：WeightedSum =====
    #[test]
    fn t125_weighted_sum_new_empty() {
        let w = WeightedSum::new();
        assert!(w.weights.is_empty());
        assert_eq!(w.get(Objective::Economy), 0.0);
    }

    #[test]
    fn t126_weighted_sum_set_overwrite() {
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, 2.0);
        assert_eq!(w.get(Objective::Economy), 2.0);
        w.set(Objective::Economy, 1.0);
        assert_eq!(w.get(Objective::Economy), 1.0);
    }

    #[test]
    fn t127_weighted_sum_normalized_basic() {
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, 2.0);
        w.set(Objective::BatteryLife, 1.0);
        w.set(Objective::Safety, 1.0);
        let n = w.normalized();
        assert!((n[&Objective::Economy] - 0.5).abs() < 1e-6);
        assert!((n[&Objective::BatteryLife] - 0.25).abs() < 1e-6);
        assert!((n[&Objective::Safety] - 0.25).abs() < 1e-6);
        assert_eq!(n[&Objective::Carbon], 0.0);
    }

    #[test]
    fn t128_weighted_sum_normalized_nan_fallback() {
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, f32::NAN);
        w.set(Objective::BatteryLife, 1.0);
        let n = w.normalized();
        for obj in OBJECTIVES.iter() {
            assert_eq!(n[obj], 0.25);
        }
    }

    #[test]
    fn t129_weighted_sum_normalized_negative_fallback() {
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, -1.0);
        w.set(Objective::BatteryLife, 2.0);
        let n = w.normalized();
        for obj in OBJECTIVES.iter() {
            assert_eq!(n[obj], 0.25);
        }
    }

    #[test]
    fn t130_weighted_sum_normalized_zero_fallback() {
        let w = WeightedSum::new();
        let n = w.normalized();
        assert_eq!(n.len(), 4);
        for obj in OBJECTIVES.iter() {
            assert_eq!(n[obj], 0.25);
        }
        let mut all_zero = WeightedSum::new();
        all_zero.set(Objective::Economy, 0.0);
        all_zero.set(Objective::Carbon, 0.0);
        let n2 = all_zero.normalized();
        assert_eq!(n2.len(), 4);
        for obj in OBJECTIVES.iter() {
            assert_eq!(n2[obj], 0.25);
        }
    }

    #[test]
    fn t131_weighted_sum_clone() {
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, 2.0);
        w.set(Objective::Carbon, 3.0);
        let c = w.clone();
        assert_eq!(c.get(Objective::Economy), 2.0);
        assert_eq!(c.get(Objective::Carbon), 3.0);
        assert_eq!(c.get(Objective::Safety), 0.0);
    }

    // ===== T132~T134：ParetoSolution / ParetoFront =====
    #[test]
    fn t132_pareto_solution_construction() {
        let mut objectives = BTreeMap::new();
        objectives.insert(Objective::Economy, 0.5f32);
        objectives.insert(Objective::BatteryLife, 0.2f32);
        objectives.insert(Objective::Safety, 1.0f32);
        objectives.insert(Objective::Carbon, 0.5f32);
        let plan = DispatchPlan {
            timestamp: 1000,
            assignments: Vec::new(),
            total_power: 3.0,
            objective_value: 0.1,
        };
        let s = ParetoSolution { objectives, plan };
        assert_eq!(s.objectives.len(), 4);
        assert_eq!(s.plan.total_power, 3.0);
    }

    #[test]
    fn t133_pareto_solution_clone() {
        let mut objectives = BTreeMap::new();
        objectives.insert(Objective::Economy, 0.5f32);
        objectives.insert(Objective::BatteryLife, 0.2f32);
        objectives.insert(Objective::Safety, 1.0f32);
        objectives.insert(Objective::Carbon, 0.5f32);
        let plan = DispatchPlan {
            timestamp: 1000,
            assignments: vec![DeviceAssignment {
                device_id: 1,
                setpoint: 2.0,
                mode: DeviceMode::Auto,
            }],
            total_power: 2.0,
            objective_value: 0.1,
        };
        let s = ParetoSolution { objectives, plan };
        let c = s.clone();
        assert_eq!(s.objectives, c.objectives);
        assert_eq!(s.plan, c.plan);
    }

    #[test]
    fn t134_pareto_front_default_and_len() {
        let f = ParetoFront::default();
        assert!(f.solutions.is_empty());
        let f2 = ParetoFront {
            solutions: vec![ParetoSolution {
                objectives: BTreeMap::new(),
                plan: DispatchPlan::default(),
            }],
        };
        assert_eq!(f2.solutions.len(), 1);
    }

    // ===== T135~T138：objective_costs =====
    #[test]
    fn t135_objective_costs_economy_and_carbon() {
        let caps = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let c = objective_costs(Objective::Economy, &caps);
        assert_eq!(c.len(), 2);
        assert!((c[0] - 0.1).abs() < 1e-5);
        assert!((c[1] - 0.2).abs() < 1e-5);
        // Carbon 与 Economy 同公式
        let cc = objective_costs(Objective::Carbon, &caps);
        assert_eq!(c, cc);
    }

    #[test]
    fn t136_objective_costs_life_and_safety() {
        let caps = vec![
            (
                1u64,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: 5.0,
                    ramp_rate: 1.0,
                    efficiency: 0.9,
                },
            ),
            (
                2u64,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: 10.0,
                    ramp_rate: 2.0,
                    efficiency: 0.8,
                },
            ),
        ];
        let life = objective_costs(Objective::BatteryLife, &caps);
        assert!((life[0] - 0.2).abs() < 1e-5);
        assert!((life[1] - 0.1).abs() < 1e-5);
        let safety = objective_costs(Objective::Safety, &caps);
        assert!((safety[0] - 1.0).abs() < 1e-5);
        assert!((safety[1] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn t137_objective_costs_degenerate() {
        let caps = vec![(
            1u64,
            DeviceCapability {
                p_min: 0.0,
                p_max: 0.0,
                ramp_rate: 0.0,
                efficiency: 0.5,
            },
        )];
        let life = objective_costs(Objective::BatteryLife, &caps);
        assert_eq!(life[0], 1.0);
        let safety = objective_costs(Objective::Safety, &caps);
        assert_eq!(safety[0], 1.0);
    }

    #[test]
    fn t138_objective_costs_empty() {
        let caps: Vec<(u64, DeviceCapability)> = vec![];
        assert!(objective_costs(Objective::Economy, &caps).is_empty());
        assert!(objective_costs(Objective::BatteryLife, &caps).is_empty());
        assert!(objective_costs(Objective::Safety, &caps).is_empty());
        assert!(objective_costs(Objective::Carbon, &caps).is_empty());
    }

    // ===== T139~T140：normalize_costs =====
    #[test]
    fn t139_normalize_costs_basic() {
        let mut c = vec![0.1, 0.2];
        normalize_costs(&mut c);
        assert!((c[0] - 0.5).abs() < 1e-6);
        assert!((c[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn t140_normalize_costs_zero_and_nan() {
        let mut z = vec![0.0, 0.0];
        normalize_costs(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
        let mut n = vec![f64::NAN, f64::NAN];
        normalize_costs(&mut n);
        assert_eq!(n, vec![0.0, 0.0]);
    }

    // ===== T141~T143：generate_weight_sample =====
    #[test]
    fn t141_generate_weight_sample_deterministic() {
        let a = generate_weight_sample(0, 4);
        let b = generate_weight_sample(0, 4);
        for obj in OBJECTIVES.iter() {
            assert_eq!(a.get(*obj), b.get(*obj));
        }
    }

    #[test]
    fn t142_generate_weight_sample_varies_with_i() {
        let a = generate_weight_sample(0, 4);
        let b = generate_weight_sample(1, 4);
        assert!(
            a.get(Objective::Economy) != b.get(Objective::Economy)
                || a.get(Objective::BatteryLife) != b.get(Objective::BatteryLife)
                || a.get(Objective::Safety) != b.get(Objective::Safety)
                || a.get(Objective::Carbon) != b.get(Objective::Carbon)
        );
    }

    #[test]
    fn t143_generate_weight_sample_normalized_sum_one() {
        let w = generate_weight_sample(0, 1);
        let n = w.normalized();
        assert_eq!(n.len(), 4);
        let sum: f32 = n.values().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    // ===== T144：eval_plan_objectives =====
    #[test]
    fn t144_eval_plan_objectives_two_devices() {
        let (pool, _) = two_device_setup();
        let plan = DispatchPlan {
            timestamp: 1000,
            assignments: vec![
                DeviceAssignment {
                    device_id: 1,
                    setpoint: 3.0,
                    mode: DeviceMode::Auto,
                },
                DeviceAssignment {
                    device_id: 2,
                    setpoint: 2.0,
                    mode: DeviceMode::Auto,
                },
            ],
            total_power: 5.0,
            objective_value: 0.0,
        };
        let objs = eval_plan_objectives(&plan, &pool);
        assert_eq!(objs.len(), 4);
        // Economy/Carbon = (1-0.9)*3 + (1-0.8)*2 = 0.7
        assert!((objs[&Objective::Economy] - 0.7).abs() < 1e-4);
        assert!((objs[&Objective::Carbon] - 0.7).abs() < 1e-4);
        // BatteryLife = 3/5 + 2/5 = 1.0；Safety = 3/1 + 2/1 = 5.0
        assert!((objs[&Objective::BatteryLife] - 1.0).abs() < 1e-4);
        assert!((objs[&Objective::Safety] - 5.0).abs() < 1e-4);
    }

    // ===== T145~T148：weighted 校验与正常路径 =====
    #[test]
    fn t145_weighted_invalid_target() {
        let pool = DevicePool::new();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let socs = BTreeMap::new();
        let w = WeightedSum::new();
        assert_eq!(
            opt.weighted(f32::NAN, &socs, &w, 1000).unwrap_err(),
            DispatchError::InvalidTarget
        );
        assert_eq!(
            opt.weighted(f32::INFINITY, &socs, &w, 1000).unwrap_err(),
            DispatchError::InvalidTarget
        );
    }

    #[test]
    fn t146_weighted_empty_pool() {
        let pool = DevicePool::new();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        let w = WeightedSum::new();
        assert_eq!(
            opt.weighted(5.0, &socs, &w, 1000).unwrap_err(),
            DispatchError::EmptyPool
        );
        // 2 设备但 SOC 全 0.0 → EmptyPool
        let (pool2, mut socs2) = two_device_setup();
        socs2.insert(1, 0.0);
        socs2.insert(2, 0.0);
        let solver2: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt2 = MultiObjectiveOptimizer::new(pool2, solver2);
        assert_eq!(
            opt2.weighted(5.0, &socs2, &w, 1000).unwrap_err(),
            DispatchError::EmptyPool
        );
    }

    #[test]
    fn t147_weighted_soc_filter() {
        let (pool, mut socs) = two_device_setup();
        socs.insert(1, 0.0); // 设备 1 SOC 耗尽 → 跳过
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(SolveResult::optimal(
            0.0,
            vec![2.0],
        )));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let w = WeightedSum::new();
        let plan = opt.weighted(2.0, &socs, &w, 1000).unwrap();
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].device_id, 2);
        assert_eq!(plan.assignments[0].setpoint, 2.0);
    }

    #[test]
    fn t148_weighted_happy_path() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(SolveResult::optimal(
            0.4,
            vec![3.0, 2.0],
        )));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let w = WeightedSum::new();
        let plan = opt.weighted(5.0, &socs, &w, 2000).unwrap();
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].device_id, 1);
        assert_eq!(plan.assignments[0].setpoint, 3.0);
        assert_eq!(plan.assignments[1].device_id, 2);
        assert_eq!(plan.assignments[1].setpoint, 2.0);
        assert!(plan.assignments[0].device_id < plan.assignments[1].device_id);
        assert_eq!(plan.assignments[0].mode, DeviceMode::Auto);
        assert_eq!(plan.total_power, 5.0);
        assert_eq!(plan.objective_value, 0.4);
        assert_eq!(plan.timestamp, 2000);
        assert_eq!(opt.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(opt.last_setpoints.get(&2), Some(&2.0));
    }

    // ===== T149~T150：LP 构建（直接调用 build_weighted_lp）=====
    #[test]
    fn t149_weighted_objective_combination() {
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        // 手动复现第 5 步：E=1.0 / B=1.0 等权 → combined[i] = 0.5*norm_e[i] + 0.5*norm_b[i]
        let mut w = WeightedSum::new();
        w.set(Objective::Economy, 1.0);
        w.set(Objective::BatteryLife, 1.0);
        let wn = w.normalized();
        let mut norm_e = objective_costs(Objective::Economy, &eligible);
        normalize_costs(&mut norm_e);
        let mut norm_b = objective_costs(Objective::BatteryLife, &eligible);
        normalize_costs(&mut norm_b);
        // norm_e = [0.5, 1.0]（0.1/0.2 归一化）；norm_b = [1.0, 1.0]（0.2/0.2 归一化）
        assert!((norm_e[0] - 0.5).abs() < 1e-6);
        assert!((norm_e[1] - 1.0).abs() < 1e-6);
        assert!((norm_b[0] - 1.0).abs() < 1e-6);
        assert!((norm_b[1] - 1.0).abs() < 1e-6);
        let mut combined = vec![0.0f64; eligible.len()];
        for i in 0..eligible.len() {
            combined[i] = wn[&Objective::Economy] as f64 * norm_e[i]
                + wn[&Objective::BatteryLife] as f64 * norm_b[i];
        }
        assert!((combined[0] - (0.5 * norm_e[0] + 0.5 * norm_b[0])).abs() < 1e-6);
        assert!((combined[1] - (0.5 * norm_e[1] + 0.5 * norm_b[1])).abs() < 1e-6);
        // 同模块私有函数直接调用：objective 原样传入 LP
        let lp = build_weighted_lp(&eligible, 5.0, &BTreeMap::new(), combined.clone());
        assert_eq!(lp.objective, combined);
        assert_eq!(lp.variables, vec!["p_1".to_string(), "p_2".to_string()]);
        assert_eq!(lp.sense, ObjectiveSense::Minimize);
    }

    #[test]
    fn t150_build_weighted_lp_balance_and_ramp_rows() {
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let obj = vec![0.5, 0.5];
        // 无 last_setpoints → 仅 1 条平衡行
        let lp = build_weighted_lp(&eligible, 5.0, &BTreeMap::new(), obj.clone());
        assert_eq!(lp.constraints.num_rows, 1);
        assert_eq!(lp.rhs_lower.len(), 1);
        assert_eq!(lp.rhs_lower[0], 5.0);
        assert_eq!(lp.rhs_upper[0], 5.0);
        // 带 last_setpoints {1:3.0, 2:2.0}（ramp=1.0）→ 1 平衡行 + 2 爬坡行
        let mut last = BTreeMap::new();
        last.insert(1u64, 3.0f32);
        last.insert(2u64, 2.0f32);
        let lp2 = build_weighted_lp(&eligible, 5.0, &last, obj);
        assert_eq!(lp2.constraints.num_rows, 3);
        assert_eq!(lp2.rhs_lower.len(), 3);
        assert_eq!(lp2.rhs_upper.len(), 3);
        // 设备1: [3-1, 3+1]=[2,4]；设备2: [2-1, 2+1]=[1,3]
        assert_eq!(lp2.rhs_lower[1], 2.0);
        assert_eq!(lp2.rhs_upper[1], 4.0);
        assert_eq!(lp2.rhs_lower[2], 1.0);
        assert_eq!(lp2.rhs_upper[2], 3.0);
    }

    // ===== T151~T153：回退与 clamp =====
    #[test]
    fn t151_weighted_solver_error_fallback() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::failing());
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let w = WeightedSum::new();
        let plan = opt.weighted(5.0, &socs, &w, 1000).unwrap();
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let expect = equal_split(5.0, &eligible);
        assert_eq!(plan.assignments, expect);
        assert_eq!(plan.assignments[0].setpoint, 2.5);
        assert_eq!(plan.assignments[1].setpoint, 2.5);
        assert_eq!(plan.objective_value, 0.0);
        assert_eq!(plan.total_power, 5.0);
    }

    #[test]
    fn t152_weighted_infeasible_and_empty_solution_fallback() {
        let (pool, socs) = two_device_setup();
        let w = WeightedSum::new();
        // status Infeasible → 回退
        let infeasible = SolveResult {
            status: SolveStatus::Infeasible,
            objective_value: 0.0,
            solution: Vec::new(),
            elapsed_ms: 0,
            dual_solution: None,
        };
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(infeasible));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let plan = opt.weighted(6.0, &socs, &w, 1000).unwrap();
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].setpoint, 3.0);
        assert_eq!(plan.assignments[1].setpoint, 3.0);
        assert_eq!(plan.objective_value, 0.0);
        // Optimal 但解为空 vec → 长度不匹配 → 回退
        let (pool2, socs2) = two_device_setup();
        let solver2: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt2 = MultiObjectiveOptimizer::new(pool2, solver2);
        let plan2 = opt2.weighted(6.0, &socs2, &w, 1000).unwrap();
        assert_eq!(plan2.assignments.len(), 2);
        assert_eq!(plan2.assignments[0].setpoint, 3.0);
        assert_eq!(plan2.objective_value, 0.0);
    }

    #[test]
    fn t153_weighted_clamp_and_default_weights() {
        let (pool, socs) = two_device_setup();
        // 空权重表 → 归一化为各 0.25 均权，路径正常
        let w = WeightedSum::new();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(SolveResult::optimal(
            0.0,
            vec![9.0, 2.0],
        )));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let plan = opt.weighted(5.0, &socs, &w, 1000).unwrap();
        assert_eq!(plan.assignments[0].setpoint, 5.0); // 9.0 clamp 到 p_max
        assert_eq!(plan.assignments[1].setpoint, 2.0);
    }

    // ===== T154~T155：filter_dominated =====
    #[test]
    fn t154_filter_dominated_basic() {
        let solutions = vec![
            sol2(1.0, 2.0, 1.0),
            sol2(2.0, 1.0, 2.0),
            sol2(3.0, 3.0, 3.0),
        ];
        let kept = filter_dominated(solutions);
        // C(3,3) 被 A、B 支配 → 移除；A、B 互不支配 → 保留原顺序
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].plan.total_power, 1.0);
        assert_eq!(kept[1].plan.total_power, 2.0);
    }

    #[test]
    fn t155_filter_dominated_identical_keeps_first() {
        // 完全相同向量 → 保留先出现者（len 减 1）
        let kept = filter_dominated(vec![sol2(1.0, 1.0, 1.0), sol2(1.0, 1.0, 2.0)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].plan.total_power, 1.0);
        // 空输入 → 空输出
        let empty: Vec<ParetoSolution> = vec![];
        assert!(filter_dominated(empty).is_empty());
    }

    #[test]
    fn t161_filter_dominated_nan_defense() {
        // P=(NaN, 3.0) vs Q=(5.0, 0.5)：
        // 修复后 NaN → +∞，Q 支配 P（5.0 < +∞ 且 0.5 < 3.0）→ P 移除，front 只剩 Q
        let kept = filter_dominated(vec![sol2(f32::NAN, 3.0, 1.0), sol2(5.0, 0.5, 2.0)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].plan.total_power, 2.0);

        // R=(NaN, 0.5) vs S=(5.0, 3.0)：
        // 修复后互不支配（Economy 维 +∞ > 5.0，BatteryLife 维 0.5 < 3.0）→ 都保留；
        // 修复前 R 会被误判支配 S（NaN 比较全 false → Economy 维视为相等）导致 S 被错误移除
        let kept2 = filter_dominated(vec![sol2(f32::NAN, 0.5, 3.0), sol2(5.0, 3.0, 4.0)]);
        assert_eq!(kept2.len(), 2);
        assert_eq!(kept2[0].plan.total_power, 3.0);
        assert_eq!(kept2[1].plan.total_power, 4.0);
    }

    // ===== T156~T158：pareto =====
    #[test]
    fn t156_pareto_zero_samples() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let front = opt.pareto(5.0, &socs, 0, 1000).unwrap();
        assert!(front.solutions.is_empty());
    }

    #[test]
    fn t157_pareto_happy_path() {
        let (pool, socs) = two_device_setup();
        // 任何权重下解长度均匹配（2），所有采样解相同 → 支配过滤后 <= samples
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(SolveResult::optimal(
            0.0,
            vec![2.5, 2.5],
        )));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let front = opt.pareto(5.0, &socs, 4, 1000).unwrap();
        assert!(front.solutions.len() <= 4);
        assert!(!front.solutions.is_empty());
        for s in front.solutions.iter() {
            assert_eq!(s.objectives.len(), 4);
        }
    }

    #[test]
    fn t158_pareto_empty_pool() {
        let pool = DevicePool::new();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::new());
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        let socs = BTreeMap::new();
        assert_eq!(
            opt.pareto(5.0, &socs, 4, 1000).unwrap_err(),
            DispatchError::EmptyPool
        );
    }

    // ===== T159~T160：权重影响与连续调度 =====
    #[test]
    fn t159_weights_affect_combined_objective() {
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(10.0, 0.8))];
        let combined_for = |w: &WeightedSum| -> Vec<f64> {
            let wn = w.normalized();
            let mut combined = vec![0.0f64; eligible.len()];
            for obj in OBJECTIVES.iter() {
                let mut costs = objective_costs(*obj, &eligible);
                normalize_costs(&mut costs);
                let weight = wn[obj] as f64;
                for (i, c) in costs.iter().enumerate() {
                    combined[i] += weight * c;
                }
            }
            combined
        };
        let mut we = WeightedSum::new();
        we.set(Objective::Economy, 1.0);
        let mut wb = WeightedSum::new();
        wb.set(Objective::BatteryLife, 1.0);
        let ce = combined_for(&we);
        let cb = combined_for(&wb);
        assert_ne!(ce, cb);
    }

    #[test]
    fn t160_weighted_twice_rolling_last_setpoints() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(FixedSolver::with_result(SolveResult::optimal(
            0.0,
            vec![3.0, 2.0],
        )));
        let mut opt = MultiObjectiveOptimizer::new(pool, solver);
        // 第一次：经济权重
        let mut w1 = WeightedSum::new();
        w1.set(Objective::Economy, 1.0);
        let plan1 = opt.weighted(5.0, &socs, &w1, 1000).unwrap();
        assert_eq!(plan1.assignments.len(), 2);
        assert!(!opt.last_setpoints.is_empty());
        // 第二次：寿命权重，不同权重均 Ok
        let mut w2 = WeightedSum::new();
        w2.set(Objective::BatteryLife, 1.0);
        let plan2 = opt.weighted(4.0, &socs, &w2, 2000).unwrap();
        assert_eq!(plan2.assignments.len(), 2);
        // last_setpoints 为第二次（固定解相同）的解
        assert_eq!(opt.last_setpoints.len(), 2);
        assert_eq!(opt.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(opt.last_setpoints.get(&2), Some(&2.0));
        // 第二次调用前 last_setpoints 非空 → LP 含爬坡行（1 平衡行 + 2 爬坡行）
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let lp = build_weighted_lp(&eligible, 4.0, &opt.last_setpoints, vec![0.0, 0.0]);
        assert_eq!(lp.constraints.num_rows, 3);
    }
}
