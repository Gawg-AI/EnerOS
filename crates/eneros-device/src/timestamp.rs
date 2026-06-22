//! 协议时间戳模块
//!
//! 提供协议帧接收时的精确时间戳，支持：
//! - 软件时间戳（SystemTime）
//! - 内核时间戳（SO_TIMESTAMPNS，Linux AF_PACKET）
//! - PTP 时间对齐（从 eneros-os timesync 获取偏移）

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 协议时间戳
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolTimestamp {
    /// 纳秒级 Unix 时间戳
    nanos_since_epoch: u64,
    /// 时间戳来源
    source: TimestampSource,
}

/// 时间戳来源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSource {
    /// 软件时间戳（SystemTime::now()）
    Software,
    /// 内核时间戳（SO_TIMESTAMPNS）
    Kernel,
    /// PTP 校正后的时间戳
    PtpCorrected,
}

impl ProtocolTimestamp {
    /// 从 SystemTime 创建软件时间戳
    pub fn now() -> Self {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0));
        Self {
            nanos_since_epoch: dur.as_nanos() as u64,
            source: TimestampSource::Software,
        }
    }

    /// 从纳秒 Unix 时间戳创建
    pub fn from_nanos(nanos: u64, source: TimestampSource) -> Self {
        Self {
            nanos_since_epoch: nanos,
            source,
        }
    }

    /// 从 libc::timespec 创建（SO_TIMESTAMPNS 返回的类型）
    ///
    /// 在 32 位平台上 `tv_sec` 可能为 `i32`，负值会导致 `as u64` 产生接近
    /// `u64::MAX` 的值并溢出。此处显式检查负 `tv_sec`，无效时返回零时间戳。
    #[cfg(target_os = "linux")]
    pub fn from_timespec(ts: libc::timespec, source: TimestampSource) -> Self {
        if ts.tv_sec < 0 {
            // 负时间戳无效，返回零时间戳
            return Self::from_nanos(0, TimestampSource::Software);
        }
        let nanos = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
        Self {
            nanos_since_epoch: nanos,
            source,
        }
    }

    /// 获取纳秒级 Unix 时间戳
    pub fn nanos_since_epoch(&self) -> u64 {
        self.nanos_since_epoch
    }

    /// 获取秒部分
    pub fn secs(&self) -> u64 {
        self.nanos_since_epoch / 1_000_000_000
    }

    /// 获取纳秒小数部分
    pub fn subsec_nanos(&self) -> u32 {
        (self.nanos_since_epoch % 1_000_000_000) as u32
    }

    /// 获取时间戳来源
    pub fn source(&self) -> TimestampSource {
        self.source
    }

    /// 应用 PTP 偏移校正
    ///
    /// `offset_nanos`：PTP 偏移（正数表示本地时钟快于 PTP 主时钟）。
    ///
    /// 使用 `checked_sub` / `checked_add` 避免饱和运算产生 1970 年时间戳。
    /// 偏移超限（下溢/上溢）或校正后时间戳早于 2000 年时返回 `None`。
    pub fn apply_ptp_offset(&self, offset_nanos: i64) -> Option<ProtocolTimestamp> {
        let corrected = if offset_nanos >= 0 {
            self.nanos_since_epoch
                .checked_sub(offset_nanos as u64)?
        } else {
            self.nanos_since_epoch
                .checked_add((-offset_nanos) as u64)?
        };
        // 校正后的时间戳应合理（不早于 2000-01-01 00:00:00 UTC）
        const YEAR_2000_NANOS: u64 = 946_684_800_000_000_000;
        if corrected < YEAR_2000_NANOS {
            return None;
        }
        Some(ProtocolTimestamp {
            nanos_since_epoch: corrected,
            source: TimestampSource::PtpCorrected,
        })
    }

    /// 格式化为 ISO 8601 字符串（UTC）
    ///
    /// 使用 `chrono` 进行日期转换，避免自实现算法在远未来日期上的性能与溢出问题。
    pub fn to_iso8601(&self) -> String {
        let secs = self.secs() as i64;
        let nanos = self.subsec_nanos();
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, nanos) {
            dt.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string()
        } else {
            format!("invalid_timestamp({})", self.nanos_since_epoch)
        }
    }

    /// 转换为 Duration（自 UNIX_EPOCH）
    pub fn to_duration(&self) -> Duration {
        Duration::from_nanos(self.nanos_since_epoch)
    }
}

/// PTP 时间偏移管理器
#[derive(Debug, Clone)]
pub struct PtpOffsetProvider {
    /// 当前 PTP 偏移（纳秒）
    offset_nanos: i64,
    /// 最后更新时间
    last_update: SystemTime,
}

impl PtpOffsetProvider {
    pub fn new() -> Self {
        Self {
            offset_nanos: 0,
            last_update: SystemTime::now(),
        }
    }

    /// 更新 PTP 偏移
    pub fn update_offset(&mut self, offset_nanos: i64) {
        self.offset_nanos = offset_nanos;
        self.last_update = SystemTime::now();
    }

    /// 获取当前偏移
    pub fn offset_nanos(&self) -> i64 {
        self.offset_nanos
    }

    /// 检查偏移是否过期
    ///
    /// `max_age` 为允许的最大偏移年龄。若 `last_update` 早于当前时间超过
    /// `max_age`，或系统时钟回拨导致 `duration_since` 失败，则视为过期。
    pub fn is_stale(&self, max_age: Duration) -> bool {
        SystemTime::now()
            .duration_since(self.last_update)
            .map(|elapsed| elapsed > max_age)
            .unwrap_or(true)
    }

