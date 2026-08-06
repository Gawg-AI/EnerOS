//! 追踪器实现与链路查询/渲染/不完整检测（v0.122.0）.
//!
//! 对应蓝图 §4.2（接口定义）/ §4.5（关键代码）。偏差适配：
//! - D1：start_span/finish_span 经 `now_ms` 参数传入时间戳；
//! - D2：`get_trace` 返回 `alloc::vec::Vec<Span>`；
//! - D3：`start_span` 的 parent 为 `Option<&TraceContext>`（跨 Agent 父
//!   Span 不在本地存储，继承父 trace_id 而非本地查找）；
//! - D4：采样在 trace 创建时判定并随上下文传播，未采样 span 不入库。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::span::{Span, SpanBuilder, SpanId, SpanStatus, TraceId};
use crate::trace_context::TraceContext;

/// 追踪器 trait（蓝图 §4.2；偏差 D2/D3 签名调整）.
pub trait Tracer {
    /// 开始一个 Span；`parent` 为上游注入的追踪上下文（跨 Agent 场景父
    /// Span 不在本地存储），携带时继承其 trace_id/sampled 并以父 span_id
    /// 为 parent_span_id；`now_ms` 为开始时间戳.
    fn start_span(&mut self, operation: &str, parent: Option<&TraceContext>, now_ms: u64) -> Span;
    /// 结束 Span：设置 end_ms/status=Finished，并同步更新内部存储中对应
    /// span_id 的 end_ms/status.
    fn finish_span(&mut self, span: &mut Span, now_ms: u64);
    /// 注入上下文（用于跨 Agent 传播）；sampled 与 span 一致（偏差 D4，
    /// 蓝图每次重掷采样，同一 trace 内不一致）.
    fn inject_context(&self, span: &Span) -> TraceContext;
    /// 提取上下文（偏差 D3）：校验 trace_id 非全零后返回父引用；跨 Agent
    /// 父 Span 不在本地存储，不做本地查找.
    fn extract_context(&self, ctx: &TraceContext) -> Option<TraceContext>;
    /// 查询某条链路的全部入库 Span（保持入库顺序）.
    fn get_trace(&self, trace_id: &TraceId) -> Vec<Span>;
}

/// 追踪器实现（蓝图 §4.5；spans 容量 512，满时丢弃最旧）.
pub struct TracerImpl {
    /// Span 存储（满 512 丢弃最旧）.
    spans: heapless::Vec<Span, 512>,
    /// 采样率 0.0~1.0.
    sampling_rate: f32,
    /// Span ID 计数器.
    next_span_counter: u64,
}

impl TracerImpl {
    /// 创建追踪器；`sampling_rate` 超出 [0.0, 1.0] 时截断到边界.
    pub fn new(sampling_rate: f32) -> Self {
        Self {
            spans: heapless::Vec::new(),
            sampling_rate: sampling_rate.clamp(0.0, 1.0),
            next_span_counter: 1,
        }
    }

    /// 生成新的 Trace ID：前 8 字节 = now_ms 大端，后 8 字节 = 计数器大端.
    fn generate_trace_id(&self, now_ms: u64) -> TraceId {
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&now_ms.to_be_bytes());
        id[8..].copy_from_slice(&self.next_span_counter.to_be_bytes());
        TraceId(id)
    }

    /// 生成新的 Span ID：计数器大端（每次调用自增）.
    fn generate_span_id(&mut self) -> SpanId {
        let id = self.next_span_counter;
        self.next_span_counter += 1;
        SpanId(id.to_be_bytes())
    }

    /// 采样判定（偏差 D4：仅在 trace 创建时调用；确定性计数器取模，
    /// rate=1.0 全采、0.0 全不采）.
    fn should_sample(&self) -> bool {
        if self.sampling_rate >= 1.0 {
            return true;
        }
        if self.sampling_rate <= 0.0 {
            return false;
        }
        ((self.next_span_counter % 100) as f32 / 100.0) < self.sampling_rate
    }

    /// 当前入库 Span 数（供测试与运维观测）.
    pub fn stored(&self) -> usize {
        self.spans.len()
    }

    /// 链路不完整检测：返回该 trace 中 status==Active 的 Span（用于
    /// Agent 崩溃导致的链路不完整检测）.
    pub fn incomplete_spans(&self, trace_id: &TraceId) -> Vec<Span> {
        self.spans
            .iter()
            .filter(|s| s.trace_id == *trace_id && s.status == SpanStatus::Active)
            .cloned()
            .collect()
    }
}

