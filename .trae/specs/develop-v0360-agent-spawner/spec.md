# v0.36.0 — Agent 启动与初始化 Spec

> **蓝图依据**：`蓝图/phase1.md` §v0.36.0（行 5747~5951）
> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **子版本检查**：蓝图 grep `v0.36.[1-9]` 返回 0 匹配，本任务为单版本开发（无增强子版本）。

## Why

v0.35.0 实现了生命周期状态机，但 Agent 无法实际启动。v0.36.0 实现 `AgentSpawner` 启动器与 `AgentEntry` 入口 trait，使 Agent 从 `Created` 状态经过完整的初始化流程（注册 → Created→Ready → 加载代码 → on_init → Ready→Running → on_start）进入 `Running`。解锁 v0.37.0（心跳检测）/ v0.38.0（崩溃恢复）。

## What Changes

- **新增** `crates/agents/agent/src/init.rs` — `AgentConfig` / `AgentContext` / `AgentEntry` trait
- **新增** `crates/agents/agent/src/spawner.rs` — `AgentSpawner` / `AgentFactory` trait
- **修改** `crates/agents/agent/src/error.rs` — 追加 3 个 String-carrying 错误变体（`CodeLoadFailed` / `InitFailed` / `StartFailed`）
- **修改** `crates/agents/agent/src/lib.rs` — 声明 `init` + `spawner` 模块 + re-export + VERSION → "0.36.0"
- **新增** `crates/agents/agent/tests/spawner_test.rs` — 集成测试（含 TestAgent / TestAgentFactory / FailingAgent 等）
- **新增** `docs/agents/agent-spawner-design.md` — 设计文档
- **版本标识同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`
- **BREAKING**：`AgentError` 新增 3 个 `String`-carrying 变体（首个携带 String 的变体；既有 8 个变体不变，但任何 `match` AgentError 的代码需考虑 exhaustiveness）

## Impact

- **Affected specs**：v0.33.0（AgentDescriptor，被引用不修改）/ v0.34.0（AgentRegistry，被引用不修改）/ v0.35.0（LifecycleManager，被引用不修改）/ v0.37.0（心跳检测，将使用 spawn 启动 Agent）/ v0.38.0（崩溃恢复，将使用 force_state + on_stop）
- **Affected code**：
  - `crates/agents/agent/src/init.rs`（新增）
  - `crates/agents/agent/src/spawner.rs`（新增）
  - `crates/agents/agent/src/error.rs`（追加 3 变体）
  - `crates/agents/agent/src/lib.rs`（追加模块声明与 re-export）
  - `crates/agents/agent/tests/spawner_test.rs`（新增）
  - `docs/agents/agent-spawner-design.md`（新增）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本号）
- **回归保护**：v0.31.0（crypto）+ v0.32.0（PKI）+ v0.33.0（descriptor）+ v0.34.0（registry）+ v0.35.0（lifecycle）所有测试必须继续通过

## 设计决策与偏差声明（Think Before Coding）

### 偏差 D1：`Rc<RefCell<LifecycleManager>>` 代替蓝图的 `Rc<LifecycleManager>`

**蓝图设计**：`AgentSpawner { registry: Rc<RefCell<AgentRegistry>>, lifecycle: Rc<LifecycleManager> }`

**问题**：v0.35.0 的 `LifecycleManager::force_state` 签名为 `&mut self`，`Rc<LifecycleManager>` 只能调用 `&self` 方法（`transition` / `can_transition` / `current_state`）。但 spawn 的错误清理路径需要 `force_state`（见 D5），`Rc<LifecycleManager>` 不可用。

**决策**：改为 `lifecycle: Rc<RefCell<LifecycleManager>>`，通过 `borrow_mut().force_state(...)` 调用。

**理由**：
1. 错误清理必须将 Agent 置于 `Error` 状态（蓝图 §4.3 mermaid 图：init 失败 → Error 状态）
2. `Ready→Error` 不在 TRANSITIONS 表中（v0.35.0 合法转换表只有 12 条），`transition` 会拒绝
3. `force_state` 是 v0.35.0 D2 偏差明确为"崩溃恢复/测试"设计的特权操作，正是此处所需
4. 不修改 v0.35.0 的 `force_state` 签名（Surgical Changes 原则）
5. `RefCell` 运行时借用检查在单线程下安全（Phase 1 单线程模型）

**代价**：多一层 `RefCell` 借用。spawn 流程中 `borrow_mut()` 调用 `force_state` 时，不能同时持有其他 `borrow()`（会 panic）。经审查 spawn 实现，错误清理路径不持有其他借用，安全。

### 偏差 D2：新增 `AgentFactory` trait（蓝图 `load_code` 引用不存在的 `crate::agents::create_agent`）

**蓝图设计**：`load_code` 调用 `crate::agents::create_agent(&config.agent_type, &config.name)`。

**问题**：v0.36.0 不存在 `crate::agents` 模块，也无 `create_agent` 函数。蓝图前向引用了未实现的代码。

**决策**：新增 `AgentFactory` trait 作为依赖注入点：
```rust
pub trait AgentFactory {
    fn create(&self, agent_type: AgentType, name: &str) -> Result<Box<dyn AgentEntry>, AgentError>;
}
```
`AgentSpawner` 持有 `factory: Rc<dyn AgentFactory>`，`load_code` 委托给 `factory.create(...)`。

**理由**：
1. 静态注册 + trait 对象是蓝图 §5 选定方案（"静态注册 + trait 对象 | 类型安全 | 需编译时已知 | ✅ Phase 1"）
2. 依赖注入让 AgentSpawner 可测试（测试提供 `TestAgentFactory`）
3. 生产环境在启动时注册具体 factory（如 EnergyAgentFactory）
4. Phase 3 可替换为动态加载 factory（蓝图 §5 "动态加载 .so | 热插拔 | Phase 3"）
5. trait object-safe（`&self` 接收者，无泛型，支持 `dyn AgentFactory`）

**代价**：AgentSpawner 构造时需传入 factory。测试需提供 factory 实现。

### 偏差 D3：`spawn_blocking` 委托给 `spawn`（Phase 1 单线程无异步运行时）

**蓝图设计**：`AgentSpawner` 同时提供 `spawn(&self, config) -> Result<AgentId, AgentError>` 和 `spawn_blocking(&self, config) -> Result<AgentId, AgentError>`。

**问题**：Phase 1 为单线程 no_std 环境，无异步运行时（no `async`/`await`），`spawn` 与 `spawn_blocking` 语义完全相同。

**决策**：`spawn_blocking` 直接委托 `spawn`（调用 `self.spawn(config, now)`）。

**理由**：
1. 保留蓝图接口签名，便于未来 Phase 3 引入异步运行时后拆分
2. 当前不引入投机性异步复杂度（Karpathy "Simplicity First"）
3. 文档明确标注"Phase 1 单线程下两者等价"

### 偏差 D4：`spawn` 签名追加 `now: u64` 参数

**蓝图设计**：`spawn(&self, config: AgentConfig) -> Result<AgentId, AgentError>`

**问题**：`AgentDescriptor::new(agent_type, name, now: u64)` 需要 3 个参数（v0.33.0 实际 API），`now` 为创建时间戳。蓝图 §4.5 的 `spawn` 示例用 2 参数调用 `AgentDescriptor::new(config.agent_type, &config.name)`，与实际 API 不符。

**决策**：`spawn` 签名改为 `spawn(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>`。

**理由**：
1. no_std 无系统时钟，时间戳必须由外部提供（v0.33.0 既定约定）
2. `now` 是运行时数据，不属于 `AgentConfig` 配置（配置是静态属性，时间是动态值）
3. 不修改 v0.33.0 的 `AgentDescriptor::new` 签名（Surgical Changes）
4. `spawn_blocking` 同样追加 `now: u64`

### 偏差 D5：错误清理使用 `force_state` 而非 `transition`

**蓝图设计**：`on_init` 失败时 `self.lifecycle.transition(id, AgentState::Error)`。

**问题**：`on_init` 失败时 Agent 处于 `Ready` 状态（步骤 4 已 `Created→Ready`）。但 `Ready→Error` 不在 v0.35.0 的 TRANSITIONS 表中（12 条合法转换不含 `Ready→Error`），`transition` 会返回 `Err(InvalidStateTransition)`，Agent 留在 `Ready` 而非 `Error`，违反蓝图 §4.3 mermaid 图意图。

**决策**：错误清理统一使用 `force_state(id, AgentState::Error)`（绕过转换表）。`on_init` 失败与 `on_start` 失败均用 `force_state`。

**理由**：
1. v0.35.0 D2 偏差明确 `force_state` 为"崩溃恢复/测试"特权操作设计，正是此处场景
2. `force_state` 不触发 hooks、不验证转换表，语义为"强制设置"
3. `on_start` 失败时 Agent 在 `Running` 状态，`Running→Error` 虽合法，但为一致性也用 `force_state`
4. 错误清理的 `force_state` 返回值用 `let _ =` 忽略（不掩盖原始错误）

**代价**：错误路径不触发 hooks。若需 hook 通知错误，可在 v0.38.0 增加 `force_state_with_hooks`。

## ADDED Requirements

### Requirement: AgentConfig 配置结构

系统 SHALL 提供 `AgentConfig` 结构体，包含：
- `agent_type: AgentType` — Agent 类型
- `name: String` — Agent 名称
- `binary_path: Option<String>` — 二进制路径（Phase 3 动态加载用，Phase 1 可 None）
- `config_path: Option<String>` — 配置文件路径
- `priority_override: Option<u8>` — 优先级覆盖
- `mem_override: Option<usize>` — 内存配额覆盖

derive `Clone, Debug, PartialEq, Eq`。

#### Scenario: 创建 AgentConfig
- **WHEN** 构造 `AgentConfig { agent_type: AgentType::Energy, name: String::from("e1"), binary_path: None, config_path: None, priority_override: None, mem_override: None }`
- **THEN** 返回完整的 AgentConfig 实例

### Requirement: AgentContext 上下文结构

系统 SHALL 提供 `AgentContext` 结构体，包含：
- `agent_id: AgentId` — Agent 唯一标识
- `config: AgentConfig` — Agent 配置（克隆）
- `registry: Rc<RefCell<AgentRegistry>>` — 共享注册表引用

derive `Debug`（不 derive Clone，context 按 `&mut` 传递）。

### Requirement: AgentEntry 入口 trait

系统 SHALL 提供 `AgentEntry` trait（object-safe），包含 3 个方法：
- `on_init(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>` — 初始化回调
- `on_start(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>` — 启动回调
- `on_stop(&mut self, ctx: &mut AgentContext)` — 停止回调（v0.36.0 不调用，预留给 v0.38.0）

#### Scenario: object-safe
- **WHEN** 将具体 Agent 实现装箱为 `Box<dyn AgentEntry>`
- **THEN** 编译通过，可动态派发 on_init/on_start/on_stop

### Requirement: AgentFactory 工厂 trait（D2 偏差）

系统 SHALL 提供 `AgentFactory` trait（object-safe）：
```rust
pub trait AgentFactory {
    fn create(&self, agent_type: AgentType, name: &str) -> Result<Box<dyn AgentEntry>, AgentError>;
}
```

#### Scenario: factory 创建成功
- **WHEN** 调用 `factory.create(AgentType::Energy, "e1")`
- **THEN** 返回 `Ok(Box<dyn AgentEntry>)`

#### Scenario: factory 创建失败
- **WHEN** 调用 `factory.create(unknown_type, "x")`
- **THEN** 返回 `Err(AgentError::CodeLoadFailed(...))`

### Requirement: AgentSpawner 启动器

系统 SHALL 提供 `AgentSpawner` 结构体：
```rust
pub struct AgentSpawner {
    registry: Rc<RefCell<AgentRegistry>>,
    lifecycle: Rc<RefCell<LifecycleManager>>,
    factory: Rc<dyn AgentFactory>,
}
```

#### Scenario: 构造 AgentSpawner
- **WHEN** 调用 `AgentSpawner::new(registry, lifecycle, factory)`
- **THEN** 返回 AgentSpawner 实例

#### Scenario: spawn 成功
- **WHEN** 调用 `spawn(config, now)` 且 factory/on_init/on_start 全部成功
- **THEN** Agent 进入 `Running` 状态，返回 `Ok(AgentId)`

#### Scenario: spawn_blocking 等价 spawn（D3 偏差）
- **WHEN** 调用 `spawn_blocking(config, now)`
- **THEN** 行为与 `spawn(config, now)` 完全相同

### Requirement: spawn 流程（8 步）

`spawn(config, now)` SHALL 按以下顺序执行：

1. 创建 `AgentDescriptor::new(config.agent_type, &config.name, now)`
2. 应用覆盖：`if let Some(p) = config.priority_override { desc.priority = p; }` + `if let Some(m) = config.mem_override { desc.mem_quota = m; }`
3. 注册到 registry：`self.registry.borrow_mut().register(desc)?`
4. `Created→Ready` 转换：`self.lifecycle.borrow().transition(id, AgentState::Ready)?`
5. 加载代码：`let mut agent = self.load_code(&config)?;`（委托 factory.create）
6. 初始化上下文：`let mut ctx = self.init_context(id, &config);`
7. 调用 `agent.on_init(&mut ctx)` — 失败时 `force_state(id, Error)` 并返回原始错误（D5 偏差）
8. `Ready→Running` 转换：`self.lifecycle.borrow().transition(id, AgentState::Running)?`
9. 调用 `agent.on_start(&mut ctx)` — 失败时 `force_state(id, Error)` 并返回原始错误（D5 偏差）
10. 返回 `Ok(id)`

#### Scenario: 完整成功路径
- **WHEN** factory 返回成功 Agent，on_init/on_start 均返回 Ok
- **THEN** Agent 状态序列：Created → Ready → Running，返回 Ok(id)

#### Scenario: on_init 失败 → Error 状态
- **WHEN** on_init 返回 Err(e)
- **THEN** Agent 被 force_state 到 Error 状态，spawn 返回 Err(e)

#### Scenario: on_start 失败 → Error 状态
- **WHEN** on_start 返回 Err(e)
- **THEN** Agent 被 force_state 到 Error 状态，spawn 返回 Err(e)

#### Scenario: load_code 失败 → Error 状态
- **WHEN** factory.create() 返回 Err(e)
- **THEN** Agent 被 force_state 到 Error 状态，spawn 返回 Err(e)

### Requirement: AgentError 扩展

系统 SHALL 在 `AgentError` 中追加 3 个 `String`-carrying 变体：
- `CodeLoadFailed(String)` — 代码加载失败
- `InitFailed(String)` — 初始化失败
- `StartFailed(String)` — 启动失败

### Requirement: no_std 合规

`init.rs` 和 `spawner.rs` 必须：
- 不使用 `std::*`（仅 `alloc::*` / `core::*`）
- 不在子模块重复 `#![cfg_attr(not(test), no_std)]`
- 不使用 `panic!` / `todo!` / `unimplemented!`（非测试代码）
- 通过 `aarch64-unknown-none` 交叉编译

### Requirement: 零外部依赖

`crates/agents/agent/Cargo.toml` 的 `[dependencies]` 必须保持为空。

### Requirement: 测试覆盖

- spawn 成功路径测试（Agent 进入 Running）
- on_init 失败 → Error 状态测试
- on_start 失败 → Error 状态测试
- load_code 失败 → Error 状态测试
- spawn_blocking 等价 spawn 测试
- priority_override / mem_override 应用测试
- 多 Agent 独立 spawn 测试
- AgentContext 正确性测试（agent_id / config / registry 字段）
- AgentEntry trait object-safe 测试（Box<dyn AgentEntry>）
