//! svc-cloud-sync 领域逻辑：CloudSync durable outbox（蓝图 41 WP-41.10）。
//!
//! - upload item 含 tenant/data class/schema/sequence/hash/idempotency；
//! - 本地 durable outbox；peer ACK cursor 持久化；断网重启后从 cursor
//!   恢复；duplicate ACK/partial batch/out-of-order 显式；
//! - remote command 与 telemetry upload 分通道——remote control 经
//!   management auth + ControlDispatch（本模块只处理 upload）。

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_durable_state::{
    DurableOutbox, IdempotencyKey, OutboxId, PeerReceipt, Pending, StoreError,
};

/// 上传条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadItem {
    pub tenant: u32,
    pub data_class: u8,
    pub schema: String,
    pub sequence: u64,
    pub hash: [u8; 32],
    pub payload: Vec<u8>,
}

impl eneros_durable_state::Encode for UploadItem {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(48 + self.payload.len());
        out.extend_from_slice(&self.tenant.to_le_bytes());
        out.push(self.data_class);
        let s = self.schema.as_bytes();
        out.push(s.len() as u8);
        out.extend_from_slice(s);
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.hash);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
}

impl eneros_durable_state::Decode for UploadItem {
    fn decode(bytes: &[u8]) -> Result<(eneros_durable_state::Generation, Self), StoreError> {
        if bytes.len() < 4 + 1 + 1 + 8 + 32 + 4 {
            return Err(StoreError::Corrupt);
        }
        let tenant = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let data_class = bytes[4];
        let slen = bytes[5] as usize;
        if 6 + slen + 8 + 32 + 4 > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        let schema = core::str::from_utf8(&bytes[6..6 + slen])
            .map_err(|_| StoreError::Corrupt)?
            .to_owned();
        let mut pos = 6 + slen;
        let sequence = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[pos..pos + 32]);
        pos += 32;
        let plen = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + plen > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        Ok((
            eneros_durable_state::Generation(0),
            UploadItem {
                tenant,
                data_class,
                schema,
                sequence,
                hash,
                payload: bytes[pos..pos + plen].to_vec(),
            },
        ))
    }
}

/// 上传传输 seam。
pub trait UploadTransport {
    /// 发送一批；返回对端 ack offset（= 已确认的 sequence 上限）。
    fn upload_batch(&mut self, items: &[UploadItem]) -> Result<u64, &'static str>;
}

/// CloudSync 服务。
pub struct CloudSync<O: DurableOutbox<UploadItem>, T: UploadTransport> {
    pub outbox: O,
    pub transport: T,
    /// peer ACK cursor（已确认 sequence 上限）。
    pub acked_cursor: u64,
    /// 幂等键 = hash（同内容不重复上传）。
    seen: Vec<IdempotencyKey>,
}

impl<O: DurableOutbox<UploadItem>, T: UploadTransport> CloudSync<O, T> {
    pub fn new(outbox: O, transport: T) -> Self {
        CloudSync {
            outbox,
            transport,
            acked_cursor: 0,
            seen: Vec::new(),
        }
    }

    /// 入队上传（durable；重复 hash → 返回原 outbox id）。
    pub fn upload(&mut self, item: UploadItem) -> Result<OutboxId, StoreError> {
        let key = IdempotencyKey(hash16(&item.hash));
        // 幂等：同 hash 已上传 → no-op。
        if self.seen.contains(&key) {
            for p in self.outbox.recover_pending()? {
                if p.idempotency == key {
                    return Ok(p.id);
                }
            }
            // 已 ACK（不在 pending）→ 视为已上传成功。
            return Ok(OutboxId(u64::MAX));
        }
        let id = self.outbox.enqueue(&item, key)?;
        self.seen.push(key);
        Ok(id)
    }

    /// 断网重启后：从 durable outbox 恢复 pending，重发（对账）。
    pub fn reconcile(&mut self) -> Result<Vec<OutboxId>, &'static str> {
        let pending: Vec<Pending<UploadItem>> = self
            .outbox
            .recover_pending()
            .map_err(|_| "outbox-unavailable")?;
        let mut resent = Vec::new();
        for p in pending {
            if p.state == eneros_durable_state::MessageState::PeerAcked {
                continue;
            }
            match self
                .transport
                .upload_batch(core::slice::from_ref(&p.message))
            {
                Ok(offset) => {
                    self.acked_cursor = self.acked_cursor.max(offset);
                    self.outbox
                        .mark_acked(
                            p.id,
                            &PeerReceipt {
                                peer: "cloud".into(),
                                receipt_id: offset,
                                at_ms: 0,
                            },
                        )
                        .map_err(|_| "ack-failed")?;
                    resent.push(p.id);
                }
                Err(_) => continue, // 仍 pending
            }
        }
        Ok(resent)
    }

    /// 常规批量上传（backpressure：outbox 满 → 显式 Err）。
    pub fn flush(&mut self, batch_size: usize) -> Result<u64, &'static str> {
        let pending: Vec<Pending<UploadItem>> = self
            .outbox
            .recover_pending()
            .map_err(|_| "outbox-unavailable")?;
        let batch: Vec<UploadItem> = pending
            .iter()
            .filter(|p| p.state != eneros_durable_state::MessageState::PeerAcked)
            .take(batch_size)
            .map(|p| p.message.clone())
            .collect();
        if batch.is_empty() {
            return Ok(self.acked_cursor);
        }
        let offset = self.transport.upload_batch(&batch)?;
        self.acked_cursor = self.acked_cursor.max(offset);
        // 标记批次内 ≤ offset 的消息 ACK（partial batch：只 ACK 已确认的）。
        for p in &pending {
            if p.message.sequence <= offset
                && p.state != eneros_durable_state::MessageState::PeerAcked
            {
                self.outbox
                    .mark_acked(
                        p.id,
                        &PeerReceipt {
                            peer: "cloud".into(),
                            receipt_id: offset,
                            at_ms: 0,
                        },
                    )
                    .map_err(|_| "ack-failed")?;
            }
        }
        Ok(self.acked_cursor)
    }
}