impl Tracer for TracerImpl {
    fn start_span(&mut self, operation: &str, parent: Option<&TraceContext>, now_ms: u64) -> Span {
        // 采样判定仅在 trace 创建时（无父上下文）进行；有父时继承父上下文
        let (trace_id, parent_span_id, sampled) = match parent {
            Some(ctx) => (ctx.trace_id, Some(ctx.span_id), ctx.sampled),
            None => (self.generate_trace_id(now_ms), None, self.should_sample()),
        };
        let span_id = self.generate_span_id();
        let mut builder = SpanBuilder::new(operation, trace_id, span_id, now_ms).sampled(sampled);
        if let Some(p) = parent_span_id {
            builder = builder.parent(p);
        }
        let span = builder.build();
        // 未采样 span 不入库但正常返回句柄（sampled=false 随上下文传播）
        if sampled {
            if self.spans.is_full() {
                // 满 512 丢弃最旧
                self.spans.remove(0);
            }
            let _ = self.spans.push(span.clone());
        }
        span
    }

    fn finish_span(&mut self, span: &mut Span, now_ms: u64) {
        span.end_ms = now_ms;
        span.status = SpanStatus::Finished;
        // 同步更新存储中的 Span（未采样的不在存储中，查找落空即跳过）
        if let Some(stored) = self.spans.iter_mut().find(|s| s.span_id == span.span_id) {
            stored.end_ms = now_ms;
            stored.status = SpanStatus::Finished;
        }
    }

    fn inject_context(&self, span: &Span) -> TraceContext {
        TraceContext {
            trace_id: span.trace_id,
            span_id: span.span_id,
            sampled: span.sampled,
        }
    }

    fn extract_context(&self, ctx: &TraceContext) -> Option<TraceContext> {
        if ctx.trace_id.0.iter().all(|b| *b == 0) {
            None
        } else {
            Some(*ctx)
        }
    }

    fn get_trace(&self, trace_id: &TraceId) -> Vec<Span> {
        self.spans
            .iter()
            .filter(|s| s.trace_id == *trace_id)
            .cloned()
            .collect()
    }
}

/// 判定 span 是否为渲染树的根：parent 为 None 或父不在集合中.
fn is_root(spans: &[Span], s: &Span) -> bool {
    match s.parent_span_id {
        None => true,
        Some(p) => !spans.iter().any(|x| x.span_id == p),
    }
}

/// 状态标记：Finished=OK，Error=ERR，Active=未完成.
fn status_mark(s: &Span) -> &'static str {
    match s.status {
        SpanStatus::Finished => "OK",
        SpanStatus::Error => "ERR",
        SpanStatus::Active => "未完成",
    }
}

/// 输出一行：`{缩进}{operation} [{agent}] {耗时ms|-|} {状态}`.
fn push_line(out: &mut String, s: &Span, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(s.operation_name.as_str());
    out.push_str(" [");
    out.push_str(s.agent_id.as_str());
    out.push_str("] ");
    if s.status == SpanStatus::Active {
        // 未完成 span 无耗时
        out.push('-');
    } else {
        let _ = write!(out, "{}ms", s.end_ms.saturating_sub(s.start_ms));
    }
    out.push(' ');
    out.push_str(status_mark(s));
    out.push('\n');
}

