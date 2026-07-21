# Tasks

- [x] Task 1: 新建 policy.rs — RoutingPolicy / Permission / CapabilityVerifier / MockCapabilityVerifier / RouteError / DropReason / RouteDecision
  - [x] SubTask 1.1: 定义 `Permission` 枚举（`Publish` / `Subscribe`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 1.2: 定义 `DropReason` 枚举（`Unauthorized` / `RateLimited` / `InvalidTopic` / `TokenExpired`），派生 `Debug, Clone, Copy, PartialEq, Eq`；实现 `reason_name(&self) -> &'static str`
  - [x] SubTask 1.3: 定义 `RoutingPolicy` 结构体（`require_publish_token: bool` / `require_subscribe_token: bool` / `priority_preempt: bool` / `rate_limit_per_agent: Option<u32>`），派生 `Debug, Clone`；实现 `Default`（全部 false / None）与 `strict()`（全部 true / Some(100)）
  - [x] SubTask 1.4: 定义 `RouteError` 枚举（`InvalidPattern(String)` / `Dropped(DropReason)` / `InvalidTopic(String)`），派生 `Debug`；实现 `Display` + `core::error::Error`
  - [x] SubTask 1.5: 定义 `RouteDecision` 枚举（`Deliver { priority: i32 }` / `Drop { reason: DropReason }`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 1.6: 定义 `CapabilityVerifier` trait（`fn verify(&self, perm: Permission, agent: AgentId, pattern: &str) -> Result<(), DropReason>`），无 `Send + Sync` bound（D7）
  - [x] SubTask 1.7: 实现 `MockCapabilityVerifier`（所有 `verify()` 返回 `Ok(())`），派生 `Debug, Default`
  - [x] SubTask 1.8: 注意 `AgentId` 在 router.rs 中定义，policy.rs 需 `use crate::router::AgentId`（或定义位置调整 — 先 router.rs 再 policy.rs，或用 forward declaration）

- [x] Task 2: 新建 router.rs — MessageRouter / Subscription / SubId / RouterStats / pattern 匹配 / route / dispatch
  - [x] SubTask 2.1: 定义 `AgentId(pub u64)` newtype（D12），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
  - [x] SubTask 2.2: 定义 `SubId(pub u64)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
  - [x] SubTask 2.3: 定义 `Subscription` 结构体（`id: SubId` / `subscriber_id: AgentId` / `pattern: String` / `callback: Box<dyn Fn(&DdsSample)>`），派生 `Debug`（callback 字段需手动 impl Debug 或跳过）
  - [x] SubTask 2.4: 定义 `RouterStats` 结构体（`total_routed: u64` / `total_dropped: u64` / `dropped_by_reason: BTreeMap<&'static str, u64>`），派生 `Debug, Default`
  - [x] SubTask 2.5: 实现 `pattern_matches(pattern: &str, topic: &str) -> bool`（D4：仅 `*` 后缀通配，与 v0.76.0 D4 一致）
  - [x] SubTask 2.6: 定义 `MessageRouter` 结构体（`registry: TopicRegistry` / `subscriptions: BTreeMap<String, Vec<Subscription>>` / `policy: RoutingPolicy` / `stats: RouterStats` / `next_sub_id: u64` / `verifier: Box<dyn CapabilityVerifier>`）
  - [x] SubTask 2.7: 实现 `MessageRouter::new(registry, policy) -> Self`（使用 `MockCapabilityVerifier` 默认）
  - [x] SubTask 2.8: 实现 `MessageRouter::with_verifier(registry, policy, verifier: Box<dyn CapabilityVerifier>) -> Self`
  - [x] SubTask 2.9: 实现 `MessageRouter::subscribe(&mut self, pattern: &str, subscriber_id: AgentId, callback: Box<dyn Fn(&DdsSample)>) -> Result<SubId, RouteError>`：校验 pattern 合法性（`validate_topic_name`）→ 若 `policy.require_subscribe_token` 则调 `verifier.verify(Permission::Subscribe, ...)` → 分配 SubId → 插入 subscriptions
  - [x] SubTask 2.10: 实现 `MessageRouter::unsubscribe(&mut self, id: SubId) -> Result<(), RouteError>`：遍历 subscriptions 找到并移除
  - [x] SubTask 2.11: 实现 `MessageRouter::route(&self, topic: &str, sample: &DdsSample) -> RouteDecision`：查 `registry.lookup(topic)` 获取 priority；未注册返回 priority=0；返回 `Deliver { priority }`（D9：topic 作为独立参数）
  - [x] SubTask 2.12: 实现 `MessageRouter::dispatch(&mut self, topic: &str, sample: &DdsSample) -> Result<usize, RouteError>`：调 `route()` → 若 Drop 则更新 stats 返回 Err(Dropped) → 否则遍历 subscriptions 匹配 pattern 调回调 → 更新 `total_routed` 返回 Ok(count)（D8：`&mut self` 无需 Mutex）
  - [x] SubTask 2.13: 实现 `MessageRouter::stats(&self) -> &RouterStats`

