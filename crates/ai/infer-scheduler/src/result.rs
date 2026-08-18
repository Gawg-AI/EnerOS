//! 推理结果.

use alloc::string::String;

use crate::error::SchedulerError;

/// LLM 推理结果.
///
/// 包含请求 ID、关联 ID 与推理输出（`Ok(String)`）或错误（`Err(SchedulerError)`）。
#[derive(Debug, Clone)]
pub struct InferResult {
    /// 请求 ID.
    pub id: u64,
    /// 蓝图 40 WP-40.4：请求关联 ID（调用方据此丢弃 late result）。
    pub correlation: [u8; 16],
    /// 推理结果.
    pub result: Result<String, SchedulerError>,
}

impl InferResult {
    /// 创建推理结果.
    pub fn new(id: u64, correlation: [u8; 16], result: Result<String, SchedulerError>) -> Self {
        Self {
            id,
            correlation,
            result,
        }
    }

    /// 创建成功结果.
    pub fn success(id: u64, correlation: [u8; 16], output: String) -> Self {
        Self {
            id,
            correlation,
            result: Ok(output),
        }
    }

    /// 创建失败结果.
    pub fn failure(id: u64, correlation: [u8; 16], error: SchedulerError) -> Self {
        Self {
            id,
            correlation,
            result: Err(error),
        }
    }
}
