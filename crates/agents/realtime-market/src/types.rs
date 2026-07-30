//! 实时市场数据类型定义

/// 交易方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDir {
    /// 买入
    Buy,
    /// 卖出
    Sell,
}

/// 会话状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// 开放中
    Open,
    /// 正在关闭
    Closing,
    /// 已关闭
    Closed,
    /// 错误
    Error,
}

/// 实时报价
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeQuote {
    /// 会话 ID
    pub session_id: u64,
    /// 时段起始时间 (unix timestamp)
    pub period_start: u64,
    /// 报价电量 (kWh)
    pub volume_kwh: f64,
    /// 报价价格 (元/MWh)
    pub price: f64,
    /// 交易方向
    pub direction: TradeDir,
    /// 报价序号
    pub seq: u64,
}

/// 实时交易会话
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeSession {
    /// 会话 ID
    pub id: u64,
    /// 会话开始时间 (unix timestamp)
    pub started_at: u64,
    /// 已发送报价数
    pub quotes_sent: u64,
    /// 最后出清序号
    pub last_clearing_seq: u64,
    /// 会话状态
    pub state: SessionState,
}

/// 实时出清结果
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeClearing {
    /// 会话 ID
    pub session_id: u64,
    /// 出清序号
    pub seq: u64,
    /// 出清价格 (元/MWh)
    pub clearing_price: f64,
    /// 出清电量 (kWh)
    pub cleared_volume: f64,
    /// 出清时间 (unix timestamp)
    pub cleared_at: u64,
}

/// 报价调度配置
#[derive(Debug, Clone, PartialEq)]
pub struct QuoteSchedule {
    /// 报价间隔 (秒), 默认 900 (15min)
    pub interval_sec: u64,
    /// 上次报价时间 (unix timestamp)
    pub last_quote_at: u64,
}

impl Default for QuoteSchedule {
    fn default() -> Self {
        Self {
            interval_sec: 900,
            last_quote_at: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trade_dir_eq() {
        assert_eq!(TradeDir::Buy, TradeDir::Buy);
        assert_ne!(TradeDir::Buy, TradeDir::Sell);
    }

    #[test]
    fn test_session_state_eq() {
        assert_eq!(SessionState::Open, SessionState::Open);
        assert_ne!(SessionState::Open, SessionState::Closed);
        assert_ne!(SessionState::Closing, SessionState::Error);
    }

    #[test]
    fn test_realtime_quote_creation() {
        let q = RealtimeQuote {
            session_id: 1,
            period_start: 1000,
            volume_kwh: 500.0,
            price: 350.0,
            direction: TradeDir::Buy,
            seq: 1,
        };
        assert_eq!(q.session_id, 1);
        assert_eq!(q.period_start, 1000);
        assert!((q.volume_kwh - 500.0).abs() < f64::EPSILON);
        assert!((q.price - 350.0).abs() < f64::EPSILON);
        assert_eq!(q.direction, TradeDir::Buy);
        assert_eq!(q.seq, 1);
    }

    #[test]
    fn test_realtime_session_creation() {
        let s = RealtimeSession {
            id: 1,
            started_at: 1000,
            quotes_sent: 0,
            last_clearing_seq: 0,
            state: SessionState::Open,
        };
        assert_eq!(s.id, 1);
        assert_eq!(s.started_at, 1000);
        assert_eq!(s.quotes_sent, 0);
        assert_eq!(s.last_clearing_seq, 0);
        assert_eq!(s.state, SessionState::Open);
    }

    #[test]
    fn test_realtime_clearing_creation() {
        let c = RealtimeClearing {
            session_id: 1,
            seq: 1,
            clearing_price: 320.0,
            cleared_volume: 450.0,
            cleared_at: 1900,
        };
        assert_eq!(c.session_id, 1);
        assert_eq!(c.seq, 1);
        assert!((c.clearing_price - 320.0).abs() < f64::EPSILON);
        assert!((c.cleared_volume - 450.0).abs() < f64::EPSILON);
        assert_eq!(c.cleared_at, 1900);
    }

    #[test]
    fn test_quote_schedule_default() {
        let qs = QuoteSchedule::default();
        assert_eq!(qs.interval_sec, 900);
        assert_eq!(qs.last_quote_at, 0);
    }

    #[test]
    fn test_quote_schedule_custom() {
        let qs = QuoteSchedule {
            interval_sec: 600,
            last_quote_at: 500,
        };
        assert_eq!(qs.interval_sec, 600);
        assert_eq!(qs.last_quote_at, 500);
    }
}
