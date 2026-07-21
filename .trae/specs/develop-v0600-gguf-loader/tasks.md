# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.59.0` → `0.60.0`
  - [x] members 添加 `crates/ai/gguf-loader`
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-gguf-loader` crate 骨架
  - [x] 新建 `crates/ai/gguf-loader/Cargo.toml`，package name = `eneros-gguf-loader`
  - [x] dependencies 添加 `eneros-llm-engine = { path = "../llm-engine" }`（D11 复用 v0.59.0 类型）
  - [x] features 声明：`[features] llama-cpp = []`（默认关闭，D3）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / dtype / value / header / metadata / tensor / backend / loader / memory / gpu_ops
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — GgufError 错误类型
  - [x] `GgufError` 枚举：InvalidMagic / InvalidVersion(u32) / TruncatedFile / InvalidValueType(u32) / InvalidDtype(u32) / BackendError / GpuUnavailable / AlreadyLoaded / NotLoaded / Utf8Error
  - [x] 派生 `Debug` + `Clone`，实现 `core::fmt::Display`
  - [x] 实现 `From<core::str::Utf8Error>` 转换
  - [x] 验证：`cargo build -p eneros-gguf-loader` 通过

- [x] Task 4: 实现 `dtype.rs` — GgufDtype 张量数据类型
  - [x] `GgufDtype` 枚举：F32 / F16 / Q4_0 / Q4_1 / Q5_0 / Q5_1 / Q8_0 / Q8_1 / Q2_K / Q3_K / Q4_K / Q5_K / Q6_K / Q8_K
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq`
  - [x] `from_u32(value: u32) -> Option<GgufDtype>` 方法（GGUF dtype 整数→枚举）
  - [x] `to_quantization(&self) -> Option<Quantization>` 方法（映射到 v0.59.0 Quantization，D11）
  - [x] 验证：单元测试 — from_u32 / to_quantization 映射

- [x] Task 5: 实现 `value.rs` — GgufValueType + GgufValue 元数据值
  - [x] `GgufValueType` 枚举：Uint8 / Int8 / Uint16 / Int16 / Uint32 / Int32 / Float32 / Bool / String / Array / Uint64 / Int64 / Float64
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq`
  - [x] `from_u32(value: u32) -> Option<GgufValueType>` 方法
  - [x] `GgufValue` 枚举：Uint8(u8) / Int8(i8) / ... / String(String) / Array(Vec<GgufValue>)
  - [x] 派生 `Debug / Clone`
  - [x] 验证：单元测试 — from_u32 / GgufValue 构造

- [x] Task 6: 实现 `header.rs` — GgufHeader 文件头
  - [x] `GGUF_MAGIC: u32 = 0x46554747` 常量
  - [x] `GgufHeader` 结构体：magic: u32 / version: u32 / tensor_count: u64 / metadata_kv_count: u64
  - [x] 派生 `Debug / Clone / Copy`
  - [x] `parse(bytes: &[u8]) -> Result<(GgufHeader, usize), GgufError>` 方法（解析前 28 字节，返回 header + 已消费字节数）
  - [x] 验证：单元测试 — 有效头 / 无效魔数 / 截断文件

- [x] Task 7: 实现 `metadata.rs` — GgufMetadata 模型元数据
  - [x] `GgufMetadata` 结构体：name / architecture / context_length / embedding_length / block_count / head_count / head_count_kv / quantization: Quantization（D11 复用 v0.59.0）
  - [x] 派生 `Debug / Clone`
  - [x] `parse(bytes: &[u8], offset: usize, kv_count: u64) -> Result<(GgufMetadata, usize), GgufError>` 方法（解析 KV 对，返回 metadata + 已消费字节数）
  - [x] 内部辅助：parse_string / parse_value / parse_array
  - [x] 验证：单元测试 — 解析完整元数据 / 缺失字段默认值

- [x] Task 8: 实现 `tensor.rs` — GgufTensorInfo 张量信息
  - [x] `GgufTensorInfo` 结构体：name: String / dimensions: Vec<u32> / dtype: GgufDtype / offset: u64
  - [x] 派生 `Debug / Clone`
  - [x] `parse(bytes: &[u8], offset: usize, tensor_count: u64) -> Result<(Vec<GgufTensorInfo>, usize), GgufError>` 方法
  - [x] 验证：单元测试 — 解析张量列表 / 无效 dtype

- [x] Task 9: 实现 `backend.rs` — MmapBackend trait + MemoryBackend（D2/D12）
  - [x] `MmapRegion` 结构体：封装 `Vec<u8>`，提供 `as_ptr() / len() / as_bytes()`
  - [x] `MmapBackend` trait：`fn map(&self, path: &str) -> Result<MmapRegion, GgufError>`
  - [x] `MemoryBackend` 结构体：`data: Option<Vec<u8>>`
  - [x] `MemoryBackend::new(data: Vec<u8>) -> Self`（预加载数据）
  - [x] `MemoryBackend::empty() -> Self`（空，load 返回 Err）
  - [x] 实现 `MmapBackend` for `MemoryBackend`
  - [x] 验证：单元测试 — new 返回数据 / empty 返回错误

- [x] Task 10: 实现 `memory.rs` — ModelMemoryManager 内存统计
  - [x] `MemoryStats` 结构体：cpu_bytes: u64 / gpu_bytes: u64 / model_count: u32（D5 普通 u64/u32）
  - [x] 派生 `Debug / Clone / Default`
  - [x] `ModelMemoryManager` 结构体：stats: MemoryStats
  - [x] `ModelMemoryManager::new() -> Self`
  - [x] `record_load(device: ComputeDevice, bytes: u64)` — 更新统计
  - [x] `record_unload(device: ComputeDevice, bytes: u64)` — 更新统计
  - [x] `stats() -> &MemoryStats`
  - [x] 验证：单元测试 — load/unload 累加 / stats 查询

- [x] Task 11: 实现 `loader.rs` — GgufLoader 主加载器
  - [x] `LoadedModel` 结构体：metadata: GgufMetadata / tensors: Vec<GgufTensorInfo> / data: MmapRegion / device: ComputeDevice / n_gpu_layers: u32 / data_offset: u64
  - [x] `GgufLoader` 结构体：backend: Box<dyn MmapBackend> / loaded: Option<LoadedModel> / mem_manager: ModelMemoryManager
  - [x] `GgufLoader::new() -> Self`（带 MemoryBackend::empty()）
  - [x] `GgufLoader::with_backend(backend: Box<dyn MmapBackend>) -> Self`
  - [x] `load(&mut self, path: &str, device: ComputeDevice) -> Result<GgufMetadata, GgufError>` — 完整加载流程
  - [x] `unload(&mut self) -> Result<(), GgufError>` — 释放模型
  - [x] `loaded_model(&self) -> Option<&LoadedModel>` — 查询已加载模型
  - [x] `memory_stats(&self) -> &MemoryStats` — 查询内存统计
  - [x] 实现 `Drop`：自动 unload（D8）
  - [x] 验证：单元测试 — 完整加载流程 / 重复加载错误 / 未加载卸载错误 / Drop 自动清理

- [x] Task 12: 实现 `gpu_ops.rs` — GpuOps trait（D3 feature-gated）
  - [x] `#[cfg(feature = "llama-cpp")]` 门控整个模块
  - [x] `GpuHandle` 结构体：ptr: *mut u8 / size: usize
  - [x] `GpuOps` trait：`fn load_to_gpu(&self, data: &[u8]) -> Result<GpuHandle, GgufError>` / `fn free_gpu_memory(&mut self, handle: GpuHandle)`
  - [x] FFI 声明：`extern "C"` GPU 内存分配/释放函数（D10 SAFETY 注释）
  - [x] `LlamaGpuOps` 结构体实现 `GpuOps` trait
  - [x] 验证：默认 feature 下不编译（`cargo build` 不报错）