    /// 校正时间戳
    ///
    /// 偏移过期（默认 60 秒）或校正超限时返回原时间戳，不进行校正。
    pub fn correct(&self, ts: ProtocolTimestamp) -> ProtocolTimestamp {
        if self.is_stale(Duration::from_secs(60)) {
            // 偏移过期，不校正
            return ts;
        }
        ts.apply_ptp_offset(self.offset_nanos).unwrap_or(ts)
    }
}

impl Default for PtpOffsetProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now() {
        let ts = ProtocolTimestamp::now();
        assert!(ts.nanos_since_epoch() > 1_700_000_000_000_000_000); // > 2023
        assert_eq!(ts.source(), TimestampSource::Software);
    }

    #[test]
    fn test_from_nanos() {
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        assert_eq!(ts.secs(), 1_600_000_000);
        assert_eq!(ts.subsec_nanos(), 0);
        assert_eq!(ts.source(), TimestampSource::Kernel);
    }

    #[test]
    fn test_apply_ptp_offset_positive() {
        // 本地时钟快 1ms
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        let corrected = ts.apply_ptp_offset(1_000_000).unwrap(); // +1ms
        assert_eq!(corrected.nanos_since_epoch(), 1_599_999_999_999_000_000);
        assert_eq!(corrected.source(), TimestampSource::PtpCorrected);
    }

    #[test]
    fn test_apply_ptp_offset_negative() {
        // 本地时钟慢 1ms
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        let corrected = ts.apply_ptp_offset(-1_000_000).unwrap(); // -1ms
        assert_eq!(corrected.nanos_since_epoch(), 1_600_000_000_001_000_000);
    }

    #[test]
    fn test_apply_ptp_offset_overflow() {
        // 校正后早于 2000 年，应返回 None
        let ts = ProtocolTimestamp::from_nanos(1_000_000_000_000, TimestampSource::Kernel); // ~1970
        assert!(ts.apply_ptp_offset(1_000_000).is_none());

        // 偏移大于时间戳，checked_sub 下溢返回 None
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        assert!(ts.apply_ptp_offset(i64::MAX).is_none());
    }

    #[test]
    fn test_to_iso8601() {
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Software);
        let iso = ts.to_iso8601();
        // 1600000000 = 2020-09-13 12:26:40 UTC
        assert!(iso.starts_with("2020-09-13T12:26:40"));
        assert!(iso.ends_with('Z'));
    }

    #[test]
    fn test_ptp_offset_provider() {
        let mut provider = PtpOffsetProvider::new();
        assert_eq!(provider.offset_nanos(), 0);
        provider.update_offset(500_000);
        assert_eq!(provider.offset_nanos(), 500_000);

        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        let corrected = provider.correct(ts);
        assert_eq!(corrected.nanos_since_epoch(), 1_599_999_999_999_500_000);
    }

    #[test]
    fn test_ptp_offset_provider_is_stale() {
        let mut provider = PtpOffsetProvider::new();
        provider.update_offset(1_000_000);

        // 刚更新，不应过期
        assert!(!provider.is_stale(Duration::from_secs(60)));

        // 模拟过期：将 last_update 设为 120 秒前
        provider.last_update = SystemTime::now() - Duration::from_secs(120);
        assert!(provider.is_stale(Duration::from_secs(60)));

        // 偏移过期时 correct 应返回原时间戳
        let ts = ProtocolTimestamp::from_nanos(1_600_000_000_000_000_000, TimestampSource::Kernel);
        let corrected = provider.correct(ts);
        assert_eq!(corrected.nanos_since_epoch(), 1_600_000_000_000_000_000);
        assert_eq!(corrected.source(), TimestampSource::Kernel);
    }

    #[test]
    fn test_secs_and_subsec() {
        let ts = ProtocolTimestamp::from_nanos(1_234_567_890_123_456_789, TimestampSource::Software);
        assert_eq!(ts.secs(), 1_234_567_890);
        assert_eq!(ts.subsec_nanos(), 123_456_789);
    }

    #[test]
    fn test_to_duration() {
        let ts = ProtocolTimestamp::from_nanos(1_500_000_000, TimestampSource::Software);
        let dur = ts.to_duration();
        assert_eq!(dur.as_secs(), 1);
        assert_eq!(dur.subsec_nanos(), 500_000_000);
    }

    #[test]
    fn test_from_timespec_linux() {
        #[cfg(target_os = "linux")]
        {
            let ts = libc::timespec {
                tv_sec: 1600000000,
                tv_nsec: 500_000_000,
            };
            let pts = ProtocolTimestamp::from_timespec(ts, TimestampSource::Kernel);
            assert_eq!(pts.nanos_since_epoch(), 1_600_000_000_500_000_000);
            assert_eq!(pts.source(), TimestampSource::Kernel);
        }
    }

    #[test]
    fn test_from_timespec_negative_linux() {
        #[cfg(target_os = "linux")]
        {
            // 负 tv_sec 应返回零时间戳（32 位平台溢出防护）
            let ts = libc::timespec {
                tv_sec: -1,
                tv_nsec: 500_000_000,
            };
            let pts = ProtocolTimestamp::from_timespec(ts, TimestampSource::Kernel);
            assert_eq!(pts.nanos_since_epoch(), 0);
            assert_eq!(pts.source(), TimestampSource::Software);
        }
    }
}
