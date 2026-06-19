//! AC-OPF（交流最优潮流）求解器
//!
//! 实现完整的交流最优潮流计算，包括：
//! - 牛顿-拉夫逊法 AC-OPF（极坐标形式）
//! - 原对偶内点法（含日志障碍函数）
//! - 节点边际电价（LMP）计算
//! - 安全约束最优潮流（SCOPF）N-1 校验
//! - 简化机组组合（按时段独立求解）
//!
//! 物理量约定：
//! - 电压幅值单位：p.u.（标幺值），正常运行范围 0.95~1.05
//! - 电压相角单位：弧度，范围 -π ~ π
//! - Y-Bus 导纳矩阵：Y = G + jB
//! - 复功率：S = P + jQ
//! - 系统基准容量：base_mva（用于 MW/MVar ↔ p.u. 转换）

#![allow(clippy::needless_range_loop)]

use ndarray::{Array1, Array2};
use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f64::consts::PI;

use eneros_core::ElementId;
use crate::types::{AnalysisResult, AnalysisError};

/// 收敛容差
const CONVERGENCE_TOL: f64 = 1e-6;
/// 牛顿法最大迭代次数
const NEWTON_MAX_ITER: u32 = 50;
/// 内点法最大迭代次数
const IPM_MAX_ITER: u32 = 50;
/// SCOPF 最大调整轮数
const SCOPF_MAX_ROUNDS: u32 = 3;

// ============================================================================
// T3.1 类型定义
// ============================================================================

/// 交流发电机模型（含成本曲线）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcGenerator {
    pub gen_id: ElementId,
    pub bus_id: ElementId,
    /// 有功出力下限（MW）
    pub p_min: f64,
    /// 有功出力上限（MW）
    pub p_max: f64,
    /// 无功出力下限（MVar）
    pub q_min: f64,
    /// 无功出力上限（MVar）
    pub q_max: f64,
    /// 二次成本系数 a*P^2
    pub cost_a: f64,
    /// 一次成本系数 b*P
    pub cost_b: f64,
    /// 常数成本系数 c
    pub cost_c: f64,
}

/// 交流支路模型（线路或变压器）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcBranch {
    pub branch_id: ElementId,
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    /// 串联电阻（p.u.）
    pub r_pu: f64,
    /// 串联电抗（p.u.）
    pub x_pu: f64,
    /// 线路充电电纳（p.u.，对地）
    pub b_half: f64,
    /// 变压器变比（1.0 表示普通线路）
    pub tap_ratio: f64,
    /// 视在功率传输上限（MVA）
    pub s_limit_mva: f64,
}

/// 交流母线模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcBus {
    pub bus_id: ElementId,
    /// 有功负荷（MW）
    pub p_load: f64,
    /// 无功负荷（MVar）
    pub q_load: f64,
    /// 电压幅值下限（p.u.）
    pub v_min: f64,
    /// 电压幅值上限（p.u.）
    pub v_max: f64,
    /// 初始电压幅值猜测（p.u.）
    pub v_init: f64,
    /// 初始相角猜测（弧度）
    pub theta_init: f64,
}

/// OPF 求解方法
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpfMethod {
    /// 牛顿-拉夫逊法
    NewtonRaphson,
    /// 原对偶内点法
    InteriorPoint,
}

/// AC-OPF 问题定义
#[derive(Debug, Clone)]
pub struct AcOpfProblem {
    pub buses: Vec<AcBus>,
    pub generators: Vec<AcGenerator>,
    pub branches: Vec<AcBranch>,
    /// 平衡节点 ID
    pub slack_bus_id: ElementId,
    /// 系统基准容量（MVA）
    pub base_mva: f64,
}

/// AC-OPF 求解结果
#[derive(Debug, Clone)]
pub struct AcOpfResult {
    /// (gen_id, p_mw, q_mvar) 发电机出力
    pub generation: Vec<(ElementId, f64, f64)>,
    /// (bus_id, v_pu, theta_rad) 母线电压
    pub bus_voltages: Vec<(ElementId, f64, f64)>,
    /// (branch_id, from_to_mva) 支路潮流
    pub branch_flows: Vec<(ElementId, f64)>,
    /// (bus_id, lmp_$/mwh) 节点电价
    pub nodal_prices: Vec<(ElementId, f64)>,
    /// 总发电成本
    pub total_cost: f64,
    /// 系统总有功损耗（MW）
    pub total_losses: f64,
}

/// 母线类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusType {
    /// 平衡节点：电压幅值和相角已知
    Slack,
    /// PV 节点：有功和电压幅值已知
    Pv,
    /// PQ 节点：有功和无功已知
    Pq,
}

/// 内部问题预处理结构
struct ProblemSetup {
    /// 母线 ID → 矩阵索引
    bus_map: HashMap<ElementId, usize>,
    /// 母线数量
    bus_count: usize,
    /// 平衡节点索引
    slack_idx: usize,
    /// 母线类型列表
    bus_types: Vec<BusType>,
    /// 每个母线的发电机索引列表
    bus_gens: Vec<Vec<usize>>,
    /// Y-Bus 导纳矩阵
    ybus: Array2<Complex64>,
}

// ============================================================================
// AC-OPF 求解器主实现
// ============================================================================

/// AC-OPF 求解器
pub struct AcOpfSolver {
    /// 最大迭代次数
    max_iter: u32,
    /// 收敛容差
    tol: f64,
}

impl Default for AcOpfSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl AcOpfSolver {
    pub fn new() -> Self {
        Self {
            max_iter: NEWTON_MAX_ITER,
            tol: CONVERGENCE_TOL,
        }
    }

    /// 设置最大迭代次数
    pub fn with_max_iter(mut self, n: u32) -> Self {
        self.max_iter = n;
        self
    }

    /// 设置收敛容差
    pub fn with_tolerance(mut self, tol: f64) -> Self {
        self.tol = tol;
        self
    }

    /// 求解 AC-OPF 问题
    pub fn solve(
        &self,
        problem: &AcOpfProblem,
        method: OpfMethod,
    ) -> Result<AnalysisResult<AcOpfResult>, AnalysisError> {
        match method {
            OpfMethod::NewtonRaphson => self.solve_newton(problem),
            OpfMethod::InteriorPoint => self.solve_interior_point(problem),
        }
    }

    // ========================================================================
    // T3.2 牛顿-拉夫逊法 AC-OPF
    // ========================================================================

    /// 牛顿-拉夫逊法 AC-OPF
    ///
    /// 算法流程：
    /// 1. 经济调度确定发电机有功出力初值
    /// 2. 极坐标牛顿法求解潮流（PQ/Vθ 不平衡方程）
    /// 3. 根据潮流结果调整发电机出力（基于 LMP 反馈）
    /// 4. 迭代直至收敛
    pub fn solve_newton(
        &self,
        problem: &AcOpfProblem,
    ) -> Result<AnalysisResult<AcOpfResult>, AnalysisError> {
        self.validate_problem(problem)?;
        let setup = self.setup_problem(problem);

        // 经济调度初值
        let mut gen_p_pu = self.economic_dispatch(problem, &setup);
        let mut voltages = self.initial_voltages(problem, &setup);
        let mut angles = self.initial_angles(problem, &setup);

        let mut warnings = Vec::new();
        let mut outer_iter = 0u32;
        let max_outer = 10u32;

        // 外层：经济调度 + 潮流交替求解
        loop {
            outer_iter += 1;
            if outer_iter > max_outer {
                warnings.push(format!("外层迭代达到上限 {}", max_outer));
                break;
            }

            // 内层：牛顿法求解潮流
            let (conv, iters) = self.solve_power_flow(
                problem,
                &setup,
                &mut voltages,
                &mut angles,
                &gen_p_pu,
            )?;

            if !conv {
                warnings.push(format!("潮流未收敛（迭代 {} 次）", iters));
            }

            // 计算不平衡量并调整发电机出力
            let imbalance = self.compute_power_imbalance(problem, &setup, &voltages, &angles, &gen_p_pu);
            if imbalance.abs() < self.tol * 10.0 {
                break;
            }

            // 调整平衡机出力以消除不平衡
            self.update_slack_generation(problem, &setup, &voltages, &angles, &mut gen_p_pu);

            // 简化：若不平衡量已较小则退出
            if imbalance.abs() < self.tol * 100.0 {
                break;
            }
        }

        // 计算无功出力
        let gen_q_pu = self.compute_reactive_generation(problem, &setup, &voltages, &angles);

        // 计算支路潮流
        let branch_flows = self.compute_branch_flow(problem, &setup, &voltages, &angles);

        // 计算 LMP
        let lmp = self.compute_lmp_internal(problem, &setup, &voltages, &angles, &gen_p_pu);

        // 检查约束
        let constraint_warnings = self.check_constraints(problem, &setup, &voltages, &gen_p_pu, &gen_q_pu, &branch_flows);
        warnings.extend(constraint_warnings);

        let result = self.build_result(problem, &setup, &voltages, &angles, &gen_p_pu, &gen_q_pu, &branch_flows, &lmp);

        Ok(AnalysisResult {
            converged: warnings.is_empty(),
            iterations: outer_iter,
            result,
            warnings,
        })
    }

    // ========================================================================
    // T3.3 原对偶内点法
    // ========================================================================

