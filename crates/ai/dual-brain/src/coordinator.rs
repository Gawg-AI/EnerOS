//! 双脑协调器 + 双脑结果（D3/D4/D5/D6/D8/D9）.
//!
//! 端到端编排双脑链路：路径选择 → 感知 → LLM 推理 → 意图解析 → LP 求解 →
//! 安全校验 → 命令下发。快路径跳过 LLM，慢路径执行完整 7 步。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::model::EnergyScheduleModel;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_fast_path::engine::RealtimePathEngine;
use eneros_fast_path::selector::{PathSelector, PathType};
use eneros_fast_path::state::RealtimeState;
use eneros_intent_contract::contract::{
    DeviceStatus, FeedbackContract, IntentContract, LlmMeta, SystemContext,
};
use eneros_intent_contract::converter::ContractConverter;
use eneros_intent_contract::validator::ContractValidator;
use eneros_intent_parser::parser::IntentParser;
use eneros_llm_engine::engine::LlmEngine;
use eneros_llm_engine::params::InferParams;
// 以下类型仅 mock 引擎（cfg(any(test, feature="mock"))）使用。
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::device::ComputeDevice;
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::error::LlmError;
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::model::{ModelInfo, Quantization};
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::stats::{EngineHealth, EngineStats};
use eneros_prompt_template::context::TemplateContext;
use eneros_prompt_template::template::PromptTemplate;
use eneros_prompt_template::templates::ChargeDischargeTemplate;
use eneros_safety_validator::state::SystemState;
use eneros_safety_validator::validator::SafetyValidator;
#[cfg(any(test, feature = "mock"))]
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::error::DualBrainError;
use crate::latency::LatencyBreakdown;
use crate::sink::{CommandSink, DispatchCommand};
#[cfg(any(test, feature = "mock"))]
use crate::sink::MockCommandSink;

/// Mock LLM 输出（合法 Charge intent JSON，满足 ContractValidator 6 项校验）.
#[cfg(any(test, feature = "mock"))]
const MOCK_INTENT_JSON: &str = r#"{"intent_type":"Charge","power":{"power_kw":50.0,"power_ratio":0.5},"time_range":{"start_period":0,"end_period":5},"reason":"price low","confidence":0.85}"#;

/// 双脑结果.
///
/// 包含路径类型、调度方案、延迟分解与反馈契约（快路径 `feedback` 为 `None`）。
#[derive(Debug)]
pub struct DualBrainResult {
    /// 路径类型（FastPath / SlowPath）.
    pub path_type: PathType,
    /// 调度方案.
    pub schedule: ScheduleResult,
    /// 延迟分解.
    pub latency: LatencyBreakdown,
    /// 反馈契约（慢路径为 `Some`，快路径为 `None`）.
    pub feedback: Option<FeedbackContract>,
}

/// 默认单调时钟（毫秒）：eneros-time 全局时钟；boot 前未初始化返回 0。
fn default_clock_ms() -> u64 {
    eneros_time::get_monotonic_ns() / 1_000_000
}

/// 双脑协调器（D4：泛型 `Solver`，默认 `MockSolver`）.
///
/// 端到端编排双脑链路。快路径委托 `RealtimePathEngine`，慢路径执行完整 7 步。
pub struct DualBrainCoordinator<S: Solver> {
    /// 路径选择器.
    pub path_selector: PathSelector,
    /// 快速路径引擎（含 `pub solver` 字段，D8 复用 `solver.set_param`）.
    pub fast_path: RealtimePathEngine<S>,
    /// LLM 推理引擎（D3：`Box<dyn LlmEngine>`，默认 `DualBrainMockEngine`）.
    pub llm_engine: Box<dyn LlmEngine>,
    /// Prompt 模板.
    pub prompt_template: ChargeDischargeTemplate,
    /// 意图解析器.
    pub intent_parser: IntentParser,
    /// 契约转换器.
    pub converter: ContractConverter,
    /// 安全校验器.
    pub validator: SafetyValidator,
    /// 契约校验器.
    pub contract_validator: ContractValidator,
    /// 命令下发 sink（D6）.
    pub sink: Box<dyn CommandSink>,
    /// 请求计数器（D2：生成 `request_id`）.
    pub request_counter: u64,
    /// 单调时钟（毫秒；v0.248.0 真实计时——默认 eneros-time 全局时钟，
    /// 测试可注入可控时钟）。
    pub clock_fn: fn() -> u64,
}

