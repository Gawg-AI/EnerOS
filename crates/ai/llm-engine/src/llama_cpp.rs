//! llama.cpp 真实推理引擎后端（feature = "llama-cpp"，AI-001/002/003）.
//!
//! 通过 FFI 调用静态链接的 llama.cpp C 库执行真实 LLM 推理。
//! 这是 MASTER §G7 要求的"pinned source/static build + 真实最小推理系统测试"。
//!
//! # 资源管理
//!
//! - `LlamaCppEngine` 持有 model 指针和 context 指针（`*mut c_void`）
//! - `Drop` 释放 context → model（逆序）
//! - `llama_backend_init()` 通过 `AtomicBool` 保证只调用一次
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::*` + `core::*` + FFI。`elapsed_ns` 设为 0（no_std 无时钟）。

#![cfg(feature = "llama-cpp")]

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::device::ComputeDevice;
use crate::engine::LlmEngine;
use crate::error::LlmError;
use crate::ffi as lffi;
use crate::model::{ModelInfo, Quantization};
use crate::params::InferParams;
use crate::stats::{EngineHealth, EngineStats};

/// 全局后端初始化标志（AtomicBool 保证只调用一次 `llama_backend_init`）。
static BACKEND_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn ensure_backend_init() {
    if !BACKEND_INITIALIZED.swap(true, Ordering::SeqCst) {
        // SAFETY: llama_backend_init 是全局初始化函数，无前置条件。
        unsafe { lffi::llama_backend_init() };
    }
}

/// llama.cpp 真实推理引擎（静态链接 C 库）.
///
/// 不派生 `Clone`（唯一指针所有权）。
pub struct LlamaCppEngine {
    model: *mut c_void,
    ctx: *mut c_void,
    device: ComputeDevice,
    model_info: Option<ModelInfo>,
    stats: EngineStats,
}

impl LlamaCppEngine {
    /// 创建引擎实例（未加载模型）。
    pub fn new(device: ComputeDevice) -> Self {
        ensure_backend_init();
        Self {
            model: core::ptr::null_mut(),
            ctx: core::ptr::null_mut(),
            device,
            model_info: None,
            stats: EngineStats::default(),
        }
    }

    /// 获取 GPU offload 层数（基于设备类型，D4）。
    fn n_gpu_layers(&self) -> i32 {
        match self.device {
            ComputeDevice::Cpu => 0,
            ComputeDevice::Cuda | ComputeDevice::Metal | ComputeDevice::Npu => 99,
        }
    }
}

impl Drop for LlamaCppEngine {
    fn drop(&mut self) {
        // 逆序释放：先 ctx 后 model
        if !self.ctx.is_null() {
            // SAFETY: ctx 由 llama_new_context_with_model 返回，未在其他地方释放。
            unsafe { lffi::llama_free(self.ctx) };
            self.ctx = core::ptr::null_mut();
        }
        if !self.model.is_null() {
            // SAFETY: model 由 llama_load_model_from_file 返回，未在其他地方释放。
            unsafe { lffi::llama_free_model(self.model) };
            self.model = core::ptr::null_mut();
        }
    }
}

/// 从 logits 数组中选取概率最大的 token（贪心 argmax 采样）。
///
/// 不使用 `llama_sampler_sample`（b4000 CPU 后端有 abort 问题），
/// 直接读取 logits 做 argmax 是更稳定的方案。
///
/// # Safety
/// `logits` 必须指向至少 `vocab_size` 个有效 `f32`。调用方负责确保。
fn argmax(logits: *const f32, vocab_size: i32) -> lffi::LlamaToken {
    let mut max_logit = f32::MIN;
    let mut best: lffi::LlamaToken = 0;
    for i in 0..vocab_size {
        // SAFETY: caller guarantees logits points to vocab_size f32s.
        let logit = unsafe { *logits.add(i as usize) };
        if logit > max_logit {
            max_logit = logit;
            best = i;
        }
    }
    best
}

impl LlmEngine for LlamaCppEngine {
    fn load_model(&mut self, path: &str) -> Result<(), LlmError> {
        if path.is_empty() {
            return Err(LlmError::InvalidPath);
        }

        // 释放旧模型（如果有）
        if !self.ctx.is_null() {
            unsafe { lffi::llama_free(self.ctx) };
            self.ctx = core::ptr::null_mut();
        }
        if !self.model.is_null() {
            unsafe { lffi::llama_free_model(self.model) };
            self.model = core::ptr::null_mut();
        }

        // 转换路径为 C 字符串
        let c_path = CString::new(path).map_err(|_| LlmError::InvalidPath)?;

        // 模型参数
        // SAFETY: llama_model_default_params 返回正确初始化的结构体。
        let mut model_params = unsafe { lffi::llama_model_default_params() };
        model_params.n_gpu_layers = self.n_gpu_layers();
        model_params.use_mmap = true;
        model_params.use_mlock = false;

        // 加载模型
        // SAFETY: c_path 是有效的 C 字符串，model_params 由 default 函数初始化。
        // CString::as_ptr() 返回 *const c_char (i8 on Linux)，ffi 声明 *const u8，需 cast。
        let model =
            unsafe { lffi::llama_load_model_from_file(c_path.as_ptr() as *const u8, model_params) };
        if model.is_null() {
            return Err(LlmError::LoadFailed);
        }
        self.model = model;

        // 上下文参数
        // SAFETY: llama_context_default_params 返回正确初始化的结构体。
        let mut ctx_params = unsafe { lffi::llama_context_default_params() };
        ctx_params.n_ctx = 2048;
        ctx_params.n_batch = 512;
        ctx_params.n_threads = 4;
        ctx_params.n_threads_batch = 4;
        ctx_params.no_perf = true;
        // logits_all 已废弃（b4000）：改用 llama_batch_get_one + logits=NULL，
        // llama_decode 仅返回最后一个 token 的 logits（通过 llama_get_logits_ith 取）。
        ctx_params.logits_all = false;

        // 创建上下文
        // SAFETY: model 非 null（已检查），ctx_params 由 default 函数初始化。
        let ctx = unsafe { lffi::llama_new_context_with_model(model, ctx_params) };
        if ctx.is_null() {
            unsafe { lffi::llama_free_model(model) };
            self.model = core::ptr::null_mut();
            return Err(LlmError::LoadFailed);
        }
        self.ctx = ctx;

        // 收集模型信息
        // SAFETY: model 非 null。
        let model_size = unsafe { lffi::llama_model_size(model) };
        // SAFETY: model 非 null。
        let n_ctx_train = unsafe { lffi::llama_n_ctx_train(model) };

        self.model_info = Some(ModelInfo {
            name: String::from(path),
            size_bytes: model_size,
            quantization: Quantization::Q4_K_M, // 从模型推断较复杂，用默认
            context_length: n_ctx_train as u32,
            device: self.device,
        });

        self.stats.model_load_count += 1;
        self.stats.gpu_layers = self.n_gpu_layers() as u32;

        Ok(())
    }

    fn infer(&mut self, prompt: &str, params: &InferParams) -> Result<String, LlmError> {
        if self.model.is_null() || self.ctx.is_null() {
            return Err(LlmError::ModelNotLoaded);
        }

        let mut output = String::new();
        self.infer_stream(prompt, params, &mut |token: &str| {
            output.push_str(token);
            true
        })?;

        self.stats.inference_count += 1;
        // 统计 token 数（近似：按字符数，非精确）
        self.stats.total_tokens_generated += output.chars().count() as u64;

        Ok(output)
    }

    fn infer_stream(
        &mut self,
        prompt: &str,
        params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        if self.model.is_null() || self.ctx.is_null() {
            return Err(LlmError::ModelNotLoaded);
        }

        // 1. 分词
        // b4000: llama_tokenize 返回负数表示所需大小（取绝对值），正数表示成功写入
        let prompt_bytes = prompt.as_bytes();
        let needed_raw = unsafe {
            lffi::llama_tokenize(
                self.model,
                prompt_bytes.as_ptr(),
                prompt_bytes.len() as i32,
                core::ptr::null_mut(),
                0,
                false,
                false,
            )
        };
        let needed = if needed_raw < 0 {
            (-needed_raw) as usize
        } else {
            needed_raw as usize
        };
        if needed == 0 {
            return Err(LlmError::InferFailed);
        }

        let mut tokens: Vec<lffi::LlamaToken> = vec![0; needed];
        let n_tokens_raw = unsafe {
            lffi::llama_tokenize(
                self.model,
                prompt_bytes.as_ptr(),
                prompt_bytes.len() as i32,
                tokens.as_mut_ptr(),
                needed as i32,
                false,
                false,
            )
        };
        let n_tokens = if n_tokens_raw < 0 {
            (-n_tokens_raw) as usize
        } else {
            n_tokens_raw as usize
        };
        tokens.truncate(n_tokens);

        // 2. 解码 prompt（逐 chunk，每次最多 n_batch=512 tokens）
        //
        // 使用 llama_batch_get_one（对齐 official simple 示例的 canonical pattern）：
        //   - pos=NULL    → llama_decode 自动追踪位置
        //   - seq_id=NULL → 序列 ID 默认为 0
        //   - logits=NULL → 仅返回最后一个 token 的 logits
        //   - 无堆分配，无需 llama_batch_free
        //
        // 不使用 llama_batch_init：文档明确 "All members are left uninitialized"，
        // 手动设置 logits[]/seq_id[][] 易留未初始化内存导致 decode 失败。
        let mut last_batch_n: i32 = 0;
        for chunk in tokens.chunks_mut(512) {
            let chunk_len = chunk.len() as i32;
            // SAFETY: chunk.as_mut_ptr() 指向有效 token 数组，chunk_len > 0。
            // llama_batch_get_one 仅包装指针（无堆分配，无需 free）。
            let batch = unsafe { lffi::llama_batch_get_one(chunk.as_mut_ptr(), chunk_len) };
            // SAFETY: ctx 非 null，batch 包装有效 token 数组。
            let ret = unsafe { lffi::llama_decode(self.ctx, batch) };
            // b4000 语义: 0=成功, >0=KV 槽满(警告), <0=错误
            if ret < 0 {
                return Err(LlmError::InferFailed);
            }
            if ret > 0 {
                break; // KV 槽满——上下文已耗尽
            }
            last_batch_n = chunk_len;
        }

        // 3. 生成循环：贪心 argmax 采样
        // SAFETY: model 非 null。
        let vocab_size = unsafe { lffi::llama_n_vocab(self.model) };
        if vocab_size <= 0 || last_batch_n == 0 {
            return Ok(());
        }

        // 取 prompt 最后一个 token 的 logits → 采样第一个生成 token
        // SAFETY: ctx 非 null，last_batch_n-1 是上次 decode 最后 token 的索引。
        let logits_ptr = unsafe { lffi::llama_get_logits_ith(self.ctx, last_batch_n - 1) };
        if logits_ptr.is_null() {
            return Ok(());
        }
        let mut next_token = argmax(logits_ptr, vocab_size);

        let mut generated = 0u32;
        while generated < params.max_tokens {
            // EOG 检查
            // SAFETY: model 非 null。
            if unsafe { lffi::llama_token_is_eog(self.model, next_token) } {
                break;
            }

            // token → 文本（b4000 需 6 参数，含 special: bool）
            let mut buf = vec![0u8; 64];
            // SAFETY: model 非 null，buf 已分配。
            let n = unsafe {
                lffi::llama_token_to_piece(
                    self.model,
                    next_token,
                    buf.as_mut_ptr(),
                    buf.len() as i32,
                    0,
                    false,
                )
            };
            if n > 0 {
                let text =
                    core::str::from_utf8(&buf[..n as usize]).map_err(|_| LlmError::Utf8Error)?;
                if !callback(text) {
                    break;
                }
            }

            generated += 1;

            // 解码新 token（batch_get_one，batch size=1）
            // SAFETY: &mut next_token 指向有效 token，get_one 仅包装指针（不写）。
            let batch = unsafe { lffi::llama_batch_get_one(&mut next_token, 1) };
            // SAFETY: ctx 非 null，batch 包装有效 token。
            let ret = unsafe { lffi::llama_decode(self.ctx, batch) };
            if ret != 0 {
                break;
            }

            // 取新 token 的 logits → 采样下一个
            // SAFETY: ctx 非 null，上次 decode batch.n_tokens=1，最后 token 索引=0。
            let logits_ptr = unsafe { lffi::llama_get_logits_ith(self.ctx, 0) };
            if logits_ptr.is_null() {
                break;
            }
            next_token = argmax(logits_ptr, vocab_size);
        }

        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        self.model_info.as_ref()
    }

    fn health_check(&self) -> EngineHealth {
        EngineHealth {
            loaded: !self.model.is_null() && !self.ctx.is_null(),
            device: self.device,
            gpu_layers: self.n_gpu_layers() as u32,
            last_error: None,
        }
    }

    fn stats(&self) -> &EngineStats {
        &self.stats
    }
}

// ============================================================================
// Tests — 真实 llama.cpp 推理测试（AI-003 真实最小推理系统测试）
// ============================================================================

#[cfg(all(test, feature = "llama-cpp"))]
mod tests {
    use std::env;
    use std::path::Path;

    use super::*;
    use crate::engine::LlmEngine;
    use crate::params::InferParams;

    /// 获取测试模型路径。
    ///
    /// 优先级：`LLAMA_TEST_MODEL` 环境变量 > `third_party/test-model.gguf`
    fn test_model_path() -> Option<String> {
        if let Ok(p) = env::var("LLAMA_TEST_MODEL") {
            if Path::new(&p).exists() {
                return Some(p);
            }
        }
        // 检查 third_party 下的测试模型
        let candidates = [
            "../../../third_party/test-model.gguf",
            "../../../third_party/llama.cpp/test-model.gguf",
        ];
        for c in &candidates {
            if Path::new(c).exists() {
                return Some(c.to_string());
            }
        }
        None
    }

    /// T1：真实模型加载 + 基本推理。
    ///
    /// 此测试需要真实 GGUF 模型文件。设置 `LLAMA_TEST_MODEL` 环境变量指向模型路径。
    /// 若无模型文件，测试 FAIL（不 skip）—— 这是 MASTER "非 Mock" 要求。
    #[test]
    fn t1_real_model_load_and_infer() {
        let model_path = test_model_path().expect(
            "No test model found. Set LLAMA_TEST_MODEL env var to a .gguf file path. \
             Download a small model, e.g.: \
             wget -O third_party/test-model.gguf https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf"
        );

        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        engine
            .load_model(&model_path)
            .expect("model load should succeed");
        assert!(engine.health_check().loaded);

        let params = InferParams {
            max_tokens: 16,
            temperature: 0.1,
            top_p: 0.9,
            top_k: 40,
            repeat_penalty: 1.1,
            stop_tokens: vec![],
        };

        let output = engine
            .infer("Hello", &params)
            .expect("inference should succeed");
        assert!(
            !output.is_empty(),
            "inference should produce non-empty output"
        );

        // 统计应更新
        assert!(engine.stats().inference_count >= 1);
        assert!(engine.stats().model_load_count >= 1);
    }

    /// T2：模型信息查询。
    #[test]
    fn t2_model_info() {
        let model_path = match test_model_path() {
            Some(p) => p,
            None => {
                eprintln!("No test model — skipping model_info test (not a failure)");
                return;
            }
        };

        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        engine.load_model(&model_path).expect("load");

        let info = engine.model_info().expect("model_info should be Some");
        assert!(!info.name.is_empty());
        assert!(info.size_bytes > 0);
        assert!(info.context_length > 0);
    }

    /// T3：流式推理 callback。
    #[test]
    fn t3_stream_inference() {
        let model_path = match test_model_path() {
            Some(p) => p,
            None => {
                eprintln!("No test model — skipping stream test (not a failure)");
                return;
            }
        };

        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        engine.load_model(&model_path).expect("load");

        let params = InferParams {
            max_tokens: 8,
            temperature: 0.1,
            ..Default::default()
        };

        let mut received = String::new();
        engine
            .infer_stream("Hello", &params, &mut |token: &str| {
                received.push_str(token);
                true
            })
            .expect("stream inference should succeed");

        // 应收到至少一些文本
        assert!(!received.is_empty(), "stream should produce tokens");
    }

    /// T4：未加载模型时推理返回错误。
    #[test]
    fn t4_infer_without_model() {
        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        let params = InferParams::default();
        let r = engine.infer("hello", &params);
        assert!(matches!(r, Err(LlmError::ModelNotLoaded)));
    }

    /// T5：空路径拒绝。
    #[test]
    fn t5_empty_path_rejected() {
        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        let r = engine.load_model("");
        assert!(matches!(r, Err(LlmError::InvalidPath)));
    }

    /// T6：不存在的模型文件返回 LoadFailed。
    #[test]
    fn t6_nonexistent_model() {
        let mut engine = LlamaCppEngine::new(ComputeDevice::Cpu);
        let r = engine.load_model("/nonexistent/model.gguf");
        assert!(matches!(r, Err(LlmError::LoadFailed)));
    }
}
