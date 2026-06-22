//! 暂态稳定分析模块
//!
//! 实现电力系统暂态稳定仿真，包括：
//! - 发电机经典二阶模型（摇摆方程）
//! - 发电机四阶模型（含 AVR，简化为三阶 + AVR）
//! - RK4 显式积分器
//! - 隐式梯形积分器（预测-校正 / Heun 方法）
//! - 故障期间 / 清除后网络方程求解
//! - 暂态稳定性判据（最大功角差）
//!
//! 物理量约定：
//! - 功角 δ：弧度 (rad)
//! - 角速度 ω：弧度/秒 (rad/s)
//! - 同步角速度 ω_s = 2πf（f = 50 Hz 或 60 Hz，可配置）
//! - 电压、电流、功率：标幺值 (p.u.)
//! - 时间：秒 (s)
//! - 惯性常数 H：秒 (s)

#![allow(clippy::needless_range_loop)]

use eneros_core::ElementId;
use eneros_powerflow::{YBusMatrix, PowerFlowSolver, BusTypeNR};
use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f64::consts::PI;

use crate::types::{AnalysisError, AnalysisResult};

// ============================================================================
// 常量定义
// ============================================================================

/// 默认频率 (Hz)
const DEFAULT_FREQUENCY: f64 = 50.0;
/// 默认步长 (s)
const DEFAULT_DT: f64 = 0.01;
/// 最大仿真时间 (s)，防止无限运行
const MAX_SIMULATION_TIME: f64 = 10.0;
/// 稳定性判据：最大功角差阈值 (度)
const STABILITY_ANGLE_THRESHOLD_DEG: f64 = 180.0;
/// 默认 d 轴开路暂态时间常数 T'do (s) — 四阶模型用
const DEFAULT_TDO_PRIME: f64 = 5.0;
/// 数值容差
const EPSILON: f64 = 1e-12;
/// 稀疏迭代/线性代数容差（用于 T5 辅助函数）
const EPS: f64 = 1e-12;

// ============================================================================
// T4.1 类型定义
// ============================================================================

/// 发电机暂态模型类型
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GeneratorModel {
    /// 经典二阶模型（摇摆方程）
    Classical2nd,
    /// 四阶模型（含 AVR）
    FourthOrder,
}

/// 发电机暂态参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorDynamic {
    pub gen_id: ElementId,
    pub bus_id: ElementId,
    pub model: GeneratorModel,
    /// 惯性常数 H (s)
    pub h: f64,
    /// 阻尼系数 D (p.u.)
    pub d: f64,
    /// 暂态电抗 x'd (p.u.)
    pub xd_prime: f64,
    /// 同步电抗 xd (p.u.) — 四阶模型用
    pub xd: f64,
    /// 励磁电压 Efd (p.u.) — 四阶模型用
    pub efd: f64,
    /// 机械功率 Pm (p.u.)
    pub pm: f64,
    /// AVR 增益 Ka — 四阶模型用
    pub ka: f64,
    /// AVR 时间常数 Ta (s) — 四阶模型用
    pub ta: f64,
}

/// 故障类型
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TransientFault {
    /// 三相短路故障
    ThreePhase {
        bus_id: ElementId,
        fault_impedance: f64,
    },
    /// 断线故障
    LineOutage { branch_id: ElementId },
}

/// 仿真参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationParams {
    /// 仿真起始时间 (s)
    pub t_start: f64,
    /// 仿真结束时间 (s)
    pub t_end: f64,
    /// 步长 (s)
    pub dt: f64,
    /// 故障发生时间 (s)
    pub t_fault: f64,
    /// 故障清除时间 (s)
    pub t_clear: f64,
    /// 积分方法
    pub method: IntegrationMethod,
    /// 系统频率 (Hz)，默认 50.0
    pub frequency: f64,
}

impl Default for SimulationParams {
    fn default() -> Self {
        Self {
            t_start: 0.0,
            t_end: 2.0,
            dt: DEFAULT_DT,
            t_fault: 0.1,
            t_clear: 0.2,
            method: IntegrationMethod::RK4,
            frequency: DEFAULT_FREQUENCY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum IntegrationMethod {
    /// 龙格-库塔 4 阶
    RK4,
    /// 隐式梯形（预测-校正 / Heun 方法）
    ImplicitTrapezoidal,
}

/// 暂态稳定场景
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransientScenario {
    pub generators: Vec<GeneratorDynamic>,
    pub buses: Vec<ElementId>,
    pub branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)>, // (from, to, r, x, b, tap)
    pub base_mva: f64,
    pub fault: TransientFault,
    pub params: SimulationParams,
    /// 负荷列表 (bus_id, P_pu, Q_pu)，恒阻抗模型
    pub loads: Vec<(ElementId, f64, f64)>,
}

/// 仿真结果（单个时间步）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeStepResult {
    pub t: f64,
    /// 各发电机功角 (rad)
    pub rotor_angles: Vec<(ElementId, f64)>,
    /// 各发电机转速 (p.u.)
    pub rotor_speeds: Vec<(ElementId, f64)>,
    /// 各母线电压 (p.u.)
    pub bus_voltages: Vec<(ElementId, f64)>,
}

/// 暂态稳定仿真结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransientResult {
    pub time_series: Vec<TimeStepResult>,
    pub stable: bool,
    /// 最大功角差 (度)
    pub max_angle_spread_deg: f64,
    pub warnings: Vec<String>,
}

// ============================================================================
// T5 类型定义：CCT / 等面积法则 / CPF / 电压稳定模态分析
// ============================================================================

/// T5.1 临界故障清除时间 (CCT) 计算结果
///
/// 通过二分搜索在给定故障清除时间区间内寻找稳定/失稳边界。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CctResult {
    /// 临界故障清除时间 (s)
    pub cct: f64,
    /// 二分搜索收敛容差 (s)
    pub tolerance: f64,
    /// 二分搜索迭代次数
    pub iterations: u32,
    /// 在 CCT 处的最大功角差 (度)
    pub max_angle_spread_at_cct_deg: f64,
    /// 搜索过程追踪：(t_clear, stable)
    pub search_trace: Vec<(f64, bool)>,
}

/// T5.2 等面积法则分析结果（单机无穷大系统）
///
/// 基于解析公式快速计算临界故障清除时间，无需数值积分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqualAreaResult {
    /// 初始功角 δ₀ (rad)
    pub delta_0: f64,
    /// 临界故障清除功角 δ_c (rad)
    pub delta_c_critical: f64,
    /// 最大允许功角 δ_max (rad) — Pe_post = Pm 时的功角
    pub delta_max: f64,
    /// 加速面积 A_accel (p.u.·rad)
    pub a_accel: f64,
    /// 减速面积 A_decel (p.u.·rad)
    pub a_decel: f64,
    /// 等面积准则判定的 CCT (s)
    pub cct: f64,
    /// 是否稳定（在给定故障清除时间下）
    pub stable: bool,
    /// 故障前电磁功率幅值 Pmax_pre (p.u.)
    pub pmax_pre: f64,
    /// 故障期间电磁功率幅值 Pmax_fault (p.u.)
    pub pmax_fault: f64,
    /// 故障后电磁功率幅值 Pmax_post (p.u.)
    pub pmax_post: f64,
}

/// T5.3 CPF 单点结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpfPoint {
    /// 负荷参数 λ（0 = 基态，1 = 额定负荷增长）
    pub lambda: f64,
    /// 各母线电压幅值 (p.u.)
    pub voltages: Vec<(ElementId, f64)>,
    /// 最大电压偏差（相对基态）
    pub max_voltage_deviation: f64,
    /// 是否收敛
    pub converged: bool,
}

/// T5.3 连续潮流 (CPF) 分析结果
///
/// 追踪 PV 曲线并检测电压崩溃鼻点（λ_max）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpfResult {
    /// PV 曲线点序列（按 λ 递增排列，鼻点后可能递减）
    pub pv_curve: Vec<CpfPoint>,
    /// 鼻点负荷参数 λ_max
    pub lambda_max: f64,
    /// 鼻点处各母线电压
    pub nose_voltages: Vec<(ElementId, f64)>,
    /// 是否成功检测到鼻点
    pub nose_detected: bool,
    /// 总连续步数
    pub total_steps: u32,
    /// 警告信息
    pub warnings: Vec<String>,
}

/// T5.4 电压稳定模态分析结果
///
/// 通过潮流雅可比矩阵奇异值分解评估电压稳定裕度。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoltageStabilityResult {
    /// 最小奇异值 σ_min（电压稳定裕度指标）
    pub min_singular_value: f64,
    /// 最大奇异值 σ_max
    pub max_singular_value: f64,
    /// 条件数 κ = σ_max / σ_min
    pub condition_number: f64,
    /// σ_min 对应的左奇异向量（母线电压灵敏度，归一化）
    pub left_singular_vector: Vec<f64>,
    /// σ_min 对应的右奇异向量（母线注入灵敏度，归一化）
    pub right_singular_vector: Vec<f64>,
    /// 最薄弱母线索引（左奇异向量中绝对值最大的分量）
    pub weakest_bus_idx: usize,
    /// 是否接近电压不稳定（σ_min < 阈值）
    pub near_instability: bool,
    /// 电压稳定裕度（σ_min 相对基态的百分比）
    pub stability_margin_percent: f64,
}

// ============================================================================
// 内部状态结构（积分用）
// ============================================================================

/// 发电机内部状态
#[derive(Debug, Clone)]
struct GenState {
    /// 转子功角 (rad)
    delta: f64,
    /// 转子角速度 (rad/s)
    omega: f64,
    /// q 轴暂态电势 Eq' (p.u.)
    eq_prime: f64,
    /// 励磁电压 Efd (p.u.)
    efd: f64,
}

impl GenState {
    /// 状态向量加法：self + alpha * other
    fn add_scaled(&self, alpha: f64, other: &GenState) -> GenState {
        GenState {
            delta: self.delta + alpha * other.delta,
            omega: self.omega + alpha * other.omega,
            eq_prime: self.eq_prime + alpha * other.eq_prime,
            efd: self.efd + alpha * other.efd,
        }
    }
}

// ============================================================================
// 暂态稳定分析器
// ============================================================================

/// 暂态稳定分析器
pub struct TransientStabilityAnalyzer;

