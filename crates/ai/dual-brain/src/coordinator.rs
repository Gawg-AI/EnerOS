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
use eneros_infer_scheduler::error::SchedulerError;
use eneros_infer_scheduler::request::InferRequest;
use eneros_infer_scheduler::scheduler::InferScheduler;
use eneros_intent_contract::contract::{
    DeviceStatus, FeedbackContract, IntentContract, LlmMeta, SystemContext,
};
use eneros_intent_contract::converter::ContractConverter;
use eneros_intent_contract::validator::ContractValidator;
use eneros_intent_parser::parser::IntentParser;
// 以下类型仅 mock 引擎（cfg(any(test, feature="mock"))）使用。
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::device::ComputeDevice;
use eneros_llm_engine::engine::LlmEngine;
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::error::LlmError;
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::model::{ModelInfo, Quantization};
use eneros_llm_engine::params::InferParams;
#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::stats::{EngineHealth, EngineStats};
use eneros_prompt_template::context::TemplateContext;
use eneros_prompt_template::template::PromptTemplate;
use eneros_prompt_template::templates::ChargeDischargeTemplate;
use eneros_safety_validator::state::SystemState;
use eneros_safety_validator::validator::SafetyValidator;
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::decision::{
    DecisionOutcome, DecisionPath, DecisionRejection, NoActionReason, ProposedControlPlan,
    RejectionReason,
};
use crate::error::DualBrainError;
use crate::latency::LatencyBreakdown;

/// Mock LLM 输出（合法 Charge intent JSON，满足 ContractValidator 6 项校验）.
#[cfg(any(test, feature = "mock"))]
const MOCK_INTENT_JSON: &str = r#"{"intent_type":"Charge","power":{"power_kw":50.0,"power_ratio":0.5},"time_range":{"start_period":0,"end_period":5},"reason":"price low","confidence":0.85}"#;

/// 双脑结果（蓝图 40 WP-40.3 之前的返回形态；现由 [`DecisionOutcome`]
/// 取代——四路径只产 Proposal/NoAction/Rejected，本类型保留仅供旧测试
/// 追溯，不再由 `execute` 返回）。
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
    // 蓝图 40 WP-40.3：协调器不再持有 sink——决策只产生 Proposal，
    // 由调用方提交给唯一 ControlDispatch（WP-40.6）。sink 字段已移除。
    /// 32 AG-04：决策审计（W5.1 接线——旧实现 audit.rs 实现+测试在位但
    /// 零生产消费）。
    pub audit: Option<crate::audit::DecisionAudit>,
    /// 32 AG-04：审计写入失败丢弃计数（不阻塞决策路径，fail-open 计数
    /// 供告警/可观测）。
    pub audit_drop_count: u64,
    /// 请求计数器（D2：生成 `request_id`）.
    pub request_counter: u64,
    /// 单调时钟（毫秒；v0.248.0 真实计时——默认 eneros-time 全局时钟，
    /// 测试可注入可控时钟）。
    pub clock_fn: fn() -> u64,
    // --- 22 §2.W2.3：LLM 有界延迟（经 InferScheduler）---
    /// LLM 推理调度器（deadline 检测 + 统计）。
    pub infer_scheduler: InferScheduler,
    /// 连续 LLM 超时计数（≥2 → 降级模式 + 告警）。
    pub llm_timeout_streak: u32,
    /// 是否处于降级模式（连续超时后）。
    pub degraded: bool,
    // 蓝图 40 WP-40.3：降级回放基线 = 上次**已提交**计划（调用方在
    // ControlDispatch 返回 Committed 后经 `mark_committed` 登记；旧
    // `last_successful_schedule` 由协调器在 sink 写后自记，已移除）。
    /// 上次已提交计划（降级模式回放基线；None = 无基线 → NoAction）。
    pub last_committed_schedule: Option<ScheduleResult>,
    // 蓝图 40 WP-40.3：决策闸（速率/能量/并发预算）移入唯一 ControlDispatch
    //（dispatch 层 check+record；拒绝/失败不消耗成功预算）。协调器不再持有。
}

/// 调度窗口（小时）——能量预算估算系数（蓝图 40：由 control-dispatch 的
/// DispatchGate 消费：单次调度能量 ≈ `|net_power_kw| × 0.25h`）。
pub const SCHEDULE_WINDOW_HOURS: f64 = 0.25;

