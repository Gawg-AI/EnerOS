//! 策略网络封装 (workspace v0.192.0).
//!
//! 策略推理（act）与动作采样。

use alloc::vec::Vec;

use crate::types::AgentPolicy;
use crate::RlError;

/// 动作采样器 trait.
pub trait ActionSampler {
    /// 从 logits 采样动作（推理时使用 no_grad）.
    fn sample(&self, logits: &[f32]) -> Vec<f32>;
}

/// 确定性采样器（取 argmax 方向）.
pub struct DeterministicSampler;

impl ActionSampler for DeterministicSampler {
    fn sample(&self, logits: &[f32]) -> Vec<f32> {
        // 确定性策略：直接输出 tanh(logits) 作为连续动作
        logits.iter().map(|&l| tanh_approx(l)).collect()
    }
}

/// 策略推理：state × weights → action logits → 采样.
///
/// Mock 实现：简单矩阵乘法 state`[W]` → logits。
pub fn policy_act(policy: &AgentPolicy, state: &[f32]) -> Result<Vec<f32>, RlError> {
    if state.len() != policy.state_dim {
        return Err(RlError::StateDimMismatch);
    }

    let sampler = DeterministicSampler;
    let logits = forward(policy, state);
    Ok(sampler.sample(&logits))
}

/// 前向传播（mock：简单线性变换）.
///
/// logits[j] = sum_i(state[i] * weights[i * action_dim + j])
fn forward(policy: &AgentPolicy, state: &[f32]) -> Vec<f32> {
    let mut logits = alloc::vec![0.0f32; policy.action_dim];
    for (i, &s) in state.iter().enumerate() {
        for (j, logit) in logits.iter_mut().enumerate() {
            let w_idx = i * policy.action_dim + j;
            if w_idx < policy.weights.len() {
                *logit += s * policy.weights[w_idx];
            }
        }
    }
    logits
}

/// tanh 近似（no_std 无 libm）.
fn tanh_approx(x: f32) -> f32 {
    // 简单 clamp 到 [-1, 1] 作为 tanh 近似
    x.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_act() {
        let policy = AgentPolicy::new(4, 2);
        let state = alloc::vec![1.0, 0.5, -0.3, 0.8];
        let action = policy_act(&policy, &state).unwrap();
        assert_eq!(action.len(), 2);
        // 动作在 [-1, 1] 范围内
        for &a in &action {
            assert!((-1.0..=1.0).contains(&a));
        }
    }

    #[test]
    fn test_policy_act_dim_mismatch() {
        let policy = AgentPolicy::new(4, 2);
        let state = alloc::vec![1.0, 2.0]; // 维度不匹配
        assert_eq!(
            policy_act(&policy, &state).unwrap_err(),
            RlError::StateDimMismatch
        );
    }

    #[test]
    fn test_deterministic_sampler() {
        let sampler = DeterministicSampler;
        let logits = alloc::vec![0.5, -0.3, 1.5, -2.0];
        let actions = sampler.sample(&logits);
        assert_eq!(actions.len(), 4);
        assert!((actions[0] - 0.5).abs() < 1e-6);
        assert!((actions[2] - 1.0).abs() < 1e-6); // clamped
        assert!((actions[3] - (-1.0)).abs() < 1e-6); // clamped
    }

    #[test]
    fn test_forward() {
        let mut policy = AgentPolicy::new(2, 2);
        policy.weights = alloc::vec![1.0, 0.0, 0.0, 1.0]; // 单位矩阵
        let state = alloc::vec![3.0, 7.0];
        let logits = forward(&policy, &state);
        assert!((logits[0] - 3.0).abs() < 1e-6);
        assert!((logits[1] - 7.0).abs() < 1e-6);
    }
}
