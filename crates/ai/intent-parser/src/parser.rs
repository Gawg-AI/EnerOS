//! 意图解析器主接口（D2/D3/D4/D6/D11）.
//!
//! 桥接 LLM 输出（`Intent`）与 Solver 输入（`ScheduleConfig` / `LpProblem`），
//! 是双脑架构的关键转换环节。

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::model::EnergyScheduleModel;
use eneros_safety_validator::state::SystemState;
use eneros_solver_core::problem::LpProblem;

use alloc::string::{String, ToString};

use crate::error::IntentError;
use crate::intent::{Intent, IntentType};

/// 将 LLM 输出的 JSON 归一化为 [`Intent`] 反序列化可接受的形态（P0-16，v0.203.0）。
///
/// 处理 LLM 常见偏离：
/// 1. **多余文本包裹**：剥离 JSON 前后的说明文字/围栏（提取首个 `{...}` 平衡对象）。
/// 2. **`action` 别名 → `intent_type`**：`{"action":"charge"}` → `{"intent_type":"Charge"}`。
/// 3. **顶层 `power_kw` → 嵌套 `power`**：`{"power_kw":-50.0}` → `{"power":{"power_kw":-50.0}}`。
/// 4. **`power_ratio` 顶层 → 嵌套**：同 3。
///
/// 返回归一化后的 JSON 字符串；无法提取 JSON 对象时原样返回（由 serde 报错）。
pub fn normalize_llm_json(raw: &str) -> String {
    let Some(json) = extract_balanced_json(raw) else {
        return raw.to_string();
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return json;
    };
    let Some(obj) = value.as_object_mut() else {
        return json;
    };

    // 1) action 别名 → intent_type（大小写不敏感）。
    if !obj.contains_key("intent_type") {
        let action_owned = obj.remove("action").and_then(|a| match a {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        });
        if let Some(action) = action_owned {
            let canonical: String = match action.to_lowercase().as_str() {
                "charge" | "charging" => "Charge".to_string(),
                "discharge" | "discharging" => "Discharge".to_string(),
                "hold" | "idle" | "standby" => "Hold".to_string(),
                "stop" => "Stop".to_string(),
                "emergency_stop" | "emergency" => "EmergencyStop".to_string(),
                "autonomous" | "auto" => "AutonomousSchedule".to_string(),
                "setpoint" | "set_setpoint" => "SetSetpoint".to_string(),
                other => other.to_string(),
            };
            obj.insert("intent_type".to_string(), serde_json::Value::String(canonical));
        }
    }

    // 2) 顶层 power_kw / power_ratio → 嵌套 power。
    let mut power = serde_json::Map::new();
    if let Some(pk) = obj.remove("power_kw") {
        power.insert("power_kw".to_string(), pk);
    }
    if let Some(pr) = obj.remove("power_ratio") {
        power.insert("power_ratio".to_string(), pr);
    }
    if !power.is_empty() {
        if let Some(existing) = obj.get_mut("power").and_then(|p| p.as_object_mut()) {
            for (k, v) in power {
                existing.entry(k).or_insert(v);
            }
        } else {
            obj.insert(
                "power".to_string(),
                serde_json::Value::Object(power),
            );
        }
    }

    serde_json::Value::Object(obj.clone()).to_string()
}

/// 提取字符串中首个平衡的 JSON 对象 `{...}`（含嵌套）。
///
/// 用于剥离 LLM 输出前后的说明文字/围栏；无平衡对象时返回 `None`。
pub fn extract_balanced_json(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'"' if !escaped => in_string = !in_string,
            b'\\' if in_string => escaped = !escaped,
            _ => escaped = false,
        }
        if !in_string {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(raw[start..=i].to_string());
                    }
                }
                _ => {}
            }
        }
    }
    None
}

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
    /// v0.203.0（P0-16）：先经 [`normalize_llm_json`] 归一化 LLM 常见偏离
    /// （`action` 别名 → `intent_type`、顶层 `power_kw` → 嵌套 `power`、
    /// 多余文本包裹剥离），再反序列化。
    pub fn parse_json(&self, json: &str) -> Result<Intent, IntentError> {
        let normalized = normalize_llm_json(json);
        serde_json::from_str(&normalized).map_err(|e| IntentError::ParseError(e.to_string()))
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
