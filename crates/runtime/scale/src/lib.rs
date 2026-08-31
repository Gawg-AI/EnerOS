//! # eneros-scale — 规模资格基础设施（蓝图 61 §12 v0.314.48）
//!
//! 分片（rendezvous hash + 版本化环 + 五步重分片）、HA（3 voter quorum +
//! leader lease + 单调 fencing）、DR（恢复顺序规划 + RPO/RTO 记账）与
//! 容量测试基底（负载生成器 + oracle 等价面 + 有界队列观测）。
//!
//! no_std + alloc；零第三方依赖（仅 core/alloc）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod capacity;
pub mod dr;
pub mod quorum;
pub mod shard;

pub use capacity::{
    equivalence_check, DropPolicy, EquivalenceReport, LoadEvent, LoadGen, Oracle, QueueStats,
    QueuedSink,
};
pub use dr::{
    RecoveryMetrics, RestoreComponent, RestorePhase, RestorePlanner, RestoreProgress, RTO_TO_D0_MS,
    RTO_TO_D4_MS,
};
pub use quorum::{
    FenceGuard, FenceToken, LeaderLease, QuorumError, QuorumSet, Readiness, ReadyComponent,
};
pub use shard::{
    dual_route, ReshardPlan, ReshardStep, RouteResult, ShardError, ShardKey, ShardRing,
};
