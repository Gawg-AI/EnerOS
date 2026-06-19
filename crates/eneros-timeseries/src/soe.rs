//! SOE (Sequence of Events) event sequence recorder (v0.10.0 — Task 4).
//!
//! Provides 1ms-precision event recording with a global atomic sequence number
//! and dual storage backends (in-memory and SQLite). Designed for breaker
//! state-change detection, protection trips, alarms and manual operations.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use utoipa::ToSchema;

// ---------------------------------------------------------------------------
// Event type
// ---------------------------------------------------------------------------

/// SOE event type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoeEventType {
    BreakerOpen,
    BreakerClose,
    ProtectionTrip,
    Alarm,
    Manual,
}

impl SoeEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SoeEventType::BreakerOpen => "breaker_open",
            SoeEventType::BreakerClose => "breaker_close",
            SoeEventType::ProtectionTrip => "protection_trip",
            SoeEventType::Alarm => "alarm",
            SoeEventType::Manual => "manual",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "breaker_open" => Some(SoeEventType::BreakerOpen),
            "breaker_close" => Some(SoeEventType::BreakerClose),
            "protection_trip" => Some(SoeEventType::ProtectionTrip),
            "alarm" => Some(SoeEventType::Alarm),
            "manual" => Some(SoeEventType::Manual),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

