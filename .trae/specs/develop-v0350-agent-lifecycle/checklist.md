# Checklist — v0.35.0 Agent 生命周期状态机

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0 crypto + v0.32.0 PKI + v0.33.0 descriptor + v0.34.0 registry）必须全部继续通过。
> **验证方式**：逐项检查代码 / 运行命令 / 审查文档。

## 一、目录结构校验

- [x] **C1 lifecycle.rs 位置**：`crates/agents/agent/src/lifecycle.rs` 存在
- [x] **C2 transitions.rs 位置**：`crates/agents/agent/src/lifecycle/transitions.rs` 存在
- [x] **C3 集成测试位置**：`crates/agents/agent/tests/lifecycle_test.rs` 存在
- [x] **C4 文档分类**：`docs/agents/agent-lifecycle-design.md` 在 `docs/agents/` 子目录下
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 二、代码结构校验

- [x] **C6 lifecycle.rs 存在**：`lifecycle.rs` 文件存在
- [x] **C7 transitions 子模块**：`lifecycle.rs` 包含 `pub mod transitions;`
- [x] **C8 lib.rs 模块声明**：`lib.rs` 包含 `pub mod lifecycle;`
- [x] **C9 lib.rs re-export**：`lib.rs` 包含 `pub use lifecycle::{LifecycleEvent, LifecycleHook, LifecycleManager};`
- [x] **C10 no_std 声明**：`lib.rs` 仍有 `#![cfg_attr(not(test), no_std)]`（未改动）
- [x] **C11 extern crate alloc**：`lib.rs` 仍有 `extern crate alloc;`（未改动）
- [x] **C12 零外部依赖**：`Cargo.toml` 的 `[dependencies]` 仍为空
- [x] **C13 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.35.0";`

## 三、AgentError 扩展校验

- [x] **C14 InvalidStateTransition 变体**：`error.rs` 包含 `InvalidStateTransition { from: AgentState, to: AgentState }`（结构变体）
- [x] **C15 AgentNotAlive 变体**：`error.rs` 包含 `AgentNotAlive`（单元变体）
- [x] **C16 Display 实现**：`InvalidStateTransition` 显示 from/to 状态；`AgentNotAlive` 显示 `"agent not alive"`
- [x] **C17 既有变体未改动**：6 个既有变体（InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId / AgentNotFound / AlreadyRegistered）及其 Display 保持不变
- [x] **C18 新变体测试**：tests 模块包含 InvalidStateTransition 和 AgentNotAlive 的 Display + clone/eq 测试
- [x] **C19 AgentState import**：`error.rs` 追加了 `use crate::types::AgentState;`

## 四、TRANSITIONS 转换表校验

- [x] **C20 TRANSITIONS 常量**：`transitions.rs` 定义 `pub const TRANSITIONS: &[(AgentState, AgentState)]`
- [x] **C21 12 条合法转换**：TRANSITIONS 包含蓝图 §4.1 的全部 12 条转换
- [x] **C22 can_transition 函数**：`pub fn can_transition(from: AgentState, to: AgentState) -> bool`
- [x] **C23 Dead 无传出转换**：TRANSITIONS 中无任何 `(Dead, _)` 条目
- [x] **C24 Error→Running 不在表中**：TRANSITIONS 中无 `(Error, Running)` 条目（蓝图 §8.5）
- [x] **C25 自转换不在表中**：TRANSITIONS 中无任何 `(X, X)` 条目

## 五、LifecycleManager 结构校验

- [x] **C26 结构体定义**：`LifecycleManager { registry: Rc<RefCell<AgentRegistry>>, hooks: Vec<Box<dyn LifecycleHook>> }`
- [x] **C27 new() 方法**：`pub fn new(registry: Rc<RefCell<AgentRegistry>>) -> Self`，hooks 初始化为空
- [x] **C28 add_hook() 方法**：`pub fn add_hook(&mut self, hook: Box<dyn LifecycleHook>)`（D3 偏差）
- [x] **C29 can_transition() 方法**：委托 `transitions::can_transition`
- [x] **C30 transition() 方法**：签名 `(&self, id: AgentId, target: AgentState) -> Result<AgentState, AgentError>`；合法时更新状态 + 触发 hooks；非法时返回 `InvalidStateTransition`
- [x] **C31 current_state() 方法**：签名 `(&self, id: AgentId) -> Result<AgentState, AgentError>`
- [x] **C32 force_state() 方法**：签名 `(&mut self, id: AgentId, state: AgentState) -> Result<(), AgentError>`；直接设置状态，不验证，不触发 hooks（D2 偏差）

## 六、LifecycleHook trait 校验

- [x] **C33 trait 定义**：`pub trait LifecycleHook` 含 `on_enter(&self, state: AgentState, id: AgentId)` + `on_exit(&self, state: AgentState, id: AgentId)`
- [x] **C34 object-safe**：trait 无泛型方法、无 Self 类型参数、无关联函数（支持 `dyn LifecycleHook`）

## 七、LifecycleEvent 枚举校验

- [x] **C35 StateChanged 变体**：`StateChanged { from: AgentState, to: AgentState, agent_id: AgentId }`
- [x] **C36 TransitionRejected 变体**：`TransitionRejected { from: AgentState, to: AgentState, reason: String }`
- [x] **C37 derive**：`#[derive(Debug, Clone, PartialEq, Eq)]`

