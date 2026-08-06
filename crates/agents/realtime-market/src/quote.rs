//! 报价调度与验证

use crate::types::{QuoteSchedule, RealtimeQuote, SessionState};
use crate::RealtimeError;

/// 报价调度器
#[derive(Debug, Clone, Default)]
pub struct QuoteScheduler {
    /// 调度配置
    pub schedule: QuoteSchedule,
}

impl QuoteScheduler {
    /// 创建新的报价调度器
    pub fn new(interval_sec: u64) -> Self {
        Self {
            schedule: QuoteSchedule {
                interval_sec,
                last_quote_at: 0,
            },
        }
    }

    /// 判断当前时刻是否应该发送报价
    ///
    /// 当 `now - last_quote_at >= interval_sec` 时返回 true
    pub fn should_quote(&self, now: u64) -> bool {
        now.saturating_sub(self.schedule.last_quote_at) >= self.schedule.interval_sec
    }

    /// 计算下一个 15 分钟对齐的时段起始时间
    ///
    /// 将 now 向上对齐到 interval_sec 的整数倍
    pub fn next_period_start(&self, now: u64) -> u64 {
        let interval = self.schedule.interval_sec;
        if interval == 0 {
            return now;
        }
        let remainder = now % interval;
        if remainder == 0 {
            now
        } else {
            now + (interval - remainder)
        }
    }

    /// 记录报价时间
    pub fn record_quote(&mut self, now: u64) {
        self.schedule.last_quote_at = now;
    }
}

/// 报价验证器
#[derive(Debug, Clone, Default)]
pub struct QuoteValidator;

impl QuoteValidator {
    /// 创建新的报价验证器
    pub fn new() -> Self {
        Self
    }

    /// 验证报价合法性
    ///
    /// - volume_kwh 必须 > 0
    /// - price 必须 >= 0
    /// - 会话必须处于 Open 状态
    pub fn validate_quote(
        &self,
        q: &RealtimeQuote,
        session_state: SessionState,
    ) -> Result<(), RealtimeError> {
        if q.volume_kwh <= 0.0 {
            return Err(RealtimeError::InvalidQuote);
        }
        if q.price.0 < 0.0 {
            return Err(RealtimeError::InvalidQuote);
        }
        if session_state != SessionState::Open {
            return Err(RealtimeError::SessionNotOpen);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::types::YuanPerMwh;

    use super::*;
    use crate::types::TradeDir;

    #[test]
    fn test_should_quote_initial() {
        let scheduler = QuoteScheduler::default();
        // last_quote_at = 0, interval = 900, now = 900 => should quote
        assert!(scheduler.should_quote(900));
        // now = 899 => should not quote
        assert!(!scheduler.should_quote(899));
    }

    #[test]
    fn test_should_quote_after_record() {
        let mut scheduler = QuoteScheduler::default();
        scheduler.record_quote(1000);
        assert!(!scheduler.should_quote(1899));
        assert!(scheduler.should_quote(1900));
    }

    #[test]
    fn test_next_period_start_aligned() {
        let scheduler = QuoteScheduler::default();
        // 已经对齐
        assert_eq!(scheduler.next_period_start(900), 900);
        assert_eq!(scheduler.next_period_start(1800), 1800);
    }

    #[test]
    fn test_next_period_start_not_aligned() {
        let scheduler = QuoteScheduler::default();
        assert_eq!(scheduler.next_period_start(901), 1800);
        assert_eq!(scheduler.next_period_start(1), 900);
        assert_eq!(scheduler.next_period_start(1799), 1800);
    }

    #[test]
    fn test_next_period_start_zero() {
        let scheduler = QuoteScheduler::default();
        assert_eq!(scheduler.next_period_start(0), 0);
    }

    #[test]
    fn test_record_quote() {
        let mut scheduler = QuoteScheduler::default();
        scheduler.record_quote(5000);
        assert_eq!(scheduler.schedule.last_quote_at, 5000);
    }

    #[test]
    fn test_validate_quote_valid() {
        let validator = QuoteValidator::new();
        let q = RealtimeQuote {
            session_id: 1,
            period_start: 900,
            volume_kwh: 100.0,
            price: YuanPerMwh(350.0),
            direction: TradeDir::Buy,
            seq: 1,
        };
        assert!(validator.validate_quote(&q, SessionState::Open).is_ok());
    }

    #[test]
    fn test_validate_quote_zero_volume() {
        let validator = QuoteValidator::new();
        let q = RealtimeQuote {
            session_id: 1,
            period_start: 900,
            volume_kwh: 0.0,
            price: YuanPerMwh(350.0),
            direction: TradeDir::Buy,
            seq: 1,
        };
        assert_eq!(
            validator.validate_quote(&q, SessionState::Open),
            Err(RealtimeError::InvalidQuote)
        );
    }

    #[test]
    fn test_validate_quote_negative_price() {
        let validator = QuoteValidator::new();
        let q = RealtimeQuote {
            session_id: 1,
            period_start: 900,
            volume_kwh: 100.0,
            price: YuanPerMwh(-1.0),
            direction: TradeDir::Sell,
            seq: 1,
        };
        assert_eq!(
            validator.validate_quote(&q, SessionState::Open),
            Err(RealtimeError::InvalidQuote)
        );
    }

    #[test]
    fn test_validate_quote_session_not_open() {
        let validator = QuoteValidator::new();
        let q = RealtimeQuote {
            session_id: 1,
            period_start: 900,
            volume_kwh: 100.0,
            price: YuanPerMwh(350.0),
            direction: TradeDir::Buy,
            seq: 1,
        };
        assert_eq!(
            validator.validate_quote(&q, SessionState::Closed),
            Err(RealtimeError::SessionNotOpen)
        );
    }
}