/// SOE record with 1ms precision timestamp and global sequence number
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SoeRecord {
    pub sequence_number: u64,
    pub timestamp: DateTime<Utc>,
    pub device_id: String,
    pub event_type: SoeEventType,
    pub priority: u8,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Storage backend
// ---------------------------------------------------------------------------

/// Storage backend for SOE records.
///
/// `Memory` keeps records in a `parking_lot::RwLock<Vec<SoeRecord>>` for fast
/// tests and ephemeral runs. `Sqlite` wraps a `rusqlite::Connection` in a
/// `std::sync::Mutex` (rusqlite's `Connection` is `!Sync`).
pub enum SoeStorage {
    Memory(RwLock<Vec<SoeRecord>>),
    Sqlite(Mutex<Connection>),
}

impl SoeStorage {
    /// Initialize a SQLite-backed storage at `db_path`, creating the
    /// `soe_events` table and supporting indexes if they don't exist.
    fn new_sqlite(db_path: &str) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS soe_events (
                sequence_number INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                device_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                priority INTEGER NOT NULL,
                value TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_soe_time ON soe_events(timestamp)",
            [],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_soe_device ON soe_events(device_id)",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(SoeStorage::Sqlite(Mutex::new(conn)))
    }

    /// Append a record to the underlying storage.
    fn append(&self, record: SoeRecord) -> Result<(), String> {
        match self {
            SoeStorage::Memory(lock) => {
                lock.write().push(record);
                Ok(())
            }
            SoeStorage::Sqlite(mutex) => {
                let conn = mutex.lock().map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT OR REPLACE INTO soe_events
                        (sequence_number, timestamp, device_id, event_type, priority, value)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        record.sequence_number as i64,
                        record.timestamp.to_rfc3339(),
                        record.device_id,
                        record.event_type.as_str(),
                        record.priority as i64,
                        record.value,
                    ],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            }
        }
    }

    /// Query records by time range with optional device_id / event_type filters.
    /// Results are sorted by sequence_number ascending.
    fn query(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        device_id: Option<&str>,
        event_type: Option<&SoeEventType>,
    ) -> Result<Vec<SoeRecord>, String> {
        match self {
            SoeStorage::Memory(lock) => {
                let guard = lock.read();
                let mut results: Vec<SoeRecord> = guard
                    .iter()
                    .filter(|r| r.timestamp >= start && r.timestamp <= end)
                    .filter(|r| device_id.is_none_or(|d| r.device_id == d))
                    .filter(|r| event_type.is_none_or(|et| &r.event_type == et))
                    .cloned()
                    .collect();
                results.sort_by_key(|r| r.sequence_number);
                Ok(results)
            }
            SoeStorage::Sqlite(mutex) => {
                let conn = mutex.lock().map_err(|e| e.to_string())?;
                let mut sql = String::from(
                    "SELECT sequence_number, timestamp, device_id, event_type, priority, value
                     FROM soe_events
                     WHERE timestamp >= ?1 AND timestamp <= ?2",
                );
                let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![
                    Box::new(start.to_rfc3339()),
                    Box::new(end.to_rfc3339()),
                ];
                if let Some(d) = device_id {
                    sql.push_str(" AND device_id = ?3");
                    params_vec.push(Box::new(d.to_string()));
                }
                if let Some(et) = event_type {
                    sql.push_str(" AND event_type = ?4");
                    params_vec.push(Box::new(et.as_str().to_string()));
                }
                sql.push_str(" ORDER BY sequence_number ASC");

                let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    params_vec.iter().map(|p| p.as_ref()).collect();
                let rows = stmt
                    .query_map(param_refs.as_slice(), |row| {
                        let seq: i64 = row.get(0)?;
                        let ts_str: String = row.get(1)?;
                        let device_id: String = row.get(2)?;
                        let event_type_str: String = row.get(3)?;
                        let priority: i64 = row.get(4)?;
                        let value: String = row.get(5)?;
                        Ok((seq, ts_str, device_id, event_type_str, priority, value))
                    })
                    .map_err(|e| e.to_string())?;

                let mut results = Vec::new();
                for row in rows {
                    let (seq, ts_str, device_id, event_type_str, priority, value) =
                        row.map_err(|e| e.to_string())?;
                    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                        .map_err(|e| e.to_string())?
                        .with_timezone(&Utc);
                    let event_type = SoeEventType::from_str(&event_type_str)
                        .ok_or_else(|| format!("unknown event_type '{}'", event_type_str))?;
                    results.push(SoeRecord {
                        sequence_number: seq as u64,
                        timestamp,
                        device_id,
                        event_type,
                        priority: priority as u8,
                        value,
                    });
                }
                Ok(results)
            }
        }
    }

    /// Return the most recent `limit` records, ordered by sequence_number
    /// descending (newest first).
    fn latest(&self, limit: usize) -> Result<Vec<SoeRecord>, String> {
        match self {
            SoeStorage::Memory(lock) => {
                let guard = lock.read();
                let mut records: Vec<SoeRecord> = guard.iter().cloned().collect();
                records.sort_by_key(|r| std::cmp::Reverse(r.sequence_number));
                records.truncate(limit);
                Ok(records)
            }
            SoeStorage::Sqlite(mutex) => {
                let conn = mutex.lock().map_err(|e| e.to_string())?;
                let mut stmt = conn
                    .prepare(
                        "SELECT sequence_number, timestamp, device_id, event_type, priority, value
                         FROM soe_events
                         ORDER BY sequence_number DESC
                         LIMIT ?1",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![limit as i64], |row| {
                        let seq: i64 = row.get(0)?;
                        let ts_str: String = row.get(1)?;
                        let device_id: String = row.get(2)?;
                        let event_type_str: String = row.get(3)?;
                        let priority: i64 = row.get(4)?;
                        let value: String = row.get(5)?;
                        Ok((seq, ts_str, device_id, event_type_str, priority, value))
                    })
                    .map_err(|e| e.to_string())?;

                let mut results = Vec::new();
                for row in rows {
                    let (seq, ts_str, device_id, event_type_str, priority, value) =
                        row.map_err(|e| e.to_string())?;
                    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                        .map_err(|e| e.to_string())?
                        .with_timezone(&Utc);
                    let event_type = SoeEventType::from_str(&event_type_str)
                        .ok_or_else(|| format!("unknown event_type '{}'", event_type_str))?;
                    results.push(SoeRecord {
                        sequence_number: seq as u64,
                        timestamp,
                        device_id,
                        event_type,
                        priority: priority as u8,
                        value,
                    });
                }
                Ok(results)
            }
        }
    }

    /// Return the total number of stored records.
    fn count(&self) -> Result<usize, String> {
        match self {
            SoeStorage::Memory(lock) => Ok(lock.read().len()),
            SoeStorage::Sqlite(mutex) => {
                let conn = mutex.lock().map_err(|e| e.to_string())?;
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM soe_events", [], |row| row.get(0))
                    .map_err(|e| e.to_string())?;
                Ok(count as usize)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// SOE recorder with global atomic sequence and SQLite persistence.
///
/// The sequence number is a process-global monotonic counter backed by an
/// `AtomicU64`. `fetch_add(1, Relaxed)` is sufficient because each record
/// receives a unique number; ordering across threads is not required for
/// correctness — only uniqueness and monotonicity within a thread.
pub struct SoeRecorder {
    sequence: AtomicU64,
    storage: SoeStorage,
}

impl SoeRecorder {
    /// Create a recorder backed by an in-memory `Vec<SoeRecord>`.
    pub fn new_memory() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            storage: SoeStorage::Memory(RwLock::new(Vec::new())),
        }
    }

    /// Create a recorder backed by a SQLite database at `db_path`.
    /// Creates the `soe_events` table and indexes if they don't exist.
    pub fn new_sqlite(db_path: &str) -> Result<Self, String> {
        let storage = SoeStorage::new_sqlite(db_path)?;
        Ok(Self {
            sequence: AtomicU64::new(0),
            storage,
        })
    }

    /// Record an event with an explicit timestamp.
    ///
    /// Allocates the next global sequence number via `fetch_add`, persists
    /// the record, and returns a clone of the stored record.
    pub fn record(
        &self,
        device_id: &str,
        event_type: SoeEventType,
        priority: u8,
        value: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<SoeRecord, String> {
        let sequence_number = self.sequence.fetch_add(1, Ordering::Relaxed);
        let record = SoeRecord {
            sequence_number,
            timestamp,
            device_id: device_id.to_string(),
            event_type,
            priority,
            value: value.to_string(),
        };
        self.storage.append(record.clone())?;
        Ok(record)
    }

    /// Record an event using `Utc::now()` as the timestamp.
    pub fn record_now(
        &self,
        device_id: &str,
        event_type: SoeEventType,
        priority: u8,
        value: &str,
    ) -> Result<SoeRecord, String> {
        self.record(device_id, event_type, priority, value, Utc::now())
    }

    /// Query events by time range with optional device_id / event_type filters.
    /// Results are sorted by sequence_number ascending.
    pub fn query(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        device_id: Option<&str>,
        event_type: Option<&SoeEventType>,
    ) -> Result<Vec<SoeRecord>, String> {
        self.storage.query(start, end, device_id, event_type)
    }

    /// Return the most recent `limit` events (newest first).
    pub fn latest(&self, limit: usize) -> Result<Vec<SoeRecord>, String> {
        self.storage.latest(limit)
    }

    /// Total number of stored events.
    pub fn count(&self) -> Result<usize, String> {
        self.storage.count()
    }
}

impl Default for SoeRecorder {
    fn default() -> Self {
        Self::new_memory()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Generate a unique temporary SQLite path for a test.
    fn temp_db_path(name: &str) -> String {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("eneros_soe_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}_{}.db", name, id))
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn test_soe_record_sequence_increment() {
        let recorder = SoeRecorder::new_memory();
        let ts = Utc::now();
        let r1 = recorder
            .record("dev1", SoeEventType::BreakerOpen, 1, "1 -> 0", ts)
            .unwrap();
        let r2 = recorder
            .record("dev1", SoeEventType::BreakerClose, 1, "0 -> 1", ts)
            .unwrap();
        let r3 = recorder
            .record("dev2", SoeEventType::Alarm, 2, "overload", ts)
            .unwrap();
        assert_eq!(r1.sequence_number, 0);
        assert_eq!(r2.sequence_number, 1);
        assert_eq!(r3.sequence_number, 2);
    }

    #[test]
    fn test_soe_memory_storage_and_query() {
        let recorder = SoeRecorder::new_memory();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let t1 = t0 + chrono::Duration::seconds(10);
        let t2 = t0 + chrono::Duration::seconds(20);

        recorder
            .record("dev1", SoeEventType::BreakerOpen, 1, "1 -> 0", t0)
            .unwrap();
        recorder
            .record("dev1", SoeEventType::BreakerClose, 1, "0 -> 1", t1)
            .unwrap();
        recorder
            .record("dev2", SoeEventType::Alarm, 2, "warn", t2)
            .unwrap();

        let results = recorder
            .query(t0 - chrono::Duration::seconds(1), t2 + chrono::Duration::seconds(1), None, None)
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].sequence_number, 0);
        assert_eq!(results[2].sequence_number, 2);
    }

    #[test]
    fn test_soe_sqlite_storage_and_query() {
        let db_path = temp_db_path("sqlite_storage");
        {
            let recorder = SoeRecorder::new_sqlite(&db_path).unwrap();
            let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
            recorder
                .record("dev1", SoeEventType::BreakerOpen, 1, "1 -> 0", t0)
                .unwrap();
            recorder
                .record("dev2", SoeEventType::Manual, 3, "operator", t0 + chrono::Duration::seconds(5))
                .unwrap();

            let results = recorder
                .query(
                    t0 - chrono::Duration::seconds(1),
                    t0 + chrono::Duration::seconds(60),
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].device_id, "dev1");
            assert_eq!(results[1].device_id, "dev2");
        }
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path));
        let _ = std::fs::remove_file(format!("{}-shm", db_path));
    }

    #[test]
    fn test_soe_query_by_device_id() {
        let recorder = SoeRecorder::new_memory();
        let ts = Utc::now();
        recorder.record("devA", SoeEventType::Alarm, 1, "a", ts).unwrap();
        recorder.record("devB", SoeEventType::Alarm, 1, "b", ts).unwrap();
        recorder.record("devA", SoeEventType::Alarm, 1, "c", ts).unwrap();

        let results = recorder
            .query(ts - chrono::Duration::seconds(1), ts + chrono::Duration::seconds(1), Some("devA"), None)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.device_id == "devA"));
    }

    #[test]
    fn test_soe_query_by_time_range() {
        let recorder = SoeRecorder::new_memory();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        recorder.record("d", SoeEventType::Alarm, 1, "1", t0).unwrap();
        recorder
            .record("d", SoeEventType::Alarm, 1, "2", t0 + chrono::Duration::seconds(30))
            .unwrap();
        recorder
            .record("d", SoeEventType::Alarm, 1, "3", t0 + chrono::Duration::seconds(60))
            .unwrap();

        // Window [t0+10s, t0+40s] should include only the second event.
        let results = recorder
            .query(
                t0 + chrono::Duration::seconds(10),
                t0 + chrono::Duration::seconds(40),
                None,
                None,
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "2");
    }

    #[test]
    fn test_soe_latest_limit() {
        let recorder = SoeRecorder::new_memory();
        let ts = Utc::now();
        for i in 0..10 {
            recorder
                .record("d", SoeEventType::Alarm, 1, &format!("v{}", i), ts)
                .unwrap();
        }
        let latest_three = recorder.latest(3).unwrap();
        assert_eq!(latest_three.len(), 3);
        // Newest first: sequence numbers 9, 8, 7
        assert_eq!(latest_three[0].sequence_number, 9);
        assert_eq!(latest_three[1].sequence_number, 8);
        assert_eq!(latest_three[2].sequence_number, 7);
    }

    #[test]
    fn test_soe_event_type_from_str() {
        // Round-trip every variant through as_str / from_str.
        let variants = vec![
            SoeEventType::BreakerOpen,
            SoeEventType::BreakerClose,
            SoeEventType::ProtectionTrip,
            SoeEventType::Alarm,
            SoeEventType::Manual,
        ];
        for v in variants {
            let s = v.as_str();
            assert_eq!(SoeEventType::from_str(s), Some(v.clone()));
        }
        assert_eq!(SoeEventType::from_str("unknown"), None);

        // serde round-trip
        let json = serde_json::to_string(&SoeEventType::ProtectionTrip).unwrap();
        assert_eq!(json, "\"protection_trip\"");
        let parsed: SoeEventType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SoeEventType::ProtectionTrip);
    }

    #[test]
    fn test_soe_count() {
        let recorder = SoeRecorder::new_memory();
        assert_eq!(recorder.count().unwrap(), 0);
        let ts = Utc::now();
        for _ in 0..5 {
            recorder.record("d", SoeEventType::Alarm, 1, "x", ts).unwrap();
        }
        assert_eq!(recorder.count().unwrap(), 5);
    }

    #[test]
    fn test_soe_query_by_event_type() {
        let recorder = SoeRecorder::new_memory();
        let ts = Utc::now();
        recorder.record("d", SoeEventType::BreakerOpen, 1, "a", ts).unwrap();
        recorder.record("d", SoeEventType::Alarm, 2, "b", ts).unwrap();
        recorder.record("d", SoeEventType::BreakerClose, 1, "c", ts).unwrap();

        let et = SoeEventType::BreakerOpen;
        let results = recorder
            .query(ts - chrono::Duration::seconds(1), ts + chrono::Duration::seconds(1), None, Some(&et))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event_type, SoeEventType::BreakerOpen);
    }

    #[test]
    fn test_soe_default_is_memory() {
        let recorder = SoeRecorder::default();
        let ts = Utc::now();
        recorder.record("d", SoeEventType::Manual, 1, "x", ts).unwrap();
        assert_eq!(recorder.count().unwrap(), 1);
    }

    #[test]
    fn test_soe_record_now_uses_current_time() {
        let recorder = SoeRecorder::new_memory();
        let before = Utc::now();
        let record = recorder
            .record_now("d", SoeEventType::Manual, 1, "now")
            .unwrap();
        let after = Utc::now();
        assert!(record.timestamp >= before);
        assert!(record.timestamp <= after);
    }
}
