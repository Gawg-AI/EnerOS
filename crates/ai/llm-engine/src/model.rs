//! 模型元数据与量化方式（D11）.

use crate::device::ComputeDevice;

/// 模型量化方式.
///
/// 默认 `Q4_K_M`（k-quant 4-bit medium，边缘推理推荐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(non_camel_case_types)]
pub enum Quantization {
    /// 16-bit 浮点（精度高、体积大，仅供测试）.
    F16,
    /// 8-bit 量化（平衡精度与体积）.
    Q8_0,
    /// 4-bit 量化（基础版）.
    Q4_0,
    /// k-quant 4-bit medium（推荐，D11 默认）.
    #[default]
    Q4_K_M,
}

/// 模型元数据.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// 模型名称（或路径）.
    pub name: alloc::string::String,
    /// 模型文件大小（字节）.
    pub size_bytes: u64,
    /// 量化方式（默认 Q4_K_M）.
    pub quantization: Quantization,
    /// 上下文长度（默认 2048）.
    pub context_length: u32,
    /// 目标计算设备（默认 Cpu）.
    pub device: ComputeDevice,
}

impl Default for ModelInfo {
    fn default() -> Self {
        Self {
            name: alloc::string::String::new(),
            size_bytes: 0,
            quantization: Quantization::Q4_K_M,
            context_length: 2048,
            device: ComputeDevice::Cpu,
        }
    }
}
