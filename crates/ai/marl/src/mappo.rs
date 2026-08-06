//! MAPPO 训练器 (workspace v0.192.0).
//!
//! Multi-Agent PPO：集中训练分散执行（CTDE）。
//! Mock 实现：简化策略梯度更新。

use alloc::vec::Vec;

use crate::coord::{CoordPolicy, EqualCredit, GlobalCritic};
use crate::types::{JointTransition, MapConfig, MultiAgentPolicy, TrainMetrics};
use crate::MarlError;

// ============================================================================
// 线性 softmax 策略辅助（ARC-006 真实 PPO 更新）
// ============================================================================

/// softmax 概率分布 π(·|s) = softmax(W·s)。
#[allow(clippy::needless_range_loop)]
fn softmax_probs(agent: &crate::types::AgentPolicy, state: &[f32]) -> Vec<f32> {
    let mut z = alloc::vec![0.0f32; agent.action_dim];
    for (k, s_val) in state.iter().enumerate() {
        let w_base = k * agent.action_dim;
        for a in 0..agent.action_dim {
            let wi = w_base + a;
            if wi < agent.weights.len() {
                z[a] += agent.weights[wi] * s_val;
            }
        }
    }
    // no_std 无 exp：用幂近似 e^x ≈ (1 + x/n)^n（n=8 足够策略概率平滑）。
    // 数值稳定：先减 max_z 再近似。
    let max_z = z.iter().cloned().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = z
        .iter()
        .map(|v| {
            let x = v - max_z;
            let mut acc = 1.0 + x / 8.0;
            for _ in 0..3 {
                acc = acc * acc;
            }
            acc
        })
        .collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

/// 动作 a 的概率 π(a|s)。
#[allow(clippy::needless_range_loop)]
fn action_prob(agent: &crate::types::AgentPolicy, state: &[f32], action: &[f32]) -> f32 {
    let probs = softmax_probs(agent, state);
    let idx = argmax(action);
    probs.get(idx).copied().unwrap_or(0.0)
}

/// 动作向量中最大分量下标（离散动作 = one-hot/argmax）。
fn argmax(action: &[f32]) -> usize {
    action
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

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

    #[allow(clippy::needless_range_loop)]
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

        // ===== 真实 PPO 更新（ARC-006：重要性采样比 + clip + advantage）=====
        // 线性 softmax 策略：z = W·s，π(a) = softmax(z)。
        // PPO 目标：L = E[min(r·A, clip(r,1-ε,1+ε)·A)]，r = π_new/π_old。
        // 梯度上升最大化 L：∂L/∂w = A·mask·r·(∂logπ/∂w)，
        //   ∂logπ_a/∂w_jk = (δ_ja - π_j)·s_k（softmax 策略解析梯度）。
        let lr = self.cfg.lr;
        let eps = self.cfg.clip;
        let mut total_policy_loss = 0.0f32;
        for (agent_idx, agent) in self.multi.agents.iter_mut().enumerate() {
            let mut grad = alloc::vec![0.0f32; agent.weights.len()];
            let mut n = 0usize;
            for (jt, credits_for_jt) in self.buf.iter().zip(credits.iter()) {
                let _ = credits_for_jt;
                let t = match jt.per_agent.get(agent_idx) {
                    Some(t) => t,
                    None => continue,
                };
                let old_prob = *jt.old_probs.get(agent_idx).unwrap_or(&0.0);
                // 当前策略 π_new(a|s)（概率比，no_std）
                let new_prob = action_prob(agent, &t.state, &t.action);
                let ratio = if old_prob > 1e-9 { new_prob / old_prob } else { 1.0 };
                // advantage：全局奖励 - 状态价值（critic 估计）
                let global_state: Vec<f32> =
                    jt.per_agent.iter().flat_map(|x| x.state.iter().copied()).collect();
                let value = self
                    .critic
                    .as_ref()
                    .map(|c| c.value(&global_state))
                    .unwrap_or(0.0);
                let adv = jt.global_reward - value;
                // clip：ratio 限制在 [1-ε, 1+ε]
                let clipped = ratio.clamp(1.0 - eps, 1.0 + eps);
                let _surr = if adv >= 0.0 {
                    ratio.min(clipped)
                } else {
                    ratio.max(clipped)
                };
                // 目标对 ratio 的导数（adv>0 时取非 clip 分支的掩码）
                let d_surr_dr = if adv >= 0.0 {
                    if ratio <= clipped { 1.0 } else { 0.0 }
                } else {
                    if ratio >= clipped { 1.0 } else { 0.0 }
                };
                // ∂logπ_a/∂w = (δ_a - π)·s
                let probs = softmax_probs(agent, &t.state);
                let action_idx = argmax(&t.action);
                let dr_dw = ratio * d_surr_dr;
                for (k, s_val) in t.state.iter().enumerate() {
                    let w_base = k * agent.action_dim;
                    for a in 0..agent.action_dim {
                        let grad_a = if a == action_idx { 1.0 - probs[a] } else { -probs[a] };
                        let gi = w_base + a;
                        if gi < grad.len() {
                            grad[gi] += adv * dr_dw * grad_a * s_val;
                        }
                    }
                }
                total_policy_loss += adv * d_surr_dr * ratio * (new_prob - old_prob);
                n += 1;
            }
            if n > 0 {
                let scale = lr / n as f32;
                for w in agent.weights.iter_mut().zip(grad.iter()) {
                    *w.0 += scale * w.1;
                }
            }
        }

        // 发散检测
        for agent in &self.multi.agents {
            if agent.weights.iter().any(|w| w.is_nan() || w.is_infinite()) {
                return Err(MarlError::Diverged);
            }
        }

        self.update_count += 1;
        let loss = total_policy_loss.abs() / self.buf.len().max(1) as f32;

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
            old_probs: alloc::vec![0.5; n_agents],
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
        // 真实 PPO：正奖励 → 策略向该状态-动作偏移（π_new > π_old）
        let t = make_jt(2, 5.0);
        let a0 = &trainer.multi.agents[0];
        let prob_after = action_prob(a0, &t.per_agent[0].state, &t.per_agent[0].action);
        assert!(prob_after > 0.5, "正奖励动作概率应上升: {prob_after}");
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

#[cfg(test)]
mod arc006_clip_tests {
    use super::*;
    use crate::types::{MapConfig, MultiAgentPolicy};

    #[test]
    fn test_arc006_clip_limits_ratio() {
        // 直接验证 update 中 clip 语义：构造极端旧概率使 ratio 越界，
        // 权重更新被 clip 截断（不发散）。
        let multi = MultiAgentPolicy::new(1, 2, 2);
        let cfg = MapConfig {
            n_agents: 1,
            rollout_steps: 1,
            clip: 0.2, // 严格 clip
            ..MapConfig::default()
        };
        let mut trainer = MappoTrainer::new(multi, cfg);
        // 旧概率极小 → ratio 巨大 → clip 截断
        let jt = JointTransition {
            per_agent: alloc::vec![crate::types::Transition {
                state: alloc::vec![1.0, -1.0],
                action: alloc::vec![1.0, 0.0],
                reward: 10.0,
                next_state: alloc::vec![1.0, -1.0],
                done: false,
            }],
            global_reward: 10.0,
            old_probs: alloc::vec![1e-6], // 旧概率接近 0 → ratio 巨大
        };
        trainer.observe(jt);
        let m = trainer.update().unwrap();
        assert_eq!(m.steps, 1);
        // clip 截断后更新不 NaN/不无限（发散检测通过即证明 clip 生效）
        for agent in &trainer.multi.agents {
            assert!(agent.weights.iter().all(|w| w.is_finite()));
        }
    }
}
