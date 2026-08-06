//! EnerOS 数据治理 (DATA-005/006/009/010).
//!
//! MASTER §21.17:
//! - DATA-005 (G8): "deletion/backup propagation — 可验证删除报告"
//! - DATA-006 (G8): "legal hold — 权限/审计"
//! - DATA-009 (G8): "telemetry privacy controls — opt-in/脱敏"
//! - DATA-010 (G9): "privacy threat assessment — 控制与证据"
//!
//! # 设计原则
//!
//! 所有合规结论**完全由证据推导**，不得硬编码 Passed：
//! - 删除传播：每个副本（主存/备份/副本/缓存）均有删除证据 → 完成
//! - 法定保留：保留项有权限审批 + 审计记录 → 合规
//! - 遥测隐私：opt-in 记录 + 脱敏验证（无密钥/PII 泄露）→ 通过
//! - 隐私评估：威胁映射控制 + 残留风险可接受 → 通过
//!
//! # 禁止捷径（MASTER G8/G9）
//!
//! - 不得用"删除请求已发送"代表删除完成
//! - 不得用配置项存在代表隐私控制生效
//! - 不得跳过脱敏验证声称无泄露
//! - 不得空评估报告声称隐私合规

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod deletion_propagation;
pub mod legal_hold;
pub mod privacy_assessment;
pub mod telemetry_privacy;

pub use deletion_propagation::{
    DeletionPropagation, DeletionRecord, DeletionReport, DeletionStatus, StoreKind,
};
pub use legal_hold::{
    HoldAuditAction, HoldAuditEntry, HoldPermission, HoldRecord, HoldState, LegalHold,
    LegalHoldManager,
};
pub use privacy_assessment::{
    PrivacyControl, PrivacyThreat, PrivacyThreatAssessment, PrivacyTraceGap, ResidualRisk,
    ThreatLikelihood, ThreatSeverity,
};
pub use telemetry_privacy::{
    PrivacyCategory, RedactionRule, RedactionVerify, TelemetryConsent, TelemetryPrivacy,
    TelemetryPrivacyReport,
};
