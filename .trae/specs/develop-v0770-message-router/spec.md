# v0.77.0 Agent 消息路由器 Spec

## Why

v0.76.0 完成了 DDS 语义层（TopicSpec / QosPolicy / TopicRegistry），但 DDS pub/sub 本身无访问控制——任意 Agent 可订阅任意 Topic，高优先级消息也无法优先转发。v0.77.0 在 DDS 之上引入应用层消息路由器 `MessageRouter`，将 pub/sub 升级为带策略的语义路由：支持 Topic 通配匹配、能力模型校验、优先级排序、速率限制。

**业务价值**：Agent 间通信受能力模型管控，防止越权订阅/发布；高优先级消息优先转发；为 v0.89.0 数字孪生旁路监听、v0.92.0 仲裁提供路由基础。

## What Changes

- **新建 `crates/protocols/agent-bus-dds/src/router.rs`**：`MessageRouter` / `Subscription` / `SubId` / `RouterStats` / `RouteDecision` / `DropReason` / pattern 匹配 + 路由 + 派发逻辑
- **新建 `crates/protocols/agent-bus-dds/src/policy.rs`**：`RoutingPolicy` / `Permission` 枚举 / `CapabilityVerifier` trait / `MockCapabilityVerifier` / `RouteError`
- **修改 `crates/protocols/agent-bus-dds/src/lib.rs`**：新增 `pub mod router;` + `pub mod policy;` 模块声明与重新导出；更新偏差声明表（v0.77.0 D1~D13）；新增 T32~T48 集成测试（17 个新测试）
- **新建 `configs/router_policy.toml`**：路由策略配置模板（项目规则 §2.3，非蓝图 `config/`）
- **新建 `docs/protocols/message-router-design.md`**：设计文档（12 章节 + 2 Mermaid 图 + D1~D13 偏差声明）
- **版本同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本号 `0.76.0 → 0.77.0`

### 标注说明

- **ADDED**：新增 Router / Policy 全部类型与逻辑
- **MODIFIED**：lib.rs（模块声明 + 导出 + 偏差表 + 新增测试，不破坏 v0.76.0 现有 T1~T31 测试）
- **REMOVED**：无（v0.76.0 类型保持不变）
- **BREAKING**：无（DdsSample 不修改；Router API 接受 `topic: &str` 作为独立参数，避免破坏 v0.76.0 DdsSample）

## Impact

