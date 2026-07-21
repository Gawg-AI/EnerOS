//! 调度器错误类型（D7：5 变体 + Display + From<LlmError>）.
//!
//! `SchedulerError` 包装队列满、超时、KV Cache 耗尽、引擎错误、未调度 5 类失败。
//! `Engine` 变体内嵌 v0.59.0 `LlmError`，通过 `From<LlmError>` 自动转换。

use core::fmt;

use eneros_llm_engine::LlmError;

/// 调度器错误.
///
/// 覆盖队列满、超时、KV Cache 耗尽、引擎错误、未调度 5 类失败场景。
#[derive(Debug, Clone)]
pub enum SchedulerError {
    /// 队列已满（超出并发上限）.
    QueueFull,
    /// 请求超时（`now_ns - submitted_at_ns > timeout_ns`）.
    Timeout,
    /// KV Cache 耗尽（超出预算且无可淘汰条目）.
    CacheExhausted,
    /// 引擎错误（包装 v0.59.0 `LlmError`）.
    Engine(LlmError),
    /// 请求未调度（未提交到队列）.
    NotScheduled,
}

// 手动实现 PartialEq：LlmError 未派生 PartialEq，用 discriminant 比较（全单元变体）.
impl PartialEq for SchedulerError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::QueueFull, Self::QueueFull) => true,
            (Self::Timeout, Self::Timeout) => true,
            (Self::CacheExhausted, Self::CacheExhausted) => true,
            (Self::NotScheduled, Self::NotScheduled) => true,
            (Self::Engine(a), Self::Engine(b)) => {
                core::mem::discriminant(a) == core::mem::discriminant(b)
            }
            _ => false,
        }
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerError::QueueFull => f.write_str("queue full"),
            SchedulerError::Timeout => f.write_str("request timeout"),
            SchedulerError::CacheExhausted => f.write_str("kv cache exhausted"),
            SchedulerError::Engine(e) => write!(f, "engine error: {}", e),
            SchedulerError::NotScheduled => f.write_str("not scheduled"),
        }
    }
}

impl From<LlmError> for SchedulerError {
    fn from(e: LlmError) -> Self {
        SchedulerError::Engine(e)
    }
}
