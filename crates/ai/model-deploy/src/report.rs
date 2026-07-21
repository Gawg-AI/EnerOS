//! 部署验证报告（D5：普通 u64，不使用 AtomicU64）.
//!
//! [`DeployReport`] 记录一次部署验证的全部指标：设备、GPU offload 层数、
//! 加载耗时、推理次数、token 总量、推理总耗时、平均 tokens/sec、是否通过、
//! 失败列表。单线程下无需原子操作（D5）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_llm_engine::ComputeDevice;

use crate::error::DeployError;

/// 部署验证失败项.
#[derive(Debug, Clone)]
pub struct DeployFailure {
    /// 触发失败的提示词.
    pub prompt: String,
    /// 失败原因.
    pub error: DeployError,
}

/// 部署验证报告.
///
/// 由 [`crate::verifier::DeployVerifier::deploy`] 生成，记录硬件检查、模型加载、
/// 推理验证全流程指标。
#[derive(Debug, Clone)]
pub struct DeployReport {
    /// 计算设备.
    pub device: ComputeDevice,
    /// GPU offload 层数（D4：Cpu=0，其余=99）.
    pub n_gpu_layers: u32,
    /// 模型加载耗时（纳秒）.
    pub load_time_ns: u64,
    /// 推理次数.
    pub inference_count: u64,
    /// 生成 token 总量.
    pub total_tokens: u64,
    /// 推理总耗时（纳秒）.
    pub total_inference_ns: u64,
    /// 平均 tokens/sec（由 `finalize` 计算）.
    pub avg_tokens_per_sec: f64,
    /// 是否通过（无失败项即通过）.
    pub passed: bool,
    /// 失败项列表.
    pub failures: Vec<DeployFailure>,
}

impl DeployReport {
    /// 创建初始报告（passed=true，failures 为空）.
    pub fn new(device: ComputeDevice, n_gpu_layers: u32) -> Self {
        Self {
            device,
            n_gpu_layers,
            load_time_ns: 0,
            inference_count: 0,
            total_tokens: 0,
            total_inference_ns: 0,
            avg_tokens_per_sec: 0.0,
            passed: true,
            failures: Vec::new(),
        }
    }

    /// 记录模型加载耗时.
    pub fn record_load_time(&mut self, ns: u64) {
        self.load_time_ns = ns;
    }

    /// 记录一次推理的 token 数与耗时.
    pub fn record_inference(&mut self, tokens: u64, ns: u64) {
        self.inference_count += 1;
        self.total_tokens += tokens;
        self.total_inference_ns += ns;
    }

    /// 添加失败项（同时将 `passed` 置为 `false`）.
    pub fn add_failure(&mut self, prompt: String, error: DeployError) {
        self.passed = false;
        self.failures.push(DeployFailure { prompt, error });
    }

    /// 结束统计，计算 `avg_tokens_per_sec`.
    ///
    /// 总耗时为 0 时不计算（保持 0.0，避免除零）。
    pub fn finalize(&mut self) {
        if self.total_inference_ns > 0 {
            self.avg_tokens_per_sec =
                (self.total_tokens as f64) * 1_000_000_000.0 / (self.total_inference_ns as f64);
        }
    }
}
