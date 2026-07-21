# v0.86.0 Bid Generation Spec — Market Agent 报价生成

## Why

v0.85.0 完成市场数据订阅（`MarketFeed`/`MarketSubscriber`），但 Market Agent 尚不具备**基于市场数据生成报价（Bid）**的能力。本版本在既有 `eneros-energy-market-agent` crate 追加 `bid_generator.rs`：消费 `MarketFeed`（电价点列表），结合储能 SOC/容量与 `BidStrategy`，经「意图 → 优化 → 生成 → 发布」流水线产出 `Vec<Bid>`；意图/优化两级失败分别回退规则策略与保守报价（蓝图 §4.4）。为 v0.100.0 竞价与 VPP 市场交易提供核心能力。

## What Changes

- **ADDED**：`crates/agents/energy-market-agent/src/bid_generator.rs` — 报价生成
  - `BidSide` 枚举（`Buy` / `Sell`，默认 `Buy`）
  - `Bid` 结构体（8 字段：`bid_id: u64` / `market_type: MarketType` / `resource_id: u64` / `price: f32` / `quantity: f32` / `side: BidSide` / `period: Period` / `timestamp: u64`，Copy）
  - `BidStrategy` 结构体（3 字段：`margin: f32` / `max_quantity: f32` / `soc_threshold: f32`，Copy）
  - `BidIntent`（`side` / `target_quantity`）/ `BidOptimization`（`price_adjust` / `quantity`）中间结构
  - `BidError` 枚举（4 变体：`InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed`）
  - `BidIntentSource` / `BidOptimizer` / `BidPublisher` trait + 3 个 Mock
  - `BidGenerator`（6 字段；`generate(feed, soc, capacity, now_ms) -> Result<Vec<Bid>, BidError>`）
  - `rule_intent()` / `conservative_optimize()` 回退自由函数
- **MODIFIED**：`crates/agents/energy-market-agent/src/lib.rs` — 追加 `pub mod bid_generator;` + 重导出 + v0.86.0 文档段落（surgical）
- **MODIFIED**：`crates/agents/energy-market-agent/Cargo.toml` — description 追加（无新依赖）
- **ADDED**：`configs/bid_strategy.toml` — 报价策略配置模板
- **ADDED**：`docs/agents/bid-generation-design.md` — 设计文档（12 章 + Mermaid + D1~D14）
- **MODIFIED**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步 0.85.0 → 0.86.0

无 **BREAKING** 变更：v0.72.0 + v0.85.0 全部既有公共 API 保留。

## Impact

- **Affected specs**：v0.85.0 市场订阅（`MarketFeed`/`MarketType`/`Period` 被本版本消费）；为 v0.100.0 竞价提供 `Bid` 输入
- **Affected code**：`crates/agents/energy-market-agent/src/bid_generator.rs`（新建）、`lib.rs` / `Cargo.toml`（追加）、根 4 文件版本同步
- **依赖不变**：无新第三方依赖；无新 workspace crate 依赖（不依赖 eneros-llm-engine / eneros-solver-core，D5/D6）；SBOM 不变
- **回归面**：既有 66 tests（v0.72.0 24 + v0.85.0 42）必须全过；grid-agent 130 / device-agent 24 / tsn-time 84 / agent-bus-dds 63 无回归

## ADDED Requirements

### Requirement: Bid Data Structures

系统 SHALL 提供报价数据模型（`bid_generator.rs`）：

