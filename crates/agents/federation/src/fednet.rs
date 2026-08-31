//! # Phase 7 联邦网络抽象与确定性模拟（蓝图 61 §12 §2.4/§2.7）
//!
//! [`FedNet`] 是联邦消息投递的确定性模拟：节点邮箱 + 故障注入
//! （分区/丢包/重复/乱序/时延），用于 3/10/100 站压力、网络分区、
//! 协调器重启、重放等测试。生产路径使用真实 mTLS transport
//! （`tls_transport` + `svc-federation`，E3 证据见 tests/phase7_tls.rs）——
//! 本模块与记录式 `real::SeedPresenceBus` 不同：它是显式 test-only
//! 之物（`#[cfg(any(test, feature = "mock"))]`），不进入生产闭包。
//!
//! 帧在 [`crate::offer::FedFrame`]；本模块只负责投递语义。

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::offer::{FedEnvelope, FedFrame, WireError};

/// 节点标识（站点 id / 协调器 0）。
pub type FedNodeId = u32;

/// 网络注入（按测试需要确定性配置）。
#[derive(Clone, Debug, Default)]
pub struct NetFaults {
    /// 每对 (from,to) 丢包计数（确定性：投递时计数 >0 则丢并减一）。
    pub drop_pairs: BTreeMap<(FedNodeId, FedNodeId), u32>,
    /// 全局丢包计数。
    pub drop_global: u32,
    /// 每个信封复制份数（重放/重复送达注入）。
    pub duplicate: u32,
    /// 乱序窗口：投递前按 (seq, from) 对邮箱内消息做逆序（模拟乱序）。
    pub reorder_window: u32,
    /// 单向链路时延（ns，注入 now 推进）。
    pub latency_ns: u64,
}

/// 投递错误（生产 transport 的系统性失败）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    /// 目标不在网络（未注册/被分区）。
    NoRoute,
    /// 邮箱溢出（有界邮箱，见 [`FedNet::mailbox_cap`]）。
    MailboxFull,
    /// 发送方未注册。
    UnknownSender,
    /// 帧编码失败。
    BadFrame,
}

/// 在途信封（`to` 由所在邮箱隐含，不重复存储）。
#[derive(Clone, Debug)]
struct Flight {
    env: FedEnvelope,
    arrive_at: u64,
    seq: u64,
    from: FedNodeId,
}

/// 模拟网络（有界邮箱；显式 test/mock-only）。
pub struct FedNet {
    nodes: BTreeMap<FedNodeId, VecDeque<Flight>>,
    mailbox_cap: usize,
    now_ns: u64,
    faults: NetFaults,
    /// 观测计数（可观测性：drop/duplicate/reorder/overflow）。
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub overflowed: u64,
    pub delivered: u64,
}

impl FedNet {
    pub fn new(mailbox_cap: usize) -> Self {
        Self {
            nodes: BTreeMap::new(),
            mailbox_cap,
            now_ns: 0,
            faults: NetFaults::default(),
            dropped: 0,
            duplicated: 0,
            reordered: 0,
            overflowed: 0,
            delivered: 0,
        }
    }

    /// 注册节点（分区前必须注册）。
    pub fn register(&mut self, id: FedNodeId) {
        self.nodes.entry(id).or_default();
    }

