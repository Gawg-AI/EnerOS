# Tasks — v0.37.0 Agent 心跳与健康检查

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 错误扩展 → Wave 2 health 模块 → Wave 3 heartbeat 模块 → Wave 4 lib.rs → Wave 5 测试 → Wave 6 文档+版本 → Wave 7 验证
> **目标驱动**：每个任务附验证条件，可独立 loop 直到通过。

## Wave 1: 错误类型扩展（前置）

- [x] **Task 1: 扩展 AgentError 两个心跳错误变体**
  - 修改 `crates/agents/agent/src/error.rs`：
    - 在 `AgentError` 枚举末尾（`StartFailed(String)` 之后）追加 2 个变体：
      - `HeartbeatTimeout { agent_id: AgentId, missed: u32 }` — 注释"心跳超时"
      - `AgentUnhealthy { agent_id: AgentId }` — 注释"Agent 不健康"
    - 在 `use` 区追加 `use crate::id::AgentId;`（新变体需要 AgentId）
    - 在 `Display` impl 追加：
      - `AgentError::HeartbeatTimeout { agent_id, missed } => write!(f, "heartbeat timeout: agent {:?} missed {} beats", agent_id, missed),`
      - `AgentError::AgentUnhealthy { agent_id } => write!(f, "agent unhealthy: {:?}", agent_id),`
    - 在 tests 模块追加 `test_heartbeat_error_variants_display`：
      - 验证 `HeartbeatTimeout { agent_id: AgentId(42), missed: 3 }` Display 输出含 "heartbeat timeout" 和 "missed 3 beats"
      - 验证 `AgentUnhealthy { agent_id: AgentId(42) }` Display 输出含 "agent unhealthy"
    - 在 tests 模块追加 `test_heartbeat_error_variants_eq`：
      - 验证 `HeartbeatTimeout { agent_id: AgentId(1), missed: 2 } == HeartbeatTimeout { agent_id: AgentId(1), missed: 2 }`
      - 验证 `HeartbeatTimeout { agent_id: AgentId(1), missed: 2 } != HeartbeatTimeout { agent_id: AgentId(1), missed: 3 }`（missed 不同）
      - 验证 `AgentUnhealthy { agent_id: AgentId(1) } != HeartbeatTimeout { agent_id: AgentId(1), missed: 0 }`（不同变体）
  - **不修改**既有 11 个变体
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo test -p eneros-agent` 全部通过

## Wave 2: health 模块（HealthStatus + HealthCheck）

- [x] **Task 2: 创建 health.rs — HealthStatus 枚举与 HealthCheck trait**
  - 创建 `crates/agents/agent/src/health.rs`：
    - 模块文档注释（Agent 健康检查 / HealthStatus 健康状态枚举 / HealthCheck 自定义健康检查 trait）
    - `HealthStatus` 枚举（4 变体，derive `Clone, Copy, Debug, PartialEq, Eq`，D3 偏差）：
      ```rust
      pub enum HealthStatus {
          Healthy,
          Degraded,
          Unhealthy,
          Dead,
      }
      ```
      每个变体加文档注释
    - `HealthCheck` trait（object-safe）：
      ```rust
      pub trait HealthCheck {
          fn check_health(&self) -> HealthStatus;
      }
      ```
      文档注释说明：Agent 可实现此 trait 提供自定义健康检查（蓝图 §9.7），v0.37.0 仅定义 trait 不主动调用
  - **验证**：`cargo build -p eneros-agent` 编译通过（需先在 lib.rs 声明模块，实际在 Task 4 统一声明）

## Wave 3: heartbeat 模块（HeartbeatMonitor + HeartbeatState）

- [x] **Task 3: 创建 heartbeat.rs — 心跳监控器**
  - 创建 `crates/agents/agent/src/heartbeat.rs`：
    - 模块文档注释（Agent 心跳监控 / HeartbeatMonitor / HeartbeatState / check 算法 / D1~D7 偏差）
    - `use alloc::collections::BTreeMap;`（D1 偏差）
    - `use alloc::vec::Vec;`
    - `use crate::health::HealthStatus;`
    - `use crate::id::AgentId;`
    - 常量：
      ```rust
      const DEFAULT_INTERVAL_MS: u64 = 1000;
      const DEFAULT_MAX_MISSED: u32 = 3;
      ```
    - `HeartbeatState` 结构体（4 字段，derive `Clone, Debug`，D4 偏差）：
      ```rust
      pub struct HeartbeatState {
          pub last_heartbeat: u64,
          pub missed_count: u32,
          pub status: HealthStatus,
          pub interval_ms: u64,
      }
      ```
    - `HeartbeatMonitor` 结构体（3 字段，derive `Debug`，D4 偏差）：
      ```rust
      pub struct HeartbeatMonitor {
          agents: BTreeMap<AgentId, HeartbeatState>,
          default_interval_ms: u64,
          max_missed: u32,
      }
      ```
    - `impl HeartbeatMonitor`：
      - `pub fn new(interval_ms: u64, max_missed: u32) -> Self`
      - `pub fn register(&mut self, id: AgentId, now: u64)` — D2 偏差（追加 now 参数）
        - 插入 `HeartbeatState { last_heartbeat: now, missed_count: 0, status: HealthStatus::Healthy, interval_ms: self.default_interval_ms }`
      - `pub fn heartbeat(&mut self, id: AgentId, timestamp: u64)`
        - 若 agent 存在：`last_heartbeat = timestamp`，`missed_count = 0`，`status = Healthy`
      - `pub fn check(&mut self, now: u64) -> Vec<(AgentId, HealthStatus)>`
        - 遍历 `agents.iter_mut()`，对每个 agent：
          - `elapsed = now.saturating_sub(state.last_heartbeat)` — 防溢出
          - 若 `elapsed > state.interval_ms`：
            - `state.missed_count = (elapsed / state.interval_ms) as u32`
            - 若 `missed_count >= self.max_missed` → `status = Unhealthy`（D7：不设 Dead）
            - 否则若 `missed_count > 0` → `status = Degraded`
          - `results.push((id, state.status))`
        - 返回 results
      - `pub fn is_healthy(&self, id: AgentId) -> bool`
        - `agents.get(&id).map(|s| matches!(s.status, HealthStatus::Healthy)).unwrap_or(false)`
      - `pub fn set_interval(&mut self, id: AgentId, interval_ms: u64)`
        - 若 agent 存在：`state.interval_ms = interval_ms`
      - `pub fn unregister(&mut self, id: AgentId)`
        - `agents.remove(&id);`
  - **验证**：`cargo build -p eneros-agent` 编译通过

## Wave 4: lib.rs 更新

- [x] **Task 4: 更新 lib.rs — 模块声明与 re-export**
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在模块声明区追加 `pub mod health;`（在 `pub mod error;` 之后）和 `pub mod heartbeat;`（在 `pub mod health;` 之后）
    - 在 re-export 区追加：
      - `pub use health::{HealthCheck, HealthStatus};`
      - `pub use heartbeat::{HeartbeatMonitor, HeartbeatState};`
    - 更新 `VERSION`：`pub const VERSION: &str = "0.37.0";`
    - 更新文件头部文档注释：版本号 0.36.0 → 0.37.0，追加 health 和 heartbeat 模块说明：
      ```
      //! - [`HealthStatus`] / [`HealthCheck`] — Agent 健康状态与自定义健康检查
      //! - [`HeartbeatMonitor`] / [`HeartbeatState`] — Agent 心跳监控（1s 周期、3 次超时=故障）
      ```
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo doc -p eneros-agent` 无警告