- `BidSide` 枚举（2 变体：`Buy` / `Sell`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Buy`）
- `Bid` 结构体（8 字段：`bid_id: u64` / `market_type: MarketType` / `resource_id: u64` / `price: f32`（元/MWh）/ `quantity: f32`（MW）/ `side: BidSide` / `period: Period` / `timestamp: u64`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `BidStrategy` 结构体（3 字段：`margin: f32` / `max_quantity: f32` / `soc_threshold: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `BidIntent` 结构体（2 字段：`side: BidSide` / `target_quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `BidOptimization` 结构体（2 字段：`price_adjust: f32` / `quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `BidError` 枚举（4 变体：`InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- `MarketType` / `Period` 复用 v0.85.0 `crate::market_feed` 定义（不重定义）

#### Scenario: Default values
- **WHEN** 调用 `BidSide::default()` / `Bid::default()`
- **THEN** 分别返回 `Buy` / 全零结构（`side == Buy`、`period == Flat`、`market_type == Spot`）

### Requirement: BidIntentSource / BidOptimizer / BidPublisher Traits + Mocks

系统 SHALL 提供意图/优化/发布抽象（不要求 `Send + Sync`，no_std 单线程）：

```rust
pub trait BidIntentSource {
    /// 由市场数据 + SOC 生成报价意图（同步语义）.
    fn generate_intent(&mut self, feed: &MarketFeed, soc: f32) -> Result<BidIntent, BidError>;
}

pub trait BidOptimizer {
    /// 优化报价量价（同步语义）.
    fn optimize(
        &mut self,
        intent: &BidIntent,
        feed: &MarketFeed,
        soc: f32,
        capacity: f32,
    ) -> Result<BidOptimization, BidError>;
}

pub trait BidPublisher {
    /// 发布报价列表到 /power/market/bid.
    fn publish_bids(&mut self, bids: &[Bid]) -> Result<(), BidError>;
}
```

Mock 实现：
- `MockBidIntentSource`（`next_intent: Option<BidIntent>` / `fail: bool`）：`new(intent)` / `new_failing()` / `with_intent(i)` builder；fail 或 None → `Err(IntentFailed)`
- `MockBidOptimizer`（`next_opt: Option<BidOptimization>` / `fail: bool`）：`new(opt)` / `new_failing()`；fail 或 None → `Err(OptimizeFailed)`
- `MockBidPublisher`（`published: Vec<Bid>` / `fail: bool`）：`new()` / `new_failing()`；fail → `Err(PublishFailed)`，否则追加记录并 `Ok(())`

### Requirement: Fallback Functions

系统 SHALL 提供两级确定性回退（蓝图 §4.4）：

- `pub fn rule_intent(feed: &MarketFeed, soc: f32, capacity: f32, strategy: &BidStrategy) -> BidIntent`
  — 规则策略：`side = Sell`；`target_quantity = strategy.max_quantity.min(capacity * soc.max(0.0))`（SOC 可用能量，floor 0）
- `pub fn conservative_optimize(intent: &BidIntent, strategy: &BidStrategy) -> BidOptimization`
  — 保守报价：`price_adjust = strategy.margin`；`quantity = intent.target_quantity.min(strategy.max_quantity)`

#### Scenario: Rule intent deterministic
- **WHEN** `rule_intent(feed, 0.5, 10.0, &BidStrategy { margin: 0.1, max_quantity: 3.0, soc_threshold: 0.2 })`
- **THEN** `BidIntent { side: Sell, target_quantity: 3.0 }`（min(3.0, 10.0*0.5)）

### Requirement: BidGenerator

系统 SHALL 提供 `BidGenerator`：

- 字段（6 个）：`strategy: BidStrategy` / `resource_id: u64` / `intent_source: Box<dyn BidIntentSource>` / `optimizer: Box<dyn BidOptimizer>` / `publisher: Box<dyn BidPublisher>` / `next_bid_id: u64`
- `BidGenerator::new(strategy, resource_id, intent_source, optimizer, publisher) -> Self`（`next_bid_id = 1`）
- `BidGenerator::generate(&mut self, feed: &MarketFeed, soc: f32, capacity: f32, now_ms: u64) -> Result<Vec<Bid>, BidError>` 核心逻辑（严格按序）：
  1. `capacity <= 0.0` → `Err(InvalidInput)`
  2. `feed.prices.is_empty()` → `Ok(vec![])`（MVP 仅对电价点报价，DR 信号不报价）
  3. `intent = intent_source.generate_intent(feed, soc)`，失败 → `rule_intent(feed, soc, capacity, &strategy)`（§4.4 LLM 失败回退规则）
  4. `intent.side == Sell && soc < strategy.soc_threshold` → `Ok(vec![])`（SOC 不足禁卖，§7.3 安全）
  5. `opt = optimizer.optimize(&intent, feed, soc, capacity)`，失败 → `conservative_optimize(&intent, &strategy)`（§4.4 Solver 不可用保守报价）
  6. 对 `feed.prices` 每个 `PricePoint` 生成一条 `Bid`：
     - `price`：`Sell → point.price + opt.price_adjust`；`Buy → (point.price - opt.price_adjust).max(0.0)`（floor 0）
     - `quantity`：`opt.quantity.min(strategy.max_quantity).min(capacity).max(0.0)`（§7.3 不超容量上限）
     - `bid_id = next_bid_id++`；`market_type = feed.market_type`；`resource_id`；`period = point.period`；`timestamp = now_ms`
  7. `bids` 非空 → `publisher.publish_bids(&bids)`，失败 → `Err(PublishFailed)`
  8. `Ok(bids)`

#### Scenario: Happy path Sell
- **WHEN** intent 成功（Sell）+ opt 成功（`price_adjust=0.1, quantity=5.0`）+ feed 含 2 个 PricePoint（peak 0.9 / valley 0.3）+ soc=0.8 ≥ threshold + capacity=10.0
- **THEN** 返回 2 条 Bid：`price = 1.0 / 0.4`；`quantity = 5.0`；`period = Peak / Valley`；`bid_id = 1 / 2`

#### Scenario: SOC gate blocks Sell
- **WHEN** intent.side == Sell 且 soc=0.1 < soc_threshold=0.2
- **THEN** `Ok(vec![])`，optimizer/publisher 未被调用

#### Scenario: Intent failure falls back to rule
- **WHEN** intent_source `fail=true`，其余同 Happy path
- **THEN** 仍生成 bids，side=Sell（规则回退），optimizer 收到规则 intent

#### Scenario: Optimizer failure falls back to conservative
- **WHEN** optimizer `fail=true`，intent Sell 成功，margin=0.1
- **THEN** bids `price = point.price + 0.1`（保守报价），quantity = min(intent.target, max_quantity, capacity)

#### Scenario: Publish failure propagates
- **WHEN** publisher `fail=true` 且 bids 非空
- **THEN** `Err(PublishFailed)`

### Requirement: no_std Compliance

- `bid_generator.rs` 不加 `#![cfg_attr(not(test), no_std)]`（继承 crate 级）
- 仅用 `alloc::boxed::Box` / `alloc::vec::Vec` / `alloc::vec!` + `core::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Arc` / `String`（结构字段）
- 不依赖 `eneros-llm-engine` / `eneros-solver-core` / `eneros-dual-brain`（D5/D6）

