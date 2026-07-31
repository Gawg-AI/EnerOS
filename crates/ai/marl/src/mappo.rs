//! MAPPO 训练器 (v0.188.0).
//!
//! Multi-Agent PPO：集中训练分散执行（CTDE）。
//! Mock 实现：简化策略梯度更新。

use alloc::vec::Vec;

use crate::coord::{CoordPolicy, EqualCredit, GlobalCritic};
use crate::types::{JointTransition, MapConfig, MultiAgentPolicy, TrainMetrics};
use crate::MarlError;

/// MARL 训练器 trait.
pub trait MarlTrainer {
    /// 联合推理：各 Agent 独立推理（分散执行，no_grad）.
    fn joint_act(&mut self, states: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, MarlError>;
    /// 观察联合转移.
    fn observe(&mut self, jt: JointTransition);
    /// 更新所有策略（集中训练）.
    fn update(&mut self) -> Result<TrainMetrics, MarlError>;
}

/// MAPPO 训练器实现.
pub struct MappoTrainer {
    /// 多 Agent 策略.
    pub multi: MultiAgentPolicy,
    /// 配置.
    pub cfg: MapConfig,
    /// 联合经验缓冲.
    buf: Vec<JointTransition>,
    /// 信用分配策略.
    credit: EqualCredit,
    /// 全局 Critic.
    critic: Option<alloc::boxed::Box<dyn GlobalCritic>>,
    /// 更新计数.
    update_count: u32,
}

impl MappoTrainer {
    /// 创建 MAPPO 训练器.
    pub fn new(multi: MultiAgentPolicy, cfg: MapConfig) -> Self {
        let critic = if cfg.centralized_critic {
            Some(
                alloc::boxed::Box::new(crate::coord::LinearCritic::new(multi.global_state_dim))
                    as alloc::boxed::Box<dyn GlobalCritic>,
            )
        } else {
            None
        };
        Self {
            multi,
            cfg,
            buf: Vec::new(),
            credit: EqualCredit,
            critic,
            update_count: 0,
        }
    }

    /// 获取缓冲大小.
    pub fn buffer_size(&self) -> usize {
        self.buf.len()
    }

