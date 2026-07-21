//! v0.99.0 联邦共识协议：PBFT 三阶段（PrePrepare / Prepare / Commit）。
//!
//! 提供法定人数数学（`f` / `quorum` / `primary_of`）、SM2 域分离签名辅助
//! （`message_body` / `sign_message` / `verify_message`）与 `ConsensusEngine`
//! 的三阶段处理扩展（`do_submit` / `on_pre_prepare` / `on_prepare` / `on_commit`）。
//!
//! ## 设计要点
//!
//! - **法定人数**（D7）：`f(n) = (n-1)/3`，`quorum(n) = 2f+1`；
//!   主节点的 PrePrepare 计入 prepare 票（PBFT 经典变体优化），
//!   n=4 时主票 + 2 备份 Prepare 即 prepared，容忍 1 备份静默/作恶。
//! - **签名域分离**（D9）：`msg_body = type:u8‖view:u64be‖seq:u64be‖digest‖sender:u64be(‖payload)`，
//!   复用 eneros-crypto 既有 `sm2_sign` / `sm2_verify`（§5.5 防重复造轮子）。
//! - **拜占庭防护**：digest ≠ SM3(payload) 的 PrePrepare 拒绝；Prepare/Commit 的
//!   digest 与本地日志不符拒绝（防 equivocation 跨摘要计票）；BTreeSet 投票去重。
//! - **恢复重置语义**（EX6）：对未 committed 的既有日志条目收到新视图 PrePrepare 时，
//!   重置投票集为 {主节点} 并重新广播 Prepare（已 committed 条目视为终态忽略）。
//! - **状态迁移先于投票广播**（EX8 活性）：广播失败（BusError）时节点仍保持
//!   Prepare 态，`check_timeout` 可触发 ViewChange 恢复，避免永久停滞 Idle。

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use eneros_crypto::{
    sm2_sign, sm2_verify, sm3_hash, CsRng, Sm2KeyPair, Sm2PublicKey, Sm2Signature,
};

use crate::consensus::{
    ConsensusBus, ConsensusEngine, ConsensusError, ConsensusResult, ConsensusState, LogEntry,
    MsgType, NodeId, PbftMessage,
};

/// 容错上限：f(n) = (n-1)/3（n ≥ 1）
pub fn f(n: usize) -> usize {
    (n - 1) / 3
}

/// 法定人数：quorum(n) = 2f+1
pub fn quorum(n: usize) -> usize {
    2 * f(n) + 1
}

/// 视图 view 的主节点（nodes 升序取模轮换）
pub fn primary_of(nodes: &[NodeId], view: u64) -> NodeId {
    nodes[(view % nodes.len() as u64) as usize]
}

/// D9 域分离签名消息体：`type:u8‖view:u64be‖seq:u64be‖digest‖sender:u64be(‖payload)`
pub fn message_body(
    msg_type: MsgType,
    view: u64,
    sequence: u64,
    digest: &[u8; 32],
    sender: NodeId,
    payload: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(57 + payload.len());
    body.push(msg_type.to_u8());
    body.extend_from_slice(&view.to_be_bytes());
    body.extend_from_slice(&sequence.to_be_bytes());
    body.extend_from_slice(digest);
    body.extend_from_slice(&sender.to_be_bytes());
    body.extend_from_slice(payload);
    body
}

/// SM2 签名并返回 64 字节（r‖s）。
///
/// 失败关闭（fail-closed）：`sm2_sign` 仅在密钥非法时失败（密钥对由
/// `Sm2KeyPair::generate` 产生，实际不可达），失败返回全零签名，
/// 验签必然失败，不会误放行。
pub fn sign_message(kp: &Sm2KeyPair, msg_body: &[u8], rng: &mut CsRng) -> [u8; 64] {
    match sm2_sign(msg_body, &kp.private_key, &kp.public_key, rng) {
        Ok(sig) => sig.to_bytes(),
        Err(_) => [0u8; 64],
    }
}

