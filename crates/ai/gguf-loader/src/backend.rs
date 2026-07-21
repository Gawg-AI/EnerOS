//! 内存后端抽象（D2 / D12）.
//!
//! no_std RTOS 无 `mmap` 系统调用，本模块用 `Vec<u8>` 作为后端数据载体。
//! [`MmapBackend`] trait 抽象"按路径加载文件内容到内存"，[`MemoryBackend`]
//! 为默认实现（D12：测试/默认用例使用预加载数据）。

use alloc::vec::Vec;

use crate::error::GgufError;

/// 从后端加载的一段字节区域（包装 `Vec<u8>`，D2）.
pub struct MmapRegion {
    data: Vec<u8>,
}

impl MmapRegion {
    /// 从已有 `Vec<u8>` 构造区域.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// 返回数据起始指针.
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// 返回数据长度（字节）.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 返回数据字节切片.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }
}

/// 文件内容加载后端 trait（D2）.
///
/// no_std RTOS 无 mmap；用 `Vec<u8>` 载体替代。实现方负责按 `path`
/// 读取数据并返回 [`MmapRegion`]。
pub trait MmapBackend {
    /// 按 `path` 加载文件内容到内存区域.
    fn map(&self, path: &str) -> Result<MmapRegion, GgufError>;
}

/// 内存后端：使用预加载数据（D12，测试/默认用例）.
pub struct MemoryBackend {
    data: Option<Vec<u8>>,
}

impl MemoryBackend {
    /// 使用预加载数据构造后端.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data: Some(data) }
    }

    /// 构造空后端（`map` 将返回 `BackendError`）.
    pub fn empty() -> Self {
        Self { data: None }
    }
}

impl MmapBackend for MemoryBackend {
    fn map(&self, _path: &str) -> Result<MmapRegion, GgufError> {
        match &self.data {
            Some(d) => Ok(MmapRegion::new(d.clone())),
            None => Err(GgufError::BackendError),
        }
    }
}
