//! Time-series data schema definitions.
//!
//! Defines the core data structures used throughout the TSDB:
//! [`TimeSeriesPoint`], [`TsdbConfig`], [`ColumnarChunk`], [`ChunkHeader`],
//! [`Query`], [`Aggregation`], and [`AggResult`].

use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// Newtype IDs
// ============================================================================

/// Unique identifier for a physical or logical device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DeviceId(pub u32);

/// Unique identifier for a metric (e.g. SOC, power, temperature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct MetricId(pub u32);

// ============================================================================
// Data Quality
// ============================================================================

/// Data quality flag (IEC 61850 style).
///
/// Encoded as `u8` for compact columnar storage:
/// `Good = 0`, `Uncertain = 1`, `Bad = 2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataQuality {
    Good = 0,
    Uncertain = 1,
    Bad = 2,
}

impl DataQuality {
    /// Converts the quality to its `u8` representation.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl From<u8> for DataQuality {
    fn from(value: u8) -> Self {
        match value {
            0 => DataQuality::Good,
            1 => DataQuality::Uncertain,
            _ => DataQuality::Bad,
        }
    }
}

// ============================================================================
// Time Series Point
// ============================================================================

/// A single time-series data point.
///
/// This is the fundamental unit written to the TSDB. Each point carries
/// a millisecond timestamp, device/metric IDs, a numeric value, and a
/// quality flag.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeSeriesPoint {
    /// Millisecond-level timestamp (Unix epoch).
    pub timestamp: u64,
    /// Device that produced the measurement.
    pub device_id: DeviceId,
    /// Metric identifier (e.g. SOC, power).
    pub metric: MetricId,
    /// Numeric measurement value.
    pub value: f64,
    /// Quality of the measurement.
    pub quality: DataQuality,
}

// ============================================================================
// Compression Type
// ============================================================================

/// Compression algorithm selector for chunk storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    /// No compression (pass-through).
    None,
    /// Snappy-compatible compression (currently backed by lz4_flex).
    Snappy,
}

// ============================================================================
// TSDB Configuration
// ============================================================================

/// Configuration for the time-series database.
///
/// # Defaults
///
/// | Field                 | Default       | Meaning                        |
/// |-----------------------|---------------|--------------------------------|
/// | `chunk_duration_ms`   | 3 600 000     | 1 hour per chunk               |
/// | `max_points_per_chunk`| 10 000        | Max points before flush        |
/// | `compression`         | `Snappy`      | LZ4 compression enabled        |
/// | `retention_ms`        | 2 592 000 000 | 30 days TTL                    |
/// | `flush_interval_ms`   | 5 000         | Flush every 5 s                |
#[derive(Debug, Clone, PartialEq)]
pub struct TsdbConfig {
    /// Root data directory on the filesystem.
    pub data_dir: String,
    /// Time span covered by a single chunk (milliseconds).
    pub chunk_duration_ms: u64,
    /// Maximum points buffered in a chunk before flush.
    pub max_points_per_chunk: u32,
    /// Compression algorithm for columnar data.
    pub compression: CompressionType,
    /// Data retention period (milliseconds). Older chunks are expired.
    pub retention_ms: u64,
    /// Interval between automatic flushes (milliseconds).
    pub flush_interval_ms: u64,
}

impl Default for TsdbConfig {
    fn default() -> Self {
        Self {
            data_dir: String::from("/tsdb"),
            chunk_duration_ms: 3_600_000,
            max_points_per_chunk: 10_000,
            compression: CompressionType::Snappy,
            retention_ms: 2_592_000_000,
            flush_interval_ms: 5_000,
        }
    }
}

// ============================================================================
// Chunk Header
// ============================================================================

/// Fixed-size header preceding each persisted chunk on disk.
///
/// Serialized as 32 little-endian bytes via [`ChunkHeader::to_bytes`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHeader {
    /// Device this chunk belongs to.
    pub device_id: DeviceId,
    /// Metric this chunk belongs to.
    pub metric: MetricId,
    /// Timestamp of the first point in the chunk.
    pub start_time: u64,
    /// Timestamp of the last point in the chunk.
    pub end_time: u64,
    /// Number of points stored in the chunk.
    pub point_count: u32,
    /// CRC32 checksum of the chunk payload.
    pub crc32: u32,
}

/// Serialized size of [`ChunkHeader`] in bytes.
pub const CHUNK_HEADER_SIZE: usize = 32;

impl ChunkHeader {
    /// Serializes the header to 32 little-endian bytes.
    pub fn to_bytes(&self) -> [u8; CHUNK_HEADER_SIZE] {
        let mut buf = [0u8; CHUNK_HEADER_SIZE];
        let mut off = 0;
        buf[off..off + 4].copy_from_slice(&self.device_id.0.to_le_bytes());
        off += 4;
        buf[off..off + 4].copy_from_slice(&self.metric.0.to_le_bytes());
        off += 4;
        buf[off..off + 8].copy_from_slice(&self.start_time.to_le_bytes());
        off += 8;
        buf[off..off + 8].copy_from_slice(&self.end_time.to_le_bytes());
        off += 8;
        buf[off..off + 4].copy_from_slice(&self.point_count.to_le_bytes());
        off += 4;
        buf[off..off + 4].copy_from_slice(&self.crc32.to_le_bytes());
        buf
    }

