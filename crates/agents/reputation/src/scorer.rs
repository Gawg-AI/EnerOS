//! 信誉评分引擎。

use alloc::vec::Vec;

use crate::types::{ReputationEvent, ReputationGrade, ReputationKind, ReputationScore};
use crate::RepError;

/// 信誉引擎 trait。
pub trait ReputationEngine {
    /// 应用信誉事件，更新 Agent 分数。
    fn update(&mut self, event: &ReputationEvent) -> Result<ReputationScore, RepError>;
    /// 查询 Agent 当前信誉分。
    fn score(&self, agent: u64) -> Result<ReputationScore, RepError>;
}

/// 默认信誉分（新 Agent 初始值）。
const DEFAULT_SCORE: f64 = 50.0;

/// 信誉引擎实现。
#[derive(Debug, Clone, Default)]
pub struct ReputationEngineImpl {
    /// 所有 Agent 的信誉记录。
    scores: Vec<ReputationScore>,
}

impl ReputationEngineImpl {
    /// 创建空引擎。
    pub fn new() -> Self {
        Self { scores: Vec::new() }
    }

    /// 获取事件类型对应的默认 delta。
    pub fn default_delta(kind: ReputationKind) -> f64 {
        match kind {
            ReputationKind::OnTimeDelivery => 5.0,
            ReputationKind::Deviation => -10.0,
            ReputationKind::Dispute => -15.0,
            ReputationKind::MarketResponsiveness => 3.0,
            ReputationKind::Misbehavior => -30.0,
        }
    }
}

impl ReputationEngine for ReputationEngineImpl {
    fn update(&mut self, event: &ReputationEvent) -> Result<ReputationScore, RepError> {
        let delta = if event.delta != 0.0 {
            event.delta
        } else {
            Self::default_delta(event.kind)
        };

        if let Some(entry) = self.scores.iter_mut().find(|s| s.agent == event.agent) {
            let new_score = (entry.score + delta).clamp(0.0, 100.0);
            entry.score = new_score;
            entry.grade = ReputationGrade::from_score(new_score);
            entry.updated_at = event.at;
            Ok(*entry)
        } else {
            let new_score = (DEFAULT_SCORE + delta).clamp(0.0, 100.0);
            let rs = ReputationScore {
                agent: event.agent,
                score: new_score,
                grade: ReputationGrade::from_score(new_score),
                updated_at: event.at,
            };
            self.scores.push(rs);
            Ok(rs)
        }
    }

    fn score(&self, agent: u64) -> Result<ReputationScore, RepError> {
        self.scores
            .iter()
            .find(|s| s.agent == agent)
            .copied()
            .ok_or(RepError::AgentNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(agent: u64, kind: ReputationKind, delta: f64, at: u64) -> ReputationEvent {
        ReputationEvent {
            agent,
            kind,
            delta,
            ref_deal: None,
            at,
        }
    }

    #[test]
    fn test_initial_score_not_found() {
        let engine = ReputationEngineImpl::new();
        assert_eq!(engine.score(1), Err(RepError::AgentNotFound));
    }

    #[test]
    fn test_positive_event() {
        let mut engine = ReputationEngineImpl::new();
        let ev = make_event(1, ReputationKind::OnTimeDelivery, 5.0, 100);
        let rs = engine.update(&ev).unwrap();
        // 默认 50 + 5 = 55
        assert_eq!(rs.score, 55.0);
        assert_eq!(rs.grade, ReputationGrade::C);
        assert_eq!(rs.updated_at, 100);
    }

    #[test]
    fn test_negative_event() {
        let mut engine = ReputationEngineImpl::new();
        let ev = make_event(1, ReputationKind::Misbehavior, -30.0, 200);
        let rs = engine.update(&ev).unwrap();
        // 默认 50 - 30 = 20
        assert_eq!(rs.score, 20.0);
        assert_eq!(rs.grade, ReputationGrade::D);
    }

    #[test]
    fn test_clamping_upper() {
        let mut engine = ReputationEngineImpl::new();
        // 先给一个高分
        let ev1 = make_event(1, ReputationKind::OnTimeDelivery, 60.0, 100);
        engine.update(&ev1).unwrap();
        // 50 + 60 = 110 -> clamp to 100
        let rs = engine.score(1).unwrap();
        assert_eq!(rs.score, 100.0);
        assert_eq!(rs.grade, ReputationGrade::A);
    }

    #[test]
    fn test_clamping_lower() {
        let mut engine = ReputationEngineImpl::new();
        let ev = make_event(1, ReputationKind::Misbehavior, -80.0, 100);
        let rs = engine.update(&ev).unwrap();
        // 50 - 80 = -30 -> clamp to 0
        assert_eq!(rs.score, 0.0);
        assert_eq!(rs.grade, ReputationGrade::D);
    }

    #[test]
    fn test_grade_transitions() {
        let mut engine = ReputationEngineImpl::new();
        // Start at C (50), go to B (50+15=65)
        let ev1 = make_event(1, ReputationKind::OnTimeDelivery, 15.0, 100);
        let rs1 = engine.update(&ev1).unwrap();
        assert_eq!(rs1.grade, ReputationGrade::B);

        // Go to A (65+20=85)
        let ev2 = make_event(1, ReputationKind::OnTimeDelivery, 20.0, 200);
        let rs2 = engine.update(&ev2).unwrap();
        assert_eq!(rs2.grade, ReputationGrade::A);

        // Drop to D (85-50=35)
        let ev3 = make_event(1, ReputationKind::Misbehavior, -50.0, 300);
        let rs3 = engine.update(&ev3).unwrap();
        assert_eq!(rs3.grade, ReputationGrade::D);
    }

    #[test]
    fn test_default_delta_values() {
        assert_eq!(
            ReputationEngineImpl::default_delta(ReputationKind::OnTimeDelivery),
            5.0
        );
        assert_eq!(
            ReputationEngineImpl::default_delta(ReputationKind::Deviation),
            -10.0
        );
        assert_eq!(
            ReputationEngineImpl::default_delta(ReputationKind::Dispute),
            -15.0
        );
        assert_eq!(
            ReputationEngineImpl::default_delta(ReputationKind::MarketResponsiveness),
            3.0
        );
        assert_eq!(
            ReputationEngineImpl::default_delta(ReputationKind::Misbehavior),
            -30.0
        );
    }

    #[test]
    fn test_zero_delta_uses_default() {
        let mut engine = ReputationEngineImpl::new();
        // delta=0 should use default for OnTimeDelivery (+5)
        let ev = make_event(1, ReputationKind::OnTimeDelivery, 0.0, 100);
        let rs = engine.update(&ev).unwrap();
        assert_eq!(rs.score, 55.0);
    }
}