/// 将一组 Span 渲染为按父子关系缩进的文本调用树（链路可视化）.
///
/// 根 = parent_span_id 为 None 或父不在集合中；缩进 2 空格/层；每行含
/// operation_name/agent_id/耗时 ms（未完成显示 "-"）/状态标记
/// （OK/ERR/未完成）。使用显式栈迭代（避免深层递归占用 no_std 栈）。
/// 已渲染索引防护：跨 Agent 汇聚场景下 span_id 计数器各自独立可能冲突
/// 成环（如两 Agent 的 span_id 相同导致自指父链），跳过重复索引防止
/// 死循环（实现期补充，见 lib.rs 偏差声明 D7）。
pub fn render_trace(spans: &[Span]) -> String {
    let mut out = String::new();
    // 显式栈：(span 索引, 深度)；子节点逆序入栈使弹出顺序与入参一致
    let mut stack: Vec<(usize, usize)> = Vec::new();
    // 已渲染索引集合（span_id 冲突成环时跳过重复渲染，防死循环）
    let mut done: Vec<usize> = Vec::new();
    for (i, s) in spans.iter().enumerate().rev() {
        if is_root(spans, s) {
            stack.push((i, 0));
        }
    }
    while let Some((i, depth)) = stack.pop() {
        if done.contains(&i) {
            continue;
        }
        done.push(i);
        let s = &spans[i];
        push_line(&mut out, s, depth);
        for (j, c) in spans.iter().enumerate().rev() {
            if c.parent_span_id == Some(s.span_id) {
                stack.push((j, depth + 1));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::trace_context::{decode_trace_context, encode_trace_context};

    /// 手工构造上游（另一 Agent）传来的父上下文.
    fn remote_parent(sampled: bool) -> TraceContext {
        TraceContext {
            trace_id: TraceId([0x11; 16]),
            span_id: SpanId([0x22; 8]),
            sampled,
        }
    }

    /// TRC7：start 无父生成新 trace（非零 ID、Active、end=0、parent=None、入库）.
    #[test]
    fn trc7_start_without_parent_creates_trace() {
        let mut t = TracerImpl::new(1.0);
        let s = t.start_span("federate_request", None, 1000);
        // trace_id 非全零；前 8 字节为 now_ms 大端
        assert!(s.trace_id.0.iter().any(|b| *b != 0));
        assert_eq!(&s.trace_id.0[..8], &1000u64.to_be_bytes());
        // span_id 非全零（首个计数器为 1）
        assert_eq!(s.span_id.0, 1u64.to_be_bytes());
        assert_eq!(s.parent_span_id, None);
        assert_eq!(s.status, SpanStatus::Active);
        assert_eq!(s.end_ms, 0);
        assert!(s.sampled);
        assert_eq!(t.stored(), 1);
    }

    /// TRC8：带父 ctx 继承 trace_id 且父子正确（跨 Agent 父 Span 不在本地存储亦正确，D3 核心）.
    #[test]
    fn trc8_start_with_remote_parent_inherits() {
        let mut t = TracerImpl::new(1.0);
        // 模拟经 26B 头部接收上游上下文（D5：编解码还原后提取）
        let wire = encode_trace_context(&remote_parent(true));
        let received = decode_trace_context(&wire);
        let parent = t.extract_context(&received);
        assert_eq!(parent, Some(received));
        // 本地存储中并无父 Span（跨 Agent），仍能正确继承
        let s = t.start_span("process", parent.as_ref(), 2000);
        assert_eq!(s.trace_id, received.trace_id);
        assert_eq!(s.parent_span_id, Some(received.span_id));
        assert!(s.sampled);
        assert_eq!(t.stored(), 1);
        // 全零 trace_id 视为非法上下文
        let invalid = TraceContext {
            trace_id: TraceId([0; 16]),
            span_id: SpanId([1; 8]),
            sampled: true,
        };
        assert_eq!(t.extract_context(&invalid), None);
    }

    /// TRC9：finish 后存储中 end_ms/status 同步更新.
    #[test]
    fn trc9_finish_updates_storage() {
        let mut t = TracerImpl::new(1.0);
        let mut s = t.start_span("op", None, 1000);
        t.finish_span(&mut s, 1500);
        assert_eq!(s.end_ms, 1500);
        assert_eq!(s.status, SpanStatus::Finished);
        let stored = t.get_trace(&s.trace_id);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].end_ms, 1500);
        assert_eq!(stored[0].status, SpanStatus::Finished);
    }

    /// TRC10：spans 满 512 丢最旧.
    #[test]
    fn trc10_buffer_full_drops_oldest() {
        let mut t = TracerImpl::new(1.0);
        let mut first: Option<Span> = None;
        let mut last: Option<Span> = None;
        for i in 0..513u64 {
            let s = t.start_span("op", None, 10_000 + i);
            if i == 0 {
                first = Some(s.clone());
            }
            last = Some(s);
        }
        assert_eq!(t.stored(), 512);
        // 最旧的第 1 条已被丢弃，最新一条在库
        let first = first.unwrap();
        let last = last.unwrap();
        assert!(t.get_trace(&first.trace_id).is_empty());
        assert_eq!(t.get_trace(&last.trace_id).len(), 1);
    }

    /// TRC11：inject_context 的 trace/span/sampled 与 span 一致.
    #[test]
    fn trc11_inject_context_carries_sampled() {
        let mut t = TracerImpl::new(1.0);
        let s = t.start_span("op", None, 1000);
        let ctx = t.inject_context(&s);
        assert_eq!(ctx.trace_id, s.trace_id);
        assert_eq!(ctx.span_id, s.span_id);
        assert_eq!(ctx.sampled, s.sampled);
        assert!(ctx.sampled);
    }

    /// TRC12：get_trace 只返回目标 trace 的 span.
    #[test]
    fn trc12_get_trace_filters() {
        let mut t = TracerImpl::new(1.0);
        let a1 = t.start_span("a1", None, 1000);
        let ctx_a = t.inject_context(&a1);
        let _a2 = t.start_span("a2", Some(&ctx_a), 1010);
        let _b1 = t.start_span("b1", None, 2000);
        let ta = t.get_trace(&a1.trace_id);
        assert_eq!(ta.len(), 2);
        assert!(ta.iter().all(|s| s.trace_id == a1.trace_id));
        assert_eq!(ta[0].operation_name.as_str(), "a1");
        assert_eq!(ta[1].operation_name.as_str(), "a2");
        let unknown = TraceId([0xEE; 16]);
        assert!(t.get_trace(&unknown).is_empty());
    }

    /// TRC13：incomplete_spans 检出 Active（模拟 Agent 崩溃未 finish）.
    #[test]
    fn trc13_incomplete_spans_detects_active() {
        let mut t = TracerImpl::new(1.0);
        let mut s1 = t.start_span("root", None, 1000);
        let ctx = t.inject_context(&s1);
        let _s2 = t.start_span("child", Some(&ctx), 1010); // 未 finish
        t.finish_span(&mut s1, 1090);
        let inc = t.incomplete_spans(&s1.trace_id);
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].operation_name.as_str(), "child");
        assert_eq!(inc[0].status, SpanStatus::Active);
        // 其它 trace 无 Active
        let other = TraceId([0x33; 16]);
        assert!(t.incomplete_spans(&other).is_empty());
    }

    /// TRC14：render_trace 三 span 缩进树含 operation 与耗时.
    #[test]
    fn trc14_render_trace_indented_tree() {
        let mut t = TracerImpl::new(1.0);
        let mut s1 = t.start_span("federate_request", None, 100);
        let c1 = t.inject_context(&s1);
        let mut s2 = t.start_span("process", Some(&c1), 110);
        let c2 = t.inject_context(&s2);
        let mut s3 = t.start_span("execute", Some(&c2), 120);
        // finish 顺序 C→B→A（跨 Agent 响应回传）
        t.finish_span(&mut s3, 130);
        t.finish_span(&mut s2, 140);
        t.finish_span(&mut s1, 150);
        let spans = t.get_trace(&s1.trace_id);
        let tree = render_trace(&spans);
        // 三行：根无缩进，子 2 空格，孙 4 空格
        assert!(tree.contains("federate_request ["));
        assert!(tree.contains("\n  process ["));
        assert!(tree.contains("\n    execute ["));
        // 耗时 = end - start
        assert!(tree.contains("50ms OK"));
        assert!(tree.contains("30ms OK"));
        assert!(tree.contains("10ms OK"));
        // 未完成标记：再造一个未 finish 的链路
        let mut t2 = TracerImpl::new(1.0);
        let u1 = t2.start_span("root", None, 0);
        let uc = t2.inject_context(&u1);
        let _u2 = t2.start_span("stuck", Some(&uc), 1);
        let tree2 = render_trace(&t2.get_trace(&u1.trace_id));
        assert!(tree2.contains("\n  stuck ["));
        assert!(tree2.contains("- 未完成"));
    }

    /// SMP15：rate=1.0 全部入库且 sampled=true.
    #[test]
    fn smp15_rate_one_all_stored() {
        let mut t = TracerImpl::new(1.0);
        let mut parent: Option<TraceContext> = None;
        for _ in 0..5 {
            let s = t.start_span("op", parent.as_ref(), 1000);
            assert!(s.sampled);
            parent = Some(t.inject_context(&s));
        }
        assert_eq!(t.stored(), 5);
        // 超界截断：>1.0 视为 1.0
        let mut t2 = TracerImpl::new(2.5);
        let s = t2.start_span("op", None, 0);
        assert!(s.sampled);
        assert_eq!(t2.stored(), 1);
    }

    /// SMP16：rate=0.0 全不入库且 inject sampled=false（未采样句柄仍正常返回）.
    #[test]
    fn smp16_rate_zero_none_stored() {
        let mut t = TracerImpl::new(0.0);
        let s = t.start_span("op", None, 1000);
        assert!(!s.sampled);
        // 句柄正常返回：ID 有效、状态 Active
        assert!(s.trace_id.0.iter().any(|b| *b != 0));
        assert_eq!(s.status, SpanStatus::Active);
        assert_eq!(t.stored(), 0);
        // sampled=false 随上下文传播给下游
        let ctx = t.inject_context(&s);
        assert!(!ctx.sampled);
        let child = t.start_span("child", Some(&ctx), 1010);
        assert!(!child.sampled);
        assert_eq!(child.trace_id, s.trace_id);
        assert_eq!(child.parent_span_id, Some(s.span_id));
        assert_eq!(t.stored(), 0);
        // 超界截断：<0.0 视为 0.0
        let mut t2 = TracerImpl::new(-1.0);
        let s2 = t2.start_span("op", None, 0);
        assert!(!s2.sampled);
        assert_eq!(t2.stored(), 0);
    }
}
