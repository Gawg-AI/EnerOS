# v0.85.0 Market Subscription Spec — Market Agent 市场数据订阅

## Why

v0.72.0 完成 Market Agent 基础（`MarketData`/`MarketChannel`/`MarketDataSource`），v0.84.0 完成 Grid Agent 并离网切换，但 Market Agent 尚不具备**现货/辅助服务/DR 市场数据订阅**能力。本版本扩展 `eneros-energy-market-agent` crate 增加 3 个新模块：`market_feed.rs`（市场数据源数据模型）、`parser.rs`（文本行解析）、`subscriber.rs`（订阅 + 轮询 + 缓存降级 + 发布抽象）。为 v0.86.0 报价生成（BidGenerator 消费 `MarketFeed`）提供数据输入，支撑 VPP 参与需求响应。

## What Changes

- **ADDED**：`crates/agents/energy-market-agent/src/market_feed.rs` — 市场数据模型
  - `MarketType` 枚举（3 变体：`Spot` / `AncillaryService` / `DemandResponse`，默认 `Spot`）
  - `Period` 枚举（3 变体：`Peak` / `Flat` / `Valley`，默认 `Flat`）
  - `PricePoint` 结构体（3 字段：`time: u64` / `price: f32` / `period: Period`，Copy）
  - `DrSignal` 结构体（5 字段：`event_id: u64` / `target_mw: f32` / `start: u64` / `end: u64` / `reward: f32`，Copy）
  - `MarketFeed` 结构体（4 字段：`market_type: MarketType` / `timestamp: u64` / `prices: Vec<PricePoint>` / `dr_signals: Vec<DrSignal>`）
  - `MarketError` 枚举（3 变体：`SourceFailed` / `ParseFailed` / `PublishFailed`）
- **ADDED**：`crates/agents/energy-market-agent/src/parser.rs` — 文本行解析
  - `parse_price_point(line: &str) -> Result<PricePoint, MarketError>` — 格式 `P,<time>,<price>,<period>`
  - `parse_dr_signal(line: &str) -> Result<DrSignal, MarketError>` — 格式 `D,<event_id>,<target_mw>,<start>,<end>,<reward>`
  - `parse_feed(input: &str, market_type: MarketType, timestamp: u64) -> MarketFeed` — 多行解析，格式错误行跳过（蓝图 §4.4 "数据格式错误 → 跳过"）
- **ADDED**：`crates/agents/energy-market-agent/src/subscriber.rs` — 订阅管理
  - `MarketFeedSource` trait + `MockMarketFeedSource`（数据源抽象，沿用 v0.82.0 `GridSampler` 模式）
  - `MarketFeedPublisher` trait + `MockMarketFeedPublisher`（发布抽象，替代蓝图 `DdsNode`）
  - `MarketFeedCache` 结构体（last-good 缓存，蓝图 §4.4 "接口超时 → 使用缓存"）
  - `MarketSubscriber` 结构体（source / publisher / cache / subscribed / poll_interval_ms / last_poll_ms）
  - `MarketSubscriber::subscribe(mt)` / `is_subscribed(mt)` / `poll(now_ms) -> Result<Option<MarketFeed>, MarketError>`
