# v0.35.0 — Agent 生命周期状态机 Spec

> **蓝图依据**：`蓝图/phase1.md` §v0.35.0（行 5538~5744）
> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **子版本检查**：蓝图 grep `v0.35.[1-9]` 返回 0 匹配，本任务为单版本开发（无增强子版本）。

## Why

v0.34.0 实现了 Agent 注册表，但缺少生命周期管理。Agent 需要在正确的状态下执行（Created→Ready→Running→Suspended→Error→Recovering→Dead），防止僵尸 Agent 或非法状态转换。v0.35.0 实现状态表驱动的生命周期状态机，确保所有状态转换合法、非法转换被拒绝、Dead 状态不可逆。解锁 v0.36.0（启动初始化）/ v0.37.0（心跳检测）/ v0.38.0（崩溃恢复）。

## What Changes

- **新增** `crates/agents/agent/src/lifecycle.rs` — `LifecycleManager` 状态机 + `LifecycleHook` trait + `LifecycleEvent` 枚举
- **新增** `crates/agents/agent/src/lifecycle/transitions.rs` — `TRANSITIONS` 合法转换表 + `can_transition` 函数
- **修改** `crates/agents/agent/src/error.rs` — 追加 `InvalidStateTransition { from, to }` (结构变体) + `AgentNotAlive` (单元变体)
- **修改** `crates/agents/agent/src/lib.rs` — 声明 `lifecycle` 模块 + re-export + VERSION → "0.35.0"
- **新增** `crates/agents/agent/tests/lifecycle_test.rs` — 集成测试
- **新增** `docs/agents/agent-lifecycle-design.md` — 设计文档
- **版本标识同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`
- **BREAKING**：`AgentError` 新增结构变体 `InvalidStateTransition { from: AgentState, to: AgentState }`（首个带数据的变体；既有 6 个单元变体不变，但任何 `match` AgentError 的代码需考虑 exhaustiveness）

## Impact

- **Affected specs**：v0.33.0（AgentState，被引用不修改）/ v0.34.0（AgentRegistry，被引用不修改）/ v0.36.0（启动初始化，将使用 LifecycleManager）/ v0.37.0（心跳检测）/ v0.38.0（崩溃恢复）
- **Affected code**：
  - `crates/agents/agent/src/lifecycle.rs`（新增）
  - `crates/agents/agent/src/lifecycle/transitions.rs`（新增）
  - `crates/agents/agent/src/error.rs`（追加 2 变体）
  - `crates/agents/agent/src/lib.rs`（追加模块声明与 re-export）
  - `crates/agents/agent/tests/lifecycle_test.rs`（新增）
  - `docs/agents/agent-lifecycle-design.md`（新增）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本号）
- **回归保护**：v0.31.0（crypto）+ v0.32.0（PKI）+ v0.33.0（descriptor）+ v0.34.0（registry）所有测试必须继续通过

## 设计决策与偏差声明（Think Before Coding）

### 偏差 D1：`Rc<RefCell<AgentRegistry>>` 单线程内部可变性

**蓝图设计**：`LifecycleManager` 持有 `registry: Rc<RefCell<AgentRegistry>>`，通过 `RefCell` 实现内部可变性。

**决策**：遵循蓝图设计。

**理由**：
1. `alloc::rc::Rc` + `core::cell::RefCell` 均在 no_std + alloc 可用，零外部依赖
2. Phase 1 为单线程模型（pre-seL4），`Rc<RefCell<...>>` 足够
3. `Rc` 非 `Send`/`Sync` — 单线程约束是**有意为之**，防止误用于多核场景
4. Phase 3（seL4）或 v0.36.0+ 如需多核安全，将替换为 `Arc<Mutex<...>>` 或 seL4 能力机制

**代价**：单线程 only。`RefCell` 运行时借用检查（double-borrow 会 panic）。

### 偏差 D2：`force_state` 不触发 hooks

**蓝图 §3** 列出 `force_state(&mut self, id, state)` 但 §4.5 未展示实现。

**决策**：`force_state` 直接设置状态，**不触发** `on_exit`/`on_enter` hooks，不验证转换合法性。

**理由**（Karpathy "Simplicity First"）：
1. `force_state` 是特权操作（崩溃恢复 / 测试用），语义为"强制设置"
2. 不触发 hooks 符合"强制"语义 — 绕过所有常规流程
3. v0.38.0 崩溃恢复如需 hook 通知，可在那时增加 `force_state_with_hooks` 方法
4. 避免在 v0.35.0 引入当前无消费者的复杂度

### 偏差 D3：`add_hook` 方法（蓝图未显式声明）

**蓝图 §4.5** 的 `LifecycleManager` 有 `hooks: Vec<Box<dyn LifecycleHook>>` 字段，但未提供添加 hook 的方法。

**决策**：追加 `pub fn add_hook(&mut self, hook: Box<dyn LifecycleHook>)` 方法。

**理由**：
1. 蓝图定义了字段但无法填充 — 缺少方法会导致字段永远为空，Hook 机制形同虚设
2. `&mut self` 签名确保添加 hook 需要独占访问（配置阶段操作）
3. 最小实现：仅添加方法，不添加移除/清空 hook 的方法（YAGNI）

### 偏差 D4：`LifecycleEvent` 仅定义数据结构

**蓝图 §3** 定义 `LifecycleEvent` 枚举但 §4.5 关键代码无任何消费方。

**决策**：实现 `LifecycleEvent` 枚举（含 `StateChanged` / `TransitionRejected` 两个变体），但**不**实现事件分发基础设施（无事件队列、无事件发射器）。

**理由**（Karpathy "Simplicity First"）：
1. 蓝图 §3 将 `LifecycleEvent` 列为交付物 — 必须实现
2. 但当前无消费者 — 添加事件分发是投机性复杂度
3. 未来版本（如 v0.37.0 心跳检测或 v0.38.0 崩溃恢复）需要时可扩展

### 偏差 D5：Hook 回调在 RefCell 借用期间调用

**蓝图 §4.5** 的 `transition` 方法在 `self.registry.borrow_mut()` 期间调用 hooks。

**决策**：遵循蓝图设计 — hooks 在 RefCell 借用期间调用。

**影响**：Hook 实现不得访问 registry（会导致 `RefCell` double-borrow panic）。Hook 仅接收 `AgentState`（Copy）和 `AgentId`（Copy），不访问 registry 引用。

**理由**：在 hooks 调用前释放借用并重新借用会引入 TOCTOU 窗口（单线程 reentrancy 下状态可能变化）。保持借用期间的原子性更安全。

## ADDED Requirements

### Requirement: LifecycleManager 生命周期状态机

系统 SHALL 提供 `LifecycleManager` 结构体，持有 `Rc<RefCell<AgentRegistry>>` 共享注册表引用与 `Vec<Box<dyn LifecycleHook>>` hook 列表。

#### Scenario: 创建 LifecycleManager
- **WHEN** 调用 `LifecycleManager::new(registry: Rc<RefCell<AgentRegistry>>)` 
- **THEN** 返回 `LifecycleManager`，hooks 为空 Vec

#### Scenario: 合法状态转换
- **WHEN** 调用 `transition(id, target)` 且 `(current_state, target)` 在 TRANSITIONS 表中
- **THEN** Agent 的 state 被更新为 target，触发 on_exit/on_enter hooks，返回 `Ok(target)`

#### Scenario: 非法状态转换被拒绝
- **WHEN** 调用 `transition(id, target)` 且 `(current_state, target)` 不在 TRANSITIONS 表中
- **THEN** 返回 `Err(AgentError::InvalidStateTransition { from: current_state, to: target })`，状态不变，不触发 hooks

#### Scenario: 转换不存在的 Agent
- **WHEN** 调用 `transition(id, target)` 且 `id` 不在注册表中
- **THEN** 返回 `Err(AgentError::AgentNotFound)`

#### Scenario: 查询当前状态
- **WHEN** 调用 `current_state(id)` 且 `id` 存在
- **THEN** 返回 `Ok(AgentState)`

#### Scenario: 强制设置状态
- **WHEN** 调用 `force_state(id, state)` 且 `id` 存在
- **THEN** Agent 的 state 被直接设置为指定值（绕过转换表），不触发 hooks，返回 `Ok(())`

#### Scenario: 添加 Hook
- **WHEN** 调用 `add_hook(box dyn LifecycleHook)`
- **THEN** hook 被添加到 hooks 列表，后续 transition 将触发该 hook 的 on_exit/on_enter

### Requirement: TRANSITIONS 合法转换表

系统 SHALL 定义 12 条合法状态转换：
1. Created → Ready
2. Ready → Running
3. Running → Suspended
4. Running → Error
5. Suspended → Running
6. Suspended → Error
7. Error → Recovering
8. Recovering → Ready
9. Recovering → Dead
10. Error → Dead
11. Running → Dead
12. Ready → Dead

所有不在表中的转换（含自转换如 Created→Created）SHALL 被拒绝。

### Requirement: can_transition 查询函数

系统 SHALL 提供 `can_transition(from: AgentState, to: AgentState) -> bool` 函数，查询某转换是否合法。

### Requirement: LifecycleHook trait

系统 SHALL 提供 `LifecycleHook` trait，包含 `on_enter(&self, state: AgentState, id: AgentId)` 和 `on_exit(&self, state: AgentState, id: AgentId)` 两个方法。该 trait 必须是 object-safe（支持 `dyn LifecycleHook`）。

### Requirement: LifecycleEvent 枚举

系统 SHALL 提供 `LifecycleEvent` 枚举，包含 `StateChanged { from, to, agent_id }` 和 `TransitionRejected { from, to, reason: String }` 两个变体。

### Requirement: AgentError 扩展

系统 SHALL 在 `AgentError` 中追加：
- `InvalidStateTransition { from: AgentState, to: AgentState }` — 结构变体，携带非法转换的源/目标状态
- `AgentNotAlive` — 单元变体，Agent 不在存活状态

### Requirement: Dead 状态不可逆

系统 MUST 确保 `Dead` 状态没有任何合法的传出转换。调用 `transition(dead_agent_id, any_state)` MUST 返回 `Err(InvalidStateTransition)`。

### Requirement: Error→Running 非法

系统 MUST 确保 `Error → Running` 不在 TRANSITIONS 表中（蓝图 §8.5）。从 Error 恢复必须经过 `Error → Recovering → Ready → Running` 路径。

### Requirement: no_std 合规

`lifecycle.rs` 和 `transitions.rs` 必须：
- 不使用 `std::*`（仅 `alloc::*` / `core::*`）
- 不在子模块重复 `#![cfg_attr(not(test), no_std)]`
- 不使用 `panic!` / `todo!` / `unimplemented!`（非测试代码）
- 通过 `aarch64-unknown-none` 交叉编译

### Requirement: 零外部依赖

`crates/agents/agent/Cargo.toml` 的 `[dependencies]` 必须保持为空。`Rc` / `RefCell` / `Box` / `Vec` / `String` 均来自 `alloc` / `core`。

### Requirement: 测试覆盖

- 所有 12 条合法转换测试（蓝图 §6.1，≥80%）
- 非法转换拒绝测试（蓝图 §6.5），包括：Created→Running / Error→Running / Dead→Ready / Dead→any / 自转换
- 状态机端到端测试（蓝图 §6.2）：完整路径 Created→Ready→Running→Suspended→Running→Error→Recovering→Ready→Running→Dead
- Hook 触发顺序测试（on_exit before state change, on_enter after）
- force_state 绕过转换表测试
- Dead 不可逆测试
