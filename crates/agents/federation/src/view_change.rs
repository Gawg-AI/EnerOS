//! v0.99.0 联邦共识协议：ViewChange 视图切换（蓝图 §4.4 / §6.5）。
//!
//! 提供 `ConsensusEngine` 的视图切换扩展：超时检测（`check_timeout`，指数退避
//! 防 ViewChange 风暴）、ViewChange 投票收集（`on_view_change`，按目标视图分桶
//! 去重）与进入新视图（`enter_view`，新主重发 PrePrepare 恢复共识）。
//!
//! ## 设计要点
//!
//! - **指数退避**（D8，蓝图 §8.5 坑点"ViewChange 风暴"对策）：有效超时 =
//!   `timeout_ms << min(consecutive_vc, 3)`，连续 VC 越多等待越久，封顶 8 倍。
//! - **无独立 NewView 消息**（D8）：VC 全网广播，诚实节点收齐 quorum 后自主
//!   `enter_view(new_view)` 自然收敛同一视图，消除 NewView 伪造面。
//! - **新主恢复**（D8）：`enter_view` 时若本节点为新主且日志尾部存在未 committed
//!   条目，以原 sequence/digest/request 重发 PrePrepare，接续被中断的共识。
//! - **VC 票集独立**（EX2）：`vc_votes: BTreeMap<u64, BTreeSet<NodeId>>` 按目标
//!   视图分桶，BTreeSet 天然防拜占庭重复投票；进入新视图后清理不高于该视图的票。
//! - **VC 消息复用同一帧**（D4）：`msg_type=ViewChange`，`view` 承载目标视图，
//!   `sequence=0`，digest 全零占位（无请求语义），签名域分离同 D9。

use alloc::vec::Vec;

use crate::consensus::{
    ConsensusBus, ConsensusEngine, ConsensusError, ConsensusState, MsgType, NodeId, PbftMessage,
};
use crate::pbft::{message_body, primary_of, quorum, sign_message};

impl ConsensusEngine {
    /// 有效超时（毫秒）：`timeout_ms << min(consecutive_vc, 3)`（D8 指数退避）
    pub(crate) fn effective_timeout_ms(&self) -> u64 {
        self.timeout_ms << core::cmp::min(self.consecutive_vc, 3)
    }