- **MODIFIED**：`crates/agents/energy-market-agent/src/lib.rs` — 追加 3 个 `pub mod` + 重导出（surgical：仅追加，不修改 v0.72.0 既有代码）
- **MODIFIED**：`crates/agents/energy-market-agent/Cargo.toml` — `description` 字段追加 "+ v0.85.0 市场数据订阅"（无新依赖）
- **ADDED**：`configs/market_source.toml` — 市场源配置模板
- **ADDED**：`docs/agents/market-subscription-design.md` — 设计文档（12 章 + Mermaid 图 + D1~D14 偏差表）
- **MODIFIED**：根 `Cargo.toml` workspace 版本 `0.84.0` → `0.85.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步
- **未新增 crate**：3 个新模块追加到既有 `eneros-energy-market-agent` crate（D2）

无 **BREAKING** 变更：v0.72.0 既有公共 API（`EnergyAgent` / `MarketAgent` / `AgentRuntime` / `HeartbeatStatus` / `AgentRuntimeError` / `MarketChannel` / `MarketData` / `MarketDataSource` / `MarketSignal` / `MockMarketSource`）全部保留；新增类型与函数仅追加。

## Impact

- **Affected specs**：v0.72.0 Energy/Market Agent（追加市场订阅子模块，不破坏既有 API）；为 v0.86.0 报价生成提供 `MarketFeed` / `PricePoint` / `DrSignal` 输入
- **Affected code**：
  - `crates/agents/energy-market-agent/src/market_feed.rs`（新建）
  - `crates/agents/energy-market-agent/src/parser.rs`（新建）
  - `crates/agents/energy-market-agent/src/subscriber.rs`（新建）
  - `crates/agents/energy-market-agent/src/lib.rs`（追加 3 个 `pub mod` + 重导出）
  - `crates/agents/energy-market-agent/Cargo.toml`（description 字段更新）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **依赖不变**：无新第三方依赖；无新 workspace crate 依赖；SBOM 不变
- **回归面**：v0.72.0 的 25 tests 必须仍全部通过；v0.82.0/v0.83.0/v0.84.0 grid-agent（130 tests）、v0.73.0 device-agent、v0.79~81 tsn-time、v0.75~78 agent-bus-dds 必须无回归

## ADDED Requirements

### Requirement: Market Feed Data Structures

系统 SHALL 提供市场数据模型（`market_feed.rs`）：

- `MarketType` 枚举（3 变体：`Spot` / `AncillaryService` / `DemandResponse`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Spot`）
- `Period` 枚举（3 变体：`Peak` / `Flat` / `Valley`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Flat`）
- `PricePoint` 结构体（3 字段：`time: u64` / `price: f32` / `period: Period`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `DrSignal` 结构体（5 字段：`event_id: u64` / `target_mw: f32` / `start: u64` / `end: u64` / `reward: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `MarketFeed` 结构体（4 字段：`market_type: MarketType` / `timestamp: u64` / `prices: Vec<PricePoint>` / `dr_signals: Vec<DrSignal>`），派生 `Debug, Clone, PartialEq, Default`（含 `Vec` 不派生 `Copy`）
- `MarketError` 枚举（3 变体：`SourceFailed` / `ParseFailed` / `PublishFailed`），派生 `Debug, Clone, Copy, PartialEq, Eq`

#### Scenario: Default values
- **WHEN** 调用 `MarketType::default()` / `Period::default()` / `MarketFeed::default()`
- **THEN** 分别返回 `Spot` / `Flat` / 空结构（`prices.is_empty() && dr_signals.is_empty()`）

### Requirement: Parser Functions

系统 SHALL 提供文本行解析（`parser.rs`），使用 `core::str` 方法（`split` / `trim` / `parse`），无 `serde_json` 依赖：

- `parse_price_point(line: &str) -> Result<PricePoint, MarketError>`：
  - 输入格式：`P,<time>,<price>,<period>`（period ∈ `peak`/`flat`/`valley`，大小写不敏感）
  - 前缀非 `P` / 字段数不足 / 数字解析失败 / period 未知 → `Err(MarketError::ParseFailed)`
- `parse_dr_signal(line: &str) -> Result<DrSignal, MarketError>`：
  - 输入格式：`D,<event_id>,<target_mw>,<start>,<end>,<reward>`
  - 前缀非 `D` / 字段数不足 / 数字解析失败 → `Err(MarketError::ParseFailed)`
- `parse_feed(input: &str, market_type: MarketType, timestamp: u64) -> MarketFeed`：
  - 逐行解析；`P` 行入 `prices`，`D` 行入 `dr_signals`；解析失败行**跳过**（蓝图 §4.4）；空行/空白行跳过
  - 返回 `MarketFeed { market_type, timestamp, prices, dr_signals }`

#### Scenario: Parse valid price line
- **WHEN** `parse_price_point("P,1000,0.85,peak")`
- **THEN** `Ok(PricePoint { time: 1000, price: 0.85, period: Period::Peak })`

#### Scenario: Parse invalid line returns ParseFailed
- **WHEN** `parse_price_point("P,abc,0.85,peak")` 或 `parse_price_point("X,1000,0.85,peak")`
- **THEN** `Err(MarketError::ParseFailed)`

