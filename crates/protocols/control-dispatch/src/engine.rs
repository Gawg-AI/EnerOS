//! 蓝图 40 WP-40.6/40.7/40.12：`DispatchEngine`——唯一 ControlDispatch 实现。
//!
//! 12 步链（WP-40.6 顺序）：
//!
//! 1. proposal schema/units/ranges/idempotency 校验；
//! 2. telemetry 执行时刻新鲜度（`valid_until_ms`）；
//! 3. actor/capability/lease/fencing 验证（`ControlChain` gateway +
//!    lease + policy——唯一 AuthorizedCommand 授权路径，WP-40.7）；
//! 4. safety catalog/constraint validator（chain 的 CTL-010 反馈调和 +
//!    CTL-008 独立安全校验，反馈来自受信任遥测）；
//! 5. rate/energy 预算 reserve（[`DecisionGate`]——**不 record**）；
//! 6. durable intent 写入（[`IntentStore`]）；
//! 7. DeliveryChannel EIPC 投递（`ControlEnvelope`，WP-40.8）；
//! 8. device-agent AuthorizedWriter 二次验证（对端职责）；
//! 9. 协议/设备 transport ACK（对端回执帧）；
//! 10. 独立 readback（对端；读回不符 → `Failed`，不可得 → `Indeterminate`）；
//! 11. commit 预算（`gate.record`——**仅 confirmed dispatch**）+ 审计；
//! 12. 回执反馈给 Agent（learning 只消费 `Committed`）。
//!
//! 任何前置失败零投递（`Err`）；投递后超时/结果不明 → `Indeterminate`
//! 对账，不盲重试非幂等命令（WP-40.12）。
//!
//! 61 v0.314.24：执行状态与幂等真值由 [`crate::journal::ExecutionJournal`]
//! 唯一持有（`Authorized→Persisted→Sent→TransportAcked→AppliedConfirmed`
//! 严格写序，expected-previous 强制）；`Committed` 只能从
//! `AppliedConfirmed`（独立回读匹配）映射；同 key 不同内容摘要 =
//! `IdempotencyConflict` 拒绝。gateway 只登记 `Authorized`/`DryRun`/
//! `Rejected`（纯裁决面，投递前无执行事实）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_gateway::chain::{ChainOutcome, ControlChain, FeedbackBundle};
use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
use eneros_control_gateway::idempotency::OpOutcome;
use eneros_control_gateway::wire::{ControlEnvelope, OutcomeCode, OutcomeFrame};
use eneros_control_gateway::{ChainDenyReason, ControlProfile};
use eneros_dual_brain::audit::{DecisionAudit, ExecOutcome, SafetyOutcome};
use eneros_dual_brain::chain_sink::deny_reason_str;
use eneros_dual_brain::gate::{DecisionGate, GateDecision};
use eneros_dual_brain::ProposedControlPlan;
use eneros_protocol_abstract::write::PointWriteIntent;
use eneros_safety_validator::state::SystemState;
use eneros_safety_validator::TrustedTelemetry;
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
use eneros_upa_model::PointValue;

use crate::channel::DeliveryChannel;
use crate::dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchError, DispatchReceipt, DispatchState,
};
use crate::feedback::TrustedFeedbackBuilder;
use crate::intent::{IntentRecord, IntentStore};
use crate::journal::{ExecutionJournal, ExecutionStatus, IdempotencyRecord, JournalHit};

/// 47（WP-47.1）：协议写意图 TTL（秒；信封过期窗，无意图自带 TTL）。
pub const INTENT_TTL_SECS: u64 = 60;

/// WP-54.2 合核：两入口的身份派生差异（枚举而非闭包——派生必须发生在
/// actor 门之后，见 [`DispatchEngine::execute_core`]）。
enum CoreIdentity {
    /// 协议写意图：command_id = correlation = (protocol, seq) 派生；
    /// 不占引擎自增序列（跨重启由链幂等键兜底）。
    Intent {
        /// 来源协议 id。
        protocol_id: String,
        /// 协议侧单调序号。
        sequence: u64,
    },
    /// AI 计划：command_id = 引擎自增 seq 前 8B LE；
    /// correlation = plan_id 派生（WP-40.12 幂等键同源）。
    Plan {
        /// 计划 ID。
        plan_id: String,
    },
}

/// WP-54.2 合核入参：入口完成纯计算（校验/值域/target/digest），副作用序
/// 由 [`DispatchEngine::execute_core`] 独占。
struct CoreCommand {
    /// 身份派生方式。
    identity: CoreIdentity,
    /// 信封 TTL（秒）：意图=INTENT_TTL_SECS；计划=proposal.ttl_secs.max(1)。
    ttl_secs: u64,
    /// 功率载荷（kW，正=放电/合）。
    power_kw: f64,
    /// durable 记录目标设备串。
    target_device: String,
    /// durable 记录输入摘要（意图写恒 None）。
    input_digest: Option<[u8; 32]>,
    /// None=意图写（无能量语义，仅速率）；Some(kWh)=AI 计划能量预算。
    energy_kwh: Option<f64>,
    /// IdempotentHit 回执文案（两入口注记不同）。
    idem_note: String,
}

/// 调度窗口（小时）——能量预算估算（与 dual-brain 同系数）。
pub const SCHEDULE_WINDOW_HOURS: f64 = eneros_dual_brain::decision::SCHEDULE_WINDOW_HOURS;

/// 单次调度能量估算（首时段功率 × 窗口；空调度 = 0）。
pub fn schedule_energy_kwh(s: &eneros_energy_lp_model::result::ScheduleResult) -> f64 {
    s.schedule
        .first()
        .map(|e| e.net_power_kw.abs() * SCHEDULE_WINDOW_HOURS)
        .unwrap_or(0.0)
}

/// 装配配置。
pub struct DispatchConfig {
    /// 授权链（gateway/lease/idempotency/policy/safety/SBO——default-deny）。
    pub chain: ControlChain,
    /// 信封签名密钥（与 chain gateway 同源）。
    pub key: Vec<u8>,
    /// 同源档案（蓝图 55）：租约 ID/策略代次/绑定设备与点——签发侧信封
    /// 字段与链装配共用一份事实，禁再散抄。
    pub profile: ControlProfile,
    /// 当前 fencing token。
    pub fencing_token: u64,
    /// 签发服务标识。
    pub issuer_service: [u8; 16],
    /// 签发服务 epoch（重启计数）。
    pub issuer_epoch: u64,
    /// 投递目标端点（EIPC 服务名）。
    pub target_endpoint: String,
    /// 功率上限（kW）——proposal 校验。
    pub max_power_kw: f64,
    /// SPEC WP-A.2：电压保护窗（V；缺省 = 蓝图默认 (340,420)）。
    pub safety_voltage_window: (f64, f64),
    /// SPEC WP-A.2：频率保护窗（Hz；缺省 = (49.5,50.5)）。
    pub safety_freq_window: (f64, f64),
    /// SPEC WP-A.2：保护配合爬坡率上限（kW/min；缺省 = 校验器默认 200）。
    pub safety_ramp_kw_per_min: f64,
    /// 速率/能量预算闸（check 在 submit；record 仅在 confirmed dispatch）。
    pub gate: DecisionGate,
    /// 决策审计（可选）。
    pub audit: Option<DecisionAudit>,
    /// 47（WP-47.4）：SHA-256 审计链（security/audit D4 线）——每个
    /// Committed/Failed 控制事件入链。存储选型归装配方：SPEC-E WP-E.2 起
    /// svc-control-dispatch 以 `DurableAuditLog` 真落盘（audit 区段 +
    /// boot verify_log 校验，篡改 ⇒ AUDIT-VERIFY-FAIL 停止追加）；测试
    /// 装配可用 `MemStore`（易失，仅供单测）。失败计入 audit_drop_count。
    pub control_audit: Option<Box<dyn eneros_audit::AuditLogger>>,
    /// durable intent 存储（WP-40.12 对账）。
    pub intent_log: Box<dyn IntentStore>,
    /// 投递通道（生产：Sel4EipcChannel；测试：内存通道）。
    pub delivery: Box<dyn DeliveryChannel>,
    /// 反馈点映射（受信任遥测 → FeedbackBundle）。
    pub feedback_mapping: crate::feedback::FeedbackPointMapping,
    /// 单调时钟（毫秒）。
    pub now_fn: fn() -> u64,
}

