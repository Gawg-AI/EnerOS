//! 容差档（[09] §3.5：这些值进 ToleranceProfile，不得散落）。
//!
//! - max primal residual ≤ 1e-7 × max(1, scale)
//! - max integrality residual ≤ 1e-6
//! - relative gap 按用途配置：L2 默认只接受 Optimal；L3 可配置 ≤0.5%
//! - elapsed + downstream worst case < deadline

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToleranceProfile {
    /// 档 id（CertifiedPlan.tolerance_profile_id 同源：1=L2，2=L3）。
    pub id: u32,
    /// 原始残差系数（×max(1, scale)）。
    pub primal_residual_factor: f64,
    /// 整数性残差上限。
    pub integrality_residual: f64,
    /// 允许的最大相对 gap（0 = 只接受最优）。
    pub max_relative_gap: f64,
}

impl ToleranceProfile {
    /// L2：站控实时层——只接受 Optimal。
    pub fn l2() -> Self {
        Self {
            id: 1,
            primal_residual_factor: 1e-7,
            integrality_residual: 1e-6,
            max_relative_gap: 0.0,
        }
    }

    /// L3：计划/市场层——允许 ≤0.5% gap。
    pub fn l3() -> Self {
        Self {
            id: 2,
            primal_residual_factor: 1e-7,
            integrality_residual: 1e-6,
            max_relative_gap: 0.005,
        }
    }

    /// 残差上限（按规模缩放）。
    pub fn primal_limit(&self, scale: f64) -> f64 {
        self.primal_residual_factor * (1.0 + scale.max(0.0))
    }

    /// Suboptimal 是否被本档接受。
    pub fn accepts_suboptimal(&self) -> bool {
        self.max_relative_gap > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_and_limits() {
        let l2 = ToleranceProfile::l2();
        let l3 = ToleranceProfile::l3();
        assert_eq!(l2.id, 1);
        assert_eq!(l3.id, 2);
        assert!(!l2.accepts_suboptimal());
        assert!(l3.accepts_suboptimal());
        assert!((l3.max_relative_gap - 0.005).abs() < 1e-12);
        assert!((l2.primal_limit(0.0) - 1e-7).abs() < 1e-18);
        assert!((l2.primal_limit(1000.0) - 1e-7 * 1001.0).abs() < 1e-18);
    }
}
