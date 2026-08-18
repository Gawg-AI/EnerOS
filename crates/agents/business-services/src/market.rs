//! svc-market 领域逻辑：市场接入服务（蓝图 41 WP-41.4）。
//!
//! 链路（蓝图）：`submit` 先 durable outbox（Queued）→ 真实 transport
//! 发送（Sent）→ peer receipt（PeerAcked）→ 业务成功。重启后
//! `recover_pending` 对账重发；duplicate/late ACK/cancel race 显式处理。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_durable_state::{
    DurableOutbox, IdempotencyKey, OutboxId, PeerReceipt, Pending, StoreError,
};
use eneros_market_types::{MarketType, MilliYuan, TimeSlot, YuanPerMwh};

/// 外部报价引用（外部 ID + 幂等 + deadline + scope）。
#[derive(Debug, Clone, PartialEq)]
pub struct QuoteRef {
    pub external_id: u64,
    pub idempotency: IdempotencyKey,
    pub deadline_ms: u64,
    pub market: MarketType,
    pub tenant: u32,
    pub product: u32,
}

/// 报价消息（durable outbox 载荷）。
#[derive(Debug, Clone, PartialEq)]
pub struct QuoteMessage {
    pub quote_ref: QuoteRef,
    pub slot: TimeSlot,
    pub volume_kwh: u64,
    pub price: YuanPerMwh,
    pub amount: MilliYuan,
}

impl eneros_durable_state::Encode for QuoteMessage {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(80);
        out.extend_from_slice(&self.quote_ref.external_id.to_le_bytes());
        out.extend_from_slice(&self.quote_ref.idempotency.0);
        out.extend_from_slice(&self.quote_ref.deadline_ms.to_le_bytes());
        out.push(match self.quote_ref.market {
            MarketType::DayAhead => 1,
            MarketType::IntraDay => 2,
            MarketType::Realtime => 3,
            MarketType::Spot => 4,
            MarketType::AncillaryService => 5,
            MarketType::DemandResponse => 6,
        });
        out.extend_from_slice(&self.quote_ref.tenant.to_le_bytes());
        out.extend_from_slice(&self.quote_ref.product.to_le_bytes());
        out.extend_from_slice(&self.slot.start.to_le_bytes());
        out.extend_from_slice(&self.slot.end.to_le_bytes());
        out.extend_from_slice(&self.volume_kwh.to_le_bytes());
        out.extend_from_slice(&self.price.0.to_le_bytes());
        out.extend_from_slice(&self.amount.0.to_le_bytes());
        Ok(out)
    }
}

impl eneros_durable_state::Decode for QuoteMessage {
    fn decode(bytes: &[u8]) -> Result<(eneros_durable_state::Generation, Self), StoreError> {
        if bytes.len() != 8 + 16 + 8 + 1 + 4 + 4 + 8 + 8 + 8 + 8 + 8 {
            return Err(StoreError::Corrupt);
        }
        let external_id = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let mut key = [0u8; 16];
        key.copy_from_slice(&bytes[8..24]);
        let deadline_ms = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let market = match bytes[32] {
            1 => MarketType::DayAhead,
            2 => MarketType::IntraDay,
            3 => MarketType::Realtime,
            4 => MarketType::Spot,
            5 => MarketType::AncillaryService,
            6 => MarketType::DemandResponse,
            _ => return Err(StoreError::Corrupt),
        };
        let tenant = u32::from_le_bytes(bytes[33..37].try_into().unwrap());
        let product = u32::from_le_bytes(bytes[37..41].try_into().unwrap());
        let start = u64::from_le_bytes(bytes[41..49].try_into().unwrap());
        let end = u64::from_le_bytes(bytes[49..57].try_into().unwrap());
        let volume_kwh = u64::from_le_bytes(bytes[57..65].try_into().unwrap());
        let price = YuanPerMwh(f64::from_le_bytes(bytes[65..73].try_into().unwrap()));
        let amount = MilliYuan(i64::from_le_bytes(bytes[73..81].try_into().unwrap()));
        Ok((
            eneros_durable_state::Generation(0),
            QuoteMessage {
                quote_ref: QuoteRef {
                    external_id,
                    idempotency: IdempotencyKey(key),
                    deadline_ms,
                    market,
                    tenant,
                    product,
                },
                slot: TimeSlot { start, end },
                volume_kwh,
                price,
                amount,
            },
        ))
    }
}

/// 提交状态（分层成功语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitStatus {
    /// 已入 durable outbox，未发送。
    Queued,
    /// 已发送，等 peer ACK。
    Sent,
    /// peer 按协议确认。
    PeerAcked,
    /// 被拒（策略/超时/对端错误）——携带结构化原因。
    Rejected,
}

