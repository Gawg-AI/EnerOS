# v0.61.0 7B INT4 量化模型部署 Spec

## Why

v0.60.0 实现了 GGUF 模型加载器（`GgufLoader`），v0.59.0 定义了 `LlmEngine` trait 与 `MockEngine`/`LlamaCppEngine`。本版本定义 7B INT4 量化模型部署配置（`QuantConfig7B`）、部署验证器（`DeployVerifier`）与电力场景测试用例（`PowerPromptSet`），为实际部署 Qwen2.5-7B-Q4_K_M 模型到边缘设备提供配置化、可验证的部署流程。

## What Changes

- 新增 crate `eneros-model-deploy`（位置：`crates/ai/model-deploy/`，子系统：ai）
- 新增 `QuantConfig7B` — 7B INT4 量化模型配置（模型名/量化方式/文件大小/RAM/VRAM 需求/上下文长度/推理参数）
- 新增 `DeployVerifier` — 部署验证器（加载模型 + 执行测试用例 + 收集性能指标）
- 新增 `DeployReport` — 部署验证报告（设备/加载时间/推理速度/token 统计）
- 新增 `PowerPromptSet` — 电力场景测试 prompt 集合（储能策略/电价响应/异常处理）
- 新增 `DeployError` — 部署错误类型
- 复用 v0.59.0 类型：`LlmEngine` / `InferParams` / `Quantization` / `ComputeDevice` / `EngineStats`（D11，不重定义）
- 复用 v0.60.0 类型：`GgufLoader` / `GgufMetadata`（D11，不重定义）
- 新增 `MockDeployBackend` — 默认可用的 Mock 部署后端（D12，用于无模型文件场景测试）
- 设计文档 `docs/ai/model-deploy-design.md`

## Impact

- Affected specs: v0.59.0（依赖其 `LlmEngine` / `InferParams` / `Quantization` / `ComputeDevice` 类型），v0.60.0（依赖其 `GgufLoader` 类型）
- Affected code: 新增 `crates/ai/model-deploy/`；根 `Cargo.toml` members 新增条目
- **无破坏性改动**：本版本仅新增 crate，不修改 v0.59.0 / v0.60.0 既有代码

## 偏差声明（D1~D12）

> 依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 no_std / 项目约束的偏差。

### D1：no_std 合规 — `alloc::*` 替代 `std::*`

**蓝图**：`pub struct QuantConfig7B { model_name: String, ... }`，隐含 `std::string::String`。部署验证脚本是 Python（`deploy_verify()`）。

**实际**：本项目所有 Rust 代码必须 no_std（蓝图 §43.1，覆盖全项目）。Python 脚本不能作为 crate 交付物。

**决策**：
- 使用 `alloc::string::String` 与 `alloc::vec::Vec`
- `lib.rs` 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- 蓝图的 Python 脚本翻译为 Rust `DeployVerifier`（D5），不引入 Python 依赖

### D2：`DeployVerifier` 泛型化 — 不绑定具体 `LlmEngine` 实现

**蓝图**：`engine = LlamaCppEngine(...)` 直接绑定 `LlamaCppEngine`。

**实际**：
1. `LlamaCppEngine` 需 `llama-cpp` feature + C++ 库链接（v0.59.0 D3）
2. 测试环境（CI）无 C++ 库，需要 `MockEngine` 可替代
3. 蓝图 §9.7 要求"支持其他 7B 模型"（可扩展性）

**决策**：
- `DeployVerifier<E: LlmEngine>` 泛型化，接受任意 `LlmEngine` 实现
- `DeployVerifier::new(engine: E, config: QuantConfig7B) -> Self`
- 测试时注入 `MockEngine`（默认可用），实际部署时注入 `LlamaCppEngine`（feature-gated）
- `MockDeployBackend`（D12）提供 Mock 部署后端，用于无模型文件场景

### D3：`MockDeployBackend` 默认可用 + `LlamaDeployBackend` feature-gated

**蓝图**：`engine.load_model("models/Qwen2.5-7B-Q4_K_M.gguf")` 直接加载真实模型文件。

**实际**：
1. 真实模型文件 4GB，不入仓（蓝图 §3.3 大文件处理）
2. CI 环境无模型文件、无 GPU
3. 需要在无模型文件环境下测试部署逻辑

