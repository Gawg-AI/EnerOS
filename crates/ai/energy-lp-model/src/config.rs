//! 调度参数配置（D10）.

use alloc::vec::Vec;

/// 储能调度参数配置.
#[derive(Debug, Clone)]
pub struct ScheduleConfig {
    /// 调度时段数（如 24 小时 = 96 个 15min 时段）.
    pub num_periods: usize,
    /// 时段时长（小时，如 0.25 = 15min）.
    pub period_hours: f64,
    /// PCS 额定功率（kW）.
    pub pcs_power_kw: f64,
    /// 电池容量（kWh）.
    pub battery_capacity_kwh: f64,
    /// SOC 下限（0.0~1.0）.
    pub soc_min: f64,
    /// SOC 上限（0.0~1.0）.
    pub soc_max: f64,
    /// 初始 SOC（0.0~1.0）.
    pub soc_init: f64,
    /// 终值 SOC（可选，None = 不约束终值）.
    pub soc_final: Option<f64>,
    /// 充电爬坡率限制（kW/时段）.
    pub charge_ramp_kw: f64,
    /// 放电爬坡率限制（kW/时段）.
    pub discharge_ramp_kw: f64,
    /// 充电效率（0.0~1.0）.
    pub charge_efficiency: f64,
    /// 放电效率（0.0~1.0）.
    pub discharge_efficiency: f64,
    /// 电价曲线（元/kWh，长度 = num_periods）.
    pub price: Vec<f64>,
    /// 负荷需求曲线（kW，长度 = num_periods，可选）.
    pub load_demand: Option<Vec<f64>>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            num_periods: 96,
            period_hours: 0.25,
            pcs_power_kw: 100.0,
            battery_capacity_kwh: 200.0,
            soc_min: 0.1,
            soc_max: 0.9,
            soc_init: 0.5,
            soc_final: None,
            charge_ramp_kw: 50.0,
            discharge_ramp_kw: 50.0,
            charge_efficiency: 0.95,
            discharge_efficiency: 0.95,
            price: alloc::vec![0.5; 96],
            load_demand: None,
        }
    }
}
