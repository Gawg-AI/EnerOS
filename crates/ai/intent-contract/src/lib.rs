//! EnerOS v0.69.0 LLM ↔ Solver 意图契约.
//!
//! 双脑架构（LLM 为感知者，Solver 为执行者）的契约接口层：
//! - [`IntentContract`]：正向契约（LLM → Solver），包含版本化意图与上下文
//! - [`FeedbackContract`]：反向契约（Solver → LLM），反馈求解与校验结果
//! - [`ContractValidator`]：6 项校验规则 + 版本兼容性
//! - [`ContractConverter`]：双向转换器，桥接契约与 v0.68.0 IntentParser
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 复用 v0.68.0 `Intent`（通过 `eneros-intent-parser` 依赖，不重定义） |
//! | **D2** | 复用 v0.67.0 `SystemState` / `ValidationResult` / `Violation` |
//! | **D3** | 复用 v0.66.0 `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult` / `ScheduleEntry` |
//! | **D4** | 复用 v0.64.0 `LpProblem` / `SolveResult` / `SolveStatus` |
//! | **D5** | 复用 v0.68.0 `IntentParser`（通过 `to_solver_params` 内部构造） |
//! | **D6** | `serde_json::to_string_pretty` 在 no_std + alloc 下可用 |
//! | **D7** | `DeviceStatus` 本地定义（蓝图 §4.1 line 14632 引用但未定义，最小集合：Normal/Warning/Fault/Maintenance/Offline） |
//! | **D8** | no_std 合规：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` |
//! | **D9** | `ContractError` 仅派生 `Debug`（Karpathy 简化原则，与 v0.68.0 `IntentError` 一致） |
//! | **D10** | `IntentError` 显式 `map_err` 为 `ContractError::SerializationError`（未实现 `From`） |
//! | **D11** | 保留蓝图 `SerializationError` 命名用于 compile 错误（Surgical Changes，虽命名不准确） |
//! | **D12** | 保留蓝图 `intent.reason` 非空校验（契约比单步 Intent 严格，合理） |

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod contract;
pub mod converter;
pub mod error;
pub mod validator;

