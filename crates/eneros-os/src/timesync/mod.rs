//! Time synchronization service (PTP/NTP)

pub mod ptp;
pub mod ntp;

pub use ptp::{PtpClient, PtpStatus};
pub use ntp::{NtpClient, NtpStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSyncSource {
    Ptp,
    Ntp,
    LocalClock,
}

#[derive(Debug, Clone)]
pub struct TimeSyncStatus {
    pub source: TimeSyncSource,
    pub offset_micros: i64,
    pub last_sync: chrono::DateTime<chrono::Utc>,
}