- [x] Task 13: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 GgufHeader 解析有效头（magic=0x46554747）
  - [x] T2 GgufHeader 无效魔数返回 InvalidMagic
  - [x] T3 GgufHeader 截断文件返回 TruncatedFile
  - [x] T4 GgufDtype from_u32 映射（Q4_K=12）
  - [x] T5 GgufDtype to_quantization（Q4_K → Q4_K_M）
  - [x] T6 GgufValueType from_u32 映射（String=8）
  - [x] T7 MemoryBackend new 返回数据
  - [x] T8 MemoryBackend empty 返回 BackendError
  - [x] T9 ModelMemoryManager record_load CPU 累加
  - [x] T10 ModelMemoryManager record_load GPU 累加
  - [x] T11 ModelMemoryManager record_unload 递减
  - [x] T12 GgufLoader 完整加载流程（构造 GGUF 字节流 → load → 验证 metadata）
  - [x] T13 GgufLoader 重复加载返回 AlreadyLoaded
  - [x] T14 GgufLoader unload 后再加载成功
  - [x] T15 GgufLoader Drop 自动清理（Drop 后 loaded 为 None）
  - [x] 验证：`cargo test -p eneros-gguf-loader` 全部通过

- [x] Task 14: 设计文档 `docs/ai/gguf-loader-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / GGUF 格式 / GgufLoader / MmapBackend / ModelMemoryManager / GgufDtype 映射 / 内存管理 / GPU 策略 / 错误处理 / feature 门控 / 偏差声明
  - [x] 2 Mermaid 图：GgufLoader 类图 + 加载时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0 创建的目录）

- [x] Task 15: 版本号同步 + gate.rs 注释更新 + 构建校验
  - [x] `Makefile` 版本号 `0.59.0` → `0.60.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.59.0` → `0.60.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-gguf-loader` 说明
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-gguf-loader` 全部通过（15 tests）
  - [x] `cargo build -p eneros-gguf-loader --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-gguf-loader -- --check` 格式通过
  - [x] `cargo clippy -p eneros-gguf-loader --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~10 → Task 2（各模块依赖 crate 骨架）
- Task 3（error）→ Task 4~8（各解析模块返回 GgufError）
- Task 4（dtype）→ Task 8（tensor 使用 GgufDtype）
- Task 5（value）→ Task 7（metadata 使用 GgufValue）
- Task 6（header）→ Task 7（metadata 从 header 后开始解析）
- Task 7（metadata）→ Task 8（tensor 从 metadata 后开始解析）
- Task 9（backend）→ Task 11（loader 使用 MmapBackend）
- Task 10（memory）→ Task 11（loader 使用 ModelMemoryManager）
- Task 11（loader）→ Task 12（gpu_ops 独立，feature-gated）
- Task 13 → Task 3~12（集成测试依赖所有模块）
- Task 14 → Task 13（文档在测试通过后撰写）
- Task 15 → Task 14（版本同步与校验在功能完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（dtype）+ Task 5（value）+ Task 6（header）可并行（无依赖）
- Task 7（metadata）依赖 Task 5 + Task 6
- Task 8（tensor）依赖 Task 4 + Task 7
- Task 9（backend）+ Task 10（memory）可并行（独立）
- Task 11（loader）依赖 Task 3~10 全部
- Task 12（gpu_ops）独立，feature-gated
- Task 13 → Task 11, 12
