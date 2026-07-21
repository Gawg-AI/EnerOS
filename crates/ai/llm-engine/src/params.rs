//! 推理参数（默认值符合 LLM 通用配置）.

/// LLM 推理参数.
///
/// 所有字段均有默认值，调用方按需覆盖。
#[derive(Debug, Clone)]
pub struct InferParams {
    /// 最大生成 token 数（默认 128）.
    pub max_tokens: u32,
    /// 温度（默认 0.7，越高越随机）.
    pub temperature: f32,
    /// top-p 采样阈值（默认 0.9）.
    pub top_p: f32,
    /// top-k 采样阈值（默认 40）.
    pub top_k: u32,
    /// 重复惩罚（默认 1.1）.
    pub repeat_penalty: f32,
    /// 停止 token 列表（默认空）.
    pub stop_tokens: alloc::vec::Vec<alloc::string::String>,
}

impl Default for InferParams {
    fn default() -> Self {
        Self {
            max_tokens: 128,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            repeat_penalty: 1.1,
            stop_tokens: alloc::vec::Vec::new(),
        }
    }
}

impl InferParams {
    /// 使用默认值构造推理参数.
    pub fn new() -> Self {
        Self::default()
    }
}
