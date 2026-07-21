# v0.60.0 模型加载与内存管理 Spec

## Why

v0.59.0 定义了 `LlmEngine` trait 与 `MockEngine`/`LlamaCppEngine`，但尚未实现 GGUF 模型文件的解析与加载。本版本实现 GGUF（GPT-Generated Unified Format）二进制格式解析、模型权重内存管理、按设备加载（CPU RAM / GPU VRAM）与卸载机制，为 v0.61.0（7B INT4 量化部署）提供模型生命周期基础。

## What Changes

- 新增 crate `eneros-gguf-loader`（位置：`crates/ai/gguf-loader/`，子系统：ai）
- 新增 `GgufLoader` — GGUF 文件解析与模型加载器
- 新增 `MmapBackend` trait — 文件→字节缓冲抽象（D2：no_std 无 mmap，用 `Vec<u8>` 后端）
- 新增 `MemoryBackend` — 默认可用的内存后端（测试与无文件系统场景，D12）
- 新增 `ModelMemoryManager` — 内存使用统计与跟踪（CPU/GPU 字节计数）
- 新增 GGUF 类型：`GgufHeader` / `GgufMetadata` / `GgufTensorInfo` / `GgufDtype` / `GgufValueType` / `GgufValue`
- 新增 `LoadedModel` — 已加载模型的运行时表示
- 新增 `GgufError` — GGUF 解析/加载错误类型
- 新增 `GpuOps` trait — GPU 操作抽象（feature-gated，D3）
- 复用 v0.59.0 类型：`ComputeDevice` / `Quantization` / `LlmError`（D11，不重定义）
- 设计文档 `docs/ai/gguf-loader-design.md`

## Impact

- Affected specs: v0.59.0（依赖其 `ComputeDevice` / `Quantization` / `LlmError` 类型）
- Affected code: 新增 `crates/ai/gguf-loader/`；根 `Cargo.toml` members 新增条目
- **无破坏性改动**：本版本仅新增 crate，不修改 v0.59.0 既有代码

## 偏差声明（D1~D12）

> 依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 no_std / 项目约束的偏差。

### D1：no_std 合规 — `alloc::*` 替代 `std::*`

**蓝图**：`pub struct GgufMetadata { name: String, ... }` / `pub struct GgufTensorInfo { dimensions: Vec<u32>, ... }`，隐含 `std::string::String` 与 `std::vec::Vec`。

**实际**：本项目所有 Rust 代码必须 no_std（蓝图 §43.1，覆盖全项目）。

**决策**：使用 `alloc::string::String` 与 `alloc::vec::Vec`。`lib.rs` 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）。

### D2：`MmapBackend` trait 抽象 — no_std 无 mmap 系统调用

**蓝图**：`MmapBackend` 使用真实 mmap 系统调用，返回 `MmapRegion`（含 `as_ptr()` / `len()`）。

**实际**：
1. no_std RTOS 环境无 mmap 系统调用（无 Linux 内核或 POSIX 层）。
2. 即便有文件系统（v0.24.0 littlefs2），也是 `read` 而非 `mmap`。
3. 测试在 host 侧运行（有 std），但 crate 本身必须 no_std。

**决策**：
- 定义 `MmapBackend` trait：`fn map(&self, path: &str) -> Result<MmapRegion, GgufError>`
- `MmapRegion` 封装 `Vec<u8>`（owned 字节缓冲），提供 `as_ptr()` / `len()` / `as_bytes()`
- `MemoryBackend`（默认可用）— 包装预加载的 `Vec<u8>`，忽略 path 参数，直接返回缓冲（用于测试）
- 真实文件读取后端（`FileBackend`）延后至 v0.24.0 集成版本，本版本不实现
- **理由**：保证 crate 在默认配置下可编译、可测试、可交叉编译

### D3：Feature gating for GPU FFI — `GpuOps` trait + `llama-cpp` feature

**蓝图**：`load_to_gpu(&mmap, &tensors)` / `free_gpu_memory(data_ptr, data_len)` 直接调用 GPU 操作。

