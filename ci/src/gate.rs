//! EnerOS Quality Gate — Core
//!
//! Defines the quality gate data structures, the [`QualityGate`] trait, and
//! the [`DefaultGate`] implementation that wraps `cargo` subcommands
//! (fmt / clippy / deny / test).

use std::io;
use std::process::Command;
use std::time::Instant;

use crate::error::GateError;

/// Result of a single quality check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Check name: `"fmt"`, `"clippy"`, `"audit"`, or `"test"`.
    pub name: &'static str,
    pub passed: bool,
    pub duration_ms: u64,
    /// Failure detail, or a degraded-mode notice when `passed` is true.
    pub message: Option<String>,
}

/// Aggregated report for all four quality checks.
#[derive(Debug)]
pub struct GateReport {
    /// Results for `[fmt, clippy, audit, test]` in that order.
    pub results: [CheckResult; 4],
    pub overall_pass: bool,
}

/// Abstraction over the four quality checks.
pub trait QualityGate {
    /// Run all checks sequentially and return an aggregated report.
    fn run_all(&self) -> GateReport;
    fn run_fmt_check(&self) -> Result<(), GateError>;
    fn run_clippy(&self) -> Result<(), GateError>;
    fn run_audit(&self) -> Result<(), GateError>;
    fn run_tests(&self) -> Result<(), GateError>;
}

/// Default gate that shells out to `cargo` subcommands.
pub struct DefaultGate;

impl DefaultGate {
    pub fn new() -> Self {
        Self
    }

