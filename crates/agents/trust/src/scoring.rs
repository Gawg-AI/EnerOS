//! 评分规则 (workspace v0.192.0).
//!
//! 定义各行为类型的分数变化规则，支持自定义配置。

use crate::types::{ActionImpact, ActionType, AgentAction};

/// 评分规则配置.
///
/// 定义每种行为类型对应的分数增减值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoringRules {
    /// 任务完成加分.
    pub task_complete: i32,
    /// 任务失败扣分.
    pub task_failed: i32,
    /// 越权访问扣分.
    pub unauthorized: i32,
    /// 恶意行为扣分.
    pub malicious: i32,
    /// 崩溃扣分.
    pub crash: i32,
    /// 恢复加分.
    pub recovery: i32,
    /// 协作加分.
    pub cooperation: i32,
}

impl Default for ScoringRules {
    fn default() -> Self {
        Self {
            task_complete: 5,
            task_failed: -3,
            unauthorized: -50,
            malicious: -200,
            crash: -10,
            recovery: 2,
            cooperation: 3,
        }
    }
}

impl ScoringRules {
    /// 创建自定义评分规则.
    pub fn custom(
        task_complete: i32,
        task_failed: i32,
        unauthorized: i32,
        malicious: i32,
        crash: i32,
        recovery: i32,
        cooperation: i32,
    ) -> Self {
        Self {
            task_complete,
            task_failed,
            unauthorized,
            malicious,
            crash,
            recovery,
            cooperation,
        }
    }

    /// 计算行为的分数变化.
    pub fn calculate_delta(&self, action: &AgentAction) -> i32 {
        let base = match action.action_type {
            ActionType::TaskComplete => self.task_complete,
            ActionType::TaskFailed => self.task_failed,
            ActionType::Unauthorized => self.unauthorized,
            ActionType::MaliciousBehavior => self.malicious,
            ActionType::Crash => self.crash,
            ActionType::Recovery => self.recovery,
            ActionType::Cooperation => self.cooperation,
        };

        // 影响程度修正：正面行为失败时减半，负面行为成功时加重
        match action.impact {
            ActionImpact::Positive if !action.success => base / 2,
            ActionImpact::Negative if action.success => base * 2,
            _ => base,
        }
    }

    /// 获取行为类型的基础分值.
    pub fn base_score(&self, action_type: ActionType) -> i32 {
        match action_type {
            ActionType::TaskComplete => self.task_complete,
            ActionType::TaskFailed => self.task_failed,
            ActionType::Unauthorized => self.unauthorized,
            ActionType::MaliciousBehavior => self.malicious,
            ActionType::Crash => self.crash,
            ActionType::Recovery => self.recovery,
            ActionType::Cooperation => self.cooperation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentAction;

    #[test]
    fn test_default_rules() {
        let rules = ScoringRules::default();
        assert_eq!(rules.task_complete, 5);
        assert_eq!(rules.task_failed, -3);
        assert_eq!(rules.unauthorized, -50);
        assert_eq!(rules.malicious, -200);
        assert_eq!(rules.crash, -10);
        assert_eq!(rules.recovery, 2);
        assert_eq!(rules.cooperation, 3);
    }

    #[test]
    fn test_calculate_delta_positive() {
        let rules = ScoringRules::default();
        let action = AgentAction::new(1, ActionType::TaskComplete, 100, true);
        assert_eq!(rules.calculate_delta(&action), 5);
    }

    #[test]
    fn test_calculate_delta_negative() {
        let rules = ScoringRules::default();
        let action = AgentAction::new(1, ActionType::MaliciousBehavior, 100, false);
        assert_eq!(rules.calculate_delta(&action), -200);
    }

    #[test]
    fn test_positive_action_failed_halved() {
        let rules = ScoringRules::default();
        // 正面行为但失败 → 加分减半
        let action = AgentAction::new(1, ActionType::TaskComplete, 100, false);
        assert_eq!(rules.calculate_delta(&action), 2); // 5/2 = 2
    }

    #[test]
    fn test_negative_action_success_doubled() {
        let rules = ScoringRules::default();
        // 负面行为且成功 → 扣分加倍
        let action = AgentAction::new(1, ActionType::Unauthorized, 100, true);
        assert_eq!(rules.calculate_delta(&action), -100); // -50*2
    }

    #[test]
    fn test_custom_rules() {
        let rules = ScoringRules::custom(10, -5, -100, -500, -20, 5, 8);
        assert_eq!(rules.base_score(ActionType::TaskComplete), 10);
        assert_eq!(rules.base_score(ActionType::MaliciousBehavior), -500);
    }
}
