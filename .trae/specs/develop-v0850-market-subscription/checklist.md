# Checklist

## Task 1: market_feed.rs — 市场数据模型
- [x] C1: `crates/agents/energy-market-agent/src/market_feed.rs` 文件创建
- [x] C2: `MarketType` 枚举 3 变体 `Spot` / `AncillaryService` / `DemandResponse`
- [x] C3: `MarketType` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Spot`）
- [x] C4: `Period` 枚举 3 变体 `Peak` / `Flat` / `Valley`
- [x] C5: `Period` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Flat`）
- [x] C6: `PricePoint` 结构体 3 字段（`time: u64` / `price: f32` / `period: Period`）
- [x] C7: `PricePoint` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C8: `DrSignal` 结构体 5 字段（`event_id: u64` / `target_mw: f32` / `start: u64` / `end: u64` / `reward: f32`）
- [x] C9: `DrSignal` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C10: `MarketFeed` 结构体 4 字段（`market_type: MarketType` / `timestamp: u64` / `prices: Vec<PricePoint>` / `dr_signals: Vec<DrSignal>`）
- [x] C11: `MarketFeed` 派生 `Debug, Clone, PartialEq, Default`（含 Vec 不派生 Copy）
- [x] C12: `MarketError` 枚举 3 变体 `SourceFailed` / `ParseFailed` / `PublishFailed`
- [x] C13: `MarketError` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C14: `market_feed.rs` 使用 `use alloc::vec::Vec;`（no_std 合规）
- [x] C15: `market_feed.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`
- [x] C16: `market_feed.rs` 中文模块文档注释（v0.85.0 + 偏差 D3/D4/D5/D6/D7 引用）

## Task 2: market_feed.rs — 单元测试 T1~T12
- [x] C17: T1 — `MarketType::default() == Spot`
- [x] C18: T2 — `MarketType` 3 变体 `Debug` 输出非空
- [x] C19: T3 — `Period::default() == Flat`
- [x] C20: T4 — `Period` 3 变体 `Debug` 输出非空
- [x] C21: T5 — `PricePoint::default()` 全零 + `period == Flat`
- [x] C22: T6 — `PricePoint` 字段构造与访问
- [x] C23: T7 — `PricePoint` 派生 `Copy` 可复制
- [x] C24: T8 — `DrSignal::default()` 全零
- [x] C25: T9 — `DrSignal` 5 字段构造与访问
- [x] C26: T10 — `DrSignal` 派生 `Copy` 可复制
- [x] C27: T11 — `MarketFeed::default()` 空 prices/dr_signals + `market_type == Spot` + `timestamp == 0`
- [x] C28: T12 — `MarketError` 3 变体 `PartialEq` 相等性 + `Debug` 非空

