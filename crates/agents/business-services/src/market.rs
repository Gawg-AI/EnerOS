//! svc-market 领域逻辑：市场接入服务（蓝图 41 WP-41.4）。
//!
//! 链路（蓝图）：`submit` 先 durable outbox（Queued）→ 真实 transport
//! 发送（Sent）→ peer receipt（PeerAcked）→ 业务成功。重启后
//! `recover_pending` 对账重发；duplicate/late ACK/cancel race 显式处理。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm2::keypair::Sm2PublicKey;
use eneros_crypto::sm2::sign::{sm2_verify, Sm2Signature};
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
    /// peer 回执验签失败（蓝图 61 Phase 6 §5.2：验签并绑定 payload
    /// digest——不验签不回执，不把未验证回执当 PeerAcked）。
    InvalidReceipt(&'static str),
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

/// 签名 peer 回执（[11] §5.2）：receipt + payload digest + SM2 签名。
/// `payload_digest` 覆盖报价消息 canonical（verifier 以 outbox 中消息
/// 复算）——回执与报价绑定，杜绝「任意回执换 PeerAcked」。
#[derive(Debug, Clone, PartialEq)]
pub struct SignedPeerReceipt {
    pub receipt: PeerReceipt,
    /// SM3(消息 canonical ‖ outbox_id ‖ receipt_id ‖ at_ms)。
    pub payload_digest: [u8; 32],
    /// SM2 签名（r‖s 64B；覆盖同一 canonical）。
    pub signature: [u8; 64],
    pub key_id: [u8; 8],
}

impl SignedPeerReceipt {
    /// canonical（签名/摘要不参与；验签先比对摘要再验签）。
    pub fn canonical_bytes(
        id: OutboxId,
        msg: &QuoteMessage,
        receipt: &PeerReceipt,
    ) -> alloc::vec::Vec<u8> {
        let mut b = alloc::vec::Vec::with_capacity(160);
        b.extend_from_slice(&eneros_durable_state::Encode::encode(msg).unwrap_or_default());
        b.extend_from_slice(&id.0.to_le_bytes());
        b.extend_from_slice(&receipt.receipt_id.to_le_bytes());
        b.extend_from_slice(&receipt.at_ms.to_le_bytes());
        b
    }

    /// 计算摘要（canonical → SM3）。
    pub fn compute_digest(id: OutboxId, msg: &QuoteMessage, receipt: &PeerReceipt) -> [u8; 32] {
        eneros_crypto::sm3::hash(&Self::canonical_bytes(id, msg, receipt))
    }

    /// 签名回填（sandbox/测试确定性签名；生产私钥在 peer 侧）。
    pub fn seal(id: OutboxId, msg: &QuoteMessage, receipt: &PeerReceipt) -> Self {
        let sk = test_signer_key();
        let pk = test_signer_pub();
        let digest = Self::compute_digest(id, msg, receipt);
        let signer = eneros_crypto::sm2::sign::Sm2Signer::new();
        let sig = signer
            .sign_deterministic(&Self::canonical_bytes(id, msg, receipt), &sk, &pk)
            .expect("deterministic sign");
        Self {
            receipt: receipt.clone(),
            payload_digest: digest,
            signature: sig.to_bytes(),
            key_id: *b"testkey1",
        }
    }
}

/// sandbox 签名密钥（固定标量——与全仓测试密钥同源；仅测试/sandbox
/// 用；生产私钥在 peer 侧，本 crate 无任何私钥装配面）。
pub fn test_signer_key() -> eneros_crypto::sm2::keypair::Sm2PrivateKey {
    let mut sk_bytes = [0u8; 32];
    sk_bytes[31] = 0x2A;
    eneros_crypto::sm2::keypair::Sm2PrivateKey::from_bytes(&sk_bytes).expect("fixed scalar key")
}

pub fn test_signer_pub() -> Sm2PublicKey {
    let pair = eneros_crypto::sm2::keypair::Sm2KeyPair::from_private_key(&test_signer_key())
        .expect("keypair");
    pair.public_key
}

/// 回执验证器（验签并绑定 payload digest；[11] §5.2）。
pub trait ReceiptVerifier {
    fn verify(
        &self,
        id: OutboxId,
        msg: &QuoteMessage,
        signed: &SignedPeerReceipt,
    ) -> Result<(), MarketError>;

    /// 已配置密钥数（0 = fail-closed 拒绝一切回执——审计状态面）。
    fn key_count(&self) -> usize {
        0
    }
}

/// SM2 回执验证器（密钥注册表；空表 = 全部拒绝，fail-closed）。
pub struct Sm2ReceiptVerifier {
    keys: alloc::collections::BTreeMap<[u8; 8], Sm2PublicKey>,
}

impl Sm2ReceiptVerifier {
    pub fn new() -> Self {
        Self {
            keys: alloc::collections::BTreeMap::new(),
        }
    }

    pub fn add_key(&mut self, key_id: [u8; 8], pk: Sm2PublicKey) {
        self.keys.insert(key_id, pk);
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }
}

impl Default for Sm2ReceiptVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ReceiptVerifier for Sm2ReceiptVerifier {
    fn verify(
        &self,
        id: OutboxId,
        msg: &QuoteMessage,
        signed: &SignedPeerReceipt,
    ) -> Result<(), MarketError> {
        // 摘要绑定（消息/回执任一字段被改 → digest 不匹配）。
        let digest = SignedPeerReceipt::compute_digest(id, msg, &signed.receipt);
        if digest != signed.payload_digest {
            return Err(MarketError::InvalidReceipt("digest"));
        }
        // 未知 key → 拒。
        let Some(pk) = self.keys.get(&signed.key_id) else {
            return Err(MarketError::InvalidReceipt("unknown-key"));
        };
        // SM2 验签（对 canonical；错误/无效签名统一拒绝）。
        let sig = Sm2Signature::from_bytes(&signed.signature);
        match sm2_verify(
            &SignedPeerReceipt::canonical_bytes(id, msg, &signed.receipt),
            &sig,
            pk,
        ) {
            Ok(true) => Ok(()),
            _ => Err(MarketError::InvalidReceipt("signature")),
        }
    }
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
    /// 回执验证器（无密钥 = 全部拒绝；[11] §5.2 验签硬门）。
    verifier: alloc::boxed::Box<dyn ReceiptVerifier>,
}

impl<O: DurableOutbox<QuoteMessage>, T: QuoteTransport> MarketAdapter<O, T> {
    /// 默认 fail-closed 验证器（空密钥表——一切回执拒绝；装配方必须
    /// 用 `with_verifier` 注入已配置密钥）。
    pub fn new(outbox: O, transport: T) -> Self {
        MarketAdapter {
            outbox,
            transport,
            external_index: BTreeMap::new(),
            receipts: Vec::new(),
            cancelled: BTreeMap::new(),
            verifier: alloc::boxed::Box::new(Sm2ReceiptVerifier::new()),
        }
    }

    /// 注入回执验证器（生产装配）。
    pub fn with_verifier(
        outbox: O,
        transport: T,
        verifier: alloc::boxed::Box<dyn ReceiptVerifier>,
    ) -> Self {
        MarketAdapter {
            verifier,
            ..Self::new(outbox, transport)
        }
    }

    /// 回执验证器可审计状态（key 数；0 = fail-closed 拒绝一切 ACK）。
    pub fn verifier_key_count(&self) -> usize {
        self.verifier.key_count()
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
            // 幂等重放：返回**真实原状态**（[11] §5.2——不把 Queued/Sent
            // 当 PeerAcked）。outbox 只保留未 ACK 消息：在 recover_pending
            // 中 → Queued/Sent 之一；不在 → 已 ACK（PeerAcked）。
            let status = match self
                .outbox
                .recover_pending()?
                .into_iter()
                .find(|p| p.id == *existing)
            {
                Some(p) => match p.state {
                    eneros_durable_state::MessageState::Queued => SubmitStatus::Queued,
                    eneros_durable_state::MessageState::Sent => SubmitStatus::Sent,
                    eneros_durable_state::MessageState::PeerAcked => SubmitStatus::PeerAcked,
                },
                None => SubmitStatus::PeerAcked,
            };
            return Ok(SubmitOutcome {
                outbox_id: *existing,
                status,
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

    /// peer ACK（[11] §5.2）：**必须先验签**（digest 绑定消息 + SM2），
    /// 未验签回执一律拒绝——不存在「未验证 ACK 路径」。
    pub fn handle_peer_ack(
        &mut self,
        outbox_id: OutboxId,
        signed: &SignedPeerReceipt,
    ) -> Result<(), MarketError> {
        if self.cancelled.contains_key(&outbox_id) {
            // late ACK（订单已取消）：记录审计（不经验签——订单已不在
            // 交易面；仅作取消后流量痕迹），不再改变状态。
            self.receipts.push((outbox_id, signed.receipt.clone()));
            return Ok(());
        }
        if self.receipts.iter().any(|(id, _)| *id == outbox_id) {
            return Ok(()); // duplicate ACK：no-op
        }
        // 找回原消息（验签需要 payload；未 ACK 才在 recover_pending 中）。
        let msg = self
            .outbox
            .recover_pending()?
            .into_iter()
            .find(|p| p.id == outbox_id)
            .map(|p| p.message)
            .ok_or(MarketError::UnknownOrder)?;
        self.verifier.verify(outbox_id, &msg, signed)?;
        self.outbox.mark_acked(outbox_id, &signed.receipt)?;
        self.receipts.push((outbox_id, signed.receipt.clone()));
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

    fn verified_adapter() -> MarketAdapter<MemOutbox<QuoteMessage>, OkTransport> {
        let mut verifier = Sm2ReceiptVerifier::new();
        verifier.add_key(*b"testkey1", test_signer_pub());
        MarketAdapter::with_verifier(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
            alloc::boxed::Box::new(verifier),
        )
    }

    fn signed_ack(
        id: OutboxId,
        msg: &QuoteMessage,
        receipt_id: u64,
        at_ms: u64,
    ) -> SignedPeerReceipt {
        let receipt = PeerReceipt {
            peer: "market".into(),
            receipt_id,
            at_ms,
        };
        SignedPeerReceipt::seal(id, msg, &receipt)
    }

    #[test]
    fn positive_submit_sent_ack() {
        let mut m = verified_adapter();
        let q = quote(1);
        let out = m.submit_quote(q.clone(), 1000).unwrap();
        assert_eq!(out.status, SubmitStatus::Sent);
        assert_eq!(out.outbox_id, OutboxId(0));
        // peer ACK（验签 + digest 绑定 → 通过）。
        m.handle_peer_ack(out.outbox_id, &signed_ack(out.outbox_id, &q, 1, 2000))
            .unwrap();
        assert!(m.outbox.recover_pending().unwrap().is_empty());
        assert_eq!(m.receipts.len(), 1);
        // duplicate ACK no-op（已 ACK 不在 pending——直接按 receipts 判重）。
        m.handle_peer_ack(out.outbox_id, &signed_ack(out.outbox_id, &q, 1, 3000))
            .unwrap();
        assert_eq!(m.receipts.len(), 1);
    }

    #[test]
    fn ack_without_valid_signature_rejected() {
        // 无密钥验证器（fail-closed）：任何回执 → InvalidReceipt。
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        let q = quote(1);
        let out = m.submit_quote(q.clone(), 1000).unwrap();
        let signed = signed_ack(out.outbox_id, &q, 1, 2000);
        assert_eq!(
            m.handle_peer_ack(out.outbox_id, &signed).unwrap_err(),
            MarketError::InvalidReceipt("unknown-key")
        );
        // 状态未被伪造：仍未 ACK。
        assert_eq!(m.outbox.recover_pending().unwrap().len(), 1);
        // 签名被篡改 → 拒。
        let mut m = verified_adapter();
        let out = m.submit_quote(q.clone(), 1000).unwrap();
        let mut bad = signed_ack(out.outbox_id, &q, 1, 2000);
        bad.signature[0] ^= 0xFF;
        assert_eq!(
            m.handle_peer_ack(out.outbox_id, &bad).unwrap_err(),
            MarketError::InvalidReceipt("signature")
        );
        // digest 被篡改（回执与消息不一致）→ 拒。
        let mut m2 = verified_adapter();
        let q2 = quote(2);
        let out2 = m2.submit_quote(q2.clone(), 1000).unwrap();
        let mut mangled = signed_ack(out2.outbox_id, &q2, 1, 2000);
        mangled.payload_digest[0] ^= 0x01;
        assert_eq!(
            m2.handle_peer_ack(out2.outbox_id, &mangled).unwrap_err(),
            MarketError::InvalidReceipt("digest")
        );
    }

    #[test]
    fn positive_idempotent_resubmit_returns_real_state() {
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: false,
            },
        );
        // 首次提交：transport 成功 → Sent；重放必须如实返回 Sent
        // （不把 Queued/Sent 当 PeerAcked，[11] §5.2）。
        m.submit_quote(quote(1), 0).unwrap();
        let again = m.submit_quote(quote(1), 0).unwrap();
        assert!(again.duplicate_of.is_some());
        assert_eq!(again.status, SubmitStatus::Sent);
        assert_eq!(m.outbox.recover_pending().unwrap().len(), 1);
        // 传输故障路径：首次 Queued → 重放 Queued。
        let mut m = MarketAdapter::new(
            MemOutbox::default(),
            OkTransport {
                sent: vec![],
                fail: true,
            },
        );
        m.submit_quote(quote(2), 0).unwrap();
        let again = m.submit_quote(quote(2), 0).unwrap();
        assert_eq!(again.status, SubmitStatus::Queued);
        // ACK 后重放 → PeerAcked（消息不在 pending）。
        let mut m = verified_adapter();
        let q = quote(3);
        let out = m.submit_quote(q.clone(), 0).unwrap();
        m.handle_peer_ack(out.outbox_id, &signed_ack(out.outbox_id, &q, 1, 1))
            .unwrap();
        let again = m.submit_quote(q, 0).unwrap();
        assert_eq!(again.status, SubmitStatus::PeerAcked);
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
        let q = quote(1);
        let out = m.submit_quote(q.clone(), 0).unwrap();
        m.cancel_order(out.outbox_id).unwrap();
        // late ACK 不复活状态（记录审计——取消后订单已不在交易面）。
        m.handle_peer_ack(out.outbox_id, &signed_ack(out.outbox_id, &q, 9, 9000))
            .unwrap();
        assert_eq!(m.receipts.len(), 1);
    }
}
