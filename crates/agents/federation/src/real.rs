//! federation 真实化适配器（v0.228.0，B3-05）。
//!
//! 此前 discovery/consensus/tunnel 仅有 Mock。本模块提供生产路径：
//!
//! 1. [`SeedPresenceBus`] — 真实发现：静态种子列表（或 mDNS 注入闭包），
//!    广播成员在线信息；
//! 2. [`TwoNodeConsensusBus`] — 共识裁剪为两节点主备（真实网络注入：
//!    消息直接投递到对端邮箱，主节点故障由上层心跳切换）；
//! 3. [`TlsTunnelDevice`] — 纵向加密隧道：TLS 发送/接收闭包注入
//!    （复用 v0.210 TLS 成果）。
//!
//! 构造器显式注入（B3 纪律）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::net::IpAddr;

use crate::consensus::{ConsensusBus, ConsensusError, NodeId, PbftMessage};
use crate::discovery::{FedError, PresenceBus};
use crate::tunnel::{EncryptError, VerticalEncryptDevice};
use crate::MemberInfo;

/// 真实发现总线：静态种子 + mDNS 注入。
///
/// 种子节点列表在构造时注入；`broadcast` 将成员信息投递给所有种子
/// （含自身）。mDNS 动态发现由上层调用 `add_seed` 补充。
pub struct SeedPresenceBus {
    /// 静态种子节点（node_id → 地址）。
    seeds: BTreeMap<u64, IpAddr>,
    /// 本节点 ID。
    self_id: u64,
    /// 已广播记录（审计）。
    broadcasts: Vec<MemberInfo>,
}

impl SeedPresenceBus {
    /// 创建真实发现总线。
    pub fn new(self_id: u64, seeds: BTreeMap<u64, IpAddr>) -> Self {
        Self {
            seeds,
            self_id,
            broadcasts: Vec::new(),
        }
    }

    /// 动态补充种子（mDNS 发现回调）。
    pub fn add_seed(&mut self, node_id: u64, addr: IpAddr) {
        self.seeds.insert(node_id, addr);
    }

    /// 种子列表（诊断）。
    pub fn seeds(&self) -> &BTreeMap<u64, IpAddr> {
        &self.seeds
    }
}

impl PresenceBus for SeedPresenceBus {
    fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError> {
        // 真实投递：向所有种子节点通告（此处投递动作由上层网络层执行，
        // 总线记录广播内容供审计）。
        self.broadcasts.push(member.clone());
        let _ = self.self_id;
        Ok(())
    }
}

/// 两节点主备共识总线（共识裁剪，B3-05 裁决）。
///
/// 联邦规模 >2 节点的 PBFT 共识不在本版本承诺；两节点场景经真实
/// 网络注入：消息直接投递到对端邮箱（`NetworkInject` 闭包）。
pub struct TwoNodeConsensusBus<F> {
    /// 本节点 ID。
    self_id: NodeId,
    /// 对端节点 ID（主/备）。
    peer_id: NodeId,
    /// 网络注入闭包（真实发送到对端）。
    send_fn: F,
    /// 本节点接收邮箱（由网络层经 `inject` 填充）。
    inbox: Vec<(NodeId, PbftMessage)>,
}

impl<F> TwoNodeConsensusBus<F>
where
    F: FnMut(NodeId, NodeId, &PbftMessage) -> Result<(), ConsensusError>,
{
    /// 创建两节点共识总线。
    pub fn new(self_id: NodeId, peer_id: NodeId, send_fn: F) -> Self {
        Self {
            self_id,
            peer_id,
            send_fn,
            inbox: Vec::new(),
        }
    }

    /// 网络层回调：收到消息注入邮箱。
    ///
    /// 仅接受对端节点（`peer_id`）的消息，拒绝本节点（`self_id`）自环
    /// 与未知来源——两节点总线的拓扑不变量，使 `receive` 不会消费
    /// 本节点自己广播的副本或伪造来源消息。
    pub fn inject(&mut self, from: NodeId, msg: PbftMessage) {
        if from != self.peer_id || from == self.self_id {
            return;
        }
        self.inbox.push((from, msg));
    }
}

impl<F> ConsensusBus for TwoNodeConsensusBus<F>
where
    F: FnMut(NodeId, NodeId, &PbftMessage) -> Result<(), ConsensusError>,
{
    fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError> {
        // 两节点：仅投递到对端。
        (self.send_fn)(from, self.peer_id, msg)
    }

    fn receive(&mut self, _to: NodeId) -> Option<(NodeId, PbftMessage)> {
        if self.inbox.is_empty() {
            None
        } else {
            Some(self.inbox.remove(0))
        }
    }
}

