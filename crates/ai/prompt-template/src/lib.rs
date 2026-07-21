//! EnerOS v0.63.0 Prompt 模板系统 + JSON 输出约束.
//!
//! P1-I LLM 推理层收官版本。构建电力专用 Prompt 模板系统和 JSON 输出约束
//! （Schema 校验 + 重试机制），作为 LLM → Solver 的桥梁。
//!
//! # 偏差声明（D1~D12，应用 Karpathy 原则）
//!
//! | ID | 蓝图原文 | 偏差说明 | 理由 |
//! |----|---------|---------|------|
//! | D1 | `pub trait PromptTemplate: Send + Sync` | 不派生 `Send + Sync` | 与 v0.59.0 `LlmEngine` trait 保持一致；单线程 no_std 无需 Send/Sync |
//! | D2 | `LlmError::JsonParseFailed`（蓝图伪代码引用） | 新增独立 `TemplateError` 枚举 | v0.59.0 `LlmError` 仅 8 变体，无 `JsonParseFailed`；JSON 解析/Schema 校验失败属于模板层错误 |
//! | D3 | `infer_with_constraint(...) -> Result<Value, LlmError>` | 返回 `Result<Value, TemplateError>` | 错误类型分离：推理错误通过 `From<LlmError>` 转换为 `TemplateError::Engine(_)` |
//! | D4 | `lazy_static! { static ref CHARGE_DISCHARGE_SCHEMA: JsonSchema = json!({...}); }` | 改用 `SchemaSpec` 结构体 + `&'static` 静态字段 | `lazy_static!` 是 std-only；no_std 下用编译期常量，运行时零分配 |
//! | D5 | 完整 JSON Schema draft 7+ 校验 | 实现最小验证器：required / type / enum / minimum / maximum | 电力场景仅需字段存在性、类型、枚举值、数值范围；完整 JSON Schema 是过度工程 |
//! | D6 | `log_warn!("JSON parse attempt {} failed: {:?}", attempt, e)` | 静默重试，失败计入 `ConstraintStats` 统计 | `log_warn!` 在 no_std 不可用；统计计数器更适合 no_std 场景 |
//! | D7 | 蓝图 Python 测试代码（`test_prompt_template_gpu`） | 实现等价 Rust 测试（`MockEngine` + 固定 JSON 输出） | 项目规则：v0.63.0 是 Rust no_std；GPU 优先测试通过 `ComputeDevice` 控制（v0.59.0 已实现） |
//! | D8 | crate 位置未明确 | 路径 `crates/ai/prompt-template/` | 遵循 §2.3.1 crate 分组规则（AI 子系统） |
//! | D9 | `JsonSchema = serde_json::Value`（蓝图类型混用） | 分离 `SchemaSpec`（验证规范）和 `serde_json::Value`（解析输出） | 蓝图将"JSON Schema 验证规范"与"JSON 解析结果"混为 `serde_json::Value` |
//! | D10 | 无 `unsafe` 块声明 | 显式声明无 `unsafe`（纯 safe Rust） | Prompt 模板不涉及 FFI/内存操作；与 v0.62.0 一致 |
//! | D11 | 依赖未明确 | 仅依赖 `eneros-llm-engine` + `serde_json`（alloc feature） | 不依赖 v0.60.0/v0.61.0/v0.62.0 — Prompt 模板与模型加载/部署/调度解耦 |
//! | D12 | 无 feature 门控声明 | 无 `[features]` 段（纯 Rust，无 FFI） | `infer_with_constraint` 通过 `&mut dyn LlmEngine` trait 对象间接调用 llama.cpp |

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod constraint;
pub mod context;
pub mod error;
pub mod extract;
pub mod schema;
pub mod template;
pub mod templates;

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T15（覆盖 D1~D12 偏差声明与 checklist）.

    use eneros_llm_engine::device::ComputeDevice;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_llm_engine::error::LlmError;
    use eneros_llm_engine::params::InferParams;
    use eneros_llm_engine::stats::{EngineHealth, EngineStats};
    use serde_json::Value;

    use super::constraint::JsonConstraint;
    use super::context::TemplateContext;
    use super::error::TemplateError;
    use super::extract::extract_json;
    use super::template::PromptTemplate;
    use super::templates::{AlarmTemplate, ChargeDischargeTemplate, DispatchTemplate};

    /// 可切换输出的测试 Mock 引擎（T13~T15）.
    ///
    /// v0.59.0 `MockEngine::infer` 固定返回 `"mock: <prompt>"`（忽略 `mock_output`），
    /// 无法产出校验通过的 JSON，故此处实现按调用顺序返回预设输出的 mock。
    struct SwitchableMockEngine {
        outputs: Vec<String>,
        call_count: usize,
        loaded: bool,
        stats: EngineStats,
    }

    impl SwitchableMockEngine {
        fn new(outputs: Vec<String>) -> Self {
            Self {
                outputs,
                call_count: 0,
                loaded: true,
                stats: EngineStats::default(),
            }
        }
    }

    impl LlmEngine for SwitchableMockEngine {
        fn load_model(&mut self, _path: &str) -> Result<(), LlmError> {
            self.loaded = true;
            Ok(())
        }

        fn infer(&mut self, _prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
            if !self.loaded {
                return Err(LlmError::ModelNotLoaded);
            }
            if self.call_count >= self.outputs.len() {
                return Err(LlmError::InferFailed);
            }
            let out = self.outputs[self.call_count].clone();
            self.call_count += 1;
            Ok(out)
        }

        fn infer_stream(
            &mut self,
            _prompt: &str,
            _params: &InferParams,
            _callback: &mut dyn FnMut(&str) -> bool,
        ) -> Result<(), LlmError> {
            if !self.loaded {
                return Err(LlmError::ModelNotLoaded);
            }
            Ok(())
        }

        fn model_info(&self) -> Option<&eneros_llm_engine::ModelInfo> {
            None
        }

        fn health_check(&self) -> EngineHealth {
            EngineHealth {
                loaded: self.loaded,
                device: ComputeDevice::Cpu,
                gpu_layers: 0,
                last_error: None,
            }
        }

        fn stats(&self) -> &EngineStats {
            &self.stats
        }
    }

    // ===== T1：TemplateContext::new 构造 + 字段访问 =====
    #[test]
    fn test_t1_template_context_new() {
        let ctx = TemplateContext::new(
            0.8,
            65.0,
            -30.0,
            28.5,
            String::from("峰时"),
            vec![1.0, 2.0, 3.0],
        );
        assert_eq!(ctx.market_price, 0.8);
        assert_eq!(ctx.soc, 65.0);
        assert_eq!(ctx.power_current, -30.0);
        assert_eq!(ctx.temperature, 28.5);
        assert_eq!(ctx.time_of_day, "峰时");
        assert_eq!(ctx.historical_data, vec![1.0, 2.0, 3.0]);
    }

    // ===== T2：TemplateContext::default 默认值 =====
    #[test]
    fn test_t2_template_context_default() {
        let ctx = TemplateContext::default();
        assert_eq!(ctx.market_price, 0.5);
        assert_eq!(ctx.soc, 50.0);
        assert_eq!(ctx.power_current, 0.0);
        assert_eq!(ctx.temperature, 25.0);
        assert_eq!(ctx.time_of_day, "谷时");
        assert!(ctx.historical_data.is_empty());
    }

    // ===== T3：SchemaSpec::validate 有效 JSON 通过 =====
    #[test]
    fn test_t3_schema_validate_valid_json() {
        let template = ChargeDischargeTemplate;
        let json: Value = serde_json::from_str(
            r#"{"action":"charge","power_kw":-50.0,"reason":"low price","confidence":0.9}"#,
        )
        .unwrap();
        assert!(template.output_schema().validate(&json).is_ok());
    }

    // ===== T4：SchemaSpec::validate 缺 required 字段失败 =====
    #[test]
    fn test_t4_schema_validate_missing_field() {
        let template = ChargeDischargeTemplate;
        // 缺少 confidence
        let json: Value =
            serde_json::from_str(r#"{"action":"charge","power_kw":-50.0,"reason":"low price"}"#)
                .unwrap();
        let r = template.output_schema().validate(&json);
        assert!(matches!(r, Err(TemplateError::SchemaValidation(_))));
    }

    // ===== T5：SchemaSpec::validate 类型不匹配失败 =====
    #[test]
    fn test_t5_schema_validate_type_mismatch() {
        let template = ChargeDischargeTemplate;
        // action 应为 string，实为 number
        let json: Value = serde_json::from_str(
            r#"{"action":123,"power_kw":-50.0,"reason":"x","confidence":0.9}"#,
        )
        .unwrap();
        let r = template.output_schema().validate(&json);
        assert!(matches!(r, Err(TemplateError::SchemaValidation(_))));
    }

    // ===== T6：SchemaSpec::validate 枚举值不合法失败 =====
    #[test]
    fn test_t6_schema_validate_invalid_enum() {
        let template = ChargeDischargeTemplate;
        // action 非法值
        let json: Value = serde_json::from_str(
            r#"{"action":"invalid","power_kw":-50.0,"reason":"x","confidence":0.9}"#,
        )
        .unwrap();
        let r = template.output_schema().validate(&json);
        assert!(matches!(r, Err(TemplateError::SchemaValidation(_))));
    }

    // ===== T7：extract_json 纯 JSON 提取 =====
    #[test]
    fn test_t7_extract_json_pure() {
        let s = extract_json(r#"{"a":1}"#).unwrap();
        assert_eq!(s, r#"{"a":1}"#);
    }

    // ===== T8：extract_json markdown 代码块提取 =====
    #[test]
    fn test_t8_extract_json_markdown() {
        let input = "```json\n{\"a\":1}\n```";
        let s = extract_json(input).unwrap();
        assert_eq!(s, r#"{"a":1}"#);
    }

    // ===== T9：extract_json 含多余文字提取 =====
    #[test]
    fn test_t9_extract_json_with_extra_text() {
        let input = "The result is: {\"a\":1} done";
        let s = extract_json(input).unwrap();
        assert_eq!(s, r#"{"a":1}"#);
    }

    // ===== T10：extract_json 无 JSON 返回 NoJson =====
    #[test]
    fn test_t10_extract_json_no_json() {
        let r = extract_json("no json here");
        assert!(matches!(r, Err(TemplateError::NoJson)));
    }

    // ===== T11：ChargeDischargeTemplate build 输出含 ctx 参数 =====
    #[test]
    fn test_t11_charge_discharge_template_build() {
        let ctx =
            TemplateContext::new(0.8, 65.0, -30.0, 28.5, String::from("峰时"), vec![1.0, 2.0]);
        let prompt = ChargeDischargeTemplate.build(&ctx);
        assert!(prompt.contains("0.8"));
        assert!(prompt.contains("65"));
        assert!(prompt.contains("-30"));
        assert!(prompt.contains("28.5"));
        assert!(prompt.contains("峰时"));
        assert!(prompt.contains("action"));
    }

    // ===== T12：DispatchTemplate + AlarmTemplate build 输出 =====
    #[test]
    fn test_t12_dispatch_alarm_template_build() {
        let ctx = TemplateContext::default();
        let dp = DispatchTemplate.build(&ctx);
        assert!(dp.contains("target_power"));
        assert!(dp.contains("ramp_rate"));
        assert!(dp.contains("功率调度"));
        let ap = AlarmTemplate.build(&ctx);
        assert!(ap.contains("alarm_type"));
        assert!(ap.contains("severity"));
        assert!(ap.contains("告警"));
    }

    // ===== T13：JsonConstraint 首次推理成功 =====
    #[test]
    fn test_t13_constraint_first_try_success() {
        let valid = r#"{"action":"charge","power_kw":-50.0,"reason":"low price","confidence":0.9}"#;
        let mut engine = SwitchableMockEngine::new(vec![String::from(valid)]);
        let mut constraint = JsonConstraint::new(2);
        let ctx = TemplateContext::default();
        let result = constraint.infer_with_constraint(&mut engine, &ChargeDischargeTemplate, &ctx);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v["action"].as_str(), Some("charge"));
        let stats = constraint.stats();
        assert_eq!(stats.total_attempts, 1);
        assert_eq!(stats.successful, 1);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.retries, 0);
    }

    // ===== T14：JsonConstraint 重试成功（首次无效，第二次有效）=====
    #[test]
    fn test_t14_constraint_retry_success() {
        let invalid = "no json here";
        let valid =
            r#"{"action":"discharge","power_kw":40.0,"reason":"high price","confidence":0.85}"#;
        let mut engine =
            SwitchableMockEngine::new(vec![String::from(invalid), String::from(valid)]);
        let mut constraint = JsonConstraint::new(2);
        let ctx = TemplateContext::default();
        let result = constraint.infer_with_constraint(&mut engine, &ChargeDischargeTemplate, &ctx);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v["action"].as_str(), Some("discharge"));
        let stats = constraint.stats();
        assert_eq!(stats.total_attempts, 1);
        assert_eq!(stats.successful, 1);
        assert_eq!(stats.retries, 1);
        assert_eq!(stats.failed, 0);
    }

    // ===== T15：JsonConstraint 重试耗尽返回 MaxRetriesExceeded + 统计累加 =====
    #[test]
    fn test_t15_constraint_max_retries_exceeded() {
        let invalid = "no json";
        // max_retries=2 → 3 次尝试，提供 3 个无效输出
        let mut engine = SwitchableMockEngine::new(vec![
            String::from(invalid),
            String::from(invalid),
            String::from(invalid),
        ]);
        let mut constraint = JsonConstraint::new(2);
        let ctx = TemplateContext::default();
        let result = constraint.infer_with_constraint(&mut engine, &ChargeDischargeTemplate, &ctx);
        assert!(matches!(result, Err(TemplateError::MaxRetriesExceeded)));
        let stats = constraint.stats();
        assert_eq!(stats.total_attempts, 1);
        assert_eq!(stats.successful, 0);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.retries, 0);
    }
}
