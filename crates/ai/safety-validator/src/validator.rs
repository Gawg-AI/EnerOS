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
    /// - `passed` 在**全部**违规（规则链 + clamp 终态上的后置同时充放检查）
    ///   组装完成后计算：无 Fatal，且无 hard 规则产生的 Critical。
    ///   （R15-Q 订正：旧文案「passed = 非空 || 所有 violation 非 Fatal」
    ///   双错——漏 hard-rule Critical 语义，且旧实现在后置检查入列**前**
    ///   定型结论，mutex 违规拦不住 passed。）
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

        // 31-review（P1 AI-05）：后置校验（规则链之后、最终调度上）——同一时段
        // 不得同时充电与放电。LP 线性互斥允许部分同时（60+40≤100），蓝图要求
        // 二元变量（MIP，已入模型）或 safety-validator 后置拒绝；此处为纵深
        // 防御：任何来源（解算器/规则 clamp）的同时充放一律 Fatal。
        //
        // R15-Q（蓝图74-01 WP-74.1.1）：本块必须在 `has_hard_reject` 计算
        // **之前**入列——结论面只允许在违规集最终组装完成后定型（fail-closed）。
        // 顺序正确性：本检查只读 `current_schedule`（规则链 + clamp 的终态），
        // 与违规列表无交互；规则循环在两种顺序下都先于此完成；上方规则
        // Fatal 早退路径同样不受影响（早退时本块尚未执行，与原行为一致）。
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

    /// 复查 FIX（61 v0.314.25 §3.4）：clamp 建议不得直接采信——把截断结果
    /// 作为**新候选**重跑全部规则（有界 ≤4 轮：相邻对 clamp 传播需多轮收敛；
    /// 仍不干净即放弃，绝不循环放宽阈值）。
    ///
    /// 返回 `Some((schedule, validation))` = 收敛干净的截断候选；
    /// `None` = 未收敛（调用方必须回落到已通过全部规则的原始解，
    /// 不得采信半程截断）。
    pub fn revalidate_clamped(
        &self,
        first_clamped: ScheduleResult,
        state: &SystemState,
    ) -> Option<(ScheduleResult, ValidationResult)> {
        let mut current = Some(first_clamped);
        for _ in 0..4 {
            let clamped = current.clone()?;
            let r = self.validate(&clamped, state);
            if r.passed && !r.clamped {
                return Some((clamped, r));
            }
            if !r.clamped {
                return None;
            }
            current = r.clamped_schedule.clone().or(Some(clamped));
        }
        None
    }
}