/// SPEC WP-A.2：护栏参数束（工程单位见各字段；测试域可自由构造）。
#[derive(Debug, Clone, Copy)]
struct SafetyGuardParams {
    max_power_kw: f64,
    max_current_a: f64,
    voltage_window: (f64, f64),
    freq_window: (f64, f64),
    ramp_kw_per_min: f64,
    protection_overcurrent_a: f64,
    protection_overvoltage_v: f64,
    protection_undervoltage_v: f64,
}

/// 唯一 ControlDispatch 实现（12 步链）。
pub struct DispatchEngine {
    chain: ControlChain,
    key: Vec<u8>,
    /// 同源档案（蓝图 55）：lease_id/policy_version/device_id/point_id 的
    /// 单一来源。
    profile: ControlProfile,
    fencing_token: u64,
    issuer_service: [u8; 16],
    issuer_epoch: u64,
    target_endpoint: String,
    max_power_kw: f64,
    /// SPEC WP-A.2：电气护栏参数（窗口/爬坡，见 DispatchConfig 同名字段）。
    safety_voltage_window: (f64, f64),
    safety_freq_window: (f64, f64),
    safety_ramp_kw_per_min: f64,
    /// 速率/能量预算闸（观测：used_kwh/rejected 计数）。
    pub gate: DecisionGate,
    /// 决策审计（观测：链校验/条目）。
    pub audit: Option<DecisionAudit>,
    /// 47：SHA-256 控制事件审计链。
    control_audit: Option<Box<dyn eneros_audit::AuditLogger>>,
    /// durable intent 存储（观测：find/pending 对账）。
    pub intent_log: Box<dyn IntentStore>,
    delivery: Box<dyn DeliveryChannel>,
    /// 61 v0.314.24：执行 journal（执行状态与幂等真值唯一持有者）。
    /// 启动时从 intent store 未决记录播种 Reconciling——旧版/崩溃遗留的
    /// 非终态记录永不以 Committed 复活。
    journal: ExecutionJournal,
    feedback_builder: TrustedFeedbackBuilder,
    now_fn: fn() -> u64,
    seq: u64,
    /// 已提交（Committed）次数（观测/审计）。
    pub committed_count: u64,
    /// 审计丢弃计数。
    pub audit_drop_count: u64,
    /// 47（Exit 补证）：成功入链条数（append 计数——QEMU 串口
    /// AUDIT-CHAIN 行依据；零计数不打印，杜绝恒 ok 式假证据）。
    pub audit_append_count: u64,
    /// 47（WP-47.1）：每来源协议已见最大意图序号（重放门——重复
    /// sequence 零写；跨重启由链幂等（意图 correlation 派生）兜底）。
    last_seq: BTreeMap<String, u64>,
}

impl DispatchEngine {
    /// SPEC WP-A.2：护栏预留阈（与 safety-validator 蓝图默认一致；
    /// D12 域保留面——接入真机保护配合策略后转装配配置）。
    const GUARD_MAX_CURRENT_A: f64 = 200.0;
    const GUARD_PROT_OVERCURRENT_A: f64 = 220.0;
    const GUARD_PROT_OVERVOLTAGE_V: f64 = 440.0;
    const GUARD_PROT_UNDERVOLTAGE_V: f64 = 320.0;

    /// 构造。
    pub fn new(config: DispatchConfig) -> Self {
        let pending = config.intent_log.pending();
        let mut engine = Self {
            chain: config.chain,
            key: config.key,
            profile: config.profile,
            fencing_token: config.fencing_token,
            issuer_service: config.issuer_service,
            issuer_epoch: config.issuer_epoch,
            target_endpoint: config.target_endpoint,
            max_power_kw: config.max_power_kw,
            safety_voltage_window: config.safety_voltage_window,
            safety_freq_window: config.safety_freq_window,
            safety_ramp_kw_per_min: config.safety_ramp_kw_per_min,
            gate: config.gate,
            audit: config.audit,
            control_audit: config.control_audit,
            intent_log: config.intent_log,
            delivery: config.delivery,
            // 与 durable 槽位同阶的有界执行 journal（61 v0.314.24）。
            journal: ExecutionJournal::new(256),
            feedback_builder: TrustedFeedbackBuilder::new(config.feedback_mapping),
            now_fn: config.now_fn,
            seq: 0,
            committed_count: 0,
            audit_drop_count: 0,
            audit_append_count: 0,
            last_seq: BTreeMap::new(),
        };
        // 启动恢复播种：durable 未决记录（v1 遗留 Applied/Persisted 或
        // 崩溃窗口遗留）→ Reconciling。投递与否不可证明——重放零投递、
        // 永不直接 Committed（蓝图 05 §2.3 兼容规则）。
        let now0 = (engine.now_fn)();
        let mut recovered = 0usize;
        for rec in pending {
            if engine.journal.seed_reconciling(
                rec.correlation_id,
                rec.command_id,
                rec.command_digest.unwrap_or([0u8; 32]),
                now0,
            ) {
                recovered += 1;
            }
        }
        // 61 v0.314.24：启动恢复可见性——遗留/崩溃遗留未决记录一律进对账，
        // 永不以 Committed 复活。
        #[cfg(target_os = "none")]
        if recovered > 0 {
            eneros_runtime::println!(
                "[svc-control-dispatch] EXEC-JOURNAL recovered pending={recovered} → reconciling"
            );
        }
        let _ = recovered;
        engine
    }

    /// 更新 fencing token（租约续期后）。
    pub fn update_fencing_token(&mut self, token: u64) {
        self.fencing_token = token;
    }

