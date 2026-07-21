//! # eneros-cloud-coordinator — v0.95.0 Cloud Coordinator 策略下发 + v0.96.0 数据汇聚
//!
//! ## 版本目标
//!
//! **v0.95.0**（蓝图 phase2，P2-D 第 4 版，云边协同起点）：实现 Cloud Coordinator
//! **策略下发**能力——云端生成全局策略（优化权重/电价预测/DR 响应/模型更新）→
//! 下发到边缘 → 边缘安全校验。**策略非强制、边缘主权保留**——违反安全约束的
//! 策略边缘可拒绝（[`validate_strategy`]）。
//!
//! **v0.96.0**（蓝图 phase2，P2-D 收尾）：实现 Cloud Coordinator **数据汇聚**
//! 能力——收集域内多个 Edge Box 状态（复用 v0.93.0 `EdgeBoxState`）→ 汇总为
//! [`DomainData`]（轻量级快照 + metrics）→ 通过 [`DataSink`] trait 存储 →
//! 支撑全局分析与审计，为 v0.112.0 云端孪生主节点提供数据基础。
//!
//! ```text
//! Edge Box ×N                    Cloud Coordinator
//!      │ 上报状态                      │ DataAggregator
//!      │──── EdgeBoxState ────────────▶│ collect → DomainData
//!      │                              │ store → DataSink
//! ```
//!
//! ```text
//! Cloud Coordinator                    Edge Coordinator
//!      │ 生成全局策略                        │
//!      │──── 下发策略(非强制) ───────────────▶│ 安全校验
//!      │◀─── Ack(accepted / rejected) ──────│
//! ```
//!
//! ## 核心类型清单
//!
//! | 模块 | 导出 | 说明 |
//! |------|------|------|
//! | [`strategy`] | [`Strategy`] / [`StrategyContent`] / [`ModelRef`] / [`EdgeAck`] / [`RejectReason`] / [`LocalState`] / [`validate_strategy`] + 常量 [`SAFETY_WEIGHT_MIN`] / [`DEFAULT_ACK_TIMEOUT_MS`] / [`DEFAULT_MAX_RETRIES`] | 策略数据结构（4 变体）与边缘安全校验（蓝图 §4.5 落地） |
//! | [`channel`] | [`CloudChannel`] / [`CloudError`] / [`MockCloudChannel`] | 云边通道抽象（sync trait）与故障注入 Mock |
//! | [`publisher`] | [`StrategyPublisher`] | 策略发布器（超时重试 + pending 断网补发 + 4 个 pub 可观测计数器） |
//! | [`aggregator`] | [`DomainData`] / [`EventRecord`] / [`EventType`] / [`Severity`] / [`DataSource`] / [`DataSink`] / [`DataAggregator`] / [`AggError`] + [`MockDataSource`] / [`MockDataSink`] | 数据汇聚（collect 多源容错 + store 委托 + 3 个 pub 可观测计数器）与故障注入 Mock |
//!
//! `Objective` / `PricePoint` / `DrSignal` 复用 `eneros-energy-market-agent`，
//! `Priority` 复用 `eneros-coordinator`（D5，不重复定义）；
//! `EdgeBoxState` 复用 `eneros-coordinator`（v0.93.0 已导出，v0.96.0 D6 不重复定义）。
//!
//! ## v0.95.0 偏差记录（D1~D12）
//!
//! - **D1**：crate 位于 `crates/agents/cloud-coordinator/`（蓝图 `crates/cloud_coordinator/`
//!   → 项目 §2.3.1 硬规则，Agent 实现归 agents 子系统；文档归 `docs/agents/`）。
//! - **D2**：`strategy_id` / `targets` / `edge_id` 全部 `u64` / `Vec<u64>`
//!   （蓝图 String → 无堆字符串 + 确定性，v0.87.0 D3 / v0.94.0 D2 惯例）。
//! - **D3**：sync `publish` / `collect_acks`（蓝图 async → no_std 无 async runtime，
//!   v0.93.0 D5 惯例）；`timeout_ms: u64` 参数注入（默认常量 10_000，语义等价
//!   蓝图 `Duration::from_secs(10)`）。
//! - **D4**：`OptimizationWeights(BTreeMap<Objective, f32>)`（蓝图 HashMap → no_std
//!   alloc 无 HashMap；`Objective` 已 derive Ord（v0.88.0 核实）；BTreeMap 确定性
//!   迭代可重放）。
//! - **D5**：`priority: Priority` 复用 `eneros-coordinator::Priority`（v0.92.0，
//!   派生 Ord 序即优先级序；§5.5 防重复造轮子，不重新定义）。
//! - **D6**：蓝图未定义 `ModelRef` / `LocalState` / `CloudError` → MVP 最小定义：
//!   `ModelRef { model_id: u64, version: u32 }`、`LocalState { edge_id: u64,
//!   max_capacity_mw: f32 }`、`CloudError { BroadcastFailed }`（单变体，重试耗尽
//!   即广播失败）。
//! - **D7**：`EdgeAck.reason: Option<RejectReason>`（蓝图 `Option<String>` → 结构化
//!   无堆字符串，机读审计；`RejectReason` 2 变体 `SafetyWeightTooLow` /
//!   `ExceedsCapacity`，与蓝图关键代码一致）。
//! - **D8**：`CloudChannel` 本地 sync trait `{ broadcast, collect_acks }` +
//!   [`MockCloudChannel`]（v0.86.0 D11 BidPublisher 模式；Socket v0.29.0 / DDS
//!   适配器后续注入，不在本版本）。
//! - **D9**：[`StrategyPublisher`] 4 个 pub 计数器 `published_count` / `ack_count` /
//!   `reject_count` / `retry_count` + `pending: Vec<Strategy>` 待补发队列与
//!   `republish_pending() -> u32`（蓝图 §9 Ack/拒绝 metric + §6.5 断网重连补发）。
//! - **D10**：蓝图硬编码 `0.5` 安全门限 → 命名常量 [`SAFETY_WEIGHT_MIN`]；
//!   safety weight **缺失或 NaN 按 0.0 → 拒绝**（安全侧默认拒绝，宁拒勿放）。
//! - **D11**：测试 crate 内嵌 `#[cfg(test)]` 40 个（蓝图 `tests/strategy_push.rs` →
//!   v0.87.0~v0.94.0 项目惯例，不新增 tests/ 文件；集成场景以 Mock 故障注入覆盖）。
//! - **D12**：NaN 防御（v0.88.0 C140 / v0.94.0 D12 教训）：weight 非有限 → 0.0
//!   （触发安全拒绝）；DR `target_mw` 非有限 → `ExceedsCapacity`；
//!   `max_capacity_mw` 非有限或 ≤0 → 一切 DR 策略拒绝（安全侧）。
//!
//! ## v0.96.0 偏差记录（D1~D12）
//!
//! - **D1**：复用既有 crate `crates/agents/cloud-coordinator/` 追加 `src/aggregator.rs`
//!   （蓝图 `crates/cloud_coordinator/src/{aggregator,storage,schema}.rs` → §2.3.1 硬规则 + P2-D 同一 crate 连续追加惯例）。
//! - **D2**：`domain_id` / metrics 键全部 `u64`（蓝图 String → 无堆字符串 + 确定性，v0.95.0 D2 惯例）。
//! - **D3**：sync `collect` / `store`（蓝图 async → no_std 无 async runtime，v0.95.0 D3 惯例）；`now_ms: u64` 参数注入；`run` 不实现（无 ticker，集成阶段由调用方循环驱动）。
//! - **D4**：metrics `BTreeMap<u64, f32>`（蓝图 HashMap → no_std alloc 无 HashMap；BTreeMap 确定性迭代可重放）。
//! - **D5**：`DataSink` 为 sync trait + [`MockDataSink`]（蓝图 `DataSink { Tsdb, File, S3 }` 枚举 → §5.5 防重复造轮子，本版本仅定义接口与 Mock，真实存储后续注入 `Box<dyn DataSink>`）。
//! - **D6**：`EdgeBoxState` 复用 `eneros-coordinator`（v0.93.0 已导出；§5.5 防重复造轮子，不重复定义）。
//! - **D7**：不用 warn! 宏（no_std 无 log crate）；数据源失败通过 [`AggError::SourceFailed`] + `timeout_count` 计数器暴露可观测。
//! - **D8**：测试 crate 内嵌 `#[cfg(test)]` 40 个（蓝图 `tests/data_agg.rs` → v0.87.0~v0.95.0 项目惯例）。
//! - **D9**：NaN 防御：metric 值非有限 → 存入前 sanitize 为 0.0；数据量计数独立 u64（`collect_count`/`timeout_count`/`store_count`）不依赖 metric。
//! - **D10**：本版本不做压缩（蓝图 §5.4 → no_std 无标准压缩库）；仅保留 EdgeBoxState 快照轻量汇总，压缩列入后续版本评估。
//! - **D11**：统一 u64 ms UTC epoch 时间戳（`now_ms` 外部注入），不涉及时区转换（蓝图 §8.5）。
//! - **D12**：仅定义脱敏标记字段 `is_sensitive: bool`（默认 false）；脱敏执行逻辑后续 v0.101.0 断网处理实现（蓝图 §7.3）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod aggregator;
pub mod channel;
pub mod publisher;
pub mod strategy;

pub use aggregator::{
    AggError, DataAggregator, DataSink, DataSource, DomainData, EventRecord, EventType,
    MockDataSink, MockDataSource, Severity,
};
pub use channel::{CloudChannel, CloudError, MockCloudChannel};
pub use publisher::StrategyPublisher;
pub use strategy::{
    validate_strategy, EdgeAck, LocalState, ModelRef, RejectReason, Strategy, StrategyContent,
    DEFAULT_ACK_TIMEOUT_MS, DEFAULT_MAX_RETRIES, SAFETY_WEIGHT_MIN,
};
