//! 引擎统计与健康检查（D5：普通 u64，不使用 AtomicU64）.

use crate::device::ComputeDevice;
use crate::error::LlmError;

/// 引擎累计统计.
///
/// 单线程无需原子操作（D5）。所有字段默认 0。
#[derive(Debug, Clone, Default)]
pub struct EngineStats {
    /// 累计推理次数.
    pub inference_count: u64,
    /// 累计生成 token 数.
    pub total_tokens_generated: u64,
    /// 累计推理耗时（纳秒）.
    pub total_inference_ns: u64,
    /// 最近一次推理耗时（纳秒）.
    pub last_inference_ns: u64,
    /// 累计模型加载次数.
    pub model_load_count: u64,
    /// 当前 GPU offload 层数（llama.cpp `n_gpu_layers`）.
    pub gpu_layers: u32,
}

/// 引擎健康状态.
#[derive(Debug, Clone)]
pub struct EngineHealth {
    /// 模型是否已加载.
    pub loaded: bool,
    /// 当前计算设备.
    pub device: ComputeDevice,
    /// GPU offload 层数.
    pub gpu_layers: u32,
    /// 最近一次错误（若存在）.
    pub last_error: Option<LlmError>,
}
