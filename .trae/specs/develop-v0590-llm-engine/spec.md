# v0.59.0 LLM 推理引擎选型与 FFI 封装 Spec

## Why

v0.58.0 完成了 P1-H RTOS 组件收官，本版本进入 P1-I AI Runtime LLM 第一层。双脑架构（LLM + Solver）的 LLM 是"感知者"，负责理解市场信号和自然语言指令输出 JSON 意图。本版本定义统一的 `LlmEngine` trait、llama.cpp FFI 绑定（feature-gated）与 `MockEngine`（默认可用），为后续 v0.60.0~v0.63.0 LLM 模型加载/量化/调度/模板奠定接口基础。

## What Changes

- 新增 crate `eneros-llm-engine`（位置：`crates/ai/llm-engine/`，子系统：ai）
- 新增 `LlmEngine` trait（无 Send + Sync，D2）
- 新增 `LlamaCppEngine` FFI 实现（`#[cfg(feature = "llama-cpp")]` 门控，D3）
- 新增 `MockEngine`（默认可用，用于测试与无 C 库场景，D3）
- 新增类型：`InferParams` / `ModelInfo` / `Quantization` / `ComputeDevice` / `EngineStats` / `EngineHealth` / `LlmError`
- FFI 绑定模块 `ffi`（`extern "C"` 声明，feature-gated）
- 设计文档 `docs/ai/llm-engine-design.md`（位置：`docs/ai/`，新增子目录）

## Impact

- Affected specs: 无（本版本为 P1-I 起点，无既有 spec 受影响）
- Affected code: 新增 `crates/ai/llm-engine/`；根 `Cargo.toml` members 新增条目；新增 `docs/ai/` 目录
- **无破坏性改动**：本版本仅新增 crate，不修改既有 crate

## 偏差声明（D1~D12）

> 依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 no_std / 项目约束的偏差。

### D1：no_std 合规 — `alloc::*` 替代 `std::*`

**蓝图**：`pub struct InferParams { stop_tokens: Vec<String> }` / `pub struct ModelInfo { name: String, ... }`，隐含 `std::string::String` 与 `std::vec::Vec`。

**实际**：本项目所有 Rust 代码必须 no_std（蓝图 §43.1，覆盖全项目）。

**决策**：使用 `alloc::string::String` 与 `alloc::vec::Vec`。`lib.rs` 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）。

### D2：drop `Send + Sync` bound（单线程 no_std）

**蓝图**：`pub trait LlmEngine: Send + Sync`。

**实际**：no_std RTOS 单线程（与 v0.57.0 D6 / v0.58.0 D6 一致）。`Send + Sync` 在单线程下无意义，且 `*mut c_void`（FFI 上下文）非 `Send`，强加会导致 `LlamaCppEngine` 无法实现。

**决策**：`pub trait LlmEngine`（无 Send + Sync bound）。

### D3：MockEngine 默认可用 + LlamaCppEngine feature-gated

**蓝图**：`LlamaCppEngine` 为默认实现，假设 llama.cpp C 库已链接。

**实际**：
1. llama.cpp 是 C++ 库，需要 cmake 编译并提供 `libllama.a` / `libllama.so`。CI 环境（无 GPU、无 CUDA toolkit）无法编译。
2. 本 crate 必须 no_std，但 `extern "C"` 调用 C++ 库需要 `std` 链接器（除非使用 `cstr_core` 等 no_std 兼容 crate，但复杂度高）。
3. 测试需在无 C 库环境下运行。