## Wave 5: 测试

- [x] **Task 5: 编写 health.rs 单元测试**
  - 在 `health.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_health_status_clone_copy` — 验证 HealthStatus 可 Clone/Copy（赋值后相等）
    - `test_health_status_debug` — 验证 Debug 输出含变体名
    - `test_health_status_eq` — 验证 Healthy == Healthy, Healthy != Degraded, 4 变体互不相等
    - `test_health_status_ordering` — 验证所有 4 变体可收集到 Vec 并去重
    - `test_health_check_object_safe` — 定义 `struct AlwaysHealthy;` 实现 HealthCheck（返回 Healthy），装箱为 `Box<dyn HealthCheck>`，调用 `check_health()` 返回 Healthy
    - `test_health_check_custom_impl` — 定义 `struct CustomChecker { status: HealthStatus }` 实现 HealthCheck（返回 self.status），验证可返回任意状态
  - **验证**：`cargo test -p eneros-agent` 通过

- [x] **Task 6: 编写 heartbeat.rs 单元测试**
  - 在 `heartbeat.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_new_defaults` — `HeartbeatMonitor::new(1000, 3)` 构造，验证默认值
    - `test_register_agent` — register 后，is_healthy == true
    - `test_heartbeat_updates_state` — register → check(Degraded) → heartbeat → is_healthy == true
    - `test_check_healthy_no_missed` — register(now=1000) + check(now=1500) → Healthy（elapsed=500 <= 1000）
    - `test_check_degraded_one_missed` — register(now=1000) + check(now=2500) → Degraded（elapsed=1500 > 1000, missed=1）
    - `test_check_unhealthy_max_missed` — register(now=1000) + check(now=4500) → Unhealthy（elapsed=3500, missed=3 >= max_missed=3）
    - `test_check_unhealthy_exceeds_max` — register(now=1000) + check(now=10000) → Unhealthy（elapsed=9000, missed=9 >= 3）
    - `test_is_healthy_unregistered` — 未注册 agent → is_healthy == false
    - `test_is_healthy_degraded` — Degraded 状态 → is_healthy == false
    - `test_set_interval_override` — register(default=1000) + set_interval(500) + check(now=1750) → Degraded（elapsed=750 > 500, missed=1）
    - `test_unregister_removes_agent` — register + unregister + check → 结果 Vec 不含该 agent
    - `test_check_empty_monitor` — 空 monitor + check → 返回空 Vec
    - `test_check_multiple_agents` — 注册 3 个 agent，各有不同 last_heartbeat，check 返回 3 条结果
    - `test_check_multiple_agents_independent` — agent A 健康（有心跳），agent B 超时（无心跳），check 返回 A=Healthy, B=Degraded/Unhealthy
    - `test_clock_rollback_saturating` — register(now=2000) + check(now=1000) → elapsed=0（saturating_sub），不触发超时，status 保持 Healthy
    - `test_heartbeat_resets_missed_count` — register → check(Degraded, missed=2) → heartbeat → check(now 略后) → missed_count=0, Healthy
    - `test_check_boundary_exact_interval` — register(now=1000) + check(now=2000) → elapsed=1000，不大于 interval=1000，不触发超时（边界：> 而非 >=）
  - **验证**：`cargo test -p eneros-agent` 全部通过

