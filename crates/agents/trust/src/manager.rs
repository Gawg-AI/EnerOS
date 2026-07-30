//! 信誉体系管理器 (v0.168.0).
//!
//! TrustSystem trait 实现，统一管理所有 Agent 信誉。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::adjuster::TrustAdjuster;
use crate::history::TrustHistory;
use crate::scoring::ScoringRules;
use crate::types::{
    AgentAction, AgentId, TrustEvent, TrustGrade, TrustScore, TrustStats, INITIAL_SCORE,
};
use crate::TrustError;

/// 信誉体系 trait（蓝图 §3 接口类）.
pub trait TrustSystem {
    /// 获取 Agent 信任分.
    fn get_trust_score(&self, agent_id: &AgentId) -> TrustScore;
    /// 获取信誉等级.
    fn get_trust_grade(&self, agent_id: &AgentId) -> TrustGrade;
    /// 记录行为并更新分数.
    fn record_action(&mut self, action: &AgentAction) -> Result<(), TrustError>;
    /// 手动调整分数.
    fn adjust_score(
        &mut self,
        agent_id: &AgentId,
        delta: i32,
        reason: &str,
    ) -> Result<(), TrustError>;
}

/// 信誉体系实现.
pub struct TrustSystemImpl {
    /// Agent 信任历史表.
    histories: BTreeMap<AgentId, TrustHistory>,
    /// 评分规则.
    scoring_rules: ScoringRules,
    /// 动态调整器.
    adjuster: TrustAdjuster,
    /// 审计事件日志.
    events: Vec<TrustEvent>,
    /// 当前时间（由外部注入）.
    current_time: u64,
}

impl TrustSystemImpl {
    /// 创建信誉体系.
    pub fn new() -> Self {
        Self {
            histories: BTreeMap::new(),
            scoring_rules: ScoringRules::default(),
            adjuster: TrustAdjuster::default(),
            events: Vec::new(),
            current_time: 0,
        }
    }

    /// 使用自定义评分规则创建.
    pub fn with_rules(scoring_rules: ScoringRules) -> Self {
        Self {
            scoring_rules,
            ..Self::new()
        }
    }

    /// 设置当前时间.
    pub fn set_time(&mut self, time: u64) {
        self.current_time = time;
    }

    /// 注册 Agent（初始 500 分）.
    pub fn register_agent(&mut self, agent_id: AgentId) {
        self.histories
            .entry(agent_id)
            .or_insert_with(|| TrustHistory::new(agent_id, self.current_time));
    }

    /// Agent 是否已注册.
    pub fn is_registered(&self, agent_id: &AgentId) -> bool {
        self.histories.contains_key(agent_id)
    }

    /// Agent 是否被封禁.
    pub fn is_blocked(&self, agent_id: &AgentId) -> bool {
        self.get_trust_grade(agent_id).is_blocked()
    }

    /// 获取信任趋势.
    pub fn get_trend(&self, agent_id: &AgentId) -> Option<crate::types::TrustTrend> {
        self.histories.get(agent_id).map(|h| h.trend)
    }

    /// 获取审计事件日志.
    pub fn events(&self) -> &[TrustEvent] {
        &self.events
    }

    /// 获取统计信息.
    pub fn stats(&self) -> TrustStats {
        let total_agents = self.histories.len() as u32;
        let blocked_agents = self.histories.values().filter(|h| h.is_blocked()).count() as u32;
        let total_actions = self
            .histories
            .values()
            .map(|h| h.action_count() as u32)
            .sum();
        let total_grade_changes = self.histories.values().map(|h| h.grade_changes).sum();
        TrustStats {
            total_agents,
            blocked_agents,
            total_actions,
            total_grade_changes,
        }
    }

    /// 应用时间衰减（定期调用）.
    pub fn apply_decay_all(&mut self, current_time: u64) {
        for history in self.histories.values_mut() {
            let elapsed = current_time.saturating_sub(history.last_updated);
            if elapsed > 0 {
                let new_score = self.adjuster.apply_decay(history.current_score, elapsed);
                history.current_score = new_score;
                history.last_updated = current_time;
            }
        }
        self.current_time = current_time;
    }

    /// 获取所有被封禁的 Agent ID.
    pub fn blocked_agents(&self) -> Vec<AgentId> {
        self.histories
            .values()
            .filter(|h| h.is_blocked())
            .map(|h| h.agent_id)
            .collect()
    }
}

impl Default for TrustSystemImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl TrustSystem for TrustSystemImpl {
    fn get_trust_score(&self, agent_id: &AgentId) -> TrustScore {
        self.histories
            .get(agent_id)
            .map(|h| h.current_score)
            .unwrap_or(INITIAL_SCORE)
    }

    fn get_trust_grade(&self, agent_id: &AgentId) -> TrustGrade {
        TrustGrade::from_score(self.get_trust_score(agent_id))
    }

    fn record_action(&mut self, action: &AgentAction) -> Result<(), TrustError> {
        let history = self
            .histories
            .get_mut(&action.agent_id)
            .ok_or(TrustError::AgentNotRegistered)?;

        // 计算分数变化
        let delta = self.scoring_rules.calculate_delta(action);

        // 应用分数
        let (old_score, new_score, old_grade, new_grade) =
            history.apply_delta(delta, action.timestamp);

        // 记录行为
        history.record_action(action.clone());

        // 记录审计事件
        self.events.push(TrustEvent {
            agent_id: action.agent_id,
            old_score,
            new_score,
            old_grade,
            new_grade,
            action_type: action.action_type,
            timestamp: action.timestamp,
        });

        Ok(())
    }

