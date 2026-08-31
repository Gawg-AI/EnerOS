//! 输出候选产物（[11] §2 顺序链第一步）：`Explanation` / `ObjectiveDraft`
//! / `IntentCandidate`（decision-contract）。类型上没有设备面字段——
//! 本模块不定义、不导入 AuthorizedCommand。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::ids::{ReasonCode, SnapshotId};
use eneros_decision_contract::objective::ObjectiveKind;

const EXPLANATION_TEXT_MAX: usize = 512;
const EXPLANATION_EVENT_REFS_MAX: usize = 8;

/// 告警/事件解释（[11] §4.5 skill 2；无控制）。
#[derive(Debug, Clone, PartialEq)]
pub struct Explanation {
    scope_snapshot: SnapshotId,
    event_refs: Vec<[u8; 16]>,
    /// 严重度 1..=3。
    severity: u8,
    model_digest: [u8; 32],
    prompt_digest: [u8; 32],
    /// 有界文本（脱敏：控制字符剥离；< 512B）。
    text: String,
    valid_until_ms: u64,
}

impl Explanation {
    pub fn new(
        scope_snapshot: SnapshotId,
        event_refs: Vec<[u8; 16]>,
        severity: u8,
        model_digest: [u8; 32],
        prompt_digest: [u8; 32],
        text: String,
        valid_until_ms: u64,
    ) -> Result<Self, &'static str> {
        if model_digest == [0; 32] {
            return Err("model_digest=0");
        }
        if event_refs.len() > EXPLANATION_EVENT_REFS_MAX {
            return Err("too many event refs");
        }
        if !(1..=3).contains(&severity) {
            return Err("severity∉1..=3");
        }
        let text = sanitize(&text);
        if text.len() > EXPLANATION_TEXT_MAX {
            return Err("text too long");
        }
        if text.is_empty() {
            return Err("text empty");
        }
        if valid_until_ms == 0 {
            return Err("valid_until_ms=0");
        }
        Ok(Self {
            scope_snapshot,
            event_refs,
            severity,
            model_digest,
            prompt_digest,
            text,
            valid_until_ms,
        })
    }

    pub fn scope_snapshot(&self) -> SnapshotId {
        self.scope_snapshot
    }
    pub fn event_refs(&self) -> &[[u8; 16]] {
        &self.event_refs
    }
    pub fn severity(&self) -> u8 {
        self.severity
    }
    pub fn model_digest(&self) -> &[u8; 32] {
        &self.model_digest
    }
    pub fn prompt_digest(&self) -> &[u8; 32] {
        &self.prompt_digest
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.valid_until_ms
    }
}

/// 目标草稿（[11] §4.5 skill 1；市场事实 → ObjectiveDraft）。**草稿**即
/// 未批准建议——下游第 7 层一律拒绝（objective-engine
/// `UNAPPROVED_ADVICE`）；本类型不携带任何硬约束值。
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveDraft {
    scope_snapshot: SnapshotId,
    hint: ObjectiveKind,
    reason: ReasonCode,
    /// 建议优先级层（1..=7；层 7 = 未批准建议——下游编译拒绝）。
    priority: u8,
    /// 依据摘要（事实/快照证据 digest；非零）。
    rationale_digest: [u8; 32],
    /// 事实来源事件（非零；可追溯）。
    facts_ref: [u8; 16],
    model_digest: [u8; 32],
    prompt_digest: [u8; 32],
    valid_until_ms: u64,
}

