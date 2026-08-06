//! EnerOS v0.126.0 VPP 场景端到端验证 — 多机联邦 VPP 聚合 + DR 参与 + Phase 2 出口报告.
//!
//! Phase 2 P2-K 第 4 版（Phase 2 收官版）：在仿真园区场景中组建 5-10 台 Edge Box
//! 联邦集群，验证多机联邦 VPP 聚合能力，参与需求响应（DR）端到端链路并测量响应
//! 时间（★ < 30s），逐项执行 10 项 Phase 2 出口标准验证并生成出口报告，宣告系统
//! 具备商用级 VPP 运营能力。
//!
//! # 交付物清单
//!
//! - `src/vpp_scenario.rs` — VppRole / EdgeNode / VppAggregation / VppError /
//!   VppScenario trait / VppScenarioImpl：集群组建、Coordinator 选举、聚合计算、
//!   DR 响应执行、出口标准评估
//! - `src/dr_participation.rs` — DrEvent / DrState / DrHandler：DR 事件接收→调度→
//!   确认→完成状态机与调度计划记录
//! - `src/exit_report.rs` — ExitCriterion / ExitEvidence / Phase2ExitReport：证据
//!   注册、出口判定、报告渲染
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*` / `heapless::*`，可交叉编译到
//! `aarch64-unknown-none`。
//!
//! - 无 `use std::*`
//! - 无 `panic!` / `todo!` / `unimplemented!` / `unwrap()`
//! - 无 `SystemTime::now()` / `thread::sleep`（时间/延迟一律参数注入）
//! - 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
//!
//! # 蓝图偏差声明（D1~D7）
//!
//! - D1：`crate::current_time_ms()` → 响应延迟确定性仿真模型（link_latency_ms/coord_processing_ms 构造注入，now_ms 参数传入），no_std 无系统时钟
//! - D2：`Vec`/`String`/`format!` → alloc 版本；定界字段用 heapless::String（address≤64/name≤64/target≤128/actual≤128）
//! - D3：crate 落位 `crates/e2e/` → `crates/agents/vpp-scenario/`，包名 eneros-vpp-scenario（仓库规范子系统归口；与 v0.74.0 mvp-scenario 同口径）
//! - D4：`run_exit_criteria` 第 4~10 项硬编码 `true` → ExitEvidence 证据注入（未注册 → passed=false "未验证"，杜绝谎报）；集成测试经 dev-dependencies 驱动真实子系统产生证据
//! - D5：`DrEvent.deadline_ms` 语义澄清为响应时限预算（毫秒）——蓝图字段 doc 写"响应截止时间"而 §4.5 代码 `elapsed > event.deadline_ms` 按预算使用，以代码语义为准
//! - D6：DR 调度"下发指令（简化）"→ DrHandler 记录调度计划（node_id→share_kw，容量 16）+ 状态机推进；SOC 不在本版本仿真层更新（属储能设备侧行为，v0.101.0 孤岛/调度版本范畴）
//! - D7：蓝图 §4.3 sequenceDiagram DR 目标 -500kW 与出口报告聚合容量 350kW 矛盾（500>350 必触发 InsufficientCapacity）→ 配置与测试采用 -300kW（≤ 可用容量），sequenceDiagram 数字视为示意

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod dr_participation;
pub mod exit_report;
pub mod vpp_scenario;

pub use dr_participation::{DrEvent, DrState};
pub use exit_report::{ExitCriterion, ExitEvidence, Phase2ExitReport};
pub use vpp_scenario::{EdgeNode, VppAggregation, VppError, VppRole, VppScenario};
