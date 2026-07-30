//! DR 信号验证与激活判断

use crate::types::DrSignal;
use crate::DrError;

/// 验证 DR 信号的有效性
///
/// 检查条件：
/// - target_kw > 0
/// - end_at > start_at
/// - signal_type 为有效类型（枚举保证）
pub fn validate_signal(sig: &DrSignal) -> Result<(), DrError> {
    if sig.target_kw <= 0.0 || !sig.target_kw.is_finite() {
        return Err(DrError::InvalidSignal);
    }
    if sig.end_at <= sig.start_at {
        return Err(DrError::InvalidSignal);
    }
    Ok(())
}

/// 判断 DR 事件在当前时刻是否激活
///
/// 当 `start_at <= now < end_at` 时返回 true。
pub fn is_active(sig: &DrSignal, now: u64) -> bool {
    now >= sig.start_at && now < sig.end_at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DrSignalType;

    fn make_signal(target_kw: f64, start_at: u64, end_at: u64) -> DrSignal {
        DrSignal {
            event_id: 1,
            signal_type: DrSignalType::LoadReduce,
            start_at,
            end_at,
            target_kw,
            incentive_yuan: 100.0,
        }
    }

    #[test]
    fn test_validate_signal_ok() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(validate_signal(&sig).is_ok());
    }

    #[test]
    fn test_validate_signal_zero_target() {
        let sig = make_signal(0.0, 1000, 2000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_validate_signal_negative_target() {
        let sig = make_signal(-10.0, 1000, 2000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_validate_signal_nan_target() {
        let sig = make_signal(f64::NAN, 1000, 2000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_validate_signal_inf_target() {
        let sig = make_signal(f64::INFINITY, 1000, 2000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_validate_signal_end_before_start() {
        let sig = make_signal(500.0, 2000, 1000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_validate_signal_end_equals_start() {
        let sig = make_signal(500.0, 1000, 1000);
        assert_eq!(validate_signal(&sig), Err(DrError::InvalidSignal));
    }

    #[test]
    fn test_is_active_before_start() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(!is_active(&sig, 999));
    }

    #[test]
    fn test_is_active_at_start() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(is_active(&sig, 1000));
    }

    #[test]
    fn test_is_active_during() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(is_active(&sig, 1500));
    }

    #[test]
    fn test_is_active_at_end() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(!is_active(&sig, 2000));
    }

    #[test]
    fn test_is_active_after_end() {
        let sig = make_signal(500.0, 1000, 2000);
        assert!(!is_active(&sig, 2001));
    }
}
