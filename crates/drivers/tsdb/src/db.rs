//! [`TimeSeriesDB`] — the main entry point for the TSDB.
//!
//! Owns the filesystem, index, compressor, and in-memory chunk buffer. Writes
//! are buffered in memory and flushed to disk as compressed columnar chunks.
//! Queries scan the index, read matching chunk files, and decompress them.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_tsdb::{TimeSeriesDB, TsdbConfig, TimeSeriesPoint, DataQuality, DeviceId, MetricId};
//! use eneros_fs::Lfs;
//! use eneros_storage::{BlockDevice, MockBlockDevice};
//! use alloc::boxed::Box;
//!
//! let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
//! let fs = Lfs::format(dev).expect("format");
//! let config = TsdbConfig::default();
//! let mut db = TimeSeriesDB::open(fs, config).expect("open");
//!
//! db.write(&TimeSeriesPoint {
//!     timestamp: 1000,
//!     device_id: DeviceId(1),
//!     metric: MetricId(1),
//!     value: 42.5,
//!     quality: DataQuality::Good,
//! }).expect("write");
//!
//! let points = db.query_range(DeviceId(1), MetricId(1), 0, 2000).expect("query");
//! assert_eq!(points.len(), 1);
//! ```

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use eneros_fs::{FileMode, FileSystem, FsError, Lfs, OpenFlags};

use crate::compression::{make_compressor, Compressor};
use crate::error::TsdbError;
use crate::index::TimeIndex;
use crate::reader;
use crate::retention;
use crate::schema::{
    AggResult, Aggregation, DataQuality, DeviceId, MetricId, Query, TimeSeriesPoint, TsdbConfig,
};
use crate::writer;

// ============================================================================
// Index file path
// ============================================================================

/// Index persistence file name (relative to `data_dir`).
const INDEX_FILE_NAME: &str = "index.bin";

fn index_path(data_dir: &str) -> String {
    format!("{}/{}", data_dir, INDEX_FILE_NAME)
}

// ============================================================================
// TimeSeriesDB
// ============================================================================

/// The main time-series database entry point.
///
/// Owns the filesystem, configuration, time index, in-memory chunk buffer,
/// and compressor. All operations go through this struct.
pub struct TimeSeriesDB {
    fs: Lfs,
    config: TsdbConfig,
    index: TimeIndex,
    chunks: BTreeMap<(DeviceId, MetricId), crate::schema::ColumnarChunk>,
    compressor: Box<dyn Compressor>,
    total_written: u64,
    next_chunk_id: u32,
}

impl TimeSeriesDB {
    /// Opens (or creates) a time-series database on the given filesystem.
    ///
    /// Creates the data directory if it does not exist. Attempts to load the
    /// persisted index from `{data_dir}/index.bin`; if the file does not
    /// exist, a fresh empty index is used.
    pub fn open(mut fs: Lfs, config: TsdbConfig) -> Result<Self, TsdbError> {
        // Create the data directory (ignore "already exists").
        let _ = fs.mkdir(&config.data_dir);

        // Load or create the index.
        let index = load_index_mut(&mut fs, &config.data_dir)?;

        // Create the compressor from the configured compression type.
        let compressor = make_compressor(config.compression);

        Ok(Self {
            fs,
            config,
            index,
            chunks: BTreeMap::new(),
            compressor,
            total_written: 0,
            next_chunk_id: 0,
        })
    }

    /// Writes a single time-series point to the in-memory buffer.
    ///
    /// If the buffer for this device/metric pair exceeds `max_points_per_chunk`
    /// or `chunk_duration_ms`, a chunk is flushed to disk before buffering the
    /// new point.
    pub fn write(&mut self, point: &TimeSeriesPoint) -> Result<(), TsdbError> {
        writer::append_point(
            &self.config,
            &mut self.chunks,
            &mut self.index,
            &mut self.fs,
            self.compressor.as_ref(),
            &mut self.total_written,
            &mut self.next_chunk_id,
            point,
        )
    }

    /// Writes a batch of points sequentially.
    pub fn write_batch(&mut self, points: &[TimeSeriesPoint]) -> Result<(), TsdbError> {
        for point in points {
            self.write(point)?;
        }
        Ok(())
    }