impl ObjectiveDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope_snapshot: SnapshotId,
        hint: ObjectiveKind,
        reason: ReasonCode,
        priority: u8,
        rationale_digest: [u8; 32],
        facts_ref: [u8; 16],
        model_digest: [u8; 32],
        prompt_digest: [u8; 32],
        valid_until_ms: u64,
    ) -> Result<Self, &'static str> {
        if model_digest == [0; 32] {
            return Err("model_digest=0");
        }
        if !(1..=7).contains(&priority) {
            return Err("priority∉1..=7");
        }
        if rationale_digest == [0; 32] {
            return Err("rationale_digest=0");
        }
        if facts_ref == [0; 16] {
            return Err("facts_ref=0");
        }
        if valid_until_ms == 0 {
            return Err("valid_until_ms=0");
        }
        Ok(Self {
            scope_snapshot,
            hint,
            reason,
            priority,
            rationale_digest,
            facts_ref,
            model_digest,
            prompt_digest,
            valid_until_ms,
        })
    }

    pub fn scope_snapshot(&self) -> SnapshotId {
        self.scope_snapshot
    }
    pub fn hint(&self) -> ObjectiveKind {
        self.hint
    }
    pub fn reason(&self) -> ReasonCode {
        self.reason
    }
    pub fn priority(&self) -> u8 {
        self.priority
    }
    pub fn rationale_digest(&self) -> &[u8; 32] {
        &self.rationale_digest
    }
    pub fn facts_ref(&self) -> &[u8; 16] {
        &self.facts_ref
    }
    pub fn model_digest(&self) -> &[u8; 32] {
        &self.model_digest
    }
    pub fn prompt_digest(&self) -> &[u8; 32] {
        &self.prompt_digest
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.valid_until_ms
    }
}

/// 文本脱敏：剥离控制字符（保持 UTF-8 合法）。
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explanation_validation() {
        let ok = Explanation::new(
            SnapshotId([1; 16]),
            vec![[2; 16]],
            2,
            [3; 32],
            [4; 32],
            "过热预警".into(),
            1000,
        )
        .unwrap();
        assert_eq!(ok.severity(), 2);
        assert!(ok.is_valid_at(999));
        assert!(!ok.is_valid_at(1001));
        // 空文本/超长/严重度非法 → 拒绝。
        assert!(Explanation::new(
            SnapshotId([1; 16]),
            vec![],
            1,
            [3; 32],
            [4; 32],
            String::new(),
            1,
        )
        .is_err());
        let long = "x".repeat(EXPLANATION_TEXT_MAX + 1);
        assert!(
            Explanation::new(SnapshotId([1; 16]), vec![], 1, [3; 32], [4; 32], long, 1,).is_err()
        );
        assert!(Explanation::new(
            SnapshotId([1; 16]),
            vec![],
            0,
            [3; 32],
            [4; 32],
            "ok".into(),
            1,
        )
        .is_err());
        // 零模型摘要 → 拒绝。
        assert!(Explanation::new(
            SnapshotId([1; 16]),
            vec![],
            1,
            [0; 32],
            [4; 32],
            "ok".into(),
            1,
        )
        .is_err());
    }

    #[test]
    fn explanation_sanitizes_control_chars() {
        let e = Explanation::new(
            SnapshotId([1; 16]),
            vec![],
            1,
            [3; 32],
            [4; 32],
            "a\u{0}b\nc".into(),
            1,
        )
        .unwrap();
        // 控制字符（含换行）剥离。
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn objective_draft_validation() {
        let ok = ObjectiveDraft::new(
            SnapshotId([1; 16]),
            ObjectiveKind::PeakShave,
            ReasonCode::PeakShave,
            6,
            [5; 32],
            [6; 16],
            [7; 32],
            [8; 32],
            1000,
        )
        .unwrap();
        assert_eq!(ok.hint(), ObjectiveKind::PeakShave);
        // 零证据/零事实源/零模型摘要/非法层 → 拒绝。
        assert!(ObjectiveDraft::new(
            SnapshotId([1; 16]),
            ObjectiveKind::PeakShave,
            ReasonCode::PeakShave,
            6,
            [0; 32],
            [6; 16],
            [7; 32],
            [8; 32],
            1000,
        )
        .is_err());
        assert!(ObjectiveDraft::new(
            SnapshotId([1; 16]),
            ObjectiveKind::PeakShave,
            ReasonCode::PeakShave,
            6,
            [5; 32],
            [0; 16],
            [7; 32],
            [8; 32],
            1000,
        )
        .is_err());
        assert!(ObjectiveDraft::new(
            SnapshotId([1; 16]),
            ObjectiveKind::PeakShave,
            ReasonCode::PeakShave,
            8,
            [5; 32],
            [6; 16],
            [7; 32],
            [8; 32],
            1000,
        )
        .is_err());
    }
}
