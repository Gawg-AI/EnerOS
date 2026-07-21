//! EnerOS v0.74.0 MVP 端到端集成 — 储能自治场景.
//!
//! Phase 1 P1-L MVP 集成第三层：编排 v0.72.0 Energy/Market Agent 与 v0.73.0 Device
//! Agent 协同完成储能自治端到端场景（电价→双脑决策→设备执行），并提供收益对比
//! 基准（vs 传统规则 EMS），为 Phase 1 三项出口标准（autonomous 24h / 双脑链路
//! < 2s / 收益提升 ≥ 10%）提供可验证的代码骨架。
//!
//! # 核心类型
//!
//! - [`MvpOrchestrator`] — MVP 编排器，统一调度 Energy/Market/Device 三个 Agent
//! - [`MvpTickReport`] — 单 tick 报告（收益对比）
//! - [`RevenueComparator`] — 收益对比器，追踪双脑 EMS vs 传统 EMS 收益
//! - [`TraditionalEms`] — 传统规则 EMS 基准策略（谷充峰放）
//! - [`MvpError`] — MVP 编排错误类型
//!
//! # 偏差声明（D1~D15，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `log::info!("=== EnerOS MVP 储能自治场景启动 ===")` 等 | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57~v0.73 一致 |
//! | **D2** | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57~v0.73 一致 |
//! | **D3** | `std::thread::sleep(Duration::from_secs(15))` 无限循环 | `tick(now_ms)` 单步方法；循环由集成测试驱动 | no_std 无 `std::thread`；24h 测试是集成层关注，非单元测试；Karpathy "Simplicity First" |
//! | **D4** | `AgentRegistry::new()` + `Box::new(self.energy_agent.clone())` | Orchestrator 直接持有 3 个 Agent（非 clone） | Agent 未实现 `Clone`；v0.33.0 `AgentRegistry` 管理 `AgentDescriptor` 而非运行时 Agent 实例；直接持有更简单（Karpathy 简化原则） |
//! | **D5** | `system_agent: SystemAgent` + `self.system_agent.on_tick()` | 跳过 SystemAgent | SystemAgent（v0.41.0）为监控用途，对三项出口标准（autonomous/延迟/收益）非必需；Karpathy "don't add what wasn't asked" |
//! | **D6** | `watchdog: WatchdogDegradeFlow` + `self.watchdog.start()` + `self.watchdog.feed()` | 跳过 Watchdog 集成 | Watchdog（v0.58.0）为运行时基础设施；MVP 编排器聚焦 3-Agent 协作 + 收益对比逻辑；Watchdog 喂狗属集成层关注 |
//! | **D7** | `agent.on_start()?`（无 `now_ms` 参数） | `agent.on_start(now_ms)?` | v0.72.0 `AgentRuntime` trait 签名要求 `now_ms: u64`；与 v0.72.0/v0.73.0 一致 |
//! | **D8** | `MarketAgent::new("market-server:8080")`（字符串服务器地址） | `MarketAgent::new_default(now_ms)` | v0.72.0 实际 API `MarketAgent::new(name, source: Box<dyn MarketDataSource>, now_ms)` 接收数据源 trait 对象，非服务器地址字符串；MVP 用 Mock 即可 |
//! | **D9** | `self.energy_agent.coordinator.execute(&energy_state)?` 直接访问 coordinator | `self.energy_agent.on_tick(now_ms)?` | 通过 `AgentRuntime` trait 调用，封装更清晰；Energy Agent 内部构建 `RealtimeState` 并调用 coordinator（v0.72.0 D11 已实现） |
//! | **D10** | `fn collect_energy_state(&self) -> SystemState { ... }` | 跳过（Energy Agent 内部自建状态） | v0.72.0 `EnergyAgent::on_tick` 已从 `current_price` 构建 `RealtimeState`（v0.72.0 D11），Orchestrator 无需重复构建 |
//! | **D11** | `self.market_agent.price_cache[0]` | `self.market_agent.price_cache` 字段直接访问 | v0.72.0 `MarketAgent.price_cache` 为 `pub Vec<f64>`，可直接访问 |
//! | **D12** | 分离的 `revenue_tracker: RevenueTracker` + `RevenueComparator` | 合并为单一 `RevenueComparator` | 两者职责重叠（追踪收益）；Karpathy 简化原则——单一结构体即可 |
//! | **D13** | `TraditionalEms::schedule(&self, state: &SystemState)` | `TraditionalEms::schedule(&self, current_price: f64, soc: f64)` | 蓝图仅用 `state.soc`；传基本类型避免 `SystemState` 依赖耦合（v0.67.0 `SystemState` 字段不同） |
//! | **D14** | Python 24h 端到端测试 + GPU 优先（llama.cpp） | Rust 集成测试（Mock 双脑 + Mock 设备） | no_std Rust crate 无 Python；MockSolver/DualBrainMockEngine 为 CPU-only；24h 真实运行 + GPU 推理属集成测试层（蓝图 §6.1 Python 测试在硬件/QEMU 环境运行） |
//! | **D15** | `revenue_yuan: (discharge - charge) * price * self.config.period_hours` | 保持蓝图公式 | 与蓝图一致； TraditionalEms 用 `ScheduleConfig` 的 `period_hours` 计算收益 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。
//!
//! - 无 `use std::*`
//! - 无 `panic!` / `todo!` / `unimplemented!`
//! - 无 `SystemTime::now()` / `thread::sleep`
//! - 无 `log::*` 宏
//! - 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）

#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod error;
mod orchestrator;
mod revenue;
mod traditional_ems;

pub use error::MvpError;
pub use orchestrator::{MvpOrchestrator, MvpTickReport};
pub use revenue::RevenueComparator;
pub use traditional_ems::TraditionalEms;

#[cfg(test)]
mod tests;