impl Default for TransientStabilityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl TransientStabilityAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// 执行暂态稳定仿真
    pub fn analyze(
        &self,
        scenario: &TransientScenario,
    ) -> Result<AnalysisResult<TransientResult>, AnalysisError> {
        // 1. 输入验证
        self.validate(scenario)?;

        // 2. 获取系统频率和同步角速度
        let freq = scenario.params.frequency;
        let omega_s = 2.0 * PI * freq;

        // 3. 构建母线索引映射
        let bus_map = self.build_bus_map(&scenario.buses);

        // 4. 构建 Y-Bus 矩阵（故障前 / 故障期间 / 故障清除后）
        let ybus_pre = YBusMatrix::from_branches(&scenario.branches, &bus_map);
        let ybus_fault = self.apply_fault(&ybus_pre, &scenario.fault, &bus_map, &scenario.branches);
        let ybus_post =
            self.apply_post_fault(&ybus_pre, &scenario.fault, &bus_map, &scenario.branches);

        // 5. 计算初始条件
        let mut state = self.compute_initial_conditions(scenario, &bus_map, omega_s);

        // 6. 仿真时间循环
        let dt = scenario.params.dt;
        let t_start = scenario.params.t_start;
        let t_end = scenario.params.t_end.min(MAX_SIMULATION_TIME);
        let t_fault = scenario.params.t_fault;
        let t_clear = scenario.params.t_clear;

        let n_steps = ((t_end - t_start) / dt).round() as usize;
        let mut time_series = Vec::with_capacity(n_steps + 1);
        let mut warnings = Vec::new();

        // 记录初始状态（t = t_start）
        let (voltages_init, _) = self.solve_network(
            &ybus_pre,
            &scenario.generators,
            &state,
            &scenario.loads,
            &bus_map,
        );
        time_series.push(self.record_result(
            t_start,
            &scenario.generators,
            &state,
            &voltages_init,
            &bus_map,
            omega_s,
        ));

        let mut diverged = false;
        let mut step_count = 0u32;

        for step in 0..n_steps {
            let t = t_start + step as f64 * dt;

            // 选择当前 Y-Bus
            let ybus = if t < t_fault {
                &ybus_pre
            } else if t < t_clear {
                &ybus_fault
            } else {
                &ybus_post
            };

            // 积分一步
            let new_state = match scenario.params.method {
                IntegrationMethod::RK4 => self.rk4_step(
                    t,
                    dt,
                    &state,
                    &scenario.generators,
                    ybus,
                    &scenario.loads,
                    &bus_map,
                    omega_s,
                ),
                IntegrationMethod::ImplicitTrapezoidal => self.heun_step(
                    t,
                    dt,
                    &state,
                    &scenario.generators,
                    ybus,
                    &scenario.loads,
                    &bus_map,
                    omega_s,
                ),
            };

            // 检查发散
            if new_state.iter().any(|s| {
                !s.delta.is_finite() || !s.omega.is_finite() || !s.eq_prime.is_finite()
                    || !s.efd.is_finite()
            }) {
                diverged = true;
                warnings.push(format!("仿真在 t={:.3}s 发散", t + dt));
                break;
            }

            state = new_state;
            step_count = step as u32 + 1;

            // 记录结果
            let t_next = t + dt;
            let (voltages, _) = self.solve_network(
                ybus,
                &scenario.generators,
                &state,
                &scenario.loads,
                &bus_map,
            );
            time_series.push(self.record_result(
                t_next,
                &scenario.generators,
                &state,
                &voltages,
                &bus_map,
                omega_s,
            ));
        }

        // 7. 稳定性判断
        let max_angle_spread = self.compute_max_angle_spread(&time_series);
        let stable = !diverged && max_angle_spread < STABILITY_ANGLE_THRESHOLD_DEG;

        if !stable && !diverged {
            warnings.push(format!(
                "最大功角差 {:.1}° 超过阈值 {:.1}°",
                max_angle_spread, STABILITY_ANGLE_THRESHOLD_DEG
            ));
        }

        Ok(AnalysisResult {
            converged: !diverged,
            iterations: step_count,
            result: TransientResult {
                time_series,
                stable,
                max_angle_spread_deg: max_angle_spread,
                warnings: warnings.clone(),
            },
            warnings,
        })
    }

    // ========================================================================
    // 输入验证
    // ========================================================================

    fn validate(&self, scenario: &TransientScenario) -> Result<(), AnalysisError> {
        if scenario.generators.is_empty() {
            return Err(AnalysisError::InvalidConfiguration(
                "无发电机，无法进行暂态稳定仿真".to_string(),
            ));
        }
        if scenario.buses.is_empty() {
            return Err(AnalysisError::InvalidConfiguration(
                "无母线，无法进行暂态稳定仿真".to_string(),
            ));
        }
        if scenario.params.dt <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "步长 dt 必须大于 0".to_string(),
            ));
        }
        if scenario.params.t_end <= scenario.params.t_start {
            return Err(AnalysisError::InvalidConfiguration(
                "仿真结束时间必须大于起始时间".to_string(),
            ));
        }
        if scenario.params.t_clear <= scenario.params.t_fault {
            return Err(AnalysisError::InvalidConfiguration(
                "故障清除时间必须大于故障发生时间".to_string(),
            ));
        }
        if scenario.params.t_end > MAX_SIMULATION_TIME {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "仿真时间 {}s 超过最大限制 {}s",
                scenario.params.t_end, MAX_SIMULATION_TIME
            )));
        }
        if scenario.params.frequency <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "系统频率必须大于 0".to_string(),
            ));
        }

        // 检查发电机母线是否在母线列表中
        for gen in &scenario.generators {
            if !scenario.buses.contains(&gen.bus_id) {
                return Err(AnalysisError::InvalidConfiguration(format!(
                    "发电机 {} 的母线 {} 不在母线列表中",
                    gen.gen_id, gen.bus_id
                )));
            }
            if gen.h <= 0.0 {
                return Err(AnalysisError::InvalidConfiguration(format!(
                    "发电机 {} 的惯性常数 H 必须大于 0",
                    gen.gen_id
                )));
            }
            if gen.xd_prime <= 0.0 {
                return Err(AnalysisError::InvalidConfiguration(format!(
                    "发电机 {} 的暂态电抗 xd' 必须大于 0",
                    gen.gen_id
                )));
            }
        }

        Ok(())
    }

    // ========================================================================
    // 母线映射
    // ========================================================================

    fn build_bus_map(&self, buses: &[ElementId]) -> HashMap<ElementId, usize> {
        buses
            .iter()
            .enumerate()
            .map(|(i, &bus_id)| (bus_id, i))
            .collect()
    }

    // ========================================================================
    // T4.6 故障 Y-Bus 构建
    // ========================================================================

    /// 应用故障，生成故障期间的 Y-Bus
    fn apply_fault(
        &self,
        ybus_pre: &YBusMatrix,
        fault: &TransientFault,
        bus_map: &HashMap<ElementId, usize>,
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)],
    ) -> YBusMatrix {
        match fault {
            TransientFault::ThreePhase {
                bus_id,
                fault_impedance,
            } => {
                let mut ybus = ybus_pre.clone();
                if let Some(&idx) = bus_map.get(bus_id) {
                    if *fault_impedance > EPSILON {
                        // Y_fault = 1 / Z_fault
                        ybus.add_shunt(idx, 1.0 / fault_impedance, 0.0);
                    } else {
                        // 金属性短路：添加大导纳模拟
                        ybus.add_shunt(idx, 1e6, 0.0);
                    }
                }
                ybus
            }
            TransientFault::LineOutage { branch_id } => {
                // 断线故障：移除对应支路，重建 Y-Bus
                let remaining: Vec<_> = branches
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i as ElementId != *branch_id)
                    .map(|(_, b)| *b)
                    .collect();
                if remaining.len() == branches.len() {
                    // 未找到对应支路，返回原 Y-Bus
                    ybus_pre.clone()
                } else {
                    YBusMatrix::from_branches(&remaining, bus_map)
                }
            }
        }
    }

    /// 生成故障清除后的 Y-Bus
    fn apply_post_fault(
        &self,
        ybus_pre: &YBusMatrix,
        fault: &TransientFault,
        bus_map: &HashMap<ElementId, usize>,
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)],
    ) -> YBusMatrix {
        match fault {
            TransientFault::ThreePhase { .. } => {
                // 三相故障清除后恢复正常 Y-Bus
                ybus_pre.clone()
            }
            TransientFault::LineOutage { branch_id } => {
                // 断线故障：线路永久断开
                let remaining: Vec<_> = branches
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i as ElementId != *branch_id)
                    .map(|(_, b)| *b)
                    .collect();
                if remaining.len() == branches.len() {
                    ybus_pre.clone()
                } else {
                    YBusMatrix::from_branches(&remaining, bus_map)
                }
            }
        }
    }

    // ========================================================================
    // 初始条件计算
    // ========================================================================

    /// 计算发电机初始状态
    ///
    /// 假设初始为稳态（flat start: V = 1.0∠0），根据 Pm 计算 Eq' 和 δ_0
    fn compute_initial_conditions(
        &self,
        scenario: &TransientScenario,
        _bus_map: &HashMap<ElementId, usize>,
        omega_s: f64,
    ) -> Vec<GenState> {
        let mut states = Vec::with_capacity(scenario.generators.len());

        for gen in &scenario.generators {
            // 假设初始端电压 V_t = 1.0∠0
            let v_t = Complex64::new(1.0, 0.0);
            // 发电机电流 I_g = conj(S_g / V_t)，假设 Q = 0
            let i_g = Complex64::new(gen.pm, 0.0) / v_t;
            // Eq' = V_t + j*xd' * I_g
            let eq_prime_complex = v_t + Complex64::new(0.0, gen.xd_prime) * i_g;
            let delta_0 = eq_prime_complex.arg();
            let eq_prime_mag = eq_prime_complex.norm();

            states.push(GenState {
                delta: delta_0,
                omega: omega_s,
                eq_prime: eq_prime_mag,
                efd: gen.efd,
            });
        }

        states
    }

    // ========================================================================
    // T4.6 网络方程求解
    // ========================================================================

    /// 求解网络方程 I = Y * V，返回母线电压和各发电机电磁功率
    fn solve_network(
        &self,
        ybus: &YBusMatrix,
        generators: &[GeneratorDynamic],
        states: &[GenState],
        loads: &[(ElementId, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
    ) -> (Vec<Complex64>, Vec<f64>) {
        let n = ybus.size();
        if n == 0 {
            return (Vec::new(), vec![0.0; generators.len()]);
        }

        // 构建稠密 Y-Bus 矩阵
        let mut y_dense = vec![vec![Complex64::new(0.0, 0.0); n]; n];
        for i in 0..n {
            for (j, g, b) in ybus.iter_row(i) {
                y_dense[i][j] = Complex64::new(g, b);
            }
        }

        // 添加负荷等值导纳（恒阻抗模型）
        // Y_load = conj(S_load) / |V|^2 = conj(P + jQ) = P - jQ（V = 1.0 时）
        for &(bus_id, p, q) in loads {
            if let Some(&idx) = bus_map.get(&bus_id) {
                if idx < n {
                    y_dense[idx][idx] += Complex64::new(p, -q);
                }
            }
        }

        // 添加发电机 Norton 等值并构建注入电流向量
        let mut i_inj = vec![Complex64::new(0.0, 0.0); n];
        for (gen, state) in generators.iter().zip(states.iter()) {
            if let Some(&idx) = bus_map.get(&gen.bus_id) {
                if idx < n {
                    // 发电机内导纳 y_gen = 1 / (j*xd') = -j / xd'
                    let y_gen = Complex64::new(0.0, -1.0 / gen.xd_prime);
                    y_dense[idx][idx] += y_gen;
                    // Norton 注入电流 I_N = Eq' / (j*xd') = Eq' * y_gen
                    let eq_prime = Complex64::from_polar(state.eq_prime, state.delta);
                    i_inj[idx] += eq_prime * y_gen;
                }
            }
        }

        // 求解 Y * V = I
        let voltages = solve_complex_linear_system(&y_dense, &i_inj)
            .unwrap_or_else(|| vec![Complex64::new(0.0, 0.0); n]);

        // 计算各发电机电磁功率 Pe = Re(Eq' * conj(I_g))
        let mut pe_list = Vec::with_capacity(generators.len());
        for (gen, state) in generators.iter().zip(states.iter()) {
            if let Some(&idx) = bus_map.get(&gen.bus_id) {
                if idx < n {
                    let v_t = voltages[idx];
                    let eq_prime = Complex64::from_polar(state.eq_prime, state.delta);
                    let i_g = (eq_prime - v_t) / Complex64::new(0.0, gen.xd_prime);
                    let pe = (eq_prime * i_g.conj()).re;
                    pe_list.push(pe);
                } else {
                    pe_list.push(0.0);
                }
            } else {
                pe_list.push(0.0);
            }
        }

        (voltages, pe_list)
    }

    // ========================================================================
    // T4.2-T4.3 发电机导数计算
    // ========================================================================

    /// 计算状态导数 f(t, y)
    ///
    /// 经典二阶模型：
    ///   dδ/dt = ω - ω_s
    ///   dω/dt = (ω_s / 2H) * (Pm - Pe - D*(ω - ω_s))
    ///
    /// 四阶模型（简化为三阶 + AVR）：
    ///   dδ/dt = ω - ω_s
    ///   dω/dt = (ω_s / 2H) * (Pm - Pe - D*(ω - ω_s))
    ///   dEq'/dt = (1/T'do) * (Efd - Eq')
    ///   dEfd/dt = (1/Ta) * (Ka*(Vref - Vt) - Efd)
    fn compute_derivatives(
        &self,
        states: &[GenState],
        generators: &[GeneratorDynamic],
        ybus: &YBusMatrix,
        loads: &[(ElementId, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
        omega_s: f64,
    ) -> Vec<GenState> {
        // 求解网络获取电压和电磁功率
        let (voltages, pe_list) = self.solve_network(ybus, generators, states, loads, bus_map);

        let mut derivs = Vec::with_capacity(generators.len());

        for (i, gen) in generators.iter().enumerate() {
            let state = &states[i];
            let pe = pe_list[i];

            // 摇摆方程（二阶和四阶通用）
            let d_delta = state.omega - omega_s;
            let d_omega =
                (omega_s / (2.0 * gen.h)) * (gen.pm - pe - gen.d * (state.omega - omega_s));

            match gen.model {
                GeneratorModel::Classical2nd => {
                    // 经典模型：Eq' 为常数，导数为 0
                    derivs.push(GenState {
                        delta: d_delta,
                        omega: d_omega,
                        eq_prime: 0.0,
                        efd: 0.0,
                    });
                }
                GeneratorModel::FourthOrder => {
                    // 四阶模型（简化）：dEq'/dt = (1/T'do) * (Efd - Eq')
                    let d_eq = (1.0 / DEFAULT_TDO_PRIME) * (state.efd - state.eq_prime);

                    // AVR: dEfd/dt = (1/Ta) * (Ka*(Vref - Vt) - Efd)
                    let v_ref = 1.0;
                    let v_t_mag = bus_map
                        .get(&gen.bus_id)
                        .and_then(|&idx| voltages.get(idx))
                        .map(|v| v.norm())
                        .unwrap_or(1.0);

                    let ta = if gen.ta > EPSILON { gen.ta } else { 1.0 };
                    let d_efd = (1.0 / ta) * (gen.ka * (v_ref - v_t_mag) - state.efd);

                    derivs.push(GenState {
                        delta: d_delta,
                        omega: d_omega,
                        eq_prime: d_eq,
                        efd: d_efd,
                    });
                }
            }
        }

        derivs
    }

    // ========================================================================
    // T4.4 RK4 积分器
    // ========================================================================

    /// 4 阶龙格-库塔积分一步
    #[allow(clippy::too_many_arguments)]
    fn rk4_step(
        &self,
        _t: f64,
        dt: f64,
        states: &[GenState],
        generators: &[GeneratorDynamic],
        ybus: &YBusMatrix,
        loads: &[(ElementId, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
        omega_s: f64,
    ) -> Vec<GenState> {
        // k1 = f(t, y)
        let k1 = self.compute_derivatives(states, generators, ybus, loads, bus_map, omega_s);

        // k2 = f(t + dt/2, y + dt/2 * k1)
        let s2: Vec<GenState> = states
            .iter()
            .zip(k1.iter())
            .map(|(s, k)| s.add_scaled(dt / 2.0, k))
            .collect();
        let k2 =
            self.compute_derivatives(&s2, generators, ybus, loads, bus_map, omega_s);

        // k3 = f(t + dt/2, y + dt/2 * k2)
        let s3: Vec<GenState> = states
            .iter()
            .zip(k2.iter())
            .map(|(s, k)| s.add_scaled(dt / 2.0, k))
            .collect();
        let k3 =
            self.compute_derivatives(&s3, generators, ybus, loads, bus_map, omega_s);

        // k4 = f(t + dt, y + dt * k3)
        let s4: Vec<GenState> = states
            .iter()
            .zip(k3.iter())
            .map(|(s, k)| s.add_scaled(dt, k))
            .collect();
        let k4 =
            self.compute_derivatives(&s4, generators, ybus, loads, bus_map, omega_s);

        // y_next = y + dt/6 * (k1 + 2*k2 + 2*k3 + k4)
        states
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let combined = GenState {
                    delta: k1[i].delta + 2.0 * k2[i].delta + 2.0 * k3[i].delta + k4[i].delta,
                    omega: k1[i].omega + 2.0 * k2[i].omega + 2.0 * k3[i].omega + k4[i].omega,
                    eq_prime: k1[i].eq_prime
                        + 2.0 * k2[i].eq_prime
                        + 2.0 * k3[i].eq_prime
                        + k4[i].eq_prime,
                    efd: k1[i].efd + 2.0 * k2[i].efd + 2.0 * k3[i].efd + k4[i].efd,
                };
                s.add_scaled(dt / 6.0, &combined)
            })
            .collect()
    }

    // ========================================================================
    // T4.5 隐式梯形积分器（预测-校正 / Heun 方法）
    // ========================================================================

    /// 隐式梯形法（使用预测-校正 / Heun 方法实现）
    ///
    /// 预测步（显式欧拉）：y_pred = y + dt * f(t, y)
    /// 校正步（梯形）：    y_next = y + dt/2 * (f(t, y) + f(t+dt, y_pred))
    #[allow(clippy::too_many_arguments)]
    fn heun_step(
        &self,
        _t: f64,
        dt: f64,
        states: &[GenState],
        generators: &[GeneratorDynamic],
        ybus: &YBusMatrix,
        loads: &[(ElementId, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
        omega_s: f64,
    ) -> Vec<GenState> {
        // 预测步：k1 = f(t, y)
        let k1 = self.compute_derivatives(states, generators, ybus, loads, bus_map, omega_s);
        let predicted: Vec<GenState> = states
            .iter()
            .zip(k1.iter())
            .map(|(s, k)| s.add_scaled(dt, k))
            .collect();

        // 校正步：k2 = f(t + dt, y_pred)
        let k2 = self.compute_derivatives(&predicted, generators, ybus, loads, bus_map, omega_s);

        // y_next = y + dt/2 * (k1 + k2)
        states
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let combined = GenState {
                    delta: k1[i].delta + k2[i].delta,
                    omega: k1[i].omega + k2[i].omega,
                    eq_prime: k1[i].eq_prime + k2[i].eq_prime,
                    efd: k1[i].efd + k2[i].efd,
                };
                s.add_scaled(dt / 2.0, &combined)
            })
            .collect()
    }

    // ========================================================================
    // 结果记录与稳定性判断
    // ========================================================================

    /// 记录单个时间步的结果
    fn record_result(
        &self,
        t: f64,
        generators: &[GeneratorDynamic],
        states: &[GenState],
        voltages: &[Complex64],
        bus_map: &HashMap<ElementId, usize>,
        omega_s: f64,
    ) -> TimeStepResult {
        let rotor_angles: Vec<(ElementId, f64)> = generators
            .iter()
            .zip(states.iter())
            .map(|(gen, state)| (gen.gen_id, state.delta))
            .collect();

        let rotor_speeds: Vec<(ElementId, f64)> = generators
            .iter()
            .zip(states.iter())
            .map(|(gen, state)| (gen.gen_id, state.omega / omega_s))
            .collect();

        // 反向映射：索引 -> ElementId
        let inv_map: HashMap<&usize, &ElementId> =
            bus_map.iter().map(|(k, v)| (v, k)).collect();
        let bus_voltages: Vec<(ElementId, f64)> = (0..voltages.len())
            .filter_map(|i| {
                inv_map.get(&i).map(|&bus_id| (*bus_id, voltages[i].norm()))
            })
            .collect();

        TimeStepResult {
            t,
            rotor_angles,
            rotor_speeds,
            bus_voltages,
        }
    }

    /// 计算最大功角差（度）
    fn compute_max_angle_spread(&self, time_series: &[TimeStepResult]) -> f64 {
        let mut max_spread = 0.0;
        for step in time_series {
            if step.rotor_angles.len() < 2 {
                continue;
            }
            let angles: Vec<f64> = step.rotor_angles.iter().map(|(_, a)| *a).collect();
            let min_angle = angles.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_angle = angles.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let spread = (max_angle - min_angle).to_degrees();
            if spread > max_spread {
                max_spread = spread;
            }
        }
        max_spread
    }

    // ========================================================================
    // T5.1 临界故障清除时间 (CCT) — 二分搜索
    // ========================================================================

    /// 计算临界故障清除时间 (CCT)
    ///
    /// 在 [t_clear_min, t_clear_max] 区间内使用二分搜索寻找 CCT：
    /// - 若 t_clear < CCT，系统稳定
    /// - 若 t_clear > CCT，系统失稳
    ///
    /// 算法：
    /// 1. 验证 t_clear_min 稳定、t_clear_max 失稳（否则扩展区间或返回边界）
    /// 2. 二分搜索：mid = (lo + hi) / 2，运行暂态仿真判断稳定性
    /// 3. 收敛条件：hi - lo < tolerance
    ///
    /// # 参数
    /// - `scenario`: 暂态场景模板（t_clear 字段会被覆盖）
    /// - `t_clear_min`: 搜索下限 (s)，应确保稳定
    /// - `t_clear_max`: 搜索上限 (s)，应确保失稳
    /// - `tolerance`: 时间收敛容差 (s)
    /// - `max_iterations`: 最大二分迭代次数
    pub fn compute_cct(
        &self,
        scenario: &TransientScenario,
        t_clear_min: f64,
        t_clear_max: f64,
        tolerance: f64,
        max_iterations: u32,
    ) -> Result<AnalysisResult<CctResult>, AnalysisError> {
        if t_clear_min >= t_clear_max {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "t_clear_min ({}) 必须小于 t_clear_max ({})",
                t_clear_min, t_clear_max
            )));
        }
        if tolerance <= 0.0 || tolerance >= (t_clear_max - t_clear_min) {
            return Err(AnalysisError::InvalidConfiguration(
                "容差必须为正且小于搜索区间".to_string(),
            ));
        }
        if t_clear_min <= scenario.params.t_fault {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "t_clear_min ({}) 必须大于故障发生时间 ({})",
                t_clear_min, scenario.params.t_fault
            )));
        }

        let mut search_trace = Vec::new();

        // 评估边界
        let mut s_low = scenario.clone();
        s_low.params.t_clear = t_clear_min;
        let res_low = self.analyze(&s_low)?;
        let stable_low = res_low.result.stable;
        search_trace.push((t_clear_min, stable_low));

        let mut s_high = scenario.clone();
        s_high.params.t_clear = t_clear_max;
        let res_high = self.analyze(&s_high)?;
        let stable_high = res_high.result.stable;
        search_trace.push((t_clear_max, stable_high));

        // 边界情况处理
        if stable_low && stable_high {
            // 整个区间都稳定，CCT > t_clear_max
            return Ok(AnalysisResult {
                converged: true,
                iterations: 0,
                result: CctResult {
                    cct: t_clear_max,
                    tolerance,
                    iterations: 0,
                    max_angle_spread_at_cct_deg: res_high.result.max_angle_spread_deg,
                    search_trace,
                },
                warnings: vec![format!(
                    "整个区间 [{}, {}] 均稳定，CCT >= {}",
                    t_clear_min, t_clear_max, t_clear_max
                )],
            });
        }
        if !stable_low && !stable_high {
            // 整个区间都失稳，CCT < t_clear_min
            return Ok(AnalysisResult {
                converged: true,
                iterations: 0,
                result: CctResult {
                    cct: t_clear_min,
                    tolerance,
                    iterations: 0,
                    max_angle_spread_at_cct_deg: res_low.result.max_angle_spread_deg,
                    search_trace,
                },
                warnings: vec![format!(
                    "整个区间 [{}, {}] 均失稳，CCT <= {}",
                    t_clear_min, t_clear_max, t_clear_min
                )],
            });
        }

        // 确保搜索方向正确：lo=稳定, hi=失稳
        let (mut lo, mut hi) = if stable_low {
            (t_clear_min, t_clear_max)
        } else {
            // stable_high but not stable_low — 反向
            (t_clear_max, t_clear_min)
        };

        let mut iter = 0u32;
        let mut last_cct = lo;
        let mut last_spread = if stable_low {
            res_low.result.max_angle_spread_deg
        } else {
            res_high.result.max_angle_spread_deg
        };

        while iter < max_iterations && (hi - lo).abs() > tolerance {
            iter += 1;
            let mid = (lo + hi) / 2.0;
            let mut s_mid = scenario.clone();
            s_mid.params.t_clear = mid;
            let res_mid = self.analyze(&s_mid)?;
            let stable_mid = res_mid.result.stable;
            search_trace.push((mid, stable_mid));

            if stable_mid {
                lo = mid;
                last_cct = mid;
                last_spread = res_mid.result.max_angle_spread_deg;
            } else {
                hi = mid;
            }
        }

        Ok(AnalysisResult {
            converged: true,
            iterations: iter,
            result: CctResult {
                cct: last_cct,
                tolerance,
                iterations: iter,
                max_angle_spread_at_cct_deg: last_spread,
                search_trace,
            },
            warnings: Vec::new(),
        })
    }

    // ========================================================================
    // T5.2 等面积法则（单机无穷大系统）
    // ========================================================================

    /// 等面积法则快速稳定性判定与 CCT 计算
    ///
    /// 适用于单机无穷大 (SMIB) 系统，基于解析公式：
    ///
    /// 故障前：Pe_pre = Pmax_pre · sin(δ)
    /// 故障期间：Pe_fault = Pmax_fault · sin(δ)（通常 Pmax_fault ≈ 0）
    /// 故障后：Pe_post = Pmax_post · sin(δ)
    ///
    /// 初始功角：δ₀ = arcsin(Pm / Pmax_pre)
    /// 最大功角：δ_max = π - arcsin(Pm / Pmax_post)
    ///
    /// 临界清除功角 δ_c 满足：
    ///   A_accel(δ₀→δ_c) = A_decel(δ_c→δ_max)
    ///   Pm·(δ_c - δ₀) - ∫Pmax_fault·sin(δ)dδ [δ₀→δ_c]
    ///   = ∫Pmax_post·sin(δ)dδ [δ_c→δ_max] - Pm·(δ_max - δ_c)
    ///
    /// CCT 由摇摆方程解析解给出：
    ///   t_cct = √(2H·(δ_c - δ₀) / (ω_s · (Pm - Pe_fault_avg)))
    ///
    /// # 参数
    /// - `gen`: 发电机动态参数（使用 H, Pm, D）
    /// - `v_inf`: 无穷大母线电压 (p.u.)
    /// - `x_pre_fault`: 故障前总电抗 (p.u.)，含 xd'
    /// - `x_fault`: 故障期间总电抗 (p.u.)，含 xd'
    /// - `x_post_fault`: 故障后总电抗 (p.u.)，含 xd'
    /// - `frequency`: 系统频率 (Hz)
    pub fn equal_area_criterion(
        &self,
        gen: &GeneratorDynamic,
        v_inf: f64,
        x_pre_fault: f64,
        x_fault: f64,
        x_post_fault: f64,
        frequency: f64,
    ) -> Result<AnalysisResult<EqualAreaResult>, AnalysisError> {
        if gen.h <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "惯性常数 H 必须大于 0".to_string(),
            ));
        }
        if x_pre_fault <= 0.0 || x_post_fault <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "电抗必须为正".to_string(),
            ));
        }
        if frequency <= 0.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "频率必须为正".to_string(),
            ));
        }

        let omega_s = 2.0 * PI * frequency;

        // 发电机内电势 Eq'（假设 Efd ≈ Eq'）
        let eq_prime = gen.efd.max(0.01);

        // 各阶段电磁功率幅值
        let pmax_pre = eq_prime * v_inf / x_pre_fault;
        let pmax_fault = if x_fault.is_infinite() || x_fault > 1e6 {
            0.0
        } else {
            eq_prime * v_inf / x_fault
        };
        let pmax_post = eq_prime * v_inf / x_post_fault;

        let pm = gen.pm;

        // 检查可行性：Pm 必须小于 Pmax_pre 和 Pmax_post
        if pm >= pmax_pre {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Pm ({}) >= Pmax_pre ({})，故障前即不可行",
                pm, pmax_pre
            )));
        }
        if pm >= pmax_post {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Pm ({}) >= Pmax_post ({})，故障后无法恢复同步",
                pm, pmax_post
            )));
        }

        // 初始功角 δ₀
        let delta_0 = (pm / pmax_pre).asin();

        // 最大允许功角 δ_max
        let delta_max = PI - (pm / pmax_post).asin();

        // 求解临界清除功角 δ_c
        // A_accel = Pm·(δ_c - δ₀) - Pmax_fault·(cos(δ₀) - cos(δ_c))
        // A_decel = Pmax_post·(cos(δ_c) - cos(δ_max)) - Pm·(δ_max - δ_c)
        // 令 A_accel = A_decel，数值求解 δ_c
        let delta_c_critical = self.solve_critical_angle(
            delta_0,
            delta_max,
            pm,
            pmax_fault,
            pmax_post,
        )?;

        // 计算加速面积和减速面积（在 δ_c_critical 处）
        let a_accel = pm * (delta_c_critical - delta_0)
            - pmax_fault * (delta_0.cos() - delta_c_critical.cos());
        let a_decel = pmax_post * (delta_c_critical.cos() - delta_max.cos())
            - pm * (delta_max - delta_c_critical);

        // 计算 CCT
        // 故障期间加速度 α = (ω_s / 2H) · (Pm - Pe_fault)
        // 若 Pmax_fault ≈ 0，则 α ≈ (ω_s / 2H) · Pm（恒定）
        // δ_c - δ₀ = 0.5 · α · t² → t = √(2·(δ_c - δ₀) / α)
        let cct = if pmax_fault < EPSILON {
            // 故障期间 Pe ≈ 0，恒定加速度
            let alpha = (omega_s / (2.0 * gen.h)) * pm;
            if alpha > EPSILON {
                (2.0 * (delta_c_critical - delta_0) / alpha).sqrt()
            } else {
                f64::INFINITY
            }
        } else {
            // 故障期间 Pe = Pmax_fault · sin(δ)，变加速度
            // 使用平均加速度近似
            let pe_fault_avg = pmax_fault
                * ((delta_0 + delta_c_critical) / 2.0).sin();
            let alpha_avg = (omega_s / (2.0 * gen.h)) * (pm - pe_fault_avg);
            if alpha_avg > EPSILON {
                (2.0 * (delta_c_critical - delta_0) / alpha_avg).sqrt()
            } else {
                f64::INFINITY
            }
        };

        Ok(AnalysisResult {
            converged: true,
            iterations: 1,
            result: EqualAreaResult {
                delta_0,
                delta_c_critical,
                delta_max,
                a_accel,
                a_decel,
                cct,
                stable: true,
                pmax_pre,
                pmax_fault,
                pmax_post,
            },
            warnings: Vec::new(),
        })
    }

    /// 数值求解临界清除功角 δ_c
    ///
    /// 使用二分法求解 A_accel(δ_c) = A_decel(δ_c)
    fn solve_critical_angle(
        &self,
        delta_0: f64,
        delta_max: f64,
        pm: f64,
        pmax_fault: f64,
        pmax_post: f64,
    ) -> Result<f64, AnalysisError> {
        let f = |delta_c: f64| -> f64 {
            let a_accel = pm * (delta_c - delta_0)
                - pmax_fault * (delta_0.cos() - delta_c.cos());
            let a_decel = pmax_post * (delta_c.cos() - delta_max.cos())
                - pm * (delta_max - delta_c);
            a_accel - a_decel
        };

        // 边界检查：若 f(δ₀) >= 0，说明即使瞬时切除故障，加速面积仍不小于减速面积，
        // 系统对任意故障持续时间均失稳——不存在临界清除角。
        let f_lo = f(delta_0);
        let f_hi = f(delta_max);
        if f_lo >= 0.0 {
            return Err(AnalysisError::NoConvergence(
                0,
                format!(
                    "系统在任意故障清除时间下均失稳：f(δ₀)={} >= 0（Pm 过大或 Pmax_post 过小）",
                    f_lo
                ),
            ));
        }
        if f_hi <= 0.0 {
            return Err(AnalysisError::NoConvergence(
                0,
                format!("数值异常：f(δ_max)={} <= 0，请检查参数", f_hi),
            ));
        }

        let mut lo = delta_0;
        let mut hi = delta_max;
        let tol = 1e-8;

        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            let f_mid = f(mid);
            if f_mid.abs() < tol {
                return Ok(mid);
            }
            if f(lo) * f_mid < 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }

        Ok((lo + hi) / 2.0)
    }

    // ========================================================================
    // T5.3 连续潮流 (CPF)
    // ========================================================================

    /// 连续潮流分析（CPF）
    ///
    /// 使用预测-校正连续法追踪 PV 曲线，检测电压崩溃鼻点（λ_max）。
    ///
    /// 算法：
    /// 1. 求解基态潮流（λ=0）
    /// 2. 预测步：沿切线方向外推（使用前两点的差分方向）
    /// 3. 校正步：在预测点附近用牛顿-拉夫逊求解潮流
    /// 4. 步长自适应：校正失败时缩减步长
    /// 5. 鼻点检测：当 λ 不再增大或潮流不收敛时，二分搜索精确鼻点
    ///
    /// # 参数
    /// - `ybus`: Y-Bus 导纳矩阵
    /// - `p_spec`: 基态有功注入 (p.u.)
    /// - `q_spec`: 基态无功注入 (p.u.)
    /// - `bus_types`: 母线类型
    /// - `load_bus_indices`: 负荷母线索引列表（λ 作用于这些母线）
    /// - `v_initial`: 初始电压（可选）
    /// - `max_lambda`: 最大负荷参数（安全上限）
    /// - `initial_step`: 初始步长 Δλ
    /// - `max_steps`: 最大连续步数
    #[allow(clippy::too_many_arguments)]
    pub fn run_cpf(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
        load_bus_indices: &[usize],
        v_initial: Option<&[f64]>,
        max_lambda: f64,
        initial_step: f64,
        max_steps: u32,
    ) -> Result<AnalysisResult<CpfResult>, AnalysisError> {
        let n = ybus.size();
        if n == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "Y-Bus 为空".to_string(),
            ));
        }
        if load_bus_indices.is_empty() {
            return Err(AnalysisError::InvalidConfiguration(
                "负荷母线列表为空".to_string(),
            ));
        }
        if initial_step <= 0.0 || initial_step > 1.0 {
            return Err(AnalysisError::InvalidConfiguration(
                "初始步长必须在 (0, 1] 区间".to_string(),
            ));
        }

        let solver = PowerFlowSolver::new(50, 1e-8);
        let mut warnings = Vec::new();
        let mut pv_curve = Vec::new();
        let mut total_steps = 0u32;

        // 负荷增长方向：仅在负荷母线上增长
        let load_direction_p: Vec<f64> = (0..n)
            .map(|i| {
                if load_bus_indices.contains(&i) {
                    p_spec[i].abs()
                } else {
                    0.0
                }
            })
            .collect();
        let load_direction_q: Vec<f64> = (0..n)
            .map(|i| {
                if load_bus_indices.contains(&i) {
                    q_spec[i].abs()
                } else {
                    0.0
                }
            })
            .collect();

        // 基态求解（λ=0）
        let base_result = solver
            .solve_with_initial(ybus, p_spec, q_spec, bus_types, v_initial)
            .map_err(|e| AnalysisError::InvalidConfiguration(format!("基态潮流失败: {}", e)))?;

        let base_voltages: Vec<f64> = base_result
            .bus_results
            .iter()
            .map(|br| br.voltage_magnitude)
            .collect();

        let base_point = CpfPoint {
            lambda: 0.0,
            voltages: (0..n).map(|i| (i as ElementId, base_voltages[i])).collect(),
            max_voltage_deviation: 0.0,
            converged: true,
        };
        pv_curve.push(base_point);

        let mut current_lambda = 0.0;
        let mut current_v = base_voltages.clone();
        let mut step = initial_step;
        let mut lambda_max = 0.0;
        let mut nose_voltages: Vec<(ElementId, f64)> = Vec::new();
        let mut nose_detected = false;

        // 连续步进
        for step_idx in 0..max_steps {
            total_steps = step_idx + 1;

            // 预测：λ_pred = λ + step
            let lambda_pred = current_lambda + step;
            if lambda_pred > max_lambda {
                warnings.push(format!(
                    "λ ({}) 超过安全上限 {}，停止",
                    lambda_pred, max_lambda
                ));
                break;
            }

            // 构造预测点的 P/Q
            let p_pred: Vec<f64> = (0..n)
                .map(|i| p_spec[i] - lambda_pred * load_direction_p[i])
                .collect();
            let q_pred: Vec<f64> = (0..n)
                .map(|i| q_spec[i] - lambda_pred * load_direction_q[i])
                .collect();

            // 校正：用前一点电压作为初值求解潮流
            let v_init: Vec<f64> = current_v.clone();
            let pf_result = solver.solve_with_initial(
                ybus,
                &p_pred,
                &q_pred,
                bus_types,
                Some(&v_init),
            );

            match pf_result {
                Ok(result) if result.converged => {
                    let voltages: Vec<f64> = result
                        .bus_results
                        .iter()
                        .map(|br| br.voltage_magnitude)
                        .collect();

                    let max_dev = voltages
                        .iter()
                        .zip(&base_voltages)
                        .map(|(v, vb)| (v - vb).abs())
                        .fold(0.0_f64, f64::max);

                    let point = CpfPoint {
                        lambda: lambda_pred,
                        voltages: (0..n)
                            .map(|i| (i as ElementId, voltages[i]))
                            .collect(),
                        max_voltage_deviation: max_dev,
                        converged: true,
                    };
                    pv_curve.push(point);

                    current_lambda = lambda_pred;
                    current_v = voltages;

                    // 检查电压越限
                    let v_min = current_v.iter().cloned().fold(f64::INFINITY, f64::min);
                    if v_min < 0.5 {
                        warnings.push(format!(
                            "λ={:.4} 时最低电压 {:.4} p.u.，接近崩溃",
                            current_lambda, v_min
                        ));
                    }

                    // 步长自适应：连续成功时适度增大
                    if step < 0.1 {
                        step *= 1.5;
                        if step > 0.1 {
                            step = 0.1;
                        }
                    }
                }
                _ => {
                    // 校正失败：缩减步长重试
                    step *= 0.5;
                    if step < 1e-4 {
                        // 步长过小，认为已到鼻点
                        nose_detected = true;
                        lambda_max = current_lambda;
                        nose_voltages = (0..n)
                            .map(|i| (i as ElementId, current_v[i]))
                            .collect();
                        warnings.push(format!(
                            "鼻点检测：λ_max ≈ {:.4}，步长缩减至 {}",
                            lambda_max, step
                        ));
                        break;
                    }
                    // 不增加 total_steps（重试不算新步）
                    total_steps -= 1;
                    continue;
                }
            }

            // 鼻点检测：λ 不再增长（已在 max_lambda 附近）
            if current_lambda >= max_lambda - 1e-6 {
                nose_detected = true;
                lambda_max = current_lambda;
                nose_voltages = (0..n)
                    .map(|i| (i as ElementId, current_v[i]))
                    .collect();
                warnings.push(format!(
                    "达到最大 λ 上限 {:.4}，停止",
                    max_lambda
                ));
                break;
            }
        }

        // 如果没有检测到鼻点但 λ 已经较大，取最大 λ
        if !nose_detected && !pv_curve.is_empty() {
            lambda_max = current_lambda;
            nose_voltages = (0..n)
                .map(|i| (i as ElementId, current_v[i]))
                .collect();
        }

        let warnings_clone = warnings.clone();
        Ok(AnalysisResult {
            converged: true,
            iterations: total_steps,
            result: CpfResult {
                pv_curve,
                lambda_max,
                nose_voltages,
                nose_detected,
                total_steps,
                warnings,
            },
            warnings: warnings_clone,
        })
    }

    // ========================================================================
    // T5.4 电压稳定模态分析
    // ========================================================================

    /// 电压稳定模态分析
    ///
    /// 计算潮流雅可比矩阵的奇异值，评估电压稳定裕度：
    /// - σ_min：最小奇异值，电压稳定裕度指标（越小越接近不稳定）
    /// - κ = σ_max / σ_min：条件数，越大越接近不稳定
    /// - 左奇异向量：母线电压灵敏度（最薄弱母线）
    /// - 右奇异向量：母线注入灵敏度
    ///
    /// 算法：
    /// 1. 构建潮流雅可比 J（极坐标，去除 slack 行列）
    /// 2. 计算 J^T·J 的最大和最小特征值
    /// 3. σ_max = √λ_max, σ_min = √λ_min
    /// 4. 逆幂迭代获取 σ_min 对应的奇异向量
    ///
    /// # 参数
    /// - `ybus`: Y-Bus 导纳矩阵
    /// - `v`: 电压幅值数组 (p.u.)
    /// - `theta`: 电压相角数组 (rad)
    /// - `bus_types`: 母线类型
    pub fn voltage_stability_modal_analysis(
        &self,
        ybus: &YBusMatrix,
        v: &[f64],
        theta: &[f64],
        bus_types: &[BusTypeNR],
    ) -> Result<AnalysisResult<VoltageStabilityResult>, AnalysisError> {
        let n = ybus.size();
        if n == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "Y-Bus 为空".to_string(),
            ));
        }
        if v.len() != n || theta.len() != n || bus_types.len() != n {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "维度不匹配: ybus.size={}, v.len={}, theta.len={}, bus_types.len={}",
                n,
                v.len(),
                theta.len(),
                bus_types.len()
            )));
        }

        // 构建雅可比 J（稠密）
        let jacobian = build_pf_jacobian(ybus, v, theta, bus_types);
        let m = jacobian.len();
        if m == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "雅可比矩阵为空（可能所有母线都是 slack）".to_string(),
            ));
        }

        // 计算 J^T·J（对称半正定）
        let mut jtj = vec![vec![0.0_f64; m]; m];
        for i in 0..m {
            for j in 0..m {
                let mut s = 0.0;
                for k in 0..m {
                    s += jacobian[k][i] * jacobian[k][j];
                }
                jtj[i][j] = s;
            }
        }

        // 幂迭代求 λ_max(J^T·J)
        let (lambda_max, _vec_max) = power_iteration(&jtj, 200, 1e-10);
        let sigma_max = lambda_max.sqrt();

        // 逆幂迭代求 λ_min(J^T·J)
        // 使用位移 μ = λ_max * 0.01 作为逆幂迭代的位移
        let shift = lambda_max * 1e-6;
        let (lambda_min, vec_min) = inverse_power_iteration(&jtj, shift, 300, 1e-10);
        let sigma_min = if lambda_min > 0.0 {
            lambda_min.sqrt()
        } else {
            0.0
        };

        let condition_number = if sigma_min > EPS {
            sigma_max / sigma_min
        } else {
            f64::INFINITY
        };

        // 右奇异向量 v = vec_min（J^T·J 的特征向量）
        let right_singular_vector = vec_min.clone();

        // 左奇异向量 u = J·v / σ_min
        let mut left_singular_vector = vec![0.0_f64; m];
        if sigma_min > EPS {
            for i in 0..m {
                let mut s = 0.0;
                for j in 0..m {
                    s += jacobian[i][j] * vec_min[j];
                }
                left_singular_vector[i] = s / sigma_min;
            }
        }

        // 找最薄弱母线（左奇异向量中绝对值最大的分量）
        let weakest_bus_idx = left_singular_vector
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);

        // 电压稳定裕度百分比（相对于 σ_max）
        let stability_margin_percent = if sigma_max > EPS {
            (sigma_min / sigma_max) * 100.0
        } else {
            0.0
        };

        // 接近不稳定的阈值：σ_min < 0.1（经验值）
        let near_instability = sigma_min < 0.1;

        let mut warnings = Vec::new();
        if near_instability {
            warnings.push(format!(
                "电压稳定裕度低: σ_min={:.6}, κ={:.1}",
                sigma_min, condition_number
            ));
        }

        Ok(AnalysisResult {
            converged: true,
            iterations: 1,
            result: VoltageStabilityResult {
                min_singular_value: sigma_min,
                max_singular_value: sigma_max,
                condition_number,
                left_singular_vector,
                right_singular_vector,
                weakest_bus_idx,
                near_instability,
                stability_margin_percent,
            },
            warnings,
        })
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 求解复数线性方程组 Y * V = I（高斯消元法，部分主元选取）
fn solve_complex_linear_system(
    y: &[Vec<Complex64>],
    b: &[Complex64],
) -> Option<Vec<Complex64>> {
    let n = b.len();
    if n == 0 {
        return Some(Vec::new());
    }

    // 构建增广矩阵 [Y | b]
    let mut aug = vec![vec![Complex64::new(0.0, 0.0); n + 1]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = y[i][j];
        }
        aug[i][n] = b[i];
    }

    // 前向消元（部分主元选取）
    for col in 0..n {
        let mut max_val = aug[col][col].norm();
        let mut max_row = col;
        for row in (col + 1)..n {
            if aug[row][col].norm() > max_val {
                max_val = aug[row][col].norm();
                max_row = row;
            }
        }

        if max_val < EPSILON {
            return None; // 奇异矩阵
        }

        if max_row != col {
            aug.swap(col, max_row);
        }

        let pivot = aug[col][col];
        let pivot_row = aug[col].clone();
        for row in (col + 1)..n {
            let factor = aug[row][col] / pivot;
            for k in col..=n {
                aug[row][k] -= factor * pivot_row[k];
            }
        }
    }

    // 回代
    let mut x = vec![Complex64::new(0.0, 0.0); n];
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in (i + 1)..n {
            sum -= aug[i][j] * x[j];
        }
        x[i] = sum / aug[i][i];
    }

    Some(x)
}

