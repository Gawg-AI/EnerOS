//! 遥测队列持久化与导出（v0.238.0，B5-01）。
//!
//! - `fs_persist` — 队列经 eneros-fs 落盘（行式，重启补传）；
//! - [`persist_snapshot`] / [`load_snapshot`] — 快照读写（幂等断点：
//!   记录已确认发送的 `last_acked_id`，重启后从断点继续）；
//! - [`OtlpExporter`] — OTLP 兼容 exporter（注入 HTTP 闭包，host 可
//!   采集真实指标流）。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::queue::TelemetryEvent;

/// 持久化快照：队列事件 + 幂等断点。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueSnapshot {
    /// 已确认发送的最大事件 ID（断点，重启后跳过 ≤ 此 ID 的事件）。
    pub last_acked_id: u64,
    /// 待补传事件。
    pub events: Vec<TelemetryEvent>,
    /// 33 OB-03：原队列容量（缺省 = 按事件数推导，兼容旧快照）。
    pub capacity: usize,
    /// 33 OB-03：溢出策略（缺省 DropOldest，兼容旧快照）。
    pub policy: crate::queue::OverflowPolicy,
}

/// 序列化快照为文本行。
///
/// 首行：`#last_acked=<id>`；其后每行一个事件：
/// `<id>,<category>,<timestamp>,<payload_hex>`。
pub fn persist_snapshot(snapshot: &QueueSnapshot) -> Vec<u8> {
    let mut out = Vec::new();
    let header = alloc::format!("#last_acked={}", snapshot.last_acked_id);
    out.extend_from_slice(header.as_bytes());
    out.push(b'\n');
    for e in &snapshot.events {
        let mut line = String::new();
        line.push_str(&e.id.to_string());
        line.push(',');
        line.push_str(&(e.category as u8).to_string());
        line.push(',');
        line.push_str(&e.timestamp.to_string());
        line.push(',');
        for b in e.payload.as_bytes() {
            line.push(HEX[(b >> 4) as usize] as char);
            line.push(HEX[(b & 0xF) as usize] as char);
        }
        line.push('\n');
        out.extend_from_slice(line.as_bytes());
    }
    out
}

const HEX: [u8; 16] = *b"0123456789abcdef";

/// 解析快照文本；格式非法行跳过。
pub fn load_snapshot(data: &[u8]) -> QueueSnapshot {
    let mut snap = QueueSnapshot::default();
    let text = match core::str::from_utf8(data) {
        Ok(t) => t,
        Err(_) => return snap,
    };
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#last_acked=") {
            if let Ok(id) = rest.parse::<u64>() {
                snap.last_acked_id = id;
            }
            continue;
        }
        let mut parts = line.splitn(4, ',');
        let (Some(id), Some(cat), Some(ts), Some(payload_hex)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(id), Ok(cat), Ok(ts)) = (id.parse::<u64>(), cat.parse::<u8>(), ts.parse::<u64>())
        else {
            continue;
        };
        let Some(payload) = hex_decode(payload_hex) else {
            continue;
        };
        let payload = String::from_utf8(payload).unwrap_or_default();
        snap.events.push(TelemetryEvent {
            id,
            category: match cat {
                0 => crate::queue::TelemetryCategory::SystemMetrics,
                1 => crate::queue::TelemetryCategory::PerformanceMetrics,
                2 => crate::queue::TelemetryCategory::AlertEvents,
                3 => crate::queue::TelemetryCategory::SecurityAudit,
                _ => crate::queue::TelemetryCategory::AgentBehavior,
            },
            timestamp: ts,
            payload,
        });
    }
    snap
}

/// hex 解码。
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

// ============================================================================
// OTLP exporter
// ============================================================================

/// OTLP 兼容导出器（v0.238.0，B5-01）。
///
/// 指标/日志事件编码为 OTLP 风格 protobuf-json 行，经注入发送闭包
/// 输出（真实 HTTP/OTLP 端点由集成层提供；串口/TSDB 后端可注入
/// 同签名闭包）。
pub struct OtlpExporter<S> {
    /// 发送闭包（序列化批次 → 导出通道）。
    send_fn: S,
    /// 已导出事件计数。
    exported: u64,
}

