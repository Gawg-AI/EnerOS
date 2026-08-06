//! 电量定点类型（v0.230.0，B3-07）。
//!
//! 对齐 ARC-003 `MilliYuan` 做法：kWh 以毫 kWh（1/1000 kWh）定点整数
//! 表示，消除浮点累计误差。换算：
//! - `MilliKwh::from_kwh(1.5) = MilliKwh(1500)`
//! - `MilliKwh(1500).as_kwh() = 1.5`

use core::fmt;
use core::ops::{Add, Sub};

/// 电量（毫 kWh 定点，1 MilliKwh = 0.001 kWh）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct MilliKwh(pub i64);

impl MilliKwh {
    /// 零电量。
    pub const ZERO: MilliKwh = MilliKwh(0);

    /// 从 kWh（f64）换算（四舍五入到毫 kWh）。
    pub fn from_kwh(kwh: f64) -> Self {
        if kwh.is_finite() {
            let scaled = kwh * 1000.0;
            MilliKwh(round_f64(scaled) as i64)
        } else {
            MilliKwh(0)
        }
    }

    /// 转 kWh（f64，仅展示/对外接口）。
    pub fn as_kwh(self) -> f64 {
        self.0 as f64 / 1000.0
    }

    /// 加法（溢出饱和）。
    pub fn saturating_add(self, rhs: MilliKwh) -> MilliKwh {
        MilliKwh(self.0.saturating_add(rhs.0))
    }

    /// 减法（下溢饱和到 0）。
    pub fn saturating_sub(self, rhs: MilliKwh) -> MilliKwh {
        MilliKwh(self.0.saturating_sub(rhs.0).max(0))
    }

    /// 绝对值。
    pub fn abs(self) -> MilliKwh {
        MilliKwh(self.0.abs())
    }
}

impl Add for MilliKwh {
    type Output = MilliKwh;
    fn add(self, rhs: MilliKwh) -> MilliKwh {
        MilliKwh(self.0.saturating_add(rhs.0))
    }
}

impl Sub for MilliKwh {
    type Output = MilliKwh;
    fn sub(self, rhs: MilliKwh) -> MilliKwh {
        MilliKwh(self.0.saturating_sub(rhs.0).max(0))
    }
}

/// 四舍五入（no_std 无 f64::round）。
fn round_f64(x: f64) -> f64 {
    if x >= 0.0 {
        ((x + 0.5) as i64) as f64
    } else {
        ((x - 0.5) as i64) as f64
    }
}

impl fmt::Display for MilliKwh {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} mWh", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_kwh_rounds() {
        assert_eq!(MilliKwh::from_kwh(1.5), MilliKwh(1500));
        assert_eq!(MilliKwh::from_kwh(0.0), MilliKwh(0));
        assert_eq!(MilliKwh::from_kwh(-2.25), MilliKwh(-2250));
    }

    #[test]
    fn test_from_kwh_non_finite_zero() {
        assert_eq!(MilliKwh::from_kwh(f64::NAN), MilliKwh(0));
        assert_eq!(MilliKwh::from_kwh(f64::INFINITY), MilliKwh(0));
    }

    #[test]
    fn test_as_kwh_roundtrip() {
        assert_eq!(MilliKwh(1500).as_kwh(), 1.5);
        assert_eq!(MilliKwh(1).as_kwh(), 0.001);
    }

    #[test]
    fn test_arith_saturating() {
        let a = MilliKwh(1000);
        let b = MilliKwh(2000);
        assert_eq!(a + b, MilliKwh(3000));
        assert_eq!(a - b, MilliKwh(0), "下溢饱和到 0");
        assert_eq!(b.saturating_sub(a), MilliKwh(1000));
    }

    #[test]
    fn test_abs_and_ordering() {
        assert_eq!(MilliKwh(-5).abs(), MilliKwh(5));
        assert!(MilliKwh(1) < MilliKwh(2));
        assert_eq!(MilliKwh::ZERO, MilliKwh(0));
    }
}
