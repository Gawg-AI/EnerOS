# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.60.0`
- [x] C2 members 列表已添加 `crates/ai/gguf-loader`
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/gguf-loader/Cargo.toml` 存在，package name = `eneros-gguf-loader`
- [x] C5 dependencies 包含 `eneros-llm-engine = { path = "../llm-engine" }`（D11 复用 v0.59.0 类型）
- [x] C6 `[features] llama-cpp = []` 声明（D3，默认关闭）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / dtype / value / header / metadata / tensor / backend / loader / memory / gpu_ops

## error.rs — GgufError
- [x] C10 `GgufError` 枚举包含 10 变体（InvalidMagic / InvalidVersion / TruncatedFile / InvalidValueType / InvalidDtype / BackendError / GpuUnavailable / AlreadyLoaded / NotLoaded / Utf8Error）
- [x] C11 派生 `Debug` + `Clone`
- [x] C12 实现 `core::fmt::Display`
- [x] C13 实现 `From<core::str::Utf8Error>`

## dtype.rs — GgufDtype
- [x] C14 `GgufDtype` 枚举包含 14 变体（F32 / F16 / Q4_0 / Q4_1 / Q5_0 / Q5_1 / Q8_0 / Q8_1 / Q2_K / Q3_K / Q4_K / Q5_K / Q6_K / Q8_K）
- [x] C15 派生 `Debug / Clone / Copy / PartialEq / Eq`
- [x] C16 `from_u32(value: u32) -> Option<GgufDtype>` 方法实现
- [x] C17 `to_quantization(&self) -> Option<Quantization>` 方法实现（D11 映射到 v0.59.0）
- [x] C18 单元测试：from_u32 映射 + to_quantization 映射（Q4_K → Q4_K_M）

## value.rs — GgufValueType + GgufValue
- [x] C19 `GgufValueType` 枚举包含 13 变体（Uint8~Float64 + String + Array）
- [x] C20 派生 `Debug / Clone / Copy / PartialEq / Eq`
- [x] C21 `from_u32(value: u32) -> Option<GgufValueType>` 方法实现
- [x] C22 `GgufValue` 枚举包含所有标量变体 + String + Array
- [x] C23 派生 `Debug / Clone`
- [x] C24 单元测试：from_u32 映射

## header.rs — GgufHeader
- [x] C25 `GGUF_MAGIC: u32 = 0x46554747` 常量
- [x] C26 `GgufHeader` 结构体：magic / version / tensor_count / metadata_kv_count
- [x] C27 派生 `Debug / Clone / Copy`
- [x] C28 `parse(bytes: &[u8]) -> Result<(GgufHeader, usize), GgufError>` 方法实现
- [x] C29 单元测试：有效头 / 无效魔数 / 截断文件

## metadata.rs — GgufMetadata
- [x] C30 `GgufMetadata` 结构体：name / architecture / context_length / embedding_length / block_count / head_count / head_count_kv / quantization: Quantization（D11）
- [x] C31 派生 `Debug / Clone`
- [x] C32 `parse(bytes: &[u8], offset: usize, kv_count: u64) -> Result<(GgufMetadata, usize), GgufError>` 方法实现
- [x] C33 内部辅助：parse_string / parse_value / parse_array
- [x] C34 单元测试：解析完整元数据 / 缺失字段默认值

## tensor.rs — GgufTensorInfo
- [x] C35 `GgufTensorInfo` 结构体：name / dimensions / dtype / offset
- [x] C36 派生 `Debug / Clone`
- [x] C37 `parse(bytes: &[u8], offset: usize, tensor_count: u64) -> Result<(Vec<GgufTensorInfo>, usize), GgufError>` 方法实现
- [x] C38 单元测试：解析张量列表 / 无效 dtype

## backend.rs — MmapBackend + MemoryBackend
- [x] C39 `MmapRegion` 结构体封装 `Vec<u8>`，提供 `as_ptr() / len() / as_bytes()`
- [x] C40 `MmapBackend` trait 定义：`fn map(&self, path: &str) -> Result<MmapRegion, GgufError>`
- [x] C41 `MemoryBackend` 结构体：`data: Option<Vec<u8>>`
- [x] C42 `MemoryBackend::new(data: Vec<u8>) -> Self`
- [x] C43 `MemoryBackend::empty() -> Self`
- [x] C44 实现 `MmapBackend` for `MemoryBackend`
- [x] C45 单元测试：new 返回数据 / empty 返回错误

## memory.rs — ModelMemoryManager
- [x] C46 `MemoryStats` 结构体：cpu_bytes: u64 / gpu_bytes: u64 / model_count: u32（D5）
- [x] C47 派生 `Debug / Clone / Default`
- [x] C48 `ModelMemoryManager` 结构体：stats: MemoryStats
- [x] C49 `ModelMemoryManager::new() -> Self`
- [x] C50 `record_load(device: ComputeDevice, bytes: u64)` 方法实现
- [x] C51 `record_unload(device: ComputeDevice, bytes: u64)` 方法实现
- [x] C52 `stats() -> &MemoryStats` 方法实现
- [x] C53 单元测试：load/unload 累加 / stats 查询

## loader.rs — GgufLoader
- [x] C54 `LoadedModel` 结构体：metadata / tensors / data: MmapRegion / device: ComputeDevice / n_gpu_layers: u32 / data_offset: u64
- [x] C55 `GgufLoader` 结构体：backend / loaded / mem_manager
- [x] C56 `GgufLoader::new() -> Self`（带 MemoryBackend::empty()）
- [x] C57 `GgufLoader::with_backend(backend: Box<dyn MmapBackend>) -> Self`
- [x] C58 `load(&mut self, path: &str, device: ComputeDevice) -> Result<GgufMetadata, GgufError>` 完整加载流程
- [x] C59 `unload(&mut self) -> Result<(), GgufError>` 释放模型
- [x] C60 `loaded_model(&self) -> Option<&LoadedModel>` 查询
- [x] C61 `memory_stats(&self) -> &MemoryStats` 查询
- [x] C62 实现 `Drop`（D8 自动 unload）
- [x] C63 单元测试：完整加载 / 重复加载错误 / 未加载卸载错误 / Drop 清理

## gpu_ops.rs — GpuOps（feature-gated）
- [x] C64 `#[cfg(feature = "llama-cpp")]` 门控整个模块
- [x] C65 `GpuHandle` 结构体：ptr: *mut u8 / size: usize
- [x] C66 `GpuOps` trait 定义
- [x] C67 FFI 声明使用 `core::ffi::*` 类型 + SAFETY 注释（D10）
- [x] C68 `LlamaGpuOps` 实现 `GpuOps` trait
- [x] C69 默认 feature 下不编译（`cargo build` 不报错）