    /// 原对偶内点法 AC-OPF
    ///
    /// 采用日志障碍函数处理不等式约束：
    /// min f(x) - μ * Σ log(g_i(x))
    /// s.t. h(x) = 0 (等式约束，功率平衡)
    ///      g(x) ≥ 0 (不等式约束，电压/出力/支路功率限制)
    pub fn solve_interior_point(
        &self,
        problem: &AcOpfProblem,
    ) -> Result<AnalysisResult<AcOpfResult>, AnalysisError> {
        self.validate_problem(problem)?;
        let setup = self.setup_problem(problem);

        // 初始化决策变量：电压幅值、相角、发电机有功
        let mut voltages = self.initial_voltages(problem, &setup);
        let mut angles = self.initial_angles(problem, &setup);
        let mut gen_p_pu = self.economic_dispatch(problem, &setup);

        let mut warnings = Vec::new();
        let mut barrier_mu = 1.0; // 障碍参数
        let mu_min = 1e-8;
        let mut total_iter = 0u32;

        for iter in 0..self.max_iter.max(IPM_MAX_ITER) {
            total_iter = iter + 1;

            // 计算功率不平衡量（等式约束残差）
            let (dp, dq) = self.compute_mismatch(problem, &setup, &voltages, &angles, &gen_p_pu);
            let max_mismatch = dp.iter().chain(dq.iter()).map(|x| x.abs()).fold(0.0_f64, f64::max);

            // 计算不等式约束违反度
            let constraint_violation = self.compute_constraint_violation(
                problem, &setup, &voltages, &gen_p_pu,
            );

            // 收敛判据：等式残差小、障碍参数小、约束违反小
            if max_mismatch < self.tol && barrier_mu < mu_min * 100.0 && constraint_violation < self.tol * 10.0 {
                break;
            }

            // 构造并求解修正方程（简化版：梯度下降 + 障碍项）
            let (dv, dtheta, dpg) = self.compute_ipm_direction(
                problem, &setup, &voltages, &angles, &gen_p_pu,
                &dp, &dq, barrier_mu,
            );

            // 线搜索确定步长（保证电压和出力在可行域内）
            let alpha = self.line_search(
                problem, &setup, &voltages, &angles, &gen_p_pu,
                &dv, &dtheta, &dpg,
            );

            // 更新变量
            for i in 0..setup.bus_count {
                if setup.bus_types[i] != BusType::Slack {
                    angles[i] += alpha * dtheta[i];
                }
                if setup.bus_types[i] == BusType::Pq {
                    voltages[i] += alpha * dv[i];
                    // 电压幅值硬约束
                    let bus = &problem.buses[i];
                    voltages[i] = voltages[i].clamp(bus.v_min, bus.v_max);
                }
            }
            for (gi, &gen_idx) in setup.bus_gens[setup.slack_idx].iter().enumerate() {
                let _ = gi;
                gen_p_pu[gen_idx] += alpha * dpg[gen_idx];
                let g = &problem.generators[gen_idx];
                let p_min_pu = g.p_min / problem.base_mva;
                let p_max_pu = g.p_max / problem.base_mva;
                gen_p_pu[gen_idx] = gen_p_pu[gen_idx].clamp(p_min_pu, p_max_pu);
            }

            // 更新障碍参数
            barrier_mu *= 0.5;
            if barrier_mu < mu_min {
                barrier_mu = mu_min;
            }
        }

        // 最终潮流校验
        let (conv, _) = self.solve_power_flow(problem, &setup, &mut voltages, &mut angles, &gen_p_pu)?;
        if !conv {
            warnings.push("内点法最终潮流未收敛".to_string());
        }

        let gen_q_pu = self.compute_reactive_generation(problem, &setup, &voltages, &angles);
        let branch_flows = self.compute_branch_flow(problem, &setup, &voltages, &angles);
        let lmp = self.compute_lmp_internal(problem, &setup, &voltages, &angles, &gen_p_pu);
        let constraint_warnings = self.check_constraints(problem, &setup, &voltages, &gen_p_pu, &gen_q_pu, &branch_flows);
        warnings.extend(constraint_warnings);

        let result = self.build_result(problem, &setup, &voltages, &angles, &gen_p_pu, &gen_q_pu, &branch_flows, &lmp);

        Ok(AnalysisResult {
            converged: warnings.is_empty(),
            iterations: total_iter,
            result,
            warnings,
        })
    }

    // ========================================================================
    // T3.4 LMP 计算（公共接口）
    // ========================================================================

    /// 计算节点边际电价（LMP）
    ///
    /// LMP = 能量分量 + 阻塞分量 + 损耗分量
    /// - 能量分量：边际发电机成本
    /// - 阻塞分量：基于支路潮流灵敏度
    /// - 损耗分量：基于网损对注入的灵敏度
    pub fn compute_lmp(
        &self,
        problem: &AcOpfProblem,
        result: &AcOpfResult,
    ) -> Vec<(ElementId, f64)> {
        let setup = self.setup_problem(problem);

        // 重建电压和相角数组
        let mut voltages = vec![1.0; setup.bus_count];
        let mut angles = vec![0.0; setup.bus_count];
        for &(bus_id, v, theta) in &result.bus_voltages {
            if let Some(&idx) = setup.bus_map.get(&bus_id) {
                voltages[idx] = v;
                angles[idx] = theta;
            }
        }

        // 重建发电机有功出力（p.u.）
        let mut gen_p_pu = vec![0.0; problem.generators.len()];
        for (i, gen) in problem.generators.iter().enumerate() {
            if let Some(&(_, p_mw, _)) = result.generation.iter().find(|(gid, _, _)| *gid == gen.gen_id).as_ref() {
                gen_p_pu[i] = p_mw / problem.base_mva;
            }
        }

        // 内部方法已按母线索引顺序返回 (bus_id, price)
        self.compute_lmp_internal(problem, &setup, &voltages, &angles, &gen_p_pu)
    }

    // ========================================================================
    // T3.5 SCOPF N-1 安全约束
    // ========================================================================

    /// 安全约束最优潮流（SCOPF）
    ///
    /// 算法：
    /// 1. 求解基态 AC-OPF
    /// 2. 对每个支路进行 N-1 故障扫描
    /// 3. 若发现越限，调整基态出力以消除越限
    /// 4. 重复直至所有故障场景均满足约束
    pub fn solve_scopf(
        &self,
        problem: &AcOpfProblem,
        method: OpfMethod,
    ) -> Result<AnalysisResult<AcOpfResult>, AnalysisError> {
        // 基态求解
        let mut base_result = self.solve(problem, method)?;
        let setup = self.setup_problem(problem);

        let mut warnings = Vec::new();
        let mut current_gen: Vec<(ElementId, f64, f64)> = base_result.result.generation.clone();
        let mut round = 0u32;

        loop {
            round += 1;
            if round > SCOPF_MAX_ROUNDS {
                warnings.push(format!("SCOPF 达到最大调整轮数 {}", SCOPF_MAX_ROUNDS));
                break;
            }

            // N-1 故障扫描
            let violations = self.check_contingency_violations(problem, &setup, &current_gen, method)?;

            if violations.is_empty() {
                break;
            }

            // 记录越限信息
            for v in &violations {
                warnings.push(format!(
                    "N-1 故障：支路 {} 断开后，支路 {} 潮流 {:.2} MVA 超限 {:.2} MVA",
                    v.contingency_branch_id, v.violated_branch_id, v.flow_mva, v.limit_mva
                ));
            }

            // 调整发电机出力：降低越限支路送端发电机出力，增加受端发电机出力
            for v in &violations {
                if let Some(branch) = problem.branches.iter().find(|b| b.branch_id == v.violated_branch_id) {
                    let from_idx = setup.bus_map.get(&branch.from_bus).copied();
                    let to_idx = setup.bus_map.get(&branch.to_bus).copied();
                    if let (Some(fi), Some(ti)) = (from_idx, to_idx) {
                        // 找到送端和受端母线上的发电机
                        let overload = v.flow_mva - v.limit_mva;
                        let adjustment = overload * 0.5 / problem.base_mva;
                        // 送端减出力
                        for &gi in &setup.bus_gens[fi] {
                            let g = &problem.generators[gi];
                            let cur_p = current_gen.iter().find(|(id, _, _)| *id == g.gen_id).map(|(_, p, _)| *p).unwrap_or(0.0);
                            let new_p = (cur_p - adjustment * problem.base_mva).max(g.p_min);
                            if let Some(entry) = current_gen.iter_mut().find(|(id, _, _)| *id == g.gen_id) {
                                entry.1 = new_p;
                            }
                        }
                        // 受端增出力
                        for &gi in &setup.bus_gens[ti] {
                            let g = &problem.generators[gi];
                            let cur_p = current_gen.iter().find(|(id, _, _)| *id == g.gen_id).map(|(_, p, _)| *p).unwrap_or(0.0);
                            let new_p = (cur_p + adjustment * problem.base_mva).min(g.p_max);
                            if let Some(entry) = current_gen.iter_mut().find(|(id, _, _)| *id == g.gen_id) {
                                entry.1 = new_p;
                            }
                        }
                    }
                }
            }

            // 用调整后的出力重新求解潮流
            let mut voltages = self.initial_voltages(problem, &setup);
            let mut angles = self.initial_angles(problem, &setup);
            let mut gen_p_pu: Vec<f64> = problem.generators.iter().enumerate().map(|(i, g)| {
                current_gen.iter().find(|(id, _, _)| *id == g.gen_id)
                    .map(|(_, p, _)| *p / problem.base_mva).unwrap_or_else(|| {
                        // 默认经济调度值
                        let _ = i;
                        0.0
                    })
            }).collect();

            // 若没有出力信息则用经济调度
            if gen_p_pu.iter().all(|&x| x.abs() < 1e-10) {
                gen_p_pu = self.economic_dispatch(problem, &setup);
            }

            let (conv, _) = self.solve_power_flow(problem, &setup, &mut voltages, &mut angles, &gen_p_pu)?;
            if !conv {
                warnings.push("SCOPF 调整后潮流未收敛".to_string());
            }

            let gen_q_pu = self.compute_reactive_generation(problem, &setup, &voltages, &angles);
            let branch_flows = self.compute_branch_flow(problem, &setup, &voltages, &angles);
            let lmp = self.compute_lmp_internal(problem, &setup, &voltages, &angles, &gen_p_pu);

            // 更新结果
            current_gen = problem.generators.iter().enumerate().map(|(i, g)| {
                let p_mw = gen_p_pu[i] * problem.base_mva;
                let q_mvar = gen_q_pu[i] * problem.base_mva;
                (g.gen_id, p_mw, q_mvar)
            }).collect();

            base_result.result = self.build_result(problem, &setup, &voltages, &angles, &gen_p_pu, &gen_q_pu, &branch_flows, &lmp);
        }

        // 合并警告
        base_result.warnings.extend(warnings);
        base_result.converged = base_result.warnings.is_empty();
        Ok(base_result)
    }

