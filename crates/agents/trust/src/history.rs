//! 行为历史 (v0.168.0).
//!
//! 记录 Agent 行为历史，维护信任分和趋势。

use alloc::vec::Vec;

use crate::types::{
    AgentAction, AgentId, TrustGrade, TrustScore, TrustTrend, INITIAL_SCORE, MAX_SCORE, MIN_SCORE,
};

/// 最大历史记录数（防内存膨胀）.
pub const MAX_HISTORY_SIZE: usize = 256;

/// Agent 信任历史.
#[derive(Debug, Clone)]
pub struct TrustHistory {
    /// Agent ID.
    pub agent_id: AgentId,
    /// 当前信任分.
    pub current_score: TrustScore,
    /// 行为记录列表.
    pub actions: Vec<AgentAction>,
    /// 最后更新时间戳.
    pub last_updated: u64,
    /// 信任趋势.
    pub trend: TrustTrend,
    /// 等级变更次数.
    pub grade_changes: u32,
}

impl TrustHistory {
    /// 创建新的信任历史（初始 500 分）.
    pub fn new(agent_id: AgentId, timestamp: u64) -> Self {
        Self {
            agent_id,
            current_score: INITIAL_SCORE,
            actions: Vec::new(),
            last_updated: timestamp,
            trend: TrustTrend::Stable,
            grade_changes: 0,
        }
    }

    /// 获取当前信誉等级.
    pub fn grade(&self) -> TrustGrade {
        TrustGrade::from_score(self.current_score)
    }

    /// 是否被封禁.
    pub fn is_blocked(&self) -> bool {
        self.grade().is_blocked()
    }

    /// 应用分数变化.
    ///
    /// 返回 (旧分数, 新分数, 旧等级, 新等级)。
    pub fn apply_delta(
        &mut self,
        delta: i32,
        timestamp: u64,
    ) -> (TrustScore, TrustScore, TrustGrade, TrustGrade) {
        let old_score = self.current_score;
        let old_grade = self.grade();

        let new_score =
            (self.current_score as i32 + delta).clamp(MIN_SCORE as i32, MAX_SCORE as i32) as u16;
        self.current_score = new_score;
        self.last_updated = timestamp;

        let new_grade = self.grade();

        // 更新趋势
        self.trend = if new_score > old_score {
            TrustTrend::Rising
        } else if new_score < old_score {
            TrustTrend::Declining
        } else {
            TrustTrend::Stable
        };

        // 等级变更计数
        if old_grade != new_grade {
            self.grade_changes += 1;
        }

        (old_score, new_score, old_grade, new_grade)
    }

    /// 记录行为.
    pub fn record_action(&mut self, action: AgentAction) {
        self.actions.push(action);
        // 限制历史大小
        if self.actions.len() > MAX_HISTORY_SIZE {
            let drain_count = self.actions.len() - MAX_HISTORY_SIZE;
            self.actions.drain(..drain_count);
        }
    }

    /// 获取最近 N 条行为.
    pub fn recent_actions(&self, n: usize) -> &[AgentAction] {
        let start = self.actions.len().saturating_sub(n);
        &self.actions[start..]
    }

    /// 获取行为总数.
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActionType;

    #[test]
    fn test_new_history() {
        let h = TrustHistory::new(1, 1000);
        assert_eq!(h.current_score, INITIAL_SCORE);
        assert_eq!(h.grade(), TrustGrade::Silver);
        assert_eq!(h.trend, TrustTrend::Stable);
        assert!(!h.is_blocked());
    }

    #[test]
    fn test_apply_delta_positive() {
        let mut h = TrustHistory::new(1, 1000);
        let (old, new, old_g, new_g) = h.apply_delta(5, 1001);
        assert_eq!(old, 500);
        assert_eq!(new, 505);
        assert_eq!(old_g, TrustGrade::Silver);
        assert_eq!(new_g, TrustGrade::Silver);
        assert_eq!(h.trend, TrustTrend::Rising);
    }

    #[test]
    fn test_apply_delta_clamped() {
        let mut h = TrustHistory::new(1, 1000);
        // 不会超过 1000
        h.apply_delta(600, 1001);
        assert_eq!(h.current_score, MAX_SCORE);
        // 不会低于 0
        h.apply_delta(-2000, 1002);
        assert_eq!(h.current_score, MIN_SCORE);
    }

    #[test]
    fn test_grade_change_counted() {
        let mut h = TrustHistory::new(1, 1000);
        assert_eq!(h.grade_changes, 0);
        // Silver → Gold
        h.apply_delta(200, 1001);
        assert_eq!(h.grade_changes, 1);
        assert_eq!(h.grade(), TrustGrade::Gold);
    }

    #[test]
    fn test_blocked() {
        let mut h = TrustHistory::new(1, 1000);
        h.apply_delta(-450, 1001); // 500 → 50
        assert!(h.is_blocked());
        assert_eq!(h.grade(), TrustGrade::Blocked);
    }

    #[test]
    fn test_history_size_limit() {
        let mut h = TrustHistory::new(1, 1000);
        for i in 0..300 {
            h.record_action(AgentAction::new(1, ActionType::TaskComplete, i, true));
        }
        assert_eq!(h.action_count(), MAX_HISTORY_SIZE);
    }

    #[test]
    fn test_recent_actions() {
        let mut h = TrustHistory::new(1, 1000);
        for i in 0..10 {
            h.record_action(AgentAction::new(1, ActionType::TaskComplete, i, true));
        }
        let recent = h.recent_actions(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].timestamp, 7);
    }
}
