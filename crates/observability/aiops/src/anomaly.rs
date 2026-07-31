//! 异常检测模块。

use alloc::format;
use alloc::string::String;

use crate::types::AnomalyResult;

/// 异常检测器
#[derive(Debug, Clone)]
pub struct AnomalyDetector {
    /// 灵敏度阈值 (score > sensitivity 则判定为异常)
    pub sensitivity: f32,
}

impl AnomalyDetector {
    /// 创建异常检测器
    pub fn new(sensitivity: f32) -> Self {
        Self { sensitivity }
    }

    /// 检测指标是否异常
    ///
    /// - `metric`: 指标名称
    /// - `value`: 当前值
    /// - `baseline`: 基线值
    /// - `ts`: 时间戳
    ///
    /// score = |value - baseline| / baseline (相对偏差)
    /// is_anomaly = score > sensitivity
    pub fn detect(&self, metric: &str, value: f32, baseline: f32, ts: u64) -> AnomalyResult {
        let score = if baseline.abs() < f32::EPSILON {
            // 基线为零时，任何非零值都视为最大偏差
            if value.abs() < f32::EPSILON {
                0.0
            } else {
                f32::INFINITY
            }
        } else {
            (value - baseline).abs() / baseline.abs()
        };

        let is_anomaly = score > self.sensitivity;

        let explanation = if is_anomaly {
            format!(
                "指标 {} 偏离基线 {:.2}%，超过阈值 {:.2}%",
                metric,
                score * 100.0,
                self.sensitivity * 100.0
            )
        } else {
            format!("指标 {} 在正常范围内", metric)
        };

        AnomalyResult {
            metric: String::from(metric),
            score,
            is_anomaly,
            ts,
            explanation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_value() {
        let detector = AnomalyDetector::new(0.3);
        let result = detector.detect("temperature", 102.0, 100.0, 1000);
        // score = |102 - 100| / 100 = 0.02
        assert!(!result.is_anomaly);
        assert!((result.score - 0.02).abs() < 1e-5);
        assert_eq!(result.ts, 1000);
    }

    #[test]
    fn test_anomalous_value() {
        let detector = AnomalyDetector::new(0.3);
        let result = detector.detect("cpu_usage", 95.0, 50.0, 2000);
        // score = |95 - 50| / 50 = 0.9
        assert!(result.is_anomaly);
        assert!((result.score - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_zero_baseline() {
        let detector = AnomalyDetector::new(0.3);
        // 基线为零，值非零 → 异常
        let result = detector.detect("error_count", 5.0, 0.0, 3000);
        assert!(result.is_anomaly);
        assert!(result.score.is_infinite());

        // 基线为零，值也为零 → 正常
        let result_zero = detector.detect("error_count", 0.0, 0.0, 3001);
        assert!(!result_zero.is_anomaly);
        assert_eq!(result_zero.score, 0.0);
    }
}
