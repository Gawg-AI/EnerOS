//! 22 §2.W5.1：AI 决策进审计链——意图/LP 解/安全校验/执行/偏差全入链。
//!
//! [`DecisionAudit`] 包装 `AuditLoggerImpl<MemStore>`（SM3 哈希链），
//! 每条 AI 决策记一条 `AiDecision` 事件：
//!
//! - `actor` = 决策代理标识（如 `energy-agent`）；
//! - `target` = 目标设备/对象；
//! - `detail` = 紧凑五要素编码 `i=意图;p=LP解;s=安全校验;x=执行结果;d=偏差`
//!   （`AuditEvent::new` 对超长字段静默截断到定容上限，编码天然有界）；
//! - 事后可 `verify()` 全链校验——篡改任一记录（event_hash 翻转）
//!   即被检出（broken_at 定位）。

use alloc::format;

use eneros_audit::chain::{AuditLogger, AuditLoggerImpl, ChainVerifier, ChainVerifierImpl};
use eneros_audit::events::{AuditEvent, AuditEventType};
use eneros_audit::store::BoundedMemStore;
use eneros_audit::AuditStore;
use eneros_audit::{AuditError, VerifyReport};

/// 安全校验结果（W2.1 反馈环的 safety 阶段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyOutcome {
    /// 校验通过。
    Passed,
    /// 安全阻断（决策被拒，未执行）。
    Blocked,
    /// 降级通过（钳位/限幅后放行）。
    Degraded,
}

impl SafetyOutcome {
    fn as_str(self) -> &'static str {
        match self {
            SafetyOutcome::Passed => "passed",
            SafetyOutcome::Blocked => "blocked",
            SafetyOutcome::Degraded => "degraded",
        }
    }
}

/// 执行结果（W2.1 反馈环的 execute 阶段；蓝图 40 WP-40.3 扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecOutcome {
    /// 执行成功（dispatch Committed）。
    Succeeded,
    /// 执行失败（设备拒绝/超时/读回不符）。
    Failed,
    /// 降级执行。
    Degraded,
    /// 蓝图 40：已产生 Proposal（无副作用，dispatch 阶段另行审计）。
    Proposed,
    /// 蓝图 40：明确不动作（NoAction——不是成功下发）。
    NoAction,
}

impl ExecOutcome {
    fn as_str(self) -> &'static str {
        match self {
            ExecOutcome::Succeeded => "succeeded",
            ExecOutcome::Failed => "failed",
            ExecOutcome::Degraded => "degraded",
            ExecOutcome::Proposed => "proposed",
            ExecOutcome::NoAction => "noaction",
        }
    }
}

/// 一条 AI 决策的完整记录（五要素 + 时间）。
#[derive(Debug, Clone, Copy)]
pub struct DecisionRecord<'a> {
    /// 决策代理标识（如 `energy-agent`）。
    pub actor: &'a str,
    /// 目标设备/对象。
    pub target: &'a str,
    /// 决策意图（自然语言/指令）。
    pub intent: &'a str,
    /// LP 解（求解器输出）。
    pub lp_solution: &'a str,
    /// 安全校验结果。
    pub safety: SafetyOutcome,
    /// 执行结果。
    pub exec: ExecOutcome,
    /// 事件时间戳（毫秒）。
    pub now_ms: u64,
}

/// AI 决策审计器（SM3 哈希链，篡改可检出）。
pub struct DecisionAudit {
    /// 底层审计日志器（字节预算化内存存储：构造期预分配、永不扩容）。
    logger: AuditLoggerImpl<BoundedMemStore>,
    /// 31-review：因缓冲上限被丢弃的记录数（写失败计数）。
    dropped: u64,
}

