//! EnerOS 远程遥测队列 + 隐私 opt-in (OBS-013/014).
//!
//! MASTER §21.13:
//! - OBS-013 (G8): "remote telemetry queue — 离线有界/恢复补传"
//! - OBS-014 (G8): "privacy/telemetry opt-in — 客户可配置并审计"
//!
//! G8 Exit: "设备离线时遥测有界缓存，恢复后补传；客户可配置遥测
//! 类别并审计变更。"
//!
//! # 模块
//!
//! - [`queue`] — 有界遥测队列（离线缓存 + 恢复补传 + 溢出降级）
//! - [`privacy`] — 隐私配置（遥测类别 opt-in/opt-out + 审计日志）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod mqtt_export;
pub mod persist;
pub mod privacy;
pub mod queue;

pub use persist::{load_snapshot, persist_snapshot, OtlpExporter, QueueSnapshot};
pub use privacy::{PrivacyAuditEntry, PrivacyAuditLog, PrivacyConfig, TelemetryConsent};
pub use queue::{OverflowPolicy, QueueStats, TelemetryCategory, TelemetryEvent, TelemetryQueue};

#[cfg(all(test, feature = "loom"))]
pub mod loom_queue;
