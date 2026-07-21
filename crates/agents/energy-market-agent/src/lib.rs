//! EnerOS v0.72.0 Energy Agent + Market Agent.
//!
//! Phase 1 P1-L MVP 集成第一层：实现 Energy Agent（能源调度核心）和 Market Agent
//! （市场数据接收），作为 MVP 场景的两个核心 Agent。Energy Agent 编排 v0.71.0 双脑
//! 协调器执行储能调度，Market Agent 接收外部电价/负荷预测并通过 Agent 间通道传递给
//! Energy Agent。双 Agent 协作构成 MVP 端到端集成的业务核心。
//!
//! # 核心类型
//!
//! - [`EnergyAgent`] — 能源调度 Agent，持有 `DualBrainCoordinator<MockSolver>`
//! - [`MarketAgent`] — 市场数据 Agent，从 `MarketDataSource` 接收并转发
//! - [`AgentRuntime`] — Agent 运行时生命周期 trait（on_start/on_tick/on_stop/on_heartbeat）
//! - [`HeartbeatStatus`] — 心跳状态（Alive/Dead）
//! - [`AgentRuntimeError`] — Agent 运行时错误
//! - [`MarketData`] / [`MarketSignal`] — 市场数据结构
//! - [`MarketChannel`] — Agent 间通信通道（Vec-backed 非阻塞）
//! - [`MarketDataSource`] / [`MockMarketSource`] — 市场数据源抽象
//!
//! # 偏差声明（D1~D14，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `log::info!` / `log::warn!` / `log::error!` | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57/v0.64/v0.70/v0.71 一致 |
//! | **D2** | `SystemTime::now()` / `UNIX_EPOCH` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57/v0.64/v0.70/v0.71 一致 |
//! | **D3** | `uuid::Uuid::new_v4().to_string()` | `AgentId::generate()` (v0.33.0) | no_std 无 uuid crate；复用 v0.33.0 原子计数器 ID 生成 |
//! | **D4** | `ChannelReceiver<MarketData>` / `ChannelSender<MarketData>` | 本地 `MarketChannel` (Vec-backed) | `ChannelReceiver`/`ChannelSender` 不存在；本地简单实现保持 crate 自包含可测试（与 v0.71.0 D6 一致） |
//! | **D5** | `TcpConnection::connect(market_server)` / `recv_nonblocking()` | 本地 `MarketDataSource` trait + `MockMarketSource` | `TcpConnection` 不存在；v0.29.0 socket 抽象复杂，MVP 用 Mock 即可 |
//! | **D6** | `impl AgentRuntime for EnergyAgent` | 本地定义 `AgentRuntime` trait | v0.33.0 `AgentEntry` trait 语义不同（on_init/on_start/on_stop + AgentContext，无 on_tick/on_heartbeat）；本地 trait 匹配蓝图运行时语义 |
//! | **D7** | `AgentDescriptor { id, agent_type, priority, capabilities: vec!["control.write"], trust_level, ..Default::default() }` | `AgentDescriptor::new(AgentType::Energy, name, now_ms)` | v0.33.0 `AgentDescriptor` 13 字段 + 构造器 `new(type, name, now)` 自动设置优先级/配额/信任等级；蓝图 `..Default::default()` 与 `capabilities: Vec<&str>` 类型不匹配（实际 `Vec<CapabilityRef>`） |
//! | **D8** | `HeartbeatStatus::Alive` / `HeartbeatStatus::Dead` | 本地定义 `HeartbeatStatus` 枚举（Alive/Dead） | v0.33.0 `HealthStatus` 4 级（Healthy/Degraded/Unhealthy/Dead）语义不同；蓝图 2 级（Alive/Dead）更简单 |
//! | **D9** | `DualBrainCoordinator::new(config)` | `DualBrainCoordinator::new(config, llm_engine, solver, sink)` | v0.71.0 实际构造器需 4 参数 |
//! | **D10** | `self.coordinator.execute(&state)` | `self.coordinator.execute(&state, now_ms)` | v0.71.0 `execute` 需 `now_ms: u64` 参数（D1 一致） |
//! | **D11** | 蓝图 `SystemState` 含 `soc`/`current_power`/`current_price`/`current_period`/`device_status`/`alarms`/`load_demand` | 构建 `RealtimeState`（v0.70.0）传入 `execute` | v0.67.0 `SystemState` 仅含电气字段；v0.70.0 `RealtimeState` 包装 `SystemState` + `current_price` + `load_demand`；Energy Agent 从缓存/默认值构建 |
//! | **D12** | 两个 crate：`energy-agent` + `market-agent` | 一个 crate：`eneros-energy-market-agent` | Karpathy 简化原则：两 Agent 共享 `MarketData`/`MarketChannel` 类型，单 crate 避免跨 crate 类型共享；与 v0.71.0 单 crate 多模块一致 |
//! | **D13** | `serde_json::from_slice(&data)` | `serde_json::from_slice`（alloc 支持） | no_std + alloc 下 `serde_json` 可用；Mock source 直接返回 `MarketData` 无需序列化 |
//! | **D14** | `activate_safe_default()` 构造 `ControlCommand` 功率归零 | `state = AgentState::Error` 状态标记 | v0.22.0 `ControlCommand` 字段差异大（`cmd_id: [u8;16]`/`DeviceId`/`setpoint: f32`）；功率归零下发由 v0.73.0 Device Agent 实现；本版本仅标记错误状态 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。
//!
//! # v0.85.0 市场数据订阅
//!
//! 扩展本 crate 新增 3 个模块，实现现货 / 辅助服务 / 需求响应（DR）市场数据订阅，
//! 为 v0.86.0 报价生成提供数据输入。Surgical 追加：v0.72.0 既有公共 API 全部保留。
//!
//! ## 核心类型（v0.85.0）
//!
//! - `market_feed` 模块 — [`MarketType`]（Spot/AncillaryService/DemandResponse，默认 Spot）、[`Period`]（Peak/Flat/Valley，默认 Flat）、[`PricePoint`]（time/price/period，Copy）、[`DrSignal`]（event_id/target_mw/start/end/reward，Copy）、[`MarketFeed`]（market_type/timestamp/prices/dr_signals）、[`MarketError`]（SourceFailed/ParseFailed/PublishFailed）
//! - `parser` 模块 — [`parse_price_point`]（`P,<time>,<price>,<period>`）、[`parse_dr_signal`]（`D,<event_id>,<target_mw>,<start>,<end>,<reward>`）、[`parse_feed`]（多行解析，格式错误行跳过，蓝图 §4.4）
//! - `subscriber` 模块 — [`MarketFeedSource`] / [`MockMarketFeedSource`] 数据源抽象（沿用 v0.82.0 GridSampler 模式）、[`MarketFeedPublisher`] / [`MockMarketFeedPublisher`] 发布抽象（替代蓝图 DdsNode）、[`MarketFeedCache`] last-good 缓存（接口超时降级）、[`MarketSubscriber`] 订阅 + `poll_interval_ms` 门控轮询 + 缓存降级 + 发布
//!
//! ## v0.85.0 偏差声明（D1~D14）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `async fn subscribe/poll/run` + `interval(Duration::from_secs(60))` 循环 | sync `poll(&mut self, now_ms) -> Result<Option<MarketFeed>, MarketError>` + `poll_interval_ms` 门控 | no_std 无 async runtime / 无 `Instant` / 无 `Duration`；沿用 v0.82.0 D3/D4 + v0.83.0 D1 sync 模式；外部调度器驱动 tick |
//! | **D2** | 新 crate `crates/agents/market_agent/` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已将 Energy+Market 合并为单 crate；新建 market_agent crate 会重复 `MarketAgent` 概念；surgical — 沿用 v0.83/v0.84 扩展既有 crate 模式 |
//! | **D3** | `MarketData { market_type, timestamp, prices, dr_signals }` | 命名 `MarketFeed`（文件 `market_feed.rs`） | v0.72.0 已存在 `MarketData`（字段 price_forecast/current_price/signal_type，形状不同）；改名避免 BREAKING 既有 API 与类型混淆 |
//! | **D4** | `DrSignal.event_id: String` | `event_id: u64` | no_std 无堆 String；Copy 语义使 `DrSignal` 可 derive Copy；与 v0.83.0 D2（pcc_id: u32）一致 |
//! | **D5** | 交付物列表含 `PriceSignal`，§4.1 定义 `PricePoint` | 采用 `PricePoint`（§4.1 数据结构为准） | 蓝图内部命名不一致；§4.1 为权威定义；`PriceSignal` 视为 `PricePoint` 的交付物别名 |
//! | **D6** | `Period` 未定义（`PricePoint.period: Period` 引用） | 定义 `Period` 枚举（`Peak`/`Flat`/`Valley`，默认 `Flat`） | 蓝图引用未定义类型；3 变体对应电力市场峰/平/谷时段 |
//! | **D7** | `MarketError` 引用但未定义 | 3 变体：`SourceFailed` / `ParseFailed` / `PublishFailed` | MVP 收敛错误分类；与 v0.82.0 D10 `GridError` 3 变体一致 |
//! | **D8** | `MarketSource { HttpApi(String), File(String), Simulated }` 枚举 | `MarketFeedSource` trait + `MockMarketFeedSource` | no_std 无 HTTP/文件系统；trait 抽象数据源（沿用 v0.82.0 D5 `GridSampler` 模式）；真实 HTTP/File 适配器后续注入 |
//! | **D9** | `run(&mut self, bus: &DdsNode)` + `dds::publish` | `MarketFeedPublisher` trait + `MockMarketFeedPublisher` | 避免 `eneros-agent-bus-dds` 重依赖（沿用 v0.82.0 D5/D12 `GridPublisher` 模式）；DDS 适配器后续注入 |
//! | **D10** | `MarketCache` 引用但未定义 | `MarketFeedCache` 结构体（`last: Option<MarketFeed>` + store/get/is_empty） | 蓝图 §4.4 "接口超时 → 使用缓存"需要缓存语义；最小实现：单条 last-good |
//! | **D11** | 轮询周期 60s（§6.3） | `poll_interval_ms: u64` 构造参数 + `last_poll_ms: Option<u64>` 门控 | 60s 作为推荐默认值（configs/market_source.toml）；`Option<u64>` 使首次 poll 立即执行 |
//! | **D12** | `docs/phase2/market_agent.md` + `config/market_source.toml` | `docs/agents/market-subscription-design.md` + `configs/market_source.toml` | 工作区规则 §2.3.3 禁止 `docs/phase2/` 平面化；工作区使用 `configs/` 而非 `config/` |
//! | **D13** | `tests/market_parse.rs` 集成测试 | 各新文件内 `#[cfg(test)] mod tests` 单元测试 | 沿用 v0.82.0/v0.83.0/v0.84.0 内嵌测试模式 |
//! | **D14** | v0.72.0 `MarketData` 派生 `serde` | 新类型不派生 `serde` | 解析器为手写文本行解析（`core::str`），不引入 `serde_json`；新类型由 parser 直接产出、crate 内消费，无需序列化往返 |
//!
//! # v0.86.0 报价生成
//!
//! 扩展本 crate 新增 `bid_generator` 模块：消费 v0.85.0 [`MarketFeed`]（电价点列表），
//! 结合储能 SOC/容量与 [`BidStrategy`]，经「意图 → 优化 → 生成 → 发布」流水线产出
//! `Vec<Bid>`；意图/优化两级失败分别回退规则策略与保守报价（蓝图 §4.4）。
//! Surgical 追加：v0.72.0 + v0.85.0 既有公共 API 全部保留。
//!
//! ## 核心类型（v0.86.0）
//!
//! - [`BidSide`]（Buy/Sell，默认 Buy）、[`Bid`]（8 字段：bid_id/market_type/resource_id/price/quantity/side/period/timestamp，Copy）、[`BidStrategy`]（margin/max_quantity/soc_threshold）
//! - [`BidIntent`] / [`BidOptimization`] 中间结构（D7 MVP 最小字段）
//! - [`BidError`]（InvalidInput/IntentFailed/OptimizeFailed/PublishFailed，D8）
//! - [`BidIntentSource`] / [`BidOptimizer`] / [`BidPublisher`] trait + 3 个 Mock（D5/D6/D11 本地 trait 抽象，真实 LLM/Solver/DDS 适配器后续注入）
//! - [`BidGenerator`]（6 字段；`generate(feed, soc, capacity, now_ms) -> Result<Vec<Bid>, BidError>`）
//! - [`rule_intent`] / [`conservative_optimize`] 两级确定性回退自由函数（D9/D10）
//!
//! ## v0.86.0 偏差声明（D1~D14）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `pub async fn generate(&self, market, soc, capacity)` | sync `fn generate(&mut self, feed, soc, capacity, now_ms)` | no_std 无 async runtime；`&mut` 因 `next_bid_id` 递增 + Mock 状态；`now_ms` 参数注入（沿用 v0.82~v0.85 D1/D2） |
//! | **D2** | 新文件于 `crates/agents/market_agent/` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已合并 Energy+Market 单 crate；新建会重复 MarketAgent 概念（沿用 v0.85.0 D2） |
//! | **D3** | `market: &MarketData` | `feed: &MarketFeed` | v0.85.0 D3 命名延续；同 crate 直接复用，零适配层 |
//! | **D4** | `bid_id: String` / `resource_id: String` | `u64` / `u64` | no_std 无堆 String；`Bid` 保持 Copy；`next_bid_id` 原子递增（沿用 v0.85.0 D4） |
//! | **D5** | `llm: Arc<dyn LlmEngine>` | 本地 `BidIntentSource` trait + `MockBidIntentSource` | 避免 eneros-llm-engine 重依赖（沿用 v0.85.0 D8 模式）；真实 LLM 适配器后续注入；Arc 需要原子+线程语义，单线程用 Box |
//! | **D6** | `solver: Arc<dyn Solver>` | 本地 `BidOptimizer` trait + `MockBidOptimizer` | 同上，避免 eneros-solver-core 重依赖 |
//! | **D7** | `generate_bid_intent` / `solve_bid` / `into_bids` 未定义 | `BidIntent{side,target_quantity}` / `BidOptimization{price_adjust,quantity}` 中间结构 + generate 内联映射 | 蓝图引用未定义方法；MVP 最小字段，禁止投机字段（Simplicity First） |
//! | **D8** | `BidError` 引用未定义 | 4 变体：`InvalidInput` / `IntentFailed` / `OptimizeFailed` / `PublishFailed` | MVP 错误分类；与 v0.85.0 D7 `MarketError` 3 变体风格一致 |
//! | **D9** | §4.4 "LLM 输出非法 → 回退到规则策略" | `rule_intent()` 自由函数 + generate 内失败回退 | 规则确定性：Sell + `min(max_quantity, capacity*soc)`；测试可复现 |
//! | **D10** | §4.4 "Solver 不可用 → 使用保守报价" | `conservative_optimize()` 自由函数 + 回退 | 保守确定性：`price_adjust=margin`，`quantity=min(target, max_quantity)` |
//! | **D11** | §4.3 "发布 /power/market/bid"（`dds::publish`） | `BidPublisher` trait + `MockBidPublisher` | 避免 eneros-agent-bus-dds 重依赖（沿用 v0.85.0 D9 `MarketFeedPublisher` 模式）；DDS 适配器后续注入 |
//! | **D12** | `docs/phase2/bid_generation.md` + `tests/bid_strategy.rs` | `docs/agents/bid-generation-design.md` + 文件内 `#[cfg(test)] mod tests` | 工作区规则 §2.3.3 禁止 docs/phase2 平面化；内嵌测试沿用 v0.82~v0.85 模式 |
//! | **D13** | `let bids = opt.into_bids(&self.strategy)`（生成数量未定义） | 对 `feed.prices` 每 `PricePoint` 生成 1 条 `Bid`（period 级报价） | 蓝图未定义列表长度；per-period 报价为电力市场标准做法；DR 信号 MVP 不报价（`prices` 空 → `Ok([])`) |
//! | **D14** | 报价价格/量算法未定义 | Sell: `point.price + price_adjust`；Buy: `(point.price - price_adjust).max(0.0)`；quantity: `min(opt.quantity, max_quantity, capacity).max(0.0)` | §7.3 "报价不超容量上限"具体化；Buy 价 floor 0 防负价；全确定性可测试 |
//!
//! # v0.87.0 多设备调度
//!
//! 扩展本 crate 新增 2 个模块，实现多设备（储能+光伏+充电桩）功率分配：
//! 构建 LP（容量/爬坡/平衡约束，损耗最小目标）→ Solver 求解 → 失败回退
//! `equal_split` 平均分配兜底。Surgical 追加：v0.72.0 + v0.85.0 + v0.86.0
//! 既有公共 API 全部保留。
//!
//! ## 核心类型（v0.87.0）
//!
//! - `device_pool` 模块 — [`DeviceMode`]（Auto/Manual，默认 Auto）、[`DeviceCapability`]（p_min/p_max/ramp_rate/efficiency，Copy）、[`DevicePool`]（`BTreeMap<u64, DeviceCapability>` 有序设备池，D3/D4/D7）
//! - `multi_dispatch` 模块 — [`DeviceAssignment`]（device_id/setpoint/mode，Copy）、[`DispatchPlan`]（timestamp/assignments/total_power/objective_value）、[`DispatchError`]（EmptyPool/InvalidTarget，D8）、[`equal_split`]（平均分配兜底自由函数）、[`MultiDeviceDispatcher`]（`Box<dyn Solver>` 直接复用 v0.64.0 eneros-solver-core trait，D5；`dispatch(target, socs, now_ms)`，D1/D11）
//!
//! ## v0.87.0 偏差声明
//!
//! 详见 `device_pool.rs`（D3/D4/D7）与 `multi_dispatch.rs`（D1/D5/D6/D8~D14）文件头表格。
//!
//! # v0.88.0 多目标优化
//!
//! 扩展本 crate 新增 `multi_objective` 模块：在 v0.87.0 单目标（损耗最小）LP 调度基础上
//! 扩展为经济 / 电池寿命 / 安全 / 碳排 4 目标优化——各目标成本系数归一化后按权重线性
//! 组合为单一 LP 目标求解（`weighted` 加权和），或经确定性权重采样生成多个调度方案、
//! 评估各目标取值并过滤被支配解得到 Pareto 前沿（`pareto`）。复用 v0.87.0 设备池与
//! 调度语义，Solver 失败一律回退 `equal_split` 平均分配兜底。Surgical 追加：
//! v0.72.0 + v0.85.0 + v0.86.0 + v0.87.0 既有公共 API 全部保留。
//!
//! ## 核心类型（v0.88.0）
//!
//! - [`Objective`]（Economy/BatteryLife/Safety/Carbon 4 目标枚举，默认 Economy）
//! - [`WeightedSum`]（`BTreeMap<Objective, f32>` 权重表，set/get/normalized）
//! - [`ParetoSolution`]（plan + objectives 单解）、[`ParetoFront`]（solutions 前沿集合）
//! - [`MultiObjectiveOptimizer`]（`Box<dyn Solver>` + `DevicePool`；`weighted(target, socs, now_ms)` / `pareto(target, socs, samples, now_ms)`）
//! - [`objective_costs`] / [`normalize_costs`] / [`generate_weight_sample`] / [`filter_dominated`] / [`eval_plan_objectives`] 自由函数
//!
//! ## v0.88.0 偏差声明（D1~D14）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 扩展既有 `energy-market-agent` crate（沿用 v0.85~v0.87 D2，不新建 crate） |
//! | **D2** | `BTreeMap` 替代 `HashMap`（no_std 合规 + 迭代有序确定性） |
//! | **D3** | `Box` 替代 `Arc`（单线程语义，沿用 v0.87.0 D5） |
//! | **D4** | `weighted` 补 `target/socs/now_ms` 参数（与 v0.87.0 `dispatch` 签名语义一致） |
//! | **D5** | 复用 v0.87.0 `DispatchError`（EmptyPool/InvalidTarget），不新增错误类型 |
//! | **D6** | 直接构建 `LpProblem` CSR（沿用 v0.87.0 D6，不经 solver-model 建模层） |
//! | **D7** | 模块内私有 `build_weighted_lp`（LP 构建不暴露公共 API） |
//! | **D8** | 四目标成本系数确定性定义（`objective_costs` 固定系数，可测试复现） |
//! | **D9** | max 归一化（各目标成本 ÷ 该目标最大值，`normalize_costs`） |
//! | **D10** | 非法权重 → 均权各 0.25（权重和为 0/非法时回退） |
//! | **D11** | 确定性权重采样公式（`generate_weight_sample`，非随机） |
//! | **D12** | `docs/agents/` 文档路径 + 文件内 `#[cfg(test)]` 内嵌测试（沿用 v0.85~v0.87 D12/D13） |
//! | **D13** | `eval_plan_objectives` 返回原始值；`samples=0` → 空 front |
//! | **D14** | O(n²) 支配过滤，完全相同向量保留先者（非随机 MOEA） |

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod bid_generator;
pub mod device_pool;
mod energy_agent;
mod error;
mod market;
mod market_agent;
pub mod market_feed;
pub mod multi_dispatch;
pub mod multi_objective;
pub mod parser;
mod runtime;
pub mod subscriber;

