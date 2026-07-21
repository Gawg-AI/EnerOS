# Checklist — v0.36.0 Agent 启动与初始化

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0 crypto + v0.32.0 PKI + v0.33.0 descriptor + v0.34.0 registry + v0.35.0 lifecycle）必须全部继续通过。
> **验证方式**：逐项检查代码 / 运行命令 / 审查文档。

## 一、目录结构校验

- [x] **C1 init.rs 位置**：`crates/agents/agent/src/init.rs` 存在
- [x] **C2 spawner.rs 位置**：`crates/agents/agent/src/spawner.rs` 存在
- [x] **C3 集成测试位置**：`crates/agents/agent/tests/spawner_test.rs` 存在
- [x] **C4 文档分类**：`docs/agents/agent-spawner-design.md` 在 `docs/agents/` 子目录下
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 二、代码结构校验

- [x] **C6 init.rs 存在**：`init.rs` 文件存在
- [x] **C7 spawner.rs 存在**：`spawner.rs` 文件存在
- [x] **C8 lib.rs 模块声明**：`lib.rs` 包含 `pub mod init;` 和 `pub mod spawner;`
- [x] **C9 lib.rs re-export**：`lib.rs` 包含 `pub use init::{AgentConfig, AgentContext, AgentEntry};` 和 `pub use spawner::{AgentFactory, AgentSpawner};`
- [x] **C10 no_std 声明**：`lib.rs` 仍有 `#![cfg_attr(not(test), no_std)]`（未改动）
- [x] **C11 extern crate alloc**：`lib.rs` 仍有 `extern crate alloc;`（未改动）
- [x] **C12 零外部依赖**：`Cargo.toml` 的 `[dependencies]` 仍为空
- [x] **C13 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.36.0";`

## 三、AgentError 扩展校验

- [x] **C14 CodeLoadFailed 变体**：`error.rs` 包含 `CodeLoadFailed(String)`
- [x] **C15 InitFailed 变体**：`error.rs` 包含 `InitFailed(String)`
- [x] **C16 StartFailed 变体**：`error.rs` 包含 `StartFailed(String)`
- [x] **C17 Display 实现**：3 个新变体的 Display 输出含前缀 "code load failed: " / "init failed: " / "start failed: "
- [x] **C18 既有变体未改动**：8 个既有变体（InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId / AgentNotFound / AlreadyRegistered / InvalidStateTransition / AgentNotAlive）及其 Display 保持不变
- [x] **C19 新变体测试**：tests 模块包含 CodeLoadFailed/InitFailed/StartFailed 的 Display + clone/eq 测试
- [x] **C20 String import**：`error.rs` 追加了 `use alloc::string::String;`

## 四、AgentConfig 结构校验

- [x] **C21 结构体定义**：`init.rs` 定义 `pub struct AgentConfig` 含 6 字段（agent_type / name / binary_path / config_path / priority_override / mem_override）
- [x] **C22 derive**：`#[derive(Clone, Debug, PartialEq, Eq)]`
- [x] **C23 Default 实现**：`AgentConfig` 实现 `Default`（agent_type 默认 System，name 默认 "default"，其他 None）
- [x] **C24 字段类型正确**：agent_type: AgentType / name: String / binary_path: Option<String> / config_path: Option<String> / priority_override: Option<u8> / mem_override: Option<usize>

## 五、AgentContext 结构校验

- [x] **C25 结构体定义**：`init.rs` 定义 `pub struct AgentContext` 含 3 字段（agent_id / config / registry）
- [x] **C26 derive**：`#[derive(Debug)]`（不 derive Clone/PartialEq）
- [x] **C27 字段类型正确**：agent_id: AgentId / config: AgentConfig / registry: Rc<RefCell<AgentRegistry>>

## 六、AgentEntry trait 校验

- [x] **C28 trait 定义**：`init.rs` 定义 `pub trait AgentEntry` 含 on_init / on_start / on_stop
- [x] **C29 方法签名**：
  - `fn on_init(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>`
  - `fn on_start(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>`
  - `fn on_stop(&mut self, ctx: &mut AgentContext)`
- [x] **C30 object-safe**：trait 无泛型方法、无 Self 类型参数、无关联函数（支持 `dyn AgentEntry`）

