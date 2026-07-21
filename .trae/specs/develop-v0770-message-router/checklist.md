# Checklist

## policy.rs — 新建
- [x] C1 `Permission` 枚举（`Publish` / `Subscribe`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C2 `DropReason` 枚举（`Unauthorized` / `RateLimited` / `InvalidTopic` / `TokenExpired`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C3 `DropReason::reason_name()` 返回 `&'static str`（"Unauthorized" / "RateLimited" / "InvalidTopic" / "TokenExpired"）
- [x] C4 `RoutingPolicy` 结构体（4 字段），派生 `Debug, Clone`
- [x] C5 `RoutingPolicy::default()` 全 false / None
- [x] C6 `RoutingPolicy::strict()` 全 true / Some(100)
- [x] C7 `RouteError` 枚举（3 变体），派生 `Debug`，实现 `Display` + `core::error::Error`
- [x] C8 `RouteDecision` 枚举（2 变体），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C9 `CapabilityVerifier` trait 定义（无 `Send + Sync` bound，D7）
- [x] C10 `MockCapabilityVerifier` 实现（所有 `verify()` 返回 `Ok(())`），派生 `Debug, Default`

## router.rs — 新建
- [x] C11 `AgentId(pub u64)` newtype（D12），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C12 `SubId(pub u64)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C13 `Subscription` 结构体（`id` / `subscriber_id` / `pattern` / `callback: Box<dyn Fn(&DdsSample)>`），D7 无 Send + Sync
- [x] C14 `Subscription` 手动实现 `Debug`（callback 字段无法自动 derive）
- [x] C15 `RouterStats` 结构体（3 字段），派生 `Debug, Default`
- [x] C16 `pattern_matches(pattern, topic)` 实现（D4：仅 `*` 后缀通配）
- [x] C17 `MessageRouter` 结构体（6 字段：registry / subscriptions / policy / stats / next_sub_id / verifier）
- [x] C18 `MessageRouter::new(registry, policy)` 使用 `MockCapabilityVerifier` 默认
- [x] C19 `MessageRouter::with_verifier(registry, policy, verifier)` 接受注入
- [x] C20 `subscribe()` 校验 pattern → 校验 token（若 policy.require_subscribe_token）→ 分配 SubId → 插入
- [x] C21 `unsubscribe()` 遍历找到并移除
- [x] C22 `route(topic, sample)` 返回 `RouteDecision`，priority 从 `registry.lookup(topic)` 获取（D9：topic 独立参数）
- [x] C23 `dispatch(topic, sample)` 调 `route()` → Drop 则更新 stats 返回 Err(Dropped) → 否则遍历匹配调回调 → 更新 `total_routed` 返回 Ok(count)
- [x] C24 `stats()` 返回 `&RouterStats`
- [x] C25 使用 `BTreeMap<String, Vec<Subscription>>` 而非 HashMap（D5）
- [x] C26 使用 `&mut self` 而非 `&self` + Mutex（D8）

## lib.rs — 模块声明 + 导出 + 测试
- [x] C27 添加 `pub mod policy;` + `pub mod router;`（alphabetical 顺序）
- [x] C28 添加 `pub use policy::{CapabilityVerifier, DropReason, MockCapabilityVerifier, Permission, RouteDecision, RouteError, RoutingPolicy};`
- [x] C29 添加 `pub use router::{AgentId, MessageRouter, RouterStats, SubId, Subscription};`
- [x] C30 更新 `lib.rs` 顶部模块文档注释（描述 v0.77.0 路由层）
- [x] C31 更新偏差声明表（v0.77.0 D1~D13）
- [x] C32 T32 新增：`Permission` 枚举变体
- [x] C33 T33 新增：`DropReason::reason_name()` 字符串
- [x] C34 T34 新增：`RoutingPolicy::default()` 字段
- [x] C35 T35 新增：`RoutingPolicy::strict()` 字段
- [x] C36 T36 新增：`RouteError::Display` 非空
- [x] C37 T37 新增：`MockCapabilityVerifier::verify()` 返回 Ok
- [x] C38 T38 新增：`pattern_matches` 精确匹配
- [x] C39 T39 新增：`pattern_matches` `*` 后缀通配
- [x] C40 T40 新增：`pattern_matches` 不匹配
- [x] C41 T41 新增：`MessageRouter::new()` 默认 stats 全 0
- [x] C42 T42 新增：`subscribe()` SubId 递增
- [x] C43 T43 新增：`subscribe()` 非法 pattern → Err(InvalidPattern)
- [x] C44 T44 新增：`subscribe()` require_subscribe_token=true + Mock 放行
- [x] C45 T45 新增：`dispatch()` 精确匹配 1 个订阅
- [x] C46 T46 新增：`dispatch()` 通配匹配多订阅
- [x] C47 T47 新增：`dispatch()` 不匹配 → Ok(0)
- [x] C48 T48 新增：`dispatch()` 未注册 topic 仍 Deliver priority=0

## 配置文件
- [x] C49 `configs/router_policy.toml` 存在
- [x] C50 包含 4 个策略字段（require_publish_token / require_subscribe_token / priority_preempt / rate_limit_per_agent）

## 设计文档
- [x] C51 `docs/protocols/message-router-design.md` 存在
- [x] C52 12 章节完整（版本目标 / 前置依赖 / 交付物 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
- [x] C53 2 Mermaid 图（路由派发时序图 + 策略决策流程图）
- [x] C54 D1~D13 偏差声明表
- [x] C55 文档在 `docs/protocols/` 下（非蓝图 `docs/phase2/`）

## 版本同步
- [x] C56 根 `Cargo.toml` 版本号 `0.77.0`
- [x] C57 `Makefile` 版本号 `0.77.0`（header 注释 + VERSION 变量）
- [x] C58 `.github/workflows/ci.yml` 版本号 `0.77.0`
- [x] C59 `ci/src/gate.rs` clippy 段注释更新 `eneros-agent-bus-dds v0.77.0` 含 MessageRouter / RoutingPolicy / RouteDecision / DropReason / RouteError / RouterStats / Subscription / SubId / CapabilityVerifier / MockCapabilityVerifier / Permission / AgentId
- [x] C60 `ci/src/gate.rs` test 段注释同上

## 构建校验（§2.4.2 C6~C11）
- [x] C61 `cargo metadata --format-version 1` 成功
- [x] C62 `cargo test -p eneros-agent-bus-dds` 全部通过（48 个测试 + 1 doctest）
- [x] C63 `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C64 `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
- [x] C65 `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
- [x] C66 `cargo deny check licenses bans sources` 通过

