//! 场景脚本引擎
//!
//! 用 TOML 描述时序场景，支持故障注入、负荷变化、发电机/线路跳闸等动作，
//! 并通过 [`ScenarioRunner`] 按时间顺序执行事件。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// 场景脚本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// 场景名称
    pub name: String,
    /// 场景描述
    pub description: String,
    /// 场景总时长（秒）
    pub duration: f64,
    /// 时间步长（秒，默认 0.1）
    #[serde(default = "default_timestep")]
    pub time_step: f64,
    /// 事件时间线
    pub timeline: Vec<ScenarioEvent>,
    /// 初始状态参数（可选）
    #[serde(default)]
    pub initial_state: HashMap<String, serde_json::Value>,
}

fn default_timestep() -> f64 {
    0.1
}

/// 场景事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioEvent {
    /// 事件触发时间（秒）
    pub time: f64,
    /// 事件动作
    pub action: ScenarioAction,
    /// 动作参数
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// 场景动作枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScenarioAction {
    /// 注入故障
    InjectFault,
    /// 清除故障
    ClearFault,
    /// 负荷变化
    LoadChange,
    /// 发电机跳闸
    GeneratorTrip,
    /// 线路跳闸
    LineTrip,
    /// 负荷切除
    LoadShed,
    /// 观察记录点
    Observe,
}

/// 场景运行结果
#[derive(Debug, Clone)]
pub struct RunResult {
    /// 已执行事件数
    pub events_executed: usize,
    /// 场景总时长（秒）
    pub duration: f64,
    /// 观察记录点
    pub observations: Vec<Observation>,
}

/// 观察记录点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// 观察时间（秒）
    pub time: f64,
    /// 观察时刻的状态快照
    pub state: HashMap<String, serde_json::Value>,
}

/// 场景错误类型
#[derive(Debug, Error)]
pub enum ScenarioError {
    /// 解析错误
    #[error("场景解析失败: {0}")]
    Parse(String),
    /// 校验错误
    #[error("场景校验失败: {0}")]
    Validate(String),
    /// 运行错误
    #[error("场景运行失败: {0}")]
    Run(String),
    /// IO 错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

impl Scenario {
    /// 从 TOML 字符串解析场景
    pub fn load_from_str(s: &str) -> Result<Self, ScenarioError> {
        toml::from_str(s).map_err(|e| ScenarioError::Parse(e.to_string()))
    }

    /// 从 TOML 文件加载场景
    pub fn load_from_file(path: &str) -> Result<Self, ScenarioError> {
        let content = std::fs::read_to_string(path)?;
        Self::load_from_str(&content)
    }

    /// 校验场景配置
    ///
    /// 检查项：duration > 0、time_step > 0、事件时间在 [0, duration] 范围内、
    /// 事件按时间非递减排序。
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err(ScenarioError::Validate(format!(
                "duration 必须为有限值且大于 0，当前为 {}",
                self.duration
            )));
        }
        if !self.time_step.is_finite() || self.time_step <= 0.0 {
            return Err(ScenarioError::Validate(format!(
                "time_step 必须为有限值且大于 0，当前为 {}",
                self.time_step
            )));
        }
        let mut prev_time = f64::NEG_INFINITY;
        for event in &self.timeline {
            if !event.time.is_finite() || event.time < 0.0 || event.time > self.duration {
                return Err(ScenarioError::Validate(format!(
                    "事件时间 {} 超出场景时长范围 [0, {}]",
                    event.time, self.duration
                )));
            }
            if event.time < prev_time {
                return Err(ScenarioError::Validate(format!(
                    "事件未按时间排序：在 {} 之后出现 {}",
                    prev_time, event.time
                )));
            }
            prev_time = event.time;
        }
        Ok(())
    }
}

/// 场景运行器
pub struct ScenarioRunner {
    scenario: Scenario,
}

impl ScenarioRunner {
    /// 创建运行器
    pub fn new(scenario: Scenario) -> Self {
        Self { scenario }
    }

