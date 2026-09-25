//! Span 数据模型与构建器（v0.122.0）.
//!
//! 对应蓝图 §4.1（数据结构）/ §4.4（错误处理）/ §4.5（SpanBuilder）。
//! 按偏差 D1，`SpanBuilder::new` 通过 `now_ms` 参数传入时间戳；按偏差
//! D4，`Span` 增加 `sampled` 字段；按偏差 D6，`agent_id` 为定容字符串。

use eneros_jsonl_wire::hstr;
use heapless::String as HString;

/// Trace ID（128 位，蓝图 §4.1）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceId(pub [u8; 16]);

/// Span ID（64 位，蓝图 §4.1）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanId(pub [u8; 8]);

/// Span 状态（蓝图 §4.1）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanStatus {
    /// 进行中（未 finish）.
    Active,
    /// 正常结束.
    Finished,
    /// 异常结束.
    Error,
}

/// 追踪全流程错误（蓝图 §4.4）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// Span 未找到.
    SpanNotFound,
    /// 上下文非法.
    ContextInvalid,
    /// 缓冲满.
    BufferFull,
    /// Trace 未找到.
    TraceNotFound,
}

/// Span（蓝图 §4.1；偏差 D4 增加 sampled；偏差 D6 agent_id 为字符串）.
#[derive(Debug, Clone)]
pub struct Span {
    /// 所属 Trace ID.
    pub trace_id: TraceId,
    /// Span ID.
    pub span_id: SpanId,
    /// 父 Span ID（跨 Agent 父 Span 不在本地存储时亦为 Some）.
    pub parent_span_id: Option<SpanId>,
    /// 操作名（≤64 字节）.
    pub operation_name: HString<64>,
    /// 开始时间（毫秒）.
    pub start_ms: u64,
    /// 结束时间（毫秒，未完成为 0）.
    pub end_ms: u64,
    /// 状态.
    pub status: SpanStatus,
    /// 标签（≤8 个键值对，key≤32 字节，value≤64 字节）.
    pub tags: heapless::Vec<(HString<32>, HString<64>), 8>,
    /// 所属 Agent 标识（≤32 字节）.
    pub agent_id: HString<32>,
    /// 是否采样（trace 创建时判定，随上下文传播）.
    pub sampled: bool,
}

/// Span 构建器（蓝图 §4.5；偏差 D1：new 增加 now_ms 参数）.
pub struct SpanBuilder {
    span: Span,
}

impl SpanBuilder {
    /// 创建构建器；`now_ms` 为开始时间戳（替代蓝图系统时钟调用）.
    pub fn new(operation: &str, trace_id: TraceId, span_id: SpanId, now_ms: u64) -> Self {
        Self {
            span: Span {
                trace_id,
                span_id,
                parent_span_id: None,
                operation_name: hstr(operation),
                start_ms: now_ms,
                end_ms: 0,
                status: SpanStatus::Active,
                tags: heapless::Vec::new(),
                agent_id: HString::new(),
                sampled: false,
            },
        }
    }

    /// 设置父 Span ID.
    pub fn parent(mut self, parent: SpanId) -> Self {
        self.span.parent_span_id = Some(parent);
        self
    }

    /// 追加标签；超过 8 个截断丢弃.
    pub fn tag(mut self, key: &str, value: &str) -> Self {
        let _ = self.span.tags.push((hstr(key), hstr(value)));
        self
    }

    /// 设置所属 Agent 标识（超长截断）.
    pub fn agent(mut self, agent_id: &str) -> Self {
        self.span.agent_id = hstr(agent_id);
        self
    }

    /// 设置采样标记.
    pub fn sampled(mut self, sampled: bool) -> Self {
        self.span.sampled = sampled;
        self
    }

    /// 构建 Span.
    pub fn build(self) -> Span {
        self.span
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn tid() -> TraceId {
        TraceId([0xAA; 16])
    }

    fn sid() -> SpanId {
        SpanId([0xBB; 8])
    }

    /// SPN1：builder 字段齐全（trace/span/operation/start/end/status/parent/tags/sampled 默认值）.
    #[test]
    fn spn1_builder_fields_complete() {
        let s = SpanBuilder::new("federate_request", tid(), sid(), 1234).build();
        assert_eq!(s.trace_id, tid());
        assert_eq!(s.span_id, sid());
        assert_eq!(s.operation_name.as_str(), "federate_request");
        assert_eq!(s.start_ms, 1234);
        assert_eq!(s.end_ms, 0);
        assert_eq!(s.status, SpanStatus::Active);
        assert_eq!(s.parent_span_id, None);
        assert!(s.tags.is_empty());
        assert!(s.agent_id.is_empty());
        assert!(!s.sampled);
        // 超长 operation 截断到 64 字节不 panic
        let long_op = "x".repeat(100);
        let s2 = SpanBuilder::new(&long_op, tid(), sid(), 0).build();
        assert_eq!(s2.operation_name.len(), 64);
    }

    /// SPN2：parent 设置正确，tag 超 8 个截断丢弃.
    #[test]
    fn spn2_parent_and_tag_truncation() {
        let parent = SpanId([0x01; 8]);
        let mut b = SpanBuilder::new("op", tid(), sid(), 0).parent(parent);
        for i in 0..10u8 {
            let mut key = HString::<32>::new();
            let _ = key.push('k');
            let _ = key.push(char::from(b'0' + i));
            b = b.tag(key.as_str(), "v");
        }
        let s = b.build();
        assert_eq!(s.parent_span_id, Some(parent));
        assert_eq!(s.tags.len(), 8);
        assert_eq!(s.tags[7].0.as_str(), "k7");
        // 超长 key/value 截断不 panic
        let long_k = "k".repeat(40);
        let long_v = "v".repeat(80);
        let s2 = SpanBuilder::new("op", tid(), sid(), 0)
            .tag(&long_k, &long_v)
            .build();
        assert_eq!(s2.tags[0].0.len(), 32);
        assert_eq!(s2.tags[0].1.len(), 64);
    }

    /// SPN3：agent/sampled 设置正确.
    #[test]
    fn spn3_agent_and_sampled() {
        let s = SpanBuilder::new("op", tid(), sid(), 0)
            .agent("agent-A")
            .sampled(true)
            .build();
        assert_eq!(s.agent_id.as_str(), "agent-A");
        assert!(s.sampled);
        // 超长 agent_id 截断到 32 字节不 panic
        let long_agent = "a".repeat(50);
        let s2 = SpanBuilder::new("op", tid(), sid(), 0)
            .agent(&long_agent)
            .build();
        assert_eq!(s2.agent_id.len(), 32);
    }
}
