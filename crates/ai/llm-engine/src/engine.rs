//! LLM 推理引擎统一 trait（D2：无 Send + Sync bound）.
//!
//! 单线程 no_std RTOS 下，`Send + Sync` 无意义，且 `*mut c_void`（FFI 上下文）
//! 非 `Send`，强加会导致 `LlamaCppEngine` 无法实现（D2）。

use crate::error::LlmError;
use crate::model::ModelInfo;
use crate::params::InferParams;
use crate::stats::{EngineHealth, EngineStats};

/// LLM 推理引擎统一接口.
///
/// 提供模型加载、推理、流式推理、模型信息查询、健康检查、统计查询 6 个方法。
/// 实现方包括 [`crate::mock::MockEngine`]（默认可用）与
/// [`crate::llama_cpp::LlamaCppEngine`]（feature = "llama-cpp"，D3）。
pub trait LlmEngine {
    /// 加载模型.
    ///
    /// 成功后 `model_info()` 返回 `Some`，`stats.model_load_count += 1`。
    fn load_model(&mut self, path: &str) -> Result<(), LlmError>;

    /// 同步推理.
    ///
    /// 返回生成文本。`stats.inference_count += 1`，
    /// `stats.total_tokens_generated += 生成 token 数`。
    fn infer(
        &mut self,
        prompt: &str,
        params: &InferParams,
    ) -> Result<alloc::string::String, LlmError>;

    /// 流式推理（D8：`&mut dyn FnMut(&str) -> bool`）.
    ///
    /// 逐 token 调用 `callback`。`callback` 返回 `false` 则停止生成。
    /// 返回 `Ok(())` 表示完成（含被 callback 中止的情形）。
    fn infer_stream(
        &mut self,
        prompt: &str,
        params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError>;

    /// 当前已加载模型元数据（未加载返回 `None`）.
    fn model_info(&self) -> Option<&ModelInfo>;

    /// 健康检查.
    fn health_check(&self) -> EngineHealth;

    /// 累计统计.
    fn stats(&self) -> &EngineStats;
}
