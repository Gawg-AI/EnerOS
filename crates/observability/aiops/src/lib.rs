//! # EnerOS AI 运维 (AIOps)
//!
//! 提供异常检测、预测性维护和自愈能力，用于电力原生 Agent OS 的智能运维。
//!
//! ## 模块
//!
//! - [`anomaly`][]: 基于相对偏差的异常检测
//! - [`predict`][]: 基于线性外推的预测性维护
//! - [`self_heal`][]: 基于规则的自愈引擎
//! - [`types`][]: 核心类型定义

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod anomaly;
pub mod predict;
pub mod self_heal;
pub mod types;

pub use anomaly::AnomalyDetector;
pub use predict::PredictiveMaintenance;
pub use self_heal::{RuleBasedHealer, SelfHealEngine};
pub use types::{
    AnomalyResult, HealAction, HealActionType, HealOutcome, HealPlan, PredictionResult, RiskLevel,
};

/// AI 运维错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiOpsError {
    /// 异常检测失败
    DetectionFailed,
    /// 预测失败
    PredictionFailed,
    /// 自愈执行失败
    HealFailed,
    /// 需要人工审批
    ApprovalRequired,
}

impl core::fmt::Display for AiOpsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DetectionFailed => write!(f, "异常检测失败"),
            Self::PredictionFailed => write!(f, "预测失败"),
            Self::HealFailed => write!(f, "自愈执行失败"),
            Self::ApprovalRequired => write!(f, "需要人工审批"),
        }
    }
}
