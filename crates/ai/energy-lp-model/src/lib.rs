//! EnerOS 能源调度 LP 模型（v0.66.0，P1-J Solver 第三层领域模型）.
//!
//! 基于 v0.65.0 `OptProblem` DSL 构建储能系统优化调度 LP 模型，包含：
//! - 决策变量：各时段充电功率 / 放电功率 / SOC（荷电量，kWh）
//! - 目标函数：最大化收益 = Σ (price·discharge - price·charge)·dt
//! - 约束：SOC 动态约束 / 爬坡约束 / SOC 初终值约束 / 容量约束
//!
//! # 核心类型
//!
//! - [`config::ScheduleConfig`] — 调度参数配置（时段数 / PCS 功率 / 电池容量 / SOC 上下限 / 爬坡率 / 效率 / 电价曲线）
//! - [`model::EnergyScheduleModel`] — 调度模型构建器（自动创建变量 + 约束 + 目标 + 编译 + 解析结果）
//! - [`result::ScheduleEntry`] / [`result::ScheduleResult`] — 调度结果条目与汇总
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `self.problem = std::mem::take(&mut self.problem).maximize(obj);` | 改用 `core::mem::take` | no_std 合规：`std::mem` 不可用 |
//! | **D2** | `format!("charge_{}", t)` | 依赖 `extern crate alloc` 的 `format!` 宏 | no_std 合规 |
//! | **D3** | **蓝图 Bug**：放电系数 `eff_d * dt / cap * cap` | 改为 `dt / eff_d` | **数学错误**：根据 §5 公式 `soc[t] = soc[t-1] + (charge[t]·η_c - discharge[t]/η_d)·dt`，放电项系数应为 `dt/η_d` 而非 `η_d·dt`；且 `/cap * cap` 相互抵消无意义 |
//! | **D4** | `result.solution[idx]` 直接索引 | 改用 `result.solution.get(idx).copied().unwrap_or(0.0)` | no_std panic 不可恢复，安全访问 |
//! | **D5** | 前置依赖 v0.52.0 四遥数据模型 | **不引入 crate 依赖** | `ScheduleConfig` 自带数据，与 telemetry-model 解耦 |
//! | **D6** | 蓝图未明确 crate 位置 | `crates/ai/energy-lp-model/` | 项目规则 §2.3.1：AI 子系统 |
//! | **D7** | 蓝图 §6.2 "谷充峰放求解" | 用 `MockSolver` 做端到端验证 | 与 v0.64.0/v0.65.0 一致，避免 HiGHS C 库依赖 |
//! | **D8** | 蓝图重定义 `LpProblem`/`SolverError`/`SolveResult`/`SolveStatus` | 复用 v0.64.0 `eneros-solver-core` 类型 | 避免类型重定义导致 `compile()` 返回值不匹配 |
//! | **D9** | 蓝图重定义 `OptProblem`/`VarBuilder`/`LinearExpr`/`Constraint` | 复用 v0.65.0 `eneros-solver-model` 类型 | 同 D8 理由 |
//! | **D10** | 蓝图派生 `Debug` + `Clone` | 保持一致，不额外派生 `PartialEq` | Karpathy "Simplicity First"：当前测试不需要 |
//! | **D11** | 蓝图未声明 `[features]` | 不声明 `[features]` | 纯 Rust，无 FFI |
//! | **D12** | 蓝图 `SafetyRule: Send + Sync`（line 13987） | **不适用**（该 trait 属于 v0.67.0） | v0.66.0 不实现 `SafetyRule` |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod config;
pub mod model;
pub mod result;

pub use config::ScheduleConfig;
pub use model::EnergyScheduleModel;
pub use result::{ScheduleEntry, ScheduleResult};

#[cfg(test)]
mod tests {
    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::problem::ObjectiveSense;
    use eneros_solver_core::result::{SolveResult, SolveStatus};
    use eneros_solver_core::solver::Solver;
    use eneros_solver_model::constraint::Constraint;

    use super::*;

    // === 辅助函数 ===

    /// 统计 Eq 约束中 terms 数量等于 n_terms 的个数.
    fn count_eq_with_terms(model: &EnergyScheduleModel, n_terms: usize) -> usize {
        model
            .problem
            .constraints
            .iter()
            .filter(|c| {
                if let Constraint::Eq(e, _) = c {
                    e.terms.len() == n_terms
                } else {
                    false
                }
            })
            .count()
    }

