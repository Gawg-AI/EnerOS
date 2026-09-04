//! EnerOS Quality Gate — Core
//!
//! Defines the quality gate data structures, the [`QualityGate`] trait, and
//! the [`DefaultGate`] implementation that wraps `cargo` subcommands
//! (fmt / clippy / deny / test).

use std::fs;
use std::io;
use std::path::Path;
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

/// Aggregated report for all seven quality checks.
#[derive(Debug)]
pub struct GateReport {
    /// Results for `[fmt, clippy, audit, test, version-gate, mock-gate, sign-key-gate]` in that order.
    pub results: [CheckResult; 7],
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
    /// Go/No-Go 文档与 workspace 版本一致性检查（蓝图/20 R1-1）：
    /// 任一 G 项非 PASS → 版本必须带 `-rc.` 后缀；全 PASS 才允许发布版。
    fn run_version_gate(&self) -> Result<(), GateError>;
    /// R2-1 Mock 收编门禁：crates/agents + crates/ai 下所有 `pub struct Mock`
    /// 定义行上方必须有 cfg(any(test, feature = "mock")) 保护。
    fn run_mock_gate(&self) -> Result<(), GateError>;
    /// 签名钥显式来源门禁（蓝图 66 WP-66.1.1）：tools/ 与 .github/workflows/
    /// 内所有 `xtask config compile` 调用块必须携带显式钥来源。
    fn run_sign_key_gate(&self) -> Result<(), GateError>;
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
        // eneros-agent-bus-dds (v0.78.0 Agent 消息签名层 — DdsNode / MockDdsNode / DdsConfig / DiscoveryPolicy / QosPolicy / DdsSample / DdsError / TopicSpec / TopicRegistry / TopicError / TopicCategory / PayloadType / validate_topic_name / standard_topics / MessageRouter / RoutingPolicy / RouteDecision / DropReason / RouteError / RouterStats / Subscription / SubId / CapabilityVerifier / MockCapabilityVerifier / Permission / AgentId / pattern_matches / CodecKind / CodecError / KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify),
        // eneros-tsn-time (v0.79.0 gPTP 时间同步层 — ClockIdentity / MacAddr / PtpTime / Port / PortRole / PortState / AnnounceMessage / BmcaResult / SyncMessage / FollowUpMessage / GptpConfig / GptpClock / compare_priority / run_bmca / handle_sync / handle_follow_up / adjust_clock / compute_offset / current_time / to_announce + v0.80.0 TSN 802.1Qbv 调度：TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config + v0.81.0 TSN 驱动胶合层 + 端到端时延探针：TsnError / TsnDriver / MockTsnDriver / driver_send_closure / DelayStats / LatencyProbe + v0.82.0 Grid Agent：GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors + v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor + v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer + v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber + v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize + v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split + v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives + v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError + v0.90.0 孪生预测：Predictor / ForecastModel / ForecastResult / ForecastPoint / PersistenceModel / MeanModel / ForecastError + v0.91.0 What-if 分析：WhatIfAnalyzer / SimModel / AnalyticalSimModel / Scenario / ScenarioResult / Outcome / RiskLevel / Action / WhatIfError + v0.92.0 域内仲裁：DomainArbiter / ArbiterPolicy / ArbitrationRequest / ArbitrationResult / ArbitrationReason / Claim / Priority / detect_deadlock + v0.93.0 域级优化：DomainOptimizer / EdgeBoxState / DomainPlan / OptError + v0.94.0 VPP 聚合：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType + v0.95.0 云端策略下发：Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher / validate_strategy + v0.96.0 云端数据汇聚：DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink + v0.97.0 联邦发现：MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus + v0.98.0 跨域通信通道：TlsConfig / Endpoint / ChannelError / SecureTransport / MockSecureTransport / FederationChannel + v0.98.1 纵向加密认证：TunnelKeys / VerticalEncryptTunnel / EncryptError / DispatchToken / AuthResult / VerticalEncryptDevice / MockVerticalEncryptDevice / TunnelManager / hmac_sm3 / Sm3Hmac + v0.99.0 联邦共识协议：NodeId / ConsensusState / MsgType / PbftMessage / LogEntry / ConsensusResult / ConsensusError / ConsensusBus / MockConsensusBus / ConsensusEngine / f / quorum / primary_of / sign_message / verify_message + v0.100.0 资源争抢竞价：AgentId / Price / Qty / BidOrder / AskOrder / OrderBook / Match / MatchResult / AuctionError / AuctionEngine / match_book / match_digest + v0.101.0 断网处理与孤岛模式：PartitionState / PartitionDetector / IslandMode / EventCache / RecoverySync / SyncSink / MockSyncSink / SyncError / SyncReport + v0.102.0 MILP 求解器集成：UcUnit / UnitCommitment / UnitSchedule / DayAheadPlan / DayAheadScheduler + v0.103.0 Solver 神经热启动：CandidateSolution / InferEngine / MockEngine / OnnxEngine / HeuristicNet / SolveContext / WarmStartProvider / WarmStarter / WarmError + v0.104.0 多目标 Pareto 优化：MultiObjectiveProblem / Objective / OptDirection / VariableSpec / ParetoSolution / ParetoFront / ParetoSolver / Nsga2Solver / DecisionMaker + v0.105.0 IEC 61850 信息模型：LogicalDevice / LogicalNode / LnClass / DataObject / CommonDataClass / DataAttribute / FunctionalConstraint / DaValue / Quality / Validity / Source / Iec61850Model / SclParser / ModelError + v0.106.0 IEC 61850 MMS 协议栈 — BER 编解码 + ACSE 关联 + COTP + MMS Read/Write 客户端 + MmsTransport 抽象：MmsClient / MmsConnection / ConnState / MmsRequest / MmsResponse / VarAccessSpec / MmsReadResult / MmsWriteResult / MmsErrorCode / MmsError / MmsTransport / MockTransport / BerEncoder + v0.107.0 IEC 61850 GOOSE 快速事件传输 — 二层组播 EtherType 0x88B8 + st_num/sq_num 重传状态机 + L2Transport 抽象：GooseControlBlock / GooseDataset / GooseEntry / GoosePublisher / GooseSubscriber / GoosePdu / RxStatus / GooseError / L2Transport / MockL2 + v0.108.0 IEC 61850 SV 采样值 + IEC 62351 安全 — EtherType 0x88BA SV 订阅 + SM4-GCM/SM3-HMAC 加密封装 + 会话密钥管理：SvSubscriber / SvSample / SampleStatus / RingBuffer / SvError / SecureGoose / SecureSv / SecureFrame / SessionKey / KeyMgmt / SecError + v0.109.0 故障录波 COMTRADE — 环形采样缓冲 + 7 类故障触发 + C37.111 .cfg/.dat 生成 + FileSink 导出抽象：FaultRecorder / RecorderConfig / RecorderState / RecorderError / FileSink / MockSink / RingSampleBuffer / TriggerCondition / TriggerType / ComtradeWriter / ComtradeConfig / ComtradeFormat / ChannelConfig / Phase / SampleRecord + v0.110.0 云边数据同步 — 事件溯源存储（CRC32-IEEE）+ 增量批量同步（二进制帧 0xC537）+ 指数退避重试队列（有界死信）+ SyncTransport 抽象：EventStore / Event / EventType / crc32 / DeltaSync / CompressionType / SyncBatch / SyncStats / RetryQueue / SyncTransport / MockSyncTransport / SyncError + v0.111.0 模型 OTA 推送 — 断点续传 + SM2/SM3 验签 + 白名单热切换回滚：OtaClient / ModelInfo / ModelSignature / SigAlgorithm / HotLoader / ModelInstance / OtaTransport / MockOtaTransport / OtaError / OtaStats / OtaUpdateOutcome + v0.113.0 Secure Boot 全链验证 — 118B 镜像签名头（magic ESIG + version 1 全小端）+ SM3 哈希 + SM2 验签 + 防降级时间戳下限 + 四级信任链逐级推进（Rom→Bootloader→Kernel→Runtime→Complete）：BootStage / ImageSignature / ChainOfTrust / BootVerifier / BootError / BootStats / encode_header / decode_header / HEADER_LEN + v0.114.0 测量启动与远程证明 — SM3-only 24 PCR 单 bank + TCG extend 共享函数 + 事件日志 measure/replay + nonce 内嵌 PcrQuote（quote_digest 签名绑定防重放）+ SM2 AK 签名 Quote（64B 定长签名）+ AttestVerifier 四步验证流水线（NonceMismatch→SignatureInvalid→EventLogInconsistent→PcrMismatch）+ 故障注入 SoftTpm/MockAttestTransport：PcrBank / TpmBackend / SoftTpm / TcgEvent / TcgEventLog / PcrQuote / RemoteAttestation / AttestVerifier / AttestResult / AttestReason / TpmError / AttestError / AttestStats / AttestTransport / MockAttestTransport / pcr_extend_value / quote_digest / PCR_COUNT + v0.115.0 mTLS 双向认证通信安全 — SM 密码套件协商（服务端优先选首个交集）+ 证书管理（验签→有效期→CRL 固定顺序 + Copy CertError）+ 双向握手状态机（SM2 临时密钥交换签名证明私钥持有 + SM3-HMAC Finished + SM3 标签分离密钥派生）+ 记录层 SM4-GCM/CBC（序列号 nonce + AAD 绑序列号 + 64 位防重放窗口）+ MtlsTransport 同步传输抽象：SmCipherSuite / KeyExchange / Cipher / MacAlgorithm / negotiate / CertError / TlsError / TlsStats / MtlsTransport / MockMtlsTransport / CertManager / MtlsContext / HandshakeOutcome / MtlsRecord + v0.116.0 模型签名与白名单 — SM3 哈希登记白名单（含 added_at/expiry 有效期 + 严格/宽松模式 + BTreeMap 存储）+ SM2 模型签名验证（可信公钥列表 + 吊销列表 + 复用 sm2_verify 对原消息验签）+ 加载门禁 ModelGuard（白名单→有效期→签名三步检查 + AUDIT_CAPACITY 1000 环形审计日志 + GuardStats 统计）：WhitelistEntry / ModelWhitelist / ModelSig / ModelSignatureVerifier / ModelGuard / ModelLoadAttempt / GuardStats / GuardError / VerifyError / LoadResult / AUDIT_CAPACITY + v0.117.0 审计日志与哈希链 — SM3 哈希链（event_hash=SM3(事件编码) + combined_hash=SM3(prev‖event) 链式防篡改 + 序列号单调检查 + 断裂定位验证）+ 11 类审计事件（heapless 定容字符串）+ AuditStore 持久化 trait 与 MemStore 内存后端 + 重启 recover 链状态恢复：AuditEventType / AuditEvent / ChainEntry / HashChain / AuditLogger / AuditLoggerImpl / ChainVerifier / ChainVerifierImpl / AuditStore / MemStore / AuditError / VerifyReport + v0.118.0 行为审计与异常检测 — 行为基线建模（按分钟分桶速率序列 + compute_stats 均值/总体标准差/min/max/p95 + MIN_TRAIN_SAMPLES 100 样本门槛 + trained_at 证据时间）+ 滑动窗口异常检测（定容 64 条窗口满淘汰最旧 + 2σ/4σ 偏离分级 RateSpike/RateDrop + 非工作时段 OffHours + 越权访问 ScopeExpansion 最小规则）+ 定容告警队列（满显式 AlertQueueFull 不静默丢弃）：BehaviorDim / StatBaseline / BehaviorBaseline / AnomalyType / AlertLevel / Alert / BaselineTrainer / BaselineTrainerImpl / compute_stats / MIN_TRAIN_SAMPLES / DEFAULT_WINDOW_HOURS / AlertHandler / AlertQueue / ThresholdConfig / AnomalyDetector / StatAnomalyDetector / AnomalyError + v0.119.0 纵深安全联调与渗透测试 — 7 层安全编排（SecurityLayer/LayerStatus + check_all_layers 外部时间注入 + disabled 层 unhealthy）+ 安全事件分级响应（Critical→Isolate+Shutdown / High→Freeze+Alert / Medium→Throttle / Low|Info→Alert）+ 30 用例渗透套件真实调用各层组件 API 断言拒绝（L1 篡改镜像/回滚、L2 伪造 PCR/Quote 重放、L3 MITM/自签证书/CRL 吊销/弱套件、L4 未签名/篡改模型、L5 篡改/删除/注入日志、L6 高频/凌晨/越权、L7 共识伪造/身份伪造/DoS/分区）+ PentestReport 出口判定（30/30 Passed 且 0 Critical → overall_pass）：SecurityOrchestrator / SecurityOrchestratorImpl / SecurityIncident / ResponseAction / PentestSuite / PentestSuiteImpl / PentestCase / PentestSeverity / PentestStatus / PentestReport / PentestError + v0.120.0 Metrics 指标采集（P2-J 可观测性第 1 版）— 统一指标框架覆盖系统层（CPU/内存/温度）+ Agent 层（活跃数/延迟/错误率）+ 业务层（控制次数/优化调用数）：指标数据点模型（name≤64B + 标签≤8 个）+ 注册表（容量 64 拒绝重复注册）+ 采集器 record/increment/query + 256 条内存环形缓冲 + 定时批量刷入 + 按名分桶环形存储（写满丢弃最旧）：MetricType / MetricValue / Label / Metric / MetricDef / MetricRegistry / MetricsCollector / MetricsCollectorImpl / MetricsStore / MemoryMetricsStore / MetricsError + v0.121.0 结构化日志（P2-J 可观测性第 2 版）— crates/observability/logging：LogLevel / LogEntry / LogFilter / LogBuilder / StructuredLogger / StructuredLoggerImpl / LogStorage / MemoryLogStorage / LogError + v0.122.0 分布式追踪（P2-J 可观测性第 3 版·收官版）— crates/observability/tracing：TraceId / SpanId / SpanStatus / Span / SpanBuilder / TraceError / TraceContext / encode_trace_context / decode_trace_context / Tracer / TracerImpl / render_trace + v0.123.0 A/B 分区升级（P2-K OTA 与 Recovery 第 1 版）— crates/security/ota-ab：PartitionSlot / PartitionState / Partition / UpgradeMeta / OtaError / OtaManager / AbPartitionOta / boot_health_check / PartitionTable + v0.124.0 Agent 热更新（P2-K OTA 与 Recovery 第 2 版）— crates/agents/hot-update：HotUpdateState / AgentVersion / HotUpdateContext / HotUpdateError / AgentHotUpdater / AgentHotUpdaterImpl / ArcHotSwap + v0.125.0 Recovery Mode 最小化恢复内核（P2-K OTA 与 Recovery 第 3 版）— crates/security/recovery：RecoveryCommand / RecoveryState / DiagResult / FlashProgress / RecoveryError / parse_command / menu_text / RecoveryMode / RecoveryModeImpl / FlashWriter / FactoryReset / ConfigEntry / Diagnostician / HealthProbe / DefaultProbe / CONFIG_CAPACITY + v0.126.0 VPP 场景端到端验证（P2-K OTA 与 Recovery 第 4 版·Phase 2 收官版）— crates/agents/vpp-scenario：VppRole / EdgeNode / VppAggregation / VppError / VppScenario / VppScenarioImpl / DrEvent / DrState / DrHandler / ExitCriterion / ExitEvidence / Phase2ExitReport / ExitReportBuilder + v0.127.0 自研内核架构设计 + 接口规范（Phase 3 首版）— crates/kernel/microkernel：KernelError / BootInfo / KernelState / KernelVersion / KERNEL_VERSION / KernelSubsystem / SubsystemRegistry / SyscallHandler / SyscallNum / BootstrapHandle / CapabilityHandle / HalHandle / IpcHandle / MemoryHandle / SchedulerHandle / TimeHandle + v0.128.0 内核构建系统 + 兼容性设计（Phase 3 P3-A 第 2 版）— crates/kernel/microkernel：build.rs / linker.ld / _start / rust_main / KernelImageLayout / BuildConfig / OptLevel / Sel4CompatLayer / build_smoke + v0.129.0 Bootloader + 内核入口（Phase 3 P3-A 第 3 版★）— crates/kernel/bootloader：BootProtocol / BOOT_PROTOCOL_MAGIC / BOOT_PROTOCOL_VERSION / BootStage / BootError / SerialPort / Pl011Uart / RegIo / MmioRegIo / parse_dtb_ram / FdtError / boot_test.sh + v0.130.0 物理内存管理 buddy system（Phase 3 P3-B 首版★）— crates/kernel/microkernel mem：PhysFrameNum / ZoneType / PageFlags / PageMetadata / PhysMemManager / BuddyAllocator / Zone / PAGE_SIZE / MAX_ORDER / MAX_ZONES / MAX_FRAMES_PER_ZONE + v0.131.0 虚拟内存与页表（Phase 3 P3-B 第 2 版）— crates/kernel/microkernel mem：PageTableEntry / PageTable / map_page / unmap_page / translate / map_block / VirtAddr / PageTableLevel / MemAttrs / attrs_to_bits / AddressSpaceId / AddressSpace / VirtMemManager / Vmm / enable_mmu / tlbi_va + v0.132.0 分区内存隔离（Phase 3 P3-B 第 3 版·P3-B 收官★）— crates/kernel/microkernel mem：PartitionId / MemRegion / Partition / PartitionTable / PartitionMem / PartitionManager / RTOS_PARTITION_QUOTA / AGENT_PARTITION_QUOTA / DeviceId / DmaGuard / configure_dma / check_dma / check_access / create_partition / map_partition_mem / destroy_partition + v0.133.0 中断管理 GICv3（Phase 3 P3-C 首版）— crates/kernel/microkernel interrupt：IrqId / IrqTrigger / SavedRegs / IrqContext / IrqResult / IrqHandler / InterruptController / IRQ_SPURIOUS / IrqHandlerTable / IRQ_TABLE_SIZE / dispatch_irq / Gicv3 / GICD_CTLR / GICD_IGROUPR / GICD_ISENABLER / GICD_ICENABLER / GICD_ICPENDR / GICD_ICFGR / GICD_IROUTER / GICR_WAKER / GICR_SGI_BASE / PPI_BASE / SPI_BASE / register_irq_handler + v0.134.0 线程抽象与基本调度（Phase 3 P3-C 第 2 版）— crates/kernel/microkernel sched：Tid / Priority / ThreadState / SavedRegs / Tcb / Scheduler / SchedulerImpl / ReadyQueue / ReadyRing / context_switch / init_stack_frame / thread_trampoline / MAX_THREADS / TIME_QUANTUM_MS / PRIO_LEVELS / STACK_SIZE + v0.135.0 分区调度器 ARINC 653（Phase 3 P3-C 第 3 版）— crates/kernel/microkernel sched：TimeSlot / PartitionSchedule / PartitionState / MAX_SLOTS / default_schedule / ArincScheduler / PartitionScheduler / switch_address_space / last_address_space_switch) are standalone no_std crates
        // eneros-microkernel (v0.136.0 SMP 支持 多核启动/IPI/绑核 — Phase 3 P3-C 第 4 版 — crates/kernel/microkernel smp + sched：CpuId / CpuInfo / IpiMessage / IpiRing / PerCpu / SmpBootState / MAX_CPUS / IPI_RING_CAPACITY / boot_secondary / secondary_entry_addr / set_per_cpu / per_cpu / current_cpu / IPI_SGI_ID / send_ipi / broadcast_ipi / ipi_handler / drain_ipi_ring / IpiStats / SmpManager / SmpManagerImpl / pin_thread / schedule_on / Tcb.pinned_cpu)
        // eneros-microkernel (v0.137.0 多核内存一致性 自旋锁/屏障/原子封装 — Phase 3 P3-C 第 5 版 — crates/kernel/microkernel sync：SpinLock / SpinLockGuard / new / lock / try_lock / dmb / dsb / isb / dmb_ish / AtomicPtr / load / store / swap / compare_exchange / as_core / sync_smoke / SYNC_SGI_ID / sync_sgi_handler)
        // eneros-microkernel (v0.138.0 调度确定性与 WCET 验证 — Phase 3 P3-C 终版 — crates/kernel/microkernel sched/wcet：WcetMeasurement / WcetReport / WcetProfiler / WcetProfilerImpl / JitterStats / 抖动 / 上下文切换 / IRQ 延迟 / 10,000采样 / [wcet] ok)
        // eneros-microkernel (v0.139.0 同步消息传递 IPC — Phase 3 P3-D 首版 — crates/kernel/microkernel ipc：Endpoint / Message / IpcManager / Ipc trait / send / recv / call / reply / 消息寄存器 / ping-pong / [ipc] ok)
        // eneros-microkernel (v0.140.0 通知机制与异步信号 — Phase 3 P3-D 第 2 版 — crates/kernel/microkernel ipc：Notification / NotificationManager / NotificationOps / signal / poll / wait / bind_irq / WaiterRing / NotifyStats / 64 位信号 word / 中断绑定 / [notify] ok)
        // eneros-microkernel (v0.141.0 共享内存 IPC — Phase 3 P3-D 第 3 版 — crates/kernel/microkernel ipc：ShmId / ShmMapping / ShmCap / SharedMemory / ShmManager / ShmIpc / ShmStats / MAX_SHM=128 / MAX_SHM_SIZE=64MB / 连续帧游 / 能力控制映射 / grant_shm / map_partition_frame / sync_frames / dc cvau+ic ivau 缓存同步 / [shm] ok)
        // eneros-microkernel (v0.142.0 能力管理器 Capability-based — Phase 3 P3-D 终版 — crates/kernel/microkernel cap：Capability / CapId / CapRights / CapType / CapTarget / ConstraintPack / CapSpace / CapabilityManager / mint / check / revoke / derive / move / copy / create_space / destroy_space / 派生链 parent 反向链接 / 时间窗约束 / 过期令牌 / CapStats / MAX_CAPS_PER_PARTITION=4096 / [cap] ok)
        // eneros-microkernel (v0.143.0 HAL 接口与 ARM64 实现 — Phase 3 P3-E 首版 — crates/kernel/microkernel hal：Hal trait / HalRegistry / register / hal() / platform::qemu_virt / GenericTimer / cycles_to_us / clock_deadline / clock_disarm / Pl011<B: RegIo> 泛型封装 / Gpio 占位 / Aarch64Hal / from_parts / new_system / cpu_init / irq_init / enable_irq / disable_irq / irq≥1024 no-op / [hal] ok)
        // eneros-microkernel (v0.144.0 x86_64 BSP 接口占位 — Phase 3 P3-E 第 2 版 — crates/kernel/microkernel hal/mem：X86Hal / LocalApic / Uart8250 / X86PageTable / from_parts / new_system (x86_64 cfg-gated) / ADR-0001 降级口径接口占位 / [x86] ok)
        // eneros-microkernel (v0.145.0 飞腾 D2000 BSP — Phase 3 P3-E 第 3 版 ★ 信创核心交付 — crates/kernel/microkernel hal/boot：PhytiumHal 组合 Aarch64Hal + PhytiumSmmu + PhytiumCrypto / from_parts 注入式 / new(dtb) cfg-gated 真机路径 / mem_init 联动 smmu.init / PhytiumSmmu CR0 cfg-gated unsafe 写 + host Cell 状态记录 / PhytiumCrypto has_sm4/has_sm2 占位 true / boot::dtb_find_node DTB 节点查找 aarch64 cfg-gated FDT 解析 + host 占位 None / platform::phytium_d2000 八常量组 / 真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径 / [phytium] skip 占位)
        // eneros-microkernel (v0.146.0 鲲鹏 920 BSP — Phase 3 P3-E 第 4 版 ★ 信创核心 双国产平台 — crates/kernel/microkernel hal/boot：KunpengHal 组合 Aarch64Hal + KunpengSmmu + KunpengNuma / from_parts 注入式 / new(dtb) cfg-gated 真机路径 / mem_init 联动 smmu.init / KunpengSmmu SMMU v3 cfg-gated unsafe 写 + host Cell 状态记录 / KunpengNuma NUMA 拓扑探测 cfg-gated aarch64 读取 + host 占位 0 / boot::dtb_count_nodes DTB 节点统计 aarch64 cfg-gated FDT 遍历 + host 占位 0 / platform::kunpeng_920 常量组 / 真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径 / [kunpeng] skip 占位)
        // eneros-microkernel (v0.147.0 RISC-V + 龙芯 BSP 接口占位 — Phase 3 P3-E 终版本 — crates/kernel/microkernel hal：RiscvHal standalone + Plic + Clint + UartSifive + LoongArchHal standalone + LoongsonPch 复用 x86_64::Uart8250 / from_parts 注入式 / 无 cfg 门控真机路径 + 无 asm! 宏 + 无 unsafe write_volatile / Plic/Clint/LoongsonPch Cell 状态记录 + 观测口 / UartSifive 复用 eneros_bootloader::RegIo / platform::riscv_virt + platform::loongarch_qemu 常量组 / ADR-0001 BSP 收敛降级口径接口占位 + host 可测骨架 + PPI7 跨架构六平台多态 / [riscv][loongarch] skip 占位)
        // eneros-microkernel (v0.148.0 形式化规范框架 Isabelle/HOL — Phase 3 P3-F 首版本 — formal/ 目录 + Isabelle/Kernel.thy 抽象规范 + Isabelle/Refinement.thy 精化映射 + ROOT session + Makefile 证明检查入口 + 三层映射架构 / Kernel.thy 含 inv_unique_running 调度不变式 + inductive step 状态转换 + theorem step_preserves_unique_running 安全定理 + theorem liv_no_deadlock sorry 占位 + definition inv_isolation/liv_progress §3 接口占位 / Refinement.thy 含 definition refines + theorem refinement_holds sorry 占位 / ADR-0001 裁剪 4 证明→2 证明 IPC/能力复用 seL4 / Isabelle 工具链未安装降级口径 + host 集成测试 formal_framework_test F1~F6 框架结构验证 / 无 Rust 内核代码改动 + no_std 自动满足)
        // eneros-microkernel v0.149.0 Scheduler.thy 调度器规范抽象（sched_step/inv_priority/liv_fair/step_preserves_priority sorry — ADR-0001 保留项），formal/ 目录无 Rust 代码 host 可测骨架。
        // eneros-microkernel v0.150.0 Scheduler_Proof.thy 调度器形式化证明 ★（scheduler_safety 完整归纳证明 rtranclp.induct + sched_step.induct 无 sorry + scheduler_liveness sorry 占位需公平性假设 + ready_schedulable 辅助引理 D8 风险标注 — ADR-0001 保留项 #1 调度器证明），formal/ 目录无 Rust 代码 host 可测骨架。
        // eneros-microkernel v0.151.0 Memory.thy 内存隔离规范抽象（inv_isolation 跨分区不相交不变式 + map_page 映射操作 + map_preserves_isolation sorry 占位延后 v0.152.0 + D2 vaddr 类型 + D3 add_region + D4 mem_state + D5 accessible comprehension + D6 hide_const — ADR-0001 保留项 #2 内存隔离证明），formal/ 目录无 Rust 代码 host 可测骨架。
        // eneros-microkernel v0.152.0 Memory_Proof.thy 内存隔离形式化证明 ★（isolation_safety rtranclp 完整归纳证明 rtranclp.induct 外层 + map_page.induct 内层双层归纳无 sorry ★ 里程碑 + D2a map_page\<^sup>*\<^sup>* 记法修正 + D2b case (map v s target) + D2c IH 直接引用 + D2d/D2e add_region_def — ADR-0001 保留项 #2 内存隔离证明 第二个形式化证明里程碑，D3 Memory.thy sorry 保留不动 Surgical Changes），formal/ 目录无 Rust 代码 host 可测骨架。
        // eneros-microkernel v0.153.0 Ipc.thy IPC 规范抽象（inv_integrity 完整性不变式 delivered ⊆ sent + inv_no_forge 不可伪造不变式 badge ∈ valid_senders + ipc_send/ipc_recv 操作 + D2 valid_senders 常量 + D3 recv badge 校验前提 — ADR-0001 IPC 证明降级为 seL4 引用），formal/ 目录无 Rust 代码 host 可测骨架。
        // eneros-microkernel v0.154.0 Ipc_Proof.thy IPC 形式化证明 ★（ipc_integrity_all rtranclp 完整归纳证明 rtranclp.induct 外层 + ipc_step.induct 内层双层归纳无 sorry ★ 第三个形式化证明里程碑 + ipc_no_forge 不可伪造证明 + D2 ipc_step 组合谓词 + D3 erule ipc_recv.cases），formal/ 目录无 Rust 代码 host 可测骨架。
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
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(GateError::IoError(
                "cargo-deny not found in PATH (fail-closed: audit requires cargo-deny)".to_string(),
            )),
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
        // eneros-agent-bus-dds (v0.78.0 Agent 消息签名层 — DdsNode / MockDdsNode / DdsConfig / DiscoveryPolicy / QosPolicy / DdsSample / DdsError / TopicSpec / TopicRegistry / TopicError / TopicCategory / PayloadType / validate_topic_name / standard_topics / MessageRouter / RoutingPolicy / RouteDecision / DropReason / RouteError / RouterStats / Subscription / SubId / CapabilityVerifier / MockCapabilityVerifier / Permission / AgentId / pattern_matches / CodecKind / CodecError / KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify),
        // eneros-tsn-time (v0.79.0 gPTP 时间同步层 — ClockIdentity / MacAddr / PtpTime / Port / PortRole / PortState / AnnounceMessage / BmcaResult / SyncMessage / FollowUpMessage / GptpConfig / GptpClock / compare_priority / run_bmca / handle_sync / handle_follow_up / adjust_clock / compute_offset / current_time / to_announce + v0.80.0 TSN 802.1Qbv 调度：TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config + v0.81.0 TSN 驱动胶合层 + 端到端时延探针：TsnError / TsnDriver / MockTsnDriver / driver_send_closure / DelayStats / LatencyProbe + v0.82.0 Grid Agent：GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors + v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor + v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer + v0.85.0 市场数据订阅：MarketType / Period / PricePoint / DrSignal / MarketFeed / MarketError / parse_price_point / parse_dr_signal / parse_feed / MarketFeedSource / MockMarketFeedSource / MarketFeedPublisher / MockMarketFeedPublisher / MarketFeedCache / MarketSubscriber + v0.86.0 报价生成：Bid / BidSide / BidStrategy / BidIntent / BidOptimization / BidError / BidIntentSource / BidOptimizer / BidPublisher / MockBidIntentSource / MockBidOptimizer / MockBidPublisher / BidGenerator / rule_intent / conservative_optimize + v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split + v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives + v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError + v0.90.0 孪生预测：Predictor / ForecastModel / ForecastResult / ForecastPoint / PersistenceModel / MeanModel / ForecastError + v0.91.0 What-if 分析：WhatIfAnalyzer / SimModel / AnalyticalSimModel / Scenario / ScenarioResult / Outcome / RiskLevel / Action / WhatIfError + v0.92.0 域内仲裁：DomainArbiter / ArbiterPolicy / ArbitrationRequest / ArbitrationResult / ArbitrationReason / Claim / Priority / detect_deadlock + v0.93.0 域级优化：DomainOptimizer / EdgeBoxState / DomainPlan / OptError + v0.94.0 VPP 聚合：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType + v0.95.0 云端策略下发：Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher / validate_strategy + v0.96.0 云端数据汇聚：DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink + v0.97.0 联邦发现：MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus + v0.98.0 跨域通信通道：TlsConfig / Endpoint / ChannelError / SecureTransport / MockSecureTransport / FederationChannel + v0.98.1 纵向加密认证：TunnelKeys / VerticalEncryptTunnel / EncryptError / DispatchToken / AuthResult / VerticalEncryptDevice / MockVerticalEncryptDevice / TunnelManager / hmac_sm3 / Sm3Hmac + v0.99.0 联邦共识协议：NodeId / ConsensusState / MsgType / PbftMessage / LogEntry / ConsensusResult / ConsensusError / ConsensusBus / MockConsensusBus / ConsensusEngine / f / quorum / primary_of / sign_message / verify_message + v0.100.0 资源争抢竞价：AgentId / Price / Qty / BidOrder / AskOrder / OrderBook / Match / MatchResult / AuctionError / AuctionEngine / match_book / match_digest + v0.101.0 断网处理与孤岛模式：PartitionState / PartitionDetector / IslandMode / EventCache / RecoverySync / SyncSink / MockSyncSink / SyncError / SyncReport + v0.102.0 MILP 求解器集成：UcUnit / UnitCommitment / UnitSchedule / DayAheadPlan / DayAheadScheduler + v0.103.0 Solver 神经热启动：CandidateSolution / InferEngine / MockEngine / OnnxEngine / HeuristicNet / SolveContext / WarmStartProvider / WarmStarter / WarmError + v0.104.0 多目标 Pareto 优化：MultiObjectiveProblem / Objective / OptDirection / VariableSpec / ParetoSolution / ParetoFront / ParetoSolver / Nsga2Solver / DecisionMaker + v0.105.0 IEC 61850 信息模型：LogicalDevice / LogicalNode / LnClass / DataObject / CommonDataClass / DataAttribute / FunctionalConstraint / DaValue / Quality / Validity / Source / Iec61850Model / SclParser / ModelError + v0.106.0 IEC 61850 MMS 协议栈 — BER 编解码 + ACSE 关联 + COTP + MMS Read/Write 客户端 + MmsTransport 抽象：MmsClient / MmsConnection / ConnState / MmsRequest / MmsResponse / VarAccessSpec / MmsReadResult / MmsWriteResult / MmsErrorCode / MmsError / MmsTransport / MockTransport / BerEncoder + v0.107.0 IEC 61850 GOOSE 快速事件传输 — 二层组播 EtherType 0x88B8 + st_num/sq_num 重传状态机 + L2Transport 抽象：GooseControlBlock / GooseDataset / GooseEntry / GoosePublisher / GooseSubscriber / GoosePdu / RxStatus / GooseError / L2Transport / MockL2 + v0.108.0 IEC 61850 SV 采样值 + IEC 62351 安全 — EtherType 0x88BA SV 订阅 + SM4-GCM/SM3-HMAC 加密封装 + 会话密钥管理：SvSubscriber / SvSample / SampleStatus / RingBuffer / SvError / SecureGoose / SecureSv / SecureFrame / SessionKey / KeyMgmt / SecError + v0.109.0 故障录波 COMTRADE — 环形采样缓冲 + 7 类故障触发 + C37.111 .cfg/.dat 生成 + FileSink 导出抽象：FaultRecorder / RecorderConfig / RecorderState / RecorderError / FileSink / MockSink / RingSampleBuffer / TriggerCondition / TriggerType / ComtradeWriter / ComtradeConfig / ComtradeFormat / ChannelConfig / Phase / SampleRecord + v0.110.0 云边数据同步 — 事件溯源存储（CRC32-IEEE）+ 增量批量同步（二进制帧 0xC537）+ 指数退避重试队列（有界死信）+ SyncTransport 抽象：EventStore / Event / EventType / crc32 / DeltaSync / CompressionType / SyncBatch / SyncStats / RetryQueue / SyncTransport / MockSyncTransport / SyncError + v0.111.0 模型 OTA 推送 — 断点续传 + SM2/SM3 验签 + 白名单热切换回滚：OtaClient / ModelInfo / ModelSignature / SigAlgorithm / HotLoader / ModelInstance / OtaTransport / MockOtaTransport / OtaError / OtaStats / OtaUpdateOutcome + v0.113.0 Secure Boot 全链验证 — 118B 镜像签名头（magic ESIG + version 1 全小端）+ SM3 哈希 + SM2 验签 + 防降级时间戳下限 + 四级信任链逐级推进（Rom→Bootloader→Kernel→Runtime→Complete）：BootStage / ImageSignature / ChainOfTrust / BootVerifier / BootError / BootStats / encode_header / decode_header / HEADER_LEN + v0.114.0 测量启动与远程证明 — SM3-only 24 PCR 单 bank + TCG extend 共享函数 + 事件日志 measure/replay + nonce 内嵌 PcrQuote（quote_digest 签名绑定防重放）+ SM2 AK 签名 Quote（64B 定长签名）+ AttestVerifier 四步验证流水线（NonceMismatch→SignatureInvalid→EventLogInconsistent→PcrMismatch）+ 故障注入 SoftTpm/MockAttestTransport：PcrBank / TpmBackend / SoftTpm / TcgEvent / TcgEventLog / PcrQuote / RemoteAttestation / AttestVerifier / AttestResult / AttestReason / TpmError / AttestError / AttestStats / AttestTransport / MockAttestTransport / pcr_extend_value / quote_digest / PCR_COUNT + v0.115.0 mTLS 双向认证通信安全 — SM 密码套件协商（服务端优先选首个交集）+ 证书管理（验签→有效期→CRL 固定顺序 + Copy CertError）+ 双向握手状态机（SM2 临时密钥交换签名证明私钥持有 + SM3-HMAC Finished + SM3 标签分离密钥派生）+ 记录层 SM4-GCM/CBC（序列号 nonce + AAD 绑序列号 + 64 位防重放窗口）+ MtlsTransport 同步传输抽象：SmCipherSuite / KeyExchange / Cipher / MacAlgorithm / negotiate / CertError / TlsError / TlsStats / MtlsTransport / MockMtlsTransport / CertManager / MtlsContext / HandshakeOutcome / MtlsRecord + v0.116.0 模型签名与白名单 — SM3 哈希登记白名单（含 added_at/expiry 有效期 + 严格/宽松模式 + BTreeMap 存储）+ SM2 模型签名验证（可信公钥列表 + 吊销列表 + 复用 sm2_verify 对原消息验签）+ 加载门禁 ModelGuard（白名单→有效期→签名三步检查 + AUDIT_CAPACITY 1000 环形审计日志 + GuardStats 统计）：WhitelistEntry / ModelWhitelist / ModelSig / ModelSignatureVerifier / ModelGuard / ModelLoadAttempt / GuardStats / GuardError / VerifyError / LoadResult / AUDIT_CAPACITY + v0.117.0 审计日志与哈希链 — SM3 哈希链（event_hash=SM3(事件编码) + combined_hash=SM3(prev‖event) 链式防篡改 + 序列号单调检查 + 断裂定位验证）+ 11 类审计事件（heapless 定容字符串）+ AuditStore 持久化 trait 与 MemStore 内存后端 + 重启 recover 链状态恢复：AuditEventType / AuditEvent / ChainEntry / HashChain / AuditLogger / AuditLoggerImpl / ChainVerifier / ChainVerifierImpl / AuditStore / MemStore / AuditError / VerifyReport + v0.118.0 行为审计与异常检测 — 行为基线建模（按分钟分桶速率序列 + compute_stats 均值/总体标准差/min/max/p95 + MIN_TRAIN_SAMPLES 100 样本门槛 + trained_at 证据时间）+ 滑动窗口异常检测（定容 64 条窗口满淘汰最旧 + 2σ/4σ 偏离分级 RateSpike/RateDrop + 非工作时段 OffHours + 越权访问 ScopeExpansion 最小规则）+ 定容告警队列（满显式 AlertQueueFull 不静默丢弃）：BehaviorDim / StatBaseline / BehaviorBaseline / AnomalyType / AlertLevel / Alert / BaselineTrainer / BaselineTrainerImpl / compute_stats / MIN_TRAIN_SAMPLES / DEFAULT_WINDOW_HOURS / AlertHandler / AlertQueue / ThresholdConfig / AnomalyDetector / StatAnomalyDetector / AnomalyError + v0.119.0 纵深安全联调与渗透测试 — 7 层安全编排（SecurityLayer/LayerStatus + check_all_layers 外部时间注入 + disabled 层 unhealthy）+ 安全事件分级响应（Critical→Isolate+Shutdown / High→Freeze+Alert / Medium→Throttle / Low|Info→Alert）+ 30 用例渗透套件真实调用各层组件 API 断言拒绝（L1 篡改镜像/回滚、L2 伪造 PCR/Quote 重放、L3 MITM/自签证书/CRL 吊销/弱套件、L4 未签名/篡改模型、L5 篡改/删除/注入日志、L6 高频/凌晨/越权、L7 共识伪造/身份伪造/DoS/分区）+ PentestReport 出口判定（30/30 Passed 且 0 Critical → overall_pass）：SecurityOrchestrator / SecurityOrchestratorImpl / SecurityIncident / ResponseAction / PentestSuite / PentestSuiteImpl / PentestCase / PentestSeverity / PentestStatus / PentestReport / PentestError + v0.120.0 Metrics 指标采集（P2-J 可观测性第 1 版）— 统一指标框架覆盖系统层（CPU/内存/温度）+ Agent 层（活跃数/延迟/错误率）+ 业务层（控制次数/优化调用数）：指标数据点模型（name≤64B + 标签≤8 个）+ 注册表（容量 64 拒绝重复注册）+ 采集器 record/increment/query + 256 条内存环形缓冲 + 定时批量刷入 + 按名分桶环形存储（写满丢弃最旧）：MetricType / MetricValue / Label / Metric / MetricDef / MetricRegistry / MetricsCollector / MetricsCollectorImpl / MetricsStore / MemoryMetricsStore / MetricsError + v0.121.0 结构化日志（P2-J 可观测性第 2 版）— crates/observability/logging：LogLevel / LogEntry / LogFilter / LogBuilder / StructuredLogger / StructuredLoggerImpl / LogStorage / MemoryLogStorage / LogError + v0.122.0 分布式追踪（P2-J 可观测性第 3 版·收官版）— crates/observability/tracing：TraceId / SpanId / SpanStatus / Span / SpanBuilder / TraceError / TraceContext / encode_trace_context / decode_trace_context / Tracer / TracerImpl / render_trace + v0.123.0 A/B 分区升级（P2-K OTA 与 Recovery 第 1 版）— crates/security/ota-ab：PartitionSlot / PartitionState / Partition / UpgradeMeta / OtaError / OtaManager / AbPartitionOta / boot_health_check / PartitionTable + v0.124.0 Agent 热更新（P2-K OTA 与 Recovery 第 2 版）— crates/agents/hot-update：HotUpdateState / AgentVersion / HotUpdateContext / HotUpdateError / AgentHotUpdater / AgentHotUpdaterImpl / ArcHotSwap + v0.125.0 Recovery Mode 最小化恢复内核（P2-K OTA 与 Recovery 第 3 版）— crates/security/recovery：RecoveryCommand / RecoveryState / DiagResult / FlashProgress / RecoveryError / parse_command / menu_text / RecoveryMode / RecoveryModeImpl / FlashWriter / FactoryReset / ConfigEntry / Diagnostician / HealthProbe / DefaultProbe / CONFIG_CAPACITY + v0.126.0 VPP 场景端到端验证（P2-K OTA 与 Recovery 第 4 版·Phase 2 收官版）— crates/agents/vpp-scenario：VppRole / EdgeNode / VppAggregation / VppError / VppScenario / VppScenarioImpl / DrEvent / DrState / DrHandler / ExitCriterion / ExitEvidence / Phase2ExitReport / ExitReportBuilder + v0.127.0 自研内核架构设计 + 接口规范（Phase 3 首版）— crates/kernel/microkernel：KernelError / BootInfo / KernelState / KernelVersion / KERNEL_VERSION / KernelSubsystem / SubsystemRegistry / SyscallHandler / SyscallNum / BootstrapHandle / CapabilityHandle / HalHandle / IpcHandle / MemoryHandle / SchedulerHandle / TimeHandle + v0.128.0 内核构建系统 + 兼容性设计（Phase 3 P3-A 第 2 版）— crates/kernel/microkernel：build.rs / linker.ld / _start / rust_main / KernelImageLayout / BuildConfig / OptLevel / Sel4CompatLayer / build_smoke + v0.129.0 Bootloader + 内核入口（Phase 3 P3-A 第 3 版★）— crates/kernel/bootloader：BootProtocol / BOOT_PROTOCOL_MAGIC / BOOT_PROTOCOL_VERSION / BootStage / BootError / SerialPort / Pl011Uart / RegIo / MmioRegIo / parse_dtb_ram / FdtError / boot_test.sh + v0.130.0 物理内存管理 buddy system（Phase 3 P3-B 首版★）— crates/kernel/microkernel mem：PhysFrameNum / ZoneType / PageFlags / PageMetadata / PhysMemManager / BuddyAllocator / Zone / PAGE_SIZE / MAX_ORDER / MAX_ZONES / MAX_FRAMES_PER_ZONE + v0.131.0 虚拟内存与页表（Phase 3 P3-B 第 2 版）— crates/kernel/microkernel mem：PageTableEntry / PageTable / map_page / unmap_page / translate / map_block / VirtAddr / PageTableLevel / MemAttrs / attrs_to_bits / AddressSpaceId / AddressSpace / VirtMemManager / Vmm / enable_mmu / tlbi_va + v0.132.0 分区内存隔离（Phase 3 P3-B 第 3 版·P3-B 收官★）— crates/kernel/microkernel mem：PartitionId / MemRegion / Partition / PartitionTable / PartitionMem / PartitionManager / RTOS_PARTITION_QUOTA / AGENT_PARTITION_QUOTA / DeviceId / DmaGuard / configure_dma / check_dma / check_access / create_partition / map_partition_mem / destroy_partition + v0.133.0 中断管理 GICv3（Phase 3 P3-C 首版）— crates/kernel/microkernel interrupt：IrqId / IrqTrigger / SavedRegs / IrqContext / IrqResult / IrqHandler / InterruptController / IRQ_SPURIOUS / IrqHandlerTable / IRQ_TABLE_SIZE / dispatch_irq / Gicv3 / GICD_CTLR / GICD_IGROUPR / GICD_ISENABLER / GICD_ICENABLER / GICD_ICPENDR / GICD_ICFGR / GICD_IROUTER / GICR_WAKER / GICR_SGI_BASE / PPI_BASE / SPI_BASE / register_irq_handler + v0.134.0 线程抽象与基本调度（Phase 3 P3-C 第 2 版）— crates/kernel/microkernel sched：Tid / Priority / ThreadState / SavedRegs / Tcb / Scheduler / SchedulerImpl / ReadyQueue / ReadyRing / context_switch / init_stack_frame / thread_trampoline / MAX_THREADS / TIME_QUANTUM_MS / PRIO_LEVELS / STACK_SIZE + v0.135.0 分区调度器 ARINC 653（Phase 3 P3-C 第 3 版）— crates/kernel/microkernel sched：TimeSlot / PartitionSchedule / PartitionState / MAX_SLOTS / default_schedule / ArincScheduler / PartitionScheduler / switch_address_space / last_address_space_switch) are standalone no_std crates
        // eneros-microkernel (v0.136.0 SMP 支持 多核启动/IPI/绑核 — Phase 3 P3-C 第 4 版 — crates/kernel/microkernel smp + sched：CpuId / CpuInfo / IpiMessage / IpiRing / PerCpu / SmpBootState / MAX_CPUS / IPI_RING_CAPACITY / boot_secondary / secondary_entry_addr / set_per_cpu / per_cpu / current_cpu / IPI_SGI_ID / send_ipi / broadcast_ipi / ipi_handler / drain_ipi_ring / IpiStats / SmpManager / SmpManagerImpl / pin_thread / schedule_on / Tcb.pinned_cpu)
        // eneros-microkernel (v0.137.0 多核内存一致性 自旋锁/屏障/原子封装 — Phase 3 P3-C 第 5 版 — crates/kernel/microkernel sync：SpinLock / SpinLockGuard / new / lock / try_lock / dmb / dsb / isb / dmb_ish / AtomicPtr / load / store / swap / compare_exchange / as_core / sync_smoke / SYNC_SGI_ID / sync_sgi_handler)
        // eneros-microkernel (v0.138.0 调度确定性与 WCET 验证 — Phase 3 P3-C 终版 — crates/kernel/microkernel sched/wcet：WcetMeasurement / WcetReport / WcetProfiler / WcetProfilerImpl / JitterStats / 抖动 / 上下文切换 / IRQ 延迟 / 10,000采样 / [wcet] ok)
        // eneros-microkernel (v0.139.0 同步消息传递 IPC — Phase 3 P3-D 首版 — crates/kernel/microkernel ipc：Endpoint / Message / IpcManager / Ipc trait / send / recv / call / reply / 消息寄存器 / ping-pong / [ipc] ok)
        // eneros-microkernel (v0.140.0 通知机制与异步信号 — Phase 3 P3-D 第 2 版 — crates/kernel/microkernel ipc：Notification / NotificationManager / NotificationOps / signal / poll / wait / bind_irq / WaiterRing / NotifyStats / 64 位信号 word / 中断绑定 / [notify] ok)
        // eneros-microkernel (v0.141.0 共享内存 IPC — Phase 3 P3-D 第 3 版 — crates/kernel/microkernel ipc：ShmId / ShmMapping / ShmCap / SharedMemory / ShmManager / ShmIpc / ShmStats / MAX_SHM=128 / MAX_SHM_SIZE=64MB / 连续帧游 / 能力控制映射 / grant_shm / map_partition_frame / sync_frames / dc cvau+ic ivau 缓存同步 / [shm] ok)
        // eneros-microkernel (v0.142.0 能力管理器 Capability-based — Phase 3 P3-D 终版 — crates/kernel/microkernel cap：Capability / CapId / CapRights / CapType / CapTarget / ConstraintPack / CapSpace / CapabilityManager / mint / check / revoke / derive / move / copy / create_space / destroy_space / 派生链 parent 反向链接 / 时间窗约束 / 过期令牌 / CapStats / MAX_CAPS_PER_PARTITION=4096 / [cap] ok)
        // eneros-microkernel (v0.143.0 HAL 接口与 ARM64 实现 — Phase 3 P3-E 首版 — crates/kernel/microkernel hal：Hal trait / HalRegistry / register / hal() / platform::qemu_virt / GenericTimer / cycles_to_us / clock_deadline / clock_disarm / Pl011<B: RegIo> 泛型封装 / Gpio 占位 / Aarch64Hal / from_parts / new_system / cpu_init / irq_init / enable_irq / disable_irq / irq≥1024 no-op / [hal] ok)
        // eneros-microkernel (v0.144.0 x86_64 BSP 接口占位 — Phase 3 P3-E 第 2 版 — crates/kernel/microkernel hal/mem：X86Hal / LocalApic / Uart8250 / X86PageTable / from_parts / new_system (x86_64 cfg-gated) / ADR-0001 降级口径接口占位 / [x86] ok)
        // eneros-microkernel (v0.145.0 飞腾 D2000 BSP — Phase 3 P3-E 第 3 版 ★ 信创核心交付 — crates/kernel/microkernel hal/boot：PhytiumHal 组合 Aarch64Hal + PhytiumSmmu + PhytiumCrypto / from_parts 注入式 / new(dtb) cfg-gated 真机路径 / mem_init 联动 smmu.init / PhytiumSmmu CR0 cfg-gated unsafe 写 + host Cell 状态记录 / PhytiumCrypto has_sm4/has_sm2 占位 true / boot::dtb_find_node DTB 节点查找 aarch64 cfg-gated FDT 解析 + host 占位 None / platform::phytium_d2000 八常量组 / 真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径 / [phytium] skip 占位)
        // eneros-microkernel (v0.146.0 鲲鹏 920 BSP — Phase 3 P3-E 第 4 版 ★ 信创核心 双国产平台 — crates/kernel/microkernel hal/boot：KunpengHal 组合 Aarch64Hal + KunpengSmmu + KunpengNuma / from_parts 注入式 / new(dtb) cfg-gated 真机路径 / mem_init 联动 smmu.init / KunpengSmmu SMMU v3 cfg-gated unsafe 写 + host Cell 状态记录 / KunpengNuma NUMA 拓扑探测 cfg-gated aarch64 读取 + host 占位 0 / boot::dtb_count_nodes DTB 节点统计 aarch64 cfg-gated FDT 遍历 + host 占位 0 / platform::kunpeng_920 常量组 / 真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径 / [kunpeng] skip 占位)
        // eneros-microkernel (v0.147.0 RISC-V + 龙芯 BSP 接口占位 — Phase 3 P3-E 终版本 — crates/kernel/microkernel hal：RiscvHal standalone + Plic + Clint + UartSifive + LoongArchHal standalone + LoongsonPch 复用 x86_64::Uart8250 / from_parts 注入式 / 无 cfg 门控真机路径 + 无 asm! 宏 + 无 unsafe write_volatile / Plic/Clint/LoongsonPch Cell 状态记录 + 观测口 / UartSifive 复用 eneros_bootloader::RegIo / platform::riscv_virt + platform::loongarch_qemu 常量组 / ADR-0001 BSP 收敛降级口径接口占位 + host 可测骨架 + PPI7 跨架构六平台多态 / [riscv][loongarch] skip 占位)
        // eneros-microkernel (v0.148.0 形式化规范框架 Isabelle/HOL — Phase 3 P3-F 首版本 — formal/ 目录 + Isabelle/Kernel.thy 抽象规范 + Isabelle/Refinement.thy 精化映射 + ROOT session + Makefile 证明检查入口 + 三层映射架构 / Kernel.thy 含 inv_unique_running 调度不变式 + inductive step 状态转换 + theorem step_preserves_unique_running 安全定理 + theorem liv_no_deadlock sorry 占位 + definition inv_isolation/liv_progress §3 接口占位 / Refinement.thy 含 definition refines + theorem refinement_holds sorry 占位 / ADR-0001 裁剪 4 证明→2 证明 IPC/能力复用 seL4 / Isabelle 工具链未安装降级口径 + host 集成测试 formal_framework_test F1~F6 框架结构验证 / 无 Rust 内核代码改动 + no_std 自动满足)
        // eneros-microkernel v0.149.0 scheduler_spec_test（F1~F5 Scheduler.thy 关键字 + ROOT session + ADR-0001 范围 + 三层映射 + Kernel.thy 未改动核验），formal_framework_test v0.148.0 零回归。
        // eneros-microkernel v0.150.0 scheduler_proof_test（F1~F5 Scheduler_Proof.thy 关键字 + ROOT session + ADR-0001 范围 + 三层证明链 + Scheduler.thy 未改动核验），scheduler_spec_test v0.149.0 + formal_framework_test v0.148.0 零回归。
        // eneros-microkernel v0.151.0 memory_spec_test（F1~F5 Memory.thy 关键字 + ROOT session + ADR-0001 范围 + 证明链 + Scheduler_Proof.thy 未改动核验），scheduler_proof_test v0.150.0 + scheduler_spec_test v0.149.0 + formal_framework_test v0.148.0 零回归。
        // eneros-microkernel v0.152.0 memory_proof_test（F1~F5 Memory_Proof.thy 关键字 + ROOT session + ADR-0001 范围 + 三层证明链 + Memory.thy 未改动核验 + ★ isolation_safety 无 sorry 验收），memory_spec_test v0.151.0 + scheduler_proof_test v0.150.0 + scheduler_spec_test v0.149.0 + formal_framework_test v0.148.0 零回归。
        // eneros-microkernel v0.153.0/v0.154.0 ipc_proof_test（F1~F5 Ipc_Proof.thy 关键字 + ROOT session + ADR-0001 范围 + 三层证明链 Ipc_Proof→Ipc→Kernel→Main + Ipc.thy 未改动核验 + ★ ipc_integrity_all 无 sorry 验收），memory_proof_test v0.152.0 + 前置测试零回归。
        // eneros-microkernel v0.155.0 capability_proof_test（F1~F5 Capability_Proof.thy 关键字 + ROOT session + ADR-0001 范围 + 证明链 Capability_Proof→Capability→Main + Ipc_Proof.thy 未改动核验 + ★ rights_monotone/revoke_cascade/no_bypass 无 sorry 验收），ipc_proof_test v0.154.0 + 前置测试零回归。
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

