//! 模型融合模块
//!
//! 支持多种融合策略，将多源模型输出合并为统一预测值。

extern crate alloc;

use crate::types::FusionStrategy;

/// 模型融合器
#[derive(Debug)]
pub struct ModelFusion {
    /// 融合策略
    pub strategy: FusionStrategy,
}

impl ModelFusion {
    /// 创建融合器
    pub fn new(strategy: FusionStrategy) -> Self {
        Self { strategy }
    }

    /// 执行融合（v0.233.0：策略真实分化）。
    ///
    /// - [`FusionStrategy::WeightedAverage`]：加权平均；
    /// - [`FusionStrategy::BayesianFusion`]：方差倒数加权（真实贝叶斯
    ///   语义——方差小（置信高）的模型权重高）；
    /// - [`FusionStrategy::NeuralSurrogate`]：裁剪为加权平均并正名
    ///   （神经代理模型无真实后端，v0.233.0 裁决）。
    ///
    /// 权重长度不匹配或全零时返回 0.0。
    pub fn fuse(&self, values: &[f64], weights: &[f32]) -> f64 {
        match self.strategy {
            FusionStrategy::BayesianFusion => self.fuse_bayesian(values, weights),
            _ => self.fuse_weighted(values, weights),
        }
    }

    /// 加权平均。
    pub fn fuse_weighted(&self, values: &[f64], weights: &[f32]) -> f64 {
        if values.is_empty() || values.len() != weights.len() {
            return 0.0;
        }

        let weight_sum: f32 = weights.iter().sum();
        if weight_sum.abs() < f32::EPSILON {
            return 0.0;
        }

        let weighted_sum: f64 = values
            .iter()
            .zip(weights.iter())
            .map(|(&v, &w)| v * w as f64)
            .sum();

        weighted_sum / weight_sum as f64
    }

    /// 贝叶斯融合：方差倒数加权。
    ///
    /// 每个模型的权重 = 1/σ²（σ 为模型置信度方差，由 weights 表达
    /// 为 0..1 置信度 → σ = 1 - w + ε）。融合值 =
    /// Σ(v_i/σ_i²) / Σ(1/σ_i²)。
    pub fn fuse_bayesian(&self, values: &[f64], weights: &[f32]) -> f64 {
        if values.is_empty() || values.len() != weights.len() {
            return 0.0;
        }
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for (&v, &w) in values.iter().zip(weights.iter()) {
            let conf = w.clamp(0.0, 1.0) as f64;
            // σ = 1 - conf + ε（置信 1 → 方差极小 → 权重极大）。
            let sigma = (1.0 - conf + 1e-3).max(1e-3);
            let inv_var = 1.0 / (sigma * sigma);
            num += v * inv_var;
            den += inv_var;
        }
        if den.abs() < f64::EPSILON {
            0.0
        } else {
            num / den
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equal_weights() {
        let fusion = ModelFusion::new(FusionStrategy::WeightedAverage);
        let values = [10.0, 20.0, 30.0];
        let weights = [1.0f32, 1.0, 1.0];
        let result = fusion.fuse(&values, &weights);
        assert!((result - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_custom_weights() {
        let fusion = ModelFusion::new(FusionStrategy::WeightedAverage);
        let values = [100.0, 200.0];
        let weights = [3.0f32, 1.0];
        let result = fusion.fuse(&values, &weights);
        // (100*3 + 200*1) / 4 = 125
        assert!((result - 125.0).abs() < 1e-9);
    }

    #[test]
    fn test_empty_values() {
        let fusion = ModelFusion::new(FusionStrategy::BayesianFusion);
        let result = fusion.fuse(&[], &[]);
        assert!((result - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_mismatched_lengths() {
        let fusion = ModelFusion::new(FusionStrategy::NeuralSurrogate);
        let values = [1.0, 2.0, 3.0];
        let weights = [1.0f32, 1.0];
        let result = fusion.fuse(&values, &weights);
        assert!((result - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_zero_weights() {
        let fusion = ModelFusion::new(FusionStrategy::WeightedAverage);
        let values = [5.0, 10.0];
        let weights = [0.0f32, 0.0];
        let result = fusion.fuse(&values, &weights);
        assert!((result - 0.0).abs() < f64::EPSILON);
    }

    // ===== v0.233.0（B3-10）：策略真实分化 =====

    #[test]
    fn test_bayesian_high_confidence_dominates() {
        // 高置信（w→1）模型主导融合结果。
        let f = ModelFusion::new(FusionStrategy::BayesianFusion);
        let values = [100.0, 90.0];
        let weights = [1.0, 0.1]; // 模型 1 置信极高
        let r = f.fuse(&values, &weights);
        // 结果应显著偏向 100（> 加权平均 99.09）。
        assert!(r > 99.0, "bayesian result {r}");
        assert!(r < 100.5);
    }

    #[test]
    fn test_bayesian_equal_confidence_average() {
        let f = ModelFusion::new(FusionStrategy::BayesianFusion);
        let values = [100.0, 90.0];
        let weights = [0.5, 0.5];
        let r = f.fuse(&values, &weights);
        assert!((r - 95.0).abs() < 0.5, "对称权重 → 均值附近 {r}");
    }

    #[test]
    fn test_neural_surrogate_degrades_to_weighted() {
        // 神经代理正名为加权平均。
        let f = ModelFusion::new(FusionStrategy::NeuralSurrogate);
        let values = [10.0, 20.0];
        let weights = [0.25, 0.75];
        let r = f.fuse(&values, &weights);
        assert!((r - 17.5).abs() < 1e-9, "neural surrogate → weighted {r}");
    }

    #[test]
    fn test_bayesian_empty_rejected() {
        let f = ModelFusion::new(FusionStrategy::BayesianFusion);
        assert_eq!(f.fuse(&[], &[]), 0.0);
    }

    #[test]
    fn test_single_value() {
        let fusion = ModelFusion::new(FusionStrategy::WeightedAverage);
        let values = [42.0];
        let weights = [2.0f32];
        let result = fusion.fuse(&values, &weights);
        assert!((result - 42.0).abs() < 1e-9);
    }
}
