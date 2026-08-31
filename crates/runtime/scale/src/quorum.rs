//! # 蓝图 61 §12 v0.314.48：quorum / lease / fencing（§3.3）
//!
//! - 控制元数据 3 voter quorum；leader lease 与**单调 fencing**；
//! - writer 在设备边校验 fencing——旧 leader 即使网络恢复也不能写
//!   （`tests/scale_fence.rs::stale_leader_zero_device_writes`）；
//! - readiness = registry/snapshot/journal/audit 恢复，不只是进程存活
//!   （[`Readiness`]）。

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// 法定投票节点（voter）错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuorumError {
    NotEnoughVoters,
    DuplicateVoter,
    StaleTerm,
    QuorumNotReached,
    LeaseExpired,
    FenceRejected,
    UnknownVoter,
}

/// 3 voter quorum 集合（voter 数必须 ≥3；两节点自动选主被
/// [`crate::leadership::LeadPolicy`] 拒绝，此处为运行期 enforce）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumSet {
    pub term: u64,
    voters: BTreeSet<u32>,
    /// 已收到 prepare 票（确定性集合去重）。
    prepared: BTreeSet<u32>,
    /// 已收到 commit 票。
    committed: BTreeSet<u32>,
}

impl QuorumSet {
    pub fn new(term: u64, voters: &[u32]) -> Result<Self, QuorumError> {
        if voters.len() < 3 {
            return Err(QuorumError::NotEnoughVoters);
        }
        let set: BTreeSet<u32> = voters.iter().copied().collect();
        if set.len() != voters.len() {
            return Err(QuorumError::DuplicateVoter);
        }
        Ok(Self {
            term,
            voters: set,
            prepared: BTreeSet::new(),
            committed: BTreeSet::new(),
        })
    }

    /// 法定人数（多数 = (n/2)+1）。
    pub fn quorum_size(&self) -> usize {
        self.voters.len() / 2 + 1
    }

    pub fn has_prepared(&self) -> bool {
        self.prepared.len() >= self.quorum_size()
    }

    pub fn has_committed(&self) -> bool {
        self.committed.len() >= self.quorum_size()
    }

    /// 预投票（去重；未知节点拒）。
    pub fn prepare(&mut self, voter: u32) -> Result<bool, QuorumError> {
        if !self.voters.contains(&voter) {
            return Err(QuorumError::UnknownVoter);
        }
        self.prepared.insert(voter);
        Ok(self.has_prepared())
    }

    /// 提交票（必须先 prepare 过）。
    pub fn commit(&mut self, voter: u32) -> Result<bool, QuorumError> {
        if !self.prepared.contains(&voter) {
            return Err(QuorumError::UnknownVoter);
        }
        self.committed.insert(voter);
        Ok(self.has_committed())
    }
}

/// 单调 fencing token（仅增；旧 token 一律拒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FenceToken(pub u64);

impl FenceToken {
    /// 原子递增并返回新 token（单调；改名 bump 避免与 Iterator::next 混淆）。
    pub fn bump(&mut self) -> Self {
        self.0 += 1;
        *self
    }
}

/// leader lease（持有者 + term + 到期）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaderLease {
    pub holder: u32,
    pub term: u64,
    pub token: FenceToken,
    pub expires_at_ms: u64,
}

/// 设备边 fencing 守卫：只认「更高 term 或同 term 更高 token」。
#[derive(Clone, Debug)]
pub struct FenceGuard {
    accepted: Option<LeaderLease>,
    /// 被拒写次数（可观测）。
    pub rejected_writes: u64,
    /// 已接受写次数。
    pub accepted_writes: u64,
}

impl FenceGuard {
    pub fn new() -> Self {
        Self {
            accepted: None,
            rejected_writes: 0,
            accepted_writes: 0,
        }
    }

    /// 从持久化基线恢复（设备侧重启后）：首写必须满足
    /// term ≥ 基线 term 且同 term 时 token > 基线 token——旧 leader
    /// 即使先恢复连接也无法先写（P1-4 修复）。
    pub fn from_baseline(baseline: LeaderLease) -> Self {
        Self {
            accepted: Some(baseline),
            rejected_writes: 0,
            accepted_writes: 0,
        }
    }

    /// 持久化基线（供上层 durable 落盘；重启后经 from_baseline 恢复）。
    pub fn baseline(&self) -> Option<&LeaderLease> {
        self.accepted.as_ref()
    }

    /// 校验一次写请求；旧 term / 旧 token → 拒（零设备写）。
    pub fn check_write(
        &mut self,
        holder: u32,
        term: u64,
        token: FenceToken,
        now_ms: u64,
    ) -> Result<(), QuorumError> {
        match &self.accepted {
            None => {
                self.accepted = Some(LeaderLease {
                    holder,
                    term,
                    token,
                    expires_at_ms: now_ms + 30_000,
                });
                self.accepted_writes += 1;
                Ok(())
            }
            Some(cur) => {
                if term < cur.term {
                    self.rejected_writes += 1;
                    return Err(QuorumError::StaleTerm);
                }
                // 更高 term → 无条件接管（旧租约过期不能阻止新 leader，
                // 否则 fencing 在超时场景失效——双主零写靠的是 term 单调）。
                if term > cur.term {
                    self.accepted = Some(LeaderLease {
                        holder,
                        term,
                        token,
                        expires_at_ms: now_ms + 30_000,
                    });
                    self.accepted_writes += 1;
                    return Ok(());
                }
                // 同 term：token 必须单调；租约过期后同 term 续约被拒。
                if token <= cur.token {
                    self.rejected_writes += 1;
                    return Err(QuorumError::FenceRejected);
                }
                if now_ms > cur.expires_at_ms {
                    self.rejected_writes += 1;
                    return Err(QuorumError::LeaseExpired);
                }
                self.accepted = Some(LeaderLease {
                    holder,
                    term,
                    token,
                    expires_at_ms: now_ms + 30_000,
                });
                self.accepted_writes += 1;
                Ok(())
            }
        }
    }

