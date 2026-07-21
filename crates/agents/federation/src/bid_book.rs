//! v0.100.0 资源争抢竞价：定点订单簿类型。
//!
//! 蓝图 phase2.md §v0.100.0：联邦内多 Agent 对可调资源（储能充放、可中断负荷等）
//! 竞价争抢，本模块提供无堆化的订单簿数据结构与确定性撮合结果序列化。
//!
//! ## 设计要点
//!
//! - **定点数替代浮点**（D1）：价格 `Price` 单位毫元（1e-3 元），数量 `Qty` 单位
//!   Wh（1e-3 kWh），全部 `u64`。嵌入式 no_std 环境禁用 `f32/f64`（蓝图 f32 原文
//!   的定点化偏差），杜绝跨节点浮点舍入不一致。
//! - **无堆标识**（D2）：`AgentId = u64`，沿用 v0.97.0 `NodeId` 惯例，避免堆分配
//!   字符串。
//! - **确定性序列化**（D10）：[`MatchResult::to_bytes`] 输出大端定长字节流，
//!   [`match_digest`] 对其取 SM3 摘要，供 v0.99.0 联邦共识 `ConsensusEngine::submit`
//!   确认撮合结果——各节点独立撮合后 digest 一致才可共识。

use alloc::vec::Vec;

use eneros_crypto::sm3_hash;

/// Agent 标识（D2：无堆 u64，v0.97.0 NodeId 惯例）
pub type AgentId = u64;
/// 价格（毫元，1e-3 元；D1 定点替代蓝图 f32）
pub type Price = u64;
/// 数量（Wh，1e-3 kWh；D1 定点）
pub type Qty = u64;

/// 买单（竞价求购资源）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BidOrder {
    /// 出价 Agent
    pub agent: AgentId,
    /// 单位价格（毫元/Wh）
    pub price: Price,
    /// 求购数量（Wh）
    pub qty: Qty,
}

/// 卖单（出让资源）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskOrder {
    /// 出让 Agent
    pub agent: AgentId,
    /// 单位价格（毫元/Wh）
    pub price: Price,
    /// 出让数量（Wh）
    pub qty: Qty,
}

/// 一笔成交记录
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// 买方 Agent
    pub buyer: AgentId,
    /// 卖方 Agent
    pub seller: AgentId,
    /// 成交单价（统一出清价口径，毫元/Wh）
    pub price: Price,
    /// 成交数量（Wh）
    pub qty: Qty,
}

/// 撮合结果（一轮统一价格出清）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    /// 全部成交记录（按撮合顺序）
    pub matches: Vec<Match>,
    /// 出清价（末笔成交价；无成交为 0）
    pub clearing_price: Price,
}

/// 竞价错误（最小完备）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionError {
    /// price==0 或 qty==0 非法订单
    InvalidOrder,
    /// 超出 max_price 限价（D8，蓝图 §8.5）
    PriceCapExceeded,
}

/// 订单簿（买/卖双侧，快照语义由撮合层保证）
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// 买单列表（入簿顺序）
    pub bids: Vec<BidOrder>,
    /// 卖单列表（入簿顺序）
    pub asks: Vec<AskOrder>,
}

impl OrderBook {
    /// 创建空订单簿
    pub fn new() -> Self {
        Self::default()
    }

    /// 双向均空时为 true
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// 订单总数（bids + asks）
    pub fn len(&self) -> usize {
        self.bids.len() + self.asks.len()
    }

    /// 提交买单：price==0 或 qty==0 → `Err(InvalidOrder)` 且不入簿
    pub fn submit_bid(&mut self, b: BidOrder) -> Result<(), AuctionError> {
        if b.price == 0 || b.qty == 0 {
            return Err(AuctionError::InvalidOrder);
        }
        self.bids.push(b);
        Ok(())
    }

    /// 提交卖单：price==0 或 qty==0 → `Err(InvalidOrder)` 且不入簿
    pub fn submit_ask(&mut self, a: AskOrder) -> Result<(), AuctionError> {
        if a.price == 0 || a.qty == 0 {
            return Err(AuctionError::InvalidOrder);
        }
        self.asks.push(a);
        Ok(())
    }

    /// 清空双侧订单（新一轮竞价）
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
    }
}

impl MatchResult {
    /// 确定性序列化（D10）：`clearing_price:u64be ‖ count:u64be ‖
    /// 每 match buyer‖seller‖price‖qty 各 u64be`。
    ///
    /// 长度 = 16 + 32 × matches.len()。全部大端定长编码，跨节点逐字节一致。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + 32 * self.matches.len());
        out.extend_from_slice(&self.clearing_price.to_be_bytes());
        out.extend_from_slice(&(self.matches.len() as u64).to_be_bytes());
        for m in &self.matches {
            out.extend_from_slice(&m.buyer.to_be_bytes());
            out.extend_from_slice(&m.seller.to_be_bytes());
            out.extend_from_slice(&m.price.to_be_bytes());
            out.extend_from_slice(&m.qty.to_be_bytes());
        }
        out
    }
}

/// 撮合结果 SM3 摘要（D10：供 v0.99.0 共识确认）
pub fn match_digest(result: &MatchResult) -> [u8; 32] {
    sm3_hash(&result.to_bytes())
}