**决策**：
- `LlmEngine` trait + 类型（`InferParams` / `ModelInfo` / `Quantization` / `ComputeDevice` / `EngineStats` / `EngineHealth` / `LlmError`）默认可用（no_std）。
- `MockEngine` 默认可用（纯 Rust，无外部依赖，用于单元测试与无 C 库场景）。
- `LlamaCppEngine` + `ffi` 模块通过 `#[cfg(feature = "llama-cpp")]` 门控，仅当启用 `llama-cpp` feature 且链接 llama.cpp C 库时编译。
- `Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭）。

**理由**：保证 crate 在默认配置下可编译、可测试、可交叉编译到 aarch64-unknown-none。实际部署时通过 `--features llama-cpp` 启用真实推理。

### D4：GPU 优先测试规则 — llama.cpp `n_gpu_layers`，非 PyTorch

**蓝图**：§43.3 已明确"边缘 LLM 推理统一采用 llama.cpp（C API），禁止在边缘侧使用 PyTorch"。

**实际**：本 crate 是 Rust no_std，无 PyTorch 依赖。GPU 加速通过 llama.cpp 的 `n_gpu_layers` 参数控制（C 库内部实现），Rust 侧仅通过 `ComputeDevice` enum 声明目标设备 + FFI 传递 `n_gpu_layers` 整数。

**决策**：
- `ComputeDevice` enum（Cpu / Cuda / Metal / Npu）默认 `Cpu`（D12）。
- `LlamaCppEngine::new(device: ComputeDevice)` 接受设备参数，内部映射到 `n_gpu_layers`（Cpu=0，Cuda/Metal/Npu=99 全 offload）。
- `MockEngine` 接受 `ComputeDevice` 参数并记录，用于测试断言 GPU 优先逻辑。
- 单元测试验证：GPU 可用时 `ComputeDevice::Cuda`，不可用时退到 `ComputeDevice::Cpu`（与 user_profile GPU 优先规则一致）。

### D5：`EngineStats` 用普通 u64，不用 AtomicU64

**蓝图**：蓝图未定义 `EngineStats`，但 `LlamaCppEngine { stats: EngineStats }` 暗示需要统计。

**实际**：与 v0.56.0 D7 / v0.57.0 D7 / v0.58.0 D4 一致，单线程无需原子操作。

**决策**：`EngineStats { inference_count: u64, total_tokens_generated: u64, total_inference_ns: u64, last_inference_ns: u64, model_load_count: u64, gpu_layers: u32 }`，全部普通 `u64`/`u32`，派生 `Default`。

### D6：`&str` path 参数保留（no_std 兼容）

**蓝图**：`fn load_model(&mut self, path: &str) -> Result<(), LlmError>`。

**实际**：`&str` 在 no_std 下可用（`core::str`）。模型路径来自配置（v0.26.0），是 UTF-8 字符串。

**决策**：保留 `&str` 签名。`LlamaCppEngine` 内部通过 `alloc::ffi::CString::new(path)` 转换为 C 字符串（`alloc::ffi::CString` 在 no_std 可用）。

### D7：`LlmError` 错误类型 — no_std Display

**蓝图**：未定义 `LlmError` 变体。

**实际**：需要完整错误类型供 trait 返回。

**决策**：`LlmError` 枚举（`LoadFailed` / `InferFailed` / `InvalidPath` / `InvalidPrompt` / `Utf8Error` / `GpuUnavailable` / `ModelNotLoaded` / `OutOfMemory`）。派生 `Debug`，实现 `core::fmt::Display`。

### D8：`infer_stream` callback — `&mut dyn FnMut(&str) -> bool`

**蓝图**：`fn infer_stream(&mut self, prompt: &str, params: &InferParams, callback: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError>`。

**实际**：`&mut dyn FnMut` 是 trait object 引用（非 `Box<dyn>`），no_std 兼容。回调返回 `bool`（`true` 继续，`false` 停止）。

**决策**：保留蓝图签名。`MockEngine` 实现模拟流式（按字符切分 prompt 回调）。

### D9：crate 位置 — `crates/ai/llm-engine/`

**蓝图**：交付物 `llm-engine` crate，未指定位置。

**实际**：项目规则 §2.3.1 要求所有 crate 放入 `crates/<subsystem>/`。`crates/ai/` 是 AI Runtime 子系统（LLM + Solver，Phase 2+）。

**决策**：`crates/ai/llm-engine/`。同时新增 `docs/ai/` 目录存放 AI 相关文档。

### D10：FFI 安全 — `extern "C"` + `unsafe` 封装

**蓝图**：直接在 `LlamaCppEngine` 方法内调用 `unsafe { ffi::llama_*() }`。

**实际**：FFI 边界需集中封装，避免 `unsafe` 扩散。

**决策**：`ffi` 模块声明 `extern "C"` 函数（feature-gated）；`LlamaCppEngine` 方法内调用 `unsafe` 块，但每个 `unsafe` 块有 SAFETY 注释说明不变量。指针所有权明确：`llama_init` 返回的 `*mut c_void` 由 `LlamaCppEngine` 持有，`Drop` 时调用 `llama_free`。`llama_infer` 返回的 `*mut c_char` 立即转 `CStr` 拷贝为 `String`，再调用 `llama_free_result` 释放。

### D11：`Quantization` enum — 加 `Default`

**蓝图**：`Quantization` enum（F16 / Q8_0 / Q4_0 / Q4_K_M），推荐 Q4_K_M。

**实际**：`ModelInfo` 含 `quantization: Quantization` 字段，需 `Default`。

**决策**：派生 `Default`，`#[default]` 标注 `Q4_K_M`（nightly feature，项目已使用 nightly）。

### D12：`ComputeDevice` enum — 加 `Default = Cpu`

**蓝图**：`ComputeDevice` enum（Cpu / Cuda / Metal / Npu）。

**实际**：`LlamaCppEngine::new(device)` + `MockEngine::new(device)` 需默认值。

**决策**：派生 `Default`，`#[default]` 标注 `Cpu`（GPU 优先是 opt-in，默认 CPU 保证可用性）。

## ADDED Requirements

### Requirement: LlmEngine 统一推理接口

系统 SHALL 提供 `LlmEngine` trait，定义 LLM 推理引擎的统一接口（load_model / infer / infer_stream / model_info / health_check），供上层（v0.61.0 推理调度器、v0.71.0 双脑联调）依赖。

#### Scenario: 加载模型
- **WHEN** `load_model(&mut self, path: &str)` 被调用且模型文件存在
- **THEN** 返回 `Ok(())`，内部 `model_info` 更新为加载的模型信息，`stats.model_load_count += 1`

#### Scenario: 推理
- **WHEN** `infer(&mut self, prompt: &str, params: &InferParams)` 被调用且模型已加载
- **THEN** 返回 `Ok(String)` 包含生成文本，`stats.inference_count += 1`、`stats.total_tokens_generated += 生成 token 数`

#### Scenario: 流式推理
- **WHEN** `infer_stream(&mut self, prompt, params, callback)` 被调用
- **THEN** 逐 token 调用 `callback(&token)`；若 callback 返回 `false` 则停止生成；返回 `Ok(())` 表示完成

#### Scenario: 健康检查
- **WHEN** `health_check(&self)` 被调用
- **THEN** 返回 `EngineHealth { loaded: bool, device: ComputeDevice, gpu_layers: u32, last_error: Option<LlmError> }`

### Requirement: MockEngine 默认实现

系统 SHALL 提供 `MockEngine` 默认实现（无外部 C 库依赖），用于单元测试与无 llama.cpp 环境下的接口验证。

#### Scenario: 构造 MockEngine
- **WHEN** `MockEngine::new(device: ComputeDevice)` 被调用
- **THEN** 返回 `MockEngine` 实例，初始 `loaded = false`，`device` 记录

#### Scenario: MockEngine 加载模型
- **WHEN** `load_model(&mut self, path)` 被调用
- **THEN** 设置 `loaded = true`，`model_info = Some(ModelInfo { name: path, ... })`，返回 `Ok(())`

#### Scenario: MockEngine 推理
- **WHEN** `infer(&mut self, prompt, params)` 被调用且 `loaded == true`
- **THEN** 返回 `Ok(String)` 包含 mock 输出（如 `"mock: <prompt>"`），统计更新

#### Scenario: MockEngine 未加载模型推理
- **WHEN** `infer(&mut self, prompt, params)` 被调用且 `loaded == false`
- **THEN** 返回 `Err(LlmError::ModelNotLoaded)`

### Requirement: LlamaCppEngine FFI 实现（feature-gated）

系统 SHALL 提供 `LlamaCppEngine` 实现（`#[cfg(feature = "llama-cpp")]` 门控），通过 FFI 调用 llama.cpp C 库执行真实推理。

#### Scenario: 构造 LlamaCppEngine
- **WHEN** `LlamaCppEngine::new(device: ComputeDevice)` 被调用（feature = "llama-cpp" 启用）
- **THEN** 调用 `ffi::llama_init()` 获取 C 上下文指针，返回 `LlamaCppEngine` 实例

#### Scenario: GPU 优先设备选择
- **WHEN** `ComputeDevice::Cuda` 传入 `new`
- **THEN** `n_gpu_layers` 设为 99（全 offload）；若 GPU 不可用，`load_model` 返回 `Err(LlmError::GpuUnavailable)`

#### Scenario: CPU 降级
- **WHEN** `ComputeDevice::Cpu` 传入 `new`
- **THEN** `n_gpu_layers = 0`，纯 CPU 推理

### Requirement: 推理参数与模型信息类型

系统 SHALL 提供 `InferParams` / `ModelInfo` / `Quantization` / `ComputeDevice` 类型，描述推理参数与模型元数据。

#### Scenario: InferParams 默认值
- **WHEN** `InferParams::default()` 被调用
- **THEN** 返回 `max_tokens=128, temperature=0.7, top_p=0.9, top_k=40, repeat_penalty=1.1, stop_tokens=Vec::new()`

#### Scenario: ModelInfo 默认值
- **WHEN** `ModelInfo::default()` 被调用
- **THEN** 返回 `name=String::new(), size_bytes=0, quantization=Quantization::Q4_K_M, context_length=2048, device=ComputeDevice::Cpu`

### Requirement: 引擎统计与健康检查

系统 SHALL 提供 `EngineStats` 与 `EngineHealth` 类型，用于推理统计与健康状态查询。

#### Scenario: EngineStats 累加
- **WHEN** 推理完成后
- **THEN** `inference_count += 1`、`total_tokens_generated += 生成 token 数`、`total_inference_ns += 耗时`、`last_inference_ns = 本次耗时`

#### Scenario: EngineHealth 查询
- **WHEN** `health_check()` 被调用
- **THEN** 返回 `EngineHealth { loaded: bool, device: ComputeDevice, gpu_layers: u32, last_error: Option<LlmError> }`
