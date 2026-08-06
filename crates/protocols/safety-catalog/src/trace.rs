//! 安全需求追溯 (CTL-001)。
//!
//! 把 hazard / FMEA / STPA → 安全需求 → 代码 / 测试 / 验收 串联成可审计链。每条
//! [`SafetyRequirement`] 指向一个或多个 [`TraceTarget`]，证明需求已被实现与验证。

/// 需求来源（来自 hazard / FMEA / STPA 中的哪一条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementSource {
    Hazard(&'static str),
    Fmea(&'static str),
    Stpa(&'static str),
}

/// 追溯目标：需求被实现/验证的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceTarget {
    /// 代码位置（crate 或 crate::path）。
    pub code: &'static str,
    /// 验证测试名（如 `control_gateway::tests::test_authorize_denies_expired`）。
    pub test: &'static str,
    /// 验收等级（L0/L1/L2/…）。
    pub evidence_level: &'static str,
}

/// 安全需求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyRequirement {
    pub id: &'static str,
    pub source: RequirementSource,
    /// 需求陈述（必须可被代码强制）。
    pub statement: &'static str,
    pub target: TraceTarget,
}

impl SafetyRequirement {
    pub const fn new(
        id: &'static str,
        source: RequirementSource,
        statement: &'static str,
        target: TraceTarget,
    ) -> Self {
        Self {
            id,
            source,
            statement,
            target,
        }
    }
}
