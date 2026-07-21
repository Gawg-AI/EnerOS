//! Time-series data reader — locates chunk files via the index, decompresses
//! columnar data, and reconstructs [`TimeSeriesPoint`] vectors for queries.
//!
//! # Chunk File Layout
//!
//! See [`crate::writer`] for the on-disk format. The reader parses the
//! `ChunkHeader` (32 bytes) followed by three `u32` section lengths, then
//! decompresses each section independently.

use alloc::vec;
use alloc::vec::Vec;

use eneros_fs::{FileSystem, Lfs, OpenFlags};

use crate::compression::Compressor;
use crate::error::TsdbError;
use crate::index::TimeIndex;
use crate::schema::{
    AggResult, Aggregation, ChunkHeader, DataQuality, DeviceId, MetricId, Query, TimeSeriesPoint,
};
use crate::writer::{deserialize_timestamps, deserialize_values};

// ============================================================================
// TsdbReader trait
// ============================================================================

/// Abstraction for time-series readers.
///
/// Methods take `&mut self` because the underlying filesystem (`Lfs`) requires
/// `&mut` access for file I/O (the `FileSystem::open` trait method takes
/// `&mut self`, and `File::read` requires `&mut Lfs`).
pub trait TsdbReader {
    /// Reads all points for `device`/`metric` within `[start, end]` (inclusive).
    fn read_range(
        &mut self,
        device: DeviceId,
        metric: MetricId,
        start: u64,
        end: u64,
    ) -> Result<Vec<TimeSeriesPoint>, TsdbError>;

    /// Returns the most recent point for `device`/`metric`, or `None` if no
    /// data exists.
    fn read_last(
        &mut self,
        device: DeviceId,
        metric: MetricId,
    ) -> Result<Option<TimeSeriesPoint>, TsdbError>;

    /// Computes an aggregate over the points matched by `q`.
    fn aggregate(&mut self, q: &Query) -> Result<AggResult, TsdbError>;
}

// ============================================================================
// TsdbReaderImpl
// ============================================================================

/// Reader implementation bound to a filesystem, index, and compressor.
pub struct TsdbReaderImpl<'a> {
    fs: &'a mut Lfs,
    index: &'a TimeIndex,
    compressor: &'a dyn Compressor,
    chunk_duration_ms: u64,
}

impl<'a> TsdbReaderImpl<'a> {
    /// Creates a new reader.
    ///
    /// `chunk_duration_ms` is used to widen the index search window so that
    /// chunks starting before `start` but overlapping the query range are
    /// included.
    pub fn new(
        fs: &'a mut Lfs,
        index: &'a TimeIndex,
        compressor: &'a dyn Compressor,
        chunk_duration_ms: u64,
    ) -> Self {
        Self {
            fs,
            index,
            compressor,
            chunk_duration_ms,
        }
    }
}

impl<'a> TsdbReader for TsdbReaderImpl<'a> {
    fn read_range(
        &mut self,
        device: DeviceId,
        metric: MetricId,
        start: u64,
        end: u64,
    ) -> Result<Vec<TimeSeriesPoint>, TsdbError> {
        read_range(
            self.fs,
            self.index,
            self.compressor,
            self.chunk_duration_ms,
            device,
            metric,
            start,
            end,
        )
    }

    fn read_last(
        &mut self,
        device: DeviceId,
        metric: MetricId,
    ) -> Result<Option<TimeSeriesPoint>, TsdbError> {
        read_last(self.fs, self.index, self.compressor, device, metric)
    }

    fn aggregate(&mut self, q: &Query) -> Result<AggResult, TsdbError> {
        aggregate(
            self.fs,
            self.index,
            self.compressor,
            self.chunk_duration_ms,
            q,
        )
    }
}

// ============================================================================
// Free functions — core logic reusable by TimeSeriesDB
// ============================================================================

/// Reads all points for `device`/`metric` within `[start, end]` (inclusive).
#[allow(clippy::too_many_arguments)]
pub fn read_range(
    fs: &mut Lfs,
    index: &TimeIndex,
    compressor: &dyn Compressor,
    chunk_duration_ms: u64,
    device: DeviceId,
    metric: MetricId,
    start: u64,
    end: u64,
) -> Result<Vec<TimeSeriesPoint>, TsdbError> {
    if start > end {
        return Err(TsdbError::InvalidQuery);
    }

    // Widen the search window: a chunk starting before `start` may still
    // contain points in [start, end] (chunk span <= chunk_duration_ms).
    let search_start = start.saturating_sub(chunk_duration_ms);
    let entries = index.find_range(search_start, end);

    let mut result = Vec::new();
    for entry in entries {
        // Filter by device/metric using the file path to avoid reading
        // non-matching chunk files.
        if !path_matches(&entry.file_path, device, metric) {
            continue;
        }

        let points = read_chunk_file(fs, compressor, &entry.file_path)?;
        for p in points {
            if p.timestamp >= start && p.timestamp <= end {
                result.push(p);
            }
        }
    }
    Ok(result)
}

