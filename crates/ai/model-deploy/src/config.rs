//! 7B INT4 量化模型部署配置（D11：复用 v0.59.0 类型）.
//!
//! [`QuantConfig7B`] 描述 7B Q4_K_M 量化模型的部署参数：模型名称、路径、
//! 量化方式、文件大小、最小 RAM/VRAM、上下文长度、计算设备、推理参数。
//! 默认值面向能源场景（低温度 0.1 保证输出确定性，长上下文 4096）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_llm_engine::{ComputeDevice, InferParams, Quantization};

/// 7B INT4 量化模型部署配置.
///
/// 默认模型 `Qwen2.5-7B`，量化 `Q4_K_M`，上下文 4096，CPU 设备。
/// 推理参数温度 0.1（能源场景需确定性输出，非通用对话的 0.7）。
#[derive(Debug, Clone)]
pub struct QuantConfig7B {
    /// 模型名称.
    pub model_name: String,
    /// 模型文件路径（GGUF 格式）.
    pub model_path: String,
    /// 量化方式（默认 Q4_K_M，D11 复用 v0.59.0）.
    pub quantization: Quantization,
    /// 模型文件大小（GB）.
    pub file_size_gb: f64,
    /// 最小系统内存（GB）.
    pub min_ram_gb: f64,
    /// 最小显存（GB，GPU 设备时检查）.
    pub min_vram_gb: f64,
    /// 上下文长度（默认 4096）.
    pub context_length: u32,
    /// 目标计算设备（默认 Cpu，D4 GPU 优先是 opt-in）.
    pub device: ComputeDevice,
    /// 推理参数（默认低温度，适配能源场景）.
    pub infer_params: InferParams,
}

impl Default for QuantConfig7B {
    fn default() -> Self {
        Self {
            model_name: String::from("Qwen2.5-7B"),
            model_path: String::from("models/Qwen2.5-7B-Q4_K_M.gguf"),
            quantization: Quantization::Q4_K_M,
            file_size_gb: 4.0,
            min_ram_gb: 8.0,
            min_vram_gb: 6.0,
            context_length: 4096,
            device: ComputeDevice::Cpu,
            infer_params: InferParams {
                max_tokens: 512,
                temperature: 0.1,
                top_p: 0.9,
                top_k: 40,
                repeat_penalty: 1.1,
                stop_tokens: Vec::new(),
            },
        }
    }
}

impl QuantConfig7B {
    /// 创建指定模型名称的配置（其余字段为默认值）.
    pub fn new(model_name: &str) -> Self {
        Self {
            model_name: String::from(model_name),
            ..Default::default()
        }
    }

    /// builder：设置计算设备（D4 GPU 优先 opt-in）.
    pub fn with_device(mut self, device: ComputeDevice) -> Self {
        self.device = device;
        self
    }

    /// builder：设置模型路径.
    pub fn with_path(mut self, path: &str) -> Self {
        self.model_path = String::from(path);
        self
    }
}
