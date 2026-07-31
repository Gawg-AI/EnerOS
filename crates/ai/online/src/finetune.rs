//! 边缘增量微调 (v0.186.0).
//!
//! 实现 IncrementalTrainer trait，使用简化的 mock 梯度下降。
//! 未来集成 PyTorch FFI 时替换 mock 计算为真实训练步。

use crate::types::{Batch, FinetuneState, OnlineModel, TrainMetrics};
use crate::AiError;

/// 增量训练器 trait.
pub trait IncrementalTrainer {
    /// 执行一步训练.
    fn step(&mut self, batch: &Batch, lr: f32) -> Result<TrainMetrics, AiError>;
    /// 获取当前模型快照.
    fn snapshot(&self) -> OnlineModel;
    /// 恢复模型到指定快照.
    fn restore(&mut self, model: OnlineModel);
}

/// 增量微调实现（mock 梯度下降）.
///
/// 使用简化的权重更新规则：
/// - 梯度 = features 均值（模拟）
/// - weights[i] -= lr * gradient[i % features.len()]
/// - loss = 权重变化量的 MSE
pub struct IncrementalTrainerImpl {
    /// 微调状态.
    pub state: FinetuneState,
}

impl IncrementalTrainerImpl {
    /// 创建增量训练器.
    pub fn new(model: OnlineModel, lr: f32, batch_size: usize) -> Self {
        Self {
            state: FinetuneState {
                model,
                lr,
                batch_size,
                steps: 0,
                best_loss: f32::MAX,
            },
        }
    }

    /// 获取当前步数.
    pub fn steps(&self) -> u32 {
        self.state.steps
    }

    /// 获取历史最优损失.
    pub fn best_loss(&self) -> f32 {
        self.state.best_loss
    }
}

impl IncrementalTrainer for IncrementalTrainerImpl {
    fn step(&mut self, batch: &Batch, lr: f32) -> Result<TrainMetrics, AiError> {
        if batch.features.is_empty() {
            return Err(AiError::ShapeMismatch);
        }
        if batch.size == 0 {
            return Err(AiError::ShapeMismatch);
        }

        let weights = &mut self.state.model.weights;
        let feat_len = batch.features.len();

        // Mock 梯度下降：计算模拟梯度并更新权重
        let mut total_delta_sq = 0.0f32;
        let mut total_abs_delta = 0.0f32;

        for (i, w) in weights.iter_mut().enumerate() {
            // 模拟梯度：使用特征数据的对应位置值
            let gradient = batch.features[i % feat_len] / batch.size as f32;
            let delta = lr * gradient;
            *w -= delta;
            total_delta_sq += delta * delta;
            total_abs_delta += delta.abs();
        }

        let n = weights.len() as f32;
        let loss = total_delta_sq / n;
        let mae = total_abs_delta / n;

        // 发散检测
        if loss.is_nan() || loss.is_infinite() {
            return Err(AiError::Diverged);
        }

        self.state.steps += 1;
        if loss < self.state.best_loss {
            self.state.best_loss = loss;
        }

        Ok(TrainMetrics {
            loss,
            mae,
            steps: self.state.steps,
        })
    }

    fn snapshot(&self) -> OnlineModel {
        self.state.model.clone()
    }

    fn restore(&mut self, model: OnlineModel) {
        self.state.model = model;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model() -> OnlineModel {
        OnlineModel::new(1, alloc::vec![1.0, 2.0, 3.0, 4.0], alloc::vec![4])
    }

    fn test_batch() -> Batch {
        Batch::new(alloc::vec![0.1, 0.2, 0.3, 0.4], alloc::vec![1.0], 1)
    }

    #[test]
    fn test_step_updates_weights() {
        let mut trainer = IncrementalTrainerImpl::new(test_model(), 0.01, 1);
        let before = trainer.snapshot();
        let metrics = trainer.step(&test_batch(), 0.01).unwrap();
        let after = trainer.snapshot();

        assert_ne!(before.weights, after.weights);
        assert!(metrics.loss >= 0.0);
        assert_eq!(metrics.steps, 1);
    }

    #[test]
    fn test_step_empty_batch_fails() {
        let mut trainer = IncrementalTrainerImpl::new(test_model(), 0.01, 1);
        let empty = Batch::new(alloc::vec![], alloc::vec![], 0);
        assert_eq!(
            trainer.step(&empty, 0.01).unwrap_err(),
            AiError::ShapeMismatch
        );
    }

    #[test]
    fn test_snapshot_restore() {
        let mut trainer = IncrementalTrainerImpl::new(test_model(), 0.01, 1);
        let snap = trainer.snapshot();

        trainer.step(&test_batch(), 0.1).unwrap();
        assert_ne!(trainer.snapshot().weights, snap.weights);

        trainer.restore(snap.clone());
        assert_eq!(trainer.snapshot().weights, snap.weights);
    }

    #[test]
    fn test_best_loss_tracking() {
        let mut trainer = IncrementalTrainerImpl::new(test_model(), 0.01, 1);
        assert_eq!(trainer.best_loss(), f32::MAX);

        let m1 = trainer.step(&test_batch(), 0.01).unwrap();
        assert_eq!(trainer.best_loss(), m1.loss);

        // 更小学习率 → 更小 loss
        let m2 = trainer.step(&test_batch(), 0.001).unwrap();
        if m2.loss < m1.loss {
            assert_eq!(trainer.best_loss(), m2.loss);
        }
    }

    #[test]
    fn test_multiple_steps() {
        let mut trainer = IncrementalTrainerImpl::new(test_model(), 0.01, 1);
        for _ in 0..10 {
            trainer.step(&test_batch(), 0.01).unwrap();
        }
        assert_eq!(trainer.steps(), 10);
    }

    #[test]
    fn test_model_version_preserved() {
        let mut model = test_model();
        model.version = 5;
        let trainer = IncrementalTrainerImpl::new(model, 0.01, 1);
        assert_eq!(trainer.snapshot().version, 5);
    }
}