## MODIFIED Requirements

### Requirement: eneros-energy-market-agent crate 公共 API

v0.72.0 + v0.85.0 全部既有公共 API 保留不变。本版本追加：
- `pub mod bid_generator;`
- `pub use bid_generator::{Bid, BidError, BidGenerator, BidIntent, BidIntentSource, BidOptimization, BidOptimizer, BidPublisher, BidSide, BidStrategy, MockBidIntentSource, MockBidOptimizer, MockBidPublisher, conservative_optimize, rule_intent};`
- crate `description` 追加 ` + v0.86.0 报价生成 (LLM 意图+Solver 优化双脑报价/规则与保守回退, no_std)`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.86.0"`（members 不变）
- `Makefile` header 注释 + `VERSION := 0.86.0`
- `.github/workflows/ci.yml` header 注释 `v0.86.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize`

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `pub async fn generate(&self, market, soc, capacity)` | sync `fn generate(&mut self, feed, soc, capacity, now_ms)` | no_std 无 async runtime；`&mut` 因 `next_bid_id` 递增 + Mock 状态；`now_ms` 参数注入（沿用 v0.82~v0.85 D1/D2） |
| **D2** | 新文件于 `crates/agents/market_agent/` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已合并 Energy+Market 单 crate；新建会重复 MarketAgent 概念（沿用 v0.85.0 D2） |
| **D3** | `market: &MarketData` | `feed: &MarketFeed` | v0.85.0 D3 命名延续；同 crate 直接复用，零适配层 |
| **D4** | `bid_id: String` / `resource_id: String` | `u64` / `u64` | no_std 无堆 String；`Bid` 保持 Copy；`next_bid_id` 原子递增（沿用 v0.85.0 D4） |
| **D5** | `llm: Arc<dyn LlmEngine>` | 本地 `BidIntentSource` trait + `MockBidIntentSource` | 避免 eneros-llm-engine 重依赖（沿用 v0.85.0 D8 模式）；真实 LLM 适配器后续注入；Arc 需要原子+线程语义，单线程用 Box |
| **D6** | `solver: Arc<dyn Solver>` | 本地 `BidOptimizer` trait + `MockBidOptimizer` | 同上，避免 eneros-solver-core 重依赖 |
| **D7** | `generate_bid_intent` / `solve_bid` / `into_bids` 未定义 | `BidIntent{side,target_quantity}` / `BidOptimization{price_adjust,quantity}` 中间结构 + generate 内联映射 | 蓝图引用未定义方法；MVP 最小字段，禁止投机字段（Simplicity First） |
| **D8** | `BidError` 引用未定义 | 4 变体：`InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed` | MVP 错误分类；与 v0.85.0 D7 `MarketError` 3 变体风格一致 |
| **D9** | §4.4 "LLM 输出非法 → 回退到规则策略" | `rule_intent()` 自由函数 + generate 内 `unwrap_or_else` 回退 | 规则确定性：Sell + `min(max_quantity, capacity*soc)`；测试可复现 |
| **D10** | §4.4 "Solver 不可用 → 使用保守报价" | `conservative_optimize()` 自由函数 + 回退 | 保守确定性：`price_adjust=margin`，`quantity=min(target, max_quantity)` |
| **D11** | §4.3 "发布 /power/market/bid"（`dds::publish`） | `BidPublisher` trait + `MockBidPublisher` | 避免 eneros-agent-bus-dds 重依赖（沿用 v0.85.0 D9 `MarketFeedPublisher` 模式）；DDS 适配器后续注入 |
| **D12** | `docs/phase2/bid_generation.md` + `tests/bid_strategy.rs` | `docs/agents/bid-generation-design.md` + 文件内 `#[cfg(test)] mod tests` | 工作区规则 §2.3.3 禁止 docs/phase2 平面化；内嵌测试沿用 v0.82~v0.85 模式 |
| **D13** | `let bids = opt.into_bids(&self.strategy)`（生成数量未定义） | 对 `feed.prices` 每 `PricePoint` 生成 1 条 `Bid`（period 级报价） | 蓝图未定义列表长度；per-period 报价为电力市场标准做法；DR 信号 MVP 不报价（`prices` 空 → `Ok([])`) |
| **D14** | 报价价格/量算法未定义 | Sell: `point.price + price_adjust`；Buy: `(point.price - price_adjust).max(0.0)`；quantity: `min(opt.quantity, max_quantity, capacity).max(0.0)` | §7.3 "报价不超容量上限"具体化；Buy 价 floor 0 防负价；全确定性可测试 |

## no_std 合规声明

- 继承 crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`
- 仅用 `alloc::boxed::Box` / `alloc::vec::Vec` / `alloc::vec!`（测试）+ `core::*`
- 无 `Arc` / `String` / `Instant` / `Duration` / `async`
- 可交叉编译 `aarch64-unknown-none`（`-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）

## Surgical Changes 声明

- v0.72.0 既有 5 源文件 + v0.85.0 既有 3 源文件（`market_feed.rs` / `parser.rs` / `subscriber.rs`）**完全未改动**
- `lib.rs` 仅追加 1 个 `pub mod` + 1 行 `pub use` + 顶部文档注释追加 v0.86.0 段落
- `Cargo.toml` 仅更新 `description` 字段
- 既有 66 tests（v0.72.0 24 + v0.85.0 42）必须仍全部通过