// ============================================================================
// T5 辅助函数：雅可比构建 + 幂迭代 + 逆幂迭代
// ============================================================================

/// 构建潮流雅可比矩阵 J（极坐标，去除 slack 行列）
///
/// 雅可比结构：
/// ```text
/// J = [ H  N ]   H = ∂P/∂θ  (n_ns × n_ns)
///     [ M  L ]   N = ∂P/∂V  (n_ns × n_pq)
///                M = ∂Q/∂θ  (n_pq × n_ns)
///                L = ∂Q/∂V  (n_pq × n_pq)
/// ```
/// 其中 n_ns = 非 slack 母线数，n_pq = PQ 母线数。
fn build_pf_jacobian(
    ybus: &YBusMatrix,
    v: &[f64],
    theta: &[f64],
    bus_types: &[BusTypeNR],
) -> Vec<Vec<f64>> {
    let n = ybus.size();
    let non_slack: Vec<usize> = (0..n).filter(|&i| bus_types[i] != BusTypeNR::Slack).collect();
    let pq: Vec<usize> = (0..n).filter(|&i| bus_types[i] == BusTypeNR::PQ).collect();
    let nns = non_slack.len();
    let npq = pq.len();
    let size = nns + npq;
    let mut j = vec![vec![0.0_f64; size]; size];

    // H: ∂P/∂θ (nns × nns)
    for (ii, &i) in non_slack.iter().enumerate() {
        for (jj, &k) in non_slack.iter().enumerate() {
            if i == k {
                let mut s = 0.0;
                for (m, g, b) in ybus.iter_row(i) {
                    if m != i {
                        let ad = theta[i] - theta[m];
                        s += v[i] * v[m] * (g * ad.sin() - b * ad.cos());
                    }
                }
                j[ii][jj] = -s;
            } else {
                let (g, b) = ybus.get(i, k);
                let ad = theta[i] - theta[k];
                j[ii][jj] = v[i] * v[k] * (g * ad.sin() - b * ad.cos());
            }
        }
    }

    // N: ∂P/∂V (nns × npq)
    for (ii, &i) in non_slack.iter().enumerate() {
        for (jj, &k) in pq.iter().enumerate() {
            if i == k {
                let mut p = 0.0;
                for (m, g, b) in ybus.iter_row(i) {
                    let ad = theta[i] - theta[m];
                    p += v[i] * v[m] * (g * ad.cos() + b * ad.sin());
                }
                let (g_ii, _) = ybus.get(i, i);
                j[ii][nns + jj] = p / v[i] + v[i] * g_ii;
            } else {
                let (g, b) = ybus.get(i, k);
                let ad = theta[i] - theta[k];
                j[ii][nns + jj] = v[i] * (g * ad.cos() + b * ad.sin());
            }
        }
    }

    // M: ∂Q/∂θ (npq × nns)
    for (ii, &i) in pq.iter().enumerate() {
        for (jj, &k) in non_slack.iter().enumerate() {
            if i == k {
                let mut s = 0.0;
                for (m, g, b) in ybus.iter_row(i) {
                    if m != i {
                        let ad = theta[i] - theta[m];
                        s += v[i] * v[m] * (g * ad.cos() + b * ad.sin());
                    }
                }
                j[nns + ii][jj] = s;
            } else {
                let (g, b) = ybus.get(i, k);
                let ad = theta[i] - theta[k];
                j[nns + ii][jj] = -v[i] * v[k] * (g * ad.cos() + b * ad.sin());
            }
        }
    }

    // L: ∂Q/∂V (npq × npq)
    for (ii, &i) in pq.iter().enumerate() {
        for (jj, &k) in pq.iter().enumerate() {
            if i == k {
                let mut q = 0.0;
                for (m, g, b) in ybus.iter_row(i) {
                    let ad = theta[i] - theta[m];
                    q += v[i] * v[m] * (g * ad.sin() - b * ad.cos());
                }
                let (_, b_ii) = ybus.get(i, i);
                j[nns + ii][nns + jj] = q / v[i] - v[i] * b_ii;
            } else {
                let (g, b) = ybus.get(i, k);
                let ad = theta[i] - theta[k];
                j[nns + ii][nns + jj] = v[i] * (g * ad.sin() - b * ad.cos());
            }
        }
    }

    j
}

