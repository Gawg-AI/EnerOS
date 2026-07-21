# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.59.0`
- [x] C2 members 列表已添加 `crates/ai/llm-engine`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/ai/llm-engine/Cargo.toml` 存在，package name 为 `eneros-llm-engine`
- [x] C5 `[features] llama-cpp = []` 声明（默认关闭，D3）
- [x] C6 无外部依赖（纯 no_std + alloc）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 模块声明完整：error / params / model / device / stats / engine / mock / ffi / llama_cpp
- [x] C9 D1~D12 偏差声明表存在于 lib.rs

## LlmError 错误类型
- [x] C10 `LlmError` 枚举包含 8 变体（LoadFailed / InferFailed / InvalidPath / InvalidPrompt / Utf8Error / GpuUnavailable / ModelNotLoaded / OutOfMemory）
- [x] C11 派生 `Debug`
- [x] C12 实现 `core::fmt::Display`

## ComputeDevice
- [x] C13 `ComputeDevice` 枚举包含 Cpu / Cuda / Metal / Npu（D12）
- [x] C14 派生 `Debug / Clone / Copy / PartialEq / Eq / Default`
- [x] C15 `#[default]` 标注 `Cpu`
- [x] C16 `is_gpu(&self) -> bool` 方法（Cpu=false，其余=true）
- [x] C17 `n_gpu_layers(&self) -> u32` 方法（Cpu=0，其余=99，D4）
- [x] C18 单元测试 — is_gpu / n_gpu_layers

## ModelInfo + Quantization
- [x] C19 `Quantization` 枚举包含 F16 / Q8_0 / Q4_0 / Q4_K_M（D11）
- [x] C20 派生 `Debug / Clone / Copy / PartialEq / Eq / Default`，`#[default]` 标注 `Q4_K_M`
- [x] C21 `ModelInfo` 结构体包含 5 字段（name / size_bytes / quantization / context_length / device）
- [x] C22 派生 `Debug / Clone / Default`
- [x] C23 Default：name=空，size_bytes=0，quantization=Q4_K_M，context_length=2048，device=Cpu
- [x] C24 单元测试 — 默认值

## InferParams
- [x] C25 `InferParams` 结构体包含 6 字段（max_tokens / temperature / top_p / top_k / repeat_penalty / stop_tokens）
- [x] C26 派生 `Debug / Clone / Default`
- [x] C27 Default：max_tokens=128, temperature=0.7, top_p=0.9, top_k=40, repeat_penalty=1.1, stop_tokens=Vec::new()
- [x] C28 单元测试 — 默认值

## EngineStats + EngineHealth
- [x] C29 `EngineStats` 结构体包含 6 字段（inference_count / total_tokens_generated / total_inference_ns / last_inference_ns / model_load_count / gpu_layers）
- [x] C30 不使用 AtomicU64（D5）
- [x] C31 派生 `Debug / Clone / Default`
- [x] C32 `EngineHealth` 结构体包含 4 字段（loaded / device / gpu_layers / last_error）
- [x] C33 派生 `Debug / Clone`
- [x] C34 单元测试 — 累加

## LlmEngine trait（D2 无 Send + Sync）
- [x] C35 `LlmEngine` trait 定义，**无 Send + Sync bound**（D2）
- [x] C36 `load_model(&mut self, path: &str) -> Result<(), LlmError>`
- [x] C37 `infer(&mut self, prompt: &str, params: &InferParams) -> Result<String, LlmError>`
- [x] C38 `infer_stream(&mut self, prompt: &str, params: &InferParams, callback: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError>`（D8）
- [x] C39 `model_info(&self) -> Option<&ModelInfo>`
- [x] C40 `health_check(&self) -> EngineHealth`
- [x] C41 `stats(&self) -> &EngineStats`
- [x] C42 编译通过（trait 定义）

## MockEngine（D3 默认可用）
- [x] C43 `MockEngine` 结构体包含 5 字段（loaded / device / model_info / stats / mock_output）
- [x] C44 `new(device: ComputeDevice) -> Self`（初始 loaded=false）
- [x] C45 `with_output(output: &str) -> Self` builder
- [x] C46 实现 `LlmEngine` trait 全部 6 个方法
- [x] C47 load_model 设 loaded=true + model_info
- [x] C48 infer 返回 Ok(format!("mock: <prompt>"))，stats 更新
- [x] C49 infer 未加载返回 Err(ModelNotLoaded)
- [x] C50 infer_stream 按字符切分调用 callback
- [x] C51 单元测试 — 构造 / 加载 / 推理 / 未加载 / 流式 / 统计