- [x] Task 3: 修改 lib.rs — 模块声明 + 重新导出 + 偏差表 + 测试
  - [x] SubTask 3.1: 添加 `pub mod policy;` + `pub mod router;` 模块声明（alphabetical: error < mock < node < policy < qos < registry < router < topic < types）
  - [x] SubTask 3.2: 添加重新导出 `pub use policy::{...}` + `pub use router::{...}`
  - [x] SubTask 3.3: 更新 `lib.rs` 顶部模块文档注释，描述 v0.77.0 路由层
  - [x] SubTask 3.4: 更新偏差声明表（v0.77.0 D1~D13）
  - [x] SubTask 3.5: 新增 T32：`Permission` 枚举变体验证
  - [x] SubTask 3.6: 新增 T33：`DropReason::reason_name()` 返回正确字符串
  - [x] SubTask 3.7: 新增 T34：`RoutingPolicy::default()` 全 false / None
  - [x] SubTask 3.8: 新增 T35：`RoutingPolicy::strict()` 全 true / Some(100)
  - [x] SubTask 3.9: 新增 T36：`RouteError::Display` 输出非空
  - [x] SubTask 3.10: 新增 T37：`MockCapabilityVerifier::verify()` 返回 Ok
  - [x] SubTask 3.11: 新增 T38：`pattern_matches` 精确匹配
  - [x] SubTask 3.12: 新增 T39：`pattern_matches` `*` 后缀通配
  - [x] SubTask 3.13: 新增 T40：`pattern_matches` 不匹配情况
  - [x] SubTask 3.14: 新增 T41：`MessageRouter::new()` 默认状态（stats 全 0）
  - [x] SubTask 3.15: 新增 T42：`subscribe()` 成功返回 SubId 递增
  - [x] SubTask 3.16: 新增 T43：`subscribe()` 非法 pattern 返回 `Err(InvalidPattern)`
  - [x] SubTask 3.17: 新增 T44：`subscribe()` 在 `require_subscribe_token=true` + Mock 放行下成功
  - [x] SubTask 3.18: 新增 T45：`dispatch()` 精确匹配 topic 派发到 1 个订阅
  - [x] SubTask 3.19: 新增 T46：`dispatch()` 通配匹配派发到多个订阅
  - [x] SubTask 3.20: 新增 T47：`dispatch()` 不匹配 topic 返回 Ok(0)
  - [x] SubTask 3.21: 新增 T48：`dispatch()` 未注册 topic 返回 priority=0 但仍 Deliver

- [x]Task 4: 配置文件
  - [x] SubTask 4.1: 创建 `configs/router_policy.toml`（TOML 模板：require_publish_token / require_subscribe_token / priority_preempt / rate_limit_per_agent）

- [x] Task 5: 设计文档
  - [x] SubTask 5.1: 创建 `docs/protocols/message-router-design.md`（12 章节 + 2 Mermaid 图 + D1~D13 偏差声明表）

- [x] Task 6: 版本同步
  - [x] SubTask 6.1: 根 `Cargo.toml` 版本号 `0.76.0` → `0.77.0`
  - [x] SubTask 6.2: `Makefile` 版本号 `0.77.0`（header 注释 + VERSION 变量）
  - [x] SubTask 6.3: `.github/workflows/ci.yml` 版本号 `0.77.0`
  - [x] SubTask 6.4: `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-agent-bus-dds v0.77.0` 含新类型列表

- [x] Task 7: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 7.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 7.2: `cargo test -p eneros-agent-bus-dds` 全部通过（48 个测试 + 1 doctest）
  - [x] SubTask 7.3: `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 7.4: `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
  - [x] SubTask 7.5: `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
  - [x] SubTask 7.6: `cargo deny check licenses bans sources` 通过
  - [x] SubTask 7.7: 回归 — v0.76.0 现有 T1~T31 测试仍全绿

# Task Dependencies

- Task 1（policy.rs）必须先完成 — Task 2 的 `MessageRouter` 依赖 `RoutingPolicy` / `RouteError` / `CapabilityVerifier`
- Task 2（router.rs）必须先完成 — Task 3 的 lib.rs 测试依赖 Router 类型
- Task 3（lib.rs）依赖 Task 1/2 完成
- Task 4/5 可与 Task 1/2 并行（配置文件 / 文档）
- Task 6 依赖 Task 1~5 完成
- Task 7 依赖所有前置任务完成
