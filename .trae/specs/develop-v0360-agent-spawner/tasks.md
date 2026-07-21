# Tasks — v0.36.0 Agent 启动与初始化

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 错误扩展 → Wave 2 init 模块 → Wave 3 spawner 模块 → Wave 4 lib.rs → Wave 5 测试 → Wave 6 文档+版本 → Wave 7 验证
> **目标驱动**：每个任务附验证条件，可独立 loop 直到通过。

## Wave 1: 错误类型扩展（前置）

- [x] **Task 1: 扩展 AgentError 三个 String-carrying 变体**
  - 修改 `crates/agents/agent/src/error.rs`：
    - 在文件顶部 `use` 区追加 `use alloc::string::String;`（新变体需要 String）
    - 在 `AgentError` 枚举末尾（`AgentNotAlive` 之后）追加 3 个变体：
      - `CodeLoadFailed(String)` — 注释"代码加载失败"
      - `InitFailed(String)` — 注释"初始化失败"
      - `StartFailed(String)` — 注释"启动失败"
    - 在 `Display` impl 追加：
      - `AgentError::CodeLoadFailed(msg) => write!(f, "code load failed: {}", msg),`
      - `AgentError::InitFailed(msg) => write!(f, "init failed: {}", msg),`
      - `AgentError::StartFailed(msg) => write!(f, "start failed: {}", msg),`
    - 在 tests 模块追加 `test_spawn_error_variants_display`：
      - 验证 `CodeLoadFailed(String::from("unknown type"))` Display 输出 `"code load failed: unknown type"`
      - 验证 `InitFailed(String::from("timeout"))` Display 输出 `"init failed: timeout"`
      - 验证 `StartFailed(String::from("resource busy"))` Display 输出 `"start failed: resource busy"`
    - 在 tests 模块追加 `test_spawn_error_variants_eq`：
      - 验证 `CodeLoadFailed(String::from("a")) == CodeLoadFailed(String::from("a"))`
      - 验证 `CodeLoadFailed(String::from("a")) != CodeLoadFailed(String::from("b"))`
      - 验证 `InitFailed(String::from("x")) != StartFailed(String::from("x"))`（不同变体）
  - **不修改**既有 8 个变体
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo test -p eneros-agent` 全部通过

## Wave 2: init 模块（AgentConfig / AgentContext / AgentEntry）

- [x] **Task 2: 创建 init.rs — 配置、上下文与入口 trait**
  - 创建 `crates/agents/agent/src/init.rs`：
    - 模块文档注释（Agent 启动配置与入口 trait / AgentConfig 配置 / AgentContext 上下文 / AgentEntry 入口回调）
    - `use alloc::string::String;`
    - `use alloc::rc::Rc;`
    - `use core::cell::RefCell;`
    - `use crate::{AgentError, AgentId, AgentRegistry, AgentType};`
    - `AgentConfig` 结构体（6 字段，derive `Clone, Debug, PartialEq, Eq`）：
      ```rust
      pub struct AgentConfig {
          pub agent_type: AgentType,
          pub name: String,
          pub binary_path: Option<String>,
          pub config_path: Option<String>,
          pub priority_override: Option<u8>,
          pub mem_override: Option<usize>,
      }
      ```
    - `AgentContext` 结构体（3 字段，derive `Debug`，不 derive Clone/PartialEq）：
      ```rust
      pub struct AgentContext {
          pub agent_id: AgentId,
          pub config: AgentConfig,
          pub registry: Rc<RefCell<AgentRegistry>>,
      }
      ```
    - `AgentEntry` trait（object-safe）：
      ```rust
      pub trait AgentEntry {
          fn on_init(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>;
          fn on_start(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>;
          fn on_stop(&mut self, ctx: &mut AgentContext);
      }
      ```
    - 为 `AgentConfig` 实现 `Default`（方便测试构造，agent_type 默认 `AgentType::System`，name 默认 "default"，其他 None）
  - **验证**：`cargo build -p eneros-agent` 编译通过（需先在 lib.rs 声明模块，实际在 Task 4 统一声明）

## Wave 3: spawner 模块（AgentFactory / AgentSpawner）

- [x] **Task 3: 创建 spawner.rs — 工厂 trait 与启动器**
  - 创建 `crates/agents/agent/src/spawner.rs`：
    - 模块文档注释（Agent 启动器 / AgentSpawner / AgentFactory / spawn 流程 8 步 / D1~D5 偏差）
    - `use alloc::boxed::Box;`
    - `use alloc::rc::Rc;`
    - `use core::cell::RefCell;`
    - `use crate::{AgentConfig, AgentContext, AgentDescriptor, AgentEntry, AgentError, AgentId, AgentRegistry, AgentState, AgentType, LifecycleManager};`
    - `AgentFactory` trait（object-safe，D2 偏差）：
      ```rust
      pub trait AgentFactory {
          fn create(&self, agent_type: AgentType, name: &str) -> Result<Box<dyn AgentEntry>, AgentError>;
      }
      ```
    - `AgentSpawner` 结构体（D1 偏差：`Rc<RefCell<LifecycleManager>>`）：
      ```rust
      pub struct AgentSpawner {
          registry: Rc<RefCell<AgentRegistry>>,
          lifecycle: Rc<RefCell<LifecycleManager>>,
          factory: Rc<dyn AgentFactory>,
      }
      ```
    - `impl AgentSpawner`：
      - `pub fn new(registry: Rc<RefCell<AgentRegistry>>, lifecycle: Rc<RefCell<LifecycleManager>>, factory: Rc<dyn AgentFactory>) -> Self`
      - `pub fn spawn(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>` — D4 偏差（追加 `now` 参数）
        1. `let mut desc = AgentDescriptor::new(config.agent_type, &config.name, now);`
        2. `if let Some(p) = config.priority_override { desc.priority = p; }`
        3. `if let Some(m) = config.mem_override { desc.mem_quota = m; }`
        4. `let id = self.registry.borrow_mut().register(desc)?;`
        5. `self.lifecycle.borrow().transition(id, AgentState::Ready)?;` — Created→Ready
        6. `let mut agent = self.load_code(&config).map_err(|e| { let _ = self.lifecycle.borrow_mut().force_state(id, AgentState::Error); e })?;` — D5 偏差
        7. `let mut ctx = self.init_context(id, &config);`
        8. `agent.on_init(&mut ctx).map_err(|e| { let _ = self.lifecycle.borrow_mut().force_state(id, AgentState::Error); e })?;` — D5 偏差
        9. `self.lifecycle.borrow().transition(id, AgentState::Running)?;` — Ready→Running
        10. `agent.on_start(&mut ctx).map_err(|e| { let _ = self.lifecycle.borrow_mut().force_state(id, AgentState::Error); e })?;` — D5 偏差
        11. `Ok(id)`
      - `pub fn spawn_blocking(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>` — D3 偏差（委托 spawn）
        - `self.spawn(config, now)`
      - `fn load_code(&self, config: &AgentConfig) -> Result<Box<dyn AgentEntry>, AgentError>` — 委托 factory
        - `self.factory.create(config.agent_type, &config.name)`
      - `fn init_context(&self, id: AgentId, config: &AgentConfig) -> AgentContext`
        - `AgentContext { agent_id: id, config: config.clone(), registry: self.registry.clone() }`
  - **验证**：`cargo build -p eneros-agent` 编译通过

## Wave 4: lib.rs 更新

- [x] **Task 4: 更新 lib.rs — 模块声明与 re-export**
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在模块声明区追加 `pub mod init;`（在 `pub mod descriptor;` 之后）和 `pub mod spawner;`（在 `pub mod registry;` 之后）
    - 在 re-export 区追加：
      - `pub use init::{AgentConfig, AgentContext, AgentEntry};`
      - `pub use spawner::{AgentFactory, AgentSpawner};`
    - 更新 `VERSION`：`pub const VERSION: &str = "0.36.0";`
    - 更新文件头部文档注释：版本号 0.35.0 → 0.36.0，追加 init 和 spawner 模块说明：
      ```
      //! - [`AgentConfig`] / [`AgentContext`] / [`AgentEntry`] — Agent 启动配置与入口 trait
      //! - [`AgentSpawner`] / [`AgentFactory`] — Agent 启动器与工厂
      ```
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo doc -p eneros-agent` 无警告

## Wave 5: 测试

- [x] **Task 5: 编写 init.rs 单元测试**
  - 在 `init.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_agent_config_construction` — 构造 AgentConfig，验证 6 字段
    - `test_agent_config_clone_eq` — Clone 后 PartialEq/Eq 比较
    - `test_agent_config_default` — Default 实现返回合理值
    - `test_agent_config_with_overrides` — priority_override / mem_override 设置为 Some
    - `test_agent_context_construction` — 构造 AgentContext，验证 agent_id / config / registry 字段
    - `test_agent_entry_object_safe` — 定义 `struct TestAgent;` 实现 AgentEntry（on_init/on_start 返回 Ok，on_stop no-op），装箱为 `Box<dyn AgentEntry>`，调用方法
  - **验证**：`cargo test -p eneros-agent` 通过

- [x] **Task 6: 编写 spawner.rs 单元测试**
  - 在 `spawner.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - 辅助类型（测试 mod 内）：
      ```rust
      use super::*;
      use alloc::rc::Rc;
      use core::cell::RefCell;
      use crate::{AgentDescriptor, AgentRegistry, LifecycleManager};

      // 成功 Agent：on_init/on_start 返回 Ok
      struct SuccessAgent;
      impl AgentEntry for SuccessAgent {
          fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> { Ok(()) }
          fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> { Ok(()) }
          fn on_stop(&mut self, _ctx: &mut AgentContext) {}
      }

      // on_init 失败 Agent
      struct FailInitAgent;
      impl AgentEntry for FailInitAgent {
          fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
              Err(AgentError::InitFailed(String::from("test init failure")))
          }
          fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> { Ok(()) }
          fn on_stop(&mut self, _ctx: &mut AgentContext) {}
      }

      // on_start 失败 Agent
      struct FailStartAgent;
      impl AgentEntry for FailStartAgent {
          fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> { Ok(()) }
          fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
              Err(AgentError::StartFailed(String::from("test start failure")))
          }
          fn on_stop(&mut self, _ctx: &mut AgentContext) {}
      }

      // 成功 factory
      struct SuccessFactory;
      impl AgentFactory for SuccessFactory {
          fn create(&self, _agent_type: AgentType, _name: &str) -> Result<Box<dyn AgentEntry>, AgentError> {
              Ok(Box::new(SuccessAgent))
          }
      }

      // 失败 factory（load_code 失败）
      struct FailFactory;
      impl AgentFactory for FailFactory {
          fn create(&self, _agent_type: AgentType, _name: &str) -> Result<Box<dyn AgentEntry>, AgentError> {
              Err(AgentError::CodeLoadFailed(String::from("no agent registered")))
          }
      }

      // FailInit factory
      struct FailInitFactory;
      impl AgentFactory for FailInitFactory {
          fn create(&self, _agent_type: AgentType, _name: &str) -> Result<Box<dyn AgentEntry>, AgentError> {
              Ok(Box::new(FailInitAgent))
          }
      }

      // FailStart factory
      struct FailStartFactory;
      impl AgentFactory for FailStartFactory {
          fn create(&self, _agent_type: AgentType, _name: &str) -> Result<Box<dyn AgentEntry>, AgentError> {
              Ok(Box::new(FailStartAgent))
          }
      }

      fn make_spawner<F: AgentFactory + 'static>(factory: F) -> (AgentSpawner, Rc<RefCell<AgentRegistry>>) {
          let reg = Rc::new(RefCell::new(AgentRegistry::new()));
          let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
          let spawner = AgentSpawner::new(reg.clone(), lifecycle, Rc::new(factory));
          (spawner, reg)
      }

      fn make_config(agent_type: AgentType, name: &str) -> AgentConfig {
          AgentConfig {
              agent_type,
              name: String::from(name),
              binary_path: None,
              config_path: None,
              priority_override: None,
              mem_override: None,
          }
      }
      ```
    - 测试：
      - `test_spawn_success` — spawn 成功，Agent 进入 Running
      - `test_spawn_returns_agent_id` — 返回的 AgentId 在 registry 中存在
      - `test_spawn_blocking_equivalent_to_spawn` — spawn_blocking 同样成功
      - `test_spawn_on_init_failure_goes_to_error` — FailInitFactory，Agent 进入 Error，spawn 返回 InitFailed
      - `test_spawn_on_start_failure_goes_to_error` — FailStartFactory，Agent 进入 Error，spawn 返回 StartFailed
      - `test_spawn_load_code_failure_goes_to_error` — FailFactory，Agent 进入 Error，spawn 返回 CodeLoadFailed
      - `test_spawn_priority_override_applied` — priority_override: Some(200)，验证 registry 中 desc.priority == 200
      - `test_spawn_mem_override_applied` — mem_override: Some(1024)，验证 desc.mem_quota == 1024
      - `test_spawn_multiple_agents_independent` — spawn 3 个 Agent，各自 Running，count == 3
      - `test_spawn_registers_in_registry` — spawn 后 registry.exists(id) == true
      - `test_spawn_created_to_ready_to_running` — 验证状态序列（通过 lifecycle.current_state 在各步骤后检查 — 注意：由于 spawn 是原子的，需通过成功后验证最终状态为 Running，以及失败路径验证 Error）
  - **验证**：`cargo test -p eneros-agent` 全部通过

- [x] **Task 7: 编写集成测试 tests/spawner_test.rs**
  - 创建 `crates/agents/agent/tests/spawner_test.rs`：
    - `use eneros_agent::{AgentConfig, AgentContext, AgentEntry, AgentError, AgentFactory, AgentRegistry, AgentSpawner, AgentState, AgentType, LifecycleManager};`
    - `use std::rc::Rc;` / `use std::cell::RefCell;` / `use std::string::String;`（集成测试可用 std）
    - 定义测试辅助（SuccessAgent / FailInitAgent / FailStartAgent / SuccessFactory / FailFactory / FailInitFactory / FailStartFactory）— 与单元测试类似
    - 集成测试：
      - `integration_spawn_full_success_path` — 完整成功路径，验证最终状态 Running
      - `integration_spawn_init_failure_error_state` — on_init 失败，验证 Error 状态 + 错误返回
      - `integration_spawn_start_failure_error_state` — on_start 失败，验证 Error 状态 + 错误返回
      - `integration_spawn_load_code_failure` — factory 失败，验证 Error 状态 + CodeLoadFailed
      - `integration_spawn_blocking_same_as_spawn` — spawn_blocking 与 spawn 行为一致
      - `integration_spawn_multiple_agents` — 多 Agent 独立 spawn，各自 Running
      - `integration_spawn_with_overrides` — priority/mem override 生效
      - `integration_spawn_agent_context_correct` — 验证 on_init 收到的 ctx.agent_id / ctx.config 正确（通过 SuccessAgent 记录 ctx 信息）
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 6: 文档与版本标识

- [x] **Task 8: 编写设计文档**
  - 创建 `docs/agents/agent-spawner-design.md`：
    - 版本目标 / 架构定位 / 前置依赖
    - spawn 流程 8 步（含 mermaid flowchart，复制蓝图 §4.3）
    - 数据结构设计（AgentConfig / AgentContext / AgentEntry / AgentFactory / AgentSpawner）
    - 模块结构（init.rs + spawner.rs）
    - 偏差声明 D1~D5
    - 错误处理（CodeLoadFailed / InitFailed / StartFailed + force_state 清理）
    - 并发设计（Rc<RefCell> 单线程 / Phase 3 替换策略）
    - 工厂设计（AgentFactory trait / 静态注册 / Phase 3 动态加载）
    - on_stop 预留（v0.36.0 不调用 / v0.38.0 崩溃恢复使用）
    - 性能分析（spawn < 100ms 目标 / 当前无实际代码加载开销）
    - 后续解锁版本（v0.37.0 / v0.38.0）
  - **验证**：文档存在且内容完整

- [x] **Task 9: 同步版本标识**
  - 根 `Cargo.toml`：`version = "0.36.0"`
  - `Makefile`：`VERSION := 0.36.0` + header 注释 + agent-build 描述更新为 "v0.36.0 spawner"
  - `.github/workflows/ci.yml`：`Version: v0.36.0`
  - `ci/src/gate.rs`：注释更新为 v0.36.0
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.36.0"`（Task 4 已完成）
  - **验证**：`grep -r "0.35.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（历史注释除外）

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
- Task 2: 无依赖（init 模块独立于 error.rs，但 lib.rs 声明在 Task 4）
- Task 3: 依赖 Task 1（需要新错误变体）+ Task 2（需要 AgentConfig/AgentContext/AgentEntry）
- Task 4: 依赖 Task 2 + Task 3（lib.rs 声明模块前，两模块应完整）
- Task 5: 依赖 Task 4（单元测试需要模块可被引用）
- Task 6: 依赖 Task 5（spawner 测试在 init 测试后）
- Task 7: 依赖 Task 6（集成测试在单元测试后）
- Task 8-9: 依赖 Task 4（可并行，文档与版本标识独立）
- Task 10: 依赖 Task 1-9 全部完成
