# EnerOS Agent 描述符设计 (v0.33.0)

> 版本：v0.33.0 — Agent 抽象与描述符
> crate：`eneros-agent`（`crates/agents/agent/`）
> 依赖：零外部依赖（仅 `alloc` / `core`），no_std

---

## 1. 版本目标

定义 `AgentDescriptor` 数据结构，将 Agent 建立为 EnerOS 的一等操作系统公民，为后续 Agent 注册表、生命周期管理、心跳与能力管理提供基础。

---

## 2. 数据结构设计

### 2.1 AgentDescriptor（13 字段）

| 字段 | 类型 | 说明 |
|------|------|------|
| `agent_id` | `AgentId` (u128) | 唯一标识符 |
| `agent_type` | `AgentType` | Agent 类型 |
| `name` | `String` | 名称 |
| `state` | `AgentState` | 生命周期状态 |
| `priority` | `u8` | 优先级（0~255，越大越高） |
| `mem_quota` | `usize` | 内存配额（字节） |
| `cpu_quota` | `u8` | CPU 配额（百分比） |
| `trust_level` | `TrustLevel` | 信任等级 |
| `capabilities` | `Vec<CapabilityRef>` | 已授予能力列表 |
| `parent` | `Option<AgentId>` | 父 Agent（None 表示顶层） |
| `created_at` | `u64` | 创建时间戳（外部提供） |
| `restart_count` | `u32` | 重启次数 |
| `last_heartbeat` | `u64` | 最后心跳时间戳 |

### 2.2 AgentType（9 种 + Custom 扩展）

`System` / `Device` / `Market` / `Grid` / `Energy` / `Twin` / `EdgeCoord` / `CloudCoord` / `Custom(u16)`

### 2.3 AgentState（7 种）

`Created` → `Ready` → `Running` ⇄ `Suspended`；异常进入 `Error` / `Recovering`；终态 `Dead`。

### 2.4 TrustLevel（4 级，全序）

`Untrusted < Verified < Trusted < System`

---

## 3. 类型映射表

`AgentType` → 默认 `(priority, mem_quota, cpu_quota, trust_level)`：

| AgentType | priority | mem_quota | cpu_quota | trust_level |
|-----------|----------|-----------|-----------|-------------|
| `System` | 255 | 256 MB | 30% | `System` |
| `Energy` | 200 | 128 MB | 25% | `Trusted` |
| `Market` | 150 | 16 MB | 10% | `Trusted` |
| `Grid` | 150 | 16 MB | 10% | `Trusted` |
| `Device` | 100 | 32 MB | 10% | `Trusted` |
| `Twin` | 50 | 16 MB | 10% | `Verified` |
| `EdgeCoord` | 50 | 16 MB | 10% | `Verified` |
| `CloudCoord` | 50 | 16 MB | 10% | `Verified` |
| `Custom(_)` | 50 | 16 MB | 10% | `Verified` |

映射规则：`System` 为最高优先级与系统信任；`Device | Market | Grid | Energy` 受信任；其余类型（含 `Custom`）为已验证等级。

---

## 4. ID 生成策略

- 使用 `core::sync::atomic::AtomicU64` 全局计数器，从 `1` 开始递增。
- `AgentId(u128)`：上 64 位为 epoch（预留，当前为 `0`），下 64 位为计数器值。
- `AgentId::ZERO` 常量表示无效 ID（全零）。
- 采用 `Ordering::Relaxed`，单节点内递增即可保证唯一性。

---

## 5. 安全设计

### 5.1 信任等级分级

4 级全序分级（`Untrusted < Verified < Trusted < System`），用于资源访问控制与能力授权前置校验。

### 5.2 can_access 访问控制

当前 `can_access(resource)` 基于 `trust_level >= Verified` 的阈值判定。低于 `Verified` 的 Agent 无法访问任何资源。

### 5.3 配额校验

`check_quota(mem, cpu)` 校验请求的内存与 CPU 是否在配额范围内，作为资源分配的前置闸门，防止越权占用。

---

## 6. 偏差声明

| 编号 | 偏差 | 说明 |
|------|------|------|
| D1 | 原子计数器 ID | 当前使用单节点 `AtomicU64` 递增计数器；跨节点唯一性需叠加节点 ID 编码，后续版本按需引入。 |
| D2 | `now` 参数 | no_std 无系统时钟，时间戳 `now: u64` 由外部提供（遵循 no_std 惯例）。 |
| D3 | `can_access` 信任等级阈值 | 当前基于 `TrustLevel` 阈值（`>= Verified`）；v0.39.0 能力系统实现后将替换为 capability-based 检查。 |