## 七、AgentFactory trait 校验

- [x] **C31 trait 定义**：`spawner.rs` 定义 `pub trait AgentFactory` 含 `create` 方法
- [x] **C32 方法签名**：`fn create(&self, agent_type: AgentType, name: &str) -> Result<Box<dyn AgentEntry>, AgentError>`
- [x] **C33 object-safe**：trait 无泛型方法（支持 `dyn AgentFactory`）

## 八、AgentSpawner 结构校验

- [x] **C34 结构体定义**：`spawner.rs` 定义 `pub struct AgentSpawner` 含 3 字段（registry / lifecycle / factory）
- [x] **C35 字段类型正确**：
  - `registry: Rc<RefCell<AgentRegistry>>`
  - `lifecycle: Rc<RefCell<LifecycleManager>>`（D1 偏差，非蓝图的 `Rc<LifecycleManager>`）
  - `factory: Rc<dyn AgentFactory>`
- [x] **C36 new() 方法**：`pub fn new(registry, lifecycle, factory) -> Self`
- [x] **C37 spawn() 方法**：签名 `(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>`（D4 偏差：追加 now 参数）
- [x] **C38 spawn_blocking() 方法**：签名 `(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>`，委托 `self.spawn(config, now)`（D3 偏差）
- [x] **C39 load_code() 私有方法**：委托 `self.factory.create(config.agent_type, &config.name)`
- [x] **C40 init_context() 私有方法**：返回 `AgentContext { agent_id: id, config: config.clone(), registry: self.registry.clone() }`

## 九、spawn 流程校验

- [x] **C41 步骤 1 创建描述符**：`AgentDescriptor::new(config.agent_type, &config.name, now)`
- [x] **C42 步骤 2 应用覆盖**：priority_override / mem_override 应用到 desc
- [x] **C43 步骤 3 注册**：`self.registry.borrow_mut().register(desc)?`
- [x] **C44 步骤 4 Created→Ready**：`self.lifecycle.borrow().transition(id, AgentState::Ready)?`
- [x] **C45 步骤 5 load_code**：失败时 `force_state(id, Error)` 并返回错误（D5 偏差）
- [x] **C46 步骤 6 init_context**：构造 AgentContext
- [x] **C47 步骤 7 on_init**：失败时 `force_state(id, Error)` 并返回错误（D5 偏差）
- [x] **C48 步骤 8 Ready→Running**：`self.lifecycle.borrow().transition(id, AgentState::Running)?`
- [x] **C49 步骤 9 on_start**：失败时 `force_state(id, Error)` 并返回错误（D5 偏差）
- [x] **C50 步骤 10 返回 Ok(id)**

## 十、no_std 合规校验

- [x] **C51 无 use std::**：`init.rs` 和 `spawner.rs` 中搜索 `use std::` 返回 0 匹配
- [x] **C52 无 panic 宏违规**：非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C53 子模块无 no_std 重复**：`init.rs` 和 `spawner.rs` 不包含 `#![cfg_attr(not(test), no_std)]`
- [x] **C54 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 十一、测试校验

- [x] **C55 init 单元测试**：`init.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C56 AgentConfig 构造/Clone/Eq 测试**：验证 6 字段、Clone、PartialEq
- [x] **C57 AgentConfig Default 测试**：Default 实现返回合理值
- [x] **C58 AgentContext 构造测试**：验证 agent_id / config / registry 字段
- [x] **C59 AgentEntry object-safe 测试**：`Box<dyn AgentEntry>` 装箱并调用方法
- [x] **C60 spawner 单元测试**：`spawner.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C61 spawn 成功测试**：Agent 进入 Running，返回 Ok(id)
- [x] **C62 spawn_blocking 等价测试**：spawn_blocking 同样成功
- [x] **C63 on_init 失败 → Error 测试**：FailInitFactory，Agent 进入 Error，返回 InitFailed
- [x] **C64 on_start 失败 → Error 测试**：FailStartFactory，Agent 进入 Error，返回 StartFailed
- [x] **C65 load_code 失败 → Error 测试**：FailFactory，Agent 进入 Error，返回 CodeLoadFailed
- [x] **C66 priority_override 应用测试**：desc.priority == override 值
- [x] **C67 mem_override 应用测试**：desc.mem_quota == override 值
- [x] **C68 多 Agent 独立 spawn 测试**：3 个 Agent 各自 Running
- [x] **C69 集成测试存在**：`tests/spawner_test.rs` 存在且通过
- [x] **C70 集成测试完整成功路径**：spawn 成功，最终状态 Running
- [x] **C71 集成测试错误路径**：init/start/load_code 失败均进入 Error
- [x] **C72 集成测试 spawn_blocking**：与 spawn 行为一致
- [x] **C73 集成测试 override**：priority/mem override 生效
- [x] **C74 测试覆盖率**：≥ 80%（蓝图 §6.1）

