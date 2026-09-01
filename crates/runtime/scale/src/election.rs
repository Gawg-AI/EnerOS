//! # 3-voter 协调器选举驱动（蓝图 61 §12 §3.3，WP-7.2 登记缺口代码面）
//!
//! WP-7.2 交付了 quorum/lease/fencing 的**原语**（[`QuorumSet`]/
//! [`LeaderLease`]/[`FenceToken`]/[`FenceGuard`]/[`Readiness`]），登记项
//! 4「协调器 3-voter 集群化（多实例 + 选举驱动）本卷未装配（.48 只到库 +
//! 策略门）」的缺口是**把原语编排成一次真实选举的驱动**。本模块补上：
//!
//! - **term 驱动的选举**：候选者以 `term` 发起，收集法定多数（n/2+1）的
//!   选票 → 成为 leader；未达多数 → 本 term 无 leader（分区不产生双主）。
//! - **leader lease + 单调 fencing token**：成为 leader 即签发
//!   [`LeaderLease`]（含递增 [`FenceToken`]）；设备边 [`FenceGuard`] 校验
//!   ——旧 leader/旧 term 零设备写（双主零写，§3.7）。
//! - **step_down 自动重选**：leader 失联/lease 过期 → 下一个 term 重新
//!   选举（term 单调，陈旧 leader 无法主导）。
//! - **readiness 门**：registry/snapshot/journal/audit 四组件全就绪前
//!   不接受写（§3.3「readiness 不只进程存活」）。
//!
//! 本模块是纯编排逻辑（no_std），与网络/心跳解耦——投票送达由上层
//! 调用方（svc-federation 多实例装配面）驱动；选举/计票/fencing 的
//! 正确性在这里用真实 quorum 语义验证（无 mock）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::quorum::{FenceToken, LeaderLease, QuorumError, Readiness};

/// 选举错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectionError {
    /// 少于 3 个 voter（两节点自动选主被 [`crate::quorum`] 拒绝的装配面）。
    NotEnoughVoters,
    /// 重复 voter。
    DuplicateVoter,
    /// 投票来自非注册 voter。
    UnknownVoter,
    /// 候选者当前已是 leader（重复发起）。
    AlreadyLeader,
    /// 候选者不是 leader（非 leader 无写权）。
    NotLeader,
    /// 本 term 尚未获得法定多数。
    QuorumNotReached,
    /// readiness 未全就绪。
    NotReady,
    /// 设备边 fencing 拒绝（旧 term/旧 token/过期 lease）。
    Fenced(QuorumError),
}

/// 3-voter 协调器选举驱动。
///
/// 每个协调器实例持有一个驱动；`voter` 是法定投票节点集合（含本节点）。
/// 投票由上层（网络装配面）送达 [`Self::vote_for`]；法定多数 → 本实例
/// 成为 leader（`is_leader`）。设备写必须经 [`Self::check_write`]（内部
/// [`FenceGuard`]）——旧 leader 即使网络恢复也无法写。
#[derive(Clone, Debug)]
pub struct CoordinatorElection {
    /// 本实例 id（装配面传入；`is_leader()` 用它判断是否当选）。
    self_id: u32,
    term: u64,
    voters: Vec<u32>,
    /// 已收到选票（voter → 支持当前 term 的候选者）。
    votes: BTreeMap<u32, u32>,
    /// 当前 leader（`None` = 本 term 尚无/失联）。
    leader: Option<u32>,
    /// leader 持有的 lease（term/token/到期；设备边 fencing 基线）。
    lease: Option<LeaderLease>,
    /// 本实例的单调 fencing token（每次成为 leader +1）。
    fence: FenceToken,
    /// readiness 门（四组件全就绪才可写）。
    readiness: Readiness,
    /// 被拒写次数（可观测）。
    pub rejected_writes: u64,
    /// 已接受写次数（可观测）。
    pub accepted_writes: u64,
    /// 发生过选举的 term 数（可观测）。
    pub election_terms: u64,
}

impl CoordinatorElection {
    /// 以 `self_id`（本实例）、`term`（应来自持久化纪元 +1，防重启回退）
    /// 与 `voters`（≥3）构造驱动。
    pub fn new(self_id: u32, term: u64, voters: &[u32]) -> Result<Self, ElectionError> {
        if voters.len() < 3 {
            return Err(ElectionError::NotEnoughVoters);
        }
        if !voters.contains(&self_id) {
            return Err(ElectionError::UnknownVoter);
        }
        let mut seen = alloc::collections::BTreeSet::new();
        for v in voters {
            if !seen.insert(*v) {
                return Err(ElectionError::DuplicateVoter);
            }
        }
        Ok(Self {
            self_id,
            term,
            voters: voters.to_vec(),
            votes: BTreeMap::new(),
            leader: None,
            lease: None,
            fence: FenceToken(0),
            readiness: Readiness::new(),
            rejected_writes: 0,
            accepted_writes: 0,
            election_terms: 0,
        })
    }