pub use bid_generator::{
    conservative_optimize, rule_intent, Bid, BidError, BidGenerator, BidIntent, BidIntentSource,
    BidOptimization, BidOptimizer, BidPublisher, BidSide, BidStrategy, MockBidIntentSource,
    MockBidOptimizer, MockBidPublisher,
};
pub use device_pool::{DeviceCapability, DeviceMode, DevicePool};
pub use energy_agent::EnergyAgent;
pub use error::AgentRuntimeError;
pub use market::{MarketChannel, MarketData, MarketDataSource, MarketSignal, MockMarketSource};
pub use market_agent::MarketAgent;
pub use market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};
pub use multi_dispatch::{
    equal_split, DeviceAssignment, DispatchError, DispatchPlan, MultiDeviceDispatcher,
};
pub use multi_objective::{
    eval_plan_objectives, filter_dominated, generate_weight_sample, normalize_costs,
    objective_costs, MultiObjectiveOptimizer, Objective, ParetoFront, ParetoSolution, WeightedSum,
};
pub use parser::{parse_dr_signal, parse_feed, parse_price_point};
pub use runtime::{AgentRuntime, HeartbeatStatus};
pub use subscriber::{
    MarketFeedCache, MarketFeedPublisher, MarketFeedSource, MarketSubscriber,
    MockMarketFeedPublisher, MockMarketFeedSource,
};

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_agent::{AgentState, AgentType};
    use eneros_dual_brain::coordinator::DualBrainMockEngine;
    use eneros_dual_brain::{CommandSink, DispatchCommand, DualBrainCoordinator, DualBrainError};
    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_solver_core::mock::MockSolver;

    use super::*;

    /// 辅助：构造 96 时段电价预测.
    fn make_price_forecast() -> Vec<f64> {
        vec![0.5; 96]
    }

    /// 辅助：构造一条 MarketData.
    fn make_market_data(price: f64) -> MarketData {
        MarketData {
            timestamp: 1000,
            price_forecast: make_price_forecast(),
            current_price: price,
            load_forecast: None,
            signal_type: MarketSignal::RealtimePrice,
        }
    }

    // ===== T1: MarketData 构造（96 时段）=====
    #[test]
    fn t1_market_data_construction() {
        let data = make_market_data(0.5);
        assert_eq!(data.price_forecast.len(), 96);
        assert!((data.current_price - 0.5).abs() < 1e-9);
        assert_eq!(data.timestamp, 1000);
        assert!(data.load_forecast.is_none());
        assert!(matches!(data.signal_type, MarketSignal::RealtimePrice));
    }

    // ===== T2: MarketSignal 变体构造 =====
    #[test]
    fn t2_market_signal_variants() {
        let _ = MarketSignal::RealtimePrice;
        let _ = MarketSignal::DayAheadForecast;
        let _ = MarketSignal::DemandResponse;
        let _ = MarketSignal::EmergencyDispatch;
        assert!(matches!(
            MarketSignal::RealtimePrice,
            MarketSignal::RealtimePrice
        ));
        assert!(matches!(
            MarketSignal::DayAheadForecast,
            MarketSignal::DayAheadForecast
        ));
        assert!(matches!(
            MarketSignal::DemandResponse,
            MarketSignal::DemandResponse
        ));
        assert!(matches!(
            MarketSignal::EmergencyDispatch,
            MarketSignal::EmergencyDispatch
        ));
    }

    // ===== T3: MarketChannel::new 空 =====
    #[test]
    fn t3_market_channel_new_empty() {
        let ch = MarketChannel::new(16);
        assert!(ch.is_empty());
        assert_eq!(ch.len(), 0);
    }

    // ===== T4: MarketChannel::send + try_recv 成功 =====
    #[test]
    fn t4_market_channel_send_and_recv() {
        let mut ch = MarketChannel::new(16);
        let data = make_market_data(0.5);
        ch.send(data).unwrap();
        assert_eq!(ch.len(), 1);
        assert!(!ch.is_empty());
        let received = ch.try_recv();
        assert!(received.is_some());
        assert_eq!(received.unwrap().current_price, 0.5);
        assert_eq!(ch.len(), 0);
        assert!(ch.is_empty());
    }

    // ===== T5: MarketChannel 缓冲满丢弃旧数据 =====
    #[test]
    fn t5_market_channel_overflow_drops_oldest() {
        let mut ch = MarketChannel::new(2);
        ch.send(make_market_data(1.0)).unwrap();
        ch.send(make_market_data(2.0)).unwrap();
        assert_eq!(ch.len(), 2);
        // 第三条：满时丢弃最旧（1.0）
        ch.send(make_market_data(3.0)).unwrap();
        assert_eq!(ch.len(), 2);
        // 最旧被丢弃，保留 2.0 和 3.0
        let first = ch.try_recv().unwrap();
        assert!((first.current_price - 2.0).abs() < 1e-9);
        let second = ch.try_recv().unwrap();
        assert!((second.current_price - 3.0).abs() < 1e-9);
    }

    // ===== T6: MarketChannel 空时 try_recv 返回 None =====
    #[test]
    fn t6_market_channel_empty_recv_none() {
        let mut ch = MarketChannel::new(16);
        assert!(ch.try_recv().is_none());
    }

    // ===== T7: MockMarketSource::new 空 =====
    #[test]
    fn t7_mock_market_source_new_empty() {
        let mut src = MockMarketSource::new();
        let result = src.recv_nonblocking().unwrap();
        assert!(result.is_none());
    }

    // ===== T8: MockMarketSource::with_data 预加载 =====
    #[test]
    fn t8_mock_market_source_with_data() {
        let data = vec![make_market_data(0.5), make_market_data(0.6)];
        let mut src = MockMarketSource::with_data(data);
        assert!(src.recv_nonblocking().unwrap().is_some());
        assert!(src.recv_nonblocking().unwrap().is_some());
        assert!(src.recv_nonblocking().unwrap().is_none());
    }

    // ===== T9: MockMarketSource::recv_nonblocking 有数据 =====
    #[test]
    fn t9_mock_market_source_recv_with_data() {
        let mut src = MockMarketSource::new();
        src.push(make_market_data(0.42));
        let result = src.recv_nonblocking().unwrap();
        assert!(result.is_some());
        assert!((result.unwrap().current_price - 0.42).abs() < 1e-9);
    }

    // ===== T10: MockMarketSource::recv_nonblocking 空返回 None =====
    #[test]
    fn t10_mock_market_source_recv_empty_none() {
        let mut src = MockMarketSource::new();
        let result = src.recv_nonblocking().unwrap();
        assert!(result.is_none());
    }

    // ===== T11: HeartbeatStatus::Alive / Dead =====
    #[test]
    fn t11_heartbeat_status() {
        assert_eq!(HeartbeatStatus::Alive, HeartbeatStatus::Alive);
        assert_eq!(HeartbeatStatus::Dead, HeartbeatStatus::Dead);
        assert_ne!(HeartbeatStatus::Alive, HeartbeatStatus::Dead);
    }

    // ===== T12: AgentRuntimeError 变体构造 =====
    #[test]
    fn t12_agent_runtime_error_variants() {
        let _ = AgentRuntimeError::DualBrainError(DualBrainError::LlmError(String::from("llm")));
        let _ = AgentRuntimeError::ChannelError(String::from("channel"));
        let _ = AgentRuntimeError::MarketDataError(String::from("market"));
        let _ = AgentRuntimeError::AgentError(eneros_agent::AgentError::InvalidDescriptor);
        let _ = AgentRuntimeError::NotRunning;
    }

    // ===== T13: EnergyAgent::new_default 构造 =====
    #[test]
    fn t13_energy_agent_new_default() {
        let agent = EnergyAgent::new_default(1000);
        assert_eq!(agent.descriptor.agent_type, AgentType::Energy);
        assert_eq!(agent.state, AgentState::Created);
        assert!(agent.current_schedule.is_none());
        assert_eq!(agent.tick_count, 0);
        assert!((agent.current_price - 0.5).abs() < 1e-9);
        assert!(agent.market_channel.is_empty());
    }

    // ===== T14: EnergyAgent::on_start 状态转 Running =====
    #[test]
    fn t14_energy_agent_on_start() {
        let mut agent = EnergyAgent::new_default(0);
        agent.on_start(1000).unwrap();
        assert_eq!(agent.state, AgentState::Running);
    }

    // ===== T15: EnergyAgent::on_tick 执行双脑（慢路径）=====
    #[test]
    fn t15_energy_agent_on_tick_slow_path() {
        let mut agent = EnergyAgent::new_default(0);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        assert!(agent.current_schedule.is_some());
        assert_eq!(agent.tick_count, 1);
    }

    // ===== T16: EnergyAgent::on_tick 接收市场数据 =====
    #[test]
    fn t16_energy_agent_on_tick_receive_market_data() {
        let mut agent = EnergyAgent::new_default(0);
        agent.on_start(1000).unwrap();
        // 注入市场数据
        agent
            .market_channel_mut()
            .send(make_market_data(0.88))
            .unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        // current_price 应更新为市场数据的 current_price
        assert!((agent.current_price - 0.88).abs() < 1e-9);
    }

    // ===== T17: EnergyAgent::on_stop 状态转 Dead =====
    #[test]
    fn t17_energy_agent_on_stop() {
        let mut agent = EnergyAgent::new_default(0);
        agent.on_start(1000).unwrap();
        agent.on_stop(2000).unwrap();
        assert_eq!(agent.state, AgentState::Dead);
    }

    // ===== T18: EnergyAgent::on_heartbeat Running → Alive =====
    #[test]
    fn t18_energy_agent_heartbeat_alive() {
        let mut agent = EnergyAgent::new_default(0);
        agent.on_start(1000).unwrap();
        assert_eq!(agent.on_heartbeat(2000), HeartbeatStatus::Alive);
    }

    // ===== T19: EnergyAgent::on_heartbeat 非 Running → Dead =====
    #[test]
    fn t19_energy_agent_heartbeat_dead() {
        let agent = EnergyAgent::new_default(0);
        // Created 状态 → Dead
        assert_eq!(agent.on_heartbeat(1000), HeartbeatStatus::Dead);
    }

    // ===== T20: MarketAgent::new_default 构造 =====
    #[test]
    fn t20_market_agent_new_default() {
        let agent = MarketAgent::new_default(1000);
        assert_eq!(agent.descriptor.agent_type, AgentType::Market);
        assert_eq!(agent.state, AgentState::Created);
        assert_eq!(agent.price_cache.len(), 96);
        assert_eq!(agent.tick_count, 0);
        assert!(agent.market_channel.is_empty());
    }

    // ===== T21: MarketAgent::on_tick 接收并转发数据 =====
    #[test]
    fn t21_market_agent_on_tick_with_data() {
        let source: Box<dyn MarketDataSource> =
            Box::new(MockMarketSource::with_data(vec![make_market_data(0.77)]));
        let mut agent = MarketAgent::new("market", source, 0);
        agent.on_start(1000).unwrap();
        agent.on_tick(2000).unwrap();
        // price_cache 更新为市场数据
        assert_eq!(agent.price_cache.len(), 96);
        assert!((agent.price_cache[0] - 0.5).abs() < 1e-9);
        // market_channel 含 1 条数据
        assert_eq!(agent.market_channel.len(), 1);
        assert_eq!(agent.tick_count, 1);
    }

    // ===== T22: MarketAgent::on_tick 无数据正常返回 =====
    #[test]
    fn t22_market_agent_on_tick_no_data() {
        let mut agent = MarketAgent::new_default(0);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        // 无数据时 price_cache 不变（仍为初始 0.5）
        assert!((agent.price_cache[0] - 0.5).abs() < 1e-9);
        assert!(agent.market_channel.is_empty());
        assert_eq!(agent.tick_count, 1);
    }

    // ===== T23: 双 Agent 协作：MarketAgent send → EnergyAgent 接收 =====
    #[test]
    fn t23_two_agent_collaboration() {
        // 1. Market Agent 从 source 接收数据并转发到自己的 channel
        let source: Box<dyn MarketDataSource> =
            Box::new(MockMarketSource::with_data(vec![make_market_data(0.33)]));
        let mut market = MarketAgent::new("market", source, 0);
        market.on_start(1000).unwrap();
        market.on_tick(2000).unwrap();
        assert_eq!(market.market_channel.len(), 1);

        // 2. 从 Market Agent 的 channel 取出数据，注入 Energy Agent 的 channel
        let data = market.market_channel_mut().try_recv().unwrap();
        assert!((data.current_price - 0.33).abs() < 1e-9);

        let mut energy = EnergyAgent::new_default(0);
        energy.on_start(1000).unwrap();
        energy.market_channel_mut().send(data).unwrap();

        // 3. Energy Agent 接收市场数据并执行双脑
        let result = energy.on_tick(3000);
        assert!(result.is_ok());
        assert!((energy.current_price - 0.33).abs() < 1e-9);
        assert!(energy.current_schedule.is_some());
    }

    // ===== T24: EnergyAgent 双脑失败降级（state = Error）=====
    #[test]
    fn t24_energy_agent_dual_brain_failure_degrade() {
        // 构造一个总是失败的 CommandSink，使 coordinator.execute 在命令下发步骤失败
        struct FailingSink;
        impl CommandSink for FailingSink {
            fn write(&mut self, _cmd: DispatchCommand) -> Result<(), DualBrainError> {
                Err(DualBrainError::DispatchError(String::from("test failure")))
            }
        }

        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let sink: Box<dyn CommandSink> = Box::new(FailingSink);
        let coordinator = DualBrainCoordinator::new(config, llm_engine, solver, sink);

        let mut agent = EnergyAgent {
            descriptor: eneros_agent::AgentDescriptor::new(AgentType::Energy, "test", 0),
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            tick_count: 0,
        };

        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_err());
        assert_eq!(agent.state, AgentState::Error);
    }
}
