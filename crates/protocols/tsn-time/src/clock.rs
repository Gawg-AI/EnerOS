//! Clock identity, MAC address, and PTP time types for gPTP (IEEE 802.1AS).
//!
//! - [`ClockIdentity`] — EUI-64 定长 8 字节时钟标识（D13）
//! - [`MacAddr`] — 定长 6 字节 MAC 地址（D14）
//! - [`PtpTime`] — PTP 时间戳（秒 + 纳秒），支持加减与差值运算

use core::fmt;

/// EUI-64 时钟标识（D13：定长 8 字节数组，无 `uuid` 依赖）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockIdentity(pub [u8; 8]);

impl ClockIdentity {
    /// 以 8 字节数组构造时钟标识.
    pub fn new(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for ClockIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        write!(f, "{:02X}", bytes[0])?;
        for b in &bytes[1..] {
            write!(f, ":{:02X}", b)?;
        }
        Ok(())
    }
}

/// MAC 地址（D14：定长 6 字节数组）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    /// 以 6 字节数组构造 MAC 地址.
    pub fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        write!(f, "{:02X}", bytes[0])?;
        for b in &bytes[1..] {
            write!(f, ":{:02X}", b)?;
        }
        Ok(())
    }
}

/// PTP 时间戳（秒 + 纳秒）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtpTime {
    /// 自 epoch 起的完整秒数.
    pub seconds: u64,
    /// 秒内纳秒部分 `[0, 1_000_000_000)`.
    pub nanos: u32,
}

impl PtpTime {
    /// 构造 PTP 时间戳.
    pub fn new(seconds: u64, nanos: u32) -> Self {
        Self { seconds, nanos }
    }

    /// 转换为纳秒总数（`i128`，覆盖大时间跨度）.
    pub fn to_ns(&self) -> i128 {
        self.seconds as i128 * 1_000_000_000 + self.nanos as i128
    }

    /// 原地加上有符号纳秒偏移：正向进位到秒，负向从秒借位；
    /// 若结果为非正数则饱和到 `seconds=0, nanos=0`.
    pub fn add_ns(&mut self, ns: i64) {
        let total_ns = self.to_ns() + ns as i128;
        if total_ns <= 0 {
            self.seconds = 0;
            self.nanos = 0;
        } else {
            // i128 → u128 在测试用例的范围内不会溢出
            let total_u128 = total_ns as u128;
            self.seconds = (total_u128 / 1_000_000_000) as u64;
            self.nanos = (total_u128 % 1_000_000_000) as u32;
        }
    }

    /// 计算 `self - other` 的纳秒差（`i64`，测试用例范围内不会溢出）.
    pub fn diff_ns(&self, other: &PtpTime) -> i64 {
        (self.to_ns() - other.to_ns()) as i64
    }
}