impl<S: Solver> DualBrainCoordinator<S> {
    /// 构造协调器.
    ///
    /// `IntentParser` 从 `IntentParser::new(config.clone(), SystemState::unknown())`
    /// 构建（蓝图 40 WP-40.1：构造期状态为 NaN fail-closed 占位；每次慢路径
    /// 决策前经 `set_state` 覆写为受信任状态）。
    ///
    /// 蓝图 40 WP-40.3：不再接收 sink——决策只产 Proposal。
    pub fn new(config: ScheduleConfig, llm_engine: Box<dyn LlmEngine>, solver: S) -> Self {
        Self {
            path_selector: PathSelector::new(),
            fast_path: RealtimePathEngine::new(config.clone(), solver),
            llm_engine,
            prompt_template: ChargeDischargeTemplate,
            intent_parser: IntentParser::new(config.clone(), SystemState::unknown()),
            converter: ContractConverter::new(config),
            validator: SafetyValidator::new(),
            contract_validator: ContractValidator::new(),
            audit: None,
            audit_drop_count: 0,
            request_counter: 0,
            clock_fn: default_clock_ms,
            infer_scheduler: InferScheduler::new(
                1,
                64 * 1024 * 1024,
                eneros_llm_engine::device::ComputeDevice::Cpu,
            ),
            llm_timeout_streak: 0,
            degraded: false,
            last_committed_schedule: None,
        }
    }

    /// 注入可控单调时钟（毫秒）——测试与仿真用；生产用默认全局时钟。
    pub fn with_clock(mut self, clock_fn: fn() -> u64) -> Self {
        self.clock_fn = clock_fn;
        self
    }

    /// 32 AG-04：注入决策审计（W5.1 接线）——慢路径命令下发后把决策
    /// 记入 SM3 哈希链（含 path/validation/gate 字段）；写失败仅计数
    /// 丢弃，不阻塞决策路径。
    pub fn with_audit(mut self, audit: crate::audit::DecisionAudit) -> Self {
        self.audit = Some(audit);
        self
    }

    /// 22 §2.W2.3：LLM 有界推理（经 InferScheduler，带 deadline）。
    ///
    /// 同步适配：submit → 轮询 tick 直到该请求完成（成功/超时/失败）。
    /// 超时语义：`InferRequest::with_timeout`（deadline 检测在 tick 的
    /// 队列清理步）；同步引擎的单次推理中断不在 no_std 单线程能力内
    /// （推理时长由 `params.max_tokens` 预算约束）——deadline 保证排队/
    /// 调度不无限阻塞 + 连续超时降级（ADR-030）。
    fn llm_infer_bounded(
        &mut self,
        prompt: &str,
        params: &InferParams,
    ) -> Result<String, DualBrainError> {
        let now_ns = (self.clock_fn)() * 1_000_000;
        let req = InferRequest::new(0, prompt, params.clone())
            .with_timestamp(now_ns)
            .with_timeout(params.timeout_ms.saturating_mul(1_000_000));
        // 32 AI-04：背压——QueueFull 按超时降级同等处理。
        let id = match self.infer_scheduler.submit(req) {
            Ok(id) => id,
            Err(_) => return Err(DualBrainError::SolveError("infer queue full".into())),
        };
        loop {
            // deadline 检测用**当前**时钟（每次轮询推进；禁止无限阻塞）。
            let now_ns = (self.clock_fn)() * 1_000_000;
            let results = self.infer_scheduler.tick(now_ns, self.llm_engine.as_mut());
            if let Some(r) = results.into_iter().find(|r| r.id == id) {
                return match r.result {
                    Ok(out) => {
                        self.llm_timeout_streak = 0;
                        self.degraded = false;
                        Ok(out)
                    }
                    Err(e) => {
                        if matches!(e, SchedulerError::Timeout) {
                            self.llm_timeout_streak += 1;
                            if self.llm_timeout_streak >= 2 {
                                // 连续超时：降级模式（告警由上层观测 degraded）。
                                self.degraded = true;
                            }
                        }
                        Err(DualBrainError::LlmError(format!("{e:?}")))
                    }
                };
            }
            // 让出 CPU（同步等待）。
            core::hint::spin_loop();
        }
    }

    /// 登记一次已提交的计划（蓝图 40 WP-40.3：调用方在唯一 ControlDispatch
    /// 返回 `Committed` 后调用——降级模式只回放**已提交**计划，被拒/失败的
    /// 调度不得污染基线）。
    pub fn mark_committed(&mut self, plan: &ProposedControlPlan) {
        self.last_committed_schedule = Some(plan.schedule.clone());
    }

