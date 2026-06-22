//! 模拟器 API handlers (v0.28.0 — Task 17).
//!
//! 提供场景脚本运行与内置故障场景查询的 REST 端点：
//! - `POST /api/simulator/run` — 接收 TOML 场景脚本字符串，调用 `ScenarioRunner` 运行
//! - `GET /api/simulator/scenarios` — 列出 `FaultScenarioLibrary` 内置故障场景

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

use eneros_runtime::simulator::fault::{FaultScenarioLibrary, ScenarioType};
use eneros_runtime::simulator::scenario::{Observation, Scenario, ScenarioRunner};

use crate::app::AppState;

/// 将 `ScenarioType` 序列化为 snake_case 字符串
///
/// 使用 serde 序列化（`#[serde(rename_all = "snake_case")]`）确保
/// 多词变体（如 `ProtectionFailure`）正确输出为 `protection_failure`，
/// 而非 Debug 格式的 `protectionfailure`。
fn scenario_type_str(scenario_type: &ScenarioType) -> String {
    serde_json::to_value(scenario_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// `POST /api/simulator/run` 请求体
#[derive(Debug, Deserialize, ToSchema)]
pub struct SimulatorRunRequest {
    /// TOML 格式的场景脚本字符串
    pub scenario_toml: String,
}

/// `POST /api/simulator/run` 响应体
#[derive(Debug, Serialize, ToSchema)]
pub struct SimulatorRunResponse {
    /// 已执行事件数
    pub events_executed: usize,
    /// 场景总时长（秒）
    pub duration: f64,
    /// 观察记录点
    pub observations: Vec<ObservationDto>,
}

/// 观察记录点 DTO（镜像 `eneros_runtime::simulator::scenario::Observation`）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ObservationDto {
    /// 观察时间（秒）
    pub time: f64,
    /// 观察时刻的状态快照
    pub state: HashMap<String, serde_json::Value>,
}

impl From<Observation> for ObservationDto {
    fn from(o: Observation) -> Self {
        Self {
            time: o.time,
            state: o.state,
        }
    }
}

/// `GET /api/simulator/scenarios` 响应体中的单个场景条目
#[derive(Debug, Serialize, ToSchema)]
pub struct FaultScenarioDto {
    /// 场景名称
    pub name: String,
    /// 场景描述
    pub description: String,
    /// 场景类型（n1 / n2 / cascading / protection_failure / protection_maloperation）
    pub scenario_type: String,
}

/// `GET /api/simulator/scenarios` 响应体
#[derive(Debug, Serialize, ToSchema)]
pub struct SimulatorScenariosResponse {
    /// 内置故障场景列表
    pub scenarios: Vec<FaultScenarioDto>,
}

/// `POST /api/simulator/validate` 请求体
#[derive(Debug, Deserialize, ToSchema)]
pub struct SimulatorValidateRequest {
    /// TOML 格式的场景脚本字符串
    pub scenario_toml: String,
}

/// `POST /api/simulator/validate` 响应体
#[derive(Debug, Serialize, ToSchema)]
pub struct SimulatorValidateResponse {
    /// 场景是否通过校验
    pub valid: bool,
    /// 校验错误信息列表（校验通过时为空）
    pub errors: Vec<String>,
}

/// `POST /api/simulator/run` — 运行 TOML 场景脚本。
///
/// 接收 TOML 格式的场景脚本字符串，解析为 `Scenario` 后调用 `ScenarioRunner`
/// 按时间线执行事件，返回执行结果（事件数、时长、观察点）。
#[utoipa::path(
    post,
    tag = "simulator",
    path = "/api/simulator/run",
    request_body = SimulatorRunRequest,
    responses(
        (status = 200, description = "场景运行结果", body = SimulatorRunResponse),
        (status = 400, description = "场景解析或校验失败"),
    )
)]
pub async fn run_handler(
    State(_state): State<AppState>,
    Json(req): Json<SimulatorRunRequest>,
) -> axum::response::Response {
    // 解析 TOML 场景脚本
    let scenario = match Scenario::load_from_str(&req.scenario_toml) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("场景解析失败: {}", e),
                })),
            )
                .into_response();
        }
    };

    // 创建运行器并执行
    let runner = ScenarioRunner::new(scenario);
    match runner.run(|_t, _event| {}) {
        Ok(result) => {
            let response = SimulatorRunResponse {
                events_executed: result.events_executed,
                duration: result.duration,
                observations: result
                    .observations
                    .into_iter()
                    .map(ObservationDto::from)
                    .collect(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": format!("场景运行失败: {}", e),
            })),
        )
            .into_response(),
    }
}

/// `GET /api/simulator/scenarios` — 列出 `FaultScenarioLibrary` 内置故障场景。
#[utoipa::path(
    get,
    tag = "simulator",
    path = "/api/simulator/scenarios",
    responses(
        (status = 200, description = "内置故障场景列表", body = SimulatorScenariosResponse),
    )
)]
pub async fn scenarios_handler(State(_state): State<AppState>) -> axum::response::Response {
    let library = FaultScenarioLibrary::new();
    let scenarios: Vec<FaultScenarioDto> = library
        .list()
        .iter()
        .map(|s| FaultScenarioDto {
            name: s.name.clone(),
            description: s.description.clone(),
            scenario_type: scenario_type_str(&s.scenario_type),
        })
        .collect();

    let response = SimulatorScenariosResponse { scenarios };
    (StatusCode::OK, Json(response)).into_response()
}