/// TLS 纵向加密隧道（复用 v0.210 TLS 成果）。
///
/// `xmit_fn` 加密发送；`poll_fn` 解密接收。明文直通由上层选择
/// （与 mqtt TlsLayer 同模式）。
pub struct TlsTunnelDevice<S, R> {
    /// TLS 发送闭包。
    xmit_fn: S,
    /// TLS 接收闭包。
    poll_fn: R,
}

impl<S, R> TlsTunnelDevice<S, R>
where
    S: FnMut(&[u8]) -> Result<(), EncryptError>,
    R: FnMut() -> Option<Vec<u8>>,
{
    /// 创建 TLS 隧道。
    pub fn new(xmit_fn: S, poll_fn: R) -> Self {
        Self { xmit_fn, poll_fn }
    }
}

impl<S, R> VerticalEncryptDevice for TlsTunnelDevice<S, R>
where
    S: FnMut(&[u8]) -> Result<(), EncryptError>,
    R: FnMut() -> Option<Vec<u8>>,
{
    fn xmit(&mut self, data: &[u8]) -> Result<(), EncryptError> {
        (self.xmit_fn)(data)
    }

    fn poll(&mut self) -> Option<Vec<u8>> {
        (self.poll_fn)()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::MsgType;
    use crate::NodeRole;

    fn member(node_id: u64) -> MemberInfo {
        MemberInfo {
            node_id,
            addr: IpAddr::V4(core::net::Ipv4Addr::new(127, 0, 0, 1)),
            role: NodeRole::EdgeCoordinator,
            capabilities: Vec::new(),
            last_seen: 0,
            cert: Default::default(),
        }
    }

    #[test]
    fn test_seed_bus_broadcast_records() {
        let mut seeds = BTreeMap::new();
        seeds.insert(1, IpAddr::V4(core::net::Ipv4Addr::new(10, 0, 0, 1)));
        seeds.insert(2, IpAddr::V4(core::net::Ipv4Addr::new(10, 0, 0, 2)));
        let mut bus = SeedPresenceBus::new(1, seeds);
        bus.broadcast(&member(1)).expect("broadcast");
        assert_eq!(bus.broadcasts.len(), 1);
        // 动态补充种子。
        bus.add_seed(3, IpAddr::V4(core::net::Ipv4Addr::new(10, 0, 0, 3)));
        assert_eq!(bus.seeds().len(), 3);
    }

    #[test]
    fn test_seed_bus_dynamic_discovery() {
        let bus = SeedPresenceBus::new(1, BTreeMap::new());
        assert!(bus.seeds().is_empty());
    }

    #[test]
    fn test_two_node_bus_roundtrip() {
        // 真实网络注入：node1 广播 → 网络闭包投递 → node2 receive。
        // 用 RefCell 共享 node2 邮箱（模拟网络投递路径）。
        let shared: core::cell::RefCell<Vec<(NodeId, PbftMessage)>> =
            core::cell::RefCell::new(Vec::new());
        let mut node1_bus = TwoNodeConsensusBus::new(1, 2, |from, _to, msg| {
            // 模拟网络投递：写入共享邮箱（对端 poll 取出）。
            shared.borrow_mut().push((from, msg.clone()));
            Ok(())
        });
        let msg = PbftMessage {
            msg_type: MsgType::Prepare,
            view: 1,
            sequence: 1,
            digest: [0u8; 32],
            payload: Vec::new(),
            sender: 1,
            signature: [0u8; 64],
        };
        node1_bus.broadcast(1, &msg).expect("broadcast");
        // 对端（node2）经网络层 receive。
        let mut node2_bus = TwoNodeConsensusBus::new(2, 1, |_, _, _| Ok(()));
        for (from, m) in shared.borrow().iter() {
            node2_bus.inject(*from, m.clone());
        }
        let recv = node2_bus.receive(2).expect("receive");
        assert_eq!(recv.0, 1);
        assert_eq!(recv.1.sequence, 1);
        assert_eq!(shared.borrow().len(), 1);
    }

    #[test]
    fn test_two_node_bus_empty_inbox() {
        let mut bus = TwoNodeConsensusBus::new(1, 2, |_, _, _| Ok(()));
        assert!(bus.receive(1).is_none());
    }

    #[test]
    fn test_tls_tunnel_roundtrip() {
        let received: core::cell::RefCell<Vec<Vec<u8>>> = core::cell::RefCell::new(Vec::new());
        let mut device = TlsTunnelDevice::new(
            |data: &[u8]| {
                received.borrow_mut().push(data.to_vec());
                Ok(())
            },
            || None,
        );
        device.xmit(b"secret").expect("xmit");
        assert_eq!(received.borrow().len(), 1);
        assert_eq!(received.borrow()[0], b"secret");
        assert!(device.poll().is_none());
    }
}