- **Affected specs**: v0.76.0（DDS Topic/QoS 语义层，被 Router 复用）/ v0.39.0 能力 Token（被 Router 通过 trait 抽象间接复用）/ v0.40.0 能力签发校验（同上）
- **Affected code**:
  - `crates/protocols/agent-bus-dds/src/router.rs`（新建）
  - `crates/protocols/agent-bus-dds/src/policy.rs`（新建）
  - `crates/protocols/agent-bus-dds/src/lib.rs`（修改：模块声明 + 导出 + 偏差表 + 测试）
  - `configs/router_policy.toml`（新建）
  - `docs/protocols/message-router-design.md`（新建）
  - `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **下游解锁**：v0.78.0（消息序列化与签名）、v0.89.0（数字孪生旁路监听）、v0.92.0（仲裁）

## 偏差声明（D1~D13）

> 依据 andrej-karpathy-skills-main 四原则（Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution）与项目规则（`e:\eneros\.trae\rules\记忆.md`）。

| 偏差 | 蓝图原文 | 实际实现 | 理由 |
|------|---------|---------|------|
| **D1** | `crates/agent_bus_dds/src/router.rs` | `crates/protocols/agent-bus-dds/src/router.rs` | 项目规则 §2.3.1：所有 crate 必须在 `crates/<subsystem>/` 下；扩展现有 v0.75.0 crate |
| **D2** | `docs/phase2/message_router.md` | `docs/protocols/message-router-design.md` | 项目规则 §2.3.3：文档按 topic 分类入 `docs/<topic>/` |
| **D3** | `config/router_policy.toml` | `configs/router_policy.toml` | 项目规则 §2.3：配置文件入 `configs/`（注意 `configs/` 非 `config/`） |
| **D4** | `tests/router_authz.rs` / `tests/router_priority.rs` | `src/lib.rs` 内 `#[cfg(test)] mod tests` T32~T48 | 沿用 v0.75.0/v0.76.0 模式：测试内嵌 lib.rs，避免 workspace 集成测试复杂度 |
| **D5** | `HashMap<TopicPattern, Vec<Subscription>>` | `BTreeMap<String, Vec<Subscription>>` | no_std 合规（v0.76.0 D1 先例：BTreeMap 替代 HashMap） |
| **D6** | `HashMap<&'static str, u64>` for `dropped_by_reason` | `BTreeMap<&'static str, u64>` | 同 D5 |
| **D7** | `Box<dyn Fn(&DdsSample) + Send + Sync>` 回调 | `Box<dyn Fn(&DdsSample)>`（无 Send + Sync） | 沿用 v0.59.0/v0.64.0/v0.72.0 模式：no_std 单线程无需 Send + Sync bound |
| **D8** | `Mutex<RouterStats>` + `&self` 方法 | 直接 `RouterStats` + `&mut self` 方法 | Karpathy 简化：MVP 单线程，无需 interior mutability；`spin::Mutex` 在 `&self` 方法中 `.lock().unwrap()` 蓝图代码不正确（spin::Mutex::lock 不返回 Result） |
| **D9** | `route(&self, sample: &DdsSample)` 用 `sample.topic` | `route(&mut self, topic: &str, sample: &DdsSample)` | v0.76.0 `DdsSample` 仅有 `payload`/`instance_handle`/`source_timestamp`，无 `topic` 字段。避免 BREAKING 变更 DdsSample，将 topic 作为独立参数传入（Surgical Changes） |
| **D10** | `sub.token.verify_permission(Permission::Subscribe, &p)` 直接调用 CapabilityToken | 定义 `CapabilityVerifier` trait + `MockCapabilityVerifier`（默认放行） | v0.39.0 CapabilityToken 实际 API 是 `CapabilityManager::check_access(agent_id, target, permission_set, now)`，与蓝图假设的 `verify_permission(Permission, &pattern)` 不匹配。直接集成需将 topic pattern 映射到 `ResourceTarget::File(pattern)`、Permission::Subscribe 映射到 PermissionSet 位——复杂且跨 crate 耦合。用 trait 抽象解耦，真实集成后置到 feature-gate 或后续版本 |
| **D11** | `SubId::new()`（slotmap 风格） | `SubId(pub u64)` 自增计数器 | Karpathy 简化：Router 内部维护 `next_sub_id: u64`，无需 slotmap 依赖 |
| **D12** | `AgentId`（来自 v0.39.0 能力模型，u128） | Router 本地定义 `AgentId(pub u64)` | 避免 Router 直接依赖 `eneros-agent` crate；u64 对单机 Agent 足够；后续联邦可升级 |
| **D13** | 高吞吐能力校验缓存（TTL 1s）+ 性能基准 ≥50K msg/s | 不实现缓存；不实现性能基准测试 | Karpathy 简化：MVP 阶段优先正确性，缓存与基准后置到优化版本；CI 无法验证吞吐指标 |

## ADDED Requirements

### Requirement: 消息路由器核心类型

系统 SHALL 提供以下 Router 核心类型，位于 `crates/protocols/agent-bus-dds/src/router.rs`：

- `MessageRouter` 结构体，持有 `TopicRegistry` / `BTreeMap<String, Vec<Subscription>>` / `RoutingPolicy` / `RouterStats` / `u64` 计数器 / `Box<dyn CapabilityVerifier>`
- `Subscription` 结构体，含 `id: SubId` / `subscriber_id: AgentId` / `pattern: String` / `callback: Box<dyn Fn(&DdsSample)>`
- `SubId(pub u64)` newtype
- `RouterStats` 结构体，含 `total_routed: u64` / `total_dropped: u64` / `dropped_by_reason: BTreeMap<&'static str, u64>`
- `RouteDecision` 枚举：`Deliver { priority: i32 }` / `Drop { reason: DropReason }`
- `DropReason` 枚举：`Unauthorized` / `RateLimited` / `InvalidTopic` / `TokenExpired`，派生 `Debug, Clone, Copy, PartialEq, Eq`
- `AgentId(pub u64)` newtype（D12）

#### Scenario: Router 创建与默认状态
- **WHEN** 调用 `MessageRouter::new(TopicRegistry::with_standards(), RoutingPolicy::default())`
- **THEN** 返回的 router `stats().total_routed == 0` 且 `stats().total_dropped == 0`

#### Scenario: 订阅注册成功
- **WHEN** 调用 `router.subscribe("/power/state/*", subscription)` 且 pattern 合法
- **THEN** 返回 `Ok(SubId(1))`，后续订阅返回 `SubId(2)`、`SubId(3)`…

### Requirement: 路由策略与能力校验

系统 SHALL 提供 `RoutingPolicy` 与 `CapabilityVerifier` 抽象，位于 `crates/protocols/agent-bus-dds/src/policy.rs`：

