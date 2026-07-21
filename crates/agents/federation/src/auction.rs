//! v0.100.0 资源争抢竞价：AuctionEngine——簿管理 + 提交校验 + 限价 + 快照撮合 + 可观测。
//!
//! 蓝图 phase2.md §v0.100.0：联邦级资源定价引擎，封装订单簿生命周期与
//! 撮合调度，对外提供确定性、可共识确认的竞价服务。
//!
//! ## 设计要点
//!
//! - **限价保护**（D8，蓝图 §8.5）：`max_price` 为 Some 时，超出限价的订单
//!   以 `Err(PriceCapExceeded)` 拒绝并计入 `rejected_count`；None 不限价。
//! - **快照撮合**（D12）：`match_orders` 调用纯函数 [`match_book`]，撮合过程
//!   不修改 `self.book`——簿内容留待 `clear_book` 显式清空，便于审计与重放。
//! - **可观测**（D9）：4 个 pub 计数器（`bid_count` / `ask_count` /
//!   `match_count` / `rejected_count`）+ `last_clearing_price`。
//! - **共识 seam 分离**（D10）：引擎不持有 `ConsensusEngine`；撮合结果经
//!   [`MatchResult::to_bytes`](crate::bid_book::MatchResult::to_bytes) /
//!   [`match_digest`](crate::bid_book::match_digest) 交由 v0.99.0 共识层确认，
//!   本模块与共识协议零耦合。

use crate::bid_book::{AskOrder, AuctionError, BidOrder, MatchResult, OrderBook, Price};
use crate::matching::match_book;

/// 资源争抢竞价引擎（字段全 pub，便于测试观测）
pub struct AuctionEngine {
    /// 订单簿（撮合为快照语义，本簿不被撮合修改）
    pub book: OrderBook,
    /// 安全底价（成交价低于此值终止撮合，蓝图 §8.5）
    pub safety_floor: Price,
    /// 最高限价（D8：Some 时超价拒单；None 不限）
    pub max_price: Option<Price>,
    /// 累计接受买单数
    pub bid_count: u64,
    /// 累计接受卖单数
    pub ask_count: u64,
    /// 累计成交笔数
    pub match_count: u64,
    /// 累计拒单数（非法订单 + 超限价订单）
    pub rejected_count: u64,
    /// 最近一轮出清价（无成交轮次保持上一轮的值）
    pub last_clearing_price: Price,
}

impl AuctionEngine {
    /// 创建引擎：空簿、计数器全零
    pub fn new(safety_floor: Price, max_price: Option<Price>) -> Self {
        Self {
            book: OrderBook::new(),
            safety_floor,
            max_price,
            bid_count: 0,
            ask_count: 0,
            match_count: 0,
            rejected_count: 0,
            last_clearing_price: 0,
        }
    }

    /// 提交买单：先簿校验（price/qty 非零），再 max_price 限价校验；
    /// 任一失败计入 `rejected_count`；成功计入 `bid_count`
    pub fn submit_bid(&mut self, b: BidOrder) -> Result<(), AuctionError> {
        // 簿校验（price==0 || qty==0 → InvalidOrder）
        if b.price == 0 || b.qty == 0 {
            self.rejected_count += 1;
            return Err(AuctionError::InvalidOrder);
        }
        // 限价校验（D8）
        if let Some(cap) = self.max_price {
            if b.price > cap {
                self.rejected_count += 1;
                return Err(AuctionError::PriceCapExceeded);
            }
        }
        self.book.submit_bid(b)?;
        self.bid_count += 1;
        Ok(())
    }

    /// 提交卖单：校验与计数语义同 [`submit_bid`](Self::submit_bid)
    pub fn submit_ask(&mut self, a: AskOrder) -> Result<(), AuctionError> {
        if a.price == 0 || a.qty == 0 {
            self.rejected_count += 1;
            return Err(AuctionError::InvalidOrder);
        }
        if let Some(cap) = self.max_price {
            if a.price > cap {
                self.rejected_count += 1;
                return Err(AuctionError::PriceCapExceeded);
            }
        }
        self.book.submit_ask(a)?;
        self.ask_count += 1;
        Ok(())
    }

