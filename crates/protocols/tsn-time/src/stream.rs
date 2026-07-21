//! v0.80.0 TSN Stream 过滤数据类型（最小骨架，无真实 802.1Qci 过滤逻辑）.
//!
//! 提供 [`StreamId`] newtype 与 [`StreamFilter`] 纯数据结构。真实 per-stream
//! 过滤（802.1Qci）延后到后续版本；本版本仅提供类型骨架用于配置与路由
//! 关联.

use core::fmt;

/// Stream 标识（newtype：`pub u32`）.
///
/// 派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`，实现 `Display`
/// （输出十进制数字字符串）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(pub u32);

impl StreamId {
    /// 以 `u32` 构造 Stream 标识.
    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stream 过滤器（D14：纯数据类型，无真实 802.1Qci per-stream 过滤逻辑）.
///
/// 关联 [`StreamId`] / `gate_id` / `priority`，用于将流映射到 TAS 门控
/// 与优先级队列.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFilter {
    /// 关联的 Stream 标识.
    pub stream_id: StreamId,
    /// 关联的 TAS 门控 ID.
    pub gate_id: u8,
    /// 优先级（802.1Q PCP）.
    pub priority: u8,
}

impl StreamFilter {
    /// 构造 Stream 过滤器.
    pub fn new(stream_id: StreamId, gate_id: u8, priority: u8) -> Self {
        Self {
            stream_id,
            gate_id,
            priority,
        }
    }
}
