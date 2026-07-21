//! 契约转换器（D5/D10/D11）.
//!
//! 桥接契约层与 v0.68.0 `IntentParser`，实现双向转换：
//! - 正向：`IntentContract` → `(ScheduleConfig, LpProblem)`（复用 `IntentParser`，D5）
//! - 反向：`SolveResult` + `ValidationResult` + `ScheduleResult` → `FeedbackContract`

use alloc::format;
use alloc::string::String;

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::model::EnergyScheduleModel;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_intent_parser::parser::IntentParser;
use eneros_safety_validator::result::ValidationResult;
use eneros_safety_validator::state::SystemState;
use eneros_solver_core::problem::LpProblem;
use eneros_solver_core::result::SolveResult;

use crate::contract::{FeedbackContract, IntentContract};
use crate::error::ContractError;

/// 契约转换器.
///
/// 封装 v0.68.0 `IntentParser`，提供契约层 ↔ Solver 层的双向转换。
pub struct ContractConverter {
    /// 默认调度配置（每次正向转换均从此克隆）.
    pub default_config: ScheduleConfig,
}

impl ContractConverter {
    /// 创建转换器.
    pub fn new(default_config: ScheduleConfig) -> Self {
        Self { default_config }
    }

    /// 正向：`IntentContract` → `(ScheduleConfig, LpProblem)`.
    ///
    /// 内部构造 `IntentParser`（D5），将契约中的 `Intent` 转换为调度配置与 LP 问题。
    /// `IntentError` 显式 `map_err` 为 `ContractError::SerializationError`（D10）。
    pub fn to_solver_params(
        &self,
        contract: &IntentContract,
        state: &SystemState,
    ) -> Result<(ScheduleConfig, LpProblem), ContractError> {
        let parser = IntentParser::new(self.default_config.clone(), state.clone());
        // D10: IntentError 显式 map_err（IntentError 未实现 Display，用 Debug 格式化）
        let config = parser
            .to_schedule_config(&contract.intent)
            .map_err(|e| ContractError::SerializationError(format!("{:?}", e)))?;
        let model = EnergyScheduleModel::new(config.clone());
        // D11: 保留蓝图 SerializationError 命名用于 compile 错误
        let problem = model
            .compile()
            .map_err(|e| ContractError::SerializationError(format!("{:?}", e)))?;
        Ok((config, problem))
    }

    /// 反向：Solver 结果 → `FeedbackContract`.
    pub fn to_feedback(
        &self,
        request_id: &str,
        solve_result: &SolveResult,
        validation: &ValidationResult,
        schedule: &ScheduleResult,
        solve_ms: u64,
    ) -> FeedbackContract {
        FeedbackContract {
            request_id: String::from(request_id),
            solve_status: solve_result.status.clone(),
            validation_passed: validation.passed,
            clamp_info: if validation.violations.is_empty() {
                None
            } else {
                Some(validation.violations.clone())
            },
            executed_schedule: Some(schedule.schedule.clone()),
            actual_revenue: schedule.total_revenue_yuan,
            solve_ms,
        }
    }

    /// 序列化反馈契约为 JSON（D6：`serde_json::to_string_pretty`）.
    pub fn serialize_feedback(&self, feedback: &FeedbackContract) -> Result<String, ContractError> {
        serde_json::to_string_pretty(feedback)
            .map_err(|e| ContractError::SerializationError(format!("{:?}", e)))
    }
}

impl Default for ContractConverter {
    fn default() -> Self {
        Self::new(ScheduleConfig::default())
    }
}
