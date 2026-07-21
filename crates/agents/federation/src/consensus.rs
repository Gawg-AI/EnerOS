//! v0.99.0 联邦共识协议：数据结构与总线抽象 + 引擎核心。
//!
//! 提供 `NodeId`、`ConsensusState`、`MsgType`、`PbftMessage`、`LogEntry`、
//! `ConsensusResult`、`ConsensusError`、`ConsensusBus` trait、`MockConsensusBus`
//! 与 `ConsensusEngine`。引擎通过 `submit` 发起共识、`poll` 驱动状态机，
//! 支持三阶段 PBFT（PrePrepare/Prepare/Commit）与 ViewChange 超时切换。
//!
//! ## 设计要点
//!
//! - **同步状态机**（D3）：`submit` 广播 PrePrepare 后返回 seq；`poll(bus, now_ms)`
//!   循环 `receive` 至空，逐条 `handle_message` 推进状态，返回本轮新提交结果集。
//! - **投票去重**（D5）：`LogEntry` 用 `BTreeSet<NodeId>` 记录 prepare/commit 投票人，
//!   天然防拜占庭节点重复投票。
//! - **SM2 签名域分离**（D9）：签名消息体 = `type:u8‖view:u64be‖seq:u64be‖digest‖sender:u64be(‖payload)`。
//! - **Mock 总线故障注入**（D11）：`isolated` 模拟节点离线（不投不收）；`fail_times`
//!   模拟网络分区丢包。
//! - **可观测**（D12）：4 计数器 + `last_latency_ms`（提交时刻 − 请求受理时刻）。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use eneros_crypto::{CsRng, Sm2KeyPair, Sm2PublicKey};

/// 节点标识符（无堆 u64，v0.97.0 D1 惯例，D2）
pub type NodeId = u64;

/// 共识状态（D6：增 Idle 初始/视图切换后状态）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusState {
    /// 初始/视图切换后的静止态
    Idle,
    /// 已广播 PrePrepare
    PrePrepare,
    /// 已收集足够 Prepare 并广播 Commit
    Prepare,
    /// 已收集足够 Commit
    Commit,
    /// 已执行提交
    Done,
}

/// 消息类型（D4：增 ViewChange 变体，Reply 占位）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    PrePrepare,
    Prepare,
    Commit,
    Reply,
    ViewChange,
}

impl MsgType {
    /// 编码为 u8
    pub fn to_u8(&self) -> u8 {
        match self {
            MsgType::PrePrepare => 0,
            MsgType::Prepare => 1,
            MsgType::Commit => 2,
            MsgType::Reply => 3,
            MsgType::ViewChange => 4,
        }
    }

    /// 从 u8 解码，非法值返回 None
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(MsgType::PrePrepare),
            1 => Some(MsgType::Prepare),
            2 => Some(MsgType::Commit),
            3 => Some(MsgType::Reply),
            4 => Some(MsgType::ViewChange),
            _ => None,
        }
    }
}

/// PBFT 消息帧（D4：含 sender + payload，字段全 pub）
#[derive(Debug, Clone, PartialEq)]
pub struct PbftMessage {
    pub msg_type: MsgType,
    pub view: u64,
    pub sequence: u64,
    pub digest: [u8; 32],
    pub payload: Vec<u8>,
    pub sender: NodeId,
    pub signature: [u8; 64],
}

/// 日志条目（D5：voter BTreeSet 去重）
#[derive(Clone)]
pub struct LogEntry {
    pub sequence: u64,
    pub request: Vec<u8>,
    pub digest: [u8; 32],
    pub prepare_voters: BTreeSet<NodeId>,
    pub commit_voters: BTreeSet<NodeId>,
    pub prepared: bool,
    pub committed: bool,
    pub executed: bool,
    /// 请求受理时刻（主节点 submit / 备份收到 PrePrepare），用于 D12 延迟计算
    pub submitted_ms: u64,
}

impl LogEntry {
    /// 返回 prepare 投票人数（u32）
    pub fn prepare_count(&self) -> u32 {
        self.prepare_voters.len() as u32
    }

    /// 返回 commit 投票人数（u32）
    pub fn commit_count(&self) -> u32 {
        self.commit_voters.len() as u32
    }
}

