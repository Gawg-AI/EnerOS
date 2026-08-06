//! EnerOS P2P 撮合引擎 (workspace v0.192.0)
//!
//! 提供价格时间优先撮合算法与成交生命周期管理能力，
//! 支持去中心化 P2P 电力交易场景。
//!
//! # 核心功能
//!
//! - **价格时间优先撮合**: 卖方低价优先、买方高价优先、同价时间优先
//! - **成交管理**: 完整的成交状态机 (Matched → Confirmed → Delivering → Settled)
//! - **执行报告**: 交割量偏差记录与校验

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod deal;
pub mod matcher;
pub mod types;

pub use deal::DealManager;
pub use matcher::{MatchEngine, MatchStrategy, PriceTimePriority};
pub use types::{Deal, DealState, ExecReport, P2pAsk, P2pBid};

/// 撮合引擎错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    /// 无匹配订单
    NoMatch,
    /// 成交记录不存在
    DealNotFound,
    /// 非法状态转换
    InvalidTransition,
    /// 交割量不匹配
    VolumeMismatch,
    /// 价格差距无法撮合
    PriceGap,
}

impl core::fmt::Display for MatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoMatch => write!(f, "no matching orders"),
            Self::DealNotFound => write!(f, "deal not found"),
            Self::InvalidTransition => write!(f, "invalid state transition"),
            Self::VolumeMismatch => write!(f, "volume mismatch"),
            Self::PriceGap => write!(f, "price gap too large"),
        }
    }
}