## Task 3: parser.rs — 文本行解析
- [x] C29: `crates/agents/energy-market-agent/src/parser.rs` 文件创建
- [x] C30: `parse_price_point(line: &str) -> Result<PricePoint, MarketError>` 存在
- [x] C31: `parse_price_point` 格式 `P,<time>,<price>,<period>`；前缀非 `P` / 字段数不足 / 数字解析失败 / period 未知 → `Err(ParseFailed)`
- [x] C32: period 解析大小写不敏感（`peak`/`Peak`/`PEAK`/`flat`/`valley`）
- [x] C33: `parse_dr_signal(line: &str) -> Result<DrSignal, MarketError>` 存在
- [x] C34: `parse_dr_signal` 格式 `D,<event_id>,<target_mw>,<start>,<end>,<reward>`；前缀非 `D` / 字段数不足 / 数字解析失败 → `Err(ParseFailed)`
- [x] C35: `parse_feed(input: &str, market_type: MarketType, timestamp: u64) -> MarketFeed` 存在
- [x] C36: `parse_feed` 逐行解析；`P` 行入 `prices`；`D` 行入 `dr_signals`
- [x] C37: `parse_feed` 解析失败行跳过（蓝图 §4.4）；空行/空白行跳过；无 panic
- [x] C38: 字段含空白字符时 `trim()` 处理
- [x] C39: `parser.rs` 使用 `use alloc::vec::Vec;` + `use crate::market_feed::{...}`；仅 `core::str` 方法
- [x] C40: `parser.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `serde_json`（D14）

## Task 4: parser.rs — 单元测试 T13~T26
- [x] C41: T13 — `parse_price_point("P,1000,0.85,peak")` 成功
- [x] C42: T14 — `parse_price_point("P,2000,0.50,flat")` 成功（period=Flat）
- [x] C43: T15 — `parse_price_point("P,3000,0.30,valley")` 成功（period=Valley）
- [x] C44: T16 — period 大小写不敏感（PEAK/Peak 均成功）
- [x] C45: T17 — 前缀错误 `parse_price_point("X,...")` → `Err(ParseFailed)`
- [x] C46: T18 — 数字解析失败（time/price 非数字）→ `Err(ParseFailed)`
- [x] C47: T19 — 字段数不足 → `Err(ParseFailed)`
- [x] C48: T20 — period 未知 → `Err(ParseFailed)`
- [x] C49: T21 — `parse_dr_signal("D,42,2.5,1000,2000,500.0")` 成功（5 字段正确）
- [x] C50: T22 — 前缀错误 `parse_dr_signal("P,...")` → `Err(ParseFailed)`
- [x] C51: T23 — 数字解析失败 / 字段数不足 → `Err(ParseFailed)`
- [x] C52: T24 — `parse_feed` 混合行：2P + 1D + 1 非法 + 1 空行 → prices=2 / dr_signals=1 / market_type/timestamp 正确
- [x] C53: T25 — `parse_feed("", Spot, 0)` → 空，无 panic
- [x] C54: T26 — `parse_feed` 字段含空白 → trim 生效解析成功

## Task 5: subscriber.rs — 订阅管理
- [x] C55: `crates/agents/energy-market-agent/src/subscriber.rs` 文件创建
- [x] C56: `MarketFeedSource` trait 定义 `fn fetch(&mut self, now_ms: u64) -> Result<MarketFeed, MarketError>;`
- [x] C57: `MarketFeedSource` 不要求 `Send + Sync`（D8）
- [x] C58: `MockMarketFeedSource` 结构体字段 `next_feed: Option<MarketFeed>` / `fail: bool`，派生 `Debug, Clone, Default`
- [x] C59: `MockMarketFeedSource::new(feed)` / `new_failing()` / `with_feed(feed)` builder
- [x] C60: `impl MarketFeedSource for MockMarketFeedSource` — fail → `Err(SourceFailed)`；next_feed None → `Err(SourceFailed)`；否则 `Ok(next_feed.clone())`
- [x] C61: `MarketFeedPublisher` trait 定义 `publish_prices` + `publish_dr_signals`（D9）
- [x] C62: `MockMarketFeedPublisher` 结构体字段 `published: Vec<MarketFeed>` / `fail: bool`；`new()` / `new_failing()`
- [x] C63: `impl MarketFeedPublisher for MockMarketFeedPublisher` — fail → `Err(PublishFailed)`；否则 `published.push(feed.clone())` + `Ok(())`
- [x] C64: `MarketFeedCache` 结构体字段 `last: Option<MarketFeed>`；`new()` / `store(feed)` / `get() -> Option<&MarketFeed>` / `is_empty()`
- [x] C65: `MarketSubscriber` 结构体 6 字段（source / publisher / cache / subscribed / poll_interval_ms / last_poll_ms: Option<u64>）
- [x] C66: `MarketSubscriber::new(source, publisher, poll_interval_ms)` 初始化正确
- [x] C67: `subscribe(mt)` 幂等（重复订阅不重复添加）
- [x] C68: `is_subscribed(mt) -> bool` / `cache() -> &MarketFeedCache` 访问器
- [x] C69: `poll(now_ms)` 轮询门控（D11）：`last_poll_ms == Some(last)` 且 `now_ms - last < poll_interval_ms` → `Ok(None)`
- [x] C70: `poll` 设置 `last_poll_ms = Some(now_ms)`
- [x] C71: `poll` source 失败 + cache 有数据 → `Ok(Some(cached.clone()))`（§4.4 缓存降级）
- [x] C72: `poll` source 失败 + cache 空 → `Err(SourceFailed)`
- [x] C73: `poll` 成功 + 未订阅 `feed.market_type` → `Ok(None)`（不缓存不发布）
- [x] C74: `poll` 成功 + 已订阅 → cache.store + `Ok(Some(feed))`
- [x] C75: `poll` 成功 + `feed.prices` 非空 → `publish_prices`；失败 → `Err(PublishFailed)`
- [x] C76: `poll` 成功 + `feed.dr_signals` 非空 → `publish_dr_signals`；失败 → `Err(PublishFailed)`
- [x] C77: `subscriber.rs` 使用 `use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use crate::market_feed::{...}`
- [x] C78: `subscriber.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