#### Scenario: parse_feed skips bad lines
- **WHEN** 输入 3 行（1 行合法 P + 1 行非法 + 1 行合法 D）
- **THEN** 返回 `MarketFeed` 含 1 个 price + 1 个 dr_signal，无 panic

### Requirement: MarketFeedSource / MarketFeedPublisher Traits + Mocks

系统 SHALL 提供数据源与发布抽象（`subscriber.rs`），不要求 `Send + Sync`（no_std 单线程）：

```rust
pub trait MarketFeedSource {
    /// 拉取一次市场数据（同步语义，now_ms 参数注入）.
    fn fetch(&mut self, now_ms: u64) -> Result<MarketFeed, MarketError>;
}

pub trait MarketFeedPublisher {
    /// 发布电价点列表.
    fn publish_prices(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;
    /// 发布 DR 信号列表.
    fn publish_dr_signals(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;
}
```

系统 SHALL 提供 Mock 实现：
- `MockMarketFeedSource`（字段 `next_feed: Option<MarketFeed>` / `fail: bool`）：`new(feed: MarketFeed)` / `new_failing()` / `with_feed(feed)` builder；`fail == true` → `Err(SourceFailed)`；`next_feed == None` → `Err(SourceFailed)`；否则 `Ok(next_feed.clone())`
- `MockMarketFeedPublisher`（字段 `published: Vec<MarketFeed>` / `fail: bool`）：`new()` / `new_failing()`；`fail == true` → `Err(PublishFailed)`；否则记录 `feed.clone()` 到 `published` 并返回 `Ok(())`

### Requirement: MarketFeedCache

系统 SHALL 提供 `MarketFeedCache`（蓝图 §4.4 "接口超时 → 使用缓存"）：

- 字段（1 个）：`last: Option<MarketFeed>`
- `MarketFeedCache::new() -> Self`（`last = None`）
- `MarketFeedCache::store(&mut self, feed: MarketFeed)`（`last = Some(feed)`）
- `MarketFeedCache::get(&self) -> Option<&MarketFeed>`
- `MarketFeedCache::is_empty(&self) -> bool`
- 派生 `Debug, Clone, Default`

### Requirement: MarketSubscriber

系统 SHALL 提供 `MarketSubscriber` 管理订阅与周期轮询：

- 字段（6 个）：`source: Box<dyn MarketFeedSource>` / `publisher: Box<dyn MarketFeedPublisher>` / `cache: MarketFeedCache` / `subscribed: Vec<MarketType>` / `poll_interval_ms: u64` / `last_poll_ms: Option<u64>`
- `MarketSubscriber::new(source: Box<dyn MarketFeedSource>, publisher: Box<dyn MarketFeedPublisher>, poll_interval_ms: u64) -> Self`（`subscribed = Vec::new()` / `last_poll_ms = None` / `cache = MarketFeedCache::new()`）
- `MarketSubscriber::subscribe(&mut self, mt: MarketType)`（追加到 `subscribed`，重复订阅幂等）
- `MarketSubscriber::is_subscribed(&self, mt: MarketType) -> bool`
- `MarketSubscriber::cache(&self) -> &MarketFeedCache`
- `MarketSubscriber::poll(&mut self, now_ms: u64) -> Result<Option<MarketFeed>, MarketError>` 核心逻辑：
  1. **轮询门控**（D11）：若 `last_poll_ms == Some(last)` 且 `now_ms - last < poll_interval_ms` → `Ok(None)`（未到周期）
  2. 设置 `last_poll_ms = Some(now_ms)`
  3. 调用 `source.fetch(now_ms)`：
     - **失败**（蓝图 §4.4 缓存降级）：若 `cache.get()` 有数据 → `Ok(Some(cached.clone()))`；否则 → `Err(MarketError::SourceFailed)`
     - **成功** 得 `feed`：
       a. 若 `!is_subscribed(feed.market_type)` → `Ok(None)`（未订阅该类型，不缓存不发布）
       b. `cache.store(feed.clone())`
       c. 若 `!feed.prices.is_empty()` → `publisher.publish_prices(&feed)`，失败返回 `Err(PublishFailed)`
       d. 若 `!feed.dr_signals.is_empty()` → `publisher.publish_dr_signals(&feed)`，失败返回 `Err(PublishFailed)`
       e. `Ok(Some(feed))`