/// `POST /api/simulator/validate` — 验证 TOML 场景脚本。
///
/// 接收 TOML 格式的场景脚本字符串，解析为 `Scenario` 后调用 `validate()`
/// 检查场景配置（duration、time_step、事件时间范围与排序），返回校验结果。
#[utoipa::path(
    post,
    tag = "simulator",
    path = "/api/simulator/validate",
    request_body = SimulatorValidateRequest,
    responses(
        (status = 200, description = "场景校验结果", body = SimulatorValidateResponse),
        (status = 400, description = "场景解析失败"),
    )
)]
pub async fn validate_handler(
    State(_state): State<AppState>,
    Json(req): Json<SimulatorValidateRequest>,
) -> axum::response::Response {
    // 解析 TOML 场景脚本
    let scenario = match Scenario::load_from_str(&req.scenario_toml) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "valid": false,
                    "errors": [format!("场景解析失败: {}", e)],
                })),
            )
                .into_response();
        }
    };

    // 校验场景配置
    match scenario.validate() {
        Ok(()) => {
            let response = SimulatorValidateResponse {
                valid: true,
                errors: Vec::new(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = SimulatorValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
            };
            (StatusCode::OK, Json(response)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;

    /// 有效的 TOML 场景脚本
    const VALID_SCENARIO_TOML: &str = r#"
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
time = 10.0
action = { type = "observe" }
"#;

    #[test]
    fn test_simulator_run_request_deserialization() {
        let json = r#"{"scenario_toml":"name = \"x\"\ndescription = \"y\"\nduration = 1.0\ntime_step = 0.1\n"}"#;
        let req: SimulatorRunRequest = serde_json::from_str(json).unwrap();
        assert!(req.scenario_toml.contains("name = \"x\""));
    }

    #[test]
    fn test_observation_dto_from_observation() {
        let mut state = HashMap::new();
        state.insert("voltage".to_string(), serde_json::json!(1.05));
        let obs = Observation {
            time: 1.5,
            state,
        };
        let dto = ObservationDto::from(obs);
        assert!((dto.time - 1.5).abs() < f64::EPSILON);
        assert_eq!(dto.state.get("voltage").and_then(|v| v.as_f64()), Some(1.05));
    }

    #[test]
    fn test_fault_scenario_dto_from_library() {
        let library = FaultScenarioLibrary::new();
        let scenarios: Vec<FaultScenarioDto> = library
            .list()
            .iter()
            .map(|s| FaultScenarioDto {
                name: s.name.clone(),
                description: s.description.clone(),
                scenario_type: scenario_type_str(&s.scenario_type),
            })
            .collect();
        assert_eq!(scenarios.len(), 5);
        assert!(scenarios.iter().any(|s| s.name == "n1_bus_fault"));
        // 验证 scenario_type 使用 serde snake_case 序列化
        assert!(scenarios.iter().any(|s| s.scenario_type == "protection_failure"));
        assert!(scenarios.iter().any(|s| s.scenario_type == "protection_maloperation"));
    }

    #[tokio::test]
    async fn test_simulator_run_valid_scenario() {
        let state = AppState::new();
        let req = SimulatorRunRequest {
            scenario_toml: VALID_SCENARIO_TOML.to_string(),
        };
        let response = run_handler(State(state), Json(req)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["events_executed"], 3);
        assert!((json["duration"].as_f64().unwrap() - 10.0).abs() < f64::EPSILON);
        assert_eq!(json["observations"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_simulator_run_invalid_scenario() {
        let state = AppState::new();
        let req = SimulatorRunRequest {
            scenario_toml: "invalid toml content {{{".to_string(),
        };
        let response = run_handler(State(state), Json(req)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_simulator_list_scenarios() {
        let state = AppState::new();
        let response = scenarios_handler(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let scenarios = json["scenarios"].as_array().unwrap();
        assert_eq!(scenarios.len(), 5);
        assert!(scenarios.iter().any(|s| s["name"] == "n1_bus_fault"));
        assert!(scenarios.iter().any(|s| s["name"] == "cascading_failure"));
        // 验证 scenario_type 使用 serde snake_case 序列化，
        // 而非 Debug 格式（Debug 会将 ProtectionFailure 输出为 "protectionfailure"）
        assert!(scenarios.iter().any(|s| s["scenario_type"] == "protection_failure"));
        assert!(scenarios.iter().any(|s| s["scenario_type"] == "protection_maloperation"));
        assert!(scenarios.iter().any(|s| s["scenario_type"] == "n1"));
    }

    #[tokio::test]
    async fn test_simulator_validate() {
        let state = AppState::new();

        // 1. 有效场景：校验通过
        let req = SimulatorValidateRequest {
            scenario_toml: VALID_SCENARIO_TOML.to_string(),
        };
        let response = validate_handler(State(state.clone()), Json(req)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["valid"], true);
        assert!(json["errors"].as_array().unwrap().is_empty());

        // 2. 解析失败：返回 400
        let req = SimulatorValidateRequest {
            scenario_toml: "invalid toml content {{{".to_string(),
        };
        let response = validate_handler(State(state.clone()), Json(req)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // 3. 校验失败：duration <= 0
        let invalid_toml = r#"
name = "bad"
description = "无效场景"
duration = -1.0
time_step = 0.1
timeline = []
"#;
        let req = SimulatorValidateRequest {
            scenario_toml: invalid_toml.to_string(),
        };
        let response = validate_handler(State(state), Json(req)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["valid"], false);
        assert!(!json["errors"].as_array().unwrap().is_empty());
    }
}
