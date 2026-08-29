//! 蓝图 61 v0.314.24（61.24-a）：`ExecutionJournal`——执行状态与幂等真值
//! 的唯一持有者。
//!
//! 唯一不变量：`Committed == AppliedConfirmed == 独立设备回读在容差内匹配
//! 已授权命令`。gateway 是纯裁决面（授权终点 = `Authorized`）；执行事实
//! 只能由本 journal 按严格写序推进：
//!
//! `authorize → Authorized → Persisted → Sent → TransportAcked →
//! AppliedConfirmed`（终态：`AppliedConfirmed`/`Rejected`/`Failed`）。
//!
//! 每次状态转换必须命中 expected-previous，禁止跳跃与倒退。同 key 行为
//! （蓝图 05 §2.3）：
//!
//! - digest 不同 → `JournalHit::Conflict`（零投递拒绝）；
//! - Authorized/Persisted/Sent/TransportAcked → 返回当前状态，不新投递；
//! - AppliedConfirmed → 返回原 receipt；
//! - Rejected/Failed → 返回原终态；
//! - Indeterminate/Reconciling → 对账，禁止盲发。
//!
//! 内存有界（BTreeMap + cap；满时逐出最旧终态，无终态可逐 → `Full`，
//! fail-closed 不伪造）。持久化经由 `IntentStore`（v2 编码，见
//! `durable_intent`）；本模块只管状态机与同 key 语义。

use alloc::collections::BTreeMap;

/// 执行状态（蓝图 05 §2.3 九态）。gateway 裁决终点是 `Authorized`；
/// `AppliedConfirmed` 是唯一可映射为外部 `Committed` 的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    /// 授权通过（尚未持久化意图）。
    Authorized,
    /// durable intent 已写入（投递未开始）。
    Persisted,
    /// 投递已返回 Ok（写可能已发生）。
    Sent,
    /// 对端回执帧已解码（ACK 层确认）。
    TransportAcked,
    /// 独立回读在容差内匹配（唯一成功终态）。
    AppliedConfirmed,
    /// 被拒（零投递/对端拒绝执行）。
    Rejected,
    /// 失败（不可达/写失败/读回不符）。
    Failed,
    /// 不确定（投递后超时/回执不可解——不猜成败）。
    Indeterminate,
    /// 对账中（重启恢复/对端幂等命中——原结果未知，禁止投递）。
    Reconciling,
}

impl ExecutionStatus {
    /// 是否终态（终态不再转换；重放返回原终态）。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ExecutionStatus::AppliedConfirmed | ExecutionStatus::Rejected | ExecutionStatus::Failed
        )
    }

    /// 审计/串口标签。
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionStatus::Authorized => "authorized",
            ExecutionStatus::Persisted => "persisted",
            ExecutionStatus::Sent => "sent",
            ExecutionStatus::TransportAcked => "transport-acked",
            ExecutionStatus::AppliedConfirmed => "applied-confirmed",
            ExecutionStatus::Rejected => "rejected",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Indeterminate => "indeterminate",
            ExecutionStatus::Reconciling => "reconciling",
        }
    }
}

/// 一条幂等执行记录。
///
/// `command_digest` 为 `[0; 32]` 时表示旧版记录未携带摘要（无法比对内容
/// ——同 key 重放按 in-flight/对账处理，永不重投、永不升级成功）。
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// 幂等键（= correlation；意图 (protocol,seq) 派生 / 计划 plan_id 派生）。
    pub key: [u8; 16],
    /// 命令 ID（重建原 receipt 用；Plan 路径与 key 不同源）。
    pub command_id: [u8; 16],
    /// 命令内容摘要（61 v0.314.24：SM3(device‖point‖命令类‖power)；
    /// 同 key 不同内容 → 冲突。0.314.25 由 canonical 命令摘要接替）。
    pub command_digest: [u8; 32],
    /// 当前执行状态。
    pub status: ExecutionStatus,
    /// 投递尝试次数。
    pub attempt: u32,
    /// 最近转换时刻（单调毫秒）。
    pub updated_at_ms: u64,
    /// 原 receipt 摘要（AppliedConfirmed 时携带——对端回执帧摘要）。
    pub receipt_digest: Option<[u8; 32]>,
}

/// 同 key 查询裁决。
#[derive(Debug, Clone)]
pub enum JournalHit {
    /// 同 key 不同 command_digest → 拒绝（零投递）。
    Conflict,
    /// 同 key 同 digest → 返回当前记录（调用方按状态映射回执）。
    Current(IdempotencyRecord),
}

/// journal 错误（调用方语义：授权阶段失败 = 零投递 fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    /// 有界容量满且无终态可逐出。
    Full,
    /// 非法状态转换（跳跃/倒退/重复 authorize）。
    InvalidTransition,
}

/// 执行 journal（内存状态机；有界）。
pub struct ExecutionJournal {
    entries: BTreeMap<[u8; 16], IdempotencyRecord>,
    cap: usize,
}

