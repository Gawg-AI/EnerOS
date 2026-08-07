//! 实时市场报价闭环

use alloc::vec::Vec;

use crate::quote::{QuoteScheduler, QuoteValidator};
use crate::session::SessionManager;
#[cfg(test)]
use crate::types::YuanPerMwh;
use crate::types::{RealtimeClearing, RealtimeQuote};
use crate::RealtimeError;

/// 实时市场操作 trait
pub trait RealtimeMarket {
    /// 开启新会话
    fn open_session(&mut self, now: u64) -> Result<u64, RealtimeError>;
    /// 提交报价
    fn submit_quote(&mut self, quote: &RealtimeQuote, now: u64) -> Result<(), RealtimeError>;
    /// 关闭会话
    fn close_session(&mut self, id: u64) -> Result<(), RealtimeError>;
}

/// 实时市场报价闭环引擎
#[derive(Debug, Clone)]
pub struct RealtimeLoop {
    /// 会话管理器
    pub session_mgr: SessionManager,
    /// 报价调度器
    pub scheduler: QuoteScheduler,
    /// 报价验证器
    pub validator: QuoteValidator,
    /// 出清结果记录
    pub clearings: Vec<RealtimeClearing>,
}

impl Default for RealtimeLoop {
    fn default() -> Self {
        Self {
            session_mgr: SessionManager::new(),
            scheduler: QuoteScheduler::default(),
            validator: QuoteValidator::new(),
            clearings: Vec::new(),
        }
    }
}

impl RealtimeLoop {
    /// 创建新的实时市场闭环引擎
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理出清结果
    ///
    /// 记录出清结果并更新对应会话的 last_clearing_seq
    pub fn process_clearing(&mut self, clearing: RealtimeClearing) -> Result<(), RealtimeError> {
        // 验证会话存在
        let session = self
            .session_mgr
            .sessions
            .iter_mut()
            .find(|s| s.id == clearing.session_id)
            .ok_or(RealtimeError::SessionNotFound)?;

        session.last_clearing_seq = clearing.seq;
        self.clearings.push(clearing);
        Ok(())
    }

    /// 生成模拟出清结果（v0.229.0 起仅测试使用——生产出清经 gateway）。
    #[cfg(test)]
    fn mock_clearing(quote: &RealtimeQuote, cleared_at: u64) -> RealtimeClearing {
        RealtimeClearing {
            session_id: quote.session_id,
            seq: quote.seq,
            clearing_price: YuanPerMwh(quote.price.0 * 0.95),
            cleared_volume: quote.volume_kwh * 0.90,
            cleared_at,
        }
    }
}

impl RealtimeMarket for RealtimeLoop {
    fn open_session(&mut self, now: u64) -> Result<u64, RealtimeError> {
        self.session_mgr.open_session(now)
    }

    fn submit_quote(&mut self, quote: &RealtimeQuote, now: u64) -> Result<(), RealtimeError> {
        // 检查报价频率
        if !self.scheduler.should_quote(now) {
            return Err(RealtimeError::QuoteTooFrequent);
        }

        // 验证会话状态
        let session = self
            .session_mgr
            .get_session(quote.session_id)
            .ok_or(RealtimeError::SessionNotFound)?;

        let state = session.state;

        // 验证报价
        self.validator.validate_quote(quote, state)?;

        // 更新会话报价计数
        if let Some(s) = self
            .session_mgr
            .sessions
            .iter_mut()
            .find(|s| s.id == quote.session_id)
        {
            s.quotes_sent += 1;
        }

        // 记录报价时间
        self.scheduler.record_quote(now);

        Ok(())
    }

