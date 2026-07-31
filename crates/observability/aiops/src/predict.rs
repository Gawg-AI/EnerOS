//! 预测性维护模块。

use crate::types::{PredictionResult, RiskLevel};

/// 预测性维护引擎
#[derive(Debug, Clone)]
pub struct PredictiveMaintenance {
    /// 预测时间窗口
    pub horizon: usize,
}

impl PredictiveMaintenance {
    /// 创建预测性维护引擎
    pub fn new(horizon: usize) -> Self {
        Self { horizon }
    }

    /// 预测资产剩余使用寿命
    ///
    /// 基于健康分数序列的线性外推 (mock 实现):
    /// - 计算健康分数的线性趋势 (斜率)
    /// - 外推到健康分数降至 0 时的时间步数
    /// - 根据 RUL 阈值确定风险等级
    pub fn predict(&self, asset_id: u64, health_scores: &[f32]) -> PredictionResult {
        let (rul, confidence) = if health_scores.len() < 2 {
            // 数据不足，假设健康
            (self.horizon as f64 * 10.0, 0.3)
        } else {
            let n = health_scores.len();
            let last = health_scores[n - 1] as f64;

            // 简单线性回归斜率: slope = (last - first) / (n - 1)
            let first = health_scores[0] as f64;
            let slope = (last - first) / (n as f64 - 1.0);

            if slope >= 0.0 {
                // 健康分数稳定或上升，RUL 很长
                (self.horizon as f64 * 10.0, 0.85)
            } else {
                // 外推: 多少步后降到 0
                let steps_to_zero = last / (-slope);
                let rul = steps_to_zero.min(self.horizon as f64 * 10.0);
                // 数据点越多置信度越高
                let confidence = ((n as f32).min(10.0) / 10.0).max(0.4);
                (rul, confidence)
            }
        };

        let risk_level = Self::risk_from_rul(rul);

        PredictionResult {
            asset_id,
            remaining_useful_life: rul,
            confidence,
            risk_level,
        }
    }

    /// 根据 RUL 确定风险等级
    fn risk_from_rul(rul: f64) -> RiskLevel {
        if rul > 500.0 {
            RiskLevel::Low
        } else if rul > 200.0 {
            RiskLevel::Medium
        } else if rul > 50.0 {
            RiskLevel::High
        } else {
            RiskLevel::Critical
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_asset() {
        let pm = PredictiveMaintenance::new(100);
        // 健康分数稳定在高位
        let scores = [0.95, 0.94, 0.95, 0.96, 0.95];
        let result = pm.predict(1, &scores);
        assert_eq!(result.asset_id, 1);
        assert_eq!(result.risk_level, RiskLevel::Low);
        assert!(result.remaining_useful_life > 500.0);
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_degrading_asset() {
        let pm = PredictiveMaintenance::new(100);
        // 健康分数持续下降
        let scores = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3];
        let result = pm.predict(2, &scores);
        assert_eq!(result.asset_id, 2);
        // 斜率 = (0.3 - 0.9) / 6 = -0.1, RUL = 0.3 / 0.1 = 3
        assert!(result.remaining_useful_life < 50.0);
        assert_eq!(result.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn test_insufficient_data() {
        let pm = PredictiveMaintenance::new(50);
        let scores = [0.8];
        let result = pm.predict(3, &scores);
        // 数据不足，默认较长 RUL
        assert!(result.remaining_useful_life > 100.0);
        assert!(result.confidence < 0.5);
    }
}