impl<S> OtlpExporter<S>
where
    S: FnMut(&str) -> Result<(), ()>,
{
    /// 创建导出器。
    pub fn new(send_fn: S) -> Self {
        Self {
            send_fn,
            exported: 0,
        }
    }

    /// 导出事件为 OTLP 行。
    #[allow(clippy::result_unit_err)] // Err(()) = 通道不可用，无附加信息
    pub fn export(&mut self, event: &TelemetryEvent) -> Result<(), ()> {
        let line = alloc::format!(
            "{{\"resource\":{{\"attributes\":[]}},\"scopeMetrics\":[],\"log\":{{\"event_id\":{},\"timestamp\":{},\"category\":{},\"payload\":\"{}\"}}}}",
            event.id, event.timestamp, event.category as u8, event.payload
        );
        (self.send_fn)(&line)?;
        self.exported += 1;
        Ok(())
    }

    /// 已导出计数。
    pub fn exported_count(&self) -> u64 {
        self.exported
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::TelemetryCategory;

    fn ev(id: u64, payload: &str) -> TelemetryEvent {
        TelemetryEvent {
            id,
            category: TelemetryCategory::SystemMetrics,
            timestamp: 1000 + id,
            payload: String::from(payload),
        }
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let snap = QueueSnapshot {
            last_acked_id: 42,
            events: vec![ev(43, "cpu=50%"), ev(44, "mem=1.2GB")],
                    capacity: 16,
            policy: crate::queue::OverflowPolicy::DropOldest,
        };
        let data = persist_snapshot(&snap);
        let loaded = load_snapshot(&data);
        assert_eq!(loaded.last_acked_id, 42);
        assert_eq!(loaded.events.len(), 2);
        assert_eq!(loaded.events[0].id, 43);
        assert_eq!(loaded.events[0].payload, "cpu=50%");
        assert_eq!(loaded.events[1].id, 44);
    }

    #[test]
    fn test_snapshot_empty_default() {
        let data = persist_snapshot(&QueueSnapshot::default());
        let loaded = load_snapshot(&data);
        assert_eq!(loaded.last_acked_id, 0);
        assert!(loaded.events.is_empty());
    }

    #[test]
    fn test_load_invalid_lines_skipped() {
        let snap = load_snapshot(b"garbage\n#last_acked=7\n,,,\n");
        assert_eq!(snap.last_acked_id, 7);
        assert!(snap.events.is_empty());
    }

    #[test]
    fn test_utf8_payload_roundtrip() {
        let snap = QueueSnapshot {
            last_acked_id: 1,
            events: vec![ev(2, "温度=35.5°C")],
                    capacity: 16,
            policy: crate::queue::OverflowPolicy::DropOldest,
        };
        let data = persist_snapshot(&snap);
        let loaded = load_snapshot(&data);
        assert_eq!(loaded.events[0].payload, "温度=35.5°C");
    }

    #[test]
    fn test_otlp_exporter_end_to_end() {
        let exported: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let mut exporter = OtlpExporter::new(|line: &str| {
            exported.borrow_mut().push(String::from(line));
            Ok(())
        });
        exporter.export(&ev(1, "cpu=10%")).expect("export");
        assert_eq!(exported.borrow().len(), 1);
        assert!(exported.borrow()[0].contains("\"event_id\":1"));
        assert_eq!(exporter.exported_count(), 1);
    }

    #[test]
    fn test_otlp_exporter_error_propagates() {
        let mut exporter = OtlpExporter::new(|_line: &str| Err(()));
        assert!(exporter.export(&ev(1, "x")).is_err());
        assert_eq!(exporter.exported_count(), 0);
    }

    // ===== 重启补传（幂等断点） =====

    #[test]
    fn test_restart_retransmit_from_breakpoint() {
        let mut q = crate::queue::TelemetryQueue::new(64, crate::queue::OverflowPolicy::DropOldest);
        for i in 1..=5u64 {
            q.enqueue(crate::queue::TelemetryCategory::SystemMetrics, i, "m")
                .expect("enqueue");
        }
        let snap = q.snapshot(3); // 已确认发送 1..3
        let (mut restored, last_acked) = crate::queue::TelemetryQueue::restore(snap);
        assert_eq!(last_acked, 3);
        assert_eq!(restored.len(), 5);
        // 幂等断点：只补传 > last_acked 的事件。
        let batch = restored
            .dequeue_batch(10)
            .into_iter()
            .filter(|e| e.id > last_acked)
            .collect::<Vec<_>>();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].id, 4);
        assert_eq!(batch[1].id, 5);
    }

    #[test]
    fn test_snapshot_empty_queue() {
        let q = crate::queue::TelemetryQueue::new(16, crate::queue::OverflowPolicy::DropOldest);
        let snap = q.snapshot(0);
        assert!(snap.events.is_empty());
        assert_eq!(snap.last_acked_id, 0);
    }
}
