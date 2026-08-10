//! 自愈引擎模块。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{AnomalyResult, HealAction, HealActionType, HealOutcome, HealPlan};
use crate::AiOpsError;

/// 自愈引擎 trait
pub trait SelfHealEngine {
    /// 根据异常诊断生成自愈计划
    fn diagnose(&self, anomaly: &AnomalyResult) -> HealPlan;

    /// 执行自愈计划
    fn execute(&self, plan: &HealPlan) -> Result<HealOutcome, AiOpsError>;
}

/// 真实自愈动作执行器（v0.241.0，B5-04）。
///
/// 执行真实动作（重启服务/切换链路），由集成层接 root-task 管理能力。
pub trait HealActionExecutor {
    /// 执行单个自愈动作。
    fn execute_action(&mut self, action: &HealAction) -> Result<(), AiOpsError>;
}

/// 真实动作执行器（注入闭包）。
pub struct RealHealExecutor<F> {
    /// 执行闭包（动作 → 结果）。
    exec_fn: F,
}

impl<F> RealHealExecutor<F>
where
    F: FnMut(&HealAction) -> Result<(), AiOpsError>,
{
    /// 创建真实执行器。
    pub fn new(exec_fn: F) -> Self {
        Self { exec_fn }
    }
}

impl<F> HealActionExecutor for RealHealExecutor<F>
where
    F: FnMut(&HealAction) -> Result<(), AiOpsError>,
{
    fn execute_action(&mut self, action: &HealAction) -> Result<(), AiOpsError> {
        (self.exec_fn)(action)
    }
}

/// Mock 自愈执行器（33 OB-06：Mock 回退限 cfg(test/mock)，生产路径
/// 无执行器必须 Err(HealFailed)）。
///
/// 记录已执行动作的 target，恒返回 Ok；供测试与 mock 装配显式注入，
/// 替代旧实现"无执行器即 Mock 成功"的隐式回退。
#[cfg(any(test, feature = "mock"))]
pub struct MockHealExecutor {
    /// 已执行动作的 target 序列.
    executed: core::cell::RefCell<alloc::vec::Vec<u64>>,
}

#[cfg(any(test, feature = "mock"))]
impl MockHealExecutor {
    /// 创建 Mock 执行器.
    pub fn new() -> Self {
        Self {
            executed: core::cell::RefCell::new(alloc::vec::Vec::new()),
        }
    }

    /// 已执行动作的 target 列表（按执行顺序）.
    pub fn executed_targets(&self) -> alloc::vec::Vec<u64> {
        self.executed.borrow().clone()
    }

    /// 已执行动作数.
    pub fn executed_len(&self) -> usize {
        self.executed.borrow().len()
    }
}

#[cfg(any(test, feature = "mock"))]
impl Default for MockHealExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "mock"))]
impl HealActionExecutor for MockHealExecutor {
    fn execute_action(&mut self, action: &HealAction) -> Result<(), AiOpsError> {
        self.executed.borrow_mut().push(action.target);
        Ok(())
    }
}