## 十二、版本标识一致性

- [x] **C75 根 Cargo.toml**：`version = "0.36.0"`
- [x] **C76 Makefile**：`VERSION := 0.36.0`
- [x] **C77 ci.yml**：`Version: v0.36.0`
- [x] **C78 gate.rs**：注释含 v0.36.0
- [x] **C79 lib.rs VERSION**：`VERSION = "0.36.0"`
- [x] **C80 无 0.35.0 残留**：grep "0.35.0" 无版本标识残留（历史注释除外）

## 十三、构建与质量校验

- [x] **C81 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C82 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C83 cargo test (agent)**：`cargo test -p eneros-agent` 全部通过
- [x] **C84 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C85 eneros-ci**：`cargo run -p eneros-ci` fmt/clippy/test PASS（audit 可能因网络失败 — 已知问题）
- [x] **C86 cargo deny**：`cargo deny check licenses bans sources` 通过

## 十四、文档校验

- [x] **C87 设计文档存在**：`docs/agents/agent-spawner-design.md` 存在
- [x] **C88 文档内容完整**：版本目标 / 架构定位 / spawn 流程图 / 数据结构 / 模块结构 / D1~D5 偏差 / 错误处理 / 并发设计 / 工厂设计 / on_stop 预留 / 性能 / 后续解锁
- [x] **C89 文档位置正确**：在 `docs/agents/` 子目录下

## 十五、偏差声明记录

- [x] **C90 D1 偏差记录**：Rc<RefCell<LifecycleManager>> 代替 Rc<LifecycleManager>，文档记录
- [x] **C91 D2 偏差记录**：AgentFactory trait 新增（蓝图引用不存在的 create_agent），文档记录
- [x] **C92 D3 偏差记录**：spawn_blocking 委托 spawn（Phase 1 单线程），文档记录
- [x] **C93 D4 偏差记录**：spawn 追加 now: u64 参数（no_std 时间约定），文档记录
- [x] **C94 D5 偏差记录**：错误清理用 force_state 而非 transition（Ready→Error 非法），文档记录

## 十六、蓝图合规校验

- [x] **C95 接口完备性**：蓝图 §3 的所有方法（spawn / spawn_blocking / load_code / init_context）全部实现
- [x] **C96 AgentEntry trait**：蓝图 §3 的 on_init / on_start / on_stop 全部实现
- [x] **C97 AgentConfig 字段**：蓝图 §4.1 的 6 字段全部覆盖
- [x] **C98 AgentContext 字段**：蓝图 §3 的 3 字段全部覆盖
- [x] **C99 蓝图 §4.3 spawn 流程**：8 步流程全部实现
- [x] **C100 蓝图 §4.4 错误变体**：CodeLoadFailed / InitFailed / StartFailed 全部实现
- [x] **C101 蓝图 §6.2 验收**：Agent 成功进入 Running 状态
- [x] **C102 蓝图 §6.5 init 失败 → Error**：测试覆盖
- [x] **C103 蓝图 §8.1 代码加载失败回退**：force_state(Error) + 返回错误
- [x] **C104 蓝图 §8.3 资源分配失败回收**：Agent 进入 Error 状态（注册表保留用于调试）
- [x] **C105 蓝图 §9.3 安全**：失败安全回退（force_state Error）
- [x] **C106 蓝图 §9.4 可靠**：init 失败 → Error
- [x] **C107 蓝图 §9.5 可维护**：AgentEntry trait 抽象
- [x] **C108 蓝图 §9.7 可扩展**：AgentFactory 支持动态加载（Phase 3）
