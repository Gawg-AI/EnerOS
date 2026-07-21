# Checklist — v0.38.0 Agent 崩溃自动重启

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0~v0.37.0）必须全部继续通过。
> **验证方式**：逐项检查代码 / 运行命令 / 审查文档。

## 一、目录结构校验

- [x] **C1 checkpoint.rs 位置**：`crates/agents/agent/src/checkpoint.rs` 存在
- [x] **C2 recovery.rs 位置**：`crates/agents/agent/src/recovery.rs` 存在
- [x] **C3 集成测试位置**：`crates/agents/agent/tests/recovery_test.rs` 存在
- [x] **C4 文档分类**：`docs/agents/agent-crash-recovery-design.md` 在 `docs/agents/` 子目录下
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 二、代码结构校验

- [x] **C6 checkpoint.rs 存在**：`checkpoint.rs` 文件存在
- [x] **C7 recovery.rs 存在**：`recovery.rs` 文件存在
- [x] **C8 lib.rs 模块声明**：`lib.rs` 包含 `pub mod checkpoint;` 和 `pub mod recovery;`
- [x] **C9 lib.rs re-export**：`lib.rs` 包含 `pub use checkpoint::{CheckpointStore, Checkpointable, InMemoryCheckpointStore};` 和 `pub use recovery::CrashRecovery;`
- [x] **C10 no_std 声明**：`lib.rs` 仍有 `#![cfg_attr(not(test), no_std)]`（未改动）
- [x] **C11 extern crate alloc**：`lib.rs` 仍有 `extern crate alloc;`（未改动）
- [x] **C12 零外部依赖**：`Cargo.toml` 的 `[dependencies]` 仍为空
- [x] **C13 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.38.0";`

## 三、AgentError 扩展校验

- [x] **C14 MaxRestartsExceeded 变体**：`error.rs` 包含 `MaxRestartsExceeded { agent_id: AgentId, count: u32 }`
- [x] **C15 CheckpointCorrupted 变体**：`error.rs` 包含 `CheckpointCorrupted { agent_id: AgentId }`
- [x] **C16 RestartFailed 变体**：`error.rs` 包含 `RestartFailed { agent_id: AgentId, reason: String }`
- [x] **C17 Display 实现**：3 个新变体的 Display 输出含 "max restarts exceeded" / "checkpoint corrupted" / "restart failed"
- [x] **C18 既有变体未改动**：13 个既有变体（含 v0.37.0 的 HeartbeatTimeout/AgentUnhealthy）及其 Display 保持不变
- [x] **C19 新变体测试**：tests 模块包含 MaxRestartsExceeded/CheckpointCorrupted/RestartFailed 的 Display + clone/eq 测试

## 四、CheckpointStore trait 校验

- [x] **C20 trait 定义**：`checkpoint.rs` 定义 `pub trait CheckpointStore` 含 save / load / delete 方法
- [x] **C21 save 签名**：`fn save(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError>`
- [x] **C22 load 签名**：`fn load(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError>`
- [x] **C23 delete 签名**：`fn delete(&self, id: AgentId) -> Result<(), AgentError>`
- [x] **C24 object-safe**：trait 无泛型方法、无 Self 类型参数（支持 `dyn CheckpointStore`）

## 五、InMemoryCheckpointStore 校验

- [x] **C25 结构体定义**：`checkpoint.rs` 定义 `pub struct InMemoryCheckpointStore` 含 `store: BTreeMap<AgentId, Vec<u8>>`
- [x] **C26 new() 方法**：`pub fn new() -> Self`
- [x] **C27 Default 实现**：`impl Default for InMemoryCheckpointStore`
- [x] **C28 CheckpointStore impl**：`impl CheckpointStore for InMemoryCheckpointStore` 实现 3 个方法
- [x] **C29 save 行为**：insert 数据到 BTreeMap
- [x] **C30 load 行为**：返回 `self.store.get(&id).cloned()`（不存在返回 None）
- [x] **C31 delete 行为**：`self.store.remove(&id)`（不存在不报错）