    /// 单 Agent 推理（mock 线性变换 + clamp）.
    fn act_single(&self, agent_idx: usize, state: &[f32]) -> Result<Vec<f32>, MarlError> {
        let policy = self
            .multi
            .agents
            .get(agent_idx)
            .ok_or(MarlError::AgentNotFound)?;

        if state.len() != policy.state_dim {
            return Err(MarlError::StateDimMismatch);
        }

        // Mock: logits = state × W, action = clamp(logits, -1, 1)
        let mut actions = alloc::vec![0.0f32; policy.action_dim];
        for (i, &s) in state.iter().enumerate() {
            for (j, a) in actions.iter_mut().enumerate() {
                let w_idx = i * policy.action_dim + j;
                if w_idx < policy.weights.len() {
                    *a += s * policy.weights[w_idx];
                }
            }
        }
        for a in actions.iter_mut() {
            *a = a.clamp(-1.0, 1.0);
        }
        Ok(actions)
    }
}

impl MarlTrainer for MappoTrainer {
    fn joint_act(&mut self, states: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, MarlError> {
        let mut actions = Vec::with_capacity(states.len());
        for (i, state) in states.iter().enumerate() {
            if i >= self.multi.n_agents() {
                break;
            }
            let action = self.act_single(i, state)?;
            actions.push(action);
        }
        Ok(actions)
    }

    fn observe(&mut self, jt: JointTransition) {
        if self.buf.len() >= 1024 {
            self.buf.remove(0);
        }
        self.buf.push(jt);
    }

    fn update(&mut self) -> Result<TrainMetrics, MarlError> {
        if self.buf.len() < self.cfg.rollout_steps {
            return Ok(TrainMetrics {
                loss: 0.0,
                mean_global_reward: 0.0,
                steps: 0,
            });
        }

        // 计算平均全局奖励
        let mean_global_reward: f32 =
            self.buf.iter().map(|jt| jt.global_reward).sum::<f32>() / self.buf.len() as f32;

        // 信用分配
        let credits: Vec<Vec<f32>> = self.buf.iter().map(|jt| self.credit.credit(jt)).collect();

        // 集中 Critic 估值（如启用）
        if let Some(ref critic) = self.critic {
            let global_state: Vec<f32> = self.buf[0]
                .per_agent
                .iter()
                .flat_map(|t| t.state.iter().copied())
                .collect();
            let _value = critic.value(&global_state);
        }

        // Mock MAPPO 更新：各 Agent 策略沿信用分配方向微调
        let lr = self.cfg.lr;
        for (agent_idx, agent) in self.multi.agents.iter_mut().enumerate() {
            let avg_credit: f32 =
                credits.iter().filter_map(|c| c.get(agent_idx)).sum::<f32>() / credits.len() as f32;

            let gradient_scale = avg_credit * 0.001;
            for w in agent.weights.iter_mut() {
                *w += lr * gradient_scale;
            }
        }

        // 发散检测
        for agent in &self.multi.agents {
            if agent.weights.iter().any(|w| w.is_nan() || w.is_infinite()) {
                return Err(MarlError::Diverged);
            }
        }

        self.update_count += 1;
        let loss = (lr * mean_global_reward * 0.001).abs();

        // 清空缓冲
        self.buf.clear();

        Ok(TrainMetrics {
            loss,
            mean_global_reward,
            steps: self.update_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Transition;

    fn make_trainer(n_agents: usize) -> MappoTrainer {
        let multi = MultiAgentPolicy::new(n_agents, 4, 2);
        let cfg = MapConfig {
            n_agents,
            rollout_steps: 2,
            ..MapConfig::default()
        };
        MappoTrainer::new(multi, cfg)
    }

    fn make_jt(n_agents: usize, reward: f32) -> JointTransition {
        JointTransition {
            per_agent: (0..n_agents)
                .map(|_| Transition {
                    state: alloc::vec![1.0, 0.5, -0.3, 0.8],
                    action: alloc::vec![0.1, -0.2],
                    reward: reward / n_agents as f32,
                    next_state: alloc::vec![1.1, 0.4, -0.2, 0.9],
                    done: false,
                })
                .collect(),
            global_reward: reward,
        }
    }

    #[test]
    fn test_joint_act() {
        let mut trainer = make_trainer(3);
        let states = alloc::vec![
            alloc::vec![1.0, 0.5, -0.3, 0.8],
            alloc::vec![0.2, 0.3, 0.4, 0.5],
            alloc::vec![-0.1, 0.9, 0.1, 0.2],
        ];
        let actions = trainer.joint_act(&states).unwrap();
        assert_eq!(actions.len(), 3);
        for a in &actions {
            assert_eq!(a.len(), 2);
            for &v in a {
                assert!((-1.0..=1.0).contains(&v));
            }
        }
    }

    #[test]
    fn test_joint_act_dim_mismatch() {
        let mut trainer = make_trainer(2);
        let states = alloc::vec![alloc::vec![1.0, 2.0]]; // 维度不匹配 (应为 4)
        assert_eq!(
            trainer.joint_act(&states).unwrap_err(),
            MarlError::StateDimMismatch
        );
    }

    #[test]
    fn test_observe_and_update() {
        let mut trainer = make_trainer(2);
        trainer.observe(make_jt(2, 5.0));
        trainer.observe(make_jt(2, 3.0));

        let m = trainer.update().unwrap();
        assert_eq!(m.steps, 1);
        assert!((m.mean_global_reward - 4.0).abs() < 1e-5);
        assert_eq!(trainer.buffer_size(), 0);
    }

    #[test]
    fn test_update_insufficient_data() {
        let mut trainer = make_trainer(2);
        trainer.observe(make_jt(2, 5.0));
        let m = trainer.update().unwrap();
        assert_eq!(m.steps, 0);
    }

    #[test]
    fn test_weights_change_after_update() {
        let mut trainer = make_trainer(2);
        let before = trainer.multi.agents[0].weights.clone();

        trainer.observe(make_jt(2, 10.0));
        trainer.observe(make_jt(2, 10.0));
        trainer.update().unwrap();

        assert_ne!(trainer.multi.agents[0].weights, before);
    }

    #[test]
    fn test_agent_offline_skip() {
        let mut trainer = make_trainer(3);
        // 只提供 2 个状态（第 3 个 Agent 离线）
        let states = alloc::vec![
            alloc::vec![1.0, 0.5, -0.3, 0.8],
            alloc::vec![0.2, 0.3, 0.4, 0.5],
        ];
        let actions = trainer.joint_act(&states).unwrap();
        assert_eq!(actions.len(), 2); // 只返回 2 个动作
    }
}
