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

use crate::result::{Severity, ValidationResult, Violation, ViolationReason};
use crate::rule::SafetyRule;
use crate::state::SystemState;

/// 电气安全校验规则.
///
/// WP-26.6：电气保护量（电流/电压/频率）已启用（P1-4/P1-5）——
/// `max_current_a` / `voltage_range` / `freq_range` 接入判定链；
/// 所有电气量输入先做 `is_finite` 前置拒绝（NaN/±Inf → `NonFinite` 结构化
/// 原因，证伪旧 `NaN > limit == false` 静默放行）。
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

    fn validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult {
        let mut violations = Vec::new();
        let mut clamped_schedule = schedule.clone();
        let mut clamped = false;

        // WP-26.6：电气量 finite 前置拒绝（P1-5）——任一输入非有限即拒绝，
        // 不参与后续越限比较（NaN 与任何数比较均为 false，旧行为静默放行）。
        for (field, v) in [
            ("state.voltage_v", state.voltage_v),
            ("state.current_a", state.current_a),
            ("state.frequency_hz", state.frequency_hz),
        ] {
            if !v.is_finite() {
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: 0,
                    field: field.to_string(),
                    original_value: v,
                    safe_value: 0.0,
                    severity: Severity::Fatal,
                    reason: ViolationReason::NonFinite(field),
                });
            }
        }

        for entry in &mut clamped_schedule.schedule {
            // WP-26.6：调度量 finite 前置拒绝。
            for (field, v) in [
                ("charge_power", entry.charge_power_kw),
                ("discharge_power", entry.discharge_power_kw),
                ("soc", entry.soc_pct),
            ] {
                if !v.is_finite() {
                    violations.push(Violation {
                        rule: self.name().to_string(),
                        period: entry.period,
                        field: field.to_string(),
                        original_value: v,
                        safe_value: 0.0,
                        severity: Severity::Fatal,
                        reason: ViolationReason::NonFinite(field),
                    });
                }
            }
            // 校验充电功率
            if entry.charge_power_kw > self.max_power_kw {
                violations.push(Violation::normal(
                    self.name(),
                    entry.period,
                    "charge_power",
                    entry.charge_power_kw,
                    self.max_power_kw,
                    Severity::Critical,
                ));
                entry.charge_power_kw = self.max_power_kw;
                clamped = true;
            }
            // 校验放电功率
            if entry.discharge_power_kw > self.max_power_kw {
                violations.push(Violation::normal(
                    self.name(),
                    entry.period,
                    "discharge_power",
                    entry.discharge_power_kw,
                    self.max_power_kw,
                    Severity::Critical,
                ));
                entry.discharge_power_kw = self.max_power_kw;
                clamped = true;
            }
            // 校验 SOC 上限
            if entry.soc_pct > 0.95 {
                let safe = 0.95;
                violations.push(Violation::normal(
                    self.name(),
                    entry.period,
                    "soc",
                    entry.soc_pct,
                    safe,
                    Severity::Critical,
                ));
                entry.soc_pct = safe;
                clamped = true;
            }
            // 校验 SOC 下限（Fatal）
            if entry.soc_pct < 0.05 {
                let safe = 0.05;
                violations.push(Violation::normal(
                    self.name(),
                    entry.period,
                    "soc",
                    entry.soc_pct,
                    safe,
                    Severity::Fatal,
                ));
                entry.soc_pct = safe;
                clamped = true;
            }
        }

        // WP-26.6：电气量越限（P1-4 启用）——状态采集值超阈值 → 结构化拒绝。
        if state.current_a > self.max_current_a {
            violations.push(Violation {
                rule: self.name().to_string(),
                period: 0,
                field: "current".to_string(),
                original_value: state.current_a,
                safe_value: self.max_current_a,
                severity: Severity::Critical,
                reason: ViolationReason::OverCurrent,
            });
        }
        let (vmin, vmax) = self.voltage_range;
        if state.voltage_v > vmax {
            violations.push(Violation {
                rule: self.name().to_string(),
                period: 0,
                field: "voltage".to_string(),
                original_value: state.voltage_v,
                safe_value: vmax,
                severity: Severity::Critical,
                reason: ViolationReason::OverVoltage,
            });
        } else if state.voltage_v < vmin {
            violations.push(Violation {
                rule: self.name().to_string(),
                period: 0,
                field: "voltage".to_string(),
                original_value: state.voltage_v,
                safe_value: vmin,
                severity: Severity::Critical,
                reason: ViolationReason::UnderVoltage,
            });
        }
        let (fmin, fmax) = self.freq_range;
        if state.frequency_hz > fmax || state.frequency_hz < fmin {
            violations.push(Violation {
                rule: self.name().to_string(),
                period: 0,
                field: "frequency".to_string(),
                original_value: state.frequency_hz,
                safe_value: if state.frequency_hz > fmax {
                    fmax
                } else {
                    fmin
                },
                severity: Severity::Critical,
                reason: ViolationReason::FreqOutOfRange,
            });
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
