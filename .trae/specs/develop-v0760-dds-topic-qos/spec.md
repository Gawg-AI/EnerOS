# v0.76.0 DDS Topic 设计与 QoS 策略 Spec

> **Skill**: andrej-karpathy-skills-main
> **版本**: v0.76.0（Phase 2 P2-A 第 2 版 / DDS 语义层）
> **蓝图依据**: `蓝图/phase2.md` §v0.76.0（行 397~748）
> **change-id**: `develop-v0760-dds-topic-qos`

---

## Why

v0.75.0 提供了 DDS 通信底座（DdsNode/MockDdsNode），但缺少**语义层**：Topic 命名规范、QoS 分级策略、Topic 注册表。能源场景中不同消息类型需要不同 QoS：状态类消息低延迟（最新值优先），命令类消息可靠不丢，告警类消息保留历史。本版本在 v0.75.0 通信底座上建立语义层，为 v0.77.0 路由器、v0.94.0 VPP 聚合提供消息语义保证。

---

## What Changes

- **MODIFIED `QosPolicy`** — **BREAKING**：`History` 枚举从 `KeepLast` + `history_depth: i32` 改为 `KeepLast(u32)` / `KeepAll`；新增 `deadline: Option<Duration>` / `lifespan: Option<Duration>` / `priority: i32` 字段
- **MODIFIED `MockDdsNode::write()`** — 适配 `History::KeepLast(u32)` 新签名（KeepLast 截断逻辑从 `qos.history_depth` 改为模式匹配）
- **MODIFIED 测试 T4/T5/T13** — 适配 `History::KeepLast(u32)` 破坏性变更
- **新增 `TopicSpec` / `TopicCategory` / `PayloadType`** — Topic 规范数据结构
- **新增 `TopicRegistry`** — Topic 注册表（`BTreeMap` 存储，通配符匹配）
- **新增 `TopicError`** — 3 变体错误枚举（InvalidName / Conflict / InvalidQos）
- **新增 `validate_topic_name()`** — Topic 名校验（`/` 开头，仅 `[a-zA-Z0-9_/{}`）
- **新增 `standard_topics()`** — 8 个标准预置 Topic（State/Command/Alert/Twin/Market）
- **新增 `QosPolicy::command_default()` / `alert_default()`** — 命令类/告警类默认 QoS
- **新增 `configs/topics.toml`** — Topic 注册表配置模板
- **版本号 0.75.0 → 0.76.0**（Cargo.toml / Makefile / ci.yml / ci/src/gate.rs）

---

## Impact

- **Affected specs**: v0.75.0（`QosPolicy` / `History` 破坏性变更，需同步更新 v0.75.0 测试）
- **Affected code**: `crates/protocols/agent-bus-dds/src/{qos,mock,lib}.rs` 修改 + 新增 `topic.rs` / `registry.rs`
- **依赖**: 无新增（继续使用 `slotmap` + `alloc`）
- **解锁**: v0.76.0 完成 → v0.77.0（Agent 消息路由器）开发

---

## ADDED Requirements

### Requirement: TopicSpec 与 TopicCategory

系统 SHALL 提供 `TopicSpec` 结构体与 `TopicCategory` 枚举，定义能源场景 DDS Topic 规范：

```rust
pub struct TopicSpec {
    pub name: alloc::string::String,
    pub category: TopicCategory,
    pub payload_type: PayloadType,
    pub default_qos: QosPolicy,
    pub ttl: Option<core::time::Duration>,
}

pub enum TopicCategory {
    State,    // BEST_EFFORT + KEEP_LAST(1)
    Command,  // RELIABLE + KEEP_ALL
    Alert,    // RELIABLE + KEEP_LAST(10)
    Twin,     // BEST_EFFORT + KEEP_LAST(1)
    Market,   // BEST_EFFORT + KEEP_LAST(1)
    Log,      // RELIABLE + KEEP_ALL
}
```

- **D8**：`TopicSpec` 派生 `Debug, Clone`（不派生 `PartialEq`，因 `Duration` 已实现 `PartialEq` 但 `QosPolicy` 含 `Option<Duration>`；如需比较用字段级比较）
- `ttl`：消息有效期，`None` 表示不过期

#### Scenario: 构造 TopicSpec

- **WHEN** 构造 `TopicSpec { name: "/power/state/battery/1".into(), category: TopicCategory::State, ... }`
- **THEN** 字段正确设置

---

### Requirement: PayloadType

