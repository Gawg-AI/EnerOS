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

use eneros_agent_sdk::telemetry_relay::{
    check_frame_header, encode_frame_header, FrameError, TEL_FRAME_MAX,
};
use eneros_durable_state::{
    Decode, DurableOutbox, Encode, IdempotencyKey, OutboxId, PeerReceipt, StoreError,
};

/// 批量帧解码错误（svc-mqtt 边界与 host 订阅侧共用；坏帧 → 拒绝解析）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchFrameError {
    /// 帧头非法（magic/schema 版本/截断/超限——委托 agent-sdk 线码）。
    BadHeader(FrameError),
    /// 条目计数与帧内实际条目不匹配（截断条目）。
    ItemTruncated,
    /// 条目编码损坏。
    CorruptItem,
}

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

/// 编码一批遥测条目为 MQTT 出口批量帧（固定二进制 + schema 版本；
/// 50 WP-50.1——禁自由 JSON 进 target）。
///
/// 帧布局：`OBSF(4) ‖ version(1) ‖ batch_seq(8 LE) ‖ count(2 LE)` 后接
/// `count` 条 `TelemetryItem::encode()`（每条自描述长度前缀）。
/// 超限（> TEL_FRAME_MAX）→ Err（fail-closed：不产出越界帧）。
pub fn encode_batch_frame(
    items: &[TelemetryItem],
    batch_seq: u64,
) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(
        eneros_agent_sdk::telemetry_relay::TEL_FRAME_HEADER_LEN + items.len() * 48,
    );
    out.extend_from_slice(&encode_frame_header(batch_seq, items.len() as u16));
    for item in items {
        let enc = item.encode().map_err(|_| "item-encode-failed")?;
        out.extend_from_slice(&enc);
    }
    if out.len() > TEL_FRAME_MAX {
        return Err("frame-oversize");
    }
    Ok(out)
}