impl<S: Solver> DualBrainCoordinator<S> {
    /// 构造协调器.
    ///
    /// `IntentParser` 从 `IntentParser::new(config.clone(), SystemState::default())` 构建（D9）。
    pub fn new(
        config: ScheduleConfig,
        llm_engine: Box<dyn LlmEngine>,
        solver: S,
        sink: Box<dyn CommandSink>,
    ) -> Self {
        Self {
            path_selector: PathSelector::new(),
            fast_path: RealtimePathEngine::new(config.clone(), solver),
            llm_engine,
            prompt_template: ChargeDischargeTemplate,
            intent_parser: IntentParser::new(config.clone(), SystemState::default()),
            converter: ContractConverter::new(config),
            validator: SafetyValidator::new(),
            contract_validator: ContractValidator::new(),
            sink,
            request_counter: 0,
            clock_fn: default_clock_ms,
        }
    }

    /// 注入可控单调时钟（毫秒）——测试与仿真用；生产用默认全局时钟。
    pub fn with_clock(mut self, clock_fn: fn() -> u64) -> Self {
        self.clock_fn = clock_fn;
        self
    }

    /// 端到端执行双脑链路.
    ///
    /// 7 步流程：
    /// 1. 路径选择 — `PathSelector::select()`；`FastPath` 早返回
    /// 2. 感知层 — `RealtimeState` → `SystemContext`（D5）
    /// 3. LLM 推理 — `build` + `infer`（D9）
    /// 4. 意图解析 — `parse_json` + `IntentContract` + `validate` + `to_solver_params`（D9）
    /// 5. LP 求解 — `set_param` + `solve`（D8）
    /// 6. 安全校验 — `parse_result` + `validate`
    /// 7. 命令下发 — 构建 `DispatchCommand` + `sink.write`（D6）
    pub fn execute(
        &mut self,
        state: &RealtimeState,
        now_ms: u64,
    ) -> Result<DualBrainResult, DualBrainError> {
        // v0.248.0：真实计时时钟（单调毫秒；默认 eneros-time 全局时钟，
        // 测试经 with_clock 注入可控时钟）。
        let tic = || (self.clock_fn)();
        let mut latency = LatencyBreakdown::default();

        // Step 1: 路径选择
        let path_type = self.path_selector.select(state, now_ms);

        if path_type == PathType::FastPath {
            // v0.248.0：真实计时（单调时钟）。
            let t0 = tic();
            let fast_result = self
                .fast_path
                .execute(state, now_ms)
                .map_err(|e| DualBrainError::SolveError(format!("{:?}", e)))?;
            latency.perception_ms = tic().saturating_sub(t0);
            latency.calculate_total();
            return Ok(DualBrainResult {
                path_type: PathType::FastPath,
                schedule: fast_result.schedule,
                latency,
                feedback: None,
            });
        }

        // ===== 慢路径：完整 7 步 =====

        // Step 2: 感知层 — 构建 SystemContext（D5）
        // v0.248.0：真实计时。
        let t_perception = tic();
        let system_context = SystemContext {
            current_soc: state.system.soc_pct,
            current_power_kw: state.system.current_a * state.system.voltage_v / 1000.0,
            current_price: state.current_price,
            current_period: 0,
            device_status: DeviceStatus::Normal,
            alarms: Vec::new(),
        };
        latency.perception_ms = tic().saturating_sub(t_perception);

        // Step 3: LLM 推理（D9）
        let t_ctx = TemplateContext {
            market_price: state.current_price,
            soc: state.system.soc_pct * 100.0,
            power_current: state.system.current_a * state.system.voltage_v / 1000.0,
            // 蓝图/20 R3-1：无传感器数据源时显式 None（禁假值占位）——
            // prompt 渲染 "N/A"；传感器接入后从 RealtimeState 扩展字段填充。
            temperature: None,
            time_of_day: None,
            historical_data: state.load_demand.clone().unwrap_or_default(),
        };
        let prompt = self.prompt_template.build(&t_ctx);
        let infer_params = InferParams::default();
        // v0.248.0：LLM 推理真实计时（替换硬编码 1200ms）。
        let t_llm = tic();
        let llm_output = self
            .llm_engine
            .infer(&prompt, &infer_params)
            .map_err(|e| DualBrainError::LlmError(format!("{:?}", e)))?;
        latency.llm_inference_ms = tic().saturating_sub(t_llm);

        // Step 4: 意图解析（D9）
        let intent = self
            .intent_parser
            .parse_json(&llm_output)
            .map_err(|e| DualBrainError::ParseError(format!("{:?}", e)))?;
        // 蓝图/20 R3-1：意图置信度在移动前拷贝（LLM 输出真实解析值）。
        let intent_confidence = intent.confidence;
        self.request_counter += 1;
        let request_id = format!("req-{}-{}", now_ms, self.request_counter);
        let contract = IntentContract {
            schema_version: String::from("1.1.0"),
            request_id: request_id.clone(),
            timestamp: now_ms,
            intent,
            context: system_context,
            llm_meta: LlmMeta {
                // v0.248.0：真实模型标识与 token 计数（引擎提供的能力查询；
                // 未加载模型时回退为占位名，token 计数取累计统计）。
                model_name: self
                    .llm_engine
                    .model_info()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| String::from("unloaded")),
                inference_ms: latency.llm_inference_ms,
                token_count: self.llm_engine.stats().total_tokens_generated as usize,
                // 蓝图/20 R3-1：置信度取意图解析真实值（LLM 输出 JSON 解析），
                // 不再硬编码 0.85。
                confidence: intent_confidence,
            },
        };
        self.contract_validator
            .validate(&contract)
            .map_err(|e| DualBrainError::ContractError(format!("{:?}", e)))?;
        let (config, problem) = self
            .converter
            .to_solver_params(&contract, &state.system)
            .map_err(|e| DualBrainError::ContractError(format!("{:?}", e)))?;
        // v0.248.0：真实计时（意图解析整段：parse→validate→convert）。
        latency.intent_parse_ms = tic().saturating_sub(t_llm);

