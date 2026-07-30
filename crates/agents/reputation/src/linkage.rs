//! 信誉→权限→收益联动表。

use crate::types::{PermissionLevel, ReputationGrade, ReputationScore};

/// 联动表：将信誉等级映射为权限、收益分成系数与手续费折扣。
#[derive(Debug, Clone, Copy, Default)]
pub struct LinkageTable;

impl LinkageTable {
    /// 创建联动表。
    pub fn new() -> Self {
        Self
    }

    /// 根据信誉分查询联动结果。
    ///
    /// 返回 `(权限等级, 收益分成系数, 手续费折扣)`。
    pub fn link(&self, score: &ReputationScore) -> (PermissionLevel, f64, f64) {
        match score.grade {
            ReputationGrade::A => (PermissionLevel::Full, 1.0, 0.2),
            ReputationGrade::B => (PermissionLevel::Full, 0.9, 0.1),
            ReputationGrade::C => (PermissionLevel::Restricted, 0.7, 0.0),
            ReputationGrade::D => (PermissionLevel::Banned, 0.5, 0.0),
        }
    }

    /// 根据等级直接查询权限。
    pub fn permission_for_grade(&self, grade: ReputationGrade) -> PermissionLevel {
        match grade {
            ReputationGrade::A | ReputationGrade::B => PermissionLevel::Full,
            ReputationGrade::C => PermissionLevel::Restricted,
            ReputationGrade::D => PermissionLevel::Banned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_with_grade(score: f64) -> ReputationScore {
        ReputationScore {
            agent: 1,
            score,
            grade: ReputationGrade::from_score(score),
            updated_at: 0,
        }
    }

    #[test]
    fn test_link_grade_a() {
        let table = LinkageTable::new();
        let rs = score_with_grade(90.0);
        let (perm, share, discount) = table.link(&rs);
        assert_eq!(perm, PermissionLevel::Full);
        assert_eq!(share, 1.0);
        assert_eq!(discount, 0.2);
    }

    #[test]
    fn test_link_grade_b() {
        let table = LinkageTable::new();
        let rs = score_with_grade(65.0);
        let (perm, share, discount) = table.link(&rs);
        assert_eq!(perm, PermissionLevel::Full);
        assert_eq!(share, 0.9);
        assert_eq!(discount, 0.1);
    }

    #[test]
    fn test_link_grade_c() {
        let table = LinkageTable::new();
        let rs = score_with_grade(45.0);
        let (perm, share, discount) = table.link(&rs);
        assert_eq!(perm, PermissionLevel::Restricted);
        assert_eq!(share, 0.7);
        assert_eq!(discount, 0.0);
    }

    #[test]
    fn test_link_grade_d() {
        let table = LinkageTable::new();
        let rs = score_with_grade(20.0);
        let (perm, share, discount) = table.link(&rs);
        assert_eq!(perm, PermissionLevel::Banned);
        assert_eq!(share, 0.5);
        assert_eq!(discount, 0.0);
    }

    #[test]
    fn test_permission_for_grade() {
        let table = LinkageTable::new();
        assert_eq!(
            table.permission_for_grade(ReputationGrade::A),
            PermissionLevel::Full
        );
        assert_eq!(
            table.permission_for_grade(ReputationGrade::B),
            PermissionLevel::Full
        );
        assert_eq!(
            table.permission_for_grade(ReputationGrade::C),
            PermissionLevel::Restricted
        );
        assert_eq!(
            table.permission_for_grade(ReputationGrade::D),
            PermissionLevel::Banned
        );
    }
}
