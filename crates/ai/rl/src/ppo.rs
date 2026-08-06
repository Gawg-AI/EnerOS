//! PPO 训练器 (workspace v0.192.0).
//!
//! 实现 PolicyTrainer trait，编排 act/observe/update 闭环。
//! Mock 实现：简化 PPO 更新（权重沿 reward 梯度方向微调）。

use alloc::vec::Vec;

use crate::policy::policy_act;
use crate::types::{AgentPolicy, PpoConfig, ReplayBuffer, TrainMetrics, Transition};
use crate::RlError;

/// 策略训练器 trait.
pub trait PolicyTrainer {
    /// 推理动作（no_grad）.
    fn act(&mut self, state: &[f32]) -> Result<Vec<f32>, RlError>;
    /// 观察转移（存入 Replay Buffer）.
    fn observe(&mut self, t: Transition);
    /// 更新策略网络.
    fn update(&mut self) -> Result<TrainMetrics, RlError>;
}

/// PPO 训练器.
pub struct PpoTrainer {
    /// 策略网络.
    pub policy: AgentPolicy,
    /// 经验回放缓冲.
    pub buf: ReplayBuffer,
    /// PPO 配置.
    pub cfg: PpoConfig,
    /// 累计更新次数.
    update_count: u32,
}

impl PpoTrainer {
    /// 创建 PPO 训练器.
    pub fn new(policy: AgentPolicy, cfg: PpoConfig) -> Self {
        Self {
            policy,
            buf: ReplayBuffer::new(4096),
            cfg,
            update_count: 0,
        }
    }

    /// 获取策略网络引用.
    pub fn policy(&self) -> &AgentPolicy {
        &self.policy
    }

    /// 获取 buffer 大小.
    pub fn buffer_size(&self) -> usize {
        self.buf.len()
    }
}

impl PolicyTrainer for PpoTrainer {
    fn act(&mut self, state: &[f32]) -> Result<Vec<f32>, RlError> {
        // 推理（no_grad 语义：不修改权重）
        policy_act(&self.policy, state)
    }

    fn observe(&mut self, t: Transition) {
        self.buf.push(t);
    }

    fn update(&mut self) -> Result<TrainMetrics, RlError> {
        let rollout = self.cfg.rollout_steps as usize;
        if self.buf.len() < rollout {
            return Ok(TrainMetrics {
                loss: 0.0,
                mean_reward: 0.0,
                steps: 0,
            });
        }

        // 计算平均奖励
        let mean_reward: f32 =
            self.buf.buf.iter().map(|t| t.reward).sum::<f32>() / self.buf.len() as f32;

        // Mock PPO 更新：沿平均奖励方向微调权重
        let lr = self.cfg.lr;
        let gradient_scale = mean_reward * 0.001; // 简化梯度

        for w in self.policy.weights.iter_mut() {
            *w += lr * gradient_scale;
        }

        // 发散检测
        if self
            .policy
            .weights
            .iter()
            .any(|w| w.is_nan() || w.is_infinite())
        {
            return Err(RlError::Diverged);
        }

        self.update_count += 1;

        // 计算 loss（mock：权重变化量的均值）
        let loss = (lr * gradient_scale).abs();

        // 清空已使用的经验
        self.buf.clear();

        Ok(TrainMetrics {
            loss,
            mean_reward,
            steps: self.update_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> AgentPolicy {
        AgentPolicy::new(4, 2)
    }

    fn test_transition(reward: f32) -> Transition {
        Transition {
            state: alloc::vec![1.0, 0.5, -0.3, 0.8],
            action: alloc::vec![0.1, -0.2],
            reward,
            next_state: alloc::vec![1.1, 0.4, -0.2, 0.9],
            done: false,
        }
    }

    #[test]
    fn test_act() {
        let mut trainer = PpoTrainer::new(test_policy(), PpoConfig::default());
        let state = alloc::vec![1.0, 0.5, -0.3, 0.8];
        let action = trainer.act(&state).unwrap();
        assert_eq!(action.len(), 2);
    }

    #[test]
    fn test_observe() {
        let mut trainer = PpoTrainer::new(test_policy(), PpoConfig::default());
        trainer.observe(test_transition(1.0));
        trainer.observe(test_transition(2.0));
        assert_eq!(trainer.buffer_size(), 2);
    }

    #[test]
    fn test_update_insufficient_data() {
        let mut trainer = PpoTrainer::new(test_policy(), PpoConfig::default());
        trainer.observe(test_transition(1.0));
        // rollout_steps=128, only 1 sample
        let m = trainer.update().unwrap();
        assert_eq!(m.steps, 0);
    }

    #[test]
    fn test_update_with_enough_data() {
        let cfg = PpoConfig {
            rollout_steps: 4,
            ..PpoConfig::default()
        };
        let mut trainer = PpoTrainer::new(test_policy(), cfg);

        for _ in 0..4 {
            trainer.observe(test_transition(10.0));
        }

        let m = trainer.update().unwrap();
        assert_eq!(m.steps, 1);
        assert!((m.mean_reward - 10.0).abs() < 1e-4);
        assert!(m.loss > 0.0);
        // Buffer cleared after update
        assert_eq!(trainer.buffer_size(), 0);
    }

    #[test]
    fn test_policy_weights_change_after_update() {
        let cfg = PpoConfig {
            rollout_steps: 2,
            lr: 0.01,
            ..PpoConfig::default()
        };
        let mut trainer = PpoTrainer::new(test_policy(), cfg);
        let before = trainer.policy().weights.clone();

        trainer.observe(test_transition(5.0));
        trainer.observe(test_transition(5.0));
        trainer.update().unwrap();

        assert_ne!(trainer.policy().weights, before);
    }

    #[test]
    fn test_full_loop() {
        let cfg = PpoConfig {
            rollout_steps: 2,
            ..PpoConfig::default()
        };
        let mut trainer = PpoTrainer::new(test_policy(), cfg);

        // act → observe → update 循环
        for _ in 0..3 {
            let state = alloc::vec![1.0, 0.5, -0.3, 0.8];
            let action = trainer.act(&state).unwrap();
            assert_eq!(action.len(), 2);

            trainer.observe(test_transition(1.0));
            trainer.observe(test_transition(2.0));
            let m = trainer.update().unwrap();
            assert!(m.steps > 0);
        }
    }
}