## Task 6: subscriber.rs — 单元测试 T27~T42
- [x] C79: T27 — `MockMarketFeedSource::new(feed)` fetch → `Ok(feed)`
- [x] C80: T28 — `MockMarketFeedSource::new_failing()` fetch → `Err(SourceFailed)`
- [x] C81: T29 — `MockMarketFeedSource::default()`（next_feed None）fetch → `Err(SourceFailed)`
- [x] C82: T30 — `MockMarketFeedSource::with_feed(feed)` builder 生效
- [x] C83: T31 — `MockMarketFeedPublisher::new()` publish_prices 成功记录 1 条
- [x] C84: T32 — `MockMarketFeedPublisher::new_failing()` publish → `Err(PublishFailed)`，published 仍空
- [x] C85: T33 — `MarketFeedCache` store/get/is_empty 语义正确
- [x] C86: T34 — `MarketSubscriber::new(...)` 初始化（未订阅 / cache 空 / last_poll_ms None）
- [x] C87: T35 — `subscribe(Spot)` 生效 + 幂等
- [x] C88: T36 — 首次 `poll(0)` 立即 fetch → `Ok(Some(feed))` / cache 已存 / publisher 已记录
- [x] C89: T37 — 轮询门控：`poll(0)` 后 `poll(30_000)`（interval=60_000）→ `Ok(None)`
- [x] C90: T38 — 过期间隔：`poll(60_000)` → 重新 fetch
- [x] C91: T39 — 缓存降级：成功后 source fail → `Ok(Some(cached_feed))`
- [x] C92: T40 — 无缓存失败：恒 fail + 首次 poll → `Err(SourceFailed)`
- [x] C93: T41 — 未订阅过滤：DR feed + 仅订阅 Spot → `Ok(None)` / cache 不更新 / publisher 无记录
- [x] C94: T42 — 发布失败传播：publisher fail → `Err(PublishFailed)`

## Task 7: lib.rs surgical 修改
- [x] C95: `pub mod market_feed;` / `pub mod parser;` / `pub mod subscriber;` 追加
- [x] C96: `pub use market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};` 重导出
- [x] C97: `pub use parser::{parse_dr_signal, parse_feed, parse_price_point};` 重导出
- [x] C98: `pub use subscriber::{MarketFeedCache, MarketFeedPublisher, MarketFeedSource, MarketSubscriber, MockMarketFeedPublisher, MockMarketFeedSource};` 重导出
- [x] C99: 顶部模块文档注释追加 v0.85.0 类型说明 + D1~D14 偏差表（新增段落）
- [x] C100: v0.72.0 既有 5 个私有 `mod`（energy_agent/error/market/market_agent/runtime）保留不变
- [x] C101: v0.72.0 既有 5 行 `pub use` 保留不变
- [x] C102: v0.72.0 既有 24 个测试保留不变
- [x] C103: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 8: Cargo.toml description 更新
- [x] C104: `description` 字段更新为含 "v0.85.0 市场数据订阅" 字样
- [x] C105: `[dependencies]` 段不变（无新依赖）
- [x] C106: workspace members 列表不变

## Task 9: configs/market_source.toml
- [x] C107: 文件位于 `configs/market_source.toml`
- [x] C108: 含 `[market]` 段 + `subscribe_types` / `poll_interval_ms = 60000` 字段
- [x] C109: 含 `[source]` 段 + `kind = "simulated"` / `endpoint`（预留注释，D8）
- [x] C110: 含 `[publish]` 段 + `price_topic = "/power/market/price"` / `dr_topic = "/power/market/signal"`
- [x] C111: 含中文注释说明各字段用途（与 v0.83.0/v0.84.0 风格一致）