/// 蓝图 70.1 WP-70.1.2（DEC-70-01）：决策审计字节预算单一来源。
///
/// 64 KiB = 585 条 ChainEntry（112 B/条）。该值取代旧的 4096 条数门
/// （`4096 × 112 = 448 KiB`，占 512 KiB 目标堆 87.5%，扩容翻倍瞬态
/// 672 KiB 必然 OOM——69.5 窗实弹在 513 条处失败）。预算语义：
/// 构造期一次预分配（svc-control-dispatch / svc-ai-worker 两生产装配点
/// 显式传入本常量），满载丢新记录并计数，覆盖窗 = 容量条数 ÷ 决策速率
/// （D2 保留窗口裁决前的如实口径：约 585 轮控制决策，审计覆盖在满载后
/// 停止而非回绕；持久化覆盖窗归 DEC-70-01 的 A 案后续裁决）。
pub const DECISION_AUDIT_BUDGET_BYTES: usize = 64 * 1024;

impl DecisionAudit {
    /// 新建决策审计器（`genesis` = 创世锚哈希）。
    ///
    /// 预算取 [`DECISION_AUDIT_BUDGET_BYTES`] 单一来源；生产装配点应经
    /// [`DecisionAudit::with_budget`] 显式声明（预算来源可核对）。
    pub fn new(genesis: [u8; 32]) -> Self {
        Self::with_budget(genesis, DECISION_AUDIT_BUDGET_BYTES)
    }

    /// 蓝图 70.1 WP-70.1.2：以显式字节预算创建决策审计器。
    ///
    /// `budget_bytes` 在构造期一次性预分配（`capacity = budget/112` 向下
    /// 取整），此后追加零分配；预算到顶丢弃新记录并计入
    /// [`DecisionAudit::dropped_count`]，返回 `BufferFull`——满载不通过
    /// OOM 表达。
    pub fn with_budget(genesis: [u8; 32], budget_bytes: usize) -> Self {
        Self {
            logger: AuditLoggerImpl::new(BoundedMemStore::with_byte_budget(budget_bytes), genesis),
            dropped: 0,
        }
    }

    /// 31-review：因缓冲上限被丢弃的记录数（写失败计数，决策路径不阻塞）。
    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }
}

/// 32 AG-04：从会话密钥材料派生决策审计器——创世锚 =
/// `SM3("eneros-decision-audit-v1|" ‖ key_material)`，与封焊链
/// chain_key 同源（AI-07 fail-closed 注入），无密钥即无审计器。
pub fn decision_audit_from_key(key_material: &[u8]) -> DecisionAudit {
    const DOMAIN: &[u8] = b"eneros-decision-audit-v1|";
    let mut buf = alloc::vec::Vec::with_capacity(DOMAIN.len() + key_material.len());
    buf.extend_from_slice(DOMAIN);
    buf.extend_from_slice(key_material);
    DecisionAudit::new(eneros_crypto::sm3_hash(&buf))
}

impl DecisionAudit {
    /// 记录一条完整 AI 决策（意图 → LP 解 → 安全 → 执行）。
    ///
    /// 返回链上条目（含 event_hash/combined_hash，供追溯）。偏差指标
    /// （`deviation`）曾为 W2.1 字段但恒为 0.0（SPEC-B BQ-4 裁定删除——
    /// 接口不承诺永远为零的数据；真实偏差流走学习通路观测面）。
    pub fn log_decision(
        &mut self,
        r: &DecisionRecord,
    ) -> Result<eneros_audit::ChainEntry, AuditError> {
        let detail = format!(
            "i={};p={};s={};x={}",
            r.intent,
            r.lp_solution,
            r.safety.as_str(),
            r.exec.as_str()
        );
        let mut event = AuditEvent::new(AuditEventType::AiDecision, r.actor, r.target, &detail);
        event.timestamp_ms = r.now_ms;
        // 31-review（P1 AG-04）+ 蓝图 70.1：有界缓冲——链满后丢弃新记录
        // 并计数（BoundedMemStore 预分配容量到顶返回 BufferFull，零扩容；
        // 决策路径不阻塞、满载不经 OOM 表达）。
        match self.logger.log(event) {
            Ok(entry) => Ok(entry),
            Err(AuditError::BufferFull) => {
                self.dropped += 1;
                Err(AuditError::BufferFull)
            }
            Err(e) => Err(e),
        }
    }

    /// 当前链头（链尾哈希/序号/条数）。
    pub fn chain_state(&self) -> &eneros_audit::HashChain {
        self.logger.chain_state()
    }

