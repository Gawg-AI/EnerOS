//! 推理结果.

use alloc::string::String;

use crate::error::SchedulerError;

/// LLM 推理结果.
///
/// 包含请求 ID 与推理输出（`Ok(String)`）或错误（`Err(SchedulerError)`）。
#[derive(Debug, Clone)]
pub struct InferResult {
    /// 请求 ID.
    pub id: u64,
    /// 推理结果.
    pub result: Result<String, SchedulerError>,
}

impl InferResult {
    /// 创建推理结果.
    pub fn new(id: u64, result: Result<String, SchedulerError>) -> Self {
        Self { id, result }
    }

    /// 创建成功结果.
    pub fn success(id: u64, output: String) -> Self {
        Self {
            id,
            result: Ok(output),
        }
    }

    /// 创建失败结果.
    pub fn failure(id: u64, error: SchedulerError) -> Self {
        Self {
            id,
            result: Err(error),
        }
    }
}