系统 SHALL 提供 `PayloadType` 枚举，定义 DDS 负载编码格式：

```rust
pub enum PayloadType {
    Json,
    Bincode,
    Cdr,  // DDS 标准 CDR 编码
}
```

- **D6**：仅定义枚举，不实现 CDR 编码（CDR 编码由 v0.77.0 路由器或后续版本实现）

#### Scenario: 枚举变体

- **WHEN** 访问 `PayloadType::Json` / `Bincode` / `Cdr`
- **THEN** 三个变体均存在

---

### Requirement: TopicRegistry

系统 SHALL 提供 `TopicRegistry`，管理 Topic 规范注册表：

```rust
pub struct TopicRegistry {
    specs: alloc::collections::BTreeMap<alloc::string::String, TopicSpec>,
}

impl TopicRegistry {
    pub fn new() -> Self;
    pub fn with_standards() -> Self;  // 预加载 8 个标准 Topic
    pub fn register(&mut self, spec: TopicSpec) -> Result<(), TopicError>;
    pub fn lookup(&self, name: &str) -> Option<&TopicSpec>;
    pub fn match_pattern(&self, pattern: &str) -> alloc::vec::Vec<&TopicSpec>;
}
```

- **D1**：使用 `alloc::collections::BTreeMap` 替代 `std::collections::HashMap`（no_std 兼容）
- **D4**：`match_pattern` 实现简化通配符匹配（仅支持 `*`，不引入 `regex` 依赖）
- **D5**：不实现 `load_from_toml`（`toml` crate 需 `std`；`configs/topics.toml` 作为配置模板，运行时加载由后续版本 std 环境实现）

#### Scenario: 注册新 Topic

- **WHEN** 调用 `registry.register(spec)` 且 topic 名合法且未注册
- **THEN** 返回 `Ok(())`，后续 `lookup` 可查到

#### Scenario: 重复注册同名且 QoS 一致

- **WHEN** 调用 `registry.register(spec)` 且同名已注册且 `default_qos` 相同
- **THEN** 返回 `Ok(())`（幂等）

#### Scenario: 重复注册同名且 QoS 不一致

- **WHEN** 调用 `registry.register(spec)` 且同名已注册但 `default_qos` 不同
- **THEN** 返回 `Err(TopicError::Conflict)`

#### Scenario: 注册非法 topic 名

- **WHEN** 调用 `registry.register(spec)` 且 topic 名不以 `/` 开头或含非法字符
- **THEN** 返回 `Err(TopicError::InvalidName)`

#### Scenario: 查询已注册 Topic

- **WHEN** 调用 `registry.lookup("/power/state/battery/1")` 且已注册
- **THEN** 返回 `Some(&TopicSpec)`

#### Scenario: 查询未注册 Topic

- **WHEN** 调用 `registry.lookup("/unknown")` 
- **THEN** 返回 `None`

#### Scenario: 通配符匹配

- **WHEN** 调用 `registry.match_pattern("/power/state/*")` 且注册了 `/power/state/battery/1` 和 `/power/state/pv/1`
- **THEN** 返回包含两个 `&TopicSpec` 的 `Vec`

#### Scenario: 预加载标准 Topic

- **WHEN** 调用 `TopicRegistry::with_standards()`
- **THEN** 注册表包含 8 个标准 Topic（battery/pv/grid/market price/market signal/command internal/alert fault/twin update）

---

### Requirement: standard_topics() 函数

系统 SHALL 提供 `standard_topics()` 函数，返回 8 个标准预置 TopicSpec：

```rust
pub fn standard_topics() -> alloc::vec::Vec<TopicSpec>;
```

- **D8**：使用普通函数替代 `once_cell::sync::Lazy`（no_std 兼容；`once_cell::sync` 需 `std`）

#### Scenario: 标准 Topic 数量

- **WHEN** 调用 `standard_topics()`
- **THEN** 返回 `Vec` 长度为 8

#### Scenario: 标准 Topic 名称

- **WHEN** 检查 `standard_topics()` 返回值
- **THEN** 包含：`/power/state/battery/{id}` / `/power/state/pv/{id}` / `/power/state/grid` / `/power/market/price` / `/power/market/signal` / `/power/command/internal` / `/power/alert/fault` / `/power/twin/update`

---

### Requirement: validate_topic_name()

系统 SHALL 提供 `validate_topic_name()` 函数，校验 Topic 名合法性：

```rust
pub fn validate_topic_name(name: &str) -> Result<(), TopicError>;
```

