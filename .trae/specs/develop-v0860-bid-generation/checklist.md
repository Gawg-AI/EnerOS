# Checklist

## Task 1: bid_generator.rs — 数据模型 + trait + Mock
- [x] C1: `crates/agents/energy-market-agent/src/bid_generator.rs` 文件创建
- [x] C2: `BidSide` 枚举 2 变体 `Buy` / `Sell`
- [x] C3: `BidSide` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Buy`）
- [x] C4: `Bid` 结构体 8 字段（`bid_id: u64` / `market_type: MarketType` / `resource_id: u64` / `price: f32` / `quantity: f32` / `side: BidSide` / `period: Period` / `timestamp: u64`）
- [x] C5: `Bid` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C6: `BidStrategy` 结构体 3 字段（`margin` / `max_quantity` / `soc_threshold`，均 f32），派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C7: `BidIntent` 结构体 2 字段（`side: BidSide` / `target_quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C8: `BidOptimization` 结构体 2 字段（`price_adjust: f32` / `quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C9: `BidError` 枚举 4 变体 `InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed`，派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C10: `BidIntentSource` trait 签名 `fn generate_intent(&mut self, feed: &MarketFeed, soc: f32) -> Result<BidIntent, BidError>;`，无 Send+Sync
- [x] C11: `MockBidIntentSource` 字段 `next_intent: Option<BidIntent>` / `fail: bool`；`new(intent)` / `new_failing()` / `with_intent(i)` builder
- [x] C12: `impl BidIntentSource for MockBidIntentSource`：fail → `Err(IntentFailed)`；None → `Err(IntentFailed)`；Some → `Ok(intent)`（match 不用 unwrap）
- [x] C13: `BidOptimizer` trait 签名 `fn optimize(&mut self, intent: &BidIntent, feed: &MarketFeed, soc: f32, capacity: f32) -> Result<BidOptimization, BidError>;`
- [x] C14: `MockBidOptimizer` 字段 `next_opt: Option<BidOptimization>` / `fail: bool`；`new(opt)` / `new_failing()`；fail 或 None → `Err(OptimizeFailed)`
- [x] C15: `BidPublisher` trait 签名 `fn publish_bids(&mut self, bids: &[Bid]) -> Result<(), BidError>;`
- [x] C16: `MockBidPublisher` 字段 `published: Vec<Bid>` / `fail: bool`；`new()` / `new_failing()`；fail → `Err(PublishFailed)`，否则记录 + `Ok(())`
- [x] C17: `rule_intent(feed, soc, capacity, strategy) -> BidIntent`：`side=Sell`；`target = max_quantity.min(capacity * soc.max(0.0))`
- [x] C18: `conservative_optimize(intent, strategy) -> BidOptimization`：`price_adjust = margin`；`quantity = target.min(max_quantity)`
- [x] C19: `use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use crate::market_feed::{MarketFeed, MarketType, Period};`（复用 v0.85.0 类型，不重定义）
- [x] C20: 中文模块文档注释（v0.86.0 + 偏差引用）
- [x] C21: 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!` / 无 `Arc` / 结构字段无 `String`

## Task 2: bid_generator.rs — BidGenerator 核心逻辑
- [x] C22: `BidGenerator` 6 字段（strategy / resource_id / intent_source / optimizer / publisher / next_bid_id）
- [x] C23: `BidGenerator::new(...)` 初始化 `next_bid_id = 1`
- [x] C24: `generate` capacity `<= 0.0` → `Err(InvalidInput)`
- [x] C25: `generate` `feed.prices.is_empty()` → `Ok(Vec::new())`（不调用 intent/optimizer/publisher）
- [x] C26: intent 失败 → `rule_intent` 回退（D9）
- [x] C27: SOC 门控：Sell 且 `soc < soc_threshold` → `Ok(Vec::new())`（门控在 optimizer 之前）
- [x] C28: optimizer 失败 → `conservative_optimize` 回退（D10）
- [x] C29: 每 `PricePoint` 映射 1 条 Bid（D13）
- [x] C30: Sell price = `point.price + opt.price_adjust`；Buy price = `(point.price - opt.price_adjust).max(0.0)`（D14）
- [x] C31: quantity = `opt.quantity.min(max_quantity).min(capacity).max(0.0)`（D14，§7.3 不超容量）
- [x] C32: `bid_id = next_bid_id++` 递增；market_type / resource_id / period / timestamp(now_ms) 正确传播
- [x] C33: bids 非空 → `publish_bids`；失败 → `Err(PublishFailed)`；成功 → `Ok(bids)`
- [x] C34: 私有辅助 `compute_price(side, base, adjust) -> f32` 与 `clamp_quantity(q, max_q, capacity) -> f32`
- [x] C35: 无 unwrap / panic! / unsafe