## Task 10: docs/agents/market-subscription-design.md
- [x] C112: 文件位于 `docs/agents/market-subscription-design.md`（非 `docs/phase2/`，D12 + 工作区规则 §2.3.3）
- [x] C113: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
- [x] C114: 至少 1 个 Mermaid 图（MarketSubscriber.poll 流程图：门控 → fetch → 缓存降级/订阅过滤 → 发布）
- [x] C115: 至少 1 个 Mermaid 图（蓝图 §4.3 数据流：市场接口 → 轮询 → 解析 → 发布 2 个 Topic）
- [x] C116: D1~D14 偏差声明表完整
- [x] C117: 引用 v0.72.0 Energy/Market Agent + v0.51.0 协议抽象作为前置依赖
- [x] C118: 包含性能目标说明（轮询 60s / 延迟 < 60s，标注"集成阶段验收，本版本仅算法骨架"）
- [x] C119: 引用 v0.86.0 报价生成（BidGenerator 消费 MarketFeed）作为下游消费者
- [x] C120: 包含选型对比表（REST API / 文件 / 专网直连，蓝图 §5.1）

## Task 11: 版本同步根目录文件
- [x] C121: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.85.0"`
- [x] C122: 根 `Cargo.toml` `[workspace.members]` 列表**不变**
- [x] C123: `Makefile` 中 `# Version: v0.85.0` 与 `VERSION := 0.85.0`
- [x] C124: `.github/workflows/ci.yml` 中 `# Version: v0.85.0`
- [x] C125: `ci/src/gate.rs` clippy 段注释含 `+ v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber`
- [x] C126: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 12: 构建校验（§2.4.2）
- [x] C127: `cargo metadata --format-version 1` 成功
- [x] C128: `cargo test -p eneros-energy-market-agent` 全部通过（v0.72.0 24 tests + v0.85.0 T1~T42 = 66 tests，0 failures）
- [x] C129: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C130: `cargo fmt -p eneros-energy-market-agent -- --check` 退出码 0
- [x] C131: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C132: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
- [x] C133: 回归 — `cargo test -p eneros-grid-agent` 仍通过 130 tests + 1 doctest（无回归）
- [x] C134: 回归 — `cargo test -p eneros-device-agent` 仍通过 24 tests（AgentRuntime trait 未变）
- [x] C135: 回归 — `cargo test -p eneros-tsn-time` 84 tests + `cargo test -p eneros-agent-bus-dds` 63 tests（无回归）

## 总体校验
- [x] C136: 无根目录新 crate（`crates/agents/energy-market-agent/` 既有 crate 追加 3 个新模块文件，符合 §2.3.1）
- [x] C137: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C138: 无 `config/` 目录（新配置在 `configs/market_source.toml`）
- [x] C139: `.gitignore` 未需更新（无新文件类型）
- [x] C140: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C141: 提交信息遵循 Conventional Commits（如 `feat(agents/energy-market-agent): v0.85.0 实现市场数据订阅`）
- [x] C142: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C143: no_std 合规性：3 个新文件继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C144: 内存预算：订阅模块 ≤ 1MB（本版本为算法骨架，实际占用远小于此）
- [x] C145: SBOM 未变化（无新第三方依赖，无新 workspace crate 依赖）
- [x] C146: 文档同步：v0.72.0 历史偏差声明保留，v0.85.0 新增 D1~D14 段落
- [x] C147: Surgical Changes 原则：v0.72.0 既有源文件 `energy_agent.rs` / `error.rs` / `market.rs` / `market_agent.rs` / `runtime.rs` 完全未改动
- [x] C148: `lib.rs` 仅追加 3 个 `pub mod` + 3 行 `pub use` + 顶部文档注释（不修改任何 v0.72.0 既有代码行）
- [x] C149: v0.72.0 `MarketData` / `MarketSignal` 命名不冲突（新类型命名 `MarketFeed`，D3）
