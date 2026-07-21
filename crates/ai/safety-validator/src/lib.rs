//! EnerOS 安全校验器（v0.67.0，P1-J Solver 第四层安全屏障）.
//!
//! 对 LP 求解结果（`ScheduleResult`）执行三重校验：
//! - 电气安全校验（功率/SOC 范围）
//! - 保护配合校验（爬坡率/不触发保护）
//! - 约束包一致性（与 v0.56.0 ConstraintChecker 形成 double barrier）
//!
//! 校验失败时**截断到安全边界**而非拒绝，保证系统始终有输出。
//! 致命违规（Fatal）立即终止后续规则并返回 `passed: false`。
//!
//! # 核心类型
//!
//! - [`rule::SafetyRule`] — 校验规则抽象 trait（D1：无 Send + Sync）
//! - [`validator::SafetyValidator`] — 校验器主接口（链式执行 + 致命终止）
//! - [`electrical::ElectricalSafetyRule`] — 电气安全校验规则（priority=10）
//! - [`protection::ProtectionCoordinationRule`] — 保护配合校验规则（priority=20）
//! - [`result::ValidationResult`] / [`result::Violation`] / [`result::Severity`] — 校验结果
//! - [`state::SystemState`] — 最小系统状态（D2：本地定义）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `pub trait SafetyRule: Send + Sync` | **移除 Send + Sync** | no_std 单线程；与 v0.59.0/v0.63.0/v0.64.0 一致 |
//! | **D2** | 蓝图使用 `SystemState` 但未定义 | **本地定义最小 SystemState** | HMI crate 的 SystemState 是显示状态，与电气校验无关；Karpathy "Simplicity First" |
//! | **D3** | `rule: self.name().into()` | 保留 `.to_string()` / `String::from` | `extern crate alloc` 后可用 |
//! | **D4** | `Vec<Box<dyn SafetyRule>>` | 使用 `alloc::vec::Vec` + `alloc::boxed::Box` | no_std 合规 |
//! | **D5** | `self.rules.sort_by_key(...)` | 保留 `Vec::sort_by_key` | `alloc` 原生支持 |
//! | **D6** | 前置依赖 v0.56.0/v0.57.0/v0.52.0 | **不引入这 3 个 crate** | 仅依赖 v0.66.0 ScheduleResult + 本地 SystemState；解耦 |
//! | **D7** | 蓝图未声明 `[features]` | 不声明 `[features]` | 纯 Rust，无 FFI |
//! | **D8** | 蓝图 `Severity` 派生 Debug+Clone+Copy+PartialEq | 保持一致 | `PartialEq` 需用于 `==` 比较，`Copy` 因简单枚举 |
//! | **D9** | 蓝图 ValidationResult/Violation 派生 Debug+Clone | 保持一致，不派生 PartialEq | Karpathy "Simplicity First"：当前测试不需要 |
//! | **D10** | 蓝图 `if soc_pct > 0.95` 直接比较 | 保留直接比较 | SOC 边界 0.95/0.05 是固定阈值，非迭代结果，安全 |
//! | **D11** | 蓝图截断逻辑 `discharge += diff.max(0.0)` | 保留蓝图截断逻辑（精确复制） | Karpathy "Surgical Changes"：不修改业务逻辑 |
//! | **D12** | 蓝图 ElectricalSafetyRule 有未使用字段 | 保留字段（未来扩展） | Karpathy "Surgical Changes"：不删除预留字段 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod electrical;
pub mod protection;
pub mod result;
pub mod rule;
pub mod state;
pub mod validator;

