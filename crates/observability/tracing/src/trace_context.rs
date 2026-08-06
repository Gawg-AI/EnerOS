//! 追踪上下文与 26 字节编解码（v0.122.0）.
//!
//! 对应蓝图 §4.1（TraceContext）/ §4.5（encode/decode）。按偏差 D5，
//! 26 字节格式作为 DDS 消息头部：16B trace_id + 8B span_id +
//! 1B sampled(0/1) + 1B 保留(0)，encode/decode 互为逆运算。

use crate::span::{SpanId, TraceId};

/// 追踪上下文（跨 Agent 传播，蓝图 §4.1）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceContext {
    /// Trace ID.
    pub trace_id: TraceId,
    /// 当前 Span ID（下游以其为 parent）.
    pub span_id: SpanId,
    /// 是否采样（trace 创建时判定，随上下文传播）.
    pub sampled: bool,
}

/// 编码追踪上下文为 26 字节（蓝图 §4.5；DDS 消息头部格式）.
pub fn encode_trace_context(ctx: &TraceContext) -> [u8; 26] {
    let mut buf = [0u8; 26];
    buf[..16].copy_from_slice(&ctx.trace_id.0);
    buf[16..24].copy_from_slice(&ctx.span_id.0);
    buf[24] = ctx.sampled as u8;
    buf[25] = 0; // 保留
    buf
}

/// 解码 26 字节为追踪上下文（与 [`encode_trace_context`] 互逆）.
pub fn decode_trace_context(buf: &[u8; 26]) -> TraceContext {
    let mut trace_id = [0u8; 16];
    trace_id.copy_from_slice(&buf[..16]);
    let mut span_id = [0u8; 8];
    span_id.copy_from_slice(&buf[16..24]);
    TraceContext {
        trace_id: TraceId(trace_id),
        span_id: SpanId(span_id),
        sampled: buf[24] != 0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn ctx() -> TraceContext {
        let mut t = [0u8; 16];
        for (i, b) in t.iter_mut().enumerate() {
            *b = i as u8 + 1; // 1..=16
        }
        let mut s = [0u8; 8];
        for (i, b) in s.iter_mut().enumerate() {
            *b = i as u8 + 101; // 101..=108
        }
        TraceContext {
            trace_id: TraceId(t),
            span_id: SpanId(s),
            sampled: true,
        }
    }

    /// CTX4：encode 字节布局正确（逐字节断言偏移）.
    #[test]
    fn ctx4_encode_byte_layout() {
        let c = ctx();
        let buf = encode_trace_context(&c);
        // 0..16：trace_id 原样
        for (i, b) in buf.iter().enumerate().take(16) {
            assert_eq!(*b, i as u8 + 1);
        }
        // 16..24：span_id 原样
        for i in 0..8 {
            assert_eq!(buf[16 + i], i as u8 + 101);
        }
        // 24：sampled=1；25：保留=0
        assert_eq!(buf[24], 1);
        assert_eq!(buf[25], 0);
        // sampled=false 时第 24 字节为 0
        let mut c2 = c;
        c2.sampled = false;
        assert_eq!(encode_trace_context(&c2)[24], 0);
    }

    /// CTX5：decode 还原字段.
    #[test]
    fn ctx5_decode_restores_fields() {
        let mut buf = [0u8; 26];
        for (i, b) in buf.iter_mut().enumerate().take(16) {
            *b = 0xF0 - i as u8;
        }
        for (i, b) in buf.iter_mut().enumerate().skip(16).take(8) {
            *b = 0x0F + (i - 16) as u8;
        }
        buf[24] = 1;
        buf[25] = 0;
        let c = decode_trace_context(&buf);
        assert_eq!(&c.trace_id.0[..], &buf[..16]);
        assert_eq!(&c.span_id.0[..], &buf[16..24]);
        assert!(c.sampled);
        // 第 24 字节非 1 的非零值亦视为 sampled
        let mut buf2 = buf;
        buf2[24] = 7;
        assert!(decode_trace_context(&buf2).sampled);
        buf2[24] = 0;
        assert!(!decode_trace_context(&buf2).sampled);
    }

    /// CTX6：encode→decode 往返逐字段相等.
    #[test]
    fn ctx6_roundtrip_field_equal() {
        for sampled in [true, false] {
            let mut c = ctx();
            c.sampled = sampled;
            let back = decode_trace_context(&encode_trace_context(&c));
            assert_eq!(back.trace_id, c.trace_id);
            assert_eq!(back.span_id, c.span_id);
            assert_eq!(back.sampled, c.sampled);
            assert_eq!(back, c);
        }
    }
}