    /// 统计 Le 约束中 terms 数量等于 n_terms 的个数.
    fn count_le_with_terms(model: &EnergyScheduleModel, n_terms: usize) -> usize {
        model
            .problem
            .constraints
            .iter()
            .filter(|c| {
                if let Constraint::Le(e, _) = c {
                    e.terms.len() == n_terms
                } else {
                    false
                }
            })
            .count()
    }

    // === T1: ScheduleConfig::default() 字段验证 ===
    #[test]
    fn t1_schedule_config_default() {
        let c = ScheduleConfig::default();
        assert_eq!(c.num_periods, 96);
        assert_eq!(c.period_hours, 0.25);
        assert_eq!(c.pcs_power_kw, 100.0);
        assert_eq!(c.battery_capacity_kwh, 200.0);
        assert_eq!(c.soc_min, 0.1);
        assert_eq!(c.soc_max, 0.9);
        assert_eq!(c.soc_init, 0.5);
        assert!(c.soc_final.is_none());
        assert_eq!(c.charge_ramp_kw, 50.0);
        assert_eq!(c.discharge_ramp_kw, 50.0);
        assert_eq!(c.charge_efficiency, 0.95);
        assert_eq!(c.discharge_efficiency, 0.95);
        assert_eq!(c.price.len(), 96);
        assert_eq!(c.price[0], 0.5);
        assert!(c.load_demand.is_none());
    }

    // === T2: ScheduleConfig 字段访问 + Clone ===
    #[test]
    fn t2_schedule_config_clone() {
        let c = ScheduleConfig::default();
        let cloned = c.clone();
        assert_eq!(c.num_periods, cloned.num_periods);
        assert_eq!(c.period_hours, cloned.period_hours);
        assert_eq!(c.pcs_power_kw, cloned.pcs_power_kw);
        assert_eq!(c.battery_capacity_kwh, cloned.battery_capacity_kwh);
        assert_eq!(c.soc_min, cloned.soc_min);
        assert_eq!(c.soc_max, cloned.soc_max);
        assert_eq!(c.soc_init, cloned.soc_init);
        assert_eq!(c.soc_final, cloned.soc_final);
        assert_eq!(c.charge_ramp_kw, cloned.charge_ramp_kw);
        assert_eq!(c.discharge_ramp_kw, cloned.discharge_ramp_kw);
        assert_eq!(c.charge_efficiency, cloned.charge_efficiency);
        assert_eq!(c.discharge_efficiency, cloned.discharge_efficiency);
        assert_eq!(c.price.len(), cloned.price.len());
        assert_eq!(c.price[0], cloned.price[0]);
        assert_eq!(c.load_demand, cloned.load_demand);
    }