    /// 按时序执行事件
    ///
    /// `callback` 在每个事件触发时调用，参数为 (事件时间, 事件引用)。
    /// 返回执行结果，包含已执行事件数和观察记录点。
    pub fn run<F>(&self, mut callback: F) -> Result<RunResult, ScenarioError>
    where
        F: FnMut(f64, &ScenarioEvent),
    {
        self.scenario.validate()?;
        let mut observations = Vec::new();
        let mut events_executed = 0usize;
        for event in &self.scenario.timeline {
            callback(event.time, event);
            events_executed += 1;
            if event.action == ScenarioAction::Observe {
                observations.push(Observation {
                    time: event.time,
                    state: event.params.clone(),
                });
            }
        }
        Ok(RunResult {
            events_executed,
            duration: self.scenario.duration,
            observations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML_SCENARIO: &str = r#"
name = "test_scenario"
description = "测试场景"
duration = 10.0
time_step = 0.1

[[timeline]]
time = 0.0
action = { type = "observe" }

[[timeline]]
time = 5.0
action = { type = "inject_fault" }
params = { bus_id = "bus_3", fault_type = "three_phase" }

[[timeline]]
time = 5.2
action = { type = "load_shed" }
params = { zone = "zone_1", percentage = 0.3 }

[[timeline]]
time = 10.0
action = { type = "observe" }
"#;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn test_scenario_parse() {
        let scenario = Scenario::load_from_str(TOML_SCENARIO).expect("解析场景失败");
        assert_eq!(scenario.name, "test_scenario");
        assert_eq!(scenario.description, "测试场景");
        assert!(approx_eq(scenario.duration, 10.0));
        assert!(approx_eq(scenario.time_step, 0.1));
        assert_eq!(scenario.timeline.len(), 4);
        assert!(scenario.initial_state.is_empty());
    }

    #[test]
    fn test_scenario_run() {
        let scenario = Scenario::load_from_str(TOML_SCENARIO).expect("解析场景失败");
        let runner = ScenarioRunner::new(scenario);
        let mut times = Vec::new();
        let result = runner
            .run(|t, _event| {
                times.push(t);
            })
            .expect("运行场景失败");
        assert_eq!(result.events_executed, 4);
        assert!(approx_eq(result.duration, 10.0));
        assert_eq!(times.len(), 4);
        // 验证事件按时间顺序执行
        for w in times.windows(2) {
            assert!(w[1] >= w[0], "事件未按时间顺序执行");
        }
    }

    #[test]
    fn test_scenario_event_order() {
        let scenario = Scenario::load_from_str(TOML_SCENARIO).expect("解析场景失败");
        let times: Vec<f64> = scenario.timeline.iter().map(|e| e.time).collect();
        // 验证事件按 time 排序
        for w in times.windows(2) {
            assert!(w[1] >= w[0], "事件未按时间排序");
        }
        assert!(approx_eq(times[0], 0.0));
        assert!(approx_eq(times[1], 5.0));
        assert!(approx_eq(times[2], 5.2));
        assert!(approx_eq(times[3], 10.0));
        // 校验通过
        assert!(scenario.validate().is_ok());
    }

    #[test]
    fn test_scenario_inject_fault() {
        let scenario = Scenario::load_from_str(TOML_SCENARIO).expect("解析场景失败");
        let fault_event = scenario
            .timeline
            .iter()
            .find(|e| e.action == ScenarioAction::InjectFault)
            .expect("未找到 InjectFault 事件");
        assert!(approx_eq(fault_event.time, 5.0));
        assert_eq!(fault_event.action, ScenarioAction::InjectFault);
        assert_eq!(
            fault_event.params.get("bus_id").and_then(|v| v.as_str()),
            Some("bus_3")
        );
        assert_eq!(
            fault_event
                .params
                .get("fault_type")
                .and_then(|v| v.as_str()),
            Some("three_phase")
        );
    }

    #[test]
    fn test_scenario_observe() {
        let scenario = Scenario::load_from_str(TOML_SCENARIO).expect("解析场景失败");
        let runner = ScenarioRunner::new(scenario);
        let result = runner
            .run(|_t, _event| {})
            .expect("运行场景失败");
        // 两个 observe 事件，应记录两个观察点
        assert_eq!(result.observations.len(), 2);
        assert!(approx_eq(result.observations[0].time, 0.0));
        assert!(approx_eq(result.observations[1].time, 10.0));
    }

    /// 构造一个合法的场景用于校验测试
    fn make_valid_scenario() -> Scenario {
        Scenario {
            name: "test".to_string(),
            description: "test".to_string(),
            duration: 10.0,
            time_step: 0.1,
            timeline: vec![ScenarioEvent {
                time: 5.0,
                action: ScenarioAction::Observe,
                params: HashMap::new(),
            }],
            initial_state: HashMap::new(),
        }
    }

    #[test]
    fn test_validate_rejects_nan_duration() {
        let mut scenario = make_valid_scenario();
        scenario.duration = f64::NAN;
        assert!(scenario.validate().is_err(), "NaN duration 应被拒绝");
    }

    #[test]
    fn test_validate_rejects_nan_time_step() {
        let mut scenario = make_valid_scenario();
        scenario.time_step = f64::NAN;
        assert!(scenario.validate().is_err(), "NaN time_step 应被拒绝");
    }

    #[test]
    fn test_validate_rejects_inf_duration() {
        let mut scenario = make_valid_scenario();
        scenario.duration = f64::INFINITY;
        assert!(scenario.validate().is_err(), "Infinity duration 应被拒绝");
    }

    #[test]
    fn test_validate_rejects_nan_event_time() {
        let mut scenario = make_valid_scenario();
        scenario.timeline[0].time = f64::NAN;
        assert!(scenario.validate().is_err(), "NaN 事件时间应被拒绝");
    }

    /// 验证空时间线场景校验通过。
    #[test]
    fn test_validate_empty_timeline() {
        let mut scenario = make_valid_scenario();
        scenario.timeline.clear();
        // 空时间线场景应校验通过（无事件需要检查）
        assert!(scenario.validate().is_ok(), "空时间线场景应校验通过");
    }

    /// 验证零时长场景返回错误。
    #[test]
    fn test_validate_zero_duration() {
        let mut scenario = make_valid_scenario();
        scenario.duration = 0.0;
        // 零时长场景应返回错误
        let result = scenario.validate();
        assert!(result.is_err(), "零时长场景应返回错误");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("duration"),
            "错误信息应包含 duration，实际: {}",
            err_msg
        );
    }

    /// 验证负时间步长返回错误。
    #[test]
    fn test_validate_negative_time_step() {
        let mut scenario = make_valid_scenario();
        scenario.time_step = -0.1;
        // 负时间步长应返回错误
        let result = scenario.validate();
        assert!(result.is_err(), "负时间步长应返回错误");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("time_step"),
            "错误信息应包含 time_step，实际: {}",
            err_msg
        );
    }
}