- `RoutingPolicy` 结构体：`require_publish_token: bool` / `require_subscribe_token: bool` / `priority_preempt: bool` / `rate_limit_per_agent: Option<u32>`
- `Permission` 枚举：`Publish` / `Subscribe`，派生 `Debug, Clone, Copy, PartialEq, Eq`
- `CapabilityVerifier` trait：`fn verify(&self, perm: Permission, agent: AgentId, pattern: &str) -> Result<(), DropReason>`
- `MockCapabilityVerifier`：默认实现，所有校验返回 `Ok(())`
- `RouteError` 枚举：`InvalidPattern(String)` / `Dropped(DropReason)` / `InvalidTopic(String)`，实现 `Display` + `core::error::Error`

#### Scenario: 越权订阅被拒绝
- **WHEN** `RoutingPolicy { require_subscribe_token: true, .. }` 且 `verifier.verify(Permission::Subscribe, agent, pattern)` 返回 `Err(DropReason::Unauthorized)`
- **THEN** `router.subscribe(pattern, sub)` 返回 `Err(RouteError::Dropped(DropReason::Unauthorized))`

#### Scenario: Mock 默认放行
- **WHEN** 使用 `MockCapabilityVerifier`（默认）
- **THEN** 所有 `verify(...)` 调用返回 `Ok(())`，订阅与派发均不受能力校验阻断

### Requirement: 通配匹配与派发

系统 SHALL 实现简化的 `*` 后缀通配匹配（与 v0.76.0 D4 一致）：

- `pattern_matches(pattern: &str, topic: &str) -> bool`：若 pattern 以 `*` 结尾，匹配前缀；否则精确匹配
- `MessageRouter::dispatch(&mut self, topic: &str, sample: &DdsSample) -> Result<usize, RouteError>`：遍历所有 subscription，匹配 topic 的回调被调用，返回受派发的订阅数

#### Scenario: 通配匹配多订阅派发
- **WHEN** 2 个订阅 pattern `/power/state/*` 和 `/power/state/battery`，派发 topic `/power/state/battery`
- **THEN** 2 个回调均被调用，`dispatch()` 返回 `Ok(2)`

#### Scenario: 不匹配 topic 无派发
- **WHEN** 1 个订阅 pattern `/market/*`，派发 topic `/power/state/battery`
- **THEN** 无回调被调用，`dispatch()` 返回 `Ok(0)`

### Requirement: 优先级排序

系统 SHALL 在 `route()` 决策中提供 topic 优先级查询：

- `MessageRouter::route(&self, topic: &str, sample: &DdsSample) -> RouteDecision`
- 优先级从 `TopicRegistry::lookup(topic).default_qos.priority` 获取
- 未注册 topic 默认 priority=0

#### Scenario: 已注册 topic 优先级
- **WHEN** `route("/command/internal", sample)` 且该 topic 已在 `TopicRegistry::with_standards()` 注册（QoS `command_default` priority=6）
- **THEN** 返回 `RouteDecision::Deliver { priority: 6 }`

#### Scenario: 未注册 topic 默认优先级
- **WHEN** `route("/unknown/topic", sample)` 且该 topic 未注册
- **THEN** 返回 `RouteDecision::Deliver { priority: 0 }`

### Requirement: 统计与可观测

系统 SHALL 提供 `RouterStats` 统计：

- `dispatch()` 成功时 `total_routed += 1`
- `dispatch()` 因 `DropReason` 拒绝时 `total_dropped += 1` 且 `dropped_by_reason[reason_name] += 1`
- `stats()` 返回 `&RouterStats` 供查询

#### Scenario: 拒绝统计
- **WHEN** `route()` 返回 `Drop { reason: Unauthorized }`，调用 `dispatch()`
- **THEN** `stats().total_dropped == 1` 且 `stats().dropped_by_reason["Unauthorized"] == 1`

## MODIFIED Requirements

### Requirement: eneros-agent-bus-dds crate 模块声明

v0.76.0 的 `lib.rs` SHALL 在现有模块基础上新增：

```rust
pub mod policy;
pub mod router;
```

并新增重新导出：

```rust
pub use policy::{
    CapabilityVerifier, DropReason, MockCapabilityVerifier, Permission, RouteDecision,
    RouteError, RoutingPolicy,
};
pub use router::{AgentId, MessageRouter, RouterStats, SubId, Subscription};
```

偏差声明表更新为 v0.77.0 D1~D13（保留 v0.76.0 历史记录于设计文档）。

## REMOVED Requirements

无。v0.77.0 是纯增量版本，不删除任何 v0.76.0 类型或测试。