impl Default for SafetyValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod revalidate_tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
    use eneros_solver_core::result::SolveStatus;

    use super::*;
    use crate::result::Violation;

    fn entry(period: usize, discharge: f64) -> ScheduleEntry {
        ScheduleEntry {
            period,
            charge_power_kw: 0.0,
            discharge_power_kw: discharge,
            net_power_kw: discharge,
            soc_pct: 0.5,
            revenue_yuan: 0.0,
        }
    }

    fn schedule(first_discharge: f64) -> ScheduleResult {
        ScheduleResult {
            schedule: alloc::vec![entry(0, first_discharge)],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: SolveStatus::Optimal,
        }
    }

    fn state() -> SystemState {
        SystemState::new(380.0, 0.0, 50.0, 0.5, 0)
    }

    /// 软 clamp 规则（is_hard=false + Warning）：首时段放电超限 → 截断并报
    /// Warning。`converge=false` 时截断为渐近逼近（永不达限）——模拟
    /// 永不收敛的 clamp 相邻传播。
    struct SoftClamp {
        limit: f64,
        converge: bool,
    }

    impl SafetyRule for SoftClamp {
        fn name(&self) -> &str {
            "soft_clamp"
        }
        fn priority(&self) -> u32 {
            5
        }
        fn is_hard(&self) -> bool {
            false
        }
        fn validate(&self, schedule: &ScheduleResult, _state: &SystemState) -> ValidationResult {
            let mut out = schedule.clone();
            let mut violations = Vec::new();
            let mut clamped = false;
            let e = &mut out.schedule[0];
            if e.discharge_power_kw > self.limit {
                violations.push(Violation::normal(
                    "soft_clamp",
                    e.period,
                    "discharge_power_kw",
                    e.discharge_power_kw,
                    self.limit,
                    Severity::Warning,
                ));
                e.discharge_power_kw = if self.converge {
                    self.limit
                } else {
                    (e.discharge_power_kw + self.limit) / 2.0
                };
                e.net_power_kw = e.discharge_power_kw - e.charge_power_kw;
                clamped = true;
            }
            ValidationResult {
                passed: true,
                clamped,
                clamped_schedule: if clamped { Some(out) } else { None },
                violations,
            }
        }
    }

    fn soft_only(converge: bool) -> SafetyValidator {
        let mut v = SafetyValidator { rules: Vec::new() };
        v.add_rule(Box::new(SoftClamp {
            limit: 10.0,
            converge,
        }));
        v
    }

    #[test]
    fn converged_clamp_accepted_with_full_revalidation() {
        let v = soft_only(true);
        let (sched, re) = v
            .revalidate_clamped(schedule(30.0), &state())
            .expect("截断候选两轮内收敛干净");
        assert!((sched.schedule[0].discharge_power_kw - 10.0).abs() < 1e-9);
        assert!(re.passed && !re.clamped, "采信前提 = 重跑全部规则后干净");
    }

    #[test]
    fn non_converging_clamp_never_accepted() {
        let v = soft_only(false);
        // 渐近逼近永不达限：≤4 轮有界迭代后必须放弃（None）——调用方
        // 回落原解，绝不采信半程截断，也绝不循环放宽阈值。
        assert!(v.revalidate_clamped(schedule(30.0), &state()).is_none());
    }

    #[test]
    fn already_clean_first_clamp_passes_through() {
        let v = soft_only(true);
        let (sched, re) = v
            .revalidate_clamped(schedule(5.0), &state())
            .expect("首轮即干净");
        assert!((sched.schedule[0].discharge_power_kw - 5.0).abs() < 1e-9);
        assert!(re.passed && !re.clamped);
    }

    // ---- R15-Q（WP-74.1.0）：后置同时充放检查的结论面 ----
    // 31-review P1 AI-05 的 mutex 后置检查此前在 `has_hard_reject` 定型
    // **之后**才入列违规——仅有 mutex 违规时
    // `passed = 非空 || !has_hard_reject = true`，纵深防御形同虚设。
    // 三个测试：拒绝（红→绿）、revalidate 传导（红→绿）、合法解不误伤（恒绿）。

    fn mutex_schedule() -> ScheduleResult {
        // LP 部分同时解的典型形状（60+40≤100 各自满足 max_power=100 ⇒
        // 规则链零违规），唯一防线就是 mutex 后置检查。
        ScheduleResult {
            schedule: alloc::vec![ScheduleEntry {
                period: 0,
                charge_power_kw: 60.0,
                discharge_power_kw: 40.0,
                net_power_kw: 100.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            }],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: SolveStatus::Optimal,
        }
    }

    #[test]
    fn simultaneous_charge_discharge_is_rejected() {
        let v = SafetyValidator::new();
        let r = v.validate(&mutex_schedule(), &state());
        assert!(!r.passed, "同时充放必须拒绝——mutex 后置检查是 Fatal 纵深防御");
        let m = r
            .violations
            .iter()
            .find(|v| v.rule == "charge_discharge_mutex")
            .expect("违规列表必须含 charge_discharge_mutex");
        assert_eq!(m.severity, Severity::Fatal);
    }

    #[test]
    fn revalidate_clamped_propagates_mutex_reject() {
        // :161 只看 `r.passed` 的传导面——validate 拒绝后 revalidate_clamped
        // 不得把同时充放候选当"收敛干净"放行。
        let v = SafetyValidator::empty();
        assert!(
            v.revalidate_clamped(mutex_schedule(), &state()).is_none(),
            "携带同时充放条目的截断候选不得被采信"
        );
    }

    #[test]
    fn legal_schedules_still_pass_after_mutex_gate() {
        // 反向锁（防过收紧）：纯放电 / 纯充电 / 零条目 schedule 的 passed
        // 不变——mutex 门只咬双同时>eps 的条目。
        let v = SafetyValidator::new();
        assert!(v.validate(&schedule(30.0), &state()).passed, "纯放电合法解");
        let pure_charge = ScheduleResult {
            schedule: alloc::vec![ScheduleEntry {
                period: 0,
                charge_power_kw: 30.0,
                discharge_power_kw: 0.0,
                net_power_kw: -30.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            }],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: SolveStatus::Optimal,
        };
        assert!(v.validate(&pure_charge, &state()).passed, "纯充电合法解");
        let empty = ScheduleResult {
            schedule: alloc::vec![],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: SolveStatus::Optimal,
        };
        assert!(v.validate(&empty, &state()).passed, "零条目 schedule");
    }
}