    pub fn is_registered(&self, id: FedNodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn now(&self) -> u64 {
        self.now_ns
    }

    pub fn advance(&mut self, ns: u64) {
        self.now_ns = self.now_ns.saturating_add(ns);
    }

    pub fn faults_mut(&mut self) -> &mut NetFaults {
        &mut self.faults
    }

    /// 分区：移除目的节点注册（分区期消息全部 NoRoute；解除后不可达
    /// 消息自然消失——与真实网络分区语义一致：投递窗口内未送出的消息
    /// 不再送达）。
    pub fn partition(&mut self, node: FedNodeId) {
        self.nodes.remove(&node);
    }

    /// 恢复分区（重新注册，清空邮箱）。
    pub fn heal(&mut self, node: FedNodeId) {
        self.nodes.entry(node).or_default();
    }

    /// 发送（确定性）：分区 → NoRoute；丢包；时延排程。
    pub fn send(
        &mut self,
        from: FedNodeId,
        to: FedNodeId,
        env: FedEnvelope,
    ) -> Result<(), NetError> {
        if !self.nodes.contains_key(&from) {
            return Err(NetError::UnknownSender);
        }
        let mailbox = match self.nodes.get_mut(&to) {
            Some(m) => m,
            None => return Err(NetError::NoRoute),
        };
        // 注入：按对丢包。
        if let Some(n) = self.faults.drop_pairs.get_mut(&(from, to)) {
            if *n > 0 {
                *n -= 1;
                self.dropped += 1;
                return Ok(());
            }
        }
        if self.faults.drop_global > 0 {
            self.faults.drop_global -= 1;
            self.dropped += 1;
            return Ok(());
        }
        // 时延。
        let mut copies = 1u32 + self.faults.duplicate;
        if copies > 8 {
            copies = 8;
        }
        self.duplicated += (copies - 1) as u64;
        // 邮箱容量门（有界，防无界增长）。
        let len = mailbox.len();
        let free = self.mailbox_cap.saturating_sub(len);
        if (copies as usize) > free {
            self.overflowed += (copies as usize - free) as u64;
            return Err(NetError::MailboxFull);
        }
        for i in 0..copies {
            mailbox.push_back(Flight {
                env: env.clone(),
                arrive_at: self.now_ns + self.faults.latency_ns * (i as u64 + 1),
                seq: env.sequence,
                from,
            });
        }
        Ok(())
    }

    /// 推进时钟并投递到期消息到目标邮箱（保持到达序）。
    pub fn drain_due(&mut self) {
        for mailbox in self.nodes.values_mut() {
            let mut arrived: Vec<Flight> = mailbox
                .iter()
                .filter(|f| f.arrive_at <= self.now_ns)
                .cloned()
                .collect();
            mailbox.retain(|f| f.arrive_at > self.now_ns);
            if arrived.is_empty() {
                continue;
            }
            // 确定性乱序注入：窗口内按 seq 逆序。
            if self.faults.reorder_window >= 2 {
                arrived.sort_by(|a, b| b.seq.cmp(&a.seq).then(a.from.cmp(&b.from)));
                self.reordered += 1;
            } else {
                arrived.sort_by(|a, b| a.seq.cmp(&b.seq).then(a.from.cmp(&b.from)));
            }
            mailbox.extend(arrived);
        }
    }

    /// 排空指定节点的整个邮箱（按投递序）。
    pub fn take_all(&mut self, id: FedNodeId) -> Vec<FedEnvelope> {
        let mut out = Vec::new();
        if let Some(m) = self.nodes.get_mut(&id) {
            while let Some(f) = m.pop_front() {
                out.push(f.env);
            }
        }
        self.delivered += out.len() as u64;
        out
    }

    /// 单条取（先进先出）。
    pub fn pop(&mut self, id: FedNodeId) -> Option<FedEnvelope> {
        let e = self.nodes.get_mut(&id)?.pop_front().map(|f| f.env);
        if e.is_some() {
            self.delivered += 1;
        }
        e
    }
}

/// 网络往返时间（纳秒）。
pub const NET_RTT_NS: u64 = 1_000_000;

/// 便捷：发送已编码帧。
pub fn send_frame(
    net: &mut FedNet,
    from: FedNodeId,
    to: FedNodeId,
    env: &FedEnvelope,
) -> Result<(), NetError> {
    net.send(from, to, env.clone())
}

/// 便捷：解码帧（模拟对端收到字节流）。
pub fn recv_frame(frame: &[u8]) -> Result<FedEnvelope, WireError> {
    FedFrame::decode(frame)
}

#[cfg(test)]
mod tests {
    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm2::Sm2KeyPair;

    use super::*;
    use crate::offer::*;

    fn kp() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
    }

