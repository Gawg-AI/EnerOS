//! EnerOS Agent 强化学习 (v0.187.0) — P4-D 高级 AI.
//!
//! 实现 Agent RL 框架：PPO 策略训练 + Reward 设计 + Replay Buffer。
//! 使调度策略通过 Reward 反馈自进化，超越静态优化。
//!
//! # 核心模块
//!
//! - [`types`] — AgentPolicy, Transition, PpoConfig, ReplayBuffer 等
//! - [`policy`] — 策略推理（act）与动作采样
//! - [`reward`] — Reward Shaping（收益-成本-安全惩罚）
//! - [`ppo`] — PPO 训练器（act/observe/update 闭环）
//!
//! # RL 循环
//!
//! ```text
//! 状态(电价/SOC/负荷) → 策略推理(no_grad) → 动作(充放电功率)
//!   → 环境执行 → Reward(收益-成本-惩罚) → Replay Buffer
//!   → 达 rollout → PPO 更新策略网络 → 循环
//! ```
//!
//! # GPU 规则（蓝图 §43.3）
//!
//! 本版本适用 GPU 优先规则：
//! - 策略网络 policy.to(device)，状态 state.to(device)
//! - act 推理包 with torch.no_grad()
//! - GPU 不可用回退 CPU
//!
//! 当前 Rust 实现为算法逻辑层（mock），PyTorch FFI 为后续预留。
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `ai/rl/` → `crates/ai/rl/`（eneros-rl） | 记忆 §2.3.1 强制 |
//! | **D2** | 蓝图 PyTorch FFI → mock 线性策略 + 简化 PPO | no_std 无 PyTorch |
//! | **D3** | 蓝图 Python 测试 → Rust 内嵌测试 | 项目惯例 |
//!
//! # no_std 合规
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 零第三方依赖，零 unsafe。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod policy;
pub mod ppo;
pub mod reward;
pub mod types;

pub use policy::{policy_act, ActionSampler, DeterministicSampler};
pub use ppo::{PolicyTrainer, PpoTrainer};
pub use reward::{DefaultReward, RewardShaper};
pub use types::{
    AgentPolicy, ComputeDevice, PpoConfig, ReplayBuffer, RewardCtx, TrainMetrics, Transition,
};

/// 强化学习错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlError {
    /// 状态维度不匹配.
    StateDimMismatch,
    /// 动作维度不匹配.
    ActionDimMismatch,
    /// 训练发散.
    Diverged,
    /// 经验不足.
    InsufficientData,
    /// 设备不可用.
    DeviceUnavailable,
}