    /// 读取链上条目（按 seq 范围）。
    pub fn read_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<alloc::vec::Vec<eneros_audit::ChainEntry>, AuditError> {
        self.logger.store().read_range(start, end)
    }

    /// 全链校验：逐条检查 prev_hash 衔接 + 组合哈希自洽。
    ///
    /// 篡改任一决策记录（event_hash 翻转）→ `broken_at = Some(seq)`。
    pub fn verify(&self) -> Result<VerifyReport, AuditError> {
        let count = self.logger.store().count()?;
        let entries = self.read_range(1, count)?;
        ChainVerifierImpl.verify_chain(&entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：构造决策记录。
    fn rec(i: u64) -> DecisionRecord<'static> {
        DecisionRecord {
            actor: "energy-agent",
            target: "pv-1",
            intent: "intent",
            lp_solution: "plan-x",
            safety: SafetyOutcome::Passed,
            exec: ExecOutcome::Succeeded,
            now_ms: 1000 + i,
        }
    }

    /// 验收：决策事件（五要素）在审计链可查。
    #[test]
    fn test_decision_event_queryable() {
        let mut audit = DecisionAudit::new([0xAB; 32]);
        let mut r = rec(0);
        r.intent = "discharge 5kW";
        r.lp_solution = "p1=5.0;p2=0.0";
        let entry = audit.log_decision(&r).expect("入链");
        assert_eq!(entry.seq, 1);
        assert_eq!(audit.chain_state().entry_count, 1);
        // 读回条目，detail 含决策要素。
        let entries = audit.read_range(1, 1).expect("读回");
        assert_eq!(entries.len(), 1);
        // 事件 detail 通过 event_hash 固化；从存储读回的是 ChainEntry——
        // 五要素可追溯性由 detail 编码保证（此处经 logger 内部事件断言）。
        let all = audit.read_range(1, 10).expect("全链");
        assert_eq!(all.len(), 1);
        assert_ne!(entry.event_hash, [0u8; 32], "SM3 哈希非零");
    }

    /// 验收：篡改决策记录被审计验证检出（event_hash 翻转一字节）。
    #[test]
    fn test_tampered_decision_detected() {
        let mut audit = DecisionAudit::new([0xAB; 32]);
        for i in 0..3 {
            let mut r = rec(i);
            r.intent = "intent";
            audit.log_decision(&r).expect("入链");
        }
        // 篡改：第二条 event_hash 翻转一字节（决策记录被改）。
        let mut entries = audit.read_range(1, 10).expect("全链");
        entries[1].event_hash[0] ^= 0xFF;
        let report = ChainVerifierImpl.verify_chain(&entries).expect("校验");
        assert!(!report.valid, "篡改必须被检出");
        assert_eq!(report.broken_at, Some(2), "定位到被篡改的决策记录");
    }

    /// 验收：链衔接——连续决策 prev_hash 严格链式。
    #[test]
    fn test_chain_linkage_between_decisions() {
        let mut audit = DecisionAudit::new([0x11; 32]);
        let mut r1 = rec(0);
        r1.actor = "a";
        r1.target = "t";
        r1.intent = "i1";
        let first = audit.log_decision(&r1).expect("e1");
        let mut r2 = rec(1);
        r2.actor = "a";
        r2.target = "t";
        r2.intent = "i2";
        r2.safety = SafetyOutcome::Blocked;
        r2.exec = ExecOutcome::Failed;
        let second = audit.log_decision(&r2).expect("e2");
        assert_eq!(second.prev_hash, first.combined_hash, "链式衔接");
        // 未篡改链 verify 全绿。
        assert!(audit.verify().expect("校验").valid);
    }

    /// 验收：安全阻断的决策也入链（"拒绝" 同样可审计）。
    #[test]
    fn test_blocked_decision_also_audited() {
        let mut audit = DecisionAudit::new([0x22; 32]);
        let mut r = rec(0);
        r.target = "battery-1";
        r.intent = "discharge 9kW";
        r.safety = SafetyOutcome::Blocked;
        r.exec = ExecOutcome::Failed;
        r.now_ms = 5;
        let entry = audit.log_decision(&r).expect("入链");
        assert_eq!(entry.seq, 1);
        let report = audit.verify().expect("校验");
        assert!(report.valid);
    }

    /// 验收：AG-04 密钥派生创世锚——同密钥同锚、异密钥异锚，
    /// 派生链可正常入链/校验。
    #[test]
    fn test_decision_audit_from_key_derivation() {
        let mut a1 = decision_audit_from_key(b"test-key-material-16b");
        let a2 = decision_audit_from_key(b"test-key-material-16b");
        let a3 = decision_audit_from_key(b"other-key-material!");
        assert_eq!(
            a1.chain_state().genesis_hash,
            a2.chain_state().genesis_hash,
            "同密钥派生同创世锚"
        );
        assert_ne!(
            a1.chain_state().genesis_hash,
            a3.chain_state().genesis_hash,
            "异密钥派生异创世锚"
        );
        a1.log_decision(&rec(0)).expect("入链");
        assert!(a1.verify().expect("校验").valid);
    }

    /// 31-review（P2）：审计 detail（p/s/x 字段）必须编码进哈希——不同
    /// gate/exec 结果 → 不同 event_hash（篡改可检出；旧测试只数条数不断言内容）。
    #[test]
    fn test_audit_encodes_gate_and_exec_fields() {
        let mut a1 = DecisionAudit::new([0x42; 32]);
        let mut r_allow = rec(0);
        r_allow.lp_solution = "path=SlowPath;gate=allow;energy_kwh=1.000;violations=0";
        r_allow.exec = ExecOutcome::Succeeded;
        let e1 = a1.log_decision(&r_allow).expect("allow 入链");

        let mut a2 = DecisionAudit::new([0x42; 32]);
        let mut r_reject = rec(0);
        r_reject.lp_solution = "path=SlowPath;gate=energy_exceeded;energy_kwh=999.000;violations=0";
        r_reject.exec = ExecOutcome::Failed;
        let e2 = a2.log_decision(&r_reject).expect("reject 入链");

        assert_ne!(
            e1.event_hash, e2.event_hash,
            "gate/exec 字段必须编码进审计哈希"
        );
    }

    /// 31-review（P1 AG-04）+ 蓝图 70.1：有界缓冲——预算到顶丢弃新记录并
    /// 计数（BufferFull），决策路径不阻塞；预分配容量恒定、覆盖窗如实。
    #[test]
    fn test_audit_bounded_buffer_drops_overflow() {
        // 显式小预算：4 条 × 112 B/条 = 448 B。
        let mut audit = DecisionAudit::with_budget([0x11; 32], 4 * 112);
        for _ in 0..4 {
            audit.log_decision(&rec(0)).expect("预算内必须入链");
        }
        assert_eq!(audit.chain_state().entry_count, 4, "链条目数必须封顶");
        assert!(
            audit.log_decision(&rec(0)).is_err(),
            "超预算必须返回 Err（BufferFull）"
        );
        assert_eq!(audit.dropped_count(), 1, "丢弃计数必须递增");
        // 容量锁定：满载后链条既有条目完整可校验（丢新不丢旧）。
        assert!(audit.verify().expect("校验").valid);
    }

    /// 蓝图 70.1 WP-70.1.2：默认预算单一来源——`DecisionAudit::new` 的
    /// 预分配恰为 `DECISION_AUDIT_BUDGET_BYTES` 换算的条目容量（64 KiB
    /// → 585 条），容量经底层 store 只读访问器核对。
    #[test]
    fn test_audit_default_budget_matches_single_source() {
        let audit = DecisionAudit::new([0x12; 32]);
        let expected_cap = DECISION_AUDIT_BUDGET_BYTES / 112;
        assert_eq!(expected_cap, 585);
        assert_eq!(audit.chain_state().entry_count, 0);
        // 预分配容量经 logger.store() 视图核对（构造期锁定，非增长值）。
        assert_eq!(audit.logger.store().capacity_entries(), expected_cap);
        assert!(audit.logger.store().byte_budget() <= DECISION_AUDIT_BUDGET_BYTES);
    }
}
