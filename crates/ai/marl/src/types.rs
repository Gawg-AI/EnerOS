//! MARL 核心类型 (v0.188.0).

use alloc::vec::Vec;

/// 计算设备.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeDevice {
    /// CUDA GPU.
    Cuda,
    /// CPU.
    Cpu,
}

/// 单 Agent 策略.
#[derive(Debug, Clone)]
pub struct AgentPolicy {
    /// 状态维度.
    pub state_dim: usize,
    /// 动作维度.
    pub action_dim: usize,
    /// 网络权重.
    pub weights: Vec<f32>,
}

impl AgentPolicy {
    /// 创建策略.
    pub fn new(state_dim: usize, action_dim: usize) -> Self {
        let n = state_dim * action_dim;
        Self {
            state_dim,
            action_dim,
            weights: alloc::vec![0.01f32; n],
        }
    }
}

/// 多 Agent 策略集合.
#[derive(Debug, Clone)]
pub struct MultiAgentPolicy {
    /// 各 Agent 策略.
    pub agents: Vec<AgentPolicy>,
    /// 全局状态维度.
    pub global_state_dim: usize,
    /// 计算设备.
    pub device: ComputeDevice,
}

impl MultiAgentPolicy {
    /// 创建多 Agent 策略.
    pub fn new(n_agents: usize, state_dim: usize, action_dim: usize) -> Self {
        let agents = (0..n_agents)
            .map(|_| AgentPolicy::new(state_dim, action_dim))
            .collect();
        Self {
            agents,
            global_state_dim: state_dim * n_agents,
            device: ComputeDevice::Cpu,
        }
    }

    /// Agent 数量.
    pub fn n_agents(&self) -> usize {
        self.agents.len()
    }
}

/// 单 Agent 转移.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// 局部状态.
    pub state: Vec<f32>,
    /// 动作.
    pub action: Vec<f32>,
    /// 局部奖励.
    pub reward: f32,
    /// 下一状态.
    pub next_state: Vec<f32>,
    /// 是否终止.
    pub done: bool,
}

/// 联合转移（多 Agent 一步）.
#[derive(Debug, Clone, PartialEq)]
pub struct JointTransition {
    /// 各 Agent 的转移.
    pub per_agent: Vec<Transition>,
    /// 全局奖励.
    pub global_reward: f32,
}

/// 训练指标.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainMetrics {
    /// 损失.
    pub loss: f32,
    /// 平均全局奖励.
    pub mean_global_reward: f32,
    /// 更新步数.
    pub steps: u32,
}

/// MAPPO 配置.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapConfig {
    /// Agent 数量.
    pub n_agents: usize,
    /// PPO clip.
    pub clip: f32,
    /// 折扣因子.
    pub gamma: f32,
    /// 学习率.
    pub lr: f32,
    /// 是否使用集中 Critic.
    pub centralized_critic: bool,
    /// Rollout 步数.
    pub rollout_steps: usize,
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            n_agents: 3,
            clip: 0.2,
            gamma: 0.99,
            lr: 3e-4,
            centralized_critic: true,
            rollout_steps: 64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_agent_policy() {
        let multi = MultiAgentPolicy::new(3, 8, 2);
        assert_eq!(multi.n_agents(), 3);
        assert_eq!(multi.global_state_dim, 24);
        assert_eq!(multi.agents[0].weights.len(), 16);
    }

    #[test]
    fn test_joint_transition() {
        let jt = JointTransition {
            per_agent: alloc::vec![
                Transition {
                    state: alloc::vec![1.0],
                    action: alloc::vec![0.5],
                    reward: 1.0,
                    next_state: alloc::vec![2.0],
                    done: false,
                },
                Transition {
                    state: alloc::vec![3.0],
                    action: alloc::vec![0.2],
                    reward: 0.5,
                    next_state: alloc::vec![4.0],
                    done: false,
                },
            ],
            global_reward: 1.5,
        };
        assert_eq!(jt.per_agent.len(), 2);
        assert!((jt.global_reward - 1.5).abs() < 1e-6);
    }

    #[test]
    fn test_map_config_default() {
        let cfg = MapConfig::default();
        assert_eq!(cfg.n_agents, 3);
        assert!(cfg.centralized_critic);
        assert_eq!(cfg.rollout_steps, 64);
    }
}
