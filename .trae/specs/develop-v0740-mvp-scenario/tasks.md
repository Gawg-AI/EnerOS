# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.73.0` → `0.74.0`
  - [x] members 添加 `crates/agents/mvp-scenario`（置于 `crates/agents/device-agent` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-mvp-scenario` crate 骨架
  - [x] 新建 `crates/agents/mvp-scenario/Cargo.toml`，package name = `eneros-mvp-scenario`
  - [x] dependencies：`eneros-energy-market-agent` / `eneros-device-agent` / `eneros-energy-lp-model` / `eneros-agent`（均为相对路径）
  - [x] 无 `[features]` 段（纯 Rust）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / revenue / traditional_ems / orchestrator
  - [x] lib.rs 包含 D1~D15 偏差声明表

- [x] Task 3: 实现 `error.rs` — MvpError
  - [x] `MvpError` 枚举：`AgentError(AgentRuntimeError)` / `NotRunning`
  - [x] 派生 `Debug`
  - [x] 实现 `fmt::Display`
  - [x] 实现 `From<AgentRuntimeError> for MvpError`

- [x] Task 4: 实现 `revenue.rs` — RevenueComparator
  - [x] `RevenueComparator` 结构体：`dual_brain_revenue: Vec<f64>` / `traditional_revenue: Vec<f64>`
  - [x] `new() -> Self`（空构造）
  - [x] `record_dual_brain(&mut self, revenue: f64)` — push 到 dual_brain_revenue
  - [x] `record_traditional(&mut self, revenue: f64)` — push 到 traditional_revenue
  - [x] `dual_brain_total(&self) -> f64` — sum
  - [x] `traditional_total(&self) -> f64` — sum
  - [x] `improvement_pct(&self) -> f64` — `(dual - trad) / trad * 100`，trad==0 返回 INFINITY
  - [x] `meets_target(&self) -> bool` — `improvement_pct() >= 10.0`
  - [x] `report(&self) -> String` — 格式化对比报告（含双脑总收益/传统总收益/提升百分比/达标结果）

- [x] Task 5: 实现 `traditional_ems.rs` — TraditionalEms
  - [x] `TraditionalEms` 结构体：`config: ScheduleConfig`
  - [x] `new(config: ScheduleConfig) -> Self`
  - [x] `schedule(&self, current_price: f64, soc: f64) -> ScheduleResult`（D13：传基本类型）
    - [x] 遍历 `0..config.num_periods`
    - [x] 每时段：`price = config.price[t]`
    - [x] `price < 0.3` → `(config.pcs_power_kw, 0.0)` 谷充
    - [x] `price > 0.8` → `(0.0, config.pcs_power_kw)` 峰放
    - [x] 否则 → `(0.0, 0.0)` 平保持
    - [x] 构建 `ScheduleEntry { period: t, charge_power_kw, discharge_power_kw, net_power_kw: discharge - charge, soc_pct: soc, revenue_yuan: (discharge - charge) * price * config.period_hours }`
    - [x] `total_revenue_yuan` = 各时段收益之和
    - [x] `objective_value` = total_revenue_yuan
    - [x] `solve_status` = `SolveStatus::Optimal`

- [x] Task 6: 实现 `orchestrator.rs` — MvpOrchestrator
  - [x] `MvpOrchestrator` 结构体：`energy_agent: EnergyAgent` / `market_agent: MarketAgent` / `device_agent: DeviceAgent` / `revenue_comparator: RevenueComparator` / `traditional_ems: TraditionalEms` / `tick_count: u64` / `running: bool`
  - [x] `new_default(now_ms: u64) -> Self` — 3 个 Agent 用 `new_default`，`TraditionalEms::new(ScheduleConfig::default())`，`RevenueComparator::new()`，`running = false`
  - [x] `new(energy, market, device, config, now_ms) -> Self` — 自定义 Agent 构造
  - [x] `start(&mut self, now_ms: u64) -> Result<(), MvpError>` — 依次调用 3 个 Agent 的 `on_start(now_ms)`，失败返回 `MvpError::AgentError`，成功后 `running = true`
  - [x] `tick(&mut self, now_ms: u64) -> Result<MvpTickReport, MvpError>`
    - [x] `running == false` → 返回 `Err(MvpError::NotRunning)`
    - [x] Step 1：`self.market_agent.on_tick(now_ms)?`
    - [x] Step 2：转发市场数据 `while let Some(data) = market_agent.market_channel.try_recv() { energy_agent.market_channel_mut().send(data)?; }`
    - [x] Step 3：`self.energy_agent.on_tick(now_ms)?`
    - [x] Step 4：若 `energy_agent.current_schedule` 为 `Some(schedule)`：
      - [x] `revenue_comparator.record_dual_brain(schedule.total_revenue_yuan)`
      - [x] `let trad = traditional_ems.schedule(energy_agent.current_price, 0.5);`（soc 用默认 0.5，因 EnergyAgent 不暴露 soc）
      - [x] `revenue_comparator.record_traditional(trad.total_revenue_yuan)`
    - [x] Step 5：`self.device_agent.on_tick(now_ms)?`
    - [x] Step 6：`tick_count += 1`
    - [x] 返回 `MvpTickReport { tick: tick_count, dual_brain_revenue, traditional_revenue, improvement_pct }`
  - [x] `stop(&mut self, now_ms: u64) -> Result<(), MvpError>` — 依次调用 3 个 Agent 的 `on_stop(now_ms)`，`running = false`
  - [x] `report(&self) -> String` — 委托 `revenue_comparator.report()`
  - [x] `MvpTickReport` 结构体：`tick: u64` / `dual_brain_revenue: f64` / `traditional_revenue: f64` / `improvement_pct: f64`

- [x] Task 7: 集成测试（lib.rs `#[cfg(test)] mod tests`）— 至少 22 个测试
  - [x] T1 MvpError 变体构造（AgentError + NotRunning）
  - [x] T2 From<AgentRuntimeError> for MvpError 转换
  - [x] T3 RevenueComparator::new 空
  - [x] T4 RevenueComparator::record_dual_brain + dual_brain_total
  - [x] T5 RevenueComparator::record_traditional + traditional_total
  - [x] T6 RevenueComparator::improvement_pct 正常计算（dual=100, trad=80 → 25.0）
  - [x] T7 RevenueComparator::improvement_pct traditional=0 返回 INFINITY
  - [x] T8 RevenueComparator::meets_target（improvement ≥ 10%）
  - [x] T9 RevenueComparator::report 返回非空字符串
  - [x] T10 TraditionalEms::new 构造
  - [x] T11 TraditionalEms::schedule 谷时充电（price < 0.3）
  - [x] T12 TraditionalEms::schedule 峰时放电（price > 0.8）
  - [x] T13 TraditionalEms::schedule 平时保持（0.3 ≤ price ≤ 0.8）
  - [x] T14 TraditionalEms::schedule 全 96 时段 + total_revenue_yuan 求和正确
  - [x] T15 MvpOrchestrator::new_default 构造 + 3 Agent + tick_count=0 + running=false
  - [x] T16 MvpOrchestrator::start 全部 Agent 转 Running + running=true
  - [x] T17 MvpOrchestrator::tick 未运行返回 NotRunning 错误
  - [x] T18 MvpOrchestrator::tick 单周期执行 + tick_count += 1
  - [x] T19 MvpOrchestrator::tick 市场数据从 market → energy 流转（current_price 更新）
  - [x] T20 MvpOrchestrator::tick 记录双脑收益 + 传统收益
  - [x] T21 MvpOrchestrator 多 tick 累积收益
  - [x] T22 MvpOrchestrator::stop 全部 Agent 转 Dead + running=false
  - [x] T23 MvpOrchestrator 端到端：start → 3 ticks → stop → report 非空
  - [x] T24 MvpOrchestrator::report 委托 RevenueComparator

