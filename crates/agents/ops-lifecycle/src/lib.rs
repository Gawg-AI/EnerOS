//! EnerOS 设备生命周期 (OPS-008~016).
//!
//! MASTER §21.14:
//! - OPS-008 (G8): "backup snapshot/package — 加密签名与 manifest"
//! - OPS-009 (G8): "empty-device restore drill — RPO/RTO 报告"
//! - OPS-010 (G8): "first-boot commissioning — 无默认密码/安全只读"
//! - OPS-011 (G8): "site acceptance test — 签名验收报告"
//! - OPS-012 (G8): "factory reset — 保留/删除规则准确"
//! - OPS-013 (G8): "secure erase/decommission — cert revoke + report"
//! - OPS-014 (G8): "RMA identity rebind — 双授权/不泄密"
//! - OPS-015 (G9): "support/EOL policy — LTS/漏洞/备件承诺"
//! - OPS-016 (G9): "disaster recovery drill — fleet/离线站点场景"

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod backup;
pub mod commissioning;
pub mod disaster_recovery;
pub mod eol_policy;
pub mod factory_reset;
pub mod rma;
pub mod sat;
pub mod secure_erase;

pub use backup::{
    BackupItem, BackupManifest, BackupPackage, RestoreDrill, RestoreResult, RpoRtoReport,
};
pub use commissioning::{
    CommissioningResult, CommissioningStatus, CommissioningStep, FirstBootCommissioning,
};
pub use disaster_recovery::{
    DisasterRecoveryDrill, DrScenario, DrScenarioKind, DrStep, DrStepResult,
};
pub use eol_policy::{
    CvssLevel, EolPolicy, SparePartCommitment, SupportTier, VersionSupport, VulnerabilitySla,
};
pub use factory_reset::{DeleteRule, FactoryReset, PreserveRule, ResetPolicy};
pub use rma::{RmaAuthorization, RmaRebind, RmaResult, RmaStatus};
pub use sat::{SatCheck, SatChecklist, SatReport, SatResult, SiteAcceptanceTest};
pub use secure_erase::{EraseItem, EraseLevel, EraseReport, SecureErase};
