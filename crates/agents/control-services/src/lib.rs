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
// SPEC-B（WP-B.3）：AI tick 编排单源（svc-ai-worker 双 handler 收编；
// 注入路径等价性分析与锁定测试见模块头）。
pub mod ai_handler;
pub mod cfg_bindings;
// 蓝图 61 v0.314.33：SemanticCatalog 生产装载窄口（反馈槽/覆盖/控制
// 绑定自目录派生；三服务共用）。
pub mod catalog_gateway;
pub mod collector;
// SPEC-B（WP-B.2）：svc-control-dispatch 装配面下沉（fail-closed 阶梯 +
// 出站通道；bin 退化为装配接线）。阶梯核心 host 可测（DoD B1：seL4 叶子
// 以 closure seam 注入）；EIPC 通道/引擎装配限目标构建（cfg(none) 项级门）。
pub mod dispatch_assembly;
pub mod dispatch_service;
pub mod energy_agent_service;
// SPEC-SERIES-2026-08-27/E（WP-E.3）：有界学习闭环（svc-ai-worker 宿主）。
pub mod learn;
pub mod obs_shadow;
// 蓝图 68 卷 68.4（WP-68.4.1/68.4.2）：OBS 事件面（StoreSink）+ 快照面
// （SnapshotModule 生产入口）。
pub mod event_face;
// 蓝图 68 卷 68.4：host 测试共享夹具（test-util 缝——WP-64.3.2.1 先例；
// dependent dev-deps 启用，生产面零 feature）。
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;
// 蓝图 61 v0.314.34：evaluator ↔ snapshot 差分测试（全模块 test-only）。
pub mod remote_brain;
pub mod service;
// 蓝图 68 卷 68.1 WP-68.1.2：分块接收槽位表（服务主循环私有单源；host
// 测试面编译——消费方 control_service_main 为 target-only，host 非测试
// 构建下整模块排除，无 dead_code 面）。
#[cfg(any(test, target_os = "none"))]
mod service_slots;
#[cfg(test)]
pub mod snapshot_diff;
pub mod telemetry;
// SPEC-C（WP-C.1）：TSDB 磁盘历史适配（D-TSDB-2 方案 A 句柄让渡）。
pub mod telemetry_history;

pub use service::{control_service_main, ControlServiceHandler};