/// Returns the most recent point for `device`/`metric`, or `None`.
pub fn read_last(
    fs: &mut Lfs,
    index: &TimeIndex,
    compressor: &dyn Compressor,
    device: DeviceId,
    metric: MetricId,
) -> Result<Option<TimeSeriesPoint>, TsdbError> {
    // Scan all entries in reverse time order (BTreeMap is sorted ascending,
    // so find_range returns ascending order — iterate from the end).
    let entries = index.find_range(0, u64::MAX);
    for entry in entries.iter().rev() {
        if !path_matches(&entry.file_path, device, metric) {
            continue;
        }
        let points = read_chunk_file(fs, compressor, &entry.file_path)?;
        if let Some(last) = points.last() {
            return Ok(Some(last.clone()));
        }
    }
    Ok(None)
}

/// Computes an aggregate over the points matched by `q`.
pub fn aggregate(
    fs: &mut Lfs,
    index: &TimeIndex,
    compressor: &dyn Compressor,
    chunk_duration_ms: u64,
    q: &Query,
) -> Result<AggResult, TsdbError> {
    let agg = q.aggregation.ok_or(TsdbError::InvalidQuery)?;
    let (start, end) = q.time_range;
    if start > end {
        return Err(TsdbError::InvalidQuery);
    }

    // Collect all matching points.
    let mut values: Vec<f64> = Vec::new();
    for &device in &q.device_ids {
        for &metric in &q.metrics {
            let points = read_range(
                fs,
                index,
                compressor,
                chunk_duration_ms,
                device,
                metric,
                start,
                end,
            )?;
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

// ============================================================================
// Chunk file reading
// ============================================================================

/// Reads and decodes a chunk file, returning all contained points.
fn read_chunk_file(
    fs: &mut Lfs,
    compressor: &dyn Compressor,
    path: &str,
) -> Result<Vec<TimeSeriesPoint>, TsdbError> {
    let data = read_entire_file(fs, path)?;

    // Parse ChunkHeader (32 bytes).
    if data.len() < 32 {
        return Err(TsdbError::ChunkCorrupted { chunk_id: 0 });
    }
    let header = ChunkHeader::from_bytes(&data).ok_or(TsdbError::ChunkCorrupted { chunk_id: 0 })?;

    // Parse section lengths (3 × u32 = 12 bytes after header).
    let mut off = 32;
    if data.len() < off + 12 {
        return Err(TsdbError::ChunkCorrupted { chunk_id: 0 });
    }
    let ts_len =
        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
    off += 4;
    let val_len =
        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
    off += 4;
    let qual_len =
        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
    off += 4;

    // Extract compressed sections.
    if data.len() < off + ts_len + val_len + qual_len {
        return Err(TsdbError::ChunkCorrupted { chunk_id: 0 });
    }
    let compressed_ts = &data[off..off + ts_len];
    off += ts_len;
    let compressed_val = &data[off..off + val_len];
    off += val_len;
    let compressed_qual = &data[off..off + qual_len];

    // Decompress and deserialize columns.
    let ts_bytes = compressor.decompress(compressed_ts)?;
    let val_bytes = compressor.decompress(compressed_val)?;
    let qual_bytes = compressor.decompress(compressed_qual)?;

    let timestamps = deserialize_timestamps(&ts_bytes)?;
    let values = deserialize_values(&val_bytes)?;

    // Qualities are raw u8 — no further deserialization needed.
    if qual_bytes.len() != timestamps.len() || values.len() != timestamps.len() {
        return Err(TsdbError::ChunkCorrupted { chunk_id: 0 });
    }

    // Assemble points.
    let mut points = Vec::with_capacity(timestamps.len());
    for i in 0..timestamps.len() {
        points.push(TimeSeriesPoint {
            timestamp: timestamps[i],
            device_id: header.device_id,
            metric: header.metric,
            value: values[i],
            quality: DataQuality::from(qual_bytes[i]),
        });
    }
    Ok(points)
}

/// Reads an entire file into a byte vector.
fn read_entire_file(fs: &mut Lfs, path: &str) -> Result<Vec<u8>, TsdbError> {
    let stat = fs.stat(path)?;
    let size = stat.size as usize;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size];
    let mut file = fs.open(path, OpenFlags::READ)?;
    let mut read_total = 0;
    while read_total < size {
        let n = file.read(fs, &mut buf[read_total..])?;
        if n == 0 {
            break;
        }
        read_total += n;
    }
    buf.truncate(read_total);
    Ok(buf)
}

/// Checks whether a chunk file path corresponds to the given device/metric.
///
/// Path format: `{data_dir}/{device_id}/{metric}/{start_time:020}`
fn path_matches(path: &str, device: DeviceId, metric: MetricId) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 3 {
        return false;
    }
    let metric_part = parts[parts.len() - 2];
    let device_part = parts[parts.len() - 3];
    match (device_part.parse::<u32>(), metric_part.parse::<u32>()) {
        (Ok(d), Ok(m)) => d == device.0 && m == metric.0,
        _ => false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_storage::MockBlockDevice;

    use super::*;
    use crate::compression::SnappyCompressor;
    use crate::schema::{DataQuality, TimeSeriesPoint, TsdbConfig};
    use crate::writer::{make_chunk_path, TsdbWriter, TsdbWriterImpl};

    fn make_fs() -> Lfs {
        let dev: Box<dyn eneros_storage::BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        Lfs::format(dev).expect("format should succeed")
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

    /// Writes points, flushes them, and returns a reader-ready (fs, index).
    fn setup_with_points(
        points: &[TimeSeriesPoint],
    ) -> (Lfs, crate::index::TimeIndex, SnappyCompressor) {
        let mut fs = make_fs();
        let mut index = crate::index::TimeIndex::new();
        let compressor = SnappyCompressor::new();
        let config = TsdbConfig::default();

        let mut writer = TsdbWriterImpl::new(config, &mut index, &mut fs, &compressor);
        for p in points {
            writer.append(p).expect("append");
        }
        writer.flush().expect("flush");
        (fs, index, compressor)
    }

    #[test]
    fn test_read_range_single_point() {
        let points = vec![make_point(1000, 1, 2, 42.5)];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader
            .read_range(DeviceId(1), MetricId(2), 0, 2000)
            .expect("read");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].timestamp, 1000);
        assert_eq!(result[0].value, 42.5);
        assert_eq!(result[0].quality, DataQuality::Good);
    }

    #[test]
    fn test_read_range_multiple_points() {
        let points: Vec<TimeSeriesPoint> = (0..5u64)
            .map(|i| make_point(100 + i * 100, 1, 1, i as f64))
            .collect();
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        // Query [200, 400] → points at 200, 300, 400.
        let result = reader
            .read_range(DeviceId(1), MetricId(1), 200, 400)
            .expect("read");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].timestamp, 200);
        assert_eq!(result[2].timestamp, 400);
    }

    #[test]
    fn test_read_range_empty_result() {
        let points = vec![make_point(1000, 1, 2, 42.5)];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader
            .read_range(DeviceId(1), MetricId(2), 2000, 3000)
            .expect("read");
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_range_wrong_device() {
        let points = vec![make_point(1000, 1, 2, 42.5)];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader
            .read_range(DeviceId(99), MetricId(2), 0, 2000)
            .expect("read");
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_range_invalid_range() {
        let points = vec![make_point(1000, 1, 2, 42.5)];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader.read_range(DeviceId(1), MetricId(2), 2000, 1000);
        assert!(matches!(result, Err(TsdbError::InvalidQuery)));
    }

    #[test]
    fn test_read_last() {
        let points = vec![
            make_point(1000, 1, 2, 1.0),
            make_point(2000, 1, 2, 2.0),
            make_point(3000, 1, 2, 3.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let last = reader
            .read_last(DeviceId(1), MetricId(2))
            .expect("read_last");
        assert!(last.is_some());
        let last = last.unwrap();
        assert_eq!(last.timestamp, 3000);
        assert_eq!(last.value, 3.0);
    }

    #[test]
    fn test_read_last_no_data() {
        let mut fs = make_fs();
        let index = crate::index::TimeIndex::new();
        let compressor = SnappyCompressor::new();

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader
            .read_last(DeviceId(1), MetricId(2))
            .expect("read_last");
        assert!(result.is_none());
    }

    #[test]
    fn test_aggregate_avg() {
        let points = vec![
            make_point(100, 1, 1, 10.0),
            make_point(200, 1, 1, 20.0),
            make_point(300, 1, 1, 30.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Avg),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert_eq!(result.aggregation, Aggregation::Avg);
        assert_eq!(result.count, 3);
        assert!((result.value - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_max() {
        let points = vec![
            make_point(100, 1, 1, 10.0),
            make_point(200, 1, 1, 50.0),
            make_point(300, 1, 1, 30.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Max),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert_eq!(result.value, 50.0);
        assert_eq!(result.count, 3);
    }

    #[test]
    fn test_aggregate_min() {
        let points = vec![
            make_point(100, 1, 1, 10.0),
            make_point(200, 1, 1, 5.0),
            make_point(300, 1, 1, 30.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Min),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert_eq!(result.value, 5.0);
    }

    #[test]
    fn test_aggregate_sum() {
        let points = vec![
            make_point(100, 1, 1, 10.0),
            make_point(200, 1, 1, 20.0),
            make_point(300, 1, 1, 30.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Sum),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert!((result.value - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_count() {
        let points = vec![
            make_point(100, 1, 1, 10.0),
            make_point(200, 1, 1, 20.0),
            make_point(300, 1, 1, 30.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Count),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert_eq!(result.value, 3.0);
        assert_eq!(result.count, 3);
    }

    #[test]
    fn test_aggregate_empty() {
        let mut fs = make_fs();
        let index = crate::index::TimeIndex::new();
        let compressor = SnappyCompressor::new();

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: Some(Aggregation::Avg),
            limit: None,
        };
        let result = reader.aggregate(&q).expect("aggregate");
        assert_eq!(result.count, 0);
        assert!((result.value - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_no_aggregation_is_error() {
        let mut fs = make_fs();
        let index = crate::index::TimeIndex::new();
        let compressor = SnappyCompressor::new();

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let q = Query {
            device_ids: vec![DeviceId(1)],
            metrics: vec![MetricId(1)],
            time_range: (0, 500),
            aggregation: None,
            limit: None,
        };
        let result = reader.aggregate(&q);
        assert!(matches!(result, Err(TsdbError::InvalidQuery)));
    }

    #[test]
    fn test_path_matches() {
        let path = make_chunk_path("/tsdb", DeviceId(1), MetricId(2), 1000);
        assert!(path_matches(&path, DeviceId(1), MetricId(2)));
        assert!(!path_matches(&path, DeviceId(1), MetricId(3)));
        assert!(!path_matches(&path, DeviceId(2), MetricId(2)));
    }

    #[test]
    fn test_path_matches_invalid_path() {
        assert!(!path_matches("/short", DeviceId(1), MetricId(2)));
        assert!(!path_matches("/a/b/notanumber", DeviceId(1), MetricId(2)));
    }

    #[test]
    fn test_read_chunk_file_roundtrip() {
        // Write a chunk, then read it back and verify all fields.
        let points: Vec<TimeSeriesPoint> = (0..10u64)
            .map(|i| make_point(1000 + i * 100, 1, 2, i as f64 * 1.5))
            .collect();
        let (mut fs, index, compressor) = setup_with_points(&points);

        // Read all points back.
        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        let result = reader
            .read_range(DeviceId(1), MetricId(2), 0, u64::MAX)
            .expect("read");
        assert_eq!(result.len(), 10);
        for (i, p) in result.iter().enumerate() {
            assert_eq!(p.timestamp, 1000 + i as u64 * 100);
            assert!((p.value - (i as f64 * 1.5)).abs() < f64::EPSILON);
            assert_eq!(p.quality, DataQuality::Good);
        }
    }

    #[test]
    fn test_read_range_inclusive_bounds() {
        let points = vec![
            make_point(100, 1, 1, 1.0),
            make_point(200, 1, 1, 2.0),
            make_point(300, 1, 1, 3.0),
        ];
        let (mut fs, index, compressor) = setup_with_points(&points);

        let mut reader = TsdbReaderImpl::new(&mut fs, &index, &compressor, 3_600_000);
        // Query [100, 300] should include all 3 points (inclusive bounds).
        let result = reader
            .read_range(DeviceId(1), MetricId(1), 100, 300)
            .expect("read");
        assert_eq!(result.len(), 3);
    }
}