/// 共识结果（单条已提交请求的摘要）
#[derive(Debug, Clone, PartialEq)]
pub struct ConsensusResult {
    pub sequence: u64,
    pub digest: [u8; 32],
    pub view: u64,
}

/// 共识错误（D10：7 变体最小完备）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusError {
    /// 非主节点却尝试提交
    NotPrimary,
    /// 未知节点（sender 不在 nodes，或 peers 缺失）
    UnknownNode,
    /// 签名验证失败
    InvalidSignature,
    /// sender 不是该 view 的主节点
    ViewMismatch,
    /// view 过期或 digest 不匹配等陈旧消息
    StaleMessage,
    /// 节点数不足（空、重复、不满足 3f+1）
    NotEnoughNodes,
    /// 总线广播失败
    BusError,
}

/// 共识总线抽象（sync，无 async，无 Send+Sync，D3）
pub trait ConsensusBus {
    /// 将消息广播到网络（Mock 中投递到各节点邮箱）
    fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError>;
    /// 从指定节点的邮箱接收一条消息（FIFO）
    fn receive(&mut self, to: NodeId) -> Option<(NodeId, PbftMessage)>;
}

/// Mock 共识总线（故障注入：isolated 离线、fail_times 丢包）
#[derive(Debug, Clone, Default)]
pub struct MockConsensusBus {
    /// 各节点邮箱队列
    pub queues: BTreeMap<NodeId, Vec<(NodeId, PbftMessage)>>,
    /// 被隔离的节点（不投出也不收信）
    pub isolated: BTreeSet<NodeId>,
    /// 剩余应失败次数（>0 → Err(BusError) 并递减）
    pub fail_times: u32,
}

impl MockConsensusBus {
    /// 创建空 Mock 总线
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册节点（建立空邮箱）
    pub fn register(&mut self, id: NodeId) {
        self.queues.entry(id).or_default();
    }
}

impl ConsensusBus for MockConsensusBus {
    fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(ConsensusError::BusError);
        }
        if self.isolated.contains(&from) {
            return Ok(());
        }
        // 向所有已注册且非 isolated 节点投递（含 from 自身）
        for (&node_id, queue) in self.queues.iter_mut() {
            if !self.isolated.contains(&node_id) {
                queue.push((from, msg.clone()));
            }
        }
        Ok(())
    }

    fn receive(&mut self, to: NodeId) -> Option<(NodeId, PbftMessage)> {
        if self.isolated.contains(&to) {
            return None;
        }
        let queue = self.queues.get_mut(&to)?;
        if queue.is_empty() {
            return None;
        }
        // FIFO 弹出队首（EX7：与 spec "弹出队首" 一致，保证确定性消息序）
        Some(queue.remove(0))
    }
}

/// 共识引擎（PBFT 变体，禁 Debug，含 Sm2KeyPair）
///
/// 字段全 pub，便于测试观测。
pub struct ConsensusEngine {
    /// 排序后的节点列表
    pub nodes: Vec<NodeId>,
    /// 本节点 id
    pub local_id: NodeId,
    /// 当前视图号
    pub view: u64,
    /// 最新已分配序列号
    pub sequence: u64,
    /// 当前状态
    pub state: ConsensusState,
    /// 请求日志
    pub log: Vec<LogEntry>,
    /// 本节点 SM2 密钥对
    pub kp: Sm2KeyPair,
    /// 各节点公钥（含自身）
    pub peers: BTreeMap<NodeId, Sm2PublicKey>,
    /// 随机数生成器（SM2 签名用，EX1）
    pub rng: CsRng,
    /// 超时基准（毫秒）
    pub timeout_ms: u64,
    /// 最近一次有进展的时刻（毫秒）
    pub last_progress_ms: u64,
    /// 连续 ViewChange 次数
    pub consecutive_vc: u32,
    /// ViewChange 票集（按目标视图分桶，EX2）
    pub vc_votes: BTreeMap<u64, BTreeSet<NodeId>>,
    /// 提交请求计数
    pub submit_count: u64,
    /// 已提交计数
    pub committed_count: u64,
    /// 被拒绝消息计数
    pub rejected_count: u64,
    /// 视图切换次数
    pub view_change_count: u64,
    /// 最近一次提交延迟（毫秒）
    pub last_latency_ms: u64,
}