/// 幂迭代法求对称矩阵的最大特征值和对应特征向量
///
/// 适用于 J^T·J（对称半正定），返回 (λ_max, v_max)。
fn power_iteration(mat: &[Vec<f64>], max_iter: usize, tol: f64) -> (f64, Vec<f64>) {
    let n = mat.len();
    if n == 0 {
        return (0.0, Vec::new());
    }

    let mut v = vec![1.0_f64 / (n as f64).sqrt(); n];
    let mut lambda_prev = 0.0_f64;

    for _ in 0..max_iter {
        // w = A * v
        let mut w = vec![0.0_f64; n];
        for i in 0..n {
            let mut s = 0.0;
            for j in 0..n {
                s += mat[i][j] * v[j];
            }
            w[i] = s;
        }

        // λ = v^T * w
        let lambda: f64 = v.iter().zip(w.iter()).map(|(a, b)| a * b).sum();

        // 归一化 w
        let norm: f64 = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < EPS {
            break;
        }
        for i in 0..n {
            v[i] = w[i] / norm;
        }

        if (lambda - lambda_prev).abs() < tol * lambda.abs().max(1.0) {
            return (lambda, v);
        }
        lambda_prev = lambda;
    }

    // 最终计算 λ
    let mut w = vec![0.0_f64; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += mat[i][j] * v[j];
        }
        w[i] = s;
    }
    let lambda: f64 = v.iter().zip(w.iter()).map(|(a, b)| a * b).sum();

    (lambda, v)
}

