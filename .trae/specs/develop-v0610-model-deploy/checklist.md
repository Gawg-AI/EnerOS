# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.61.0`
- [x] C2 members 列表已添加 `crates/ai/model-deploy`
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/model-deploy/Cargo.toml` 存在，package name = `eneros-model-deploy`
- [x] C5 dependencies 包含 `eneros-llm-engine = { path = "../llm-engine" }` + `eneros-gguf-loader = { path = "../gguf-loader" }`（D11）
- [x] C6 `[features] llama-cpp = []` 声明（D3，默认关闭）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：config / error / report / prompts / verifier / backend / mock_backend

## error.rs — DeployError
- [x] C10 `DeployError` 枚举包含 7 变体（HardwareInsufficient / ModelLoadFailed / InferenceFailed / InvalidResult / Timeout / BackendError / NotDeployed）
- [x] C11 派生 `Debug` + `Clone`
- [x] C12 实现 `core::fmt::Display`
- [x] C13 实现 `From<LlmError>` 转换（复用 v0.59.0 错误）

## config.rs — QuantConfig7B
- [x] C14 `QuantConfig7B` 结构体：model_name / model_path / quantization / file_size_gb / min_ram_gb / min_vram_gb / context_length / device / infer_params（D11 复用 v0.59.0）
- [x] C15 派生 `Debug / Clone`
- [x] C16 `QuantConfig7B::default()` — Qwen2.5-7B, Q4_K_M, 4.0GB, 8.0GB RAM, 6.0GB VRAM, 4096 context, Cpu, temperature=0.1
- [x] C17 `QuantConfig7B::new(model_name: &str)` — 自定义模型名
- [x] C18 `with_device(device)` builder（D4）
- [x] C19 `with_path(path)` builder
- [x] C20 单元测试：默认值 / 自定义模型名 / builder

## prompts.rs — PowerPromptSet
- [x] C21 `PowerPrompt` 结构体：prompt / expected_keywords / description
- [x] C22 `PowerPromptSet` 结构体：prompts: Vec<PowerPrompt>
- [x] C23 `PowerPromptSet::default()` — 5 个电力场景 prompt（储能策略/电价响应/异常处理等）
- [x] C24 `PowerPromptSet::prompts()` 查询方法
- [x] C25 `PowerPrompt::validate_result(result)` — 校验结果非空 + 包含任一关键词
- [x] C26 单元测试：默认 prompt 数量 / validate_result 有效/无效

## report.rs — DeployReport
- [x] C27 `DeployFailure` 结构体：prompt / error
- [x] C28 `DeployReport` 结构体：device / n_gpu_layers / load_time_ns / inference_count / total_tokens / total_inference_ns / avg_tokens_per_sec / passed / failures（D5）
- [x] C29 派生 `Debug / Clone`
- [x] C30 `DeployReport::new(device, n_gpu_layers)` 构造
- [x] C31 `record_load_time(ns)` 方法
- [x] C32 `record_inference(tokens, ns)` 方法
- [x] C33 `add_failure(prompt, error)` 方法
- [x] C34 `finalize()` — 计算 avg_tokens_per_sec 并设 passed
- [x] C35 单元测试：构造 / 累加 / finalize / add_failure

## backend.rs — DeployBackend trait + HardwareCheck
- [x] C36 `HardwareCheck` 结构体：ram_gb / vram_gb / meets_requirements
- [x] C37 `DeployBackend` trait 定义：`check_hardware` + `load_model`（返回加载时间 ns）
- [x] C38 trait 编译通过

## mock_backend.rs — MockDeployBackend
- [x] C39 `MockDeployBackend` 结构体
- [x] C40 `MockDeployBackend::new()` 构造
- [x] C41 实现 `DeployBackend` for `MockDeployBackend`
- [x] C42 `check_hardware` 返回 `Ok(HardwareCheck { meets_requirements: true })`（Mock 通过）
- [x] C43 `load_model` 调用 `engine.load_model`，返回固定时间
- [x] C44 单元测试：check_hardware / load_model

## verifier.rs — DeployVerifier
- [x] C45 `DeployVerifier<E: LlmEngine>` 结构体：engine / config / backend / prompts（D2 泛型化）
- [x] C46 `DeployVerifier::new(engine, config)` — 带 MockDeployBackend + PowerPromptSet::default()
- [x] C47 `DeployVerifier::with_backend(engine, config, backend)` — 自定义后端
- [x] C48 `deploy()` — 完整部署验证流程
- [x] C49 `undeploy()` — 卸载
- [x] C50 单元测试：完整部署 / 部署失败 / undeploy

## llama_backend.rs — LlamaDeployBackend（feature-gated）
- [x] C51 `#[cfg(feature = "llama-cpp")]` 门控整个模块
- [x] C52 `LlamaDeployBackend` 结构体
- [x] C53 实现 `DeployBackend` for `LlamaDeployBackend`
- [x] C54 默认 feature 下不编译（`cargo build` 不报错）