impl ConsensusEngine {
    /// 创建引擎并校验输入合法性。
    ///
    /// - nodes 为空或含重复 → `NotEnoughNodes`
    /// - nodes 不含 local_id → `UnknownNode`
    /// - peers 缺失任一 nodes 节点公钥 → `UnknownNode`
    pub fn new(
        mut nodes: Vec<NodeId>,
        local_id: NodeId,
        kp: Sm2KeyPair,
        peers: BTreeMap<NodeId, Sm2PublicKey>,
        rng: CsRng,
        timeout_ms: u64,
    ) -> Result<Self, ConsensusError> {
        if nodes.is_empty() {
            return Err(ConsensusError::NotEnoughNodes);
        }
        // 检测重复
        let set: BTreeSet<_> = nodes.iter().copied().collect();
        if set.len() != nodes.len() {
            return Err(ConsensusError::NotEnoughNodes);
        }
        if !nodes.contains(&local_id) {
            return Err(ConsensusError::UnknownNode);
        }
        for node in &nodes {
            if !peers.contains_key(node) {
                return Err(ConsensusError::UnknownNode);
            }
        }
        nodes.sort();
        Ok(Self {
            nodes,
            local_id,
            view: 0,
            sequence: 0,
            state: ConsensusState::Idle,
            log: Vec::new(),
            kp,
            peers,
            rng,
            timeout_ms,
            last_progress_ms: 0,
            consecutive_vc: 0,
            vc_votes: BTreeMap::new(),
            submit_count: 0,
            committed_count: 0,
            rejected_count: 0,
            view_change_count: 0,
            last_latency_ms: 0,
        })
    }

    /// 当前视图下本节点是否为主节点
    pub fn is_primary(&self) -> bool {
        crate::pbft::primary_of(&self.nodes, self.view) == self.local_id
    }

    /// 指定序列号是否已提交
    pub fn is_committed(&self, seq: u64) -> bool {
        self.log.iter().any(|e| e.sequence == seq && e.committed)
    }

    /// 按序列号查找日志条目（可变引用）
    pub(crate) fn find_entry(&mut self, seq: u64) -> Option<&mut LogEntry> {
        self.log.iter_mut().find(|e| e.sequence == seq)
    }