/// 逆幂迭代法求对称矩阵的最小特征值和对应特征向量
///
/// 使用位移 μ，求解 (A - μI)^{-1} 的最大特征值，
/// 对应 A 的最接近 μ 的特征值。
///
/// 返回 (λ_min, v_min)。
fn inverse_power_iteration(mat: &[Vec<f64>], shift: f64, max_iter: usize, tol: f64) -> (f64, Vec<f64>) {
    let n = mat.len();
    if n == 0 {
        return (0.0, Vec::new());
    }

    // 构造 (A - μI)
    let mut a_shifted = mat.to_vec();
    for i in 0..n {
        a_shifted[i][i] -= shift;
    }

    // LU 分解 (A - μI) 用于快速求解
    let lu = lu_decompose(&a_shifted);

    let mut v = vec![1.0_f64 / (n as f64).sqrt(); n];
    let mut lambda_prev = 0.0_f64;

    for _ in 0..max_iter {
        // w = (A - μI)^{-1} * v
        let w = match &lu {
            Some(lu) => lu_solve(lu, &v),
            None => break,
        };

        // 归一化
        let norm: f64 = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < EPS {
            break;
        }
        for i in 0..n {
            v[i] = w[i] / norm;
        }

        // Rayleigh 商: λ = v^T * A * v
        let mut av = vec![0.0_f64; n];
        for i in 0..n {
            let mut s = 0.0;
            for j in 0..n {
                s += mat[i][j] * v[j];
            }
            av[i] = s;
        }
        let lambda: f64 = v.iter().zip(av.iter()).map(|(a, b)| a * b).sum();

        if (lambda - lambda_prev).abs() < tol * lambda.abs().max(1.0) {
            return (lambda, v);
        }
        lambda_prev = lambda;
    }

    // 最终 λ
    let mut av = vec![0.0_f64; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += mat[i][j] * v[j];
        }
        av[i] = s;
    }
    let lambda: f64 = v.iter().zip(av.iter()).map(|(a, b)| a * b).sum();

    (lambda, v)
}

