//! EnerOS Agent 抽象与描述符 (v0.43.0).
//!
//! 定义 Agent 作为 EnerOS 的一等操作系统公民。本 crate 提供核心数据结构：
//! - [`AgentDescriptor`] — Agent 描述符（13 字段），定义 Agent 的完整属性
//! - [`AgentType`] — Agent 类型（9 种 + Custom 扩展）
//! - [`AgentState`] — Agent 生命周期状态（7 种）
//! - [`TrustLevel`] — 信任等级（4 级分级）
//! - [`AgentId`] — 全局唯一标识符（基于原子计数器）
//! - [`CapabilityRef`] — 能力引用
//! - [`AgentMetadata`] — Agent 元数据
//! - [`AgentError`] — 错误类型
//! - [`AgentRegistry`] / [`RegistryStats`] — Agent 注册表（双索引，注册/查询/统计）
//! - [`LifecycleManager`] / [`LifecycleHook`] / [`LifecycleEvent`] — 生命周期状态机（12 合法转换）
//! - [`AgentConfig`] / [`AgentContext`] / [`AgentEntry`] — Agent 启动配置与入口 trait
//! - [`AgentSpawner`] / [`AgentFactory`] — Agent 启动器与工厂
//! - [`HealthStatus`] / [`HealthCheck`] — Agent 健康状态与自定义健康检查
//! - [`HeartbeatMonitor`] / [`HeartbeatState`] — Agent 心跳监控（1s 周期、3 次超时=故障）
//! - [`CheckpointStore`] / [`InMemoryCheckpointStore`] / [`Checkpointable`] — 检查点存储与可检查点 trait
//! - [`CrashRecovery`] — Agent 崩溃恢复器（最多 3 次重启、检查点恢复、3 次失败→Dead）
//! - [`CapabilityToken`] / [`CapabilityTokenBuilder`] / [`TokenVerifier`] — 能力令牌（SM2 签名 + 权限位集 + 电力约束）
//! - [`TokenStore`] / [`CapabilityManager`] — 能力管理器（签发/校验/冻结/撤销/过期清理）
//! - [`SystemAgent`] / [`ResourceMonitor`] / [`SystemConfig`] / [`SystemStats`] / [`SystemEvent`] — OS 级管理 Agent（v0.41.0，资源监控+生命周期编排+OOM/过热保护）
//! - [`DependencyGraph`] / [`RecoveryOrchestrator`] / [`RecoveryPriority`] — 故障恢复编排（v0.42.0，依赖图+优先级调度）
//!
//! # 架构定位
//! 本 crate 是 Agent Runtime 的基础，所有后续 Agent 管理功能
//! （注册表 / 生命周期 / 心跳 / 能力管理）都基于此结构。
//!
//! # no_std 合规
//! 所有代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`。依赖 `eneros-crypto`（国密 SM2/SM3/SM4）。
//!
//! # 偏差声明
//! 1. **ID 生成策略**：当前使用基于 `AtomicU64` 的全局递增计数器（从 1 开始），
//!    预留上 64 位为 epoch。生产环境如需跨节点唯一性，可叠加节点 ID 编码。
//! 2. **时间戳来源**：no_std 无系统时钟，`now: u64` 由外部提供。
//! 3. **访问控制**：`can_access` 当前基于 `TrustLevel` 阈值（>= Verified），
//!    v0.39.0 能力系统已实现（capability-based 检查）。
//!    v0.40.0 能力管理器已实现（集中式签发/校验/冻结/撤销）。
//!    v0.41.0 System Agent 已实现（OS 级管理 Agent，tick 主循环+OOM/过热保护）。
//!    v0.42.0 故障恢复编排已实现（DependencyGraph + RecoveryOrchestrator，依赖图+优先级调度）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod capability;
pub mod checkpoint;
pub mod descriptor;
pub mod error;
pub mod health;
pub mod heartbeat;
pub mod id;
pub mod init;
pub mod lifecycle;
pub mod recovery;
pub mod registry;
pub mod spawner;
pub mod system_agent;
pub mod types;

// Re-export key types for convenience.
pub use capability::{
    CapabilityManager, CapabilityToken, CapabilityTokenBuilder, ConstraintPack, ConstraintType,
    DeviceId, PermissionSet, ResourceTarget, SocketAddr, SystemResource, TokenStore, TokenVerifier,
};
pub use checkpoint::{CheckpointStore, Checkpointable, InMemoryCheckpointStore};
pub use descriptor::AgentDescriptor;
pub use error::AgentError;
pub use health::{HealthCheck, HealthStatus};
pub use heartbeat::{HeartbeatMonitor, HeartbeatState};
pub use id::AgentId;
pub use init::{AgentConfig, AgentContext, AgentEntry};
pub use lifecycle::{LifecycleEvent, LifecycleHook, LifecycleManager};
pub use recovery::CrashRecovery;
pub use registry::{AgentRegistry, RegistryStats};
pub use spawner::{AgentFactory, AgentSpawner};
pub use system_agent::{
    priority_of, AgentResourceUsage, DependencyGraph, RecoveryOrchestrator, RecoveryPriority,
    ResourceMonitor, ResourceSource, SystemAgent, SystemConfig, SystemEvent, SystemStats,
};
pub use types::{AgentMetadata, AgentState, AgentType, CapabilityRef, TrustLevel};

/// Crate version string.
pub const VERSION: &str = "0.43.0";