pub use contract::{DeviceStatus, FeedbackContract, IntentContract, LlmMeta, SystemContext};
pub use converter::ContractConverter;
pub use error::ContractError;
pub use validator::ContractValidator;

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
    use eneros_intent_parser::intent::{Intent, IntentType, PowerIntent, SocIntent, TimeRange};
    use eneros_safety_validator::result::{Severity, ValidationResult, Violation};
    use eneros_safety_validator::state::SystemState;
    use eneros_solver_core::result::{SolveResult, SolveStatus};

    use super::*;

    // 构造合法 IntentContract 用于多个测试
    fn make_valid_contract() -> IntentContract {
        IntentContract {
            schema_version: String::from("1.1.0"),
            request_id: String::from("req-001"),
            timestamp: 1700000000,
            intent: Intent {
                intent_type: IntentType::Charge,
                time_range: Some(TimeRange {
                    start_period: 0,
                    end_period: 5,
                }),
                power: Some(PowerIntent {
                    power_kw: 50.0,
                    power_ratio: Some(0.5),
                }),
                soc_target: None,
                priority: 3,
                reason: String::from("price low"),
                confidence: 0.85,
            },
            context: SystemContext {
                current_soc: 0.5,
                current_power_kw: 0.0,
                current_price: 0.3,
                current_period: 2,
                device_status: DeviceStatus::Normal,
                alarms: vec![],
            },
            llm_meta: LlmMeta {
                model_name: String::from("qwen2.5-7b"),
                inference_ms: 1200,
                token_count: 512,
                confidence: 0.85,
            },
        }
    }

    // === T1: IntentContract 构造 + 序列化 ===
    #[test]
    fn t1_intent_contract_serialize() {
        let contract = make_valid_contract();
        let json = serde_json::to_string(&contract).expect("serialize failed");
        assert!(json.contains("1.1.0"), "json should contain schema_version");
        assert!(json.contains("req-001"), "json should contain request_id");
        assert!(json.contains("Charge"), "json should contain intent_type");
        assert!(json.contains("price low"), "json should contain reason");
    }

    // === T2: IntentContract 反序列化（缺可选字段 priority/reason/confidence） ===
    #[test]
    fn t2_intent_contract_deserialize_defaults() {
        let json = r#"{
            "schema_version": "1.1.0",
            "request_id": "req-002",
            "timestamp": 1700000001,
            "intent": {"intent_type": "Hold"},
            "context": {
                "current_soc": 0.5,
                "current_power_kw": 0.0,
                "current_price": 0.3,
                "current_period": 2,
                "device_status": "Normal",
                "alarms": []
            },
            "llm_meta": {
                "model_name": "qwen2.5-7b",
                "inference_ms": 1000,
                "token_count": 256,
                "confidence": 0.9
            }
        }"#;
        let contract: IntentContract = serde_json::from_str(json).expect("deserialize failed");
        assert_eq!(contract.schema_version, "1.1.0");
        assert_eq!(contract.request_id, "req-002");
        // Intent 默认值（D9：priority=3, reason="", confidence=0.0）
        assert_eq!(contract.intent.priority, 3);
        assert_eq!(contract.intent.reason, "");
        assert!(
            (contract.intent.confidence - 0.0).abs() < 1e-9,
            "confidence default should be 0.0"
        );
    }

    // === T3: SystemContext 构造 ===
    #[test]
    fn t3_system_context_construction() {
        let ctx = SystemContext {
            current_soc: 0.6,
            current_power_kw: -30.0,
            current_price: 0.4,
            current_period: 5,
            device_status: DeviceStatus::Warning,
            alarms: vec![String::from("over_temp")],
        };
        assert!((ctx.current_soc - 0.6).abs() < 1e-9);
        assert!((ctx.current_power_kw - (-30.0)).abs() < 1e-9);
        assert!((ctx.current_price - 0.4).abs() < 1e-9);
        assert_eq!(ctx.current_period, 5);
        assert!(matches!(ctx.device_status, DeviceStatus::Warning));
        assert_eq!(ctx.alarms.len(), 1);
    }

    // === T4: LlmMeta 构造 ===
    #[test]
    fn t4_llm_meta_construction() {
        let meta = LlmMeta {
            model_name: String::from("qwen2.5-7b"),
            inference_ms: 1500,
            token_count: 1024,
            confidence: 0.92,
        };
        assert_eq!(meta.model_name, "qwen2.5-7b");
        assert_eq!(meta.inference_ms, 1500);
        assert_eq!(meta.token_count, 1024);
        assert!((meta.confidence - 0.92).abs() < 1e-9);
    }

    // === T5: DeviceStatus 枚举变体 ===
    #[test]
    fn t5_device_status_variants() {
        let _ = DeviceStatus::Normal;
        let _ = DeviceStatus::Warning;
        let _ = DeviceStatus::Fault;
        let _ = DeviceStatus::Maintenance;
        let _ = DeviceStatus::Offline;
        // 序列化变体
        let json = serde_json::to_string(&DeviceStatus::Fault).expect("serialize failed");
        assert_eq!(json, "\"Fault\"");
        let parsed: DeviceStatus =
            serde_json::from_str("\"Maintenance\"").expect("deserialize failed");
        assert!(matches!(parsed, DeviceStatus::Maintenance));
    }

    // === T6: FeedbackContract 构造 + 序列化 ===
    #[test]
    fn t6_feedback_contract_serialize() {
        let feedback = FeedbackContract {
            request_id: String::from("req-001"),
            solve_status: SolveStatus::Optimal,
            validation_passed: true,
            clamp_info: None,
            executed_schedule: Some(vec![]),
            actual_revenue: 100.0,
            solve_ms: 50,
        };
        let json = serde_json::to_string(&feedback).expect("serialize failed");
        assert!(json.contains("req-001"));
        assert!(json.contains("validation_passed"));
        assert!(json.contains("100.0"));
        // solve_status/clamp_info/executed_schedule 被 #[serde(skip)] 跳过
        assert!(!json.contains("Optimal"));
    }

    // === T7: ContractValidator::new 默认版本列表（1.0.0 + 1.1.0） ===
    #[test]
    fn t7_validator_new_default_versions() {
        let v = ContractValidator::new();
        assert_eq!(v.supported_versions.len(), 2);
        assert!(v.supported_versions.contains(&String::from("1.0.0")));
        assert!(v.supported_versions.contains(&String::from("1.1.0")));
        assert_eq!(v.current_version, "1.1.0");
    }

    // === T8: validate 合法契约通过 ===
    #[test]
    fn t8_validate_valid_contract() {
        let v = ContractValidator::new();
        let contract = make_valid_contract();
        let result = v.validate(&contract);
        assert!(result.is_ok(), "valid contract should pass");
    }

    // === T9: validate 不支持版本失败（schema_version="0.9.0"） ===
    #[test]
    fn t9_validate_unsupported_version() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.schema_version = String::from("0.9.0");
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::UnsupportedVersion(_))),
            "expected UnsupportedVersion"
        );
    }

    // === T10: validate 缺 request_id 失败（空字符串） ===
    #[test]
    fn t10_validate_missing_request_id() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.request_id = String::from("");
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::MissingField(_))),
            "expected MissingField"
        );
    }

    // === T11: validate 空 reason 失败（D12，reason=""） ===
    #[test]
    fn t11_validate_empty_reason() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.intent.reason = String::from("");
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::MissingField(_))),
            "expected MissingField for empty reason"
        );
    }

    // === T12: validate confidence > 1.0 失败 ===
    #[test]
    fn t12_validate_confidence_over_one() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.intent.confidence = 1.5;
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::InvalidValue(_, _))),
            "expected InvalidValue for confidence > 1.0"
        );
    }

    // === T13: validate priority=0 失败（超下界） ===
    #[test]
    fn t13_validate_priority_zero() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.intent.priority = 0;
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::InvalidValue(_, _))),
            "expected InvalidValue for priority=0"
        );
    }

    // === T14: validate time_range 倒置失败（start=5, end=0） ===
    #[test]
    fn t14_validate_time_range_inverted() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.intent.time_range = Some(TimeRange {
            start_period: 5,
            end_period: 0,
        });
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::InvalidValue(_, _))),
            "expected InvalidValue for inverted time_range"
        );
    }

    // === T15: validate soc_target target_soc=1.5 失败 ===
    #[test]
    fn t15_validate_soc_target_out_of_range() {
        let v = ContractValidator::new();
        let mut contract = make_valid_contract();
        contract.intent.soc_target = Some(SocIntent {
            target_soc: 1.5,
            by_period: 10,
        });
        let result = v.validate(&contract);
        assert!(
            matches!(result, Err(ContractError::InvalidValue(_, _))),
            "expected InvalidValue for target_soc=1.5"
        );
    }

    // === T16: is_compatible("1.0.0")==true, is_compatible("0.9.0")==false ===
    #[test]
    fn t16_is_compatible() {
        let v = ContractValidator::new();
        assert!(v.is_compatible("1.0.0"));
        assert!(v.is_compatible("1.1.0"));
        assert!(!v.is_compatible("0.9.0"));
    }

    // === T17: to_solver_params 正向转换（使用 Charge intent + SystemState::default()） ===
    #[test]
    fn t17_to_solver_params() {
        let converter = ContractConverter::default();
        let contract = make_valid_contract();
        let state = SystemState::default();
        let result = converter.to_solver_params(&contract, &state);
        assert!(result.is_ok(), "to_solver_params should succeed");
        let (config, problem) = result.unwrap();
        // Charge intent with power_kw=50.0, time_range 0-5
        // price[0..=5] should be -50.0 (|50|.min(100) = 50)
        assert!(
            (config.price[0] - (-50.0)).abs() < 1e-9,
            "price[0] should be -50.0"
        );
        assert!(
            (config.price[5] - (-50.0)).abs() < 1e-9,
            "price[5] should be -50.0"
        );
        // 范围外保持默认 0.5
        assert!(
            (config.price[6] - 0.5).abs() < 1e-9,
            "price[6] should be 0.5"
        );
        // LP problem variables = 3 * 96 = 288
        assert_eq!(problem.variables.len(), 288);
    }

    // === T18: to_feedback 反向转换（构造 SolveResult/ScheduleResult/ValidationResult） ===
    #[test]
    fn t18_to_feedback() {
        let converter = ContractConverter::default();
        let solve_result = SolveResult::optimal(42.0, vec![0.0; 288]);

        // Case 1: validation passed, no violations → clamp_info = None
        let validation_pass = ValidationResult {
            passed: true,
            clamped: false,
            clamped_schedule: None,
            violations: vec![],
        };
        let schedule = ScheduleResult {
            schedule: vec![ScheduleEntry {
                period: 0,
                charge_power_kw: 10.0,
                discharge_power_kw: 50.0,
                net_power_kw: 40.0,
                soc_pct: 0.5,
                revenue_yuan: 5.0,
            }],
            total_revenue_yuan: 5.0,
            objective_value: 42.0,
            solve_status: SolveStatus::Optimal,
        };
        let feedback =
            converter.to_feedback("req-001", &solve_result, &validation_pass, &schedule, 100);
        assert_eq!(feedback.request_id, "req-001");
        assert!(matches!(feedback.solve_status, SolveStatus::Optimal));
        assert!(feedback.validation_passed);
        assert!(feedback.clamp_info.is_none());
        assert!(feedback.executed_schedule.is_some());
        assert!(
            (feedback.actual_revenue - 5.0).abs() < 1e-9,
            "actual_revenue should be 5.0"
        );
        assert_eq!(feedback.solve_ms, 100);

        // Case 2: validation with violations → clamp_info = Some
        let validation_fail = ValidationResult {
            passed: false,
            clamped: true,
            clamped_schedule: None,
            violations: vec![Violation {
                rule: String::from("electrical_safety"),
                period: 0,
                field: String::from("charge_power"),
                original_value: 120.0,
                safe_value: 100.0,
                severity: Severity::Critical,
            }],
        };
        let feedback2 =
            converter.to_feedback("req-002", &solve_result, &validation_fail, &schedule, 200);
        assert!(!feedback2.validation_passed);
        assert!(feedback2.clamp_info.is_some());
        assert_eq!(feedback2.clamp_info.as_ref().unwrap().len(), 1);
        assert_eq!(feedback2.solve_ms, 200);
    }

    // === T19: serialize_feedback JSON 输出包含 "request_id" ===
    #[test]
    fn t19_serialize_feedback() {
        let converter = ContractConverter::default();
        let feedback = FeedbackContract {
            request_id: String::from("req-001"),
            solve_status: SolveStatus::Optimal,
            validation_passed: true,
            clamp_info: None,
            executed_schedule: Some(vec![]),
            actual_revenue: 100.0,
            solve_ms: 50,
        };
        let json = converter
            .serialize_feedback(&feedback)
            .expect("serialize failed");
        assert!(
            json.contains("request_id"),
            "json should contain request_id field"
        );
        assert!(json.contains("req-001"));
    }

    // === T20: 端到端 JSON → Contract → Validate → SolverParams ===
    #[test]
    fn t20_end_to_end_json_to_solver_params() {
        let contract = make_valid_contract();
        let json = serde_json::to_string(&contract).expect("serialize failed");
        let parsed: IntentContract = serde_json::from_str(&json).expect("deserialize failed");
        let validator = ContractValidator::new();
        assert!(
            validator.validate(&parsed).is_ok(),
            "parsed contract should validate"
        );
        let converter = ContractConverter::default();
        let state = SystemState::default();
        let result = converter.to_solver_params(&parsed, &state);
        assert!(result.is_ok(), "to_solver_params should succeed");
        let (config, _problem) = result.unwrap();
        assert!(
            (config.price[0] - (-50.0)).abs() < 1e-9,
            "price[0] should be -50.0 after round-trip"
        );
    }

    // === T21: IntentContract round-trip（序列化后反序列化等价） ===
    #[test]
    fn t21_intent_contract_round_trip() {
        let contract = make_valid_contract();
        let json = serde_json::to_string(&contract).expect("serialize failed");
        let parsed: IntentContract = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.schema_version, contract.schema_version);
        assert_eq!(parsed.request_id, contract.request_id);
        assert_eq!(parsed.timestamp, contract.timestamp);
        assert!(matches!(parsed.intent.intent_type, IntentType::Charge));
        assert_eq!(parsed.intent.priority, contract.intent.priority);
        assert_eq!(parsed.intent.reason, contract.intent.reason);
        assert!((parsed.intent.confidence - contract.intent.confidence).abs() < 1e-9);
        assert!((parsed.context.current_soc - contract.context.current_soc).abs() < 1e-9);
        assert_eq!(parsed.llm_meta.model_name, contract.llm_meta.model_name);
    }

    // === T22: ContractConverter::default 等价 new(ScheduleConfig::default()) ===
    #[test]
    fn t22_converter_default_equivalent() {
        let default = ContractConverter::default();
        let explicit = ContractConverter::new(ScheduleConfig::default());
        // 两者均成功构造；验证 default_config.num_periods 一致（无 PartialEq，逐字段抽样）
        assert_eq!(default.default_config.num_periods, 96);
        assert_eq!(explicit.default_config.num_periods, 96);
        assert!(
            (default.default_config.pcs_power_kw - explicit.default_config.pcs_power_kw).abs()
                < 1e-9
        );
    }
}
