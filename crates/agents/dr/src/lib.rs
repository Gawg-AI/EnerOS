//! # EnerOS 需求响应聚合 (Demand Response Aggregation)
//!
//! v0.173.0 — DR 信号响应 + 负荷聚合
//!
//! 本 crate 实现需求响应聚合的核心逻辑：
//! - 数据类型定义 (`types`)
//! - DR 信号验证与激活 (`signal`)
//! - 负荷聚合与调度 (`aggregator`)
//! - 响应跟踪 (`response`)

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod aggregator;
pub mod response;
pub mod signal;
pub mod types;

pub use aggregator::{DrAggregator, DrAggregatorImpl};
pub use response::DrResponseTracker;
pub use signal::{is_active, validate_signal};
pub use types::{AggregatedCapacity, DrAssignment, DrParticipant, DrSignal, DrSignalType};

/// 需求响应错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrError {
    /// 信号无效
    InvalidSignal,
    /// 无可用参与者
    NoParticipants,
    /// 容量不足
    CapacityInsufficient,
    /// 参与者未找到
    ParticipantNotFound,
    /// 已确认过
    AlreadyAcknowledged,
    /// 事件未激活
    EventNotActive,
}

impl core::fmt::Display for DrError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSignal => write!(f, "invalid DR signal"),
            Self::NoParticipants => write!(f, "no available participants"),
            Self::CapacityInsufficient => write!(f, "capacity insufficient"),
            Self::ParticipantNotFound => write!(f, "participant not found"),
            Self::AlreadyAcknowledged => write!(f, "already acknowledged"),
            Self::EventNotActive => write!(f, "event not active"),
        }
    }
}