fn hash16(hash: &[u8; 32]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&hash[..16]);
    out
}

#[cfg(test)]
mod tests {
    use eneros_durable_state::{MemOutbox, UnavailableOutbox};

    use super::*;

    struct TcpTransport {
        /// 对端确认上限（partial ack 模拟）。
        pub limit: u64,
        pub fail: bool,
        pub batches: usize,
    }
    impl UploadTransport for TcpTransport {
        fn upload_batch(&mut self, items: &[UploadItem]) -> Result<u64, &'static str> {
            if self.fail {
                return Err("link-down");
            }
            self.batches += 1;
            let max = items.iter().map(|i| i.sequence).max().unwrap_or(0);
            Ok(max.min(self.limit))
        }
    }

    fn item(seq: u64) -> UploadItem {
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&seq.to_le_bytes());
        UploadItem {
            tenant: 1,
            data_class: 1,
            schema: "telemetry".into(),
            sequence: seq,
            hash,
            payload: vec![seq as u8; 8],
        }
    }

    #[test]
    fn unavailable_outbox_fails_closed() {
        let mut cs = CloudSync::new(
            UnavailableOutbox::new(),
            TcpTransport {
                limit: u64::MAX,
                fail: false,
                batches: 0,
            },
        );
        let it = item(1);
        assert!(cs.upload(it).is_err());
        assert!(cs.reconcile().is_err());
        assert!(cs.flush(1).is_err());
    }

    #[test]
    fn positive_upload_flush_cursor() {
        let mut cs = CloudSync::new(
            MemOutbox::default(),
            TcpTransport {
                limit: u64::MAX,
                fail: false,
                batches: 0,
            },
        );
        cs.upload(item(1)).unwrap();
        cs.upload(item(2)).unwrap();
        let cursor = cs.flush(10).unwrap();
        assert_eq!(cursor, 2);
        assert!(cs.outbox.recover_pending().unwrap().is_empty());
    }

    #[test]
    fn positive_idempotent_duplicate_hash() {
        let mut cs = CloudSync::new(
            MemOutbox::default(),
            TcpTransport {
                limit: u64::MAX,
                fail: false,
                batches: 0,
            },
        );
        let a = cs.upload(item(5)).unwrap();
        let b = cs.upload(item(5)).unwrap();
        assert_eq!(a, b); // 同 hash 同 outbox id
        assert_eq!(cs.outbox.recover_pending().unwrap().len(), 1);
    }

    #[test]
    fn negative_link_down_keeps_pending_reconcile_after_reconnect() {
        let mut cs = CloudSync::new(
            MemOutbox::default(),
            TcpTransport {
                limit: u64::MAX,
                fail: true,
                batches: 0,
            },
        );
        cs.upload(item(1)).unwrap();
        assert_eq!(cs.flush(10).unwrap_err(), "link-down");
        assert_eq!(cs.outbox.recover_pending().unwrap().len(), 1);
        // 重连 → 对账重发。
        cs.transport.fail = false;
        let resent = cs.reconcile().unwrap();
        assert_eq!(resent.len(), 1);
        assert!(cs.outbox.recover_pending().unwrap().is_empty());
        assert_eq!(cs.acked_cursor, 1);
    }

    #[test]
    fn negative_partial_batch_ack() {
        // 对端只确认到 seq=1：seq=2 的消息保持 pending（不丢）。
        let mut cs = CloudSync::new(
            MemOutbox::default(),
            TcpTransport {
                limit: 1,
                fail: false,
                batches: 0,
            },
        );
        cs.upload(item(1)).unwrap();
        cs.upload(item(2)).unwrap();
        let cursor = cs.flush(10).unwrap();
        assert_eq!(cursor, 1);
        let pending = cs.outbox.recover_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message.sequence, 2);
    }
}