/// 解码批量帧（host 订阅侧/负例 DoD 共用）。
///
/// 先经 agent-sdk 帧头校验（**schema 版本不匹配 → BadHeader(UnsupportedVersion)
/// 拒绝解析**），再按 count 逐条 `TelemetryItem::decode`。任何截断/损坏
/// → Err（不静默跳过）。
pub fn decode_batch_frame(bytes: &[u8]) -> Result<(u64, Vec<TelemetryItem>), BatchFrameError> {
    let (batch_seq, count) = check_frame_header(bytes).map_err(BatchFrameError::BadHeader)?;
    let mut pos = eneros_agent_sdk::telemetry_relay::TEL_FRAME_HEADER_LEN;
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if pos >= bytes.len() {
            return Err(BatchFrameError::ItemTruncated);
        }
        let (_, item) = TelemetryItem::decode(&bytes[pos..]).map_err(|e| match e {
            StoreError::Corrupt => BatchFrameError::CorruptItem,
            _ => BatchFrameError::ItemTruncated,
        })?;
        // TelemetryItem::decode 不暴露消耗长度——按确定性编码重新计算推进量。
        let enc = item.encode().map_err(|_| BatchFrameError::CorruptItem)?;
        pos += enc.len();
        items.push(item);
    }
    Ok((batch_seq, items))
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
                // 50（WP-50.1）：稳定幂等键（条目内容派生）——断网期失败
                // 重入队命中原键（不累积重复），成功时每项恰好一次。
                let key = item_idempotency_key(item);
                self.export_cursor = self.export_cursor.saturating_add(1);
                match durable.enqueue(item, key) {
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

/// 稳定幂等键（50 WP-50.1 补传不丢）：由条目内容派生——重入队命中原键，
/// durable outbox 不累积重复。旧实现用 export_cursor 派生键，断网期每次
/// 失败重入队产生新键 → 成功时重复导出（outbox 序号不连续）。
///
/// 键 = `created_ms(8 LE) ‖ FNV-1a(service+schema+payload)(8 LE)`。
pub fn item_idempotency_key(item: &TelemetryItem) -> IdempotencyKey {
    let mut key = [0u8; 16];
    key[..8].copy_from_slice(&item.created_ms.to_le_bytes());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in item
        .service
        .as_bytes()
        .iter()
        .chain(item.schema.as_bytes())
        .chain(item.payload.iter())
    {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    key[8..16].copy_from_slice(&hash.to_le_bytes());
    IdempotencyKey(key)
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

    // ================= 50（WP-50.1）批量帧编解码 =================

    /// 50（DoD 补传不丢）：连续失败 flush（断网期）后成功 flush——每项
    /// **恰好导出一次**，不重复；durable outbox 不累积重复条目（幂等键
    /// 稳定——重入队命中原 id）。旧实现以 export_cursor 派生键 → 每次
    /// 失败重入队产生新键 → 成功时重复导出（outbox 序号不连续）。
    #[test]
    fn 补传_exports_each_item_exactly_once_after_repeated_failures() {
        struct FlakyExporter {
            fails: u32,
            calls: u32,
        }
        impl TelemetryExporter for FlakyExporter {
            fn export_batch(
                &mut self,
                _items: &[TelemetryItem],
            ) -> Result<PeerReceipt, &'static str> {
                self.calls += 1;
                if self.calls <= self.fails {
                    return Err("export-link-down");
                }
                Ok(PeerReceipt {
                    peer: "mqtt-exporter".into(),
                    receipt_id: self.calls as u64,
                    at_ms: 1,
                })
            }
        }
        let mut svc = ObservabilityService::new(
            FlakyExporter { fails: 3, calls: 0 },
            OverflowPolicy::DropLowPriority,
            16,
        );
        svc.durable = Some(Box::new(eneros_durable_state::MemOutbox::default()));
        svc.enqueue(item(TelemetryClass::Telemetry, 1)).unwrap();
        svc.enqueue(item(TelemetryClass::Alarm, 2)).unwrap();
        // 断网期多次失败 flush（每次 recover → 重入队 → 重试）。
        for _ in 0..3 {
            let _ = svc.flush_batch(16);
        }
        // 网络恢复：成功 flush。
        let exported = svc.flush_batch(16).unwrap();
        // 每项恰好导出一次（不重复）。
        assert_eq!(exported, 2);
        assert_eq!(svc.stats.exported, 2);
        // durable pending 清空（无残留重复条目）。
        let pending = svc.durable.as_mut().unwrap().recover_pending().unwrap();
        assert!(pending
            .iter()
            .all(|p| p.state == eneros_durable_state::MessageState::PeerAcked));
        assert!(svc.queue.is_empty());
    }

    #[test]
    fn positive_batch_frame_roundtrip() {
        let items = vec![
            item(TelemetryClass::Telemetry, 100),
            item(TelemetryClass::SecurityAudit, 200),
            item(TelemetryClass::Log, 300),
        ];
        let frame = encode_batch_frame(&items, 42).expect("encode");
        assert!(frame.starts_with(b"OBSF"));
        assert_eq!(frame[4], 1, "schema version=1");
        let (batch_seq, decoded) = decode_batch_frame(&frame).expect("decode");
        assert_eq!(batch_seq, 42);
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].created_ms, 100);
        assert_eq!(decoded[1].class, TelemetryClass::SecurityAudit);
        assert_eq!(decoded[2].created_ms, 300);
    }

    /// 50（DoD 负例）：schema 版本不匹配 → 拒绝解析。
    #[test]
    fn negative_schema_version_mismatch_rejected() {
        let items = vec![item(TelemetryClass::Telemetry, 1)];
        let frame = encode_batch_frame(&items, 1).expect("encode");
        // 篡改 version 字节 → 解码必须拒绝（不能按不兼容 schema 解析）。
        let mut bad = frame.clone();
        bad[4] = 2;
        assert_eq!(
            decode_batch_frame(&bad),
            Err(BatchFrameError::BadHeader(
                eneros_agent_sdk::telemetry_relay::FrameError::UnsupportedVersion
            ))
        );
        let mut bad0 = frame.clone();
        bad0[4] = 0;
        assert_eq!(
            decode_batch_frame(&bad0),
            Err(BatchFrameError::BadHeader(
                eneros_agent_sdk::telemetry_relay::FrameError::UnsupportedVersion
            ))
        );
    }

    #[test]
    fn negative_batch_frame_corruption_rejected() {
        let items = vec![
            item(TelemetryClass::Telemetry, 1),
            item(TelemetryClass::Alarm, 2),
        ];
        let frame = encode_batch_frame(&items, 1).expect("encode");
        // count 虚报 → 截断条目拒绝。
        let mut bad = frame.clone();
        bad[13] = 99;
        assert!(decode_batch_frame(&bad).is_err());
        // 帧体截断 → 拒绝。
        assert!(decode_batch_frame(&frame[..frame.len() - 3]).is_err());
    }
}