## ffi 模块（D3 feature-gated, D10）
- [x] C52 `#[cfg(feature = "llama-cpp")]` 门控整个 ffi 模块
- [x] C53 `extern "C"` 声明 6 函数（llama_init / llama_load_model / llama_infer / llama_free_result / llama_free / llama_set_device）
- [x] C54 使用 `core::ffi::*` 类型（c_void / c_char / c_int / c_uint / c_float）
- [x] C55 默认 feature 下不编译（`cargo build` 不报错）

## LlamaCppEngine（D3 feature-gated）
- [x] C56 `#[cfg(feature = "llama-cpp")]` 门控整个 llama_cpp 模块
- [x] C57 `LlamaCppEngine` 结构体（ctx: *mut c_void / model_info / device / stats）
- [x] C58 `new(device: ComputeDevice) -> Self`（调用 ffi::llama_init()）
- [x] C59 实现 `LlmEngine` trait
- [x] C60 load_model：CString + ffi::llama_load_model + 设置 device + 更新 model_info
- [x] C61 infer：CString + ffi::llama_infer + CStr + String + llama_free_result + 统计
- [x] C62 实现 `Drop`：调用 ffi::llama_free(ctx)
- [x] C63 默认 feature 下不编译（`cargo build` 不报错）

## 集成测试
- [x] C64 T1 ComputeDevice is_gpu
- [x] C65 T2 ComputeDevice n_gpu_layers
- [x] C66 T3 ComputeDevice Default（Cpu）
- [x] C67 T4 Quantization Default（Q4_K_M）
- [x] C68 T5 ModelInfo Default
- [x] C69 T6 InferParams Default
- [x] C70 T7 EngineStats Default
- [x] C71 T8 MockEngine 构造 + 加载 + 推理
- [x] C72 T9 MockEngine 未加载推理返回 Err
- [x] C73 T10 MockEngine 流式推理
- [x] C74 T11 MockEngine 流式 callback 返回 false 停止
- [x] C75 T12 MockEngine 统计累加
- [x] C76 T13 MockEngine health_check
- [x] C77 T14 GPU 优先逻辑（MockEngine::new(Cuda) 时 device=Cuda）
- [x] C78 T15 CPU 降级逻辑（MockEngine::new(Cpu) 时 device=Cpu）

## 设计文档
- [x] C79 `docs/ai/llm-engine-design.md` 存在
- [x] C80 `docs/ai/` 目录存在（若新建）
- [x] C81 文档包含 12 章节
- [x] C82 文档包含 2 Mermaid 图（LlmEngine trait UML + 推理时序图）
- [x] C83 D1~D12 偏差声明表
- [x] C84 文档位置在 `docs/ai/` 下

## 版本号同步
- [x] C85 `Makefile` 版本号 0.58.0 → 0.59.0
- [x] C86 `.github/workflows/ci.yml` 版本号 0.58.0 → 0.59.0
- [x] C87 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-llm-engine` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C88 `cargo metadata --format-version 1` 成功
- [x] C89 `cargo test -p eneros-llm-engine` 全部通过（15 tests）
- [x] C90 `cargo build -p eneros-llm-engine --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C91 `cargo fmt -p eneros-llm-engine -- --check` 格式通过
- [x] C92 `cargo clippy -p eneros-llm-engine --all-targets -- -D warnings` lint 通过
- [x] C93 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C94 llm-engine 在 `crates/ai/` 下（子系统归属正确）
- [x] C95 无外部依赖（纯 no_std + alloc）
- [x] C96 设计文档在 `docs/ai/` 下
- [x] C97 无根目录 crate
- [x] C98 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C99 所有 Rust 代码无 `use std::*`
- [x] C100 不使用 `panic!` / `todo!` / `unimplemented!`（feature-gated 的 LlamaCppEngine 例外，仅在 `#[cfg(feature = "llama-cpp")]` 下可用 `unimplemented!`）
- [x] C101 不要求 `Send + Sync`（D2 trait 无 bound）
- [x] C102 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`

## Karpathy 原则校验
- [x] C103 使用 `alloc::*` 而非 `std::*`（D1）
- [x] C104 trait 无 `Send + Sync` bound（D2）
- [x] C105 MockEngine 默认可用 + LlamaCppEngine feature-gated（D3）
- [x] C106 GPU 优先通过 `n_gpu_layers` 参数（D4，非 PyTorch）
- [x] C107 EngineStats 普通 u64（D5 无 AtomicU64）
- [x] C108 `&str` path 参数保留（D6 no_std 兼容）
- [x] C109 LlmError 完整变体 + Display（D7）
- [x] C110 `infer_stream` 使用 `&mut dyn FnMut`（D8 非 Box<dyn>）
- [x] C111 crate 位置 `crates/ai/llm-engine/`（D9）
- [x] C112 FFI `extern "C"` + `unsafe` 封装 + SAFETY 注释（D10）
- [x] C113 Quantization 默认 Q4_K_M（D11）
- [x] C114 ComputeDevice 默认 Cpu（D12）