    /// 端到端执行双脑链路（蓝图 40 WP-40.3：**只产生决策结果，不写设备**）。
    ///
    /// 返回 [`DecisionOutcome`]：
    /// - `Proposal`：无副作用计划——由调用方提交给唯一 `ControlDispatch`
    ///   完成 validate→authorize→persist→deliver→ACK→readback→audit；
    /// - `NoAction`：明确不动作（降级无基线/空调度）——审计但不下发；
    /// - `Rejected`：结构化拒绝（速率/能量闸、SafetyFatal、校验失败）——
    ///   `SafetyFatal` 分类保留（立即上报人工，不重试）。
    ///
    /// 所有路径（Fast/Slow/Fallback/Degraded）都入审计链；`gate.record`
    /// 不再在协调器内调用——成功预算记账移到 dispatch 的 confirmed
    /// dispatch 阶段（拒绝/失败不消耗成功预算）。
    pub fn execute(&mut self, state: &RealtimeState, now_ms: u64) -> DecisionOutcome {
        // v0.248.0：真实计时时钟（单调毫秒；默认 eneros-time 全局时钟，
        // 测试经 with_clock 注入可控时钟）。
        let clock = self.clock_fn; // fn 指针 Copy（避免闭包长期借用 self）
        let tic = || clock();
        let mut latency = LatencyBreakdown::default();

        // Step 1: 路径选择
        let path_type = self.path_selector.select(state, now_ms);

        if path_type == PathType::FastPath {
            // v0.248.0：真实计时（单调时钟）。
            let t0 = tic();
            let fast_result = match self.fast_path.execute(state, now_ms) {
                Ok(r) => r,
                Err(eneros_fast_path::error::FastPathError::SafetyFatal(n)) => {
                    return self.audit_outcome(
                        now_ms,
                        DecisionOutcome::Rejected(DecisionRejection {
                            reason: RejectionReason::SafetyFatal(format!(
                                "fast-path safety validation FATAL: {n} violation(s)"
                            )),
                            path: DecisionPath::Fast,
                            issued_at_ms: now_ms,
                        }),
                    );
                }
                Err(other) => {
                    return self.audit_outcome(
                        now_ms,
                        DecisionOutcome::Rejected(DecisionRejection {
                            reason: RejectionReason::Internal(format!("fast-path: {other:?}")),
                            path: DecisionPath::Fast,
                            issued_at_ms: now_ms,
                        }),
                    );
                }
            };
            latency.perception_ms = tic().saturating_sub(t0);
            latency.calculate_total();
            return self.audit_outcome(
                now_ms,
                DecisionOutcome::Proposal(self.build_proposal(
                    DecisionPath::Fast,
                    fast_result.schedule,
                    latency,
                    None,
                    now_ms,
                    fast_result.validation.clamped_schedule.is_some(),
                )),
            );
        }

        // ===== 慢路径：完整 7 步 =====

        // Step 2: 感知层 — 构建 SystemContext（D5）。
        // v0.248.0：真实计时。蓝图 40 WP-40.1：解析器状态在解析前
        // 覆写为**受信任状态**（构造期 unknown 占位不得用于决策）。
        let t_perception = tic();
        self.intent_parser.set_state(state.system.clone());
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
            // 蓝图/20 R3-1：无传感器数据源时显式 None（禁假值占位）。
            temperature: None,
            time_of_day: None,
            historical_data: state.load_demand.clone().unwrap_or_default(),
        };
        let prompt = self.prompt_template.build(&t_ctx);
        let infer_params = InferParams::default();
        // 22 §2.W2.3：LLM 经 InferScheduler 有界推理（deadline 在 params）。
        let t_llm = tic();
        let llm_output = match self.llm_infer_bounded(&prompt, &infer_params) {
            Ok(out) => out,
            Err(_e) => {
                latency.llm_inference_ms = tic().saturating_sub(t_llm);
                // 超时回退策略（蓝图 W2.3；40 WP-40.3 统一为 Proposal/NoAction）：
                // - 连续超时（≥2）：降级模式——回放上次**已提交**计划
                //   （预批准保守 proposal，仍走唯一 dispatch 链复验）；
                //   无基线 → NoAction（不制造"安全默认计划"）；
                // - 首次超时：回退 fast-path（保守解，path=Fallback）。
                if self.llm_timeout_streak >= 2 {
                    if let Some(last) = self.last_committed_schedule.clone() {
                        // 降级模式（degraded 标记供上层告警/审计）。
                        self.degraded = true;
                        let mut plan = self.build_proposal(
                            DecisionPath::Degraded,
                            last,
                            latency,
                            None,
                            now_ms,
                            false,
                        );
                        plan.plan_id = format!("req-{}-degraded", now_ms);
                        return self.audit_outcome(now_ms, DecisionOutcome::Proposal(plan));
                    }
                    return self.audit_outcome(
                        now_ms,
                        DecisionOutcome::NoAction(NoActionReason::NoSafeFallback),
                    );
                }
                // 首次超时：回退 fast-path（不阻塞决策）。
                // 31-review（P1）：Fatal 分类必须保留。
                let fast = match self.fast_path.execute(state, now_ms) {
                    Ok(f) => f,
                    Err(eneros_fast_path::error::FastPathError::SafetyFatal(n)) => {
                        return self.audit_outcome(
                            now_ms,
                            DecisionOutcome::Rejected(DecisionRejection {
                                reason: RejectionReason::SafetyFatal(format!(
                                    "fast fallback safety FATAL: {n} violation(s)"
                                )),
                                path: DecisionPath::Fallback,
                                issued_at_ms: now_ms,
                            }),
                        );
                    }
                    Err(other) => {
                        return self.audit_outcome(
                            now_ms,
                            DecisionOutcome::Rejected(DecisionRejection {
                                reason: RejectionReason::Internal(format!(
                                    "fast fallback: {other:?}"
                                )),
                                path: DecisionPath::Fallback,
                                issued_at_ms: now_ms,
                            }),
                        );
                    }
                };
                return self.audit_outcome(
                    now_ms,
                    DecisionOutcome::Proposal(self.build_proposal(
                        DecisionPath::Fallback,
                        fast.schedule,
                        latency,
                        None,
                        now_ms,
                        fast.validation.clamped_schedule.is_some(),
                    )),
                );
            }
        };
        latency.llm_inference_ms = tic().saturating_sub(t_llm);