#### Scenario: First poll fetches
- **WHEN** 新建 subscriber（`last_poll_ms = None`）+ 已订阅 `Spot` + source 返回 Spot feed
- **THEN** `poll(0)` 返回 `Ok(Some(feed))`，cache 已存储，publisher 已记录

#### Scenario: Interval gate
- **WHEN** `poll_interval_ms = 60_000`，`poll(0)` 成功后立即 `poll(30_000)`
- **THEN** 第二次返回 `Ok(None)`（未到周期），source 未被二次调用

#### Scenario: Poll after interval
- **WHEN** `poll(0)` 成功后 `poll(60_000)`
- **THEN** 第二次重新 fetch

#### Scenario: Source failure with cache degrades
- **WHEN** `poll(0)` 成功（cache 已存），随后 source 设为 fail，`poll(60_000)`
- **THEN** 返回 `Ok(Some(cached_feed))`（缓存降级，蓝图 §4.4）

#### Scenario: Source failure without cache errors
- **WHEN** 新建 subscriber + source 恒 fail + 首次 `poll(0)`
- **THEN** 返回 `Err(MarketError::SourceFailed)`

#### Scenario: Unsubscribed type filtered
- **WHEN** subscriber 未订阅 `DemandResponse`，source 返回 DR feed
- **THEN** `poll` 返回 `Ok(None)`，cache 不更新，publisher 无记录

#### Scenario: Publisher failure propagates
- **WHEN** publisher `fail = true`，source 返回 Spot feed 含 prices
- **THEN** `poll` 返回 `Err(MarketError::PublishFailed)`

### Requirement: no_std Compliance

