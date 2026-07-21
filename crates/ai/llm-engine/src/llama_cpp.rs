//! LlamaCppEngine — llama.cpp C 库的 Rust 封装（D3 / D10：feature-gated）.
//!
//! 仅当启用 `llama-cpp` feature 且链接 llama.cpp C 库时编译。
//! 所有 `unsafe` 块附 SAFETY 注释说明不变量（D10）。

#![cfg(feature = "llama-cpp")]

use alloc::ffi::CString;
use alloc::string::String;
use core::ffi::{c_int, c_void};

use crate::device::ComputeDevice;
use crate::engine::LlmEngine;
use crate::error::LlmError;
use crate::ffi;
use crate::model::ModelInfo;
use crate::params::InferParams;
use crate::stats::{EngineHealth, EngineStats};

/// llama.cpp 推理引擎实现.
///
/// 通过 FFI 调用 llama.cpp C 库执行真实推理。`ctx` 指针所有权由本结构体持有，
/// `Drop` 时调用 `ffi::llama_free` 释放（D10）。
pub struct LlamaCppEngine {
    /// C 库推理上下文（由 `llama_init` 返回，`Drop` 时释放）.
    ctx: *mut c_void,
    /// 当前模型元数据.
    model_info: Option<ModelInfo>,
    /// 目标计算设备.
    device: ComputeDevice,
    /// 累计统计.
    stats: EngineStats,
}

/// ComputeDevice 到 llama.cpp 设备 ID 的映射.
fn device_id(device: ComputeDevice) -> c_int {
    match device {
        ComputeDevice::Cpu => 0,
        ComputeDevice::Cuda => 1,
        ComputeDevice::Metal => 2,
        ComputeDevice::Npu => 3,
    }
}

impl LlamaCppEngine {
    /// 创建 LlamaCppEngine.
    ///
    /// 调用 `ffi::llama_init()` 获取 C 上下文指针，并记录 GPU offload 层数。
    pub fn new(device: ComputeDevice) -> Self {
        // SAFETY: `llama_init` 是无副作用的 C 函数，仅分配内部上下文。
        // 返回的指针所有权转移到 `LlamaCppEngine`，由 `Drop` 释放。
        let ctx = unsafe { ffi::llama_init() };
        let gpu_layers = device.n_gpu_layers();
        let mut stats = EngineStats::default();
        stats.gpu_layers = gpu_layers;
        Self {
            ctx,
            model_info: None,
            device,
            stats,
        }
    }
}

impl LlmEngine for LlamaCppEngine {
    fn load_model(&mut self, path: &str) -> Result<(), LlmError> {
        if path.is_empty() {
            return Err(LlmError::InvalidPath);
        }
        let c_path = CString::new(path).map_err(|_| LlmError::InvalidPath)?;

        // SAFETY: `self.ctx` 由 `llama_init` 返回且未被释放（Drop 之前一直有效）。
        // `c_path` 是合法的 NUL 结尾 C 字符串，调用期间不会被释放。
        let load_result = unsafe { ffi::llama_load_model(self.ctx, c_path.as_ptr()) };
        if load_result != 0 {
            return Err(LlmError::LoadFailed);
        }

        // SAFETY: `self.ctx` 已成功加载模型（load_result == 0），上下文有效。
        // `device_id` 返回合法 i32，与 llama.cpp 内部设备枚举一致。
        let dev_result = unsafe { ffi::llama_set_device(self.ctx, device_id(self.device)) };
        if dev_result != 0 && self.device.is_gpu() {
            return Err(LlmError::GpuUnavailable);
        }

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

    fn infer(&mut self, prompt: &str, params: &InferParams) -> Result<String, LlmError> {
        let c_prompt = CString::new(prompt).map_err(|_| LlmError::InvalidPrompt)?;

        // SAFETY: `self.ctx` 由 `llama_init` 返回且未被释放。
        // `c_prompt` 是合法的 NUL 结尾 C 字符串，调用期间不会被释放。
        // 返回的指针所有权转移到 Rust 侧，下方立即拷贝并调用 `llama_free_result` 释放。
        let result_ptr = unsafe {
            ffi::llama_infer(
                self.ctx,
                c_prompt.as_ptr(),
                params.max_tokens,
                params.temperature,
            )
        };
        if result_ptr.is_null() {
            return Err(LlmError::InferFailed);
        }

        // SAFETY: `result_ptr` 由 `llama_infer` 返回，指向合法的 NUL 结尾 C 字符串。
        // 本块立即拷贝为 Rust `String`，随后释放，不会出现 use-after-free。
        let output = unsafe {
            let cstr = core::ffi::CStr::from_ptr(result_ptr);
            let s = cstr.to_str().map_err(|_| LlmError::Utf8Error)?;
            String::from(s)
        };

        // SAFETY: `result_ptr` 由 `llama_infer` 返回且尚未释放，调用后所有权交还 C 库。
        unsafe { ffi::llama_free_result(result_ptr) };

        self.stats.inference_count += 1;
        self.stats.total_tokens_generated += output.len() as u64;
        Ok(output)
    }

    fn infer_stream(
        &mut self,
        prompt: &str,
        params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        // 简化实现：单次推理后按字符切分回调。真实流式需 C 库支持 token 级回调。
        let output = self.infer(prompt, params)?;
        for ch in output.chars() {
            let token = String::from(ch);
            if !callback(&token) {
                break;
            }
        }
        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        self.model_info.as_ref()
    }

    fn health_check(&self) -> EngineHealth {
        EngineHealth {
            loaded: self.model_info.is_some(),
            device: self.device,
            gpu_layers: self.stats.gpu_layers,
            last_error: None,
        }
    }

    fn stats(&self) -> &EngineStats {
        &self.stats
    }
}

impl Drop for LlamaCppEngine {
    fn drop(&mut self) {
        // SAFETY: `self.ctx` 由 `llama_init` 返回且仅在本结构体 Drop 时释放一次。
        // 释放后不再访问 `self.ctx`，避免 double-free。
        unsafe { ffi::llama_free(self.ctx) };
    }
}