- [x] **Task 7: 编写集成测试 tests/heartbeat_test.rs**
  - 创建 `crates/agents/agent/tests/heartbeat_test.rs`：
    - `use eneros_agent::{AgentId, HealthCheck, HealthStatus, HeartbeatMonitor, HeartbeatState};`
    - `use std::collections::BTreeMap;`（集成测试可用 std）
    - 集成测试：
      - `integration_heartbeat_full_lifecycle` — register → heartbeat → check(Healthy) → 停止心跳 → check(Degraded) → check(Unhealthy)
      - `integration_heartbeat_recovery` — register → 超时到 Degraded → heartbeat 恢复 → check(Healthy)
      - `integration_multiple_agents_independent` — 3 个 agent，各自独立心跳/超时
      - `integration_set_interval_affects_timing` — agent A 默认 1000ms，agent B set_interval(500ms)，同时停止心跳，B 先进入 Degraded
      - `integration_unregister_stops_monitoring` — register + unregister + check → 不含该 agent
      - `integration_health_check_trait` — 实现 HealthCheck 的自定义结构，验证 Box<dyn HealthCheck> 可调用
      - `integration_health_status_all_variants` — 验证 4 个 HealthStatus 变体可构造、比较
      - `integration_clock_rollback_safe` — register(now=5000) + check(now=1000) → 不触发超时（saturating_sub）
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 6: 文档与版本标识

- [x] **Task 8: 编写设计文档**
  - 创建 `docs/agents/agent-heartbeat-design.md`：
    - 版本目标 / 架构定位 / 前置依赖
    - check 算法流程（含 mermaid flowchart，复制蓝图 §4.3）
    - 数据结构设计（HealthStatus / HeartbeatState / HeartbeatMonitor / HealthCheck）
    - 模块结构（health.rs + heartbeat.rs）
    - 偏差声明 D1~D7
    - 错误处理（HeartbeatTimeout / AgentUnhealthy）
    - 心跳协议设计（1s 周期 / 3 次超时 = 故障 / per-Agent 间隔可配置）
    - 独立监控器设计（D6：不引用 registry/lifecycle，v0.38.0 集成）
    - Dead 状态归属（D7：v0.38.0 设置，非 v0.37.0）
    - 时钟回拨处理（saturating_sub）
    - 性能分析（check 遍历 O(n)，n = Agent 数）
    - 后续解锁版本（v0.38.0 崩溃恢复）
  - **验证**：文档存在且内容完整

- [x] **Task 9: 同步版本标识**
  - 根 `Cargo.toml`：`version = "0.37.0"`
  - `Makefile`：`VERSION := 0.37.0` + header 注释 + agent-build 描述更新为 "v0.37.0 Heartbeat"
  - `.github/workflows/ci.yml`：`Version: v0.37.0`
  - `ci/src/gate.rs`：注释更新为 v0.37.0
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.37.0"`（Task 4 已完成）
  - **验证**：`grep -r "0.36.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（历史注释除外）

## Wave 7: 构建验证

- [x] **Task 10: 全量构建与质量验证**
  - `cargo fmt --all -- --check`
  - `cargo clippy -p eneros-agent --all-targets -- -D warnings`
  - `cargo test -p eneros-agent`（含新增单元 + 集成测试）
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（回归）
  - `cargo run -p eneros-ci`（Overall: PASS，audit 步骤可能因 GitHub 网络不可达失败 — 已知环境问题）
  - WSL2: `cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - `cargo deny check licenses bans sources`
  - **验证**：全部 PASS（audit 除外，已知网络问题）

## Task Dependencies

- Task 1: 无依赖（错误类型扩展先行）
- Task 2: 无依赖（health 模块独立于 error.rs）
- Task 3: 依赖 Task 1（需要新错误变体引用）+ Task 2（需要 HealthStatus）
- Task 4: 依赖 Task 2 + Task 3（lib.rs 声明模块前，两模块应完整）
- Task 5: 依赖 Task 4（单元测试需要模块可被引用）
- Task 6: 依赖 Task 5（heartbeat 测试在 health 测试后）
- Task 7: 依赖 Task 6（集成测试在单元测试后）
- Task 8-9: 依赖 Task 4（可并行，文档与版本标识独立）
- Task 10: 依赖 Task 1-9 全部完成