    fn adjust_score(
        &mut self,
        agent_id: &AgentId,
        delta: i32,
        _reason: &str,
    ) -> Result<(), TrustError> {
        let history = self
            .histories
            .get_mut(agent_id)
            .ok_or(TrustError::AgentNotRegistered)?;

        let new_score = self.adjuster.manual_adjust(history.current_score, delta);
        let old_score = history.current_score;
        let old_grade = history.grade();
        history.current_score = new_score;
        let new_grade = history.grade();

        if old_grade != new_grade {
            history.grade_changes += 1;
        }

        self.events.push(TrustEvent {
            agent_id: *agent_id,
            old_score,
            new_score,
            old_grade,
            new_grade,
            action_type: crate::types::ActionType::Recovery, // 手动调整标记
            timestamp: self.current_time,
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActionType;

    fn setup() -> TrustSystemImpl {
        let mut sys = TrustSystemImpl::new();
        sys.register_agent(1);
        sys.register_agent(2);
        sys
    }

    #[test]
    fn test_initial_score() {
        let sys = setup();
        assert_eq!(sys.get_trust_score(&1), INITIAL_SCORE);
        assert_eq!(sys.get_trust_grade(&1), TrustGrade::Silver);
    }

    #[test]
    fn test_unregistered_agent_default() {
        let sys = setup();
        assert_eq!(sys.get_trust_score(&999), INITIAL_SCORE);
    }

    #[test]
    fn test_record_positive_action() {
        let mut sys = setup();
        let action = AgentAction::new(1, ActionType::TaskComplete, 100, true);
        sys.record_action(&action).unwrap();
        assert_eq!(sys.get_trust_score(&1), 505);
    }

    #[test]
    fn test_record_negative_action() {
        let mut sys = setup();
        let action = AgentAction::new(1, ActionType::MaliciousBehavior, 100, false);
        sys.record_action(&action).unwrap();
        assert_eq!(sys.get_trust_score(&1), 300); // 500 - 200
        assert_eq!(sys.get_trust_grade(&1), TrustGrade::Bronze);
    }

    #[test]
    fn test_agent_blocked() {
        let mut sys = setup();
        let action = AgentAction::new(1, ActionType::MaliciousBehavior, 100, true);
        // success=true 的负面行为 → 扣分加倍 -400
        sys.record_action(&action).unwrap();
        assert_eq!(sys.get_trust_score(&1), 100); // 500 - 400
        assert!(!sys.is_blocked(&1)); // 100 恰好是 Bronze

        let action2 = AgentAction::new(1, ActionType::Crash, 101, false);
        sys.record_action(&action2).unwrap();
        assert_eq!(sys.get_trust_score(&1), 90);
        assert!(sys.is_blocked(&1));
    }

    #[test]
    fn test_unregistered_action_fails() {
        let mut sys = setup();
        let action = AgentAction::new(999, ActionType::TaskComplete, 100, true);
        assert_eq!(
            sys.record_action(&action).unwrap_err(),
            TrustError::AgentNotRegistered
        );
    }

    #[test]
    fn test_manual_adjust() {
        let mut sys = setup();
        sys.adjust_score(&1, 100, "奖励").unwrap();
        assert_eq!(sys.get_trust_score(&1), 600);

        sys.adjust_score(&1, -700, "惩罚").unwrap();
        assert_eq!(sys.get_trust_score(&1), 0); // clamp
    }

    #[test]
    fn test_adjust_unregistered_fails() {
        let mut sys = setup();
        assert_eq!(
            sys.adjust_score(&999, 10, "test").unwrap_err(),
            TrustError::AgentNotRegistered
        );
    }

    #[test]
    fn test_stats() {
        let mut sys = setup();
        let action = AgentAction::new(1, ActionType::TaskComplete, 100, true);
        sys.record_action(&action).unwrap();

        let stats = sys.stats();
        assert_eq!(stats.total_agents, 2);
        assert_eq!(stats.blocked_agents, 0);
        assert_eq!(stats.total_actions, 1);
    }

    #[test]
    fn test_audit_events() {
        let mut sys = setup();
        let action = AgentAction::new(1, ActionType::TaskComplete, 100, true);
        sys.record_action(&action).unwrap();
        assert_eq!(sys.events().len(), 1);
        assert_eq!(sys.events()[0].old_score, 500);
        assert_eq!(sys.events()[0].new_score, 505);
    }

    #[test]
    fn test_blocked_agents_list() {
        let mut sys = setup();
        let action = AgentAction::new(2, ActionType::MaliciousBehavior, 100, true);
        sys.record_action(&action).unwrap(); // 500 - 400 = 100
        let action2 = AgentAction::new(2, ActionType::Crash, 101, false);
        sys.record_action(&action2).unwrap(); // 100 - 10 = 90 → Blocked

        let blocked = sys.blocked_agents();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0], 2);
    }

    #[test]
    fn test_decay_all() {
        let mut sys = setup();
        // 先加分到 800
        sys.adjust_score(&1, 300, "test").unwrap();
        assert_eq!(sys.get_trust_score(&1), 800);

        // 经过大量时间后衰减
        sys.apply_decay_all(3600 * 100);
        // 800 → 向 500 回归 100 分 → 700
        assert_eq!(sys.get_trust_score(&1), 700);
    }

    #[test]
    fn test_grade_upgrade_downgrade() {
        let mut sys = setup();
        // Silver → Gold
        sys.adjust_score(&1, 200, "upgrade").unwrap();
        assert_eq!(sys.get_trust_grade(&1), TrustGrade::Gold);

        // Gold → Platinum
        sys.adjust_score(&1, 200, "upgrade2").unwrap();
        assert_eq!(sys.get_trust_grade(&1), TrustGrade::Platinum);
    }
}
