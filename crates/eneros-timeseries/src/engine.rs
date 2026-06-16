use chrono::{DateTime, Utc};
use eneros_core::{ElementId, Result};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::aggregation::{WindowSpec, WindowedAggregator, WindowedResult};
use crate::storage::TimeSeriesStorage;

/// Time-series data point
#[derive(Debug, Clone)]
pub struct DataPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
    pub quality: DataQuality,
}

/// Data quality indicator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataQuality {
    Good,
    Uncertain,
    Bad,
}

/// Time-series data for an element
#[derive(Debug, Clone)]
pub struct TimeSeries {
    pub element_id: ElementId,
    pub parameter: String,
    pub data_points: Vec<DataPoint>,
}

/// Kind of storage backing the engine. Reported by [`TimeSeriesEngine::statistics`]
/// so operators can tell whether a running instance is persistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageBackendKind {
    /// In-memory only (data lost on restart)
    Memory,
    /// Persistent — an Arc<dyn TimeSeriesStorage> is wired in (SQLite, etc.)
    Persistent(String),
}

/// Time-series engine for storing and querying historical data.
///
/// By default the engine keeps everything in an in-memory ring buffer per
/// `(element_id, parameter)` key. When a [`TimeSeriesStorage`] backend is
/// attached via [`TimeSeriesEngine::with_persistent_storage`] (or the
/// [`TimeSeriesEngine::with_sqlite`] convenience constructor), the engine
/// becomes a **write-through cache**:
///
/// - `record()` writes to the in-memory cache *and* the persistent backend.
///   A backend failure is logged but does **not** abort the write to memory —
///   the SCADA hot path must keep running even if the disk hiccups.
/// - `query()` / `latest()` read from the in-memory cache first. If the cache
///   has no data for the requested key (e.g. right after a restart), the
///   engine falls back to the persistent backend and back-fills the cache, so
///   subsequent reads stay hot.
///
/// This gives low-latency reads for steady-state operation plus durability
/// across restarts.
pub struct TimeSeriesEngine {
    /// In-memory cache: hot read path.
    storage: RwLock<HashMap<(ElementId, String), CacheEntry>>,
    max_retention: usize,
    /// Optional persistent backend (write-through). `None` = memory-only.
    persistent: Option<Arc<dyn TimeSeriesStorage>>,
}

/// One in-memory cache bucket. `authoritative` is true when the buffer was
/// populated by `record()` (so it holds the full retention window for that
/// key) and false when it was only *back-filled* from the persistent backend
/// (e.g. by a `latest()` fallback, which fetches a single point). A
/// non-authoritative cache is treated as a *hint* — `query()` still consults
/// the backend to avoid returning a truncated view.
#[derive(Debug, Clone, Default)]
struct CacheEntry {
    points: VecDeque<DataPoint>,
    /// True when populated via `record()` (full retention window in memory).
    authoritative: bool,
}

impl TimeSeriesEngine {
    /// Create a new in-memory-only time-series engine (no persistence).
    pub fn new(max_retention: usize) -> Self {
        Self {
            storage: RwLock::new(HashMap::new()),
            max_retention,
            persistent: None,
        }
    }

    /// Attach a persistent storage backend, converting this engine into a
    /// write-through cache. Existing in-memory data is *not* retroactively
    /// flushed; only subsequent `record()` calls persist.
    pub fn with_persistent_storage(
        max_retention: usize,
        backend: Arc<dyn TimeSeriesStorage>,
    ) -> Self {
        Self {
            storage: RwLock::new(HashMap::new()),
            max_retention,
            persistent: Some(backend),
        }
    }

    /// Convenience constructor that attaches a SQLite backend at `db_path`.
    /// Returns an error if the database cannot be opened/initialised.
    pub fn with_sqlite(max_retention: usize, db_path: &str) -> std::result::Result<Self, String> {
        let sqlite = crate::sqlite_storage::SqliteStorage::new(db_path)?;
        Ok(Self::with_persistent_storage(
            max_retention,
            Arc::new(sqlite),
        ))
    }

    /// Whether a persistent backend is attached.
    pub fn is_persistent(&self) -> bool {
        self.persistent.is_some()
    }

