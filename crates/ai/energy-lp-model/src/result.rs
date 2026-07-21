//! 调度结果类型（D8/D10）.

use alloc::vec::Vec;

use eneros_solver_core::result::SolveStatus;

/// 调度结果条目.
#[derive(Debug, Clone)]
pub struct ScheduleEntry {
    /// 时段索引.
    pub period: usize,
    /// 充电功率（kW）.
    pub charge_power_kw: f64,
    /// 放电功率（kW）.
    pub discharge_power_kw: f64,
    /// 净功率（kW，discharge - charge）.
    pub net_power_kw: f64,
    /// SOC 百分比（0.0~1.0）.
    pub soc_pct: f64,
    /// 收益（元）.
    pub revenue_yuan: f64,
}

/// 调度结果.
#[derive(Debug, Clone)]
pub struct ScheduleResult {
    /// 各时段调度方案.
    pub schedule: Vec<ScheduleEntry>,
    /// 总收益（元）.
    pub total_revenue_yuan: f64,
    /// 目标函数值.
    pub objective_value: f64,
    /// 求解状态.
    pub solve_status: SolveStatus,
}