所有新增代码 MUST 满足 no_std 合规：
- 3 个新文件不添加 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs crate 级属性）
- 仅使用 `alloc::boxed::Box` / `alloc::vec::Vec` / `alloc::string::String`（仅测试）/ `core::str` 方法
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()` / `Duration`
- 不依赖 `eneros-agent-bus-dds` / `serde_json` / `eneros-time`（D9/D11/D14）

## MODIFIED Requirements

### Requirement: eneros-energy-market-agent crate 公共 API

v0.72.0 既有公共 API（`EnergyAgent` / `MarketAgent` / `AgentRuntime` / `HeartbeatStatus` / `AgentRuntimeError` / `MarketChannel` / `MarketData` / `MarketDataSource` / `MarketSignal` / `MockMarketSource`）全部保留不变。

本版本追加以下公共 API（仅追加，不修改既有签名）：
- 模块：`pub mod market_feed;` + `pub mod parser;` + `pub mod subscriber;`（既有 `mod` 为私有，新模块用 `pub mod` 公开）
- 重导出：
  - `pub use market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};`
  - `pub use parser::{parse_dr_signal, parse_feed, parse_price_point};`
  - `pub use subscriber::{MarketFeedCache, MarketFeedPublisher, MarketFeedSource, MarketSubscriber, MockMarketFeedPublisher, MockMarketFeedSource};`
- crate `description` 字段更新追加 `+ v0.85.0 市场数据订阅`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.85.0"`
- `Makefile` VERSION 变量 + header 注释 → `0.85.0`
- `.github/workflows/ci.yml` header 注释 → `0.85.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber`
- workspace members 列表**不变**（3 个新模块是既有 crate 的新文件）

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `async fn subscribe/poll/run` + `interval(Duration::from_secs(60))` 循环 | sync `poll(&mut self, now_ms) -> Result<Option<MarketFeed>, MarketError>` + `poll_interval_ms` 门控 | no_std 无 async runtime / 无 `Instant` / 无 `Duration`；沿用 v0.82.0 D3/D4 + v0.83.0 D1 sync 模式；外部调度器驱动 tick |
| **D2** | 新 crate `crates/agents/market_agent/` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已将 Energy+Market 合并为单 crate；新建 market_agent crate 会重复 `MarketAgent` 概念；surgical — 沿用 v0.83/v0.84 扩展既有 crate 模式 |
| **D3** | `MarketData { market_type, timestamp, prices, dr_signals }` | 命名 `MarketFeed`（文件 `market_feed.rs`） | v0.72.0 已存在 `MarketData`（字段 price_forecast/current_price/signal_type，形状不同）；改名避免 BREAKING 既有 API 与类型混淆 |
| **D4** | `DrSignal.event_id: String` | `event_id: u64` | no_std 无堆 String；Copy 语义使 `DrSignal` 可 derive Copy；与 v0.83.0 D2（pcc_id: u32）一致 |
| **D5** | 交付物列表含 `PriceSignal`，§4.1 定义 `PricePoint` | 采用 `PricePoint`（§4.1 数据结构为准） | 蓝图内部命名不一致；§4.1 为权威定义；`PriceSignal` 视为 `PricePoint` 的交付物别名 |
| **D6** | `Period` 未定义（`PricePoint.period: Period` 引用） | 定义 `Period` 枚举（`Peak`/`Flat`/`Valley`，默认 `Flat`） | 蓝图引用未定义类型；3 变体对应电力市场峰/平/谷时段 |
| **D7** | `MarketError` 引用但未定义 | 3 变体：`SourceFailed` / `ParseFailed` / `PublishFailed` | MVP 收敛错误分类；与 v0.82.0 D10 `GridError` 3 变体一致 |
| **D8** | `MarketSource { HttpApi(String), File(String), Simulated }` 枚举 | `MarketFeedSource` trait + `MockMarketFeedSource` | no_std 无 HTTP/文件系统；trait 抽象数据源（沿用 v0.82.0 D5 `GridSampler` 模式）；真实 HTTP/File 适配器后续注入 |
| **D9** | `run(&mut self, bus: &DdsNode)` + `dds::publish` | `MarketFeedPublisher` trait + `MockMarketFeedPublisher` | 避免 `eneros-agent-bus-dds` 重依赖（沿用 v0.82.0 D5/D12 `GridPublisher` 模式）；DDS 适配器后续注入 |
| **D10** | `MarketCache` 引用但未定义 | `MarketFeedCache` 结构体（`last: Option<MarketFeed>` + store/get/is_empty） | 蓝图 §4.4 "接口超时 → 使用缓存"需要缓存语义；最小实现：单条 last-good |
| **D11** | 轮询周期 60s（§6.3） | `poll_interval_ms: u64` 构造参数 + `last_poll_ms: Option<u64>` 门控 | 60s 作为推荐默认值（configs/market_source.toml）；`Option<u64>` 使首次 poll 立即执行 |
| **D12** | `docs/phase2/market_agent.md` + `config/market_source.toml` | `docs/agents/market-subscription-design.md` + `configs/market_source.toml` | 工作区规则 §2.3.3 禁止 `docs/phase2/` 平面化；工作区使用 `configs/` 而非 `config/` |
| **D13** | `tests/market_parse.rs` 集成测试 | 各新文件内 `#[cfg(test)] mod tests` 单元测试 | 沿用 v0.82.0/v0.83.0/v0.84.0 内嵌测试模式 |
| **D14** | v0.72.0 `MarketData` 派生 `serde` | 新类型不派生 `serde` | 解析器为手写文本行解析（`core::str`），不引入 `serde_json`；新类型由 parser 直接产出、crate 内消费，无需序列化往返 |

## no_std 合规声明

本版本所有新增代码：
- 继承 crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`（已在 v0.72.0 lib.rs 设置）
- `market_feed.rs` 仅使用 `alloc::vec::Vec` + `core::*`
- `parser.rs` 仅使用 `alloc::vec::Vec` + `core::str` 方法（`split`/`trim`/`parse`）
- `subscriber.rs` 仅使用 `alloc::boxed::Box` + `alloc::vec::Vec` + `crate::market_feed::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()` / `Duration`
- 可交叉编译到 `aarch64-unknown-none`（`-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）

## Surgical Changes 声明

- v0.72.0 既有源文件 `energy_agent.rs` / `error.rs` / `market.rs` / `market_agent.rs` / `runtime.rs` **完全未改动**
- `lib.rs` 仅追加 3 个 `pub mod` + 3 行 `pub use` + 顶部文档注释追加 v0.85.0 段落（不修改任何既有代码行）
- `Cargo.toml` 仅更新 `description` 字段（依赖列表不变）
- v0.72.0 既有 25 个测试必须仍全部通过
