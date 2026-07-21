# Tasks — v0.35.0 Agent 生命周期状态机

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 错误扩展 → Wave 2 转换表 → Wave 3 状态机 → Wave 4 lib.rs → Wave 5 测试 → Wave 6 文档+版本 → Wave 7 验证
> **目标驱动**：每个任务附验证条件，可独立 loop 直到通过。

## Wave 1: 错误类型扩展（前置）

- [x] **Task 1: 扩展 AgentError 两个新变体**
  - 修改 `crates/agents/agent/src/error.rs`：
    - 追加结构变体 `InvalidStateTransition { from: AgentState, to: AgentState }`（注释：非法状态转换）
    - 追加单元变体 `AgentNotAlive`（注释：Agent 不在存活状态）
    - 在文件顶部 `use` 区追加 `use crate::types::AgentState;`（InvalidStateTransition 需要 AgentState）
    - 在 `Display` impl 追加：
      - `InvalidStateTransition { from, to } => write!(f, "invalid state transition: {:?} -> {:?}", from, to)`
      - `AgentNotAlive => write!(f, "agent not alive")`
    - 在 tests 模块追加 `test_lifecycle_error_variants_display`：验证 InvalidStateTransition 和 AgentNotAlive 的 Display 输出
    - 在 tests 模块追加 `test_invalid_state_transition_eq`：验证 `InvalidStateTransition { from: Created, to: Running } == InvalidStateTransition { from: Created, to: Running }` 且 `!= InvalidStateTransition { from: Created, to: Dead }`
  - **不修改**既有 6 个变体
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo test -p eneros-agent` 全部通过

## Wave 2: 转换规则模块

- [x] **Task 2: 创建 lifecycle/transitions.rs — 转换表与查询函数**
  - 创建目录 `crates/agents/agent/src/lifecycle/`
  - 创建 `crates/agents/agent/src/lifecycle/transitions.rs`：
    - 模块文档注释（合法状态转换表 / 12 条转换 / Dead 不可逆 / Error→Running 非法）
    - `use crate::types::AgentState;`
    - `pub const TRANSITIONS: &[(AgentState, AgentState)]` — 12 条合法转换的数组（按蓝图 §4.1 顺序）
    - `pub fn can_transition(from: AgentState, to: AgentState) -> bool` — `TRANSITIONS.contains(&(from, to))`
  - **验证**：`cargo build -p eneros-agent` 编译通过（需先在 lib.rs 声明模块，或暂用 `#[path]`；实际在 Task 4 统一声明）

## Wave 3: 生命周期状态机

- [x] **Task 3: 创建 lifecycle.rs — LifecycleManager + LifecycleHook + LifecycleEvent**
  - 创建 `crates/agents/agent/src/lifecycle.rs`：
    - 模块文档注释（生命周期状态机 / Rc<RefCell> 设计 D1 / force_state 不触发 hooks D2 / Hook 在 RefCell 借用期间调用 D5）
    - `pub mod transitions;`（声明子模块）
    - `pub use transitions::{can_transition, TRANSITIONS};`（re-export）
    - `use alloc::boxed::Box;` / `use alloc::rc::Rc;` / `use alloc::string::String;` / `use alloc::vec::Vec;`
    - `use core::cell::RefCell;`
    - `use crate::{AgentError, AgentId, AgentRegistry, AgentState};`
    - `pub trait LifecycleHook`：`on_enter(&self, state: AgentState, id: AgentId)` + `on_exit(&self, state: AgentState, id: AgentId)`
    - `pub enum LifecycleEvent`：`StateChanged { from: AgentState, to: AgentState, agent_id: AgentId }` + `TransitionRejected { from: AgentState, to: AgentState, reason: String }`，derive Debug, Clone, PartialEq, Eq
    - `pub struct LifecycleManager { registry: Rc<RefCell<AgentRegistry>>, hooks: Vec<Box<dyn LifecycleHook>> }`
    - `impl LifecycleManager`：
      - `pub fn new(registry: Rc<RefCell<AgentRegistry>>) -> Self` — hooks 初始化为空 Vec
      - `pub fn add_hook(&mut self, hook: Box<dyn LifecycleHook>)` — push 到 hooks Vec（D3 偏差）
      - `pub fn can_transition(&self, from: AgentState, to: AgentState) -> bool` — 委托 `transitions::can_transition`
      - `pub fn transition(&self, id: AgentId, target: AgentState) -> Result<AgentState, AgentError>`：
        1. `let mut reg = self.registry.borrow_mut();`
        2. `let desc = reg.get_mut(id).ok_or(AgentError::AgentNotFound)?;`
        3. `let from = desc.state;`
        4. `if !self.can_transition(from, target) { return Err(AgentError::InvalidStateTransition { from, to: target }); }`
        5. `for hook in &self.hooks { hook.on_exit(from, id); }`
        6. `desc.state = target;`
        7. `for hook in &self.hooks { hook.on_enter(target, id); }`
        8. `Ok(target)`
      - `pub fn current_state(&self, id: AgentId) -> Result<AgentState, AgentError>`：
        `let reg = self.registry.borrow(); reg.get(id).map(|d| d.state).ok_or(AgentError::AgentNotFound)`
      - `pub fn force_state(&mut self, id: AgentId, state: AgentState) -> Result<(), AgentError>`：
        1. `let mut reg = self.registry.borrow_mut();`
        2. `let desc = reg.get_mut(id).ok_or(AgentError::AgentNotFound)?;`
        3. `desc.state = state;`（直接设置，不验证，不触发 hooks — D2 偏差）
        4. `Ok(())`
  - **验证**：`cargo build -p eneros-agent` 编译通过

