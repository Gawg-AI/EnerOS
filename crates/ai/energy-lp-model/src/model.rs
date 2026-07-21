//! 储能调度 LP 模型构建器（D3/D4）.
//!
//! 基于 v0.65.0 `OptProblem` DSL 构建储能系统优化调度 LP 模型，
//! 包含 SOC 动态约束（D3 关键修正）、爬坡约束、SOC 初终值约束与
//! 收益最大化目标函数。

use alloc::format;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;
use eneros_solver_core::problem::LpProblem;
use eneros_solver_core::result::SolveResult;
use eneros_solver_model::constraint::Constraint;
use eneros_solver_model::expr::LinearExpr;
use eneros_solver_model::problem::OptProblem;
use eneros_solver_model::variable::VarBuilder;

use crate::config::ScheduleConfig;
use crate::result::{ScheduleEntry, ScheduleResult};

/// 储能调度 LP 模型构建器.
///
/// 自动创建 3×n 决策变量（charge / discharge / soc），添加约束，
/// 设置目标函数，并支持编译为 `LpProblem` 与解析求解结果。
pub struct EnergyScheduleModel {
    /// 调度配置.
    config: ScheduleConfig,
    /// 优化问题容器.
    pub(crate) problem: OptProblem,
    /// 充电功率变量索引列表.
    pub charge_var_idx: Vec<usize>,
    /// 放电功率变量索引列表.
    pub discharge_var_idx: Vec<usize>,
    /// SOC 变量索引列表.
    pub soc_var_idx: Vec<usize>,
}

impl EnergyScheduleModel {
    /// 创建储能调度模型.
    ///
    /// 自动创建 3×n 决策变量（charge / discharge / soc），添加约束，
    /// 设置目标函数。变量顺序：charge[0..n], discharge[n..2n], soc[2n..3n]。
    pub fn new(config: ScheduleConfig) -> Self {
        let n = config.num_periods;
        let pcs = config.pcs_power_kw;
        let cap = config.battery_capacity_kwh;
        let soc_lo = config.soc_min * cap;
        let soc_hi = config.soc_max * cap;

        let mut problem = OptProblem::new();

        // 决策变量：charge[0..n]
        let mut charge_var_idx = Vec::with_capacity(n);
        for t in 0..n {
            let idx = problem.add_var(
                VarBuilder::new(&format!("charge_{}", t))
                    .range(0.0, pcs)
                    .build(),
            );
            charge_var_idx.push(idx);
        }

        // 决策变量：discharge[0..n]
        let mut discharge_var_idx = Vec::with_capacity(n);
        for t in 0..n {
            let idx = problem.add_var(
                VarBuilder::new(&format!("discharge_{}", t))
                    .range(0.0, pcs)
                    .build(),
            );
            discharge_var_idx.push(idx);
        }

        // 决策变量：soc[0..n]（单位 kWh，边界已乘容量）
        let mut soc_var_idx = Vec::with_capacity(n);
        for t in 0..n {
            let idx = problem.add_var(
                VarBuilder::new(&format!("soc_{}", t))
                    .range(soc_lo, soc_hi)
                    .build(),
            );
            soc_var_idx.push(idx);
        }

        let mut model = Self {
            config,
            problem,
            charge_var_idx,
            discharge_var_idx,
            soc_var_idx,
        };

        // 添加约束（顺序：SOC 动态 → 爬坡 → SOC 初值 → SOC 终值）
        model.add_soc_dynamics_constraints();
        model.add_ramp_constraints();
        model.add_soc_init_constraint();
        if let Some(soc_final) = model.config.soc_final {
            model.add_soc_final_constraint(soc_final);
        }

        // 设置目标函数
        model.set_objective();

        model
    }

    /// 添加 SOC 动态约束（D3 CRITICAL FIX）.
    ///
    /// 修正后公式：
    /// `soc[t] - soc[t-1] - charge[t]·η_c·dt + discharge[t]·(dt/η_d) == 0`
    ///
    /// **D3 关键修正**：蓝图原文放电系数为 `η_d·dt`，这是数学错误。
    /// 根据能量守恒，放电会损失能量，放电 1 kWh 实际从电池移除
    /// `1/η_d` kWh。因此放电项系数应为 `dt / η_d`，而非 `η_d · dt`。
    ///
    /// SOC 变量单位为 kWh，边界为 `[soc_min·cap, soc_max·cap]`，
    /// 无需 cap 归一化（蓝图原文 `/cap * cap` 相互抵消无意义）。
    pub fn add_soc_dynamics_constraints(&mut self) {
        let dt = self.config.period_hours;
        let eta_c = self.config.charge_efficiency;
        let eta_d = self.config.discharge_efficiency;
        // D3 关键修正：充电系数 = η_c·dt，放电系数 = dt/η_d（非 η_d·dt）
        let charge_coeff = eta_c * dt;
        let discharge_coeff = dt / eta_d;

        for t in 1..self.config.num_periods {
            let mut expr = LinearExpr::new();
            expr.add_term(self.soc_var_idx[t], 1.0);
            expr.add_term(self.soc_var_idx[t - 1], -1.0);
            expr.add_term(self.charge_var_idx[t], -charge_coeff);
            expr.add_term(self.discharge_var_idx[t], discharge_coeff);
            self.problem
                .add_constraint(&format!("soc_dynamics_{}", t), Constraint::Eq(expr, 0.0));
        }
    }