    pub fn accepted_lease(&self) -> Option<&LeaderLease> {
        self.accepted.as_ref()
    }
}

impl Default for FenceGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// readiness 组分（registry/snapshot watermark/journal/audit head）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadyComponent {
    Registry,
    SnapshotWatermarks,
    Journal,
    AuditHead,
}

/// readiness 门：全部组件就绪才可服务决策（不只进程存活）。
#[derive(Clone, Debug)]
pub struct Readiness {
    ready: BTreeSet<ReadyComponent>,
}

impl Readiness {
    pub fn new() -> Self {
        Self {
            ready: BTreeSet::new(),
        }
    }
    pub fn mark(&mut self, c: ReadyComponent) {
        self.ready.insert(c);
    }
    pub fn is_ready(&self) -> bool {
        self.ready.len() == 4
    }
    pub fn missing(&self) -> Vec<ReadyComponent> {
        [
            ReadyComponent::Registry,
            ReadyComponent::SnapshotWatermarks,
            ReadyComponent::Journal,
            ReadyComponent::AuditHead,
        ]
        .iter()
        .copied()
        .filter(|c| !self.ready.contains(c))
        .collect()
    }
}

impl Default for Readiness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn quorum_prepare_commit() {
        let mut q = QuorumSet::new(5, &[1, 2, 3, 4, 5]).expect("set");
        assert_eq!(q.quorum_size(), 3);
        assert!(!q.has_prepared());
        assert!(!q.prepare(1).expect("p"), "1 票未达 quorum");
        assert!(!q.prepare(2).expect("p"), "2 票未达 quorum");
        assert!(!q.has_prepared());
        assert!(q.prepare(3).expect("p"), "3 票达 quorum");
        assert!(q.has_prepared());
        // 重复票去重（仍 3 票）。
        assert!(q.prepare(3).expect("dup ok"));
        assert!(!q.commit(1).expect("c"), "1 commit 未达");
        assert!(!q.commit(2).expect("c"), "2 commit 未达");
        assert!(q.commit(3).expect("c"), "3 commit 达 quorum");
        assert!(q.has_committed());
    }

    #[test]
    fn quorum_rejects_unknown_and_few_voters() {
        let mut q = QuorumSet::new(5, &[1, 2, 3]).expect("set");
        assert_eq!(q.prepare(99), Err(QuorumError::UnknownVoter));
        assert_eq!(
            QuorumSet::new(5, &[1, 2]),
            Err(QuorumError::NotEnoughVoters)
        );
        assert_eq!(
            QuorumSet::new(5, &[1, 1, 2]),
            Err(QuorumError::DuplicateVoter)
        );
    }

    #[test]
    fn fence_rejects_stale_term_then_accepts_higher() {
        let mut g = FenceGuard::new();
        let mut tok = FenceToken(100);
        // 新 leader term 2 token 101 写。
        assert_eq!(g.check_write(7, 2, tok.bump(), 1_000), Ok(()));
        assert_eq!(g.accepted_writes, 1);
        // 旧 leader（term 1）→ 拒（旧 term）。
        assert_eq!(
            g.check_write(7, 1, FenceToken(999), 1_000),
            Err(QuorumError::StaleTerm)
        );
        assert_eq!(g.rejected_writes, 1);
        // 同 term 旧 token → 拒。
        assert_eq!(
            g.check_write(7, 2, FenceToken(100), 1_000),
            Err(QuorumError::FenceRejected)
        );
        assert_eq!(g.rejected_writes, 2);
        // 同 term 新 token → 接受。
        assert_eq!(g.check_write(7, 2, tok.bump(), 1_000), Ok(()));
        assert_eq!(g.accepted_writes, 2);
        // 租约过期后同 term 续约 → 拒（fencing 语义：过期即失效）。
        assert_eq!(
            g.check_write(7, 2, tok.bump(), 1_000 + 31_000),
            Err(QuorumError::LeaseExpired)
        );
        // 更高 term（另一 leader）即使旧租约过期仍可接管 → Ok。
        assert_eq!(g.check_write(9, 3, tok.bump(), 1_000 + 31_000), Ok(()));
        assert_eq!(g.accepted_writes, 3);
    }

    #[test]
    fn readiness_requires_all_components() {
        let mut r = Readiness::new();
        assert!(!r.is_ready());
        r.mark(ReadyComponent::Registry);
        r.mark(ReadyComponent::SnapshotWatermarks);
        assert_eq!(r.missing().len(), 2);
        r.mark(ReadyComponent::Journal);
        r.mark(ReadyComponent::AuditHead);
        assert!(r.is_ready());
        assert!(r.missing().is_empty());
    }
}