        // Step 5: LP 求解（D8）
        let _ = self.fast_path.solver.set_param("time_limit", "0.5");
        // v0.248.0：LP 模型构建真实计时（solve 前一刻）。
        let t_lp_build = tic();
        let solve_result = self
            .fast_path
            .solver
            .solve(&problem, now_ms)
            .map_err(|e| DualBrainError::SolveError(format!("{:?}", e)))?;
        latency.lp_build_ms = tic().saturating_sub(t_lp_build);
        latency.lp_solve_ms = solve_result.elapsed_ms;

        let model = EnergyScheduleModel::new(config);
        let schedule = model.parse_result(&solve_result);

        // Step 6: 安全校验（v0.248.0：真实计时）。
        let t_safety = tic();
        let validation = self.validator.validate(&schedule, &state.system);
        latency.safety_validate_ms = tic().saturating_sub(t_safety);

        // v0.206.0（P0-17）：安全屏障闭环——Fatal 违规（validation.passed ==
        // false，如 SOC<0.05）拒绝命令下发并上报告警，不再取 clamped_schedule
        // 照常写入 sink。clamped_schedule 仅在非 Fatal（passed）时使用。
        if !validation.passed {
            // v0.206.0（P0-17）：Fatal 违规阻断命令下发。
            return Err(DualBrainError::SafetyFatal(format!(
                "safety validation FATAL: {} violation(s)",
                validation.violations.len()
            )));
        }
        let final_schedule = validation.clamped_schedule.clone().unwrap_or(schedule);