## Wave 4: lib.rs 更新

- [x] **Task 4: 更新 lib.rs — 模块声明与 re-export**
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在模块声明区追加 `pub mod lifecycle;`
    - 在 re-export 区追加 `pub use lifecycle::{LifecycleEvent, LifecycleHook, LifecycleManager};`
    - 更新 `VERSION`：`pub const VERSION: &str = "0.35.0";`
    - 更新文件头部文档注释：版本号 0.34.0 → 0.35.0，追加 lifecycle 模块说明
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo doc -p eneros-agent` 无警告

## Wave 5: 测试

- [x] **Task 5: 编写 transitions.rs 单元测试**
  - 在 `transitions.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_all_12_legal_transitions`：遍历 TRANSITIONS 表，逐条验证 `can_transition` 返回 true
    - `test_created_to_running_illegal`：`can_transition(Created, Running)` == false（必须经过 Ready）
    - `test_error_to_running_illegal`：`can_transition(Error, Running)` == false（蓝图 §8.5，必须经过 Recovering）
    - `test_dead_to_anything_illegal`：遍历所有 6 个非 Dead 状态，`can_transition(Dead, s)` == false（Dead 不可逆）
    - `test_self_transitions_illegal`：遍历 7 个状态，`can_transition(s, s)` == false
    - `test_no_legal_transition_to_created`：`can_transition(Ready, Created)` == false（不能回到 Created）
    - `test_transitions_count`：`TRANSITIONS.len()` == 12
  - **验证**：`cargo test -p eneros-agent` 通过

- [x] **Task 6: 编写 lifecycle.rs 单元测试**
  - 在 `lifecycle.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - 辅助函数 `make_registry_with_agent(agent_type, state) -> (Rc<RefCell<AgentRegistry>>, AgentId)`：创建注册表，注册一个 Agent，force 设置状态，返回共享注册表和 ID
    - `test_transition_legal_created_to_ready`：Created→Ready 返回 Ok(Ready)
    - `test_transition_legal_ready_to_running`：Ready→Running 返回 Ok(Running)
    - `test_transition_legal_running_to_suspended`：Running→Suspended
    - `test_transition_legal_running_to_error`：Running→Error
    - `test_transition_legal_suspended_to_running`：Suspended→Running
    - `test_transition_legal_suspended_to_error`：Suspended→Error
    - `test_transition_legal_error_to_recovering`：Error→Recovering
    - `test_transition_legal_recovering_to_ready`：Recovering→Ready
    - `test_transition_legal_recovering_to_dead`：Recovering→Dead
    - `test_transition_legal_error_to_dead`：Error→Dead
    - `test_transition_legal_running_to_dead`：Running→Dead
    - `test_transition_legal_ready_to_dead`：Ready→Dead
    - `test_transition_illegal_created_to_running`：Created→Running 返回 `Err(InvalidStateTransition { from: Created, to: Running })`
    - `test_transition_illegal_error_to_running`：Error→Running 返回 Err（§8.5）
    - `test_transition_illegal_dead_to_ready`：Dead→Ready 返回 Err（§8.1 不可逆）
    - `test_transition_illegal_self_transition`：Running→Running 返回 Err
    - `test_transition_nonexistent_agent`：对不存在的 ID 调用 transition 返回 `Err(AgentNotFound)`
    - `test_current_state_existing`：注册 Agent 后 current_state 返回正确状态
    - `test_current_state_nonexistent`：不存在的 ID 返回 `Err(AgentNotFound)`
    - `test_force_state_bypasses_table`：force_state 从 Created 直接设置 Running（绕过转换表），成功
    - `test_force_state_nonexistent`：不存在的 ID 返回 `Err(AgentNotFound)`
    - `test_force_state_no_hooks`：添加 RecordingHook 后 force_state 不触发 hook（D2 偏差）
    - `test_hook_on_exit_before_on_enter`：注册 RecordingHook，执行 transition，验证 on_exit 在 on_enter 之前调用
    - `test_hook_receives_correct_states`：transition 从 Created→Ready，验证 on_exit 收到 Created、on_enter 收到 Ready
    - `test_add_hook`：add_hook 后 hooks 列表长度增加，transition 触发新 hook
    - `test_dead_irreversible_all_states`：Agent 进入 Dead 后，尝试转换到所有 6 个非 Dead 状态均返回 Err
    - `test_full_lifecycle_path`：Created→Ready→Running→Suspended→Running→Error→Recovering→Ready→Running→Dead，每步 Ok
    - `test_lifecycle_event_eq`：验证 LifecycleEvent::StateChanged 的 PartialEq
  - RecordingHook 实现（测试辅助）：
    ```rust
    use core::cell::RefCell;
    struct RecordingHook {
        events: RefCell<Vec<(AgentState, AgentId, bool)>>, // (state, id, is_enter)
    }
    impl LifecycleHook for RecordingHook {
        fn on_enter(&self, state: AgentState, id: AgentId) {
            self.events.borrow_mut().push((state, id, true));
        }
        fn on_exit(&self, state: AgentState, id: AgentId) {
            self.events.borrow_mut().push((state, id, false));
        }
    }
    ```
  - **验证**：`cargo test -p eneros-agent` 全部通过

