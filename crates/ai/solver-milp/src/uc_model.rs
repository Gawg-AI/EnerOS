//! 机组组合（Unit Commitment, UC）MILP 模型构建模块（v0.102.0，D6/D7/D8）.
//!
//! 基于 v0.65.0 `OptProblem` DSL 构建标准 UC 日前调度 MILP 模型，编译为
//! v0.64.0 `LpProblem`（CSR 矩阵格式，含 Binary 变量类型）。
//!
//! # 变量布局
//!
//! 每机组每时段四元组（k=0 P 出力 / k=1 U 运行状态 / k=2 V 启动动作 /
//! k=3 W 停机动作），索引公式：`(i * periods + t) * 4 + k`。
//!
//! # 约束组与行区间（严格按添加顺序）
//!
//! 设 n = 机组数，t = 周期数：
//!
//! | 组 | 行区间 | 行数 | 形式 |
//! |----|--------|------|------|
//! | a. 功率平衡 | `[0, t)` | t | `Eq(Σ_i P[i,t], load[t])` |
//! | b. pmax 联动 | `[t, t+nt)` | nt | `Le(P - p_max·U, 0)` |
//! | c. pmin 联动 | `[t+nt, t+2nt)` | nt | `Ge(P - p_min·U, 0)` |
//! | d. 爬坡上行 | `[t+2nt, t+2nt+n(t−1))` | n(t−1) | `Le(P[t]−P[t−1], ramp_up·interval_min)` |
//! | e. 爬坡下行 | `[t+2nt+n(t−1), t+2nt+2n(t−1))` | n(t−1) | `Le(P[t−1]−P[t], ramp_down·interval_min)` |
//! | f. 启停逻辑 | 接下 | nt | t=0: `Eq(V−W−U, −init)`；t≥1: `Eq(V−W−U[t]+U[t−1], 0)` |
//! | g. 最小运行 | 接下 | nt | `Le(Σ_τ V[τ] − U[t], 0)`，窗口 τ∈[max(0,t+1−min_up), t] |
//! | h. 最小停机 | 末 nt 行 | nt | `Le(Σ_τ W[τ] + U[t], 1)`，窗口同上（min_down） |
//!
//! 组内按机组优先（i 外层、t 内层）排序。
//!
//! **行数说明（任务书算术修正）**：上表 8 组枚举合计 `t + 5nt + 2n(t−1)`
//! （5×24 → 24 + 600 + 230 = **854**），松弛模型跳过 g/h 两组后为
//! `t + 3nt + 2n(t−1)`（5×24 → **614**）。任务书正文公式 "`t + 6nt + 2n(t−1)`
//! → 974" 与其自身枚举的 8 组约束（a~h，含 5 个 nt 组）不一致——974 与
//! "relaxed = 974 − 2nt = 614" 亦无法同时成立（974 − 240 = 730），而
//! 854 − 240 = 614 自洽，故以 8 组枚举为准，TU12/TU15 按 854/614 断言。
//!
//! # D6 蓝图 Bug 修正
//!
//! 蓝图 §4.5 `col_cost[base+1] = start_cost` 注释自称"V 启动成本"，但
//! base+1 实为 U 列。本实现启动成本挂 V（k=2），U/W 目标系数为 0（TU5 锁定）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;
use eneros_solver_core::problem::LpProblem;
use eneros_solver_model::constraint::Constraint;
use eneros_solver_model::expr::LinearExpr;
use eneros_solver_model::problem::OptProblem;
use eneros_solver_model::variable::VarBuilder;

/// UC 机组参数.
#[derive(Debug, Clone)]
pub struct UcUnit {
    /// 机组标识.
    pub id: String,
    /// 最小技术出力（MW，U=1 时生效）.
    pub p_min: f64,
    /// 额定最大出力（MW）.
    pub p_max: f64,
    /// 上行爬坡率（MW/min）.
    pub ramp_up: f64,
    /// 下行爬坡率（MW/min）.
    pub ramp_down: f64,
    /// 单次启动成本（元，挂 V 变量，D6）.
    pub start_cost: f64,
    /// 最小运行周期数（0 按 1 处理）.
    pub min_up: usize,
    /// 最小停机周期数（0 按 1 处理）.
    pub min_down: usize,
    /// 初始运行状态（t=0 之前是否在线）.
    pub init_status: bool,
}

