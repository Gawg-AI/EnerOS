//! 测试替身：脚本化引擎（`mock_output` 仅用于流式——`infer` 返回
//! `mock: <prompt>` 前缀；本替身让单测能精确控制 `infer` 输出与延迟）。

#![cfg(test)]

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_llm_engine::device::ComputeDevice;
use eneros_llm_engine::engine::LlmEngine;
use eneros_llm_engine::error::LlmError;
use eneros_llm_engine::model::ModelInfo;
use eneros_llm_engine::params::InferParams;
use eneros_llm_engine::stats::{EngineHealth, EngineStats};

/// 脚本化引擎：每次 `infer` 依序弹出脚本输出；耗尽后返回固定尾输出。
#[derive(Debug)]
pub(crate) struct StubEngine {
    pub outputs: VecDeque<String>,
    pub tail_output: String,
    pub stats: EngineStats,
    pub loaded: bool,
    pub load_fail: bool,
}

impl StubEngine {
    pub fn new(outputs: Vec<&str>) -> Self {
        Self {
            outputs: outputs.iter().map(|s| String::from(*s)).collect(),
            tail_output: String::from("{}"),
            stats: EngineStats::default(),
            // 脚本替身默认已加载（测试直接驱动 infer；load 语义由 harness
            // 的 deploy 面单独测试）。
            loaded: true,
            load_fail: false,
        }
    }

    /// 单输出引擎（`MockEngine::with_output` 同名语义：`infer` 恒返回
    /// 该文本，与 llm-engine mock 的 `mock: <prompt>` 前缀解耦）。
    pub fn single(output: &str) -> Self {
        Self::new(vec![output])
    }
}

impl LlmEngine for StubEngine {
    fn load_model(&mut self, _path: &str) -> Result<(), LlmError> {
        if self.load_fail {
            return Err(LlmError::LoadFailed);
        }
        self.loaded = true;
        Ok(())
    }

    fn infer(&mut self, _prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
        if !self.loaded {
            return Err(LlmError::ModelNotLoaded);
        }
        if self.load_fail {
            return Err(LlmError::InferFailed);
        }
        self.stats.inference_count += 1;
        let out = self
            .outputs
            .pop_front()
            .unwrap_or_else(|| self.tail_output.clone());
        self.stats.total_tokens_generated += out.len() as u64;
        Ok(out)
    }

    fn infer_stream(
        &mut self,
        _prompt: &str,
        _params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        let out = self
            .outputs
            .pop_front()
            .unwrap_or_else(|| self.tail_output.clone());
        for chunk in out.chars() {
            if !callback(&chunk.to_string()) {
                break;
            }
        }
        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        None
    }

    fn health_check(&self) -> EngineHealth {
        EngineHealth {
            loaded: self.loaded,
            device: ComputeDevice::Cpu,
            gpu_layers: 0,
            last_error: None,
        }
    }

    fn stats(&self) -> &EngineStats {
        &self.stats
    }

    fn unload(&mut self) -> Result<(), LlmError> {
        self.loaded = false;
        Ok(())
    }
}
