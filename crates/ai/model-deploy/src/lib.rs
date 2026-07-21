//! EnerOS 7B INT4 量化模型部署验证（v0.61.0）.
//!
//! 本 crate 验证 7B Q4_K_M 量化模型在目标硬件上的可部署性：硬件检查 → 模型加载
//! → 多能源场景提示词推理 → 生成部署报告。双脑架构（LLM + Solver）的 LLM 是
//! "感知者"，本 crate 确保 LLM 在边缘硬件（CPU 或 GPU）上能正确加载与推理。
//!
//! # 核心类型
//!
//! - [`verifier::DeployVerifier<E>`] — 部署验证器（D2 泛型 `E: LlmEngine`）
//! - [`config::QuantConfig7B`] — 7B INT4 量化部署配置（D11 复用 v0.59.0 类型）
//! - [`backend::DeployBackend`] — 部署后端 trait（D2 抽象）
//! - [`mock_backend::MockDeployBackend`] — 默认 Mock 后端（D12）
//! - [`llama_backend::LlamaDeployBackend`] — llama.cpp 后端（D3 feature-gated）
//! - [`prompts::PowerPromptSet`] — 5 类能源场景提示词
//! - [`report::DeployReport`] — 部署报告（D5 普通 u64）
//! - [`error::DeployError`] — 部署错误（D7 7 变体 + From<LlmError>）
//!
//! # 依赖关系（D11）
//!
//! 复用 v0.59.0 `eneros-llm-engine` 与 v0.60.0 `eneros-gguf-loader` 的类型，
//! 不重定义：
//! - `eneros_llm_engine::LlmEngine`（推理引擎 trait）
//! - `eneros_llm_engine::MockEngine`（默认 Mock 引擎）
//! - `eneros_llm_engine::ComputeDevice`（Cpu / Cuda / Metal / Npu）
//! - `eneros_llm_engine::Quantization`（F16 / Q8_0 / Q4_0 / Q4_K_M）
//! - `eneros_llm_engine::InferParams`（推理参数）
//! - `eneros_llm_engine::LlmError`（引擎错误，转换为 `DeployError`）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | no_std 合规：`alloc::string::String` / `alloc::vec::Vec` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明 |
//! | **D2** | `DeployVerifier<E: LlmEngine>` 泛型 over `LlmEngine`（不绑定 `LlamaCppEngine`） |
//! | **D3** | `MockDeployBackend` 默认可用；`LlamaDeployBackend` 通过 `#[cfg(feature = "llama-cpp")]` 门控；`Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭） |
//! | **D4** | GPU 优先通过 `n_gpu_layers` 跟踪（复用 v0.59.0 `ComputeDevice`），非 PyTorch |
//! | **D5** | `DeployReport` 用普通 `u64`/`u32`/`f64`，不使用 `AtomicU64`（单线程无需） |
//! | **D6** | `load_model` 保留 `&str` 路径签名（no_std 兼容） |
//! | **D7** | `DeployError` 7 变体（HardwareInsufficient / ModelLoadFailed / InferenceFailed / InvalidResult / Timeout / BackendError / NotDeployed）；派生 `Debug`/`Clone`，实现 `core::fmt::Display` + `From<LlmError>` |
//! | **D8** | `DeployVerifier` **不**实现 `Drop`；部署/卸载由 `deploy` / `undeploy` 显式调用 |
//! | **D9** | crate 位置 `crates/ai/model-deploy/`（AI 子系统；项目规则 §2.3.1） |
//! | **D10** | 不声明 FFI；通过 `LlmEngine` trait 间接调用 llama.cpp（FFI 由 v0.59.0 封装） |
//! | **D11** | 复用 v0.59.0 + v0.60.0 类型（`Quantization` / `InferParams` / `ComputeDevice` / `LlmEngine` / `LlmError`） |
//! | **D12** | `MockDeployBackend` 作为默认后端（`DeployVerifier::new` 默认注入） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，可交叉编译到 `aarch64-unknown-none`。
//! 默认 feature 下不引入任何 `std::*`，不调用 `panic!` / `todo!` / `unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod backend;
pub mod config;
pub mod error;
pub mod mock_backend;
pub mod prompts;
pub mod report;
pub mod verifier;