**实际**：
1. GPU 显存分配/释放是 llama.cpp C 库的 FFI 调用，需要 C++ 库链接。
2. CI 环境（无 GPU、无 CUDA toolkit）无法编译 C++ 库。
3. 与 v0.59.0 D3 一致，需要 feature gating。

**决策**：
- 定义 `GpuOps` trait（GPU 操作抽象）：`fn load_to_gpu(&self, data: &[u8]) -> Result<GpuHandle, GgufError>` / `fn free_gpu_memory(&mut self, handle: GpuHandle)`
- `GpuOps` 模块通过 `#[cfg(feature = "llama-cpp")]` 门控
- 默认配置下 `GgufLoader` 仅在 CPU 侧保存模型数据，GPU offload 延后至推理时由 v0.59.0 `LlamaCppEngine` 的 `n_gpu_layers` 参数处理
- `Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭，与 v0.59.0 一致）

### D4：GPU 优先通过 `n_gpu_layers` 跟踪（非 PyTorch）

**蓝图**：§43.3 已明确"边缘 LLM 推理统一采用 llama.cpp（C API），禁止在边缘侧使用 PyTorch"。

**实际**：本 crate 是 Rust no_std，无 PyTorch 依赖。GPU 加速通过 llama.cpp 的 `n_gpu_layers` 参数控制（C 库内部实现）。`GgufLoader` 仅记录目标设备与建议的 `n_gpu_layers` 值，实际 GPU offload 由 v0.59.0 `LlamaCppEngine` 在推理时执行。

**决策**：
- `LoadedModel` 记录 `device: ComputeDevice`（来自 v0.59.0）与 `n_gpu_layers: u32`（Cpu=0，其余=99）
- `GgufLoader::load(path, device)` 接受 `ComputeDevice` 参数，记录到 `LoadedModel`
- 不在本 crate 执行实际 GPU 数据传输（那是 llama.cpp C 库的职责）

### D5：普通 `u64` 统计，不用 `AtomicU64`

**蓝图**：未定义统计类型，但 `ModelMemoryManager` 暗示需要内存使用统计。

**实际**：与 v0.56.0 D7 / v0.57.0 D7 / v0.58.0 D4 / v0.59.0 D5 一致，单线程无需原子操作。

**决策**：`MemoryStats { cpu_bytes: u64, gpu_bytes: u64, model_count: u32 }`，全部普通 `u64`/`u32`，派生 `Default`。

### D6：`&str` path 参数保留

**蓝图**：`fn load(&mut self, path: &str, device: ComputeDevice)`。

**实际**：`&str` 在 no_std 下可用（`core::str`）。模型路径来自配置（v0.26.0）。

**决策**：保留 `&str` 签名。`MemoryBackend` 忽略 path（返回预加载数据）；真实后端使用 path 打开文件。

### D7：`GgufError` 错误类型 — 完整 GGUF 解析错误变体

**蓝图**：未定义 `GgufError` 变体。

**实际**：GGUF 解析有多种失败模式，需要完整错误类型。

**决策**：`GgufError` 枚举：
- `InvalidMagic` — 文件头魔数不匹配（非 0x46554747）
- `InvalidVersion(u32)` — 不支持的版本号
- `TruncatedFile` — 文件意外截断
- `InvalidValueType(u32)` — 未知元数据值类型
- `InvalidDtype(u32)` — 未知张量数据类型
- `BackendError` — 后端读取失败
- `GpuUnavailable` — GPU 不可用
- `AlreadyLoaded` — 已有模型加载中
- `NotLoaded` — 无模型已加载
- `Utf8Error` — 字符串解码失败

派生 `Debug` + `Clone`，实现 `core::fmt::Display`。提供 `From<LlmError>` 转换（复用 v0.59.0 错误）。

### D8：`Drop` for `GgufLoader` — 自动卸载

**蓝图**：`unload()` 手动释放。

**实际**：Rust 惯例是 `Drop` 自动清理，避免内存泄漏。

**决策**：
- `GgufLoader` 实现 `Drop`：若 `loaded` 存在，自动调用 `unload()` 逻辑（释放 `LoadedModel`）
- `unload()` 仍保留为公开方法，供手动显式卸载
- GPU 内存释放（若 feature 启用）在 `Drop` 中调用 `GpuOps::free_gpu_memory`

### D9：crate 位置 `crates/ai/gguf-loader/`

**蓝图**：`gguf-loader` crate。

**实际**：项目规则 §2.3.1 要求所有 crate 放入 `crates/<subsystem>/`。GGUF 加载器属于 AI 子系统。

**决策**：crate 路径 `crates/ai/gguf-loader/`，package name `eneros-gguf-loader`。跨 crate 引用 v0.59.0：`path = "../llm-engine"`。

### D10：FFI 安全 — SAFETY 注释 + `core::ffi::*`

**蓝图**：FFI 调用无安全说明。

**实际**：Rust FFI 规范要求 `unsafe` 块附 SAFETY 注释，使用 `core::ffi::*` 类型。

**决策**：`GpuOps` FFI 模块（feature-gated）中：
- 所有 `extern "C"` 使用 `core::ffi::*` 类型（`c_void` / `c_int` / `c_uint`）
- 每个 `unsafe` 块附 `// SAFETY: ...` 注释
- 指针所有权明确（`GpuHandle` 持有 GPU 内存句柄，`Drop` 调用 `free_gpu_memory`）

