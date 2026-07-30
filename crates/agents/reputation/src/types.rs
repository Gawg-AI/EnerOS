//! 信誉经济激励核心数据类型。

/// 信誉等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReputationGrade {
    /// 最低等级（score < 40）。
    D,
    /// 较低等级（40 <= score < 60）。
    C,
    /// 中等级（60 <= score < 80）。
    B,
    /// 最高等级（score >= 80）。
    A,
}

impl ReputationGrade {
    /// 根据分数计算等级。
    ///
    /// - A: score >= 80
    /// - B: score >= 60
    /// - C: score >= 40
    /// - D: score < 40
    pub fn from_score(score: f64) -> Self {
        if score >= 80.0 {
            Self::A
        } else if score >= 60.0 {
            Self::B
        } else if score >= 40.0 {
            Self::C
        } else {
            Self::D
        }
    }
}

/// 权限等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// 完全权限。
    Full,
    /// 受限权限。
    Restricted,
    /// 观察期。
    Probation,
    /// 已封禁。
    Banned,
}

/// 信誉事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReputationKind {
    /// 按时交付。
    OnTimeDelivery,
    /// 偏差。
    Deviation,
    /// 争议。
    Dispute,
    /// 市场响应度。
    MarketResponsiveness,
    /// 违规行为。
    Misbehavior,
}

/// 信誉分数记录。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReputationScore {
    /// Agent ID。
    pub agent: u64,
    /// 信誉分（0~100）。
    pub score: f64,
    /// 当前等级。
    pub grade: ReputationGrade,
    /// 最后更新时间戳。
    pub updated_at: u64,
}

/// 信誉事件。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReputationEvent {
    /// Agent ID。
    pub agent: u64,
    /// 事件类型。
    pub kind: ReputationKind,
    /// 分数变化量。
    pub delta: f64,
    /// 关联交易 ID（可选）。
    pub ref_deal: Option<u64>,
    /// 事件时间戳。
    pub at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grade_from_score_a() {
        assert_eq!(ReputationGrade::from_score(80.0), ReputationGrade::A);
        assert_eq!(ReputationGrade::from_score(95.0), ReputationGrade::A);
        assert_eq!(ReputationGrade::from_score(100.0), ReputationGrade::A);
    }

    #[test]
    fn test_grade_from_score_b() {
        assert_eq!(ReputationGrade::from_score(60.0), ReputationGrade::B);
        assert_eq!(ReputationGrade::from_score(79.9), ReputationGrade::B);
    }

    #[test]
    fn test_grade_from_score_c() {
        assert_eq!(ReputationGrade::from_score(40.0), ReputationGrade::C);
        assert_eq!(ReputationGrade::from_score(59.9), ReputationGrade::C);
    }

    #[test]
    fn test_grade_from_score_d() {
        assert_eq!(ReputationGrade::from_score(0.0), ReputationGrade::D);
        assert_eq!(ReputationGrade::from_score(39.9), ReputationGrade::D);
    }

    #[test]
    fn test_grade_ordering() {
        assert!(ReputationGrade::A > ReputationGrade::B);
        assert!(ReputationGrade::B > ReputationGrade::C);
        assert!(ReputationGrade::C > ReputationGrade::D);
    }

    #[test]
    fn test_reputation_score_creation() {
        let rs = ReputationScore {
            agent: 1,
            score: 75.0,
            grade: ReputationGrade::B,
            updated_at: 1000,
        };
        assert_eq!(rs.agent, 1);
        assert_eq!(rs.score, 75.0);
        assert_eq!(rs.grade, ReputationGrade::B);
        assert_eq!(rs.updated_at, 1000);
    }

    #[test]
    fn test_reputation_event_creation() {
        let ev = ReputationEvent {
            agent: 2,
            kind: ReputationKind::OnTimeDelivery,
            delta: 5.0,
            ref_deal: Some(100),
            at: 2000,
        };
        assert_eq!(ev.agent, 2);
        assert_eq!(ev.kind, ReputationKind::OnTimeDelivery);
        assert_eq!(ev.delta, 5.0);
        assert_eq!(ev.ref_deal, Some(100));
        assert_eq!(ev.at, 2000);
    }

    #[test]
    fn test_permission_level_equality() {
        assert_eq!(PermissionLevel::Full, PermissionLevel::Full);
        assert_ne!(PermissionLevel::Full, PermissionLevel::Banned);
    }
}
