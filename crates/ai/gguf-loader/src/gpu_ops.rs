//! GPU 操作（D3: feature-gated）.
//!
//! 仅在启用 `llama-cpp` feature 时编译。通过 FFI 调用 llama.cpp C 库的
//! GPU 分配/拷贝/释放接口，为模型加载提供 GPU 侧内存管理（D10: 每个
//! `unsafe` 块附 SAFETY 注释，指针所有权明确）。

use crate::error::GgufError;

/// GPU 已分配内存的句柄.
///
/// `ptr` 指向 GPU 内存，由 [`GpuOps::load_to_gpu`] 返回，必须通过
/// [`GpuOps::free_gpu_memory`] 释放。
pub struct GpuHandle {
    /// GPU 内存起始指针.
    pub ptr: *mut u8,
    /// 已分配字节数.
    pub size: usize,
}

/// GPU 操作 trait（D3 / D10）.
pub trait GpuOps {
    /// 将数据加载到 GPU 内存.
    ///
    /// # Safety
    /// 涉及对 llama.cpp C 库的 FFI 调用。
    fn load_to_gpu(&self, data: &[u8]) -> Result<GpuHandle, GgufError>;

    /// 释放 GPU 内存.
    fn free_gpu_memory(&mut self, handle: GpuHandle);
}

// FFI 声明（D10: SAFETY 注释）
extern "C" {
    // SAFETY: 这些是 C 函数签名，与 llama.cpp 的 GPU API 一致。
    fn eneros_gpu_alloc(size: core::ffi::c_uint) -> *mut core::ffi::c_void;
    fn eneros_gpu_free(ptr: *mut core::ffi::c_void);
    fn eneros_gpu_memcpy(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        size: core::ffi::c_uint,
    );
}

/// llama.cpp GPU 操作实现.
pub struct LlamaGpuOps;

impl LlamaGpuOps {
    /// 构造 GPU 操作实例.
    pub fn new() -> Self {
        Self
    }
}

impl Default for LlamaGpuOps {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuOps for LlamaGpuOps {
    fn load_to_gpu(&self, data: &[u8]) -> Result<GpuHandle, GgufError> {
        let size = data.len() as u32;
        // SAFETY: eneros_gpu_alloc 按请求大小分配 GPU 内存；
        // 失败时返回空指针。
        let ptr = unsafe { eneros_gpu_alloc(size) };
        if ptr.is_null() {
            return Err(GgufError::GpuUnavailable);
        }
        // SAFETY: eneros_gpu_memcpy 从 `data.as_ptr()` 拷贝 `size` 字节到 `ptr`；
        // 调用期间两个指针均有效。
        unsafe {
            eneros_gpu_memcpy(ptr, data.as_ptr() as *const _, size);
        }
        Ok(GpuHandle {
            ptr: ptr as *mut u8,
            size: data.len(),
        })
    }

    fn free_gpu_memory(&mut self, handle: GpuHandle) {
        // SAFETY: handle.ptr 由 eneros_gpu_alloc 分配且仍有效；
        // 调用后 GPU 内存被释放，不得再使用。
        unsafe {
            eneros_gpu_free(handle.ptr as *mut _);
        }
    }
}
