# Tasks — v0.34.0 Agent 注册表与发现

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 错误扩展 → Wave 2 核心实现 → Wave 3 测试 → Wave 4 文档+版本 → Wave 5 验证
> **目标驱动**：每个任务附验证条件，可独立 loop 直到通过。

## Wave 1: 错误类型扩展（前置）

- [x] **Task 1: 扩展 AgentError 两个变体**
  - 修改 `crates/agents/agent/src/error.rs`：
    - 追加变体 `AgentNotFound`（注释：注册表中未找到 Agent）
    - 追加变体 `AlreadyRegistered`（注释：Agent ID 已在注册表中）
    - 在 `Display` impl 的 match 中追加两条分支：
      - `AgentNotFound` => `"agent not found"`
      - `AlreadyRegistered` => `"agent already registered"`
    - 在 `tests` 模块追加测试 `test_new_error_variants_display`：验证两个新变体的 Display 输出
    - 在 `test_error_clone_eq` 中追加对新变体的 clone/eq 断言
  - **不修改**既有 4 个变体（InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId）
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo test -p eneros-agent` 全部通过（含 v0.33.0 既有测试）

## Wave 2: 核心实现

- [x] **Task 2: 实现 registry.rs — AgentRegistry 结构与基础方法**
  - 创建 `crates/agents/agent/src/registry.rs`：
    - 头部模块文档注释（说明：Agent 注册表 / 双索引设计 / D1 BTreeMap 偏差 / D2 无锁偏差）
    - `use alloc::collections::BTreeMap;` / `use alloc::vec::Vec;`
    - `use crate::{AgentDescriptor, AgentId, AgentType, AgentError};`
    - `pub struct AgentRegistry { agents: BTreeMap<AgentId, AgentDescriptor>, by_type: BTreeMap<AgentType, Vec<AgentId>> }`
    - `impl AgentRegistry`：
      - `pub fn new() -> Self`：初始化两个空 BTreeMap
      - `pub fn register(&mut self, desc: AgentDescriptor) -> Result<AgentId, AgentError>`：ID 已存在返回 `AlreadyRegistered`；否则插入主表 + 追加类型索引，返回 Ok(id)
      - `pub fn unregister(&mut self, id: AgentId) -> Result<(), AgentError>`：不存在返回 `AgentNotFound`；否则从主表移除 + 从类型索引 `retain` 过滤；返回 Ok(())
      - `pub fn get(&self, id: AgentId) -> Option<&AgentDescriptor>`
      - `pub fn get_mut(&mut self, id: AgentId) -> Option<&mut AgentDescriptor>`
      - `pub fn exists(&self, id: AgentId) -> bool`：`self.agents.contains_key(&id)`
  - **验证**：`cargo build -p eneros-agent` 编译通过

- [x] **Task 3: 实现 registry.rs — 查找/枚举/统计方法**
  - 在 `impl AgentRegistry` 追加：
    - `pub fn find_by_type(&self, agent_type: AgentType) -> Vec<&AgentDescriptor>`：从 by_type 索引取 ID 列表，filter_map 从主表取引用（顺序按 AgentId 升序，因 BTreeMap 主表天然有序）
    - `pub fn find_by_name(&self, name: &str) -> Option<&AgentDescriptor>`：`agents.values().find(|a| a.name == name)`
    - `pub fn list_all(&self) -> Vec<&AgentDescriptor>`：`agents.values().collect()`（按 AgentId 升序）
    - `pub fn list_alive(&self) -> Vec<&AgentDescriptor>`：`agents.values().filter(|a| a.is_alive()).collect()`
    - `pub fn count(&self) -> usize`：`self.agents.len()`
    - `pub fn count_by_type(&self, agent_type: AgentType) -> usize`：`self.by_type.get(&agent_type).map(|v| v.len()).unwrap_or(0)`
    - `pub fn stats(&self) -> RegistryStats`：遍历构造统计
  - `pub struct RegistryStats { pub total: usize, pub alive: usize, pub by_type: BTreeMap<AgentType, usize> }`，derive Clone, Debug
  - **验证**：`cargo build -p eneros-agent` 编译通过

- [x] **Task 4: 更新 lib.rs — 模块声明与 re-export**
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在模块声明区追加 `pub mod registry;`
    - 在 re-export 区追加 `pub use registry::{AgentRegistry, RegistryStats};`
    - 更新 `VERSION` 常量：`pub const VERSION: &str = "0.34.0";`
    - 更新文件头部文档注释：版本号 0.33.0 → 0.34.0，追加 registry 模块说明
  - **验证**：`cargo build -p eneros-agent` 编译通过；`cargo doc -p eneros-agent` 无警告

## Wave 3: 测试

- [x] **Task 5: 编写 registry.rs 单元测试**
  - 在 `registry.rs` 末尾追加 `#[cfg(test)] mod tests`：
    - `test_register_and_get`：注册一个 Agent，get 返回 Some
    - `test_register_duplicate_rejected`：注册同一 ID 两次，第二次返回 `AlreadyRegistered`
    - `test_unregister_existing`：注册后注销，get 返回 None
    - `test_unregister_nonexistent`：注销不存在的 ID 返回 `AgentNotFound`
    - `test_unregister_cleans_type_index`：注册同类型 2 个 Agent，注销 1 个后 `count_by_type` = 1 且 `find_by_type` 仅返回剩余 1 个（蓝图 §8.2 / §8.4 索引一致性）
    - `test_find_by_type_returns_sorted_by_id`：注册同类型 3 个 Agent（ID 递增），`find_by_type` 返回顺序与 ID 升序一致
    - `test_find_by_type_empty`：查找不存在的类型返回空 Vec
    - `test_find_by_name`：注册带名称的 Agent，`find_by_name` 返回 Some
    - `test_find_by_name_not_found`：查找不存在的名称返回 None
    - `test_list_all_sorted`：注册 3 个 Agent，`list_all` 长度 = 3 且按 AgentId 升序
    - `test_list_alive_filters_dead`：构造 3 个 Agent（1 个 Dead、1 个 Created、1 个 Running），`list_alive` 仅返回 Running 那个
    - `test_count_and_count_by_type`：注册 3 个（2 个 Energy + 1 个 Device），count=3, count_by_type(Energy)=2, count_by_type(Device)=1, count_by_type(Market)=0
    - `test_exists`：注册后 exists(id)=true，注销后 exists(id)=false
    - `test_stats`：注册 3 个（2 Energy + 1 Device，其中 1 Energy 为 Dead），stats.total=3, stats.alive=2, stats.by_type[Energy]=2, stats.by_type[Device]=1
    - `test_get_mut_updates_descriptor`：get_mut 修改 state 后 get 反映新状态
    - `test_empty_registry`：new() 后 count=0, list_all 为空, stats.total=0
    - `test_register_multiple_types`：注册 4 种不同类型的 Agent，find_by_type 各返回 1 个
    - `test_unregister_all_then_register`：注册→注销→再注册同 ID（新描述符），第二次注册成功（验证注销后 ID 可复用，蓝图 §8.5 坑点已处理）
  - 覆盖率目标 ≥ 80%
  - **验证**：`cargo test -p eneros-agent` 全部通过

