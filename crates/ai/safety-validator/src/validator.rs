//! `SafetyValidator` 主接口（规则链式执行 + 致命违规终止）.
//!
//! D4：使用 `alloc::vec::Vec` + `alloc::boxed::Box`。
//! D5：`Vec::sort_by_key` 排序规则（`alloc` 原生支持）。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleResult;

use crate::electrical::ElectricalSafetyRule;
use crate::protection::ProtectionCoordinationRule;
use crate::result::{Severity, ValidationResult, Violation};
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

    /// 空校验器（SPEC WP-A.2）：生产决策咽喉接线用——由调用方按装配配置
    /// 显式注册参数化规则，不隐含蓝图演示默认值。
    #[must_use]
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
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

        // 61 v0.314.25 §3.4：`is_hard()==true` 规则的任何 Critical/Fatal
        // 都拒绝（旧语义「只有 Fatal 才 passed=false」退役——状态过流等
        // Critical 不能带着 passed=true 放行）。
        let hard_rules: alloc::vec::Vec<&str> = self
            .rules
            .iter()
            .filter(|r| r.is_hard())
            .map(|r| r.name())
            .collect();
        let has_hard_reject = all_violations.iter().any(|v| {
            v.severity == Severity::Fatal
                || (v.severity == Severity::Critical && hard_rules.contains(&v.rule.as_str()))
        });

        // 31-review（P1 AI-05）：后置校验（规则链之后、最终调度上）——同一时段
        // 不得同时充电与放电。LP 线性互斥允许部分同时（60+40≤100），蓝图要求
        // 二元变量（MIP，已入模型）或 safety-validator 后置拒绝；此处为纵深
        // 防御：任何来源（解算器/规则 clamp）的同时充放一律 Fatal。
        const SIMUL_EPS: f64 = 1e-6;
        for entry in current_schedule.schedule.iter() {
            if entry.charge_power_kw > SIMUL_EPS && entry.discharge_power_kw > SIMUL_EPS {
                all_violations.push(Violation::normal(
                    "charge_discharge_mutex",
                    entry.period,
                    "charge_power_kw",
                    entry.charge_power_kw,
                    0.0,
                    Severity::Fatal,
                ));
            }
        }

        ValidationResult {
            passed: all_violations.is_empty() || !has_hard_reject,
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