**决策**：
- 定义 `DeployBackend` trait：`fn load_model(&self, engine: &mut dyn LlmEngine, config: &QuantConfig7B) -> Result<(), DeployError>` + `fn check_hardware(&self, config: &QuantConfig7B) -> Result<HardwareCheck, DeployError>`
- `MockDeployBackend`（默认可用）— 调用 `engine.load_model()` 并记录时间，不校验真实硬件
- `LlamaDeployBackend`（`#[cfg(feature = "llama-cpp")]` 门控）— 校验真实 RAM/VRAM + 加载真实 GGUF 文件
- `Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭，与 v0.59.0/v0.60.0 一致）

### D4：GPU 优先通过 `n_gpu_layers` 跟踪（非 PyTorch）

**蓝图**：`use_gpu = llama_gpu_available()` + `engine = LlamaCppEngine(n_gpu_layers=-1 if use_gpu else 0)`。

**实际**：本 crate 是 Rust no_std，无 PyTorch 依赖。GPU 加速通过 llama.cpp 的 `n_gpu_layers` 参数控制（v0.59.0 `ComputeDevice::n_gpu_layers()`）。

**决策**：
- `DeployVerifier` 记录 `device: ComputeDevice`（来自 `QuantConfig7B`）
- `QuantConfig7B::default()` 默认 `ComputeDevice::Cpu`（与 v0.59.0 D12 一致）
- `DeployVerifier::with_device(device)` 切换设备（GPU 优先是 opt-in）
- `DeployReport` 记录 `device` 与 `n_gpu_layers`，用于验证 GPU 优先逻辑

### D5：`DeployReport` 用普通 u64 统计，不用 AtomicU64

**蓝图**：未定义报告类型，但 `deploy_verify()` 输出 `tokens_per_sec` / `mem` 等指标。

**实际**：与 v0.56.0 D7 / v0.57.0 D7 / v0.58.0 D4 / v0.59.0 D5 / v0.60.0 D5 一致，单线程无需原子操作。

**决策**：`DeployReport { device: ComputeDevice, n_gpu_layers: u32, load_time_ns: u64, inference_count: u64, total_tokens: u64, total_inference_ns: u64, avg_tokens_per_sec: f64, passed: bool, failures: Vec<DeployFailure> }`，普通 `u64`/`u32`/`f64`，派生 `Debug`/`Clone`。

### D6：`&str` path 参数保留

**蓝图**：`engine.load_model("models/Qwen2.5-7B-Q4_K_M.gguf")`。

**实际**：`&str` 在 no_std 下可用（`core::str`）。模型路径来自配置（v0.26.0）。

**决策**：保留 `&str` 签名。`QuantConfig7B` 包含 `model_path: String` 字段。

### D7：`DeployError` 错误类型 — 部署场景专用错误

**蓝图**：未定义 `DeployError` 变体。

**实际**：部署场景有独特的失败模式（硬件不足/推理失败/结果无效）。

**决策**：`DeployError` 枚举：
- `HardwareInsufficient` — RAM/VRAM 不满足最低要求
- `ModelLoadFailed` — 模型加载失败
- `InferenceFailed` — 推理失败
- `InvalidResult` — 推理结果不符合预期（如非 JSON）
- `Timeout` — 推理超时
- `BackendError` — 后端错误
- `NotDeployed` — 未部署就执行验证

派生 `Debug` + `Clone`，实现 `core::fmt::Display`。提供 `From<LlmError>` 转换（复用 v0.59.0 错误）。

### D8：`DeployVerifier` 无 `Drop` — 显式部署/卸载

**蓝图**：`deploy_verify()` 一次性执行，无生命周期管理。

**实际**：部署是长期运行状态（模型驻留内存），不应在 Drop 时自动卸载。

**决策**：
- `DeployVerifier` 不实现 `Drop`（与 v0.60.0 D8 不同）
- `deploy()` 返回 `Result<DeployReport, DeployError>`，执行完整部署验证流程
- `undeploy()` 显式卸载（调用 `engine` 的清理逻辑，实际清理由 `LlmEngine` 实现负责）

### D9：crate 位置 `crates/ai/model-deploy/`

**蓝图**：`model-deploy` 模块。

**实际**：项目规则 §2.3.1 要求所有 crate 放入 `crates/<subsystem>/`。模型部署属于 AI 子系统。

**决策**：crate 路径 `crates/ai/model-deploy/`，package name `eneros-model-deploy`。跨 crate 引用 v0.59.0：`path = "../llm-engine"`，v0.60.0：`path = "../gguf-loader"`。

### D10：无 FFI — 本版本不直接调用 C 库

**蓝图**：`llama_gpu_available()` / `llama_gpu_memory_used()` 是 FFI 调用。

**实际**：本 crate 通过 `LlmEngine` trait 间接调用 llama.cpp（v0.59.0 已封装 FFI）。不直接声明 `extern "C"`。

**决策**：
- 不引入新的 FFI 声明
- GPU 检测通过 `ComputeDevice` 枚举（调用方指定，而非运行时检测）
- 内存使用通过 `EngineStats`（v0.59.0）查询

### D11：复用 v0.59.0 + v0.60.0 类型 — 不重定义

**蓝图**：`QuantConfig7B { quantization: Quantization, infer_params: InferParams }`。

**实际**：v0.59.0 已定义 `Quantization`（F16/Q8_0/Q4_0/Q4_K_M）、`InferParams`、`LlmEngine`、`ComputeDevice`、`EngineStats`。v0.60.0 已定义 `GgufLoader`、`GgufMetadata`。重定义会导致类型不兼容。

**决策**：
- `eneros-model-deploy` 依赖 `eneros-llm-engine`（`path = "../llm-engine"`）+ `eneros-gguf-loader`（`path = "../gguf-loader"`）
- `QuantConfig7B` 使用 `quantization: Quantization`（来自 v0.59.0）、`infer_params: InferParams`（来自 v0.59.0）
- `DeployVerifier<E: LlmEngine>` 使用 v0.59.0 trait
- `DeployBackend` 实现可选使用 `GgufLoader`（来自 v0.60.0）

### D12：`MockDeployBackend` 作为默认后端

**蓝图**：`deploy_verify()` 直接加载真实模型文件。

**实际**：CI 环境无模型文件，需要默认可用的 Mock 后端。

**决策**：
- `DeployVerifier::new(engine, config)` 默认使用 `MockDeployBackend`
- `DeployVerifier::with_backend(engine, config, backend)` 接受自定义后端
- `MockDeployBackend` 调用 `engine.load_model()`（MockEngine 会成功），记录时间，不校验硬件
- 测试时注入 `MockEngine` + `MockDeployBackend`，验证部署逻辑

## ADDED Requirements

### Requirement: 7B INT4 量化配置

系统 SHALL 提供 `QuantConfig7B` 结构体，定义 7B INT4 量化模型的部署配置，包括模型名、量化方式、文件大小、RAM/VRAM 最低需求、上下文长度与推理参数。

#### Scenario: 默认配置
- **WHEN** `QuantConfig7B::default()` 被调用
- **THEN** 返回 `QuantConfig7B { model_name: "Qwen2.5-7B", quantization: Q4_K_M, file_size_gb: 4.0, min_ram_gb: 8.0, min_vram_gb: 6.0, context_length: 4096, infer_params: InferParams { max_tokens: 512, temperature: 0.1, ... } }`

#### Scenario: 自定义配置
- **WHEN** `QuantConfig7B::new("Llama2-7B")` 被调用
- **THEN** 返回配置，`model_name` 为 "Llama2-7B"，其余字段为默认值

### Requirement: 部署验证器

系统 SHALL 提供 `DeployVerifier<E: LlmEngine>` 泛型部署验证器，执行加载模型、运行测试用例、收集性能指标的完整流程。

#### Scenario: 部署并验证
- **WHEN** `DeployVerifier::new(engine, config).deploy()` 被调用
- **THEN** 加载模型（`engine.load_model`）
- **AND** 执行 `PowerPromptSet` 中的测试用例
- **AND** 收集加载时间、推理速度、token 统计
- **AND** 返回 `Ok(DeployReport)`

#### Scenario: 部署失败
- **WHEN** `engine.load_model` 返回 `Err`
- **THEN** 返回 `Err(DeployError::ModelLoadFailed)`

### Requirement: 电力场景测试用例

系统 SHALL 提供 `PowerPromptSet`，包含储能策略、电价响应、异常处理等电力场景测试 prompt，用于验证 LLM 在电力场景的推理正确性。

#### Scenario: 储能策略 prompt
- **WHEN** `PowerPromptSet::default().prompts()` 被调用
- **THEN** 返回包含 "当前电价为 0.5 元/kWh，储能 SOC 为 80%，请输出充放电策略 JSON" 等 prompt 的列表

#### Scenario: 验证结果合理性
- **WHEN** `DeployVerifier` 执行测试用例
- **THEN** 对每个 prompt 的推理结果进行基本校验（非空、长度 > 0）

### Requirement: 部署报告

系统 SHALL 提供 `DeployReport` 结构体，记录部署验证的完整结果，包括设备、加载时间、推理速度、token 统计、通过/失败状态。

#### Scenario: 成功报告
- **WHEN** 所有测试用例通过
- **THEN** `DeployReport.passed == true`，`failures` 为空

#### Scenario: 失败报告
- **WHEN** 部分测试用例失败
- **THEN** `DeployReport.passed == false`，`failures` 包含失败详情

### Requirement: `MockDeployBackend` 默认可用

系统 SHALL 提供 `MockDeployBackend` 作为默认部署后端，在无模型文件环境下测试部署逻辑。

#### Scenario: Mock 部署
- **WHEN** `MockDeployBackend.deploy(engine, config)` 被调用
- **THEN** 调用 `engine.load_model(config.model_path)` 并记录时间
- **AND** 不校验真实硬件（RAM/VRAM）
- **AND** 返回 `Ok(())`（若 `engine.load_model` 成功）