    /// Run `cargo <args>`, returning `on_failure` when the process exits
    /// with a non-zero status. Spawn failures (`cargo` not in PATH, etc.)
    /// are mapped to [`GateError::IoError`].
    fn run_cargo(args: &[&str], on_failure: GateError) -> Result<(), GateError> {
        match Command::new("cargo").args(args).status() {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(on_failure),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(GateError::IoError("`cargo` not found in PATH".to_string()))
            }
            Err(e) => Err(GateError::IoError(format!(
                "failed to spawn `cargo {}`: {}",
                args.join(" "),
                e
            ))),
        }
    }

    /// Check whether `cargo-deny` is installed and reachable in PATH.
    /// Used by [`QualityGate::run_all`] to detect the degraded case.
    fn cargo_deny_available() -> bool {
        Command::new("cargo-deny")
            .arg("--version")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Default for DefaultGate {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityGate for DefaultGate {
    fn run_fmt_check(&self) -> Result<(), GateError> {
        Self::run_cargo(&["fmt", "--all", "--", "--check"], GateError::FmtDirty)
    }

    fn run_clippy(&self) -> Result<(), GateError> {
        // eneros-kernel and eneros-hello are excluded from host-side clippy:
        // both define #[panic_handler] / #[lang = "eh_personality"] which
        // conflict with std on the host target. They're validated via cross-build.
        // eneros-runtime (v0.4.0) and eneros-hal (v0.5.0+) are library crates
        // without panic_handler and are host-testable.
        // Note: eneros-hal arm64 module (v0.6.0 core + v0.7.0 peripherals + v0.8.0 mm + v0.9.0 partition) is cfg-gated by
        // #[cfg(target_arch = "aarch64")] and excluded from host clippy.
        // eneros-heap (v0.10.0), eneros-user-heap (v0.11.0), eneros-time (v0.12.0 + v0.12.1 beidou + v0.12.2 holdover),
        // eneros-watchdog (v0.13.0), eneros-panic (v0.14.0), eneros-smp (v0.15.0), eneros-sched (v0.16.0), eneros-smp coherence (v0.17.0),
        // eneros-mm isolation (v0.9.1 compliance), eneros-power (v0.17.1 power management), eneros-sched (v0.18.0 thread abstraction, v0.19.0 partition scheduler),
        // eneros-ipc (v0.20.0 IPC endpoint + v0.21.0 SPSC ring), eneros-controlbus (v0.22.0 Control Bus + TTL + dual-plane),
        // eneros-storage (v0.23.0 eMMC/NVMe Block Device),
        // eneros-fs (v0.24.0 littlefs2 file system + v0.24.1 wear-leveling),
        // eneros-tsdb (v0.25.0 TSDB),
        // eneros-config (v0.26.0 config management),
        // and eneros-net (v0.27.0 Ethernet network driver + v0.28.0 TCP/IP stack + v0.29.0 Socket abstraction layer),
        // eneros-cellular (v0.30.0/v0.30.1/v0.30.2 Cellular Modem + PPP + Failover),
        // eneros-crypto (v0.33.0 国密 SM2/SM3/SM4 + CSRNG + PKI 证书基础 — pure Rust, no_std, no C FFI),
        // eneros-agent (v0.42.1 Agent Lifecycle + Spawner + Heartbeat + Crash Recovery + Capability Token + Capability Manager + System Agent + 故障恢复编排 — LifecycleManager / LifecycleHook / AgentSpawner / AgentFactory / HeartbeatMonitor / CrashRecovery / CheckpointStore / CapabilityToken / CapabilityManager / TokenStore / SystemAgent / ResourceMonitor / DependencyGraph / RecoveryOrchestrator),
        // eneros-driver-framework (v0.43.0 用户态设备驱动统一抽象 — DeviceDriver trait / DriverId / DriverType / DriverState / DriverHealth / DriverError / registry / handle / mock),
        // eneros-rs485 (v0.44.0 RS-485 串行驱动), eneros-can (v0.47.0 CAN 2.0A/B 驱动),
        // eneros-modbus-rtu (v0.45.0 Modbus RTU 主站), eneros-modbus-tcp (v0.46.0 Modbus TCP 主站),
        // eneros-iec104-slave (v0.48.0 IEC 60870-5-104 从站 — APDU/ASDU codec + interrogation/command/clock sync),
        // eneros-iec104-master (v0.49.0 IEC 60870-5-104 主站 — multi-device polling + interrogation/command/clock sync),
        // eneros-upa-model (v0.50.0 统一点表模型 UPA — DataPoint / PointType / PointValue / PointQuality / PointDatabase),
        // eneros-protocol-abstract (v0.51.0 协议抽象层 — PointAccess / ProtocolAdapter / ProtocolManager / MockAdapter),
        // eneros-calibration (v0.51.1 计量校准 — CalibCoeffs / AccuracyClass / MeterCalibration / CalibStore / InMemoryCalibStore),
        // eneros-telemetry-model (v0.52.0 四遥标准数据模型 — Telemetry / Telesignaling / Telecontrol / Teleadjust / DeadbandFilter / QualityFlag),
        // eneros-soe-engine (v0.53.0 SOE 事件顺序记录引擎 — SoeEvent / SoeEventType / EventPriority / SoeEngine / SoeStorage / EventTrigger),
        // eneros-mqtt (v0.53.1 MQTT 物联网上报 — MqttClient / QoS / MqttPacket / LastWill / TopicFilter / 指数退避重连),
        // eneros-alarm (v0.53.2 告警管理体系 — AlarmManager / AlarmRecord / AlarmLevel / SuppressionRule / EscalationPolicy),
        // eneros-rtos-control (v0.54.0 RTOS 控制闭环引擎 — PidController / SetpointTracker / ControlLoop trait / ControlLoopEngine / PowerControlLoop),
        // eneros-rtos-sampling (v0.55.0 高频采样服务 — SamplingService / StateSnapshot / SharedMemorySnapshot / SampledPoint / SamplingStats),
        // eneros-rtos-cmd-exec (v0.56.0 命令消费与执行 — CommandExecutor / DeviceStateProvider / DevicePointMap / ExecutorStats / ExecutorReport),
        // eneros-rtos-degrade (v0.57.0 降级规则引擎 — DegradeEngine / DegradeMode / DegradeRule / DegradeContext / SafeDefaults / DegradeStats / 5 builtin rules),
        // eneros-rtos-watchdog-degrade (v0.58.0 看门狗与端到端降级流程 — WatchdogDegradeFlow / DegradeState / HeartbeatWatcher / RecoveryManager / FlowStats / 5-state machine),
        // eneros-llm-engine (v0.59.0 LLM 推理引擎选型与 FFI 封装 — LlmEngine trait / MockEngine / LlamaCppEngine / ComputeDevice / Quantization / InferParams / ModelInfo / EngineStats),
        // eneros-gguf-loader (v0.60.0 模型加载与内存管理 — GgufLoader / GgufHeader / GgufMetadata / GgufTensorInfo / GgufDtype / MmapBackend / MemoryBackend / ModelMemoryManager / GpuOps),
        // eneros-model-deploy (v0.61.0 7B INT4 量化模型部署 — QuantConfig7B / DeployVerifier / DeployBackend / MockDeployBackend / LlamaDeployBackend / PowerPromptSet / DeployReport / DeployError),
        // eneros-infer-scheduler (v0.62.0 推理调度与并发控制 — InferScheduler / InferRequest / InferResult / RequestPriority / KvCacheManager / KvCacheEntry / SchedulerStats / SchedulerError),
        // eneros-prompt-template (v0.63.0 Prompt 模板系统 + JSON 输出约束 — PromptTemplate trait / TemplateContext / SchemaSpec / SchemaField / SchemaType / ChargeDischargeTemplate / DispatchTemplate / AlarmTemplate / JsonConstraint / ConstraintStats / extract_json / TemplateError),
        // eneros-solver-core (v0.64.0 LP 求解器集成 — Solver trait / MockSolver / HighsSolver / LpProblem / ConstraintMatrix / VarType / ObjectiveSense / SolveResult / SolveStatus / SolverStatus / SolverError),
        // eneros-solver-model (v0.65.0 优化问题建模框架 — Variable / VarBuilder / LinearExpr / Constraint / OptProblem / compile() to LpProblem CSR),
        // eneros-energy-lp-model (v0.66.0 能源调度 LP 模型 — ScheduleConfig / EnergyScheduleModel / ScheduleEntry / ScheduleResult / SOC dynamics / ramp / objective),
        // eneros-safety-validator (v0.67.0 安全校验器 — SafetyValidator / SafetyRule / ElectricalSafetyRule / ProtectionCoordinationRule / ValidationResult / Violation / Severity / SystemState),
        // eneros-intent-parser (v0.68.0 意图解析器 — IntentParser / Intent / IntentType / TimeRange / PowerIntent / SocIntent / IntentError),
        // eneros-intent-contract (v0.69.0 意图契约 — IntentContract / FeedbackContract / ContractValidator / ContractConverter / ContractError / SystemContext / LlmMeta / DeviceStatus),
        // eneros-fast-path (v0.70.0 实时快速路径 — RealtimePathEngine / PathSelector / StrategyTable / PathType / RealtimeState / FastPathResult / FastPathError),
        // eneros-dual-brain (v0.71.0 双脑协同联调 — DualBrainCoordinator / DualBrainResult / LatencyBreakdown / DualBrainError / DispatchCommand / CommandSink / MockCommandSink),
        // eneros-energy-market-agent (v0.72.0 Energy Agent + Market Agent — EnergyAgent / MarketAgent / AgentRuntime / HeartbeatStatus / AgentRuntimeError / MarketData / MarketSignal / MarketChannel / MarketDataSource / MockMarketSource),
        // eneros-device-agent (v0.73.0 Device Agent — DeviceAgent / DeviceAdapter / MockDevice / DeviceRegistry / DeviceInfo / DeviceType / DeviceState / DeviceSnapshot / DeviceCommand / CommandSource / MockCommandSource / DeviceError),
        // eneros-mvp-scenario (v0.74.0 MVP 端到端集成 — MvpOrchestrator / MvpTickReport / RevenueComparator / TraditionalEms / MvpError),
        // eneros-agent-bus-dds (v0.78.0 Agent 消息签名层 — DdsNode / MockDdsNode / CycloneDdsNode / DdsConfig / DiscoveryPolicy / QosPolicy / DdsSample / DdsError / TopicSpec / TopicRegistry / TopicError / TopicCategory / PayloadType / validate_topic_name / standard_topics / MessageRouter / RoutingPolicy / RouteDecision / DropReason / RouteError / RouterStats / Subscription / SubId / CapabilityVerifier / MockCapabilityVerifier / Permission / AgentId / pattern_matches / CodecKind / CodecError / KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify),
        // eneros-tsn-time (v0.79.0 gPTP 时间同步层 — ClockIdentity / MacAddr / PtpTime / Port / PortRole / PortState / AnnounceMessage / BmcaResult / SyncMessage / FollowUpMessage / GptpConfig / GptpClock / compare_priority / run_bmca / handle_sync / handle_follow_up / adjust_clock / compute_offset / current_time / to_announce + v0.80.0 TSN 802.1Qbv 调度：TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config + v0.81.0 TSN 驱动胶合层 + 端到端时延探针：TsnError / TsnDriver / MockTsnDriver / driver_send_closure / DelayStats / LatencyProbe + v0.82.0 Grid Agent：GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors + v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor + v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer + v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber + v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize + v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split + v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives + v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError + v0.90.0 孪生预测：Predictor / ForecastModel / ForecastResult / ForecastPoint / PersistenceModel / MeanModel / ForecastError + v0.91.0 What-if 分析：WhatIfAnalyzer / SimModel / AnalyticalSimModel / Scenario / ScenarioResult / Outcome / RiskLevel / Action / WhatIfError + v0.92.0 域内仲裁：DomainArbiter / ArbiterPolicy / ArbitrationRequest / ArbitrationResult / ArbitrationReason / Claim / Priority / detect_deadlock + v0.93.0 域级优化：DomainOptimizer / EdgeBoxState / DomainPlan / OptError + v0.94.0 VPP 聚合：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType + v0.95.0 云端策略下发：Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher / validate_strategy + v0.96.0 云端数据汇聚：DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink + v0.97.0 联邦发现：MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus + v0.98.0 跨域通信通道：TlsConfig / Endpoint / ChannelError / SecureTransport / MockSecureTransport / FederationChannel + v0.98.1 纵向加密认证：TunnelKeys / VerticalEncryptTunnel / EncryptError / DispatchToken / AuthResult / VerticalEncryptDevice / MockVerticalEncryptDevice / TunnelManager / hmac_sm3 / Sm3Hmac + v0.99.0 联邦共识协议：NodeId / ConsensusState / MsgType / PbftMessage / LogEntry / ConsensusResult / ConsensusError / ConsensusBus / MockConsensusBus / ConsensusEngine / f / quorum / primary_of / sign_message / verify_message + v0.100.0 资源争抢竞价：AgentId / Price / Qty / BidOrder / AskOrder / OrderBook / Match / MatchResult / AuctionError / AuctionEngine / match_book / match_digest + v0.101.0 断网处理与孤岛模式：PartitionState / PartitionDetector / IslandMode / EventCache / RecoverySync / SyncSink / MockSyncSink / SyncError / SyncReport + v0.102.0 MILP 求解器集成：UcUnit / UnitCommitment / UnitSchedule / DayAheadPlan / DayAheadScheduler + v0.103.0 Solver 神经热启动：CandidateSolution / InferEngine / MockEngine / OnnxEngine / HeuristicNet / SolveContext / WarmStartProvider / WarmStarter / WarmError + v0.104.0 多目标 Pareto 优化：MultiObjectiveProblem / Objective / OptDirection / VariableSpec / ParetoSolution / ParetoFront / ParetoSolver / Nsga2Solver / DecisionMaker + v0.105.0 IEC 61850 信息模型：LogicalDevice / LogicalNode / LnClass / DataObject / CommonDataClass / DataAttribute / FunctionalConstraint / DaValue / Quality / Validity / Source / Iec61850Model / SclParser / ModelError + v0.106.0 IEC 61850 MMS 协议栈 — BER 编解码 + ACSE 关联 + COTP + MMS Read/Write 客户端 + MmsTransport 抽象：MmsClient / MmsConnection / ConnState / MmsRequest / MmsResponse / VarAccessSpec / MmsReadResult / MmsWriteResult / MmsErrorCode / MmsError / MmsTransport / MockTransport / BerEncoder + v0.107.0 IEC 61850 GOOSE 快速事件传输 — 二层组播 EtherType 0x88B8 + st_num/sq_num 重传状态机 + L2Transport 抽象：GooseControlBlock / GooseDataset / GooseEntry / GoosePublisher / GooseSubscriber / GoosePdu / RxStatus / GooseError / L2Transport / MockL2 + v0.108.0 IEC 61850 SV 采样值 + IEC 62351 安全 — EtherType 0x88BA SV 订阅 + SM4-GCM/SM3-HMAC 加密封装 + 会话密钥管理：SvSubscriber / SvSample / SampleStatus / RingBuffer / SvError / SecureGoose / SecureSv / SecureFrame / SessionKey / KeyMgmt / SecError + v0.109.0 故障录波 COMTRADE — 环形采样缓冲 + 7 类故障触发 + C37.111 .cfg/.dat 生成 + FileSink 导出抽象：FaultRecorder / RecorderConfig / RecorderState / RecorderError / FileSink / MockSink / RingSampleBuffer / TriggerCondition / TriggerType / ComtradeWriter / ComtradeConfig / ComtradeFormat / ChannelConfig / Phase / SampleRecord + v0.110.0 云边数据同步 — 事件溯源存储（CRC32-IEEE）+ 增量批量同步（二进制帧 0xC537）+ 指数退避重试队列（有界死信）+ SyncTransport 抽象：EventStore / Event / EventType / crc32 / DeltaSync / CompressionType / SyncBatch / SyncStats / RetryQueue / SyncTransport / MockSyncTransport / SyncError + v0.111.0 模型 OTA 推送 — 断点续传 + SM2/SM3 验签 + 白名单热切换回滚：OtaClient / ModelInfo / ModelSignature / SigAlgorithm / HotLoader / ModelInstance / OtaTransport / MockOtaTransport / OtaError / OtaStats / OtaUpdateOutcome + v0.113.0 Secure Boot 全链验证 — 118B 镜像签名头（magic ESIG + version 1 全小端）+ SM3 哈希 + SM2 验签 + 防降级时间戳下限 + 四级信任链逐级推进（Rom→Bootloader→Kernel→Runtime→Complete）：BootStage / ImageSignature / ChainOfTrust / BootVerifier / BootError / BootStats / encode_header / decode_header / HEADER_LEN + v0.114.0 测量启动与远程证明 — SM3-only 24 PCR 单 bank + TCG extend 共享函数 + 事件日志 measure/replay + nonce 内嵌 PcrQuote（quote_digest 签名绑定防重放）+ SM2 AK 签名 Quote（64B 定长签名）+ AttestVerifier 四步验证流水线（NonceMismatch→SignatureInvalid→EventLogInconsistent→PcrMismatch）+ 故障注入 SoftTpm/MockAttestTransport：PcrBank / TpmBackend / SoftTpm / TcgEvent / TcgEventLog / PcrQuote / RemoteAttestation / AttestVerifier / AttestResult / AttestReason / TpmError / AttestError / AttestStats / AttestTransport / MockAttestTransport / pcr_extend_value / quote_digest / PCR_COUNT + v0.115.0 mTLS 双向认证通信安全 — SM 密码套件协商（服务端优先选首个交集）+ 证书管理（验签→有效期→CRL 固定顺序 + Copy CertError）+ 双向握手状态机（SM2 临时密钥交换签名证明私钥持有 + SM3-HMAC Finished + SM3 标签分离密钥派生）+ 记录层 SM4-GCM/CBC（序列号 nonce + AAD 绑序列号 + 64 位防重放窗口）+ MtlsTransport 同步传输抽象：SmCipherSuite / KeyExchange / Cipher / MacAlgorithm / negotiate / CertError / TlsError / TlsStats / MtlsTransport / MockMtlsTransport / CertManager / MtlsContext / HandshakeOutcome / MtlsRecord) are standalone no_std crates
        // with no arch-specific code, host-testable.
        Self::run_cargo(
            &[
                "clippy",
                "--workspace",
                "--exclude",
                "eneros-kernel",
                "--exclude",
                "eneros-hello",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            GateError::ClippyWarning("clippy reported warnings (see output above)".to_string()),
        )
    }

    fn run_audit(&self) -> Result<(), GateError> {
        match Command::new("cargo")
            .args(["deny", "check", "advisories", "licenses", "bans", "sources"])
            .status()
        {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(GateError::VulnFound(
                "cargo-deny reported advisories/license/ban/source issues".to_string(),
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(GateError::IoError(format!(
                "failed to spawn `cargo deny`: {}",
                e
            ))),
        }
    }

    fn run_tests(&self) -> Result<(), GateError> {
        // eneros-kernel and eneros-hello are excluded: both define
        // #[panic_handler] / #![no_main] which can't be tested on host.
        // eneros-runtime (v0.4.0) and eneros-hal (v0.5.0+) are library crates
        // and are host-testable.
        // Note: eneros-hal arm64 module (v0.6.0 core + v0.7.0 peripherals + v0.8.0 mm + v0.9.0 partition) is cfg-gated by
        // #[cfg(target_arch = "aarch64")] and excluded from host tests.
        // eneros-heap (v0.10.0), eneros-user-heap (v0.11.0), eneros-time (v0.12.0 + v0.12.1 beidou + v0.12.2 holdover),
        // eneros-watchdog (v0.13.0), eneros-panic (v0.14.0), eneros-smp (v0.15.0), eneros-sched (v0.16.0), eneros-smp coherence (v0.17.0),
        // eneros-mm isolation (v0.9.1 compliance), eneros-power (v0.17.1 power management), eneros-sched (v0.18.0 thread abstraction, v0.19.0 partition scheduler),
        // eneros-ipc (v0.20.0 IPC endpoint + v0.21.0 SPSC ring), eneros-controlbus (v0.22.0 Control Bus + TTL + dual-plane),
        // eneros-storage (v0.23.0 eMMC/NVMe Block Device),
        // eneros-fs (v0.24.0 littlefs2 file system + v0.24.1 wear-leveling),
        // eneros-tsdb (v0.25.0 TSDB),
        // eneros-config (v0.26.0 config management),
        // and eneros-net (v0.27.0 Ethernet network driver + v0.28.0 TCP/IP stack + v0.29.0 Socket abstraction layer),
        // eneros-cellular (v0.30.0/v0.30.1/v0.30.2 Cellular Modem + PPP + Failover),
        // eneros-crypto (v0.33.0 国密 SM2/SM3/SM4 + CSRNG + PKI 证书基础 — pure Rust, no_std, no C FFI),
        // eneros-agent (v0.42.1 Agent Lifecycle + Spawner + Heartbeat + Crash Recovery + Capability Token + Capability Manager + System Agent + 故障恢复编排 — LifecycleManager / LifecycleHook / AgentSpawner / AgentFactory / HeartbeatMonitor / CrashRecovery / CheckpointStore / CapabilityToken / CapabilityManager / TokenStore / SystemAgent / ResourceMonitor / DependencyGraph / RecoveryOrchestrator),
        // eneros-driver-framework (v0.43.0 用户态设备驱动统一抽象 — DeviceDriver trait / DriverId / DriverType / DriverState / DriverHealth / DriverError / registry / handle / mock),
        // eneros-rs485 (v0.44.0 RS-485 串行驱动), eneros-can (v0.47.0 CAN 2.0A/B 驱动),
        // eneros-modbus-rtu (v0.45.0 Modbus RTU 主站), eneros-modbus-tcp (v0.46.0 Modbus TCP 主站),
        // eneros-iec104-slave (v0.48.0 IEC 60870-5-104 从站 — APDU/ASDU codec + interrogation/command/clock sync),
        // eneros-iec104-master (v0.49.0 IEC 60870-5-104 主站 — multi-device polling + interrogation/command/clock sync),
        // eneros-upa-model (v0.50.0 统一点表模型 UPA — DataPoint / PointType / PointValue / PointQuality / PointDatabase),
        // eneros-protocol-abstract (v0.51.0 协议抽象层 — PointAccess / ProtocolAdapter / ProtocolManager / MockAdapter),
        // eneros-calibration (v0.51.1 计量校准 — CalibCoeffs / AccuracyClass / MeterCalibration / CalibStore / InMemoryCalibStore),
        // eneros-telemetry-model (v0.52.0 四遥标准数据模型 — Telemetry / Telesignaling / Telecontrol / Teleadjust / DeadbandFilter / QualityFlag),
        // eneros-soe-engine (v0.53.0 SOE 事件顺序记录引擎 — SoeEvent / SoeEventType / EventPriority / SoeEngine / SoeStorage / EventTrigger),
        // eneros-mqtt (v0.53.1 MQTT 物联网上报 — MqttClient / QoS / MqttPacket / LastWill / TopicFilter / 指数退避重连),
        // eneros-alarm (v0.53.2 告警管理体系 — AlarmManager / AlarmRecord / AlarmLevel / SuppressionRule / EscalationPolicy),
        // eneros-rtos-control (v0.54.0 RTOS 控制闭环引擎 — PidController / SetpointTracker / ControlLoop trait / ControlLoopEngine / PowerControlLoop),
        // eneros-rtos-sampling (v0.55.0 高频采样服务 — SamplingService / StateSnapshot / SharedMemorySnapshot / SampledPoint / SamplingStats),
        // eneros-rtos-cmd-exec (v0.56.0 命令消费与执行 — CommandExecutor / DeviceStateProvider / DevicePointMap / ExecutorStats / ExecutorReport),
        // eneros-rtos-degrade (v0.57.0 降级规则引擎 — DegradeEngine / DegradeMode / DegradeRule / DegradeContext / SafeDefaults / DegradeStats / 5 builtin rules),
        // eneros-rtos-watchdog-degrade (v0.58.0 看门狗与端到端降级流程 — WatchdogDegradeFlow / DegradeState / HeartbeatWatcher / RecoveryManager / FlowStats / 5-state machine),
        // eneros-llm-engine (v0.59.0 LLM 推理引擎选型与 FFI 封装 — LlmEngine trait / MockEngine / LlamaCppEngine / ComputeDevice / Quantization / InferParams / ModelInfo / EngineStats),
        // eneros-gguf-loader (v0.60.0 模型加载与内存管理 — GgufLoader / GgufHeader / GgufMetadata / GgufTensorInfo / GgufDtype / MmapBackend / MemoryBackend / ModelMemoryManager / GpuOps),
        // eneros-model-deploy (v0.61.0 7B INT4 量化模型部署 — QuantConfig7B / DeployVerifier / DeployBackend / MockDeployBackend / LlamaDeployBackend / PowerPromptSet / DeployReport / DeployError),
        // eneros-infer-scheduler (v0.62.0 推理调度与并发控制 — InferScheduler / InferRequest / InferResult / RequestPriority / KvCacheManager / KvCacheEntry / SchedulerStats / SchedulerError),
        // eneros-prompt-template (v0.63.0 Prompt 模板系统 + JSON 输出约束 — PromptTemplate trait / TemplateContext / SchemaSpec / SchemaField / SchemaType / ChargeDischargeTemplate / DispatchTemplate / AlarmTemplate / JsonConstraint / ConstraintStats / extract_json / TemplateError),
        // eneros-solver-core (v0.64.0 LP 求解器集成 — Solver trait / MockSolver / HighsSolver / LpProblem / ConstraintMatrix / VarType / ObjectiveSense / SolveResult / SolveStatus / SolverStatus / SolverError),
        // eneros-solver-model (v0.65.0 优化问题建模框架 — Variable / VarBuilder / LinearExpr / Constraint / OptProblem / compile() to LpProblem CSR),
        // eneros-energy-lp-model (v0.66.0 能源调度 LP 模型 — ScheduleConfig / EnergyScheduleModel / ScheduleEntry / ScheduleResult / SOC dynamics / ramp / objective),
        // eneros-safety-validator (v0.67.0 安全校验器 — SafetyValidator / SafetyRule / ElectricalSafetyRule / ProtectionCoordinationRule / ValidationResult / Violation / Severity / SystemState),
        // eneros-intent-parser (v0.68.0 意图解析器 — IntentParser / Intent / IntentType / TimeRange / PowerIntent / SocIntent / IntentError),
        // eneros-intent-contract (v0.69.0 意图契约 — IntentContract / FeedbackContract / ContractValidator / ContractConverter / ContractError / SystemContext / LlmMeta / DeviceStatus),
        // eneros-fast-path (v0.70.0 实时快速路径 — RealtimePathEngine / PathSelector / StrategyTable / PathType / RealtimeState / FastPathResult / FastPathError),
        // eneros-dual-brain (v0.71.0 双脑协同联调 — DualBrainCoordinator / DualBrainResult / LatencyBreakdown / DualBrainError / DispatchCommand / CommandSink / MockCommandSink),
        // eneros-energy-market-agent (v0.72.0 Energy Agent + Market Agent — EnergyAgent / MarketAgent / AgentRuntime / HeartbeatStatus / AgentRuntimeError / MarketData / MarketSignal / MarketChannel / MarketDataSource / MockMarketSource),
        // eneros-device-agent (v0.73.0 Device Agent — DeviceAgent / DeviceAdapter / MockDevice / DeviceRegistry / DeviceInfo / DeviceType / DeviceState / DeviceSnapshot / DeviceCommand / CommandSource / MockCommandSource / DeviceError),
        // eneros-mvp-scenario (v0.74.0 MVP 端到端集成 — MvpOrchestrator / MvpTickReport / RevenueComparator / TraditionalEms / MvpError),
        // eneros-agent-bus-dds (v0.78.0 Agent 消息签名层 — DdsNode / MockDdsNode / CycloneDdsNode / DdsConfig / DiscoveryPolicy / QosPolicy / DdsSample / DdsError / TopicSpec / TopicRegistry / TopicError / TopicCategory / PayloadType / validate_topic_name / standard_topics / MessageRouter / RoutingPolicy / RouteDecision / DropReason / RouteError / RouterStats / Subscription / SubId / CapabilityVerifier / MockCapabilityVerifier / Permission / AgentId / pattern_matches / CodecKind / CodecError / KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify),
        // eneros-tsn-time (v0.79.0 gPTP 时间同步层 — ClockIdentity / MacAddr / PtpTime / Port / PortRole / PortState / AnnounceMessage / BmcaResult / SyncMessage / FollowUpMessage / GptpConfig / GptpClock / compare_priority / run_bmca / handle_sync / handle_follow_up / adjust_clock / compute_offset / current_time / to_announce + v0.80.0 TSN 802.1Qbv 调度：TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config + v0.81.0 TSN 驱动胶合层 + 端到端时延探针：TsnError / TsnDriver / MockTsnDriver / driver_send_closure / DelayStats / LatencyProbe + v0.82.0 Grid Agent：GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors + v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor + v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer + v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber + v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize + v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split + v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives + v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError + v0.90.0 孪生预测：Predictor / ForecastModel / ForecastResult / ForecastPoint / PersistenceModel / MeanModel / ForecastError + v0.91.0 What-if 分析：WhatIfAnalyzer / SimModel / AnalyticalSimModel / Scenario / ScenarioResult / Outcome / RiskLevel / Action / WhatIfError + v0.92.0 域内仲裁：DomainArbiter / ArbiterPolicy / ArbitrationRequest / ArbitrationResult / ArbitrationReason / Claim / Priority / detect_deadlock + v0.93.0 域级优化：DomainOptimizer / EdgeBoxState / DomainPlan / OptError + v0.94.0 VPP 聚合：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType + v0.95.0 云端策略下发：Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher / validate_strategy + v0.96.0 云端数据汇聚：DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink + v0.97.0 联邦发现：MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus + v0.98.0 跨域通信通道：TlsConfig / Endpoint / ChannelError / SecureTransport / MockSecureTransport / FederationChannel + v0.98.1 纵向加密认证：TunnelKeys / VerticalEncryptTunnel / EncryptError / DispatchToken / AuthResult / VerticalEncryptDevice / MockVerticalEncryptDevice / TunnelManager / hmac_sm3 / Sm3Hmac + v0.99.0 联邦共识协议：NodeId / ConsensusState / MsgType / PbftMessage / LogEntry / ConsensusResult / ConsensusError / ConsensusBus / MockConsensusBus / ConsensusEngine / f / quorum / primary_of / sign_message / verify_message + v0.100.0 资源争抢竞价：AgentId / Price / Qty / BidOrder / AskOrder / OrderBook / Match / MatchResult / AuctionError / AuctionEngine / match_book / match_digest + v0.101.0 断网处理与孤岛模式：PartitionState / PartitionDetector / IslandMode / EventCache / RecoverySync / SyncSink / MockSyncSink / SyncError / SyncReport + v0.102.0 MILP 求解器集成：UcUnit / UnitCommitment / UnitSchedule / DayAheadPlan / DayAheadScheduler + v0.103.0 Solver 神经热启动：CandidateSolution / InferEngine / MockEngine / OnnxEngine / HeuristicNet / SolveContext / WarmStartProvider / WarmStarter / WarmError + v0.104.0 多目标 Pareto 优化：MultiObjectiveProblem / Objective / OptDirection / VariableSpec / ParetoSolution / ParetoFront / ParetoSolver / Nsga2Solver / DecisionMaker + v0.105.0 IEC 61850 信息模型：LogicalDevice / LogicalNode / LnClass / DataObject / CommonDataClass / DataAttribute / FunctionalConstraint / DaValue / Quality / Validity / Source / Iec61850Model / SclParser / ModelError + v0.106.0 IEC 61850 MMS 协议栈 — BER 编解码 + ACSE 关联 + COTP + MMS Read/Write 客户端 + MmsTransport 抽象：MmsClient / MmsConnection / ConnState / MmsRequest / MmsResponse / VarAccessSpec / MmsReadResult / MmsWriteResult / MmsErrorCode / MmsError / MmsTransport / MockTransport / BerEncoder + v0.107.0 IEC 61850 GOOSE 快速事件传输 — 二层组播 EtherType 0x88B8 + st_num/sq_num 重传状态机 + L2Transport 抽象：GooseControlBlock / GooseDataset / GooseEntry / GoosePublisher / GooseSubscriber / GoosePdu / RxStatus / GooseError / L2Transport / MockL2 + v0.108.0 IEC 61850 SV 采样值 + IEC 62351 安全 — EtherType 0x88BA SV 订阅 + SM4-GCM/SM3-HMAC 加密封装 + 会话密钥管理：SvSubscriber / SvSample / SampleStatus / RingBuffer / SvError / SecureGoose / SecureSv / SecureFrame / SessionKey / KeyMgmt / SecError + v0.109.0 故障录波 COMTRADE — 环形采样缓冲 + 7 类故障触发 + C37.111 .cfg/.dat 生成 + FileSink 导出抽象：FaultRecorder / RecorderConfig / RecorderState / RecorderError / FileSink / MockSink / RingSampleBuffer / TriggerCondition / TriggerType / ComtradeWriter / ComtradeConfig / ComtradeFormat / ChannelConfig / Phase / SampleRecord + v0.110.0 云边数据同步 — 事件溯源存储（CRC32-IEEE）+ 增量批量同步（二进制帧 0xC537）+ 指数退避重试队列（有界死信）+ SyncTransport 抽象：EventStore / Event / EventType / crc32 / DeltaSync / CompressionType / SyncBatch / SyncStats / RetryQueue / SyncTransport / MockSyncTransport / SyncError + v0.111.0 模型 OTA 推送 — 断点续传 + SM2/SM3 验签 + 白名单热切换回滚：OtaClient / ModelInfo / ModelSignature / SigAlgorithm / HotLoader / ModelInstance / OtaTransport / MockOtaTransport / OtaError / OtaStats / OtaUpdateOutcome + v0.113.0 Secure Boot 全链验证 — 118B 镜像签名头（magic ESIG + version 1 全小端）+ SM3 哈希 + SM2 验签 + 防降级时间戳下限 + 四级信任链逐级推进（Rom→Bootloader→Kernel→Runtime→Complete）：BootStage / ImageSignature / ChainOfTrust / BootVerifier / BootError / BootStats / encode_header / decode_header / HEADER_LEN + v0.114.0 测量启动与远程证明 — SM3-only 24 PCR 单 bank + TCG extend 共享函数 + 事件日志 measure/replay + nonce 内嵌 PcrQuote（quote_digest 签名绑定防重放）+ SM2 AK 签名 Quote（64B 定长签名）+ AttestVerifier 四步验证流水线（NonceMismatch→SignatureInvalid→EventLogInconsistent→PcrMismatch）+ 故障注入 SoftTpm/MockAttestTransport：PcrBank / TpmBackend / SoftTpm / TcgEvent / TcgEventLog / PcrQuote / RemoteAttestation / AttestVerifier / AttestResult / AttestReason / TpmError / AttestError / AttestStats / AttestTransport / MockAttestTransport / pcr_extend_value / quote_digest / PCR_COUNT + v0.115.0 mTLS 双向认证通信安全 — SM 密码套件协商（服务端优先选首个交集）+ 证书管理（验签→有效期→CRL 固定顺序 + Copy CertError）+ 双向握手状态机（SM2 临时密钥交换签名证明私钥持有 + SM3-HMAC Finished + SM3 标签分离密钥派生）+ 记录层 SM4-GCM/CBC（序列号 nonce + AAD 绑序列号 + 64 位防重放窗口）+ MtlsTransport 同步传输抽象：SmCipherSuite / KeyExchange / Cipher / MacAlgorithm / negotiate / CertError / TlsError / TlsStats / MtlsTransport / MockMtlsTransport / CertManager / MtlsContext / HandshakeOutcome / MtlsRecord) are standalone no_std crates
        // with no arch-specific code, host-testable.
        Self::run_cargo(
            &[
                "test",
                "--workspace",
                "--exclude",
                "eneros-kernel",
                "--exclude",
                "eneros-hello",
            ],
            GateError::TestFailed,
        )
    }

    fn run_all(&self) -> GateReport {
        // fmt
        let start = Instant::now();
        let mut fmt_cr = CheckResult::from(self.run_fmt_check());
        fmt_cr.name = "fmt";
        fmt_cr.duration_ms = start.elapsed().as_millis() as u64;

        // clippy
        let start = Instant::now();
        let mut clippy_cr = CheckResult::from(self.run_clippy());
        clippy_cr.name = "clippy";
        clippy_cr.duration_ms = start.elapsed().as_millis() as u64;

        // audit (special: detect degraded mode so we can annotate the result)
        let start = Instant::now();
        let audit_cr = if !Self::cargo_deny_available() {
            CheckResult {
                name: "audit",
                passed: true,
                duration_ms: start.elapsed().as_millis() as u64,
                message: Some("cargo-deny not found, audit skipped (degraded)".to_string()),
            }
        } else {
            let mut cr = CheckResult::from(self.run_audit());
            cr.name = "audit";
            cr.duration_ms = start.elapsed().as_millis() as u64;
            cr
        };

        // test
        let start = Instant::now();
        let mut test_cr = CheckResult::from(self.run_tests());
        test_cr.name = "test";
        test_cr.duration_ms = start.elapsed().as_millis() as u64;

        let results = [fmt_cr, clippy_cr, audit_cr, test_cr];
        let overall_pass = results.iter().all(|r| r.passed);
        GateReport {
            results,
            overall_pass,
        }
    }
}

impl From<Result<(), GateError>> for CheckResult {
    fn from(r: Result<(), GateError>) -> Self {
        match r {
            Ok(()) => CheckResult {
                name: "",
                passed: true,
                duration_ms: 0,
                message: None,
            },
            Err(e) => CheckResult {
                name: "",
                passed: false,
                duration_ms: 0,
                message: Some(e.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GateError;

    fn pass(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            passed: true,
            duration_ms: 0,
            message: None,
        }
    }

    fn fail(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            passed: false,
            duration_ms: 0,
            message: Some("failure".to_string()),
        }
    }

    #[test]
    fn test_all_pass() {
        let results = [pass("fmt"), pass("clippy"), pass("audit"), pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(overall_pass);

        let report = GateReport {
            results,
            overall_pass,
        };
        assert!(report.overall_pass);
    }

    #[test]
    fn test_one_fail() {
        let results = [pass("fmt"), fail("clippy"), pass("audit"), pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(!overall_pass);

        // Any single failure flips the overall result.
        for i in 0..4 {
            let mut r = [pass("fmt"), pass("clippy"), pass("audit"), pass("test")];
            r[i] = fail("x");
            assert!(!r.iter().all(|c| c.passed), "index {} should fail", i);
        }
    }

    #[test]
    fn test_from_ok() {
        let cr = CheckResult::from(Ok(()));
        assert!(cr.passed);
        assert!(cr.message.is_none());
        assert_eq!(cr.duration_ms, 0);
    }

    #[test]
    fn test_from_err() {
        let cr = CheckResult::from(Err(GateError::TestFailed));
        assert!(!cr.passed);
        let msg = cr.message.expect("expected a failure message");
        assert!(msg.contains("TestFailed"));
    }

    #[test]
    fn test_audit_degraded() {
        // A degraded audit passes but carries a notice.
        let degraded = CheckResult {
            name: "audit",
            passed: true,
            duration_ms: 0,
            message: Some("cargo-deny not found, audit skipped (degraded)".to_string()),
        };
        assert!(degraded.passed);
        assert!(degraded.message.as_ref().unwrap().contains("degraded"));

        // A degraded audit must not cause overall failure.
        let results = [pass("fmt"), pass("clippy"), degraded, pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(overall_pass);
    }
}
