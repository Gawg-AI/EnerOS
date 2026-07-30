//! EnerOS Agent 信誉经济激励 (v0.181.0)。
//!
//! 提供信誉评分（Scorer）、经济激励（Incentive）与联动表（Linkage）三大核心能力，
//! 实现信誉→权限→收益的完整联动链路。
//!
//! # 架构
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  eneros-reputation (v0.181.0)                               │
//! │  ┌────────────┐  ┌──────────────┐  ┌───────────────────┐   │
//! │  │ scorer     │  │ incentive    │  │ linkage           │   │
//! │  │ (Engine)   │  │(Policy/Gate) │  │(LinkageTable)     │   │
//! │  └────────────┘  └──────────────┘  └───────────────────┘   │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │ types (ReputationGrade, ReputationScore, ...)         │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # no_std 合规
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod incentive;
pub mod linkage;
pub mod scorer;
pub mod types;

pub use incentive::{IncentivePolicy, LinearIncentive, PermissionGate};
pub use linkage::LinkageTable;
pub use scorer::{ReputationEngine, ReputationEngineImpl};
pub use types::{
    PermissionLevel, ReputationEvent, ReputationGrade, ReputationKind, ReputationScore,
};

/// 信誉模块错误类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepError {
    /// Agent 未找到。
    AgentNotFound,
    /// Agent 已被封禁。
    Banned,
    /// 交易量超出限制。
    VolumeExceedsLimit,
    /// 无效的信誉分。
    InvalidScore,
}
