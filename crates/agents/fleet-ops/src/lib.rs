//! EnerOS Fleet 运维 (OPS-001~007).
//!
//! MASTER §21.14:
//! - OPS-001 (G8): "device enrollment — 一次性 token + attestation"
//! - OPS-002 (G8): "inventory/desired-observed state — 漂移检测"
//! - OPS-003 (G8): "100+ virtual fleet harness — 网络分区/批次更新"
//! - OPS-004 (G8): "canary/rollout/auto-stop — 坏版本不扩散"
//! - OPS-005 (G8): "maintenance window/preflight — 控制状态不安全时拒绝"
//! - OPS-006 (G8): "OS/config/Agent/model rollout — 兼容矩阵强制"
//! - OPS-007 (G8): "remote package/cert revoke — 离线后重连生效"

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod canary;
pub mod enrollment;
pub mod inventory;
pub mod maintenance;
pub mod revoke;
pub mod rollout;

pub use canary::{AutoStopReason, BatchHealth, CanaryPhase, CanaryRollout, RolloutPolicy};
pub use enrollment::{AttestationReport, DeviceEnrollment, EnrollmentStatus, EnrollmentToken};
pub use inventory::{DeviceInventory, DeviceState, DriftType, StateDrift};
pub use maintenance::{
    MaintenanceAction, MaintenancePreflight, MaintenanceWindow, PreflightCheck, PreflightResult,
    SystemSnapshot,
};
pub use revoke::{RevocationRecord, RevokeManager, RevokeStatus, RevokeTarget};
pub use rollout::{
    CompatError, CompatRule, CompatibilityMatrix, RolloutManager, RolloutStatus, RolloutTarget,
    RolloutType,
};

#[cfg(test)]
mod fleet_harness;
