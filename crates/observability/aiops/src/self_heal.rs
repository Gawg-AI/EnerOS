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

/// 基于规则的自愈器
#[derive(Debug, Clone)]
pub struct RuleBasedHealer;

impl RuleBasedHealer {
    /// 创建基于规则的自愈器
    pub fn new() -> Self {
        Self
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

        if anomaly.score > 0.8 {
            // 高异常分数 → 重启 + 通知人工
            actions.push(HealAction {
                action_type: HealActionType::RestartComponent,
                target: anomaly.ts,
                description: String::from("重启异常组件"),
            });
            actions.push(HealAction {
                action_type: HealActionType::NotifyHuman,
                target: anomaly.ts,
                description: String::from("通知运维人员介入"),
            });
        } else if anomaly.score > 0.5 {
            // 中等异常 → 限流 + 清缓存
            actions.push(HealAction {
                action_type: HealActionType::ThrottleLoad,
                target: anomaly.ts,
                description: String::from("降低负载"),
            });
            actions.push(HealAction {
                action_type: HealActionType::ClearCache,
                target: anomaly.ts,
                description: String::from("清除缓存"),
            });
        } else {
            // 低异常 → 仅通知
            actions.push(HealAction {
                action_type: HealActionType::NotifyHuman,
                target: anomaly.ts,
                description: String::from("低级别告警通知"),
            });
        }

        let requires_approval = anomaly.score > 0.8;

        let rollback = if requires_approval {
            Some(HealAction {
                action_type: HealActionType::SwitchToBackup,
                target: anomaly.ts,
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

        // Mock: 执行成功
        let side_effects: Vec<String> =
            plan.actions.iter().map(|a| a.description.clone()).collect();

        Ok(HealOutcome {
            success: true,
            duration_ms: 150,
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
        let healer = RuleBasedHealer::new();
        let anomaly = make_anomaly(0.6);
        let plan = healer.diagnose(&anomaly);
        let outcome = healer.execute(&plan).unwrap();

        assert!(outcome.success);
        assert_eq!(outcome.duration_ms, 150);
        assert!(!outcome.side_effects.is_empty());
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
        let healer = RuleBasedHealer::default();
        let anomaly = make_anomaly(0.2);
        let plan = healer.diagnose(&anomaly);
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].action_type, HealActionType::NotifyHuman);
    }
}
