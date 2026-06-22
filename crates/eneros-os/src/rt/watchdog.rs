#[cfg(target_os = "linux")]
use std::fs::OpenOptions;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Maximum number of entries retained in the in-memory ring buffer.
const MAX_LOG_ENTRIES: usize = 100;

/// Hardware watchdog timer
pub struct HardwareWatchdog {
    #[cfg(target_os = "linux")]
    fd: Option<std::fs::File>,
    timeout_ms: u32,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    logger: Option<WatchdogLogger>,
}

impl HardwareWatchdog {
    /// Open the hardware watchdog device without a logger.
    pub fn open(path: &Path, timeout_ms: u32) -> Result<Self, WatchdogError> {
        Self::open_with_logger(path, timeout_ms, None)
    }

    /// Open the hardware watchdog device with an optional event logger.
    ///
    /// When `logger` is `Some`, keepalive failures are recorded for post-mortem
    /// analysis. The existing [`open`](Self::open) API is preserved (logger = `None`).
    pub fn open_with_logger(
        path: &Path,
        timeout_ms: u32,
        logger: Option<WatchdogLogger>,
    ) -> Result<Self, WatchdogError> {
        #[cfg(target_os = "linux")]
        {
            let file = OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(WatchdogError::OpenFailed)?;

            // Set timeout via ioctl
            let fd = file.as_raw_fd();
            let timeout_secs = (timeout_ms / 1000) as i32;
            // WDIOC_SETTIMEOUT = 0xC0045706
            const WDIOC_SETTIMEOUT: u64 = 0xC0045706;
            let ret = unsafe { libc::ioctl(fd, WDIOC_SETTIMEOUT, &timeout_secs) };
            if ret != 0 {
                return Err(WatchdogError::SetTimeoutFailed(std::io::Error::last_os_error()));
            }

            Ok(Self {
                fd: Some(file),
                timeout_ms,
                logger,
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            // Non-Linux: no-op for development
            let _ = path;
            Ok(Self {
                timeout_ms,
                logger,
            })
        }
    }

    /// Keep the watchdog alive (must be called periodically).
    ///
    /// On failure, the event is recorded to the attached logger (if any) as
    /// `keepalive_missed` for post-mortem analysis.
    pub fn keepalive(&mut self) -> Result<(), WatchdogError> {
        #[cfg(target_os = "linux")]
        {
            if let Some(ref mut file) = self.fd {
                // Write a single byte to keep alive
                if let Err(e) = file.write_all(&[0]) {
                    if let Some(ref logger) = self.logger {
                        logger.record("keepalive_missed", &e.to_string());
                    }
                    return Err(WatchdogError::KeepaliveFailed(e));
                }
            }
        }
        Ok(())
    }

    /// Disable the watchdog (magic close)
    pub fn disable(self) -> Result<(), WatchdogError> {
        #[cfg(target_os = "linux")]
        {
            if let Some(mut file) = self.fd {
                // Write 'V' to disable watchdog before close
                file.write_all(b"V").map_err(WatchdogError::DisableFailed)?;
            }
        }
        Ok(())
    }

    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WatchdogError {
    #[error("failed to open watchdog device: {0}")]
    OpenFailed(std::io::Error),
    #[error("failed to set timeout: {0}")]
    SetTimeoutFailed(std::io::Error),
    #[error("keepalive failed: {0}")]
    KeepaliveFailed(std::io::Error),
    #[error("disable failed: {0}")]
    DisableFailed(std::io::Error),
    #[error("failed to persist watchdog log: {0}")]
    PersistFailed(std::io::Error),
}

/// A single watchdog event record, serialized as JSONL on disk for post-mortem
/// analysis after a hardware reset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogLogEntry {
    /// ISO 8601 timestamp of the event.
    pub timestamp: String,
    /// Event kind: `"timeout"`, `"keepalive_missed"`, `"recovered"`.
    pub event: String,
    /// Human-readable detail.
    pub detail: String,
}

/// In-memory ring buffer of recent watchdog events with optional JSONL
/// persistence. Retains at most `MAX_LOG_ENTRIES` entries; the oldest entry
/// is evicted once the limit is exceeded.
pub struct WatchdogLogger {
    entries: Mutex<Vec<WatchdogLogEntry>>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    log_path: PathBuf,
}

impl WatchdogLogger {
    /// Create a new logger that persists to `log_path` (JSONL format).
    pub fn new(log_path: PathBuf) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            log_path,
        }
    }

    /// Record a new event. The ring buffer keeps at most `MAX_LOG_ENTRIES`
    /// entries; the oldest is dropped when the limit is exceeded.
    pub fn record(&self, event: &str, detail: &str) {
        let entry = WatchdogLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            event: event.to_string(),
            detail: detail.to_string(),
        };
        let mut entries = self.entries.lock();
        entries.push(entry);
        if entries.len() > MAX_LOG_ENTRIES {
            entries.remove(0);
        }
    }

    /// Append all buffered entries to `log_path` in JSONL format, then clear
    /// the buffer.
    ///
    /// On non-Linux platforms this is a no-op (development environment).
    pub fn persist(&self) -> Result<(), WatchdogError> {
        #[cfg(target_os = "linux")]
        {
            let mut entries = self.entries.lock();
            if entries.is_empty() {
                return Ok(());
            }
            if let Some(parent) = self.log_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(WatchdogError::PersistFailed)?;
                }
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)
                .map_err(WatchdogError::PersistFailed)?;
            for entry in entries.iter() {
                let line = serde_json::to_string(entry).map_err(|e| {
                    WatchdogError::PersistFailed(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                file.write_all(line.as_bytes()).map_err(WatchdogError::PersistFailed)?;
                file.write_all(b"\n").map_err(WatchdogError::PersistFailed)?;
            }
            entries.clear();
        }
        #[cfg(not(target_os = "linux"))]
        {
            // No-op on non-Linux (development environment).
        }
        Ok(())
    }

    /// Return a snapshot of the most recent buffered entries (oldest first).
    pub fn load_recent(&self) -> Vec<WatchdogLogEntry> {
        self.entries.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watchdog_creation_non_linux() {
        // On non-Linux, this should succeed with no-op
        let wd = HardwareWatchdog::open(Path::new("/dev/watchdog"), 500).unwrap();
        assert_eq!(wd.timeout_ms(), 500);
    }

    #[test]
    fn test_watchdog_logger_ring_buffer_evicts_oldest() {
        let logger = WatchdogLogger::new(PathBuf::from("/tmp/eneros-watchdog-test.jsonl"));
        for i in 0..(MAX_LOG_ENTRIES + 10) {
            logger.record("keepalive_missed", &format!("iter {}", i));
        }
        let recent = logger.load_recent();
        assert_eq!(recent.len(), MAX_LOG_ENTRIES);
        // Oldest entries evicted; first remaining is iter 10.
        assert_eq!(recent[0].detail, "iter 10");
        assert_eq!(recent[0].event, "keepalive_missed");
    }
}