/// SM2 验签：签名非法或验证未通过均返回 false（不暴露内部错误细节）
pub fn verify_message(pk: &Sm2PublicKey, msg_body: &[u8], sig: &[u8; 64]) -> bool {
    let signature = Sm2Signature::from_bytes(sig);
    matches!(sm2_verify(msg_body, &signature, pk), Ok(true))
}

impl ConsensusEngine {
    /// 主节点提交路径（D3 sync）：广播 PrePrepare 后返回分配的 seq。
    pub(crate) fn do_submit(
        &mut self,
        request: Vec<u8>,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<u64, ConsensusError> {
        if !self.is_primary() {
            return Err(ConsensusError::NotPrimary);
        }
        // 多 outstanding 安全：取日志最大 seq 与已提交 seq 的较大者 +1
        let max_log_seq = self.log.iter().map(|e| e.sequence).max().unwrap_or(0);
        let seq = core::cmp::max(self.sequence, max_log_seq) + 1;
        let digest = sm3_hash(&request);
        let body = message_body(
            MsgType::PrePrepare,
            self.view,
            seq,
            &digest,
            self.local_id,
            &request,
        );
        let signature = sign_message(&self.kp, &body, &mut self.rng);
        let msg = PbftMessage {
            msg_type: MsgType::PrePrepare,
            view: self.view,
            sequence: seq,
            digest,
            payload: request.clone(),
            sender: self.local_id,
            signature,
        };
        bus.broadcast(self.local_id, &msg)?;
        let mut prepare_voters = BTreeSet::new();
        prepare_voters.insert(self.local_id); // D7 主节点 PrePrepare 计票
        self.log.push(LogEntry {
            sequence: seq,
            request,
            digest,
            prepare_voters,
            commit_voters: BTreeSet::new(),
            prepared: false,
            committed: false,
            executed: false,
            submitted_ms: now_ms,
        });
        self.state = ConsensusState::PrePrepare;
        self.submit_count += 1;
        self.last_progress_ms = now_ms;
        Ok(seq)
    }

    /// 广播 Prepare/Commit 投票（payload 为空，digest 标识请求）
    pub(crate) fn broadcast_vote(
        &mut self,
        msg_type: MsgType,
        sequence: u64,
        digest: [u8; 32],
        bus: &mut dyn ConsensusBus,
    ) -> Result<(), ConsensusError> {
        let body = message_body(msg_type, self.view, sequence, &digest, self.local_id, &[]);
        let signature = sign_message(&self.kp, &body, &mut self.rng);
        let msg = PbftMessage {
            msg_type,
            view: self.view,
            sequence,
            digest,
            payload: Vec::new(),
            sender: self.local_id,
            signature,
        };
        bus.broadcast(self.local_id, &msg)
    }

    /// 备份节点处理 PrePrepare。
    ///
    /// - sender 非该 view 主节点 → rejected_count+=1 + `Err(ViewMismatch)`
    /// - digest ≠ SM3(payload) → rejected_count+=1 + `Err(StaleMessage)`
    /// - seq 已有 committed 条目 → 忽略 `Ok(None)`
    /// - seq 已有未 committed 条目 → EX6 恢复重置（voters={主}，重广播 Prepare）
    /// - 新 seq → 建条目（prepare_voters={主}）+ 广播 Prepare + state=Prepare
    pub(crate) fn on_pre_prepare(
        &mut self,
        _from: NodeId,
        msg: PbftMessage,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<Option<ConsensusResult>, ConsensusError> {
        let primary = primary_of(&self.nodes, msg.view);
        if msg.sender != primary {
            self.rejected_count += 1;
            return Err(ConsensusError::ViewMismatch);
        }
        if msg.digest != sm3_hash(&msg.payload) {
            self.rejected_count += 1;
            return Err(ConsensusError::StaleMessage);
        }
        // 已存在条目：committed 终态忽略；未 committed 走恢复重置
        if let Some(pos) = self.log.iter().position(|e| e.sequence == msg.sequence) {
            if self.log[pos].committed {
                return Ok(None);
            }
            let entry = &mut self.log[pos];
            entry.request = msg.payload.clone();
            entry.digest = msg.digest;
            entry.prepare_voters.clear();
            entry.prepare_voters.insert(primary);
            entry.commit_voters.clear();
            entry.prepared = false;
            // submitted_ms 保留原受理时刻（D12 延迟口径：请求受理 → 提交）
        } else {
            let mut prepare_voters = BTreeSet::new();
            prepare_voters.insert(primary);
            self.log.push(LogEntry {
                sequence: msg.sequence,
                request: msg.payload.clone(),
                digest: msg.digest,
                prepare_voters,
                commit_voters: BTreeSet::new(),
                prepared: false,
                committed: false,
                executed: false,
                submitted_ms: now_ms,
            });
        }
        // EX8 活性：状态迁移先于投票广播——广播失败（BusError）时节点仍记录
        // "已接受 PrePrepare"（state=Prepare），check_timeout 可触发 ViewChange
        // 恢复；否则故障注入场景永久停滞 Idle 无法自愈。
        self.state = ConsensusState::Prepare;
        self.broadcast_vote(MsgType::Prepare, msg.sequence, msg.digest, bus)?;
        Ok(None)
    }

    /// 处理 Prepare 投票：去重累计，达 quorum 且未 prepared → 广播 Commit。
    pub(crate) fn on_prepare(
        &mut self,
        _from: NodeId,
        msg: PbftMessage,
        bus: &mut dyn ConsensusBus,
        _now_ms: u64,
    ) -> Result<Option<ConsensusResult>, ConsensusError> {
        let n = self.nodes.len();
        let local_id = self.local_id;
        let mut should_commit = false;
        {
            let entry = match self.find_entry(msg.sequence) {
                Some(e) => e,
                None => {
                    self.rejected_count += 1;
                    return Err(ConsensusError::StaleMessage);
                }
            };
            // 防 equivocation 跨摘要计票
            if entry.digest != msg.digest {
                self.rejected_count += 1;
                return Err(ConsensusError::StaleMessage);
            }
            entry.prepare_voters.insert(msg.sender);
            if entry.prepare_voters.len() >= quorum(n) && !entry.prepared {
                entry.prepared = true;
                entry.commit_voters.insert(local_id);
                should_commit = true;
            }
        }
        if should_commit {
            self.broadcast_vote(MsgType::Commit, msg.sequence, msg.digest, bus)?;
            self.state = ConsensusState::Commit;
        }
        Ok(None)
    }

    /// 处理 Commit 投票：去重累计，达 quorum 且未 committed → 提交并产出结果。
    pub(crate) fn on_commit(
        &mut self,
        _from: NodeId,
        msg: PbftMessage,
        now_ms: u64,
    ) -> Result<Option<ConsensusResult>, ConsensusError> {
        let n = self.nodes.len();
        let mut committed_now = false;
        let mut latency = 0u64;
        {
            let entry = match self.find_entry(msg.sequence) {
                Some(e) => e,
                None => {
                    self.rejected_count += 1;
                    return Err(ConsensusError::StaleMessage);
                }
            };
            if entry.digest != msg.digest {
                self.rejected_count += 1;
                return Err(ConsensusError::StaleMessage);
            }
            entry.commit_voters.insert(msg.sender);
            if entry.commit_voters.len() >= quorum(n) && !entry.committed {
                entry.committed = true;
                entry.executed = true;
                latency = now_ms.saturating_sub(entry.submitted_ms);
                committed_now = true;
            }
        }
        if committed_now {
            self.sequence = core::cmp::max(self.sequence, msg.sequence);
            self.state = ConsensusState::Done;
            self.committed_count += 1;
            self.last_latency_ms = latency;
            return Ok(Some(ConsensusResult {
                sequence: msg.sequence,
                digest: msg.digest,
                view: self.view,
            }));
        }
        Ok(None)
    }
}

// ============================================================
// Unit Tests TP13~TP28
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::consensus::testutil::*;

    // TP13: f(n) 数学
    #[test]
    fn tp13_f_math() {
        assert_eq!(f(1), 0);
        assert_eq!(f(3), 0);
        assert_eq!(f(4), 1);
        assert_eq!(f(6), 1);
        assert_eq!(f(7), 2);
        assert_eq!(f(10), 3);
    }

    // TP14: quorum(n) 数学
    #[test]
    fn tp14_quorum_math() {
        assert_eq!(quorum(1), 1);
        assert_eq!(quorum(4), 3);
        assert_eq!(quorum(7), 5);
        assert_eq!(quorum(10), 7);
    }

    // TP15: primary_of 轮换
    #[test]
    fn tp15_primary_of_rotation() {
        let nodes = [1u64, 2, 3, 4];
        assert_eq!(primary_of(&nodes, 0), 1);
        assert_eq!(primary_of(&nodes, 1), 2);
        assert_eq!(primary_of(&nodes, 3), 4);
        assert_eq!(primary_of(&nodes, 4), 1);
        assert_eq!(primary_of(&nodes, 5), 2);
    }

    // TP16: 签名/验签往返
    #[test]
    fn tp16_sign_verify_roundtrip() {
        let (_, kps, _) = gen_nodes(1);
        let mut rng = CsRng::from_seed(&[3u8; 32]);
        let body = message_body(MsgType::Prepare, 1, 2, &[7u8; 32], 1, &[]);
        let sig = sign_message(&kps[0], &body, &mut rng);
        assert!(verify_message(&kps[0].public_key, &body, &sig));
    }

    // TP17: 错密钥 / 篡改消息体验签失败
    #[test]
    fn tp17_verify_rejects_wrong_key_or_tamper() {
        let (_, kps, _) = gen_nodes(2);
        let mut rng = CsRng::from_seed(&[4u8; 32]);
        let body = message_body(MsgType::Commit, 0, 1, &[1u8; 32], 1, &[]);
        let sig = sign_message(&kps[0], &body, &mut rng);
        // 错公钥
        assert!(!verify_message(&kps[1].public_key, &body, &sig));
        // 篡改消息体
        let body2 = message_body(MsgType::Commit, 0, 2, &[1u8; 32], 1, &[]);
        assert!(!verify_message(&kps[0].public_key, &body2, &sig));
        // 全零签名
        assert!(!verify_message(&kps[0].public_key, &body, &[0u8; 64]));
    }

    // TP18: 非主节点 submit → NotPrimary
    #[test]
    fn tp18_non_primary_submit_rejected() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // view 0 主节点为 nodes[0]=1，engines[1]（节点2）非主
        assert_eq!(
            engines[1].do_submit(vec![1, 2, 3], &mut bus, 0).err(),
            Some(ConsensusError::NotPrimary)
        );
        assert_eq!(engines[1].submit_count, 0);
        assert!(engines[1].log.is_empty());
    }

