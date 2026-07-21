//! MockEngine — 默认可用的 Mock 实现（D3）.
//!
//! 纯 Rust，无外部 C 库依赖。用于单元测试与无 llama.cpp 环境下的接口验证。
//! 推理输出固定（`"mock: <prompt>"`），流式输出按字符切分 `mock_output`。

use alloc::format;
use alloc::string::String;

use crate::device::ComputeDevice;
use crate::engine::LlmEngine;
use crate::error::LlmError;
use crate::model::ModelInfo;
use crate::params::InferParams;
use crate::stats::{EngineHealth, EngineStats};

/// Mock LLM 推理引擎.
#[derive(Debug)]
pub struct MockEngine {
    /// 模型是否已加载.
    loaded: bool,
    /// 目标计算设备.
    device: ComputeDevice,
    /// 当前模型元数据.
    model_info: Option<ModelInfo>,
    /// 累计统计.
    stats: EngineStats,
    /// 流式推理输出内容（默认 `"mock response"`）.
    mock_output: String,
}

impl MockEngine {
    /// 创建 MockEngine.
    ///
    /// 初始 `loaded = false`，`mock_output = "mock response"`。
    pub fn new(device: ComputeDevice) -> Self {
        Self {
            loaded: false,
            device,
            model_info: None,
            stats: EngineStats::default(),
            mock_output: String::from("mock response"),
        }
    }

    /// 构造指定输出的 MockEngine（builder，默认 Cpu 设备）.
    pub fn with_output(output: &str) -> Self {
        let mut engine = Self::new(ComputeDevice::Cpu);
        engine.mock_output = String::from(output);
        engine
    }

    /// 当前计算设备.
    pub fn device(&self) -> ComputeDevice {
        self.device
    }
}

impl LlmEngine for MockEngine {
    fn load_model(&mut self, path: &str) -> Result<(), LlmError> {
        self.loaded = true;
        self.model_info = Some(ModelInfo {
            name: String::from(path),
            size_bytes: 0,
            quantization: crate::model::Quantization::Q4_K_M,
            context_length: 2048,
            device: self.device,
        });
        self.stats.model_load_count += 1;
        Ok(())
    }

    fn infer(&mut self, prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
        if !self.loaded {
            return Err(LlmError::ModelNotLoaded);
        }
        let output = format!("mock: {}", prompt);
        self.stats.inference_count += 1;
        self.stats.total_tokens_generated += output.len() as u64;
        Ok(output)
    }

    fn infer_stream(
        &mut self,
        _prompt: &str,
        _params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        if !self.loaded {
            return Err(LlmError::ModelNotLoaded);
        }
        let mut emitted: u64 = 0;
        for ch in self.mock_output.chars() {
            let token = String::from(ch);
            emitted += 1;
            if !callback(&token) {
                break;
            }
        }
        self.stats.inference_count += 1;
        self.stats.total_tokens_generated += emitted;
        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        self.model_info.as_ref()
    }

    fn health_check(&self) -> EngineHealth {
        EngineHealth {
            loaded: self.loaded,
            device: self.device,
            gpu_layers: self.device.n_gpu_layers(),
            last_error: None,
        }
    }

    fn stats(&self) -> &EngineStats {
        &self.stats
    }
}