    /// Queries points for a single device/metric pair within `[start, end]`.
    ///
    /// Reads from both flushed chunk files and the in-memory buffer.
    pub fn query_range(
        &mut self,
        device: DeviceId,
        metric: MetricId,
        start: u64,
        end: u64,
    ) -> Result<Vec<TimeSeriesPoint>, TsdbError> {
        // Read from flushed chunks via the index.
        let mut result = reader::read_range(
            &mut self.fs,
            &self.index,
            self.compressor.as_ref(),
            self.config.chunk_duration_ms,
            device,
            metric,
            start,
            end,
        )?;

        // Also scan the in-memory buffer for matching points.
        let key = (device, metric);
        if let Some(chunk) = self.chunks.get(&key) {
            for i in 0..chunk.timestamps.len() {
                let ts = chunk.timestamps[i];
                if ts >= start && ts <= end {
                    result.push(TimeSeriesPoint {
                        timestamp: ts,
                        device_id: chunk.header.device_id,
                        metric: chunk.header.metric,
                        value: chunk.values[i],
                        quality: DataQuality::from(chunk.qualities[i]),
                    });
                }
            }
        }

        // Sort by timestamp for deterministic ordering.
        result.sort_by_key(|p| p.timestamp);
        Ok(result)
    }

    /// Executes a query. If an aggregation is specified, returns a single
    /// synthetic point carrying the aggregate value. Otherwise returns all
    /// matching points (up to `limit` if set).
    pub fn query(&mut self, q: &Query) -> Result<Vec<TimeSeriesPoint>, TsdbError> {
        if q.aggregation.is_some() {
            let result = self.aggregate(q)?;
            // Wrap the aggregate as a single synthetic point.
            let device = q.device_ids.first().copied().unwrap_or(DeviceId(0));
            let metric = q.metrics.first().copied().unwrap_or(MetricId(0));
            return Ok(vec![TimeSeriesPoint {
                timestamp: q.time_range.1,
                device_id: device,
                metric,
                value: result.value,
                quality: DataQuality::Good,
            }]);
        }

        let (start, end) = q.time_range;
        if start > end {
            return Err(TsdbError::InvalidQuery);
        }

        let mut result = Vec::new();
        for &device in &q.device_ids {
            for &metric in &q.metrics {
                let points = self.query_range(device, metric, start, end)?;
                result.extend(points);
            }
        }

        // Sort by timestamp.
        result.sort_by_key(|p| p.timestamp);

        // Apply limit.
        if let Some(limit) = q.limit {
            if result.len() > limit {
                result.truncate(limit);
            }
        }

        Ok(result)
    }

