//! 计算设备枚举（D4 / D12）.
//!
//! `ComputeDevice` 声明推理目标设备，通过 `n_gpu_layers` 映射到 llama.cpp
//! 的 `n_gpu_layers` 参数（Cpu=0，其余=99 全 offload）。Rust 侧不直接调用
//! PyTorch / CUDA，GPU 加速由 llama.cpp C 库内部实现（D4）。

/// 计算设备.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComputeDevice {
    /// CPU 推理（默认，D12）.
    #[default]
    Cpu,
    /// NVIDIA CUDA GPU.
    Cuda,
    /// Apple Metal GPU.
    Metal,
    /// 神经网络处理器（NPU）.
    Npu,
}

impl ComputeDevice {
    /// 是否为 GPU 类设备（Cuda / Metal / Npu）.
    ///
    /// Cpu 返回 `false`，其余返回 `true`。
    pub fn is_gpu(&self) -> bool {
        !matches!(self, ComputeDevice::Cpu)
    }

    /// llama.cpp `n_gpu_layers` 参数（D4）.
    ///
    /// - Cpu → 0（纯 CPU 推理）
    /// - Cuda / Metal / Npu → 99（全 offload 到 GPU）
    pub fn n_gpu_layers(&self) -> u32 {
        match self {
            ComputeDevice::Cpu => 0,
            _ => 99,
        }
    }
}
