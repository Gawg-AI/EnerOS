//! EnerOS 自动结算系统 (v0.180.0)。
//!
//! 提供清算（Clearing）、对账（Reconciliation）与争议处理（Dispute）三大核心能力，
//! 适用于电力交易自动结算场景。
//!
//! # 架构
//! ```text
//! ┌───────────────────────────────────────────────────┐
//! │  eneros-settlement (v0.180.0)                     │
//! │  ┌──────────┐  ┌────────────┐  ┌──────────────┐  │
//! │  │ clearing │  │ reconcile  │  │ dispute      │  │
//! │  │ (Engine) │  │(Reconciler)│  │(DisputeMgr)  │  │
//! │  └──────────┘  └────────────┘  └──────────────┘  │
//! │  ┌──────────────────────────────────────────────┐ │
//! │  │ types (ClearingJob, ClearingResult, ...)     │ │
//! │  └──────────────────────────────────────────────┘ │
//! └───────────────────────────────────────────────────┘
//! ```
//!
//! # no_std 合规
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod clearing;
pub mod dispute;
pub mod reconcile;
pub mod types;

pub use clearing::ClearingEngine;
pub use dispute::DisputeManager;
pub use reconcile::Reconciler;
pub use types::{
    ClearStatus, ClearingJob, ClearingResult, DisputeCase, DisputeState, ReconcileReport,
};

/// 结算错误类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettlementError {
    /// 清算执行失败。
    ClearingFailed,
    /// 无效的清算任务（如合同电量为零）。
    InvalidJob,
    /// 未找到对应记录。
    NotFound,
    /// 无效的状态转换。
    InvalidTransition,
    /// 重复争议（同一交易已存在未解决争议）。
    DuplicateDispute,
    /// 对账失败。
    ReconcileFailed,
}

/// 自动结算 trait，统一清算/对账/争议入口。
pub trait AutoSettlement {
    /// 执行单笔清算。
    fn clear(&mut self, job: &ClearingJob) -> Result<ClearingResult, SettlementError>;
    /// 对账：将清算结果与预期金额比对。
    fn reconcile(&self, results: &[ClearingResult], expected: &[(u64, f64)]) -> ReconcileReport;
    /// 发起争议。
    fn dispute(
        &mut self,
        deal_id: u64,
        reason: &str,
        now: u64,
    ) -> Result<DisputeCase, SettlementError>;
}