## 回归
- [x] C67 v0.76.0 现有 T1~T31 测试仍全绿（无回归）

## no_std 合规
- [x] C68 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C69 无 `panic!` / `todo!` / `unimplemented!`
- [x] C70 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] C71 无 `std::collections::HashMap`（D5/D6：BTreeMap）
- [x] C72 无 `Send + Sync` bound（D7）
- [x] C73 无 `spin::Mutex` 包装 stats（D8：直接 `&mut self`）
- [x] C74 使用 `core::time::Duration`（非 `std::time::Duration`）

## 目录规范
- [x] C75 新文件在 `crates/protocols/agent-bus-dds/src/`（扩展现有 crate，D1）
- [x] C76 文档在 `docs/protocols/` 下（D2）
- [x] C77 配置在 `configs/` 下（D3）
- [x] C78 无根目录 crate（除 `ci/`）
- [x] C79 无垃圾文件（target/ / *.elf / *.bin / IDE 缓存）

## 简化设计验证（Karpathy 原则）
- [x] C80 无 `regex` 依赖（手动 `*` 通配符匹配，D4 一致）
- [x] C81 无 `slotmap` 新增依赖（SubId 用 u64 计数器，D11）
- [x] C82 无 `eneros-agent` crate 依赖（D10：trait 抽象 + MockCapabilityVerifier）
- [x] C83 无 `eneros-crypto` crate 依赖（D10）
- [x] C84 无性能基准测试代码（D13：CI 无法验证 ≥50K msg/s）
- [x] C85 无 token 校验结果缓存（D13：不实现 TTL 1s 缓存）
- [x] C86 扩展现有 crate 而非新建（D1）
- [x] C87 DdsSample 未修改（D9：topic 作为独立参数，避免 BREAKING）
- [x] C88 破坏性变更：无（纯增量版本）