    /// Deserializes a header from a byte slice.
    ///
    /// Returns `None` if `data` is shorter than [`CHUNK_HEADER_SIZE`].
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < CHUNK_HEADER_SIZE {
            return None;
        }
        let mut off = 0;
        let device_id = DeviceId(u32::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
        ]));
        off += 4;
        let metric = MetricId(u32::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
        ]));
        off += 4;
        let start_time = u64::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
            data[off + 4],
            data[off + 5],
            data[off + 6],
            data[off + 7],
        ]);
        off += 8;
        let end_time = u64::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
            data[off + 4],
            data[off + 5],
            data[off + 6],
            data[off + 7],
        ]);
        off += 8;
        let point_count =
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        off += 4;
        let crc32 = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        Some(Self {
            device_id,
            metric,
            start_time,
            end_time,
            point_count,
            crc32,
        })
    }
}

// ============================================================================
// Columnar Chunk
// ============================================================================

/// In-memory columnar chunk buffering points before flush.
///
/// Each column (`timestamps`, `values`, `qualities`) is stored separately
/// so that column-specific compression can be applied during flush.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnarChunk {
    /// Chunk metadata header.
    pub header: ChunkHeader,
    /// Timestamp column (milliseconds).
    pub timestamps: Vec<u64>,
    /// Numeric value column.
    pub values: Vec<f64>,
    /// Quality column (u8-encoded).
    pub qualities: Vec<u8>,
    /// Pre-compressed payload (populated during flush).
    pub compressed: Vec<u8>,
}

impl ColumnarChunk {
    /// Creates a new empty chunk whose header is seeded from `point`.
    ///
    /// The point itself is **not** inserted; the caller is expected to
    /// push to the column vectors separately.
    pub fn new(point: &TimeSeriesPoint) -> Self {
        Self {
            header: ChunkHeader {
                device_id: point.device_id,
                metric: point.metric,
                start_time: point.timestamp,
                end_time: point.timestamp,
                point_count: 0,
                crc32: 0,
            },
            timestamps: Vec::new(),
            values: Vec::new(),
            qualities: Vec::new(),
            compressed: Vec::new(),
        }
    }
}

// ============================================================================
// Aggregation
// ============================================================================

/// Aggregation function applied to a query result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    Avg,
    Max,
    Min,
    Sum,
    Count,
}

/// Result of an aggregation query.
#[derive(Debug, Clone, PartialEq)]
pub struct AggResult {
    /// The aggregation that was applied.
    pub aggregation: Aggregation,
    /// Computed aggregate value (count is stored as f64 for uniformity).
    pub value: f64,
    /// Number of points contributing to the aggregate.
    pub count: u32,
}

// ============================================================================
// Query
// ============================================================================

/// Query specification for time-series retrieval.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// Devices to query (empty = all devices).
    pub device_ids: Vec<DeviceId>,
    /// Metrics to query (empty = all metrics).
    pub metrics: Vec<MetricId>,
    /// Inclusive time range [start, end] in milliseconds.
    pub time_range: (u64, u64),
    /// Optional aggregation to apply.
    pub aggregation: Option<Aggregation>,
    /// Maximum number of points to return.
    pub limit: Option<usize>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = TsdbConfig::default();
        assert_eq!(cfg.chunk_duration_ms, 3_600_000);
        assert_eq!(cfg.max_points_per_chunk, 10_000);
        assert_eq!(cfg.compression, CompressionType::Snappy);
        assert_eq!(cfg.retention_ms, 2_592_000_000);
        assert_eq!(cfg.flush_interval_ms, 5_000);
        assert_eq!(cfg.data_dir, "/tsdb");
    }

    #[test]
    fn test_data_quality_roundtrip() {
        assert_eq!(DataQuality::Good.as_u8(), 0);
        assert_eq!(DataQuality::Uncertain.as_u8(), 1);
        assert_eq!(DataQuality::Bad.as_u8(), 2);

        assert_eq!(DataQuality::from(0u8), DataQuality::Good);
        assert_eq!(DataQuality::from(1u8), DataQuality::Uncertain);
        assert_eq!(DataQuality::from(2u8), DataQuality::Bad);
        // Unknown values map to Bad.
        assert_eq!(DataQuality::from(255u8), DataQuality::Bad);
    }

    #[test]
    fn test_chunk_header_serialize_roundtrip() {
        let header = ChunkHeader {
            device_id: DeviceId(42),
            metric: MetricId(7),
            start_time: 1_000,
            end_time: 2_000,
            point_count: 500,
            crc32: 0xDEAD_BEEF,
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), CHUNK_HEADER_SIZE);
        let restored = ChunkHeader::from_bytes(&bytes).expect("roundtrip");
        assert_eq!(restored, header);
    }

    #[test]
    fn test_chunk_header_from_bytes_too_short() {
        let short = [0u8; 16];
        assert!(ChunkHeader::from_bytes(&short).is_none());
    }

    #[test]
    fn test_columnar_chunk_new() {
        let point = TimeSeriesPoint {
            timestamp: 1_000,
            device_id: DeviceId(1),
            metric: MetricId(2),
            value: 42.5,
            quality: DataQuality::Good,
        };
        let chunk = ColumnarChunk::new(&point);
        assert_eq!(chunk.header.device_id, DeviceId(1));
        assert_eq!(chunk.header.metric, MetricId(2));
        assert_eq!(chunk.header.start_time, 1_000);
        assert_eq!(chunk.header.end_time, 1_000);
        assert_eq!(chunk.header.point_count, 0);
        assert!(chunk.timestamps.is_empty());
        assert!(chunk.values.is_empty());
        assert!(chunk.qualities.is_empty());
        assert!(chunk.compressed.is_empty());
    }

    #[test]
    fn test_time_series_point_equality() {
        let p1 = TimeSeriesPoint {
            timestamp: 100,
            device_id: DeviceId(1),
            metric: MetricId(1),
            value: 1.0,
            quality: DataQuality::Good,
        };
        let p2 = p1.clone();
        assert_eq!(p1, p2);
    }
}