#[cfg(feature = "llama-cpp")]
pub mod llama_backend;

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T15（覆盖 D1~D12 偏差声明）.
    //!
    //! 全部使用 `MockEngine`（`LlamaDeployBackend` 受 feature-gated，需 C 库链接）。

    use alloc::string::String;
    use alloc::vec;

    use eneros_llm_engine::{ComputeDevice, LlmEngine, MockEngine, Quantization};

    use super::*;
    use crate::backend::DeployBackend;
    use crate::backend::HardwareCheck;
    use crate::config::QuantConfig7B;
    use crate::error::DeployError;
    use crate::mock_backend::MockDeployBackend;
    use crate::prompts::{PowerPrompt, PowerPromptSet};
    use crate::report::DeployReport;
    use crate::verifier::DeployVerifier;

    // ===== T1：QuantConfig7B Default（model_name=Qwen2.5-7B, Q4_K_M, temperature=0.1, context=4096, Cpu）=====
    #[test]
    fn test_t1_quant_config_default() {
        let c = QuantConfig7B::default();
        assert_eq!(c.model_name, "Qwen2.5-7B");
        assert_eq!(c.quantization, Quantization::Q4_K_M);
        assert_eq!(c.infer_params.temperature, 0.1f32);
        assert_eq!(c.context_length, 4096);
        assert_eq!(c.device, ComputeDevice::Cpu);
        assert_eq!(c.file_size_gb, 4.0);
        assert_eq!(c.min_ram_gb, 8.0);
        assert_eq!(c.min_vram_gb, 6.0);
        assert_eq!(c.infer_params.max_tokens, 512);
    }

    // ===== T2：QuantConfig7B::new("Llama2-7B") 保留自定义名称，其余字段默认 =====
    #[test]
    fn test_t2_quant_config_new_custom_name() {
        let c = QuantConfig7B::new("Llama2-7B");
        assert_eq!(c.model_name, "Llama2-7B");
        // 其余字段保持默认
        assert_eq!(c.quantization, Quantization::Q4_K_M);
        assert_eq!(c.context_length, 4096);
        assert_eq!(c.device, ComputeDevice::Cpu);
        assert_eq!(c.infer_params.temperature, 0.1f32);
    }

    // ===== T3：QuantConfig7B::with_device(Cuda) builder（device=Cuda, is_gpu=true, n_gpu_layers=99）=====
    #[test]
    fn test_t3_quant_config_with_device_cuda() {
        let c = QuantConfig7B::default().with_device(ComputeDevice::Cuda);
        assert_eq!(c.device, ComputeDevice::Cuda);
        assert!(c.device.is_gpu());
        assert_eq!(c.device.n_gpu_layers(), 99);
    }

    // ===== T4：QuantConfig7B::with_path("custom.gguf") builder =====
    #[test]
    fn test_t4_quant_config_with_path() {
        let c = QuantConfig7B::default().with_path("custom.gguf");
        assert_eq!(c.model_path, "custom.gguf");
        // 其他字段保持默认
        assert_eq!(c.model_name, "Qwen2.5-7B");
        assert_eq!(c.quantization, Quantization::Q4_K_M);
    }

    // ===== T5：PowerPromptSet::default() 有 5 个提示词，且均非空 =====
    #[test]
    fn test_t5_power_prompt_set_default_5_prompts() {
        let set = PowerPromptSet::default();
        assert_eq!(set.prompts().len(), 5);
        for p in set.prompts() {
            assert!(!p.prompt.is_empty(), "prompt should be non-empty");
            assert!(!p.description.is_empty());
            assert!(!p.expected_keywords.is_empty());
        }
    }

    // ===== T6：PowerPrompt::validate_result("") 返回 false（空字符串）=====
    #[test]
    fn test_t6_validate_result_empty_returns_false() {
        let p = PowerPrompt {
            prompt: String::from("test"),
            expected_keywords: vec![String::from("charge")],
            description: String::from("test"),
        };
        assert!(!p.validate_result(""));
    }

    // ===== T7：PowerPrompt::validate_result("non-empty text") 返回 true（非空，无关键词但通过非空分支）=====
    #[test]
    fn test_t7_validate_result_non_empty_no_keyword_returns_true() {
        let p = PowerPrompt {
            prompt: String::from("test"),
            expected_keywords: vec![String::from("charge")],
            description: String::from("test"),
        };
        // "non-empty text" 不含 "charge"，但非空，故通过
        assert!(p.validate_result("non-empty text"));
    }

    // ===== T8：PowerPrompt::validate_result 含关键词返回 true =====
    #[test]
    fn test_t8_validate_result_with_keyword_returns_true() {
        let p = PowerPrompt {
            prompt: String::from("test"),
            expected_keywords: vec![String::from("charge")],
            description: String::from("test"),
        };
        assert!(p.validate_result("please charge the battery"));
    }

    // ===== T9：DeployReport::new(Cpu, 0) 初始状态（passed=true, failures 空）=====
    #[test]
    fn test_t9_deploy_report_new_initial_state() {
        let r = DeployReport::new(ComputeDevice::Cpu, 0);
        assert_eq!(r.device, ComputeDevice::Cpu);
        assert_eq!(r.n_gpu_layers, 0);
        assert_eq!(r.load_time_ns, 0);
        assert_eq!(r.inference_count, 0);
        assert_eq!(r.total_tokens, 0);
        assert_eq!(r.total_inference_ns, 0);
        assert_eq!(r.avg_tokens_per_sec, 0.0);
        assert!(r.passed);
        assert!(r.failures.is_empty());
    }

    // ===== T10：DeployReport record_load_time + record_inference + finalize（avg_tokens_per_sec 计算）=====
    #[test]
    fn test_t10_deploy_report_record_and_finalize() {
        let mut r = DeployReport::new(ComputeDevice::Cpu, 0);
        r.record_load_time(1_000_000);
        // 100 tokens in 500_000_000 ns (0.5s) → 200 tokens/sec
        r.record_inference(100, 500_000_000);
        r.finalize();
        assert_eq!(r.load_time_ns, 1_000_000);
        assert_eq!(r.inference_count, 1);
        assert_eq!(r.total_tokens, 100);
        assert_eq!(r.total_inference_ns, 500_000_000);
        assert!((r.avg_tokens_per_sec - 200.0).abs() < 0.001);
    }

    // ===== T11：DeployReport add_failure 将 passed=false，failures 有 1 项 =====
    #[test]
    fn test_t11_deploy_report_add_failure() {
        let mut r = DeployReport::new(ComputeDevice::Cpu, 0);
        assert!(r.passed);
        assert!(r.failures.is_empty());
        r.add_failure(String::from("test prompt"), DeployError::InferenceFailed);
        assert!(!r.passed);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].prompt, "test prompt");
        assert_eq!(r.failures[0].error, DeployError::InferenceFailed);
    }

    // ===== T12：MockDeployBackend check_hardware 返回 meets_requirements=true =====
    #[test]
    fn test_t12_mock_backend_check_hardware() {
        let backend = MockDeployBackend::new();
        let config = QuantConfig7B::default();
        let hw: HardwareCheck = backend.check_hardware(&config).expect("hardware check");
        assert!(hw.meets_requirements);
        assert!(hw.ram_gb > 0.0);
        assert!(hw.vram_gb > 0.0);
    }

    // ===== T13：MockDeployBackend load_model 成功（MockEngine）=====
    #[test]
    fn test_t13_mock_backend_load_model_success() {
        let backend = MockDeployBackend::new();
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        let config = QuantConfig7B::default();
        let ns = backend
            .load_model(&mut engine, &config)
            .expect("load_model should succeed");
        assert!(ns > 0, "load time should be positive");
        assert!(engine.health_check().loaded, "engine should be loaded");
    }

    // ===== T14：DeployVerifier deploy() 返回 Ok(DeployReport)，passed=true, inference_count=5 =====
    #[test]
    fn test_t14_deploy_verifier_deploy_success() {
        let engine = MockEngine::new(ComputeDevice::Cpu);
        let config = QuantConfig7B::default();
        let mut verifier = DeployVerifier::new(engine, config);
        let report = verifier.deploy().expect("deploy should succeed");
        assert!(report.passed, "all prompts should pass with MockEngine");
        assert_eq!(report.inference_count, 5);
        assert!(report.failures.is_empty());
        assert_eq!(report.device, ComputeDevice::Cpu);
        assert_eq!(report.n_gpu_layers, 0);
        assert!(report.load_time_ns > 0);
        assert!(report.total_tokens > 0);
    }

    // ===== T15：DeployVerifier undeploy 返回 Ok(()) =====
    #[test]
    fn test_t15_deploy_verifier_undeploy_ok() {
        let engine = MockEngine::new(ComputeDevice::Cpu);
        let config = QuantConfig7B::default();
        let mut verifier = DeployVerifier::new(engine, config);
        let result = verifier.undeploy();
        assert!(result.is_ok());
    }
}
