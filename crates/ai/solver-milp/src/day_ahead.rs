//! 日前计划调度与三级降级链（v0.102.0，D9/D10）.
//!
//! 在 [`crate::uc_model`] 构建的 UC MILP 模型之上，提供日前计划编排：
//!
//! # 三级降级链（D9，蓝图 §4.4 FFI 崩溃降级语义）
//!
//! 1. **完整 MILP**：`build_model` → 求解。可接受状态
//!    （`Optimal` / `Suboptimal` / `Timeout`）→ 解析返回；失败状态
//!    （`Infeasible` / `Unbounded` / `Error(_)`）或 `Err(_)` → 进入第 2 级，
//!    `relax_count += 1`。
//! 2. **松弛 MILP**：`build_model_relaxed`（跳过最小运行/停机约束）→ 求解。
//!    判定规则同上；失败 → 进入第 3 级，`lp_fallback_count += 1`。
//! 3. **LP 松弛**：对第 1 级完整模型做 [`DayAheadScheduler::relax_lp`]
//!    （Binary/Integer 全转 Continuous，原 Binary 位上界保持 1.0）→ 求解。
//!    可接受 → 解析返回；仍失败 → 返回空计划 + 末级状态（不传播 Err）。
//!
//! # 参数注入（D10）
//!
//! `plan()` 在任何一次求解前注入 `time_limit` 与 `mip_rel_gap` 两项参数；
//! `set_param` 返回 `Err` 时直接传播（调度器配置错误属于编程错误，不走降级链）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;
use eneros_solver_core::problem::{LpProblem, VarType};
use eneros_solver_core::result::{SolveResult, SolveStatus};
use eneros_solver_core::solver::Solver;

use crate::uc_model::UnitCommitment;

/// 单台机组的日前计划结果.
#[derive(Debug, Clone)]
pub struct UnitSchedule {
    /// 机组标识（透传 `UcUnit::id`）.
    pub unit_id: String,
    /// 各周期运行状态（U > 0.5 判定为开机）.
    pub commitments: Vec<bool>,
    /// 各周期出力计划（MW，P 变量解值）.
    pub generation: Vec<f64>,
}

/// 日前计划（全部机组 + 总成本 + 求解状态）.
#[derive(Debug, Clone)]
pub struct DayAheadPlan {
    /// 各机组计划（顺序与 `UnitCommitment::units` 一致）.
    pub schedule: Vec<UnitSchedule>,
    /// 总成本（目标函数值；三级全失败时为 0.0）.
    pub total_cost: f64,
    /// 求解状态（三级全失败时为末级状态）.
    pub solve_status: SolveStatus,
}

/// 日前计划调度器.
///
/// 持有求解参数与降级链计数器；计数器随 `plan()` 调用单调累计，
/// 供运行监控/告警统计降级频次。
#[derive(Debug, Clone)]
pub struct DayAheadScheduler {
    /// 求解时间上限（秒，注入 `time_limit`）.
    pub time_limit_s: f64,
    /// MIP 相对间隙（注入 `mip_rel_gap`）.
    pub mip_rel_gap: f64,
    /// 松弛重建触发次数（第 1 级 MILP 失败累计）.
    pub relax_count: u64,
    /// LP 松弛触发次数（第 2 级 relaxed 失败累计）.
    pub lp_fallback_count: u64,
}

impl DayAheadScheduler {
    /// 创建调度器（计数器清零）.
    pub fn new(time_limit_s: f64, mip_rel_gap: f64) -> Self {
        Self {
            time_limit_s,
            mip_rel_gap,
            relax_count: 0,
            lp_fallback_count: 0,
        }
    }

    /// LP 松弛：`var_types` 全转 `Continuous`；原 `Binary` 变量上界保持 1.0
    /// （其余变量上界不动）；其余字段 Clone 透传.
    pub fn relax_lp(model: &LpProblem) -> LpProblem {
        let mut relaxed = model.clone();
        for (idx, vt) in relaxed.var_types.iter_mut().enumerate() {
            // 原 Binary 位上界保持 1.0（Binary 本即 [0,1]，显式赋值兜底）
            if *vt == VarType::Binary {
                relaxed.upper_bounds[idx] = 1.0;
            }
            *vt = VarType::Continuous;
        }
        relaxed
    }