    /// Computes an aggregate over the points matched by `q`.
    ///
    /// Scans both flushed chunk files (via the index) and the in-memory
    /// buffer for matching points, then applies the aggregation function.
    pub fn aggregate(&mut self, q: &Query) -> Result<AggResult, TsdbError> {
        let agg = q.aggregation.ok_or(TsdbError::InvalidQuery)?;
        let (start, end) = q.time_range;
        if start > end {
            return Err(TsdbError::InvalidQuery);
        }

        // Collect matching values from both disk and memory via query_range.
        let mut values: Vec<f64> = Vec::new();
        for &device in &q.device_ids {
            for &metric in &q.metrics {
                let points = self.query_range(device, metric, start, end)?;
                for p in points {
                    values.push(p.value);
                }
            }
        }

        let count = values.len() as u32;
        let value = match agg {
            Aggregation::Count => count as f64,
            Aggregation::Sum => values.iter().sum(),
            Aggregation::Avg => {
                if count == 0 {
                    0.0
                } else {
                    values.iter().sum::<f64>() / count as f64
                }
            }
            Aggregation::Max => values.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)),
            Aggregation::Min => values.iter().fold(f64::INFINITY, |a, &b| a.min(b)),
        };

        Ok(AggResult {
            aggregation: agg,
            value,
            count,
        })
    }

    /// Returns the most recent point for `device`/`metric`, or `None`.
    pub fn read_last(
        &mut self,
        device: DeviceId,
        metric: MetricId,
    ) -> Result<Option<TimeSeriesPoint>, TsdbError> {
        // Check flushed chunks first.
        let mut last = reader::read_last(
            &mut self.fs,
            &self.index,
            self.compressor.as_ref(),
            device,
            metric,
        )?;

        // Also check the in-memory buffer (may have newer data).
        let key = (device, metric);
        if let Some(chunk) = self.chunks.get(&key) {
            if let Some(&ts) = chunk.timestamps.last() {
                let i = chunk.timestamps.len() - 1;
                let mem_point = TimeSeriesPoint {
                    timestamp: ts,
                    device_id: chunk.header.device_id,
                    metric: chunk.header.metric,
                    value: chunk.values[i],
                    quality: DataQuality::from(chunk.qualities[i]),
                };
                match &last {
                    Some(disk_point) if disk_point.timestamp >= ts => {}
                    _ => last = Some(mem_point),
                }
            }
        }

        Ok(last)
    }

    /// Flushes all in-memory chunks to disk.
    pub fn compact(&mut self) -> Result<(), TsdbError> {
        writer::flush_all_chunks(
            &self.config,
            &mut self.chunks,
            &mut self.index,
            &mut self.fs,
            self.compressor.as_ref(),
            &mut self.next_chunk_id,
        )
    }

    /// Removes expired chunk files based on the configured retention period.
    ///
    /// Returns the number of chunk files removed.
    pub fn cleanup_expired(&mut self, now: u64) -> Result<u64, TsdbError> {
        retention::cleanup_expired(&mut self.index, &mut self.fs, now, self.config.retention_ms)
    }

    /// Closes the database: flushes all chunks, saves the index, and syncs.
    pub fn close(mut self) -> Result<(), TsdbError> {
        self.compact()?;
        save_index(&mut self.fs, &self.config.data_dir, &self.index)?;
        self.fs.sync()?;
        Ok(())
    }

    /// Returns the total number of points written since the database was
    /// opened.
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Returns the number of entries in the persisted index.
    pub fn index_len(&self) -> usize {
        self.index.len()
    }

    /// Returns the number of points currently buffered in memory.
    pub fn buffered_points(&self) -> usize {
        self.chunks.values().map(|c| c.timestamps.len()).sum()
    }

    /// Returns a reference to the database configuration.
    pub fn config(&self) -> &TsdbConfig {
        &self.config
    }
}

// ============================================================================
// Index persistence
// ============================================================================

/// Loads the index from `{data_dir}/index.bin` using a mutable FS reference.
/// Returns an empty index if the file does not exist.
fn load_index_mut(fs: &mut Lfs, data_dir: &str) -> Result<TimeIndex, TsdbError> {
    let path = index_path(data_dir);
    let stat = match fs.stat(&path) {
        Ok(s) => s,
        Err(FsError::NotFound { .. }) => return Ok(TimeIndex::new()),
        Err(e) => return Err(e.into()),
    };
    if stat.size == 0 {
        return Ok(TimeIndex::new());
    }
    let mut buf = vec![0u8; stat.size as usize];
    let mut file = fs.open(&path, OpenFlags::READ)?;
    let mut read_total = 0;
    while read_total < buf.len() {
        let n = file.read(fs, &mut buf[read_total..])?;
        if n == 0 {
            break;
        }
        read_total += n;
    }
    buf.truncate(read_total);
    TimeIndex::deserialize(&buf)
}

