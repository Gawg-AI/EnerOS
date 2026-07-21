# Agent 崩溃自动重启 (v0.38.0) Spec

## Why

v0.37.0 实现了心跳监控，能检测 Agent 故障（Unhealthy 状态），但检测到故障后无法自动恢复。储能场景要求高可用，Agent 崩溃后需自动重启（最多 3 次、检查点恢复、3 次失败→Dead），减少人工干预。

本版本实现 `CrashRecovery` 崩溃恢复器与 `CheckpointStore` 检查点存储，在心跳检测到故障后自动执行恢复流程。

## What Changes

- **新增** `crates/agents/agent/src/recovery.rs` — `CrashRecovery` 崩溃恢复器（handle_crash / restart / restore_checkpoint / save_checkpoint）
- **新增** `crates/agents/agent/src/checkpoint.rs` — `CheckpointStore` trait + `InMemoryCheckpointStore` 默认实现 + `Checkpointable` trait
- **修改** `crates/agents/agent/src/error.rs` — 新增 3 个错误变体（MaxRestartsExceeded / CheckpointCorrupted / RestartFailed）
- **修改** `crates/agents/agent/src/lib.rs` — 声明新模块 + re-export + VERSION 0.38.0
- **新增** `crates/agents/agent/tests/recovery_test.rs` — 集成测试
- **新增** `docs/agents/agent-crash-recovery-design.md` — 设计文档
- **同步版本标识** Cargo.toml / Makefile / ci.yml / gate.rs → 0.38.0

## Impact

- **Affected specs**: v0.37.0 心跳监控（CrashRecovery 集成 HeartbeatMonitor）、v0.35.0 生命周期（使用 Recovering 状态）、v0.36.0 启动器（registry 共享引用）、v0.33.0 描述符（restart_count / last_heartbeat 字段）
- **Affected code**: `crates/agents/agent/src/error.rs`、`crates/agents/agent/src/lib.rs`、新增 `recovery.rs` / `checkpoint.rs`
- **后续解锁**: v0.41.0 System Agent、v0.42.0 故障恢复编排、v0.58.0 降级流程

## ADDED Requirements

### Requirement: CrashRecovery 崩溃恢复器

系统 SHALL 提供 `CrashRecovery` 结构体，在 Agent 崩溃（心跳超时 → Error 状态）后自动执行恢复流程：

1. `Error → Recovering` 状态转换
2. 检查 `restart_count` 是否超过 `max_restarts`（默认 3）
3. 若超过：`Recovering → Dead`，返回 `MaxRestartsExceeded` 错误
4. 若未超过：加载检查点（若有）→ `Recovering → Ready → Running` → 更新 restart_count + last_heartbeat → 重新注册心跳
5. 返回恢复结果

#### Scenario: 首次崩溃恢复成功
- **WHEN** Agent 在 Error 状态，restart_count = 0
- **AND** 调用 `handle_crash(id, now)`
- **THEN** Agent 状态变为 Running，restart_count = 1，心跳重新注册

#### Scenario: 超过最大重启次数
- **WHEN** Agent 在 Error 状态，restart_count = 3，max_restarts = 3
- **AND** 调用 `handle_crash(id, now)`
- **THEN** Agent 状态变为 Dead，返回 `MaxRestartsExceeded { agent_id, count: 3 }`

#### Scenario: 检查点恢复
- **WHEN** Agent 崩溃前保存了检查点
- **AND** 调用 `restore_checkpoint(id)`
- **THEN** 返回 `Ok(Some(Vec<u8>))` 检查点数据

#### Scenario: 无检查点时从初始状态重启
- **WHEN** Agent 崩溃前未保存检查点
- **AND** 调用 `restore_checkpoint(id)`
- **THEN** 返回 `Ok(None)`，Agent 从初始状态重启

### Requirement: CheckpointStore 检查点存储

系统 SHALL 提供 `CheckpointStore` trait 作为检查点存储抽象层，支持 save / load / delete 操作。默认提供 `InMemoryCheckpointStore`（基于 BTreeMap 的内存实现），生产环境可注入文件系统后端。

#### Scenario: 保存并加载检查点
- **WHEN** 调用 `save(id, &[0x01, 0x02])`
- **AND** 调用 `load(id)`
- **THEN** 返回 `Ok(Some([0x01, 0x02]))`

#### Scenario: 加载不存在的检查点
- **WHEN** 调用 `load(id)` 且未保存过
- **THEN** 返回 `Ok(None)`

#### Scenario: 删除检查点
- **WHEN** 调用 `delete(id)` 后再 `load(id)`
- **THEN** 返回 `Ok(None)`

### Requirement: Checkpointable trait

系统 SHALL 提供 `Checkpointable` trait，供 Agent 实现者提供自定义的状态保存/恢复逻辑：

```rust
pub trait Checkpointable {
    fn save_state(&self) -> Vec<u8>;
    fn restore_state(&mut self, data: &[u8]) -> Result<(), AgentError>;
}
```

CrashRecovery 不直接调用此 trait；调用方（如编排器）负责通过 `Checkpointable` 序列化 Agent 状态，再通过 `CrashRecovery::save_checkpoint` 持久化。

### Requirement: 3 个新错误变体

系统 SHALL 在 `AgentError` 中新增 3 个变体：

