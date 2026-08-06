//! 协同协调 — 信用分配 (workspace v0.192.0).
//!
//! 将全局 Reward 分配到各 Agent（信用分配问题）。

use alloc::vec::Vec;

use crate::types::JointTransition;

/// 协同策略 trait — 信用分配.
pub trait CoordPolicy {
    /// 将全局奖励分配到各 Agent.
    ///
    /// 返回每个 Agent 的局部奖励。
    fn credit(&self, jt: &JointTransition) -> Vec<f32>;
}

/// 均分信用分配.
///
/// 将全局奖励平均分配给所有 Agent。
/// 简单、可解释，适用于合作博弈。
pub struct EqualCredit;

impl CoordPolicy for EqualCredit {
    fn credit(&self, jt: &JointTransition) -> Vec<f32> {
        let n = jt.per_agent.len();
        if n == 0 {
            return Vec::new();
        }
        let share = jt.global_reward / n as f32;
        alloc::vec![share; n]
    }
}

/// 按局部奖励比例的信用分配.
///
/// 根据各 Agent 局部奖励占全局奖励的比例分配。
/// 若全局奖励为 0，退化为均分。
pub struct ProportionalCredit;

impl CoordPolicy for ProportionalCredit {
    fn credit(&self, jt: &JointTransition) -> Vec<f32> {
        let n = jt.per_agent.len();
        if n == 0 {
            return Vec::new();
        }

        let local_sum: f32 = jt.per_agent.iter().map(|t| t.reward.abs()).sum();
        if local_sum < 1e-9 {
            // 退化为均分
            let share = jt.global_reward / n as f32;
            return alloc::vec![share; n];
        }

        jt.per_agent
            .iter()
            .map(|t| (t.reward.abs() / local_sum) * jt.global_reward)
            .collect()
    }
}

/// 全局 Critic trait（集中训练）.
pub trait GlobalCritic {
    /// 评估全局状态价值（no_grad 推理）.
    fn value(&self, global_state: &[f32]) -> f32;
}

/// 简单线性 Critic（mock 实现）.
pub struct LinearCritic {
    /// 权重.
    pub weights: Vec<f32>,
}

impl LinearCritic {
    /// 创建.
    pub fn new(dim: usize) -> Self {
        Self {
            weights: alloc::vec![0.01f32; dim],
        }
    }
}

impl GlobalCritic for LinearCritic {
    fn value(&self, global_state: &[f32]) -> f32 {
        // 简单点积
        global_state
            .iter()
            .zip(self.weights.iter())
            .map(|(s, w)| s * w)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Transition;

    fn make_jt(rewards: &[f32], global: f32) -> JointTransition {
        JointTransition {
            per_agent: rewards
                .iter()
                .map(|&r| Transition {
                    state: alloc::vec![1.0],
                    action: alloc::vec![0.5],
                    reward: r,
                    next_state: alloc::vec![2.0],
                    done: false,
                })
                .collect(),
            global_reward: global,
            old_probs: alloc::vec![0.5; rewards.len()],
        }
    }

    #[test]
    fn test_equal_credit() {
        let credit = EqualCredit;
        let jt = make_jt(&[1.0, 2.0, 3.0], 9.0);
        let shares = credit.credit(&jt);
        assert_eq!(shares.len(), 3);
        for s in &shares {
            assert!((s - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_equal_credit_empty() {
        let credit = EqualCredit;
        let jt = JointTransition {
            per_agent: Vec::new(),
            global_reward: 5.0,
            old_probs: Vec::new(),
        };
        assert!(credit.credit(&jt).is_empty());
    }

    #[test]
    fn test_proportional_credit() {
        let credit = ProportionalCredit;
        let jt = make_jt(&[1.0, 3.0], 8.0);
        let shares = credit.credit(&jt);
        // Agent 0: 1/4 * 8 = 2, Agent 1: 3/4 * 8 = 6
        assert!((shares[0] - 2.0).abs() < 1e-5);
        assert!((shares[1] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn test_proportional_credit_zero_local() {
        let credit = ProportionalCredit;
        let jt = make_jt(&[0.0, 0.0], 6.0);
        let shares = credit.credit(&jt);
        // 退化为均分
        assert!((shares[0] - 3.0).abs() < 1e-6);
        assert!((shares[1] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_linear_critic() {
        let critic = LinearCritic::new(4);
        let state = alloc::vec![1.0, 2.0, 3.0, 4.0];
        let v = critic.value(&state);
        // 0.01 * (1+2+3+4) = 0.1
        assert!((v - 0.1).abs() < 1e-5);
    }
}