    /// 日前计划：MILP →（失败）→ relaxed 重建 →（仍失败）→ LP 松弛；
    /// 三级全失败返回空 plan + 末级状态.
    ///
    /// # 错误
    ///
    /// 仅以下情况返回 `Err`（均属输入/配置错误，不走降级链）：
    /// - `set_param` 注入失败；
    /// - `build_model` / `build_model_relaxed` 输入长度校验失败。
    ///
    /// 求解器返回 `Err(_)`（如 FFI 崩溃）视为该级失败并进入下一级，
    /// 三级全 `Err` 时以 `SolveStatus::Error(<末次错误描述>)` 返回 `Ok` 空计划。
    pub fn plan(
        &mut self,
        uc: &UnitCommitment,
        load: &[f64],
        price: &[f64],
        solver: &mut dyn Solver,
        now_ms: u64,
    ) -> Result<DayAheadPlan, SolverError> {
        // D10：求解参数注入（在任何一次 solve 之前；注入失败直接传播）
        solver.set_param("time_limit", &format!("{}", self.time_limit_s))?;
        solver.set_param("mip_rel_gap", &format!("{}", self.mip_rel_gap))?;

        // 第 1 级：完整 MILP
        let model = uc.build_model(load, price)?;
        match solver.solve(&model, now_ms) {
            Ok(result) if Self::is_acceptable(&result.status) => Ok(Self::parse(uc, &result)),
            Ok(_) | Err(_) => {
                // 失败状态 / Err（FFI 崩溃降级语义，蓝图 §4.4）：进入第 2 级，不传播
                self.relax_count += 1;
                self.relax_level(uc, load, price, solver, now_ms, &model)
            }
        }
    }

    /// 第 2 级：松弛 MILP（跳过最小运行/停机约束）.
    fn relax_level(
        &mut self,
        uc: &UnitCommitment,
        load: &[f64],
        price: &[f64],
        solver: &mut dyn Solver,
        now_ms: u64,
        model: &LpProblem,
    ) -> Result<DayAheadPlan, SolverError> {
        // 输入已在第 1 级校验过，此处的 Err 属内部不一致，直接传播
        let relaxed_model = uc.build_model_relaxed(load, price)?;
        match solver.solve(&relaxed_model, now_ms) {
            Ok(result) if Self::is_acceptable(&result.status) => Ok(Self::parse(uc, &result)),
            Ok(_) | Err(_) => {
                self.lp_fallback_count += 1;
                Ok(self.lp_level(uc, solver, now_ms, model))
            }
        }
    }

    /// 第 3 级：LP 松弛（对第 1 级完整模型去整数化）.
    ///
    /// 可接受状态 → 解析返回；仍失败 → 返回空计划 + 末级状态
    ///（Ok 取其 status，Err 转 `SolveStatus::Error`，不传播）。
    fn lp_level(
        &mut self,
        uc: &UnitCommitment,
        solver: &mut dyn Solver,
        now_ms: u64,
        model: &LpProblem,
    ) -> DayAheadPlan {
        let lp_model = Self::relax_lp(model);
        match solver.solve(&lp_model, now_ms) {
            Ok(result) if Self::is_acceptable(&result.status) => Self::parse(uc, &result),
            Ok(result) => Self::empty_plan(result.status),
            Err(e) => Self::empty_plan(SolveStatus::Error(format!("{}", e))),
        }
    }

    /// 可接受状态判定：最优 / 次优 / 超时（带可行解）均接受.
    fn is_acceptable(status: &SolveStatus) -> bool {
        matches!(
            status,
            SolveStatus::Optimal | SolveStatus::Suboptimal | SolveStatus::Timeout
        )
    }

