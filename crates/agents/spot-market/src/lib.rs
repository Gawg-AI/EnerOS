//! # EnerOS 现货市场参与 (Spot Market Participation)
//!
//! v0.171.0 — 日前/日内报价 + 出清 + 执行跟踪
//!
//! 本 crate 实现电力现货市场参与的核心逻辑：
//! - 报价构建与校验 (`bid`)
//! - 出清结果处理 (`clearing`)
//! - 执行状态跟踪 (`tracker`)
//! - 参与者抽象 (`participant`)

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bid;
pub mod clearing;
pub mod gateway;
pub mod participant;
pub mod tracker;
pub mod types;

pub use bid::{validate_bid, BidBuilder};
pub use clearing::ClearingProcessor;
pub use participant::{MockParticipant, SpotMarketParticipant};
pub use tracker::ExecTracker;
pub use types::{
    Bid, BidAck, BidSegment, ClearedSegment, ClearingResult, ExecStatus, SpotMarketType, TimeSlot,
    TradeDir,
};

/// 现货市场错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketError {
    /// 报价无效（段为空、量/价非法、时段无效）
    InvalidBid,
    /// 提交超时
    SubmitTimeout,
    /// 出清结果未找到
    ClearingNotFound,
    /// 签名无效
    SignatureInvalid,
    /// ISO 不可达
    IsoUnreachable,
    /// 偏差超限
    DeviationExceeds,
    /// 非法状态转换
    InvalidStateTransition,
}

impl core::fmt::Display for MarketError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBid => write!(f, "invalid bid"),
            Self::SubmitTimeout => write!(f, "submit timeout"),
            Self::ClearingNotFound => write!(f, "clearing not found"),
            Self::SignatureInvalid => write!(f, "signature invalid"),
            Self::IsoUnreachable => write!(f, "ISO unreachable"),
            Self::DeviationExceeds => write!(f, "deviation exceeds threshold"),
            Self::InvalidStateTransition => write!(f, "invalid state transition"),
        }
    }
}
