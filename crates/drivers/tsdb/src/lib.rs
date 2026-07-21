//! EnerOS Time-Series Database Engine (v0.25.0)
//!
//! Columnar time-series storage engine built on the v0.24.0 [`FileSystem`]
//! trait. Provides efficient storage and querying for four-remote telemetry
//! (四遥) data, SOE events, and device state history in EnerOS.
//!
//! The engine buffers writes in per-(device, metric) columnar chunks, then
//! flushes them to disk as compressed columnar files. Timestamps use
//! Delta-of-delta encoding (≈50:1 ratio for uniform sampling); value and
//! quality columns are compressed via the [`Compressor`] abstraction
//! (currently backed by `lz4_flex`). A `BTreeMap`-based [`TimeIndex`]
//! locates chunk files by time range, and [`retention`] enforces TTL-based
//! expiry.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  Caller (v0.52.0 四遥 / v0.53.0 SOE)          │
//! └─────────────┬────────────────────────────────┘
//!               │  TimeSeriesDB API (write/query/aggregate)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-tsdb::TimeSeriesDB (this crate)       │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  Writer (Delta-of-delta + LZ4 压缩)     │  │
//! │  │  Reader (范围查询 + 聚合)               │  │
//! │  │  Index (BTreeMap 时间索引)             │  │
//! │  │  Retention (TTL 过期清理)              │  │
//! │  └────────────────────────────────────────┘  │
//! └─────────────┬────────────────────────────────┘
//!               │  FileSystem trait (open/create/remove/mkdir)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-fs::Lfs (v0.24.0, littlefs2)          │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  BlockDeviceStorage adapter            │  │
//! │  └────────────────────────────────────────┘  │
//! └─────────────┬────────────────────────────────┘
//!               │  read_block / write_block / erase_block
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-storage::BlockDevice (v0.23.0)        │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_tsdb::{TimeSeriesDB, TsdbConfig, TimeSeriesPoint, DeviceId, MetricId, DataQuality};
//! use eneros_fs::Lfs;
//! use eneros_storage::{BlockDevice, MockBlockDevice};
//! use alloc::boxed::Box;
//!
//! let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
//! let fs = Lfs::format(dev).expect("format failed");
//! let config = TsdbConfig::default();
//! let mut tsdb = TimeSeriesDB::open(fs, config).expect("open failed");
//!
//! let point = TimeSeriesPoint {
//!     timestamp: 1000,
//!     device_id: DeviceId(1),
//!     metric: MetricId(1),
//!     value: 42.0,
//!     quality: DataQuality::Good,
//! };
//! tsdb.write(&point).expect("write failed");
//!
//! let results = tsdb.query_range(DeviceId(1), MetricId(1), 0, 2000)
//!     .expect("query failed");
//! assert_eq!(results.len(), 1);
//! ```
//!
//! # Design Decisions
//!
//! - **完整版 TSDB**（用户决策）：6 模块 + 列式存储 + Delta-of-delta + LZ4 压缩
//!   + 完整聚合。虽蓝图 §42.4 标注为中度过度设计，但保留完整能力避免 v0.52.0
//!     四遥数据落地时重构。
//! - **LZ4 替代 Snappy**：`snap` crate 不兼容 no_std（依赖 `std::io`），改用
//!   `lz4_flex`（纯 Rust，no_std 友好）。[`Compressor`] trait 抽象确保压缩后端
//!   可替换，上层 writer/reader 不感知后端变化。`CompressionType::Snappy` 变体名
//!   保留以维持配置兼容性，实际由 `lz4_flex` 实现。
//! - **Delta-of-delta 编码**：等间隔时间戳压缩率 50:1+，非等间隔序列仍能正确
//!   编解码（仅压缩率下降），适合储能场景的周期性采样。
//! - **BTreeMap 时间索引**：使用 `alloc::collections::BTreeMap<u64, Vec<IndexEntry>>`
//!   按起始时间排序，no_std 友好，无需自研 B+ 树，确定性迭代顺序便于持久化。
//! - **TSDB 持有 `Lfs` 具体类型**：`eneros-fs::File::read/write` 签名要求
//!   `&mut Lfs`（具体类型，非 trait object），故 [`TimeSeriesDB`] 持有 `Lfs` 实例
//!   而非 `Box<dyn FileSystem>`，静态分发，无虚调用开销。
//!
//! [`FileSystem`]: eneros_fs::FileSystem
//! [`Compressor`]: crate::compression::Compressor
//! [`TimeIndex`]: crate::index::TimeIndex
//! [`retention`]: crate::retention
//! [`TimeSeriesDB`]: crate::db::TimeSeriesDB

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod compression;
pub mod db;
pub mod error;
pub mod index;
pub mod reader;
pub mod retention;
pub mod schema;
pub mod writer;

// Re-export the most commonly used types at the crate root.
pub use compression::{make_compressor, Compressor, NoopCompressor, SnappyCompressor};
pub use db::TimeSeriesDB;
pub use error::TsdbError;
pub use index::{IndexEntry, TimeIndex};
pub use reader::{aggregate, read_last, read_range, TsdbReader, TsdbReaderImpl};
pub use retention::{cleanup_expired, should_expire};
pub use schema::{
    AggResult, Aggregation, ChunkHeader, ColumnarChunk, CompressionType, DataQuality, DeviceId,
    MetricId, Query, TimeSeriesPoint, TsdbConfig, CHUNK_HEADER_SIZE,
};
pub use writer::{
    append_point, flush_all_chunks, flush_chunk, make_chunk_path, TsdbWriter, TsdbWriterImpl,
};

/// The crate version string.
pub const VERSION: &str = "0.25.0";
