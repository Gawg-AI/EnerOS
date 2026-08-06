//! 部署验证器（D2：泛型 `DeployVerifier<E: LlmEngine>`，不绑定具体引擎）.
//!
//! [`DeployVerifier`] 编排硬件检查 → 模型加载 → 多提示词推理验证全流程，
//! 生成 [`crate::report::DeployReport`]。泛型 `E: LlmEngine` 允许注入
//! [`eneros_llm_engine::MockEngine`]（测试）或 `LlamaCppEngine`（生产）。
//!
//! D8：`DeployVerifier` **不**实现 `Drop`，部署/卸载由调用方显式调用
//! [`DeployVerifier::deploy`] / [`DeployVerifier::undeploy`]。

use alloc::boxed::Box;

use eneros_llm_engine::LlmEngine;

use crate::backend::DeployBackend;
use crate::config::QuantConfig7B;
use crate::error::DeployError;
use crate::mock_backend::MockDeployBackend;
use crate::prompts::PowerPromptSet;
use crate::report::DeployReport;

/// 部署验证器.
///
/// 泛型 `E: LlmEngine` 解耦具体引擎实现（D2）。默认使用
/// [`MockDeployBackend`]（D12），可通过 [`DeployVerifier::with_backend`] 注入
/// 自定义后端（如 `LlamaDeployBackend`）。
pub struct DeployVerifier<E: LlmEngine> {
    /// 推理引擎.
    pub engine: E,
    /// 部署配置.
    pub config: QuantConfig7B,
    /// 部署后端（trait object，D2 抽象）.
    pub backend: Box<dyn DeployBackend>,
    /// 提示词集合.
    pub prompts: PowerPromptSet,
    /// 部署状态（ARC-006：undeploy 真实化）。
    deployed: bool,
}

impl<E: LlmEngine> DeployVerifier<E> {
    /// 创建验证器（默认 MockDeployBackend，D12）.
    pub fn new(engine: E, config: QuantConfig7B) -> Self {
        Self {
            engine,
            config,
            backend: Box::new(MockDeployBackend::new()),
            prompts: PowerPromptSet::default(),
            deployed: false,
        }
    }

    /// 创建验证器并注入自定义后端.
    pub fn with_backend(engine: E, config: QuantConfig7B, backend: Box<dyn DeployBackend>) -> Self {
        Self {
            engine,
            config,
            backend,
            prompts: PowerPromptSet::default(),
            deployed: false,
        }
    }

    /// 执行部署验证.
    ///
    /// 流程：
    /// 1. 硬件检查（`check_hardware`）；
    /// 2. 模型加载（`load_model`），记录加载耗时；
    /// 3. 逐条运行提示词推理，记录 token 数与耗时（取自引擎 `stats` 差值），
    ///    校验结果，失败项记入报告；
    /// 4. `finalize` 计算平均 tokens/sec。
    pub fn deploy(&mut self) -> Result<DeployReport, DeployError> {
        let device = self.config.device;
        let n_gpu_layers = device.n_gpu_layers();
        let mut report = DeployReport::new(device, n_gpu_layers);

        // 1. 硬件检查（meets_requirements=false → 拒绝部署，ARC-006 非恒真）
        let hw = self.backend.check_hardware(&self.config)?;
        if !hw.meets_requirements {
            return Err(DeployError::HardwareInsufficient);
        }

        // 2. 模型加载
        let load_ns = self.backend.load_model(&mut self.engine, &self.config)?;
        report.record_load_time(load_ns);

        // 3. 逐条提示词推理
        let infer_params = self.config.infer_params.clone();
        for prompt in self.prompts.prompts() {
            let start_tokens = self.engine.stats().total_tokens_generated;
            let start_ns = self.engine.stats().total_inference_ns;

            match self.engine.infer(&prompt.prompt, &infer_params) {
                Ok(result) => {
                    let end_tokens = self.engine.stats().total_tokens_generated;
                    let end_ns = self.engine.stats().total_inference_ns;
                    let tokens = end_tokens.saturating_sub(start_tokens);
                    let ns = end_ns.saturating_sub(start_ns);
                    report.record_inference(tokens, ns);

                    if !prompt.validate_result(&result) {
                        report.add_failure(prompt.prompt.clone(), DeployError::InvalidResult);
                    }
                }
                Err(e) => {
                    report.add_failure(prompt.prompt.clone(), DeployError::from(e));
                }
            }
        }

        // 4. 结束统计
        report.finalize();
        self.deployed = true;
        Ok(report)
    }

    /// 卸载部署（ARC-006：不再空操作——委托引擎 `unload` 释放资源）.
    ///
    /// 清理委托给 `LlmEngine::unload`（各后端实现显式释放）；重复卸载
    /// 返回 `DeployError::NotDeployed`（非幂等静默成功）。
    pub fn undeploy(&mut self) -> Result<(), DeployError> {
        if !self.deployed {
            return Err(DeployError::NotDeployed);
        }
        self.engine.unload().map_err(DeployError::from)?;
        self.deployed = false;
        Ok(())
    }
}
