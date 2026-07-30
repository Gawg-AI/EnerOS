//! 实时交易会话管理

use alloc::vec::Vec;

use crate::types::{RealtimeSession, SessionState};
use crate::RealtimeError;

/// 会话管理器
#[derive(Debug, Clone)]
pub struct SessionManager {
    /// 所有会话
    pub sessions: Vec<RealtimeSession>,
    /// 下一个会话 ID
    pub next_id: u64,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
        }
    }
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new() -> Self {
        Self::default()
    }

    /// 开启新会话，返回会话 ID
    pub fn open_session(&mut self, now: u64) -> Result<u64, RealtimeError> {
        let id = self.next_id;
        self.next_id += 1;
        let session = RealtimeSession {
            id,
            started_at: now,
            quotes_sent: 0,
            last_clearing_seq: 0,
            state: SessionState::Open,
        };
        self.sessions.push(session);
        Ok(id)
    }

    /// 关闭指定会话
    pub fn close_session(&mut self, id: u64) -> Result<(), RealtimeError> {
        let session = self
            .sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(RealtimeError::SessionNotFound)?;

        if session.state == SessionState::Closed {
            return Err(RealtimeError::SessionAlreadyClosed);
        }

        session.state = SessionState::Closed;
        Ok(())
    }

    /// 获取指定会话的引用
    pub fn get_session(&self, id: u64) -> Option<&RealtimeSession> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// 获取所有开放状态的会话 ID 列表
    pub fn active_sessions(&self) -> Vec<u64> {
        self.sessions
            .iter()
            .filter(|s| s.state == SessionState::Open)
            .map(|s| s.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_session() {
        let mut mgr = SessionManager::new();
        let id = mgr.open_session(1000).unwrap();
        assert_eq!(id, 1);
        let session = mgr.get_session(id).unwrap();
        assert_eq!(session.started_at, 1000);
        assert_eq!(session.state, SessionState::Open);
        assert_eq!(session.quotes_sent, 0);
    }

    #[test]
    fn test_open_multiple_sessions() {
        let mut mgr = SessionManager::new();
        let id1 = mgr.open_session(1000).unwrap();
        let id2 = mgr.open_session(2000).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(mgr.active_sessions().len(), 2);
    }

    #[test]
    fn test_close_session() {
        let mut mgr = SessionManager::new();
        let id = mgr.open_session(1000).unwrap();
        mgr.close_session(id).unwrap();
        let session = mgr.get_session(id).unwrap();
        assert_eq!(session.state, SessionState::Closed);
    }

    #[test]
    fn test_double_close_error() {
        let mut mgr = SessionManager::new();
        let id = mgr.open_session(1000).unwrap();
        mgr.close_session(id).unwrap();
        let err = mgr.close_session(id).unwrap_err();
        assert_eq!(err, RealtimeError::SessionAlreadyClosed);
    }

    #[test]
    fn test_close_not_found() {
        let mut mgr = SessionManager::new();
        let err = mgr.close_session(999).unwrap_err();
        assert_eq!(err, RealtimeError::SessionNotFound);
    }

    #[test]
    fn test_get_session_not_found() {
        let mgr = SessionManager::new();
        assert!(mgr.get_session(999).is_none());
    }

    #[test]
    fn test_active_sessions() {
        let mut mgr = SessionManager::new();
        let id1 = mgr.open_session(1000).unwrap();
        let id2 = mgr.open_session(2000).unwrap();
        mgr.close_session(id1).unwrap();
        let active = mgr.active_sessions();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0], id2);
    }
}