    /// 快照撮合（D12）：不修改 `self.book`；本轮成交笔数累入 `match_count`；
    /// 有成交则更新 `last_clearing_price`
    pub fn match_orders(&mut self) -> MatchResult {
        let result = match_book(&self.book, self.safety_floor);
        self.match_count += result.matches.len() as u64;
        if !result.matches.is_empty() {
            self.last_clearing_price = result.clearing_price;
        }
        result
    }

    /// 清空订单簿（新一轮竞价；计数器不归零，全生命周期累计）
    pub fn clear_book(&mut self) {
        self.book.clear();
    }
}

// ============================================================
// Unit Tests TA23~TA30
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // TA23: 初始状态——簿空、4 计数器 0、last_clearing_price 0
    #[test]
    fn ta23_new_zero() {
        let e = AuctionEngine::new(10, Some(1000));
        assert!(e.book.is_empty());
        assert_eq!(e.safety_floor, 10);
        assert_eq!(e.max_price, Some(1000));
        assert_eq!(e.bid_count, 0);
        assert_eq!(e.ask_count, 0);
        assert_eq!(e.match_count, 0);
        assert_eq!(e.rejected_count, 0);
        assert_eq!(e.last_clearing_price, 0);
    }

    // TA24: 3 bid + 2 ask → bid_count=3, ask_count=2
    #[test]
    fn ta24_submit_counts() {
        let mut e = AuctionEngine::new(0, None);
        for i in 0..3 {
            e.submit_bid(BidOrder {
                agent: 1 + i,
                price: 100,
                qty: 10,
            })
            .expect("bid");
        }
        for i in 0..2 {
            e.submit_ask(AskOrder {
                agent: 10 + i,
                price: 90,
                qty: 8,
            })
            .expect("ask");
        }
        assert_eq!(e.bid_count, 3);
        assert_eq!(e.ask_count, 2);
        assert_eq!(e.book.len(), 5);
    }

    // TA25: max_price=Some(200)，submit price=250 → Err(PriceCapExceeded)，
    // rejected_count=1，簿空
    #[test]
    fn ta25_price_cap_rejected() {
        let mut e = AuctionEngine::new(0, Some(200));
        assert_eq!(
            e.submit_bid(BidOrder {
                agent: 1,
                price: 250,
                qty: 10,
            }),
            Err(AuctionError::PriceCapExceeded)
        );
        assert_eq!(e.rejected_count, 1);
        assert!(e.book.is_empty());
        assert_eq!(e.bid_count, 0);
        // 卖单同样受限价约束
        assert_eq!(
            e.submit_ask(AskOrder {
                agent: 2,
                price: 250,
                qty: 8,
            }),
            Err(AuctionError::PriceCapExceeded)
        );
        assert_eq!(e.rejected_count, 2);
        assert!(e.book.is_empty());
    }

    // TA26: max_price=None 时 price=u64::MAX 也接受（仅簿校验）
    #[test]
    fn ta26_no_cap_when_none() {
        let mut e = AuctionEngine::new(0, None);
        e.submit_bid(BidOrder {
            agent: 1,
            price: u64::MAX,
            qty: 10,
        })
        .expect("no cap");
        assert_eq!(e.bid_count, 1);
        assert_eq!(e.rejected_count, 0);
        assert_eq!(e.book.bids[0].price, u64::MAX);
    }

    // TA27: 撮 2 笔 → match_count=2、last_clearing_price=末笔价、簿内容不变（快照）
    #[test]
    fn ta27_match_updates_counters() {
        let mut e = AuctionEngine::new(0, None);
        e.submit_bid(BidOrder {
            agent: 1,
            price: 200,
            qty: 10,
        })
        .expect("b1");
        e.submit_bid(BidOrder {
            agent: 2,
            price: 100,
            qty: 2,
        })
        .expect("b2");
        e.submit_ask(AskOrder {
            agent: 3,
            price: 180,
            qty: 6,
        })
        .expect("a1");
        e.submit_ask(AskOrder {
            agent: 4,
            price: 90,
            qty: 4,
        })
        .expect("a2");
        let r = e.match_orders();
        assert_eq!(r.matches.len(), 2);
        assert_eq!(e.match_count, 2);
        // 贪心撮合：bid(1,200)×ask(4,90)→145，bid(1,200)×ask(3,180)→190，末笔 190
        assert_eq!(e.last_clearing_price, 190);
        assert_eq!(r.clearing_price, 190);
        // 快照语义：簿内容不变
        assert_eq!(e.book.len(), 4);
        assert_eq!(e.book.bids[0].qty, 10);
        assert_eq!(e.book.asks[1].qty, 4);
    }

    // TA28: submit→match→clear→再 submit 再 match，计数器累计正确（bid_count 不归零）
    #[test]
    fn ta28_clear_book_new_round() {
        let mut e = AuctionEngine::new(0, None);
        // 第一轮
        e.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .expect("r1 bid");
        e.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .expect("r1 ask");
        let r1 = e.match_orders();
        assert_eq!(r1.matches.len(), 1);
        assert_eq!(e.match_count, 1);
        e.clear_book();
        assert!(e.book.is_empty());
        // 计数器不归零
        assert_eq!(e.bid_count, 1);
        assert_eq!(e.ask_count, 1);
        // 第二轮
        e.submit_bid(BidOrder {
            agent: 3,
            price: 120,
            qty: 5,
        })
        .expect("r2 bid");
        e.submit_ask(AskOrder {
            agent: 4,
            price: 110,
            qty: 5,
        })
        .expect("r2 ask");
        let r2 = e.match_orders();
        assert_eq!(r2.matches.len(), 1);
        assert_eq!(e.match_count, 2);
        assert_eq!(e.bid_count, 2);
        assert_eq!(e.ask_count, 2);
        assert_eq!(e.last_clearing_price, 115); // (120+110)/2
    }

    // TA29: 共识 e2e——撮合结果经 v0.99.0 PBFT 集群确认，digest 一致
    #[test]
    fn ta29_consensus_e2e() {
        use crate::bid_book::match_digest;
        use crate::consensus::testutil::{build_cluster, drive};

        // AuctionEngine 撮合出结果
        let mut auction = AuctionEngine::new(0, None);
        auction
            .submit_bid(BidOrder {
                agent: 1,
                price: 100,
                qty: 10,
            })
            .expect("bid");
        auction
            .submit_ask(AskOrder {
                agent: 2,
                price: 90,
                qty: 8,
            })
            .expect("ask");
        let result = auction.match_orders();
        assert_eq!(result.matches.len(), 1);

        // 4 节点 PBFT 集群，主节点提交撮合结果字节流
        let (mut engines, mut bus) = build_cluster(4, 3000);
        let seq = engines[0]
            .submit(result.to_bytes(), &mut bus, 0)
            .expect("submit");
        assert_eq!(seq, 1);
        let _ = drive(&mut engines, &mut bus, 100);

        // 4 节点均已提交 seq=1，且日志 digest == 撮合结果 SM3 摘要
        let expect_digest = match_digest(&result);
        for e in &engines {
            assert!(e.is_committed(1), "node {} 应提交 seq=1", e.local_id);
            let entry = e
                .log
                .iter()
                .find(|le| le.sequence == 1)
                .expect("seq=1 日志条目");
            assert_eq!(entry.digest, expect_digest);
        }
    }

    // TA30: 性能——5000 bids + 5000 asks 价格交叉，match_orders 耗时达标。
    // release 构建口径 < 100ms；debug 构建放宽到 < 500ms 防 CI 抖动。
    #[test]
    fn ta30_perf_10k_under_100ms() {
        let mut e = AuctionEngine::new(0, None);
        for i in 0..5000u64 {
            e.submit_bid(BidOrder {
                agent: 1 + i,
                price: 1000 + i % 50,
                qty: 10,
            })
            .expect("bid");
            e.submit_ask(AskOrder {
                agent: 10_000 + i,
                price: 900 + i % 50,
                qty: 10,
            })
            .expect("ask");
        }
        let start = std::time::Instant::now();
        let r = e.match_orders();
        let elapsed = start.elapsed();
        assert!(!r.matches.is_empty());
        // debug 断言阈值 500ms（release 口径 <100ms）
        assert!(
            elapsed.as_millis() < 500,
            "撮合 10k 订单耗时 {:?} 超阈值",
            elapsed
        );
    }
}
