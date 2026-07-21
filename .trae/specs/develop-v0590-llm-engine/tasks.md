# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.58.0` → `0.59.0`
  - [x] members 添加 `crates/ai/llm-engine`
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-llm-engine` crate 骨架
  - [x] 新建 `crates/ai/llm-engine/Cargo.toml`，package name = `eneros-llm-engine`
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] features 声明：`[features] llama-cpp = []`（默认关闭，D3）
  - [x] dependencies：无外部依赖（纯 no_std + alloc）；dev-dependencies 可选
  - [x] 新建 `src/lib.rs`，模块声明：error / params / model / device / stats / engine / mock / ffi
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — LlmError 错误类型
  - [x] `LlmError` 枚举：LoadFailed / InferFailed / InvalidPath / InvalidPrompt / Utf8Error / GpuUnavailable / ModelNotLoaded / OutOfMemory
  - [x] 派生 `Debug`，实现 `core::fmt::Display`
  - [x] 验证：`cargo build -p eneros-llm-engine` 通过

- [x] Task 4: 实现 `device.rs` — ComputeDevice
  - [x] `ComputeDevice` 枚举：Cpu / Cuda / Metal / Npu（D12）
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq / Default`
  - [x] `#[default]` 标注 `Cpu`
  - [x] `is_gpu(&self) -> bool` 方法（Cpu=false，其余=true）
  - [x] `n_gpu_layers(&self) -> u32` 方法（Cpu=0，其余=99，D4）
  - [x] 验证：单元测试 — is_gpu / n_gpu_layers

- [x] Task 5: 实现 `model.rs` — ModelInfo + Quantization
  - [x] `Quantization` 枚举：F16 / Q8_0 / Q4_0 / Q4_K_M（D11）
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq / Default`，`#[default]` 标注 `Q4_K_M`
  - [x] `ModelInfo` 结构体：name: String / size_bytes: u64 / quantization: Quantization / context_length: u32 / device: ComputeDevice
  - [x] 派生 `Debug / Clone / Default`
  - [x] 验证：单元测试 — 默认值

- [x] Task 6: 实现 `params.rs` — InferParams
  - [x] `InferParams` 结构体：max_tokens: u32 / temperature: f32 / top_p: f32 / top_k: u32 / repeat_penalty: f32 / stop_tokens: Vec<String>
  - [x] 派生 `Debug / Clone / Default`
  - [x] Default：max_tokens=128, temperature=0.7, top_p=0.9, top_k=40, repeat_penalty=1.1, stop_tokens=Vec::new()
  - [x] 验证：单元测试 — 默认值

- [x] Task 7: 实现 `stats.rs` — EngineStats + EngineHealth
  - [x] `EngineStats` 结构体：inference_count / total_tokens_generated / total_inference_ns / last_inference_ns / model_load_count / gpu_layers（D5 普通 u64/u32）
  - [x] 派生 `Debug / Clone / Default`
  - [x] `EngineHealth` 结构体：loaded: bool / device: ComputeDevice / gpu_layers: u32 / last_error: Option<LlmError>
  - [x] 派生 `Debug / Clone`
  - [x] 验证：单元测试 — 累加

- [x] Task 8: 实现 `engine.rs` — LlmEngine trait（D2 无 Send + Sync）
  - [x] `LlmEngine` trait 定义（D2：无 Send + Sync bound）
  - [x] 方法：`load_model(&mut self, path: &str) -> Result<(), LlmError>`
  - [x] 方法：`infer(&mut self, prompt: &str, params: &InferParams) -> Result<String, LlmError>`
  - [x] 方法：`infer_stream(&mut self, prompt: &str, params: &InferParams, callback: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError>`
  - [x] 方法：`model_info(&self) -> Option<&ModelInfo>`
  - [x] 方法：`health_check(&self) -> EngineHealth`
  - [x] 方法：`stats(&self) -> &EngineStats`
  - [x] 验证：编译通过（trait 定义）

- [x] Task 9: 实现 `mock.rs` — MockEngine（D3 默认可用）
  - [x] `MockEngine` 结构体：loaded: bool / device: ComputeDevice / model_info: Option<ModelInfo> / stats: EngineStats / mock_output: String
  - [x] `new(device: ComputeDevice) -> Self`（初始 loaded=false，mock_output="mock response"）
  - [x] `with_output(output: &str) -> Self`（builder 设置 mock 输出）
  - [x] 实现 `LlmEngine` trait 全部 6 个方法
  - [x] load_model：设 loaded=true，model_info=Some(ModelInfo { name: path.to_string(), ... })
  - [x] infer：若 loaded=false 返回 Err(ModelNotLoaded)；否则返回 Ok(format!("mock: <prompt>"))，stats 更新
  - [x] infer_stream：按字符切分 mock 输出，逐字符调用 callback
  - [x] 验证：单元测试 — 构造 / 加载 / 推理 / 未加载推理 / 流式 / 统计

