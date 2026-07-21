//! v0.100.0 资源争抢竞价：统一价格撮合纯函数。
//!
//! 蓝图 §4.5：联邦级资源定价采用统一价格（uniform price）撮合，出清价为
//! 末笔成交的成交单价。所有撮合逻辑封装为纯函数（D6：不修改入参），
//! 便于测试与共识层复用。
//!
//! ## 撮合规则（蓝图 §4.5 + D13 偏差）
//!
//! - 买单调降序（高价优先），卖单调升序（低价优先）；同价按 `agent` 升序
//!   保证跨节点可复现（C41）。
//! - 买价 ≥ 卖价时成交，成交单价 = `(bid_price + ask_price) / 2` 向下取整
//!   （D13：u64 除法天然向下取整）。
//! - 若成交单价 < `safety_floor`（安全底价，蓝图 §8.5），终止撮合。
//! - 出清价 `clearing_price` = 末笔成交价；无成交时为 0。
//! - 快照语义：对 `book` 的 clone 副本排序并撮合，原簿不变（D12）。

use alloc::vec::Vec;

use crate::bid_book::{Match, MatchResult, OrderBook, Price};

/// 统一价格撮合（蓝图 §4.5）：bids 价格降序 × asks 价格升序双指针；
/// 价格交叉成交，价=(bid+ask)/2 向下取整；`price < safety_floor` 停止；
/// 快照语义——不修改入参簿（D12）。
pub fn match_book(book: &OrderBook, safety_floor: Price) -> MatchResult {
    let mut bids = book.bids.clone();
    let mut asks = book.asks.clone();

    // 买单调降序，同价按 agent 升序（跨节点可复现，C41）
    bids.sort_by(|a, b| b.price.cmp(&a.price).then(a.agent.cmp(&b.agent)));
    // 卖单调升序，同价按 agent 升序
    asks.sort_by(|a, b| a.price.cmp(&b.price).then(a.agent.cmp(&b.agent)));

    let mut matches = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;

    while i < bids.len() && j < asks.len() {
        if bids[i].price < asks[j].price {
            break;
        }
        let price = (bids[i].price + asks[j].price) / 2;
        if price < safety_floor {
            break;
        }
        let qty = core::cmp::min(bids[i].qty, asks[j].qty);
        // 数量必大于 0（已校验）
        debug_assert!(qty > 0);
        matches.push(Match {
            buyer: bids[i].agent,
            seller: asks[j].agent,
            price,
            qty,
        });
        bids[i].qty -= qty;
        asks[j].qty -= qty;
        if bids[i].qty == 0 {
            i += 1;
        }
        if asks[j].qty == 0 {
            j += 1;
        }
    }

    let clearing_price = matches.last().map(|m| m.price).unwrap_or(0);
    MatchResult {
        matches,
        clearing_price,
    }
}