/// 提交结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SubmitOutcome {
    pub outbox_id: OutboxId,
    pub status: SubmitStatus,
    pub duplicate_of: Option<OutboxId>,
}

/// 市场错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketError {
    Transport(&'static str),
    DeadlineExpired,
    PeerRejected(String),
    Outbox(StoreError),
    UnknownOrder,
}

impl From<StoreError> for MarketError {
    fn from(e: StoreError) -> Self {
        MarketError::Outbox(e)
    }
}

/// 真实报价传输 seam（生产实现不得为恒真闭包）。
pub trait QuoteTransport {
    /// 发送报价到 peer；返回结构化错误（超时/对端拒绝）。
    fn send_quote(&mut self, quote: &QuoteMessage) -> Result<(), MarketError>;
}

/// 市场适配器：outbox + transport + ACK 状态机。
pub struct MarketAdapter<O: DurableOutbox<QuoteMessage>, T: QuoteTransport> {
    pub outbox: O,
    pub transport: T,
    /// external_id → outbox_id（重连对账）。
    external_index: BTreeMap<u64, OutboxId>,
    /// 已 ACK 回执（审计）。
    pub receipts: Vec<(OutboxId, PeerReceipt)>,
    /// 已取消订单（late ACK 处理）。
    cancelled: BTreeMap<OutboxId, u64>,
}

impl<O: DurableOutbox<QuoteMessage>, T: QuoteTransport> MarketAdapter<O, T> {
    pub fn new(outbox: O, transport: T) -> Self {
        MarketAdapter {
            outbox,
            transport,
            external_index: BTreeMap::new(),
            receipts: Vec::new(),
            cancelled: BTreeMap::new(),
        }
    }

    /// 提交报价：先 durable outbox → 发送 → Sent。幂等：同 external_id
    /// 或同 idempotency 已存在 → 返回原 outbox_id（不重复副作用）。
    pub fn submit_quote(
        &mut self,
        quote: QuoteMessage,
        now_ms: u64,
    ) -> Result<SubmitOutcome, MarketError> {
        if quote.quote_ref.deadline_ms != 0 && now_ms > quote.quote_ref.deadline_ms {
            return Err(MarketError::DeadlineExpired);
        }
        if let Some(existing) = self.external_index.get(&quote.quote_ref.external_id) {
            return Ok(SubmitOutcome {
                outbox_id: *existing,
                status: SubmitStatus::PeerAcked, // 已提交过（幂等重放）
                duplicate_of: Some(*existing),
            });
        }
        let outbox_id = self.outbox.enqueue(&quote, quote.quote_ref.idempotency)?;
        self.external_index
            .insert(quote.quote_ref.external_id, outbox_id);
        // 发送（transport 真实调用；失败保持 Queued，不伪装成功）。
        match self.transport.send_quote(&quote) {
            Ok(()) => {
                self.outbox.mark_sent(outbox_id, now_ms)?;
                Ok(SubmitOutcome {
                    outbox_id,
                    status: SubmitStatus::Sent,
                    duplicate_of: None,
                })
            }
            Err(MarketError::PeerRejected(_)) => Ok(SubmitOutcome {
                outbox_id,
                status: SubmitStatus::Rejected,
                duplicate_of: None,
            }),
            Err(_) => {
                // 传输故障（超时/断网）：留在 outbox，重连后重发。
                Ok(SubmitOutcome {
                    outbox_id,
                    status: SubmitStatus::Queued,
                    duplicate_of: None,
                })
            }
        }
    }

    /// peer ACK（幂等；late ACK = 已取消订单 → 记录不改变状态）。
    pub fn handle_peer_ack(
        &mut self,
        outbox_id: OutboxId,
        receipt: PeerReceipt,
    ) -> Result<(), MarketError> {
        if self.cancelled.contains_key(&outbox_id) {
            // late ACK（订单已取消）：记录审计，不再改变状态。
            self.receipts.push((outbox_id, receipt));
            return Ok(());
        }
        if self.receipts.iter().any(|(id, _)| *id == outbox_id) {
            return Ok(()); // duplicate ACK：no-op
        }
        self.outbox.mark_acked(outbox_id, &receipt)?;
        self.receipts.push((outbox_id, receipt));
        Ok(())
    }

    /// 取消订单（cancel race：已 Sent 的订单取消后，late ACK 不复活）。
    pub fn cancel_order(&mut self, outbox_id: OutboxId) -> Result<(), MarketError> {
        self.cancelled.insert(outbox_id, 1);
        Ok(())
    }

