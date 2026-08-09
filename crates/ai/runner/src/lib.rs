//! EnerOS AI 决策链 release 端到端入口库（21-B A1：断链 A 装配）。
//!
//! 装配真实后端：`LlamaCppEngine`（llama.cpp）+ `HighsSolver`（HiGHS）+
//! `SafetyValidator`（coordinator 内部注册）+ `FileCommandSink`（本 crate）。
//! 生产路径零 mock；依赖链（llm-engine / solver-core）在 release 下由
//! `compile_error!` 熔断 mock（llm-engine lib.rs:56、solver-core lib.rs:64）。
//!
//! # OS 承载（21-A G-KERNEL 通过后的承载计划）
//!
//! 本阶段在 host 侧闭环（llama/highs 静态链接原生二进制）。作为 EL0 服务
//! 被 root-task spawn 的承载（aarch64-none 交叉编译 + init bundle 集成 +
//! IPC 服务注册）在 A3 方案中设计——受 491MB 模型文件无法进 init bundle
//! 限制，guest 侧模型供给（data 盘 / 网络加载）为承载前置条件（ADR-0036 §5）。

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use eneros_dual_brain::sink::CommandSink;
use eneros_dual_brain::{DualBrainCoordinator, DualBrainError, DualBrainResult};
use eneros_llm_engine::device::ComputeDevice;
use eneros_llm_engine::engine::LlmEngine;
use eneros_llm_engine::error::LlmError;
use eneros_llm_engine::llama_cpp::LlamaCppEngine;
use eneros_llm_engine::model::ModelInfo;
use eneros_llm_engine::params::InferParams;
use eneros_llm_engine::stats::{EngineHealth, EngineStats};
use eneros_solver_core::highs::HighsSolver;

use crate::scenario::{Overrides, Scenario};
use crate::sink::FileExecutor;

pub mod scenario;
pub mod sink;

/// Runner 错误（装配/执行/证据输出）.
#[derive(Debug)]
pub enum RunnerError {
    /// CLI/场景参数非法.
    Config(String),
    /// LLM 加载/推理失败.
    Llm(LlmError),
    /// HiGHS 初始化失败.
    Solver(String),
    /// sink 文件打开失败.
    Io(sink::RunnerIoError),
    /// 双脑链路失败（含 Fatal 安全违规）.
    DualBrain(DualBrainError),
    /// 证据 JSON 序列化失败.
    Evidence(String),
    /// 22 §2.W2.2：ControlChain 装配失败（租约/策略/安全点）.
    Chain(String),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "config error: {m}"),
            Self::Llm(e) => write!(f, "llm error: {e:?}"),
            Self::Solver(m) => write!(f, "solver error: {m}"),
            Self::Io(e) => write!(f, "sink io error ({}): {:?}", e.path, e.source),
            Self::DualBrain(e) => write!(f, "dual-brain error: {e:?}"),
            Self::Evidence(m) => write!(f, "evidence error: {m}"),
            Self::Chain(m) => write!(f, "chain error: {m}"),
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<sink::RunnerIoError> for RunnerError {
    fn from(e: sink::RunnerIoError) -> Self {
        Self::Io(e)
    }
}

/// LLM 推理记录：与引擎共享，执行后可取回"LLM 原始输出"（证据锚定）。
#[derive(Default)]
struct InferenceRecord {
    last_prompt: Option<String>,
    last_output: Option<String>,
}

/// LLM 推理记录引擎：包装真实 `LlamaCppEngine`，记录最后一次推理输出。
///
/// 仅增加记录旁路（真推理不变），用于端到端证据锚定"LLM 原始输出"；
/// 非 mock——`infer` 全部转发给真实 llama.cpp 后端。
pub struct RecordingLlmEngine {
    inner: LlamaCppEngine,
    record: Rc<RefCell<InferenceRecord>>,
}