- `MaxRestartsExceeded { agent_id: AgentId, count: u32 }` — 超过最大重启次数
- `CheckpointCorrupted { agent_id: AgentId }` — 检查点数据损坏
- `RestartFailed { agent_id: AgentId, reason: String }` — 重启失败

## 偏差声明（D1~D9）

### D1: CheckpointStore 为 trait 而非 struct

**蓝图设计**：`CheckpointStore` 为 struct，持有 `fs: Box<dyn FileSystem>`。

**问题**：`eneros-agent` crate 保持零外部依赖不变量（v0.33~v0.37），无 `FileSystem` trait。引入 `eneros-fs` 依赖会破坏不变量。

**决策**：定义 `CheckpointStore` 为 trait（类似 `AgentFactory` 的 DI 模式），提供 `InMemoryCheckpointStore` 默认实现。生产环境由调用方注入文件系统后端。

### D2: handle_crash / restart 追加 now: u64 参数

**蓝图设计**：`handle_crash(&self, id: AgentId)` 无时间参数，内部使用 `crate::time::now_ms()`。

**问题**：no_std 无系统时钟，`crate::time::now_ms()` 不存在（v0.36.0 D4 / v0.37.0 D2 同类问题）。

**决策**：`handle_crash(&self, id: AgentId, now: u64)` 和 `restart(&self, id: AgentId, now: u64)` 追加 `now` 参数。

### D3: lifecycle 使用 Rc<RefCell<LifecycleManager>>

**蓝图设计**：`lifecycle: Rc<LifecycleManager>`。

**问题**：`LifecycleManager::force_state` 需要 `&mut self`（v0.36.0 D1 同类问题）。崩溃恢复可能需 force_state（如强制 Dead）。

**决策**：`lifecycle: Rc<RefCell<LifecycleManager>>`。

### D4: registry 直接传入（不通过 spawner）

**蓝图设计**：`spawner: Rc<AgentSpawner>`，通过 `self.spawner.registry.borrow()` 访问。

**问题**：`AgentSpawner.registry` 为私有字段（v0.36.0 设计），添加 public accessor 违反 Surgical Changes 原则。

**决策**：`registry: Rc<RefCell<AgentRegistry>>` 直接传入 CrashRecovery，不持有 spawner。

### D5: 不持有 spawner

**蓝图设计**：`CrashRecovery` 持有 `spawner: Rc<AgentSpawner>`。

**问题**：蓝图的 `restart_with_checkpoint` 仅做状态转换（Recovering→Ready→Running），不调用 `spawner.spawn()`。spawner 仅用于访问 registry，而 D4 已直接传入 registry。

**决策**：不持有 spawner。restart 仅做状态转换 + 元数据更新 + 心跳重注册，不重新加载 Agent 代码。

### D6: register 调用使用 now 参数

**蓝图设计**：`self.heartbeat.borrow_mut().register(id);`（无 now 参数）。

**问题**：v0.37.0 D2 已将 `register` 签名改为 `register(id, now: u64)`。

**决策**：`self.heartbeat.borrow_mut().register(id, now);`。

### D7: 3 个新错误变体

**蓝图设计**：`MaxRestartsExceeded`、`CheckpointCorrupted`、`RestartFailed`。

**决策**：照搬蓝图，3 个新变体含 `agent_id` 字段，`RestartFailed` 额外含 `reason: String`。

### D8: Checkpointable trait 不被 CrashRecovery 直接调用

**蓝图设计**：定义 `Checkpointable` trait，但 `CrashRecovery` 代码未展示如何调用它。

**问题**：CrashRecovery 仅负责保存/加载原始字节（通过 CheckpointStore），将字节应用到 Agent 实例需调用方通过 `Checkpointable` trait 完成。

**决策**：定义 `Checkpointable` trait 供 Agent 实现者使用，CrashRecovery 不直接调用。`restore_checkpoint` 返回 `Option<Vec<u8>>`，调用方负责通过 `Checkpointable::restore_state` 应用。

### D9: handle_crash 假设 Agent 在 Error 状态

**蓝图设计**：算法流程 `A[心跳超时→检测崩溃] → B[冻结 Agent 能力] → C[Error→Recovering]`，假设 Agent 已在 Error 状态。

**问题**：若 Agent 不在 Error 状态，`Error → Recovering` 转换会失败。

**决策**：`handle_crash` 假设 Agent 在 Error 状态（符合蓝图算法）。若不在 Error，`transition` 自然返回 `InvalidStateTransition` 错误。调用方（心跳监控器/编排器）负责先将 Agent 转为 Error 状态。

## MODIFIED Requirements

### Requirement: AgentError 错误类型

在 v0.37.0 的 13 个变体基础上，新增 3 个变体（共 16 个）：

```rust
pub enum AgentError {
    // ... 既有 13 个变体 ...
    MaxRestartsExceeded { agent_id: AgentId, count: u32 },
    CheckpointCorrupted { agent_id: AgentId },
    RestartFailed { agent_id: AgentId, reason: String },
}
```

### Requirement: lib.rs 模块声明

新增 `pub mod checkpoint;` 和 `pub mod recovery;`，re-export `CheckpointStore`、`InMemoryCheckpointStore`、`Checkpointable`、`CrashRecovery`。VERSION 更新为 "0.38.0"。