    fn audit(&mut self, exec: ExecOutcome, safety: SafetyOutcome, detail: &str, now_ms: u64) {
        // 47（WP-47.4）：每个控制事件（含 Committed/Failed）入 SHA-256
        // 审计链；失败 → audit_drop_count（不阻断控制面）。
        if let Some(chain) = self.control_audit.as_mut() {
            let event_type = match exec {
                ExecOutcome::Succeeded => eneros_audit::AuditEventType::WriteExecuted,
                _ => eneros_audit::AuditEventType::WriteDenied,
            };
            let severity = if matches!(safety, SafetyOutcome::Blocked) {
                2
            } else {
                1
            };
            let target = alloc::format!("control-dispatch/{}", self.target_endpoint);
            let mut ev =
                eneros_audit::AuditEvent::new(event_type, "control-dispatch", &target, detail);
            ev.severity = severity;
            if chain.log(ev).is_ok() {
                self.audit_append_count += 1;
            } else {
                self.audit_drop_count += 1;
            }
        }
        let Some(audit) = self.audit.as_mut() else {
            return;
        };
        let rec = eneros_dual_brain::audit::DecisionRecord {
            actor: "control-dispatch",
            target: &self.target_endpoint,
            intent: "dispatch",
            lp_solution: detail,
            safety,
            exec,
            now_ms,
        };
        if audit.log_decision(&rec).is_err() {
            self.audit_drop_count += 1;
        }
    }

    /// 从 proposal 派生 correlation（plan_id 前 16 字节，定长）。
    fn correlation_of(plan_id: &str) -> [u8; 16] {
        let mut id = [0u8; 16];
        let bytes = plan_id.as_bytes();
        let n = core::cmp::min(16, bytes.len());
        id[..n].copy_from_slice(&bytes[..n]);
        id
    }

    /// 步骤 1：proposal 校验（schema/units/ranges/idempotency）。
    fn validate_proposal(&self, p: &ProposedControlPlan) -> Result<(), DispatchError> {
        if p.power_kw.is_nan() || p.power_kw.is_infinite() {
            return Err(DispatchError::ProposalInvalid("power_kw 非有限".into()));
        }
        if p.power_kw.abs() > self.max_power_kw {
            return Err(DispatchError::ProposalInvalid(format!(
                "power_kw {} 超上限 {}",
                p.power_kw, self.max_power_kw
            )));
        }
        if p.ttl_secs == 0 {
            return Err(DispatchError::ProposalInvalid(
                "ttl_secs 为 0（防零有效期）".into(),
            ));
        }
        if p.schedule.schedule.is_empty() {
            return Err(DispatchError::ProposalInvalid("空调度不可投递".into()));
        }
        Ok(())
    }

    /// 47（WP-47.1 协议入站写）：协议写意图 12 态链（与 [`run`] 同链，
    /// 入口换为 [`PointWriteIntent`]）。重放门 + 链幂等双防重复 sequence。
    fn run_intent(
        &mut self,
        intent: &PointWriteIntent,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        let mut states: Vec<DispatchState> = Vec::with_capacity(12);
        states.push(DispatchState::Proposed);

        // 步骤 1：意图校验——目标设备/点必须等于装配绑定（WP-47.2 点表
        // 绑定；防协议层越点写）+ 值域有限。
        if intent.device_id as u32 != self.profile.device_id
            || intent.point_id != self.profile.point_id
        {
            return Err(DispatchError::NotAuthorized(format!(
                "intent target device={} point={} 与装配绑定 device={} point={} 不符",
                intent.device_id, intent.point_id, self.profile.device_id, self.profile.point_id
            )));
        }
        let power_kw = match intent.value {
            PointValue::Float(f) if f.is_finite() => f,
            PointValue::Int(i) => i as f64,
            PointValue::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            _ => {
                return Err(DispatchError::ProposalInvalid(format!(
                    "intent 值域不可写（protocol={} seq={}）",
                    intent.protocol_id, intent.sequence
                )))
            }
        };
        states.push(DispatchState::Validated);

        // 步骤 2：telemetry 执行时刻新鲜度（同 run——控制时域外零投递）。
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }

        // 步骤 2.5：序号重放门（每来源协议单调；重复 sequence 零写。
        // 跨重启（序号复位）由链 idempotency_key（意图 correlation 派生）
        // 兜底——同 (protocol, seq) 不重复写）。
        let proto_key = intent.protocol_id.clone();
        if let Some(&last) = self.last_seq.get(&proto_key) {
            if intent.sequence <= last {
                let note = alloc::format!(
                    "protocol-replay dup sequence protocol={} seq={} <= last={}（零写）",
                    intent.protocol_id,
                    intent.sequence,
                    last
                );
                self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                return Ok(DispatchReceipt {
                    command_id: [0u8; 16],
                    correlation_id: Self::intent_correlation(&intent.protocol_id, intent.sequence),
                    final_state: DispatchState::Rejected,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                });
            }
        }
        self.last_seq.insert(proto_key, intent.sequence);