## 集成测试（lib.rs）
- [x] C70 T1 GgufHeader 解析有效头
- [x] C71 T2 GgufHeader 无效魔数返回 InvalidMagic
- [x] C72 T3 GgufHeader 截断文件返回 TruncatedFile
- [x] C73 T4 GgufDtype from_u32 映射
- [x] C74 T5 GgufDtype to_quantization 映射
- [x] C75 T6 GgufValueType from_u32 映射
- [x] C76 T7 MemoryBackend new 返回数据
- [x] C77 T8 MemoryBackend empty 返回错误
- [x] C78 T9 ModelMemoryManager record_load CPU 累加
- [x] C79 T10 ModelMemoryManager record_load GPU 累加
- [x] C80 T11 ModelMemoryManager record_unload 递减
- [x] C81 T12 GgufLoader 完整加载流程
- [x] C82 T13 GgufLoader 重复加载返回 AlreadyLoaded
- [x] C83 T14 GgufLoader unload 后再加载成功
- [x] C84 T15 GgufLoader Drop 自动清理
- [x] C85 `cargo test -p eneros-gguf-loader` 15/15 通过

## 设计文档
- [x] C86 `docs/ai/gguf-loader-design.md` 存在
- [x] C87 12 章节完整
- [x] C88 2 Mermaid 图（GgufLoader 类图 + 加载时序图）
- [x] C89 D1~D12 偏差声明表
- [x] C90 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C91 `Makefile` 版本号 `0.60.0`
- [x] C92 `.github/workflows/ci.yml` 版本号 `0.60.0`
- [x] C93 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-gguf-loader`

## 构建校验（§2.4.2 C6~C11）
- [x] C94 `cargo metadata --format-version 1` 成功
- [x] C95 `cargo test -p eneros-gguf-loader` 全部通过（15 tests）
- [x] C96 `cargo build -p eneros-gguf-loader --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C97 `cargo fmt -p eneros-gguf-loader -- --check` 通过
- [x] C98 `cargo clippy -p eneros-gguf-loader --all-targets -- -D warnings` 无 warning
- [x] C99 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C100 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C101 无 `panic!` / `todo!` / `unimplemented!`
- [x] C102 子模块不重复 `#![cfg_attr(not(test), no_std)]`

## 目录规范
- [x] C103 crate 在 `crates/ai/gguf-loader/`（D9）
- [x] C104 跨 crate path 引用 `../llm-engine`（相对路径）
- [x] C105 文档在 `docs/ai/` 下
- [x] C106 无根目录 crate（除 `ci/`）
- [x] C107 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## FFI 安全（D10）
- [x] C108 FFI 模块 `#[cfg(feature = "llama-cpp")]` 门控
- [x] C109 所有 `unsafe` 块附 SAFETY 注释
- [x] C110 使用 `core::ffi::*` 类型（c_void / c_int 等）
- [x] C111 指针所有权明确（GpuHandle Drop 调用 free）
