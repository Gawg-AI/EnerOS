//! # EnerOS 辅助服务参与 (Ancillary Service Participation)
//!
//! v0.172.0 — 调频/调峰 + AGC 接口
//!
//! 本 crate 实现辅助服务参与的核心逻辑：
//! - 数据类型定义 (`types`)
//! - AGC 命令处理 (`agc`)
//! - 调频控制 (`frequency`)
//! - 削峰控制 (`peak_shaving`)

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod agc;
pub mod frequency;
pub mod peak_shaving;
pub mod types;

pub use agc::AgcHandler;
pub use frequency::{FreqRegImpl, FrequencyRegulator};
pub use peak_shaving::PeakShaveController;
pub use types::{
    AgcAck, AgcCommand, AncillaryBid, AncillaryType, BidAck, FreqRegState, PeakShavePlan,
};

/// 辅助服务错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AncillaryError {
    /// 报价无效
    InvalidBid,
    /// AGC 命令超时
    AgcTimeout,
    /// SOC 过低，无法放电
    SocTooLow,
    /// 爬坡速率超限
    RampExceeded,
    /// 命令未找到
    CommandNotFound,
    /// 频率值无效 (NaN / Inf)
    InvalidFrequency,
}

impl core::fmt::Display for AncillaryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBid => write!(f, "invalid bid"),
            Self::AgcTimeout => write!(f, "AGC command timeout"),
            Self::SocTooLow => write!(f, "SOC too low"),
            Self::RampExceeded => write!(f, "ramp rate exceeded"),
            Self::CommandNotFound => write!(f, "command not found"),
            Self::InvalidFrequency => write!(f, "invalid frequency value"),
        }
    }
}

/// 辅助服务参与者 trait
///
/// 抽象参与辅助服务市场的设备/代理行为。
pub trait AncillaryParticipant {
    /// 提交辅助服务报价
    fn submit_bid(&mut self, bid: &AncillaryBid) -> Result<BidAck, AncillaryError>;

    /// 处理 AGC 调度命令
    fn handle_agc(&mut self, cmd: &AgcCommand) -> Result<AgcAck, AncillaryError>;

    /// 获取当前可用容量 (kW)
    fn available_capacity_kw(&self) -> f64;
}
