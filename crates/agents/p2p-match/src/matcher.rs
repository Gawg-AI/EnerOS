//! 价格时间优先撮合引擎

use alloc::vec::Vec;

use crate::types::{Deal, DealState, P2pAsk, P2pBid};

/// 撮合策略 trait
pub trait MatchStrategy {
    /// 从 bids/asks 中撮合出一笔成交
    fn match_one(&self, bids: &[P2pBid], asks: &[P2pAsk]) -> Option<Deal>;
}

/// 价格时间优先撮合策略
///
/// - 卖方 (bids): 按价格升序 (最低价优先)，同价按 seq 升序 (时间优先)
/// - 买方 (asks): 按 max_price 降序 (最高出价优先)，同价按 seq 升序
/// - 当 bid.price <= ask.max_price 时撮合成功，成交价 = bid.price
#[derive(Debug, Clone, Copy, Default)]
pub struct PriceTimePriority;

impl MatchStrategy for PriceTimePriority {
    fn match_one(&self, bids: &[P2pBid], asks: &[P2pAsk]) -> Option<Deal> {
        if bids.is_empty() || asks.is_empty() {
            return None;
        }

        // 排序卖方: 价格升序, seq 升序
        let mut sorted_bids: Vec<&P2pBid> = bids.iter().collect();
        sorted_bids.sort_by(|a, b| {
            a.price
                .partial_cmp(&b.price)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then(a.seq.cmp(&b.seq))
        });

        // 排序买方: max_price 降序, seq 升序
        let mut sorted_asks: Vec<&P2pAsk> = asks.iter().collect();
        sorted_asks.sort_by(|a, b| {
            b.max_price
                .partial_cmp(&a.max_price)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then(a.seq.cmp(&b.seq))
        });

        // 寻找第一对满足条件的 bid/ask
        for bid in &sorted_bids {
            for ask in &sorted_asks {
                if bid.price <= ask.max_price {
                    let volume = bid.volume_kwh.min(ask.volume_kwh);
                    return Some(Deal {
                        deal_id: 0, // 由 MatchEngine 分配
                        seller: bid.agent,
                        buyer: ask.agent,
                        seller_seq: bid.seq,
                        buyer_seq: ask.seq,
                        volume_kwh: volume,
                        price: bid.price,
                        window_start: 0,
                        window_end: 0,
                        state: DealState::Matched,
                    });
                }
            }
        }

        None
    }
}

/// 撮合引擎
#[derive(Debug, Clone, Default)]
pub struct MatchEngine {
    /// 卖方报价队列
    pub bids: Vec<P2pBid>,
    /// 买方询价队列
    pub asks: Vec<P2pAsk>,
    /// 下一个成交 ID
    pub next_deal_id: u64,
}

impl MatchEngine {
    /// 创建新的撮合引擎
    pub fn new() -> Self {
        Self {
            bids: Vec::new(),
            asks: Vec::new(),
            next_deal_id: 1,
        }
    }

    /// 添加卖方报价
    pub fn add_bid(&mut self, bid: P2pBid) {
        self.bids.push(bid);
    }

    /// 添加买方询价
    pub fn add_ask(&mut self, ask: P2pAsk) {
        self.asks.push(ask);
    }

    /// 执行撮合，返回一笔成交 (如有)。
    ///
    /// v0.211.0（P0-25）：
    /// 1. 过期订单（`valid_until < now`）从订单簿移除，不参与撮合。
    /// 2. 成交后从订单簿移除已成交的 bid/ask（防止同一订单重复成交）。
    pub fn run_match(&mut self, now: u64) -> Option<Deal> {
        // 1. 移除过期订单（fail-closed：过期订单不得成交）。
        self.bids.retain(|b| b.valid_until >= now);
        self.asks.retain(|a| a.valid_until >= now);

        // 2. 撮合。
        let strategy = PriceTimePriority;
        let deal = strategy.match_one(&self.bids, &self.asks)?;

        // 3. 移除已成交订单（按 agent+seq 定位，防重复成交）。
        self.bids
            .retain(|b| !(b.agent == deal.seller && b.seq == deal.seller_seq));
        self.asks
            .retain(|a| !(a.agent == deal.buyer && a.seq == deal.buyer_seq));

        let mut deal = deal;
        deal.deal_id = self.next_deal_id;
        self.next_deal_id += 1;
        Some(deal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bid(agent: u64, volume: f64, price: f64, seq: u64) -> P2pBid {
        P2pBid {
            agent,
            volume_kwh: volume,
            price,
            valid_until: 99999,
            seq,
        }
    }

    fn make_ask(agent: u64, volume: f64, max_price: f64, seq: u64) -> P2pAsk {
        P2pAsk {
            agent,
            volume_kwh: volume,
            max_price,
            valid_until: 99999,
            seq,
        }
    }

    #[test]
    fn test_successful_match() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 100.0, 0.60, 1));
        engine.add_ask(make_ask(2, 80.0, 0.70, 1));

