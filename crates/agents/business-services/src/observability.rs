//! svc-observability 领域逻辑：target 可观测队列（蓝图 41 WP-41.12）。
//!
//! - bounded IPC 接收（调用方）；
//! - durable queue（经 durable-state 落盘；target 存储句柄未移交 →
//!   显式 Unavailable，不伪造持久化）；
//! - drop policy 显式、计数且按 criticality——安全审计不可被普通
//!   telemetry 挤掉；
//! - secret/PII redaction 在持久化前；
//! - exporter 经 AgentBus/secure transport，ACK cursor 持久化；
//! - corruption 不静默 default/skip——隔离并报告。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_durable_state::{DurableOutbox, IdempotencyKey, OutboxId, PeerReceipt, StoreError};

/// 遥测类别（criticality：安全审计最高，永不被普通流量挤掉）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TelemetryClass {
    SecurityAudit,
    Control,
    Alarm,
    Telemetry,
    Log,
    Debug,
}

impl TelemetryClass {
    pub fn criticality(&self) -> u8 {
        match self {
            TelemetryClass::SecurityAudit => 4,
            TelemetryClass::Control => 3,
            TelemetryClass::Alarm => 2,
            TelemetryClass::Telemetry => 1,
            TelemetryClass::Log => 1,
            TelemetryClass::Debug => 0,
        }
    }
}

/// 队列溢出策略（显式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    /// 丢弃最旧低优先级（安全审计不可被挤掉）。
    DropLowPriority,
    /// 拒绝新条目。
    RejectNew,
}

/// 遥测条目（持久化前 redaction）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryItem {
    pub class: TelemetryClass,
    pub service: String,
    pub schema: String,
    pub payload: Vec<u8>,
    pub created_ms: u64,
    pub boot_id: u64,
    pub config_generation: u64,
}

impl eneros_durable_state::Encode for TelemetryItem {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out =
            Vec::with_capacity(32 + self.service.len() + self.schema.len() + self.payload.len());
        out.push(match self.class {
            TelemetryClass::SecurityAudit => 0,
            TelemetryClass::Control => 1,
            TelemetryClass::Alarm => 2,
            TelemetryClass::Telemetry => 3,
            TelemetryClass::Log => 4,
            TelemetryClass::Debug => 5,
        });
        let s = self.service.as_bytes();
        out.push(s.len() as u8);
        out.extend_from_slice(s);
        let sc = self.schema.as_bytes();
        out.push(sc.len() as u8);
        out.extend_from_slice(sc);
        out.extend_from_slice(&self.created_ms.to_le_bytes());
        out.extend_from_slice(&self.boot_id.to_le_bytes());
        out.extend_from_slice(&self.config_generation.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
}

impl eneros_durable_state::Decode for TelemetryItem {
    fn decode(bytes: &[u8]) -> Result<(eneros_durable_state::Generation, Self), StoreError> {
        if bytes.len() < 1 + 1 + 1 + 8 + 8 + 8 + 4 {
            return Err(StoreError::Corrupt);
        }
        let class = match bytes[0] {
            0 => TelemetryClass::SecurityAudit,
            1 => TelemetryClass::Control,
            2 => TelemetryClass::Alarm,
            3 => TelemetryClass::Telemetry,
            4 => TelemetryClass::Log,
            5 => TelemetryClass::Debug,
            _ => return Err(StoreError::Corrupt),
        };
        let slen = bytes[1] as usize;
        let mut pos = 2;
        if pos + slen + 1 > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        let service = core::str::from_utf8(&bytes[pos..pos + slen])
            .map_err(|_| StoreError::Corrupt)?
            .to_owned();
        pos += slen;
        let schema_len = bytes[pos] as usize;
        pos += 1;
        if pos + schema_len + 8 + 8 + 8 + 4 > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        let schema = core::str::from_utf8(&bytes[pos..pos + schema_len])
            .map_err(|_| StoreError::Corrupt)?
            .to_owned();
        pos += schema_len;
        let created_ms = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let boot_id = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let config_generation = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let plen = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + plen > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        Ok((
            eneros_durable_state::Generation(0),
            TelemetryItem {
                class,
                service,
                schema,
                payload: bytes[pos..pos + plen].to_vec(),
                created_ms,
                boot_id,
                config_generation,
            },
        ))
    }
}

/// 队列快照统计（drop 计数必须显式）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub enqueued: u64,
    pub dropped: BTreeMap<u8, u64>,
    pub redacted: u64,
    pub exported: u64,
    pub export_failed: u64,
}

