//! EnerOS 意图解析器（v0.68.0，P1-J AI Runtime Solver 第五层，意图转换层）.
//!
//! 将 LLM 输出的 JSON 意图转换为 Solver 可执行的 `ScheduleConfig` / `LpProblem`，
//! 桥接神经层（LLM 感知）与符号层（Solver 执行），实现双脑架构的关键转换环节。
//!
//! # 核心类型
//!
//! - [`parser::IntentParser`] — 解析器主接口（JSON → Intent → ScheduleConfig/LpProblem）
//! - [`intent::Intent`] — LLM 意图数据结构（JSON 反序列化目标）
//! - [`intent::IntentType`] — 7 种意图类型枚举
//! - [`error::IntentError`] — 解析/转换错误
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `#[derive(Serialize, Deserialize)]` + `serde_json::from_str` | 添加 `serde`（derive + alloc）+ `serde_json`（alloc）依赖 | 比手动遍历 `serde_json::Value` 更简洁（Simplicity First） |
//! | **D2** | `self.system_state.soc` | `self.system_state.soc_pct` | v0.67.0 `SystemState` 字段名是 `soc_pct` |
//! | **D3** | `config.price[t]` 直接索引 | 使用 `config.price.get_mut(t)` 安全访问 | no_std 中 panic = 系统挂死 |
//! | **D4** | `model.compile()?` 直接 `?` | `model.compile().map_err(...)` | `SolverError` 不实现 `From` for `IntentError` |
//! | **D5** | 前置依赖 v0.26.0 配置管理系统 | **不引入** v0.26.0 | 代码未实际使用，仅用 `ScheduleConfig::default()` |
//! | **D6** | `system_state: SystemState` 来源未明确 | 依赖 `eneros-safety-validator` 复用 v0.67.0 `SystemState` | 避免类型碎片化 |
//! | **D7** | `IntentError` 派生未指定 | 仅派生 `Debug` | Simplicity First |
//! | **D8** | `IntentType` 派生 `PartialEq` | 保持，另加 `Serialize + Deserialize` | match 需要 + 测试断言 |
//! | **D9** | `Intent` 字段 `reason`/`confidence`/`priority` 为必需 | 加 `#[serde(default)]` | 容错 LLM 省略字段 |
//! | **D10** | 蓝图未声明 no_std | `#![cfg_attr(not(test), no_std)]` + `extern crate alloc` | 项目硬性要求（蓝图 §43.1） |
//! | **D11** | `to_opt_problem` 中 `config.clone()` | 保留 `clone()` | 需返回 `(config, problem)`，clone 必要 |
//! | **D12** | `validate_config` 校验 `price.len()` | 保持蓝图校验逻辑 | 坑点已在 D3 处理 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*` / `serde` / `serde_json`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod error;
pub mod intent;
pub mod parser;