    // TP19: 4 节点全链路共识达成
    #[test]
    fn tp19_four_node_consensus() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let request = b"feeder-capacity-alloc".to_vec();
        let seq = engines[0]
            .do_submit(request.clone(), &mut bus, 0)
            .expect("submit");
        assert_eq!(seq, 1);
        assert_eq!(engines[0].submit_count, 1);
        assert_eq!(engines[0].state, ConsensusState::PrePrepare);
        drive(&mut engines, &mut bus, 100);
        for e in &engines {
            assert!(e.is_committed(1));
            assert_eq!(e.sequence, 1);
            assert_eq!(e.state, ConsensusState::Done);
            assert_eq!(e.committed_count, 1);
        }
    }

    // TP20: 各节点日志 digest 与 SM3(request) 一致
    #[test]
    fn tp20_digest_consistency() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let request = b"dispatch-plan-42".to_vec();
        let expect_digest = sm3_hash(&request);
        engines[0].do_submit(request, &mut bus, 0).expect("submit");
        drive(&mut engines, &mut bus, 50);
        for e in &engines {
            let entry = e.log.iter().find(|x| x.sequence == 1).expect("entry");
            assert_eq!(entry.digest, expect_digest);
            assert!(entry.committed);
            assert!(entry.executed);
        }
    }

    // TP21: 主节点获得 ConsensusResult + 延迟观测（D12）
    #[test]
    fn tp21_consensus_result_and_latency() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let request = b"x".to_vec();
        let expect_digest = sm3_hash(&request);
        engines[0].do_submit(request, &mut bus, 0).expect("submit");
        let results = drive(&mut engines, &mut bus, 100);
        // 每个节点各产出 1 条结果
        assert_eq!(results.len(), 4);
        let r0 = results
            .iter()
            .find(|(id, _)| *id == 1)
            .expect("primary res");
        assert_eq!(r0.1.sequence, 1);
        assert_eq!(r0.1.digest, expect_digest);
        assert_eq!(r0.1.view, 0);
        // 主节点 submitted_ms=0，提交于 now=100 → 延迟 100
        assert_eq!(engines[0].last_latency_ms, 100);
    }

    // TP22: 1 备份被隔离仍达成（quorum=3 含主票，D7）
    #[test]
    fn tp22_one_backup_isolated_still_commits() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        bus.isolated.insert(4);
        engines[0]
            .do_submit(b"plan".to_vec(), &mut bus, 0)
            .expect("submit");
        drive(&mut engines, &mut bus, 100);
        for e in &engines[..3] {
            assert!(e.is_committed(1));
            assert_eq!(e.committed_count, 1);
        }
        // 被隔离节点未参与
        assert!(!engines[3].is_committed(1));
    }

    // TP23: 伪造签名 Prepare 拒绝（InvalidSignature + rejected_count）
    #[test]
    fn tp23_forged_signature_rejected() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let mut msg = PbftMessage {
            msg_type: MsgType::Prepare,
            view: 0,
            sequence: 1,
            digest: [1u8; 32],
            payload: Vec::new(),
            sender: 2,
            signature: [0xAAu8; 64],
        };
        let before = engines[0].rejected_count;
        assert_eq!(
            engines[0].handle_message(2, msg.clone(), &mut bus, 0).err(),
            Some(ConsensusError::InvalidSignature)
        );
        assert_eq!(engines[0].rejected_count, before + 1);
        // 篡改 payload 后原签名同样失效
        msg.payload = vec![9];
        assert_eq!(
            engines[0].handle_message(2, msg, &mut bus, 0).err(),
            Some(ConsensusError::InvalidSignature)
        );
    }

    // TP24: digest 与 payload 不符的 PrePrepare 拒绝（正确签名但假 digest）
    #[test]
    fn tp24_fake_digest_pre_prepare_rejected() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // 取节点1（主）密钥对伪造 digest 错误但签名有效的 PrePrepare
        let kp = engines[0].kp.clone();
        let mut rng = CsRng::from_seed(&[5u8; 32]);
        let bad_digest = [9u8; 32]; // != sm3(payload)
        let msg = make_msg(
            &kp,
            &mut rng,
            MsgType::PrePrepare,
            0,
            1,
            bad_digest,
            b"payload".to_vec(),
            1,
        );
        let before = engines[1].rejected_count;
        assert_eq!(
            engines[1].handle_message(1, msg, &mut bus, 0).err(),
            Some(ConsensusError::StaleMessage)
        );
        assert_eq!(engines[1].rejected_count, before + 1);
        assert!(engines[1].log.is_empty());
    }

    // TP25: 重复投票去重（同一 Prepare 二次处理不重复计票）
    #[test]
    fn tp25_duplicate_vote_dedup() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // 注入主节点 PrePrepare 让节点2 建条目
        let kp1 = engines[0].kp.clone();
        let mut rng = CsRng::from_seed(&[6u8; 32]);
        let payload = b"req".to_vec();
        let digest = sm3_hash(&payload);
        let pp = make_msg(
            &kp1,
            &mut rng,
            MsgType::PrePrepare,
            0,
            1,
            digest,
            payload,
            1,
        );
        engines[1]
            .handle_message(1, pp, &mut bus, 0)
            .expect("pp ok");
        // 节点3 的 Prepare 连续处理两次
        let kp3 = engines[2].kp.clone();
        let prep = make_msg(&kp3, &mut rng, MsgType::Prepare, 0, 1, digest, vec![], 3);
        engines[1]
            .handle_message(3, prep.clone(), &mut bus, 0)
            .expect("p1");
        engines[1].handle_message(3, prep, &mut bus, 0).expect("p2");
        let entry = engines[1].log.iter().find(|e| e.sequence == 1).expect("e");
        // voters = {1(主), 3}，重复不计
        assert_eq!(entry.prepare_count(), 2);
    }

    // TP26: 错误 view 消息拒绝（未来 view Prepare / 陈旧 view Prepare）
    #[test]
    fn tp26_wrong_view_rejected() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let kp2 = engines[1].kp.clone();
        let mut rng = CsRng::from_seed(&[7u8; 32]);
        let digest = [1u8; 32];
        // 未来 view=1 的 Prepare（非 VC 非 PrePrepare）→ StaleMessage
        let future = make_msg(&kp2, &mut rng, MsgType::Prepare, 1, 1, digest, vec![], 2);
        let before = engines[0].rejected_count;
        assert_eq!(
            engines[0].handle_message(2, future, &mut bus, 0).err(),
            Some(ConsensusError::StaleMessage)
        );
        assert_eq!(engines[0].rejected_count, before + 1);
        // 引擎进入 view=1 后收到 view=0 的 Prepare → StaleMessage
        engines[0].view = 1;
        let stale = make_msg(&kp2, &mut rng, MsgType::Prepare, 0, 1, digest, vec![], 2);
        assert_eq!(
            engines[0].handle_message(2, stale, &mut bus, 0).err(),
            Some(ConsensusError::StaleMessage)
        );
        // 非主节点发 PrePrepare（view=0 主为节点1，节点2 冒充）→ ViewMismatch
        engines[0].view = 0;
        let payload = b"q".to_vec();
        let d = sm3_hash(&payload);
        let fake_pp = make_msg(&kp2, &mut rng, MsgType::PrePrepare, 0, 9, d, payload, 2);
        assert_eq!(
            engines[0].handle_message(2, fake_pp, &mut bus, 0).err(),
            Some(ConsensusError::ViewMismatch)
        );
    }

    // TP27: equivocation 双 digest 不安全态不提交
    #[test]
    fn tp27_equivocation_no_commit() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let kp1 = engines[0].kp.clone();
        let mut rng = CsRng::from_seed(&[8u8; 32]);
        // 拜占庭主节点1 构造同 seq 两个不同 digest 的 PrePrepare
        let p1 = b"plan-A".to_vec();
        let p2 = b"plan-B".to_vec();
        let pp1 = make_msg(
            &kp1,
            &mut rng,
            MsgType::PrePrepare,
            0,
            1,
            sm3_hash(&p1),
            p1,
            1,
        );
        let pp2 = make_msg(
            &kp1,
            &mut rng,
            MsgType::PrePrepare,
            0,
            1,
            sm3_hash(&p2),
            p2,
            1,
        );
        // 节点1 随即被隔离（只发不出），向 2/4 投毒 pp1，向 3 投毒 pp2
        bus.isolated.insert(1);
        engines[1]
            .handle_message(1, pp1.clone(), &mut bus, 0)
            .expect("e2 pp1");
        engines[2]
            .handle_message(1, pp2, &mut bus, 0)
            .expect("e3 pp2");
        engines[3]
            .handle_message(1, pp1, &mut bus, 0)
            .expect("e4 pp1");
        drive(&mut engines, &mut bus, 100);
        // 任一 digest 均无法收齐 quorum 的 Commit（安全不提交）
        for e in &engines[1..] {
            assert!(!e.is_committed(1));
            assert_eq!(e.committed_count, 0);
        }
    }

    // TP28: 7 节点 f=2 容忍 2 备份静默仍共识
    #[test]
    fn tp28_seven_node_f2_consensus() {
        let (mut engines, mut bus) = build_cluster(7, 1000);
        assert_eq!(f(7), 2);
        assert_eq!(quorum(7), 5);
        bus.isolated.insert(6);
        bus.isolated.insert(7);
        engines[0]
            .do_submit(b"plan-7".to_vec(), &mut bus, 0)
            .expect("submit");
        drive(&mut engines, &mut bus, 100);
        for e in &engines[..5] {
            assert!(e.is_committed(1));
        }
        assert!(!engines[5].is_committed(1));
        assert!(!engines[6].is_committed(1));
    }
}