- [x] **Task 7: 编写集成测试 tests/lifecycle_test.rs**
  - 创建 `crates/agents/agent/tests/lifecycle_test.rs`：
    - `integration_full_lifecycle`：创建 Agent → 注册 → LifecycleManager → 完整路径 Created→Ready→Running→Suspended→Running→Error→Recovering→Ready→Running→Dead
    - `integration_multiple_agents_independent_lifecycles`：3 个 Agent 各自独立走不同生命周期路径
    - `integration_hook_recording`：注册 RecordingHook，执行多步转换，验证 hook 调用序列
    - `integration_force_state_for_recovery`：Agent 进入 Error 后 force_state 直接设为 Ready（模拟崩溃恢复场景，D2 偏差）
    - `integration_dead_agent_rejected`：Agent 进入 Dead 后所有后续转换均失败
    - `integration_shared_registry_multiple_managers`：两个 LifecycleManager 共享同一 Rc<RefCell<AgentRegistry>>，验证一个 manager 的状态变更对另一个可见
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 6: 文档与版本标识

- [x] **Task 8: 编写设计文档**
  - 创建 `docs/agents/agent-lifecycle-design.md`：
    - 版本目标 / 架构定位 / 前置依赖
    - 7 状态 + 12 合法转换表
    - 状态转换图（mermaid stateDiagram-v2，复制蓝图 §4.3）
    - 数据结构设计（LifecycleManager / LifecycleHook / LifecycleEvent）
    - 模块结构（lifecycle.rs + lifecycle/transitions.rs）
    - 偏差声明 D1~D5
    - 性能分析（转换 <1μs，TRANSITIONS.contains 线性扫描 12 条，可接受）
    - 并发设计（Rc<RefCell> 单线程 / Phase 3 替换策略）
    - Hook 设计（on_exit before / on_enter after / 不可访问 registry）
    - Dead 不可逆保证
    - 后续解锁版本（v0.36.0 / v0.37.0 / v0.38.0）
  - **验证**：文档存在且内容完整

- [x] **Task 9: 同步版本标识**
  - 根 `Cargo.toml`：`version = "0.35.0"`
  - `Makefile`：`VERSION := 0.35.0` + header 注释 + agent-build 描述更新为 "v0.35.0 lifecycle"
  - `.github/workflows/ci.yml`：`Version: v0.35.0`
  - `ci/src/gate.rs`：注释更新为 v0.35.0
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.35.0"`（Task 4 已完成）
  - **验证**：`grep -r "0.34.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（历史注释除外）

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
- Task 2: 无依赖（转换表独立于 error.rs，但 lib.rs 声明在 Task 4）
- Task 3: 依赖 Task 1（需要 InvalidStateTransition）+ Task 2（需要 transitions 模块）
- Task 4: 依赖 Task 3（lib.rs 声明 lifecycle 模块前，lifecycle.rs 应完整）
- Task 5: 依赖 Task 4（单元测试需要模块可被引用）
- Task 6: 依赖 Task 5（lifecycle 测试在 transitions 测试后）
- Task 7: 依赖 Task 6（集成测试在单元测试后）
- Task 8-9: 依赖 Task 4（可并行，文档与版本标识独立）
- Task 10: 依赖 Task 1-9 全部完成