/// UC 模型构建器.
///
/// 负责变量创建、目标函数、约束组装配与编译；`build_model` 输出完整
/// MILP 模型，`build_model_relaxed` 输出跳过最小运行/停机约束的松弛模型
/// （供 D9 降级链第一级使用）。
#[derive(Debug, Clone)]
pub struct UnitCommitment {
    /// 机组列表.
    pub units: Vec<UcUnit>,
    /// 调度周期数.
    pub periods: usize,
    /// 单周期时长（分钟，参与爬坡约束量纲换算：MW/min × min = MW/周期，D12）.
    pub interval_min: u32,
}

impl UnitCommitment {
    /// 创建 UC 模型构建器.
    pub fn new(units: Vec<UcUnit>, periods: usize, interval_min: u32) -> Self {
        Self {
            units,
            periods,
            interval_min,
        }
    }

    /// 决策变量总数 = 机组数 × 周期数 × 4（P/U/V/W）.
    pub fn num_vars(&self) -> usize {
        self.units.len() * self.periods * 4
    }

    /// 变量索引公式：`(unit * periods + period) * 4 + kind`.
    ///
    /// kind：0=P，1=U，2=V，3=W。
    pub fn var_index(&self, unit: usize, period: usize, kind: usize) -> usize {
        (unit * self.periods + period) * 4 + kind
    }

    /// 构建完整 UC MILP 模型（含最小运行/停机约束）.
    ///
    /// D8：长度校验失败返回 `SolverError::InvalidProblem`，不 panic。
    pub fn build_model(&self, load: &[f64], price: &[f64]) -> Result<LpProblem, SolverError> {
        self.build(load, price, true)
    }

    /// 构建松弛 UC 模型（跳过最小运行/停机约束，D9 降级链第一级）.
    ///
    /// 约束行为完整模型的严格前缀：`t + 3nt + 2n(t−1)` 行。
    pub fn build_model_relaxed(
        &self,
        load: &[f64],
        price: &[f64],
    ) -> Result<LpProblem, SolverError> {
        self.build(load, price, false)
    }

