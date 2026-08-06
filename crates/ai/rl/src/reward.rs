//! Reward 设计 (workspace v0.192.0).
//!
//! 收益-成本-安全惩罚加权 Reward Shaping。

use crate::types::RewardCtx;

/// Reward 整形器 trait.
pub trait RewardShaper {
    /// 根据上下文计算奖励值.
    fn shape(&self, ctx: &RewardCtx) -> f32;
}

/// 默认 Reward 整形器.
///
/// reward = w_revenue * (revenue - cost) - w_safety * safety_penalty - w_soc * soc_penalty
pub struct DefaultReward {
    /// 收益权重.
    pub w_revenue: f64,
    /// 安全惩罚权重.
    pub w_safety: f64,
    /// SOC 偏差惩罚权重.
    pub w_soc: f64,
}

impl Default for DefaultReward {
    fn default() -> Self {
        Self {
            w_revenue: 1.0,
            w_safety: 10.0,
            w_soc: 0.5,
        }
    }
}

impl DefaultReward {
    /// 创建自定义权重的 Reward 整形器.
    pub fn new(w_revenue: f64, w_safety: f64, w_soc: f64) -> Self {
        Self {
            w_revenue,
            w_safety,
            w_soc,
        }
    }
}

impl RewardShaper for DefaultReward {
    fn shape(&self, ctx: &RewardCtx) -> f32 {
        let profit = ctx.revenue_yuan - ctx.cost_yuan;
        let safety_pen = ctx.safety_violation as f64 * 10.0;
        let soc_pen = (ctx.soc_deviation.abs() as f64) * 0.5;
        let lag_pen = (ctx.market_response_lag_s as f64) * 0.1;

        let reward =
            self.w_revenue * profit - self.w_safety * safety_pen - self.w_soc * soc_pen - lag_pen;

        reward as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positive_reward() {
        let shaper = DefaultReward::default();
        let ctx = RewardCtx {
            revenue_yuan: 100.0,
            cost_yuan: 30.0,
            soc_deviation: 0.0,
            safety_violation: 0,
            market_response_lag_s: 0.0,
        };
        let r = shaper.shape(&ctx);
        assert!((r - 70.0).abs() < 1e-4);
    }

    #[test]
    fn test_safety_penalty() {
        let shaper = DefaultReward::default();
        let ctx = RewardCtx {
            revenue_yuan: 100.0,
            cost_yuan: 0.0,
            soc_deviation: 0.0,
            safety_violation: 2,
            market_response_lag_s: 0.0,
        };
        // 100 - 10*(2*10) = 100 - 200 = -100
        let r = shaper.shape(&ctx);
        assert!(r < 0.0);
    }

    #[test]
    fn test_soc_penalty() {
        let shaper = DefaultReward::default();
        let ctx = RewardCtx {
            revenue_yuan: 50.0,
            cost_yuan: 0.0,
            soc_deviation: 0.8,
            safety_violation: 0,
            market_response_lag_s: 0.0,
        };
        // 50 - 0.5*(0.8*0.5) = 50 - 0.2 = 49.8
        let r = shaper.shape(&ctx);
        assert!((r - 49.8).abs() < 0.01);
    }

    #[test]
    fn test_custom_weights() {
        let shaper = DefaultReward::new(2.0, 0.0, 0.0);
        let ctx = RewardCtx {
            revenue_yuan: 50.0,
            cost_yuan: 20.0,
            soc_deviation: 0.5,
            safety_violation: 1,
            market_response_lag_s: 0.0,
        };
        // 2.0 * (50-20) = 60 (safety/soc weights are 0)
        let r = shaper.shape(&ctx);
        assert!((r - 60.0).abs() < 1e-4);
    }

    #[test]
    fn test_negative_profit() {
        let shaper = DefaultReward::default();
        let ctx = RewardCtx {
            revenue_yuan: 10.0,
            cost_yuan: 50.0,
            soc_deviation: 0.0,
            safety_violation: 0,
            market_response_lag_s: 0.0,
        };
        let r = shaper.shape(&ctx);
        assert!(r < 0.0);
    }
}