- [x] Task 10: 实现 `ffi.rs` — llama.cpp FFI 绑定（D3 feature-gated）
  - [x] `#[cfg(feature = "llama-cpp")]` 门控整个模块
  - [x] `extern "C"` 声明：llama_init / llama_load_model / llama_infer / llama_free_result / llama_free / llama_set_device（D10）
  - [x] 使用 `core::ffi::*` 类型（c_void / c_char / c_int / c_uint / c_float）
  - [x] 验证：默认 feature 下不编译（`cargo build` 不报错）

- [x] Task 11: 实现 `llama_cpp.rs` — LlamaCppEngine（D3 feature-gated）
  - [x] `#[cfg(feature = "llama-cpp")]` 门控整个模块
  - [x] `LlamaCppEngine` 结构体：ctx: *mut c_void / model_info: Option<ModelInfo> / device: ComputeDevice / stats: EngineStats
  - [x] `new(device: ComputeDevice) -> Self`（调用 ffi::llama_init()）
  - [x] 实现 `LlmEngine` trait
  - [x] load_model：CString::new(path) → ffi::llama_load_model → 设置 device → 更新 model_info
  - [x] infer：CString::new(prompt) → ffi::llama_infer → CStr::to_str → String → llama_free_result → 统计更新
  - [x] infer_stream：循环调用 ffi::llama_infer（单 token 模式）+ callback
  - [x] 实现 `Drop`：调用 ffi::llama_free(ctx)
  - [x] 验证：默认 feature 下不编译（`cargo build` 不报错）

- [x] Task 12: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 ComputeDevice is_gpu（Cpu=false，Cuda/Metal/Npu=true）
  - [x] T2 ComputeDevice n_gpu_layers（Cpu=0，其余=99）
  - [x] T3 ComputeDevice Default（Cpu）
  - [x] T4 Quantization Default（Q4_K_M）
  - [x] T5 ModelInfo Default（name=空，quantization=Q4_K_M，device=Cpu）
  - [x] T6 InferParams Default（max_tokens=128, temperature=0.7, ...）
  - [x] T7 EngineStats Default（全 0）
  - [x] T8 MockEngine 构造 + 加载 + 推理
  - [x] T9 MockEngine 未加载模型推理返回 Err(ModelNotLoaded)
  - [x] T10 MockEngine 流式推理（callback 累计字符）
  - [x] T11 MockEngine 流式推理 callback 返回 false 停止
  - [x] T12 MockEngine 统计累加（inference_count / total_tokens_generated）
  - [x] T13 MockEngine health_check（loaded=true, device=Cpu）
  - [x] T14 GPU 优先逻辑测试（MockEngine::new(Cuda) 时 device=Cuda，is_gpu=true）
  - [x] T15 CPU 降级逻辑测试（MockEngine::new(Cpu) 时 device=Cpu，is_gpu=false）
  - [x] 验证：`cargo test -p eneros-llm-engine` 全部通过

- [x] Task 13: 设计文档 `docs/ai/llm-engine-design.md`
  - [x] 新建 `docs/ai/` 目录（若不存在）
  - [x] 12 章节：版本目标 / 架构定位 / LlmEngine trait / 类型定义 / MockEngine / LlamaCppEngine FFI / GPU 优先策略 / 错误处理 / 统计与可观测 / 内存管理 / feature 门控 / 偏差声明
  - [x] 2 Mermaid 图：LlmEngine trait UML 类图 + 推理时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（符合目录规范）

- [x] Task 14: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.58.0` → `0.59.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.58.0` → `0.59.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-llm-engine` 说明
  - [x] 验证：`cargo build -p eneros-llm-engine` 通过

- [x] Task 15: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-llm-engine` 全部通过（15 tests）
  - [x] `cargo build -p eneros-llm-engine --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-llm-engine -- --check` 格式通过
  - [x] `cargo clippy -p eneros-llm-engine --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~9 → Task 2（各模块依赖 crate 骨架）
- Task 10（ffi）独立，feature-gated
- Task 11（LlamaCppEngine）依赖 Task 8（trait）+ Task 10（ffi）
- Task 12 → Task 8, 9（集成测试依赖 trait + MockEngine）
- Task 13 → Task 12（文档在测试通过后撰写）
- Task 14 → Task 13（版本同步在功能完成后）
- Task 15 → Task 14（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（device）+ Task 5（model）+ Task 6（params）+ Task 7（stats）可并行
- Task 8（trait）依赖 Task 3 + 5 + 6 + 7
- Task 9（MockEngine）依赖 Task 8
- Task 10（ffi）独立
- Task 11（LlamaCppEngine）依赖 Task 8 + 10
- Task 12 → Task 9, 11
