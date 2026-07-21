# Tasks — v0.38.0 Agent 崩溃自动重启

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 错误扩展 → Wave 2 checkpoint 模块 → Wave 3 recovery 模块 → Wave 4 lib.rs → Wave 5 测试 → Wave 6 文档+版本 → Wave 7 验证
> **目标驱动**：每个任务附验证条件，可独立 loop 直到通过。

## Wave 1: 错误类型扩展（前置）

- [x] **Task 1: 扩展 AgentError 三个崩溃恢复错误变体**
  - 修改 `crates/agents/agent/src/error.rs`：
    - 在 `AgentError` 枚举末尾（`AgentUnhealthy` 之后）追加 3 个变体：
      - `MaxRestartsExceeded { agent_id: AgentId, count: u32 }` — 注释"超过最大重启次数"
      - `CheckpointCorrupted { agent_id: AgentId }` — 注释"检查点数据损坏"
      - `RestartFailed { agent_id: AgentId, reason: String }` — 注释"重启失败"
    - 在 `Display` impl 追加：
      - `MaxRestartsExceeded { agent_id, count } => write!(f, "max restarts exceeded: agent {:?} restarted {} times", agent_id, count),`
      - `CheckpointCorrupted { agent_id } => write!(f, "checkpoint corrupted: agent {:?}", agent_id),`
      - `RestartFailed { agent_id, reason } => write!(f, "restart failed: agent {:?}, reason: {}", agent_id, reason),`
    - 在 tests 模块追加 `test_recovery_error_variants_display`：
      - 验证 `MaxRestartsExceeded { agent_id: AgentId(42), count: 3 }` Display 输出含 "max restarts exceeded" 和 "restarted 3 times"
      - 验证 `CheckpointCorrupted { agent_id: AgentId(42) }` Display 输出含 "checkpoint corrupted"
      - 验证 `RestartFailed { agent_id: AgentId(42), reason: "timeout" }` Display 输出含 "restart failed" 和 "reason: timeout"
    - 在 tests 模块追加 `test_recovery_error_variants_eq`：
      - 验证 `MaxRestartsExceeded` 同参数相等、不同 count 不等
      - 验证 `CheckpointCorrupted` 同参数相等
      - 验证 `RestartFailed` 同参数相等、不同 reason 不等
      - 验证 3 个新变体互不相等
  - **不修改**既有 13 个变体
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo test -p eneros-agent` 全部通过

## Wave 2: checkpoint 模块（CheckpointStore + Checkpointable）

- [x] **Task 2: 创建 checkpoint.rs — CheckpointStore trait + InMemoryCheckpointStore + Checkpointable trait**
  - 创建 `crates/agents/agent/src/checkpoint.rs`：
    - 模块文档注释（Agent 检查点 / CheckpointStore trait / InMemoryCheckpointStore / Checkpointable / D1+D8 偏差）
    - `use alloc::collections::BTreeMap;`
    - `use alloc::vec::Vec;`
    - `use crate::id::AgentId;`
    - `use crate::error::AgentError;`
    - `CheckpointStore` trait（object-safe，D1 偏差）：
      ```rust
      pub trait CheckpointStore {
          fn save(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError>;
          fn load(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError>;
          fn delete(&self, id: AgentId) -> Result<(), AgentError>;
      }
      ```
    - `InMemoryCheckpointStore` struct（D1 默认实现）：
      ```rust
      pub struct InMemoryCheckpointStore {
          store: BTreeMap<AgentId, Vec<u8>>,
      }
      ```
      - `pub fn new() -> Self`
      - `impl Default for InMemoryCheckpointStore`
      - `impl CheckpointStore for InMemoryCheckpointStore`：
        - `save`: `self.store.insert(id, Vec::from(data)); Ok(())`
        - `load`: `Ok(self.store.get(&id).cloned())`
        - `delete`: `self.store.remove(&id); Ok(())`
    - `Checkpointable` trait（object-safe，D8 偏差）：
      ```rust
      pub trait Checkpointable {
          fn save_state(&self) -> Vec<u8>;
          fn restore_state(&mut self, data: &[u8]) -> Result<(), AgentError>;
      }
      ```
      文档注释说明：供 Agent 实现者提供自定义状态保存/恢复，CrashRecovery 不直接调用
  - **验证**：`cargo build -p eneros-agent` 编译通过（需先在 lib.rs 声明模块，实际在 Task 4 统一声明）

## Wave 3: recovery 模块（CrashRecovery）

- [x] **Task 3: 创建 recovery.rs — CrashRecovery 崩溃恢复器**
  - 创建 `crates/agents/agent/src/recovery.rs`：
    - 模块文档注释（Agent 崩溃恢复 / CrashRecovery / handle_crash 算法 / D1~D9 偏差）
    - `use alloc::rc::Rc;`
    - `use core::cell::RefCell;`
    - `use crate::checkpoint::CheckpointStore;`
    - `use crate::error::AgentError;`
    - `use crate::id::AgentId;`
    - `use crate::registry::AgentRegistry;`
    - `use crate::heartbeat::HeartbeatMonitor;`
    - `use crate::lifecycle::LifecycleManager;`
    - `use crate::types::AgentState;`
    - 常量：
      ```rust
      const DEFAULT_MAX_RESTARTS: u32 = 3;
      ```
    - `CrashRecovery` struct（5 字段，derive `Debug`）：
      ```rust
      pub struct CrashRecovery {
          registry: Rc<RefCell<AgentRegistry>>,
          heartbeat: Rc<RefCell<HeartbeatMonitor>>,
          lifecycle: Rc<RefCell<LifecycleManager>>,
          checkpoint_store: Rc<dyn CheckpointStore>,
          max_restarts: u32,
      }
      ```
    - `impl CrashRecovery`：
      - `pub fn new(registry, heartbeat, lifecycle, checkpoint_store, max_restarts) -> Self`（D3+D4+D5 偏差）
      - `pub fn with_defaults(registry, heartbeat, lifecycle, checkpoint_store) -> Self`（使用 DEFAULT_MAX_RESTARTS）
      - `pub fn handle_crash(&self, id: AgentId, now: u64) -> Result<(), AgentError>`（D2+D9 偏差）
        - 步骤 1：`self.lifecycle.borrow().transition(id, AgentState::Recovering)?`（Error→Recovering，D9：假设 Error 状态）
        - 步骤 2：获取 restart_count：
          ```rust
          let restart_count = self.registry.borrow().get(id).map(|d| d.restart_count).unwrap_or(0);
          ```
        - 步骤 3：若 `restart_count >= self.max_restarts`：
          - `self.lifecycle.borrow().transition(id, AgentState::Dead)?`（Recovering→Dead）
          - 返回 `Err(AgentError::MaxRestartsExceeded { agent_id: id, count: restart_count })`
        - 步骤 4：调用 `self.restart(id, now)?`
        - 步骤 5：返回 `Ok(())`
      - `pub fn restart(&self, id: AgentId, now: u64) -> Result<(), AgentError>`（D2+D5 偏差）
        - 步骤 1：`self.lifecycle.borrow().transition(id, AgentState::Ready)?`（Recovering→Ready）
        - 步骤 2：`self.lifecycle.borrow().transition(id, AgentState::Running)?`（Ready→Running）
        - 步骤 3：更新 restart_count + last_heartbeat：
          ```rust
          let mut reg = self.registry.borrow_mut();
          if let Some(desc) = reg.get_mut(id) {
              desc.restart_count += 1;
              desc.last_heartbeat = now;
          }
          ```
        - 步骤 4：重新注册心跳：`self.heartbeat.borrow_mut().register(id, now);`（D6 偏差）
        - 步骤 5：返回 `Ok(())`
      - `pub fn restore_checkpoint(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError>`
        - 委托 `self.checkpoint_store.load(id)`
      - `pub fn save_checkpoint(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError>`
        - 委托 `self.checkpoint_store.save(id, data)`
  - **验证**：`cargo build -p eneros-agent` 编译通过

## Wave 4: lib.rs 更新

- [x] **Task 4: 更新 lib.rs — 模块声明与 re-export**
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在模块声明区追加 `pub mod checkpoint;`（在 `pub mod error;` 之后）和 `pub mod recovery;`（在 `pub mod heartbeat;` 之后）
    - 在 re-export 区追加：
      - `pub use checkpoint::{CheckpointStore, Checkpointable, InMemoryCheckpointStore};`
      - `pub use recovery::CrashRecovery;`
    - 更新 `VERSION`：`pub const VERSION: &str = "0.38.0";`
    - 更新文件头部文档注释：版本号 0.37.0 → 0.38.0，追加 checkpoint 和 recovery 模块说明：
      ```
      //! - [`CheckpointStore`] / [`InMemoryCheckpointStore`] / [`Checkpointable`] — 检查点存储与可检查点 trait
      //! - [`CrashRecovery`] — Agent 崩溃恢复器（最多 3 次重启、检查点恢复、3 次失败→Dead）
      ```
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo doc -p eneros-agent` 无警告

## Wave 5: 测试

- [x] **Task 5: 编写 checkpoint.rs 单元测试**
  - 在 `checkpoint.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_inmemory_save_load` — save 后 load 返回 Some，数据一致
    - `test_inmemory_load_nonexistent` — 未保存的 id load 返回 None
    - `test_inmemory_delete` — save 后 delete 再 load 返回 None
    - `test_inmemory_delete_nonexistent` — 删除不存在的 id 不报错（返回 Ok）
    - `test_inmemory_overwrite` — 同一 id 二次 save 覆盖旧数据
    - `test_inmemory_default` — Default trait 构造空存储
    - `test_inmemory_multiple_agents` — 多个 Agent 检查点独立存储
    - `test_checkpoint_store_trait_object` — `Rc<dyn CheckpointStore>` 装箱 InMemoryCheckpointStore 并调用方法
    - `test_checkpointable_object_safe` — 定义 `struct Agent with state: u32`，实现 Checkpointable，装箱为 `Box<dyn Checkpointable>` 调用 save_state/restore_state
    - `test_checkpointable_restore` — save_state 后 restore_state 恢复状态一致
  - **验证**：`cargo test -p eneros-agent` 通过

- [x] **Task 6: 编写 recovery.rs 单元测试**
  - 在 `recovery.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - 辅助函数 `make_recovery()` — 构造 CrashRecovery（registry + heartbeat + lifecycle + InMemoryCheckpointStore + max_restarts=3）
    - 辅助函数 `make_recovery_with_max(max_restarts)` — 可配置 max_restarts
    - 辅助函数 `spawn_agent_at_error(recovery, agent_type, name, now)` — spawn Agent 并 force_state 到 Error
    - `test_handle_crash_first_restart` — Agent 在 Error（restart_count=0），handle_crash 后 Running，restart_count=1
    - `test_handle_crash_second_restart` — restart_count=1，handle_crash 后 Running，restart_count=2
    - `test_handle_crash_third_restart` — restart_count=2，handle_crash 后 Running，restart_count=3
    - `test_handle_crash_exceeds_max_restarts` — restart_count=3，handle_crash 后 Dead，返回 MaxRestartsExceeded
    - `test_handle_crash_not_in_error_state` — Agent 在 Running，handle_crash 返回 InvalidStateTransition
    - `test_handle_crash_nonexistent_agent` — 不存在的 id 返回 AgentNotFound
    - `test_restart_transitions_to_running` — restart 后状态 Running
    - `test_restart_increments_restart_count` — restart 后 restart_count +1
    - `test_restart_updates_last_heartbeat` — restart 后 last_heartbeat == now
    - `test_restart_re_registers_heartbeat` — restart 后 is_healthy == true
    - `test_save_and_restore_checkpoint` — save_checkpoint 后 restore_checkpoint 返回 Some
    - `test_restore_checkpoint_nonexistent` — 未保存的 id restore_checkpoint 返回 None
    - `test_with_defaults` — with_defaults 使用 DEFAULT_MAX_RESTARTS=3
    - `test_custom_max_restarts` — max_restarts=1，第二次 handle_crash 即 Dead
    - `test_handle_crash_multiple_agents_independent` — 2 个 Agent 独立恢复
  - **验证**：`cargo test -p eneros-agent` 全部通过

- [x] **Task 7: 编写集成测试 tests/recovery_test.rs**
  - 创建 `crates/agents/agent/tests/recovery_test.rs`：
    - `use eneros_agent::{AgentId, AgentRegistry, AgentState, AgentType, AgentDescriptor, CrashRecovery, HeartbeatMonitor, InMemoryCheckpointStore, LifecycleManager, AgentError};`
    - `use alloc::rc::Rc;`（或 std::rc::Rc 在集成测试中）
    - `use core::cell::RefCell;`
    - 辅助函数 `make_recovery()` — 构造完整 CrashRecovery 环境
    - 辅助函数 `spawn_and_crash(recovery, agent_type, name, now)` — spawn Agent 并 force_state 到 Error
    - 集成测试：
      - `integration_crash_recovery_full_lifecycle` — spawn → crash（Error）→ handle_crash → Running，restart_count=1
      - `integration_crash_recovery_with_checkpoint` — save_checkpoint → crash → handle_crash → restore_checkpoint 返回 Some
      - `integration_crash_recovery_no_checkpoint` — crash → handle_crash → restore_checkpoint 返回 None
      - `integration_max_restarts_exceeds_to_dead` — 连续 3 次 handle_crash 成功，第 4 次返回 MaxRestartsExceeded，状态 Dead
      - `integration_recovery_re_registers_heartbeat` — handle_crash 后心跳监控器中 is_healthy == true
      - `integration_multiple_agents_independent_recovery` — 2 个 Agent 各自崩溃恢复独立
      - `integration_custom_max_restarts` — max_restarts=2，第 3 次即 Dead
      - `integration_checkpoint_store_trait_object` — Rc<dyn CheckpointStore> 装箱使用
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 6: 文档与版本标识

- [x] **Task 8: 编写设计文档**
  - 创建 `docs/agents/agent-crash-recovery-design.md`：
    - 版本目标 / 架构定位 / 前置依赖
    - handle_crash 算法流程（含 mermaid flowchart，复制蓝图 §4.3）
    - 数据结构设计（CrashRecovery / CheckpointStore / InMemoryCheckpointStore / Checkpointable）
    - 模块结构（checkpoint.rs + recovery.rs）
    - 偏差声明 D1~D9
    - 错误处理（MaxRestartsExceeded / CheckpointCorrupted / RestartFailed）
    - 检查点设计（trait 抽象 / InMemoryCheckpointStore / 生产环境注入）
    - 重启策略（最多 3 次 / 检查点优先 / 3 次失败→Dead）
    - 状态转换路径（Error→Recovering→Ready→Running 或 Recovering→Dead）
    - 不完整重启说明（D5：仅状态转换，不重新加载代码）
    - 性能分析（handle_crash O(log n) 状态转换 + O(1) 检查点操作）
    - 后续解锁版本（v0.41.0 System Agent / v0.42.0 故障恢复编排）
  - **验证**：文档存在且内容完整

- [x] **Task 9: 同步版本标识**
  - 根 `Cargo.toml`：`version = "0.38.0"`
  - `Makefile`：`VERSION := 0.38.0` + header 注释 + agent-build 描述更新为 "v0.38.0 Crash Recovery"
  - `.github/workflows/ci.yml`：`Version: v0.38.0`
  - `ci/src/gate.rs`：注释更新为 v0.38.0
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.38.0"`（Task 4 已完成）
  - **验证**：`grep -r "0.37.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（历史注释除外）

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
- Task 2: 无依赖（checkpoint 模块独立于 error.rs）
- Task 3: 依赖 Task 1（需要新错误变体）+ Task 2（需要 CheckpointStore trait）
- Task 4: 依赖 Task 2 + Task 3（lib.rs 声明模块前，两模块应完整）
- Task 5: 依赖 Task 4（单元测试需要模块可被引用）
- Task 6: 依赖 Task 5（recovery 测试在 checkpoint 测试后）
- Task 7: 依赖 Task 6（集成测试在单元测试后）
- Task 8-9: 依赖 Task 4（可并行，文档与版本标识独立）
- Task 10: 依赖 Task 1-9 全部完成
