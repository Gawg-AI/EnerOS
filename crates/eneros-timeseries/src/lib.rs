pub mod aggregation;
pub mod anomaly;
pub mod downsample;
pub mod engine;
pub mod interpolation;
pub mod query;
pub mod soe;
pub mod sqlite_storage;
pub mod storage;

pub use engine::TimeSeriesEngine;
pub use sqlite_storage::SqliteStorage;
pub use storage::{InMemoryStorage, TimeSeriesStorage};
pub use query::TimeSeriesQuery;
pub use downsample::{AggregatedPoint, DownsampleLevel, DownsampledCache};
pub use soe::{SoeRecord, SoeEventType, SoeRecorder, SoeStorage};

/// Configuration for the time-series storage backend
#[derive(Debug, Clone)]
pub struct TimeSeriesConfig {
    /// Storage backend type: "memory" (default) or "sqlite"
    pub storage_backend: String,
    /// Path to SQLite database file (only used when storage_backend is "sqlite")
    pub sqlite_path: String,
}

impl Default for TimeSeriesConfig {
    fn default() -> Self {
        Self {
            storage_backend: "memory".to_string(),
            sqlite_path: "eneros_timeseries.db".to_string(),
        }
    }
}
