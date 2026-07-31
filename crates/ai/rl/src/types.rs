//! 强化学习核心类型 (v0.187.0).

use alloc::vec::Vec;

/// 计算设备.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeDevice {
    /// NVIDIA CUDA GPU.
    Cuda,
    /// CPU 回退.
    Cpu,
}

/// Agent 策略网络.
#[derive(Debug, Clone)]
pub struct AgentPolicy {
    /// 状态空间维度.
    pub state_dim: usize,
    /// 动作空间维度.
    pub action_dim: usize,
    /// 网络权重（扁平化）.
    pub weights: Vec<f32>,
    /// 计算设备.
    pub device: ComputeDevice,
}

impl AgentPolicy {
    /// 创建策略网络.
    pub fn new(state_dim: usize, action_dim: usize) -> Self {
        // 简单初始化：state_dim * action_dim 个权重
        let n = state_dim * action_dim;
        Self {
            state_dim,
            action_dim,
            weights: alloc::vec![0.01f32; n],
            device: ComputeDevice::Cpu,
        }
    }
}

/// 状态转移（经验）.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// 当前状态.
    pub state: Vec<f32>,
    /// 执行的动作.
    pub action: Vec<f32>,
    /// 获得的奖励.
    pub reward: f32,
    /// 下一状态.
    pub next_state: Vec<f32>,
    /// 是否终止.
    pub done: bool,
}

/// 训练指标.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainMetrics {
    /// 策略损失.
    pub loss: f32,
    /// 平均奖励.
    pub mean_reward: f32,
    /// 更新步数.
    pub steps: u32,
}

/// PPO 配置.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PpoConfig {
    /// PPO clip 范围.
    pub clip: f32,
    /// 折扣因子.
    pub gamma: f32,
    /// GAE lambda.
    pub lam: f32,
    /// 学习率.
    pub lr: f32,
    /// Rollout 步数.
    pub rollout_steps: u32,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            clip: 0.2,
            gamma: 0.99,
            lam: 0.95,
            lr: 3e-4,
            rollout_steps: 128,
        }
    }
}

/// Reward 上下文.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardCtx {
    /// 收益（元）.
    pub revenue_yuan: f64,
    /// 成本（元）.
    pub cost_yuan: f64,
    /// SOC 偏差.
    pub soc_deviation: f32,
    /// 安全违规次数.
    pub safety_violation: u32,
    /// 市场响应延迟（秒）.
    pub market_response_lag_s: f32,
}

/// Replay Buffer.
#[derive(Debug, Clone)]
pub struct ReplayBuffer {
    /// 容量.
    pub capacity: usize,
    /// 存储的转移.
    pub buf: Vec<Transition>,
}

impl ReplayBuffer {
    /// 创建 Replay Buffer.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buf: Vec::new(),
        }
    }

    /// 添加转移.
    pub fn push(&mut self, t: Transition) {
        if self.buf.len() >= self.capacity {
            self.buf.remove(0);
        }
        self.buf.push(t);
    }

    /// 当前大小.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 清空.
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_policy_creation() {
        let policy = AgentPolicy::new(16, 4);
        assert_eq!(policy.state_dim, 16);
        assert_eq!(policy.action_dim, 4);
        assert_eq!(policy.weights.len(), 64);
        assert_eq!(policy.device, ComputeDevice::Cpu);
    }

    #[test]
    fn test_replay_buffer() {
        let mut buf = ReplayBuffer::new(3);
        assert!(buf.is_empty());

        let t = Transition {
            state: alloc::vec![1.0],
            action: alloc::vec![0.5],
            reward: 1.0,
            next_state: alloc::vec![2.0],
            done: false,
        };

        buf.push(t.clone());
        buf.push(t.clone());
        buf.push(t.clone());
        assert_eq!(buf.len(), 3);

        // 超出容量，移除最旧的
        buf.push(t);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn test_ppo_config_default() {
        let cfg = PpoConfig::default();
        assert!((cfg.clip - 0.2).abs() < 1e-6);
        assert!((cfg.gamma - 0.99).abs() < 1e-6);
        assert_eq!(cfg.rollout_steps, 128);
    }

    #[test]
    fn test_replay_buffer_clear() {
        let mut buf = ReplayBuffer::new(10);
        let t = Transition {
            state: alloc::vec![0.0],
            action: alloc::vec![0.0],
            reward: 0.0,
            next_state: alloc::vec![0.0],
            done: true,
        };
        buf.push(t);
        assert!(!buf.is_empty());
        buf.clear();
        assert!(buf.is_empty());
    }
}
