//! EnerOS LLM 推理引擎选型与 FFI 封装（v0.59.0，P1-I 起点）.
//!
//! 双脑架构（LLM + Solver）的 LLM 是"感知者"，负责理解市场信号和自然语言指令
//! 输出 JSON 意图。本 crate 定义统一的 [`engine::LlmEngine`] trait、llama.cpp
//! FFI 绑定（feature-gated）与 [`mock::MockEngine`]（默认可用），为后续
//! v0.60.0~v0.63.0 LLM 模型加载/量化/调度/模板奠定接口基础。
//!
//! # 核心类型
//!
//! - [`engine::LlmEngine`] — 推理引擎统一 trait（D2 无 Send + Sync bound）
//! - [`mock::MockEngine`] — 默认可用的 Mock 实现（D3，纯 Rust）
//! - [`llama_cpp::LlamaCppEngine`] — llama.cpp C 库实现（feature = "llama-cpp"，D3）
//! - [`device::ComputeDevice`] — 计算设备（Cpu / Cuda / Metal / Npu，D4 / D12）
//! - [`model::ModelInfo`] / [`model::Quantization`] — 模型元数据（D11）
//! - [`params::InferParams`] — 推理参数
//! - [`stats::EngineStats`] / [`stats::EngineHealth`] — 统计与健康检查（D5）
//! - [`error::LlmError`] — 错误类型（D7）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | no_std 合规：`alloc::string::String` / `alloc::vec::Vec` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明 |
//! | **D2** | `LlmEngine` trait **不要求** `Send + Sync`（no_std 单线程；`*mut c_void` 非 `Send`） |
//! | **D3** | `MockEngine` 默认可用；`LlamaCppEngine` + `ffi` 模块通过 `#[cfg(feature = "llama-cpp")]` 门控；`Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭） |
//! | **D4** | GPU 优先通过 llama.cpp `n_gpu_layers` 参数（Cpu=0，Cuda/Metal/Npu=99 全 offload），非 PyTorch |
//! | **D5** | `EngineStats` 用普通 `u64`/`u32`，不使用 `AtomicU64`（单线程无需） |
//! | **D6** | `load_model(path: &str)` 保留 `&str` 签名（no_std 兼容，`core::str` 可用）；`LlamaCppEngine` 内部用 `alloc::ffi::CString` 转换 |
//! | **D7** | `LlmError` 8 变体（LoadFailed / InferFailed / InvalidPath / InvalidPrompt / Utf8Error / GpuUnavailable / ModelNotLoaded / OutOfMemory）；派生 `Debug`，实现 `core::fmt::Display` |
//! | **D8** | `infer_stream` callback 使用 `&mut dyn FnMut(&str) -> bool`（trait object 引用，非 `Box<dyn>`，no_std 兼容） |
//! | **D9** | crate 位置 `crates/ai/llm-engine/`（AI 子系统；项目规则 §2.3.1） |
//! | **D10** | FFI 集中封装于 `ffi` 模块；每个 `unsafe` 块附 SAFETY 注释；指针所有权明确（`llama_init` 返回值由 `LlamaCppEngine` 持有，`Drop` 调用 `llama_free`；`llama_infer` 返回值立即拷贝并调用 `llama_free_result`） |
//! | **D11** | `Quantization` 派生 `Default`，`#[default]` 标注 `Q4_K_M`（nightly feature） |
//! | **D12** | `ComputeDevice` 派生 `Default`，`#[default]` 标注 `Cpu`（GPU 优先是 opt-in） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，无外部依赖，可交叉编译到 `aarch64-unknown-none`。
//! 默认 feature 下不引入任何 `std::*`，不调用 `panic!` / `todo!` / `unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod device;
pub mod engine;
pub mod error;
pub mod mock;
pub mod model;
pub mod params;
pub mod stats;

#[cfg(feature = "llama-cpp")]
pub mod ffi;

#[cfg(feature = "llama-cpp")]
pub mod llama_cpp;

