//! EnerOS GGUF 模型加载与内存管理（v0.60.0）.
//!
//! 本 crate 解析 GGUF（GPT-Generated Unified Format）二进制文件，为
//! v0.61.0+ 的模型量化与 v0.59.0 的 [`eneros_llm_engine`] 推理引擎提供
//! 模型加载基础。双脑架构（LLM + Solver）的 LLM 是"感知者"，本 crate
//! 负责"把权重安全地装进内存"这一步。
//!
//! # 核心类型
//!
//! - [`loader::GgufLoader`] — 模型加载器（D2 后端抽象 + D8 Drop 自动卸载）
//! - [`loader::LoadedModel`] — 已加载模型（元数据 + 张量描述符 + 原始字节）
//! - [`header::GgufHeader`] — 文件头（24 字节定长）
//! - [`metadata::GgufMetadata`] — 元数据（已知键映射，D11 默认 Q4_K_M）
//! - [`tensor::GgufTensorInfo`] — 张量描述符
//! - [`dtype::GgufDtype`] — 张量数据类型（映射到 v0.59.0 `Quantization`）
//! - [`value::GgufValueType`] / [`value::GgufValue`] — 元数据值类型与值
//! - [`backend::MmapBackend`] / [`backend::MemoryBackend`] — 内存后端（D2/D12）
//! - [`memory::ModelMemoryManager`] / [`memory::MemoryStats`] — 内存管理（D5）
//! - [`error::GgufError`] — 错误类型（D7）
//! - `gpu_ops::GpuOps` — GPU 操作（D3 feature-gated，启用 `llama-cpp`）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | no_std 合规：`alloc::string::String` / `alloc::vec::Vec` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明；禁用 `panic!` / `todo!` / `unimplemented!` |
//! | **D2** | no_std RTOS 无 `mmap`，用 `MmapBackend` trait + `MmapRegion`（`Vec<u8>` 载体）抽象文件读取；`MemoryBackend` 为默认实现 |
//! | **D3** | `gpu_ops` 模块（`GpuOps` trait + `LlamaGpuOps`）通过 `#[cfg(feature = "llama-cpp")]` 门控；`Cargo.toml` 声明 `[features] llama-cpp = []`（默认关闭） |
//! | **D4** | GPU 优先通过 `ComputeDevice::n_gpu_layers`（Cpu=0，Cuda/Metal/Npu=99 全 offload），复用 v0.59.0 设备枚举，非 PyTorch |
//! | **D5** | `MemoryStats` / `ModelMemoryManager` 用普通 `u64`/`u32`，不使用原子类型（单线程无需）；卸载用 `saturating_sub` 防下溢 |
//! | **D6** | `load_model(path: &str)` 保留 `&str` 签名（no_std 兼容）；GGUF 字符串用 `core::str::from_utf8` 解码 |
//! | **D7** | `GgufError` 10 变体（InvalidMagic / InvalidVersion / TruncatedFile / InvalidValueType / InvalidDtype / BackendError / GpuUnavailable / AlreadyLoaded / NotLoaded / Utf8Error）；派生 `Debug`/`Clone`，实现 `core::fmt::Display` |
//! | **D8** | `GgufLoader` 实现 `Drop`，自动卸载已加载模型（调用 `unload`），避免内存泄漏 |
//! | **D9** | crate 位置 `crates/ai/gguf-loader/`（AI 子系统；项目规则 §2.3.1） |
//! | **D10** | FFI 集中封装于 `gpu_ops` 模块；每个 `unsafe` 块附 SAFETY 注释；`GpuHandle.ptr` 所有权明确（`load_to_gpu` 分配，`free_gpu_memory` 释放） |
//! | **D11** | `GgufMetadata::quantization` 默认 `Q4_K_M`（复用 v0.59.0 `Quantization`，不重定义）；`GgufDtype::to_quantization` 仅 4 种类型有映射 |
//! | **D12** | `GgufLoader::new()` 默认空 `MemoryBackend`（GPU 优先是 opt-in）；`MemoryBackend::empty()` 用于无数据场景 |
//!
//! # 依赖关系（D11）
//!
//! 复用 v0.59.0 `eneros-llm-engine` 的类型，不重定义：
//! - `eneros_llm_engine::device::ComputeDevice`（Cpu / Cuda / Metal / Npu）
//! - `eneros_llm_engine::model::Quantization`（F16 / Q8_0 / Q4_0 / Q4_K_M）
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
pub mod dtype;
pub mod error;
pub mod header;
pub mod loader;
pub mod memory;
pub mod metadata;
pub mod tensor;
pub mod value;