    /// 内部统一构建入口（`include_min_time=false` 时跳过 g/h 两组约束）.
    fn build(
        &self,
        load: &[f64],
        price: &[f64],
        include_min_time: bool,
    ) -> Result<LpProblem, SolverError> {
        let n = self.units.len();
        let t = self.periods;

        // D8：输入长度校验（no_std 禁 panic，返回错误）
        if load.len() != t {
            return Err(SolverError::InvalidProblem(format!(
                "load 长度 {} 与周期数 {} 不一致",
                load.len(),
                t
            )));
        }
        if price.len() != t {
            return Err(SolverError::InvalidProblem(format!(
                "price 长度 {} 与周期数 {} 不一致",
                price.len(),
                t
            )));
        }

        let mut problem = OptProblem::new();

        // 1. 决策变量：每 (i,t) 四元组 P/U/V/W，添加顺序严格对齐 var_index 布局
        //    （OptProblem::add_var 按添加顺序分配索引，compile 后列索引一致）
        for (i, unit) in self.units.iter().enumerate() {
            for tt in 0..t {
                problem.add_var(
                    VarBuilder::new(&format!("P_{}_{}", i, tt))
                        .range(0.0, unit.p_max)
                        .build(),
                );
                problem.add_var(VarBuilder::new(&format!("U_{}_{}", i, tt)).binary().build());
                problem.add_var(VarBuilder::new(&format!("V_{}_{}", i, tt)).binary().build());
                problem.add_var(VarBuilder::new(&format!("W_{}_{}", i, tt)).binary().build());
            }
        }

        // 2. 目标函数（Minimize）：Σ_{i,t} ( price[t]·P[i,t] + start_cost_i·V[i,t] )
        //    D6：启动成本挂 V（k=2），U/W 系数 0
        let mut obj = LinearExpr::new();
        for (i, unit) in self.units.iter().enumerate() {
            for (tt, &p) in price.iter().enumerate() {
                obj.add_term(self.var_index(i, tt, 0), p);
                obj.add_term(self.var_index(i, tt, 2), unit.start_cost);
            }
        }
        problem = problem.minimize(obj);

        // 3a. 功率平衡（t 行）：Σ_i P[i,t] == load[t]
        for (tt, &l) in load.iter().enumerate() {
            let mut expr = LinearExpr::new();
            for i in 0..n {
                expr.add_term(self.var_index(i, tt, 0), 1.0);
            }
            problem.add_constraint(&format!("balance_{}", tt), Constraint::Eq(expr, l));
        }

        // 3b. pmax 联动（nt 行）：P[i,t] - p_max_i·U[i,t] <= 0
        for (i, unit) in self.units.iter().enumerate() {
            for tt in 0..t {
                let mut expr = LinearExpr::new();
                expr.add_term(self.var_index(i, tt, 0), 1.0);
                expr.add_term(self.var_index(i, tt, 1), -unit.p_max);
                problem.add_constraint(&format!("pmax_{}_{}", i, tt), Constraint::Le(expr, 0.0));
            }
        }

        // 3c. pmin 联动（nt 行）：P[i,t] - p_min_i·U[i,t] >= 0
        for (i, unit) in self.units.iter().enumerate() {
            for tt in 0..t {
                let mut expr = LinearExpr::new();
                expr.add_term(self.var_index(i, tt, 0), 1.0);
                expr.add_term(self.var_index(i, tt, 1), -unit.p_min);
                problem.add_constraint(&format!("pmin_{}_{}", i, tt), Constraint::Ge(expr, 0.0));
            }
        }

        // 3d. 爬坡上行（n(t−1) 行）：P[i,t] - P[i,t−1] <= ramp_up_i · interval_min
        //     D12：ramp 单位 MW/min，乘 interval_min 得 MW/周期
        let dt_min = f64::from(self.interval_min);
        for (i, unit) in self.units.iter().enumerate() {
            for tt in 1..t {
                let mut expr = LinearExpr::new();
                expr.add_term(self.var_index(i, tt, 0), 1.0);
                expr.add_term(self.var_index(i, tt - 1, 0), -1.0);
                problem.add_constraint(
                    &format!("ramp_up_{}_{}", i, tt),
                    Constraint::Le(expr, unit.ramp_up * dt_min),
                );
            }
        }

        // 3e. 爬坡下行（n(t−1) 行）：P[i,t−1] - P[i,t] <= ramp_down_i · interval_min
        for (i, unit) in self.units.iter().enumerate() {
            for tt in 1..t {
                let mut expr = LinearExpr::new();
                expr.add_term(self.var_index(i, tt - 1, 0), 1.0);
                expr.add_term(self.var_index(i, tt, 0), -1.0);
                problem.add_constraint(
                    &format!("ramp_down_{}_{}", i, tt),
                    Constraint::Le(expr, unit.ramp_down * dt_min),
                );
            }
        }

        // 3f. 启停逻辑（nt 行）：V[i,t] - W[i,t] == U[i,t] - U[i,t−1]
        //     t=0 时 U[i,−1] 由 init_status 代入：V − W − U == −init
        for (i, unit) in self.units.iter().enumerate() {
            let init = if unit.init_status { 1.0 } else { 0.0 };
            for tt in 0..t {
                let mut expr = LinearExpr::new();
                expr.add_term(self.var_index(i, tt, 2), 1.0); // V[t]
                expr.add_term(self.var_index(i, tt, 3), -1.0); // W[t]
                expr.add_term(self.var_index(i, tt, 1), -1.0); // U[t]
                if tt == 0 {
                    problem.add_constraint(
                        &format!("startup_{}_{}", i, tt),
                        Constraint::Eq(expr, -init),
                    );
                } else {
                    expr.add_term(self.var_index(i, tt - 1, 1), 1.0); // U[t−1]
                    problem.add_constraint(
                        &format!("startup_{}_{}", i, tt),
                        Constraint::Eq(expr, 0.0),
                    );
                }
            }
        }

        if include_min_time {
            // 3g. 最小运行（nt 行）：Σ_{τ∈窗口} V[i,τ] <= U[i,t]
            //     窗口 τ ∈ [max(0, t+1−min_up_eff), t]；min_up_eff = max(1, min_up)
            for (i, unit) in self.units.iter().enumerate() {
                let min_up_eff = unit.min_up.max(1);
                for tt in 0..t {
                    let lo = (tt + 1).saturating_sub(min_up_eff);
                    let mut expr = LinearExpr::new();
                    for tau in lo..=tt {
                        expr.add_term(self.var_index(i, tau, 2), 1.0); // V[τ]
                    }
                    expr.add_term(self.var_index(i, tt, 1), -1.0); // U[t]
                    problem
                        .add_constraint(&format!("min_up_{}_{}", i, tt), Constraint::Le(expr, 0.0));
                }
            }

            // 3h. 最小停机（nt 行）：Σ_{τ∈窗口} W[i,τ] <= 1 − U[i,t]
            //     即 Σ W + U <= 1；窗口同上（min_down_eff）
            for (i, unit) in self.units.iter().enumerate() {
                let min_down_eff = unit.min_down.max(1);
                for tt in 0..t {
                    let lo = (tt + 1).saturating_sub(min_down_eff);
                    let mut expr = LinearExpr::new();
                    for tau in lo..=tt {
                        expr.add_term(self.var_index(i, tau, 3), 1.0); // W[τ]
                    }
                    expr.add_term(self.var_index(i, tt, 1), 1.0); // U[t]
                    problem.add_constraint(
                        &format!("min_down_{}_{}", i, tt),
                        Constraint::Le(expr, 1.0),
                    );
                }
            }
        }

        problem.compile()
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_solver_core::error::SolverError;
    use eneros_solver_core::problem::{ObjectiveSense, VarType};

    use super::*;

    // === 辅助函数 ===

    /// 构造单台机组（全参数显式指定）.
    #[allow(clippy::too_many_arguments)] // 测试辅助：9 个字段一一对应，不引入构建器
    fn sample_unit(
        id: &str,
        p_min: f64,
        p_max: f64,
        ramp_up: f64,
        ramp_down: f64,
        start_cost: f64,
        min_up: usize,
        min_down: usize,
        init_status: bool,
    ) -> UcUnit {
        UcUnit {
            id: id.to_string(),
            p_min,
            p_max,
            ramp_up,
            ramp_down,
            start_cost,
            min_up,
            min_down,
            init_status,
        }
    }

    /// 5 机组 × 24 周期 fixture：机组 i 参数随 i 递增，init 奇偶交替.
    fn sample_uc() -> UnitCommitment {
        let units = (0..5)
            .map(|i| {
                sample_unit(
                    &format!("G{}", i),
                    50.0 + 10.0 * i as f64,
                    200.0 + 20.0 * i as f64,
                    5.0,
                    5.0,
                    100.0 + 10.0 * i as f64,
                    2,
                    2,
                    i % 2 == 0,
                )
            })
            .collect();
        UnitCommitment::new(units, 24, 15)
    }

    /// fixture 负荷/电价曲线.
    fn sample_load_price() -> (Vec<f64>, Vec<f64>) {
        (vec![600.0; 24], vec![0.5; 24])
    }

    // === TU1: UcUnit 构造字段逐项断言 ===
    #[test]
    fn tu1_uc_unit_fields() {
        let u = sample_unit("G1", 50.0, 200.0, 5.0, 4.0, 120.0, 3, 2, true);
        assert_eq!(u.id, "G1");
        assert_eq!(u.p_min, 50.0);
        assert_eq!(u.p_max, 200.0);
        assert_eq!(u.ramp_up, 5.0);
        assert_eq!(u.ramp_down, 4.0);
        assert_eq!(u.start_cost, 120.0);
        assert_eq!(u.min_up, 3);
        assert_eq!(u.min_down, 2);
        assert!(u.init_status);
    }

    // === TU2: UnitCommitment::new 三字段 + num_vars/var_index 公式 ===
    #[test]
    fn tu2_unit_commitment_new() {
        let uc = sample_uc();
        assert_eq!(uc.units.len(), 5);
        assert_eq!(uc.periods, 24);
        assert_eq!(uc.interval_min, 15);
        assert_eq!(uc.num_vars(), 5 * 24 * 4);
        assert_eq!(uc.var_index(0, 0, 0), 0);
        assert_eq!(uc.var_index(1, 2, 3), (24 + 2) * 4 + 3);
        assert_eq!(uc.var_index(4, 23, 3), 479);
    }

    // === TU3: 5×24 变量数 == 480 ===
    #[test]
    fn tu3_build_model_var_count() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        assert_eq!(lp.variables.len(), 480);
        assert_eq!(lp.var_types.len(), 480);
        assert_eq!(lp.objective.len(), 480);
    }

