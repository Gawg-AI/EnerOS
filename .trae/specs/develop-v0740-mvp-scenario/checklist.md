# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.74.0`
- [x] C2 members 列表已添加 `crates/agents/mvp-scenario`（置于 `crates/agents/device-agent` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/agents/mvp-scenario/Cargo.toml` 存在，package name = `eneros-mvp-scenario`
- [x] C5 dependencies 包含 `eneros-energy-market-agent` / `eneros-device-agent` / `eneros-energy-lp-model` / `eneros-agent`
- [x] C6 无 `[features]` 段（纯 Rust）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D15 偏差声明表
- [x] C9 模块声明：error / revenue / traditional_ems / orchestrator

## error.rs — MvpError
- [x] C10 `MvpError` 枚举：`AgentError(AgentRuntimeError)` / `NotRunning`
- [x] C11 派生 `Debug`
- [x] C12 实现 `fmt::Display`
- [x] C13 实现 `From<AgentRuntimeError> for MvpError`

## revenue.rs — RevenueComparator
- [x] C14 `RevenueComparator` 结构体：`dual_brain_revenue: Vec<f64>` / `traditional_revenue: Vec<f64>`
- [x] C15 `new()` 创建空实例
- [x] C16 `record_dual_brain(revenue)` push 到 dual_brain_revenue
- [x] C17 `record_traditional(revenue)` push 到 traditional_revenue
- [x] C18 `dual_brain_total()` 返回 dual_brain_revenue 总和
- [x] C19 `traditional_total()` 返回 traditional_revenue 总和
- [x] C20 `improvement_pct()` 计算 `(dual - trad) / trad * 100`，trad==0 返回 INFINITY
- [x] C21 `meets_target()` 返回 `improvement_pct() >= 10.0`
- [x] C22 `report()` 返回格式化对比报告字符串

## traditional_ems.rs — TraditionalEms
- [x] C23 `TraditionalEms` 结构体：`config: ScheduleConfig`
- [x] C24 `new(config)` 构造
- [x] C25 `schedule(current_price, soc)` 返回 `ScheduleResult`（D13：传基本类型）
- [x] C26 谷时（price < 0.3）充电：charge = pcs_power_kw, discharge = 0
- [x] C27 峰时（price > 0.8）放电：charge = 0, discharge = pcs_power_kw
- [x] C28 平时（0.3 ≤ price ≤ 0.8）保持：charge = 0, discharge = 0
- [x] C29 全 96 时段遍历，构建 96 个 `ScheduleEntry`
- [x] C30 `total_revenue_yuan` 为各时段收益之和
- [x] C31 `solve_status = SolveStatus::Optimal`

## orchestrator.rs — MvpOrchestrator
- [x] C32 `MvpOrchestrator` 结构体：7 字段（energy_agent / market_agent / device_agent / revenue_comparator / traditional_ems / tick_count / running）
- [x] C33 `new_default(now_ms)` — 3 个 Agent 用 `new_default`，TraditionalEms + RevenueComparator 初始化
- [x] C34 `new(energy, market, device, config, now_ms)` — 自定义 Agent 构造
- [x] C35 `start(now_ms)` — 依次调用 3 个 Agent `on_start(now_ms)`，`running = true`
- [x] C36 `tick(now_ms)` — `running == false` 返回 `Err(MvpError::NotRunning)`
- [x] C37 `tick` Step 1：market_agent.on_tick(now_ms)
- [x] C38 `tick` Step 2：转发市场数据 market_channel → energy_agent.market_channel
- [x] C39 `tick` Step 3：energy_agent.on_tick(now_ms)
- [x] C40 `tick` Step 4：current_schedule 为 Some 时记录双脑收益 + 传统 EMS 收益
- [x] C41 `tick` Step 5：device_agent.on_tick(now_ms)
- [x] C42 `tick` Step 6：tick_count += 1
- [x] C43 `tick` 返回 `MvpTickReport`
- [x] C44 `MvpTickReport` 结构体：tick / dual_brain_revenue / traditional_revenue / improvement_pct
- [x] C45 `stop(now_ms)` — 依次调用 3 个 Agent `on_stop(now_ms)`，`running = false`
- [x] C46 `report()` — 委托 `revenue_comparator.report()`

## 集成测试（lib.rs）
- [x] C47 T1 MvpError 变体构造
- [x] C48 T2 From<AgentRuntimeError> for MvpError 转换
- [x] C49 T3 RevenueComparator::new 空
- [x] C50 T4 RevenueComparator record_dual_brain + total
- [x] C51 T5 RevenueComparator record_traditional + total
- [x] C52 T6 RevenueComparator improvement_pct 正常计算
- [x] C53 T7 RevenueComparator improvement_pct trad=0 返回 INFINITY
- [x] C54 T8 RevenueComparator meets_target（≥ 10%）
- [x] C55 T9 RevenueComparator report 非空字符串
- [x] C56 T10 TraditionalEms::new 构造
- [x] C57 T11 TraditionalEms 谷时充电
- [x] C58 T12 TraditionalEms 峰时放电
- [x] C59 T13 TraditionalEms 平时保持
- [x] C60 T14 TraditionalEms 全 96 时段 + total_revenue 求和
- [x] C61 T15 MvpOrchestrator::new_default 构造
- [x] C62 T16 MvpOrchestrator::start 全部 Agent 转 Running
- [x] C63 T17 MvpOrchestrator::tick 未运行返回 NotRunning
- [x] C64 T18 MvpOrchestrator::tick 单周期 + tick_count += 1
- [x] C65 T19 MvpOrchestrator::tick 市场数据 market → energy 流转
- [x] C66 T20 MvpOrchestrator::tick 记录双脑 + 传统收益
- [x] C67 T21 MvpOrchestrator 多 tick 累积收益
- [x] C68 T22 MvpOrchestrator::stop 全部 Agent 转 Dead
- [x] C69 T23 MvpOrchestrator 端到端 start → 3 ticks → stop → report
- [x] C70 T24 MvpOrchestrator::report 委托 RevenueComparator
- [x] C71 `cargo test -p eneros-mvp-scenario` 全部通过

## 设计文档
- [x] C72 `docs/agents/mvp-scenario-design.md` 存在
- [x] C73 12 章节完整
- [x] C74 2 Mermaid 图（MVP tick 编排流程图 + 收益对比时序图）
- [x] C75 D1~D15 偏差声明表
- [x] C76 文档在 `docs/agents/` 下

## 版本同步
- [x] C77 `Makefile` 版本号 `0.74.0`（header + VERSION 变量 2 处）
- [x] C78 `.github/workflows/ci.yml` 版本号 `0.74.0`
- [x] C79 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-mvp-scenario`

