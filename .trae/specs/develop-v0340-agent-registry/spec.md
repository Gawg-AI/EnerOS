# v0.34.0 — Agent 注册表与发现 Spec

> **蓝图依据**：`蓝图/phase1.md` §v0.34.0（行 5342~5535）
> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **子版本检查**：蓝图 grep `v0.34.[1-9]` 返回 0 匹配，本任务为单版本开发（无增强子版本）。

## Why

Agent 需要知道其他 Agent 的存在才能通信。v0.33.0 已定义 `AgentDescriptor` 数据结构，但缺少全局注册表来管理"当前系统中有哪些 Agent、如何按 ID/类型/名称查找"。v0.34.0 实现 `AgentRegistry` 作为 Agent 发现的基础，解锁 v0.35.0（生命周期）/ v0.36.0（启动初始化）/ Agent 间通信。

## What Changes

- **新增** `crates/agents/agent/src/registry.rs`：`AgentRegistry` 结构体（双索引：主表 + 类型索引）+ `RegistryStats` 统计结构
- **新增** `AgentError` 两个变体：`AgentNotFound` / `AlreadyRegistered`（外科手术式扩展，不改动既有 4 个变体）
- **修改** `crates/agents/agent/src/lib.rs`：声明 `registry` 模块 + re-export `AgentRegistry` / `RegistryStats` + VERSION → "0.34.0"
- **修改** `crates/agents/agent/src/error.rs`：追加 2 个变体 + Display 实现
- **新增** `tests/registry_test.rs`：集成测试
- **新增** `docs/agents/agent-registry-design.md`：设计文档
- **版本标识同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` / `lib.rs` VERSION
- **BREAKING**：无（仅追加，不修改既有 API）

## Impact

- **Affected specs**：v0.33.0（AgentDescriptor，被引用不修改）/ v0.35.0（生命周期，将引用 AgentRegistry）
- **Affected code**：
  - `crates/agents/agent/src/registry.rs`（新增）
  - `crates/agents/agent/src/lib.rs`（追加模块声明与 re-export）
  - `crates/agents/agent/src/error.rs`（追加 2 变体）
  - `crates/agents/agent/tests/registry_test.rs`（新增）
  - `docs/agents/agent-registry-design.md`（新增）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本号）
- **回归保护**：v0.31.0（249 tests）+ v0.32.0（402 tests）+ v0.33.0（agent crate tests）必须全部继续通过

## 设计决策与偏差声明（Think Before Coding）

### 偏差 D1：使用 `BTreeMap` 而非 `HashMap`

**蓝图矛盾**：§4.5 代码示例中结构体声明为 `BTreeMap<AgentId, AgentDescriptor>`，但 `new()` 实现却调用 `HashMap::new()`；§5 技术交底选定"HashMap 双索引 O(1)"；§2 前置依赖写"v0.11.0 用户堆 → HashMap"。

**决策**：采用 `alloc::collections::BTreeMap`。

**理由**（Karpathy "Simplicity First" + "Think Before Coding"）：
1. v0.33.0 agent crate 明确设计为**零外部依赖**（`Cargo.toml` 已验证），`HashMap` 需要 hasher crate（`ahash`/`fnv`/`hashbrown`），会破坏此不变量
2. `BTreeMap` 在 `alloc::collections` 中，零外部依赖
3. Agent 注册表典型规模 < 100 个 Agent，`BTreeMap` 的 O(log n) 查找（< 7 次比较）在 n=100 时远低于 1μs，蓝图 §6.3 "查找延迟 <1μs" 要求轻松满足
4. `BTreeMap` 自带按 key 排序，`list_all()` 返回结果有序（确定性迭代顺序，利于测试与调试）
5. project_memory.md 明确记录"no_std 合规：使用 `alloc::collections::BTreeMap`"

**代价**：O(log n) 而非 O(1)，但 n 极小，可忽略。

### 偏差 D2：注册表无内部锁（plain `&mut self`）

**蓝图 §8.1** 提及"并发访问需加锁或用 RwLock"；§6.2 提及"并发注册测试"。

**决策**：`AgentRegistry` 所有方法使用 `&self` / `&mut self`，**不引入内部锁**。

**理由**（Karpathy "Simplicity First"）：
1. 蓝图 §4.5 关键代码本身就是 `&mut self` 签名，未使用内部可变性
2. `spin::RwLock` / `spin::Mutex` 是外部依赖，会破坏零依赖不变量
3. 同步原语属于**更高层抽象**（v0.36.0 启动初始化、v0.19.0 分区调度）的职责，注册表本身是纯数据结构
4. §6.2 "并发注册测试" 重新解读为"顺序压力测试（大量注册/注销）验证数据结构正确性"，真正的多线程并发测试后置到 v0.36.0

### 偏差 D3：新增 `AlreadyRegistered` 而非复用 `DuplicateId`

v0.33.0 已有 `DuplicateId`（描述符级 ID 冲突）。蓝图 §4.4 明确要求新增 `AlreadyRegistered`（注册表级重复注册）。

**决策**：按蓝图新增 `AgentNotFound` + `AlreadyRegistered` 两个变体，保留既有 `DuplicateId` 不动。

**理由**（Karpathy "Surgical Changes"）：
1. 语义不同：`DuplicateId` = 构造描述符时 ID 冲突；`AlreadyRegistered` = 注册表已存在该 ID
2. 蓝图明确要求新增，遵循蓝图是首要原则
3. 既有调用方不受影响（仅追加变体，不修改既有变体）

## ADDED Requirements

### Requirement: AgentRegistry 全局注册表

系统 SHALL 提供全局 Agent 注册表 `AgentRegistry`，支持 Agent 的注册、注销、查找与枚举。

#### Scenario: 注册新 Agent
- **WHEN** 调用 `register(desc)` 且 `desc.agent_id` 不在注册表中
- **THEN** 描述符被插入主表与类型索引，返回 `Ok(AgentId)`

#### Scenario: 重复注册被拒绝
- **WHEN** 调用 `register(desc)` 且 `desc.agent_id` 已存在
- **THEN** 返回 `Err(AgentError::AlreadyRegistered)`，注册表状态不变

#### Scenario: 注销已注册 Agent
- **WHEN** 调用 `unregister(id)` 且 `id` 存在
- **THEN** 从主表与类型索引中移除，返回 `Ok(())`

#### Scenario: 注销不存在的 Agent
- **WHEN** 调用 `unregister(id)` 且 `id` 不存在
- **THEN** 返回 `Err(AgentError::AgentNotFound)`

#### Scenario: 按 ID 查找
- **WHEN** 调用 `get(id)` / `get_mut(id)`
- **THEN** 返回 `Option<&AgentDescriptor>` / `Option<&mut AgentDescriptor>`

#### Scenario: 按类型查找
- **WHEN** 调用 `find_by_type(agent_type)`
- **THEN** 返回 `Vec<&AgentDescriptor>`，包含所有该类型的 Agent（顺序按 AgentId 升序）

#### Scenario: 按名称查找
- **WHEN** 调用 `find_by_name(name)`
- **THEN** 返回 `Option<&AgentDescriptor>`（首个匹配，名称不保证唯一）

#### Scenario: 枚举所有/存活 Agent
- **WHEN** 调用 `list_all()` / `list_alive()`
- **THEN** 返回 `Vec<&AgentDescriptor>`，`list_alive` 仅含 `is_alive() == true` 的 Agent

#### Scenario: 统计信息
- **WHEN** 调用 `stats()` / `count()` / `count_by_type(t)` / `exists(id)`
- **THEN** 返回 `RegistryStats` / `usize` / `usize` / `bool`

### Requirement: RegistryStats 统计结构

系统 SHALL 提供 `RegistryStats` 结构，包含 `total` / `alive` / `by_type` 三个字段，反映注册表当前状态。

### Requirement: AgentError 扩展

系统 SHALL 在 `AgentError` 中追加 `AgentNotFound` 与 `AlreadyRegistered` 两个变体，并实现 `Display` trait。

### Requirement: no_std 合规

`registry.rs` 必须：
- 不使用 `std::*`（仅 `alloc::*` / `core::*`）
- 不在子模块重复 `#![cfg_attr(not(test), no_std)]`（由 lib.rs 统一声明）
- 不使用 `panic!` / `todo!` / `unimplemented!`（非测试代码）
- 通过 `aarch64-unknown-none` 交叉编译

### Requirement: 零外部依赖

`crates/agents/agent/Cargo.toml` 的 `[dependencies]` 必须保持为空（继承 v0.33.0 不变量）。

### Requirement: 测试覆盖

- 单元测试覆盖率 ≥ 80%（蓝图 §6.1）
- 重复注册拒绝测试（蓝图 §6.5）
- 注销后索引一致性测试（蓝图 §8.2 / §8.4）
- 集成测试：多 Agent 注册/查找/枚举场景
