# v0.33.0 — Agent 抽象与描述符 Spec

## Why

EnerOS 的核心概念是 Agent——能源场景下的自治调度实体。v0.33.0 定义 Agent 描述符（`AgentDescriptor`）作为 Agent Runtime 子系统的基础数据结构，建立 Agent 作为 OS 第一公民的内核级抽象。此版本解锁整个 P1-D Agent Runtime（v0.34.0 注册表 / v0.35.0 生命周期 / v0.36.0~v0.40.0 后续版本）。

## What Changes

- **新增 crate** `eneros-agent`（`crates/agents/agent/`），建立 `agents` 子系统目录
- 定义 `AgentDescriptor` 结构体：ID / 类型 / 名称 / 状态 / 优先级 / 内存配额 / CPU 配额 / 信任等级 / 能力列表 / 父 Agent / 创建时间 / 重启次数 / 最后心跳
- 定义 `AgentType` 枚举（System/Device/Market/Grid/Energy/Twin/EdgeCoord/CloudCoord/Custom(u16)）
- 定义 `AgentState` 枚举（Created/Ready/Running/Suspended/Error/Recovering/Dead）
- 定义 `TrustLevel` 枚举（Untrusted/Verified/Trusted/System）
- 定义 `AgentId(pub u128)` + 基于原子计数器的唯一 ID 生成
- 定义 `CapabilityRef` 结构体（cap_id/granted_at/expires_at）
- 定义 `AgentMetadata` 结构体（name/version/author/description/entry_point/required_capabilities）
- 定义 `AgentError` 错误枚举（InvalidDescriptor/QuotaExceeded/InvalidTrustLevel/DuplicateId）
- 实现 `AgentDescriptor` 方法：`new` / `is_alive` / `can_access` / `check_quota`
- 新增文档 `docs/agents/agent-descriptor-design.md`
- 版本标识更新：Cargo.toml / Makefile / ci.yml / gate.rs → 0.33.0

## Impact

- **Affected specs**: 无（新建 crate，不修改已有功能）
- **Affected code**:
  - 新增 `crates/agents/agent/`（整个 crate 新建）
  - 修改 `Cargo.toml`（workspace members 添加 `crates/agents/agent`，version → 0.33.0）
  - 修改 `Makefile`（VERSION → 0.33.0）
  - 修改 `.github/workflows/ci.yml`（Version → v0.33.0）
  - 修改 `ci/src/gate.rs`（注释更新为 v0.33.0）
  - 新增 `docs/agents/agent-descriptor-design.md`

## 设计决策

### D1: ID 生成策略 — 原子计数器（非加密 RNG）

**决策**：`AgentId::generate()` 使用 `AtomicU128` 计数器，从 1 开始递增。

**理由**：
- 蓝图依赖表仅列出 v0.22.0 + v0.11.0，未列出 v0.31.0（国密），说明不应依赖 eneros-crypto
- Agent ID 只需全局唯一，不需密码学随机性
- 原子计数器是最简单的唯一 ID 方案，符合 Simplicity First 原则
- `AtomicU128` 在 aarch64 上可用 `core::sync::atomic::AtomicU64` 双字模拟或直接使用（nightly 支持）

### D2: 时间戳处理 — `new()` 接受 `now: u64` 参数

**决策**：`AgentDescriptor::new(agent_type, name, now: u64)` 接受外部时间戳。

**偏差声明**：蓝图原签名为 `new(agent_type, name)`，内部调用 `crate::time::now()`。本实现偏差原因：
- 蓝图依赖表未列出 v0.12.0（RTC/系统时钟），不应依赖 eneros-time
- eneros-crypto PKI 模块已建立"接受 `now: u64` 参数"的项目惯例
- 避免引入 placeholder/stub 的 `time::now()` 模块（返回 0 毫无意义）
- 显式参数比隐式全局状态更清晰、更易测试

### D3: `can_access` 实现 — 信任等级阈值检查

**决策**：`can_access(resource)` 基于 `trust_level >= TrustLevel::Verified` 返回 bool。

**理由**：
- 能力系统（v0.39.0）尚未实现，无法做真正的 capability-based access control
- 蓝图未定义 `resource` 参数的语义和格式
- 信任等级分级是蓝图 §9.3 明确要求的安全维度
- v0.39.0 实现后将替换为 capability-based 检查

### D4: crate 位置 — `crates/agents/agent/`