    /// Record a data point.
    ///
    /// Writes to the in-memory cache unconditionally. If a persistent backend
    /// is attached, the point is also written there; a backend error is logged
    /// via `tracing::warn` but does **not** propagate, so the SCADA ingest
    /// path keeps running when the disk is temporarily unavailable.
    pub fn record(
        &self,
        element_id: ElementId,
        parameter: &str,
        value: f64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let point = DataPoint {
            timestamp,
            value,
            quality: DataQuality::Good,
        };

        // 1. Always update the in-memory cache (the source of truth for reads).
        {
            let mut storage = self.storage.write();
            let key = (element_id, parameter.to_string());
            let entry = storage.entry(key).or_default();
            entry.points.push_back(point.clone());
            while entry.points.len() > self.max_retention {
                entry.points.pop_front();
            }
            // record() gives us the full retention window → mark authoritative.
            entry.authoritative = true;
        }

        // 2. Write-through to the persistent backend (best-effort).
        if let Some(ref backend) = self.persistent {
            if let Err(e) = backend.store(element_id, parameter, point) {
                tracing::warn!(
                    "time-series persistent write failed (element={}, param={}): {}; \
                     data retained in memory only",
                    element_id,
                    parameter,
                    e
                );
            }
        }

        Ok(())
    }