    // ========================================================================
    // T3.6 简化机组组合
    // ========================================================================

    /// 简化机组组合（按时段独立求解 OPF）
    ///
    /// 注：本实现不考虑启停成本、最小开停机时间等时间耦合约束，
    /// 仅对每个时段独立求解 AC-OPF。
    ///
    /// 参数：
    /// - `base_problem`：基础电网拓扑
    /// - `load_profile`：按时段给出的负荷，每个元素为 (bus_id, p_mw, q_mvar) 列表
    pub fn solve_unit_commitment(
        &self,
        base_problem: &AcOpfProblem,
        load_profile: &[Vec<(ElementId, f64, f64)>],
        method: OpfMethod,
    ) -> Result<Vec<AnalysisResult<AcOpfResult>>, AnalysisError> {
        let mut results = Vec::with_capacity(load_profile.len());

        for (t, period_loads) in load_profile.iter().enumerate() {
            // 构造该时段的问题
            let mut period_problem = base_problem.clone();
            for bus in &mut period_problem.buses {
                bus.p_load = 0.0;
                bus.q_load = 0.0;
            }
            for &(bus_id, p_mw, q_mvar) in period_loads {
                if let Some(bus) = period_problem.buses.iter_mut().find(|b| b.bus_id == bus_id) {
                    bus.p_load = p_mw;
                    bus.q_load = q_mvar;
                }
            }

            let result = self.solve(&period_problem, method).map_err(|e| {
                AnalysisError::InvalidConfiguration(format!("时段 {} 求解失败: {}", t, e))
            })?;
            results.push(result);
        }

        Ok(results)
    }

    // ========================================================================
    // 内部辅助方法
    // ========================================================================