pub use device::ComputeDevice;
pub use engine::LlmEngine;
pub use error::LlmError;
pub use mock::MockEngine;
pub use model::{ModelInfo, Quantization};
pub use params::InferParams;
pub use stats::{EngineHealth, EngineStats};

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T15（覆盖 D1~D12 偏差声明与 checklist C64~C78）.
    //!
    //! 全部使用 `MockEngine`（`LlamaCppEngine` 受 feature-gated，需 C 库链接）。

    use super::*;
    use crate::device::ComputeDevice;
    use crate::error::LlmError;
    use crate::mock::MockEngine;
    use crate::model::{ModelInfo, Quantization};
    use crate::params::InferParams;
    use crate::stats::EngineStats;

    // ===== T1：ComputeDevice is_gpu（Cpu=false，Cuda/Metal/Npu=true）=====
    #[test]
    fn test_t1_compute_device_is_gpu() {
        assert!(!ComputeDevice::Cpu.is_gpu());
        assert!(ComputeDevice::Cuda.is_gpu());
        assert!(ComputeDevice::Metal.is_gpu());
        assert!(ComputeDevice::Npu.is_gpu());
    }

    // ===== T2：ComputeDevice n_gpu_layers（Cpu=0，其余=99，D4）=====
    #[test]
    fn test_t2_compute_device_n_gpu_layers() {
        assert_eq!(ComputeDevice::Cpu.n_gpu_layers(), 0);
        assert_eq!(ComputeDevice::Cuda.n_gpu_layers(), 99);
        assert_eq!(ComputeDevice::Metal.n_gpu_layers(), 99);
        assert_eq!(ComputeDevice::Npu.n_gpu_layers(), 99);
    }

    // ===== T3：ComputeDevice Default（Cpu，D12）=====
    #[test]
    fn test_t3_compute_device_default_is_cpu() {
        let d = ComputeDevice::default();
        assert_eq!(d, ComputeDevice::Cpu);
        assert!(!d.is_gpu());
    }

    // ===== T4：Quantization Default（Q4_K_M，D11）=====
    #[test]
    fn test_t4_quantization_default_is_q4_k_m() {
        let q = Quantization::default();
        assert_eq!(q, Quantization::Q4_K_M);
    }

    // ===== T5：ModelInfo Default（name=空，quantization=Q4_K_M，context=2048，device=Cpu）=====
    #[test]
    fn test_t5_model_info_default() {
        let m = ModelInfo::default();
        assert!(m.name.is_empty());
        assert_eq!(m.size_bytes, 0);
        assert_eq!(m.quantization, Quantization::Q4_K_M);
        assert_eq!(m.context_length, 2048);
        assert_eq!(m.device, ComputeDevice::Cpu);
    }

    // ===== T6：InferParams Default（max_tokens=128, temperature=0.7, top_p=0.9, top_k=40, repeat_penalty=1.1, stop_tokens=空）=====
    #[test]
    fn test_t6_infer_params_default() {
        let p = InferParams::default();
        assert_eq!(p.max_tokens, 128);
        assert_eq!(p.temperature, 0.7f32);
        assert_eq!(p.top_p, 0.9f32);
        assert_eq!(p.top_k, 40);
        assert_eq!(p.repeat_penalty, 1.1f32);
        assert!(p.stop_tokens.is_empty());
    }

    // ===== T7：EngineStats Default（全 0，D5）=====
    #[test]
    fn test_t7_engine_stats_default() {
        let s = EngineStats::default();
        assert_eq!(s.inference_count, 0);
        assert_eq!(s.total_tokens_generated, 0);
        assert_eq!(s.total_inference_ns, 0);
        assert_eq!(s.last_inference_ns, 0);
        assert_eq!(s.model_load_count, 0);
        assert_eq!(s.gpu_layers, 0);
    }

    // ===== T8：MockEngine 构造 + 加载 + 推理 =====
    #[test]
    fn test_t8_mock_engine_load_and_infer() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        assert!(!engine.health_check().loaded);

        // 加载模型
        engine.load_model("/models/test.gguf").unwrap();
        assert!(engine.health_check().loaded);

        // model_info 更新
        let info = engine.model_info().expect("model_info should be Some");
        assert_eq!(info.name, "/models/test.gguf");
        assert_eq!(info.device, ComputeDevice::Cpu);

        // 推理
        let params = InferParams::default();
        let output = engine.infer("hello", &params).unwrap();
        assert_eq!(output, "mock: hello");
    }

    // ===== T9：MockEngine 未加载推理返回 Err(ModelNotLoaded) =====
    #[test]
    fn test_t9_mock_engine_infer_not_loaded() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        let params = InferParams::default();
        let r = engine.infer("hello", &params);
        assert!(matches!(r, Err(LlmError::ModelNotLoaded)));
    }

    // ===== T10：MockEngine 流式推理（callback 累计字符）=====
    #[test]
    fn test_t10_mock_engine_infer_stream_accumulate() {
        let mut engine = MockEngine::with_output("abc");
        engine.load_model("/m.gguf").unwrap();

        let mut received = String::new();
        let params = InferParams::default();
        engine
            .infer_stream("ignored", &params, &mut |token: &str| {
                received.push_str(token);
                true
            })
            .unwrap();

        assert_eq!(received, "abc");
        // 统计更新：3 个 token
        assert_eq!(engine.stats().total_tokens_generated, 3);
        assert_eq!(engine.stats().inference_count, 1);
    }

    // ===== T11：MockEngine 流式 callback 返回 false 停止 =====
    #[test]
    fn test_t11_mock_engine_infer_stream_stop_early() {
        let mut engine = MockEngine::with_output("hello");
        engine.load_model("/m.gguf").unwrap();

        let mut count = 0u32;
        let params = InferParams::default();
        engine
            .infer_stream("ignored", &params, &mut |_token: &str| {
                count += 1;
                // 第 2 个 token 后停止
                count < 2
            })
            .unwrap();

        // 'h' (count=1, true) → 'e' (count=2, false) → break，共 2 个字符发出
        assert_eq!(count, 2);
        // 仅 2 个 token 计入统计
        assert_eq!(engine.stats().total_tokens_generated, 2);
    }

    // ===== T12：MockEngine 统计累加（inference_count / total_tokens_generated）=====
    #[test]
    fn test_t12_mock_engine_stats_accumulate() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/m.gguf").unwrap();
        assert_eq!(engine.stats().model_load_count, 1);

        let params = InferParams::default();
        // 第一次推理："mock: a" 长度 7
        let _ = engine.infer("a", &params).unwrap();
        assert_eq!(engine.stats().inference_count, 1);
        assert_eq!(engine.stats().total_tokens_generated, 7);

        // 第二次推理："mock: bb" 长度 8
        let _ = engine.infer("bb", &params).unwrap();
        assert_eq!(engine.stats().inference_count, 2);
        assert_eq!(engine.stats().total_tokens_generated, 15);

        // 第三次推理："mock: ccc" 长度 9
        let _ = engine.infer("ccc", &params).unwrap();
        assert_eq!(engine.stats().inference_count, 3);
        assert_eq!(engine.stats().total_tokens_generated, 24);
    }

    // ===== T13：MockEngine health_check（loaded=true, device=Cpu）=====
    #[test]
    fn test_t13_mock_engine_health_check() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        // 未加载
        let h = engine.health_check();
        assert!(!h.loaded);
        assert_eq!(h.device, ComputeDevice::Cpu);
        assert_eq!(h.gpu_layers, 0);
        assert!(h.last_error.is_none());

        // 加载后
        engine.load_model("/m.gguf").unwrap();
        let h = engine.health_check();
        assert!(h.loaded);
        assert_eq!(h.device, ComputeDevice::Cpu);
        assert_eq!(h.gpu_layers, 0);
    }

    // ===== T14：GPU 优先逻辑（MockEngine::new(Cuda) 时 device=Cuda，is_gpu=true）=====
    #[test]
    fn test_t14_gpu_priority_cuda() {
        let mut engine = MockEngine::new(ComputeDevice::Cuda);
        assert_eq!(engine.device(), ComputeDevice::Cuda);
        assert!(engine.device().is_gpu());
        assert_eq!(engine.device().n_gpu_layers(), 99);

        // health_check 也应反映 Cuda 设备
        let h = engine.health_check();
        assert_eq!(h.device, ComputeDevice::Cuda);
        assert_eq!(h.gpu_layers, 99);

        // 加载模型后，model_info.device 应为 Cuda
        engine.load_model("/m.gguf").unwrap();
        let info = engine.model_info().expect("model_info");
        assert_eq!(info.device, ComputeDevice::Cuda);
    }

    // ===== T15：CPU 降级逻辑（MockEngine::new(Cpu) 时 device=Cpu，is_gpu=false）=====
    #[test]
    fn test_t15_cpu_fallback() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        assert_eq!(engine.device(), ComputeDevice::Cpu);
        assert!(!engine.device().is_gpu());
        assert_eq!(engine.device().n_gpu_layers(), 0);

        // health_check 反映 CPU
        let h = engine.health_check();
        assert_eq!(h.device, ComputeDevice::Cpu);
        assert_eq!(h.gpu_layers, 0);

        // 加载模型后，model_info.device 应为 Cpu
        engine.load_model("/m.gguf").unwrap();
        let info = engine.model_info().expect("model_info");
        assert_eq!(info.device, ComputeDevice::Cpu);
    }
}