pub use electrical::ElectricalSafetyRule;
pub use protection::ProtectionCoordinationRule;
pub use result::{Severity, ValidationResult, Violation};
pub use rule::SafetyRule;
pub use state::SystemState;
pub use validator::SafetyValidator;

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
    use eneros_solver_core::result::SolveStatus;

    use super::*;

    fn make_entry(period: usize, charge: f64, discharge: f64, soc: f64) -> ScheduleEntry {
        ScheduleEntry {
            period,
            charge_power_kw: charge,
            discharge_power_kw: discharge,
            net_power_kw: discharge - charge,
            soc_pct: soc,
            revenue_yuan: 0.0,
        }
    }

    fn make_schedule(entries: &[ScheduleEntry]) -> ScheduleResult {
        ScheduleResult {
            schedule: entries.to_vec(),
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: SolveStatus::Optimal,
        }
    }

    // T1: SystemState::default 默认值（D2）
    #[test]
    fn t1_system_state_default() {
        let s = SystemState::default();
        assert!((s.voltage_v - 380.0).abs() < 1e-9);
        assert!((s.current_a - 0.0).abs() < 1e-9);
        assert!((s.frequency_hz - 50.0).abs() < 1e-9);
        assert!((s.soc_pct - 0.5).abs() < 1e-9);
        assert_eq!(s.timestamp_ms, 0);
    }

    // T2: Severity 枚举变体 + PartialEq（D8）
    #[test]
    fn t2_severity_variants_and_partial_eq() {
        assert_eq!(Severity::Critical, Severity::Critical);
        assert_ne!(Severity::Critical, Severity::Fatal);
        // 验证 4 个变体均可构造
        let _ = Severity::Info;
        let _ = Severity::Warning;
        let _ = Severity::Critical;
        let _ = Severity::Fatal;
    }

    // T3: Violation 构造 + 字段访问
    #[test]
    fn t3_violation_construction() {
        let v = Violation {
            rule: "electrical_safety".to_string(),
            period: 2,
            field: "charge_power".to_string(),
            original_value: 120.0,
            safe_value: 100.0,
            severity: Severity::Critical,
        };
        assert_eq!(v.rule, "electrical_safety");
        assert_eq!(v.period, 2);
        assert_eq!(v.field, "charge_power");
        assert!((v.original_value - 120.0).abs() < 1e-9);
        assert!((v.safe_value - 100.0).abs() < 1e-9);
        assert_eq!(v.severity, Severity::Critical);
    }

    // T4: ValidationResult 构造（passed=true, clamped=false）
    #[test]
    fn t4_validation_result_construction() {
        let r = ValidationResult {
            passed: true,
            clamped: false,
            clamped_schedule: None,
            violations: Vec::new(),
        };
        assert!(r.passed);
        assert!(!r.clamped);
        assert!(r.clamped_schedule.is_none());
        assert!(r.violations.is_empty());
    }

    // T5: ElectricalSafetyRule::new 构造
    #[test]
    fn t5_electrical_safety_rule_new() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        assert_eq!(rule.name(), "electrical_safety");
        assert_eq!(rule.priority(), 10);
        assert!(rule.is_hard());
    }

    // T6: ElectricalSafetyRule validate 全部通过（功率/SOC 正常）
    #[test]
    fn t6_electrical_validate_all_pass() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        let entries = [
            make_entry(0, 50.0, 50.0, 0.5),
            make_entry(1, 50.0, 50.0, 0.5),
        ];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.passed);
        assert!(!result.clamped);
        assert!(result.violations.is_empty());
        assert!(result.clamped_schedule.is_none());
    }

    // T7: 充电功率超限截断（120→100, Critical）
    #[test]
    fn t7_electrical_charge_over_limit_clamp() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        let entries = [make_entry(0, 120.0, 0.0, 0.5)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.clamped);
        assert!(result.passed); // Critical 不影响 passed
        assert_eq!(result.violations.len(), 1);
        let v = &result.violations[0];
        assert_eq!(v.field, "charge_power");
        assert_eq!(v.severity, Severity::Critical);
        assert!((v.original_value - 120.0).abs() < 1e-9);
        assert!((v.safe_value - 100.0).abs() < 1e-9);
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[0].charge_power_kw - 100.0).abs() < 1e-9);
    }

    // T8: 放电功率超限截断（120→100, Critical）
    #[test]
    fn t8_electrical_discharge_over_limit_clamp() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        let entries = [make_entry(0, 0.0, 120.0, 0.5)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.clamped);
        assert_eq!(result.violations.len(), 1);
        let v = &result.violations[0];
        assert_eq!(v.field, "discharge_power");
        assert_eq!(v.severity, Severity::Critical);
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[0].discharge_power_kw - 100.0).abs() < 1e-9);
    }

    // T9: SOC 上限截断（0.98→0.95, Critical）
    #[test]
    fn t9_electrical_soc_upper_clamp() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        let entries = [make_entry(0, 50.0, 0.0, 0.98)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.clamped);
        assert!(result.passed);
        let v = &result.violations[0];
        assert_eq!(v.field, "soc");
        assert_eq!(v.severity, Severity::Critical);
        assert!((v.safe_value - 0.95).abs() < 1e-9);
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[0].soc_pct - 0.95).abs() < 1e-9);
    }

    // T10: SOC 下限致命（0.03→0.05, Fatal, passed=false）
    #[test]
    fn t10_electrical_soc_lower_fatal() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        let entries = [make_entry(0, 50.0, 0.0, 0.03)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(!result.passed);
        assert!(result.clamped);
        let v = &result.violations[0];
        assert_eq!(v.field, "soc");
        assert_eq!(v.severity, Severity::Fatal);
        assert!((v.safe_value - 0.05).abs() < 1e-9);
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[0].soc_pct - 0.05).abs() < 1e-9);
    }

    // T11: ProtectionCoordinationRule::new 构造
    #[test]
    fn t11_protection_rule_new() {
        let rule = ProtectionCoordinationRule::new(220.0, 440.0, 320.0, (49.0, 51.0), 200.0);
        assert_eq!(rule.name(), "protection_coordination");
        assert_eq!(rule.priority(), 20);
        assert!(rule.is_hard());
    }

    // T12: 爬坡率正常（无 violation）
    #[test]
    fn t12_protection_ramp_normal() {
        let rule = ProtectionCoordinationRule::new(220.0, 440.0, 320.0, (49.0, 51.0), 200.0);
        // 相邻时段 net 功率差 25kW，delta_per_min = 25/0.25 = 100 < 200
        let entries = [make_entry(0, 0.0, 50.0, 0.5), make_entry(1, 0.0, 75.0, 0.5)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.passed);
        assert!(!result.clamped);
        assert!(result.violations.is_empty());
    }

    // T13: 爬坡率超限截断（Critical）
    #[test]
    fn t13_protection_ramp_over_limit_clamp() {
        let rule = ProtectionCoordinationRule::new(220.0, 440.0, 320.0, (49.0, 51.0), 200.0);
        // prev net=0, curr net=100, delta=100, delta_per_min = 100/0.25 = 400 > 200
        let entries = [make_entry(0, 0.0, 0.0, 0.5), make_entry(1, 0.0, 100.0, 0.5)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        assert!(result.clamped);
        assert!(result.passed); // Critical 不影响 passed
        assert_eq!(result.violations.len(), 1);
        let v = &result.violations[0];
        assert_eq!(v.field, "ramp_rate");
        assert_eq!(v.severity, Severity::Critical);
        assert_eq!(v.period, 1);
        // safe_value = prev + max_ramp_rate * 0.25 * curr.signum() = 0 + 200*0.25*1 = 50
        assert!((v.safe_value - 50.0).abs() < 1e-9);
        assert!(result.clamped_schedule.is_some());
    }

    // T14: SafetyValidator::new 默认注册 2 条规则
    #[test]
    fn t14_validator_new_has_two_rules() {
        let v = SafetyValidator::new();
        assert_eq!(v.rules.len(), 2);
        // priority=10 在前
        assert_eq!(v.rules[0].name(), "electrical_safety");
        assert_eq!(v.rules[1].name(), "protection_coordination");
    }

    // T15: SafetyValidator validate 全部通过
    #[test]
    fn t15_validator_validate_all_pass() {
        let v = SafetyValidator::new();
        let entries = [
            make_entry(0, 50.0, 50.0, 0.5),
            make_entry(1, 50.0, 75.0, 0.5),
        ];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = v.validate(&sched, &state);
        assert!(result.passed);
        assert!(!result.clamped);
        assert!(result.violations.is_empty());
        assert!(result.clamped_schedule.is_none());
    }

    // T16: 链式截断（Electrical 截断后 Protection 继续校验）
    #[test]
    fn t16_validator_chain_clamp() {
        let v = SafetyValidator::new();
        // 充电超限 + 爬坡超限：electrical 先截断 charge，protection 再基于截断后 schedule 校验
        // 注：D11 下 electrical 不重算 net_power_kw，protection 用原始 net
        let entries = [
            make_entry(0, 50.0, 0.0, 0.5),
            make_entry(1, 120.0, 0.0, 0.5), // charge > 100，net=-120
        ];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = v.validate(&sched, &state);
        assert!(result.clamped);
        // 应同时包含 electrical（charge_power）+ protection（ramp_rate）违规
        let has_electrical = result
            .violations
            .iter()
            .any(|v| v.rule == "electrical_safety");
        let has_protection = result
            .violations
            .iter()
            .any(|v| v.rule == "protection_coordination");
        assert!(has_electrical, "should have electrical violation");
        assert!(has_protection, "should have protection violation");
    }

    // T17: SOC < 0.05 Fatal 立即终止（passed=false, 仅有 electrical 违规）
    #[test]
    fn t17_validator_fatal_terminates_early() {
        let v = SafetyValidator::new();
        // 2 entries：第一个 SOC=0.03 触发 Fatal；第二个 net 与第一个差异巨大，
        // 若 protection 运行必产生 ramp_rate 违规；Fatal 终止后 protection 不应运行。
        let entries = [
            make_entry(0, 50.0, 0.0, 0.03), // SOC < 0.05 → Fatal
            make_entry(1, 0.0, 200.0, 0.5), // 若 protection 运行会触发 ramp_rate
        ];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = v.validate(&sched, &state);
        assert!(!result.passed);
        // 所有 violation 应来自 electrical_safety（protection 未运行）
        for v in &result.violations {
            assert_eq!(
                v.rule, "electrical_safety",
                "protection should not have run after Fatal"
            );
        }
    }

    // T18: add_rule 自定义规则（priority=5，插队到最前）
    #[test]
    fn t18_validator_add_rule_priority_front() {
        struct CustomRule;
        impl SafetyRule for CustomRule {
            fn name(&self) -> &str {
                "custom_priority_5"
            }
            fn validate(&self, _: &ScheduleResult, _: &SystemState) -> ValidationResult {
                ValidationResult {
                    passed: true,
                    clamped: false,
                    clamped_schedule: None,
                    violations: Vec::new(),
                }
            }
            fn priority(&self) -> u32 {
                5
            }
        }

        let mut v = SafetyValidator::new();
        v.add_rule(Box::new(CustomRule));
        assert_eq!(v.rules.len(), 3);
        // priority=5 应排在最前（electrical=10, protection=20）
        assert_eq!(v.rules[0].name(), "custom_priority_5");
        assert_eq!(v.rules[1].name(), "electrical_safety");
        assert_eq!(v.rules[2].name(), "protection_coordination");
    }

    // T19: 截断后 clamped_schedule 为 Some
    #[test]
    fn t19_validator_clamped_schedule_some() {
        let v = SafetyValidator::new();
        let entries = [make_entry(0, 120.0, 0.0, 0.5)]; // charge 超限
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = v.validate(&sched, &state);
        assert!(result.clamped);
        assert!(result.clamped_schedule.is_some());
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[0].charge_power_kw - 100.0).abs() < 1e-9);
    }

    // T20: 截断后 net_power_kw NOT 重算（D11）
    // 蓝图 electrical 截断逻辑不更新 net_power_kw；保留原始值。
    #[test]
    fn t20_net_power_not_recomputed_after_clamp() {
        let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
        // 原始 net = discharge - charge = 0 - 120 = -120
        let entries = [make_entry(0, 120.0, 0.0, 0.5)];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = rule.validate(&sched, &state);
        let clamped = result.clamped_schedule.as_ref().unwrap();
        // charge 已截断为 100
        assert!((clamped.schedule[0].charge_power_kw - 100.0).abs() < 1e-9);
        assert!((clamped.schedule[0].discharge_power_kw - 0.0).abs() < 1e-9);
        // D11：net_power_kw 保留原值 -120（不重算为 -100）
        assert!(
            (clamped.schedule[0].net_power_kw - (-120.0)).abs() < 1e-9,
            "net_power_kw should NOT be recomputed (D11); got {}",
            clamped.schedule[0].net_power_kw
        );
    }

    // T21: SafetyRule trait 默认方法（priority=100, is_hard=true）
    #[test]
    fn t21_safety_rule_default_methods() {
        struct DefaultRule;
        impl SafetyRule for DefaultRule {
            fn name(&self) -> &str {
                "default_rule"
            }
            fn validate(&self, _: &ScheduleResult, _: &SystemState) -> ValidationResult {
                ValidationResult {
                    passed: true,
                    clamped: false,
                    clamped_schedule: None,
                    violations: Vec::new(),
                }
            }
        }

        let r = DefaultRule;
        assert_eq!(r.priority(), 100); // 默认
        assert!(r.is_hard()); // 默认
        assert_eq!(r.name(), "default_rule");
    }

    // T22: 端到端 — 3 entries 混合 + SafetyValidator.validate
    #[test]
    fn t22_end_to_end_mixed_schedule() {
        let v = SafetyValidator::new();
        let entries = [
            make_entry(0, 50.0, 50.0, 0.5), // 正常
            make_entry(1, 120.0, 0.0, 0.5), // 充电超限 → electrical 截断
            make_entry(2, 0.0, 50.0, 0.98), // SOC 超限 → electrical 截断（Critical）
        ];
        let sched = make_schedule(&entries);
        let state = SystemState::default();
        let result = v.validate(&sched, &state);
        // 应发生截断
        assert!(result.clamped);
        assert!(result.clamped_schedule.is_some());
        // 应有电气违规（charge_power + soc 上限）
        let electrical_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == "electrical_safety")
            .collect();
        assert!(
            !electrical_violations.is_empty(),
            "expected electrical violations"
        );
        // 无 Fatal（passed=true）
        let has_fatal = result
            .violations
            .iter()
            .any(|v| v.severity == Severity::Fatal);
        assert!(!has_fatal);
        assert!(
            result.passed,
            "passed should be true (no Fatal); violations={}",
            result.violations.len()
        );
        // 验证截断后值
        let clamped = result.clamped_schedule.as_ref().unwrap();
        assert!((clamped.schedule[1].charge_power_kw - 100.0).abs() < 1e-9);
        assert!((clamped.schedule[2].soc_pct - 0.95).abs() < 1e-9);
    }

    // 附加：Default impl 委托到 new()
    #[test]
    fn t_extra_validator_default_delegates_to_new() {
        let v = SafetyValidator::default();
        assert_eq!(v.rules.len(), 2);
    }
}