## 构建校验（§2.4.2 C6~C11）
- [x] C80 `cargo metadata --format-version 1` 成功
- [x] C81 `cargo test -p eneros-mvp-scenario` 全部通过
- [x] C82 `cargo test -p eneros-energy-market-agent` 回归通过
- [x] C83 `cargo test -p eneros-device-agent` 回归通过
- [x] C84 `cargo build -p eneros-mvp-scenario --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C85 `cargo fmt -p eneros-mvp-scenario -- --check` 通过
- [x] C86 `cargo clippy -p eneros-mvp-scenario --all-targets -- -D warnings` 无 warning
- [x] C87 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C88 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C89 无 `panic!` / `todo!` / `unimplemented!`
- [x] C90 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C91 无 `unsafe` 块
- [x] C92 无 `SystemTime::now()` / `thread::sleep` / `uuid::Uuid::new_v4()`（D2/D3）
- [x] C93 无 `log::warn!` / `log::info!` / `log::error!`（D1）
- [x] C94 无 `std::collections::HashMap` / `std::sync::Mutex`

## 目录规范
- [x] C95 crate 在 `crates/agents/mvp-scenario/`
- [x] C96 跨 crate path 引用均为相对路径
- [x] C97 文档在 `docs/agents/` 下
- [x] C98 无根目录 crate（除 `ci/`）
- [x] C99 无垃圾文件

## 依赖复用
- [x] C100 复用 v0.72.0 `EnergyAgent` / `MarketAgent` / `AgentRuntime` / `HeartbeatStatus` / `AgentRuntimeError`
- [x] C101 复用 v0.73.0 `DeviceAgent`
- [x] C102 复用 v0.66.0 `ScheduleConfig` / `ScheduleResult` / `ScheduleEntry`
- [x] C103 复用 v0.64.0 `SolveStatus`

## 简化设计验证（Karpathy 原则）
- [x] C104 跳过 SystemAgent（D5：非出口标准必需）
- [x] C105 跳过 Watchdog（D6：运行时基础设施，集成层关注）
- [x] C106 跳过 AgentRegistry（D4：直接持有 Agent，无需 clone）
- [x] C107 合并 RevenueTracker/Comparator（D12：单一结构体）
- [x] C108 TraditionalEms 传基本类型（D13：不依赖 SystemState）
- [x] C109 tick 单步方法替代无限循环（D3：循环由集成测试驱动）
- [x] C110 无外科手术式变更（仅依赖 v0.72.0/v0.73.0 pub API）
