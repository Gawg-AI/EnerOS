//! TSDB error type and conversions.
//!
//! [`TsdbError`] is the unified error enum returned by all TSDB operations.
//! It converts from [`eneros_fs::FsError`] so that filesystem errors
//! propagate seamlessly via the `?` operator.

use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::schema::{DeviceId, MetricId};

/// Unified error type for all TSDB operations.
#[derive(Debug, Clone, PartialEq)]
pub enum TsdbError {
    /// Storage device is full (mapped from `FsError::NoSpace`).
    DiskFull,
    /// I/O error from the underlying filesystem or storage layer.
    IoError(String),
    /// Decompression of a chunk payload failed.
    DecompressFailed,
    /// The on-disk index file is corrupted or unreadable.
    IndexCorrupted,
    /// The query parameters are invalid (empty range, bad device, etc.).
    InvalidQuery,
    /// The requested device does not exist in the index.
    DeviceNotFound(DeviceId),
    /// The requested metric does not exist in the index.
    MetricNotFound(MetricId),
    /// A chunk file failed CRC validation or is otherwise unreadable.
    ChunkCorrupted { chunk_id: u32 },
}

impl fmt::Display for TsdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TsdbError::DiskFull => write!(f, "disk full"),
            TsdbError::IoError(detail) => write!(f, "I/O error: {}", detail),
            TsdbError::DecompressFailed => write!(f, "decompression failed"),
            TsdbError::IndexCorrupted => write!(f, "index corrupted"),
            TsdbError::InvalidQuery => write!(f, "invalid query"),
            TsdbError::DeviceNotFound(id) => write!(f, "device not found: {}", id.0),
            TsdbError::MetricNotFound(id) => write!(f, "metric not found: {}", id.0),
            TsdbError::ChunkCorrupted { chunk_id } => {
                write!(f, "chunk corrupted: {}", chunk_id)
            }
        }
    }
}

/// Convert filesystem errors into TSDB errors.
///
/// Mapping:
/// - `FsError::NoSpace` → [`TsdbError::DiskFull`] (semantically equivalent:
///   no space left = disk full).
/// - All other `FsError` variants → [`TsdbError::IoError`] with a debug
///   representation of the original error.
impl From<eneros_fs::FsError> for TsdbError {
    fn from(e: eneros_fs::FsError) -> Self {
        match e {
            eneros_fs::FsError::NoSpace => TsdbError::DiskFull,
            other => TsdbError::IoError(format!("{:?}", other)),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_fserror_no_space_to_disk_full() {
        let fs_err = eneros_fs::FsError::NoSpace;
        let tsdb_err: TsdbError = fs_err.into();
        assert_eq!(tsdb_err, TsdbError::DiskFull);
    }

    #[test]
    fn test_fserror_not_found_to_io_error() {
        let fs_err = eneros_fs::FsError::NotFound {
            path: String::from("/tsdb/missing"),
        };
        let tsdb_err: TsdbError = fs_err.into();
        match tsdb_err {
            TsdbError::IoError(msg) => {
                assert!(msg.contains("NotFound"));
                assert!(msg.contains("/tsdb/missing"));
            }
            other => panic!("expected IoError, got {:?}", other),
        }
    }

    #[test]
    fn test_fserror_corrupted_block_to_io_error() {
        let fs_err = eneros_fs::FsError::CorruptedBlock { block: 42 };
        let tsdb_err: TsdbError = fs_err.into();
        match tsdb_err {
            TsdbError::IoError(msg) => assert!(msg.contains("CorruptedBlock")),
            other => panic!("expected IoError, got {:?}", other),
        }
    }

    #[test]
    fn test_display_disk_full() {
        assert_eq!(format!("{}", TsdbError::DiskFull), "disk full");
    }

    #[test]
    fn test_display_io_error() {
        let err = TsdbError::IoError(String::from("write failed"));
        assert_eq!(format!("{}", err), "I/O error: write failed");
    }

    #[test]
    fn test_display_device_not_found() {
        let err = TsdbError::DeviceNotFound(DeviceId(7));
        assert_eq!(format!("{}", err), "device not found: 7");
    }

    #[test]
    fn test_display_metric_not_found() {
        let err = TsdbError::MetricNotFound(MetricId(3));
        assert_eq!(format!("{}", err), "metric not found: 3");
    }

    #[test]
    fn test_display_chunk_corrupted() {
        let err = TsdbError::ChunkCorrupted { chunk_id: 99 };
        assert_eq!(format!("{}", err), "chunk corrupted: 99");
    }

    #[test]
    fn test_display_decompress_failed() {
        assert_eq!(
            format!("{}", TsdbError::DecompressFailed),
            "decompression failed"
        );
    }

    #[test]
    fn test_display_index_corrupted() {
        assert_eq!(format!("{}", TsdbError::IndexCorrupted), "index corrupted");
    }

    #[test]
    fn test_display_invalid_query() {
        assert_eq!(format!("{}", TsdbError::InvalidQuery), "invalid query");
    }

    #[test]
    fn test_equality() {
        assert_eq!(TsdbError::DiskFull, TsdbError::DiskFull);
        assert_ne!(TsdbError::DiskFull, TsdbError::IoError(String::from("x")));
        assert_eq!(
            TsdbError::DeviceNotFound(DeviceId(1)),
            TsdbError::DeviceNotFound(DeviceId(1))
        );
        assert_eq!(
            TsdbError::ChunkCorrupted { chunk_id: 5 },
            TsdbError::ChunkCorrupted { chunk_id: 5 }
        );
    }
}
