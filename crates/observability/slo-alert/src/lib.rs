//! EnerOS SLO 仪表盘基线 + 告警/Runbook 目录 (OBS-011/012).
//!
//! MASTER §21.13:
//! - OBS-011 (G8): "dashboards/SLO — workload 基线"
//! - OBS-012 (G8): "alert/runbook catalog — owner/recovery/演练"
//!
//! G8 Exit: "仪表盘有冻结 SLO 基线和 workload profile；每个告警有 owner、
//! recovery 步骤和定期演练记录。"
//!
//! # 模块
//!
//! - [`slo_baseline`] — 冻结 SLO 基线 + workload profile + 合规状态
//! - [`alert_catalog`] — 告警目录（每条告警含 owner/recovery/runbook）
//! - [`runbook`] — Runbook 步骤定义与执行检查

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod alert_catalog;
pub mod mqtt_sink;
pub mod runbook;
pub mod slo_baseline;
pub mod trigger;

pub use alert_catalog::{Alert, AlertCatalog, AlertError, AlertSeverity, AlertStatus};
pub use mqtt_sink::MqttAlertSink;
pub use runbook::{Runbook, RunbookStep, RunbookStepType};
pub use slo_baseline::{SloBaseline, SloCompliance, SloStatus, WorkloadProfile, FROZEN_BASELINES};
pub use trigger::{
    AlertState, FnNotificationSink, NotificationSink, TriggerEngine, TriggeredAlert,
};
