//! cloud-sync 真实传输后端（v0.233.0，B3-10）。
//!
//! [`SyncTransport`] 此前仅 `MockSyncTransport`。本模块提供
//! [`HttpSyncTransport`]：经注入 HTTP 闭包发送批次（真实云端上报）。

use alloc::string::String;

use crate::delta_sync::SyncBatch;
use crate::{SyncError, SyncTransport};

/// 基于注入 HTTP 闭包的云边同步传输（真实后端）。
pub struct HttpSyncTransport<F> {
    /// 发送闭包（序列化批次 → 云端；返回 ack_offset）。
    send_fn: F,
    /// 云端端点（诊断）。
    #[allow(dead_code)]
    endpoint: String,
}

impl<F> HttpSyncTransport<F>
where
    F: FnMut(&SyncBatch, &[u8]) -> Result<u64, SyncError>,
{
    /// 创建真实同步传输。
    pub fn new(endpoint: &str, send_fn: F) -> Self {
        Self {
            send_fn,
            endpoint: String::from(endpoint),
        }
    }
}

impl<F> SyncTransport for HttpSyncTransport<F>
where
    F: FnMut(&SyncBatch, &[u8]) -> Result<u64, SyncError>,
{
    fn send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError> {
        (self.send_fn)(batch, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> SyncBatch {
        SyncBatch {
            batch_id: 7,
            events: Vec::new(),
            from_offset: 0,
            to_offset: 100,
            created_at: 0,
            retry_count: 0,
        }
    }

    #[test]
    fn test_send_batch_end_to_end() {
        let mut t =
            HttpSyncTransport::new("https://cloud.example.com", |b: &SyncBatch, p: &[u8]| {
                assert_eq!(b.to_offset, 100);
                assert_eq!(p, b"data");
                Ok(42)
            });
        let ack = t.send_batch(&batch(), b"data").expect("send");
        assert_eq!(ack, 42);
    }

    #[test]
    fn test_send_error_propagates() {
        let mut t =
            HttpSyncTransport::new("https://cloud.example.com", |_b: &SyncBatch, _p: &[u8]| {
                Err(SyncError::TransportError)
            });
        assert!(t.send_batch(&batch(), b"x").is_err());
    }
}