## 六、Checkpointable trait 校验

- [x] **C32 trait 定义**：`checkpoint.rs` 定义 `pub trait Checkpointable` 含 save_state / restore_state 方法
- [x] **C33 save_state 签名**：`fn save_state(&self) -> Vec<u8>`
- [x] **C34 restore_state 签名**：`fn restore_state(&mut self, data: &[u8]) -> Result<(), AgentError>`
- [x] **C35 object-safe**：trait 支持 `dyn Checkpointable`（restore_state 的 `&mut self` 在 trait object 中合法）
- [x] **C36 文档说明**：注释说明 CrashRecovery 不直接调用此 trait（D8 偏差）

## 七、CrashRecovery 结构校验

- [x] **C37 结构体定义**：`recovery.rs` 定义 `pub struct CrashRecovery` 含 5 字段（registry / heartbeat / lifecycle / checkpoint_store / max_restarts）
- [x] **C38 derive**：`#[derive(Debug)]`
- [x] **C39 字段类型正确**：
  - `registry: Rc<RefCell<AgentRegistry>>`（D4 偏差）
  - `heartbeat: Rc<RefCell<HeartbeatMonitor>>`
  - `lifecycle: Rc<RefCell<LifecycleManager>>`（D3 偏差）
  - `checkpoint_store: Rc<dyn CheckpointStore>`（D1 偏差）
  - `max_restarts: u32`
- [x] **C40 无 spawner 字段**：CrashRecovery 不持有 spawner（D5 偏差）

## 八、CrashRecovery API 校验

- [x] **C41 new() 方法**：`pub fn new(registry, heartbeat, lifecycle, checkpoint_store, max_restarts) -> Self`
- [x] **C42 with_defaults() 方法**：`pub fn with_defaults(registry, heartbeat, lifecycle, checkpoint_store) -> Self`（使用 DEFAULT_MAX_RESTARTS）
- [x] **C43 handle_crash() 方法**：签名 `(&self, id: AgentId, now: u64) -> Result<(), AgentError>`（D2 偏差）
- [x] **C44 restart() 方法**：签名 `(&self, id: AgentId, now: u64) -> Result<(), AgentError>`（D2 偏差）
- [x] **C45 restore_checkpoint() 方法**：签名 `(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError>`
- [x] **C46 save_checkpoint() 方法**：签名 `(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError>`

## 九、handle_crash 算法校验

- [x] **C47 步骤 1 Error→Recovering**：`self.lifecycle.borrow().transition(id, AgentState::Recovering)?`（D9：假设 Error 状态）
- [x] **C48 步骤 2 获取 restart_count**：从 registry 读取 `desc.restart_count`
- [x] **C49 步骤 3 超限判定**：`restart_count >= self.max_restarts` → Recovering→Dead + MaxRestartsExceeded
- [x] **C50 步骤 4 调用 restart**：`self.restart(id, now)?`
- [x] **C51 步骤 5 返回 Ok**：恢复成功返回 `Ok(())`

## 十、restart 算法校验

- [x] **C52 步骤 1 Recovering→Ready**：`self.lifecycle.borrow().transition(id, AgentState::Ready)?`
- [x] **C53 步骤 2 Ready→Running**：`self.lifecycle.borrow().transition(id, AgentState::Running)?`
- [x] **C54 步骤 3 更新 restart_count**：`desc.restart_count += 1`
- [x] **C55 步骤 4 更新 last_heartbeat**：`desc.last_heartbeat = now`
- [x] **C56 步骤 5 重新注册心跳**：`self.heartbeat.borrow_mut().register(id, now)`（D6 偏差）
- [x] **C57 步骤 6 返回 Ok**

## 十一、检查点方法校验

- [x] **C58 restore_checkpoint 委托**：`self.checkpoint_store.load(id)`
- [x] **C59 save_checkpoint 委托**：`self.checkpoint_store.save(id, data)`

