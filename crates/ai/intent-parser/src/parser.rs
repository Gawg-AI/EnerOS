//! 意图解析器主接口（D2/D3/D4/D6/D11）.
//!
//! 桥接 LLM 输出（`Intent`）与 Solver 输入（`ScheduleConfig` / `LpProblem`），
//! 是双脑架构的关键转换环节。

use alloc::string::ToString;

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::model::EnergyScheduleModel;
use eneros_safety_validator::state::SystemState;
use eneros_solver_core::problem::LpProblem;

use crate::error::IntentError;
use crate::intent::{Intent, IntentType};

/// 意图解析器.
///
/// 将 LLM 输出的 JSON 意图转换为 Solver 可执行的调度配置与 LP 问题。
/// 解析逻辑遵循蓝图 §6.2"意图转换"规范，并对 no_std 环境做了安全加固：
///
/// - **D2**：`SystemState.soc_pct`（非 `soc`）
/// - **D3**：`config.price.get_mut(t)` 安全索引（no_std panic 不可恢复）
/// - **D4**：`SolverError` 显式 `map_err` 为 `IntentError::CompileError`
/// - **D6**：复用 v0.67.0 `SystemState`
/// - **D11**：`to_opt_problem` 保留 `config.clone()`（需返回 `(config, problem)`）
pub struct IntentParser {
    /// 默认调度配置（每次解析均从此克隆）.
    default_config: ScheduleConfig,
    /// 当前系统状态（仅 `EmergencyStop` 使用）.
    system_state: SystemState,
}

impl IntentParser {
    /// 创建意图解析器.
    pub fn new(default_config: ScheduleConfig, state: SystemState) -> Self {
        Self {
            default_config,
            system_state: state,
        }
    }

    /// 解析 JSON 字符串为 `Intent`.
    ///
    /// 失败返回 `IntentError::ParseError`（D1：使用 `serde_json::from_str`）。
    pub fn parse_json(&self, json: &str) -> Result<Intent, IntentError> {
        serde_json::from_str(json).map_err(|e| IntentError::ParseError(e.to_string()))
    }

    /// 将 `Intent` 转换为 `ScheduleConfig`.
    ///
    /// 转换规则（蓝图 §6.2）：
    /// - `AutonomousSchedule`：设置 `soc_final`
    /// - `Charge`：在时间范围内将 `price[t]` 置为 `-power_kw`（鼓励充电）
    /// - `Discharge`：在时间范围内将 `price[t]` 置为 `power_kw * 10.0`（鼓励放电）
    /// - `Hold` / `Stop`：`pcs_power_kw = 0.0`
    /// - `EmergencyStop`：`pcs_power_kw = 0.0` 且锁定 SOC 上下限为当前 SOC
    /// - `SetSetpoint`：覆盖 `pcs_power_kw`
    pub fn to_schedule_config(&self, intent: &Intent) -> Result<ScheduleConfig, IntentError> {
        let mut config = self.default_config.clone();

        match intent.intent_type {
            IntentType::AutonomousSchedule => {
                if let Some(soc_intent) = &intent.soc_target {
                    config.soc_final = Some(soc_intent.target_soc);
                }
            }
            IntentType::Charge => {
                if let Some(power) = &intent.power {
                    let power_kw = power.power_kw.abs().min(config.pcs_power_kw);
                    if let Some(time_range) = &intent.time_range {
                        let end = time_range
                            .end_period
                            .min(config.num_periods.saturating_sub(1));
                        for t in time_range.start_period..=end {
                            // D3: 安全索引，no_std panic 不可恢复
                            if let Some(price) = config.price.get_mut(t) {
                                *price = -power_kw;
                            }
                        }
                    }
                }
            }
            IntentType::Discharge => {
                if let Some(power) = &intent.power {
                    let power_kw = power.power_kw.abs().min(config.pcs_power_kw);
                    if let Some(time_range) = &intent.time_range {
                        let end = time_range
                            .end_period
                            .min(config.num_periods.saturating_sub(1));
                        for t in time_range.start_period..=end {
                            if let Some(price) = config.price.get_mut(t) {
                                *price = power_kw * 10.0;
                            }
                        }
                    }
                }
            }
            IntentType::Hold => {
                config.pcs_power_kw = 0.0;
            }
            IntentType::Stop => {
                config.pcs_power_kw = 0.0;
            }
            IntentType::EmergencyStop => {
                config.pcs_power_kw = 0.0;
                // D2: 使用 soc_pct（v0.67.0 SystemState 字段名）
                config.soc_min = self.system_state.soc_pct;
                config.soc_max = self.system_state.soc_pct;
            }
            IntentType::SetSetpoint => {
                if let Some(power) = &intent.power {
                    config.pcs_power_kw = power.power_kw.abs();
                }
            }
        }

        self.validate_config(&config)?;
        Ok(config)
    }

    /// 将 `Intent` 转换为 `(ScheduleConfig, LpProblem)`（D11）.
    ///
    /// 返回 `config` 的克隆用于后续结果解析与安全校验；`LpProblem` 由
    /// `EnergyScheduleModel::compile()` 生成（D4：显式 `map_err`）。
    pub fn to_opt_problem(
        &self,
        intent: &Intent,
    ) -> Result<(ScheduleConfig, LpProblem), IntentError> {
        let config = self.to_schedule_config(intent)?;
        // D11: clone 必要——需同时返回 config 与编译后的 problem
        let model = EnergyScheduleModel::new(config.clone());
        // D4: SolverError 不实现 From<IntentError>，显式 map_err
        let problem = model
            .compile()
            .map_err(|e| IntentError::CompileError(e.to_string()))?;
        Ok((config, problem))
    }

    /// 校验调度配置合法性.
    ///
    /// 校验项（蓝图 §6.2 验收条件，D12 保持 `price.len()` 校验）：
    /// - `num_periods > 0`
    /// - `pcs_power_kw >= 0.0`
    /// - `0.0 <= soc_min < soc_max <= 1.0`
    /// - `price.len() == num_periods`
    pub fn validate_config(&self, config: &ScheduleConfig) -> Result<(), IntentError> {
        if config.num_periods == 0 {
            return Err(IntentError::InvalidConfig("时段数为 0".into()));
        }
        if config.pcs_power_kw < 0.0 {
            return Err(IntentError::InvalidConfig("PCS 功率为负".into()));
        }
        // 注意：使用 `>` 而非 `>=`，允许 `soc_min == soc_max`（点约束）。
        // EmergencyStop 意图需将 SOC 锁定到当前值（soc_min == soc_max == soc_pct），
        // 这是合法的"点约束"语义，不应视为非法。
        if config.soc_min < 0.0 || config.soc_max > 1.0 || config.soc_min > config.soc_max {
            return Err(IntentError::InvalidConfig("SOC 范围不合理".into()));
        }
        if config.price.len() != config.num_periods {
            return Err(IntentError::InvalidConfig("价格曲线长度不匹配".into()));
        }
        Ok(())
    }
}

impl Default for IntentParser {
    fn default() -> Self {
        Self::new(ScheduleConfig::default(), SystemState::default())
    }
}