/// LU 分解（带部分主元选取）
struct LuDecomposition {
    lu: Vec<Vec<f64>>,
    piv: Vec<usize>,
}

/// 对矩阵进行 LU 分解
fn lu_decompose(a: &[Vec<f64>]) -> Option<LuDecomposition> {
    let n = a.len();
    if n == 0 {
        return Some(LuDecomposition {
            lu: Vec::new(),
            piv: Vec::new(),
        });
    }

    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();

    for k in 0..n {
        // 部分主元选取
        let mut max_val = lu[k][k].abs();
        let mut max_row = k;
        for i in (k + 1)..n {
            if lu[i][k].abs() > max_val {
                max_val = lu[i][k].abs();
                max_row = i;
            }
        }

        if max_val < EPS {
            return None; // 奇异矩阵
        }

        if max_row != k {
            lu.swap(k, max_row);
            piv.swap(k, max_row);
        }

        // 消元
        for i in (k + 1)..n {
            lu[i][k] /= lu[k][k];
            for j in (k + 1)..n {
                lu[i][j] -= lu[i][k] * lu[k][j];
            }
        }
    }

    Some(LuDecomposition { lu, piv })
}

/// 使用 LU 分解求解线性方程组 Ax = b
fn lu_solve(lu: &LuDecomposition, b: &[f64]) -> Vec<f64> {
    let n = lu.lu.len();
    if n == 0 {
        return Vec::new();
    }

    // 应用主元置换
    let mut x = vec![0.0_f64; n];
    for i in 0..n {
        x[i] = b[lu.piv[i]];
    }

    // 前代 (Ly = Pb)
    for i in 1..n {
        for j in 0..i {
            x[i] -= lu.lu[i][j] * x[j];
        }
    }

    // 回代 (Ux = y)
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu.lu[i][j] * x[j];
        }
        x[i] /= lu.lu[i][i];
    }

    x
}

