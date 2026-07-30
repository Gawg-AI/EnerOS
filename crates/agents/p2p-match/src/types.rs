//! P2P 撮合引擎数据类型定义

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

/// 成交状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DealState {
    /// 已撮合
    Matched,
    /// 已确认
    Confirmed,
    /// 交割中
    Delivering,
    /// 已结算
    Settled,
    /// 已取消
    Cancelled,
}

/// 成交记录
#[derive(Debug, Clone, PartialEq)]
pub struct Deal {
    /// 成交 ID
    pub deal_id: u64,
    /// 卖方 Agent ID
    pub seller: u64,
    /// 买方 Agent ID
    pub buyer: u64,
    /// 成交电量 (kWh)
    pub volume_kwh: f64,
    /// 成交单价 (元/kWh)
    pub price: f64,
    /// 交割窗口开始时间
    pub window_start: u64,
    /// 交割窗口结束时间
    pub window_end: u64,
    /// 成交状态
    pub state: DealState,
}

/// 执行报告
#[derive(Debug, Clone, PartialEq)]
pub struct ExecReport {
    /// 关联成交 ID
    pub deal_id: u64,
    /// 实际交割电量 (kWh)
    pub delivered_kwh: f64,
    /// 偏差量 (kWh)
    pub deviation: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(bid.valid_until, 9999);
        assert_eq!(bid.seq, 1);
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
        assert_eq!(ask.volume_kwh, 50.0);
        assert_eq!(ask.max_price, 0.80);
    }

    #[test]
    fn test_deal_creation() {
        let deal = Deal {
            deal_id: 1,
            seller: 10,
            buyer: 20,
            volume_kwh: 50.0,
            price: 0.70,
            window_start: 1000,
            window_end: 2000,
            state: DealState::Matched,
        };
        assert_eq!(deal.deal_id, 1);
        assert_eq!(deal.seller, 10);
        assert_eq!(deal.buyer, 20);
        assert_eq!(deal.state, DealState::Matched);
    }

    #[test]
    fn test_deal_state_transitions_enum() {
        let states = [
            DealState::Matched,
            DealState::Confirmed,
            DealState::Delivering,
            DealState::Settled,
            DealState::Cancelled,
        ];
        assert_eq!(states.len(), 5);
        assert_ne!(DealState::Matched, DealState::Settled);
    }

    #[test]
    fn test_exec_report_creation() {
        let report = ExecReport {
            deal_id: 1,
            delivered_kwh: 48.5,
            deviation: -1.5,
        };
        assert_eq!(report.deal_id, 1);
        assert_eq!(report.delivered_kwh, 48.5);
        assert_eq!(report.deviation, -1.5);
    }
}