## 集成测试（lib.rs）
- [x] C55 T1 QuantConfig7B 默认值
- [x] C56 T2 QuantConfig7B::new 自定义模型名
- [x] C57 T3 QuantConfig7B with_device builder
- [x] C58 T4 PowerPromptSet 默认 5 个 prompt
- [x] C59 T5 PowerPrompt validate_result 有效结果
- [x] C60 T6 PowerPrompt validate_result 无效结果
- [x] C61 T7 DeployReport 构造 + record_load_time
- [x] C62 T8 DeployReport record_inference + finalize
- [x] C63 T9 DeployReport add_failure
- [x] C64 T10 MockDeployBackend check_hardware
- [x] C65 T11 MockDeployBackend load_model 成功
- [x] C66 T12 DeployVerifier 完整部署
- [x] C67 T13 DeployVerifier 部署失败
- [x] C68 T14 DeployVerifier GPU 优先逻辑
- [x] C69 T15 DeployVerifier undeploy
- [x] C70 `cargo test -p eneros-model-deploy` 15/15 通过

## 设计文档
- [x] C71 `docs/ai/model-deploy-design.md` 存在
- [x] C72 12 章节完整
- [x] C73 2 Mermaid 图（DeployVerifier 类图 + 部署时序图）
- [x] C74 D1~D12 偏差声明表
- [x] C75 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C76 `Makefile` 版本号 `0.61.0`
- [x] C77 `.github/workflows/ci.yml` 版本号 `0.61.0`
- [x] C78 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-model-deploy`

## 构建校验（§2.4.2 C6~C11）
- [x] C79 `cargo metadata --format-version 1` 成功
- [x] C80 `cargo test -p eneros-model-deploy` 全部通过（15 tests）
- [x] C81 `cargo build -p eneros-model-deploy --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C82 `cargo fmt -p eneros-model-deploy -- --check` 通过
- [x] C83 `cargo clippy -p eneros-model-deploy --all-targets -- -D warnings` 无 warning
- [x] C84 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C85 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C86 无 `panic!` / `todo!` / `unimplemented!`
- [x] C87 子模块不重复 `#![cfg_attr(not(test), no_std)]`

## 目录规范
- [x] C88 crate 在 `crates/ai/model-deploy/`（D9）
- [x] C89 跨 crate path 引用 `../llm-engine` + `../gguf-loader`（相对路径）
- [x] C90 文档在 `docs/ai/` 下
- [x] C91 无根目录 crate（除 `ci/`）
- [x] C92 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖复用（D11）
- [x] C93 复用 v0.59.0 `LlmEngine` trait（不重定义）
- [x] C94 复用 v0.59.0 `InferParams` / `Quantization` / `ComputeDevice`（不重定义）
- [x] C95 复用 v0.60.0 `GgufLoader`（可选，LlamaDeployBackend 使用）
- [x] C96 `From<LlmError> for DeployError` 转换实现
