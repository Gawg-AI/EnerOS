pub mod aggregation;
pub mod anomaly;
pub mod downsample;
pub mod engine;
pub mod gorilla;
pub mod influxdb_backend;
pub mod interpolation;
pub mod query;
pub mod soe;
pub mod sqlite_storage;
pub mod storage;
pub mod tdengine_backend;

pub use engine::TimeSeriesEngine;
pub use sqlite_storage::SqliteStorage;
pub use storage::{CompressedStorage, InMemoryStorage, TimeSeriesStorage};
pub use tdengine_backend::{TDengineBackend, TDengineConfig};
pub use influxdb_backend::{InfluxdbBackend, InfluxdbConfig};
pub use query::TimeSeriesQuery;
pub use downsample::{AggregatedPoint, DownsampleLevel, DownsampledCache};
pub use soe::{SoeRecord, SoeEventType, SoeRecorder, SoeStorage};
pub use gorilla::{GorillaDecoder, GorillaEncoder};

/// Configuration for the time-series storage backend
#[derive(Debug, Clone)]
pub struct TimeSeriesConfig {
    /// Storage backend type: "memory" (default), "sqlite", "tdengine", or "influxdb"
    pub storage_backend: String,
    /// Path to SQLite database file (only used when storage_backend is "sqlite")
    pub sqlite_path: String,
    /// TDengine 连接配置（仅当 storage_backend 为 "tdengine" 时使用）
    pub tdengine: TDengineConfig,
    /// InfluxDB 连接配置（仅当 storage_backend 为 "influxdb" 时使用）
    pub influxdb: InfluxdbConfig,
}

impl Default for TimeSeriesConfig {
    fn default() -> Self {
        Self {
            storage_backend: "memory".to_string(),
            sqlite_path: "eneros_timeseries.db".to_string(),
            tdengine: TDengineConfig::default(),
            influxdb: InfluxdbConfig::default(),
        }
    }
}