### D11：复用 v0.59.0 类型 — 不重定义 `ComputeDevice` / `Quantization`

**蓝图**：`GgufMetadata { quantization: String }` / `LoadedModel { device: ComputeDevice }`。

**实际**：v0.59.0 已定义 `ComputeDevice`（Cpu/Cuda/Metal/Npu）与 `Quantization`（F16/Q8_0/Q4_0/Q4_K_M）。重定义会导致类型不兼容。

**决策**：
- `eneros-gguf-loader` 依赖 `eneros-llm-engine`（`path = "../llm-engine"`）
- `GgufMetadata` 使用 `quantization: Quantization`（来自 v0.59.0），而非 `String`
- `LoadedModel` 使用 `device: ComputeDevice`（来自 v0.59.0）
- 新增 `GgufDtype` 枚举（GGUF 原始张量数据类型，比 `Quantization` 粒度更细）+ `to_quantization() -> Option<Quantization>` 映射方法
- `GgufValue` 元数据值类型使用 `GgufValueType` 枚举（UINT8/INT8/.../ARRAY），独立于 `Quantization`

### D12：`MemoryBackend` 作为默认后端

**蓝图**：`MmapBackend` 为默认实现，假设有 mmap。

**实际**：no_std 无 mmap，需要默认可用的测试后端。

**决策**：
- `GgufLoader::new()` 创建带 `MemoryBackend::empty()` 的加载器（初始无数据）
- `GgufLoader::with_backend(backend)` 接受自定义后端
- `MemoryBackend::new(data: Vec<u8>)` 创建预加载数据后端
- `MemoryBackend::empty()` 创建空后端（`load` 返回 `Err(BackendError)`）
- 测试时构造完整 GGUF 字节流，注入 `MemoryBackend`，验证解析

## ADDED Requirements

### Requirement: GGUF 文件解析

系统 SHALL 提供 GGUF 二进制格式解析能力，包括文件头（magic/version/tensor_count/metadata_kv_count）、元数据键值对（key/value_type/value）、张量信息（name/dimensions/dtype/offset）。

#### Scenario: 解析有效的 GGUF 文件头
- **WHEN** `GgufLoader::load(path, device)` 被调用且后端返回有效 GGUF 字节流
- **THEN** 解析 magic=0x46554747、version、tensor_count、metadata_kv_count
- **AND** 返回 `Ok(GgufMetadata)` 包含模型名称、架构、上下文长度、层数等

#### Scenario: 解析无效魔数
- **WHEN** 后端返回的字节流前 4 字节非 0x46554747
- **THEN** 返回 `Err(GgufError::InvalidMagic)`