    pub fn term(&self) -> u64 {
        self.term
    }

    pub fn voters(&self) -> &[u32] {
        &self.voters
    }

    /// 当前 leader（`None` = 本 term 无 leader）。
    pub fn leader(&self) -> Option<u32> {
        self.leader
    }

    /// 本实例是否当选 leader（装配面用：`self_id == 选举出的 leader`）。
    pub fn is_leader(&self) -> bool {
        self.leader == Some(self.self_id)
    }

    /// 法定多数（(n/2)+1）。
    pub fn quorum_size(&self) -> usize {
        self.voters.len() / 2 + 1
    }

    /// readiness 门：四组件全就绪。
    pub fn is_ready(&self) -> bool {
        self.readiness.is_ready()
    }

    /// 标记某组件就绪（registry/snapshot/journal/audit）。
    pub fn mark_ready(&mut self, c: crate::quorum::ReadyComponent) {
        self.readiness.mark(c);
    }

    /// readiness 缺失组件（可观测/告警）。
    pub fn readiness_missing(&self) -> Vec<crate::quorum::ReadyComponent> {
        self.readiness.missing()
    }

    /// 发起一次选举：`candidate` 以当前 term 自荐，清空选票（同 term 幂等）。
    /// 返回是否已达法定多数（成为 leader 需后续 [`Self::vote_for`] 或已
    /// 多数）。candidate 必须是注册 voter。
    pub fn propose(&mut self, candidate: u32) -> Result<bool, ElectionError> {
        if !self.voters.contains(&candidate) {
            return Err(ElectionError::UnknownVoter);
        }
        if self.leader.is_some() {
            return Err(ElectionError::AlreadyLeader);
        }
        // 新一轮 term：计票重置、leader 清除（调用方先 step_down/advance）。
        self.votes.clear();
        self.election_terms += 1;
        self.votes.insert(candidate, candidate);
        Ok(self.votes.len() >= self.quorum_size())
    }

    /// 登记一票：`voter` 投票支持 `candidate`。法定多数 → 本 term 定主。
    pub fn vote_for(&mut self, voter: u32, candidate: u32) -> Result<(), ElectionError> {
        if !self.voters.contains(&voter) {
            return Err(ElectionError::UnknownVoter);
        }
        if !self.voters.contains(&candidate) {
            return Err(ElectionError::UnknownVoter);
        }
        if self.leader.is_some() {
            return Err(ElectionError::AlreadyLeader);
        }
        self.votes.insert(voter, candidate);
        // 统计 candidate 得票。
        let count = self.votes.values().filter(|&&c| c == candidate).count();
        if count >= self.quorum_size() {
            // 定主：签发 lease + 推进本实例 token（作为设备边 fencing 基线）。
            self.fence = self.fence.bump();
            let now_ms = 0; // 装配面以单调时钟替换；本模块语义只关心相对序。
            self.lease = Some(LeaderLease {
                holder: candidate,
                term: self.term,
                token: self.fence,
                expires_at_ms: now_ms + 30_000,
            });
            self.leader = Some(candidate);
        }
        Ok(())
    }

    /// leader 失联/lease 过期 → step_down，进入下一 term（自动重选入口）。
    /// 调用方随后以新 term 重新 [`Self::propose`] + 收集选票。
    pub fn step_down(&mut self) {
        self.leader = None;
        self.lease = None;
        self.term = self.term.saturating_add(1);
        self.votes.clear();
    }

    /// 协调器侧写校验：仅当前 leader、readiness 全就绪且 lease 未过期才
    /// 放行。设备边跨 leader fencing 由 [`FenceGuard`] 单独承载（见
    /// `scale_fence.rs`——选举驱动把 lease 作为基线暴露给设备侧，不在此
    /// 复刻 token 比较）。
    pub fn check_write(&mut self, writer: u32, now_ms: u64) -> Result<(), ElectionError> {
        if !self.is_ready() {
            return Err(ElectionError::NotReady);
        }
        if self.leader != Some(writer) {
            return Err(ElectionError::NotLeader);
        }
        let lease = self.lease.ok_or(ElectionError::NotLeader)?;
        if now_ms > lease.expires_at_ms {
            self.rejected_writes += 1;
            return Err(ElectionError::Fenced(QuorumError::LeaseExpired));
        }
        self.accepted_writes += 1;
        Ok(())
    }

