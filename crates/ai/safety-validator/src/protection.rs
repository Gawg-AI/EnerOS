//! 保护配合校验规则（priority=20）.
//!
//! 校验相邻时段功率变化率 ≤ `max_ramp_rate`（kW/min）。
//! 超限时按蓝图截断逻辑（D11：精确复制）调整 discharge/charge_power_kw。
//!
//! D12：保留 `overcurrent_threshold` / `overvoltage_threshold` /
//!      `undervoltage_threshold` / `freq_protection` 字段供未来扩展。

use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleResult;

use crate::result::{Severity, ValidationResult, Violation};
use crate::rule::SafetyRule;
use crate::state::SystemState;

/// 保护配合校验规则.
pub struct ProtectionCoordinationRule {
    overcurrent_threshold: f64,
    overvoltage_threshold: f64,
    undervoltage_threshold: f64,
    freq_protection: (f64, f64),
    max_ramp_rate: f64,
}

impl ProtectionCoordinationRule {
    /// 构造保护配合校验规则.
    ///
    /// - `overcurrent_threshold`：过流阈值（A，D12：当前未使用）。
    /// - `overvoltage_threshold`：过压阈值（V，D12：当前未使用）。
    /// - `undervoltage_threshold`：欠压阈值（V，D12：当前未使用）。
    /// - `freq_protection`：频率保护范围 `(min, max)`（D12：当前未使用）。
    /// - `max_ramp_rate`：最大爬坡率（kW/min）。
    pub fn new(
        overcurrent_threshold: f64,
        overvoltage_threshold: f64,
        undervoltage_threshold: f64,
        freq_protection: (f64, f64),
        max_ramp_rate: f64,
    ) -> Self {
        Self {
            overcurrent_threshold,
            overvoltage_threshold,
            undervoltage_threshold,
            freq_protection,
            max_ramp_rate,
        }
    }
}

impl SafetyRule for ProtectionCoordinationRule {
    fn name(&self) -> &str {
        "protection_coordination"
    }

    fn priority(&self) -> u32 {
        20
    }

    fn is_hard(&self) -> bool {
        true
    }

    fn validate(&self, schedule: &ScheduleResult, _state: &SystemState) -> ValidationResult {
        let mut violations = Vec::new();
        let mut clamped_schedule = schedule.clone();
        let mut clamped = false;

        // 校验功率变化率（爬坡率）
        for i in 1..clamped_schedule.schedule.len() {
            let prev = clamped_schedule.schedule[i - 1].net_power_kw;
            let curr = clamped_schedule.schedule[i].net_power_kw;
            let delta = (curr - prev).abs();
            // 转换为 kW/min（15min 时段 → 0.25h → 0.25 min ÷ 60 ... 蓝图按 0.25 系数）
            let delta_per_min = delta / 0.25; // 15min 时段
            if delta_per_min > self.max_ramp_rate {
                // D11：精确复制蓝图截断逻辑
                let safe_delta = self.max_ramp_rate * 0.25 * curr.signum();
                let safe_value = prev + safe_delta;
                violations.push(Violation {
                    rule: self.name().to_string(),
                    period: i,
                    field: "ramp_rate".to_string(),
                    original_value: curr,
                    safe_value,
                    severity: Severity::Critical,
                });
                // 截断功率变化（D11：PRESERVE EXACTLY — 不"修复"符号逻辑）
                let diff = safe_value - curr;
                clamped_schedule.schedule[i].discharge_power_kw += diff.max(0.0);
                clamped_schedule.schedule[i].charge_power_kw += (-diff).max(0.0);
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
