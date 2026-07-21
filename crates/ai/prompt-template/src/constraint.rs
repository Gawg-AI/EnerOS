//! JSON 输出约束 + 重试机制（D6：静默重试，统计计数器）.

use eneros_llm_engine::engine::LlmEngine;
use eneros_llm_engine::params::InferParams;
use serde_json::Value;

use crate::context::TemplateContext;
use crate::error::TemplateError;
use crate::template::PromptTemplate;

/// 约束统计（重试/成功/失败计数）.
#[derive(Debug, Clone, Default)]
pub struct ConstraintStats {
    /// 总尝试次数（每次 `infer_with_constraint` 调用 +1）.
    pub total_attempts: u64,
    /// 成功次数（Schema 校验通过）.
    pub successful: u64,
    /// 失败次数（重试耗尽）.
    pub failed: u64,
    /// 重试次数（非首次尝试成功计为一次重试）.
    pub retries: u64,
}

/// JSON 输出约束器.
///
/// 包装 `LlmEngine` 推理 + Prompt 模板校验 + 重试机制。
/// 推理失败（`LlmError`）立即返回；校验失败静默重试，耗尽返回 `MaxRetriesExceeded`（D6）。
pub struct JsonConstraint {
    max_retries: u8,
    stats: ConstraintStats,
}

impl JsonConstraint {
    /// 构造约束器.
    ///
    /// `max_retries` 为最大重试次数（总尝试次数 = `max_retries + 1`）。
    pub fn new(max_retries: u8) -> Self {
        Self {
            max_retries,
            stats: ConstraintStats::default(),
        }
    }

    /// 带约束推理.
    ///
    /// 流程：构建 prompt → 循环 `max_retries + 1` 次：推理 → 校验。
    /// - 推理失败（`LlmError`）：立即返回 `Err(TemplateError::Engine(_))`（不重试）。
    /// - 校验失败：静默重试。
    /// - 全部失败：返回 `Err(TemplateError::MaxRetriesExceeded)`。
    pub fn infer_with_constraint(
        &mut self,
        engine: &mut dyn LlmEngine,
        template: &dyn PromptTemplate,
        context: &TemplateContext,
    ) -> Result<Value, TemplateError> {
        self.stats.total_attempts += 1;
        let prompt = template.build(context);
        let params = InferParams::default();
        for attempt in 0..=self.max_retries {
            let output = engine.infer(&prompt, &params)?;
            match template.validate(&output) {
                Ok(value) => {
                    if attempt > 0 {
                        self.stats.retries += 1;
                    }
                    self.stats.successful += 1;
                    return Ok(value);
                }
                Err(_) => continue,
            }
        }
        self.stats.failed += 1;
        Err(TemplateError::MaxRetriesExceeded)
    }

    /// 当前统计.
    pub fn stats(&self) -> &ConstraintStats {
        &self.stats
    }

    /// 最大重试次数.
    pub fn max_retries(&self) -> u8 {
        self.max_retries
    }
}