## Task 3: bid_generator.rs — 单元测试 T43~T80
- [x] C36: T43 — `BidSide::default() == Buy`；2 变体 Debug 非空
- [x] C37: T44 — `BidStrategy::default()` 全零；显式构造 3 字段
- [x] C38: T45 — `Bid::default()` 全零 + side==Buy + period==Flat + market_type==Spot
- [x] C39: T46 — `Bid` 8 字段显式构造与访问
- [x] C40: T47 — `Bid` Copy 可复制
- [x] C41: T48 — `BidIntent` / `BidOptimization` 构造
- [x] C42: T49 — `BidError` 4 变体 PartialEq + Debug
- [x] C43: T50 — `MockBidIntentSource::new(intent)` → Ok
- [x] C44: T51 — `new_failing()` 与 `default()` → `Err(IntentFailed)`
- [x] C45: T52 — `with_intent(i)` builder → Ok
- [x] C46: T53 — `MockBidOptimizer::new(opt)` → Ok
- [x] C47: T54 — optimizer `new_failing()` 与 `default()` → `Err(OptimizeFailed)`
- [x] C48: T55 — `MockBidPublisher::new()` publish 2 条 → published.len()==2
- [x] C49: T56 — publisher `new_failing()` → `Err(PublishFailed)`，published 空
- [x] C50: T57 — `rule_intent` min(max, capacity*soc) 取 max 分支（3.0）
- [x] C51: T58 — `rule_intent` 取 capacity*soc 分支（2.0）；soc<0 → 0.0
- [x] C52: T59 — `conservative_optimize` price_adjust==margin；quantity==min(target, max)
- [x] C53: T60 — `compute_price` Sell/Buy/Buy-floor 三分支
- [x] C54: T61 — `clamp_quantity` min 传播 + 负值 → 0.0
- [x] C55: T62 — `generate` capacity==0.0 / -1.0 → `Err(InvalidInput)`
- [x] C56: T63 — prices 空（含 dr_signals）→ `Ok([])`
- [x] C57: T64 — happy path Sell：2 PricePoint → 2 bids，price 1.0/0.4，quantity 5.0，period/market_type 正确
- [x] C58: T65 — bid_id 递增（1,2 → 3）
- [x] C59: T66 — timestamp==now_ms；resource_id 传播
- [x] C60: T67 — Buy 路径 price = point - adjust
- [x] C61: T68 — Buy floor 0.0
- [x] C62: T69 — quantity clamp max_quantity
- [x] C63: T70 — quantity clamp capacity
- [x] C64: T71 — SOC 门控：Sell + soc < threshold → `Ok([])`（门控先于 optimizer）
- [x] C65: T72 — SOC 边界 soc == threshold → 正常生成
- [x] C66: T73 — Buy 不受 SOC 门控
- [x] C67: T74 — intent 失败 → 规则回退（bids side==Sell）
- [x] C68: T75 — optimizer 失败 → 保守回退（price==point+margin，quantity==target）
- [x] C69: T76 — 双失败 → 双回退仍生成
- [x] C70: T77 — publish 失败 → `Err(PublishFailed)`
- [x] C71: T78 — 规则回退后 SOC 门控仍生效 → `Ok([])`
- [x] C72: T79 — DemandResponse market_type 传播
- [x] C73: T80 — publish 执行证据（变通同 v0.85.0 T36，注释说明）
- [x] C74: 浮点断言用 `f32::EPSILON` 或 1e-6 容差

## Task 4: lib.rs surgical 修改
- [x] C75: `pub mod bid_generator;` 追加
- [x] C76: `pub use bid_generator::{...}` 重导出 15 项（含 rule_intent / conservative_optimize）
- [x] C77: 顶部文档注释追加 `# v0.86.0 报价生成` 段落 + D1~D14 偏差表
- [x] C78: v0.72.0 既有 5 私有 mod + 5 行原 pub use 保留
- [x] C79: v0.85.0 既有 3 pub mod + 3 行 pub use + 文档段落保留
- [x] C80: 既有 66 tests 保留不变
- [x] C81: `lib.rs` 无 std/async/panic!/unsafe

