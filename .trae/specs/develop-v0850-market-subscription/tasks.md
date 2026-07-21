# Tasks

- [x] Task 1: 创建 `crates/agents/energy-market-agent/src/market_feed.rs` — 市场数据模型
  - [x] SubTask 1.1: `MarketType` 枚举（3 变体 `Spot` / `AncillaryService` / `DemandResponse`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Spot`）
  - [x] SubTask 1.2: `Period` 枚举（3 变体 `Peak` / `Flat` / `Valley`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Flat`）
  - [x] SubTask 1.3: `PricePoint` 结构体（3 字段：`time: u64` / `price: f32` / `period: Period`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.4: `DrSignal` 结构体（5 字段：`event_id: u64` / `target_mw: f32` / `start: u64` / `end: u64` / `reward: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.5: `MarketFeed` 结构体（4 字段：`market_type: MarketType` / `timestamp: u64` / `prices: Vec<PricePoint>` / `dr_signals: Vec<DrSignal>`），派生 `Debug, Clone, PartialEq, Default`
  - [x] SubTask 1.6: `MarketError` 枚举（3 变体：`SourceFailed` / `ParseFailed` / `PublishFailed`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 1.7: `market_feed.rs` 使用 `use alloc::vec::Vec;`；无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`（no_std 合规）
  - [x] SubTask 1.8: `market_feed.rs` 中文模块文档注释（v0.85.0 市场数据模型 + 偏差 D3/D4/D5/D6/D7 引用）

- [x] Task 2: 在 `market_feed.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T1~T12
  - [x] SubTask 2.1: T1 — `MarketType::default() == Spot`
  - [x] SubTask 2.2: T2 — `MarketType` 3 变体 `Debug` 输出非空
  - [x] SubTask 2.3: T3 — `Period::default() == Flat`
  - [x] SubTask 2.4: T4 — `Period` 3 变体 `Debug` 输出非空
  - [x] SubTask 2.5: T5 — `PricePoint::default()` 全零 + `period == Flat`
  - [x] SubTask 2.6: T6 — `PricePoint` 字段构造与访问（time=1000 / price=0.85 / period=Peak）
  - [x] SubTask 2.7: T7 — `PricePoint` 派生 `Copy` 可复制
  - [x] SubTask 2.8: T8 — `DrSignal::default()` 全零
  - [x] SubTask 2.9: T9 — `DrSignal` 5 字段构造与访问（event_id=42 / target_mw=2.5 / start=1000 / end=2000 / reward=500.0）
  - [x] SubTask 2.10: T10 — `DrSignal` 派生 `Copy` 可复制
  - [x] SubTask 2.11: T11 — `MarketFeed::default()` 空 prices/dr_signals + `market_type == Spot` + `timestamp == 0`
  - [x] SubTask 2.12: T12 — `MarketError` 3 变体 `PartialEq` 相等性 + `Debug` 非空

- [x] Task 3: 创建 `crates/agents/energy-market-agent/src/parser.rs` — 文本行解析
  - [x] SubTask 3.1: `parse_price_point(line: &str) -> Result<PricePoint, MarketError>` — 格式 `P,<time>,<price>,<period>`；前缀非 `P` / 字段数不足 / 数字解析失败 / period 未知 → `Err(ParseFailed)`
  - [x] SubTask 3.2: period 解析大小写不敏感（`peak`/`Peak`/`PEAK` 均可）；`flat`/`valley` 同理
  - [x] SubTask 3.3: `parse_dr_signal(line: &str) -> Result<DrSignal, MarketError>` — 格式 `D,<event_id>,<target_mw>,<start>,<end>,<reward>`；前缀非 `D` / 字段数不足 / 数字解析失败 → `Err(ParseFailed)`
  - [x] SubTask 3.4: `parse_feed(input: &str, market_type: MarketType, timestamp: u64) -> MarketFeed` — 逐行解析；`P` 行入 `prices`；`D` 行入 `dr_signals`；解析失败行跳过（蓝图 §4.4）；空行/空白行跳过
  - [x] SubTask 3.5: 字段含空白字符时 `trim()` 处理（`P, 1000 , 0.85 , peak` 可解析）
  - [x] SubTask 3.6: `parser.rs` 使用 `use alloc::vec::Vec;` + `use crate::market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};`；仅 `core::str` 方法（`split`/`trim`/`parse`）
  - [x] SubTask 3.7: `parser.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `serde_json`（D14）