    /// 超时检测：必要时发起 ViewChange。
    ///
    /// - state ∈ {Idle, Done}（无进行中共识）→ `Ok(false)`
    /// - `now − last_progress ≤ 有效超时`（边界存活，严格大于才触发）→ `Ok(false)`
    /// - 否则广播 ViewChange（`view = self.view + 1`，sequence=0，签名）→
    ///   `view_change_count += 1`、`consecutive_vc += 1` → `Ok(true)`
    ///
    /// EX9 防风暴：发起 VC 时 `last_progress_ms = now_ms` 重启退避计时。
    /// 否则到达退避封顶（8x）后每次调用都触发 VC，违背 §8.5 防 ViewChange
    /// 风暴目的；重启后 VC 间隔为 1x/2x/4x/8x/8x…（频率有界）。
    pub fn check_timeout(
        &mut self,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<bool, ConsensusError> {
        if matches!(self.state, ConsensusState::Idle | ConsensusState::Done) {
            return Ok(false);
        }
        if now_ms.saturating_sub(self.last_progress_ms) <= self.effective_timeout_ms() {
            return Ok(false);
        }
        let target_view = self.view + 1;
        let digest = [0u8; 32]; // VC 无请求语义，digest 占位
        let body = message_body(
            MsgType::ViewChange,
            target_view,
            0,
            &digest,
            self.local_id,
            &[],
        );
        let signature = sign_message(&self.kp, &body, &mut self.rng);
        let msg = PbftMessage {
            msg_type: MsgType::ViewChange,
            view: target_view,
            sequence: 0,
            digest,
            payload: Vec::new(),
            sender: self.local_id,
            signature,
        };
        bus.broadcast(self.local_id, &msg)?;
        self.view_change_count += 1;
        self.consecutive_vc += 1;
        self.last_progress_ms = now_ms; // EX9 重启退避计时
        Ok(true)
    }

    /// 处理 ViewChange 投票。
    ///
    /// - `msg.view ≤ self.view`（陈旧）→ 忽略 `Ok(false)`
    /// - 否则按目标视图收集投票（BTreeSet 去重），达 quorum → `enter_view` → `Ok(true)`
    pub(crate) fn on_view_change(
        &mut self,
        _from: NodeId,
        msg: PbftMessage,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<bool, ConsensusError> {
        if msg.view <= self.view {
            return Ok(false);
        }
        let n = self.nodes.len();
        let reached = {
            let votes = self.vc_votes.entry(msg.view).or_default();
            votes.insert(msg.sender);
            votes.len() >= quorum(n)
        };
        if reached {
            self.enter_view(msg.view, bus, now_ms)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// 进入新视图：重置状态机；新主对尾部未提交日志重发 PrePrepare（D8 恢复）。
    ///
    /// - `view = new_view`、`state = Idle`、`consecutive_vc = 0`、
    ///   `last_progress_ms = now_ms`
    /// - 清理不高于 `new_view` 的 VC 票（防陈旧票污染未来视图）
    /// - 若本节点为 `new_view` 主节点且日志尾部有未 committed 条目 → 以原
    ///   sequence/digest/request 重发 PrePrepare（备份侧经 EX6 重置投票集接续共识）
    pub fn enter_view(
        &mut self,
        new_view: u64,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<(), ConsensusError> {
        self.view = new_view;
        self.state = ConsensusState::Idle;
        self.consecutive_vc = 0;
        self.last_progress_ms = now_ms;
        self.vc_votes.retain(|&v, _| v > new_view);
        if primary_of(&self.nodes, new_view) == self.local_id {
            // 取最靠后的未提交条目（日志尾部）
            let pending = self
                .log
                .iter()
                .rev()
                .find(|e| !e.committed)
                .map(|e| (e.sequence, e.digest, e.request.clone()));
            if let Some((seq, digest, request)) = pending {
                let body = message_body(
                    MsgType::PrePrepare,
                    new_view,
                    seq,
                    &digest,
                    self.local_id,
                    &request,
                );
                let signature = sign_message(&self.kp, &body, &mut self.rng);
                let msg = PbftMessage {
                    msg_type: MsgType::PrePrepare,
                    view: new_view,
                    sequence: seq,
                    digest,
                    payload: request,
                    sender: self.local_id,
                    signature,
                };
                bus.broadcast(self.local_id, &msg)?;
            }
        }
        Ok(())
    }
}

// ============================================================
// Unit Tests TV29~TV40
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::vec;

    use eneros_crypto::{sm3_hash, CsRng};

    use super::*;
    use crate::consensus::testutil::*;
    use crate::consensus::LogEntry;

    // TV29: Idle 状态 check_timeout 不触发（无进行中共识）
    #[test]
    fn tv29_idle_no_view_change() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        assert_eq!(engines[0].state, ConsensusState::Idle);
        assert_eq!(engines[0].check_timeout(&mut bus, 10_000), Ok(false));
        assert!(bus.queues.values().all(|q| q.is_empty()));
        assert_eq!(engines[0].view_change_count, 0);
        assert_eq!(engines[0].consecutive_vc, 0);
    }

    // TV30: Done 状态 check_timeout 不触发（共识已完成）
    #[test]
    fn tv30_done_no_view_change() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        engines[0]
            .do_submit(b"x".to_vec(), &mut bus, 0)
            .expect("submit");
        drive(&mut engines, &mut bus, 100);
        assert_eq!(engines[0].state, ConsensusState::Done);
        assert_eq!(engines[0].check_timeout(&mut bus, 100_000), Ok(false));
        assert_eq!(engines[0].view_change_count, 0);
    }

    // TV31: 未超时不触发（边界：now − last_progress == 有效超时 → false）
    #[test]
    fn tv31_not_timed_out() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // submit 后 state=PrePrepare、last_progress=0
        engines[0]
            .do_submit(b"x".to_vec(), &mut bus, 0)
            .expect("submit");
        assert_eq!(engines[0].check_timeout(&mut bus, 1000), Ok(false));
        assert_eq!(engines[0].view_change_count, 0);
        assert!(bus.queues.values().all(|q| q.len() == 1)); // 仅 PP，无 VC
    }

    // TV32: 超时触发 VC 广播：消息字段 + 签名有效 + 计数器
    #[test]
    fn tv32_timeout_broadcasts_view_change() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        engines[0]
            .do_submit(b"x".to_vec(), &mut bus, 0)
            .expect("submit");
        assert_eq!(engines[0].check_timeout(&mut bus, 1001), Ok(true));
        assert_eq!(engines[0].view_change_count, 1);
        assert_eq!(engines[0].consecutive_vc, 1);
        // 节点2 邮箱：先 PP 后 VC（FIFO）
        let (_, pp) = bus.receive(2).expect("pp");
        assert_eq!(pp.msg_type, MsgType::PrePrepare);
        let (_, vc) = bus.receive(2).expect("vc");
        assert_eq!(vc.msg_type, MsgType::ViewChange);
        assert_eq!(vc.view, 1); // self.view + 1
        assert_eq!(vc.sequence, 0);
        assert_eq!(vc.sender, 1);
        // VC 签名有效（D9 域分离，digest 全零占位）
        let body = message_body(MsgType::ViewChange, 1, 0, &[0u8; 32], 1, &[]);
        assert!(crate::pbft::verify_message(
            &engines[0].kp.public_key,
            &body,
            &vc.signature
        ));
    }