## Task 5: Cargo.toml description 更新
- [x] C82: `description` 含 "v0.86.0 报价生成" 字样
- [x] C83: `[dependencies]` 段不变（无新依赖）
- [x] C84: workspace members 列表不变

## Task 6: configs/bid_strategy.toml
- [x] C85: 文件位于 `configs/bid_strategy.toml`
- [x] C86: `[strategy]` 段含 `margin` / `max_quantity` / `soc_threshold`
- [x] C87: `[resource]` 段含 `resource_id`
- [x] C88: `[publish]` 段含 `bid_topic = "/power/market/bid"`
- [x] C89: `[fallback]` 注释段说明两级回退（蓝图 §4.4）
- [x] C90: 中文注释风格与 market_source.toml 一致

## Task 7: docs/agents/bid-generation-design.md
- [x] C91: 文件位于 `docs/agents/bid-generation-design.md`
- [x] C92: 12 章节完整
- [x] C93: Mermaid 图 1：蓝图 §4.3 核心算法流程
- [x] C94: Mermaid 图 2：generate 决策流程（含两级回退分支）
- [x] C95: D1~D14 偏差声明表完整
- [x] C96: 前置依赖引用 v0.85.0 / v0.69.0 / v0.66.0
- [x] C97: 性能目标（< 2s，标注"集成阶段验收，本版本仅算法骨架"）
- [x] C98: 下游引用 v0.100.0 竞价
- [x] C99: 选型对比表（规则/LP/LLM+LP，LP ⭐ 主用）
- [x] C100: 错误处理章节含 BidError 4 变体 + 两级回退映射

## Task 8: 版本同步根目录文件
- [x] C101: 根 `Cargo.toml` `version = "0.86.0"`（members 不变）
- [x] C102: `Makefile` `# Version: v0.86.0` + `VERSION := 0.86.0`
- [x] C103: `.github/workflows/ci.yml` `# Version: v0.86.0`
- [x] C104: `ci/src/gate.rs` clippy 段注释含完整 v0.86.0 类型列表
- [x] C105: `ci/src/gate.rs` test 段注释同步追加

## Task 9: 构建校验（§2.4.2）
- [x] C106: `cargo metadata --format-version 1` 成功
- [x] C107: `cargo test -p eneros-energy-market-agent` 全部通过（66 既有 + 38 新增 = 104 tests，0 failures）
- [x] C108: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C109: `cargo fmt -p eneros-energy-market-agent -- --check` 退出码 0
- [x] C110: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
- [x] C111: `cargo deny check advisories licenses bans sources` 通过（advisories 因网络环境跳过；licenses/bans/sources 通过，无新依赖）
- [x] C112: 回归 — `cargo test -p eneros-grid-agent` 130 tests + 1 doctest
- [x] C113: 回归 — `cargo test -p eneros-device-agent` 24 tests
- [x] C114: 回归 — `cargo test -p eneros-tsn-time` 84 tests + `cargo test -p eneros-agent-bus-dds` 63 tests

## 总体校验
- [x] C115: 无根目录新 crate（既有 crate 追加 1 个新模块文件，符合 §2.3.1）
- [x] C116: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/`）
- [x] C117: 新配置在 `configs/bid_strategy.toml`（无 `config/` 目录）
- [x] C118: `.gitignore` 未需更新（无新文件类型）
- [x] C119: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C120: 提交信息遵循 Conventional Commits（如 `feat(agents/energy-market-agent): v0.86.0 实现市场报价生成`）
- [x] C121: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C122: no_std 合规性：`bid_generator.rs` 继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C123: 内存预算：报价模块 ≤ 1MB（算法骨架，实际远小于此）
- [x] C124: SBOM 未变化（无新第三方依赖、无新 workspace crate 依赖，D5/D6/D11）
- [x] C125: 文档同步：v0.72.0/v0.85.0 历史偏差声明保留，v0.86.0 新增 D1~D14 段落
- [x] C126: Surgical Changes 原则：v0.72.0 五文件 + v0.85.0 三文件（market_feed/parser/subscriber）完全未改动
- [x] C127: `lib.rs` 仅追加 1 个 `pub mod` + 1 行 `pub use` + 顶部文档注释
- [x] C128: `Bid` 与 v0.85.0 `MarketFeed`/`PricePoint`/`DrSignal` 命名不冲突；`MarketType`/`Period` 复用不重定义
