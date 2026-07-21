//! `SafetyValidator` 主接口（规则链式执行 + 致命违规终止）.
//!
//! D4：使用 `alloc::vec::Vec` + `alloc::boxed::Box`。
//! D5：`Vec::sort_by_key` 排序规则（`alloc` 原生支持）。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleResult;

use crate::electrical::ElectricalSafetyRule;
use crate::protection::ProtectionCoordinationRule;
use crate::result::{Severity, ValidationResult};
use crate::rule::SafetyRule;
use crate::state::SystemState;

/// 校验器主接口.
///
/// `new()` 注册两条默认规则（电气安全 priority=10 + 保护配合 priority=20），
/// `validate()` 链式执行：前序规则截断后继续执行后续规则，致命违规立即终止。
pub struct SafetyValidator {
    /// 规则链（按 `priority()` 升序）。`pub(crate)` 仅供同 crate 测试访问。
    pub(crate) rules: Vec<Box<dyn SafetyRule>>,
}

impl SafetyValidator {
    /// 创建校验器并注册默认规则.
    ///
    /// 默认规则按蓝图参数：
    /// - `ElectricalSafetyRule`：max_power=100kW, max_current=200A,
    ///   voltage=(340, 420), freq=(49.5, 50.5)。
    /// - `ProtectionCoordinationRule`：overcurrent=220A, overvoltage=440V,
    ///   undervoltage=320V, freq_prot=(49, 51), max_ramp=200 kW/min。
    pub fn new() -> Self {
        let mut validator = Self { rules: Vec::new() };
        // 注册默认规则（按优先级，add_rule 内部会 sort_by_key）
        validator.add_rule(Box::new(ElectricalSafetyRule::new(
            100.0,
            200.0,
            (340.0, 420.0),
            (49.5, 50.5),
        )));
        validator.add_rule(Box::new(ProtectionCoordinationRule::new(
            220.0,
            440.0,
            320.0,
            (49.0, 51.0),
            200.0,
        )));
        validator
    }

    /// 添加规则并按 `priority()` 升序排序（D5）。
    pub fn add_rule(&mut self, rule: Box<dyn SafetyRule>) {
        self.rules.push(rule);
        self.rules.sort_by_key(|r| r.priority());
    }

    /// 链式执行所有规则.
    ///
    /// - 前序规则截断后，后续规则基于截断后的调度方案继续校验。
    /// - 任一规则触发 `Fatal` 违规时立即终止并返回 `passed: false`。
    /// - 最终 `passed = all_violations.is_empty() || 所有 violation 非 Fatal`。
    pub fn validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult {
        let mut current_schedule = schedule.clone();
        let mut all_violations = Vec::new();
        let mut any_clamped = false;

        for rule in &self.rules {
            let result = rule.validate(&current_schedule, state);
            // 在 extend 之前检测 Fatal（extend 会消费 result.violations）
            let has_fatal = result
                .violations
                .iter()
                .any(|v| v.severity == Severity::Fatal);
            all_violations.extend(result.violations);
            if result.clamped {
                if let Some(clamped) = result.clamped_schedule {
                    current_schedule = clamped;
                    any_clamped = true;
                }
            }
            // 致命违规立即终止
            if has_fatal {
                return ValidationResult {
                    passed: false,
                    clamped: any_clamped,
                    clamped_schedule: Some(current_schedule),
                    violations: all_violations,
                };
            }
        }

        ValidationResult {
            passed: all_violations.is_empty()
                || all_violations.iter().all(|v| v.severity != Severity::Fatal),
            clamped: any_clamped,
            clamped_schedule: if any_clamped {
                Some(current_schedule)
            } else {
                None
            },
            violations: all_violations,
        }
    }
}

impl Default for SafetyValidator {
    fn default() -> Self {
        Self::new()
    }
}