    /// 重启恢复：pending 重发（对账）。
    pub fn reconcile_pending(&mut self, now_ms: u64) -> Result<Vec<OutboxId>, MarketError> {
        let pending: Vec<Pending<QuoteMessage>> = self.outbox.recover_pending()?;
        let mut resent = Vec::new();
        for p in pending {
            if p.state == eneros_durable_state::MessageState::PeerAcked {
                continue;
            }
            if let Ok(()) = self.transport.send_quote(&p.message) {
                self.outbox.mark_sent(p.id, now_ms)?;
                resent.push(p.id);
            } // Err → 仍 pending，下次再试
        }
        Ok(resent)
    }
}

#[cfg(test)]
mod tests {
    use eneros_durable_state::{MemOutbox, UnavailableOutbox};

    use super::*;

    struct OkTransport {
        sent: Vec<u64>,
        fail: bool,
    }
    impl QuoteTransport for OkTransport {
        fn send_quote(&mut self, quote: &QuoteMessage) -> Result<(), MarketError> {
            if self.fail {
                return Err(MarketError::Transport("link-down"));
            }
            self.sent.push(quote.quote_ref.external_id);
            Ok(())
        }
    }

    fn quote(n: u64) -> QuoteMessage {
        QuoteMessage {
            quote_ref: QuoteRef {
                external_id: n,
                idempotency: IdempotencyKey([n as u8; 16]),
                deadline_ms: 0,
                market: MarketType::Spot,
                tenant: 1,
                product: 2,
            },
            slot: TimeSlot {
                start: 100,
                end: 200,
            },
            volume_kwh: 10,
            price: YuanPerMwh(500.0),
            amount: MilliYuan(5_000),
        }
    }

    #[test]
    fn positive_submit_sent_ack() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        let out = m.submit_quote(quote(1), 1000).unwrap();
        assert_eq!(out.status, SubmitStatus::Sent);
        assert_eq!(out.outbox_id, OutboxId(0));
        // peer ACK。
        m.handle_peer_ack(
            out.outbox_id,
            PeerReceipt {
                peer: "market".into(),
                receipt_id: 1,
                at_ms: 2000,
            },
        )
        .unwrap();
        assert!(m.outbox.recover_pending().unwrap().is_empty());
        // duplicate ACK no-op。
        m.handle_peer_ack(
            out.outbox_id,
            PeerReceipt {
                peer: "market".into(),
                receipt_id: 1,
                at_ms: 3000,
            },
        )
        .unwrap();
        assert_eq!(m.receipts.len(), 1);
    }

    #[test]
    fn positive_idempotent_resubmit() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        m.submit_quote(quote(1), 0).unwrap();
        let again = m.submit_quote(quote(1), 0).unwrap();
        assert!(again.duplicate_of.is_some());
        assert_eq!(m.outbox.recover_pending().unwrap().len(), 1);
    }

    #[test]
    fn negative_deadline_expired() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        let mut q = quote(1);
        q.quote_ref.deadline_ms = 100;
        assert_eq!(
            m.submit_quote(q, 200).unwrap_err(),
            MarketError::DeadlineExpired
        );
    }

    #[test]
    fn negative_transport_failure_stays_queued() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: true,
            },
        );
        let out = m.submit_quote(quote(1), 0).unwrap();
        assert_eq!(out.status, SubmitStatus::Queued);
        // 重连后对账重发。
        m.transport.fail = false;
        let resent = m.reconcile_pending(5000).unwrap();
        assert_eq!(resent, vec![OutboxId(0)]);
        assert_eq!(m.transport.sent, vec![1]);
    }

    #[test]
    fn unavailable_outbox_fails_closed() {
        let mut m = MarketAdapter::new(
            UnavailableOutbox::new(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        let quote = QuoteMessage {
            quote_ref: QuoteRef {
                external_id: 1,
                idempotency: IdempotencyKey([1; 16]),
                deadline_ms: u64::MAX,
                market: MarketType::DayAhead,
                tenant: 1,
                product: 1,
            },
            slot: TimeSlot { start: 1, end: 2 },
            volume_kwh: 100,
            price: YuanPerMwh(1.0),
            amount: MilliYuan(100_000),
        };
        assert!(m.submit_quote(quote, 0).is_err());
    }

    #[test]
    fn negative_cancel_race_late_ack() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        let out = m.submit_quote(quote(1), 0).unwrap();
        m.cancel_order(out.outbox_id).unwrap();
        // late ACK 不复活状态（记录审计）。
        m.handle_peer_ack(
            out.outbox_id,
            PeerReceipt {
                peer: "market".into(),
                receipt_id: 9,
                at_ms: 9000,
            },
        )
        .unwrap();
        assert_eq!(m.receipts.len(), 1);
    }
}