    // === T3: EnergyScheduleModel::new 创建 3×n 变量 ===
    #[test]
    fn t3_model_new_variable_counts() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        assert_eq!(model.charge_var_idx.len(), 96);
        assert_eq!(model.discharge_var_idx.len(), 96);
        assert_eq!(model.soc_var_idx.len(), 96);
    }

    // === T4: 变量索引正确 — charge 0..96, discharge 96..192, soc 192..288 ===
    #[test]
    fn t4_variable_indices() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        for t in 0..96 {
            assert_eq!(model.charge_var_idx[t], t);
            assert_eq!(model.discharge_var_idx[t], 96 + t);
            assert_eq!(model.soc_var_idx[t], 192 + t);
        }
    }

    // === T5: model.compile() 返回 Ok ===
    #[test]
    fn t5_compile_ok() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        assert!(model.compile().is_ok());
    }

    // === T6: LpProblem 变量数 = 288 ===
    #[test]
    fn t6_lp_variable_count() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let lp = model.compile().unwrap();
        assert_eq!(lp.variables.len(), 288);
    }

    // === T7: CSR row_start.len() == num_constraints + 1 ===
    #[test]
    fn t7_csr_row_start_length() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let lp = model.compile().unwrap();
        let num_constraints = lp.constraints.num_rows;
        assert_eq!(lp.constraints.row_start.len(), num_constraints + 1);
    }

    // === T8: SOC 动态约束数 = 95 ===
    #[test]
    fn t8_soc_dynamics_constraint_count() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        // SOC 动态约束 = Eq with 4 terms
        assert_eq!(count_eq_with_terms(&model, 4), 95);
    }

    // === T9: 爬坡约束数 = 190 ===
    #[test]
    fn t9_ramp_constraint_count() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        // 爬坡约束 = Le with 2 terms
        assert_eq!(count_le_with_terms(&model, 2), 190);
    }

    // === T10: SOC 初始约束数 = 1 ===
    #[test]
    fn t10_soc_init_constraint_count() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        // SOC init = Eq with 1 term；默认无 soc_final
        assert_eq!(count_eq_with_terms(&model, 1), 1);
    }

    // === T11: soc_final = Some(0.5) 时存在 SOC 终值约束 ===
    #[test]
    fn t11_soc_final_constraint_exists() {
        let config = ScheduleConfig {
            soc_final: Some(0.5),
            ..Default::default()
        };
        let model = EnergyScheduleModel::new(config);
        // 有 soc_final 时：Eq with 1 term = 2（soc_init + soc_final）
        assert_eq!(count_eq_with_terms(&model, 1), 2);
    }

    // === T12: 目标方向 = Maximize ===
    #[test]
    fn t12_objective_sense_maximize() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let lp = model.compile().unwrap();
        assert_eq!(lp.sense, ObjectiveSense::Maximize);
    }

    // === T13: parse_result 返回 schedule.len() == num_periods ===
    #[test]
    fn t13_parse_result_schedule_length() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let result = SolveResult::optimal(0.0, alloc::vec![0.0; 288]);
        let schedule = model.parse_result(&result);
        assert_eq!(schedule.schedule.len(), 96);
    }

    // === T14: parse_result soc_pct = soc / capacity ===
    #[test]
    fn t14_parse_result_soc_pct() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let mut solution = alloc::vec![0.0; 288];
        // soc[0] = 100.0 kWh, capacity = 200.0 → soc_pct = 0.5
        solution[192] = 100.0; // soc_var_idx[0] = 192
        let result = SolveResult::optimal(0.0, solution);
        let schedule = model.parse_result(&result);
        assert!((schedule.schedule[0].soc_pct - 0.5).abs() < 1e-9);
    }

    // === T15: parse_result revenue_yuan = (discharge - charge) * price * dt ===
    #[test]
    fn t15_parse_result_revenue() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let mut solution = alloc::vec![0.0; 288];
        // period 0: charge=10, discharge=50, price=0.5, dt=0.25
        // revenue = (50 - 10) * 0.5 * 0.25 = 5.0
        solution[0] = 10.0; // charge_var_idx[0] = 0
        solution[96] = 50.0; // discharge_var_idx[0] = 96
        let result = SolveResult::optimal(0.0, solution);
        let schedule = model.parse_result(&result);
        assert!((schedule.schedule[0].revenue_yuan - 5.0).abs() < 1e-9);
    }

    // === T16: parse_result total_revenue_yuan = 所有时段收益之和 ===
    #[test]
    fn t16_parse_result_total_revenue() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let mut solution = alloc::vec![0.0; 288];
        // period 0: charge=10, discharge=50 → revenue = (50-10)*0.5*0.25 = 5.0
        solution[0] = 10.0;
        solution[96] = 50.0;
        // period 1: charge=0, discharge=20 → revenue = (20-0)*0.5*0.25 = 2.5
        solution[97] = 20.0; // discharge_var_idx[1] = 97
        let result = SolveResult::optimal(0.0, solution);
        let schedule = model.parse_result(&result);
        let expected: f64 = schedule.schedule.iter().map(|e| e.revenue_yuan).sum();
        assert!((schedule.total_revenue_yuan - expected).abs() < 1e-9);
        assert!((schedule.total_revenue_yuan - 7.5).abs() < 1e-9);
    }

    // === T17: 端到端 new → compile → MockSolver.solve() → parse_result ===
    #[test]
    fn t17_end_to_end_with_mock_solver() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let lp = model.compile().unwrap();
        let result = SolveResult::optimal(42.0, alloc::vec![0.0; 288]);
        let mut solver = MockSolver::with_result(result);
        let solve_result = solver.solve(&lp, 0).unwrap();
        let schedule = model.parse_result(&solve_result);
        assert_eq!(schedule.schedule.len(), 96);
    }

    // === T18: 端到端返回 solve_status == Optimal ===
    #[test]
    fn t18_end_to_end_optimal_status() {
        let model = EnergyScheduleModel::new(ScheduleConfig::default());
        let lp = model.compile().unwrap();
        let result = SolveResult::optimal(42.0, alloc::vec![0.0; 288]);
        let mut solver = MockSolver::with_result(result);
        let solve_result = solver.solve(&lp, 0).unwrap();
        let schedule = model.parse_result(&solve_result);
        assert_eq!(schedule.solve_status, SolveStatus::Optimal);
    }

    // === T19: 小规模模型（4 时段）— 变量数 = 12，约束数正确 ===
    #[test]
    fn t19_small_scale_model() {
        let config = ScheduleConfig {
            num_periods: 4,
            period_hours: 0.25,
            pcs_power_kw: 100.0,
            battery_capacity_kwh: 200.0,
            soc_min: 0.1,
            soc_max: 0.9,
            soc_init: 0.5,
            soc_final: Some(0.5),
            charge_ramp_kw: 50.0,
            discharge_ramp_kw: 50.0,
            charge_efficiency: 0.95,
            discharge_efficiency: 0.95,
            price: alloc::vec![0.5; 4],
            load_demand: None,
        };
        let model = EnergyScheduleModel::new(config);
        // 变量：3 × 4 = 12
        assert_eq!(model.charge_var_idx.len(), 4);
        assert_eq!(model.discharge_var_idx.len(), 4);
        assert_eq!(model.soc_var_idx.len(), 4);
        let lp = model.compile().unwrap();
        assert_eq!(lp.variables.len(), 12);
        // 约束：soc_dynamics(3) + ramp(6) + soc_init(1) + soc_final(1) = 11
        assert_eq!(lp.constraints.num_rows, 11);
    }

    // === T20: 小规模模型 compile + MockSolver 端到端 ===
    #[test]
    fn t20_small_scale_end_to_end() {
        let config = ScheduleConfig {
            num_periods: 4,
            period_hours: 0.25,
            pcs_power_kw: 100.0,
            battery_capacity_kwh: 200.0,
            soc_min: 0.1,
            soc_max: 0.9,
            soc_init: 0.5,
            soc_final: Some(0.5),
            charge_ramp_kw: 50.0,
            discharge_ramp_kw: 50.0,
            charge_efficiency: 0.95,
            discharge_efficiency: 0.95,
            price: alloc::vec![0.5; 4],
            load_demand: None,
        };
        let model = EnergyScheduleModel::new(config);
        let lp = model.compile().unwrap();
        let result = SolveResult::optimal(10.0, alloc::vec![0.0; 12]);
        let mut solver = MockSolver::with_result(result);
        let solve_result = solver.solve(&lp, 0).unwrap();
        let schedule = model.parse_result(&solve_result);
        assert_eq!(schedule.schedule.len(), 4);
        assert_eq!(schedule.solve_status, SolveStatus::Optimal);
    }

    // === T21: ScheduleEntry 字段访问 ===
    #[test]
    fn t21_schedule_entry_field_access() {
        let entry = ScheduleEntry {
            period: 3,
            charge_power_kw: 10.0,
            discharge_power_kw: 50.0,
            net_power_kw: 40.0,
            soc_pct: 0.5,
            revenue_yuan: 5.0,
        };
        assert_eq!(entry.period, 3);
        assert_eq!(entry.charge_power_kw, 10.0);
        assert_eq!(entry.discharge_power_kw, 50.0);
        assert_eq!(entry.net_power_kw, 40.0);
        assert_eq!(entry.soc_pct, 0.5);
        assert_eq!(entry.revenue_yuan, 5.0);
    }

    // === T22: ScheduleResult 字段访问 + Clone ===
    #[test]
    fn t22_schedule_result_clone() {
        let result = ScheduleResult {
            schedule: alloc::vec![ScheduleEntry {
                period: 0,
                charge_power_kw: 10.0,
                discharge_power_kw: 50.0,
                net_power_kw: 40.0,
                soc_pct: 0.5,
                revenue_yuan: 5.0,
            }],
            total_revenue_yuan: 5.0,
            objective_value: 42.0,
            solve_status: SolveStatus::Optimal,
        };
        let cloned = result.clone();
        assert_eq!(cloned.schedule.len(), 1);
        assert_eq!(cloned.total_revenue_yuan, 5.0);
        assert_eq!(cloned.objective_value, 42.0);
        assert_eq!(cloned.solve_status, SolveStatus::Optimal);
        assert_eq!(cloned.schedule[0].period, 0);
        assert_eq!(cloned.schedule[0].charge_power_kw, 10.0);
        assert_eq!(cloned.schedule[0].discharge_power_kw, 50.0);
        assert_eq!(cloned.schedule[0].net_power_kw, 40.0);
        assert_eq!(cloned.schedule[0].soc_pct, 0.5);
        assert_eq!(cloned.schedule[0].revenue_yuan, 5.0);
    }
}