impl RecordingLlmEngine {
    /// 创建记录引擎（CPU 设备，与 runner 默认一致）.
    pub fn new() -> Self {
        Self {
            inner: LlamaCppEngine::new(ComputeDevice::Cpu),
            record: Rc::new(RefCell::new(InferenceRecord::default())),
        }
    }

    /// 最近一次推理的原始输出（证据锚定用）.
    pub fn last_output(&self) -> Option<String> {
        self.record.borrow().last_output.clone()
    }

    /// 最近一次推理的 prompt（证据锚定用）.
    pub fn last_prompt(&self) -> Option<String> {
        self.record.borrow().last_prompt.clone()
    }
}

impl Default for RecordingLlmEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmEngine for RecordingLlmEngine {
    fn load_model(&mut self, path: &str) -> Result<(), LlmError> {
        self.inner.load_model(path)
    }

    fn infer(&mut self, prompt: &str, params: &InferParams) -> Result<String, LlmError> {
        let output = self.inner.infer(prompt, params)?;
        let mut rec = self.record.borrow_mut();
        rec.last_prompt = Some(prompt.to_owned());
        rec.last_output = Some(output.clone());
        Ok(output)
    }

    fn infer_stream(
        &mut self,
        prompt: &str,
        params: &InferParams,
        callback: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), LlmError> {
        let mut collected = String::new();
        self.inner
            .infer_stream(prompt, params, &mut |token: &str| {
                collected.push_str(token);
                callback(token)
            })?;
        let mut rec = self.record.borrow_mut();
        rec.last_prompt = Some(prompt.to_owned());
        rec.last_output = Some(collected);
        Ok(())
    }

    fn model_info(&self) -> Option<&ModelInfo> {
        self.inner.model_info()
    }

    fn health_check(&self) -> EngineHealth {
        self.inner.health_check()
    }

    fn stats(&self) -> &EngineStats {
        self.inner.stats()
    }

    fn unload(&mut self) -> Result<(), LlmError> {
        self.inner.unload()
    }
}

/// 端到端运行结果（证据载体）.
#[derive(Debug)]
pub struct RunnerOutcome {
    /// 场景.
    pub scenario: Scenario,
    /// 决策前 prompt（与 execute 内部 Step 3 同模板同上下文源）.
    pub prompt: String,
    /// LLM 原始输出（真 llama.cpp 推理结果）.
    pub llm_output: String,
    /// 双脑链路结果（路径类型/调度方案/延迟分解/反馈契约）.
    pub result: DualBrainResult,
    /// sink 实际写入的命令数.
    pub sink_written: usize,
}