    // === TU4: 变量类型 — P Continuous，U/V/W Binary ===
    #[test]
    fn tu4_var_types() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        assert_eq!(lp.var_types[uc.var_index(0, 0, 0)], VarType::Continuous);
        assert_eq!(lp.var_types[uc.var_index(0, 0, 1)], VarType::Binary);
        assert_eq!(lp.var_types[uc.var_index(0, 0, 2)], VarType::Binary);
        assert_eq!(lp.var_types[uc.var_index(0, 0, 3)], VarType::Binary);
        // 抽查末尾机组
        assert_eq!(lp.var_types[uc.var_index(4, 23, 0)], VarType::Continuous);
        assert_eq!(lp.var_types[uc.var_index(4, 23, 3)], VarType::Binary);
    }

    // === TU5: 目标系数（D6 修正断言：start_cost 挂 V，U/W 为 0） ===
    #[test]
    fn tu5_objective_coeffs() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        // P 系数 == price[t]
        assert_eq!(lp.objective[uc.var_index(2, 5, 0)], price[5]);
        // V 系数 == start_cost（机组 2：100 + 10*2 = 120）
        assert_eq!(lp.objective[uc.var_index(2, 5, 2)], 120.0);
        // U/W 系数 == 0.0（D6：蓝图误挂 U，本实现修正为 V）
        assert_eq!(lp.objective[uc.var_index(2, 5, 1)], 0.0);
        assert_eq!(lp.objective[uc.var_index(2, 5, 3)], 0.0);
    }

    // === TU6: 变量边界 — P upper==p_max；U/V/W ∈ [0,1] ===
    #[test]
    fn tu6_var_bounds() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        let p_idx = uc.var_index(1, 3, 0);
        assert_eq!(lp.lower_bounds[p_idx], 0.0);
        assert_eq!(lp.upper_bounds[p_idx], 220.0); // 机组 1：200 + 20×1
        for k in 1..=3 {
            let idx = uc.var_index(1, 3, k);
            assert_eq!(lp.lower_bounds[idx], 0.0);
            assert_eq!(lp.upper_bounds[idx], 1.0);
        }
    }

    // === TU7: 功率平衡行 [0, t) — Eq、n 个非零、系数全 1.0、列为各机组 P ===
    #[test]
    fn tu7_power_balance_rows() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        let tt = 7usize;
        assert_eq!(lp.rhs_lower[tt], load[tt]);
        assert_eq!(lp.rhs_upper[tt], load[tt]);
        let (lo, hi) = (
            lp.constraints.row_start[tt] as usize,
            lp.constraints.row_start[tt + 1] as usize,
        );
        assert_eq!(hi - lo, 5);
        assert!(lp.constraints.values[lo..hi].iter().all(|&v| v == 1.0));
        let expect_cols: Vec<i32> = (0..5).map(|i| uc.var_index(i, tt, 0) as i32).collect();
        assert_eq!(lp.constraints.col_index[lo..hi], expect_cols[..]);
    }

    // === TU8: pmax/pmin 联动行 — Le/Ge 型 rhs 与系数 ===
    #[test]
    fn tu8_pmax_pmin_rows() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        let (n, t) = (5usize, 24usize);
        // pmax 行（i=1, t=3）：Le 型
        let row = t + t + 3;
        assert_eq!(lp.rhs_upper[row], 0.0);
        assert_eq!(lp.rhs_lower[row], f64::NEG_INFINITY);
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 2);
        assert_eq!(lp.constraints.values[lo], 1.0); // P 系数
        assert_eq!(lp.constraints.values[lo + 1], -220.0); // U 系数 -p_max
        assert_eq!(lp.constraints.col_index[lo], uc.var_index(1, 3, 0) as i32);
        assert_eq!(
            lp.constraints.col_index[lo + 1],
            uc.var_index(1, 3, 1) as i32
        );
        // pmin 行（i=1, t=3）：Ge 型
        let row = t + n * t + t + 3;
        assert_eq!(lp.rhs_lower[row], 0.0);
        assert_eq!(lp.rhs_upper[row], f64::INFINITY);
        let lo = lp.constraints.row_start[row] as usize;
        assert_eq!(lp.constraints.values[lo], 1.0);
        assert_eq!(lp.constraints.values[lo + 1], -60.0); // U 系数 -p_min
    }

    // === TU9: 爬坡行 — rhs_upper == ramp·interval_min，系数 ±1 ===
    #[test]
    fn tu9_ramp_rows() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        let (n, t) = (5usize, 24usize);
        // 爬坡上行（i=2, t=4）：Le(P[4] − P[3], 5×15)
        let row = t + 2 * n * t + 2 * (t - 1) + (4 - 1);
        assert_eq!(lp.rhs_upper[row], 75.0);
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 2);
        assert_eq!(lp.constraints.col_index[lo], uc.var_index(2, 3, 0) as i32);
        assert_eq!(lp.constraints.values[lo], -1.0);
        assert_eq!(
            lp.constraints.col_index[lo + 1],
            uc.var_index(2, 4, 0) as i32
        );
        assert_eq!(lp.constraints.values[lo + 1], 1.0);
        // 爬坡下行（i=2, t=4）：Le(P[3] − P[4], 5×15)
        let row = t + 2 * n * t + n * (t - 1) + 2 * (t - 1) + (4 - 1);
        assert_eq!(lp.rhs_upper[row], 75.0);
        let lo = lp.constraints.row_start[row] as usize;
        assert_eq!(lp.constraints.values[lo], 1.0);
        assert_eq!(lp.constraints.values[lo + 1], -1.0);
    }

    // === TU10: 启停逻辑行 — t=0 rhs==−init（Eq），t≥1 rhs==0 ===
    #[test]
    fn tu10_startup_logic_rows() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        let (n, t) = (5usize, 24usize);
        let base = t + 2 * n * t + 2 * n * (t - 1);
        // 机组 0（init=true）t=0：Eq rhs == −1.0
        let row = base;
        assert_eq!(lp.rhs_lower[row], -1.0);
        assert_eq!(lp.rhs_upper[row], -1.0);
        // 机组 1（init=false）t=0：Eq rhs == 0.0
        let row = base + t;
        assert_eq!(lp.rhs_lower[row], 0.0);
        assert_eq!(lp.rhs_upper[row], 0.0);
        // 机组 0 t=2：Eq rhs == 0.0，系数 V=+1, W=−1, U[t]=−1, U[t−1]=+1
        let row = base + 2;
        assert_eq!(lp.rhs_lower[row], 0.0);
        assert_eq!(lp.rhs_upper[row], 0.0);
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 4);
        let cols = &lp.constraints.col_index[lo..hi];
        let vals = &lp.constraints.values[lo..hi];
        assert_eq!(cols[0], uc.var_index(0, 1, 1) as i32); // U[t−1]
        assert_eq!(vals[0], 1.0);
        assert_eq!(cols[1], uc.var_index(0, 2, 1) as i32); // U[t]
        assert_eq!(vals[1], -1.0);
        assert_eq!(cols[2], uc.var_index(0, 2, 2) as i32); // V[t]
        assert_eq!(vals[2], 1.0);
        assert_eq!(cols[3], uc.var_index(0, 2, 3) as i32); // W[t]
        assert_eq!(vals[3], -1.0);
    }

    // === TU11: 最小运行/停机行 — 窗口项数与 rhs（1 机组 × 8 周期小模型） ===
    #[test]
    fn tu11_min_up_down_rows() {
        let unit = sample_unit("G0", 50.0, 200.0, 5.0, 5.0, 100.0, 3, 2, true);
        let uc = UnitCommitment::new(vec![unit], 8, 15);
        let load = vec![100.0; 8];
        let price = vec![0.5; 8];
        let lp = uc.build_model(&load, &price).unwrap();
        // n=1, t=8：min_up 基址 = 8（平衡）+ 2·8（pmax/pmin）+ 2·7（爬坡）+ 8（启停）= 46
        let min_up_base = 8 + 2 * 8 + 2 * 7 + 8;
        let min_down_base = min_up_base + 8;
        // min_up=3，t=1：窗口 τ∈[0,1] → 2 个 V 项 + 1 个 U 项 = 3 非零
        let row = min_up_base + 1;
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 3);
        // CSR 列按索引升序：V[0]=2 < U[1]=5 < V[1]=6
        assert_eq!(lp.constraints.col_index[lo], uc.var_index(0, 0, 2) as i32);
        assert_eq!(lp.constraints.values[lo], 1.0);
        assert_eq!(
            lp.constraints.col_index[lo + 1],
            uc.var_index(0, 1, 1) as i32
        );
        assert_eq!(lp.constraints.values[lo + 1], -1.0);
        assert_eq!(
            lp.constraints.col_index[lo + 2],
            uc.var_index(0, 1, 2) as i32
        );
        assert_eq!(lp.constraints.values[lo + 2], 1.0);
        assert_eq!(lp.rhs_upper[row], 0.0);
        // min_up=3，t=5：窗口 τ∈[3,5] → 3 个 V 项 + 1 个 U 项 = 4 非零
        let row = min_up_base + 5;
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 4);
        // 最小停机（min_down=2，t=3）：窗口 τ∈[2,3] → 2 个 W 项 + 1 个 U 项
        let row = min_down_base + 3;
        let (lo, hi) = (
            lp.constraints.row_start[row] as usize,
            lp.constraints.row_start[row + 1] as usize,
        );
        assert_eq!(hi - lo, 3);
        assert_eq!(lp.rhs_upper[row], 1.0);
        // CSR 列按索引升序：W[2]=11 < U[3]=13 < W[3]=15
        assert_eq!(lp.constraints.col_index[lo], uc.var_index(0, 2, 3) as i32); // W[2]
        assert_eq!(lp.constraints.values[lo], 1.0);
        assert_eq!(
            lp.constraints.col_index[lo + 1],
            uc.var_index(0, 3, 1) as i32
        ); // U[t]
        assert_eq!(lp.constraints.values[lo + 1], 1.0);
        assert_eq!(
            lp.constraints.col_index[lo + 2],
            uc.var_index(0, 3, 3) as i32
        ); // W[3]
        assert_eq!(lp.constraints.values[lo + 2], 1.0);
    }

    // === TU12: 总行数与 CSR 一致性（任务书算术修正：8 组约束 854 行） ===
    #[test]
    fn tu12_total_rows_and_csr() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        // t + 5nt + 2n(t−1) = 24 + 600 + 230 = 854
        assert_eq!(lp.rhs_lower.len(), 854);
        assert_eq!(lp.rhs_upper.len(), 854);
        assert_eq!(lp.constraints.num_rows, 854);
        assert_eq!(lp.constraints.row_start.len(), 855);
        let nnz = lp.constraints.num_nz;
        assert_eq!(*lp.constraints.row_start.last().unwrap() as usize, nnz);
        assert_eq!(lp.constraints.col_index.len(), nnz);
        assert_eq!(lp.constraints.values.len(), nnz);
    }

    // === TU13: 目标方向 == Minimize ===
    #[test]
    fn tu13_sense_minimize() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let lp = uc.build_model(&load, &price).unwrap();
        assert_eq!(lp.sense, ObjectiveSense::Minimize);
    }

    // === TU14: 输入长度非法 → Err(InvalidProblem) ===
    #[test]
    fn tu14_invalid_input() {
        let uc = sample_uc();
        let bad_load = vec![0.0; 23];
        let price = vec![0.5; 24];
        assert!(matches!(
            uc.build_model(&bad_load, &price),
            Err(SolverError::InvalidProblem(_))
        ));
        let load = vec![600.0; 24];
        let bad_price = vec![0.5; 25];
        assert!(matches!(
            uc.build_model(&load, &bad_price),
            Err(SolverError::InvalidProblem(_))
        ));
        assert!(matches!(
            uc.build_model_relaxed(&bad_load, &price),
            Err(SolverError::InvalidProblem(_))
        ));
    }

    // === TU15: 松弛模型 614 行，且为完整模型前 614 行的严格前缀 ===
    #[test]
    fn tu15_relaxed_rows() {
        let uc = sample_uc();
        let (load, price) = sample_load_price();
        let full = uc.build_model(&load, &price).unwrap();
        let rel = uc.build_model_relaxed(&load, &price).unwrap();
        // 854 − 2·5·24 = 614
        assert_eq!(rel.rhs_lower.len(), 614);
        assert_eq!(rel.constraints.num_rows, 614);
        // 前 t+3nt+2n(t−1) 行与完整模型一致（rhs 与 CSR 值）
        assert_eq!(rel.rhs_lower[..], full.rhs_lower[..614]);
        assert_eq!(rel.rhs_upper[..], full.rhs_upper[..614]);
        assert_eq!(
            rel.constraints.row_start[..],
            full.constraints.row_start[..=614]
        );
        let nnz = rel.constraints.num_nz;
        assert_eq!(
            rel.constraints.col_index[..],
            full.constraints.col_index[..nnz]
        );
        assert_eq!(rel.constraints.values[..], full.constraints.values[..nnz]);
    }
}
