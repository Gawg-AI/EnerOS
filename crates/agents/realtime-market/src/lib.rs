//! # EnerOS 实时市场 (Realtime Market)
//!
//! v0.174.0 — 15min 级实时报价闭环
//!
//! 本 crate 实现实时市场的核心逻辑：
//! - 数据类型定义 (`types`)
//! - 交易会话管理 (`session`)
//! - 报价调度与验证 (`quote`)
//! - 报价闭环引擎 (`quote_loop`)

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod quote;
pub mod quote_loop;
pub mod session;
pub mod types;

pub use quote::{QuoteScheduler, QuoteValidator};
pub use quote_loop::{RealtimeLoop, RealtimeMarket};
pub use session::SessionManager;
pub use types::{
    QuoteSchedule, RealtimeClearing, RealtimeQuote, RealtimeSession, SessionState, TradeDir,
};

/// 实时市场错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeError {
    /// 会话未找到
    SessionNotFound,
    /// 会话未开放
    SessionNotOpen,
    /// 会话已关闭
    SessionAlreadyClosed,
    /// 报价无效
    InvalidQuote,
    /// 报价过于频繁
    QuoteTooFrequent,
    /// 出清结果未找到
    ClearingNotFound,
}

impl core::fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SessionNotFound => write!(f, "session not found"),
            Self::SessionNotOpen => write!(f, "session not open"),
            Self::SessionAlreadyClosed => write!(f, "session already closed"),
            Self::InvalidQuote => write!(f, "invalid quote"),
            Self::QuoteTooFrequent => write!(f, "quote too frequent"),
            Self::ClearingNotFound => write!(f, "clearing not found"),
        }
    }
}
