//! 动态调整器 (v0.168.0).
//!
//! 实现时间衰减（防刷分）和手动调整。

use crate::types::{TrustScore, INITIAL_SCORE, MAX_SCORE, MIN_SCORE};

/// 时间衰减配置.
///
/// 历史行为影响力随时间衰减，防止"刷分"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayConfig {
    /// 衰减周期（时间单位）.
    pub period: u64,
    /// 每周期衰减量（向初始分回归）.
    pub decay_per_period: u16,
    /// 是否启用衰减.
    pub enabled: bool,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            period: 3600,        // 每 3600 时间单位
            decay_per_period: 1, // 向初始分回归 1 分
            enabled: true,
        }
    }
}

/// 动态调整器.
///
/// 负责时间衰减计算和手动分数调整。
#[derive(Default)]
pub struct TrustAdjuster {
    /// 衰减配置.
    pub decay_config: DecayConfig,
}

impl TrustAdjuster {
    /// 创建调整器.
    pub fn new(decay_config: DecayConfig) -> Self {
        Self { decay_config }
    }

    /// 计算时间衰减后的分数.
    ///
    /// 分数向初始分（500）回归：
    /// - 高于 500 → 缓慢下降
    /// - 低于 500 → 缓慢上升
    /// - 等于 500 → 不变
    pub fn apply_decay(&self, current_score: TrustScore, elapsed: u64) -> TrustScore {
        if !self.decay_config.enabled || self.decay_config.period == 0 {
            return current_score;
        }

        let periods = elapsed / self.decay_config.period;
        if periods == 0 {
            return current_score;
        }

        let total_decay = (periods as u32) * (self.decay_config.decay_per_period as u32);

        if current_score > INITIAL_SCORE {
            // 高于初始分 → 下降
            let decay = total_decay.min((current_score - INITIAL_SCORE) as u32);
            current_score - decay as u16
        } else if current_score < INITIAL_SCORE {
            // 低于初始分 → 上升
            let recovery = total_decay.min((INITIAL_SCORE - current_score) as u32);
            current_score + recovery as u16
        } else {
            current_score
        }
    }

    /// 手动调整分数.
    ///
    /// 管理员可直接调整 Agent 信任分（奖励/惩罚）。
    pub fn manual_adjust(&self, current_score: TrustScore, delta: i32) -> TrustScore {
        (current_score as i32 + delta).clamp(MIN_SCORE as i32, MAX_SCORE as i32) as u16
    }

    /// 计算衰减周期数.
    pub fn periods_elapsed(&self, elapsed: u64) -> u64 {
        if self.decay_config.period == 0 {
            return 0;
        }
        elapsed / self.decay_config.period
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_decay_when_disabled() {
        let config = DecayConfig {
            enabled: false,
            ..Default::default()
        };
        let adj = TrustAdjuster::new(config);
        assert_eq!(adj.apply_decay(800, 99999), 800);
    }

    #[test]
    fn test_decay_high_score() {
        let adj = TrustAdjuster::default();
        // 800 分，经过 3 个周期 → 衰减 3 分 → 797
        assert_eq!(adj.apply_decay(800, 3600 * 3), 797);
    }

    #[test]
    fn test_decay_low_score_recovers() {
        let adj = TrustAdjuster::default();
        // 200 分，经过 5 个周期 → 恢复 5 分 → 205
        assert_eq!(adj.apply_decay(200, 3600 * 5), 205);
    }

    #[test]
    fn test_decay_at_initial_no_change() {
        let adj = TrustAdjuster::default();
        assert_eq!(adj.apply_decay(INITIAL_SCORE, 3600 * 100), INITIAL_SCORE);
    }

    #[test]
    fn test_decay_does_not_cross_initial() {
        let adj = TrustAdjuster::default();
        // 502 分，经过 10 个周期 → 衰减 10 分，但不低于 500
        assert_eq!(adj.apply_decay(502, 3600 * 10), 500);
    }

    #[test]
    fn test_decay_partial_period() {
        let adj = TrustAdjuster::default();
        // 不足一个周期不衰减
        assert_eq!(adj.apply_decay(800, 3599), 800);
    }

    #[test]
    fn test_manual_adjust() {
        let adj = TrustAdjuster::default();
        assert_eq!(adj.manual_adjust(500, 100), 600);
        assert_eq!(adj.manual_adjust(500, -600), 0); // clamp
        assert_eq!(adj.manual_adjust(900, 200), 1000); // clamp
    }

    #[test]
    fn test_periods_elapsed() {
        let adj = TrustAdjuster::default();
        assert_eq!(adj.periods_elapsed(7200), 2);
        assert_eq!(adj.periods_elapsed(3600), 1);
        assert_eq!(adj.periods_elapsed(100), 0);
    }
}