- [x] Task 4: 在 `parser.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T13~T26
  - [x] SubTask 4.1: T13 — `parse_price_point("P,1000,0.85,peak")` 成功（time=1000 / price≈0.85 / period=Peak）
  - [x] SubTask 4.2: T14 — `parse_price_point("P,2000,0.50,flat")` 成功（period=Flat）
  - [x] SubTask 4.3: T15 — `parse_price_point("P,3000,0.30,valley")` 成功（period=Valley）
  - [x] SubTask 4.4: T16 — period 大小写不敏感：`"P,1000,0.85,PEAK"` / `"P,1000,0.85,Peak"` 均成功
  - [x] SubTask 4.5: T17 — 前缀错误：`parse_price_point("X,1000,0.85,peak")` → `Err(ParseFailed)`
  - [x] SubTask 4.6: T18 — 数字解析失败：`parse_price_point("P,abc,0.85,peak")` → `Err(ParseFailed)`；`parse_price_point("P,1000,xyz,peak")` → `Err(ParseFailed)`
  - [x] SubTask 4.7: T19 — 字段数不足：`parse_price_point("P,1000,0.85")` → `Err(ParseFailed)`
  - [x] SubTask 4.8: T20 — period 未知：`parse_price_point("P,1000,0.85,unknown")` → `Err(ParseFailed)`
  - [x] SubTask 4.9: T21 — `parse_dr_signal("D,42,2.5,1000,2000,500.0")` 成功（event_id=42 / target_mw≈2.5 / start=1000 / end=2000 / reward≈500.0）
  - [x] SubTask 4.10: T22 — 前缀错误：`parse_dr_signal("P,42,2.5,1000,2000,500.0")` → `Err(ParseFailed)`
  - [x] SubTask 4.11: T23 — 数字解析失败：`parse_dr_signal("D,abc,2.5,1000,2000,500.0")` → `Err(ParseFailed)`；字段数不足 `parse_dr_signal("D,42,2.5,1000")` → `Err(ParseFailed)`
  - [x] SubTask 4.12: T24 — `parse_feed` 混合行：2 合法 P + 1 合法 D + 1 非法行 + 1 空行 → `prices.len() == 2` / `dr_signals.len() == 1` / market_type/timestamp 正确
  - [x] SubTask 4.13: T25 — `parse_feed("", Spot, 0)` → 空 prices/dr_signals，无 panic
  - [x] SubTask 4.14: T26 — `parse_feed` 字段含空白：`"P, 1000 , 0.85 , peak"` → 解析成功（trim 生效）

- [x] Task 5: 创建 `crates/agents/energy-market-agent/src/subscriber.rs` — 订阅管理
  - [x] SubTask 5.1: `MarketFeedSource` trait 定义 `fn fetch(&mut self, now_ms: u64) -> Result<MarketFeed, MarketError>;`（不要求 `Send + Sync`，D8）
  - [x] SubTask 5.2: `MockMarketFeedSource` 结构体（字段 `next_feed: Option<MarketFeed>` / `fail: bool`），派生 `Debug, Clone, Default`
  - [x] SubTask 5.3: `MockMarketFeedSource::new(feed: MarketFeed) -> Self`（`next_feed = Some(feed)` / `fail = false`）
  - [x] SubTask 5.4: `MockMarketFeedSource::new_failing() -> Self`（`next_feed = None` / `fail = true`）
  - [x] SubTask 5.5: `MockMarketFeedSource::with_feed(mut self, feed: MarketFeed) -> Self` builder
  - [x] SubTask 5.6: `impl MarketFeedSource for MockMarketFeedSource` — `fail == true` → `Err(SourceFailed)`；`next_feed == None` → `Err(SourceFailed)`；否则 `Ok(next_feed.clone())`
  - [x] SubTask 5.7: `MarketFeedPublisher` trait 定义 `fn publish_prices(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;` + `fn publish_dr_signals(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;`（D9）
  - [x] SubTask 5.8: `MockMarketFeedPublisher` 结构体（字段 `published: Vec<MarketFeed>` / `fail: bool`），派生 `Debug, Clone, Default`；`new()` / `new_failing()` 构造器
  - [x] SubTask 5.9: `impl MarketFeedPublisher for MockMarketFeedPublisher` — `fail == true` → `Err(PublishFailed)`；否则 `published.push(feed.clone())` + `Ok(())`
  - [x] SubTask 5.10: `MarketFeedCache` 结构体（字段 `last: Option<MarketFeed>`），派生 `Debug, Clone, Default`；`new()` / `store(feed)` / `get() -> Option<&MarketFeed>` / `is_empty()`
  - [x] SubTask 5.11: `MarketSubscriber` 结构体（6 字段：`source: Box<dyn MarketFeedSource>` / `publisher: Box<dyn MarketFeedPublisher>` / `cache: MarketFeedCache` / `subscribed: Vec<MarketType>` / `poll_interval_ms: u64` / `last_poll_ms: Option<u64>`）
  - [x] SubTask 5.12: `MarketSubscriber::new(source, publisher, poll_interval_ms) -> Self`（`subscribed = Vec::new()` / `last_poll_ms = None` / `cache = MarketFeedCache::new()`）
  - [x] SubTask 5.13: `MarketSubscriber::subscribe(&mut self, mt: MarketType)`（幂等：重复订阅不重复添加）
  - [x] SubTask 5.14: `MarketSubscriber::is_subscribed(&self, mt: MarketType) -> bool` / `cache(&self) -> &MarketFeedCache`
  - [x] SubTask 5.15: `MarketSubscriber::poll(&mut self, now_ms: u64) -> Result<Option<MarketFeed>, MarketError>` 核心逻辑：
    - 轮询门控（D11）：`last_poll_ms == Some(last)` 且 `now_ms - last < poll_interval_ms` → `Ok(None)`
    - 设置 `last_poll_ms = Some(now_ms)`
    - `source.fetch(now_ms)` 失败 → cache 有数据返回 `Ok(Some(cached.clone()))`（§4.4 缓存降级）；否则 `Err(SourceFailed)`
    - 成功：未订阅 `feed.market_type` → `Ok(None)`；已订阅 → cache.store + 按需 publish_prices/publish_dr_signals + `Ok(Some(feed))`
  - [x] SubTask 5.16: `subscriber.rs` 使用 `use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use crate::market_feed::{MarketError, MarketFeed, MarketType};`；无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 6: 在 `subscriber.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T27~T42
  - [x] SubTask 6.1: T27 — `MockMarketFeedSource::new(feed)` `fail == false` / fetch 返回 `Ok(feed)`
  - [x] SubTask 6.2: T28 — `MockMarketFeedSource::new_failing()` fetch → `Err(SourceFailed)`
  - [x] SubTask 6.3: T29 — `MockMarketFeedSource::default()`（`next_feed == None`）fetch → `Err(SourceFailed)`
  - [x] SubTask 6.4: T30 — `MockMarketFeedSource::with_feed(feed)` builder 生效
  - [x] SubTask 6.5: T31 — `MockMarketFeedPublisher::new()` 空 published；publish_prices 成功记录 1 条
  - [x] SubTask 6.6: T32 — `MockMarketFeedPublisher::new_failing()` publish → `Err(PublishFailed)`，published 仍空
  - [x] SubTask 6.7: T33 — `MarketFeedCache::new()` `is_empty() == true`；`store(feed)` 后 `get() == Some(&feed)` / `is_empty() == false`
  - [x] SubTask 6.8: T34 — `MarketSubscriber::new(...)` 初始化：`is_subscribed(Spot) == false` / `cache().is_empty()` / `last_poll_ms == None`
  - [x] SubTask 6.9: T35 — `subscribe(Spot)` 后 `is_subscribed(Spot) == true`；重复 `subscribe(Spot)` 幂等（subscribed.len() == 1）
  - [x] SubTask 6.10: T36 — 首次 `poll(0)` 立即 fetch（`last_poll_ms == None` 无门控）→ `Ok(Some(feed))` / cache 已存 / publisher 已记录 / `last_poll_ms == Some(0)`
  - [x] SubTask 6.11: T37 — 轮询门控：`poll_interval_ms = 60_000`，`poll(0)` 成功后 `poll(30_000)` → `Ok(None)`（source 未二次调用）
  - [x] SubTask 6.12: T38 — 过期间隔：`poll(0)` 成功后 `poll(60_000)` → 重新 fetch（`Ok(Some(feed))`）
  - [x] SubTask 6.13: T39 — 缓存降级（蓝图 §4.4）：`poll(0)` 成功后 source 设 fail，`poll(60_000)` → `Ok(Some(cached_feed))`
  - [x] SubTask 6.14: T40 — 无缓存失败：新建 subscriber + source 恒 fail + `poll(0)` → `Err(SourceFailed)`
  - [x] SubTask 6.15: T41 — 未订阅过滤：subscriber 仅订阅 `Spot`，source 返回 `DemandResponse` feed → `Ok(None)` / cache 不更新 / publisher 无记录
  - [x] SubTask 6.16: T42 — 发布失败传播：publisher `fail = true` + source 返回含 prices 的 Spot feed → `poll` 返回 `Err(PublishFailed)`

- [x] Task 7: 修改 `crates/agents/energy-market-agent/src/lib.rs` — 追加 3 个 `pub mod` + 重导出（surgical）
  - [x] SubTask 7.1: 追加 `pub mod market_feed;` / `pub mod parser;` / `pub mod subscriber;`（既有 5 个私有 `mod` 保留不变）
  - [x] SubTask 7.2: 追加 `pub use market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};`
  - [x] SubTask 7.3: 追加 `pub use parser::{parse_dr_signal, parse_feed, parse_price_point};`
  - [x] SubTask 7.4: 追加 `pub use subscriber::{MarketFeedCache, MarketFeedPublisher, MarketFeedSource, MarketSubscriber, MockMarketFeedPublisher, MockMarketFeedSource};`
  - [x] SubTask 7.5: 顶部模块文档注释追加 v0.85.0 段落（核心类型列表 + v0.85.0 D1~D14 偏差表新增段落）
  - [x] SubTask 7.6: 不修改任何 v0.72.0 既有代码行（既有 5 个 `mod` / 5 行 `pub use` / 25 个测试全部保留）
  - [x] SubTask 7.7: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 8: 修改 `crates/agents/energy-market-agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 8.1: `description` 字段追加 `+ v0.85.0 市场数据订阅 (现货/辅助服务/DR 订阅解析发布, no_std)`
  - [x] SubTask 8.2: `[dependencies]` 段不变（无新依赖，D8/D9/D14）
  - [x] SubTask 8.3: workspace members 列表不变

- [x] Task 9: 创建配置文件 `configs/market_source.toml`（D12）
  - [x] SubTask 9.1: TOML 模板含 `[market]` 段 + `subscribe_types`（`["spot", "ancillary", "dr"]`）/ `poll_interval_ms = 60000` 字段（中文注释）
  - [x] SubTask 9.2: 含 `[source]` 段 + `kind = "simulated"` / `endpoint`（预留，注释说明 HttpApi/File 适配器后续注入，D8）
  - [x] SubTask 9.3: 含 `[publish]` 段 + `price_topic = "/power/market/price"` / `dr_topic = "/power/market/signal"`（蓝图 §4.3 Topic 路径）
  - [x] SubTask 9.4: 含中文注释说明各字段用途（与 v0.83.0 pcc.toml / v0.84.0 grid_transfer.toml 风格一致）

- [x] Task 10: 创建设计文档 `docs/agents/market-subscription-design.md`（D12）
  - [x] SubTask 10.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 10.2: 至少 1 个 Mermaid 图（MarketSubscriber.poll 流程图：门控 → fetch → 缓存降级/订阅过滤 → 发布）
  - [x] SubTask 10.3: 至少 1 个 Mermaid 图（蓝图 §4.3 数据流：市场接口 → Subscriber 轮询 → Parser 解析 → 发布 /power/market/price 与 /power/market/signal）
  - [x] SubTask 10.4: D1~D14 偏差声明表完整
  - [x] SubTask 10.5: 引用 v0.72.0 Energy/Market Agent + v0.51.0 协议抽象（可选未来集成）作为前置依赖
  - [x] SubTask 10.6: 包含性能目标说明（轮询周期 60s / 延迟 < 60s，标注为"集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 10.7: 引用 v0.86.0 报价生成（BidGenerator 消费 MarketFeed）作为下游消费者
  - [x] SubTask 10.8: 包含选型对比表（REST API / 文件 / 专网直连，蓝图 §5.1）

- [x] Task 11: 版本同步根目录文件
  - [x] SubTask 11.1: 根 `Cargo.toml` `[workspace.package] version = "0.84.0"` → `"0.85.0"`
  - [x] SubTask 11.2: 根 `Cargo.toml` `[workspace.members]` 列表**不变**
  - [x] SubTask 11.3: `Makefile` 版本号 `0.84.0` → `0.85.0`（header 注释 + VERSION 变量）
  - [x] SubTask 11.4: `.github/workflows/ci.yml` 版本号 `0.84.0` → `0.85.0`
  - [x] SubTask 11.5: `ci/src/gate.rs` clippy 段注释追加 `+ v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber`
  - [x] SubTask 11.6: `ci/src/gate.rs` test 段注释同步追加类型列表

- [x] Task 12: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 12.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 12.2: `cargo test -p eneros-energy-market-agent` 全部通过（v0.72.0 25 tests + v0.85.0 T1~T42 = 67+ tests，0 failures）
  - [x] SubTask 12.3: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 12.4: `cargo fmt -p eneros-energy-market-agent -- --check` 通过
  - [x] SubTask 12.5: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 12.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
  - [x] SubTask 12.7: 回归 — `cargo test -p eneros-grid-agent` 仍通过 130 tests + 1 doctest（无回归）
  - [x] SubTask 12.8: 回归 — `cargo test -p eneros-device-agent` 仍通过 24 tests（AgentRuntime trait 未变）
  - [x] SubTask 12.9: 回归 — `cargo test -p eneros-tsn-time` 84 tests + `cargo test -p eneros-agent-bus-dds` 63 tests（无回归）

# Task Dependencies

- Task 1（market_feed.rs 数据模型）必须先完成 — Task 2/3/5 依赖其类型
- Task 2（market_feed.rs 测试 T1~T12）依赖 Task 1 完成
- Task 3（parser.rs 解析函数）依赖 Task 1（`MarketFeed` / `PricePoint` / `DrSignal` / `MarketError`）
- Task 4（parser.rs 测试 T13~T26）依赖 Task 1 + Task 3 完成
- Task 5（subscriber.rs 订阅管理）依赖 Task 1（`MarketFeed` / `MarketType` / `MarketError`）
- Task 6（subscriber.rs 测试 T27~T42）依赖 Task 1 + Task 5 完成
- Task 7（lib.rs 修改）依赖 Task 1 + Task 3 + Task 5 完成（需 3 个模块存在才能编译）
- Task 8（Cargo.toml description）可与 Task 1~7 并行
- Task 9（configs/market_source.toml）可与 Task 1~8 并行
- Task 10（docs/agents/market-subscription-design.md）可与 Task 1~9 并行
- Task 11（版本同步根目录文件）依赖 Task 1~10 完成
- Task 12（构建校验）依赖所有前置任务完成

## 并行化建议

- **Sub-Agent A**：Task 1 + Task 2（market_feed.rs 完整实现 + 测试，单文件单 agent 串行）
- **Sub-Agent B**：Task 3 + Task 4（parser.rs 完整实现 + 测试）— 依赖 Task 1，可与 Task 2 并行
- **Sub-Agent C**：Task 5 + Task 6（subscriber.rs 完整实现 + 测试）— 依赖 Task 1，可与 Task 2/B 并行
- **Sub-Agent D**：Task 9（configs）+ Task 10（docs）— 可与 A/B/C 并行
- **Sub-Agent E**：Task 7（lib.rs）+ Task 8（Cargo.toml）+ Task 11（版本同步）— 须在 A+B+C 完成后
- **最终串行**：Task 12 由主 agent 在 A/B/C/D/E 全部完成后执行
