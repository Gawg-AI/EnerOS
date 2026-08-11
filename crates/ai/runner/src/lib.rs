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

pub mod cli;
pub mod dispatcher;
pub mod scenario;
pub mod sink;

pub use dispatcher::{ChannelError, DeliveryChannel, EnvelopeDispatcher};

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
    feedback_fn: fn(u64) -> eneros_control_gateway::chain::FeedbackBundle,
    chain_key: &[u8],
) -> Result<RunnerOutcome, RunnerError> {
    // 32 AI-07：链密钥注入（旧实现编译期常量——生产固定密钥可复制；
    // fail-closed：调用方（CLI）负责读取密钥文件，缺省拒绝启动）。
    if chain_key.len() < 16 {
        return Err(RunnerError::Config(String::from(
            "chain key too short (>=16 bytes required)",
        )));
    }
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
            chain_key,
            CHAIN_LEASE_ID,
            1,
            CHAIN_ISSUER,
            1,
            10,
            CHAIN_POLICY_VERSION,
            now_fn,
            feedback_fn,
        ));
    let mut coord = DualBrainCoordinator::new(config, llm, solver, sink)
        // 32 AG-04：决策审计装配——创世锚从 chain_key SM3 派生（与封焊链
        // 同源；chain_key 已经 AI-07 fail-closed 校验 ≥16B）。
        .with_audit(eneros_dual_brain::audit::decision_audit_from_key(chain_key));

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
// 蓝图/24 §6.4：生产装配——AI 决策链经信封投递器直达 device-agent。
// ============================================================================

/// 生产 sink 装配（§6.4：ChainCommandSink 自证链 + 信封投递器 executor；
/// `FileCommandSink` 保留为测试/演示装配，见 [`run_once`]）。
///
/// `key`/`lease_id` 与 device-agent 侧同源（装配注入，§2.6：密钥不落明文）；
/// `handshake_retries` ≤ 5（§6.3 有界重试，失败 fail-closed 报 Offline）。
#[allow(clippy::too_many_arguments)]
pub fn build_production_sink(
    channel: Box<dyn DeliveryChannel>,
    key: &[u8],
    lease_id: [u8; 16],
    epoch_seed: u64,
    issuer: [u8; 16],
    device_id: u32,
    point_id: u32,
    policy_version: u64,
    handshake_retries: u32,
    now_fn: fn() -> u64,
    feedback_fn: fn(u64) -> eneros_control_gateway::chain::FeedbackBundle,
) -> Result<Box<dyn CommandSink>, RunnerError> {
    // 链装配：租约 grant 保留（自证链校验）；信封 token 的**唯一权威来源**
    // 为复验侧握手（FIX-24.R3，D7 语义归位——不再以本地 grant 铸造值进
    // 生产信封）。两侧同源（同一 epoch 种子）时握手值 == 本地 grant 值，
    // 不破坏 LeaseService 双侧校验（侦察结论，见 fix-r3 证据文档）。
    let (chain, _local_token) = build_chain_with(key, lease_id, epoch_seed, policy_version)?;
    let mut dispatcher = EnvelopeDispatcher::new(channel, key, lease_id, handshake_retries);
    // 装配期握手（fail-closed：握手失败 → 装配失败；首个信封必然在握手
    // 完成后发出；有界重试 ≤5 与零降级语义在 dispatcher 内部不变）。
    let token = dispatcher
        .handshake()
        .map_err(|e| RunnerError::Chain(format!("startup handshake failed: {e:?}")))?;
    let mut sink = eneros_dual_brain::chain_sink::ChainCommandSink::new(
        chain,
        Box::new(dispatcher),
        key,
        lease_id,
        token,
        issuer,
        device_id,
        point_id,
        policy_version,
        now_fn,
        feedback_fn,
    );
    // D7 语义归位：将复验侧铸造的 fencing token 注入签发侧（租约续期/
    // 轮换时同步机制就位；FIX-24.R3 前该 API 全仓零调用）。
    sink.update_fencing_token(token);
    Ok(Box::new(sink))
}

