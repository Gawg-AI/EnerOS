//! 调频（频率调节）控制
//!
//! 基于死区 + 下垂特性的频率响应计算。

use crate::types::FreqRegState;
use crate::AncillaryError;

/// 频率调节器 trait
pub trait FrequencyRegulator {
    /// 根据频率偏差计算调节功率 (kW)
    ///
    /// 正偏差表示频率偏高（需减出力），负偏差表示频率偏低（需增出力）。
    fn on_freq_deviation(&mut self, deviation_hz: f64) -> Result<f64, AncillaryError>;
}

/// 基于下垂特性的调频实现
#[derive(Debug, Clone)]
pub struct FreqRegImpl {
    /// 调频状态参数
    pub state: FreqRegState,
}

impl FreqRegImpl {
    /// 创建调频实现
    pub fn new(state: FreqRegState) -> Self {
        Self { state }
    }
}

impl FrequencyRegulator for FreqRegImpl {
    fn on_freq_deviation(&mut self, deviation_hz: f64) -> Result<f64, AncillaryError> {
        if !deviation_hz.is_finite() {
            return Err(AncillaryError::InvalidFrequency);
        }

        // 死区内不响应
        if deviation_hz.abs() < self.state.deadband_hz {
            self.state.last_target_kw = 0.0;
            return Ok(0.0);
        }

        // 下垂计算: power = -deviation / (droop_pct/100) * max_power / 50.0
        let droop_ratio = self.state.droop_pct / 100.0;
        if droop_ratio.abs() < f64::EPSILON {
            return Err(AncillaryError::InvalidFrequency);
        }

        let raw_power = -deviation_hz / droop_ratio * self.state.max_power_kw / 50.0;

        // 限幅到 max_power
        let clamped = raw_power.clamp(-self.state.max_power_kw, self.state.max_power_kw);
        self.state.last_target_kw = clamped;
        Ok(clamped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> FreqRegState {
        FreqRegState {
            deadband_hz: 0.033,
            droop_pct: 5.0,
            max_power_kw: 1000.0,
            last_target_kw: 0.0,
        }
    }

    #[test]
    fn test_within_deadband_returns_zero() {
        let mut reg = FreqRegImpl::new(default_state());
        let result = reg.on_freq_deviation(0.02).unwrap();
        assert!((result - 0.0).abs() < f64::EPSILON);
        assert!((reg.state.last_target_kw - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_outside_deadband_positive_deviation() {
        let mut reg = FreqRegImpl::new(default_state());
        // deviation = 0.1 Hz, droop = 5%, max = 1000 kW
        // power = -0.1 / 0.05 * 1000 / 50 = -40 kW
        let result = reg.on_freq_deviation(0.1).unwrap();
        assert!((result - (-40.0)).abs() < 1e-9);
    }

    #[test]
    fn test_outside_deadband_negative_deviation() {
        let mut reg = FreqRegImpl::new(default_state());
        // deviation = -0.1 Hz → power = +40 kW
        let result = reg.on_freq_deviation(-0.1).unwrap();
        assert!((result - 40.0).abs() < 1e-9);
    }

    #[test]
    fn test_clamping_to_max_power() {
        let mut reg = FreqRegImpl::new(default_state());
        // deviation = 5.0 Hz → raw = -5.0/0.05*1000/50 = -2000, clamped to -1000
        let result = reg.on_freq_deviation(5.0).unwrap();
        assert!((result - (-1000.0)).abs() < 1e-9);
    }

    #[test]
    fn test_clamping_positive_max() {
        let mut reg = FreqRegImpl::new(default_state());
        // deviation = -5.0 Hz → raw = +2000, clamped to +1000
        let result = reg.on_freq_deviation(-5.0).unwrap();
        assert!((result - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn test_invalid_frequency_nan() {
        let mut reg = FreqRegImpl::new(default_state());
        let result = reg.on_freq_deviation(f64::NAN);
        assert_eq!(result.unwrap_err(), AncillaryError::InvalidFrequency);
    }

    #[test]
    fn test_invalid_frequency_infinite() {
        let mut reg = FreqRegImpl::new(default_state());
        let result = reg.on_freq_deviation(f64::INFINITY);
        assert_eq!(result.unwrap_err(), AncillaryError::InvalidFrequency);
    }
}
