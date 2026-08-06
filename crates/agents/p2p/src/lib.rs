//! EnerOS P2P 电力交易协议 (workspace v0.192.0)
//!
//! 提供 Agent 间报价(Bid)、询价(Ask)、匹配(Match)通信能力，
//! 支持去中心化点对点电力交易基础协议。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod broadcast;
pub mod protocol;
pub mod types;

pub use broadcast::MockTransport;
pub use protocol::{can_match, validate_ask, validate_bid};
pub use types::{P2pAsk, P2pBid, P2pConfirm, P2pMatch, P2pMsg, P2pReject, TimeSlot};

/// P2P 交易协议错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2pError {
    /// 无效报价
    InvalidBid,
    /// 无效询价
    InvalidAsk,
    /// 消息已过期
    Expired,
    /// 传输层错误
    TransportError,
    /// 无可用消息
    NoMessage,
    /// 匹配失败
    MatchFailed,
}

impl core::fmt::Display for P2pError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBid => write!(f, "invalid bid"),
            Self::InvalidAsk => write!(f, "invalid ask"),
            Self::Expired => write!(f, "message expired"),
            Self::TransportError => write!(f, "transport error"),
            Self::NoMessage => write!(f, "no message available"),
            Self::MatchFailed => write!(f, "match failed"),
        }
    }
}

/// P2P 消息传输 trait
///
/// 实现此 trait 以提供报价/询价消息的广播与接收能力。
pub trait P2pTransport {
    /// 向指定 topic 广播消息
    fn broadcast(&mut self, topic: &str, msg: &P2pMsg) -> Result<(), P2pError>;

    /// 接收下一条消息
    fn recv(&mut self) -> Result<P2pMsg, P2pError>;
}