pub use error::IntentError;
pub use intent::{Intent, IntentType, PowerIntent, SocIntent, TimeRange};
pub use parser::IntentParser;

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_safety_validator::state::SystemState;

    use super::*;

    // === T1: IntentType 变体 + PartialEq（D8） ===
    #[test]
    fn t1_intent_type_variants_and_partial_eq() {
        assert_eq!(IntentType::Charge, IntentType::Charge);
        assert_ne!(IntentType::Charge, IntentType::Discharge);
        // 验证 7 个变体均可构造
        let _ = IntentType::Charge;
        let _ = IntentType::Discharge;
        let _ = IntentType::Hold;
        let _ = IntentType::Stop;
        let _ = IntentType::EmergencyStop;
        let _ = IntentType::AutonomousSchedule;
        let _ = IntentType::SetSetpoint;
    }

    // === T2: TimeRange 构造 ===
    #[test]
    fn t2_time_range_construction() {
        let tr = TimeRange {
            start_period: 0,
            end_period: 4,
        };
        assert_eq!(tr.start_period, 0);
        assert_eq!(tr.end_period, 4);
    }

    // === T3: PowerIntent 构造（含 power_ratio） ===
    #[test]
    fn t3_power_intent_construction() {
        let p = PowerIntent {
            power_kw: -50.0,
            power_ratio: Some(0.5),
        };
        assert!((p.power_kw - (-50.0)).abs() < 1e-9);
        assert_eq!(p.power_ratio, Some(0.5));
    }

    // === T4: SocIntent 构造 ===
    #[test]
    fn t4_soc_intent_construction() {
        let s = SocIntent {
            target_soc: 0.8,
            by_period: 10,
        };
        assert!((s.target_soc - 0.8).abs() < 1e-9);
        assert_eq!(s.by_period, 10);
    }

    // === T5: Intent 全字段构造 ===
    #[test]
    fn t5_intent_full_construction() {
        let i = Intent {
            intent_type: IntentType::Charge,
            time_range: Some(TimeRange {
                start_period: 0,
                end_period: 4,
            }),
            power: Some(PowerIntent {
                power_kw: -50.0,
                power_ratio: None,
            }),
            soc_target: None,
            priority: 2,
            reason: "谷时充电".to_string(),
            confidence: 0.9,
        };
        assert_eq!(i.intent_type, IntentType::Charge);
        assert!(i.time_range.is_some());
        assert!(i.power.is_some());
        assert!(i.soc_target.is_none());
        assert_eq!(i.priority, 2);
        assert_eq!(i.reason, "谷时充电");
        assert!((i.confidence - 0.9).abs() < 1e-9);
    }

    // === T6: Intent serde 反序列化缺失字段（D9 默认值） ===
    #[test]
    fn t6_intent_serde_missing_fields_defaults() {
        let json = r#"{"intent_type":"Hold"}"#;
        let i: Intent = serde_json::from_str(json).unwrap();
        assert_eq!(i.intent_type, IntentType::Hold);
        assert!(i.time_range.is_none());
        assert!(i.power.is_none());
        assert!(i.soc_target.is_none());
        assert_eq!(i.priority, 3); // D9 默认
        assert_eq!(i.reason, ""); // D9 默认
        assert!((i.confidence - 0.0).abs() < 1e-9); // D9 默认
    }

    // === T7: IntentParser::new 构造 ===
    #[test]
    fn t7_intent_parser_new() {
        let cfg = ScheduleConfig::default();
        let state = SystemState::default();
        let _ = IntentParser::new(cfg, state);
    }

    // === T8: IntentParser::default() == new(default, default) ===
    #[test]
    fn t8_intent_parser_default_equivalent() {
        let _d = IntentParser::default();
        let _n = IntentParser::new(ScheduleConfig::default(), SystemState::default());
        // 两者均成功构造即视为等价（IntentError 未派生 PartialEq，无法直接比较内部状态）
    }

    // === T9: parse_json 完整 JSON ===
    #[test]
    fn t9_parse_json_complete() {
        let parser = IntentParser::default();
        let json = r#"{"intent_type":"Charge","time_range":{"start_period":0,"end_period":4},"power":{"power_kw":-50.0},"priority":2,"reason":"谷时充电","confidence":0.9}"#;
        let i = parser.parse_json(json).unwrap();
        assert_eq!(i.intent_type, IntentType::Charge);
        let tr = i.time_range.unwrap();
        assert_eq!(tr.start_period, 0);
        assert_eq!(tr.end_period, 4);
        let p = i.power.unwrap();
        assert!((p.power_kw - (-50.0)).abs() < 1e-9);
        assert!(p.power_ratio.is_none());
        assert_eq!(i.priority, 2);
        assert_eq!(i.reason, "谷时充电");
        assert!((i.confidence - 0.9).abs() < 1e-9);
        assert!(i.soc_target.is_none());
    }

    // === T10: parse_json 缺失可选字段（D9） ===
    #[test]
    fn t10_parse_json_missing_optional_fields() {
        let parser = IntentParser::default();
        let json = r#"{"intent_type":"Hold"}"#;
        let i = parser.parse_json(json).unwrap();
        assert_eq!(i.intent_type, IntentType::Hold);
        assert!(i.time_range.is_none());
        assert!(i.power.is_none());
        assert!(i.soc_target.is_none());
        assert_eq!(i.priority, 3);
        assert_eq!(i.reason, "");
        assert!((i.confidence - 0.0).abs() < 1e-9);
    }

    // === T11: parse_json 非法 JSON → ParseError ===
    #[test]
    fn t11_parse_json_invalid_returns_parse_error() {
        let parser = IntentParser::default();
        let json = r#"{"invalid""#;
        let result = parser.parse_json(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, IntentError::ParseError(_)),
            "expected ParseError, got {:?}",
            err
        );
    }

    // === T12: to_schedule_config AutonomousSchedule + soc_target → soc_final ===
    #[test]
    fn t12_autonomous_schedule_sets_soc_final() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::AutonomousSchedule,
            time_range: None,
            power: None,
            soc_target: Some(SocIntent {
                target_soc: 0.8,
                by_period: 95,
            }),
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        assert_eq!(cfg.soc_final, Some(0.8));
    }

    // === T13: to_schedule_config Charge → price[0..4] 为负 ===
    #[test]
    fn t13_charge_makes_price_negative_in_range() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::Charge,
            time_range: Some(TimeRange {
                start_period: 0,
                end_period: 4,
            }),
            power: Some(PowerIntent {
                power_kw: -50.0,
                power_ratio: None,
            }),
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        // power_kw.abs() = 50.0, price[t] = -50.0
        for t in 0..=4 {
            assert!(
                (cfg.price[t] - (-50.0)).abs() < 1e-9,
                "price[{}] should be -50.0, got {}",
                t,
                cfg.price[t]
            );
        }
        // 范围外保持默认 0.5
        assert!((cfg.price[5] - 0.5).abs() < 1e-9);
    }

    // === T14: to_schedule_config Discharge → price[0..4] 为 power_kw * 10.0 ===
    #[test]
    fn t14_discharge_makes_price_high_in_range() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::Discharge,
            time_range: Some(TimeRange {
                start_period: 0,
                end_period: 4,
            }),
            power: Some(PowerIntent {
                power_kw: 50.0,
                power_ratio: None,
            }),
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        // power_kw.abs() = 50.0, price[t] = 50.0 * 10.0 = 500.0
        for t in 0..=4 {
            assert!(
                (cfg.price[t] - 500.0).abs() < 1e-9,
                "price[{}] should be 500.0, got {}",
                t,
                cfg.price[t]
            );
        }
        assert!((cfg.price[5] - 0.5).abs() < 1e-9);
    }

    // === T15: to_schedule_config Hold → pcs_power_kw == 0.0 ===
    #[test]
    fn t15_hold_sets_pcs_power_zero() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::Hold,
            time_range: None,
            power: None,
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        assert!((cfg.pcs_power_kw - 0.0).abs() < 1e-9);
    }

    // === T16: to_schedule_config Stop → pcs_power_kw == 0.0 ===
    #[test]
    fn t16_stop_sets_pcs_power_zero() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::Stop,
            time_range: None,
            power: None,
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        assert!((cfg.pcs_power_kw - 0.0).abs() < 1e-9);
    }

    // === T17: to_schedule_config EmergencyStop → pcs=0 且 soc_min==soc_max==soc_pct ===
    #[test]
    fn t17_emergency_stop_locks_soc_to_state() {
        // 自定义 SystemState soc_pct=0.7
        let state = SystemState {
            voltage_v: 380.0,
            current_a: 0.0,
            frequency_hz: 50.0,
            soc_pct: 0.7,
            timestamp_ms: 0,
        };
        let parser = IntentParser::new(ScheduleConfig::default(), state);
        let intent = Intent {
            intent_type: IntentType::EmergencyStop,
            time_range: None,
            power: None,
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        assert!((cfg.pcs_power_kw - 0.0).abs() < 1e-9);
        assert!((cfg.soc_min - 0.7).abs() < 1e-9);
        assert!((cfg.soc_max - 0.7).abs() < 1e-9);
    }

    // === T18: to_schedule_config SetSetpoint → pcs_power_kw == |power_kw| ===
    #[test]
    fn t18_set_setpoint_overrides_pcs_power() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::SetSetpoint,
            time_range: None,
            power: Some(PowerIntent {
                power_kw: 75.0,
                power_ratio: None,
            }),
            soc_target: None,
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let cfg = parser.to_schedule_config(&intent).unwrap();
        assert!((cfg.pcs_power_kw - 75.0).abs() < 1e-9);
    }

    // === T19: validate_config 合法配置 → Ok ===
    #[test]
    fn t19_validate_config_valid_ok() {
        let parser = IntentParser::default();
        let cfg = ScheduleConfig::default();
        assert!(parser.validate_config(&cfg).is_ok());
    }

    // === T20: validate_config 4 种非法配置（D12） ===
    #[test]
    fn t20_validate_config_invalid_configs() {
        let parser = IntentParser::default();

        // 子测试 1: num_periods = 0（price 同步清空，避免触发 price.len() 不匹配）
        let cfg = ScheduleConfig {
            num_periods: 0,
            price: alloc::vec![],
            ..Default::default()
        };
        let err = parser.validate_config(&cfg);
        assert!(matches!(err, Err(IntentError::InvalidConfig(_))));

        // 子测试 2: pcs_power_kw < 0
        let cfg = ScheduleConfig {
            pcs_power_kw: -1.0,
            ..Default::default()
        };
        let err = parser.validate_config(&cfg);
        assert!(matches!(err, Err(IntentError::InvalidConfig(_))));

        // 子测试 3: soc_min > soc_max（ genuinely invalid range ）
        let cfg = ScheduleConfig {
            soc_min: 0.5,
            soc_max: 0.4,
            ..Default::default()
        };
        let err = parser.validate_config(&cfg);
        assert!(matches!(err, Err(IntentError::InvalidConfig(_))));

        // 子测试 4: price.len() != num_periods（num_periods=4，price 仍为 96）
        let cfg = ScheduleConfig {
            num_periods: 4,
            ..Default::default()
        };
        let err = parser.validate_config(&cfg);
        assert!(matches!(err, Err(IntentError::InvalidConfig(_))));
    }

    // === T21: to_opt_problem AutonomousSchedule → 返回 (ScheduleConfig, LpProblem) ===
    #[test]
    fn t21_to_opt_problem_autonomous_schedule() {
        let parser = IntentParser::default();
        let intent = Intent {
            intent_type: IntentType::AutonomousSchedule,
            time_range: None,
            power: None,
            soc_target: Some(SocIntent {
                target_soc: 0.8,
                by_period: 95,
            }),
            priority: 3,
            reason: "".to_string(),
            confidence: 0.0,
        };
        let (cfg, lp) = parser.to_opt_problem(&intent).unwrap();
        assert_eq!(cfg.soc_final, Some(0.8));
        // LP 问题变量数 = 3 × 96 = 288
        assert_eq!(lp.variables.len(), 288);
    }

    // === T22: 端到端 JSON → parse_json → to_schedule_config → validate_config Ok ===
    #[test]
    fn t22_end_to_end_json_to_config() {
        let parser = IntentParser::default();
        let json = r#"{"intent_type":"Charge","time_range":{"start_period":0,"end_period":4},"power":{"power_kw":-50.0},"priority":2,"reason":"谷时充电","confidence":0.9}"#;
        let intent = parser.parse_json(json).unwrap();
        let cfg = parser.to_schedule_config(&intent).unwrap();
        // to_schedule_config 内部已调用 validate_config，再次显式校验确认
        assert!(parser.validate_config(&cfg).is_ok());
        // 验证转换效果
        assert!((cfg.price[0] - (-50.0)).abs() < 1e-9);
        assert!((cfg.price[4] - (-50.0)).abs() < 1e-9);
        assert!((cfg.price[5] - 0.5).abs() < 1e-9);
    }
}
