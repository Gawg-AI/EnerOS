//! 经济激励策略与权限门控。

use crate::types::{PermissionLevel, ReputationGrade, ReputationScore};
use crate::RepError;

/// 激励策略 trait。
pub trait IncentivePolicy {
    /// 根据信誉分计算收益分成比例。
    fn revenue_share(&self, score: &ReputationScore, base: f64) -> f64;
    /// 根据信誉分确定权限等级。
    fn permission_level(&self, score: &ReputationScore) -> PermissionLevel;
    /// 根据信誉分计算手续费折扣。
    fn fee_discount(&self, score: &ReputationScore) -> f64;
}

/// 线性激励策略实现。
///
/// - Grade A: share=1.0, permission=Full, fee_discount=0.2
/// - Grade B: share=0.9, permission=Full, fee_discount=0.1
/// - Grade C: share=0.7, permission=Restricted, fee_discount=0.0
/// - Grade D: share=0.5, permission=Banned, fee_discount=0.0
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearIncentive;

impl LinearIncentive {
    /// 创建线性激励策略。
    pub fn new() -> Self {
        Self
    }

    /// 获取等级对应的收益分成系数。
    fn share_multiplier(grade: ReputationGrade) -> f64 {
        match grade {
            ReputationGrade::A => 1.0,
            ReputationGrade::B => 0.9,
            ReputationGrade::C => 0.7,
            ReputationGrade::D => 0.5,
        }
    }

    /// 获取等级对应的手续费折扣。
    fn discount_for_grade(grade: ReputationGrade) -> f64 {
        match grade {
            ReputationGrade::A => 0.2,
            ReputationGrade::B => 0.1,
            ReputationGrade::C | ReputationGrade::D => 0.0,
        }
    }

    /// 获取等级对应的权限等级。
    fn permission_for_grade(grade: ReputationGrade) -> PermissionLevel {
        match grade {
            ReputationGrade::A | ReputationGrade::B => PermissionLevel::Full,
            ReputationGrade::C => PermissionLevel::Restricted,
            ReputationGrade::D => PermissionLevel::Banned,
        }
    }
}

impl IncentivePolicy for LinearIncentive {
    fn revenue_share(&self, score: &ReputationScore, base: f64) -> f64 {
        base * Self::share_multiplier(score.grade)
    }

    fn permission_level(&self, score: &ReputationScore) -> PermissionLevel {
        Self::permission_for_grade(score.grade)
    }

    fn fee_discount(&self, score: &ReputationScore) -> f64 {
        Self::discount_for_grade(score.grade)
    }
}

/// 受限等级最大交易量 (kWh)。
const RESTRICTED_VOLUME_LIMIT: f64 = 100.0;

/// 权限门控。
#[derive(Debug, Clone, Copy, Default)]
pub struct PermissionGate;

impl PermissionGate {
    /// 创建权限门控。
    pub fn new() -> Self {
        Self
    }

    /// 检查 Agent 是否允许交易。
    ///
    /// - Banned: 拒绝所有交易。
    /// - Restricted: 交易量不得超过 100 kWh。
    /// - Full / Probation: 无限制。
    pub fn allow_trade(&self, agent_score: &ReputationScore, volume: f64) -> Result<(), RepError> {
        let policy = LinearIncentive::new();
        let level = policy.permission_level(agent_score);
        match level {
            PermissionLevel::Banned => Err(RepError::Banned),
            PermissionLevel::Restricted => {
                if volume > RESTRICTED_VOLUME_LIMIT {
                    Err(RepError::VolumeExceedsLimit)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_with_grade(agent: u64, score: f64) -> ReputationScore {
        ReputationScore {
            agent,
            score,
            grade: ReputationGrade::from_score(score),
            updated_at: 0,
        }
    }

    #[test]
    fn test_grade_a_share_permission_discount() {
        let policy = LinearIncentive::new();
        let rs = score_with_grade(1, 85.0);
        assert_eq!(policy.revenue_share(&rs, 100.0), 100.0);
        assert_eq!(policy.permission_level(&rs), PermissionLevel::Full);
        assert_eq!(policy.fee_discount(&rs), 0.2);
    }

    #[test]
    fn test_grade_b_share_permission_discount() {
        let policy = LinearIncentive::new();
        let rs = score_with_grade(1, 70.0);
        assert_eq!(policy.revenue_share(&rs, 100.0), 90.0);
        assert_eq!(policy.permission_level(&rs), PermissionLevel::Full);
        assert_eq!(policy.fee_discount(&rs), 0.1);
    }

    #[test]
    fn test_grade_c_share_permission_discount() {
        let policy = LinearIncentive::new();
        let rs = score_with_grade(1, 50.0);
        assert_eq!(policy.revenue_share(&rs, 100.0), 70.0);
        assert_eq!(policy.permission_level(&rs), PermissionLevel::Restricted);
        assert_eq!(policy.fee_discount(&rs), 0.0);
    }

    #[test]
    fn test_grade_d_share_permission_discount() {
        let policy = LinearIncentive::new();
        let rs = score_with_grade(1, 20.0);
        assert_eq!(policy.revenue_share(&rs, 100.0), 50.0);
        assert_eq!(policy.permission_level(&rs), PermissionLevel::Banned);
        assert_eq!(policy.fee_discount(&rs), 0.0);
    }

    #[test]
    fn test_banned_rejects_all() {
        let gate = PermissionGate::new();
        let rs = score_with_grade(1, 10.0); // Grade D -> Banned
        assert_eq!(gate.allow_trade(&rs, 1.0), Err(RepError::Banned));
        assert_eq!(gate.allow_trade(&rs, 0.0), Err(RepError::Banned));
    }

    #[test]
    fn test_restricted_volume_limit() {
        let gate = PermissionGate::new();
        let rs = score_with_grade(1, 50.0); // Grade C -> Restricted
        assert_eq!(gate.allow_trade(&rs, 100.0), Ok(()));
        assert_eq!(gate.allow_trade(&rs, 50.0), Ok(()));
        assert_eq!(
            gate.allow_trade(&rs, 100.1),
            Err(RepError::VolumeExceedsLimit)
        );
    }

    #[test]
    fn test_full_permission_no_limit() {
        let gate = PermissionGate::new();
        let rs = score_with_grade(1, 90.0); // Grade A -> Full
        assert_eq!(gate.allow_trade(&rs, 10000.0), Ok(()));
    }
}