/// 链装配（key/lease/epoch/策略版本参数化；`build_chain` 保持默认参数演示）。
/// 返回 `(chain, fencing_token)`——token 与 device-agent 侧同源铸造。
fn build_chain_with(
    key: &[u8],
    lease_id: [u8; 16],
    epoch_seed: u64,
    policy_version: u64,
) -> Result<(eneros_control_gateway::chain::ControlChain, u64), RunnerError> {
    use eneros_control_gateway::chain::ControlChain;
    use eneros_control_gateway::envelope::ActorKind;
    use eneros_control_gateway::lease::LeaseScope;
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;

    let policy = Policy::new(policy_version).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    });
    let mut chain = ControlChain::new(key, 5, 64, policy, 60, 300, 64, 30);
    // D7：fencing epoch 以平台重启计数为种子（与 device-agent 同源）。
    chain.lease_service_mut().seed_epoch(epoch_seed);
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    let (token, _expiry) = chain
        .lease_service_mut()
        .issue(lease_id, scope, 3_600, now_fn())
        .ok_or_else(|| RunnerError::Chain("lease issue failed".into()))?;
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
    Ok((chain, token))
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

/// 时间源（epoch **秒**；信封 timestamp/expiry 规格为 epoch-secs（envelope.rs），
/// device-agent 复验链按秒判时——签发与复验必须同基准。原实现误用毫秒
/// （链内自洽、跨进程不一致），本 WP 修正并记录差异）。
fn now_fn() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
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

    // ============================================================================
    // 蓝图/24 §6.4 host 集成测试：决策 → 自证 → 投递 → 复验 → 落写
    // ============================================================================

    use std::collections::BTreeMap;

    use eneros_control_gateway::envelope::ActorKind;
    use eneros_control_gateway::lease::LeaseScope;
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_device_agent::{
        outcome_to_frame, verify_handshake_response, DenyClass, EnvelopeIngress, OutcomeFrame,
        HANDSHAKE_MARKER,
    };
    use eneros_dual_brain::chain_sink::{DeviceExecError, DeviceExecutor, DeviceReject};
    use eneros_protocol_abstract::access::{
        AuthorizedWriter, PointAccess, WriteError, WriteOutcome,
    };
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;
    use eneros_upa_model::{
        DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
    };

    const CHAIN_KEY: &[u8] = b"w24-host-it-key-000000000000000000000000";
    const CHAIN_LEASE: [u8; 16] = [0x42; 16];
    const CHAIN_ISSUER: [u8; 16] = [0x24; 16];
    const EPOCH_SEED: u64 = 11; // 模拟重启计数
    const POLICY_VER: u64 = 3;
    const NOW_SECS: u64 = 1_700_000_000;

    fn now_secs_fn() -> u64 {
        NOW_SECS
    }

    /// 测试装配用反馈注入：遥测可信（模拟有真实遥测的签发侧；FIX-24.R3
    /// 注入点——测试装置可注入任意语义，生产路径见 R3 偏差登记）。
    fn feedback_trusted(now: u64) -> eneros_control_gateway::chain::FeedbackBundle {
        eneros_control_gateway::chain::FeedbackBundle::fresh(true, 50.0, now)
    }

    /// 测试装配用反馈注入：显式"未知"（验证注入生效 + fail-closed）。
    fn feedback_unknown(now: u64) -> eneros_control_gateway::chain::FeedbackBundle {
        eneros_control_gateway::chain::FeedbackBundle::untrusted(now)
    }

    /// 测试设备（本 crate 测试私有；实现 PointAccess + AuthorizedWriter）。
    struct TestPointDevice {
        device_id: DeviceId,
        points: BTreeMap<PointId, f64>,
    }

    impl TestPointDevice {
        fn new(device_id: DeviceId) -> Self {
            let mut points = BTreeMap::new();
            points.insert(10, 0.0);
            // FIX-24.R3：CTL-010 真实反馈需要全部遥测点（缺任一 → 复验
            // fail-safe deny）。
            points.insert(11, 1.0); // grid_up
            points.insert(12, 50.0); // soc_pct
            points.insert(13, 600.0); // dc_bus_v
            points.insert(14, 0.0); // overcurrent
            points.insert(15, 0.0); // overtemp
            Self { device_id, points }
        }

        fn value_of(&self, point_id: PointId) -> f64 {
            self.points.get(&point_id).copied().unwrap_or(0.0)
        }
    }

    impl PointAccess for TestPointDevice {
        fn read_point(
            &mut self,
            point_id: PointId,
        ) -> Result<DataPoint, eneros_protocol_abstract::error::ProtocolError> {
            self.points
                .get(&point_id)
                .map(|&v| DataPoint {
                    point_id,
                    device_id: self.device_id,
                    name: String::from("power_setpoint"),
                    description: None,
                    point_type: PointType::Analog,
                    value: PointValue::Float(v),
                    quality: PointQuality::good(),
                    // FIX-24.R3：新鲜时间戳（CTL-010 新鲜度；过时 → Stale 拒绝）。
                    timestamp_ms: NOW_SECS * 1000,
                    source: DataSource::Internal,
                    unit: None,
                })
                .ok_or(eneros_protocol_abstract::error::ProtocolError::PointNotFound)
        }

        fn protocol_type(&self) -> eneros_protocol_abstract::config::ProtocolType {
            eneros_protocol_abstract::config::ProtocolType::Internal
        }
    }

    impl AuthorizedWriter for TestPointDevice {
        fn write_authorized(
            &mut self,
            cmd: &eneros_protocol_abstract::AuthorizedCommand,
            value: PointValue,
        ) -> Result<WriteOutcome, WriteError> {
            if cmd.device_id() != self.device_id {
                return Err(WriteError::CommandMismatch);
            }
            if !self.points.contains_key(&cmd.point_id()) {
                return Err(WriteError::CommandMismatch);
            }
            let echoed = value.clone();
            if let PointValue::Float(v) = value {
                self.points.insert(cmd.point_id(), v);
            }
            Ok(WriteOutcome {
                echoed: Some(echoed),
            })
        }
    }

    /// 进程内 EIPC 通道：直连 device-agent 收口点（信封投递/握手路由）。
    /// `applied` 记录每次 Applied 结果的功率（外部可观测：落写含回读断言）。
    struct InMemoryChannel {
        ingress: EnvelopeIngress,
        devices: eneros_device_agent::DeviceRegistry,
        applied: std::rc::Rc<std::cell::RefCell<Vec<f64>>>,
    }

    impl InMemoryChannel {
        fn new(now_ms: u64) -> Self {
            let policy = Policy::new(POLICY_VER).with_rule(PolicyRule {
                actor_kind: Some(ActorKind::Ai),
                devices: vec![1],
                points: vec![10],
                kinds: vec![CommandClass::Switch],
                time_window: (0, 0),
                obligations: vec![],
            });
            let ingress = EnvelopeIngress::new(
                CHAIN_KEY,
                CHAIN_LEASE,
                LeaseScope {
                    devices: vec![1],
                    points: vec![10],
                    max_class: 1,
                },
                3_600,
                EPOCH_SEED,
                policy,
                60,
                &[(
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
                )],
                now_ms,
            );
            let mut devices = eneros_device_agent::DeviceRegistry::new();
            devices.register(
                "pcs",
                eneros_device_agent::DeviceType::Pcs,
                1,
                Box::new(TestPointDevice::new(1)),
                BTreeMap::from([
                    (String::from("power_setpoint"), 10),
                    (String::from("grid_up"), 11),
                    (String::from("soc_pct"), 12),
                    (String::from("dc_bus_v"), 13),
                    (String::from("overcurrent"), 14),
                    (String::from("overtemp"), 15),
                ]),
            );
            Self {
                ingress,
                devices,
                applied: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            }
        }

        /// 已 Applied 的功率列表（落写可观测证据）。
        fn applied_powers(&self) -> Vec<f64> {
            self.applied.borrow().clone()
        }
    }

    impl DeliveryChannel for InMemoryChannel {
        fn request(&mut self, _target: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
            if payload == HANDSHAKE_MARKER {
                return self
                    .ingress
                    .handshake_response()
                    .map(|r| r.to_vec())
                    .map_err(|_| ChannelError::BadResponse);
            }
            let out = self
                .ingress
                .process(&mut self.devices, payload, NOW_SECS * 1000);
            if let eneros_device_agent::IngressOutcome::Applied { power_kw, .. } = &out {
                self.applied.borrow_mut().push(*power_kw);
            }
            Ok(outcome_to_frame(&out).encode().to_vec())
        }
    }

    /// 篡改注入通道：转发前翻转信封 device_id 字节（HMAC 覆盖区）。
    struct TamperChannel {
        inner: InMemoryChannel,
    }
    impl DeliveryChannel for TamperChannel {
        fn request(&mut self, target: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
            let mut p = payload.to_vec();
            if p != HANDSHAKE_MARKER && p.len() > 45 {
                p[45] ^= 0xFF;
            }
            self.inner.request(target, &p)
        }
    }

    /// 装配与 device-agent 同源的收口点（§6.3 两侧同源）。
    fn device_side_channel() -> InMemoryChannel {
        InMemoryChannel::new(NOW_SECS * 1000)
    }

    /// 场景 ①：AI 决策 → 自证 → 投递 → 复验 → 落写（含回读确认）。
    #[test]
    fn full_chain_decision_to_device_write() {
        let channel = device_side_channel();
        let applied = channel.applied.clone();
        let sink = build_production_sink(
            Box::new(channel),
            CHAIN_KEY,
            CHAIN_LEASE,
            EPOCH_SEED,
            CHAIN_ISSUER,
            1,
            10,
            POLICY_VER,
            3,
            now_secs_fn,
            feedback_trusted,
        )
        .expect("sink assembly");

        let config = Scenario::Normal.config();
        let state = Scenario::Normal.state();
        let solver = HighsSolver::new().expect("highs init");
        let llm: Box<dyn LlmEngine> = Box::new(TestIntentLlm::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink);
        let result = coord.execute(&state, NOW_SECS * 1000).expect("execute ok");
        assert_eq!(coord.sink.write_count(), 1, "决策链必须写入 1 条命令");
        // 落写含回读：device-agent 复验通过并 Applied——落写功率必须与
        // 决策链实际下发的调度值一致（外部可观测证据，非占位）。
        let expected = result
            .schedule
            .schedule
            .first()
            .expect("调度首条")
            .net_power_kw;
        let powers = applied.borrow();
        assert_eq!(powers.len(), 1, "必须恰好落写一次");
        assert!(
            (powers[0] - expected).abs() < 1e-9,
            "落写功率必须等于决策下发值 {expected}，got {}",
            powers[0]
        );
    }

    /// 场景 ②：篡改注入被拒（结构化拒绝可断言）。
    #[test]
    fn tampered_envelope_rejected_structured() {
        let channel = TamperChannel {
            inner: device_side_channel(),
        };
        let sink = build_production_sink(
            Box::new(channel),
            CHAIN_KEY,
            CHAIN_LEASE,
            EPOCH_SEED,
            CHAIN_ISSUER,
            1,
            10,
            POLICY_VER,
            3,
            now_secs_fn,
            feedback_trusted,
        )
        .expect("sink assembly");

        let config = Scenario::Normal.config();
        let state = Scenario::Normal.state();
        let solver = HighsSolver::new().expect("highs init");
        let llm: Box<dyn LlmEngine> = Box::new(TestIntentLlm::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink);
        let err = coord
            .execute(&state, NOW_SECS * 1000)
            .expect_err("篡改信封必须被拒");
        // 结构化拒绝原因经 DispatchError 文本可断言（候选 5 将直接消费载荷）。
        let msg = format!("{err:?}");
        assert!(
            msg.contains("Rejected(Hmac)"),
            "拒绝原因必须结构化传递（Hmac），got: {msg}"
        );
        assert_eq!(coord.sink.write_count(), 0, "被拒命令不得计入成功");
    }

    /// 场景 ③：握手失败 fail-closed——FIX-24.R3 后装配期即握手（fail-fast）：
    /// 握手不可用 → 装配失败（更早阻断）；≤5 次有界重试与零降级由
    /// dispatcher 层单测覆盖（handshake_bounded_retries_then_offline /
    /// write_fails_closed_without_handshake）。
    #[test]
    fn handshake_failure_fails_closed() {
        struct DeadChannel;
        impl DeliveryChannel for DeadChannel {
            fn request(&mut self, _t: &str, _p: &[u8]) -> Result<Vec<u8>, ChannelError> {
                Err(ChannelError::Unreachable)
            }
        }
        let r = build_production_sink(
            Box::new(DeadChannel),
            CHAIN_KEY,
            CHAIN_LEASE,
            EPOCH_SEED,
            CHAIN_ISSUER,
            1,
            10,
            POLICY_VER,
            2,
            now_secs_fn,
            feedback_trusted,
        );
        assert!(
            matches!(r, Err(RunnerError::Chain(msg)) if msg.contains("handshake")),
            "握手不可用 = 装配失败（fail-closed，fail-fast）"
        );
    }

    /// FIX-24.R3 修复 3 防回归：信封 fencing_token 必须 == 握手返回值
    /// （复验侧铸造权威；证伪"本地 grant 铸造值进生产信封"的旧机制——
    /// 旧代码 update_fencing_token 全仓零调用，token 来源未归位）。
    /// 注：D7 同源约束（两侧同 epoch 种子）下握手值 == 本地 grant 值，
    /// 数值不可区分；本测试断言**机制**：捕获握手响应携带的 token，断言
    /// 信封携带同一值（token 来源 = 握手通道）。
    #[test]
    fn envelope_fencing_token_equals_handshake_token() {
        use eneros_control_gateway::envelope::CommandEnvelope;

        // 捕获通道：记录握手响应 token + 投递帧的信封字节。
        struct CaptureChannel {
            inner: InMemoryChannel,
            handshake_token: std::rc::Rc<std::cell::RefCell<Option<u64>>>,
            envelopes: std::rc::Rc<std::cell::RefCell<Vec<[u8; 172]>>>,
        }
        impl DeliveryChannel for CaptureChannel {
            fn request(&mut self, t: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
                if payload == HANDSHAKE_MARKER {
                    let resp = self.inner.request(t, payload)?;
                    if let Some((_, token)) = verify_handshake_response(CHAIN_KEY, &resp) {
                        *self.handshake_token.borrow_mut() = Some(token);
                    }
                    return Ok(resp);
                }
                if payload.len() >= 172 {
                    let mut env = [0u8; 172];
                    env.copy_from_slice(&payload[..172]);
                    self.envelopes.borrow_mut().push(env);
                }
                self.inner.request(t, payload)
            }
        }

        let channel = CaptureChannel {
            inner: device_side_channel(),
            handshake_token: std::rc::Rc::new(std::cell::RefCell::new(None)),
            envelopes: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        };
        let handshake_handle = channel.handshake_token.clone();
        let envelopes_handle = channel.envelopes.clone();
        let mut sink = build_production_sink(
            Box::new(channel),
            CHAIN_KEY,
            CHAIN_LEASE,
            EPOCH_SEED,
            CHAIN_ISSUER,
            1,
            10,
            POLICY_VER,
            3,
            now_secs_fn,
            feedback_trusted,
        )
        .expect("sink assembly");

        let dc = eneros_dual_brain::sink::DispatchCommand {
            target_device: "pcs".into(),
            power_kw: 30.0,
            ttl_secs: 60,
            timestamp: NOW_SECS * 1000,
        };
        sink.write(dc).expect("写必须成功（握手后发出）");

        // 握手返回值（复验侧铸造）。
        let handshake_token = handshake_handle.borrow().expect("装配期握手必须发生");
        assert_eq!(handshake_token, EPOCH_SEED + 1, "token 以重启计数为种子");
        // 信封携带的 fencing_token == 握手返回值（D7 语义归位）。
        let envelopes = envelopes_handle.borrow();
        assert_eq!(envelopes.len(), 1, "恰好投递一个信封");
        let env = CommandEnvelope::decode(&envelopes[0]).expect("信封解码");
        assert_eq!(
            env.fencing_token, handshake_token,
            "信封 fencing_token 必须等于握手返回值（复验侧权威）"
        );
        // stale token 仍被拒（既有语义）：手动构造旧 token 信封 → 复验拒。
        // （由 ingress 用例 5 stale_token_denied + 本测试共同覆盖。）
    }

    /// FIX-24.R3 修复 1 防回归（签发侧）：feedback 注入点生效——注入显式
    /// "未知" → 自证链按真实状态裁决（fail-safe deny）；旧行为（硬编码
    /// fresh 常量）无法注入、必然放行。
    #[test]
    fn injected_unknown_feedback_denies_chain() {
        let channel = device_side_channel();
        let mut sink = build_production_sink(
            Box::new(channel),
            CHAIN_KEY,
            CHAIN_LEASE,
            EPOCH_SEED,
            CHAIN_ISSUER,
            1,
            10,
            POLICY_VER,
            3,
            now_secs_fn,
            feedback_unknown,
        )
        .expect("sink assembly");

        let dc = eneros_dual_brain::sink::DispatchCommand {
            target_device: "pcs".into(),
            power_kw: 30.0,
            ttl_secs: 60,
            timestamp: NOW_SECS * 1000,
        };
        let r = sink.write(dc);
        assert!(r.is_err(), "显式未知反馈必须 fail-closed: {r:?}");
        assert_eq!(sink.write_count(), 0, "未知反馈不得落写");
    }

    /// 结构化拒绝可精确断言（dispatcher 层）：握手走真实收口点，投递结果
    /// 由通道伪造为 Denied{Hmac} → 必须映射为 Rejected(DeviceReject::Hmac)。
    #[test]
    fn dispatcher_maps_denied_outcome_to_structured_reject() {
        struct DeniedChannel {
            inner: InMemoryChannel,
        }
        impl DeliveryChannel for DeniedChannel {
            fn request(&mut self, t: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
                if payload == HANDSHAKE_MARKER {
                    return self.inner.request(t, payload);
                }
                Ok(OutcomeFrame {
                    code: OutcomeFrame::CODE_DENIED,
                    class: DenyClass::Hmac.code(),
                }
                .encode()
                .to_vec())
            }
        }
        let mut d = EnvelopeDispatcher::new(
            Box::new(DeniedChannel {
                inner: device_side_channel(),
            }),
            CHAIN_KEY,
            CHAIN_LEASE,
            2,
        )
        .with_backoff(|_| 0);
        let cmd = eneros_control_gateway::gateway::AuthorizedCommand::from_parts(
            1,
            10,
            eneros_telemetry_model::command::ControlCommand::Single(
                eneros_telemetry_model::command::SingleCommand::On,
            ),
            ActorKind::Ai,
            [0u8; 16],
            [0u8; 16],
            1,
            [0u8; 16],
            1,
            1,
            [0u8; 16],
        );
        let r = d.write(&cmd, &[0u8; 172], 50.0);
        assert_eq!(
            r,
            Err(DeviceExecError::Rejected(DeviceReject::Hmac)),
            "复验拒绝必须结构化为 DeviceReject::Hmac"
        );
    }
}
