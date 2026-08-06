//! EnerOS v0.120.0 Metrics 指标采集（P2-J 可观测性第 1 版）.
//!
//! 建立统一的指标采集框架，覆盖系统层（CPU/内存/温度）、Agent 层（活跃数/
//! 延迟/错误率）、业务层（控制次数/优化调用数）三类指标，为运维监控、
//! 性能调优、容量规划提供量化数据基础，并为 v0.121.0（结构化日志）与
//! v0.122.0（分布式追踪）提供指标支撑。
//!
//! # 核心类型
//!
//! - [`MetricType`] / [`MetricValue`] / [`Label`] / [`Metric`] — 指标数据点
//!   模型（name≤64 字节、标签≤8 个、key≤32 字节、value≤64 字节）
//! - [`MetricDef`] / [`MetricRegistry`] — 指标注册项与注册表（容量 64，
//!   拒绝重复注册）
//! - [`MetricsCollector`] / [`MetricsCollectorImpl`] — 采集器：record/
//!   increment/query + 256 条内存环形缓冲 + 定时批量刷入
//! - [`MetricsStore`] / [`MemoryMetricsStore`] — 存储抽象与按名分桶的
//!   内存环形缓冲实现（写满丢弃最旧）
//!
//! # 偏差声明（相对蓝图）
//!
//! 1. **D1 查询返回类型**：蓝图 `query` 返回 `Vec<Metric>`（std），本实现
//!    返回 `alloc::vec::Vec<Metric>`（no_std 合规）。
//! 2. **D2 时间戳参数**：蓝图 `record`/`increment` 内部调用
//!    `crate::current_time_ms()` 取系统时钟，本实现改为 `now_ms: u64`
//!    参数传入（no_std 无系统时钟，与 v0.119.0 口径一致）。
//! 3. **D3 存储后端**：默认 `MemoryMetricsStore` 环形缓冲；TSDB（v0.25.0）
//!    持久化经 [`MetricsStore`] trait 预留对接，本版本不落地适配器。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 非测试代码仅使用 `core::*` / `alloc::*` / `heapless::*`，不调用
//! `panic!` / `todo!` / `unimplemented!` / `unwrap()`，可交叉编译到
//! `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod metrics;
pub mod registry;
pub mod store;

pub use metrics::{
    Label, Metric, MetricDef, MetricType, MetricValue, MetricsCollector, MetricsCollectorImpl,
    MetricsError,
};
pub use registry::MetricRegistry;
pub use store::{MemoryMetricsStore, MetricsStore};


#[cfg(all(test, feature = "loom"))]
pub mod loom_increment;
