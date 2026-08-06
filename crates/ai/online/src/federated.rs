//! 联邦学习聚合 (workspace v0.192.0).
//!
//! 实现 FedAvg（联邦平均）算法：按样本数加权聚合各客户端模型参数。

use alloc::vec::Vec;

use crate::AiError;

/// 联邦聚合器 trait.
pub trait FederatedAggregator {
    /// 收集客户端权重贡献.
    ///
    /// - `client`: 客户端 ID
    /// - `weights`: 模型参数
    /// - `n_samples`: 该客户端的训练样本数（用于加权）
    fn collect(&mut self, client: u64, weights: Vec<f32>, n_samples: usize);

    /// 执行 FedAvg 聚合，返回全局模型参数.
    fn aggregate(&mut self) -> Result<Vec<f32>, AiError>;
}

/// FedAvg 聚合器.
///
/// 按各客户端样本数加权平均：
/// `global[i] = sum(client[j][i] * n_samples[j] / total_samples)`
pub struct FedAvgAggregator {
    /// 期望的参数长度.
    param_len: usize,
    /// 收集的贡献: (client_id, weights, n_samples).
    contributions: Vec<(u64, Vec<f32>, usize)>,
}

impl FedAvgAggregator {
    /// 创建 FedAvg 聚合器.
    ///
    /// `param_len` 为期望的模型参数长度，不匹配的贡献将被拒绝。
    pub fn new(param_len: usize) -> Self {
        Self {
            param_len,
            contributions: Vec::new(),
        }
    }

    /// 当前收集的客户端数.
    pub fn client_count(&self) -> usize {
        self.contributions.len()
    }

    /// 总样本数.
    pub fn total_samples(&self) -> usize {
        self.contributions.iter().map(|(_, _, n)| n).sum()
    }
}

impl FederatedAggregator for FedAvgAggregator {
    fn collect(&mut self, client: u64, weights: Vec<f32>, n_samples: usize) {
        // Shape 校验：参数长度必须匹配
        if weights.len() != self.param_len {
            return; // 静默拒绝不匹配的客户端
        }
        if n_samples == 0 {
            return;
        }
        self.contributions.push((client, weights, n_samples));
    }

    fn aggregate(&mut self) -> Result<Vec<f32>, AiError> {
        let total: usize = self.contributions.iter().map(|(_, _, n)| n).sum();
        if total == 0 {
            return Err(AiError::ShapeMismatch);
        }

        let mut out = alloc::vec![0.0f32; self.param_len];

        for (_, weights, n_samples) in &self.contributions {
            let scale = (*n_samples as f32) / (total as f32);
            for (i, &v) in weights.iter().enumerate() {
                out[i] += v * scale;
            }
        }

        // 聚合后清空贡献
        self.contributions.clear();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fedavg_equal_weights() {
        let mut agg = FedAvgAggregator::new(3);
        agg.collect(1, alloc::vec![1.0, 2.0, 3.0], 10);
        agg.collect(2, alloc::vec![3.0, 4.0, 5.0], 10);

        let result = agg.aggregate().unwrap();
        // 等权重平均: (1+3)/2=2, (2+4)/2=3, (3+5)/2=4
        assert!((result[0] - 2.0).abs() < 1e-6);
        assert!((result[1] - 3.0).abs() < 1e-6);
        assert!((result[2] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn test_fedavg_weighted() {
        let mut agg = FedAvgAggregator::new(2);
        agg.collect(1, alloc::vec![1.0, 0.0], 30); // 权重 30/40 = 0.75
        agg.collect(2, alloc::vec![0.0, 1.0], 10); // 权重 10/40 = 0.25

        let result = agg.aggregate().unwrap();
        assert!((result[0] - 0.75).abs() < 1e-6);
        assert!((result[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_fedavg_shape_mismatch_rejected() {
        let mut agg = FedAvgAggregator::new(3);
        agg.collect(1, alloc::vec![1.0, 2.0, 3.0], 10);
        agg.collect(2, alloc::vec![1.0, 2.0], 10); // 长度不匹配，被拒绝

        assert_eq!(agg.client_count(), 1);
        let result = agg.aggregate().unwrap();
        assert!((result[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_fedavg_zero_samples_rejected() {
        let mut agg = FedAvgAggregator::new(2);
        agg.collect(1, alloc::vec![1.0, 2.0], 0); // 0 样本被拒绝
        assert_eq!(agg.client_count(), 0);
    }

    #[test]
    fn test_fedavg_empty_fails() {
        let mut agg = FedAvgAggregator::new(3);
        assert_eq!(agg.aggregate().unwrap_err(), AiError::ShapeMismatch);
    }

    #[test]
    fn test_fedavg_clears_after_aggregate() {
        let mut agg = FedAvgAggregator::new(2);
        agg.collect(1, alloc::vec![1.0, 2.0], 10);
        agg.aggregate().unwrap();
        assert_eq!(agg.client_count(), 0);
        assert_eq!(agg.total_samples(), 0);
    }

    #[test]
    fn test_total_samples() {
        let mut agg = FedAvgAggregator::new(2);
        agg.collect(1, alloc::vec![1.0, 2.0], 100);
        agg.collect(2, alloc::vec![3.0, 4.0], 200);
        assert_eq!(agg.total_samples(), 300);
    }
}