    fn env(k: &Sm2KeyPair, rng: &mut CsRng, site: u32, seq: u64) -> FedEnvelope {
        sign_envelope(
            &FedBody::Offer(CapabilityOffer {
                offer_id: [site as u8; 16],
                interval_start_ns: 0,
                interval_end_ns: 3_600_000_000_000,
                p_min_kw: 0.0,
                p_max_kw: 50.0,
                energy_min_kwh: 0.0,
                energy_max_kwh: 50.0,
                ramp_kw_s: 10.0,
                reserve_up_kw: 1.0,
                reserve_down_kw: 1.0,
                price_curve_ref: BlobRefV1::new(1, b"p"),
                capability_root: [0u8; 32],
                valid_until_ns: u64::MAX,
                revision: 1,
            }),
            &EnvelopeHdr {
                epoch: 1,
                sequence: seq,
                tenant_id: 1,
                site_id: site,
                created_ns: 0,
                deadline_ns: u64::MAX,
                signer_key_id: 1,
            },
            k,
            rng,
        )
        .expect("sign")
    }

    #[test]
    fn delivered_in_order() {
        let (k, mut rng) = kp();
        let mut net = FedNet::new(16);
        net.register(1);
        net.register(0);
        for seq in 1..=5 {
            net.send(1, 0, env(&k, &mut rng, 1, seq)).expect("send");
        }
        net.advance(1_000);
        net.drain_due();
        let msgs = net.take_all(0);
        assert_eq!(msgs.len(), 5);
        let seqs: Vec<u64> = msgs.iter().map(|m| m.sequence).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn drop_and_duplicate_injection() {
        let (k, mut rng) = kp();
        let mut net = FedNet::new(16);
        net.register(1);
        net.register(0);
        net.faults_mut().drop_pairs.insert((1, 0), 2);
        net.faults_mut().duplicate = 1;
        for seq in 1..=4 {
            net.send(1, 0, env(&k, &mut rng, 1, seq)).expect("send");
        }
        net.advance(1);
        net.drain_due();
        let msgs = net.take_all(0);
        assert_eq!(net.dropped, 2);
        // 4 条 → 丢 2 → 剩 2 条 × 2 副本 = 4 条送达。
        assert_eq!(msgs.len(), 4);
    }

    #[test]
    fn partition_blocks_then_heals() {
        let (k, mut rng) = kp();
        let mut net = FedNet::new(16);
        net.register(1);
        net.register(0);
        net.partition(0);
        assert_eq!(
            net.send(1, 0, env(&k, &mut rng, 1, 1)),
            Err(NetError::NoRoute)
        );
        net.heal(0);
        net.send(1, 0, env(&k, &mut rng, 1, 2)).expect("send");
        net.advance(1);
        net.drain_due();
        assert_eq!(net.take_all(0).len(), 1);
    }

    #[test]
    fn mailbox_bounded() {
        let (k, mut rng) = kp();
        let mut net = FedNet::new(2);
        net.register(1);
        net.register(0);
        net.send(1, 0, env(&k, &mut rng, 1, 1)).expect("cap ok");
        net.send(1, 0, env(&k, &mut rng, 1, 2)).expect("cap ok");
        // 满：第三条发送显式失败并计数（有界邮箱，不静默丢）。
        assert_eq!(
            net.send(1, 0, env(&k, &mut rng, 1, 3)),
            Err(NetError::MailboxFull)
        );
        assert_eq!(net.overflowed, 1);
        net.advance(1);
        net.drain_due();
        assert_eq!(net.take_all(0).len(), 2);
    }

    #[test]
    fn reorder_injection_reverses_window() {
        let (k, mut rng) = kp();
        let mut net = FedNet::new(16);
        net.register(1);
        net.register(0);
        net.faults_mut().reorder_window = 4;
        for seq in 1..=3 {
            net.send(1, 0, env(&k, &mut rng, 1, seq)).expect("send");
        }
        net.advance(1);
        net.drain_due();
        let msgs = net.take_all(0);
        let seqs: Vec<u64> = msgs.iter().map(|m| m.sequence).collect();
        assert_eq!(seqs, vec![3, 2, 1]);
        assert!(net.reordered >= 1);
    }
}