        // 步骤 3 起进入合核（WP-54.2）：actor 门、Envelope 装配+签名+幂等键、
        // chain.execute 四臂、rate reserve、IntentRecord 持久化与投递收尾，
        // 与 [`Self::run`] 共享同一实现；差异点经 [`CoreCommand`] 参数化。
        self.execute_core(
            CoreCommand {
                identity: CoreIdentity::Intent {
                    protocol_id: intent.protocol_id.clone(),
                    sequence: intent.sequence,
                },
                ttl_secs: INTENT_TTL_SECS,
                power_kw,
                target_device: alloc::format!("{}:{}", intent.protocol_id, intent.device_id),
                input_digest: None,
                energy_kwh: None,
                idem_note: String::from("idempotent-hit（同 (protocol,seq) 重放——不重复写）"),
            },
            telemetry,
            actor,
            started,
            states,
            deadline_ms,
        )
    }

    /// 意图 correlation（16B 定长）= 协议 id 前 8B ‖ seq u64 LE。
    /// 幂等键与 command_id 同源——(protocol, seq) 唯一决定一条命令。
    fn intent_correlation(protocol_id: &str, sequence: u64) -> [u8; 16] {
        let mut id = [0u8; 16];
        let pb = protocol_id.as_bytes();
        let n = core::cmp::min(8, pb.len());
        id[..n].copy_from_slice(&pb[..n]);
        id[8..16].copy_from_slice(&sequence.to_le_bytes());
        id
    }

    /// 执行完整 12 步链。
    fn run(
        &mut self,
        proposal: &ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        let mut states: Vec<DispatchState> = Vec::with_capacity(12);
        states.push(DispatchState::Proposed);

        // 步骤 1：proposal 校验。
        self.validate_proposal(proposal)?;
        states.push(DispatchState::Validated);

        // 步骤 2：telemetry 执行时刻新鲜度。
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }

        // 步骤 3 起进入合核（WP-54.2，同 run_intent）。
        let energy = schedule_energy_kwh(&proposal.schedule);
        self.execute_core(
            CoreCommand {
                identity: CoreIdentity::Plan {
                    plan_id: proposal.plan_id.clone(),
                },
                ttl_secs: u64::from(proposal.ttl_secs.max(1)),
                power_kw: proposal.power_kw,
                target_device: proposal.target_device.clone(),
                input_digest: proposal.input_digest,
                energy_kwh: Some(energy),
                idem_note: String::from("idempotent-hit（原结果，不重复写）"),
            },
            telemetry,
            actor,
            started,
            states,
            deadline_ms,
        )
    }

    /// SPEC WP-A.2：投影——受信任遥测电气量退化检查（NaN/±Inf/零时间戳
    /// ⇒ `SystemState::unknown()` fail-closed，由规则链 NonFinite 前置拒绝；
    /// 不 panic、不猜健康默认值）。生产值本身来自 trusted 评估器，此处是
    /// 纵深防御的第二道形态闸。
    #[allow(clippy::needless_pass_by_value)]
    fn project_system_state(telemetry: &TrustedTelemetry) -> SystemState {
        let st = &telemetry.state;
        if st.timestamp_ms == 0
            || !(st.voltage_v.is_finite()
                && st.current_a.is_finite()
                && st.frequency_hz.is_finite()
                && st.soc_pct.is_finite())
        {
            return SystemState::unknown();
        }
        st.clone()
    }

    /// SPEC WP-A.2：AI 电气护栏核——ai/safety-validator 规则链
    /// （ElectricalSafetyRule + ProtectionCoordinationRule + 同时充放
    /// Fatal 后置校验）在决策咽喉的接线。窗口/功率帽/爬坡率全部由
    /// [`DispatchConfig`] 注入（缺省值 = 校验器蓝图默认，行为连续）。
    ///
    /// `power_series_kw`：按时段索引排列的计划功率序（+放电/-充电）。
    /// 生产咽喉当前传单点序列（意图写/计划净功率）；多时段 LP 方案的
    /// 全谱校验归双脑协调器前置链（既有 validate 面），本护栏拒绝型、
    /// 不做静默钳位放行。任何违规（含 clamp 建议）⇒ Err(摘要)。
    fn safety_guard(
        params: SafetyGuardParams,
        power_series_kw: &[f64],
        state: &SystemState,
    ) -> Result<(), String> {
        // 窗口裁决（WP-A.2 显式化）：safety-validator 的 voltage_range/
        // freq_range 为 D12 领域预留（规则内未消费），本护栏按装配配置
        // 直接裁决并给出结构化摘要；其余交规则链。
        for (name, v, win) in [
            ("voltage", state.voltage_v, params.voltage_window),
            ("frequency", state.frequency_hz, params.freq_window),
        ] {
            if !(win.0..=win.1).contains(&v) {
                return Err(format!(
                    "electrical_safety/{name}={v} outside ({},{})",
                    win.0, win.1
                ));
            }
        }
        let mut validator = eneros_safety_validator::SafetyValidator::empty();
        validator.add_rule(Box::new(
            eneros_safety_validator::ElectricalSafetyRule::new(
                params.max_power_kw,
                params.max_current_a,
                params.voltage_window,
                params.freq_window,
            ),
        ));
        validator.add_rule(Box::new(
            eneros_safety_validator::ProtectionCoordinationRule::new(
                params.protection_overcurrent_a,
                params.protection_overvoltage_v,
                params.protection_undervoltage_v,
                (params.freq_window.0 - 0.5, params.freq_window.1 + 0.5),
                params.ramp_kw_per_min,
            ),
        ));
        // 功率序 → 单条 ScheduleEntry 序列（正=放电，负=充电）。
        let entries: alloc::vec::Vec<eneros_energy_lp_model::result::ScheduleEntry> =
            power_series_kw
                .iter()
                .enumerate()
                .map(
                    |(period, p)| eneros_energy_lp_model::result::ScheduleEntry {
                        period,
                        charge_power_kw: p.min(0.0).abs(),
                        discharge_power_kw: p.max(0.0),
                        net_power_kw: *p,
                        soc_pct: if state.soc_pct.is_finite() {
                            state.soc_pct
                        } else {
                            0.0
                        },
                        revenue_yuan: 0.0,
                    },
                )
                .collect();
        let schedule = eneros_energy_lp_model::result::ScheduleResult {
            schedule: entries,
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: eneros_energy_lp_model::result::SolveStatus::Optimal,
        };
        let verdict = validator.validate(&schedule, state);
        if verdict.passed && !verdict.clamped {
            return Ok(());
        }
        let summary = verdict
            .violations
            .iter()
            .map(|v| format!("{}/{}/{:?}", v.rule, v.field, v.severity))
            .collect::<alloc::vec::Vec<_>>()
            .join(",");
        Err(if summary.is_empty() {
            String::from("clamped-by-rules")
        } else {
            summary
        })
    }

    /// WP-54.2 合核：run/run_intent 的 12 步授权链单一实现。
    ///
    /// 合并前两份手抄副本的五段逐字同构段（actor 门 / Envelope 装配+签名+
    /// 幂等键 / chain.execute 四臂 / rate reserve / IntentRecord 持久化）
    /// 收编于此；差异点全部显式参数化：
    /// - 身份派生：意图=(protocol,seq) 双派生且不占引擎自增序列；
    ///   计划=引擎自增 seq 前 8B LE 为 command_id、plan_id 派生 correlation
    ///   （**自增保持在 actor 门之后**——未授权请求不消耗序号空间，顺序即语义）;
    /// - 能量语义：意图写无能量语义（只计速率），AI 计划另过 energy 预算;
    /// - IdempotentHit 回执文案两入口注记不同，经 `idem_note` 透传。
    ///
    /// 关键不变量（附录 B 卡 2）：两入口都只允许 `ActorKind::Ai`——合并后
    /// 此门唯一存在，语义一字不改。
    fn execute_core(
        &mut self,
        cmd: CoreCommand,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        started: u64,
        mut states: Vec<DispatchState>,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        // 步骤 3：actor/capability/lease/fencing（+ 步骤 4：safety catalog）。
        // 只允许 AI 决策链角色（产品闭环唯一决策方；协议入站写沿用 AI
        // 决策链唯一签发面，dispatch_service.rs 同约束）。
        if actor.kind != ActorKind::Ai {
            return Err(DispatchError::NotAuthorized(format!(
                "actor kind {:?} 不是 Ai",
                actor.kind
            )));
        }
        let (command_id, correlation, sequence) = match cmd.identity {
            CoreIdentity::Intent {
                protocol_id,
                sequence,
            } => {
                let c = Self::intent_correlation(&protocol_id, sequence);
                (c, c, sequence)
            }
            CoreIdentity::Plan { plan_id } => {
                self.seq += 1;
                let mut cid = [0u8; 16];
                cid[..8].copy_from_slice(&self.seq.to_le_bytes());
                let corr = Self::correlation_of(&plan_id);
                (cid, corr, self.seq)
            }
        };
        let now_secs = started / 1000;
        let single = if cmd.power_kw > 0.0 {
            SingleCommand::On // 放电/合（协议意图写同规则）
        } else {
            SingleCommand::Off // 充电/分
        };

        // Envelope 装配+签名+幂等键（唯一实现）：幂等键=correlation——
        // 同计划重放 / 同 (protocol,seq) 跨重启重放均返回原结果，不重复写。
        // 61 v0.314.24：ControlEnvelope 提前构建（纯装配，无副作用）——
        // journal 预检需要命令摘要；投递复用同一实例。
        let buf = EnvelopeBuilder::new(
            command_id,
            self.profile.device_id,
            self.profile.point_id,
            ControlCommand::Single(single),
            now_secs,
            now_secs.saturating_add(cmd.ttl_secs),
            self.issuer_service,
        )
        .lease(self.profile.lease_id, self.fencing_token)
        .policy_version(self.profile.policy_version)
        .sequence(sequence)
        .actor_kind(ActorKind::Ai)
        .idempotency_key(correlation)
        .build(&self.key)
        .encode();
        let expires_ms = started.saturating_add(cmd.ttl_secs * 1000);
        let envelope = ControlEnvelope::new(
            correlation,
            self.issuer_service,
            self.issuer_epoch,
            started,
            expires_ms,
            buf,
            cmd.power_kw,
        );
        // 命令内容摘要（61 v0.314.24）：SM3(device_id ‖ point_id ‖ 命令类 ‖
        // power_kw LE)——只覆盖同 key 重传中必须不变的语义内容。
        // 不含 sequence/时间戳/HMAC（每次尝试本就不同，混入会把合法重传
        // 误判为冲突）。0.314.25 将由 canonical AuthorizedCommand digest
        // 接替（蓝图 05 §3.3）。
        let mut dbuf = [0u8; 4 + 4 + 1 + 8];
        dbuf[0..4].copy_from_slice(&self.profile.device_id.to_le_bytes());
        dbuf[4..8].copy_from_slice(&self.profile.point_id.to_le_bytes());
        dbuf[8] = match single {
            SingleCommand::On => 1,
            SingleCommand::Off => 0,
        };
        dbuf[9..17].copy_from_slice(&cmd.power_kw.to_le_bytes());
        let command_digest = eneros_crypto::sm3_hash(&dbuf);

        // 61 v0.314.24 步骤 2.7：执行 journal 同 key 预检（durable 幂等真值，
        // 先于链裁决——冲突命令不得消耗 SBO/链状态）：
        // - digest 不同 → IdempotencyConflict，拒绝（零投递）；
        // - Authorized/Persisted/Sent/TransportAcked → 返回当前状态，不新投递；
        // - AppliedConfirmed → 返回原 receipt；
        // - Rejected/Failed → 返回原终态；
        // - Indeterminate/Reconciling → 对账，禁止盲发。
        match self.journal.lookup(&correlation, &command_digest) {
            Some(JournalHit::Conflict) => {
                let note = String::from(
                    "idempotency-conflict：同幂等键不同命令内容（digest 不匹配）——拒绝",
                );
                self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                return Err(DispatchError::IdempotencyConflict(note));
            }
            Some(JournalHit::Current(rec)) => {
                return Ok(Self::receipt_from_journal(
                    &rec,
                    command_id,
                    correlation,
                    started,
                    (self.now_fn)(),
                    &cmd.idem_note,
                ));
            }
            None => {}
        }

        // SBO：选择+执行原子（ControlGateway 九步链，不双头授权）。
        self.chain.sbo_select(self.profile.point_id, now_secs);
        let fb: FeedbackBundle = self.feedback_builder.build(telemetry, now_secs);
        match self.chain.execute(&buf, now_secs, &fb) {
            ChainOutcome::Executed { .. } => {
                // 61 v0.314.24：journal 登记 Authorized（执行事实起点）。
                // journal 满 → Err 零投递（fail-closed，不伪造可投递）。
                if let Err(e) =
                    self.journal
                        .authorize(correlation, command_id, command_digest, started)
                {
                    let note = alloc::format!("execution-journal authorize failed: {e:?}");
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Err(DispatchError::IntentPersistFailed(note));
                }
            }
            ChainOutcome::IdempotentHit(prior) => {
                // 链缓存命中但 journal 无记录：仅发生在重放被拒/干跑（无执行
                // 事实可登记）或 journal 丢失。按真实 prior 返回——绝不
                // Committed（61 v0.314.24：幂等状态不提前终结）。
                if prior == OpOutcome::Authorized {
                    // 执行事实缺失：投递是否已发生不可证明 → 对账，不盲发。
                    let _ = self.journal.seed_reconciling(
                        correlation,
                        command_id,
                        command_digest,
                        started,
                    );
                    let note = String::from(
                        "idempotent-replay：journal 无记录（中间态不可证明）——对账，不盲发",
                    );
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Ok(DispatchReceipt {
                        command_id,
                        correlation_id: correlation,
                        final_state: DispatchState::Indeterminate,
                        state_path: alloc::vec![DispatchState::Indeterminate],
                        committed_at_ms: None,
                        note: Some(note),
                        latency_ms: (self.now_fn)().saturating_sub(started),
                    });
                }
                let note = if prior == OpOutcome::Rejected {
                    String::from("idempotent-replay：prior rejected（不重投）")
                } else {
                    String::from("idempotent-replay：prior dry-run（不重投）")
                };
                self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                return Err(DispatchError::NotAuthorized(note));
            }
            ChainOutcome::DryRun { .. } => {
                let note = String::from("dry-run-obligation（策略要求不写设备）");
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                return Err(DispatchError::NotAuthorized(note));
            }
            ChainOutcome::Denied(reason) => {
                let s = deny_reason_str(&reason);
                let (exec, safety) = if matches!(reason, ChainDenyReason::Unsafe(_)) {
                    (ExecOutcome::Failed, SafetyOutcome::Blocked)
                } else {
                    (ExecOutcome::Failed, SafetyOutcome::Passed)
                };
                self.audit(
                    exec,
                    safety,
                    &format!("states={:?};deny={}", states, s),
                    started,
                );
                let err = match reason {
                    ChainDenyReason::Unsafe(d) => DispatchError::SafetyRejected(d),
                    ChainDenyReason::FeedbackUntrusted(d) => DispatchError::FeedbackUntrusted(d),
                    _ => DispatchError::NotAuthorized(s),
                };
                return Err(err);
            }
        }
        states.push(DispatchState::Authorized);

        // 步骤 4.5（SPEC-SERIES-2026-08-27/A WP-A.2）：AI 电气护栏——
        // run（启发式/AI 计划）与 run_intent（协议入站写）两翼共过的
        // 单咽喉门。拒绝 ⇒ 审计 Blocked 真实置位 + target 串口判定行 +
        // SafetyRejected（时序先于预算门：先安全后额度）。
        {
            let guard_state = Self::project_system_state(telemetry);
            let params = SafetyGuardParams {
                max_power_kw: self.max_power_kw,
                max_current_a: Self::GUARD_MAX_CURRENT_A,
                voltage_window: self.safety_voltage_window,
                freq_window: self.safety_freq_window,
                ramp_kw_per_min: self.safety_ramp_kw_per_min,
                protection_overcurrent_a: Self::GUARD_PROT_OVERCURRENT_A,
                protection_overvoltage_v: Self::GUARD_PROT_OVERVOLTAGE_V,
                protection_undervoltage_v: Self::GUARD_PROT_UNDERVOLTAGE_V,
            };
            let series = [cmd.power_kw];
            if let Err(reason) = Self::safety_guard(params, &series, &guard_state) {
                // 61 v0.314.24：授权后未投递即终局 → journal Failed。
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Authorized,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Blocked,
                    &format!("states={states:?};safety-reject={reason}"),
                    started,
                );
                #[cfg(target_os = "none")]
                eneros_runtime::println!("[svc-control-dispatch] SAFETY-REJECTED reason={reason}");
                return Err(DispatchError::SafetyRejected(reason));
            }
        }

        // 步骤 5：rate 预算 reserve（不 record）；energy 仅 AI 计划路径有语义。
        if self.gate.check_rate(started) != GateDecision::Allow {
            let _ = self.journal.transition(
                &correlation,
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed,
                started,
                None,
            );
            return Err(DispatchError::BudgetExceeded("rate".into()));
        }
        if let Some(energy) = cmd.energy_kwh {
            if self.gate.check_energy(started, energy) != GateDecision::Allow {
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Authorized,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                return Err(DispatchError::BudgetExceeded("energy".into()));
            }
        }
        states.push(DispatchState::Reserved);

        // 步骤 6：durable intent 写入（唯一实现；61 v0.314.24 v2 记录携带
        // 命令摘要/尝试数——同 key 内容冲突与重启对账的持久依据）。
        let rec = IntentRecord {
            command_id,
            correlation_id: correlation,
            issued_at_ms: started,
            expires_at_ms: expires_ms,
            target_device: cmd.target_device,
            power_kw: cmd.power_kw,
            input_digest: cmd.input_digest,
            state: DispatchState::Persisted,
            command_digest: Some(command_digest),
            attempt: 1,
            receipt_digest: None,
        };
        // 61 v0.314.24：journal append 失败时不投递（记录保持 Authorized
        // 未决——重启后对账，不会误判为已投递）。
        if let Err(e) = self.intent_log.append(&rec) {
            let note = alloc::format!("intent persist failed: {e:?}（零投递，记录未决对账）");
            self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
            return Err(DispatchError::IntentPersistFailed(format!("{e:?}")));
        }
        if let Err(e) = self.journal.transition(
            &correlation,
            ExecutionStatus::Authorized,
            ExecutionStatus::Persisted,
            started,
            None,
        ) {
            let note = alloc::format!("execution-journal Authorized→Persisted failed: {e:?}");
            self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
            return Err(DispatchError::IntentPersistFailed(note));
        }
        states.push(DispatchState::Persisted);

        // 步骤 7-12：投递 + 回执（deliver_and_settle 共享不变）。
        let timeout = if deadline_ms == 0 {
            u64::MAX
        } else {
            deadline_ms
        };
        let response = self
            .delivery
            .deliver(&self.target_endpoint, &envelope.encode(), timeout);
        self.deliver_and_settle(
            response,
            states,
            started,
            command_id,
            correlation,
            cmd.energy_kwh.unwrap_or(0.0),
        )
    }

    /// 步骤 7-12 收尾（run/run_intent 共享）：投递 ControlEnvelope 并分类回执。
    /// 前置失败零投递；投递后超时/结果不明 → `Indeterminate`，不猜成功。
    ///
    /// 61 v0.314.24：journal 写序 `Sent → TransportAcked → AppliedConfirmed`
    /// 全程 expected-previous；投递后任何 journal/持久化失败 →
    /// `Indeterminate`（对账），不伪造成功或失败。`Committed` 只能从
    /// `AppliedConfirmed`（对端独立回读匹配）映射。
    #[allow(clippy::too_many_arguments)]
    fn deliver_and_settle(
        &mut self,
        response: Result<Vec<u8>, crate::ChannelError>,
        mut states: Vec<DispatchState>,
        started: u64,
        command_id: [u8; 16],
        correlation: [u8; 16],
        energy: f64,
    ) -> Result<DispatchReceipt, DispatchError> {
        // 步骤 7：投递已由调用方完成（envelope 参数）；此处直接分类回执。
        let response = match response {
            Ok(r) => {
                // 61 v0.314.24：投递返回 Ok → journal Sent + durable
                // 「已发送」落盘。任一失败 → Indeterminate（写可能已发生）。
                if let Err(e) = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Persisted,
                    ExecutionStatus::Sent,
                    started,
                    None,
                ) {
                    let _ = self
                        .intent_log
                        .update_state(&command_id, DispatchState::Indeterminate);
                    let note = alloc::format!("delivery-ok 但 journal Sent 推进失败 {e:?}——对账");
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Ok(self.indeterminate_receipt(
                        command_id,
                        correlation,
                        states,
                        started,
                        note,
                    ));
                }
                if self
                    .intent_log
                    .update_state(&command_id, DispatchState::Delivered)
                    .is_err()
                {
                    let note = String::from("delivery-ok 但 durable mark-sent 失败——对账");
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Ok(self.indeterminate_receipt(
                        command_id,
                        correlation,
                        states,
                        started,
                        note,
                    ));
                }
                r
            }
            Err(crate::ChannelError::Timeout) => {
                // 投递后超时：写可能已发生——Indeterminate 对账，不猜成功。
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Persisted,
                    ExecutionStatus::Indeterminate,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）",
                    started,
                );
                return Ok(self.indeterminate_receipt(
                    command_id,
                    correlation,
                    states,
                    started,
                    String::from("delivery-timeout：写可能已发生，需对账"),
                ));
            }
            Err(crate::ChannelError::Unreachable) => {
                // 通道不可达 = 未投递（可证明）→ 终局 Failed。
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Persisted,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    "delivery-unreachable",
                    started,
                );
                return Err(DispatchError::DeliveryUnreachable);
            }
            Err(e) => {
                // 61 v0.314.24（对抗审查 R7）：deliver 已被调用、信封可能已
                // 到达对端（生产 EIPC 通道的发送失败与回执丢失同报 Protocol）
                // ——除 Unreachable（endpoint 不可达=可证明未投递）外，一律
                // Indeterminate 对账，不假失败、不假成功。
                if matches!(e, crate::ChannelError::Unreachable) {
                    let _ = self.journal.transition(
                        &correlation,
                        ExecutionStatus::Persisted,
                        ExecutionStatus::Failed,
                        started,
                        None,
                    );
                    let _ = self
                        .intent_log
                        .update_state(&command_id, DispatchState::Failed);
                    self.audit(
                        ExecOutcome::Failed,
                        SafetyOutcome::Passed,
                        "delivery-unreachable",
                        started,
                    );
                    return Err(DispatchError::DeliveryUnreachable);
                }
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Persisted,
                    ExecutionStatus::Indeterminate,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("delivery: {e:?} → Indeterminate（写可能已发生，对账）"),
                    started,
                );
                return Ok(self.indeterminate_receipt(
                    command_id,
                    correlation,
                    states,
                    started,
                    alloc::format!("delivery-error {e:?}：物理结果未知，需对账"),
                ));
            }
        };
        states.push(DispatchState::Delivered);

        // 步骤 8-10：device-agent 回执（AuthorizedWriter 二次验证 + transport
        // ACK + 独立 readback 已在对端完成；结果码分层见 WP-40.11）。
        let frame = match OutcomeFrame::decode(&response) {
            Ok(f) => f,
            Err(e) => {
                // 61 v0.314.24：投递已发生、回执不可解 → 结果未知 →
                // Indeterminate 对账（原 Err(BadResponse) 会误导为未发生）。
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::Sent,
                    ExecutionStatus::Indeterminate,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("response-decode: {e:?} → Indeterminate（对账）"),
                    started,
                );
                return Ok(self.indeterminate_receipt(
                    command_id,
                    correlation,
                    states,
                    started,
                    String::from("response-decode-failed：物理结果未知，需对账"),
                ));
            }
        };
        // 61 v0.314.24：ACK 帧已解码 → journal TransportAcked；失败 → 对账。
        if let Err(e) = self.journal.transition(
            &correlation,
            ExecutionStatus::Sent,
            ExecutionStatus::TransportAcked,
            started,
            None,
        ) {
            let _ = self
                .intent_log
                .update_state(&command_id, DispatchState::Indeterminate);
            let note = alloc::format!("ack-decoded 但 journal TransportAcked 推进失败 {e:?}——对账");
            self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
            return Ok(self.indeterminate_receipt(command_id, correlation, states, started, note));
        }
        states.push(DispatchState::TransportAcked);
        let outcome = frame
            .outcome()
            .ok_or_else(|| DispatchError::BadResponse("未知结果码".into()))?;
        match outcome {
            OutcomeCode::Applied => {
                // 对端已独立 readback 验证（读回不得回显刚写 payload——
                // device-agent 侧执行点，见 WP-40.11）。
                // 61 v0.314.24 写序：durable 终态 ack 先行（持久确认成功），
                // 成功后才 journal AppliedConfirmed → 外部 Committed。
                // durable ack 失败 → Indeterminate（确认无法持久 = 未确认）。
                if self
                    .intent_log
                    .update_state(&command_id, DispatchState::Committed)
                    .is_err()
                {
                    let _ = self.journal.transition(
                        &correlation,
                        ExecutionStatus::TransportAcked,
                        ExecutionStatus::Indeterminate,
                        started,
                        None,
                    );
                    let note = String::from(
                        "applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）",
                    );
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Ok(self.indeterminate_receipt(
                        command_id,
                        correlation,
                        states,
                        started,
                        note,
                    ));
                }
                let receipt_digest = eneros_crypto::sm3_hash(&response);
                if let Err(e) = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::AppliedConfirmed,
                    started,
                    Some(receipt_digest),
                ) {
                    let note = alloc::format!("applied-confirmed 但 journal 推进失败 {e:?}——对账");
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Ok(self.indeterminate_receipt(
                        command_id,
                        correlation,
                        states,
                        started,
                        note,
                    ));
                }
                states.push(DispatchState::ReadbackVerified);
                // 61 v0.314.24：E4 证据 token——串口必须出现 applied-confirmed
                //（唯一 Committed 前置事实；readback 前禁止出现该行）。
                #[cfg(target_os = "none")]
                eneros_runtime::println!(
                    "[svc-control-dispatch] EXEC-JOURNAL applied-confirmed（独立回读匹配→Committed）"
                );
                // 步骤 11：confirmed dispatch 才记账 + 审计。
                self.gate.record(started, energy);
                states.push(DispatchState::Committed);
                self.committed_count += 1;
                self.audit(
                    ExecOutcome::Succeeded,
                    SafetyOutcome::Passed,
                    &format!("states={:?}", states),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Committed,
                    state_path: states,
                    committed_at_ms: Some((self.now_fn)()),
                    note: None,
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::IdempotentHit => {
                // 61 v0.314.24：对端幂等命中——本进程不持有原结果（对端
                // 可能复位/重放），物理状态未确认 → 对账，绝不升级 Committed。
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Reconciling,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                let note = String::from("对端幂等命中：原结果未知——对账，不升级 Committed");
                self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                Ok(self.indeterminate_receipt(command_id, correlation, states, started, note))
            }
            OutcomeCode::DryRun => {
                let note = String::from("对端 dry-run（未写设备）");
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Rejected,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Rejected);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Rejected,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::Denied => {
                let note = format!("device-denied class={}", frame.class);
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::AppliedReadbackUnavailable => {
                // 写可能已发生但物理状态无法确认 → Indeterminate。
                let note = String::from("readback-unavailable：物理状态未确认，需对账");
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Indeterminate,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Indeterminate);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(self.indeterminate_receipt(command_id, correlation, states, started, note))
            }
            OutcomeCode::AppliedReadbackMismatch => {
                let note = String::from("readback-mismatch：ACK 成功但物理状态未达目标");
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Blocked,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
            OutcomeCode::Failed => {
                let note = String::from("device-exec-failed");
                let _ = self.journal.transition(
                    &correlation,
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Failed,
                    started,
                    None,
                );
                let _ = self
                    .intent_log
                    .update_state(&command_id, DispatchState::Failed);
                self.audit(
                    ExecOutcome::Failed,
                    SafetyOutcome::Passed,
                    &format!("states={:?};note={}", states, note),
                    started,
                );
                Ok(DispatchReceipt {
                    command_id,
                    correlation_id: correlation,
                    final_state: DispatchState::Failed,
                    state_path: states,
                    committed_at_ms: None,
                    note: Some(note),
                    latency_ms: (self.now_fn)().saturating_sub(started),
                })
            }
        }
    }

    /// Indeterminate 回执（61 v0.314.24 统一形态：committed_at 恒 None）。
    fn indeterminate_receipt(
        &self,
        command_id: [u8; 16],
        correlation_id: [u8; 16],
        state_path: Vec<DispatchState>,
        started: u64,
        note: String,
    ) -> DispatchReceipt {
        let latency_ms = (self.now_fn)().saturating_sub(started);
        DispatchReceipt {
            command_id,
            correlation_id,
            final_state: DispatchState::Indeterminate,
            state_path,
            committed_at_ms: None,
            note: Some(note),
            latency_ms,
        }
    }

    /// journal 记录 → 原 receipt（同 key 重放；61 v0.314.24 §2.3 同 key 行为）。
    fn receipt_from_journal(
        rec: &IdempotencyRecord,
        command_id: [u8; 16],
        correlation: [u8; 16],
        started: u64,
        now: u64,
        idem_note: &str,
    ) -> DispatchReceipt {
        let (final_state, committed_at, detail): (DispatchState, Option<u64>, &str) =
            match rec.status {
                ExecutionStatus::AppliedConfirmed => (
                    DispatchState::Committed,
                    Some(rec.updated_at_ms),
                    "journal applied-confirmed（独立回读匹配）",
                ),
                ExecutionStatus::Authorized => (
                    DispatchState::Authorized,
                    None,
                    "当前状态 authorized——不重复投递",
                ),
                ExecutionStatus::Persisted => (
                    DispatchState::Persisted,
                    None,
                    "当前状态 persisted——不重复投递",
                ),
                ExecutionStatus::Sent => (
                    DispatchState::Delivered,
                    None,
                    "当前状态 sent——对账，不盲发",
                ),
                ExecutionStatus::TransportAcked => (
                    DispatchState::TransportAcked,
                    None,
                    "当前状态 transport-acked——对账",
                ),
                ExecutionStatus::Rejected => (DispatchState::Rejected, None, "原终态 rejected"),
                ExecutionStatus::Failed => (DispatchState::Failed, None, "原终态 failed"),
                ExecutionStatus::Indeterminate | ExecutionStatus::Reconciling => {
                    (DispatchState::Indeterminate, None, "不确定——对账，不盲发")
                }
            };
        DispatchReceipt {
            command_id,
            correlation_id: correlation,
            final_state,
            state_path: alloc::vec![final_state],
            committed_at_ms: committed_at,
            note: Some(alloc::format!("{idem_note}（{detail}）")),
            latency_ms: now.saturating_sub(started),
        }
    }
}

