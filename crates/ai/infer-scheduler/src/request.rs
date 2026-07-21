//! 推理请求（D6：now_ns 注入，timeout_ns 为 u64 纳秒）.

use alloc::string::String;

use eneros_llm_engine::InferParams;

use crate::priority::RequestPriority;

/// LLM 推理请求.
///
/// 包含提示词、推理参数、优先级、提交时间戳与超时阈值。
/// 超时判定为 `now_ns - submitted_at_ns > timeout_ns`（饱和减法）。
#[derive(Debug, Clone)]
pub struct InferRequest {
    /// 请求 ID（由调度器 `submit` 分配，构造时传入的值会被覆盖）.
    pub id: u64,
    /// 提示词文本.
    pub prompt: String,
    /// 推理参数（复用 v0.59.0 `InferParams`）.
    pub params: InferParams,
    /// 请求优先级（默认 `Normal`）.
    pub priority: RequestPriority,
    /// 提交时间戳（纳秒，由调用方注入）.
    pub submitted_at_ns: u64,
    /// 超时阈值（纳秒，默认 `u64::MAX` 表示永不超时）.
    pub timeout_ns: u64,
}

impl InferRequest {
    /// 创建推理请求.
    ///
    /// 默认 `priority = Normal`，`submitted_at_ns = 0`，`timeout_ns = u64::MAX`（永不超时）。
    pub fn new(id: u64, prompt: &str, params: InferParams) -> Self {
        Self {
            id,
            prompt: String::from(prompt),
            params,
            priority: RequestPriority::default(),
            submitted_at_ns: 0,
            timeout_ns: u64::MAX,
        }
    }

    /// 设置优先级（builder）.
    pub fn with_priority(mut self, priority: RequestPriority) -> Self {
        self.priority = priority;
        self
    }

    /// 设置超时阈值（builder）.
    pub fn with_timeout(mut self, timeout_ns: u64) -> Self {
        self.timeout_ns = timeout_ns;
        self
    }

    /// 设置提交时间戳（builder）.
    pub fn with_timestamp(mut self, submitted_at_ns: u64) -> Self {
        self.submitted_at_ns = submitted_at_ns;
        self
    }

    /// 判断请求是否超时.
    ///
    /// `now_ns - submitted_at_ns > timeout_ns`（饱和减法，防止下溢）。
    pub fn is_timed_out(&self, now_ns: u64) -> bool {
        now_ns.saturating_sub(self.submitted_at_ns) > self.timeout_ns
    }
}