// ============================================================
// Unit Tests TM11~TM22
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bid_book::{AgentId, AskOrder, BidOrder, OrderBook};

    // TM11: 空簿 → 空 matches + clearing_price==0
    #[test]
    fn tm11_empty_book() {
        let book = OrderBook::new();
        let r = match_book(&book, 0);
        assert!(r.matches.is_empty());
        assert_eq!(r.clearing_price, 0);
    }

    // TM12: 单笔交叉成交 bid(1,100,10)/ask(2,90,8)
    #[test]
    fn tm12_single_match() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].buyer, 1);
        assert_eq!(r.matches[0].seller, 2);
        assert_eq!(r.matches[0].price, 95); // (100+90)/2 = 95
        assert_eq!(r.matches[0].qty, 8);
        assert_eq!(r.clearing_price, 95);
    }

    // TM13: 无交叉 bid 80 < ask 90 → 空
    #[test]
    fn tm13_no_cross() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 80,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert!(r.matches.is_empty());
        assert_eq!(r.clearing_price, 0);
    }

    // TM14: 同价 bid 90 / ask 90 → 成交 price=90
    #[test]
    fn tm14_equal_price() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 90,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].price, 90);
        assert_eq!(r.matches[0].qty, 8);
    }

    // TM15: bid(1,100,10) / asks[(2,90,6),(3,91,3)] → 2 笔（6+3），bid 余 1 未成交
    #[test]
    fn tm15_partial_bid_remainder() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 6,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 3,
            price: 91,
            qty: 3,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert_eq!(r.matches.len(), 2);
        //  ask 升序：(2,90,6) 在前，(3,91,3) 在后
        assert_eq!(r.matches[0].seller, 2);
        assert_eq!(r.matches[0].qty, 6); // (100+90)/2=95
        assert_eq!(r.matches[1].seller, 3);
        assert_eq!(r.matches[1].qty, 3); // (100+91)/2=95
                                         // bid 1 余 1 未成交
        assert_eq!(r.clearing_price, 95);
    }

    // TM16: bids[(1,100,4),(2,99,2)] / ask(3,90,10) → 2 笔（4+2），ask 余 4
    #[test]
    fn tm16_partial_ask_remainder() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 4,
        })
        .unwrap();
        book.submit_bid(BidOrder {
            agent: 2,
            price: 99,
            qty: 2,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 3,
            price: 90,
            qty: 10,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert_eq!(r.matches.len(), 2);
        //  bid 降序：1 在前，2 在后
        assert_eq!(r.matches[0].buyer, 1);
        assert_eq!(r.matches[0].qty, 4); // (100+90)/2=95
        assert_eq!(r.matches[1].buyer, 2);
        assert_eq!(r.matches[1].qty, 2); // (99+90)/2=94
                                         // ask 3 余 4
        assert_eq!(r.clearing_price, 94);
    }

    // TM17: 多对撮合，clearing_price == 末笔 price
    #[test]
    fn tm17_clearing_is_last_match() {
        let mut book = OrderBook::new();
        // 贪心双指针：最高买价 × 最低卖价优先
        // bid(1,200,10) × ask(4,90,4) → 145；bid(1,200,6) × ask(2,180,6) → 190
        book.submit_bid(BidOrder {
            agent: 1,
            price: 200,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 180,
            qty: 6,
        })
        .unwrap();
        book.submit_bid(BidOrder {
            agent: 3,
            price: 100,
            qty: 2,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 4,
            price: 90,
            qty: 4,
        })
        .unwrap();
        let r = match_book(&book, 0);
        assert_eq!(r.matches.len(), 2);
        assert_eq!(r.matches[0].price, 145); // (200+90)/2
        assert_eq!(r.matches[1].price, 190); // (200+180)/2
        assert_eq!(r.matches.last().unwrap().price, r.clearing_price);
        assert_eq!(r.clearing_price, 190);
    }

    // TM18: safety_floor=96 > 全部可成交 price（如 95）→ 空 + clearing 0
    #[test]
    fn tm18_floor_rejects_all() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .unwrap();
        let r = match_book(&book, 96);
        assert!(r.matches.is_empty());
        assert_eq!(r.clearing_price, 0);
    }

    // TM19: price == safety_floor 边界含等，应成交
    #[test]
    fn tm19_floor_boundary_equal() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .unwrap();
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .unwrap();
        let r = match_book(&book, 95); // price == 95
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].price, 95);
    }

    // TM20: 成交总 qty ≤ min(bids 总, asks 总)，且逐笔 qty > 0
    #[test]
    fn tm20_qty_conservation() {
        let mut book = OrderBook::new();
        for i in 1..=5 {
            book.submit_bid(BidOrder {
                agent: i,
                price: 100 + i,
                qty: 10,
            })
            .unwrap();
            book.submit_ask(AskOrder {
                agent: 10 + i,
                price: 90 - i,
                qty: 7,
            })
            .unwrap();
        }
        let r = match_book(&book, 0);
        let total_matched_qty: u64 = r.matches.iter().map(|m| m.qty).sum();
        let total_bids_qty: u64 = book.bids.iter().map(|b| b.qty).sum();
        let total_asks_qty: u64 = book.asks.iter().map(|a| a.qty).sum();
        assert!(total_matched_qty <= total_bids_qty);
        assert!(total_matched_qty <= total_asks_qty);
        for m in &r.matches {
            assert!(m.qty > 0);
        }
    }

    // TM21: 大簿冒烟（5000 bids + 5000 asks）→ 不 panic、非空、逐笔 price ≥ floor
    #[test]
    fn tm21_large_book_smoke() {
        let mut book = OrderBook::new();
        for i in 0..5000 {
            book.submit_bid(BidOrder {
                agent: 1 + i as u64,
                price: 1000 + (i % 50) as u64,
                qty: 10,
            })
            .unwrap();
            book.submit_ask(AskOrder {
                agent: 10_000 + i as u64,
                price: 900 + (i % 50) as u64,
                qty: 10,
            })
            .unwrap();
        }
        let floor = 0u64;
        let r = match_book(&book, floor);
        assert!(!r.matches.is_empty(), "大簿应至少成交部分");
        for m in &r.matches {
            assert!(m.price >= floor, "逐笔成交价应不低于安全底价");
        }
    }

    // TM22: 同价 bid（agent 3,1,2 乱序入簿）+ 足量 ask → 成交 buyer 顺序为 1,2,3
    #[test]
    fn tm22_same_price_stable_order() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 3,
            price: 100,
            qty: 2,
        })
        .unwrap();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 2,
        })
        .unwrap();
        book.submit_bid(BidOrder {
            agent: 2,
            price: 100,
            qty: 2,
        })
        .unwrap();
        // 足量 ask，可覆盖全部 3 笔 bid（共 6 qty）
        book.submit_ask(AskOrder {
            agent: 10,
            price: 90,
            qty: 100,
        })
        .unwrap();
        let r = match_book(&book, 0);
        // 3 笔 buyer 按 agent 升序 1,2,3
        let buyers: Vec<AgentId> = r.matches.iter().map(|m| m.buyer).collect();
        assert_eq!(buyers, alloc::vec![1, 2, 3]);
    }
}
