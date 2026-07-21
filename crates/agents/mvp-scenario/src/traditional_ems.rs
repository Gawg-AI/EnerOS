//! 传统规则 EMS 基准策略（D13：传基本类型，避免 SystemState 依赖）.

use alloc::vec::Vec;

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
use eneros_solver_core::result::SolveStatus;

/// 传统规则 EMS 基准策略.
///
/// 作为双脑（LLM + Solver）的对比基准：谷时（price < 0.3）充电，峰时
/// （price > 0.8）放电，平时（0.3 ≤ price ≤ 0.8）保持。策略简单、无优化，
/// 用于验证双脑链路的收益提升。
pub struct TraditionalEms {
    /// 调度参数（与双脑 EMS 共享同一配置以保证可比性）.
    pub config: ScheduleConfig,
}

impl TraditionalEms {
    /// 构造传统 EMS.
    pub fn new(config: ScheduleConfig) -> Self {
        Self { config }
    }

    /// 生成调度方案.
    ///
    /// 遍历 `config.num_periods` 时段，按谷充峰放规则生成 `ScheduleEntry`。
    /// 参数 `current_price` 与 `soc` 为占位（D13：传基本类型），实际规则使用
    /// `config.price[t]` 判断每个时段的电价档位。
    pub fn schedule(&self, _current_price: f64, soc: f64) -> ScheduleResult {
        let mut entries = Vec::with_capacity(self.config.num_periods);
        for t in 0..self.config.num_periods {
            let price = self.config.price[t];
            let (charge, discharge) = if price < 0.3 {
                // 谷时充电.
                (self.config.pcs_power_kw, 0.0)
            } else if price > 0.8 {
                // 峰时放电.
                (0.0, self.config.pcs_power_kw)
            } else {
                // 平时保持.
                (0.0, 0.0)
            };
            entries.push(ScheduleEntry {
                period: t,
                charge_power_kw: charge,
                discharge_power_kw: discharge,
                net_power_kw: discharge - charge,
                soc_pct: soc,
                revenue_yuan: (discharge - charge) * price * self.config.period_hours,
            });
        }
        let total: f64 = entries.iter().map(|e| e.revenue_yuan).sum();
        ScheduleResult {
            schedule: entries,
            total_revenue_yuan: total,
            objective_value: total,
            solve_status: SolveStatus::Optimal,
        }
    }
}