#[cfg(feature = "llama-cpp")]
pub mod gpu_ops;

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T15（覆盖 D1~D12 偏差声明与 GGUF 解析全流程）.

    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use eneros_llm_engine::device::ComputeDevice;
    use eneros_llm_engine::model::Quantization;

    use crate::backend::{MemoryBackend, MmapBackend};
    use crate::dtype::GgufDtype;
    use crate::error::GgufError;
    use crate::header::GgufHeader;
    use crate::loader::GgufLoader;
    use crate::memory::ModelMemoryManager;
    use crate::value::GgufValueType;

    /// 构造最小可用 GGUF 字节流（header + 2 metadata KV + 1 tensor info）.
    fn build_minimal_gguf() -> Vec<u8> {
        let mut bytes = Vec::new();

        // Header
        bytes.extend_from_slice(&0x46554747u32.to_le_bytes()); // magic
        bytes.extend_from_slice(&3u32.to_le_bytes()); // version
        bytes.extend_from_slice(&1u64.to_le_bytes()); // tensor_count
        bytes.extend_from_slice(&2u64.to_le_bytes()); // metadata_kv_count

        // Metadata KV 1: "name" -> "test_model"
        let key1 = b"name";
        bytes.extend_from_slice(&(key1.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key1);
        bytes.extend_from_slice(&8u32.to_le_bytes()); // String type
        let val1 = b"test_model";
        bytes.extend_from_slice(&(val1.len() as u64).to_le_bytes());
        bytes.extend_from_slice(val1);

        // Metadata KV 2: "architecture" -> "llama"
        let key2 = b"architecture";
        bytes.extend_from_slice(&(key2.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key2);
        bytes.extend_from_slice(&8u32.to_le_bytes()); // String type
        let val2 = b"llama";
        bytes.extend_from_slice(&(val2.len() as u64).to_le_bytes());
        bytes.extend_from_slice(val2);

        // Tensor info 1: "token_embd.weight", 1 dim [4096], dtype Q4_K (12), offset 0
        let tname = b"token_embd.weight";
        bytes.extend_from_slice(&(tname.len() as u64).to_le_bytes());
        bytes.extend_from_slice(tname);
        bytes.extend_from_slice(&1u32.to_le_bytes()); // n_dims = 1
        bytes.extend_from_slice(&4096u64.to_le_bytes()); // dim[0]
        bytes.extend_from_slice(&12u32.to_le_bytes()); // dtype = Q4_K
        bytes.extend_from_slice(&0u64.to_le_bytes()); // offset

        bytes
    }

    // ===== T1：GgufHeader 解析有效头部 =====
    #[test]
    fn test_t1_header_parse_valid() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x46554747u32.to_le_bytes()); // magic
        bytes.extend_from_slice(&3u32.to_le_bytes()); // version
        bytes.extend_from_slice(&10u64.to_le_bytes()); // tensor_count
        bytes.extend_from_slice(&5u64.to_le_bytes()); // metadata_kv_count

        let (header, consumed) = GgufHeader::parse(&bytes).expect("parse should succeed");
        assert_eq!(header.magic, 0x46554747);
        assert_eq!(header.version, 3);
        assert_eq!(header.tensor_count, 10);
        assert_eq!(header.metadata_kv_count, 5);
        assert_eq!(consumed, 24);
    }

    // ===== T2：GgufHeader 魔数无效返回 InvalidMagic =====
    #[test]
    fn test_t2_header_invalid_magic() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x12345678u32.to_le_bytes()); // 错误魔数
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&10u64.to_le_bytes());
        bytes.extend_from_slice(&5u64.to_le_bytes());

        let r = GgufHeader::parse(&bytes);
        assert!(matches!(r, Err(GgufError::InvalidMagic)));
    }

    // ===== T3：GgufHeader 文件截断返回 TruncatedFile =====
    #[test]
    fn test_t3_header_truncated() {
        let bytes = [0u8; 10]; // 不足 24 字节
        let r = GgufHeader::parse(&bytes);
        assert!(matches!(r, Err(GgufError::TruncatedFile)));
    }

    // ===== T4：GgufDtype::from_u32 映射 =====
    #[test]
    fn test_t4_dtype_from_u32() {
        assert_eq!(GgufDtype::from_u32(12), Some(GgufDtype::Q4_K));
        assert_eq!(GgufDtype::from_u32(0), Some(GgufDtype::F32));
        assert_eq!(GgufDtype::from_u32(1), Some(GgufDtype::F16));
        assert_eq!(GgufDtype::from_u32(8), Some(GgufDtype::Q8_0));
        assert_eq!(GgufDtype::from_u32(99), None); // 非法值
    }

    // ===== T5：GgufDtype::to_quantization 映射（D11）=====
    #[test]
    fn test_t5_dtype_to_quantization() {
        assert_eq!(
            GgufDtype::Q4_K.to_quantization(),
            Some(Quantization::Q4_K_M)
        );
        assert_eq!(GgufDtype::F32.to_quantization(), None);
        assert_eq!(GgufDtype::F16.to_quantization(), Some(Quantization::F16));
        assert_eq!(GgufDtype::Q8_0.to_quantization(), Some(Quantization::Q8_0));
        assert_eq!(GgufDtype::Q4_0.to_quantization(), Some(Quantization::Q4_0));
        // 无映射的类型
        assert_eq!(GgufDtype::Q5_0.to_quantization(), None);
        assert_eq!(GgufDtype::Q6_K.to_quantization(), None);
    }

    // ===== T6：GgufValueType::from_u32 映射 =====
    #[test]
    fn test_t6_value_type_from_u32() {
        assert_eq!(GgufValueType::from_u32(8), Some(GgufValueType::String));
        assert_eq!(GgufValueType::from_u32(9), Some(GgufValueType::Array));
        assert_eq!(GgufValueType::from_u32(0), Some(GgufValueType::Uint8));
        assert_eq!(GgufValueType::from_u32(12), Some(GgufValueType::Float64));
        assert_eq!(GgufValueType::from_u32(99), None); // 非法值
    }

    // ===== T7：MemoryBackend::new 返回数据 =====
    #[test]
    fn test_t7_memory_backend_new_returns_data() {
        let backend = MemoryBackend::new(alloc::vec![1, 2, 3]);
        let region = backend.map("test").expect("map should succeed");
        assert_eq!(region.len(), 3);
        assert_eq!(region.as_bytes(), &[1, 2, 3]);
    }

    // ===== T8：MemoryBackend::empty 返回 BackendError =====
    #[test]
    fn test_t8_memory_backend_empty_returns_error() {
        let backend = MemoryBackend::empty();
        let r = backend.map("test");
        assert!(matches!(r, Err(GgufError::BackendError)));
    }

    // ===== T9：ModelMemoryManager record_load CPU（D5）=====
    #[test]
    fn test_t9_memory_manager_record_load_cpu() {
        let mut mgr = ModelMemoryManager::new();
        mgr.record_load(ComputeDevice::Cpu, 1000);
        let stats = mgr.stats();
        assert_eq!(stats.cpu_bytes, 1000);
        assert_eq!(stats.gpu_bytes, 0);
        assert_eq!(stats.model_count, 1);
    }

    // ===== T10：ModelMemoryManager record_load GPU（D5）=====
    #[test]
    fn test_t10_memory_manager_record_load_gpu() {
        let mut mgr = ModelMemoryManager::new();
        mgr.record_load(ComputeDevice::Cuda, 4000);
        let stats = mgr.stats();
        assert_eq!(stats.cpu_bytes, 0);
        assert_eq!(stats.gpu_bytes, 4000);
        assert_eq!(stats.model_count, 1);
    }

    // ===== T11：ModelMemoryManager record_unload（D5 saturating）=====
    #[test]
    fn test_t11_memory_manager_record_unload() {
        let mut mgr = ModelMemoryManager::new();
        mgr.record_load(ComputeDevice::Cpu, 1000);
        mgr.record_unload(ComputeDevice::Cpu, 500);
        let stats = mgr.stats();
        assert_eq!(stats.cpu_bytes, 500);
        assert_eq!(stats.model_count, 0);
    }

    // ===== T12：GgufLoader 完整加载流程 =====
    #[test]
    fn test_t12_loader_complete_load_flow() {
        let data = build_minimal_gguf();
        let backend = MemoryBackend::new(data);
        let mut loader = GgufLoader::with_backend(Box::new(backend));

        let metadata = loader
            .load("path", ComputeDevice::Cpu)
            .expect("load should succeed");
        assert_eq!(metadata.name, "test_model");
        assert_eq!(metadata.architecture, "llama");

        // 已加载模型可访问
        let loaded = loader.loaded_model().expect("model should be loaded");
        assert_eq!(loaded.tensors.len(), 1);
        assert_eq!(loaded.tensors[0].name, "token_embd.weight");
        assert_eq!(loaded.tensors[0].dimensions, alloc::vec![4096]);
        assert_eq!(loaded.tensors[0].dtype, GgufDtype::Q4_K);
        assert_eq!(loaded.n_gpu_layers, 0); // Cpu → 0
        assert_eq!(loaded.device, ComputeDevice::Cpu);

        // 内存统计已记录
        assert_eq!(loader.memory_stats().model_count, 1);
        assert!(loader.memory_stats().cpu_bytes > 0);
    }

    // ===== T13：GgufLoader 重复加载返回 AlreadyLoaded =====
    #[test]
    fn test_t13_loader_already_loaded() {
        let data = build_minimal_gguf();
        let backend = MemoryBackend::new(data);
        let mut loader = GgufLoader::with_backend(Box::new(backend));

        loader
            .load("path", ComputeDevice::Cpu)
            .expect("first load should succeed");

        // 再次加载应失败
        let r = loader.load("path2", ComputeDevice::Cpu);
        assert!(matches!(r, Err(GgufError::AlreadyLoaded)));
    }

    // ===== T14：GgufLoader 卸载后可重新加载 =====
    #[test]
    fn test_t14_loader_unload_then_reload() {
        let data = build_minimal_gguf();
        let backend = MemoryBackend::new(data);
        let mut loader = GgufLoader::with_backend(Box::new(backend));

        loader
            .load("path", ComputeDevice::Cpu)
            .expect("first load should succeed");

        // 卸载
        loader.unload().expect("unload should succeed");
        assert!(loader.loaded_model().is_none());
        assert_eq!(loader.memory_stats().model_count, 0);

        // 重新加载
        let r = loader.load("path", ComputeDevice::Cpu);
        assert!(r.is_ok());
        assert_eq!(loader.memory_stats().model_count, 1);
    }

    // ===== T15：GgufLoader Drop 自动卸载（D8，无 double-free）=====
    #[test]
    fn test_t15_loader_drop_auto_cleanup() {
        let data = build_minimal_gguf();
        let backend = MemoryBackend::new(data);

        // 在块作用域内加载，块结束时 Drop 自动卸载
        let stats_before_drop = {
            let mut loader = GgufLoader::with_backend(Box::new(backend));
            loader
                .load("path", ComputeDevice::Cpu)
                .expect("load should succeed");
            // 确认已加载
            assert!(loader.loaded_model().is_some());
            // 记录卸载前的统计快照（Drop 后 loader 不可访问，此处仅确认状态正常）
            let stats = loader.memory_stats().clone();
            assert_eq!(stats.model_count, 1);
            stats
        };
        // 块结束：Drop 触发 unload，无 panic 即通过
        // 快照仍可访问（仅用于确认 Drop 前状态一致）
        assert_eq!(stats_before_drop.model_count, 1);
    }
}
