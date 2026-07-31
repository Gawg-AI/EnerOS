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

    /// 执行加权融合
    ///
    /// 对 `values` 按 `weights` 进行加权平均。
    /// 若权重之和为 0 或长度不匹配，返回 0.0。
    pub fn fuse(&self, values: &[f64], weights: &[f32]) -> f64 {
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

    #[test]
    fn test_single_value() {
        let fusion = ModelFusion::new(FusionStrategy::WeightedAverage);
        let values = [42.0];
        let weights = [2.0f32];
        let result = fusion.fuse(&values, &weights);
        assert!((result - 42.0).abs() < 1e-9);
    }
}