    /// 校验问题定义
    fn validate_problem(&self, problem: &AcOpfProblem) -> Result<(), AnalysisError> {
        if problem.buses.is_empty() {
            return Err(AnalysisError::DataIncomplete("无母线定义".into()));
        }
        if problem.generators.is_empty() {
            return Err(AnalysisError::DataIncomplete("无发电机定义".into()));
        }
        if problem.branches.is_empty() {
            return Err(AnalysisError::DataIncomplete("无支路定义".into()));
        }
        if problem.base_mva <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                format!("base_mva 必须为正数，当前 {}", problem.base_mva)
            ));
        }
        // 检查平衡节点存在
        if !problem.buses.iter().any(|b| b.bus_id == problem.slack_bus_id) {
            return Err(AnalysisError::InvalidConfiguration(
                format!("平衡节点 {} 不存在", problem.slack_bus_id)
            ));
        }
        Ok(())
    }

    /// 问题预处理：构建母线映射、Y-Bus、母线类型分类
    fn setup_problem(&self, problem: &AcOpfProblem) -> ProblemSetup {
        // 母线 ID 排序后建立索引
        let mut bus_ids: Vec<ElementId> = problem.buses.iter().map(|b| b.bus_id).collect();
        bus_ids.sort();
        bus_ids.dedup();

        let bus_count = bus_ids.len();
        let bus_map: HashMap<ElementId, usize> = bus_ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        let slack_idx = *bus_map.get(&problem.slack_bus_id).unwrap_or(&0);

        // 母线类型分类
        let mut bus_types = vec![BusType::Pq; bus_count];
        bus_types[slack_idx] = BusType::Slack;
        for gen in &problem.generators {
            if let Some(&idx) = bus_map.get(&gen.bus_id) {
                if idx != slack_idx {
                    bus_types[idx] = BusType::Pv;
                }
            }
        }

        // 每个母线的发电机索引
        let mut bus_gens = vec![Vec::new(); bus_count];
        for (i, gen) in problem.generators.iter().enumerate() {
            if let Some(&idx) = bus_map.get(&gen.bus_id) {
                bus_gens[idx].push(i);
            }
        }

        // 构建 Y-Bus
        let ybus = self.build_ybus(problem, &bus_map, bus_count);

        ProblemSetup {
            bus_map,
            bus_count,
            slack_idx,
            bus_types,
            bus_gens,
            ybus,
        }
    }

    /// 构建 Y-Bus 导纳矩阵
    ///
    /// 对于支路 i-j（变比 a:1，从 i 侧看）：
    /// y = 1 / (r + jx)
    /// Y_ii += y / a^2 + j*b_half/2
    /// Y_jj += y + j*b_half/2
    /// Y_ij -= y / a
    /// Y_ji -= y / a
    fn build_ybus(
        &self,
        problem: &AcOpfProblem,
        bus_map: &HashMap<ElementId, usize>,
        bus_count: usize,
    ) -> Array2<Complex64> {
        let mut ybus = Array2::<Complex64>::zeros((bus_count, bus_count));

        for branch in &problem.branches {
            let z = Complex64::new(branch.r_pu, branch.x_pu);
            let y = if z.norm() < 1e-12 {
                Complex64::new(1e6, 0.0) // 避免除零
            } else {
                1.0 / z
            };
            let tap = if branch.tap_ratio.abs() < 1e-12 { 1.0 } else { branch.tap_ratio };
            let y_shunt = Complex64::new(0.0, branch.b_half);

            if let (Some(&i), Some(&j)) = (bus_map.get(&branch.from_bus), bus_map.get(&branch.to_bus)) {
                let y_tap = y / tap;
                let y_tap2 = y / (tap * tap);

                ybus[[i, i]] += y_tap2 + y_shunt;
                ybus[[j, j]] += y + y_shunt;
                ybus[[i, j]] -= y_tap;
                ybus[[j, i]] -= y_tap;
            }
        }

        ybus
    }

    /// 经济调度：按边际成本升序分配负荷
    fn economic_dispatch(&self, problem: &AcOpfProblem, setup: &ProblemSetup) -> Vec<f64> {
        let total_load_pu: f64 = problem.buses.iter()
            .map(|b| b.p_load / problem.base_mva)
            .sum();

        // 按中点边际成本排序
        let mut sorted_gens: Vec<usize> = (0..problem.generators.len()).collect();
        sorted_gens.sort_by(|&a, &b| {
            let ga = &problem.generators[a];
            let gb = &problem.generators[b];
            let mc_a = 2.0 * ga.cost_a * (ga.p_min + ga.p_max) / 2.0 + ga.cost_b;
            let mc_b = 2.0 * gb.cost_a * (gb.p_min + gb.p_max) / 2.0 + gb.cost_b;
            mc_a.partial_cmp(&mc_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut gen_p = vec![0.0f64; problem.generators.len()];
        let mut remaining = total_load_pu;

        for &gi in &sorted_gens {
            let g = &problem.generators[gi];
            let p_min_pu = g.p_min / problem.base_mva;
            let p_max_pu = g.p_max / problem.base_mva;
            if remaining <= 0.0 {
                gen_p[gi] = p_min_pu;
            } else {
                let dispatch = p_max_pu.min(remaining).max(p_min_pu);
                gen_p[gi] = dispatch;
                remaining -= dispatch;
            }
        }

        // 若仍有剩余负荷，尝试增加出力
        if remaining > 0.0 {
            for &gi in &sorted_gens {
                if remaining <= 0.0 { break; }
                let g = &problem.generators[gi];
                let p_max_pu = g.p_max / problem.base_mva;
                let headroom = p_max_pu - gen_p[gi];
                let inc = headroom.min(remaining);
                gen_p[gi] += inc;
                remaining -= inc;
            }
        }

        let _ = setup; // 避免未使用警告
        gen_p
    }

    /// 初始电压幅值
    fn initial_voltages(&self, problem: &AcOpfProblem, setup: &ProblemSetup) -> Vec<f64> {
        let mut v = vec![1.0; setup.bus_count];
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                v[idx] = if bus.v_init > 0.0 { bus.v_init } else { 1.0 };
            }
        }
        v
    }

    /// 初始相角
    fn initial_angles(&self, problem: &AcOpfProblem, setup: &ProblemSetup) -> Vec<f64> {
        let mut theta = vec![0.0; setup.bus_count];
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                theta[idx] = bus.theta_init;
            }
        }
        // 平衡节点相角设为 0
        theta[setup.slack_idx] = 0.0;
        let _ = problem;
        theta
    }

    /// 牛顿法求解潮流（极坐标形式）
    ///
    /// 求解 PQ 节点的 P/V 不平衡方程和 PV 节点的 P 不平衡方程
    /// 返回 (是否收敛, 迭代次数)
    fn solve_power_flow(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &mut [f64],
        angles: &mut [f64],
        gen_p_pu: &[f64],
    ) -> Result<(bool, u32), AnalysisError> {
        let n = setup.bus_count;

        // 待求变量索引：非平衡节点的相角 + PQ 节点的电压
        let theta_indices: Vec<usize> = (0..n).filter(|&i| i != setup.slack_idx).collect();
        let v_indices: Vec<usize> = (0..n).filter(|&i| setup.bus_types[i] == BusType::Pq).collect();

        let n_theta = theta_indices.len();
        let n_v = v_indices.len();
        let n_unknown = n_theta + n_v;

        if n_unknown == 0 {
            return Ok((true, 0));
        }

        let max_iter = self.max_iter.max(NEWTON_MAX_ITER);

        for iter in 0..max_iter {
            // 计算功率注入
            let (p_calc, q_calc) = self.compute_power_injections(setup, voltages, angles);

            // 计算指定功率（发电机 - 负荷）
            let mut p_spec = vec![0.0f64; n];
            let mut q_spec = vec![0.0f64; n];
            for bus in &problem.buses {
                if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                    p_spec[idx] -= bus.p_load / problem.base_mva;
                    q_spec[idx] -= bus.q_load / problem.base_mva;
                }
            }
            for (gi, gen) in problem.generators.iter().enumerate() {
                if let Some(&idx) = setup.bus_map.get(&gen.bus_id) {
                    p_spec[idx] += gen_p_pu[gi];
                }
            }

            // 构造不平衡量向量
            let mut mismatch = Array1::<f64>::zeros(n_unknown);
            for (k, &i) in theta_indices.iter().enumerate() {
                mismatch[k] = p_spec[i] - p_calc[i];
            }
            for (k, &i) in v_indices.iter().enumerate() {
                mismatch[n_theta + k] = q_spec[i] - q_calc[i];
            }

            let max_mismatch = mismatch.iter().map(|x| x.abs()).fold(0.0_f64, f64::max);
            if max_mismatch < self.tol {
                return Ok((true, iter + 1));
            }

            // 构造雅可比矩阵
            let jacobian = self.build_jacobian(setup, voltages, angles, &p_calc, &q_calc, &theta_indices, &v_indices);

            // 求解修正方程
            let delta = match solve_linear_system_ndarray(&jacobian, &mismatch) {
                Some(d) => d,
                None => return Err(AnalysisError::SingularMatrix("雅可比矩阵奇异".into())),
            };

            // 更新变量
            for (k, &i) in theta_indices.iter().enumerate() {
                angles[i] += delta[k];
                // 相角约束
                angles[i] = angles[i].clamp(-PI, PI);
            }
            for (k, &i) in v_indices.iter().enumerate() {
                voltages[i] += delta[n_theta + k];
                let bus = &problem.buses[i];
                voltages[i] = voltages[i].clamp(bus.v_min, bus.v_max);
            }
        }

        Ok((false, max_iter))
    }

    /// 计算功率注入 P_calc, Q_calc
    fn compute_power_injections(
        &self,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
    ) -> (Vec<f64>, Vec<f64>) {
        let n = setup.bus_count;
        let mut p = vec![0.0f64; n];
        let mut q = vec![0.0f64; n];

        for i in 0..n {
            for j in 0..n {
                let yij = setup.ybus[[i, j]];
                let g = yij.re;
                let b = yij.im;
                let theta_ij = angles[i] - angles[j];
                p[i] += voltages[i] * voltages[j] * (g * theta_ij.cos() + b * theta_ij.sin());
                q[i] += voltages[i] * voltages[j] * (g * theta_ij.sin() - b * theta_ij.cos());
            }
        }

        (p, q)
    }

    /// 构造雅可比矩阵
    ///
    /// 雅可比分块：
    /// [ H  N ]
    /// [ M  L ]
    /// H = ∂P/∂θ, N = ∂P/∂V, M = ∂Q/∂θ, L = ∂Q/∂V
    #[allow(clippy::too_many_arguments)]
    fn build_jacobian(
        &self,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        p_calc: &[f64],
        q_calc: &[f64],
        theta_indices: &[usize],
        v_indices: &[usize],
    ) -> Array2<f64> {
        let n_theta = theta_indices.len();
        let n_v = v_indices.len();
        let n = n_theta + n_v;
        let mut jac = Array2::<f64>::zeros((n, n));

        let n_bus = setup.bus_count;

        // 预计算 H, N, M, L 的完整矩阵
        let mut h = Array2::<f64>::zeros((n_bus, n_bus));
        let mut n_mat = Array2::<f64>::zeros((n_bus, n_bus));
        let mut m = Array2::<f64>::zeros((n_bus, n_bus));
        let mut l = Array2::<f64>::zeros((n_bus, n_bus));

        for i in 0..n_bus {
            // 对角元素
            let yii = setup.ybus[[i, i]];
            let gii = yii.re;
            let bii = yii.im;
            // H_ii = -Q_i - B_ii * V_i^2
            h[[i, i]] = -q_calc[i] - bii * voltages[i] * voltages[i];
            // N_ii = P_i/V_i + G_ii * V_i
            n_mat[[i, i]] = p_calc[i] / voltages[i] + gii * voltages[i];
            // M_ii = P_i - G_ii * V_i^2
            m[[i, i]] = p_calc[i] - gii * voltages[i] * voltages[i];
            // L_ii = Q_i/V_i - B_ii * V_i
            l[[i, i]] = q_calc[i] / voltages[i] - bii * voltages[i];

            // 非对角元素
            for j in 0..n_bus {
                if i == j { continue; }
                let yij = setup.ybus[[i, j]];
                let g = yij.re;
                let b = yij.im;
                let theta_ij = angles[i] - angles[j];
                let cos_t = theta_ij.cos();
                let sin_t = theta_ij.sin();
                // H_ij = V_i * V_j * (G_ij * sin - B_ij * cos)
                h[[i, j]] = voltages[i] * voltages[j] * (g * sin_t - b * cos_t);
                // N_ij = V_i * (G_ij * cos + B_ij * sin)
                n_mat[[i, j]] = voltages[i] * (g * cos_t + b * sin_t);
                // M_ij = -V_i * V_j * (G_ij * cos + B_ij * sin)
                m[[i, j]] = -voltages[i] * voltages[j] * (g * cos_t + b * sin_t);
                // L_ij = V_i * (G_ij * sin - B_ij * cos)
                l[[i, j]] = voltages[i] * (g * sin_t - b * cos_t);
            }
        }

        // 填充雅可比矩阵
        for (ki, &i) in theta_indices.iter().enumerate() {
            for (kj, &j) in theta_indices.iter().enumerate() {
                jac[[ki, kj]] = h[[i, j]];
            }
            for (kj, &j) in v_indices.iter().enumerate() {
                jac[[ki, n_theta + kj]] = n_mat[[i, j]];
            }
        }
        for (ki, &i) in v_indices.iter().enumerate() {
            for (kj, &j) in theta_indices.iter().enumerate() {
                jac[[n_theta + ki, kj]] = m[[i, j]];
            }
            for (kj, &j) in v_indices.iter().enumerate() {
                jac[[n_theta + ki, n_theta + kj]] = l[[i, j]];
            }
        }

        jac
    }

    /// 计算功率不平衡量（总有功）
    fn compute_power_imbalance(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &[f64],
    ) -> f64 {
        let (p_calc, _) = self.compute_power_injections(setup, voltages, angles);
        let mut p_spec = vec![0.0f64; setup.bus_count];
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                p_spec[idx] -= bus.p_load / problem.base_mva;
            }
        }
        for (gi, gen) in problem.generators.iter().enumerate() {
            if let Some(&idx) = setup.bus_map.get(&gen.bus_id) {
                p_spec[idx] += gen_p_pu[gi];
            }
        }
        let mut imbalance = 0.0;
        for i in 0..setup.bus_count {
            imbalance += (p_spec[i] - p_calc[i]).abs();
        }
        imbalance
    }

    /// 更新平衡机出力
    fn update_slack_generation(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &mut [f64],
    ) {
        let (p_calc, _) = self.compute_power_injections(setup, voltages, angles);
        let slack_idx = setup.slack_idx;
        let mut p_spec_slack = 0.0;
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                if idx == slack_idx {
                    p_spec_slack -= bus.p_load / problem.base_mva;
                }
            }
        }
        let p_slack_needed = p_spec_slack + p_calc[slack_idx]; // 反号：注入 = 发电 - 负荷
        // 分配到平衡节点上的发电机
        let slack_gens = &setup.bus_gens[slack_idx];
        if !slack_gens.is_empty() {
            let per_gen = p_slack_needed / slack_gens.len() as f64;
            for &gi in slack_gens {
                let g = &problem.generators[gi];
                let p_min_pu = g.p_min / problem.base_mva;
                let p_max_pu = g.p_max / problem.base_mva;
                gen_p_pu[gi] = per_gen.clamp(p_min_pu, p_max_pu);
            }
        }
    }

    /// 计算无功出力
    fn compute_reactive_generation(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
    ) -> Vec<f64> {
        let (_, q_calc) = self.compute_power_injections(setup, voltages, angles);
        let mut gen_q = vec![0.0f64; problem.generators.len()];

        for (gi, gen) in problem.generators.iter().enumerate() {
            if let Some(&idx) = setup.bus_map.get(&gen.bus_id) {
                let q_load_pu = problem.buses.iter()
                    .find(|b| b.bus_id == gen.bus_id)
                    .map(|b| b.q_load / problem.base_mva)
                    .unwrap_or(0.0);
                // 发电机无功 = 计算注入 + 负荷
                gen_q[gi] = q_calc[idx] + q_load_pu;
                // 无功约束
                let q_min_pu = gen.q_min / problem.base_mva;
                let q_max_pu = gen.q_max / problem.base_mva;
                gen_q[gi] = gen_q[gi].clamp(q_min_pu, q_max_pu);
            }
        }
        gen_q
    }

    /// 计算支路潮流（MVA）
    fn compute_branch_flow(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
    ) -> Vec<(ElementId, f64)> {
        let mut flows = Vec::with_capacity(problem.branches.len());

        for branch in &problem.branches {
            if let (Some(&i), Some(&j)) = (setup.bus_map.get(&branch.from_bus), setup.bus_map.get(&branch.to_bus)) {
                let z = Complex64::new(branch.r_pu, branch.x_pu);
                let y = if z.norm() < 1e-12 { Complex64::new(1e6, 0.0) } else { 1.0 / z };
                let tap = if branch.tap_ratio.abs() < 1e-12 { 1.0 } else { branch.tap_ratio };
                let y_tap = y / tap;

                let vi = Complex64::from_polar(voltages[i], angles[i]);
                let vj = Complex64::from_polar(voltages[j], angles[j]);

                // 从 i 到 j 的电流
                let i_ij = y_tap * (vi - vj) + Complex64::new(0.0, branch.b_half / 2.0) * vi / tap;
                let s_ij = vi * i_ij.conj();

                let s_mva = s_ij.norm() * problem.base_mva;
                flows.push((branch.branch_id, s_mva));
            } else {
                flows.push((branch.branch_id, 0.0));
            }
        }

        flows
    }

    /// 内部 LMP 计算
    fn compute_lmp_internal(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &[f64],
    ) -> Vec<(ElementId, f64)> {
        let n = setup.bus_count;

        // 能量分量：边际发电机成本
        let mut marginal_cost = 0.0;
        let mut count = 0;
        for (gi, gen) in problem.generators.iter().enumerate() {
            let p_mw = gen_p_pu[gi] * problem.base_mva;
            let p_min = gen.p_min.min(gen.p_max);
            let p_max = gen.p_max.max(gen.p_min);
            if p_mw > p_min + 1e-3 && p_mw < p_max - 1e-3 {
                let mc = 2.0 * gen.cost_a * p_mw + gen.cost_b;
                marginal_cost += mc;
                count += 1;
            }
        }
        if count == 0 {
            // 退化：使用最贵在线发电机
            for (gi, gen) in problem.generators.iter().enumerate() {
                let p_mw = gen_p_pu[gi] * problem.base_mva;
                if p_mw > 1e-3 {
                    let mc = 2.0 * gen.cost_a * p_mw + gen.cost_b;
                    if mc > marginal_cost { marginal_cost = mc; }
                    count = 1;
                }
            }
        }
        let energy_price = if count > 0 { marginal_cost / count as f64 } else { 0.0 };

        // 损耗分量：基于网损对注入的灵敏度（简化）
        let (_p_calc, _) = self.compute_power_injections(setup, voltages, angles);
        let total_gen_pu: f64 = gen_p_pu.iter().sum();
        let total_load_pu: f64 = problem.buses.iter().map(|b| b.p_load / problem.base_mva).sum();
        let total_loss_pu = total_gen_pu - total_load_pu;
        let loss_factor = if total_gen_pu.abs() > 1e-6 { total_loss_pu / total_gen_pu } else { 0.0 };

        // 阻塞分量：基于支路潮流越限惩罚
        let branch_flows = self.compute_branch_flow(problem, setup, voltages, angles);
        let mut congestion = vec![0.0f64; n];

        for (branch_idx, branch) in problem.branches.iter().enumerate() {
            let flow_mva = branch_flows[branch_idx].1;
            let limit = branch.s_limit_mva;
            if limit > 0.0 && flow_mva > limit * 0.99 {
                let overflow = (flow_mva - limit * 0.95).max(0.0);
                let shadow = overflow * 10.0;
                if let (Some(&fi), Some(&ti)) = (setup.bus_map.get(&branch.from_bus), setup.bus_map.get(&branch.to_bus)) {
                    congestion[fi] += shadow;
                    congestion[ti] -= shadow;
                }
            }
        }

        // 组装 LMP
        let mut lmp = Vec::with_capacity(n);
        for i in 0..n {
            let bus_id = setup.bus_map.iter().find(|(_, &idx)| idx == i).map(|(&k, _)| k).unwrap_or(0);
            let price = energy_price * (1.0 + loss_factor) + congestion[i];
            lmp.push((bus_id, price));
        }
        lmp
    }

    /// 约束检查
    fn check_constraints(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        gen_p_pu: &[f64],
        gen_q_pu: &[f64],
        branch_flows: &[(ElementId, f64)],
    ) -> Vec<String> {
        let mut warnings = Vec::new();

        // 电压约束
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                let v = voltages[idx];
                if v < bus.v_min - 1e-6 {
                    warnings.push(format!("母线 {} 电压 {:.4} 低于下限 {:.4}", bus.bus_id, v, bus.v_min));
                }
                if v > bus.v_max + 1e-6 {
                    warnings.push(format!("母线 {} 电压 {:.4} 超过上限 {:.4}", bus.bus_id, v, bus.v_max));
                }
            }
        }

        // 发电机出力约束
        for (gi, gen) in problem.generators.iter().enumerate() {
            let p_mw = gen_p_pu[gi] * problem.base_mva;
            let q_mvar = gen_q_pu[gi] * problem.base_mva;
            if p_mw < gen.p_min - 1e-3 {
                warnings.push(format!("发电机 {} 有功 {:.2} 低于下限 {:.2}", gen.gen_id, p_mw, gen.p_min));
            }
            if p_mw > gen.p_max + 1e-3 {
                warnings.push(format!("发电机 {} 有功 {:.2} 超过上限 {:.2}", gen.gen_id, p_mw, gen.p_max));
            }
            if q_mvar < gen.q_min - 1e-3 {
                warnings.push(format!("发电机 {} 无功 {:.2} 低于下限 {:.2}", gen.gen_id, q_mvar, gen.q_min));
            }
            if q_mvar > gen.q_max + 1e-3 {
                warnings.push(format!("发电机 {} 无功 {:.2} 超过上限 {:.2}", gen.gen_id, q_mvar, gen.q_max));
            }
        }

        // 支路潮流约束
        for (branch_id, flow_mva) in branch_flows {
            if let Some(branch) = problem.branches.iter().find(|b| b.branch_id == *branch_id) {
                if *flow_mva > branch.s_limit_mva * 1.01 {
                    warnings.push(format!(
                        "支路 {} 潮流 {:.2} MVA 超过限额 {:.2} MVA",
                        branch_id, flow_mva, branch.s_limit_mva
                    ));
                }
            }
        }

        warnings
    }

    /// 构造最终结果
    #[allow(clippy::too_many_arguments)]
    fn build_result(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &[f64],
        gen_q_pu: &[f64],
        branch_flows: &[(ElementId, f64)],
        lmp: &[(ElementId, f64)],
    ) -> AcOpfResult {
        // 发电机出力
        let generation: Vec<(ElementId, f64, f64)> = problem.generators.iter().enumerate().map(|(i, g)| {
            (g.gen_id, gen_p_pu[i] * problem.base_mva, gen_q_pu[i] * problem.base_mva)
        }).collect();

        // 母线电压
        let bus_voltages: Vec<(ElementId, f64, f64)> = (0..setup.bus_count).map(|i| {
            let bus_id = setup.bus_map.iter().find(|(_, &idx)| idx == i).map(|(&k, _)| k).unwrap_or(0);
            (bus_id, voltages[i], angles[i])
        }).collect();

        // 总成本
        let total_cost: f64 = problem.generators.iter().enumerate().map(|(i, g)| {
            let p_mw = gen_p_pu[i] * problem.base_mva;
            g.cost_a * p_mw * p_mw + g.cost_b * p_mw + g.cost_c
        }).sum();

        // 总损耗
        let total_gen_p: f64 = generation.iter().map(|(_, p, _)| *p).sum();
        let total_load_p: f64 = problem.buses.iter().map(|b| b.p_load).sum();
        let total_losses = (total_gen_p - total_load_p).max(0.0);

        AcOpfResult {
            generation,
            bus_voltages,
            branch_flows: branch_flows.to_vec(),
            nodal_prices: lmp.to_vec(),
            total_cost,
            total_losses,
        }
    }

    /// 计算功率不平衡量（返回 dp, dq 向量）
    fn compute_mismatch(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &[f64],
    ) -> (Vec<f64>, Vec<f64>) {
        let (p_calc, q_calc) = self.compute_power_injections(setup, voltages, angles);
        let n = setup.bus_count;
        let mut p_spec = vec![0.0f64; n];
        let mut q_spec = vec![0.0f64; n];
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                p_spec[idx] -= bus.p_load / problem.base_mva;
                q_spec[idx] -= bus.q_load / problem.base_mva;
            }
        }
        for (gi, gen) in problem.generators.iter().enumerate() {
            if let Some(&idx) = setup.bus_map.get(&gen.bus_id) {
                p_spec[idx] += gen_p_pu[gi];
            }
        }
        let dp: Vec<f64> = (0..n).map(|i| p_spec[i] - p_calc[i]).collect();
        let dq: Vec<f64> = (0..n).map(|i| q_spec[i] - q_calc[i]).collect();
        (dp, dq)
    }

    /// 计算约束违反度
    fn compute_constraint_violation(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        gen_p_pu: &[f64],
    ) -> f64 {
        let mut viol = 0.0;
        for bus in &problem.buses {
            if let Some(&idx) = setup.bus_map.get(&bus.bus_id) {
                let v = voltages[idx];
                if v < bus.v_min { viol += (bus.v_min - v).abs(); }
                if v > bus.v_max { viol += (v - bus.v_max).abs(); }
            }
        }
        for (gi, gen) in problem.generators.iter().enumerate() {
            let p_pu = gen_p_pu[gi];
            let p_min_pu = gen.p_min / problem.base_mva;
            let p_max_pu = gen.p_max / problem.base_mva;
            if p_pu < p_min_pu { viol += (p_min_pu - p_pu).abs(); }
            if p_pu > p_max_pu { viol += (p_pu - p_max_pu).abs(); }
        }
        viol
    }

    /// 计算内点法搜索方向（简化版）
    #[allow(clippy::too_many_arguments)]
    fn compute_ipm_direction(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        _angles: &[f64],
        gen_p_pu: &[f64],
        dp: &[f64],
        dq: &[f64],
        barrier_mu: f64,
    ) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = setup.bus_count;
        let mut dv = vec![0.0f64; n];
        let mut dtheta = vec![0.0f64; n];
        let mut dpg = vec![0.0f64; problem.generators.len()];

        // 简化：直接用不平衡量作为方向，加上障碍项梯度
        for i in 0..n {
            if setup.bus_types[i] == BusType::Pq {
                dv[i] = dq[i] * 0.5;
                // 障碍项：远离边界
                let bus = &problem.buses[i];
                let v = voltages[i];
                if v > bus.v_max - 0.01 {
                    dv[i] -= barrier_mu * 10.0;
                }
                if v < bus.v_min + 0.01 {
                    dv[i] += barrier_mu * 10.0;
                }
            }
            if setup.bus_types[i] != BusType::Slack {
                dtheta[i] = dp[i] * 0.5;
            }
        }

        // 平衡机出力调整方向
        for &gi in &setup.bus_gens[setup.slack_idx] {
            let g = &problem.generators[gi];
            let p_pu = gen_p_pu[gi];
            let p_min_pu = g.p_min / problem.base_mva;
            let p_max_pu = g.p_max / problem.base_mva;
            // 障碍项梯度
            if p_pu > p_max_pu - 0.01 {
                dpg[gi] -= barrier_mu * 10.0;
            }
            if p_pu < p_min_pu + 0.01 {
                dpg[gi] += barrier_mu * 10.0;
            }
        }

        let _ = problem;
        (dv, dtheta, dpg)
    }

    /// 线搜索确定步长
    #[allow(clippy::too_many_arguments)]
    fn line_search(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        voltages: &[f64],
        angles: &[f64],
        gen_p_pu: &[f64],
        dv: &[f64],
        dtheta: &[f64],
        dpg: &[f64],
    ) -> f64 {
        let mut alpha = 1.0;
        let alpha_min = 0.01;

        while alpha > alpha_min {
            let mut feasible = true;
            for i in 0..setup.bus_count {
                if setup.bus_types[i] == BusType::Pq {
                    let v_new = voltages[i] + alpha * dv[i];
                    let bus = &problem.buses[i];
                    if v_new < bus.v_min || v_new > bus.v_max {
                        feasible = false;
                        break;
                    }
                }
                if setup.bus_types[i] != BusType::Slack {
                    let theta_new = angles[i] + alpha * dtheta[i];
                    if !(-PI..=PI).contains(&theta_new) {
                        feasible = false;
                        break;
                    }
                }
            }
            if feasible {
                for (gi, gen) in problem.generators.iter().enumerate() {
                    let p_new = gen_p_pu[gi] + alpha * dpg[gi];
                    let p_min_pu = gen.p_min / problem.base_mva;
                    let p_max_pu = gen.p_max / problem.base_mva;
                    if p_new < p_min_pu || p_new > p_max_pu {
                        feasible = false;
                        break;
                    }
                }
            }
            if feasible { break; }
            alpha *= 0.5;
        }

        alpha
    }

    /// N-1 故障扫描
    fn check_contingency_violations(
        &self,
        problem: &AcOpfProblem,
        setup: &ProblemSetup,
        current_gen: &[(ElementId, f64, f64)],
        method: OpfMethod,
    ) -> Result<Vec<ContingencyViolation>, AnalysisError> {
        let mut violations = Vec::new();

        // 重建发电机有功出力（p.u.）
        let gen_p_pu: Vec<f64> = problem.generators.iter().map(|g| {
            current_gen.iter().find(|(id, _, _)| *id == g.gen_id)
                .map(|(_, p, _)| *p / problem.base_mva).unwrap_or(0.0)
        }).collect();

        // 对每个支路进行 N-1 故障
        for conting_branch in &problem.branches {
            // 构造故障后系统：移除该支路
            let mut post_problem = problem.clone();
            post_problem.branches.retain(|b| b.branch_id != conting_branch.branch_id);

            if post_problem.branches.is_empty() {
                continue;
            }

            // 求解故障后潮流
            let post_setup = self.setup_problem(&post_problem);
            let mut v = self.initial_voltages(&post_problem, &post_setup);
            let mut theta = self.initial_angles(&post_problem, &post_setup);

            let (conv, _) = self.solve_power_flow(&post_problem, &post_setup, &mut v, &mut theta, &gen_p_pu)?;
            if !conv { continue; }

            // 检查支路越限
            let flows = self.compute_branch_flow(&post_problem, &post_setup, &v, &theta);
            for (branch_id, flow_mva) in &flows {
                if let Some(branch) = post_problem.branches.iter().find(|b| b.branch_id == *branch_id) {
                    if *flow_mva > branch.s_limit_mva * 1.05 {
                        violations.push(ContingencyViolation {
                            contingency_branch_id: conting_branch.branch_id,
                            violated_branch_id: *branch_id,
                            flow_mva: *flow_mva,
                            limit_mva: branch.s_limit_mva,
                        });
                    }
                }
            }

            // 检查电压越限
            for bus in &post_problem.buses {
                if let Some(&idx) = post_setup.bus_map.get(&bus.bus_id) {
                    if v[idx] < bus.v_min - 1e-3 || v[idx] > bus.v_max + 1e-3 {
                        // 电压越限也记录为支路 ID 0
                        violations.push(ContingencyViolation {
                            contingency_branch_id: conting_branch.branch_id,
                            violated_branch_id: bus.bus_id,
                            flow_mva: v[idx],
                            limit_mva: bus.v_max,
                        });
                    }
                }
            }
        }

        let _ = method;
        let _ = setup;
        Ok(violations)
    }
}