        // Step 7: 命令下发（D6）
        if let Some(entry) = final_schedule.schedule.first() {
            let cmd = DispatchCommand {
                target_device: String::from("pcs"),
                power_kw: entry.net_power_kw,
                ttl_ms: 300_000,
                timestamp: now_ms,
            };
            // v0.248.0：真实计时。
            let t_dispatch = tic();
            self.sink.write(cmd)?;
            latency.command_dispatch_ms = tic().saturating_sub(t_dispatch);
        }

        // 构建反馈契约
        let feedback = self.converter.to_feedback(
            &request_id,
            &solve_result,
            &validation,
            &final_schedule,
            latency.lp_solve_ms,
        );

        latency.calculate_total();

        Ok(DualBrainResult {
            path_type: PathType::SlowPath,
            schedule: final_schedule,
            latency,
            feedback: Some(feedback),
        })
    }
}

#[cfg(any(test, feature = "mock"))]
impl DualBrainCoordinator<MockSolver> {
    /// 默认构造（MockSolver + DualBrainMockEngine + MockCommandSink）.
    ///
    /// v0.205.0（P0-27）：仅 test/mock feature 可用；生产用
    /// [`DualBrainCoordinator::new`] 构造器注入真实 Solver/Engine/Sink。
    pub fn default_with_mock() -> Self {
        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let sink: Box<dyn CommandSink> = Box::new(MockCommandSink::new());
        Self::new(config, llm_engine, solver, sink)
    }
}

/// 双脑 Mock LLM 引擎.
///
/// 返回固定的 Charge intent JSON，用于端到端慢路径测试。
///
/// v0.59.0 `MockEngine::infer()` 返回 `"mock: {prompt}"` 而非 `mock_output`，
/// 无法满足慢路径 JSON 解析需求，故本 crate 定义独立的 Mock 引擎。
///
/// v0.205.0（P0-27）：仅 test/mock feature 构建。
#[cfg(any(test, feature = "mock"))]
pub struct DualBrainMockEngine {
    loaded: bool,
    stats: EngineStats,
    model_info: Option<ModelInfo>,
}

#[cfg(any(test, feature = "mock"))]
impl DualBrainMockEngine {
    /// 创建 Mock 引擎（预加载状态）.
    pub fn new() -> Self {
        Self {
            loaded: true,
            stats: EngineStats::default(),
            model_info: None,
        }
    }
}

#[cfg(any(test, feature = "mock"))]
impl Default for DualBrainMockEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "mock"))]
impl LlmEngine for DualBrainMockEngine {
    fn load_model(&mut self, path: &str) -> Result<(), LlmError> {
        self.loaded = true;
        self.model_info = Some(ModelInfo {
            name: String::from(path),
            size_bytes: 0,
            quantization: Quantization::Q4_K_M,
            context_length: 2048,
            device: ComputeDevice::Cpu,
        });
        self.stats.model_load_count += 1;
        Ok(())
    }

    fn infer(&mut self, _prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
        if !self.loaded {
            return Err(LlmError::ModelNotLoaded);
        }
        self.stats.inference_count += 1;
        self.stats.total_tokens_generated += MOCK_INTENT_JSON.len() as u64;
        Ok(String::from(MOCK_INTENT_JSON))
    }

    fn infer_stream(
        &mut self,
        _prompt: &str,
        _params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        if !self.loaded {
            return Err(LlmError::ModelNotLoaded);
        }
        let _ = callback(MOCK_INTENT_JSON);
        self.stats.inference_count += 1;
        self.stats.total_tokens_generated += MOCK_INTENT_JSON.len() as u64;
        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        self.model_info.as_ref()
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