**决策**：新建 `crates/agents/` 子系统，crate 放在 `crates/agents/agent/`。

**理由**：
- 蓝图 §2.3.2 标注 `crates/agents/` 为 "Agent 实现（Phase 2+）"，但 v0.33.0 是 Agent Runtime 框架本身（Phase 1），是 agents 子系统的基础
- `agents/agent/` 为后续 Agent 实现（Energy Agent / Market Agent）预留命名空间
- 符合 §2.3.1 "所有 crate 必须放入 `crates/<subsystem>/`" 规则

## ADDED Requirements

### Requirement: AgentType 枚举

系统 SHALL 提供 `AgentType` 枚举，支持 9 种类型：System / Device / Market / Grid / Energy / Twin / EdgeCoord / CloudCoord / Custom(u16)。

#### Scenario: Custom 类型扩展
- **WHEN** 第三方 Agent 使用 `AgentType::Custom(42)`
- **THEN** 该值与所有预定义类型不等，且可被 `match` 正确识别

### Requirement: AgentState 生命周期状态

系统 SHALL 提供 `AgentState` 枚举，包含 7 种状态：Created / Ready / Running / Suspended / Error / Recovering / Dead。

#### Scenario: 状态枚举完备
- **WHEN** 遍历所有状态变体
- **THEN** 共 7 种，覆盖 Agent 从创建到销毁的完整生命周期

### Requirement: TrustLevel 信任等级

系统 SHALL 提供 `TrustLevel` 枚举，支持 4 级信任：Untrusted < Verified < Trusted < System。

#### Scenario: 信任等级排序
- **WHEN** 比较 `TrustLevel::System > TrustLevel::Trusted`
- **THEN** 结果为 true（System 是最高权限）

### Requirement: AgentDescriptor 描述符

系统 SHALL 提供 `AgentDescriptor` 结构体，包含 13 个字段：agent_id / agent_type / name / state / priority / mem_quota / cpu_quota / trust_level / capabilities / parent / created_at / restart_count / last_heartbeat。

#### Scenario: 默认构造
- **WHEN** 调用 `AgentDescriptor::new(AgentType::System, "sys-agent", 1000)`
- **THEN** agent_id 为非零唯一值，state = Created，priority = 255，mem_quota = 256MB，cpu_quota = 30，trust_level = System，capabilities 为空，parent = None，created_at = 1000，restart_count = 0，last_heartbeat = 0

#### Scenario: 类型到配额映射
- **WHEN** 创建 `AgentType::Energy` 类型的描述符
- **THEN** priority = 200，mem_quota = 128MB，cpu_quota = 25，trust_level = Trusted

#### Scenario: 存活判断
- **WHEN** Agent 处于 Running 状态
- **THEN** `is_alive()` 返回 true
- **WHEN** Agent 处于 Dead 或 Created 状态
- **THEN** `is_alive()` 返回 false

#### Scenario: 配额检查
- **WHEN** 请求内存 64MB + CPU 20%，Agent 配额为 128MB / 25%
- **THEN** `check_quota(64*1024*1024, 20)` 返回 true
- **WHEN** 请求内存 256MB 超过配额 128MB
- **THEN** `check_quota(256*1024*1024, 10)` 返回 false

### Requirement: AgentId 唯一生成

系统 SHALL 提供 `AgentId(pub u128)` 类型和 `AgentId::generate()` 方法，基于原子计数器生成全局唯一 ID。

#### Scenario: ID 唯一性
- **WHEN** 连续调用 `AgentId::generate()` 100 次
- **THEN** 生成 100 个互不相同的 ID

### Requirement: AgentError 错误类型

系统 SHALL 提供 `AgentError` 枚举，包含 4 种错误：InvalidDescriptor / QuotaExceeded / InvalidTrustLevel / DuplicateId。

### Requirement: no_std 合规

系统 SHALL 在 `lib.rs` 声明 `#![cfg_attr(not(test), no_std)]`，所有代码使用 `alloc::*` / `core::*`，禁止 `use std::*`。

#### Scenario: no_std 编译
- **WHEN** 执行 `cargo build -p eneros-agent --target aarch64-unknown-none`
- **THEN** 编译成功，无 std 依赖

### Requirement: 零外部依赖

系统 SHALL 不依赖任何外部 crate（仅依赖 `alloc` / `core`）。

#### Scenario: cargo deny 通过
- **WHEN** 运行 `cargo deny check`
- **THEN** 无外部依赖违规