    /// 三级全失败时的空计划（状态为末级求解状态）.
    fn empty_plan(status: SolveStatus) -> DayAheadPlan {
        DayAheadPlan {
            schedule: Vec::new(),
            total_cost: 0.0,
            solve_status: status,
        }
    }

    /// 解析被接受的求解结果为日前计划.
    ///
    /// 解向量安全访问（`get().copied().unwrap_or(0.0)`），长度不足不 panic（D8）。
    fn parse(uc: &UnitCommitment, result: &SolveResult) -> DayAheadPlan {
        let mut schedule = Vec::with_capacity(uc.units.len());
        for (i, unit) in uc.units.iter().enumerate() {
            let mut commitments = Vec::with_capacity(uc.periods);
            let mut generation = Vec::with_capacity(uc.periods);
            for t in 0..uc.periods {
                let u_val = result
                    .solution
                    .get(uc.var_index(i, t, 1))
                    .copied()
                    .unwrap_or(0.0);
                commitments.push(u_val > 0.5);
                let p_val = result
                    .solution
                    .get(uc.var_index(i, t, 0))
                    .copied()
                    .unwrap_or(0.0);
                generation.push(p_val);
            }
            schedule.push(UnitSchedule {
                unit_id: unit.id.clone(),
                commitments,
                generation,
            });
        }
        DayAheadPlan {
            schedule,
            total_cost: result.objective_value,
            solve_status: result.status.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::problem::VarType;
    use eneros_solver_core::solver::SolverStatus;

    use super::*;
    use crate::uc_model::{UcUnit, UnitCommitment};

    // === 辅助 fixture ===

    /// 任务书固定机组参数：p_min=10 / p_max=100 / ramp=5 /
    /// start_cost=1000 / min_up=3 / min_down=2 / init=true.
    fn fixture_unit(id: &str) -> UcUnit {
        UcUnit {
            id: id.to_string(),
            p_min: 10.0,
            p_max: 100.0,
            ramp_up: 5.0,
            ramp_down: 5.0,
            start_cost: 1000.0,
            min_up: 3,
            min_down: 2,
            init_status: true,
        }
    }

    /// 5 机组（"G1".."G5"）× 24 周期 UC fixture.
    fn fixture_uc() -> UnitCommitment {
        let units = (1..=5).map(|i| fixture_unit(&format!("G{}", i))).collect();
        UnitCommitment::new(units, 24, 15)
    }

    /// fixture 负荷/电价曲线.
    fn fixture_load_price() -> (Vec<f64>, Vec<f64>) {
        (vec![300.0; 24], vec![0.5; 24])
    }

    /// 生成 5×24 全量解向量（480 维，各位置均为 `val`）.
    fn solution_5x24(val: f64) -> Vec<f64> {
        vec![val; 5 * 24 * 4]
    }

    /// 记录型求解器 stub：结果队列 + 参数注入记录 + 调用计数.
    ///
    /// `MockSolver::with_result` 每次返回同一结果，无法表达
    /// "第一次失败第二次成功"，故用本 stub（Vec + 游标，队尾后保持末值）。
    struct RecordingSolver {
        /// 预设结果队列（按次消费，耗尽后重复末值）.
        results: Vec<Result<SolveResult, SolverError>>,
        /// 游标.
        cursor: usize,
        /// 已注入参数（key, value）.
        params: Vec<(String, String)>,
        /// solve 调用次数.
        solve_calls: u64,
        /// 首次 solve 时已注入的参数数量（验证注入先于求解）.
        params_at_first_solve: usize,
    }

    impl RecordingSolver {
        fn new(results: Vec<Result<SolveResult, SolverError>>) -> Self {
            Self {
                results,
                cursor: 0,
                params: Vec::new(),
                solve_calls: 0,
                params_at_first_solve: 0,
            }
        }
    }

    impl Solver for RecordingSolver {
        fn solve(
            &mut self,
            _problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<SolveResult, SolverError> {
            if self.solve_calls == 0 {
                self.params_at_first_solve = self.params.len();
            }
            self.solve_calls += 1;
            let idx = if self.cursor < self.results.len() {
                let i = self.cursor;
                self.cursor += 1;
                i
            } else {
                // 队列耗尽后重复末值（防御性；本测试组不会走到）
                self.results.len().saturating_sub(1)
            };
            self.results[idx].clone()
        }

        fn name(&self) -> &'static str {
            "RecordingSolver"
        }

        fn version(&self) -> &'static str {
            "0.1.0"
        }

        fn set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError> {
            self.params.push((key.to_string(), value.to_string()));
            Ok(())
        }

        fn status(&self) -> SolverStatus {
            SolverStatus::Idle
        }
    }

    /// 带指定状态的求解结果（solution/elapsed/dual 随意填）.
    fn result_with(status: SolveStatus) -> SolveResult {
        SolveResult {
            status,
            objective_value: 0.0,
            solution: solution_5x24(1.0),
            elapsed_ms: 0,
            dual_solution: None,
        }
    }

    // === TD16: 端到端 5×24 — Optimal 一次通过，计划结构完整，计数器双 0 ===
    #[test]
    fn td16_e2e_5x24() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = MockSolver::with_result(SolveResult::optimal(1234.5, solution_5x24(1.0)));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.schedule.len(), 5);
        for us in &plan.schedule {
            assert_eq!(us.commitments.len(), 24);
            assert_eq!(us.generation.len(), 24);
        }
        assert_eq!(sched.relax_count, 0);
        assert_eq!(sched.lp_fallback_count, 0);
    }

    // === TD17: commitments 阈值判定 — >0.5 严格大于 ===
    #[test]
    fn td17_commitments_threshold() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut sol = solution_5x24(0.0);
        sol[uc.var_index(0, 0, 1)] = 0.8; // → true
        sol[uc.var_index(0, 1, 1)] = 0.2; // → false
        sol[uc.var_index(0, 2, 1)] = 0.5; // → false（严格大于）
        let mut solver = MockSolver::with_result(SolveResult::optimal(0.0, sol));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert!(plan.schedule[0].commitments[0]);
        assert!(!plan.schedule[0].commitments[1]);
        assert!(!plan.schedule[0].commitments[2]);
    }

    // === TD18: generation 解析 — P 位解值透传 ===
    #[test]
    fn td18_generation_parse() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut sol = solution_5x24(0.0);
        sol[uc.var_index(0, 3, 0)] = 50.0;
        let mut solver = MockSolver::with_result(SolveResult::optimal(0.0, sol));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.schedule[0].generation[3], 50.0);
    }

    // === TD19: total_cost 取目标函数值 ===
    #[test]
    fn td19_total_cost() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = MockSolver::with_result(SolveResult::optimal(1234.5, solution_5x24(1.0)));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.total_cost, 1234.5);
    }

    // === TD20: 求解状态透传（Optimal / Suboptimal 均可接受并透传） ===
    #[test]
    fn td20_status_propagated() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = MockSolver::with_result(SolveResult::optimal(1.0, solution_5x24(1.0)));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        // Suboptimal 同样可接受并透传
        let mut solver2 = MockSolver::with_result(result_with(SolveStatus::Suboptimal));
        let plan2 = sched.plan(&uc, &load, &price, &mut solver2, 0).unwrap();
        assert_eq!(plan2.solve_status, SolveStatus::Suboptimal);
        // 两次均为可接受状态，计数器保持双 0
        assert_eq!(sched.relax_count, 0);
        assert_eq!(sched.lp_fallback_count, 0);
    }

    // === TD21: Infeasible 触发松弛重建（第 2 级 Optimal 接受） ===
    #[test]
    fn td21_infeasible_triggers_relax() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![
            Ok(result_with(SolveStatus::Infeasible)),
            Ok(result_with(SolveStatus::Optimal)),
        ]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(sched.relax_count, 1);
        assert_eq!(sched.lp_fallback_count, 0);
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        assert_eq!(solver.solve_calls, 2);
    }

    // === TD22: 松弛仍失败 → LP 松弛（第 3 级 Optimal 接受），双计数器各 1 ===
    #[test]
    fn td22_relax_fail_triggers_lp() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![
            Ok(result_with(SolveStatus::Infeasible)),
            Ok(result_with(SolveStatus::Infeasible)),
            Ok(result_with(SolveStatus::Optimal)),
        ]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(sched.relax_count, 1);
        assert_eq!(sched.lp_fallback_count, 1);
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        assert_eq!(solver.solve_calls, 3);
    }

    // === TD23: relax_lp — 类型全 Continuous；原 Binary 位上界 1.0；P 位上界 p_max ===
    #[test]
    fn td23_relax_lp_types() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let model = uc.build_model(&load, &price).unwrap();
        let relaxed = DayAheadScheduler::relax_lp(&model);
        // var_types 全 Continuous
        assert!(relaxed
            .var_types
            .iter()
            .all(|&vt| vt == VarType::Continuous));
        // 原 Binary 位（k=1/2/3）上界保持 1.0
        for k in 1..=3 {
            let idx = uc.var_index(2, 7, k);
            assert_eq!(model.var_types[idx], VarType::Binary);
            assert_eq!(relaxed.upper_bounds[idx], 1.0);
        }
        // P 位上界仍 p_max（fixture 100.0），下界 0.0
        let p_idx = uc.var_index(2, 7, 0);
        assert_eq!(relaxed.upper_bounds[p_idx], 100.0);
        assert_eq!(relaxed.lower_bounds[p_idx], 0.0);
        // objective/constraints/rhs 与源一致（抽查）
        assert_eq!(relaxed.objective[..], model.objective[..]);
        assert_eq!(relaxed.constraints.values[..], model.constraints.values[..]);
        assert_eq!(relaxed.rhs_lower[..], model.rhs_lower[..]);
        assert_eq!(relaxed.rhs_upper[..], model.rhs_upper[..]);
        assert_eq!(relaxed.sense, model.sense);
        // 源模型未被修改
        assert_eq!(model.var_types[uc.var_index(2, 7, 1)], VarType::Binary);
    }

    // === TD24: Err(RunFailed) 触发降级链（不传播 Err） ===
    #[test]
    fn td24_err_triggers_chain() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![
            Err(SolverError::RunFailed(-1)),
            Ok(result_with(SolveStatus::Optimal)),
        ]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(sched.relax_count, 1);
        assert_eq!(sched.lp_fallback_count, 0);
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        assert_eq!(solver.solve_calls, 2);
    }

    // === TD25: 单次 Optimal — 不触发任何降级 ===
    #[test]
    fn td25_optimal_no_fallback() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![Ok(result_with(SolveStatus::Optimal))]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(sched.relax_count, 0);
        assert_eq!(sched.lp_fallback_count, 0);
        assert_eq!(solver.solve_calls, 1);
        assert_eq!(plan.schedule.len(), 5);
    }

    // === TD26: schedule 顺序与机组顺序一致（unit_id "G1".."G5"） ===
    #[test]
    fn td26_unit_id_order() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = MockSolver::with_result(SolveResult::optimal(0.0, solution_5x24(1.0)));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        for (i, us) in plan.schedule.iter().enumerate() {
            assert_eq!(us.unit_id, format!("G{}", i + 1));
        }
    }

    // === TD27: 2 机组 × 3 周期手工解向量定位解析 ===
    #[test]
    fn td27_small_2x3_hand_mapped() {
        let units = vec![fixture_unit("A"), fixture_unit("B")];
        let uc = UnitCommitment::new(units, 3, 15);
        let load = vec![100.0; 3];
        let price = vec![0.5; 3];
        let mut sol = vec![0.0; 2 * 3 * 4];
        // 机组 0 全周期：U=1.0、P=50.0
        for t in 0..3 {
            sol[uc.var_index(0, t, 1)] = 1.0;
            sol[uc.var_index(0, t, 0)] = 50.0;
        }
        // 机组 1 全周期：U=0.0（停机）、P=0.0
        let mut solver = MockSolver::with_result(SolveResult::optimal(7.5, sol));
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.schedule.len(), 2);
        assert!(plan.schedule[0].commitments[0]);
        assert!(plan.schedule[0].commitments[2]);
        assert_eq!(plan.schedule[0].generation[0], 50.0);
        assert_eq!(plan.schedule[0].generation[2], 50.0);
        assert!(!plan.schedule[1].commitments[0]);
        assert!(!plan.schedule[1].commitments[2]);
        assert_eq!(plan.schedule[1].generation[0], 0.0);
        assert_eq!(plan.total_cost, 7.5);
    }

    // === TD28: 完整降级链 — Err → Infeasible → LP 松弛 Optimal ===
    #[test]
    fn td28_full_chain_lp_ok() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![
            Err(SolverError::RunFailed(-1)),
            Ok(result_with(SolveStatus::Infeasible)),
            Ok(result_with(SolveStatus::Optimal)),
        ]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(sched.relax_count, 1);
        assert_eq!(sched.lp_fallback_count, 1);
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        assert!(!plan.schedule.is_empty());
        assert_eq!(solver.solve_calls, 3);
    }

    // === TD29: 10 机组 × 24 周期模型构建性能 < 1s（行数 1684） ===
    #[test]
    fn td29_build_perf_10x24() {
        let units = (1..=10).map(|i| fixture_unit(&format!("G{}", i))).collect();
        let uc = UnitCommitment::new(units, 24, 15);
        let load = vec![600.0; 24];
        let price = vec![0.5; 24];
        let start = std::time::Instant::now();
        let lp = uc.build_model(&load, &price).unwrap();
        let elapsed = start.elapsed();
        // 行数 = t + 5nt + 2n(t−1) = 24 + 5·10·24 + 2·10·23 = 24 + 1200 + 460
        assert_eq!(lp.rhs_lower.len(), 1684);
        assert!(
            elapsed.as_millis() < 1000,
            "10×24 模型构建耗时 {:?} 超阈值 1s",
            elapsed
        );
    }

    // === TD30: 参数注入 — time_limit/mip_rel_gap 在 solve 之前注入 ===
    #[test]
    fn td30_param_injected() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![Ok(result_with(SolveStatus::Optimal))]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert_eq!(plan.solve_status, SolveStatus::Optimal);
        // 键存在且值与 format!("{}", f64) 输出一致（30.0 → "30"，0.01 → "0.01"）
        assert!(solver
            .params
            .iter()
            .any(|(k, v)| k == "time_limit" && v == &format!("{}", 30.0f64)));
        assert!(solver
            .params
            .iter()
            .any(|(k, v)| k == "mip_rel_gap" && v == &format!("{}", 0.01f64)));
        // 注入发生在首次 solve 之前
        assert!(solver.params_at_first_solve >= 2);
    }

    // === TD31: 三级全 Err → Ok 空计划 + 末级 Error 状态 ===
    #[test]
    fn td31_all_fail_empty_plan() {
        let uc = fixture_uc();
        let (load, price) = fixture_load_price();
        let mut solver = RecordingSolver::new(vec![
            Err(SolverError::RunFailed(-1)),
            Err(SolverError::RunFailed(-2)),
            Err(SolverError::RunFailed(-3)),
        ]);
        let mut sched = DayAheadScheduler::new(30.0, 0.01);
        let plan = sched.plan(&uc, &load, &price, &mut solver, 0).unwrap();
        assert!(plan.schedule.is_empty());
        assert_eq!(plan.total_cost, 0.0);
        assert!(matches!(plan.solve_status, SolveStatus::Error(_)));
        assert_eq!(sched.relax_count, 1);
        assert_eq!(sched.lp_fallback_count, 1);
        assert_eq!(solver.solve_calls, 3);
    }
}