- [x] Task 8: 创建设计文档 `docs/agents/mvp-scenario-design.md`
  - [x] 12 章节完整（版本目标 / 前置依赖 / 交付物 / 详细设计 / 技术交底 / 测试计划 / 验收标准 / 风险 / 多角度要求 / ADR / 偏差声明 / no_std 合规）
  - [x] 2 Mermaid 图（MVP tick 编排流程图 + 收益对比时序图）
  - [x] D1~D15 偏差声明表
  - [x] 文档位于 `docs/agents/` 下

- [x] Task 9: 版本同步
  - [x] `Makefile` 版本号 `0.74.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.74.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-mvp-scenario`

- [x] Task 10: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-mvp-scenario` 全部通过
  - [x] `cargo test -p eneros-energy-market-agent` 通过（回归）
  - [x] `cargo test -p eneros-device-agent` 通过（回归）
  - [x] `cargo build -p eneros-mvp-scenario --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-mvp-scenario -- --check` 通过
  - [x] `cargo clippy -p eneros-mvp-scenario --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 依赖 Task 1
- Task 3~6 依赖 Task 2（并行实现）
- Task 7 依赖 Task 3~6
- Task 8 可与 Task 3~7 并行
- Task 9 依赖 Task 2
- Task 10 依赖 Task 3~9 全部完成
