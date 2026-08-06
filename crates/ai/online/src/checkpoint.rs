//! 检查点与回滚 (workspace v0.192.0).
//!
//! 保存模型快照，检测训练发散，支持自动回滚到最优检查点。

use alloc::vec::Vec;

use crate::types::OnlineModel;
use crate::AiError;

/// 检查点.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    /// 检查点 ID.
    pub id: u64,
    /// 模型快照.
    pub model: OnlineModel,
    /// 对应的损失值.
    pub loss: f32,
    /// 训练步数.
    pub step: u32,
}

/// 检查点管理器.
///
/// 维护模型快照历史，检测发散并支持回滚。
pub struct CheckpointManager {
    /// 检查点列表.
    checkpoints: Vec<Checkpoint>,
    /// 下一个检查点 ID.
    next_id: u64,
    /// 最优检查点索引.
    best_idx: Option<usize>,
    /// 连续上升计数（发散检测）.
    rising_count: u32,
    /// 上一次损失.
    last_loss: Option<f32>,
    /// 连续上升阈值.
    divergence_threshold: u32,
}

impl CheckpointManager {
    /// 创建检查点管理器.
    ///
    /// `divergence_threshold`: 连续损失上升多少次判定为发散。
    pub fn new(divergence_threshold: u32) -> Self {
        Self {
            checkpoints: Vec::new(),
            next_id: 1,
            best_idx: None,
            rising_count: 0,
            last_loss: None,
            divergence_threshold,
        }
    }

    /// 保存检查点.
    pub fn save(&mut self, model: &OnlineModel, loss: f32, step: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        // 更新最优
        let is_best = match self.best_idx {
            Some(idx) => loss < self.checkpoints[idx].loss,
            None => true,
        };
        if is_best {
            self.best_idx = Some(self.checkpoints.len());
        }

        // 发散检测
        if let Some(last) = self.last_loss {
            if loss > last {
                self.rising_count += 1;
            } else {
                self.rising_count = 0;
            }
        }
        self.last_loss = Some(loss);

        self.checkpoints.push(Checkpoint {
            id,
            model: model.clone(),
            loss,
            step,
        });

        id
    }

    /// 检测是否发散.
    pub fn is_diverged(&self) -> bool {
        self.rising_count >= self.divergence_threshold
    }

    /// 获取最优检查点.
    pub fn best(&self) -> Option<&Checkpoint> {
        self.best_idx.map(|idx| &self.checkpoints[idx])
    }

    /// 回滚到最优检查点.
    pub fn rollback_to_best(&self) -> Result<OnlineModel, AiError> {
        self.best()
            .map(|cp| cp.model.clone())
            .ok_or(AiError::CheckpointCorrupt)
    }

    /// 获取最新检查点.
    pub fn latest(&self) -> Option<&Checkpoint> {
        self.checkpoints.last()
    }

    /// 检查点数量.
    pub fn count(&self) -> usize {
        self.checkpoints.len()
    }

    /// 检查是否需要回滚（发散时自动触发）.
    pub fn should_rollback(&self) -> bool {
        self.is_diverged()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_weights(w: Vec<f32>) -> OnlineModel {
        OnlineModel::new(1, w, alloc::vec![4])
    }

    #[test]
    fn test_save_checkpoint() {
        let mut mgr = CheckpointManager::new(3);
        let model = model_with_weights(alloc::vec![1.0, 2.0, 3.0, 4.0]);
        let id = mgr.save(&model, 0.5, 1);
        assert_eq!(id, 1);
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn test_best_checkpoint() {
        let mut mgr = CheckpointManager::new(3);
        let m1 = model_with_weights(alloc::vec![1.0, 0.0, 0.0, 0.0]);
        let m2 = model_with_weights(alloc::vec![0.0, 1.0, 0.0, 0.0]);
        let m3 = model_with_weights(alloc::vec![0.0, 0.0, 1.0, 0.0]);

        mgr.save(&m1, 0.5, 1);
        mgr.save(&m2, 0.3, 2); // best
        mgr.save(&m3, 0.4, 3);

        let best = mgr.best().unwrap();
        assert!((best.loss - 0.3).abs() < 1e-6);
        assert_eq!(best.step, 2);
    }

    #[test]
    fn test_divergence_detection() {
        let mut mgr = CheckpointManager::new(3);
        let model = model_with_weights(alloc::vec![1.0, 2.0, 3.0, 4.0]);

        mgr.save(&model, 0.5, 1);
        assert!(!mgr.is_diverged());

        mgr.save(&model, 0.6, 2); // rising 1
        mgr.save(&model, 0.7, 3); // rising 2
        assert!(!mgr.is_diverged());

        mgr.save(&model, 0.8, 4); // rising 3 → diverged!
        assert!(mgr.is_diverged());
        assert!(mgr.should_rollback());
    }

    #[test]
    fn test_rollback_resets_divergence() {
        let mut mgr = CheckpointManager::new(2);
        let m1 = model_with_weights(alloc::vec![1.0, 2.0, 3.0, 4.0]);

        mgr.save(&m1, 0.5, 1);
        mgr.save(&m1, 0.6, 2);
        mgr.save(&m1, 0.7, 3); // diverged

        let restored = mgr.rollback_to_best().unwrap();
        assert_eq!(restored.weights, m1.weights);
    }

    #[test]
    fn test_rollback_empty_fails() {
        let mgr = CheckpointManager::new(3);
        assert_eq!(
            mgr.rollback_to_best().unwrap_err(),
            AiError::CheckpointCorrupt
        );
    }

    #[test]
    fn test_loss_decrease_resets_rising() {
        let mut mgr = CheckpointManager::new(3);
        let model = model_with_weights(alloc::vec![1.0, 2.0, 3.0, 4.0]);

        mgr.save(&model, 0.5, 1);
        mgr.save(&model, 0.6, 2); // rising 1
        mgr.save(&model, 0.4, 3); // decrease → reset
        mgr.save(&model, 0.5, 4); // rising 1
        mgr.save(&model, 0.6, 5); // rising 2

        assert!(!mgr.is_diverged()); // only 2 consecutive, threshold is 3
    }

    #[test]
    fn test_latest() {
        let mut mgr = CheckpointManager::new(3);
        let model = model_with_weights(alloc::vec![1.0, 2.0, 3.0, 4.0]);
        mgr.save(&model, 0.5, 1);
        mgr.save(&model, 0.3, 2);

        let latest = mgr.latest().unwrap();
        assert_eq!(latest.step, 2);
    }
}