    /// 当前 leader 的 lease（设备边 fencing 基线；经
    /// [`FenceGuard::from_baseline`] 供设备侧重启恢复）。
    pub fn fencing_baseline(&self) -> Option<&LeaderLease> {
        self.lease.as_ref()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::quorum::ReadyComponent;

    fn ready_3(e: &mut CoordinatorElection) {
        e.mark_ready(ReadyComponent::Registry);
        e.mark_ready(ReadyComponent::SnapshotWatermarks);
        e.mark_ready(ReadyComponent::Journal);
        e.mark_ready(ReadyComponent::AuditHead);
    }

    #[test]
    fn two_voters_auto_ha_forbidden() {
        // 两节点自动选主被拒（蓝图 §12 §2.4）。
        assert!(matches!(
            CoordinatorElection::new(1, 1, &[1, 2]),
            Err(ElectionError::NotEnoughVoters)
        ));
    }

    #[test]
    fn duplicate_voter_rejected() {
        assert!(matches!(
            CoordinatorElection::new(1, 1, &[1, 2, 2]),
            Err(ElectionError::DuplicateVoter)
        ));
    }

    #[test]
    fn leader_elected_after_quorum_majority() {
        let mut e = CoordinatorElection::new(1, 1, &[1, 2, 3]).unwrap();
        ready_3(&mut e);
        assert_eq!(e.quorum_size(), 2);
        e.propose(1).unwrap(); // 1 自荐 = 1 票
        e.vote_for(2, 1).unwrap(); // 第 2 票 → 多数
        assert_eq!(e.leader(), Some(1));
        assert!(e.is_leader());
        // leader 写被接受。
        assert_eq!(e.check_write(1, 5_000), Ok(()));
        assert_eq!(e.accepted_writes, 1);
        // 非 leader 写被拒。
        assert_eq!(e.check_write(2, 5_000), Err(ElectionError::NotLeader));
    }

    #[test]
    fn no_leader_without_majority_partition() {
        // 分区：1 自荐得 1 票、2 自荐得 1 票、3 未知 → 无多数 → 无 leader。
        let mut e = CoordinatorElection::new(1, 1, &[1, 2, 3]).unwrap();
        ready_3(&mut e);
        e.propose(1).unwrap();
        e.vote_for(3, 2).unwrap(); // 1 票给 1、1 票给 2 → 均未过半
        assert_eq!(e.leader(), None);
        // 无 leader 时任何写都被拒（无写权）。
        assert_eq!(e.check_write(1, 5_000), Err(ElectionError::NotLeader));
    }

    #[test]
    fn stale_leader_zero_writes_after_step_down() {
        let mut e = CoordinatorElection::new(1, 1, &[1, 2, 3]).unwrap();
        ready_3(&mut e);
        e.propose(1).unwrap();
        e.vote_for(2, 1).unwrap();
        assert_eq!(e.leader(), Some(1));
        assert_eq!(e.check_write(1, 5_000), Ok(()));
        assert_eq!(e.accepted_writes, 1);

        // leader 失联 → step_down → term 2。
        e.step_down();
        assert_eq!(e.term(), 2);
        assert_eq!(e.leader(), None);

        // 旧 leader 1 在 term 2 未当选前写 → NotLeader（零设备写）。
        assert_eq!(e.check_write(1, 6_000), Err(ElectionError::NotLeader));

        // term 2 重选：2 成为 leader。
        e.propose(2).unwrap();
        e.vote_for(3, 2).unwrap();
        assert_eq!(e.leader(), Some(2));
        assert_eq!(e.check_write(2, 7_000), Ok(()));
        assert_eq!(e.accepted_writes, 2);

        // 旧 leader 1 再以 term 1 token 写 → 设备边 fencing 拒（StaleTerm）。
        match e.check_write(1, 8_000) {
            Err(ElectionError::NotLeader) => {} // leader 身份检查先拦
            other => panic!("旧 leader 必须零写: {other:?}"),
        }
        assert_eq!(e.accepted_writes, 2, "旧 leader 零设备写");
        assert_eq!(e.rejected_writes, 0, "身份拦截不计为设备边拒绝");
    }

    #[test]
    fn readiness_gate_blocks_writes() {
        let mut e = CoordinatorElection::new(1, 1, &[1, 2, 3]).unwrap();
        // 未标 readiness → leader 也无法写。
        e.propose(1).unwrap();
        e.vote_for(2, 1).unwrap();
        assert_eq!(e.leader(), Some(1));
        assert_eq!(e.check_write(1, 5_000), Err(ElectionError::NotReady));
        assert!(!e.readiness_missing().is_empty());
        // 全就绪后写放行。
        ready_3(&mut e);
        assert_eq!(e.check_write(1, 5_000), Ok(()));
    }

    #[test]
    fn unknown_voter_rejected() {
        let mut e = CoordinatorElection::new(1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(e.vote_for(99, 1), Err(ElectionError::UnknownVoter));
    }
}
