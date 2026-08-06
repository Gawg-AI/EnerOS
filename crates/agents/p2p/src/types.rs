//! P2P 交易协议数据类型定义

use alloc::string::String;

// PTC-001：TimeSlot 统一为 market-types 定义（UTC **秒**）。
// 旧自定义定义注释为 unix **毫秒**，与 market-types 秒语义存在 1000 倍
// 冲突（B10）——已删除本地定义并 re-export 单一来源。
pub use eneros_market_types::TimeSlot;

/// 卖方报价 (Bid)
#[derive(Debug, Clone, PartialEq)]
pub struct P2pBid {
    /// 卖方 Agent ID
    pub agent: u64,
    /// 出售电量 (kWh)
    pub volume_kwh: f64,
    /// 单价 (元/kWh)
    pub price: f64,
    /// 有效截止时间戳
    pub valid_until: u64,
    /// 消息序列号
    pub seq: u64,
}

/// 买方询价 (Ask)
#[derive(Debug, Clone, PartialEq)]
pub struct P2pAsk {
    /// 买方 Agent ID
    pub agent: u64,
    /// 需求电量 (kWh)
    pub volume_kwh: f64,
    /// 最高可接受单价 (元/kWh)
    pub max_price: f64,
    /// 有效截止时间戳
    pub valid_until: u64,
    /// 消息序列号
    pub seq: u64,
}

/// 撮合结果
#[derive(Debug, Clone, PartialEq)]
pub struct P2pMatch {
    /// 撮合 ID
    pub match_id: u64,
    /// 卖方 Agent ID
    pub seller: u64,
    /// 买方 Agent ID
    pub buyer: u64,
    /// 成交电量 (kWh)
    pub volume_kwh: f64,
    /// 成交单价 (元/kWh)
    pub price: f64,
    /// 交割开始时间
    pub delivery_start: u64,
    /// 交割结束时间
    pub delivery_end: u64,
}

/// 成交确认
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pConfirm {
    /// 撮合 ID
    pub match_id: u64,
    /// 确认方 Agent ID
    pub agent: u64,
    /// 是否确认
    pub confirmed: bool,
    /// 确认时间戳
    pub at: u64,
}

/// 成交拒绝
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pReject {
    /// 撮合 ID
    pub match_id: u64,
    /// 拒绝方 Agent ID
    pub agent: u64,
    /// 拒绝原因
    pub reason: String,
    /// 拒绝时间戳
    pub at: u64,
}

/// P2P 交易消息枚举
#[derive(Debug, Clone, PartialEq)]
pub enum P2pMsg {
    /// 卖方报价
    Bid(P2pBid),
    /// 买方询价
    Ask(P2pAsk),
    /// 撮合结果
    Match(P2pMatch),
    /// 成交确认
    Confirm(P2pConfirm),
    /// 成交拒绝
    Reject(P2pReject),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeslot_creation() {
        let slot = TimeSlot {
            start: 1000,
            end: 2000,
        };
        assert_eq!(slot.start, 1000);
        assert_eq!(slot.end, 2000);
    }

    #[test]
    fn test_bid_creation() {
        let bid = P2pBid {
            agent: 1,
            volume_kwh: 100.0,
            price: 0.65,
            valid_until: 9999,
            seq: 1,
        };
        assert_eq!(bid.agent, 1);
        assert_eq!(bid.volume_kwh, 100.0);
        assert_eq!(bid.price, 0.65);
    }

    #[test]
    fn test_ask_creation() {
        let ask = P2pAsk {
            agent: 2,
            volume_kwh: 50.0,
            max_price: 0.80,
            valid_until: 9999,
            seq: 1,
        };
        assert_eq!(ask.agent, 2);
        assert_eq!(ask.max_price, 0.80);
    }

    #[test]
    fn test_match_creation() {
        let m = P2pMatch {
            match_id: 100,
            seller: 1,
            buyer: 2,
            volume_kwh: 50.0,
            price: 0.70,
            delivery_start: 1000,
            delivery_end: 2000,
        };
        assert_eq!(m.match_id, 100);
        assert_eq!(m.seller, 1);
        assert_eq!(m.buyer, 2);
    }

    #[test]
    fn test_confirm_creation() {
        let c = P2pConfirm {
            match_id: 100,
            agent: 1,
            confirmed: true,
            at: 5000,
        };
        assert!(c.confirmed);
    }

    #[test]
    fn test_reject_creation() {
        let r = P2pReject {
            match_id: 100,
            agent: 2,
            reason: String::from("price too high"),
            at: 5000,
        };
        assert_eq!(r.reason, "price too high");
    }

    #[test]
    fn test_msg_enum_matching() {
        let msg = P2pMsg::Bid(P2pBid {
            agent: 1,
            volume_kwh: 10.0,
            price: 0.5,
            valid_until: 9999,
            seq: 1,
        });
        assert!(matches!(&msg, P2pMsg::Bid(b) if b.agent == 1));

        let msg2 = P2pMsg::Ask(P2pAsk {
            agent: 2,
            volume_kwh: 20.0,
            max_price: 0.9,
            valid_until: 9999,
            seq: 2,
        });
        assert!(matches!(&msg2, P2pMsg::Ask(a) if a.agent == 2));
    }
}