/// 装配真后端并执行一次端到端决策.
///
/// 全程真实：`RecordingLlmEngine`（llama.cpp）→ `HighsSolver`（HiGHS）→
/// `FileCommandSink`（JSON Lines 落盘）。Fatal 安全违规时返回
/// `RunnerError::DualBrain(DualBrainError::SafetyFatal)` 且 sink 零写入。
///
/// 装配签名对齐 21-B §2.A1：`DualBrainCoordinator::new(config, Box<dyn LlmEngine>,
/// HighsSolver, Box<dyn CommandSink>)`——无 `new_default`/`MockSolver`/mock 引擎。
pub fn run_once(
    scenario: Scenario,
    overrides: Overrides,
    model_path: &str,
    sink_path: &str,
    now_ms: u64,
) -> Result<RunnerOutcome, RunnerError> {
    let config = overrides.apply(scenario.config());
    let state = scenario.state();

    // ---- 真实后端装配（21-B A1 强约束）----
    let recording = Rc::new(RefCell::new(InferenceRecord::default()));
    let mut llm: Box<dyn LlmEngine> = Box::new(RecordingLlmEngine {
        inner: LlamaCppEngine::new(ComputeDevice::Cpu),
        record: recording.clone(),
    });
    llm.load_model(model_path).map_err(RunnerError::Llm)?;
    let solver = HighsSolver::new().map_err(|e| RunnerError::Solver(format!("{e:?}")))?;
    // 22 §2.W2.2：AI→设备必经 ControlChain（锁定 5）——ChainCommandSink
    // 授权（default-deny 链：租约+策略+SBO+安全）→ FileExecutor 文件总线
    // 真实写入。链装配（ADR-029）：默认策略放行 Ai→dev1/pt10，租约签发。
    let chain = build_chain()?;
    let sink: Box<dyn CommandSink> =
        Box::new(eneros_dual_brain::chain_sink::ChainCommandSink::new(
            chain,
            Box::new(FileExecutor::new(sink_path)?),
            CHAIN_KEY,
            CHAIN_LEASE_ID,
            1,
            CHAIN_ISSUER,
            1,
            10,
            CHAIN_POLICY_VERSION,
            now_fn,
        ));
    let mut coord = DualBrainCoordinator::new(config, llm, solver, sink);

    // 首次 execute 必走慢路径（PathSelector 首次 `last_slow_path_ms == None`）。
    let result = match coord.execute(&state, now_ms) {
        Ok(result) => result,
        Err(e) => {
            // 错误路径可观测性（21-B 错误纪律）：Fatal 违规/链路失败时
            // 输出 LLM 原始输出与场景摘要到 stderr，供证据追溯。
            let rec = recording.borrow();
            if let Some(out) = &rec.last_output {
                eprintln!(
                    "AI-RUNNER diagnose: scenario={scenario:?} llm_output={out}",
                    scenario = scenario
                );
            }
            return Err(RunnerError::DualBrain(e));
        }
    };

    let rec = recording.borrow();
    let llm_output = rec
        .last_output
        .clone()
        .ok_or_else(|| RunnerError::Evidence("llm output not recorded".into()))?;
    let prompt = rec.last_prompt.clone().unwrap_or_default();
    drop(rec);

    Ok(RunnerOutcome {
        scenario,
        prompt,
        llm_output,
        sink_written: coord.sink.write_count(),
        result,
    })
}

// ============================================================================
// 22 §2.W2.2：ControlChain 装配（默认策略/租约/安全点；key 注入）。
// ============================================================================

/// 链密钥（HMAC 信封签名；生产由运维注入——v1.0 交付项，见 ADR-029）。
const CHAIN_KEY: &[u8] = b"eneros-runner-chain-key-000000000000000000";
/// AI 租约 ID（装配时签发，3 600 s）。
const CHAIN_LEASE_ID: [u8; 16] = [0xE1; 16];
/// 信封签发者标识。
const CHAIN_ISSUER: [u8; 16] = [0xE2; 16];
/// 策略代次（Policy::new(1) + with_rule 不 bump generation）。
const CHAIN_POLICY_VERSION: u64 = 1;

