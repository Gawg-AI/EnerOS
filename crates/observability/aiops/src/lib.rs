//! # EnerOS 运维分析（VRF-006 正名：statistical operations analytics）
//!
//! 提供异常检测、预测性维护和自愈能力——全部基于**统计与规则方法**
//! （相对偏差、线性外推、规则引擎），**不含**模型推理/机器学习
//! （无 LLM/神经网络后端）。原 "AI 运维 (AIOps)" 宣称已去除（VRF-006：
//! 未接真实模型推理的不得宣称 AI）。
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

/// 运维分析错误类型
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