- 规则：必须以 `/` 开头，仅含 `[a-zA-Z0-9_/{}` 字符

#### Scenario: 合法 topic 名

- **WHEN** 调用 `validate_topic_name("/power/state/battery/1")`
- **THEN** 返回 `Ok(())`

#### Scenario: 合法 topic 名带参数占位符

- **WHEN** 调用 `validate_topic_name("/power/state/battery/{id}")`
- **THEN** 返回 `Ok(())`

#### Scenario: 非法：不以 / 开头

- **WHEN** 调用 `validate_topic_name("power/state")`
- **THEN** 返回 `Err(TopicError::InvalidName)`

#### Scenario: 非法：含空格

- **WHEN** 调用 `validate_topic_name("/power/state battery")`
- **THEN** 返回 `Err(TopicError::InvalidName)`

#### Scenario: 非法：含特殊字符

- **WHEN** 调用 `validate_topic_name("/power/state;drop")`
- **THEN** 返回 `Err(TopicError::InvalidName)`

---

### Requirement: TopicError

系统 SHALL 提供 `TopicError` 错误枚举，3 变体：

```rust
pub enum TopicError {
    InvalidName(alloc::string::String),
    Conflict { name: alloc::string::String },
    InvalidQos(alloc::string::String),
}
```

- 派生 `Debug`
- 实现 `core::fmt::Display`
- 实现 `core::error::Error`

#### Scenario: 错误构造

- **WHEN** 构造 `TopicError::InvalidName("必须以 / 开头".into())`
- **THEN** Display 输出包含错误信息

---

## MODIFIED Requirements

### Requirement: QosPolicy（**BREAKING**）

系统 SHALL 修改 `QosPolicy`，扩展 QoS 策略字段并改变 `History` 枚举签名：

**变更前（v0.75.0）**：
```rust
pub enum History { KeepLast, KeepAll }
pub struct QosPolicy {
    pub reliability: Reliability,
    pub durability: Durability,
    pub history: History,
    pub history_depth: i32,  // 独立字段
}
```

**变更后（v0.76.0）**：
```rust
pub enum History {
    KeepLast(u32),  // 深度内嵌于枚举变体
    KeepAll,
}
pub struct QosPolicy {
    pub reliability: Reliability,
    pub durability: Durability,
    pub history: History,
    pub deadline: Option<core::time::Duration>,
    pub lifespan: Option<core::time::Duration>,
    pub priority: i32,
}
```

- **D2**：`History::KeepLast(u32)` 携带深度参数，移除 `QosPolicy::history_depth` 独立字段（蓝图规范，更符合 DDS 语义）
- **D3**：新增 `deadline` / `lifespan` / `priority` 字段（蓝图 §4.1 要求）
- `deadline`：期望最大到达间隔，超时触发告警（`Option<Duration>`，`None` 表示无限制）
- `lifespan`：样本有效期，过期丢弃（`Option<Duration>`，`None` 表示永不过期）
- `priority`：TSN 优先级映射（0=最低，7=最高）

#### Scenario: 默认 QoS

- **WHEN** 调用 `QosPolicy::default()`
- **THEN** 返回 `Reliable` + `Volatile` + `KeepLast(10)` + `deadline=None` + `lifespan=None` + `priority=0`

#### Scenario: 状态类 QoS

- **WHEN** 调用 `QosPolicy::state_default()`
- **THEN** 返回 `BestEffort` + `Volatile` + `KeepLast(1)` + `deadline=None` + `lifespan=5s` + `priority=0`

#### Scenario: 命令类 QoS（新增）

- **WHEN** 调用 `QosPolicy::command_default()`
- **THEN** 返回 `Reliable` + `TransientLocal` + `KeepAll` + `deadline=2s` + `lifespan=10s` + `priority=6`

#### Scenario: 告警类 QoS（新增）

- **WHEN** 调用 `QosPolicy::alert_default()`
- **THEN** 返回 `Reliable` + `TransientLocal` + `KeepLast(10)` + `deadline=None` + `lifespan=None` + `priority=7`

---

### Requirement: MockDdsNode::write() 适配

系统 SHALL 修改 `MockDdsNode::write()`，适配 `History::KeepLast(u32)` 新签名：

**变更前（v0.75.0）**：
```rust
if r.qos.history == History::KeepLast && r.qos.history_depth > 0 {
    let depth = r.qos.history_depth as usize;
    while r.buffer.len() > depth { r.buffer.pop_front(); }
}
```