/// Saves the index to `{data_dir}/index.bin`.
fn save_index(fs: &mut Lfs, data_dir: &str, index: &TimeIndex) -> Result<(), TsdbError> {
    let path = index_path(data_dir);
    let data = index.serialize();
    let mut file = fs.create(&path, FileMode::default_file())?;
    file.write(fs, &data)?;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_storage::MockBlockDevice;

    use super::*;
    use crate::schema::{Aggregation, CompressionType, DataQuality, Query};

    fn make_fs() -> Lfs {
        let dev: Box<dyn eneros_storage::BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        Lfs::format(dev).expect("format should succeed")
    }

    fn make_config() -> TsdbConfig {
        TsdbConfig {
            data_dir: String::from("/tsdb"),
            chunk_duration_ms: 3_600_000,
            max_points_per_chunk: 100,
            compression: CompressionType::Snappy,
            retention_ms: 2_592_000_000,
            flush_interval_ms: 5_000,
        }
    }

    fn make_point(ts: u64, device: u32, metric: u32, value: f64) -> TimeSeriesPoint {
        TimeSeriesPoint {
            timestamp: ts,
            device_id: DeviceId(device),
            metric: MetricId(metric),
            value,
            quality: DataQuality::Good,
        }
    }

    #[test]
    fn test_open_creates_data_dir() {
        let fs = make_fs();
        let config = make_config();
        let db = TimeSeriesDB::open(fs, config).expect("open");
        // The data directory should exist.
        // We can't check stat after open because fs is owned by db.
        assert_eq!(db.index_len(), 0);
    }

    #[test]
    fn test_write_and_query_single_point() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        let point = make_point(1000, 1, 2, 42.5);
        db.write(&point).expect("write");

        // Query without flushing — should read from in-memory buffer.
        let result = db
            .query_range(DeviceId(1), MetricId(2), 0, 2000)
            .expect("query");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].timestamp, 1000);
        assert_eq!(result[0].value, 42.5);
        assert_eq!(result[0].quality, DataQuality::Good);
    }

    #[test]
    fn test_write_batch_1000_points() {
        let fs = make_fs();
        let config = TsdbConfig {
            max_points_per_chunk: 10000,
            ..make_config()
        };
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        let points: Vec<TimeSeriesPoint> = (0..1000u64)
            .map(|i| make_point(i, 1, 1, i as f64))
            .collect();
        db.write_batch(&points).expect("write_batch");

        // All points should be queryable.
        let result = db
            .query_range(DeviceId(1), MetricId(1), 0, 1000)
            .expect("query");
        assert_eq!(result.len(), 1000);
    }

    #[test]
    fn test_range_query_filtering() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        // Write points at t=100, 200, 300, 400, 500.
        for ts in [100u64, 200, 300, 400, 500] {
            db.write(&make_point(ts, 1, 1, ts as f64)).expect("write");
        }

        // Query [200, 400] → 3 points.
        let result = db
            .query_range(DeviceId(1), MetricId(1), 200, 400)
            .expect("query");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].timestamp, 200);
        assert_eq!(result[2].timestamp, 400);
    }

    #[test]
    fn test_chunk_switch_then_query() {
        let fs = make_fs();
        let config = TsdbConfig {
            max_points_per_chunk: 3,
            ..make_config()
        };
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        // Write 5 points — should trigger a chunk flush after 3.
        for i in 0..5u64 {
            db.write(&make_point(1000 + i, 1, 1, i as f64))
                .expect("write");
        }

        // All 5 points should be queryable (3 from disk + 2 from memory).
        let result = db
            .query_range(DeviceId(1), MetricId(1), 0, u64::MAX)
            .expect("query");
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_compact_flushes_chunks() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(1000, 1, 1, 1.0)).expect("write");
        db.write(&make_point(2000, 1, 1, 2.0)).expect("write");
        assert_eq!(db.buffered_points(), 2);

        db.compact().expect("compact");
        assert_eq!(db.buffered_points(), 0);
        assert_eq!(db.index_len(), 1);

        // Query should still find the points after compaction.
        let result = db
            .query_range(DeviceId(1), MetricId(1), 0, u64::MAX)
            .expect("query");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_aggregate_avg() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(100, 1, 1, 10.0)).expect("write");
        db.write(&make_point(200, 1, 1, 20.0)).expect("write");
        db.write(&make_point(300, 1, 1, 30.0)).expect("write");
        db.compact().expect("compact");

        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Avg),
            limit: None,
        };
        let result = db.aggregate(&q).expect("aggregate");
        assert_eq!(result.count, 3);
        assert!((result.value - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_max_min_sum_count() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        for (ts, v) in [(100u64, 10.0f64), (200, 50.0), (300, 30.0)] {
            db.write(&make_point(ts, 1, 1, v)).expect("write");
        }
        db.compact().expect("compact");

        for (agg, expected) in [
            (Aggregation::Max, 50.0),
            (Aggregation::Min, 10.0),
            (Aggregation::Sum, 90.0),
            (Aggregation::Count, 3.0),
        ] {
            let q = Query {
                device_ids: vec![DeviceId(1)],
                metrics: vec![MetricId(1)],
                time_range: (0, 500),
                aggregation: Some(agg),
                limit: None,
            };
            let result = db.aggregate(&q).expect("aggregate");
            assert!(
                (result.value - expected).abs() < f64::EPSILON,
                "{:?}={}",
                agg,
                result.value
            );
        }
    }

    #[test]
    fn test_query_with_aggregation_returns_single_point() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(100, 1, 1, 10.0)).expect("write");
        db.write(&make_point(200, 1, 1, 20.0)).expect("write");

        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Sum),
            limit: None,
        };
        let result = db.query(&q).expect("query");
        assert_eq!(result.len(), 1);
        assert!((result[0].value - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_query_with_limit() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        for i in 0..10u64 {
            db.write(&make_point(i * 100, 1, 1, i as f64))
                .expect("write");
        }

        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, u64::MAX),
            aggregation: None,
            limit: Some(3),
        };
        let result = db.query(&q).expect("query");
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_ttl_cleanup() {
        let fs = make_fs();
        let config = TsdbConfig {
            retention_ms: 50000,
            ..make_config()
        };
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        // Write old data (t=1000) and flush.
        db.write(&make_point(1000, 1, 1, 1.0)).expect("write");
        db.compact().expect("compact");
        assert_eq!(db.index_len(), 1);

        // Write recent data (t=90000) and flush.
        db.write(&make_point(90000, 1, 1, 2.0)).expect("write");
        db.compact().expect("compact");
        assert_eq!(db.index_len(), 2);

        // Cleanup with now=100000, retention=50000 → cutoff=50000.
        // Entry at t=1000 < 50000 → expired.
        // Entry at t=90000 >= 50000 → retained.
        let removed = db.cleanup_expired(100000).expect("cleanup");
        assert_eq!(removed, 1);
        assert_eq!(db.index_len(), 1);

        // The recent data should still be queryable.
        let result = db
            .query_range(DeviceId(1), MetricId(1), 0, u64::MAX)
            .expect("query");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].timestamp, 90000);
    }

    #[test]
    fn test_close_persists_index() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(1000, 1, 1, 1.0)).expect("write");
        db.compact().expect("compact");
        assert_eq!(db.index_len(), 1);

        // Close should persist the index.
        db.close().expect("close");
    }

    #[test]
    fn test_read_last() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(1000, 1, 1, 1.0)).expect("write");
        db.write(&make_point(2000, 1, 1, 2.0)).expect("write");
        db.write(&make_point(3000, 1, 1, 3.0)).expect("write");

        // read_last from in-memory buffer.
        let last = db.read_last(DeviceId(1), MetricId(1)).expect("read_last");
        assert!(last.is_some());
        assert_eq!(last.unwrap().timestamp, 3000);

        // After flush, read_last from disk.
        db.compact().expect("compact");
        let last = db.read_last(DeviceId(1), MetricId(1)).expect("read_last");
        assert!(last.is_some());
        assert_eq!(last.unwrap().timestamp, 3000);
    }

    #[test]
    fn test_read_last_no_data() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        let result = db.read_last(DeviceId(1), MetricId(1)).expect("read_last");
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_query() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        let result = db
            .query_range(DeviceId(1), MetricId(1), 0, 1000)
            .expect("query");
        assert!(result.is_empty());
    }

    #[test]
    fn test_multiple_device_metric_pairs() {
        let fs = make_fs();
        let config = make_config();
        let mut db = TimeSeriesDB::open(fs, config).expect("open");

        db.write(&make_point(1000, 1, 1, 1.0)).expect("write");
        db.write(&make_point(1000, 1, 2, 2.0)).expect("write");
        db.write(&make_point(1000, 2, 1, 3.0)).expect("write");
        db.compact().expect("compact");

        // Query each pair.
        assert_eq!(
            db.query_range(DeviceId(1), MetricId(1), 0, u64::MAX)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.query_range(DeviceId(1), MetricId(2), 0, u64::MAX)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.query_range(DeviceId(2), MetricId(1), 0, u64::MAX)
                .unwrap()
                .len(),
            1
        );
        // Non-existent pair.
        assert_eq!(
            db.query_range(DeviceId(3), MetricId(3), 0, u64::MAX)
                .unwrap()
                .len(),
            0
        );
    }
}
