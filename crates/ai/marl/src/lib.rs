//! v1.0.0_scope = "deferred"（22 §2.W2.5：本期不做 RL/MARL 真实化；
//! 轻量在线学习见 dual-brain::learning，锁定 8）。
//!
//! # 商业范围（21-B A4）
//!
//! **`commercial_scope = "out-of-v1.0"`**（与 Cargo.toml `[package.metadata.eneros]`
//! 双处声明）：MARL 不在 v1.0.0 主链路——当前实现为 mock 算法层
//! （PyTorch FFI 后续预留），与 AI 决策主链路（eneros-ai-runner）解耦。
//! EnerOS 多 Agent 强化学习 (workspace v0.192.0) — P4-D 高级 AI.
//!
//! 实现 MARL 框架：MAPPO（集中训练分散执行）+ 信用分配 + 联邦协同进化。
//! 多个边缘 Agent 在联邦协同中共同进化，优化全局收益。
//!
//! # 核心模块
//!
//! - [`types`] — MultiAgentPolicy, JointTransition, MapConfig 等
//! - [`coord`] — 信用分配（EqualCredit / ProportionalCredit）+ GlobalCritic
//! - [`mappo`] — MAPPO 训练器（joint_act / observe / update）
//!
//! # CTDE 架构
//!
//! ```text
//! 各 Agent 局部状态 → 各策略推理(no_grad, 分散执行) → 联合动作
//!   → 环境执行 → 全局 Reward → 信用分配 → MAPPO 更新(集中训练)
//! ```
//!
//! # GPU 规则
//!
//! 各 Agent 策略网络 .to(device)，joint_act 推理 no_grad，GPU 不可用回退 CPU。
//! 当前 Rust 实现为 mock，PyTorch MARL FFI 为后续预留。
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `ai/marl/` → `crates/ai/marl/`（eneros-marl） | 记忆 §2.3.1 强制 |
//! | **D2** | 蓝图 PyTorch FFI → mock 线性策略 + 简化 MAPPO | no_std 无 PyTorch |
//! | **D3** | 蓝图 Python 测试 → Rust 内嵌测试 | 项目惯例 |
//!
//! # no_std 合规
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 零第三方依赖，零 unsafe。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod coord;
pub mod mappo;
pub mod types;

pub use coord::{CoordPolicy, EqualCredit, GlobalCritic, LinearCritic, ProportionalCredit};
pub use mappo::{MappoTrainer, MarlTrainer};
pub use types::{
    AgentPolicy, ComputeDevice, JointTransition, MapConfig, MultiAgentPolicy, TrainMetrics,
    Transition,
};

/// MARL 错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarlError {
    /// Agent 未找到.
    AgentNotFound,
    /// 状态维度不匹配.
    StateDimMismatch,
    /// 训练发散.
    Diverged,
    /// 经验不足.
    InsufficientData,
    /// 设备不可用.
    DeviceUnavailable,
}