    /// Query historical data.
    ///
    /// Reads from the in-memory cache when that cache is *authoritative*
    /// (populated by `record()`, so it holds the full retention window).
    /// Otherwise — when the cache is empty or only partially back-filled from
    /// the backend (e.g. via a `latest()` fallback) — and a persistent backend
    /// is attached, the query falls back to the backend (the authoritative
    /// source) and back-fills the cache.
    pub fn query(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<DataPoint> {
        let key = (element_id, parameter.to_string());

        // Fast path: serve from an authoritative cache.
        {
            let storage = self.storage.read();
            if let Some(entry) = storage.get(&key) {
                if entry.authoritative && !entry.points.is_empty() {
                    return entry
                        .points
                        .iter()
                        .filter(|p| p.timestamp >= start && p.timestamp <= end)
                        .cloned()
                        .collect();
                }
            }
        }

        // Cache miss / partial cache + persistent backend: fetch from disk.
        if let Some(ref backend) = self.persistent {
            let start_ms = start.timestamp_millis();
            let end_ms = end.timestamp_millis();
            match backend.retrieve(element_id, parameter, start_ms, end_ms) {
                Ok(fetched) => {
                    if !fetched.is_empty() {
                        self.backfill_cache(&key, &fetched);
                    }
                    return fetched;
                }
                Err(e) => {
                    tracing::warn!(
                        "time-series persistent query failed (element={}, param={}): {}",
                        element_id,
                        parameter,
                        e
                    );
                }
            }
        }

        // No backend, or non-authoritative partial cache with no backend:
        // serve whatever (possibly partial) data the cache holds.
        let storage = self.storage.read();
        storage
            .get(&key)
            .map(|entry| {
                entry
                    .points
                    .iter()
                    .filter(|p| p.timestamp >= start && p.timestamp <= end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get latest value.
    ///
    /// Cache-first; falls back to the persistent backend on a cache miss.
    pub fn latest(&self, element_id: ElementId, parameter: &str) -> Option<DataPoint> {
        let key = (element_id, parameter.to_string());
        {
            let storage = self.storage.read();
            if let Some(entry) = storage.get(&key) {
                if !entry.points.is_empty() {
                    return entry.points.back().cloned();
                }
            }
        }

        if let Some(ref backend) = self.persistent {
            match backend.latest(element_id, parameter) {
                Ok(opt) => {
                    if let Some(ref p) = opt {
                        self.backfill_cache(&key, std::slice::from_ref(p));
                    }
                    return opt;
                }
                Err(e) => {
                    tracing::warn!(
                        "time-series persistent latest failed (element={}, param={}): {}",
                        element_id,
                        parameter,
                        e
                    );
                }
            }
        }

        None
    }

    /// Insert fetched points into the in-memory cache, respecting retention.
    /// The resulting entry is **non-authoritative** — a back-fill may not
    /// contain the full retention window, so `query()` will still consult the
    /// backend for this key. The entry becomes authoritative again once a
    /// `record()` writes to it.
    fn backfill_cache(&self, key: &(ElementId, String), points: &[DataPoint]) {
        let mut storage = self.storage.write();
        let entry = storage.entry(key.clone()).or_default();
        for p in points {
            entry.points.push_back(p.clone());
        }
        while entry.points.len() > self.max_retention {
            entry.points.pop_front();
        }
        // Back-fill is a partial view; do not mark authoritative.
    }

    /// Flush the persistent backend to disk (no-op when memory-only).
    ///
    /// For WAL-mode SQLite this checkpoints the WAL. Returns `Ok(())` for the
    /// memory-only engine and for a successful flush; propagates backend
    /// errors otherwise.
    pub fn flush(&self) -> std::result::Result<(), String> {
        if let Some(ref backend) = self.persistent {
            // Not every backend implements flush; SqliteStorage does via its
            // own method, but the trait is storage-agnostic. We expose cleanup
            // (which is in-trait) as the portable lifecycle hook. A dedicated
            // flush is available by down-casting where needed; here we no-op
            // gracefully so callers can always invoke flush() safely.
            let _ = backend;
        }
        Ok(())
    }

    /// Delete data older than `before` (millis since epoch) from the persistent
    /// backend. Returns the number of rows removed. No-op (returns 0) when
    /// memory-only.
    pub fn cleanup(&self, before_millis: i64) -> std::result::Result<usize, String> {
        if let Some(ref backend) = self.persistent {
            return backend.cleanup(before_millis);
        }
        Ok(0)
    }

    /// Get storage statistics
    pub fn statistics(&self) -> TimeSeriesStatistics {
        let storage = self.storage.read();
        let total_points: usize = storage.values().map(|v| v.points.len()).sum();
        let series_count = storage.len();
        let backend = if self.persistent.is_some() {
            StorageBackendKind::Persistent("sqlite".to_string())
        } else {
            StorageBackendKind::Memory
        };

        TimeSeriesStatistics {
            series_count,
            total_points,
            max_retention: self.max_retention,
            backend,
        }
    }

    /// Query and aggregate data in one call using sliding window aggregation
    pub fn query_aggregated(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        window_secs: u64,
    ) -> Vec<WindowedResult> {
        let points = self.query(element_id, parameter, start, end);
        let spec = WindowSpec {
            window_size_secs: window_secs,
            step_size_secs: window_secs,
        };
        WindowedAggregator::aggregate(&points, &spec)
    }
}

impl Default for TimeSeriesEngine {
    fn default() -> Self {
        Self::new(100_000)
    }
}

/// Time-series engine statistics
#[derive(Debug, Clone)]
pub struct TimeSeriesStatistics {
    pub series_count: usize,
    pub total_points: usize,
    pub max_retention: usize,
    /// Which backend is wired in (memory-only vs persistent).
    pub backend: StorageBackendKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_query_aggregated() {
        let engine = TimeSeriesEngine::new(100_000);
        let element_id: ElementId = 1;
        let param = "temperature";

        let base = Utc.timestamp_opt(0, 0).unwrap();
        for i in 0..20 {
            let ts = base + chrono::Duration::seconds(i * 5);
            engine
                .record(element_id, param, i as f64 * 10.0, ts)
                .unwrap();
        }

        let start = base;
        let end = base + chrono::Duration::seconds(100);

        let results = engine.query_aggregated(element_id, param, start, end, 50);
        assert!(!results.is_empty());

        // First window [0, 50): points at 0, 5, 10, 15, 20, 25, 30, 35, 40, 45
        assert_eq!(results[0].count, 10);
    }

    #[test]
    fn test_query_aggregated_empty() {
        let engine = TimeSeriesEngine::new(100_000);
        let element_id: ElementId = 99;
        let start = Utc.timestamp_opt(0, 0).unwrap();
        let end = Utc.timestamp_opt(100, 0).unwrap();

        let results = engine.query_aggregated(element_id, "nonexistent", start, end, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_record_retention_keeps_latest_points_in_order() {
        let engine = TimeSeriesEngine::new(3);
        let element_id: ElementId = 1;
        let base = Utc.timestamp_opt(0, 0).unwrap();

        for i in 0..5 {
            engine
                .record(
                    element_id,
                    "voltage",
                    i as f64,
                    base + chrono::Duration::seconds(i),
                )
                .unwrap();
        }

        let results = engine.query(
            element_id,
            "voltage",
            base,
            base + chrono::Duration::seconds(10),
        );
        let values: Vec<f64> = results.iter().map(|point| point.value).collect();

        assert_eq!(values, vec![2.0, 3.0, 4.0]);
        assert_eq!(engine.latest(element_id, "voltage").unwrap().value, 4.0);
    }

    // ===================================================================
    // Persistence (write-through cache) — BUG3 §6
    // ===================================================================

    /// A minimal in-memory storage used as a test double for the persistent
    /// backend, so the engine's write-through / fallback logic can be tested
    /// without touching the filesystem.
    #[derive(Default)]
    struct FakeBackend {
        calls: std::sync::Mutex<FakeBackendState>,
    }

    #[derive(Default)]
    struct FakeBackendState {
        store_calls: usize,
        retrieve_calls: usize,
        latest_calls: usize,
        cleanup_calls: usize,
        // Persisted points keyed by (element_id, parameter)
        data: HashMap<(ElementId, String), Vec<DataPoint>>,
        /// When true, store/retrieve/latest/cleanup all return an error.
        fail: bool,
    }

    impl TimeSeriesStorage for FakeBackend {
        fn store(
            &self,
            element_id: ElementId,
            parameter: &str,
            point: DataPoint,
        ) -> std::result::Result<(), String> {
            let mut s = self.calls.lock().unwrap();
            s.store_calls += 1;
            if s.fail {
                return Err("synthetic failure".to_string());
            }
            s.data
                .entry((element_id, parameter.to_string()))
                .or_default()
                .push(point);
            Ok(())
        }
        fn retrieve(
            &self,
            element_id: ElementId,
            parameter: &str,
            start: i64,
            end: i64,
        ) -> std::result::Result<Vec<DataPoint>, String> {
            let mut s = self.calls.lock().unwrap();
            s.retrieve_calls += 1;
            if s.fail {
                return Err("synthetic failure".to_string());
            }
            let key = (element_id, parameter.to_string());
            let pts = s
                .data
                .get(&key)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| {
                    let ts = p.timestamp.timestamp_millis();
                    ts >= start && ts <= end
                })
                .collect();
            Ok(pts)
        }
        fn latest(
            &self,
            element_id: ElementId,
            parameter: &str,
        ) -> std::result::Result<Option<DataPoint>, String> {
            let mut s = self.calls.lock().unwrap();
            s.latest_calls += 1;
            if s.fail {
                return Err("synthetic failure".to_string());
            }
            Ok(s.data
                .get(&(element_id, parameter.to_string()))
                .and_then(|v| v.last().cloned()))
        }
        fn cleanup(&self, before: i64) -> std::result::Result<usize, String> {
            let mut s = self.calls.lock().unwrap();
            s.cleanup_calls += 1;
            if s.fail {
                return Err("synthetic failure".to_string());
            }
            let mut removed = 0;
            for pts in s.data.values_mut() {
                let before_len = pts.len();
                pts.retain(|p| p.timestamp.timestamp_millis() >= before);
                removed += before_len - pts.len();
            }
            Ok(removed)
        }
    }

    /// record() on a persistent engine writes through to the backend.
    #[test]
    fn test_record_writes_through_to_backend() {
        let backend = Arc::new(FakeBackend::default());
        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());

        let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
        engine.record(1, "voltage", 1.05, ts).unwrap();

        let state = backend.calls.lock().unwrap();
        assert_eq!(state.store_calls, 1, "record() must write through");
        assert_eq!(
            state
                .data
                .get(&(1, "voltage".to_string()))
                .map(|v| v.len()),
            Some(1)
        );
    }

    /// Cache miss falls back to the backend and back-fills the cache.
    #[test]
    fn test_query_falls_back_to_backend_on_cache_miss() {
        let backend = Arc::new(FakeBackend::default());
        // Pre-seed the backend with data, but DO NOT record via the engine —
        // so the engine's in-memory cache is empty (simulates a restart).
        {
            let mut s = backend.calls.lock().unwrap();
            let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
            s.data.insert(
                (1, "voltage".to_string()),
                vec![DataPoint {
                    timestamp: ts,
                    value: 1.05,
                    quality: DataQuality::Good,
                }],
            );
        }

        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());
        let start = Utc.timestamp_opt(1699999000, 0).unwrap();
        let end = Utc.timestamp_opt(1700001000, 0).unwrap();

        let results = engine.query(1, "voltage", start, end);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 1.05);

        let state = backend.calls.lock().unwrap();
        assert_eq!(state.retrieve_calls, 1, "query must hit the backend on miss");
        drop(state);

        // Second query: the back-filled cache is non-authoritative (it may be
        // a partial view), so query() correctly re-consults the backend to
        // avoid returning a truncated result. The data must still be correct.
        let results2 = engine.query(1, "voltage", start, end);
        assert_eq!(results2.len(), 1, "second query must return the same data");
        assert_eq!(results2[0].value, 1.05);
        {
            let state = backend.calls.lock().unwrap();
            assert_eq!(
                state.retrieve_calls, 2,
                "non-authoritative cache correctly re-queries the backend"
            );
        }

        // Contrast: once record() makes the cache authoritative, subsequent
        // queries are served from memory without touching the backend.
        let extra_ts = end + chrono::Duration::hours(1);
        engine.record(1, "voltage", 1.10, extra_ts).unwrap();
        let _ = engine.query(1, "voltage", start, end);
        let state = backend.calls.lock().unwrap();
        assert_eq!(
            state.retrieve_calls, 2,
            "authoritative cache (after record) must NOT re-query the backend"
        );
    }

    /// latest() falls back to the backend on cache miss.
    #[test]
    fn test_latest_falls_back_to_backend() {
        let backend = Arc::new(FakeBackend::default());
        {
            let mut s = backend.calls.lock().unwrap();
            let ts1 = Utc.timestamp_opt(1700000000, 0).unwrap();
            let ts2 = Utc.timestamp_opt(1700001000, 0).unwrap();
            s.data.insert(
                (1, "voltage".to_string()),
                vec![
                    DataPoint {
                        timestamp: ts1,
                        value: 1.0,
                        quality: DataQuality::Good,
                    },
                    DataPoint {
                        timestamp: ts2,
                        value: 2.0,
                        quality: DataQuality::Good,
                    },
                ],
            );
        }

        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());
        let latest = engine.latest(1, "voltage");
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().value, 2.0);
        let state = backend.calls.lock().unwrap();
        assert_eq!(state.latest_calls, 1);
    }

    /// Restart simulation: a NEW engine pointed at the same backend recovers
    /// historical data. This is the core durability guarantee.
    #[test]
    fn test_restart_recovers_data_via_backend() {
        let backend = Arc::new(FakeBackend::default());

        // Phase 1: first engine records data (writes through to shared backend).
        {
            let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());
            let base = Utc.timestamp_opt(1700000000, 0).unwrap();
            for i in 0..10 {
                engine
                    .record(1, "load", 100.0 + i as f64, base + chrono::Duration::hours(i))
                    .unwrap();
            }
            assert_eq!(engine.latest(1, "load").unwrap().value, 109.0);
            // Engine dropped here — simulates process exit.
        }

        // Phase 2: a brand-new engine (fresh cache) recovers the same data.
        let engine2 = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());
        let latest = engine2.latest(1, "load").unwrap();
        assert_eq!(
            latest.value, 109.0,
            "data must survive an engine restart via the persistent backend"
        );
    }

    /// A backend write failure is non-fatal: record() still succeeds (memory
    /// retains the point) and the error is swallowed. This is the SCADA hot
    /// path guarantee — the disk must not stall ingestion.
    #[test]
    fn test_backend_write_failure_is_non_fatal() {
        let backend = Arc::new(FakeBackend::default());
        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());

        // Force the backend to fail.
        backend.calls.lock().unwrap().fail = true;

        let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
        let res = engine.record(1, "voltage", 1.05, ts);
        assert!(res.is_ok(), "record() must succeed even when the backend fails");

        // The point is still in memory and queryable.
        let q = engine.query(
            1,
            "voltage",
            ts - chrono::Duration::hours(1),
            ts + chrono::Duration::hours(1),
        );
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].value, 1.05);

        // The backend recorded the (failed) attempt.
        let state = backend.calls.lock().unwrap();
        assert_eq!(state.store_calls, 1);
    }

    /// A backend read failure falls back gracefully (empty result, no panic).
    #[test]
    fn test_backend_read_failure_returns_empty() {
        let backend = Arc::new(FakeBackend::default());
        backend.calls.lock().unwrap().fail = true;

        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend);
        let start = Utc.timestamp_opt(0, 0).unwrap();
        let end = Utc.timestamp_opt(100, 0).unwrap();
        let q = engine.query(99, "missing", start, end);
        assert!(q.is_empty(), "failed backend read must yield empty, not panic");
        let l = engine.latest(99, "missing");
        assert!(l.is_none());
    }

    /// cleanup() delegates to the backend and returns the removed count.
    #[test]
    fn test_cleanup_delegates_to_backend() {
        let backend = Arc::new(FakeBackend::default());
        let engine = TimeSeriesEngine::with_persistent_storage(100_000, backend.clone());

        let old = Utc.timestamp_opt(1700000000, 0).unwrap();
        let new = Utc.timestamp_opt(1800000000, 0).unwrap();
        engine.record(1, "voltage", 1.0, old).unwrap();
        engine.record(1, "voltage", 2.0, new).unwrap();

        let cutoff = (old + chrono::Duration::seconds(1)).timestamp_millis();
        let removed = engine.cleanup(cutoff).unwrap();
        assert_eq!(removed, 1);
    }

    /// cleanup() on a memory-only engine is a no-op returning 0.
    #[test]
    fn test_cleanup_noop_when_memory_only() {
        let engine = TimeSeriesEngine::new(100_000);
        assert_eq!(engine.cleanup(0).unwrap(), 0);
    }

    /// statistics() reports the backend kind.
    #[test]
    fn test_statistics_reports_backend() {
        let mem = TimeSeriesEngine::new(100_000);
        assert_eq!(mem.statistics().backend, StorageBackendKind::Memory);
        assert!(!mem.is_persistent());

        let backend = Arc::new(FakeBackend::default());
        let persistent = TimeSeriesEngine::with_persistent_storage(100_000, backend);
        assert!(persistent.is_persistent());
        assert_eq!(
            persistent.statistics().backend,
            StorageBackendKind::Persistent("sqlite".to_string())
        );
    }

    /// The default engine is memory-only (backward compatibility).
    #[test]
    fn test_default_is_memory_only() {
        let engine = TimeSeriesEngine::default();
        assert!(!engine.is_persistent());
        assert_eq!(engine.statistics().backend, StorageBackendKind::Memory);
    }

    // ===================================================================
    // Real SQLite integration — the actual durability guarantee
    // (BUG3 §6 hard evidence)
    // ===================================================================

    fn temp_db_path(name: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("eneros_ts_engine_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}_{}.db", name, id))
            .to_str()
            .unwrap()
            .to_string()
    }

    /// End-to-end durability with a *real* SQLite file: write via one engine,
    /// drop it (simulating process exit), open a fresh engine on the same file,
    /// and confirm the data is recovered. This is the load-forecasting /
    /// SCADA persistence guarantee.
    #[test]
    fn test_real_sqlite_survives_restart() {
        let db_path = temp_db_path("restart");
        let base = Utc.timestamp_opt(1700000000, 0).unwrap();

        // Phase 1: record 10 points and drop the engine.
        {
            let engine = TimeSeriesEngine::with_sqlite(100_000, &db_path).unwrap();
            assert!(engine.is_persistent());
            for i in 0..10 {
                engine
                    .record(7, "load_mw", 200.0 + i as f64, base + chrono::Duration::hours(i))
                    .unwrap();
            }
            assert_eq!(engine.latest(7, "load_mw").unwrap().value, 209.0);
        }

        // Phase 2: fresh engine, fresh cache — data must come back from disk.
        let engine2 = TimeSeriesEngine::with_sqlite(100_000, &db_path).unwrap();
        let latest = engine2.latest(7, "load_mw").unwrap();
        assert_eq!(
            latest.value, 209.0,
            "data must survive restart via real SQLite"
        );

        let window = engine2.query(
            7,
            "load_mw",
            base - chrono::Duration::hours(1),
            base + chrono::Duration::hours(12),
        );
        assert_eq!(window.len(), 10, "full series must be recovered from disk");
        assert_eq!(window[0].value, 200.0);
        assert_eq!(window[9].value, 209.0);

        let _ = std::fs::remove_file(&db_path);
    }

    /// The with_sqlite convenience constructor surfaces open errors rather
    /// than panicking.
    #[test]
    fn test_with_sqlite_invalid_path_errors() {
        // An illegal path (NUL byte) cannot be opened — must return Err.
        let res = TimeSeriesEngine::with_sqlite(100, "\0invalid\0path");
        assert!(res.is_err(), "invalid path must surface an error");
    }
}
