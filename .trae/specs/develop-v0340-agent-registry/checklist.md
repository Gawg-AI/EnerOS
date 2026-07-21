# Checklist — v0.34.0 Agent 注册表与发现

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0 的 249 tests + v0.32.0 的 402 tests + v0.33.0 agent crate tests）必须全部继续通过。
> **验证方式**：逐项检查代码 / 运行命令 / 审查文档。

## 一、目录结构校验

- [x] **C1 新文件位置**：`crates/agents/agent/src/registry.rs` 存在，未放根目录
- [x] **C2 集成测试位置**：`crates/agents/agent/tests/registry_test.rs` 存在
- [x] **C3 文档分类**：`docs/agents/agent-registry-design.md` 在 `docs/agents/` 子目录下，未平面化放 `docs/` 根
- [x] **C4 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹
- [x] **C5 workspace members**：根 `Cargo.toml` members 仍包含 `"crates/agents/agent"`（v0.33.0 已添加，本版本不新增 crate）

## 二、代码结构校验

- [x] **C6 registry.rs 存在**：`crates/agents/agent/src/registry.rs` 文件存在
- [x] **C7 lib.rs 模块声明**：`lib.rs` 包含 `pub mod registry;`
- [x] **C8 lib.rs re-export**：`lib.rs` 包含 `pub use registry::{AgentRegistry, RegistryStats};`
- [x] **C9 no_std 声明**：`lib.rs` 仍有 `#![cfg_attr(not(test), no_std)]`（未改动）
- [x] **C10 extern crate alloc**：`lib.rs` 仍有 `extern crate alloc;`（未改动）
- [x] **C11 零外部依赖**：`crates/agents/agent/Cargo.toml` 的 `[dependencies]` 仍为空或仅注释
- [x] **C12 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.34.0";`

## 三、AgentError 扩展校验

- [x] **C13 新增 AgentNotFound 变体**：`error.rs` 的 `AgentError` 枚举包含 `AgentNotFound`
- [x] **C14 新增 AlreadyRegistered 变体**：`error.rs` 的 `AgentError` 枚举包含 `AlreadyRegistered`
- [x] **C15 Display 实现**：`AgentNotFound` => `"agent not found"`；`AlreadyRegistered` => `"agent already registered"`
- [x] **C16 既有变体未改动**：InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId 4 个变体及其 Display 输出保持不变
- [x] **C17 新变体测试**：`error.rs` 的 tests 模块包含新变体的 Display 与 clone/eq 测试

## 四、AgentRegistry 结构校验

- [x] **C18 结构体定义**：`AgentRegistry { agents: BTreeMap<AgentId, AgentDescriptor>, by_type: BTreeMap<AgentType, Vec<AgentId>> }`（D1 偏差：BTreeMap）
- [x] **C19 RegistryStats 结构**：`pub struct RegistryStats { pub total: usize, pub alive: usize, pub by_type: BTreeMap<AgentType, usize> }`，derive Clone, Debug
- [x] **C20 new() 方法**：`pub fn new() -> Self` 初始化两个空 BTreeMap
- [x] **C21 register() 方法**：签名 `register(&mut self, desc: AgentDescriptor) -> Result<AgentId, AgentError>`；重复返回 `AlreadyRegistered`；成功插入主表 + 类型索引
- [x] **C22 unregister() 方法**：签名 `unregister(&mut self, id: AgentId) -> Result<(), AgentError>`；不存在返回 `AgentNotFound`；成功时同步清理类型索引
- [x] **C23 get/get_mut 方法**：按 ID 查找返回 `Option<&AgentDescriptor>` / `Option<&mut AgentDescriptor>`
- [x] **C24 find_by_type() 方法**：返回 `Vec<&AgentDescriptor>`，按 AgentId 升序
- [x] **C25 find_by_name() 方法**：返回 `Option<&AgentDescriptor>`
- [x] **C26 list_all() 方法**：返回 `Vec<&AgentDescriptor>`，按 AgentId 升序
- [x] **C27 list_alive() 方法**：返回 `Vec<&AgentDescriptor>`，仅含 `is_alive() == true` 的 Agent
- [x] **C28 count() 方法**：返回 `usize`，等于主表长度
- [x] **C29 count_by_type() 方法**：返回 `usize`，从类型索引取长度
- [x] **C30 exists() 方法**：返回 `bool`
- [x] **C31 stats() 方法**：返回 `RegistryStats`，total / alive / by_type 三个字段正确

## 五、no_std 合规校验

- [x] **C32 无 use std::**：`crates/agents/agent/src/registry.rs` 中搜索 `use std::` 返回 0 匹配
- [x] **C33 无 panic 宏违规**：`registry.rs` 非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C34 子模块无 no_std 重复**：`registry.rs` 不包含 `#![cfg_attr(not(test), no_std)]`（由 lib.rs 统一声明）
- [x] **C35 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 六、测试校验