    /// 主节点提交请求（实际逻辑在 pbft.rs `do_submit`）
    pub fn submit(
        &mut self,
        request: Vec<u8>,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<u64, ConsensusError> {
        self.do_submit(request, bus, now_ms)
    }

    /// 驱动状态机：排空邮箱并处理所有消息，返回本轮新达成的共识结果。
    pub fn poll(
        &mut self,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<Vec<ConsensusResult>, ConsensusError> {
        let mut results = Vec::new();
        while let Some((from, msg)) = bus.receive(self.local_id) {
            if let Some(res) = self.handle_message(from, msg, bus, now_ms)? {
                results.push(res);
            }
        }
        Ok(results)
    }

    /// 单条消息处理：前置校验（sender → 验签 → view）后按 msg_type 分发。
    ///
    /// 校验顺序（D9/EX5）：
    /// 1. sender 不在 nodes → rejected_count+=1 + `Err(UnknownNode)`
    /// 2. 验签失败 → rejected_count+=1 + `Err(InvalidSignature)`
    /// 3. msg.view < self.view → rejected_count+=1 + `Err(StaleMessage)`
    /// 4. msg.view > self.view：ViewChange 放行；PrePrepare 乐观视图同步
    ///    （EX5：签名有效的 PrePrepare 证明新主已获 VC 法定人数，先 enter_view
    ///    再按正常流程处理）；Prepare/Commit → rejected_count+=1 + `Err(StaleMessage)`
    /// 5. 通过全部校验 → `last_progress_ms = now_ms`，按 msg_type 分发
    pub fn handle_message(
        &mut self,
        from: NodeId,
        msg: PbftMessage,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<Option<ConsensusResult>, ConsensusError> {
        // 未知 sender
        if !self.nodes.contains(&msg.sender) {
            self.rejected_count += 1;
            return Err(ConsensusError::UnknownNode);
        }
        // 签名验证
        let body = crate::pbft::message_body(
            msg.msg_type,
            msg.view,
            msg.sequence,
            &msg.digest,
            msg.sender,
            &msg.payload,
        );
        let pk = match self.peers.get(&msg.sender) {
            Some(pk) => pk,
            None => {
                self.rejected_count += 1;
                return Err(ConsensusError::UnknownNode);
            }
        };
        if !crate::pbft::verify_message(pk, &body, &msg.signature) {
            self.rejected_count += 1;
            return Err(ConsensusError::InvalidSignature);
        }
        // 陈旧 view
        if msg.view < self.view {
            self.rejected_count += 1;
            return Err(ConsensusError::StaleMessage);
        }
        // 未来 view
        if msg.view > self.view {
            match msg.msg_type {
                MsgType::ViewChange => {}
                MsgType::PrePrepare => {
                    // EX5 乐观视图同步：随新主 PrePrepare 进入新视图
                    self.enter_view(msg.view, bus, now_ms)?;
                }
                _ => {
                    self.rejected_count += 1;
                    return Err(ConsensusError::StaleMessage);
                }
            }
        }
        // 通过校验，更新进展时刻
        self.last_progress_ms = now_ms;
        // 按类型分发
        match msg.msg_type {
            MsgType::PrePrepare => self.on_pre_prepare(from, msg, bus, now_ms),
            MsgType::Prepare => self.on_prepare(from, msg, bus, now_ms),
            MsgType::Commit => self.on_commit(from, msg, now_ms),
            MsgType::ViewChange => self.on_view_change(from, msg, bus, now_ms).map(|_| None),
            MsgType::Reply => Ok(None),
        }
    }
}

// ============================================================
// Test Utilities（crate 内各测试模块共享）
// ============================================================

#[cfg(test)]
pub(crate) mod testutil {
    use eneros_crypto::Sm2KeyPair;

    use super::*;
    use crate::pbft::message_body;

    /// 以固定种子生成 n 个节点的 (nodes, keypairs, peers)
    pub fn gen_nodes(n: u64) -> (Vec<NodeId>, Vec<Sm2KeyPair>, BTreeMap<NodeId, Sm2PublicKey>) {
        let mut nodes = Vec::new();
        let mut kps = Vec::new();
        let mut peers = BTreeMap::new();
        for i in 0..n {
            let id = i + 1;
            let mut seed = [0u8; 32];
            seed[0] = id as u8;
            seed[1] = 0xC0;
            let mut rng = CsRng::from_seed(&seed);
            let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen");
            peers.insert(id, kp.public_key);
            nodes.push(id);
            kps.push(kp);
        }
        (nodes, kps, peers)
    }

    /// 构建 n 引擎 + 已注册总线
    pub fn build_cluster(n: u64, timeout_ms: u64) -> (Vec<ConsensusEngine>, MockConsensusBus) {
        let (nodes, kps, peers) = gen_nodes(n);
        let mut engines = Vec::new();
        for (idx, kp) in kps.into_iter().enumerate() {
            let rng = CsRng::from_seed(&{
                let mut s = [0u8; 32];
                s[0] = idx as u8 + 1;
                s[1] = 0xEE;
                s
            });
            engines.push(
                ConsensusEngine::new(
                    nodes.clone(),
                    nodes[idx],
                    kp,
                    peers.clone(),
                    rng,
                    timeout_ms,
                )
                .expect("engine new"),
            );
        }
        let mut bus = MockConsensusBus::new();
        for id in &nodes {
            bus.register(*id);
        }
        (engines, bus)
    }

    /// 用指定密钥对签名构造消息
    #[allow(clippy::too_many_arguments)] // 测试辅助：参数与 PbftMessage 字段一一对应，聚合结构反而晦涩
    pub fn make_msg(
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
        msg_type: MsgType,
        view: u64,
        seq: u64,
        digest: [u8; 32],
        payload: Vec<u8>,
        sender: NodeId,
    ) -> PbftMessage {
        let body = message_body(msg_type, view, seq, &digest, sender, &payload);
        let signature = crate::pbft::sign_message(kp, &body, rng);
        PbftMessage {
            msg_type,
            view,
            sequence: seq,
            digest,
            payload,
            sender,
            signature,
        }
    }

    /// 循环驱动所有非隔离引擎 poll 直至消息排空（忽略处理错误，错误消息已被弹出），
    /// 收集并返回全部新提交的 `(NodeId, ConsensusResult)`。
    pub fn drive(
        engines: &mut [ConsensusEngine],
        bus: &mut MockConsensusBus,
        now_ms: u64,
    ) -> Vec<(NodeId, ConsensusResult)> {
        let mut results = Vec::new();
        for _ in 0..64 {
            for e in engines.iter_mut() {
                if let Ok(rs) = e.poll(bus, now_ms) {
                    for r in rs {
                        results.push((e.local_id, r));
                    }
                }
            }
            if bus.queues.values().all(|q| q.is_empty()) {
                break;
            }
        }
        results
    }
}

// ============================================================
// Unit Tests TC1~TC12
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::testutil::*;

    // TC1: ConsensusState 派生语义
    #[test]
    fn tc1_consensus_state_derives() {
        let s = ConsensusState::PrePrepare;
        let s2 = s; // Copy
        assert_eq!(s, s2);
        assert_eq!(s.clone(), ConsensusState::PrePrepare);
        assert_ne!(ConsensusState::Idle, ConsensusState::Done);
        let _dbg = alloc::format!("{:?}", ConsensusState::Commit);
    }

    // TC2: MsgType to_u8/from_u8 往返 + 非法值
    #[test]
    fn tc2_msg_type_u8_roundtrip() {
        for m in [
            MsgType::PrePrepare,
            MsgType::Prepare,
            MsgType::Commit,
            MsgType::Reply,
            MsgType::ViewChange,
        ] {
            assert_eq!(MsgType::from_u8(m.to_u8()), Some(m));
        }
        assert_eq!(MsgType::from_u8(5), None);
        assert_eq!(MsgType::from_u8(255), None);
    }

    // TC3: MsgType 含 ViewChange 变体（D4）
    #[test]
    fn tc3_msg_type_has_view_change() {
        assert_eq!(MsgType::ViewChange.to_u8(), 4);
        assert_eq!(MsgType::PrePrepare.to_u8(), 0);
    }

    // TC4: PbftMessage 派生语义
    #[test]
    fn tc4_pbft_message_derives() {
        let m = PbftMessage {
            msg_type: MsgType::Prepare,
            view: 1,
            sequence: 2,
            digest: [3u8; 32],
            payload: alloc::vec![1, 2, 3],
            sender: 4,
            signature: [5u8; 64],
        };
        let m2 = m.clone();
        assert_eq!(m, m2);
        let _dbg = alloc::format!("{:?}", m);
    }

    // TC5: LogEntry 访问器 prepare_count/commit_count
    #[test]
    fn tc5_log_entry_accessors() {
        let mut e = LogEntry {
            sequence: 1,
            request: alloc::vec![9],
            digest: [0u8; 32],
            prepare_voters: BTreeSet::new(),
            commit_voters: BTreeSet::new(),
            prepared: false,
            committed: false,
            executed: false,
            submitted_ms: 0,
        };
        assert_eq!(e.prepare_count(), 0);
        assert_eq!(e.commit_count(), 0);
        e.prepare_voters.insert(1);
        e.prepare_voters.insert(2);
        e.commit_voters.insert(3);
        assert_eq!(e.prepare_count(), 2);
        assert_eq!(e.commit_count(), 1);
    }

    // TC6: voter BTreeSet 去重（D5）
    #[test]
    fn tc6_voter_dedup() {
        let mut voters = BTreeSet::new();
        voters.insert(7u64);
        voters.insert(7u64);
        voters.insert(7u64);
        assert_eq!(voters.len(), 1);
    }

    // TC7: Mock 总线投递与 FIFO 顺序
    #[test]
    fn tc7_mock_bus_deliver_fifo() {
        let (_, kps, _) = gen_nodes(2);
        let mut rng = CsRng::from_seed(&[9u8; 32]);
        let mut bus = MockConsensusBus::new();
        bus.register(1);
        bus.register(2);
        let m1 = make_msg(
            &kps[0],
            &mut rng,
            MsgType::Prepare,
            0,
            1,
            [1u8; 32],
            alloc::vec![],
            1,
        );
        let m2 = make_msg(
            &kps[0],
            &mut rng,
            MsgType::Prepare,
            0,
            2,
            [2u8; 32],
            alloc::vec![],
            1,
        );
        bus.broadcast(1, &m1).expect("b1");
        bus.broadcast(1, &m2).expect("b2");
        // 两个节点邮箱各 2 条，FIFO 弹出队首
        let (f1, g1) = bus.receive(2).expect("r1");
        assert_eq!((f1, g1.sequence), (1, 1));
        let (_, g2) = bus.receive(2).expect("r2");
        assert_eq!(g2.sequence, 2);
        assert!(bus.receive(2).is_none());
        assert!(bus.receive(1).is_some());
    }

    // TC8: Mock 隔离节点不投不收
    #[test]
    fn tc8_mock_bus_isolation() {
        let (_, kps, _) = gen_nodes(2);
        let mut rng = CsRng::from_seed(&[8u8; 32]);
        let mut bus = MockConsensusBus::new();
        bus.register(1);
        bus.register(2);
        bus.isolated.insert(2);
        let m = make_msg(
            &kps[0],
            &mut rng,
            MsgType::Prepare,
            0,
            1,
            [1u8; 32],
            alloc::vec![],
            1,
        );
        bus.broadcast(1, &m).expect("b");
        assert!(bus.receive(2).is_none()); // 隔离不收
        assert!(bus.receive(1).is_some()); // 节点1 正常收
                                           // 隔离节点投出被吞
        bus.broadcast(2, &m).expect("b2");
        assert!(bus.receive(1).is_none());
    }

    // TC9: Mock fail_times 故障注入
    #[test]
    fn tc9_mock_bus_fail_times() {
        let (_, kps, _) = gen_nodes(1);
        let mut rng = CsRng::from_seed(&[7u8; 32]);
        let mut bus = MockConsensusBus::new();
        bus.register(1);
        bus.fail_times = 2;
        let m = make_msg(
            &kps[0],
            &mut rng,
            MsgType::Commit,
            0,
            1,
            [1u8; 32],
            alloc::vec![],
            1,
        );
        assert_eq!(bus.broadcast(1, &m), Err(ConsensusError::BusError));
        assert_eq!(bus.broadcast(1, &m), Err(ConsensusError::BusError));
        assert!(bus.broadcast(1, &m).is_ok());
    }

    // TC10: new 校验：空 nodes / 重复 nodes / 不含 local_id
    #[test]
    fn tc10_new_validation_errors() {
        let (nodes, kps, peers) = gen_nodes(4);
        let rng = || CsRng::from_seed(&[1u8; 32]);
        // 空 nodes
        assert_eq!(
            ConsensusEngine::new(alloc::vec![], 1, kps[0].clone(), peers.clone(), rng(), 3000)
                .err(),
            Some(ConsensusError::NotEnoughNodes)
        );
        // 重复 nodes
        assert_eq!(
            ConsensusEngine::new(
                alloc::vec![1, 1, 2],
                1,
                kps[0].clone(),
                peers.clone(),
                rng(),
                3000
            )
            .err(),
            Some(ConsensusError::NotEnoughNodes)
        );
        // nodes 不含 local_id
        assert_eq!(
            ConsensusEngine::new(
                nodes.clone(),
                99,
                kps[0].clone(),
                peers.clone(),
                rng(),
                3000
            )
            .err(),
            Some(ConsensusError::UnknownNode)
        );
    }

    // TC11: new 校验：peers 缺节点公钥 → UnknownNode
    #[test]
    fn tc11_new_peers_missing() {
        let (nodes, kps, mut peers) = gen_nodes(4);
        peers.remove(&3);
        assert_eq!(
            ConsensusEngine::new(
                nodes,
                1,
                kps[0].clone(),
                peers,
                CsRng::from_seed(&[1u8; 32]),
                3000
            )
            .err(),
            Some(ConsensusError::UnknownNode)
        );
    }

    // TC12: is_primary 视图轮换（nodes 排序后取模）
    #[test]
    fn tc12_is_primary_rotation() {
        let (mut engines, _bus) = build_cluster(4, 3000);
        // view 0：nodes[0] = 1 为主
        assert!(engines[0].is_primary());
        assert!(!engines[1].is_primary());
        // view 1：nodes[1] = 2 为主
        engines[1].view = 1;
        assert!(engines[1].is_primary());
        engines[0].view = 1;
        assert!(!engines[0].is_primary());
        // view 4 回到 nodes[0]
        engines[0].view = 4;
        assert!(engines[0].is_primary());
    }
}