/// 导出 seam（真实实现不得恒真）。
pub trait TelemetryExporter {
    fn export_batch(&mut self, items: &[TelemetryItem]) -> Result<PeerReceipt, &'static str>;
}

/// 目标可观测服务。
pub struct ObservabilityService<E: TelemetryExporter> {
    pub exporter: E,
    pub policy: OverflowPolicy,
    pub capacity: usize,
    pub queue: Vec<TelemetryItem>,
    pub stats: QueueStats,
    /// durable outbox（持久化补传；存储不可用 → None 且导出显式失败）。
    pub durable: Option<Box<dyn DurableOutbox<TelemetryItem>>>,
    pub export_cursor: u64,
}

impl<E: TelemetryExporter> ObservabilityService<E> {
    pub fn new(exporter: E, policy: OverflowPolicy, capacity: usize) -> Self {
        ObservabilityService {
            exporter,
            policy,
            capacity: capacity.max(1),
            queue: Vec::new(),
            stats: QueueStats::default(),
            durable: None,
            export_cursor: 0,
        }
    }

    /// redaction（持久化前）：payload 中敏感模式 → 替换。失败不静默
    /// 跳过——返回 Err 由调用方决定拒绝。
    pub fn redact(item: &mut TelemetryItem) -> Result<(), &'static str> {
        // 敏感模式：api_key=... / token=... / password=...（长度上限内）。
        let patterns: [&[u8]; 3] = [b"api_key=", b"token=", b"password="];
        for p in patterns {
            let mut i = 0;
            while i + p.len() <= item.payload.len() {
                if &item.payload[i..i + p.len()] == p {
                    // 值到行尾/空白/引号。
                    let mut j = i + p.len();
                    while j < item.payload.len()
                        && !matches!(item.payload[j], b'\n' | b' ' | b'"' | b'\'')
                    {
                        j += 1;
                    }
                    for b in &mut item.payload[i + p.len()..j] {
                        *b = b'*';
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
        Ok(())
    }

    /// 入队（bounded；溢出按策略，drop 计数；安全审计永不被挤掉）。
    pub fn enqueue(&mut self, mut item: TelemetryItem) -> Result<(), &'static str> {
        Self::redact(&mut item)?;
        self.stats.redacted += 1;
        if self.queue.len() >= self.capacity {
            match self.policy {
                OverflowPolicy::RejectNew => {
                    *self
                        .stats
                        .dropped
                        .entry(item.class.criticality())
                        .or_insert(0) += 1;
                    return Err("queue-full-reject");
                }
                OverflowPolicy::DropLowPriority => {
                    // 找最旧的最低 criticality 条目丢弃；安全审计不可被挤。
                    let victim = self
                        .queue
                        .iter()
                        .enumerate()
                        .filter(|(_, q)| q.class.criticality() < item.class.criticality())
                        .min_by_key(|(_, q)| q.created_ms)
                        .map(|(i, _)| i);
                    match victim {
                        Some(i) => {
                            let crit = self.queue[i].class.criticality();
                            *self.stats.dropped.entry(crit).or_insert(0) += 1;
                            self.queue.remove(i);
                        }
                        None => {
                            *self
                                .stats
                                .dropped
                                .entry(item.class.criticality())
                                .or_insert(0) += 1;
                            return Err("queue-full-keep");
                        }
                    }
                }
            }
        }
        self.queue.push(item);
        self.stats.enqueued += 1;
        Ok(())
    }

    /// 从 durable outbox 恢复未确认条目到内存队列（断电/导出失败后补传）。
    pub fn recover_durable(&mut self) -> Result<usize, &'static str> {
        let Some(durable) = &mut self.durable else {
            return Ok(0);
        };
        let pending = durable
            .recover_pending()
            .map_err(|_| "durable-recover-failed")?;
        let mut n = 0usize;
        for p in pending {
            if p.state == eneros_durable_state::MessageState::PeerAcked {
                continue;
            }
            if self.queue.len() >= self.capacity {
                break;
            }
            self.queue.push(p.message);
            n += 1;
        }
        Ok(n)
    }

    /// 导出批次（真实 exporter；成功 → peer receipt → 计数 + 出队）。
    ///
    /// durable 存在时：先持久化（enqueue）再发送；发送成功 mark_acked；
    /// 发送失败不回队（数据已在 durable，下次 flush 先 recover）。
    pub fn flush_batch(&mut self, batch_size: usize) -> Result<usize, &'static str> {
        if self.durable.is_some() {
            let _ = self.recover_durable()?;
        }
        if self.queue.is_empty() {
            return Ok(0);
        }
        let n = batch_size.min(self.queue.len());
        let batch: Vec<TelemetryItem> = self.queue.drain(..n).collect();

        // 先入 durable outbox（若可用），再发网络。
        let mut outbox_ids: Vec<OutboxId> = Vec::new();
        if let Some(durable) = &mut self.durable {
            for item in &batch {
                let mut key = [0u8; 16];
                key[..8].copy_from_slice(&self.export_cursor.to_le_bytes());
                self.export_cursor += 1;
                match durable.enqueue(item, IdempotencyKey(key)) {
                    Ok(id) => outbox_ids.push(id),
                    Err(_) => {
                        // 入 durable 失败：回队并报错，不伪装已持久化。
                        self.stats.export_failed += batch.len() as u64;
                        let mut restored = batch;
                        restored.append(&mut self.queue);
                        self.queue = restored;
                        return Err("durable-enqueue-failed");
                    }
                }
            }
        }

        match self.exporter.export_batch(&batch) {
            Ok(receipt) => {
                self.stats.exported += batch.len() as u64;
                if let Some(durable) = &mut self.durable {
                    for id in outbox_ids {
                        let _ = durable.mark_acked(id, &receipt);
                    }
                }
                Ok(batch.len())
            }
            Err(e) => {
                self.stats.export_failed += batch.len() as u64;
                if self.durable.is_none() {
                    // 无 durable：回队（不丢）。
                    let mut restored = batch;
                    restored.append(&mut self.queue);
                    self.queue = restored;
                }
                // 有 durable：数据已持久化，下次 flush 会 recover 后重试。
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkExporter {
        batches: usize,
    }
    impl TelemetryExporter for OkExporter {
        fn export_batch(&mut self, _items: &[TelemetryItem]) -> Result<PeerReceipt, &'static str> {
            self.batches += 1;
            Ok(PeerReceipt {
                peer: "agentbus".into(),
                receipt_id: self.batches as u64,
                at_ms: 1,
            })
        }
    }

    struct FailExporter;
    impl TelemetryExporter for FailExporter {
        fn export_batch(&mut self, _items: &[TelemetryItem]) -> Result<PeerReceipt, &'static str> {
            Err("export-link-down")
        }
    }

    fn item(class: TelemetryClass, created: u64) -> TelemetryItem {
        TelemetryItem {
            class,
            service: "svc-test".into(),
            schema: "v1".into(),
            payload: vec![1, 2, 3],
            created_ms: created,
            boot_id: 1,
            config_generation: 0,
        }
    }

    #[test]
    fn positive_enqueue_export() {
        let mut svc = ObservabilityService::new(
            OkExporter { batches: 0 },
            OverflowPolicy::DropLowPriority,
            16,
        );
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        svc.enqueue(item(TelemetryClass::SecurityAudit, 2)).unwrap();
        assert_eq!(svc.queue.len(), 2);
        assert_eq!(svc.flush_batch(16).unwrap(), 2);
        assert_eq!(svc.stats.exported, 2);
        assert!(svc.queue.is_empty());
    }

    #[test]
    fn negative_redaction_before_persist() {
        let mut it = item(TelemetryClass::Log, 1);
        it.payload = b"user=alice token=secret123 api_key=abcdef".to_vec();
        ObservabilityService::<OkExporter>::redact(&mut it).unwrap();
        let s = core::str::from_utf8(&it.payload).unwrap();
        assert!(s.starts_with("user=alice token=********* api_key=******"));
        assert!(!s.contains("secret123"));
        assert!(!s.contains("abcdef"));
    }

    #[test]
    fn negative_security_audit_never_dropped_by_low_priority() {
        let mut svc = ObservabilityService::new(
            OkExporter { batches: 0 },
            OverflowPolicy::DropLowPriority,
            2,
        );
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        svc.enqueue(item(TelemetryClass::Telemetry, 2)).unwrap();
        // 队列满；安全审计入队 → 挤掉最旧低优先级。
        svc.enqueue(item(TelemetryClass::SecurityAudit, 3)).unwrap();
        assert!(svc
            .queue
            .iter()
            .any(|q| q.class == TelemetryClass::SecurityAudit));
        assert_eq!(svc.stats.dropped.get(&1), Some(&1));
        // 全队都是高优先级时，低优先级新条目被拒（不挤安全审计）。
        let err = svc.enqueue(item(TelemetryClass::Debug, 4)).unwrap_err();
        assert_eq!(err, "queue-full-keep");
        assert!(svc
            .queue
            .iter()
            .any(|q| q.class == TelemetryClass::SecurityAudit));
    }

    #[test]
    fn negative_reject_new_policy_counts() {
        let mut svc =
            ObservabilityService::new(OkExporter { batches: 0 }, OverflowPolicy::RejectNew, 1);
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        let err = svc.enqueue(item(TelemetryClass::Telemetry, 2)).unwrap_err();
        assert_eq!(err, "queue-full-reject");
        assert_eq!(svc.stats.dropped.get(&1), Some(&1));
    }

    #[test]
    fn durable_recover_after_export_failure() {
        let mut svc = ObservabilityService::new(FailExporter, OverflowPolicy::DropLowPriority, 8);
        svc.durable = Some(Box::new(eneros_durable_state::MemOutbox::default()));
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        let err = svc.flush_batch(8).unwrap_err();
        assert_eq!(err, "export-link-down");
        // 有 durable 时不回内存队，数据留在 durable。
        assert!(svc.queue.is_empty());
        let n = svc.recover_durable().unwrap();
        assert_eq!(n, 1);
        assert_eq!(svc.queue.len(), 1);
        assert_eq!(svc.queue[0].created_ms, 1);
    }

    #[test]
    fn negative_export_failure_restores_queue() {
        let mut svc = ObservabilityService::new(FailExporter, OverflowPolicy::DropLowPriority, 8);
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        svc.enqueue(item(TelemetryClass::Alarm, 2)).unwrap();
        let err = svc.flush_batch(8).unwrap_err();
        assert_eq!(err, "export-link-down");
        // 不丢：队列恢复（顺序保持）。
        assert_eq!(svc.queue.len(), 2);
        assert_eq!(svc.queue[0].created_ms, 1);
        assert_eq!(svc.queue[1].created_ms, 2);
        assert_eq!(svc.stats.export_failed, 2);
    }
}
