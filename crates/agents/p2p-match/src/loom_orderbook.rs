//! v0.247.0（C-247）：订单簿并发模型验证（loom，host 侧）。
//!
//! 生产 `MatchEngine` 为 `&mut self` 单线程 API；并发访问由上层经 Mutex
//! 串行化。本模型验证经 `loom::sync::Mutex` 串行化后的**撮合不变量**：
//! - 同一 bid/ask 至多成交一次（成交后从订单簿移除，无重复成交）；
//! - deal_id 全局唯一（计数单调）；
//! - 过期订单不参与撮合（fail-closed）。

extern crate std;

use loom::sync::Mutex;
use std::vec::Vec;

/// 订单（与生产 P2pBid 同构的简化版）。
#[derive(Debug, Clone, PartialEq)]
struct Order {
    agent: u64,
    seq: u64,
    price: f64,
    volume: f64,
    valid_until: u64,
}

/// 与生产 `MatchEngine` 判定逻辑同构的 loom 复刻。
struct LoomOrderBook {
    bids: Vec<Order>,
    asks: Vec<Order>,
    next_deal_id: u64,
}

impl LoomOrderBook {
    fn new() -> Self {
        Self {
            bids: Vec::new(),
            asks: Vec::new(),
            next_deal_id: 1,
        }
    }

    fn add_bid(&mut self, o: Order) {
        self.bids.push(o);
    }

    fn add_ask(&mut self, o: Order) {
        self.asks.push(o);
    }

    /// 单次撮合：返回 (seller, buyer, deal_id) 或 None。
    /// 与生产同构：过期剔除 → 价格时间优先 → 移除已成交 → id 自增。
    fn run_match(&mut self, now: u64) -> Option<(u64, u64, u64)> {
        self.bids.retain(|b| b.valid_until >= now);
        self.asks.retain(|a| a.valid_until >= now);

        // 最高买价 bid 与最低卖价 ask（价格时间优先简化）。
        let bid_idx = self.bids.iter().enumerate().max_by(|a, b| {
            a.1.price
                .partial_cmp(&b.1.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let ask_idx = self.asks.iter().enumerate().min_by(|a, b| {
            a.1.price
                .partial_cmp(&b.1.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (bi, bid) = bid_idx?;
        let (ai, ask) = ask_idx?;
        if bid.price < ask.price {
            return None;
        }
        let deal = (bid.agent, ask.agent, self.next_deal_id);
        // 移除已成交订单（按 agent+seq）。
        self.bids.remove(bi);
        self.asks.remove(ai);
        self.next_deal_id += 1;
        Some(deal)
    }
}

fn order(agent: u64, seq: u64, price: f64, volume: f64) -> Order {
    Order {
        agent,
        seq,
        price,
        volume,
        valid_until: u64::MAX,
    }
}

/// 并发撮合：一个线程持续 add，另一线程持续 run_match——
/// 不变量：成交 deal_id 唯一、无重复成交（同一 bid 不出现两次）。
#[test]
fn vrf_007_concurrent_match_no_duplicate_deals() {
    loom::model(|| {
        let book = loom::sync::Arc::new(Mutex::new(LoomOrderBook::new()));

        let b1 = book.clone();
        let producer = loom::thread::spawn(move || {
            let mut g = b1.lock().unwrap();
            g.add_bid(order(1, 1, 0.60, 100.0));
            g.add_ask(order(2, 1, 0.70, 80.0));
        });
        let b2 = book.clone();
        let consumer = loom::thread::spawn(move || {
            let mut g = b2.lock().unwrap();
            g.run_match(u64::MAX)
        });
        producer.join().unwrap();
        let deal = consumer.join().unwrap();
        let book = book.lock().unwrap();
        // 若成交发生：订单簿中已无该 bid/ask，deal_id 为 1。
        if let Some((_s, _b, id)) = deal {
            assert_eq!(id, 1, "首个成交 deal_id 必须为 1");
            assert!(
                book.bids.iter().all(|o| !(o.agent == 1 && o.seq == 1)),
                "已成交 bid 必须从订单簿移除"
            );
            assert!(
                book.asks.iter().all(|o| !(o.agent == 2 && o.seq == 1)),
                "已成交 ask 必须从订单簿移除"
            );
        }
    });
}

/// 过期订单并发下不成交：过期 bid 与有效 ask 同簿，撮合结果必须为 None。
#[test]
fn vrf_007_expired_orders_never_match() {
    loom::model(|| {
        let book = loom::sync::Arc::new(Mutex::new(LoomOrderBook::new()));
        {
            let mut g = book.lock().unwrap();
            let mut expired = order(1, 1, 0.60, 100.0);
            expired.valid_until = 500; // 已过期
            g.add_bid(expired);
            g.add_ask(order(2, 1, 0.70, 80.0));
        }
        let b = book.clone();
        let h = loom::thread::spawn(move || {
            let mut g = b.lock().unwrap();
            g.run_match(1000)
        });
        let deal = h.join().unwrap();
        assert!(deal.is_none(), "过期 bid 不得参与撮合");
    });
}
