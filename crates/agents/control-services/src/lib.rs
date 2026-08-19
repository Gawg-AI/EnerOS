//! 蓝图 40 控制闭环 EL0 服务（WP-40.2/40.5/40.6/40.9）。
//!
//! 五个 required service 进入 ProductManifest（蓝图 40 §7）：
//!
//! - `svc-telemetry`（WP-40.2）：采集订阅 → 有界队列/去重/质量传播/epoch →
//!   快照原子冻结 coverage + provenance → `TrustedTelemetry` 或结构化拒绝；
//! - `svc-device-agent`（WP-40.9）：EIPC 收包（分块 ControlEnvelope）→
//!   认证/新鲜度/重放 → registry → AuthorizedWriter 执行 → ACK/readback/
//!   审计 → 结构化响应；
//! - `svc-control-dispatch`（WP-40.6）：唯一 ControlDispatch 服务化——
//!   校验 → 授权 → 预算 → intent 持久化 → 投递 → ACK → readback → audit；
//! - `svc-ai-worker`（WP-40.5）：target AI worker——只经 EIPC 获取可信输入
//!   和返回 proposal；Llama/HiGHS 为 host native 后端，target profile **不
//!   声明**（WP-40.4）→ 未声明后端时推理请求显式 `Unavailable`（零推理、
//!   零新写，不伪造）；
//! - `svc-energy-agent`：编排服务——telemetry → ai-worker → dispatch 的
//!   消息路由（各环节可独立替换；无后端时链路诚实降级）。
//!
//! 服务循环统一复用 [`service::control_service_main`]（分块 EIPC + 有界
//! 队列 + epoch + capability 归因，fail-closed）。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

extern crate alloc;

pub mod acquisition;
pub mod ai_worker;
pub mod dispatch_service;
pub mod energy_agent_service;
pub mod service;
pub mod telemetry;

pub use service::{control_service_main, ControlServiceHandler};