        let deal = engine.run_match(1000).expect("should match");
        assert_eq!(deal.seller, 1);
        assert_eq!(deal.buyer, 2);
        assert_eq!(deal.price, 0.60);
        assert_eq!(deal.volume_kwh, 80.0); // min(100, 80)
        assert_eq!(deal.deal_id, 1);
        assert_eq!(deal.state, DealState::Matched);
    }

    #[test]
    fn test_no_match_price_gap() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 100.0, 0.80, 1)); // 卖方要价 0.80
        engine.add_ask(make_ask(2, 80.0, 0.70, 1)); // 买方最高 0.70

        let deal = engine.run_match(1000);
        assert!(deal.is_none());
    }

    #[test]
    fn test_time_priority() {
        let mut engine = MatchEngine::new();
        // 两个卖方同价，seq 小的优先
        engine.add_bid(make_bid(1, 50.0, 0.60, 2));
        engine.add_bid(make_bid(3, 50.0, 0.60, 1)); // seq=1 更早
        engine.add_ask(make_ask(2, 30.0, 0.70, 1));

        let deal = engine.run_match(1000).expect("should match");
        assert_eq!(deal.seller, 3); // seq=1 优先
    }

    #[test]
    fn test_volume_min() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 200.0, 0.50, 1));
        engine.add_ask(make_ask(2, 150.0, 0.60, 1));

        let deal = engine.run_match(1000).expect("should match");
        assert_eq!(deal.volume_kwh, 150.0); // min(200, 150)
    }

    #[test]
    fn test_price_priority_cheapest_first() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 50.0, 0.70, 1)); // 较贵
        engine.add_bid(make_bid(2, 50.0, 0.50, 2)); // 较便宜
        engine.add_ask(make_ask(3, 30.0, 0.80, 1));

        let deal = engine.run_match(1000).expect("should match");
        assert_eq!(deal.seller, 2); // 价格低的优先
        assert_eq!(deal.price, 0.50);
    }

    #[test]
    fn test_buyer_highest_price_first() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 50.0, 0.50, 1));
        engine.add_ask(make_ask(2, 30.0, 0.60, 1)); // 出价较低
        engine.add_ask(make_ask(3, 30.0, 0.90, 2)); // 出价较高

        let deal = engine.run_match(1000).expect("should match");
        assert_eq!(deal.buyer, 3); // 出价高的优先
    }

    #[test]
    fn test_empty_orders() {
        let mut engine = MatchEngine::new();
        assert!(engine.run_match(1000).is_none());

        engine.add_bid(make_bid(1, 50.0, 0.50, 1));
        assert!(engine.run_match(1000).is_none()); // 无买方
    }

    #[test]
    fn test_deal_id_increments() {
        let mut engine = MatchEngine::new();
        engine.add_bid(make_bid(1, 100.0, 0.50, 1));
        engine.add_ask(make_ask(2, 50.0, 0.60, 1));

        let deal1 = engine.run_match(1000).expect("should match");
        assert_eq!(deal1.deal_id, 1);

        // v0.211.0（P0-25）：成交后订单已从订单簿移除——再次撮合无单可成
        // （防回归：旧 bug 是同一订单重复成交出 deal_id=2）。
        assert!(
            engine.run_match(1000).is_none(),
            "成交后订单必须从订单簿移除，不能重复成交（P0-25）"
        );
    }

    #[test]
    fn test_expired_orders_removed() {
        let mut engine = MatchEngine::new();
        // 过期订单（valid_until=99999，now=1000000 → 已过期）。
        engine.add_bid(make_bid(1, 100.0, 0.50, 1));
        engine.add_ask(make_ask(2, 50.0, 0.60, 1));
        assert!(
            engine.run_match(1_000_000).is_none(),
            "过期订单不得成交（P0-25 valid_until 校验）"
        );
        // 订单簿已清空过期单。
        assert!(engine.bids.is_empty());
        assert!(engine.asks.is_empty());
    }
}
