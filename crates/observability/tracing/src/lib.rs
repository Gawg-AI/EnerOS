//! EnerOS v0.122.0 分布式追踪（P2-J 可观测性第 3 版·收官版）.
//!
//! 实现跨 Agent 请求的分布式追踪：为每个联邦请求生成全局唯一 Trace ID、
//! 记录 Span 调用链并支持链路可视化，与 v0.120.0 指标、v0.121.0 日志共同
//! 补齐"指标 + 日志 + 追踪"三支柱，支撑 Phase 2 出口运维可观测性。
//!
//! # 核心类型
//!
//! - [`TraceId`] / [`SpanId`] / [`SpanStatus`] / [`Span`] / [`TraceError`] —
//!   追踪数据模型（operation≤64 字节、tags≤8 个键值对、agent_id≤32 字节，
//!   与 OpenTelemetry 概念对齐）
//! - [`SpanBuilder`] — 流畅构建 API（tag 超 8 个截断，字符串超长截断不
//!   panic）
//! - [`TraceContext`] / [`encode_trace_context`] / [`decode_trace_context`] —
//!   跨 Agent 传播上下文与 26 字节编解码（16B trace_id + 8B span_id +
//!   1B sampled + 1B 保留，互为逆运算）
//! - [`Tracer`] / [`TracerImpl`] — 追踪器（spans 容量 512 满丢最旧、采样
//!   在 trace 创建时判定并随上下文传播）
//! - [`render_trace`] — 按父子关系缩进的文本调用树（含 operation/agent/
//!   耗时/状态标记）；[`TracerImpl::incomplete_spans`] — 链路不完整检测
//!
//! # 偏差声明（相对蓝图）
//!
//! 1. **D1 时间戳参数**：蓝图 `crate::current_time_ms()` 系统时钟调用，本
//!    实现改为 `now_ms: u64` 参数传入（start_span/finish_span/
//!    SpanBuilder::new/generate_trace_id，no_std 无系统时钟）。
//! 2. **D2 返回类型**：蓝图 `get_trace` 返回 std `Vec<Span>`，本实现返回
//!    `alloc::vec::Vec<Span>`（no_std 合规）。
//! 3. **D3 父上下文签名**：蓝图 `start_span(parent: Option<SpanId>)` 按
//!    SpanId 本地查找父 Span，跨 Agent 父 Span 不在本地存储会错误生成新
//!    trace_id；本实现调整为 `Option<&TraceContext>`，携带父上下文时继承
//!    其 trace_id、以父 span_id 为 parent_span_id；`extract_context` 语义
//!    相应调整为"校验上下文格式并返回父引用"。
//! 4. **D4 采样一致性**：Span 增加 `sampled: bool` 字段，采样判定在
//!    trace 创建时（无父上下文）确定并随 TraceContext 传播（蓝图
//!    inject_context 每次重掷采样，同一 trace 内不一致）；未采样 span
//!    不入库但正常返回句柄。
//! 5. **D5 DDS 头部传播**：以 26 字节编解码格式交付；DDS 栈消息传播在
//!    集成测试层以模拟消息验证（不依赖 eneros-agent-bus-dds，避免耦合）。
//! 6. **D6 Agent 标识类型**：蓝图 `Span.agent_id: u32`，本实现调整为
//!    `heapless::String<32>`（跨 Agent 场景以字符串标识更可读，且与
//!    v0.121.0 日志 target 口径一致）。
//! 7. **D7 渲染成环防护**：蓝图 `span_id` 计数器按 Tracer 实例独立自增，
//!    跨 Agent 汇聚时不同 Agent 的 span_id 可能冲突成环（自指父链），
//!    蓝图渲染算法会死循环；`render_trace` 增加已渲染索引防护，跳过重复
//!    索引（INT1 暴露，最小修复；运维汇聚侧宜以 (trace_id, agent_id)
//!    维度二次消歧）。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 非测试代码仅使用 `core::*` / `alloc::*` / `heapless::*`，不调用
//! `panic!` / `todo!` / `unimplemented!` / `unwrap()`，可交叉编译到
//! `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod span;
pub mod trace_context;
pub mod tracing;

pub use span::{Span, SpanBuilder, SpanId, SpanStatus, TraceError, TraceId};
pub use trace_context::{decode_trace_context, encode_trace_context, TraceContext};
pub use tracing::{render_trace, Tracer, TracerImpl};
