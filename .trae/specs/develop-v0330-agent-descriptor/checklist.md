# Checklist — v0.33.0 Agent 抽象与描述符

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0 的 249 tests + v0.32.0 的 402 tests）必须全部继续通过。

## 一、目录结构校验

- [x] **C1 crate 位置**：`crates/agents/agent/` 存在，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 包含 `"crates/agents/agent"`
- [x] **C3 文档分类**：`docs/agents/agent-descriptor-design.md` 在 `docs/agents/` 子目录下
- [x] **C4 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 二、代码结构校验

- [x] **C5 模块文件完整**：`lib.rs` / `types.rs` / `id.rs` / `descriptor.rs` / `error.rs` 全部存在
- [x] **C6 no_std 声明**：`lib.rs` 有 `#![cfg_attr(not(test), no_std)]`
- [x] **C7 extern crate alloc**：`lib.rs` 有 `extern crate alloc;`
- [x] **C8 零外部依赖**：`Cargo.toml` 的 `[dependencies]` 为空或仅注释
- [x] **C9 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.33.0";`

## 三、类型定义校验

- [x] **C10 AgentType 9 种变体**：System / Device / Market / Grid / Energy / Twin / EdgeCoord / CloudCoord / Custom(u16)
- [x] **C11 AgentState 7 种状态**：Created / Ready / Running / Suspended / Error / Recovering / Dead
- [x] **C12 TrustLevel 4 级**：Untrusted / Verified / Trusted / System（derive PartialOrd, Ord）
- [x] **C13 AgentId 结构**：`pub struct AgentId(pub u128)`，derive Clone, Copy, Debug, PartialEq, Eq, Hash
- [x] **C14 CapabilityRef 结构**：cap_id: u64 / granted_at: u64 / expires_at: Option<u64>
- [x] **C15 AgentMetadata 结构**：name / version / author / description / entry_point / required_capabilities: Vec<String>
- [x] **C16 AgentDescriptor 13 字段**：agent_id / agent_type / name / state / priority / mem_quota / cpu_quota / trust_level / capabilities / parent / created_at / restart_count / last_heartbeat
- [x] **C17 AgentError 4 变体**：InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId

## 四、方法实现校验

- [x] **C18 new() 签名**：`new(agent_type: AgentType, name: &str, now: u64) -> Self`（D2 偏差：接受 now 参数）
- [x] **C19 new() 类型映射**：System→priority=255/mem=256MB/cpu=30/trust=System；Energy→200/128MB/25/Trusted；Market/Grid→150/16MB/10/Trusted；Device→100/32MB/10/Trusted；其他→50/16MB/10/Verified
- [x] **C20 new() 默认值**：state=Created, capabilities=Vec::new(), parent=None, created_at=now, restart_count=0, last_heartbeat=0
- [x] **C21 is_alive()**：Running/Ready/Suspended/Error/Recovering → true；Created/Dead → false
- [x] **C22 can_access()**：trust_level >= Verified → true；Untrusted → false（D3 决策）
- [x] **C23 check_quota()**：mem <= mem_quota && cpu <= cpu_quota
- [x] **C24 AgentId::generate()**：基于原子计数器，从 1 开始，非零，全局唯一

## 五、测试校验

- [x] **C25 单元测试存在**：types.rs / id.rs / descriptor.rs 各有 `#[cfg(test)] mod tests`
- [x] **C26 枚举完备性测试**：AgentType 9 变体 / AgentState 7 状态 / TrustLevel 4 级
- [x] **C27 ID 唯一性测试**：连续 generate() 100 次全部不同
- [x] **C28 new() 默认值测试**：5 种主要类型（System/Energy/Market/Grid/Device）的字段映射
- [x] **C29 is_alive() 测试**：7 种状态的返回值
- [x] **C30 check_quota() 边界测试**：刚好等于 / 超过 / 零请求
- [x] **C31 can_access() 测试**：4 种 TrustLevel 的返回值
- [x] **C32 集成测试存在**：`tests/descriptor_test.rs` 存在且通过
- [x] **C33 测试覆盖率**：≥ 80%（蓝图 §6.1 要求）

## 六、no_std 合规校验

- [x] **C34 无 use std::**：`crates/agents/agent/src/` 下搜索 `use std::` 返回 0 匹配
- [x] **C35 无 panic 宏违规**：非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C36 仅 lib.rs 有 no_std**：子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] **C37 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 七、版本标识一致性

- [x] **C38 根 Cargo.toml**：version = "0.33.0"
- [x] **C39 Makefile**：VERSION := 0.33.0
- [x] **C40 ci.yml**：Version: v0.33.0
- [x] **C41 gate.rs**：注释含 v0.33.0
- [x] **C42 lib.rs VERSION**：VERSION = "0.33.0"
- [x] **C43 无 0.32.0 残留**：grep "0.32.0" 无版本标识残留（历史注释除外）

## 八、构建与质量校验

- [x] **C44 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C45 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C46 cargo test**：`cargo test -p eneros-agent` 全部通过
- [x] **C47 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C48 eneros-ci**：`cargo run -p eneros-ci` Overall: PASS
- [x] **C49 cargo deny**：`cargo deny check licenses bans sources` 通过

## 九、文档校验

- [x] **C50 设计文档存在**：`docs/agents/agent-descriptor-design.md` 存在
- [x] **C51 文档内容完整**：包含版本目标 / 数据结构 / 类型映射 / ID 策略 / 偏差声明
- [x] **C52 文档位置正确**：在 `docs/agents/` 子目录下，不在 `docs/` 根

## 十、偏差声明记录

- [x] **C53 D1 偏差记录**：ID 生成使用原子计数器而非加密 RNG（文档记录）
- [x] **C54 D2 偏差记录**：`new()` 接受 `now: u64` 参数而非内部 `time::now()`（文档记录）
- [x] **C55 D3 偏差记录**：`can_access()` 基于信任等级阈值而非能力系统（文档记录，v0.39.0 替换）