// ============================================================
// Unit Tests TB1~TB10
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // TB1: 类型派生语义（Copy / 相等比较 / 错误变体不等）
    #[test]
    fn tb1_type_derives() {
        let b = BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        };
        let b2 = b; // Copy
        assert_eq!(b, b2);
        assert_eq!(b.clone(), b);
        let a = AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        };
        let a2 = a; // Copy
        assert_eq!(a, a2);
        let m = Match {
            buyer: 1,
            seller: 2,
            price: 95,
            qty: 8,
        };
        let m2 = m; // Copy
        assert_eq!(m, m2);
        assert_ne!(
            m,
            Match {
                buyer: 1,
                seller: 2,
                price: 95,
                qty: 9
            }
        );
        // 错误变体互不等
        assert_ne!(AuctionError::InvalidOrder, AuctionError::PriceCapExceeded);
        let _dbg = alloc::format!("{:?}", AuctionError::InvalidOrder);
    }

    // TB2: 合法 bid/ask 入簿，len/is_empty 正确
    #[test]
    fn tb2_submit_ok() {
        let mut book = OrderBook::new();
        assert!(book.is_empty());
        assert_eq!(book.len(), 0);
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .expect("bid ok");
        assert!(!book.is_empty());
        assert_eq!(book.len(), 1);
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .expect("ask ok");
        assert_eq!(book.len(), 2);
        assert_eq!(book.bids[0].agent, 1);
        assert_eq!(book.asks[0].agent, 2);
    }

    // TB3: price=0 买单被拒，簿仍空
    #[test]
    fn tb3_submit_zero_price_rejected() {
        let mut book = OrderBook::new();
        assert_eq!(
            book.submit_bid(BidOrder {
                agent: 1,
                price: 0,
                qty: 10,
            }),
            Err(AuctionError::InvalidOrder)
        );
        assert_eq!(
            book.submit_ask(AskOrder {
                agent: 2,
                price: 0,
                qty: 8,
            }),
            Err(AuctionError::InvalidOrder)
        );
        assert!(book.is_empty());
    }

    // TB4: qty=0 订单被拒，簿仍空
    #[test]
    fn tb4_submit_zero_qty_rejected() {
        let mut book = OrderBook::new();
        assert_eq!(
            book.submit_bid(BidOrder {
                agent: 1,
                price: 100,
                qty: 0,
            }),
            Err(AuctionError::InvalidOrder)
        );
        assert_eq!(
            book.submit_ask(AskOrder {
                agent: 2,
                price: 90,
                qty: 0,
            }),
            Err(AuctionError::InvalidOrder)
        );
        assert!(book.is_empty());
    }

    // TB5: 入簿后 clear → is_empty
    #[test]
    fn tb5_clear() {
        let mut book = OrderBook::new();
        book.submit_bid(BidOrder {
            agent: 1,
            price: 100,
            qty: 10,
        })
        .expect("bid");
        book.submit_ask(AskOrder {
            agent: 2,
            price: 90,
            qty: 8,
        })
        .expect("ask");
        assert_eq!(book.len(), 2);
        book.clear();
        assert!(book.is_empty());
        assert_eq!(book.len(), 0);
    }

    // TB6: to_bytes 字节布局（1 笔 match：长度 48，各字段大端位置）
    #[test]
    fn tb6_to_bytes_layout() {
        let r = MatchResult {
            matches: alloc::vec![Match {
                buyer: 0x0102,
                seller: 0x0304,
                price: 0x0506,
                qty: 0x0708,
            }],
            clearing_price: 0x0506,
        };
        let bytes = r.to_bytes();
        assert_eq!(bytes.len(), 48);
        // clearing_price @ [0..8]
        assert_eq!(&bytes[0..8], &0x0506u64.to_be_bytes());
        // count @ [8..16]
        assert_eq!(&bytes[8..16], &1u64.to_be_bytes());
        // buyer @ [16..24]
        assert_eq!(&bytes[16..24], &0x0102u64.to_be_bytes());
        // seller @ [24..32]
        assert_eq!(&bytes[24..32], &0x0304u64.to_be_bytes());
        // price @ [32..40]
        assert_eq!(&bytes[32..40], &0x0506u64.to_be_bytes());
        // qty @ [40..48]
        assert_eq!(&bytes[40..48], &0x0708u64.to_be_bytes());
    }

    // TB7: 同一结果两次 to_bytes 相等（确定性）
    #[test]
    fn tb7_to_bytes_deterministic() {
        let r = MatchResult {
            matches: alloc::vec![
                Match {
                    buyer: 1,
                    seller: 2,
                    price: 95,
                    qty: 8,
                },
                Match {
                    buyer: 3,
                    seller: 4,
                    price: 96,
                    qty: 5,
                },
            ],
            clearing_price: 96,
        };
        assert_eq!(r.to_bytes(), r.to_bytes());
    }

    // TB8: 空 matches → 长度 16、count=0
    #[test]
    fn tb8_to_bytes_empty() {
        let r = MatchResult {
            matches: Vec::new(),
            clearing_price: 0,
        };
        let bytes = r.to_bytes();
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[0..8], &0u64.to_be_bytes());
        assert_eq!(&bytes[8..16], &0u64.to_be_bytes());
    }

    // TB9: 同一结果两次 match_digest 相等
    #[test]
    fn tb9_digest_deterministic() {
        let r = MatchResult {
            matches: alloc::vec![Match {
                buyer: 1,
                seller: 2,
                price: 95,
                qty: 8,
            }],
            clearing_price: 95,
        };
        assert_eq!(match_digest(&r), match_digest(&r));
    }

    // TB10: 内容敏感——qty 改 1 → digest 不同
    #[test]
    fn tb10_digest_content_sensitive() {
        let r1 = MatchResult {
            matches: alloc::vec![Match {
                buyer: 1,
                seller: 2,
                price: 95,
                qty: 8,
            }],
            clearing_price: 95,
        };
        let r2 = MatchResult {
            matches: alloc::vec![Match {
                buyer: 1,
                seller: 2,
                price: 95,
                qty: 9, // 仅 qty 差 1
            }],
            clearing_price: 95,
        };
        assert_ne!(match_digest(&r1), match_digest(&r2));
    }
}
