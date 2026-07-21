# Tasks

- [x] Task 1: 创建 `crates/agents/energy-market-agent/src/bid_generator.rs` — 数据模型 + trait + Mock
  - [x] SubTask 1.1: `BidSide` 枚举（2 变体 `Buy` / `Sell`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Buy`）
  - [x] SubTask 1.2: `Bid` 结构体（8 字段：`bid_id: u64` / `market_type: MarketType` / `resource_id: u64` / `price: f32` / `quantity: f32` / `side: BidSide` / `period: Period` / `timestamp: u64`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.3: `BidStrategy` 结构体（3 字段：`margin: f32` / `max_quantity: f32` / `soc_threshold: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.4: `BidIntent` 结构体（2 字段：`side: BidSide` / `target_quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.5: `BidOptimization` 结构体（2 字段：`price_adjust: f32` / `quantity: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.6: `BidError` 枚举（4 变体：`InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 1.7: `BidIntentSource` trait（`fn generate_intent(&mut self, feed: &MarketFeed, soc: f32) -> Result<BidIntent, BidError>;`，无 Send+Sync，D5）
  - [x] SubTask 1.8: `MockBidIntentSource`（`next_intent: Option<BidIntent>` / `fail: bool`，派生 Debug/Clone/Default）+ `new(intent)` / `new_failing()` / `with_intent(i)` builder；impl：fail 或 None → `Err(IntentFailed)`，否则 `Ok(intent)`（用 match 不用 unwrap）
  - [x] SubTask 1.9: `BidOptimizer` trait（`fn optimize(&mut self, intent: &BidIntent, feed: &MarketFeed, soc: f32, capacity: f32) -> Result<BidOptimization, BidError>;`，D6）
  - [x] SubTask 1.10: `MockBidOptimizer`（`next_opt: Option<BidOptimization>` / `fail: bool`）+ `new(opt)` / `new_failing()`；impl：fail 或 None → `Err(OptimizeFailed)`
  - [x] SubTask 1.11: `BidPublisher` trait（`fn publish_bids(&mut self, bids: &[Bid]) -> Result<(), BidError>;`，D11）
  - [x] SubTask 1.12: `MockBidPublisher`（`published: Vec<Bid>` / `fail: bool`）+ `new()` / `new_failing()`；impl：fail → `Err(PublishFailed)`，否则 `published.extend_from_slice(bids)` + `Ok(())`
  - [x] SubTask 1.13: `rule_intent(feed: &MarketFeed, soc: f32, capacity: f32, strategy: &BidStrategy) -> BidIntent` — `side=Sell`；`target_quantity = strategy.max_quantity.min(capacity * soc.max(0.0))`（D9）
  - [x] SubTask 1.14: `conservative_optimize(intent: &BidIntent, strategy: &BidStrategy) -> BidOptimization` — `price_adjust = strategy.margin`；`quantity = intent.target_quantity.min(strategy.max_quantity)`（D10）
  - [x] SubTask 1.15: 中文模块文档注释（v0.86.0 报价生成 + 偏差 D1/D5/D6/D7/D8/D11 引用）；`use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use crate::market_feed::{MarketFeed, MarketType, Period};`；无 std/async/panic!/unsafe/todo!/unimplemented!/Arc/String

- [x] Task 2: 在 `bid_generator.rs` 实现 `BidGenerator` 核心逻辑
  - [x] SubTask 2.1: `BidGenerator` 结构体 6 字段（`strategy: BidStrategy` / `resource_id: u64` / `intent_source: Box<dyn BidIntentSource>` / `optimizer: Box<dyn BidOptimizer>` / `publisher: Box<dyn BidPublisher>` / `next_bid_id: u64`）
  - [x] SubTask 2.2: `BidGenerator::new(strategy, resource_id, intent_source, optimizer, publisher) -> Self`（`next_bid_id = 1`）
  - [x] SubTask 2.3: `generate(&mut self, feed: &MarketFeed, soc: f32, capacity: f32, now_ms: u64) -> Result<Vec<Bid>, BidError>` 严格按序：
    1. `capacity <= 0.0` → `Err(InvalidInput)`
    2. `feed.prices.is_empty()` → `Ok(Vec::new())`
    3. intent：`self.intent_source.generate_intent(feed, soc)` 失败 → `rule_intent(feed, soc, capacity, &self.strategy)`
    4. SOC 门控：`intent.side == Sell && soc < self.strategy.soc_threshold` → `Ok(Vec::new())`
    5. opt：`self.optimizer.optimize(&intent, feed, soc, capacity)` 失败 → `conservative_optimize(&intent, &self.strategy)`
    6. 每 `PricePoint` 映射 1 条 Bid（price：Sell `+price_adjust` / Buy `(p - price_adjust).max(0.0)`；quantity：`opt.quantity.min(max_quantity).min(capacity).max(0.0)`；`bid_id = self.next_bid_id++`；market_type/resource_id/period/timestamp 传播）
    7. bids 非空 → `self.publisher.publish_bids(&bids)` 失败 → `Err(PublishFailed)`
    8. `Ok(bids)`
  - [x] SubTask 2.4: price/quantity 计算抽为私有自由函数 `compute_price(side, base, adjust) -> f32` 与 `clamp_quantity(q, max_q, capacity) -> f32`（便于单测）
  - [x] SubTask 2.5: 无 std/async/panic!/unsafe/unwrap（match 替代）

- [x] Task 3: 在 `bid_generator.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T43~T80
  - [x] SubTask 3.1: T43 — `BidSide::default() == Buy`；2 变体 Debug 非空
  - [x] SubTask 3.2: T44 — `BidStrategy::default()` 全零；显式构造 3 字段访问
  - [x] SubTask 3.3: T45 — `Bid::default()` 全零 + side==Buy + period==Flat + market_type==Spot
  - [x] SubTask 3.4: T46 — `Bid` 8 字段显式构造与访问
  - [x] SubTask 3.5: T47 — `Bid` Copy 可复制
  - [x] SubTask 3.6: T48 — `BidIntent` 构造 2 字段；`BidOptimization` 构造 2 字段
  - [x] SubTask 3.7: T49 — `BidError` 4 变体 PartialEq + Debug 非空
  - [x] SubTask 3.8: T50 — `MockBidIntentSource::new(intent)` generate_intent → Ok(intent)
  - [x] SubTask 3.9: T51 — `MockBidIntentSource::new_failing()` 与 `default()`（None）→ `Err(IntentFailed)`
  - [x] SubTask 3.10: T52 — `MockBidIntentSource::default().with_intent(i)` builder → Ok
  - [x] SubTask 3.11: T53 — `MockBidOptimizer::new(opt)` optimize → Ok(opt)
  - [x] SubTask 3.12: T54 — `MockBidOptimizer::new_failing()` 与 `default()` → `Err(OptimizeFailed)`
  - [x] SubTask 3.13: T55 — `MockBidPublisher::new()` publish 2 条 → published.len()==2
  - [x] SubTask 3.14: T56 — `MockBidPublisher::new_failing()` publish → `Err(PublishFailed)`，published 仍空
  - [x] SubTask 3.15: T57 — `rule_intent`：soc=0.5/capacity=10.0/max=3.0 → Sell + target==3.0（min(3, 5)）
  - [x] SubTask 3.16: T58 — `rule_intent`：soc=0.2/capacity=10.0/max=3.0 → target==2.0（capacity*soc 更小）；soc<0 → target==0.0（soc.max(0.0)）
  - [x] SubTask 3.17: T59 — `conservative_optimize`：price_adjust==margin；quantity==min(target, max_quantity)
  - [x] SubTask 3.18: T60 — `compute_price`：Sell 1.0 = 0.9+0.1；Buy 0.2 = 0.3-0.1；Buy floor：0.05-0.1 → 0.0
  - [x] SubTask 3.19: T61 — `clamp_quantity`：min 传播 + 负值 → 0.0
  - [x] SubTask 3.20: T62 — `generate` capacity==0.0 与 capacity==-1.0 → `Err(InvalidInput)`
  - [x] SubTask 3.21: T63 — `generate` feed.prices 空（含 dr_signals 1 条）→ `Ok([])`（MVP 不对 DR 报价，D13）
  - [x] SubTask 3.22: T64 — happy path Sell：feed 2 PricePoint（peak 0.9 / valley 0.3）+ intent Sell + opt(0.1, 5.0) + soc=0.8/threshold=0.2/capacity=10.0 → 2 bids：price 1.0/0.4、quantity 5.0、period Peak/Valley、market_type 传播
  - [x] SubTask 3.23: T65 — bid_id 递增：首次 generate 2 条 → 1,2；第二次 1 条 → 3
  - [x] SubTask 3.24: T66 — timestamp==now_ms 传播；resource_id 传播
  - [x] SubTask 3.25: T67 — Buy 路径：intent Buy + opt(0.1, 2.0) → price = point.price - 0.1
  - [x] SubTask 3.26: T68 — Buy floor 0：point.price=0.05 + adjust=0.1 → price==0.0
  - [x] SubTask 3.27: T69 — quantity clamp max_quantity：opt.quantity=8.0 > max=5.0 → 5.0
  - [x] SubTask 3.28: T70 — quantity clamp capacity：opt.quantity=8.0 > capacity=4.0（max=10）→ 4.0
  - [x] SubTask 3.29: T71 — SOC 门控：Sell 且 soc=0.1 < threshold=0.2 → `Ok([])`（optimizer 未被调用——用 MockOptimizer fail 验证：若被调用走 conservative 仍生成，断言空即证明门控先生效）
  - [x] SubTask 3.30: T72 — SOC 边界：soc == threshold → 正常生成 Sell bids
  - [x] SubTask 3.31: T73 — Buy 不受 SOC 门控：soc=0.1 < threshold 但 side=Buy → 正常生成
  - [x] SubTask 3.32: T74 — intent 失败 → 规则回退：MockIntentSource::new_failing() + 正常 optimizer → bids side==Sell（rule_intent 产出）
  - [x] SubTask 3.33: T75 — optimizer 失败 → 保守回退：MockOptimizer::new_failing() + intent Sell(2.0) + margin=0.1 → price==point.price+0.1，quantity==2.0（min(2, max)）
  - [x] SubTask 3.34: T76 — intent+optimizer 双失败 → 双回退仍生成 bids（side=Sell，price=point+margin）
  - [x] SubTask 3.35: T77 — publish 失败 → `Err(PublishFailed)`（publisher=new_failing() + happy path）
  - [x] SubTask 3.36: T78 — 规则回退后 SOC 门控仍生效：intent 失败 + soc < threshold → 规则 Sell 被门控 → `Ok([])`
  - [x] SubTask 3.37: T79 — `feed.market_type == DemandResponse` 传播到 bids（D13）
  - [x] SubTask 3.38: T80 — publish 调用证据：happy path Ok(Some…即 Ok(bids)）+ bids 非空即证明 publish_bids 已执行（装箱后读不到 published，变通同 v0.85.0 T36，注释说明）

- [x] Task 4: 修改 `crates/agents/energy-market-agent/src/lib.rs` — 追加 `pub mod bid_generator;` + 重导出（surgical）
  - [x] SubTask 4.1: 追加 `pub mod bid_generator;`（既有 5 私有 mod + 3 v0.85.0 pub mod 保留）
  - [x] SubTask 4.2: 追加 `pub use bid_generator::{Bid, BidError, BidGenerator, BidIntent, BidIntentSource, BidOptimization, BidOptimizer, BidPublisher, BidSide, BidStrategy, MockBidIntentSource, MockBidOptimizer, MockBidPublisher, conservative_optimize, rule_intent};`
  - [x] SubTask 4.3: 顶部文档注释追加 `# v0.86.0 报价生成` 段落（核心类型列表 + D1~D14 偏差表，从 spec.md 复制）
  - [x] SubTask 4.4: 不修改任何 v0.72.0/v0.85.0 既有代码行；既有 66 tests 保留
  - [x] SubTask 4.5: `lib.rs` 无 std/async/panic!/unsafe

- [x] Task 5: 修改 `crates/agents/energy-market-agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 5.1: `description` 末尾追加 ` + v0.86.0 报价生成 (LLM 意图+Solver 优化双脑报价/规则与保守回退, no_std)`
  - [x] SubTask 5.2: `[dependencies]` 段不变（无新依赖，D5/D6/D11）
  - [x] SubTask 5.3: workspace members 列表不变

- [x] Task 6: 创建配置文件 `configs/bid_strategy.toml`（D12）
  - [x] SubTask 6.1: `[strategy]` 段：`margin` / `max_quantity` / `soc_threshold`（中文注释，说明 D14 价格算法中 margin 即保守回退 price_adjust）
  - [x] SubTask 6.2: `[resource]` 段：`resource_id`（u64，注释说明 D4 无 String）
  - [x] SubTask 6.3: `[publish]` 段：`bid_topic = "/power/market/bid"`（蓝图 §4.3）
  - [x] SubTask 6.4: `[fallback]` 注释段：说明 LLM 失败→规则 / Solver 失败→保守（蓝图 §4.4）
  - [x] SubTask 6.5: 中文注释风格与 v0.85.0 market_source.toml 一致

- [x] Task 7: 创建设计文档 `docs/agents/bid-generation-design.md`（D12）
  - [x] SubTask 7.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 7.2: Mermaid 图 1：蓝图 §4.3 核心算法（市场数据+SOC+容量 → LLM 意图 → Solver LP → Bid 列表 → 发布 /power/market/bid）
  - [x] SubTask 7.3: Mermaid 图 2：generate 决策流程（capacity 校验 → prices 空 → intent 成功/规则回退 → SOC 门控 → optimize 成功/保守回退 → per-point 映射 → 发布）
  - [x] SubTask 7.4: D1~D14 偏差声明表完整（从 spec.md 复制）
  - [x] SubTask 7.5: 前置依赖引用 v0.85.0 市场订阅（MarketFeed 输入）+ v0.69.0 意图契约（双脑模式来源）+ v0.66.0 LP 模型（可选未来桥接）
  - [x] SubTask 7.6: 性能目标（生成 < 2s，标注"集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 7.7: 下游引用 v0.100.0 竞价（消费 Bid）
  - [x] SubTask 7.8: 选型对比表（规则/LP/LLM+LP，蓝图 §5.1：LP ⭐ 主用、规则兜底、LLM+LP 复杂场景）
  - [x] SubTask 7.9: 错误处理章节：BidError 4 变体 + 两级回退映射表

- [x] Task 8: 版本同步根目录文件
  - [x] SubTask 8.1: 根 `Cargo.toml` `[workspace.package] version = "0.85.0"` → `"0.86.0"`（members 不变）
  - [x] SubTask 8.2: `Makefile` `# Version: v0.86.0` + `VERSION := 0.86.0`
  - [x] SubTask 8.3: `.github/workflows/ci.yml` `# Version: v0.86.0`
  - [x] SubTask 8.4: `ci/src/gate.rs` clippy 段注释追加 `+ v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize`
  - [x] SubTask 8.5: `ci/src/gate.rs` test 段注释同步追加

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 9.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 9.2: `cargo test -p eneros-energy-market-agent` 全部通过（66 既有 + T43~T80 38 新增 = 104 tests，0 failures）
  - [x] SubTask 9.3: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 9.4: `cargo fmt -p eneros-energy-market-agent -- --check` 通过
  - [x] SubTask 9.5: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 9.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖）
  - [x] SubTask 9.7: 回归 — `cargo test -p eneros-grid-agent` 130 tests + 1 doctest
  - [x] SubTask 9.8: 回归 — `cargo test -p eneros-device-agent` 24 tests
  - [x] SubTask 9.9: 回归 — `cargo test -p eneros-tsn-time` 84 + `cargo test -p eneros-agent-bus-dds` 63

# Task Dependencies

- Task 1（bid_generator.rs 类型/trait/Mock）必须先完成 — Task 2 依赖其类型
- Task 2（BidGenerator 逻辑）依赖 Task 1
- Task 3（测试 T43~T80）依赖 Task 1 + Task 2（同文件，同一 agent 顺序完成）
- Task 4（lib.rs）依赖 Task 1~3 完成
- Task 5（Cargo.toml）可与 Task 1~4 并行
- Task 6（configs）/ Task 7（docs）可与 Task 1~5 并行
- Task 8（版本同步）依赖 Task 1~7 完成
- Task 9（构建校验）依赖所有前置任务完成

## 并行化建议

- **Sub-Agent A**：Task 1 + Task 2 + Task 3（bid_generator.rs 完整实现 + 测试，单文件单 agent 串行）
- **Sub-Agent B**：Task 6（configs）+ Task 7（docs）— 可与 A 并行
- **Sub-Agent C**：Task 4（lib.rs）+ Task 5（Cargo.toml）+ Task 8（版本同步）— 须在 A 完成后
- **最终串行**：Task 9 由主 agent 在 A/B/C 全部完成后执行