    /// 添加爬坡率约束.
    ///
    /// `charge[t] - charge[t-1] <= ramp_c`
    /// `discharge[t] - discharge[t-1] <= ramp_d`
    pub fn add_ramp_constraints(&mut self) {
        let ramp_c = self.config.charge_ramp_kw;
        let ramp_d = self.config.discharge_ramp_kw;

        for t in 1..self.config.num_periods {
            let mut c_expr = LinearExpr::new();
            c_expr.add_term(self.charge_var_idx[t], 1.0);
            c_expr.add_term(self.charge_var_idx[t - 1], -1.0);
            self.problem.add_constraint(
                &format!("ramp_charge_{}", t),
                Constraint::Le(c_expr, ramp_c),
            );

            let mut d_expr = LinearExpr::new();
            d_expr.add_term(self.discharge_var_idx[t], 1.0);
            d_expr.add_term(self.discharge_var_idx[t - 1], -1.0);
            self.problem.add_constraint(
                &format!("ramp_discharge_{}", t),
                Constraint::Le(d_expr, ramp_d),
            );
        }
    }

    /// 添加 SOC 初始值约束.
    ///
    /// `soc[0] == soc_init * capacity`
    pub fn add_soc_init_constraint(&mut self) {
        let init = self.config.soc_init * self.config.battery_capacity_kwh;
        let mut expr = LinearExpr::new();
        expr.add_term(self.soc_var_idx[0], 1.0);
        self.problem
            .add_constraint("soc_init", Constraint::Eq(expr, init));
    }

    /// 添加 SOC 终值约束.
    ///
    /// `soc[n-1] == soc_final * capacity`
    pub fn add_soc_final_constraint(&mut self, soc_final: f64) {
        let final_val = soc_final * self.config.battery_capacity_kwh;
        let mut expr = LinearExpr::new();
        expr.add_term(*self.soc_var_idx.last().unwrap(), 1.0);
        self.problem
            .add_constraint("soc_final", Constraint::Eq(expr, final_val));
    }

    /// 设置目标函数：最大化收益.
    ///
    /// `max Σ (price[t]·discharge[t] - price[t]·charge[t])·dt`
    ///
    /// D1：使用 `core::mem::take` 绕过借用检查器
    /// （`OptProblem::maximize` 消费 self，需先 take 再赋回）。
    pub fn set_objective(&mut self) {
        let dt = self.config.period_hours;
        let mut obj = LinearExpr::new();
        for t in 0..self.config.num_periods {
            let price = self.config.price.get(t).copied().unwrap_or(0.0);
            obj.add_term(self.discharge_var_idx[t], price * dt);
            obj.add_term(self.charge_var_idx[t], -price * dt);
        }
        // D1: core::mem::take 替代 std::mem::take（no_std 合规）
        self.problem = core::mem::take(&mut self.problem).maximize(obj);
    }

    /// 编译为 `LpProblem` 矩阵格式.
    pub fn compile(&self) -> Result<LpProblem, SolverError> {
        self.problem.compile()
    }

    /// 解析求解结果为调度方案.
    ///
    /// D4：使用 `result.solution.get(idx).copied().unwrap_or(0.0)` 安全访问，
    /// 避免越界 panic（no_std 环境下 panic 不可恢复）。
    pub fn parse_result(&self, result: &SolveResult) -> ScheduleResult {
        let cap = self.config.battery_capacity_kwh;
        let dt = self.config.period_hours;
        let n = self.config.num_periods;

        let mut schedule = Vec::with_capacity(n);
        let mut total_revenue = 0.0;

        for t in 0..n {
            // D4: 安全访问，越界返回 0.0
            let charge = result
                .solution
                .get(self.charge_var_idx[t])
                .copied()
                .unwrap_or(0.0);
            let discharge = result
                .solution
                .get(self.discharge_var_idx[t])
                .copied()
                .unwrap_or(0.0);
            let soc = result
                .solution
                .get(self.soc_var_idx[t])
                .copied()
                .unwrap_or(0.0);
            let price = self.config.price.get(t).copied().unwrap_or(0.0);

            let net_power = discharge - charge;
            let soc_pct = if cap > 0.0 { soc / cap } else { 0.0 };
            let revenue = (discharge - charge) * price * dt;
            total_revenue += revenue;

            schedule.push(ScheduleEntry {
                period: t,
                charge_power_kw: charge,
                discharge_power_kw: discharge,
                net_power_kw: net_power,
                soc_pct,
                revenue_yuan: revenue,
            });
        }

        ScheduleResult {
            schedule,
            total_revenue_yuan: total_revenue,
            objective_value: result.objective_value,
            solve_status: result.status.clone(),
        }
    }
}