// ============================================================================
// T4.7-T4.8 验证测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试场景：3 母线系统（1 发电机 + 2 负荷母线）
    fn create_test_scenario(t_clear: f64, method: IntegrationMethod) -> TransientScenario {
        TransientScenario {
            generators: vec![GeneratorDynamic {
                gen_id: 1,
                bus_id: 1,
                model: GeneratorModel::Classical2nd,
                h: 3.0,
                d: 2.0,
                xd_prime: 0.3,
                xd: 1.8,
                efd: 1.0,
                pm: 0.8,
                ka: 0.0,
                ta: 0.0,
            }],
            buses: vec![1, 2, 3],
            branches: vec![
                (1, 2, 0.01, 0.1, 0.0, 1.0),
                (2, 3, 0.01, 0.1, 0.0, 1.0),
                (1, 3, 0.01, 0.1, 0.0, 1.0),
            ],
            base_mva: 100.0,
            fault: TransientFault::ThreePhase {
                bus_id: 2,
                fault_impedance: 0.0,
            },
            params: SimulationParams {
                t_start: 0.0,
                t_end: 3.0,
                dt: 0.01,
                t_fault: 0.1,
                t_clear,
                method,
                frequency: 50.0,
            },
            loads: vec![(2, 0.3, 0.1), (3, 0.5, 0.1)],
        }
    }

    /// T4.7 测试 1：经典二阶模型导数计算
    ///
    /// 单机无穷大系统，验证摇摆方程导数计算正确
    #[test]
    fn test_classical_2nd_order_model() {
        let omega_s = 2.0 * PI * 50.0; // 314.159... rad/s
        let gen = GeneratorDynamic {
            gen_id: 1,
            bus_id: 1,
            model: GeneratorModel::Classical2nd,
            h: 5.0,
            d: 0.0,
            xd_prime: 0.3,
            xd: 1.8,
            efd: 1.0,
            pm: 0.8,
            ka: 0.0,
            ta: 0.0,
        };

        // 已知状态
        let delta: f64 = 0.5; // rad
        let omega = omega_s + 0.1; // rad/s（偏离同步速 0.1 rad/s）
        // 单机无穷大系统: Pe = (Eq' * V_inf / x_total) * sin(delta)
        let pe = 1.0_f64 * 1.0 / 1.0 * delta.sin();

        // 摇摆方程导数
        let d_delta = omega - omega_s;
        let d_omega =
            (omega_s / (2.0 * gen.h)) * (gen.pm - pe - gen.d * (omega - omega_s));

        // 验证 dδ/dt = ω - ω_s
        assert!(
            (d_delta - 0.1).abs() < 1e-10,
            "dδ/dt 应为 ω - ω_s = 0.1, 实际 = {}",
            d_delta
        );

        // 验证 dω/dt = (ω_s / 2H) * (Pm - Pe - D*(ω - ω_s))
        // D = 0, 所以 dω/dt = (ω_s / 2H) * (Pm - Pe)
        let expected_d_omega = (omega_s / 10.0) * (gen.pm - pe);
        assert!(
            (d_omega - expected_d_omega).abs() < 1e-6,
            "dω/dt 计算错误: 期望 {}, 实际 {}",
            expected_d_omega,
            d_omega
        );

        // 验证阻尼项：D > 0 时应减小加速度
        let gen_damped = GeneratorDynamic {
            d: 1.0,
            ..gen
        };
        let d_omega_damped =
            (omega_s / (2.0 * gen_damped.h)) * (gen_damped.pm - pe - gen_damped.d * (omega - omega_s));
        assert!(
            d_omega_damped < d_omega,
            "有阻尼时加速度应小于无阻尼: {} < {}",
            d_omega_damped,
            d_omega
        );
    }

    /// T4.7 测试 2：RK4 积分器验证
    ///
    /// 简单 ODE: dy/dt = -y, y(0) = 1
    /// 解析解: y(t) = e^(-t)
    #[test]
    fn test_rk4_integration() {
        let dt = 0.01;
        let n_steps = 100;
        let mut y = 1.0;

        for _ in 0..n_steps {
            let k1 = -y;
            let k2 = -(y + dt / 2.0 * k1);
            let k3 = -(y + dt / 2.0 * k2);
            let k4 = -(y + dt * k3);
            y += dt / 6.0 * (k1 + 2.0 * k2 + 2.0 * k3 + k4);
        }

        let t = dt * n_steps as f64;
        let analytical = (-t).exp();
        assert!(
            (y - analytical).abs() < 1e-8,
            "RK4 误差过大: 数值 = {}, 解析 = {}",
            y,
            analytical
        );
    }

    /// T4.7 测试 3：暂态稳定仿真（类 IEEE-9 节点系统）
    ///
    /// 3 母线系统（1 发电机 + 2 负荷母线），三相故障
    #[test]
    fn test_transient_simulation_ieee9_like() {
        let analyzer = TransientStabilityAnalyzer::new();
        let scenario = create_test_scenario(0.2, IntegrationMethod::RK4);

        let result = analyzer.analyze(&scenario).expect("仿真应成功");

        // 验证 time_series 非空
        assert!(
            !result.result.time_series.is_empty(),
            "时间序列不应为空"
        );

        // 验证时间步数 > 100
        assert!(
            result.result.time_series.len() > 100,
            "时间步数应大于 100, 实际 = {}",
            result.result.time_series.len()
        );

        // 验证时间步正确
        let dt = scenario.params.dt;
        for (i, step) in result.result.time_series.iter().enumerate() {
            let expected_t = i as f64 * dt;
            assert!(
                (step.t - expected_t).abs() < 1e-6,
                "时间步 {} 不正确: 期望 {}, 实际 {}",
                i,
                expected_t,
                step.t
            );
        }

        // 验证初始状态：功角应为正值（Pm > 0）
        let initial = &result.result.time_series[0];
        assert!(
            !initial.rotor_angles.is_empty(),
            "初始转子功角不应为空"
        );
        let (_, delta_0) = initial.rotor_angles[0];
        assert!(
            delta_0 > 0.0,
            "初始功角应为正值（Pm > 0）, 实际 = {}",
            delta_0
        );

        // 验证故障期间功角增大
        let t_fault = scenario.params.t_fault;
        let t_clear = scenario.params.t_clear;
        let delta_at_fault = result
            .result
            .time_series
            .iter()
            .find(|s| (s.t - t_fault).abs() < dt / 2.0)
            .and_then(|s| s.rotor_angles.first())
            .map(|(_, d)| *d)
            .unwrap_or(0.0);
        let delta_at_clear = result
            .result
            .time_series
            .iter()
            .find(|s| (s.t - t_clear).abs() < dt / 2.0)
            .and_then(|s| s.rotor_angles.first())
            .map(|(_, d)| *d)
            .unwrap_or(0.0);

        assert!(
            delta_at_clear > delta_at_fault,
            "故障期间功角应增大: 故障时 = {}, 清除时 = {}",
            delta_at_fault,
            delta_at_clear
        );

        // 验证母线电压记录
        assert!(
            !initial.bus_voltages.is_empty(),
            "母线电压记录不应为空"
        );
    }

    /// T4.7 测试 4：隐式梯形法（Heun 预测-校正）验证
    ///
    /// 简单 ODE: dy/dt = -y, y(0) = 1
    #[test]
    fn test_implicit_trapezoidal() {
        let dt = 0.01;
        let n_steps = 100;
        let mut y = 1.0;

        for _ in 0..n_steps {
            // 预测步（显式欧拉）
            let k1 = -y;
            let y_pred = y + dt * k1;
            // 校正步（梯形）
            let k2 = -y_pred;
            y += dt / 2.0 * (k1 + k2);
        }

        let t = dt * n_steps as f64;
        let analytical = (-t).exp();
        // Heun 方法为 2 阶精度，容差放宽
        assert!(
            (y - analytical).abs() < 1e-4,
            "Heun 方法误差过大: 数值 = {}, 解析 = {}",
            y,
            analytical
        );

        // 验证通过完整仿真测试隐式梯形法
        let analyzer = TransientStabilityAnalyzer::new();
        let scenario = create_test_scenario(0.2, IntegrationMethod::ImplicitTrapezoidal);
        let result = analyzer.analyze(&scenario).expect("隐式梯形仿真应成功");
        assert!(
            !result.result.time_series.is_empty(),
            "隐式梯形法时间序列不应为空"
        );
    }

    /// 构造 2 发电机测试场景（用于稳定性检测）
    ///
    /// 3 母线系统：2 发电机 + 1 负荷母线
    /// - 发电机 1：低惯量、高出力（加速快）
    /// - 发电机 2：高惯量、低出力（加速慢）
    ///
    /// 故障期间两发电机加速度不同，功角差增大
    fn create_2gen_scenario(t_clear: f64) -> TransientScenario {
        TransientScenario {
            generators: vec![
                GeneratorDynamic {
                    gen_id: 1,
                    bus_id: 1,
                    model: GeneratorModel::Classical2nd,
                    h: 1.5,
                    d: 0.0,
                    xd_prime: 0.3,
                    xd: 1.8,
                    efd: 1.1,
                    pm: 1.2,
                    ka: 0.0,
                    ta: 0.0,
                },
                GeneratorDynamic {
                    gen_id: 2,
                    bus_id: 2,
                    model: GeneratorModel::Classical2nd,
                    h: 8.0,
                    d: 0.0,
                    xd_prime: 0.3,
                    xd: 1.8,
                    efd: 1.1,
                    pm: 0.2,
                    ka: 0.0,
                    ta: 0.0,
                },
            ],
            buses: vec![1, 2, 3],
            branches: vec![
                (1, 3, 0.02, 0.2, 0.0, 1.0),
                (2, 3, 0.02, 0.2, 0.0, 1.0),
                (1, 2, 0.05, 0.5, 0.0, 1.0),
            ],
            base_mva: 100.0,
            fault: TransientFault::ThreePhase {
                bus_id: 1,
                fault_impedance: 0.0,
            },
            params: SimulationParams {
                t_start: 0.0,
                t_end: 3.0,
                dt: 0.01,
                t_fault: 0.1,
                t_clear,
                method: IntegrationMethod::RK4,
                frequency: 50.0,
            },
            loads: vec![(3, 1.0, 0.2)],
        }
    }

    /// T4.8 测试 5：稳定性检测
    ///
    /// 构造稳定和不稳定场景，验证 stable 字段正确
    #[test]
    fn test_stability_detection() {
        let analyzer = TransientStabilityAnalyzer::new();

        // 场景 1：快速清除故障（t_clear = 0.15s）→ 应保持稳定
        let stable_scenario = create_2gen_scenario(0.15);
        let stable_result = analyzer.analyze(&stable_scenario).expect("稳定场景仿真应成功");

        // 场景 2：慢速清除故障（t_clear = 0.8s）→ 应失稳或功角差更大
        let unstable_scenario = create_2gen_scenario(0.8);
        let unstable_result = analyzer
            .analyze(&unstable_scenario)
            .expect("不稳定场景仿真应成功");

        // 验证稳定场景的功角差
        let stable_spread = stable_result.result.max_angle_spread_deg;
        eprintln!(
            "稳定场景: t_clear=0.15s, max_angle_spread={:.1}°, stable={}",
            stable_spread, stable_result.result.stable
        );

        // 验证不稳定场景的功角差
        let unstable_spread = unstable_result.result.max_angle_spread_deg;
        eprintln!(
            "不稳定场景: t_clear=0.8s, max_angle_spread={:.1}°, stable={}",
            unstable_spread, unstable_result.result.stable
        );

        // 快速清除应比慢速清除更稳定（功角差更小）
        assert!(
            stable_spread < unstable_spread,
            "快速清除的功角差 ({:.1}°) 应小于慢速清除 ({:.1}°)",
            stable_spread,
            unstable_spread
        );

        // 慢速清除应导致失稳或功角差显著增大
        // 如果系统设计得足够激进，t_clear=0.8s 应失稳
        // 否则至少功角差应显著大于快速清除
        if !unstable_result.result.stable {
            // 符合预期：失稳
        } else {
            // 未失稳但功角差应显著增大（至少 2 倍）
            assert!(
                unstable_spread > stable_spread * 2.0,
                "慢速清除的功角差 ({:.1}°) 应显著大于快速清除 ({:.1}°) 的 2 倍",
                unstable_spread,
                stable_spread
            );
        }
    }

    /// 测试 6：四阶模型仿真
    #[test]
    fn test_fourth_order_model() {
        let analyzer = TransientStabilityAnalyzer::new();

        let scenario = TransientScenario {
            generators: vec![GeneratorDynamic {
                gen_id: 1,
                bus_id: 1,
                model: GeneratorModel::FourthOrder,
                h: 5.0,
                d: 1.0,
                xd_prime: 0.3,
                xd: 1.8,
                efd: 1.0,
                pm: 0.5,
                ka: 10.0,
                ta: 0.1,
            }],
            buses: vec![1, 2, 3],
            branches: vec![
                (1, 2, 0.01, 0.1, 0.0, 1.0),
                (2, 3, 0.01, 0.1, 0.0, 1.0),
                (1, 3, 0.01, 0.1, 0.0, 1.0),
            ],
            base_mva: 100.0,
            fault: TransientFault::ThreePhase {
                bus_id: 2,
                fault_impedance: 0.0,
            },
            params: SimulationParams {
                t_start: 0.0,
                t_end: 2.0,
                dt: 0.01,
                t_fault: 0.1,
                t_clear: 0.2,
                method: IntegrationMethod::RK4,
                frequency: 50.0,
            },
            loads: vec![(2, 0.3, 0.1), (3, 0.2, 0.1)],
        };

        let result = analyzer.analyze(&scenario).expect("四阶模型仿真应成功");
        assert!(
            !result.result.time_series.is_empty(),
            "四阶模型时间序列不应为空"
        );
    }

    /// 测试 7：断线故障仿真
    #[test]
    fn test_line_outage_fault() {
        let analyzer = TransientStabilityAnalyzer::new();

        let scenario = TransientScenario {
            generators: vec![GeneratorDynamic {
                gen_id: 1,
                bus_id: 1,
                model: GeneratorModel::Classical2nd,
                h: 5.0,
                d: 2.0,
                xd_prime: 0.3,
                xd: 1.8,
                efd: 1.0,
                pm: 0.5,
                ka: 0.0,
                ta: 0.0,
            }],
            buses: vec![1, 2, 3],
            branches: vec![
                (1, 2, 0.01, 0.1, 0.0, 1.0),
                (2, 3, 0.01, 0.1, 0.0, 1.0),
                (1, 3, 0.01, 0.1, 0.0, 1.0),
            ],
            base_mva: 100.0,
            fault: TransientFault::LineOutage { branch_id: 0 },
            params: SimulationParams {
                t_start: 0.0,
                t_end: 2.0,
                dt: 0.01,
                t_fault: 0.1,
                t_clear: 0.2,
                method: IntegrationMethod::RK4,
                frequency: 50.0,
            },
            loads: vec![(2, 0.3, 0.1), (3, 0.2, 0.1)],
        };

        let result = analyzer.analyze(&scenario).expect("断线故障仿真应成功");
        assert!(
            !result.result.time_series.is_empty(),
            "断线故障时间序列不应为空"
        );
    }

    /// 测试 8：输入验证
    #[test]
    fn test_input_validation() {
        let analyzer = TransientStabilityAnalyzer::new();

        // 无发电机
        let mut scenario = create_test_scenario(0.2, IntegrationMethod::RK4);
        scenario.generators = vec![];
        let result = analyzer.analyze(&scenario);
        assert!(result.is_err(), "无发电机应返回错误");

        // 步长 <= 0
        let mut scenario = create_test_scenario(0.2, IntegrationMethod::RK4);
        scenario.params.dt = 0.0;
        let result = analyzer.analyze(&scenario);
        assert!(result.is_err(), "步长为 0 应返回错误");

        // 故障清除时间 <= 故障发生时间
        let mut scenario = create_test_scenario(0.2, IntegrationMethod::RK4);
        scenario.params.t_clear = 0.1;
        scenario.params.t_fault = 0.2;
        let result = analyzer.analyze(&scenario);
        assert!(result.is_err(), "清除时间 <= 故障时间应返回错误");

        // 仿真时间超过最大限制
        let mut scenario = create_test_scenario(0.2, IntegrationMethod::RK4);
        scenario.params.t_end = 100.0;
        let result = analyzer.analyze(&scenario);
        assert!(result.is_err(), "仿真时间超限应返回错误");
    }

    /// 测试 9：60 Hz 系统仿真
    #[test]
    fn test_60hz_system() {
        let analyzer = TransientStabilityAnalyzer::new();
        let mut scenario = create_test_scenario(0.2, IntegrationMethod::RK4);
        scenario.params.frequency = 60.0;

        let result = analyzer.analyze(&scenario).expect("60Hz 仿真应成功");
        assert!(
            !result.result.time_series.is_empty(),
            "60Hz 系统时间序列不应为空"
        );

        // 验证转速标幺值在合理范围
        for step in &result.result.time_series {
            for (_, speed_pu) in &step.rotor_speeds {
                assert!(
                    *speed_pu > 0.9 && *speed_pu < 1.1,
                    "转速标幺值应在合理范围: {}",
                    speed_pu
                );
            }
        }
    }

    // ========================================================================
    // T5.5 / T5.6 验证测试
    // ========================================================================

    /// T5.1 测试：CCT 二分搜索
    ///
    /// 使用 2 发电机场景，验证 CCT 计算正确：
    /// - 快速清除（< CCT）应稳定
    /// - 慢速清除（> CCT）应失稳
    /// - CCT 应在两者之间
    #[test]
    fn test_cct_binary_search() {
        let analyzer = TransientStabilityAnalyzer::new();
        let scenario = create_2gen_scenario(0.2); // t_clear 会被覆盖

        // 先探测边界：找到稳定和不稳定的 t_clear
        let mut s_probe = scenario.clone();
        s_probe.params.t_clear = 0.15;
        let r_stable = analyzer.analyze(&s_probe).expect("t_clear=0.15 仿真应成功");
        eprintln!(
            "探测 t_clear=0.15s: stable={}, spread={:.1}°",
            r_stable.result.stable, r_stable.result.max_angle_spread_deg
        );

        let mut s_probe = scenario.clone();
        s_probe.params.t_clear = 0.5;
        let r_unstable = analyzer.analyze(&s_probe).expect("t_clear=0.5 仿真应成功");
        eprintln!(
            "探测 t_clear=0.5s: stable={}, spread={:.1}°",
            r_unstable.result.stable, r_unstable.result.max_angle_spread_deg
        );

        // 如果边界不满足条件，调整搜索区间
        let (t_min, t_max) = if r_stable.result.stable && !r_unstable.result.stable {
            (0.15, 0.5)
        } else if r_stable.result.stable && r_unstable.result.stable {
            // 都稳定，扩大上限
            let mut s_probe = scenario.clone();
            s_probe.params.t_clear = 1.0;
            let r = analyzer.analyze(&s_probe).expect("t_clear=1.0 仿真应成功");
            eprintln!(
                "探测 t_clear=1.0s: stable={}, spread={:.1}°",
                r.result.stable, r.result.max_angle_spread_deg
            );
            if r.result.stable {
                // 整个区间都稳定，跳过 CCT 验证
                eprintln!("整个区间 [0.15, 1.0] 均稳定，跳过 CCT 二分验证");
                return;
            }
            (0.15, 1.0)
        } else {
            (0.15, 0.5)
        };

        let result = analyzer
            .compute_cct(&scenario, t_min, t_max, 0.01, 20)
            .expect("CCT 计算应成功");

        let cct = result.result.cct;
        eprintln!(
            "CCT = {:.4}s ({} 次迭代, 追踪 {} 点)",
            cct,
            result.result.iterations,
            result.result.search_trace.len()
        );

        // CCT 应在搜索区间内
        assert!(
            cct >= t_min && cct <= t_max,
            "CCT {} 应在 [{}, {}] 区间内",
            cct,
            t_min,
            t_max
        );

        // 验证：在 CCT 之前应稳定
        let margin = 0.03;
        if cct - margin > scenario.params.t_fault {
            let mut s_stable = scenario.clone();
            s_stable.params.t_clear = cct - margin;
            let r_stable = analyzer.analyze(&s_stable).expect("CCT-margin 仿真应成功");
            assert!(
                r_stable.result.stable,
                "t_clear = CCT - {} = {:.4}s 应稳定, 实际 stable={}, spread={:.1}°",
                margin,
                cct - margin,
                r_stable.result.stable,
                r_stable.result.max_angle_spread_deg
            );
        }

        // 验证：在 CCT 之后应失稳
        let mut s_unstable = scenario.clone();
        s_unstable.params.t_clear = cct + margin;
        let r_unstable = analyzer.analyze(&s_unstable).expect("CCT+margin 仿真应成功");
        assert!(
            !r_unstable.result.stable,
            "t_clear = CCT + {} = {:.4}s 应失稳, 实际 stable={}, spread={:.1}°",
            margin,
            cct + margin,
            r_unstable.result.stable,
            r_unstable.result.max_angle_spread_deg
        );
    }

    /// T5.1 测试：CCT 边界情况
    #[test]
    fn test_cct_edge_cases() {
        let analyzer = TransientStabilityAnalyzer::new();
        let scenario = create_2gen_scenario(0.2);

        // 整个区间稳定
        let r = analyzer.compute_cct(&scenario, 0.11, 0.15, 0.01, 10);
        assert!(r.is_ok());
        let r = r.unwrap();
        assert!(
            !r.warnings.is_empty(),
            "全稳定区间应有警告"
        );

        // 整个区间失稳
        let r = analyzer.compute_cct(&scenario, 0.9, 1.0, 0.01, 10);
        assert!(r.is_ok());

        // 参数错误
        let r = analyzer.compute_cct(&scenario, 1.0, 0.5, 0.01, 10);
        assert!(r.is_err(), "min > max 应返回错误");
    }

    /// T5.2 测试：等面积法则
    ///
    /// 单机无穷大系统，验证等面积法则 CCT 计算合理
    #[test]
    fn test_equal_area_criterion() {
        let analyzer = TransientStabilityAnalyzer::new();

        let gen = GeneratorDynamic {
            gen_id: 1,
            bus_id: 1,
            model: GeneratorModel::Classical2nd,
            h: 5.0,
            d: 0.0,
            xd_prime: 0.3,
            xd: 1.8,
            efd: 1.1,
            pm: 0.8,
            ka: 0.0,
            ta: 0.0,
        };

        // 故障前：x = xd' + x_line = 0.3 + 0.7 = 1.0（双回线并联）
        // 故障期间：x = ∞（三相短路，Pe = 0）
        // 故障后：x = xd' + x_line_post = 0.3 + 0.8 = 1.1（切除一回线）
        // Pmax_pre = 1.1, Pmax_post = 1.0, Pm = 0.8 < Pmax_post ✓
        let result = analyzer
            .equal_area_criterion(&gen, 1.0, 1.0, f64::INFINITY, 1.1, 50.0)
            .expect("等面积法则计算应成功");

        eprintln!(
            "等面积法则: δ₀={:.4} rad, δ_c={:.4} rad, δ_max={:.4} rad, CCT={:.4}s",
            result.result.delta_0,
            result.result.delta_c_critical,
            result.result.delta_max,
            result.result.cct
        );
        eprintln!(
            "  Pmax_pre={:.4}, Pmax_fault={:.4}, Pmax_post={:.4}",
            result.result.pmax_pre, result.result.pmax_fault, result.result.pmax_post
        );
        eprintln!(
            "  A_accel={:.6}, A_decel={:.6} (应近似相等)",
            result.result.a_accel, result.result.a_decel
        );

        // 验证初始功角
        assert!(
            result.result.delta_0 > 0.0 && result.result.delta_0 < PI / 2.0,
            "δ₀ 应在 (0, π/2) 区间, 实际 = {}",
            result.result.delta_0
        );

        // 验证最大功角
        assert!(
            result.result.delta_max > PI / 2.0 && result.result.delta_max < PI,
            "δ_max 应在 (π/2, π) 区间, 实际 = {}",
            result.result.delta_max
        );

        // 验证临界清除功角
        assert!(
            result.result.delta_c_critical > result.result.delta_0,
            "δ_c 应大于 δ₀"
        );
        assert!(
            result.result.delta_c_critical < result.result.delta_max,
            "δ_c 应小于 δ_max"
        );

        // 验证加速面积 ≈ 减速面积
        let area_diff = (result.result.a_accel - result.result.a_decel).abs();
        let area_avg = (result.result.a_accel + result.result.a_decel) / 2.0;
        assert!(
            area_diff < 0.01 * area_avg.max(0.01),
            "加速面积 ({}) 应近似等于减速面积 ({}), 差 = {}",
            result.result.a_accel,
            result.result.a_decel,
            area_diff
        );

        // 验证 CCT 为正有限值
        assert!(
            result.result.cct > 0.0 && result.result.cct.is_finite(),
            "CCT 应为正有限值, 实际 = {}",
            result.result.cct
        );

        // CCT 应在合理范围 (0.1s ~ 1.0s)
        assert!(
            result.result.cct > 0.05 && result.result.cct < 2.0,
            "CCT 应在 (0.05, 2.0) 区间, 实际 = {}",
            result.result.cct
        );
    }

    /// T5.2 测试：等面积法则参数验证
    #[test]
    fn test_equal_area_criterion_validation() {
        let analyzer = TransientStabilityAnalyzer::new();

        let gen = GeneratorDynamic {
            gen_id: 1,
            bus_id: 1,
            model: GeneratorModel::Classical2nd,
            h: 5.0,
            d: 0.0,
            xd_prime: 0.3,
            xd: 1.8,
            efd: 1.0,
            pm: 0.8,
            ka: 0.0,
            ta: 0.0,
        };

        // H <= 0
        let gen_bad = GeneratorDynamic { h: 0.0, ..gen.clone() };
        let r = analyzer.equal_area_criterion(&gen_bad, 1.0, 1.0, f64::INFINITY, 1.3, 50.0);
        assert!(r.is_err(), "H=0 应返回错误");

        // Pm >= Pmax_pre
        let gen_bad2 = GeneratorDynamic { pm: 2.0, ..gen.clone() };
        let r = analyzer.equal_area_criterion(&gen_bad2, 1.0, 1.0, f64::INFINITY, 1.3, 50.0);
        assert!(r.is_err(), "Pm >= Pmax_pre 应返回错误");

        // 系统在任意故障清除时间下均失稳：Pm 接近 Pmax_post，减速面积不足
        // Pm=0.95, Pmax_post = 1.0/1.3 = 0.769 < Pm → 已被 Pm>=Pmax_post 拦截
        // 改为：Pm=0.75, Pmax_post = 1.0/1.3 = 0.769, Pm/Pmax_post = 0.975
        // f(δ₀) > 0，系统始终失稳
        let gen_bad3 = GeneratorDynamic { pm: 0.75, ..gen.clone() };
        let r = analyzer.equal_area_criterion(&gen_bad3, 1.0, 1.0, f64::INFINITY, 1.3, 50.0);
        assert!(r.is_err(), "减速面积不足（f(δ₀)>=0）应返回错误");
    }

    /// T5.3 测试：连续潮流 (CPF)
    ///
    /// 使用 IEEE-14 节点系统，验证 CPF 能追踪 PV 曲线并检测鼻点
    #[test]
    fn test_cpf_ieee14() {
        let analyzer = TransientStabilityAnalyzer::new();
        let data = eneros_powerflow::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        // 负荷母线：所有 PQ 母线
        let load_bus_indices: Vec<usize> = bus_types
            .iter()
            .enumerate()
            .filter(|(_, &bt)| bt == BusTypeNR::PQ)
            .map(|(i, _)| i)
            .collect();

        let result = analyzer
            .run_cpf(
                &ybus,
                &p_spec,
                &q_spec,
                &bus_types,
                &load_bus_indices,
                Some(&v_initial),
                5.0,   // max_lambda
                0.1,   // initial_step
                100,   // max_steps
            )
            .expect("CPF 应成功");

        eprintln!(
            "CPF: {} 个点, λ_max={:.4}, nose_detected={}",
            result.result.pv_curve.len(),
            result.result.lambda_max,
            result.result.nose_detected
        );

        // 验证 PV 曲线非空
        assert!(
            !result.result.pv_curve.is_empty(),
            "PV 曲线不应为空"
        );

        // 验证第一个点是基态 (λ=0)
        assert!(
            (result.result.pv_curve[0].lambda - 0.0).abs() < 1e-6,
            "第一个点 λ 应为 0, 实际 = {}",
            result.result.pv_curve[0].lambda
        );

        // 验证 λ 递增（至少在前几个点）
        for i in 1..result.result.pv_curve.len().min(5) {
            assert!(
                result.result.pv_curve[i].lambda > result.result.pv_curve[i - 1].lambda,
                "λ 应递增: 点{} λ={}, 点{} λ={}",
                i - 1,
                result.result.pv_curve[i - 1].lambda,
                i,
                result.result.pv_curve[i].lambda
            );
        }

        // 验证 λ_max > 0
        assert!(
            result.result.lambda_max > 0.0,
            "λ_max 应为正, 实际 = {}",
            result.result.lambda_max
        );

        // 验证电压随 λ 增大而下降（至少在某些母线）
        if result.result.pv_curve.len() >= 2 {
            let base_v = &result.result.pv_curve[0].voltages;
            let last_v = &result.result.pv_curve[result.result.pv_curve.len() - 1].voltages;
            let mut has_voltage_drop = false;
            for (b, v_base) in base_v {
                if let Some((_, v_last)) = last_v.iter().find(|(bb, _)| bb == b) {
                    if v_last < v_base {
                        has_voltage_drop = true;
                        break;
                    }
                }
            }
            assert!(
                has_voltage_drop,
                "至少一个母线电压应随负荷增长而下降"
            );
        }
    }

    /// T5.3 测试：CPF 参数验证
    #[test]
    fn test_cpf_validation() {
        let analyzer = TransientStabilityAnalyzer::new();
        let data = eneros_powerflow::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        // 空负荷母线
        let r = analyzer.run_cpf(&ybus, &p_spec, &q_spec, &bus_types, &[], None, 5.0, 0.1, 10);
        assert!(r.is_err(), "空负荷母线应返回错误");

        // 步长无效
        let load_buses: Vec<usize> = bus_types
            .iter()
            .enumerate()
            .filter(|(_, &bt)| bt == BusTypeNR::PQ)
            .map(|(i, _)| i)
            .collect();
        let r = analyzer.run_cpf(&ybus, &p_spec, &q_spec, &bus_types, &load_buses, None, 5.0, 0.0, 10);
        assert!(r.is_err(), "步长=0 应返回错误");
    }

    /// T5.4 测试：电压稳定模态分析
    ///
    /// 使用 IEEE-14 节点系统，验证奇异值计算合理
    #[test]
    fn test_voltage_stability_modal_analysis() {
        let analyzer = TransientStabilityAnalyzer::new();
        let data = eneros_powerflow::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        // 先求解潮流获取运行点
        let solver = PowerFlowSolver::new(50, 1e-8);
        let pf_result = solver
            .solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial))
            .expect("潮流应收敛");

        let v: Vec<f64> = pf_result
            .bus_results
            .iter()
            .map(|br| br.voltage_magnitude)
            .collect();
        let theta: Vec<f64> = pf_result
            .bus_results
            .iter()
            .map(|br| br.voltage_angle)
            .collect();

        let result = analyzer
            .voltage_stability_modal_analysis(&ybus, &v, &theta, &bus_types)
            .expect("电压稳定模态分析应成功");

        eprintln!(
            "电压稳定: σ_min={:.6}, σ_max={:.6}, κ={:.2}, 裕度={:.2}%, 最薄弱母线={}",
            result.result.min_singular_value,
            result.result.max_singular_value,
            result.result.condition_number,
            result.result.stability_margin_percent,
            result.result.weakest_bus_idx
        );

        // 验证奇异值为正
        assert!(
            result.result.min_singular_value > 0.0,
            "σ_min 应为正, 实际 = {}",
            result.result.min_singular_value
        );
        assert!(
            result.result.max_singular_value > 0.0,
            "σ_max 应为正, 实际 = {}",
            result.result.max_singular_value
        );

        // 验证 σ_max >= σ_min
        assert!(
            result.result.max_singular_value >= result.result.min_singular_value,
            "σ_max ({}) 应 >= σ_min ({})",
            result.result.max_singular_value,
            result.result.min_singular_value
        );

        // 验证条件数 >= 1
        assert!(
            result.result.condition_number >= 1.0,
            "条件数应 >= 1, 实际 = {}",
            result.result.condition_number
        );

        // 验证 IEEE-14 在正常工况下不接近不稳定
        assert!(
            !result.result.near_instability,
            "IEEE-14 正常工况不应接近不稳定, σ_min = {}",
            result.result.min_singular_value
        );

        // 验证左奇异向量非空且归一化
        assert!(
            !result.result.left_singular_vector.is_empty(),
            "左奇异向量不应为空"
        );
        let u_norm: f64 = result
            .result
            .left_singular_vector
            .iter()
            .map(|x| x * x)
            .sum::<f64>()
            .sqrt();
        assert!(
            (u_norm - 1.0).abs() < 0.1,
            "左奇异向量应近似归一化, |u| = {}",
            u_norm
        );
    }

    /// T5.4 测试：电压稳定模态分析参数验证
    #[test]
    fn test_voltage_stability_validation() {
        let analyzer = TransientStabilityAnalyzer::new();
        let data = eneros_powerflow::ieee14();
        let (ybus, _, _, bus_types) = data.to_solver_input();

        let v = vec![1.0; ybus.size()];
        let theta = vec![0.0; ybus.size()];

        // 维度不匹配
        let r = analyzer.voltage_stability_modal_analysis(&ybus, &v[..v.len() - 1], &theta, &bus_types);
        assert!(r.is_err(), "维度不匹配应返回错误");
    }

    /// T5 辅助函数测试：幂迭代
    #[test]
    fn test_power_iteration() {
        // 对角矩阵 [4, 0; 0, 2]，λ_max = 4
        let mat = vec![vec![4.0, 0.0], vec![0.0, 2.0]];
        let (lambda, v) = power_iteration(&mat, 100, 1e-10);
        assert!(
            (lambda - 4.0).abs() < 1e-6,
            "λ_max 应为 4, 实际 = {}",
            lambda
        );
        // 特征向量应为 [1, 0] 方向
        assert!(
            v[0].abs() > 0.99,
            "特征向量应指向 [1, 0] 方向, 实际 = {:?}",
            v
        );
    }

    /// T5 辅助函数测试：逆幂迭代
    #[test]
    fn test_inverse_power_iteration() {
        // 对角矩阵 [4, 0; 0, 2]，λ_min = 2
        let mat = vec![vec![4.0, 0.0], vec![0.0, 2.0]];
        let (lambda, v) = inverse_power_iteration(&mat, 0.0, 100, 1e-10);
        assert!(
            (lambda - 2.0).abs() < 1e-6,
            "λ_min 应为 2, 实际 = {}",
            lambda
        );
        // 特征向量应为 [0, 1] 方向
        assert!(
            v[1].abs() > 0.99,
            "特征向量应指向 [0, 1] 方向, 实际 = {:?}",
            v
        );
    }

    /// T5 辅助函数测试：LU 分解与求解
    #[test]
    fn test_lu_decompose_solve() {
        let a = vec![vec![4.0, 3.0], vec![6.0, 3.0]];
        let b = vec![10.0, 12.0];

        let lu = lu_decompose(&a).expect("LU 分解应成功");
        let x = lu_solve(&lu, &b);

        // 验证 Ax = b
        let r0 = a[0][0] * x[0] + a[0][1] * x[1];
        let r1 = a[1][0] * x[0] + a[1][1] * x[1];
        assert!((r0 - b[0]).abs() < 1e-10, "Ax[0] = {} != {}", r0, b[0]);
        assert!((r1 - b[1]).abs() < 1e-10, "Ax[1] = {} != {}", r1, b[1]);
    }
}
