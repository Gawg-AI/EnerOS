//! EnerOS v0.121.0 结构化日志（P2-J 可观测性第 2 版）.
//!
//! 实现 JSON 格式的结构化日志系统，支持五级分级（DEBUG/INFO/WARN/ERROR/
//! FATAL）、内存缓冲批量持久化与按字段查询检索，为故障诊断提供可检索日志
//! （满足等保审计），并为 v0.122.0 分布式追踪提供 trace_id 关联。
//!
//! # 核心类型
//!
//! - [`LogLevel`] / [`LogEntry`] / [`LogError`] — 日志级别（Debug=0..
//!   Fatal=4）、日志条目（target≤64 字节、message≤256 字节、fields≤8 个
//!   键值对）与错误模型
//! - [`LogBuilder`] — 流畅构建 API（field 超 8 个截断）
//! - [`to_json`] — 序列化为 JSON（字段 ts/level/target/msg/fields/
//!   trace_id/span_id/agent_id）
//! - [`LogFilter`] — 七条件可选过滤器（AND 语义）
//! - [`StructuredLogger`] / [`StructuredLoggerImpl`] — 分级记录 + 128 条
//!   内存缓冲 + 批量刷入
//! - [`LogStorage`] / [`MemoryLogStorage`] — 存储抽象与容量可配的环形
//!   存储实现（写满轮转丢弃最旧）
//!
//! # 偏差声明（相对蓝图）
//!
//! 1. **D1 返回类型**：蓝图 `to_json`/`query` 返回 std 的 `String`/`Vec`，
//!    本实现返回 `alloc::string::String` / `alloc::vec::Vec`（no_std 合规）。
//! 2. **D2 时间戳参数**：蓝图 `LogBuilder::new` 内部调用
//!    `crate::current_time_ms()` 取系统时钟，本实现改为 `now_ms: u64`
//!    参数传入（no_std 无系统时钟，与 v0.120.0 口径一致）。
//! 3. **D3 存储签名**：蓝图 `LogStorage::append(&[u8])` 仅存字节无法支撑
//!    `query` 返回 `LogEntry`，本实现调整为
//!    `append(entry: &LogEntry, json: &[u8])`。
//! 4. **D4 指标关联**：与 v0.120.0 指标关联在集成测试层验证
//!    （dev-dependency eneros-metrics），库本体不依赖 metrics crate。
//! 5. **实现期补充**：JSON 输出补充 `span_id` 字段（蓝图遗漏）；对
//!    target/message/field 键值中的 `"` 与 `\` 做转义，防止注入破坏
//!    JSON 结构（蓝图未处理）。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 非测试代码仅使用 `core::*` / `alloc::*` / `heapless::*`，不调用
//! `panic!` / `todo!` / `unimplemented!` / `unwrap()`，可交叉编译到
//! `aarch64-unknown-none`。

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]

extern crate alloc;

pub mod log_filter;
pub mod log_query;
pub mod logging;
#[cfg(feature = "std")]
pub mod file_store;

pub use log_filter::LogFilter;
pub use log_query::MemoryLogStorage;
pub use logging::{
    to_json, LogBuilder, LogEntry, LogError, LogLevel, LogStorage, StructuredLogger,
    StructuredLoggerImpl,
};
#[cfg(feature = "std")]
pub use file_store::FileStorage;
