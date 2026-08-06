//! EnerOS 模型在线学习 (workspace v0.192.0) — P4-D 高级 AI.
//!
//! 实现边缘端模型在线增量微调与联邦学习，让模型随运行数据持续优化。
//!
//! # 核心模块
//!
//! - [`types`] — OnlineModel, ComputeDevice, Batch, TrainMetrics 等核心类型
//! - [`finetune`] — 增量微调（IncrementalTrainer trait + mock 梯度下降）
//! - [`federated`] — 联邦学习（FedAvg 加权聚合）
//! - [`checkpoint`] — 检查点快照/回滚/发散检测
//!
//! # 架构
//!
//! ```text
//! 边缘采样数据 → 构造 Batch → 增量微调 step → 检查点快照
//!     → 周期性上报 → 联邦聚合 (FedAvg) → 全局模型下发
//!     → 发散检测 → 自动回滚
//! ```
//!
//! # GPU 规则（蓝图 §43.3）
//!
//! 本版本属于"模型训练/校准"场景，GPU 优先规则适用：
//! - 生产环境：model.to("cuda")，data.to("cuda")
//! - 测试推理：with torch.no_grad()
//! - GPU 不可用：回退 CPU
//!
//! 当前 Rust 实现为算法逻辑层（mock 计算），
//! PyTorch FFI 集成（真实 GPU 训练）为后续版本预留。
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `ai/online/` → `crates/ai/online/`（eneros-online-learning） | 记忆 §2.3.1 强制 crate 归 `crates/<subsystem>/` |
//! | **D2** | 蓝图 PyTorch FFI → mock 梯度下降 | no_std 环境无 PyTorch；本版实现算法逻辑，FFI 预留 |
//! | **D3** | 蓝图 Python 测试 → Rust 内嵌测试 | 项目惯例；GPU 测试待 FFI 集成后补充 |
//! | **D4** | 蓝图 `ClientId(Uuid)` → `u64` | 简化，零第三方依赖 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 零第三方依赖，零 unsafe，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod checkpoint;
pub mod federated;
pub mod finetune;
pub mod types;

pub use checkpoint::{Checkpoint, CheckpointManager};
pub use federated::{FedAvgAggregator, FederatedAggregator};
pub use finetune::{IncrementalTrainer, IncrementalTrainerImpl};
pub use types::{Batch, ComputeDevice, DeviceSelector, FinetuneState, OnlineModel, TrainMetrics};

/// AI 在线学习错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiError {
    /// 计算设备不可用（GPU 未找到）.
    DeviceUnavailable,
    /// 张量形状不匹配.
    ShapeMismatch,
    /// 训练发散（loss 为 NaN/Inf 或连续上升）.
    Diverged,
    /// 检查点损坏或不存在.
    CheckpointCorrupt,
}