## 八、no_std 合规校验

- [x] **C38 无 use std::**：`lifecycle.rs` 和 `transitions.rs` 中搜索 `use std::` 返回 0 匹配
- [x] **C39 无 panic 宏违规**：非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C40 子模块无 no_std 重复**：`lifecycle.rs` 和 `transitions.rs` 不包含 `#![cfg_attr(not(test), no_std)]`
- [x] **C41 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 九、测试校验

- [x] **C42 transitions 单元测试**：`transitions.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C43 12 条合法转换测试**：遍历 TRANSITIONS 表逐条验证 can_transition == true
- [x] **C44 非法转换测试**：Created→Running / Error→Running / Dead→Ready / 自转换 均返回 false
- [x] **C45 Dead 不可逆测试**：Dead→所有 6 个非 Dead 状态均 false
- [x] **C46 lifecycle 单元测试**：`lifecycle.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C47 合法转换 transition() 测试**：12 条合法转换各返回 Ok(target)
- [x] **C48 非法转换 transition() 测试**：返回 `Err(InvalidStateTransition { from, to })`
- [x] **C49 AgentNotFound 测试**：transition / current_state / force_state 对不存在的 ID 返回 `Err(AgentNotFound)`
- [x] **C50 Hook 触发顺序测试**：on_exit 在 state 变更前、on_enter 在 state 变更后
- [x] **C51 force_state 不触发 hooks 测试**：添加 hook 后 force_state 不增加 hook 调用记录（D2 偏差）
- [x] **C52 完整生命周期路径测试**：Created→Ready→Running→Suspended→Running→Error→Recovering→Ready→Running→Dead
- [x] **C53 Dead 不可逆 transition 测试**：Dead 后所有 transition 返回 Err
- [x] **C54 集成测试存在**：`tests/lifecycle_test.rs` 存在且通过
- [x] **C55 共享注册表测试**：两个 LifecycleManager 共享同一 Rc<RefCell<AgentRegistry>>，状态变更互可见
- [x] **C56 测试覆盖率**：≥ 80%（蓝图 §6.1）

## 十、版本标识一致性

- [x] **C57 根 Cargo.toml**：`version = "0.35.0"`
- [x] **C58 Makefile**：`VERSION := 0.35.0`
- [x] **C59 ci.yml**：`Version: v0.35.0`
- [x] **C60 gate.rs**：注释含 v0.35.0
- [x] **C61 lib.rs VERSION**：`VERSION = "0.35.0"`
- [x] **C62 无 0.34.0 残留**：grep "0.34.0" 无版本标识残留（历史注释除外）

## 十一、构建与质量校验

- [x] **C63 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C64 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C65 cargo test (agent)**：`cargo test -p eneros-agent` 全部通过
- [x] **C66 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C67 eneros-ci**：fmt/clippy/test PASS（audit 因 GitHub advisory 数据库网络不可达失败 — 已知环境问题，非版本阻塞）
- [x] **C68 cargo deny**：`cargo deny check licenses bans sources` 通过

## 十二、文档校验

- [x] **C69 设计文档存在**：`docs/agents/agent-lifecycle-design.md` 存在
- [x] **C70 文档内容完整**：7 状态 / 12 转换 / 状态图 / 数据结构 / 模块结构 / D1~D5 偏差 / 性能 / 并发 / Hook / Dead 不可逆 / 后续解锁
- [x] **C71 文档位置正确**：在 `docs/agents/` 子目录下

## 十三、偏差声明记录

- [x] **C72 D1 偏差记录**：Rc<RefCell> 单线程设计，Phase 3 替换策略，文档记录
- [x] **C73 D2 偏差记录**：force_state 不触发 hooks，文档记录
- [x] **C74 D3 偏差记录**：add_hook 方法（蓝图未显式声明但必需），文档记录
- [x] **C75 D4 偏差记录**：LifecycleEvent 仅定义数据结构，无事件分发，文档记录
- [x] **C76 D5 偏差记录**：Hook 在 RefCell 借用期间调用，不得访问 registry，文档记录

## 十四、蓝图合规校验

- [x] **C77 接口完备性**：蓝图 §3 的所有方法（new / transition / can_transition / current_state / force_state）全部实现
- [x] **C78 LifecycleEvent 实现**：蓝图 §3 的 LifecycleEvent 枚举已实现
- [x] **C79 LifecycleHook trait**：蓝图 §4.2 的 trait 已实现
- [x] **C80 TRANSITIONS 表**：蓝图 §4.1 的 12 条转换全部覆盖
- [x] **C81 蓝图 §8.1 Dead 不可逆**：Dead 状态无任何合法传出转换（测试覆盖）
- [x] **C82 蓝图 §8.5 Error→Running 非法**：不在 TRANSITIONS 表中（测试覆盖）
- [x] **C83 蓝图 §6.2 端到端测试**：完整生命周期路径测试通过
- [x] **C84 蓝图 §6.5 非法转换注入**：非法转换被拒绝并返回 InvalidStateTransition（测试覆盖）
- [x] **C85 蓝图 §9.3 安全**：非法转换拒绝
- [x] **C86 蓝图 §9.4 可靠**：Dead 不可逆
- [x] **C87 蓝图 §9.7 可扩展**：Hook 机制支持扩展