impl ControlDispatch for DispatchEngine {
    fn submit(
        &mut self,
        proposal: &ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        self.run(proposal, telemetry, actor, deadline_ms)
    }

    fn submit_intent(
        &mut self,
        intent: &PointWriteIntent,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        self.run_intent(intent, telemetry, actor, deadline_ms)
    }
}

// 集成测试在 tests/ 目录（需要装配完整 chain + 内存通道 + 对端 ingress）。
/// WP-A.2 测试桩：完整字段 TrustedTelemetry（engine 咽喉单测用）。
#[cfg(test)]
fn trusted_stub_state() -> TrustedTelemetry {
    TrustedTelemetry {
        state: SystemState::new(400.0, 50.0, 50.0, 0.5, 1_700_000_000_000),
        coverage: eneros_safety_validator::RequiredPointCoverage::empty(),
        valid_until_ms: u64::MAX,
        provenance_digest: [0u8; 32],
        generated_at_ms: 1,
        source_epoch: 0,
        mapping_version: 1,
        samples: BTreeMap::new(),
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn schedule_energy_estimate() {
        let s = eneros_energy_lp_model::result::ScheduleResult {
            schedule: alloc::vec![eneros_energy_lp_model::result::ScheduleEntry {
                period: 0,
                charge_power_kw: 0.0,
                discharge_power_kw: 5.0,
                net_power_kw: 5.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            }],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: eneros_energy_lp_model::result::SolveStatus::Optimal,
        };
        assert!((schedule_energy_kwh(&s) - 1.25).abs() < 1e-9);
    }
    // ===== SPEC-SERIES-2026-08-27/A · WP-A.2 电气护栏单测 =====

    fn guard_params() -> SafetyGuardParams {
        SafetyGuardParams {
            max_power_kw: 500.0,
            max_current_a: DispatchEngine::GUARD_MAX_CURRENT_A,
            voltage_window: (340.0, 420.0),
            freq_window: (49.5, 50.5),
            ramp_kw_per_min: 200.0,
            protection_overcurrent_a: DispatchEngine::GUARD_PROT_OVERCURRENT_A,
            protection_overvoltage_v: DispatchEngine::GUARD_PROT_OVERVOLTAGE_V,
            protection_undervoltage_v: DispatchEngine::GUARD_PROT_UNDERVOLTAGE_V,
        }
    }

    fn tel_state(v: f64, c: f64, f: f64, soc: f64) -> SystemState {
        SystemState::new(v, c, f, soc, 1_700_000_000_000)
    }

    /// C1-a：电压越上窗 → 拒绝。
    #[test]
    fn guard_rejects_overvoltage() {
        let st = tel_state(430.0, 50.0, 50.0, 0.5);
        let err = DispatchEngine::safety_guard(guard_params(), &[100.0], &st).unwrap_err();
        assert!(err.contains("electrical_safety"), "{err}");
    }

    /// C1-b：电压越下窗 → 拒绝。
    #[test]
    fn guard_rejects_undervoltage() {
        let st = tel_state(320.0, 50.0, 50.0, 0.5);
        assert!(DispatchEngine::safety_guard(guard_params(), &[-80.0], &st).is_err());
    }

    /// C1-c：频率越窗 → 拒绝。
    #[test]
    fn guard_rejects_frequency_out_of_window() {
        let st = tel_state(400.0, 50.0, 51.2, 0.5);
        let err = DispatchEngine::safety_guard(guard_params(), &[60.0], &st).unwrap_err();
        assert!(err.contains("electrical_safety"), "{err}");
    }

    /// C1-d：NaN 遥测投影为 unknown()（NaN fail-closed）→ 非有限前置拒绝。
    #[test]
    fn guard_nan_telemetry_fails_closed() {
        let mut tm = trusted_stub_state();
        tm.state.voltage_v = f64::NAN;
        let projected = DispatchEngine::project_system_state(&tm);
        assert!(projected.voltage_v.is_nan());
        assert_eq!(projected.timestamp_ms, 0);
        assert!(
            DispatchEngine::safety_guard(guard_params(), &[10.0], &projected).is_err(),
            "NaN 投影必须 fail-closed 拒绝"
        );
    }

    /// C1-e：合法工况直通（窗口内 + 功率帽内 + 无爬坡冲突）。
    #[test]
    fn guard_allows_nominal_dispatch() {
        let st = tel_state(400.0, 50.0, 50.0, 0.6);
        assert!(DispatchEngine::safety_guard(guard_params(), &[120.0], &st).is_ok());
        assert!(DispatchEngine::safety_guard(guard_params(), &[-150.0], &st).is_ok());
    }

    /// C1-f：功率超帽（>max_power_kw）→ 拒绝（同时充放互斥后置校验不触发——单符号序列）。
    #[test]
    fn guard_rejects_power_over_cap() {
        let st = tel_state(400.0, 60.0, 50.0, 0.55);
        let err = DispatchEngine::safety_guard(guard_params(), &[600.0], &st).unwrap_err();
        assert!(!err.is_empty(), "{err}");
    }

    /// C1-g：保护配合冲突——相邻时段爬坡超限 → 拒绝。
    #[test]
    fn guard_rejects_ramp_conflict() {
        let st = tel_state(400.0, 50.0, 50.0, 0.5);
        // 200 kW/min 上限：0→600 的跳变必炸。
        let err = DispatchEngine::safety_guard(guard_params(), &[0.0, 600.0], &st).unwrap_err();
        assert!(err.contains("protection_coordination"), "{err}");
    }

    /// C1-h：TTL 过期叠加越窗遥测 ⇒ 时效门在前（TelemetryExpired 先于护栏），
    /// 优先序由步骤序保证：guard 不做新鲜度裁决，仅作电气量裁决；
    /// 该性质在此以"投影对过期时间戳无感知、引擎层 step2 先行"的结构断言锁定：
    /// 投影函数对 ts=0 一律 unknown()，而 is_valid_at 由 run()/run_intent()
    /// 在护栏之前裁决——两级时序无歧义。
    #[test]
    fn guard_projection_precedence_is_structural() {
        let mut tm = trusted_stub_state();
        tm.state.timestamp_ms = 0; // 过期形态
        let projected = DispatchEngine::project_system_state(&tm);
        assert_eq!(projected.timestamp_ms, 0);
    }
}
