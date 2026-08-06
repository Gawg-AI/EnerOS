//! EnerOS 发布工程 (REL-001~016).
//!
//! MASTER §21.15:
//! - REL-001 (G0): "pinned toolchain manifest — digest 可验证"
//! - REL-002 (G0/G1): "hermetic/offline build env — clean host 可重建"
//! - REL-003 (G1): "SOURCE_DATE_EPOCH 清理 — 无当前时间/路径漂移"
//! - REL-004 (G0): "workspace license 决策 — LICENSE/Cargo 一致"
//! - REL-005 (G9): "third-party NOTICE 生成 — 每 artifact 对应"
//! - REL-006 (G7/G9): "model/firmware license review — 出货权利确认"
//! - REL-007 (G1/G9): "SPDX/CycloneDX SBOM — source/binary/model 全覆盖"
//! - REL-008 (G9): "signed provenance — materials/builder/artifact"
//! - REL-009 (G9): "branch/CODEOWNERS/tag policy — 双人/签名/不可变"
//! - REL-010 (G9): "HSM/offline signing ceremony — 演练与恢复"
//! - REL-011 (G9): "release verifier 独立工具 — 反向验证全部产物"
//! - REL-012 (G9): "release evidence bundle — requirement 到 artifact"
//! - REL-013 (G9): "EULA/privacy/export/crypto review — 法务批准"
//! - REL-014 (G9): "release notes/known limits — 支持矩阵一致"
//! - REL-015 (G9): "rollback/withdraw channel — 撤回演练"
//! - REL-016 (G9): "artifact repository immutability — digest/signature retention"

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod toolchain;
pub mod sbom;
pub mod provenance;
pub mod ceremony;
pub mod evidence;
pub mod legal;
pub mod lifecycle;

pub use toolchain::{ToolchainManifest, ReproducibilityCheck, ReproResult};
pub use sbom::{SbomEntry, SbomReport, NoticeEntry, LicenseDecision};
pub use provenance::{ProvenanceRecord, ProvenanceMaterial, BranchPolicy, CodeownersRule};
pub use ceremony::{SigningCeremony, CeremonyStep, CeremonyResult, HsmStatus};
pub use evidence::{ReleaseVerifier, VerificationCheck, EvidenceBundle, EvidenceItem};
pub use legal::{LegalReview, LegalDomain, LegalStatus, ReleaseNotes, KnownLimit};
pub use lifecycle::{WithdrawChannel, WithdrawStatus, ArtifactImmutability, ImmutableArtifact};