    // TV33: 指数退避（间隔 1x/2x/4x/8x/8x 封顶，EX9 计时重启，防 ViewChange 风暴）
    #[test]
    fn tv33_exponential_backoff() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        engines[0]
            .do_submit(b"x".to_vec(), &mut bus, 0)
            .expect("submit");
        assert_eq!(engines[0].effective_timeout_ms(), 1000);
        // 第 1 次 VC @1001（elapsed 1001 > 1000）：consec=1，计时重启（EX9）
        assert_eq!(engines[0].check_timeout(&mut bus, 1001), Ok(true));
        assert_eq!(engines[0].effective_timeout_ms(), 2000);
        assert_eq!(engines[0].check_timeout(&mut bus, 3001), Ok(false)); // 3001−1001=2000 边界
                                                                         // 第 2 次 VC @3002：consec=2（间隔 2000）
        assert_eq!(engines[0].check_timeout(&mut bus, 3002), Ok(true));
        assert_eq!(engines[0].effective_timeout_ms(), 4000);
        assert_eq!(engines[0].check_timeout(&mut bus, 7002), Ok(false)); // 7002−3002=4000
                                                                         // 第 3 次 VC @7003：consec=3（间隔 4000）
        assert_eq!(engines[0].check_timeout(&mut bus, 7003), Ok(true));
        assert_eq!(engines[0].effective_timeout_ms(), 8000);
        assert_eq!(engines[0].check_timeout(&mut bus, 15_003), Ok(false)); // 15003−7003=8000
                                                                           // 第 4 次 VC @15004：consec=4，eff 仍 8000（min(4,3)=3 封顶，否则应为 16000）
        assert_eq!(engines[0].check_timeout(&mut bus, 15_004), Ok(true));
        assert_eq!(engines[0].effective_timeout_ms(), 8000);
        // 封顶生效：间隔仍为 8000（若未封顶需 16000）→ VC 频率有界不风暴
        assert_eq!(engines[0].check_timeout(&mut bus, 23_004), Ok(false)); // 23004−15004=8000
        assert_eq!(engines[0].check_timeout(&mut bus, 23_005), Ok(true));
        assert_eq!(engines[0].consecutive_vc, 5);
        assert_eq!(engines[0].view_change_count, 5);
    }

    // TV34: 陈旧 VC 忽略（msg.view ≤ self.view，不计票）
    #[test]
    fn tv34_stale_view_change_ignored() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        engines[0].view = 2;
        let kp2 = engines[1].kp.clone();
        let mut rng = CsRng::from_seed(&[11u8; 32]);
        let vc = make_msg(
            &kp2,
            &mut rng,
            MsgType::ViewChange,
            2,
            0,
            [0u8; 32],
            vec![],
            2,
        );
        assert_eq!(engines[0].on_view_change(2, vc, &mut bus, 0), Ok(false));
        assert!(!engines[0].vc_votes.contains_key(&2));
        assert_eq!(engines[0].view, 2);
    }

    // TV35: VC 未达 quorum 不切换视图
    #[test]
    fn tv35_vc_below_quorum_no_enter() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let mut rng = CsRng::from_seed(&[12u8; 32]);
        // quorum(4)=3，仅收集节点2/3 两票
        for idx in 1usize..=2 {
            let kp = engines[idx].kp.clone();
            let sender = idx as u64 + 1;
            let vc = make_msg(
                &kp,
                &mut rng,
                MsgType::ViewChange,
                1,
                0,
                [0u8; 32],
                vec![],
                sender,
            );
            assert_eq!(
                engines[0].on_view_change(sender, vc, &mut bus, 0),
                Ok(false)
            );
        }
        assert_eq!(engines[0].vc_votes[&1].len(), 2);
        assert_eq!(engines[0].view, 0);
        assert_eq!(engines[0].state, ConsensusState::Idle);
    }

    // TV36: VC 达 quorum → enter_view 重置语义 + 票集清理
    #[test]
    fn tv36_vc_quorum_enters_view() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let mut rng = CsRng::from_seed(&[13u8; 32]);
        engines[0].state = ConsensusState::PrePrepare;
        engines[0].consecutive_vc = 2;
        let mut last = false;
        // 节点2/3/4 三票达 quorum(4)=3
        for idx in 1usize..=3 {
            let kp = engines[idx].kp.clone();
            let sender = idx as u64 + 1;
            let vc = make_msg(
                &kp,
                &mut rng,
                MsgType::ViewChange,
                1,
                0,
                [0u8; 32],
                vec![],
                sender,
            );
            last = engines[0]
                .on_view_change(sender, vc, &mut bus, 500)
                .expect("vc ok");
        }
        assert!(last);
        assert_eq!(engines[0].view, 1);
        assert_eq!(engines[0].state, ConsensusState::Idle);
        assert_eq!(engines[0].consecutive_vc, 0);
        assert_eq!(engines[0].last_progress_ms, 500);
        assert!(engines[0].vc_votes.is_empty()); // ≤1 的票已清理
    }

    // TV37: VC 投票去重（同一节点重复 VC 仅计一票，防拜占庭刷票）
    #[test]
    fn tv37_vc_vote_dedup() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let kp2 = engines[1].kp.clone();
        let mut rng = CsRng::from_seed(&[14u8; 32]);
        for _ in 0..3 {
            let vc = make_msg(
                &kp2,
                &mut rng,
                MsgType::ViewChange,
                1,
                0,
                [0u8; 32],
                vec![],
                2,
            );
            assert_eq!(engines[0].on_view_change(2, vc, &mut bus, 0), Ok(false));
        }
        assert_eq!(engines[0].vc_votes[&1].len(), 1);
        assert_eq!(engines[0].view, 0);
    }

    // TV38: 主节点离线全链路 ViewChange 恢复（蓝图 §6.5 故障注入）
    #[test]
    fn tv38_primary_offline_view_change_recovery() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // 1. 主节点1 submit：PrePrepare 正常投递到备份 2/3/4
        engines[0]
            .do_submit(b"plan-vc".to_vec(), &mut bus, 0)
            .expect("submit");
        // 2. 主节点离线 + 网络分区：备份受理 PP 但 Prepare 广播全部失败，
        //    共识停滞（EX8：节点仍保持 Prepare 态，可触发 VC）
        bus.isolated.insert(1);
        bus.fail_times = 1000;
        for e in engines[1..].iter_mut() {
            let _ = e.poll(&mut bus, 10);
            assert_eq!(e.state, ConsensusState::Prepare);
            assert!(!e.is_committed(1));
        }
        bus.fail_times = 0; // 分区恢复
                            // 3. 超时后备份广播 ViewChange(view=1)
        for e in engines[1..].iter_mut() {
            assert_eq!(e.check_timeout(&mut bus, 1011), Ok(true));
        }
        for e in engines[1..].iter() {
            assert_eq!(e.view_change_count, 1);
            assert_eq!(e.consecutive_vc, 1);
        }
        // 4. 3 备份互收 VC 达 quorum(3) → enter_view(1)；
        //    新主节点2 对未提交日志重发 PrePrepare（D8）→ 恢复共识
        if let Some(q) = bus.queues.get_mut(&1) {
            q.clear(); // 清空主节点自投的残留 PP，保证 drive 收敛判定
        }
        let results = drive(&mut engines, &mut bus, 1100);
        // 5. 3 诚实节点在 view=1 提交原请求；离线主节点未提交
        for e in engines[1..].iter() {
            assert!(e.is_committed(1));
            assert_eq!(e.view, 1);
            assert_eq!(e.committed_count, 1);
            assert_eq!(e.consecutive_vc, 0); // enter_view 重置退避
            assert_eq!(e.state, ConsensusState::Done);
        }
        assert!(!engines[0].is_committed(1));
        assert_eq!(results.len(), 3);
    }

    // TV39: 新主重发 PrePrepare 恢复原 sequence/digest/request（D8）
    #[test]
    fn tv39_new_primary_resends_pre_prepare() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        let request = b"recover-me".to_vec();
        let digest = sm3_hash(&request);
        // 给节点2（view=1 的新主）植入未提交日志 seq=5
        engines[1].log.push(LogEntry {
            sequence: 5,
            request: request.clone(),
            digest,
            prepare_voters: BTreeSet::new(),
            commit_voters: BTreeSet::new(),
            prepared: false,
            committed: false,
            executed: false,
            submitted_ms: 0,
        });
        engines[1].enter_view(1, &mut bus, 700).expect("enter");
        assert_eq!(engines[1].view, 1);
        assert_eq!(engines[1].state, ConsensusState::Idle);
        // 各节点收到重发的 PrePrepare：同 seq/digest/payload，sender=2，view=1
        let mut found = false;
        for id in 1..=4u64 {
            while let Some((_f, m)) = bus.receive(id) {
                assert_eq!(m.msg_type, MsgType::PrePrepare);
                assert_eq!(m.view, 1);
                assert_eq!(m.sequence, 5);
                assert_eq!(m.digest, digest);
                assert_eq!(m.payload, request);
                assert_eq!(m.sender, 2);
                found = true;
            }
        }
        assert!(found);
    }

    // TV40: 非新主 / 无未提交日志 → enter_view 不重发 PrePrepare
    #[test]
    fn tv40_enter_view_no_resend() {
        let (mut engines, mut bus) = build_cluster(4, 1000);
        // 节点1 进入 view=1（主为节点2）：非新主不重发
        engines[0].enter_view(1, &mut bus, 100).expect("enter");
        assert!(bus.queues.values().all(|q| q.is_empty()));
        // 节点2 是新主但日志全部已 committed：不重发
        let request = b"done".to_vec();
        let digest = sm3_hash(&request);
        let mut pv = BTreeSet::new();
        pv.insert(1u64);
        engines[1].log.push(LogEntry {
            sequence: 3,
            request,
            digest,
            prepare_voters: pv,
            commit_voters: BTreeSet::new(),
            prepared: true,
            committed: true,
            executed: true,
            submitted_ms: 0,
        });
        engines[1].enter_view(1, &mut bus, 100).expect("enter");
        assert!(bus.queues.values().all(|q| q.is_empty()));
    }
}
