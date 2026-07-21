//! GGUF 模型加载器（D2 / D4 / D8 / D12）.
//!
//! [`GgufLoader`] 编排"后端读取 → 头部解析 → 元数据解析 → 张量信息解析"
//! 的完整流程，并通过 [`ModelMemoryManager`] 记录内存占用。`Drop` 自动卸载
//! 已加载模型（D8），避免内存泄漏。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_llm_engine::device::ComputeDevice;

use crate::backend::{MemoryBackend, MmapBackend, MmapRegion};
use crate::error::GgufError;
use crate::header::GgufHeader;
use crate::memory::ModelMemoryManager;
use crate::metadata::GgufMetadata;
use crate::tensor::GgufTensorInfo;

/// 已加载到内存的模型.
pub struct LoadedModel {
    /// 模型元数据.
    pub metadata: GgufMetadata,
    /// 张量描述符列表.
    pub tensors: Vec<GgufTensorInfo>,
    /// 模型原始字节区域.
    pub data: MmapRegion,
    /// 目标计算设备.
    pub device: ComputeDevice,
    /// GPU offload 层数（D4）.
    pub n_gpu_layers: u32,
    /// 权重数据段相对文件起始的偏移（字节）.
    pub data_offset: u64,
}

/// GGUF 模型加载器（D2: MmapBackend 抽象，D8: Drop 自动卸载）.
pub struct GgufLoader {
    backend: Box<dyn MmapBackend>,
    loaded: Option<LoadedModel>,
    mem_manager: ModelMemoryManager,
}

impl GgufLoader {
    /// 使用空 [`MemoryBackend`] 构造加载器（D12）.
    pub fn new() -> Self {
        Self {
            backend: Box::new(MemoryBackend::empty()),
            loaded: None,
            mem_manager: ModelMemoryManager::new(),
        }
    }

    /// 使用自定义后端构造加载器.
    pub fn with_backend(backend: Box<dyn MmapBackend>) -> Self {
        Self {
            backend,
            loaded: None,
            mem_manager: ModelMemoryManager::new(),
        }
    }

    /// 从 `path` 加载 GGUF 模型到指定设备，返回解析得到的元数据.
    ///
    /// 流程：后端读取 → 头部解析 → 元数据解析 → 张量信息解析 → 记录内存占用。
    /// 若已有模型加载，返回 [`GgufError::AlreadyLoaded`]。
    pub fn load(&mut self, path: &str, device: ComputeDevice) -> Result<GgufMetadata, GgufError> {
        if self.loaded.is_some() {
            return Err(GgufError::AlreadyLoaded);
        }

        // 1. 从后端获取字节
        let region = self.backend.map(path)?;
        let bytes = region.as_bytes();

        // 2. 解析头部
        let (header, header_end) = GgufHeader::parse(bytes)?;

        // 3. 解析元数据
        let (metadata, meta_consumed) =
            GgufMetadata::parse(bytes, header_end, header.metadata_kv_count)?;

        // 4. 解析张量信息
        let (tensors, tensors_consumed) =
            GgufTensorInfo::parse(bytes, header_end + meta_consumed, header.tensor_count)?;

        // 5. 计算 n_gpu_layers（D4）
        let n_gpu_layers = device.n_gpu_layers();

        // 6. 记录内存占用
        let data_len = bytes.len() as u64;
        self.mem_manager.record_load(device, data_len);

        // 7. 保存已加载模型
        let loaded = LoadedModel {
            metadata: metadata.clone(),
            tensors,
            data: region,
            device,
            n_gpu_layers,
            data_offset: (header_end + meta_consumed + tensors_consumed) as u64,
        };
        self.loaded = Some(loaded);

        Ok(metadata)
    }

    /// 卸载当前已加载的模型.
    ///
    /// 若无已加载模型，返回 [`GgufError::NotLoaded`]。
    pub fn unload(&mut self) -> Result<(), GgufError> {
        let loaded = self.loaded.take().ok_or(GgufError::NotLoaded)?;
        self.mem_manager
            .record_unload(loaded.device, loaded.data.len() as u64);
        Ok(())
    }

    /// 返回已加载模型的不可变引用（若存在）.
    pub fn loaded_model(&self) -> Option<&LoadedModel> {
        self.loaded.as_ref()
    }

    /// 返回内存统计的不可变引用.
    pub fn memory_stats(&self) -> &crate::memory::MemoryStats {
        self.mem_manager.stats()
    }
}

impl Default for GgufLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// D8: Drop 时自动卸载已加载模型，避免内存泄漏.
impl Drop for GgufLoader {
    fn drop(&mut self) {
        if self.loaded.is_some() {
            let _ = self.unload();
        }
    }
}
