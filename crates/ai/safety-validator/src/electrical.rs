//! 电气安全校验规则（priority=10）.
//!
//! 校验充电功率 / 放电功率 ≤ `max_power_kw`、SOC ≤ 0.95、SOC ≥ 0.05；
//! 超限时截断到安全值。SOC < 0.05 触发 `Fatal` 立即终止。
//!
//! D11：截断逻辑精确复制蓝图，不修改 `net_power_kw` / `revenue_yuan`。
//! D12：保留 `max_current_a` / `voltage_range` / `freq_range` 字段供未来扩展。

use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleResult;

use crate::result::{Severity, ValidationResult, Violation};
use crate::rule::SafetyRule;
use crate::state::SystemState;

/// 电气安全校验规则.
///
/// 字段全部保留（D12），`new()` 暴露全部参数供未来扩展使用。
pub struct ElectricalSafetyRule {
    max_power_kw: f64,
    max_current_a: f64,
    voltage_range: (f64, f64),
    freq_range: (f64, f64),
}

impl ElectricalSafetyRule {
    /// 构造电气安全校验规则.
    ///
    /// - `max_power_kw`：充放电功率上限（kW）。
    /// - `max_current_a`：电流上限（A，D12：当前未使用，预留扩展）。
    /// - `voltage_range`：电压范围 `(min, max)`（D12：当前未使用，预留扩展）。
    /// - `freq_range`：频率范围 `(min, max)`（D12：当前未使用，预留扩展）。
    pub fn new(
        max_power_kw: f64,
        max_current_a: f64,
        voltage_range: (f64, f64),
        freq_range: (f64, f64),
    ) -> Self {
        Self {
            max_power_kw,
            max_current_a,
            voltage_range,
            freq_range,
        }
    }
}

impl SafetyRule for ElectricalSafetyRule {
    fn name(&self) -> &str {
        "electrical_safety"
    }

    fn priority(&self) -> u32 {
        10
    }

    fn is_hard(&self) -> bool {
        true
    }

    fn validate(&self, schedule: &ScheduleResult, _state: &SystemState) -> ValidationResult {
        let mut violations = Vec::new();
        let mut clamped_schedule = schedule.clone();
        let mut clamped = false;

        for entry in &mut clamped_schedule.schedule {
            // 校验充电功率
            if entry.charge_power_kw > self.max_power_kw {
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: entry.period,
                    field: "charge_power".to_string(),
                    original_value: entry.charge_power_kw,
                    safe_value: self.max_power_kw,
                    severity: Severity::Critical,
                });
                entry.charge_power_kw = self.max_power_kw;
                clamped = true;
            }
            // 校验放电功率
            if entry.discharge_power_kw > self.max_power_kw {
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: entry.period,
                    field: "discharge_power".to_string(),
                    original_value: entry.discharge_power_kw,
                    safe_value: self.max_power_kw,
                    severity: Severity::Critical,
                });
                entry.discharge_power_kw = self.max_power_kw;
                clamped = true;
            }
            // 校验 SOC 上限
            if entry.soc_pct > 0.95 {
                let safe = 0.95;
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: entry.period,
                    field: "soc".to_string(),
                    original_value: entry.soc_pct,
                    safe_value: safe,
                    severity: Severity::Critical,
                });
                entry.soc_pct = safe;
                clamped = true;
            }
            // 校验 SOC 下限（Fatal）
            if entry.soc_pct < 0.05 {
                let safe = 0.05;
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: entry.period,
                    field: "soc".to_string(),
                    original_value: entry.soc_pct,
                    safe_value: safe,
                    severity: Severity::Fatal,
                });
                entry.soc_pct = safe;
                clamped = true;
            }
        }

        let passed = violations.iter().all(|v| v.severity != Severity::Fatal);
        ValidationResult {
            passed,
            clamped,
            clamped_schedule: if clamped {
                Some(clamped_schedule)
            } else {
                None
            },
            violations,
        }
    }
}