/// 合法转换边（expected → allowed next）。
fn transition_allowed(from: ExecutionStatus, to: ExecutionStatus) -> bool {
    use ExecutionStatus::*;
    matches!(
        (from, to),
        (Authorized, Persisted)
            | (Authorized, Failed)
            | (Persisted, Sent)
            | (Persisted, Failed)
            | (Persisted, Indeterminate)
            | (Sent, TransportAcked)
            | (Sent, Indeterminate)
            | (Sent, Failed)
            | (TransportAcked, AppliedConfirmed)
            | (TransportAcked, Failed)
            | (TransportAcked, Rejected)
            | (TransportAcked, Indeterminate)
            | (TransportAcked, Reconciling)
            | (Indeterminate, Reconciling)
    )
}

impl ExecutionJournal {
    /// 创建（容量上限；与 durable 槽位同阶）。
    pub fn new(cap: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            cap,
        }
    }

    /// 同 key 查询：冲突 / 当前记录 / 无记录。
    pub fn lookup(&self, key: &[u8; 16], digest: &[u8; 32]) -> Option<JournalHit> {
        let rec = self.entries.get(key)?;
        // 旧版记录无 digest（[0;32] 哨兵）：无法比对内容——按当前状态
        // 返回（in-flight 永不重投），不做冲突裁决。
        if rec.command_digest != [0u8; 32] && rec.command_digest != *digest {
            return Some(JournalHit::Conflict);
        }
        Some(JournalHit::Current(rec.clone()))
    }

    /// 登记授权（chain Executed 后、任何投递动作前）。同 key 已存在 →
    /// `InvalidTransition`（fail-closed，零投递）。
    pub fn authorize(
        &mut self,
        key: [u8; 16],
        command_id: [u8; 16],
        command_digest: [u8; 32],
        now_ms: u64,
    ) -> Result<(), JournalError> {
        if self.entries.contains_key(&key) {
            return Err(JournalError::InvalidTransition);
        }
        if self.entries.len() >= self.cap && !self.evict_oldest_terminal() {
            return Err(JournalError::Full);
        }
        self.entries.insert(
            key,
            IdempotencyRecord {
                key,
                command_id,
                command_digest,
                status: ExecutionStatus::Authorized,
                attempt: 1,
                updated_at_ms: now_ms,
                receipt_digest: None,
            },
        );
        Ok(())
    }

    /// 状态转换（expected-previous 强制；非法跳跃/倒退拒绝）。
    pub fn transition(
        &mut self,
        key: &[u8; 16],
        expect: ExecutionStatus,
        to: ExecutionStatus,
        now_ms: u64,
        receipt_digest: Option<[u8; 32]>,
    ) -> Result<(), JournalError> {
        let Some(rec) = self.entries.get_mut(key) else {
            return Err(JournalError::InvalidTransition);
        };
        if rec.status != expect || !transition_allowed(rec.status, to) {
            return Err(JournalError::InvalidTransition);
        }
        rec.status = to;
        rec.updated_at_ms = now_ms;
        if receipt_digest.is_some() {
            rec.receipt_digest = receipt_digest;
        }
        Ok(())
    }

    /// 播种对账态（启动恢复/对端幂等命中且 journal 无记录）。已存在 → false。
    pub fn seed_reconciling(
        &mut self,
        key: [u8; 16],
        command_id: [u8; 16],
        command_digest: [u8; 32],
        now_ms: u64,
    ) -> bool {
        if self.entries.contains_key(&key) {
            return false;
        }
        if self.entries.len() >= self.cap && !self.evict_oldest_terminal() {
            return false;
        }
        self.entries.insert(
            key,
            IdempotencyRecord {
                key,
                command_id,
                command_digest,
                status: ExecutionStatus::Reconciling,
                attempt: 0,
                updated_at_ms: now_ms,
                receipt_digest: None,
            },
        );
        true
    }

    /// 非终态记录（对账输入；有界快照）。
    pub fn pending(&self) -> alloc::vec::Vec<IdempotencyRecord> {
        self.entries
            .values()
            .filter(|r| !r.status.is_terminal())
            .cloned()
            .collect()
    }

    /// 记录数（观测）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 逐出最旧终态记录（有界化；无终态 → false）。
    fn evict_oldest_terminal(&mut self) -> bool {
        let victim = self
            .entries
            .iter()
            .filter(|(_, r)| r.status.is_terminal())
            .min_by_key(|(_, r)| (r.updated_at_ms, r.key))
            .map(|(k, _)| *k);
        victim.is_some_and(|k| self.entries.remove(&k).is_some())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    const K: [u8; 16] = [1; 16];
    const D: [u8; 32] = [2; 32];

    #[test]
    fn authorize_then_full_chain_to_applied_confirmed() {
        let mut j = ExecutionJournal::new(8);
        j.authorize(K, [9; 16], D, 100).expect("authorize");
        j.transition(
            &K,
            ExecutionStatus::Authorized,
            ExecutionStatus::Persisted,
            110,
            None,
        )
        .expect("persisted");
        j.transition(
            &K,
            ExecutionStatus::Persisted,
            ExecutionStatus::Sent,
            120,
            None,
        )
        .expect("sent");
        j.transition(
            &K,
            ExecutionStatus::Sent,
            ExecutionStatus::TransportAcked,
            130,
            None,
        )
        .expect("acked");
        j.transition(
            &K,
            ExecutionStatus::TransportAcked,
            ExecutionStatus::AppliedConfirmed,
            140,
            Some([3; 32]),
        )
        .expect("confirmed");
        let rec = match j.lookup(&K, &D) {
            Some(JournalHit::Current(r)) => r,
            other => panic!("{other:?}"),
        };
        assert_eq!(rec.status, ExecutionStatus::AppliedConfirmed);
        assert_eq!(rec.receipt_digest, Some([3; 32]));
        assert!(rec.status.is_terminal());
    }

    #[test]
    fn illegal_jump_and_regression_rejected() {
        let mut j = ExecutionJournal::new(8);
        j.authorize(K, [9; 16], D, 100).expect("authorize");
        // 跳跃：Authorized → Sent（必须先 Persisted）。
        assert_eq!(
            j.transition(
                &K,
                ExecutionStatus::Authorized,
                ExecutionStatus::Sent,
                110,
                None
            ),
            Err(JournalError::InvalidTransition)
        );
        j.transition(
            &K,
            ExecutionStatus::Authorized,
            ExecutionStatus::Persisted,
            110,
            None,
        )
        .expect("persisted");
        // 倒退：Persisted → Authorized。
        assert_eq!(
            j.transition(
                &K,
                ExecutionStatus::Persisted,
                ExecutionStatus::Authorized,
                120,
                None
            ),
            Err(JournalError::InvalidTransition)
        );
        // 终态封口：AppliedConfirmed 不可再转换（先走到终态）。
        j.transition(
            &K,
            ExecutionStatus::Persisted,
            ExecutionStatus::Sent,
            120,
            None,
        )
        .expect("sent");
        j.transition(
            &K,
            ExecutionStatus::Sent,
            ExecutionStatus::TransportAcked,
            130,
            None,
        )
        .expect("acked");
        j.transition(
            &K,
            ExecutionStatus::TransportAcked,
            ExecutionStatus::AppliedConfirmed,
            140,
            None,
        )
        .expect("confirmed");
        assert_eq!(
            j.transition(
                &K,
                ExecutionStatus::AppliedConfirmed,
                ExecutionStatus::Failed,
                150,
                None
            ),
            Err(JournalError::InvalidTransition)
        );
    }

    #[test]
    fn same_key_different_digest_is_conflict() {
        let mut j = ExecutionJournal::new(8);
        j.authorize(K, [9; 16], D, 100).expect("authorize");
        let other_digest = [7u8; 32];
        assert!(matches!(
            j.lookup(&K, &other_digest),
            Some(JournalHit::Conflict)
        ));
    }

    #[test]
    fn legacy_record_without_digest_never_conflicts() {
        let mut j = ExecutionJournal::new(8);
        j.seed_reconciling(K, [9; 16], [0u8; 32], 100);
        // 旧版无 digest：同 key 任意内容 → 返回当前记录（对账），不冲突。
        assert!(matches!(
            j.lookup(&K, &[8u8; 32]),
            Some(JournalHit::Current(_))
        ));
    }

    #[test]
    fn bounded_full_without_terminal_is_fail_closed() {
        let mut j = ExecutionJournal::new(1);
        j.authorize([1; 16], [9; 16], D, 100).expect("first");
        // 第一条 in-flight 不可逐出 → 第二条拒绝（Full）。
        assert_eq!(
            j.authorize([2; 16], [9; 16], D, 110),
            Err(JournalError::Full)
        );
        // 终态后可逐出最旧终态腾位。
        j.transition(
            &[1; 16],
            ExecutionStatus::Authorized,
            ExecutionStatus::Failed,
            120,
            None,
        )
        .expect("failed");
        j.authorize([2; 16], [9; 16], D, 130).expect("evict+insert");
        assert_eq!(j.len(), 1);
    }

    #[test]
    fn pending_lists_non_terminal_only() {
        let mut j = ExecutionJournal::new(8);
        j.authorize([1; 16], [9; 16], D, 100).expect("a");
        j.seed_reconciling([2; 16], [9; 16], D, 100);
        j.authorize([3; 16], [9; 16], D, 100).expect("b");
        j.transition(
            &[3; 16],
            ExecutionStatus::Authorized,
            ExecutionStatus::Failed,
            110,
            None,
        )
        .expect("f");
        let pend = j.pending();
        assert_eq!(
            pend.len(),
            2,
            "Authorized + Reconciling 未决；Failed 终态不在列"
        );
    }

    #[test]
    fn transition_on_missing_key_rejected() {
        let mut j = ExecutionJournal::new(8);
        assert_eq!(
            j.transition(
                &K,
                ExecutionStatus::Authorized,
                ExecutionStatus::Persisted,
                100,
                None
            ),
            Err(JournalError::InvalidTransition)
        );
    }
}
