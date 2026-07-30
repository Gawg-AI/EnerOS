//! # eneros-standards
//!
//! EnerOS 国际标准参与 (v0.185.0) — Power Native OS 标准提案、联盟协作与一致性检查。
//!
//! 本 crate 提供：
//! - 标准提案的起草、章节管理与提交（`proposal` 模块）
//! - 一致性条款检查（`conformance` 模块）
//! - 标准组织、API 稳定性等类型定义（`types` 模块）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod conformance;
pub mod proposal;
pub mod types;

pub use conformance::{ConformanceChecker, PnoConformance};
pub use proposal::{ProposalManager, ProposalManagerImpl};
pub use types::{
    ApiStability, ApiSurface, ConformanceClause, ConformanceReport, StandardBody, StandardProposal,
    StandardSection, SubmissionReceipt, SubmissionStatus,
};

/// 标准模块错误类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdError {
    /// 提案未找到
    ProposalNotFound,
    /// 提案无章节，无法提交
    NoSections,
    /// 提案已提交
    AlreadySubmitted,
    /// 无效章节
    InvalidSection,
    /// 一致性检查失败
    ConformanceFailed,
}
