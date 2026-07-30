//! P2P 消息广播与接收 — MockTransport 实现

use alloc::vec::Vec;

use crate::types::P2pMsg;
use crate::{P2pError, P2pTransport};

/// 模拟传输层，用于测试
///
/// 内部维护发送队列 (sent) 和收件箱 (inbox)。
#[derive(Debug, Default)]
pub struct MockTransport {
    /// 已广播的消息记录
    sent: Vec<P2pMsg>,
    /// 模拟接收的消息队列
    inbox: Vec<P2pMsg>,
}

impl MockTransport {
    /// 创建空的 MockTransport
    pub fn new() -> Self {
        Self {
            sent: Vec::new(),
            inbox: Vec::new(),
        }
    }

    /// 注入一条消息到收件箱，模拟接收到消息
    pub fn inject_msg(&mut self, msg: P2pMsg) {
        self.inbox.push(msg);
    }

    /// 获取已发送消息数量
    pub fn sent_count(&self) -> usize {
        self.sent.len()
    }
}

impl P2pTransport for MockTransport {
    fn broadcast(&mut self, _topic: &str, msg: &P2pMsg) -> Result<(), P2pError> {
        self.sent.push(msg.clone());
        Ok(())
    }

    fn recv(&mut self) -> Result<P2pMsg, P2pError> {
        if self.inbox.is_empty() {
            return Err(P2pError::NoMessage);
        }
        Ok(self.inbox.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::P2pBid;

    fn sample_msg() -> P2pMsg {
        P2pMsg::Bid(P2pBid {
            agent: 1,
            volume_kwh: 100.0,
            price: 0.65,
            valid_until: 9999,
            seq: 1,
        })
    }

    #[test]
    fn test_broadcast_stores_msg() {
        let mut transport = MockTransport::new();
        let msg = sample_msg();
        assert!(transport.broadcast("p2p/bids", &msg).is_ok());
        assert_eq!(transport.sent_count(), 1);
    }

    #[test]
    fn test_recv_returns_injected_msg() {
        let mut transport = MockTransport::new();
        let msg = sample_msg();
        transport.inject_msg(msg.clone());
        let received = transport.recv().unwrap();
        assert_eq!(received, msg);
    }

    #[test]
    fn test_recv_empty_fails() {
        let mut transport = MockTransport::new();
        assert_eq!(transport.recv(), Err(P2pError::NoMessage));
    }

    #[test]
    fn test_recv_fifo_order() {
        let mut transport = MockTransport::new();
        let msg1 = P2pMsg::Bid(P2pBid {
            agent: 1,
            volume_kwh: 10.0,
            price: 0.5,
            valid_until: 9999,
            seq: 1,
        });
        let msg2 = P2pMsg::Bid(P2pBid {
            agent: 2,
            volume_kwh: 20.0,
            price: 0.6,
            valid_until: 9999,
            seq: 2,
        });
        transport.inject_msg(msg1.clone());
        transport.inject_msg(msg2.clone());
        assert_eq!(transport.recv().unwrap(), msg1);
        assert_eq!(transport.recv().unwrap(), msg2);
        assert_eq!(transport.recv(), Err(P2pError::NoMessage));
    }
}