        // Step 4: 意图解析（D9）
        let intent = match self.intent_parser.parse_json(&llm_output) {
            Ok(i) => i,
            Err(e) => {
                return self.audit_outcome(
                    now_ms,
                    DecisionOutcome::Rejected(DecisionRejection {
                        reason: RejectionReason::Validation(format!("parse: {e:?}")),
                        path: DecisionPath::Slow,
                        issued_at_ms: now_ms,
                    }),
                );
            }
        };
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
                // v0.248.0：真实模型标识与 token 计数。
                model_name: self
                    .llm_engine
                    .model_info()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| String::from("unloaded")),
                inference_ms: latency.llm_inference_ms,
                token_count: self.llm_engine.stats().total_tokens_generated as usize,
                confidence: intent_confidence,
            },
        };
        if let Err(e) = self.contract_validator.validate(&contract) {
            return self.audit_outcome(
                now_ms,
                DecisionOutcome::Rejected(DecisionRejection {
                    reason: RejectionReason::Validation(format!("contract: {e:?}")),
                    path: DecisionPath::Slow,
                    issued_at_ms: now_ms,
                }),
            );
        }
        let (config, problem) = match self.converter.to_solver_params(&contract, &state.system) {
            Ok(v) => v,
            Err(e) => {
                return self.audit_outcome(
                    now_ms,
                    DecisionOutcome::Rejected(DecisionRejection {
                        reason: RejectionReason::Validation(format!("convert: {e:?}")),
                        path: DecisionPath::Slow,
                        issued_at_ms: now_ms,
                    }),
                );
            }
        };
        // v0.248.0：真实计时（意图解析整段：parse→validate→convert）。
        latency.intent_parse_ms = tic().saturating_sub(t_llm);

        // Step 5: LP 求解（D8）
        let _ = self.fast_path.solver.set_param("time_limit", "0.5");
        let t_lp_build = tic();
        let solve_result = match self.fast_path.solver.solve(&problem, now_ms) {
            Ok(r) => r,
            Err(e) => {
                return self.audit_outcome(
                    now_ms,
                    DecisionOutcome::Rejected(DecisionRejection {
                        reason: RejectionReason::Validation(format!("solve: {e:?}")),
                        path: DecisionPath::Slow,
                        issued_at_ms: now_ms,
                    }),
                );
            }
        };
        latency.lp_build_ms = tic().saturating_sub(t_lp_build);
        latency.lp_solve_ms = solve_result.elapsed_ms;

        let model = EnergyScheduleModel::new(config);
        let schedule = model.parse_result(&solve_result);

        // Step 6: 安全校验（v0.248.0：真实计时）。
        let t_safety = tic();
        let validation = self.validator.validate(&schedule, &state.system);
        latency.safety_validate_ms = tic().saturating_sub(t_safety);

        // v0.206.0（P0-17）：安全屏障闭环——Fatal 违规拒绝（passed==false），
        // 不取 clamped_schedule 继续。分类保留：SafetyFatal → 立即上报人工。
        if !validation.passed {
            return self.audit_outcome(
                now_ms,
                DecisionOutcome::Rejected(DecisionRejection {
                    reason: RejectionReason::SafetyFatal(format!(
                        "safety validation FATAL: {} violation(s)",
                        validation.violations.len()
                    )),
                    path: DecisionPath::Slow,
                    issued_at_ms: now_ms,
                }),
            );
        }
        let final_schedule = validation.clamped_schedule.clone().unwrap_or(schedule);

        // 蓝图 40 WP-40.3：速率/能量/并发预算在唯一 ControlDispatch 的
        // reserve 阶段（步骤 5）执行——协调器只产 Proposal。

        // 空调度（无控制需要）→ 明确 NoAction（不是成功下发）。
        if final_schedule.schedule.is_empty() {
            return self
                .audit_outcome(now_ms, DecisionOutcome::NoAction(NoActionReason::EmptyPlan));
        }

        // 构建反馈契约（预测反馈——仅供学习参考，学习门控在 Committed）。
        let feedback = self.converter.to_feedback(
            &request_id,
            &solve_result,
            &validation,
            &final_schedule,
            latency.lp_solve_ms,
        );

        latency.calculate_total();

        self.audit_outcome(
            now_ms,
            DecisionOutcome::Proposal(self.build_proposal(
                DecisionPath::Slow,
                final_schedule,
                latency,
                Some(feedback),
                now_ms,
                validation.clamped_schedule.is_some(),
            )),
        )
    }

    /// 从调度结果构建无副作用计划。
    fn build_proposal(
        &self,
        path: DecisionPath,
        schedule: ScheduleResult,
        latency: LatencyBreakdown,
        feedback: Option<FeedbackContract>,
        now_ms: u64,
        safety_clamped: bool,
    ) -> ProposedControlPlan {
        let power_kw = schedule
            .schedule
            .first()
            .map(|e| e.net_power_kw)
            .unwrap_or(0.0);
        ProposedControlPlan {
            plan_id: format!("req-{}-{}", now_ms, self.request_counter),
            path,
            schedule,
            power_kw,
            target_device: String::from("pcs"),
            // 29 AI-01：ttl_secs 秒基准（300s）。
            ttl_secs: 300,
            issued_at_ms: now_ms,
            // 调用方提交时填入 TrustedTelemetry.provenance_digest。
            input_digest: None,
            model_digest: None,
            latency,
            safety_violations: 0,
            safety_clamped,
            feedback,
        }
    }

    /// 统一审计出口：所有 DecisionOutcome（Proposal/NoAction/Rejected）入链；
    /// 写失败仅计数丢弃，不阻塞决策路径。
    fn audit_outcome(&mut self, now_ms: u64, outcome: DecisionOutcome) -> DecisionOutcome {
        let label = outcome.audit_label();
        let (safety, exec) = match &outcome {
            DecisionOutcome::Proposal(p) => {
                let s = if p.safety_clamped {
                    crate::audit::SafetyOutcome::Degraded
                } else {
                    crate::audit::SafetyOutcome::Passed
                };
                (s, crate::audit::ExecOutcome::Proposed)
            }
            DecisionOutcome::NoAction(_) => (
                crate::audit::SafetyOutcome::Passed,
                crate::audit::ExecOutcome::NoAction,
            ),
            DecisionOutcome::Rejected(r) => {
                let s = if matches!(r.reason, RejectionReason::SafetyFatal(_)) {
                    crate::audit::SafetyOutcome::Blocked
                } else {
                    crate::audit::SafetyOutcome::Passed
                };
                (s, crate::audit::ExecOutcome::Failed)
            }
        };
        let rec = crate::audit::DecisionRecord {
            actor: "dual-brain",
            target: "pcs",
            intent: "decision",
            lp_solution: &label,
            safety,
            exec,
            deviation: 0.0,
            now_ms,
        };
        if let Some(audit) = self.audit.as_mut() {
            if audit.log_decision(&rec).is_err() {
                self.audit_drop_count += 1;
            }
        }
        outcome
    }
}

#[cfg(any(test, feature = "mock"))]
impl DualBrainCoordinator<MockSolver> {
    /// 默认构造（MockSolver + DualBrainMockEngine）.
    ///
    /// v0.205.0（P0-27）：仅 test/mock feature 可用；生产用
    /// [`DualBrainCoordinator::new`] 构造器注入真实 Solver/Engine。
    /// 蓝图 40 WP-40.3：协调器不持 sink（决策只产 Proposal）。
    pub fn default_with_mock() -> Self {
        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        Self::new(config, llm_engine, solver)
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