        // audit (special: fail-closed — cargo-deny 缺失视为失败而非降级 PASS)
        let start = Instant::now();
        let audit_cr = if !Self::cargo_deny_available() {
            CheckResult {
                name: "audit",
                passed: false,
                duration_ms: start.elapsed().as_millis() as u64,
                message: Some(
                    "cargo-deny not found, audit FAILED (fail-closed: toolchain is mandatory)"
                        .to_string(),
                ),
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

        // version-gate-consistency (蓝图/20 R1-1)
        let start = Instant::now();
        let mut version_cr = CheckResult::from(self.run_version_gate());
        version_cr.name = "version-gate";
        version_cr.duration_ms = start.elapsed().as_millis() as u64;

        // mock-gate (蓝图/20 R2-1)
        let start = Instant::now();
        let mut mock_cr = CheckResult::from(self.run_mock_gate());
        mock_cr.name = "mock-gate";
        mock_cr.duration_ms = start.elapsed().as_millis() as u64;

        // sign-key-gate (蓝图 66 WP-66.1.1)
        let start = Instant::now();
        let mut signkey_cr = CheckResult::from(self.run_sign_key_gate());
        signkey_cr.name = "sign-key-gate";
        signkey_cr.duration_ms = start.elapsed().as_millis() as u64;

        let results = [
            fmt_cr, clippy_cr, audit_cr, test_cr, version_cr, mock_cr, signkey_cr,
        ];
        let overall_pass = results.iter().all(|r| r.passed);
        GateReport {
            results,
            overall_pass,
        }
    }

    fn run_version_gate(&self) -> Result<(), GateError> {
        Self::version_gate_at(Path::new("."))
    }

    fn run_mock_gate(&self) -> Result<(), GateError> {
        Self::mock_gate_at(Path::new("."))
    }

    fn run_sign_key_gate(&self) -> Result<(), GateError> {
        Self::sign_key_gate_at(Path::new("."))
    }
}

impl DefaultGate {
    /// Go/No-Go 文档与 workspace 版本一致性检查（44-00 §3，取代 蓝图/20 R1-1）：
    /// 未过 GO 终验不得挂 rc/1.0 标记——预发布后缀一律非法；1.x+ 只有全 PASS
    /// 才允许；冲刺期 0.x.y 合法。只读文件真实内容，禁止 grep 注释/手工绕过。
    fn version_gate_at(root: &Path) -> Result<(), GateError> {
        let gng_path = root.join("docs/v1.0.0-go-no-go.md");
        let gng_text = fs::read_to_string(&gng_path).map_err(|e| {
            GateError::VersionGateInconsistent(format!("cannot read {}: {e}", gng_path.display()))
        })?;
        let mut any_non_pass = false;
        let mut parsed = 0usize;
        for line in gng_text.lines() {
            let t = line.trim();
            if !t.starts_with("| G") {
                continue;
            }
            let cols: Vec<&str> = t.split('|').map(str::trim).collect();
            let gate = cols.get(1).copied().unwrap_or("");
            if gate.len() == 2 && gate.starts_with('G') {
                parsed += 1;
                // 表格行格式：| G1 | 闸门项 | 判据 | 状态 | 证据 | → 状态在 cols[4]
                match cols.get(4) {
                    Some(status) if status.contains("PASS") => {}
                    // 状态列缺失、含 BLOCKED 或其他非 PASS 措辞 → 非 PASS
                    _ => any_non_pass = true,
                }
            }
        }
        if parsed == 0 {
            return Err(GateError::VersionGateInconsistent(
                "go-no-go 文档未解析到任何 G 项状态行（表格结构异常）".to_string(),
            ));
        }
        let cargo_path = root.join("Cargo.toml");
        let cargo_text = fs::read_to_string(&cargo_path).map_err(|e| {
            GateError::VersionGateInconsistent(format!("cannot read {}: {e}", cargo_path.display()))
        })?;
        let mut version: Option<String> = None;
        let mut in_workspace_package = false;
        for line in cargo_text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_workspace_package = t == "[workspace.package]";
                continue;
            }
            if in_workspace_package {
                if let Some((key, value)) = t.split_once('=') {
                    if key.trim() == "version" {
                        version = Some(value.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        let version = version.ok_or_else(|| {
            GateError::VersionGateInconsistent(
                "Cargo.toml [workspace.package] version 字段缺失".to_string(),
            )
        })?;
        if version.contains('-') {
            return Err(GateError::VersionGateInconsistent(format!(
                "workspace 版本 '{version}' 带预发布标记：未过 GO 终验不得挂 rc/1.0 标记（44-00 §3）；终验通过后直接 1.0.0"
            )));
        }
        let major: u64 = version
            .split('.')
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| {
                GateError::VersionGateInconsistent(format!(
                    "workspace 版本 '{version}' 非 x.y.z 语义化格式（44-00 §3 fail-closed）"
                ))
            })?;
        if any_non_pass && major >= 1 {
            return Err(GateError::VersionGateInconsistent(format!(
                "Go/No-Go 存在非 PASS 项，workspace 版本 '{version}' 为 1.x+；冲刺期必须 0.x.y，只有全 PASS 才允许 1.0+（44-00 §3）"
            )));
        }
        Ok(())
    }
}

impl DefaultGate {
    /// R2-1 Mock 收编门禁：扫描 crates/agents 与 crates/ai 下所有
    /// `pub struct Mock` 定义，检查定义行上方 3 行内有
    /// `#[cfg(any(test, feature = "mock"))]` 保护（含 cfg(test) 模块内的）。
    fn mock_gate_at(root: &Path) -> Result<(), GateError> {
        let mut violations: Vec<String> = Vec::new();
        for dir in ["crates/agents", "crates/ai"] {
            let base = root.join(dir);
            let mut entries: Vec<_> = std::fs::read_dir(&base)
                .map_err(|e| GateError::IoError(format!("cannot read {}: {e}", base.display())))?
                .collect::<Result<_, _>>()
                .map_err(|e| GateError::IoError(e.to_string()))?;
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let src = path.join("src");
                let files = walk_rs(&src);
                for f in files {
                    let text = std::fs::read_to_string(&f).map_err(|e| {
                        GateError::IoError(format!("cannot read {}: {e}", f.display()))
                    })?;
                    let lines: Vec<&str> = text.lines().collect();
                    for (i, line) in lines.iter().enumerate() {
                        if line.trim_start().starts_with("pub struct Mock") {
                            // 蓝图/20 R2-1 收紧：必须 `#[cfg(any(test, feature = "mock"` 前缀
                            // （防任意 #[cfg] 绕过；多 feature 追加变体亦通过）。
                            let window_start = i.saturating_sub(3);
                            let window = &lines[window_start..i];
                            let protected = window.iter().any(|w| {
                                w.trim_start()
                                    .starts_with("#[cfg(any(test, feature = \"mock\")")
                            });
                            if !protected {
                                violations.push(format!(
                                    "{}:{}: pub struct Mock 无精确 mock-cfg 保护",
                                    f.display(),
                                    i + 1
                                ));
                            }
                        }
                    }
                }
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(GateError::MockGateViolation(violations.join("; ")))
        }
    }

    /// 签名钥显式来源纪律（蓝图 66 WP-66.1.1，体检 C4）：`resolve_signing_key`
    /// fail-closed 后，tools/ 与 .github/workflows/ 内所有 `xtask config
    /// compile` 调用块（含 `\` 续行）必须携带 `--sign-key-hex` 或登记过的
    /// `--accept-test-signing-key`。静态扫描判据，不做运行时拦截
    /// （[02] WP-66.1.1 判据 4）。
    fn sign_key_gate_at(root: &Path) -> Result<(), GateError> {
        let mut violations: Vec<String> = Vec::new();
        for dir in ["tools", ".github/workflows"] {
            let base = root.join(dir);
            let files = walk_files(&base, &["sh", "yml", "yaml"]);
            for f in files {
                let text = fs::read_to_string(&f)
                    .map_err(|e| GateError::IoError(format!("cannot read {}: {e}", f.display())))?;
                for v in sign_key_violations_in(&text) {
                    violations.push(format!("{}: {v}", f.display()));
                }
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(GateError::SignKeyDisciplineViolation(violations.join("; ")))
        }
    }
}

/// 对单个文件文本扫描 `xtask config compile` 调用块；返回违规描述
/// （不含文件名前缀，便于单测）。注释行（`#` 前缀）不计。
fn sign_key_violations_in(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut violations = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        let is_invocation = !trimmed.starts_with('#') && trimmed.contains("xtask config compile");
        if !is_invocation {
            i += 1;
            continue;
        }
        // 调用块 = 命中行 + 后续 `\` 续行。
        let mut block = String::from(lines[i]);
        let mut j = i;
        while lines[j].trim_end().ends_with('\\') && j + 1 < lines.len() {
            j += 1;
            block.push(' ');
            block.push_str(lines[j]);
        }
        if !block.contains("--sign-key-hex") && !block.contains("--accept-test-signing-key") {
            violations.push(format!(
                "{line}: config compile 调用块缺显钥来源（--sign-key-hex | \
                 --accept-test-signing-key）",
                line = i + 1
            ));
        }
        i = j + 1;
    }
    violations
}

fn walk_files(dir: &std::path::Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk_files(&p, exts));
            } else if p
                .extension()
                .map(|x| exts.iter().any(|e| x == *e))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out
}

fn walk_rs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    walk_files(dir, &["rs"])
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
        let results = [
            pass("fmt"),
            pass("clippy"),
            pass("audit"),
            pass("test"),
            pass("version-gate"),
            pass("mock-gate"),
            pass("sign-key-gate"),
        ];
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
        let results = [
            pass("fmt"),
            fail("clippy"),
            pass("audit"),
            pass("test"),
            pass("version-gate"),
            pass("mock-gate"),
            pass("sign-key-gate"),
        ];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(!overall_pass);