- [x] **C36 单元测试存在**：`registry.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C37 注册/查找/枚举测试**：覆盖 register / get / find_by_type / find_by_name / list_all / list_alive
- [x] **C38 重复注册拒绝测试**：`test_register_duplicate_rejected` 验证返回 `AlreadyRegistered`
- [x] **C39 注销不存在测试**：`test_unregister_nonexistent` 验证返回 `AgentNotFound`
- [x] **C40 索引一致性测试**：`test_unregister_cleans_type_index` 验证注销后类型索引同步清理（蓝图 §8.2 / §8.4）
- [x] **C41 ID 复用测试**：`test_unregister_all_then_register` 验证注销后 ID 可被新描述符复用（蓝图 §8.5 坑点）
- [x] **C42 stats 测试**：`test_stats` 验证 total / alive / by_type 字段
- [x] **C43 list_alive 过滤测试**：`test_list_alive_filters_dead` 验证 Dead/Created 被过滤
- [x] **C44 排序测试**：`test_find_by_type_returns_sorted_by_id` / `test_list_all_sorted` 验证 BTreeMap 天然有序
- [x] **C45 集成测试存在**：`tests/registry_test.rs` 存在且通过
- [x] **C46 顺序压力测试**：`integration_stress_sequential_register` 注册 100 个 Agent 全部可查（D2 偏差：并发测试后置 v0.36.0）
- [x] **C47 测试覆盖率**：≥ 80%（蓝图 §6.1 要求）

## 七、版本标识一致性

- [x] **C48 根 Cargo.toml**：`version = "0.34.0"`
- [x] **C49 Makefile**：`VERSION := 0.34.0`
- [x] **C50 ci.yml**：`Version: v0.34.0`
- [x] **C51 gate.rs**：注释含 v0.34.0
- [x] **C52 lib.rs VERSION**：`VERSION = "0.34.0"`
- [x] **C53 无 0.33.0 残留**：`grep -r "0.33.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（v0.33.0 spec 历史文档除外）

## 八、构建与质量校验

- [x] **C54 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C55 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C56 cargo test (agent)**：`cargo test -p eneros-agent` 全部通过（含新增单元 + 集成测试 + v0.33.0 既有测试）
- [x] **C57 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿（v0.31.0/v0.32.0/v0.33.0 测试不受影响）
- [x] **C58 eneros-ci**：`cargo run -p eneros-ci` Overall: PASS（fmt/clippy/test 全通过；audit 步骤因 GitHub 网络不可达无法拉取 RustSec advisory DB 而失败 — 已知环境问题，与 v0.31.0 相同，非代码缺陷）
- [x] **C59 cargo deny**：`cargo deny check licenses bans sources` 通过

## 九、文档校验

- [x] **C60 设计文档存在**：`docs/agents/agent-registry-design.md` 存在
- [x] **C61 文档内容完整**：包含版本目标 / 数据结构 / 双索引设计 / 接口清单 / 偏差声明 D1~D3 / 性能分析 / 并发设计 / 索引一致性 / ID 复用 / 后续解锁
- [x] **C62 文档位置正确**：在 `docs/agents/` 子目录下，不在 `docs/` 根

## 十、偏差声明记录

- [x] **C63 D1 偏差记录**：使用 BTreeMap 而非 HashMap（保持零外部依赖不变量），文档记录理由与代价
- [x] **C64 D2 偏差记录**：注册表无内部锁（plain &mut self），并发同步后置 v0.36.0，文档记录
- [x] **C65 D3 偏差记录**：新增 AlreadyRegistered 而非复用 DuplicateId（语义不同 + 蓝图要求），文档记录

## 十一、蓝图合规校验

- [x] **C66 接口完备性**：蓝图 §3 接口定义的所有方法（new/register/unregister/get/get_mut/find_by_type/find_by_name/list_all/list_alive/count/count_by_type）全部实现
- [x] **C67 蓝图 §4.2 扩展接口**：stats() / exists() 已实现
- [x] **C68 蓝图 §4.4 错误处理**：AgentNotFound / AlreadyRegistered 已新增
- [x] **C69 蓝图 §6.3 性能**：查找延迟 <1μs（BTreeMap n=100 时满足，文档分析）
- [x] **C70 蓝图 §8.2 索引清理**：unregister 同步清理类型索引（测试覆盖）
- [x] **C71 蓝图 §8.4 索引一致性**：主表与类型索引保持一致（测试覆盖）
- [x] **C72 蓝图 §8.5 ID 复用**：注销后 ID 可被新描述符复用（测试覆盖）
