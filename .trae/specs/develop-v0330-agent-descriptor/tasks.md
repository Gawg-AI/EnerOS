# Tasks — v0.33.0 Agent 抽象与描述符

> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **任务分波**：Wave 1 骨架 → Wave 2 核心实现 → Wave 3 测试 → Wave 4 文档+版本 → Wave 5 验证

## Wave 1: Crate 骨架

- [x] **Task 1: 创建 crate 骨架**
  - 创建目录 `crates/agents/agent/`
  - 创建 `Cargo.toml`（name = "eneros-agent", version.workspace = true, 零依赖）
  - 创建 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + VERSION 常量
  - 创建空模块文件：`src/types.rs` / `src/id.rs` / `src/descriptor.rs` / `src/error.rs`
  - 修改根 `Cargo.toml`：members 添加 `"crates/agents/agent"`，version → "0.33.0"
  - **验证**：`cargo metadata --format-version 1 > /dev/null` 成功

## Wave 2: 核心实现（并行执行）

- [x] **Task 2: 实现 types.rs — 枚举与辅助类型**
  - `AgentType` 枚举：System / Device / Market / Grid / Energy / Twin / EdgeCoord / CloudCoord / Custom(u16)
  - `AgentState` 枚举：Created / Ready / Running / Suspended / Error / Recovering / Dead
  - `TrustLevel` 枚举：Untrusted / Verified / Trusted / System（derive PartialOrd, Ord）
  - `CapabilityRef` 结构体：cap_id: u64 / granted_at: u64 / expires_at: Option<u64>
  - `AgentMetadata` 结构体：name / version / author / description / entry_point / required_capabilities: Vec<String>
  - 所有类型 derive Clone, Debug, PartialEq, Eq（AgentType/AgentState/TrustLevel 额外 derive Copy, Hash）
  - **验证**：`cargo build -p eneros-agent` 编译通过

- [x] **Task 3: 实现 id.rs — AgentId 唯一生成**
  - `AgentId(pub u128)` 结构体，derive Clone, Copy, Debug, PartialEq, Eq, Hash
  - `AgentId::generate() -> Self`：基于 `AtomicU64` 双字模拟 AtomicU128（或直接用 AtomicU64 计数器 + epoch 前缀）
  - ID 从 1 开始递增，保证非零
  - `AgentId::ZERO` 常量（表示无效 ID）
  - **验证**：单元测试连续生成 100 个 ID 全部唯一

- [x] **Task 4: 实现 error.rs — AgentError 错误枚举**
  - `AgentError` 枚举：InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId
  - 实现 `core::fmt::Display` 和 `core::fmt::Debug`
  - **验证**：`cargo build -p eneros-agent` 编译通过

- [x] **Task 5: 实现 descriptor.rs — AgentDescriptor 核心**
  - `AgentDescriptor` 结构体（13 字段，按蓝图定义）
  - `new(agent_type: AgentType, name: &str, now: u64) -> Self`：
    - 调用 `AgentId::generate()` 生成 ID
    - 按 agent_type 映射 priority / mem_quota / cpu_quota / trust_level（蓝图 §4.5 的 match 表）
    - state = Created, capabilities = Vec::new(), parent = None
    - created_at = now, restart_count = 0, last_heartbeat = 0
  - `is_alive(&self) -> bool`：state 不是 Dead 也不是 Created
  - `can_access(&self, _resource: &str) -> bool`：trust_level >= TrustLevel::Verified（D3 决策）
  - `check_quota(&self, mem: usize, cpu: u8) -> bool`：mem <= mem_quota && cpu <= cpu_quota
  - **验证**：`cargo build -p eneros-agent` 编译通过

## Wave 3: 测试

- [x] **Task 6: 编写单元测试**
  - types.rs 测试：枚举变体完备性 / Custom 值唯一性 / TrustLevel 排序
  - id.rs 测试：generate() 唯一性（100 次）/ 非零性 / ZERO 常量
  - descriptor.rs 测试：
    - new() 默认值验证（所有 5 种主要类型：System/Energy/Market/Grid/Device）
    - is_alive() 对所有 7 种状态的返回值
    - check_quota() 边界（刚好等于 / 超过 / 零配额）
    - can_access() 对 4 种 TrustLevel 的返回值
  - **验证**：`cargo test -p eneros-agent` 全部通过，覆盖率 ≥ 80%

- [x] **Task 7: 编写集成测试**
  - 创建 `tests/descriptor_test.rs`
  - 测试多 Agent 创建场景（ID 互不冲突）
  - 测试 AgentMetadata 构造
  - 测试 CapabilityRef 过期检查逻辑
  - **验证**：`cargo test -p eneros-agent` 集成测试通过

## Wave 4: 文档与版本标识

- [x] **Task 8: 编写设计文档**
  - 创建 `docs/agents/agent-descriptor-design.md`
  - 内容：版本目标 / 数据结构设计 / 类型映射表 / ID 生成策略 / 安全设计（信任等级）/ 偏差声明（D1~D3）
  - 更新 `docs/agents/` 下 README（如有需要）
  - **验证**：文档存在且内容完整

- [x] **Task 9: 更新版本标识**
  - 根 `Cargo.toml`：version = "0.33.0"
  - `crates/agents/agent/Cargo.toml`：version.workspace = true（已在 Task 1）
  - `Makefile`：VERSION := 0.33.0 + header 注释 + agent-build 目标描述
  - `.github/workflows/ci.yml`：Version: v0.33.0
  - `ci/src/gate.rs`：注释更新为 v0.33.0（含 eneros-agent 在排除列表说明）
  - `src/lib.rs`：VERSION = "0.33.0"
  - **验证**：grep "0.32.0" 无残留（除历史注释）

## Wave 5: 构建验证

- [x] **Task 10: 全量构建验证**
  - `cargo fmt --all -- --check`
  - `cargo clippy -p eneros-agent --all-targets -- -D warnings`
  - `cargo test -p eneros-agent`
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
  - `cargo run -p eneros-ci`（Overall: PASS）
  - WSL2: `cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - `cargo deny check licenses bans sources`
  - **验证**：全部 PASS

## Task Dependencies

- Task 1: 无依赖（骨架先行）
- Task 2-4: 依赖 Task 1（可并行）
- Task 5: 依赖 Task 2 + Task 3 + Task 4（需要类型和 ID）
- Task 6: 依赖 Task 5（需要完整实现）
- Task 7: 依赖 Task 6（需要单元测试验证基础功能）
- Task 8-9: 依赖 Task 5（可并行，文档和版本标识独立）
- Task 10: 依赖 Task 1-9 全部完成