/// 从 metric 名派生组件 ID（v0.241.0，确定性：FNV-1a）。
///
/// HealAction.target 错位修复：target 为组件 ID（metric 确定性派生），
/// 而非时间戳。
pub fn component_id_of(metric: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in metric.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 基于规则的自愈器
pub struct RuleBasedHealer {
    /// 真实动作执行器（v0.241.0 注入；33 OB-06：None = 无执行器——
    /// 执行 fail-closed，不再回退 Mock 成功）。
    /// RefCell：`SelfHealEngine::execute(&self)` 需要内部可变性。
    executor: core::cell::RefCell<Option<alloc::boxed::Box<dyn HealActionExecutor>>>,
}

impl RuleBasedHealer {
    /// 创建基于规则的自愈器
    pub fn new() -> Self {
        Self {
            executor: core::cell::RefCell::new(None),
        }
    }

    /// 注入真实动作执行器（v0.241.0，接 root-task 管理能力）。
    pub fn with_executor(self, executor: alloc::boxed::Box<dyn HealActionExecutor>) -> Self {
        *self.executor.borrow_mut() = Some(executor);
        self
    }
}

impl Default for RuleBasedHealer {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfHealEngine for RuleBasedHealer {
    fn diagnose(&self, anomaly: &AnomalyResult) -> HealPlan {
        let mut actions: Vec<HealAction> = Vec::new();

        // v0.241.0：HealAction.target 错位修复——target 为组件 ID
        //（由 metric 确定性派生），不再错误填充时间戳。
        if anomaly.score > 0.8 {
            // 高异常分数 → 重启 + 通知人工
            actions.push(HealAction {
                action_type: HealActionType::RestartComponent,
                target: component_id_of(&anomaly.metric),
                description: String::from("重启异常组件"),
            });
            actions.push(HealAction {
                action_type: HealActionType::NotifyHuman,
                target: component_id_of(&anomaly.metric),
                description: String::from("通知运维人员介入"),
            });
        } else if anomaly.score > 0.5 {
            // 中等异常 → 限流 + 清缓存
            actions.push(HealAction {
                action_type: HealActionType::ThrottleLoad,
                target: component_id_of(&anomaly.metric),
                description: String::from("降低负载"),
            });
            actions.push(HealAction {
                action_type: HealActionType::ClearCache,
                target: component_id_of(&anomaly.metric),
                description: String::from("清除缓存"),
            });
        } else {
            // 低异常 → 仅通知
            actions.push(HealAction {
                action_type: HealActionType::NotifyHuman,
                target: component_id_of(&anomaly.metric),
                description: String::from("低级别告警通知"),
            });
        }

        let requires_approval = anomaly.score > 0.8;

        let rollback = if requires_approval {
            Some(HealAction {
                action_type: HealActionType::SwitchToBackup,
                target: component_id_of(&anomaly.metric),
                description: String::from("回滚: 切换到备用组件"),
            })
        } else {
            None
        };

        HealPlan {
            actions,
            requires_approval,
            rollback,
        }
    }

    fn execute(&self, plan: &HealPlan) -> Result<HealOutcome, AiOpsError> {
        if plan.requires_approval {
            return Err(AiOpsError::ApprovalRequired);
        }

        if plan.actions.is_empty() {
            return Err(AiOpsError::HealFailed);
        }

        // v0.248.0：真实计时——执行动作前后取单调时钟差值（替换硬编码 150ms）。
        let t0 = eneros_time::get_monotonic_ns();

        // v0.241.0：真实执行器（注入时逐动作执行）。
        // 33 OB-06：无执行器 = Err(HealFailed)（fail-closed）——旧实现
        // 回退"Mock 成功"谎报自愈已执行；Mock 回退仅限 cfg(test/mock)
        // 的显式 MockHealExecutor（见 tests）。
        let mut guard = self.executor.borrow_mut();
        let Some(ex) = guard.as_mut() else {
            return Err(AiOpsError::HealFailed);
        };
        let mut side_effects: Vec<String> = Vec::new();
        let mut ok = true;
        for a in &plan.actions {
            match ex.execute_action(a) {
                Ok(()) => side_effects.push(a.description.clone()),
                Err(_) => {
                    ok = false;
                    side_effects.push(alloc::format!("{}:FAILED", a.description));
                }
            }
        }
        if !ok {
            return Err(AiOpsError::HealFailed);
        }

        // v0.248.0：耗时接真实时钟（boot 后单调纳秒；未初始化回退 0）。
        let duration_ms = eneros_time::get_monotonic_ns().saturating_sub(t0) / 1_000_000;
        Ok(HealOutcome {
            success: true,
            duration_ms,
            side_effects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_anomaly(score: f32) -> AnomalyResult {
        AnomalyResult {
            metric: String::from("cpu"),
            score,
            is_anomaly: score > 0.3,
            ts: 9999,
            explanation: String::from("test"),
        }
    }

    #[test]
    fn test_diagnose_high_score_produces_plan() {
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.9);
        let plan = healer.diagnose(&anomaly);

        assert!(plan.requires_approval);
        assert!(plan.rollback.is_some());
        assert_eq!(plan.actions.len(), 2);
        assert_eq!(
            plan.actions[0].action_type,
            HealActionType::RestartComponent
        );
        assert_eq!(plan.actions[1].action_type, HealActionType::NotifyHuman);
    }

    #[test]
    fn test_diagnose_medium_score() {
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.6);
        let plan = healer.diagnose(&anomaly);

        assert!(!plan.requires_approval);
        assert!(plan.rollback.is_none());
        assert_eq!(plan.actions.len(), 2);
        assert_eq!(plan.actions[0].action_type, HealActionType::ThrottleLoad);
    }

    #[test]
    fn test_execute_succeeds_without_approval() {
        // 33 OB-06：显式注入 Mock 执行器（旧实现靠"无执行器回退 Mock
        // 成功"，已 fail-closed）。
        let mock = alloc::boxed::Box::new(MockHealExecutor::new());
        let healer = RuleBasedHealer::new().with_executor(mock);
        let anomaly = make_anomaly(0.6);
        let plan = healer.diagnose(&anomaly);
        let outcome = healer.execute(&plan).unwrap();

        assert!(outcome.success);
        // v0.248.0：耗时接真实时钟（测试环境时钟未初始化 → 0；boot 后为
        // 真实执行耗时）。断言不再依赖硬编码值。
        assert!(outcome.duration_ms <= 1000, "真实执行耗时不应异常");
        assert!(!outcome.side_effects.is_empty());
    }

    #[test]
    fn test_execute_without_executor_fails_closed() {
        // 33 OB-06 判据：无执行器 → Err(HealFailed)（生产路径不得谎报
        // 自愈成功；Mock 回退仅限 cfg(test/mock) 显式注入）。
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.6);
        let plan = healer.diagnose(&anomaly);
        let result = healer.execute(&plan);
        assert!(matches!(result, Err(AiOpsError::HealFailed)));
    }

    #[test]
    fn test_mock_executor_records_targets() {
        // Mock 执行器：显式注入路径可用，逐动作记录 target。
        let mut mock = MockHealExecutor::new();
        let action = HealAction {
            action_type: HealActionType::ClearCache,
            target: 42,
            description: String::from("test"),
        };
        mock.execute_action(&action).expect("mock executes");
        assert_eq!(mock.executed_len(), 1);
        assert_eq!(mock.executed_targets(), alloc::vec![42]);
    }

    #[test]
    fn test_execute_requires_approval() {
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.9);
        let plan = healer.diagnose(&anomaly);
        let result = healer.execute(&plan);

        assert!(matches!(result, Err(AiOpsError::ApprovalRequired)));
    }

    #[test]
    fn test_execute_empty_plan_fails() {
        let healer = RuleBasedHealer::new();
        let plan = HealPlan {
            actions: vec![],
            requires_approval: false,
            rollback: None,
        };
        let result = healer.execute(&plan);
        assert!(matches!(result, Err(AiOpsError::HealFailed)));
    }

    #[test]
    fn test_default_impl() {
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.2);
        let plan = healer.diagnose(&anomaly);
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].action_type, HealActionType::NotifyHuman);
    }

    // ===== v0.241.0（B5-04）：target 错位修复 + 真实执行器 =====

    #[test]
    fn test_target_is_component_id_not_timestamp() {
        // HealAction.target 错位修复：target = 组件 ID（metric 派生），
        // 不再是时间戳。
        let healer = RuleBasedHealer::new();
        let anomaly = AnomalyResult {
            metric: String::from("grid/voltage"),
            score: 0.9,
            is_anomaly: true,
            ts: 12345,
            explanation: String::from("电压异常"),
        };
        let plan = healer.diagnose(&anomaly);
        for action in &plan.actions {
            assert_ne!(action.target, anomaly.ts, "target 必须为组件 ID 而非时间戳");
            assert_eq!(
                action.target,
                component_id_of("grid/voltage"),
                "target 与 metric 派生组件 ID 一致"
            );
        }
    }

    #[test]
    fn test_component_id_deterministic() {
        assert_eq!(
            component_id_of("grid/voltage"),
            component_id_of("grid/voltage")
        );
        assert_ne!(
            component_id_of("grid/voltage"),
            component_id_of("grid/current")
        );
    }

    #[test]
    fn test_real_executor_end_to_end() {
        // 真实动作执行器：注入闭包 → execute 逐动作执行。
        let executed: &'static core::cell::RefCell<alloc::vec::Vec<u64>> =
            Box::leak(Box::new(core::cell::RefCell::new(alloc::vec::Vec::new())));
        let executor = RealHealExecutor::new(|action: &HealAction| {
            executed.borrow_mut().push(action.target);
            Ok(())
        });
        let healer = RuleBasedHealer::new().with_executor(alloc::boxed::Box::new(executor));
        let anomaly = AnomalyResult {
            metric: String::from("grid/voltage"),
            score: 0.6,
            is_anomaly: true,
            ts: 100,
            explanation: String::from("x"),
        };
        let plan = healer.diagnose(&anomaly);
        let outcome = healer.execute(&plan).expect("execute");
        assert!(outcome.success);
        // 中等异常：限流 + 清缓存两个动作都执行。
        assert_eq!(executed.borrow().len(), 2);
        // 都作用于同一组件。
        let target = component_id_of("grid/voltage");
        assert!(executed.borrow().iter().all(|&t| t == target));
    }

    #[test]
    fn test_real_executor_failure_propagates() {
        let executor = RealHealExecutor::new(|_action: &HealAction| Err(AiOpsError::HealFailed));
        let healer = RuleBasedHealer::new().with_executor(alloc::boxed::Box::new(executor));
        let anomaly = AnomalyResult {
            metric: String::from("m"),
            score: 0.6,
            is_anomaly: true,
            ts: 0,
            explanation: String::from("x"),
        };
        let plan = healer.diagnose(&anomaly);
        assert!(healer.execute(&plan).is_err());
    }
}