- [x] **Task 6: 编写集成测试 tests/registry_test.rs**
  - 创建 `crates/agents/agent/tests/registry_test.rs`：
    - `integration_register_workflow`：完整流程 — 创建 5 个不同类型 Agent → 注册全部 → 按 ID/类型/名称查找 → 枚举 → 注销部分 → 验证状态
    - `integration_stress_sequential_register`：顺序注册 100 个 Agent，验证 count=100 且所有 ID 可查（蓝图 §6.2 顺序压力测试，D2 偏差：并发测试后置）
    - `integration_type_index_consistency_after_unregisters`：注册 10 个同类型 Agent，交替注销偶数 ID，验证 `find_by_type` 仅返回奇数 ID 且 `count_by_type` = 5
    - `integration_mixed_types_stats`：注册 System/Energy/Market/Grid/Device 各 1 个，验证 stats.by_type 包含 5 种类型且各 = 1
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 4: 文档与版本标识

- [x] **Task 7: 编写设计文档**
  - 创建 `docs/agents/agent-registry-design.md`：
    - 版本目标 / 架构定位 / 前置依赖
    - 数据结构设计（AgentRegistry / RegistryStats）
    - 双索引设计（主表 BTreeMap + 类型索引 BTreeMap<Vec>）
    - 接口清单（所有 pub 方法）
    - 偏差声明 D1（BTreeMap vs HashMap）/ D2（无内部锁）/ D3（AlreadyRegistered vs DuplicateId）
    - 性能分析（n=100 时 BTreeMap 查找 < 1μs，满足蓝图 §6.3）
    - 并发设计说明（注册表为纯数据结构，同步由更高层负责，后置 v0.36.0）
    - 索引一致性保证（注销时同步清理类型索引）
    - ID 复用说明（注销后 ID 可被新描述符复用）
    - 后续解锁版本（v0.35.0 / v0.36.0 / Agent 间通信）
  - **验证**：文档存在且内容完整

- [x] **Task 8: 同步版本标识**
  - 根 `Cargo.toml`：`version = "0.34.0"`（workspace 版本）
  - `Makefile`：`VERSION := 0.34.0` + header 注释更新 + agent-build 目标描述更新为 "v0.34.0 registry"
  - `.github/workflows/ci.yml`：`Version: v0.34.0`
  - `ci/src/gate.rs`：注释更新为 v0.34.0（保持 eneros-agent 在排除列表说明）
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.34.0"`（Task 4 已完成）
  - **验证**：`grep -r "0.33.0" crates/agents/ Makefile .github/ ci/` 无版本标识残留（历史注释与 v0.33.0 spec 文档除外）

## Wave 5: 构建验证

- [x] **Task 9: 全量构建与质量验证** (C58 audit step failed due to GitHub network unreachability — known environment issue, same as v0.31.0; fmt/clippy/test/aarch64/deny all PASS)
  - `cargo fmt --all -- --check`
  - `cargo clippy -p eneros-agent --all-targets -- -D warnings`
  - `cargo test -p eneros-agent`（含新增单元 + 集成测试）
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（回归：v0.31.0/v0.32.0/v0.33.0 全绿）
  - `cargo run -p eneros-ci`（Overall: PASS）
  - WSL2: `cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（no_std 交叉编译）
  - `cargo deny check licenses bans sources`
  - **验证**：全部 PASS

## Task Dependencies

- Task 1: 无依赖（错误类型扩展先行，registry.rs 依赖新变体）
- Task 2: 依赖 Task 1（需要 `AlreadyRegistered` / `AgentNotFound`）
- Task 3: 依赖 Task 2（同一 impl 块）
- Task 4: 依赖 Task 3（lib.rs 声明模块前 registry.rs 应完整）
- Task 5: 依赖 Task 4（单元测试需要模块可被 crate 引用）
- Task 6: 依赖 Task 5（集成测试在单元测试验证基础功能后）
- Task 7-8: 依赖 Task 4（可并行，文档与版本标识独立）
- Task 9: 依赖 Task 1-8 全部完成