## 十二、默认常量校验

- [x] **C60 DEFAULT_MAX_RESTARTS**：`const DEFAULT_MAX_RESTARTS: u32 = 3;`

## 十三、no_std 合规校验

- [x] **C61 无 use std::**：`checkpoint.rs` 和 `recovery.rs` 中搜索 `use std::` 返回 0 匹配
- [x] **C62 无 panic 宏违规**：非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C63 子模块无 no_std 重复**：`checkpoint.rs` 和 `recovery.rs` 不包含 `#![cfg_attr(not(test), no_std)]`
- [x] **C64 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 十四、测试校验

- [x] **C65 checkpoint 单元测试**：`checkpoint.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C66 InMemoryCheckpointStore save/load 测试**：保存后加载返回 Some，数据一致
- [x] **C67 InMemoryCheckpointStore load 不存在 测试**：返回 None
- [x] **C68 InMemoryCheckpointStore delete 测试**：删除后加载返回 None
- [x] **C69 InMemoryCheckpointStore 覆盖 测试**：同 id 二次 save 覆盖
- [x] **C70 InMemoryCheckpointStore 多 Agent 测试**：多 Agent 独立
- [x] **C71 CheckpointStore trait object 测试**：`Rc<dyn CheckpointStore>` 装箱
- [x] **C72 Checkpointable object-safe 测试**：`Box<dyn Checkpointable>` 装箱
- [x] **C73 Checkpointable restore 测试**：save/restore 状态一致
- [x] **C74 recovery 单元测试**：`recovery.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C75 handle_crash 首次重启 测试**：Error→handle_crash→Running，restart_count=1
- [x] **C76 handle_crash 第二次重启 测试**：restart_count=1→2
- [x] **C77 handle_crash 第三次重启 测试**：restart_count=2→3
- [x] **C78 handle_crash 超限→Dead 测试**：restart_count=3→Dead + MaxRestartsExceeded
- [x] **C79 handle_crash 非 Error 状态 测试**：Running 状态返回 InvalidStateTransition
- [x] **C80 handle_crash 不存在 Agent 测试**：返回 AgentNotFound
- [x] **C81 restart 状态转换 测试**：restart 后 Running
- [x] **C82 restart restart_count++ 测试**：restart_count 递增
- [x] **C83 restart last_heartbeat 更新 测试**：last_heartbeat == now
- [x] **C84 restart 心跳重注册 测试**：is_healthy == true
- [x] **C85 save/restore 检查点 测试**：save 后 restore 返回 Some
- [x] **C86 restore 不存在检查点 测试**：返回 None
- [x] **C87 with_defaults 测试**：使用 DEFAULT_MAX_RESTARTS=3
- [x] **C88 自定义 max_restarts 测试**：max_restarts=1，第二次即 Dead
- [x] **C89 多 Agent 独立恢复 测试**：2 个 Agent 独立
- [x] **C90 集成测试存在**：`tests/recovery_test.rs` 存在且通过
- [x] **C91 集成测试完整生命周期**：spawn → crash → handle_crash → Running
- [x] **C92 集成测试检查点恢复**：save_checkpoint → crash → restore_checkpoint
- [x] **C93 集成测试无检查点**：crash → restore_checkpoint 返回 None
- [x] **C94 集成测试超限→Dead**：连续 3 次后第 4 次 Dead
- [x] **C95 集成测试心跳重注册**：handle_crash 后 is_healthy
- [x] **C96 集成测试多 Agent 独立**
- [x] **C97 集成测试自定义 max_restarts**
- [x] **C98 集成测试 trait object**
- [x] **C99 测试覆盖率**：≥ 80%（蓝图 §6.1）

## 十五、版本标识一致性

