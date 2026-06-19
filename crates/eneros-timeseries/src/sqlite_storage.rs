use eneros_core::ElementId;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

use crate::engine::{DataPoint, DataQuality};
use crate::storage::TimeSeriesStorage;

/// SQLite-backed persistent storage for time-series data
pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    /// Open or create a SQLite database at the given path
    pub fn new(db_path: &str) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;"
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS time_series (
                element_id INTEGER NOT NULL,
                parameter TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                value REAL NOT NULL,
                quality TEXT NOT NULL,
                PRIMARY KEY (element_id, parameter, timestamp)
            ) WITHOUT ROWID",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Composite index for range queries (element_id, parameter, timestamp)
        // WITHOUT ROWID + PRIMARY KEY already provides this, but add explicit
        // index on timestamp for cleanup() and latest() queries.
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_ts_time ON time_series(timestamp)",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Explicitly flush writes to disk
    pub fn flush(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn quality_to_str(q: &DataQuality) -> &'static str {
    match q {
        DataQuality::Good => "good",
        DataQuality::Uncertain => "uncertain",
        DataQuality::Bad => "bad",
    }
}

fn str_to_quality(s: &str) -> DataQuality {
    match s {
        "uncertain" => DataQuality::Uncertain,
        "bad" => DataQuality::Bad,
        _ => DataQuality::Good,
    }
}

impl TimeSeriesStorage for SqliteStorage {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO time_series (element_id, parameter, timestamp, value, quality)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                element_id,
                parameter,
                point.timestamp.to_rfc3339(),
                point.value,
                quality_to_str(&point.quality),
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let start_dt = chrono::DateTime::from_timestamp_millis(start)
            .ok_or("invalid start timestamp")?;
        let end_dt = chrono::DateTime::from_timestamp_millis(end)
            .ok_or("invalid end timestamp")?;

        let mut stmt = conn
            .prepare(
                "SELECT timestamp, value, quality FROM time_series
                 WHERE element_id = ?1 AND parameter = ?2
                 AND timestamp >= ?3 AND timestamp <= ?4
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(
                params![element_id, parameter, start_dt.to_rfc3339(), end_dt.to_rfc3339()],
                |row| {
                    let ts_str: String = row.get(0)?;
                    let value: f64 = row.get(1)?;
                    let quality_str: String = row.get(2)?;
                    Ok((ts_str, value, quality_str))
                },
            )
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for row in rows {
            let (ts_str, value, quality_str) = row.map_err(|e| e.to_string())?;
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map_err(|e| e.to_string())?
                .to_utc();
            results.push(DataPoint {
                timestamp,
                value,
                quality: str_to_quality(&quality_str),
            });
        }
        Ok(results)
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT timestamp, value, quality FROM time_series
                 WHERE element_id = ?1 AND parameter = ?2
                 ORDER BY timestamp DESC LIMIT 1",
            )
            .map_err(|e| e.to_string())?;

        let result = stmt
            .query_row(params![element_id, parameter], |row| {
                let ts_str: String = row.get(0)?;
                let value: f64 = row.get(1)?;
                let quality_str: String = row.get(2)?;
                Ok((ts_str, value, quality_str))
            })
            .optional()
            .map_err(|e| e.to_string())?;

        if let Some((ts_str, value, quality_str)) = result {
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map_err(|e| e.to_string())?
                .to_utc();
            Ok(Some(DataPoint {
                timestamp,
                value,
                quality: str_to_quality(&quality_str),
            }))
        } else {
            Ok(None)
        }
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let before_dt = chrono::DateTime::from_timestamp_millis(before)
            .ok_or("invalid before timestamp")?;

        let removed = conn
            .execute(
                "DELETE FROM time_series WHERE timestamp < ?1",
                params![before_dt.to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_db_path(name: &str) -> String {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("eneros_ts_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}_{}.db", name, id)).to_str().unwrap().to_string()
    }

    #[test]
    fn test_sqlite_store_and_retrieve() {
        let db_path = temp_db_path("store_retrieve");
        let storage = SqliteStorage::new(&db_path).unwrap();

        let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
        let point = DataPoint {
            timestamp: ts,
            value: 42.5,
            quality: DataQuality::Good,
        };

        storage.store(1, "voltage", point).unwrap();

        let start = (ts - chrono::Duration::hours(1)).timestamp_millis();
        let end = (ts + chrono::Duration::hours(1)).timestamp_millis();
        let results = storage.retrieve(1, "voltage", start, end).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 42.5);
        assert_eq!(results[0].quality, DataQuality::Good);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_sqlite_latest() {
        let db_path = temp_db_path("latest");
        let storage = SqliteStorage::new(&db_path).unwrap();

        let ts1 = Utc.timestamp_opt(1700000000, 0).unwrap();
        let ts2 = Utc.timestamp_opt(1700001000, 0).unwrap();

        storage
            .store(1, "voltage", DataPoint {
                timestamp: ts1,
                value: 10.0,
                quality: DataQuality::Good,
            })
            .unwrap();
        storage
            .store(1, "voltage", DataPoint {
                timestamp: ts2,
                value: 20.0,
                quality: DataQuality::Uncertain,
            })
            .unwrap();

        let latest = storage.latest(1, "voltage").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().value, 20.0);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_sqlite_cleanup() {
        let db_path = temp_db_path("cleanup");
        let storage = SqliteStorage::new(&db_path).unwrap();

        let old_ts = Utc.timestamp_opt(1700000000, 0).unwrap();
        let new_ts = Utc.timestamp_opt(1800000000, 0).unwrap();

        storage
            .store(1, "voltage", DataPoint {
                timestamp: old_ts,
                value: 10.0,
                quality: DataQuality::Good,
            })
            .unwrap();
        storage
            .store(1, "voltage", DataPoint {
                timestamp: new_ts,
                value: 20.0,
                quality: DataQuality::Good,
            })
            .unwrap();

        let cutoff = (old_ts + chrono::Duration::seconds(1)).timestamp_millis();
        let removed = storage.cleanup(cutoff).unwrap();
        assert_eq!(removed, 1);

        let latest = storage.latest(1, "voltage").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().value, 20.0);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_sqlite_round_trip() {
        let db_path = temp_db_path("round_trip");

        // Store data
        {
            let storage = SqliteStorage::new(&db_path).unwrap();
            let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
            storage
                .store(1, "current", DataPoint {
                    timestamp: ts,
                    value: 99.9,
                    quality: DataQuality::Bad,
                })
                .unwrap();
            storage.flush().unwrap();
        }

        // Reopen and verify
        {
            let storage = SqliteStorage::new(&db_path).unwrap();
            let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
            let start = (ts - chrono::Duration::hours(1)).timestamp_millis();
            let end = (ts + chrono::Duration::hours(1)).timestamp_millis();
            let results = storage.retrieve(1, "current", start, end).unwrap();

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].value, 99.9);
            assert_eq!(results[0].quality, DataQuality::Bad);
        }

        let _ = std::fs::remove_file(&db_path);
    }
}