        // Any single failure flips the overall result.
        for i in 0..7 {
            let mut r = [
                pass("fmt"),
                pass("clippy"),
                pass("audit"),
                pass("test"),
                pass("version-gate"),
                pass("mock-gate"),
                pass("sign-key-gate"),
            ];
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
    fn test_audit_degraded_is_fail_closed() {
        // A degraded audit (cargo-deny missing) must now fail closed.
        let degraded = CheckResult {
            name: "audit",
            passed: false,
            duration_ms: 0,
            message: Some(
                "cargo-deny not found, audit FAILED (fail-closed: toolchain is mandatory)"
                    .to_string(),
            ),
        };
        assert!(!degraded.passed);
        assert!(degraded.message.as_ref().unwrap().contains("fail-closed"));

        // A fail-closed audit must flip the overall result.
        let results = [
            pass("fmt"),
            pass("clippy"),
            degraded,
            pass("test"),
            pass("version-gate"),
            pass("mock-gate"),
            pass("sign-key-gate"),
        ];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(!overall_pass);
    }

    #[test]
    fn sign_key_scanner_flags_bare_invocation() {
        let text = "\
echo start
cargo xtask config compile --profile qemu-prod --root-key-hex 5a5a
echo done
";
        let v = sign_key_violations_in(text);
        assert_eq!(v.len(), 1, "裸调用必须判违：{v:?}");
        assert!(v[0].contains("缺显钥来源"));
    }

    #[test]
    fn sign_key_scanner_accepts_explicit_key_sources() {
        let with_flag = "\
cargo xtask config compile --profile qemu-prod \\
  --accept-test-signing-key \\
  --root-key-hex 5a5a
";
        assert!(sign_key_violations_in(with_flag).is_empty());

        let with_hex = "cargo xtask config compile --sign-key-hex abcdef";
        assert!(sign_key_violations_in(with_hex).is_empty());
    }

    #[test]
    fn sign_key_scanner_skips_comments_and_multi_line_blocks() {
        let text = "\
# xtask config compile 说明性注释不算调用
gate_compile_config_helper() {
  cargo xtask config compile --profile \"$P\" \\
    --accept-test-signing-key --root-key-hex k
}
";
        assert!(sign_key_violations_in(text).is_empty());
    }

    #[test]
    fn test_version_gate_consistency_matrix() {
        use std::io::Write;
        use std::path::PathBuf;

        fn write_scenario(root: &Path, gng: &str, version: &str) {
            fs::create_dir_all(root.join("docs")).unwrap();
            let mut f = fs::File::create(root.join("docs/v1.0.0-go-no-go.md")).unwrap();
            f.write_all(gng.as_bytes()).unwrap();
            let mut f = fs::File::create(root.join("Cargo.toml")).unwrap();
            write!(f, "[workspace.package]\nversion = \"{}\"\n", version).unwrap();
        }

        fn scenario_root(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "eneros-ci-gate-test-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        // 与 docs/v1.0.0-go-no-go.md 相同的 5 列表格格式：| G1 | 闸门项 | 判据 | 状态 | 证据 |
        let blocked =
            "| G1 | 真机浸泡 | 72h | ⛔ **BLOCKED** | e |\n| G2 | 互操作 | 对端 | ✅ **PASS** | e |\n";
        let all_pass =
            "| G1 | 合规 | 可重算 | ✅ **PASS** | e |\n| G2 | 工程 | 可复现 | ✅ **PASS（CI 环境）** | e |\n";

        // 非全 PASS + 发布版 1.0.0 → 必须红（一票否决项 1 的核心场景）
        let root = scenario_root("blocked-100");
        write_scenario(&root, blocked, "1.0.0");
        assert!(DefaultGate::version_gate_at(&root).is_err());

        // 非全 PASS + 1.5.0 → 红（1.x+ 一律要求全 PASS）
        let root = scenario_root("blocked-150");
        write_scenario(&root, blocked, "1.5.0");
        assert!(DefaultGate::version_gate_at(&root).is_err());

        // 非全 PASS + rc 版本 → 红（44-00 §3：未过终验不得挂 rc/1.0 标记）
        let root = scenario_root("blocked-rc");
        write_scenario(&root, blocked, "1.0.0-rc.0");
        assert!(DefaultGate::version_gate_at(&root).is_err());

        // 非全 PASS + 0.x → 绿（冲刺期合法版本线）
        let root = scenario_root("blocked-0x");
        write_scenario(&root, blocked, "0.313.0");
        assert!(DefaultGate::version_gate_at(&root).is_ok());

        // 全 PASS + 1.0.0 → 绿（放行条件）
        let root = scenario_root("pass-100");
        write_scenario(&root, all_pass, "1.0.0");
        assert!(DefaultGate::version_gate_at(&root).is_ok());

        // 全 PASS + rc → 红（rc 标记一律非法，终验后直接 1.0.0）
        let root = scenario_root("pass-rc");
        write_scenario(&root, all_pass, "1.0.0-rc.1");
        assert!(DefaultGate::version_gate_at(&root).is_err());

        // 全 PASS + 0.x → 绿（未升 1.0 前仍处 0.x 线）
        let root = scenario_root("pass-0x");
        write_scenario(&root, all_pass, "0.313.0");
        assert!(DefaultGate::version_gate_at(&root).is_ok());

        // 文档缺失 → fail-closed
        let root = scenario_root("missing-doc");
        fs::create_dir_all(&root).unwrap();
        assert!(DefaultGate::version_gate_at(&root).is_err());
    }
}