/// 时间源（毫秒；runner 单次执行用系统时钟）。
fn now_fn() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ControlChain 装配：策略放行 Ai→dev1/pt10，租约签发，安全点注册。
/// default-deny：未配置规则的点一律拒绝（fail-closed）。
pub fn build_chain() -> Result<eneros_control_gateway::chain::ControlChain, RunnerError> {
    use eneros_control_gateway::chain::ControlChain;
    use eneros_control_gateway::envelope::ActorKind;
    use eneros_control_gateway::lease::LeaseScope;
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;

    let policy = Policy::new(1).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    });
    let mut chain = ControlChain::new(CHAIN_KEY, 5, 64, policy, 60, 300, 64, 30);
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    if chain
        .lease_service_mut()
        .issue(CHAIN_LEASE_ID, scope, 3_600, now_fn())
        .is_none()
    {
        return Err(RunnerError::Chain("lease issue failed".into()));
    }
    chain.safety_mut().register(
        1,
        10,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: 10,
                device_id: 1,
                unit: PhysicalUnit::Kilowatts,
                safe_min: -500.0,
                safe_max: 500.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    );
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::FileCommandSink;

    /// 测试专用 LLM 引擎：返回合法 Charge intent JSON（本 crate 测试私有，
    /// 生产装配路径只使用 RecordingLlmEngine + LlamaCppEngine）。
    struct TestIntentLlm {
        stats: EngineStats,
    }

    impl TestIntentLlm {
        fn new() -> Self {
            Self {
                stats: EngineStats::default(),
            }
        }
    }

    const INTENT_JSON: &str = r#"{"intent_type":"Charge","power":{"power_kw":50.0,"power_ratio":0.5},"time_range":{"start_period":0,"end_period":5},"reason":"price low","confidence":0.85}"#;

    impl LlmEngine for TestIntentLlm {
        fn load_model(&mut self, _path: &str) -> Result<(), LlmError> {
            self.stats.model_load_count += 1;
            Ok(())
        }
        fn infer(&mut self, _prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
            self.stats.inference_count += 1;
            self.stats.total_tokens_generated += INTENT_JSON.len() as u64;
            Ok(INTENT_JSON.to_owned())
        }
        fn infer_stream(
            &mut self,
            _prompt: &str,
            _params: &InferParams,
            callback: &mut dyn FnMut(&str) -> bool,
        ) -> Result<(), LlmError> {
            callback(INTENT_JSON);
            Ok(())
        }
        fn model_info(&self) -> Option<&ModelInfo> {
            None
        }
        fn health_check(&self) -> EngineHealth {
            EngineHealth {
                loaded: true,
                device: ComputeDevice::Cpu,
                gpu_layers: 0,
                last_error: None,
            }
        }
        fn stats(&self) -> &EngineStats {
            &self.stats
        }
    }

    /// 端到端（真 HighsSolver + 真 SafetyValidator + 测试 LLM）：
    /// normal 场景必须产出调度方案并写入 sink。
    #[test]
    fn pipeline_normal_writes_sink_with_real_solver() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-ai-runner-pipe-normal-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sink_path = dir.join("pipe-normal.jsonl").to_str().unwrap().to_owned();

        let config = Scenario::Normal.config();
        let state = Scenario::Normal.state();
        let solver = HighsSolver::new().expect("highs init");
        let sink: Box<dyn CommandSink> = Box::new(FileCommandSink::new(&sink_path).unwrap());
        let llm: Box<dyn LlmEngine> = Box::new(TestIntentLlm::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink);

        let result = coord.execute(&state, 1000).expect("execute ok");
        assert_eq!(coord.sink.write_count(), 1, "normal 场景必须写入 1 条命令");
        assert!(!result.schedule.schedule.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Fatal 阻断（真 HighsSolver + 真 SafetyValidator + 测试 LLM）：
    /// fatal-soc 场景必须 SafetyFatal 且 sink 零写入。
    #[test]
    fn pipeline_fatal_soc_blocks_sink_with_real_solver() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-ai-runner-pipe-fatal-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sink_path = dir.join("pipe-fatal.jsonl").to_str().unwrap().to_owned();

        let config = Scenario::FatalSoc.config();
        let state = Scenario::FatalSoc.state();
        let solver = HighsSolver::new().expect("highs init");
        let sink: Box<dyn CommandSink> = Box::new(FileCommandSink::new(&sink_path).unwrap());
        let llm: Box<dyn LlmEngine> = Box::new(TestIntentLlm::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink);

        let err = coord
            .execute(&state, 1000)
            .expect_err("must fail with SafetyFatal");
        assert!(
            matches!(err, DualBrainError::SafetyFatal(_)),
            "SOC 终值 0.02 必须 Fatal，实际 {err:?}"
        );
        assert_eq!(coord.sink.write_count(), 0, "Fatal 违规下 sink 必须零写入");

        std::fs::remove_dir_all(&dir).ok();
    }
}