    fn close_session(&mut self, id: u64) -> Result<(), RealtimeError> {
        self.session_mgr.close_session(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SessionState, TradeDir};

    fn make_quote(session_id: u64, seq: u64) -> RealtimeQuote {
        RealtimeQuote {
            session_id,
            period_start: 900,
            volume_kwh: 200.0,
            price: YuanPerMwh(400.0),
            direction: TradeDir::Buy,
            seq,
        }
    }

    #[test]
    fn test_realtime_loop_open_and_submit() {
        let mut loop_engine = RealtimeLoop::new();
        let id = loop_engine.open_session(1000).unwrap();
        assert_eq!(id, 1);

        let quote = make_quote(id, 1);
        // now=1000, last_quote_at=0, interval=900 => 1000 >= 900, should pass
        loop_engine.submit_quote(&quote, 1000).unwrap();

        let session = loop_engine.session_mgr.get_session(id).unwrap();
        assert_eq!(session.quotes_sent, 1);
    }

    #[test]
    fn test_submit_quote_too_frequent() {
        let mut loop_engine = RealtimeLoop::new();
        let id = loop_engine.open_session(1000).unwrap();

        let quote = make_quote(id, 1);
        loop_engine.submit_quote(&quote, 1000).unwrap();

        // 第二次报价间隔不足
        let quote2 = make_quote(id, 2);
        let err = loop_engine.submit_quote(&quote2, 1500).unwrap_err();
        assert_eq!(err, RealtimeError::QuoteTooFrequent);
    }

    #[test]
    fn test_submit_quote_after_interval() {
        let mut loop_engine = RealtimeLoop::new();
        let id = loop_engine.open_session(1000).unwrap();

        let quote = make_quote(id, 1);
        loop_engine.submit_quote(&quote, 1000).unwrap();

        // 间隔 900 秒后可以再次报价
        let quote2 = make_quote(id, 2);
        loop_engine.submit_quote(&quote2, 1900).unwrap();

        let session = loop_engine.session_mgr.get_session(id).unwrap();
        assert_eq!(session.quotes_sent, 2);
    }

    #[test]
    fn test_submit_quote_session_not_found() {
        let mut loop_engine = RealtimeLoop::new();
        let quote = make_quote(999, 1);
        let err = loop_engine.submit_quote(&quote, 1000).unwrap_err();
        assert_eq!(err, RealtimeError::SessionNotFound);
    }

    #[test]
    fn test_submit_quote_session_closed() {
        let mut loop_engine = RealtimeLoop::new();
        let id = loop_engine.open_session(1000).unwrap();
        loop_engine.close_session(id).unwrap();

        let quote = make_quote(id, 1);
        let err = loop_engine.submit_quote(&quote, 1000).unwrap_err();
        assert_eq!(err, RealtimeError::SessionNotOpen);
    }

    #[test]
    fn test_process_clearing() {
        let mut loop_engine = RealtimeLoop::new();
        let id = loop_engine.open_session(1000).unwrap();

        let clearing = RealtimeClearing {
            session_id: id,
            seq: 1,
            clearing_price: YuanPerMwh(380.0),
            cleared_volume: 180.0,
            cleared_at: 1900,
        };
        loop_engine.process_clearing(clearing).unwrap();

        assert_eq!(loop_engine.clearings.len(), 1);
        let session = loop_engine.session_mgr.get_session(id).unwrap();
        assert_eq!(session.last_clearing_seq, 1);
    }

    #[test]
    fn test_process_clearing_session_not_found() {
        let mut loop_engine = RealtimeLoop::new();
        let clearing = RealtimeClearing {
            session_id: 999,
            seq: 1,
            clearing_price: YuanPerMwh(380.0),
            cleared_volume: 180.0,
            cleared_at: 1900,
        };
        let err = loop_engine.process_clearing(clearing).unwrap_err();
        assert_eq!(err, RealtimeError::SessionNotFound);
    }

    #[test]
    fn test_mock_clearing() {
        let quote = make_quote(1, 5);
        let clearing = RealtimeLoop::mock_clearing(&quote, 2000);
        assert_eq!(clearing.session_id, 1);
        assert_eq!(clearing.seq, 5);
        assert_eq!(clearing.clearing_price, YuanPerMwh(380.0));
        assert!((clearing.cleared_volume - 180.0).abs() < f64::EPSILON);
        assert_eq!(clearing.cleared_at, 2000);
    }

    #[test]
    fn test_full_lifecycle() {
        let mut loop_engine = RealtimeLoop::new();

        // 开启会话
        let id = loop_engine.open_session(0).unwrap();

        // 提交报价
        let quote = make_quote(id, 1);
        loop_engine.submit_quote(&quote, 900).unwrap();

        // 模拟出清
        let clearing = RealtimeLoop::mock_clearing(&quote, 1800);
        loop_engine.process_clearing(clearing).unwrap();

        // 验证状态
        let session = loop_engine.session_mgr.get_session(id).unwrap();
        assert_eq!(session.quotes_sent, 1);
        assert_eq!(session.last_clearing_seq, 1);
        assert_eq!(session.state, SessionState::Open);

        // 关闭会话
        loop_engine.close_session(id).unwrap();
        let session = loop_engine.session_mgr.get_session(id).unwrap();
        assert_eq!(session.state, SessionState::Closed);
    }
}