**变更后（v0.76.0）**：
```rust
if let History::KeepLast(depth) = r.qos.history {
    let depth = depth as usize;
    while r.buffer.len() > depth { r.buffer.pop_front(); }
}
```

- **D7**：Mock 不强制 QoS 兼容性校验（继承 v0.75.0 D10）；KeepLast 截断按 reader 自身 QoS

#### Scenario: KeepLast 截断

- **WHEN** reader QoS `KeepLast(2)`，writer 写入 3 条
- **THEN** reader `take(10)` 返回最多 2 条

#### Scenario: KeepAll 不截断

- **WHEN** reader QoS `KeepAll`，writer 写入 3 条
- **THEN** reader `take(10)` 返回 3 条

---

### Requirement: 现有测试 T4/T5/T13 适配

系统 SHALL 修改 v0.75.0 的 T4/T5/T13 测试，适配 `History::KeepLast(u32)` 破坏性变更：

- **T4**：`qos.history_depth == 10` → `qos.history == History::KeepLast(10)`
- **T5**：`qos.history_depth == 1` → `qos.history == History::KeepLast(1)`
- **T13**：构造 `QosPolicy { history: History::KeepLast, history_depth: 2 }` → `history: History::KeepLast(2)`（移除 `history_depth` 字段，新增 `deadline/lifespan/priority` 字段）

---

## REMOVED Requirements

### Requirement: QosPolicy::history_depth 字段

**Reason**: `History::KeepLast(u32)` 将深度内嵌于枚举变体，`history_depth` 独立字段冗余
**Migration**: 所有引用 `qos.history_depth` 的代码改为模式匹配 `History::KeepLast(n)`

---

## 偏差声明（D1~D12）

| 偏差 | 说明 |
|------|------|
| **D1** | `TopicRegistry` 使用 `alloc::collections::BTreeMap` 替代 `std::collections::HashMap`（no_std 兼容） |
| **D2** | `History::KeepLast(u32)` 破坏性变更：深度内嵌于枚举变体，移除 `QosPolicy::history_depth` 独立字段（蓝图规范） |
| **D3** | `QosPolicy` 新增 `deadline` / `lifespan` / `priority` 字段（蓝图 §4.1 要求）；使用 `core::time::Duration`（no_std 兼容） |
| **D4** | `match_pattern` 实现简化通配符匹配（仅支持 `*` 后缀通配，不引入 `regex` 依赖；regex crate 体积大且需 std） |
| **D5** | 不实现 `load_from_toml`（`toml` crate 需 `std`；`configs/topics.toml` 作为配置模板，运行时加载由后续版本 std 环境实现） |
| **D6** | 仅定义 `PayloadType` 枚举，不实现 CDR 编码（CDR 编码由 v0.77.0 路由器或后续版本实现） |
| **D7** | Mock 不强制 QoS 兼容性校验（继承 v0.75.0 D10）；KeepLast 截断按 reader 自身 QoS |
| **D8** | `standard_topics()` 使用普通函数替代 `once_cell::sync::Lazy`（no_std 兼容；`once_cell::sync` 需 `std`） |
| **D9** | `TopicError` 作为独立错误类型（不并入 `DdsError`；Topic 语义错误与 DDS 通信错误关注点不同） |
| **D10** | 扩展 v0.75.0 `eneros-agent-bus-dds` crate（不新建 crate；Topic/QoS 与 DdsNode 同属 DDS 语义层） |
| **D11** | 配置文件位于 `configs/topics.toml`（项目规则 §2.3，非蓝图 `config/topics.toml`） |
| **D12** | 文档位于 `docs/protocols/dds-topic-qos-design.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/`） |

---

## 简化设计验证（Karpathy 原则）

- ✅ 无 `regex` 依赖（手动实现 `*` 通配符匹配，约 20 行代码）
- ✅ 无 `once_cell::sync::Lazy`（普通函数 `standard_topics()` 返回 `Vec`）
- ✅ 无 `toml` 运行时解析（配置模板仅文档作用）
- ✅ 无 CDR 编码实现（仅枚举定义）
- ✅ 无 QoS 兼容性强制校验（Mock 继承 v0.75.0 策略）
- ✅ `History::KeepLast(u32)` 替代 `history_depth` 独立字段（更符合 DDS 规范，减少字段数）
- ✅ 扩展现有 crate 而非新建（Topic/QoS 与 DdsNode 同属语义层）