- [x] **C100 根 Cargo.toml**：`version = "0.38.0"`
- [x] **C101 Makefile**：`VERSION := 0.38.0`
- [x] **C102 ci.yml**：`Version: v0.38.0`
- [x] **C103 gate.rs**：注释含 v0.38.0
- [x] **C104 lib.rs VERSION**：`VERSION = "0.38.0"`
- [x] **C105 无 0.37.0 残留**：grep "0.37.0" 无版本标识残留（历史注释除外）

## 十六、构建与质量校验

- [x] **C106 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C107 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C108 cargo test (agent)**：`cargo test -p eneros-agent` 全部通过
- [x] **C109 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C110 eneros-ci**：`cargo run -p eneros-ci` fmt/clippy/test PASS（audit 可能因网络失败 — 已知问题）
- [x] **C111 cargo deny**：`cargo deny check licenses bans sources` 通过

## 十七、文档校验

- [x] **C112 设计文档存在**：`docs/agents/agent-crash-recovery-design.md` 存在
- [x] **C113 文档内容完整**：版本目标 / 架构定位 / handle_crash 算法图 / 数据结构 / 模块结构 / D1~D9 偏差 / 错误处理 / 检查点设计 / 重启策略 / 状态转换路径 / 不完整重启说明 / 性能 / 后续解锁
- [x] **C114 文档位置正确**：在 `docs/agents/` 子目录下

## 十八、偏差声明记录

- [x] **C115 D1 偏差记录**：CheckpointStore 为 trait（非 struct），文档记录
- [x] **C116 D2 偏差记录**：handle_crash/restart 追加 now: u64（no_std 时间约定），文档记录
- [x] **C117 D3 偏差记录**：lifecycle 使用 Rc<RefCell<>>（force_state 需 &mut self），文档记录
- [x] **C118 D4 偏差记录**：registry 直接传入（spawner.registry 私有），文档记录
- [x] **C119 D5 偏差记录**：不持有 spawner（restart 仅状态转换），文档记录
- [x] **C120 D6 偏差记录**：register 调用使用 now 参数（v0.37.0 D2 兼容），文档记录
- [x] **C121 D7 偏差记录**：3 个新错误变体，文档记录
- [x] **C122 D8 偏差记录**：Checkpointable 不被 CrashRecovery 直接调用，文档记录
- [x] **C123 D9 偏差记录**：handle_crash 假设 Error 状态，文档记录

## 十九、蓝图合规校验

- [x] **C124 接口完备性**：蓝图 §3 的所有方法（handle_crash / restart / restore_checkpoint / save_checkpoint）全部实现
- [x] **C125 Checkpointable trait**：蓝图 §3 的 save_state / restore_state 实现
- [x] **C126 CheckpointStore 接口**：蓝图 §4.2 的 save / load / delete 实现
- [x] **C127 蓝图 §4.3 handle_crash 算法**：mermaid 流程实现
- [x] **C128 蓝图 §4.4 错误变体**：MaxRestartsExceeded / CheckpointCorrupted / RestartFailed 实现
- [x] **C129 蓝图 §6.2 崩溃后自动恢复**：测试覆盖
- [x] **C130 蓝图 §6.3 重启延迟 <5s**：状态转换 O(log n)，满足
- [x] **C131 蓝图 §6.5 连续 3 次崩溃→Dead**：测试覆盖
- [x] **C132 蓝图 §8.1 检查点损坏**：CheckpointCorrupted 错误变体
- [x] **C133 蓝图 §9.1 功能**：崩溃检测/重启/检查点
- [x] **C134 蓝图 §9.2 性能**：重启 <5s
- [x] **C135 蓝图 §9.3 安全**：冻结崩溃 Agent 能力（通过状态转换实现）
- [x] **C136 蓝图 §9.4 可靠**：3 次重试 + Dead
- [x] **C137 蓝图 §9.5 可维护**：Checkpointable trait
- [x] **C138 蓝图 §9.6 可观测**：restart_count 可查
- [x] **C139 蓝图 §9.7 可扩展**：CheckpointStore trait 支持自定义后端