#### Scenario: 解析截断文件
- **WHEN** 字节流长度不足以容纳完整文件头
- **THEN** 返回 `Err(GgufError::TruncatedFile)`

### Requirement: 模型加载与卸载

系统 SHALL 提供模型加载（解析 GGUF + 保存到内存）与卸载（释放内存）能力，支持 CPU RAM 与 GPU VRAM 两种目标设备。

#### Scenario: 加载模型到 CPU
- **WHEN** `GgufLoader::load(path, ComputeDevice::Cpu)` 被调用且后端返回有效字节流
- **THEN** 解析 GGUF 元数据与张量信息
- **AND** 保存 `LoadedModel { device: Cpu, n_gpu_layers: 0, ... }`
- **AND** 返回 `Ok(GgufMetadata)`

#### Scenario: 加载模型到 GPU
- **WHEN** `GgufLoader::load(path, ComputeDevice::Cuda)` 被调用
- **THEN** 记录 `LoadedModel { device: Cuda, n_gpu_layers: 99, ... }`
- **AND** 实际 GPU 数据传输由 v0.59.0 `LlamaCppEngine` 在推理时执行（D4）

#### Scenario: 卸载已加载模型
- **WHEN** `GgufLoader::unload()` 被调用且已有模型加载
- **THEN** 释放 `LoadedModel`（`Vec<u8>` 自动 Drop）
- **AND** 更新 `ModelMemoryManager` 统计

#### Scenario: 重复加载
- **WHEN** `GgufLoader::load(path, device)` 被调用但已有模型加载中
- **THEN** 返回 `Err(GgufError::AlreadyLoaded)`

### Requirement: `MmapBackend` 抽象

系统 SHALL 提供 `MmapBackend` trait 抽象文件→字节缓冲的读取操作，支持 `MemoryBackend`（默认，预加载数据）作为测试后端。

#### Scenario: MemoryBackend 返回预加载数据
- **WHEN** `MemoryBackend::new(data)` 创建后 `map(any_path)` 被调用
- **THEN** 返回 `Ok(MmapRegion)` 包装 `data`

#### Scenario: 空 MemoryBackend 返回错误
- **WHEN** `MemoryBackend::empty()` 创建后 `map(any_path)` 被调用
- **THEN** 返回 `Err(GgufError::BackendError)`

### Requirement: `ModelMemoryManager` 内存统计

系统 SHALL 提供 `ModelMemoryManager` 跟踪 CPU RAM 与 GPU VRAM 使用量，支持加载/卸载时更新统计。

#### Scenario: 记录模型加载
- **WHEN** `ModelMemoryManager::record_load(device, bytes)` 被调用
- **THEN** 根据 device 更新 `cpu_bytes` 或 `gpu_bytes`，`model_count += 1`

#### Scenario: 记录模型卸载
- **WHEN** `ModelMemoryManager::record_unload(device, bytes)` 被调用
- **THEN** 对应 `cpu_bytes` 或 `gpu_bytes` 减少，`model_count -= 1`

#### Scenario: 查询内存使用
- **WHEN** `ModelMemoryManager::stats()` 被调用
- **THEN** 返回 `MemoryStats { cpu_bytes, gpu_bytes, model_count }`

### Requirement: `GgufDtype` 张量数据类型

系统 SHALL 提供 `GgufDtype` 枚举表示 GGUF 张量数据类型（F32/F16/Q4_0/Q4_1/Q5_0/Q5_1/Q8_0/Q8_1/Q2_K/Q3_K/Q4_K/Q5_K/Q6_K/Q8_K），并提供到 v0.59.0 `Quantization` 的映射。

#### Scenario: Q4_K_M 映射
- **WHEN** `GgufDtype::Q4_K.to_quantization()` 被调用
- **THEN** 返回 `Some(Quantization::Q4_K_M)`

#### Scenario: F32 无映射
- **WHEN** `GgufDtype::F32.to_quantization()` 被调用
- **THEN** 返回 `None`（v0.59.0 无 F32 变体）