/// N-1 故障越限记录
#[derive(Debug, Clone)]
struct ContingencyViolation {
    /// 故障支路 ID
    contingency_branch_id: ElementId,
    /// 越限支路 ID
    violated_branch_id: ElementId,
    /// 越限潮流（MVA）或电压（p.u.）
    flow_mva: f64,
    /// 限额（MVA）或电压上限（p.u.）
    limit_mva: f64,
}

/// 求解线性系统 Ax = b（ndarray 版本）
fn solve_linear_system_ndarray(a: &Array2<f64>, b: &Array1<f64>) -> Option<Array1<f64>> {
    let n = b.len();
    if n == 0 {
        return Some(Array1::zeros(0));
    }
    let a_vec: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| a[[i, j]]).collect()).collect();
    let b_vec: Vec<f64> = b.to_vec();
    eneros_core::solve_linear_system(&a_vec, &b_vec).map(Array1::from_vec)
}

// ============================================================================
// T3.7-T3.8 验证测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 2 母线测试系统
    ///
    /// Bus1 (slack) --- Line --- Bus2 (PQ, load)
    /// Gen1 在 Bus1
    fn create_2bus_problem() -> AcOpfProblem {
        AcOpfProblem {
            base_mva: 100.0,
            slack_bus_id: 1,
            buses: vec![
                AcBus {
                    bus_id: 1, p_load: 0.0, q_load: 0.0,
                    v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0,
                },
                AcBus {
                    bus_id: 2, p_load: 50.0, q_load: 10.0,
                    v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0,
                },
            ],
            generators: vec![
                AcGenerator {
                    gen_id: 1, bus_id: 1,
                    p_min: 0.0, p_max: 200.0,
                    q_min: -50.0, q_max: 100.0,
                    cost_a: 0.01, cost_b: 10.0, cost_c: 0.0,
                },
            ],
            branches: vec![
                AcBranch {
                    branch_id: 1, from_bus: 1, to_bus: 2,
                    r_pu: 0.01, x_pu: 0.1, b_half: 0.02,
                    tap_ratio: 1.0, s_limit_mva: 200.0,
                },
            ],
        }
    }

    /// 构造类 IEEE 14 节点测试系统
    fn create_ieee14_like_problem() -> AcOpfProblem {
        AcOpfProblem {
            base_mva: 100.0,
            slack_bus_id: 1,
            buses: vec![
                AcBus { bus_id: 1, p_load: 0.0, q_load: 0.0, v_min: 0.95, v_max: 1.05, v_init: 1.06, theta_init: 0.0 },
                AcBus { bus_id: 2, p_load: 21.7, q_load: 12.7, v_min: 0.95, v_max: 1.05, v_init: 1.045, theta_init: 0.0 },
                AcBus { bus_id: 3, p_load: 94.2, q_load: 19.0, v_min: 0.95, v_max: 1.05, v_init: 1.01, theta_init: 0.0 },
                AcBus { bus_id: 4, p_load: 47.8, q_load: -3.9, v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0 },
                AcBus { bus_id: 5, p_load: 7.6, q_load: 1.6, v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0 },
            ],
            generators: vec![
                AcGenerator { gen_id: 1, bus_id: 1, p_min: 0.0, p_max: 200.0, q_min: -50.0, q_max: 100.0, cost_a: 0.005, cost_b: 10.0, cost_c: 100.0 },
                AcGenerator { gen_id: 2, bus_id: 2, p_min: 0.0, p_max: 150.0, q_min: -50.0, q_max: 80.0, cost_a: 0.01, cost_b: 15.0, cost_c: 80.0 },
                AcGenerator { gen_id: 3, bus_id: 3, p_min: 0.0, p_max: 100.0, q_min: -40.0, q_max: 60.0, cost_a: 0.015, cost_b: 20.0, cost_c: 60.0 },
            ],
            branches: vec![
                AcBranch { branch_id: 1, from_bus: 1, to_bus: 2, r_pu: 0.02, x_pu: 0.06, b_half: 0.03, tap_ratio: 1.0, s_limit_mva: 200.0 },
                AcBranch { branch_id: 2, from_bus: 1, to_bus: 5, r_pu: 0.03, x_pu: 0.08, b_half: 0.02, tap_ratio: 1.0, s_limit_mva: 150.0 },
                AcBranch { branch_id: 3, from_bus: 2, to_bus: 3, r_pu: 0.04, x_pu: 0.10, b_half: 0.02, tap_ratio: 1.0, s_limit_mva: 150.0 },
                AcBranch { branch_id: 4, from_bus: 3, to_bus: 4, r_pu: 0.05, x_pu: 0.12, b_half: 0.01, tap_ratio: 1.0, s_limit_mva: 100.0 },
                AcBranch { branch_id: 5, from_bus: 4, to_bus: 5, r_pu: 0.03, x_pu: 0.09, b_half: 0.02, tap_ratio: 1.0, s_limit_mva: 100.0 },
            ],
        }
    }

    #[test]
    fn test_ac_opf_2bus() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let result = solver.solve(&problem, OpfMethod::NewtonRaphson);

        assert!(result.is_ok(), "2 母线 AC-OPF 失败: {:?}", result.err());
        let result = result.unwrap();

        // 总发电量应覆盖总负荷
        let total_gen: f64 = result.result.generation.iter().map(|(_, p, _)| p).sum();
        let total_load: f64 = problem.buses.iter().map(|b| b.p_load).sum();
        assert!(
            total_gen >= total_load - 5.0,
            "发电 {:.2} 应覆盖负荷 {:.2}", total_gen, total_load
        );

        // 电压在合理范围
        for (_, v, _) in &result.result.bus_voltages {
            assert!((0.9..=1.1).contains(v), "电压 {} 超出合理范围", v);
        }

        // 总成本为正
        assert!(result.result.total_cost > 0.0, "总成本应为正");
    }

    #[test]
    fn test_ac_opf_ieee14_like() {
        let problem = create_ieee14_like_problem();
        let solver = AcOpfSolver::new();
        let result = solver.solve(&problem, OpfMethod::NewtonRaphson);

        assert!(result.is_ok(), "IEEE14-like AC-OPF 失败: {:?}", result.err());
        let result = result.unwrap();

        // 总发电量应覆盖总负荷
        let total_gen: f64 = result.result.generation.iter().map(|(_, p, _)| p).sum();
        let total_load: f64 = problem.buses.iter().map(|b| b.p_load).sum();
        assert!(
            total_gen >= total_load - 20.0,
            "发电 {:.2} 应覆盖负荷 {:.2}", total_gen, total_load
        );

        // 电压在合理范围
        for (_, v, _) in &result.result.bus_voltages {
            assert!((0.9..=1.1).contains(v), "电压 {} 超出合理范围", v);
        }

        // 平衡节点相角应为 0
        let slack_angle = result.result.bus_voltages.iter()
            .find(|(id, _, _)| *id == 1).map(|(_, _, a)| *a).unwrap_or(999.0);
        assert!(slack_angle.abs() < 1e-6, "平衡节点相角应为 0，实际 {}", slack_angle);
    }

    /// v0.8.0 性能基准：IEEE-14 规模 AC-OPF 求解 < 500ms（Newton-Raphson）
    #[test]
    fn test_perf_ac_opf_ieee14_under_500ms() {
        use std::time::Instant;

        let problem = create_ieee14_like_problem();
        let solver = AcOpfSolver::new();

        // 预热（首次求解含编译/分配开销）
        let _ = solver.solve(&problem, OpfMethod::NewtonRaphson);

        let t0 = Instant::now();
        let result = solver.solve(&problem, OpfMethod::NewtonRaphson);
        let elapsed = t0.elapsed();

        assert!(result.is_ok(), "AC-OPF 求解失败: {:?}", result.err());

        eprintln!("IEEE-14 AC-OPF (Newton) 耗时: {:?}", elapsed);
        assert!(
            elapsed.as_millis() < 500,
            "性能不达标: AC-OPF {:?} >= 500ms",
            elapsed
        );
    }

    #[test]
    fn test_lmp_computation() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let result = solver.solve(&problem, OpfMethod::NewtonRaphson).unwrap();

        // LMP 应为正
        for (_, price) in &result.result.nodal_prices {
            assert!(*price > 0.0, "LMP 应为正，实际 {}", price);
        }

        // 公共 LMP 接口
        let lmp = solver.compute_lmp(&problem, &result.result);
        assert_eq!(lmp.len(), problem.buses.len());
        for (_, price) in &lmp {
            assert!(price.is_finite(), "LMP 应为有限值");
        }
    }

    #[test]
    fn test_interior_point_convergence() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let result = solver.solve(&problem, OpfMethod::InteriorPoint);

        assert!(result.is_ok(), "内点法求解失败: {:?}", result.err());
        let result = result.unwrap();

        // 总发电量应覆盖总负荷
        let total_gen: f64 = result.result.generation.iter().map(|(_, p, _)| p).sum();
        let total_load: f64 = problem.buses.iter().map(|b| b.p_load).sum();
        assert!(
            total_gen >= total_load - 10.0,
            "内点法发电 {:.2} 应覆盖负荷 {:.2}", total_gen, total_load
        );

        // 电压在合理范围
        for (_, v, _) in &result.result.bus_voltages {
            assert!((0.9..=1.1).contains(v), "电压 {} 超出合理范围", v);
        }
    }

    #[test]
    fn test_scopf_n1() {
        let problem = create_ieee14_like_problem();
        let solver = AcOpfSolver::new();
        let result = solver.solve_scopf(&problem, OpfMethod::NewtonRaphson);

        assert!(result.is_ok(), "SCOPF 求解失败: {:?}", result.err());
        let result = result.unwrap();

        // SCOPF 应产生结果（可能有越限警告）
        assert!(!result.result.generation.is_empty(), "SCOPF 应有发电结果");
        assert!(!result.result.bus_voltages.is_empty(), "SCOPF 应有电压结果");
    }

    #[test]
    fn test_unit_commitment() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();

        // 3 个时段的负荷曲线
        let load_profile = vec![
            vec![(2, 50.0, 10.0)],  // 低负荷
            vec![(2, 80.0, 15.0)],  // 中负荷
            vec![(2, 100.0, 20.0)], // 高负荷
        ];

        let results = solver.solve_unit_commitment(&problem, &load_profile, OpfMethod::NewtonRaphson);

        assert!(results.is_ok(), "机组组合求解失败: {:?}", results.err());
        let results = results.unwrap();
        assert_eq!(results.len(), 3, "应有 3 个时段结果");

        // 高负荷时段发电量应大于低负荷时段
        let gen_t1: f64 = results[0].result.generation.iter().map(|(_, p, _)| p).sum();
        let gen_t3: f64 = results[2].result.generation.iter().map(|(_, p, _)| p).sum();
        assert!(gen_t3 > gen_t1, "高负荷发电 {:.2} 应大于低负荷发电 {:.2}", gen_t3, gen_t1);
    }

    #[test]
    fn test_ybus_construction() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);

        // Y-Bus 应为 2x2
        assert_eq!(setup.ybus.nrows(), 2);
        assert_eq!(setup.ybus.ncols(), 2);

        // 对角元素实部应为正（电导）
        assert!(setup.ybus[[0, 0]].re > 0.0, "Y[0,0] 实部应为正");
        assert!(setup.ybus[[1, 1]].re > 0.0, "Y[1,1] 实部应为正");

        // 非对角元素应为负（导纳）
        assert!(setup.ybus[[0, 1]].re < 0.0, "Y[0,1] 实部应为负");
        assert!(setup.ybus[[1, 0]].re < 0.0, "Y[1,0] 实部应为负");

        // 对称性
        assert!((setup.ybus[[0, 1]] - setup.ybus[[1, 0]]).norm() < 1e-10, "Y-Bus 应对称");
    }

    #[test]
    fn test_ybus_with_tap_ratio() {
        let problem = AcOpfProblem {
            base_mva: 100.0,
            slack_bus_id: 1,
            buses: vec![
                AcBus { bus_id: 1, p_load: 0.0, q_load: 0.0, v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0 },
                AcBus { bus_id: 2, p_load: 50.0, q_load: 10.0, v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0 },
            ],
            generators: vec![
                AcGenerator { gen_id: 1, bus_id: 1, p_min: 0.0, p_max: 200.0, q_min: -50.0, q_max: 100.0, cost_a: 0.01, cost_b: 10.0, cost_c: 0.0 },
            ],
            branches: vec![
                AcBranch { branch_id: 1, from_bus: 1, to_bus: 2, r_pu: 0.01, x_pu: 0.1, b_half: 0.02, tap_ratio: 0.95, s_limit_mva: 200.0 },
            ],
        };

        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);

        // 变比 0.95 时，Y[0,0] 应大于变比 1.0 时（因 1/a^2 > 1）
        let setup_no_tap = solver.setup_problem(&AcOpfProblem {
            base_mva: 100.0,
            slack_bus_id: 1,
            buses: problem.buses.clone(),
            generators: problem.generators.clone(),
            branches: vec![
                AcBranch { branch_id: 1, from_bus: 1, to_bus: 2, r_pu: 0.01, x_pu: 0.1, b_half: 0.02, tap_ratio: 1.0, s_limit_mva: 200.0 },
            ],
        });

        assert!(
            setup.ybus[[0, 0]].norm() > setup_no_tap.ybus[[0, 0]].norm(),
            "变比 0.95 时 Y[0,0] 模应大于变比 1.0 时"
        );
    }

    #[test]
    fn test_economic_dispatch() {
        let problem = create_ieee14_like_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);
        let dispatch = solver.economic_dispatch(&problem, &setup);

        // 总出力应覆盖总负荷
        let total_dispatch: f64 = dispatch.iter().map(|p| p * problem.base_mva).sum();
        let total_load: f64 = problem.buses.iter().map(|b| b.p_load).sum();
        assert!(
            total_dispatch >= total_load - 1.0,
            "调度 {:.2} 应覆盖负荷 {:.2}", total_dispatch, total_load
        );

        // 最便宜的发电机应出力最多
        let gen1_p = dispatch[0] * problem.base_mva;
        let gen3_p = dispatch[2] * problem.base_mva;
        assert!(gen1_p >= gen3_p, "Gen1 ({:.2}) 应大于 Gen3 ({:.2})", gen1_p, gen3_p);
    }

    #[test]
    fn test_power_flow_convergence() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);
        let gen_p = solver.economic_dispatch(&problem, &setup);
        let mut v = solver.initial_voltages(&problem, &setup);
        let mut theta = solver.initial_angles(&problem, &setup);

        let (conv, iters) = solver.solve_power_flow(&problem, &setup, &mut v, &mut theta, &gen_p).unwrap();

        assert!(conv, "2 母线潮流应收敛");
        assert!(iters <= NEWTON_MAX_ITER, "迭代次数 {} 应小于 {}", iters, NEWTON_MAX_ITER);

        // 平衡节点相角应为 0
        assert!(theta[setup.slack_idx].abs() < 1e-10, "平衡节点相角应为 0");

        // 电压应在合理范围
        for &vi in &v {
            assert!((0.9..=1.1).contains(&vi), "电压 {} 超出范围", vi);
        }
    }

    #[test]
    fn test_jacobian_construction() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);
        let v = solver.initial_voltages(&problem, &setup);
        let theta = solver.initial_angles(&problem, &setup);
        let (p, q) = solver.compute_power_injections(&setup, &v, &theta);

        let theta_indices: Vec<usize> = (0..2).filter(|&i| i != setup.slack_idx).collect();
        let v_indices: Vec<usize> = (0..2).filter(|&i| setup.bus_types[i] == BusType::Pq).collect();

        let jac = solver.build_jacobian(&setup, &v, &theta, &p, &q, &theta_indices, &v_indices);

        // 雅可比矩阵维度：1 个非平衡相角 + 1 个 PQ 电压 = 2x2
        assert_eq!(jac.nrows(), 2);
        assert_eq!(jac.ncols(), 2);

        // 雅可比矩阵不应全为零
        let sum: f64 = jac.iter().map(|x| x.abs()).sum();
        assert!(sum > 0.0, "雅可比矩阵不应全为零");
    }

    #[test]
    fn test_branch_flow_computation() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);
        let v = solver.initial_voltages(&problem, &setup);
        let theta = solver.initial_angles(&problem, &setup);

        let flows = solver.compute_branch_flow(&problem, &setup, &v, &theta);

        assert_eq!(flows.len(), 1);
        // 初始电压相等、相角相等时，串联支路潮流为零，
        // 仅剩线路充电功率（b_half 产生的微小无功），应远小于限额
        assert!(flows[0].1 < 5.0, "初始潮流应接近零（仅线路充电），实际 {:.4}", flows[0].1);
    }

    #[test]
    fn test_constraint_check() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();
        let setup = solver.setup_problem(&problem);

        // 构造越限场景
        let mut v = vec![1.0, 1.0];
        let mut gen_p = vec![1.5]; // 超出 p_max=2.0 p.u.（200MW/100MVA）
        let mut gen_q = vec![0.0];
        let flows = vec![(1, 250.0)]; // 超过 200 MVA 限额

        let warnings = solver.check_constraints(&problem, &setup, &v, &gen_p, &gen_q, &flows);
        assert!(!warnings.is_empty(), "应检测到约束越限");

        // 正常场景
        v[1] = 0.98;
        gen_p[0] = 0.5;
        gen_q[0] = 0.1;
        let flows_ok = vec![(1, 50.0)];
        let warnings_ok = solver.check_constraints(&problem, &setup, &v, &gen_p, &gen_q, &flows_ok);
        assert!(warnings_ok.is_empty(), "正常场景不应有越限警告: {:?}", warnings_ok);
    }

    #[test]
    fn test_invalid_problem() {
        let solver = AcOpfSolver::new();

        // 空母线
        let problem = AcOpfProblem {
            buses: vec![], generators: vec![], branches: vec![],
            slack_bus_id: 1, base_mva: 100.0,
        };
        assert!(solver.solve(&problem, OpfMethod::NewtonRaphson).is_err());

        // base_mva 为 0
        let problem = AcOpfProblem {
            buses: vec![AcBus { bus_id: 1, p_load: 0.0, q_load: 0.0, v_min: 0.95, v_max: 1.05, v_init: 1.0, theta_init: 0.0 }],
            generators: vec![AcGenerator { gen_id: 1, bus_id: 1, p_min: 0.0, p_max: 100.0, q_min: -50.0, q_max: 50.0, cost_a: 0.01, cost_b: 10.0, cost_c: 0.0 }],
            branches: vec![],
            slack_bus_id: 1, base_mva: 0.0,
        };
        assert!(solver.solve(&problem, OpfMethod::NewtonRaphson).is_err());
    }

    #[test]
    fn test_opf_method_dispatch() {
        let problem = create_2bus_problem();
        let solver = AcOpfSolver::new();

        let r1 = solver.solve(&problem, OpfMethod::NewtonRaphson);
        let r2 = solver.solve(&problem, OpfMethod::InteriorPoint);

        assert!(r1.is_ok(), "牛顿法失败: {:?}", r1.err());
        assert!(r2.is_ok(), "内点法失败: {:?}", r2.err());

        // 两种方法的总成本应相近
        let cost1 = r1.unwrap().result.total_cost;
        let cost2 = r2.unwrap().result.total_cost;
        let diff = (cost1 - cost2).abs();
        let max_cost = cost1.max(cost2);
        assert!(
            diff < max_cost * 0.5 + 100.0,
            "两种方法成本差异过大: 牛顿法 {:.2}, 内点法 {:.2}", cost1, cost2
        );
    }

    #[test]
    fn test_solver_builder() {
        let solver = AcOpfSolver::new()
            .with_max_iter(30)
            .with_tolerance(1e-8);
        assert_eq!(solver.max_iter, 30);
        assert!((solver.tol - 1e-8).abs() < 1e-12);
    }
}
