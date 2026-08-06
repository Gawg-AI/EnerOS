//! EnerOS Agent 自主交易 (workspace v0.192.0)
//!
//! 实现 Agent 间自主交易全闭环：报价 → 撮合 → 冻结 → 确认 → 交付 → 结算。
//!
//! # 核心模块
//!
//! - **types**: 交易阶段、结算信息、交易上下文
//! - **orchestrator**: 交易编排器，管理状态机转换
//! - **settlement**: 结算触发器 trait 与自动结算实现
//! - **closed_loop**: 闭环驱动，自动推进交易至终态

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod closed_loop;
pub mod orchestrator;
pub mod settlement;
pub mod types;

pub use closed_loop::{ClosedLoop, ClosedLoopImpl};
pub use orchestrator::TradeOrchestrator;
pub use settlement::{AutoSettlement, SettlementTrigger};
pub use types::{Settlement, TradeContext, TradePhase};

/// 交易系统错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeError {
    /// 交易不存在
    DealNotFound,
    /// 非法阶段转换
    InvalidPhase,
    /// 结算失败
    SettlementFailed,
    /// 冻结失败
    FreezeFailed,
    /// 交付失败
    DeliveryFailed,
}

impl core::fmt::Display for TradeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DealNotFound => write!(f, "deal not found"),
            Self::InvalidPhase => write!(f, "invalid phase transition"),
            Self::SettlementFailed => write!(f, "settlement failed"),
            Self::FreezeFailed => write!(f, "freeze failed"),
            Self::DeliveryFailed => write!(f, "delivery failed"),
        }
    }
}
