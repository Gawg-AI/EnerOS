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
use eneros_telemetry_model::command::{CommandValue, ControlCommand, SingleCommand};
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
    /// AI 计划批内命令（WP-5.1，v0.314.41）：command_id = 引擎自增 seq
    /// 前 8B LE；correlation 由 compiler `plan_command_correlation` 派生
    /// （adapter 单命令形态 = PlanId 字节，Phase 0 幂等记录兼容；多步 =
    /// intent_id 派生键——跨 tick 重编译恒稳定）。
    PlanCommand {
        /// 预派生幂等键（correlation）。
        correlation: [u8; 16],
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
    /// 蓝图 61 v0.314.29：CandidatePlan wire SHA-256（typed 链 durable
    /// 绑定；意图写 None）。
    plan_sha: Option<[u8; 32]>,
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
///
/// 蓝图 69.4 WP-69.4.4：22 个平铺 pub 字段收窄为 **10 个顶层字段**：
/// 签发身份 [`IssuerIdentity`]（6）、护栏限值 [`SafetyLimits`]（4）、
/// 留痕落点 [`AuditSinks`]（3）、投递绑定 [`DeliveryBinding`]（3）四束，
/// 加 `gate` / `resources` / `contracts` / `compiler_policy` /
/// `control_mode` / `now_fn` 六个单项。字段集合与类型零变化（分组只改
/// 组织）；装配种子见 [`crate::config_seed::ConfigSeed`]。
pub struct DispatchConfig {
    /// 签发身份与授权束（谁在签发、凭什么签发）＝默认资产路由。
    pub issuer: IssuerIdentity,
    /// 蓝图 70 R10：默认路由之外的 per-asset 扩展执行路由（多资产；
    /// 单资产装配留空——行为与旧单路由形态逐字节一致）。路由表禁双绑
    /// 同一 (device,point)（含默认路由；装配面静态可查）。
    pub extra_routes: alloc::vec::Vec<AssetRoute>,
    /// 速率/能量预算闸（check 在 submit；record 仅在 confirmed dispatch）。
    pub gate: DecisionGate,
    /// SPEC WP-A.2：护栏限值束。
    pub safety: SafetyLimits,
    /// 决策留痕落点束。
    pub audit_sinks: AuditSinks,
    /// 投递绑定束（往哪投 + 怎么回读）。
    pub delivery_binding: DeliveryBinding,
    /// 蓝图 61 v0.314.35：资源注册表只读视图（命令批编译的绑定权威；
    /// 生产装配注入 resource-registry `PlannerView`——planner 输入只能
    /// 查询 Registry，[08] §4.8 门；缺装配 = `EmptyResourceRegistry`
    /// fail-closed，批编译拒绝）。
    pub resources: alloc::boxed::Box<dyn eneros_decision_contract::ResourceRegistry + 'static>,
    /// 蓝图 61 v0.314.41（WP-5.1）：控制合同编译视图（PlanCompiler 的
    /// 合同半边——readback/可逆/值域/补偿；缺装配 = 空视图 fail-closed，
    /// 全部计划编译拒绝「未知合同」）。
    pub contracts: alloc::boxed::Box<dyn crate::compiler::PlanContractView + 'static>,
    /// WP-5.1 编译器策略（全部参与 compiler_digest——策略漂移可检）。
    pub compiler_policy: crate::compiler::CompilerPolicy,
    /// WP-5.3：资源控制模式（人工/检修模式下自动命令被 Arbiter 拒绝）。
    /// 装配期一次性设定、运行期不变——人工→自动切换在生产上只发生在
    /// `Arbiter` 层（`CommandArbiter::resume_auto` 持 fresh snapshot/
    /// capability，`tests/arbiter_degrade.rs` 覆盖）；Engine 层转发壳
    /// `DispatchEngine::resume_auto` 因零调用方已删（蓝图 69.4 WP-69.4.4，
    /// 卷内裁决 ②）。
    pub control_mode: crate::arbiter::ControlMode,
    /// 单调时钟（毫秒）。
    pub now_fn: fn() -> u64,
    /// 蓝图 70.8 WP-70.8.4：可信输入策略——`Required` 下引擎拒绝一切
    /// 非冻结快照身份的候选（生产装配值）；`LegacyMigration` 仅限迁移
    /// 期差分对照（不具备新控制资格）。
    pub trusted_input: TrustedInputPolicy,
    /// 蓝图 70.9 WP-70.9.1：独立认证来源（生产 = 独立认证者；缺省 =
    /// [`crate::certify::NoCertifier`] fail-closed——无本地发放回退）。
    pub certifier: alloc::boxed::Box<dyn crate::certify::CertificateSource + 'static>,
    /// 蓝图 70.9 WP-70.9.3：验方 key 环（key id/version/MAC 验真；轮换
    /// N-1 窗口与退役即时失效由 [`eneros_certifier::proof::CertifierKeyRing`]
    /// 承载）。
    pub certifier_ring: eneros_certifier::proof::CertifierKeyRing,
}

/// 可信输入准入策略（蓝图 70.8 WP-70.8.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedInputPolicy {
    /// 生产：候选必须携带与可信输入同代的快照身份/目录版本/绑定摘要，
    /// 否则整批拒绝（零投递）。
    Required,
    /// 迁移窗：允许旧派生伪快照身份（差分/历史 wire 读入）。
    LegacyMigration,
}

impl Default for TrustedInputPolicy {
    /// 缺省 = 生产形态（fail-closed）；迁移轨必须显式声明。
    fn default() -> Self {
        Self::Required
    }
}

/// 签发身份与授权束（蓝图 69.4 WP-69.4.4 分组）——授权链、信封签名
/// 密钥、同源档案、fencing token、签发服务与代次：信封签发路径成束
/// 消费，禁再散抄。
///
/// 蓝图 70 R10：本束承载**默认资产路由**；更多资产的执行路由经
/// [`DispatchConfig::extra_routes`] 注入（与默认路由同构，见
/// [`AssetRoute`]）。
pub struct IssuerIdentity {
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
}

/// 蓝图 70 R10：per-asset 执行路由（装配期冻结，运行期只读）。
///
/// 与默认路由（[`IssuerIdentity::chain`]/[`IssuerIdentity::profile`]）
/// 同构：信封签发与 12 步链复验按本路由执行——每资产独立链实例
/// （重放缓存/幂等/SBO/租约各自成立，安全语义零放宽、零共享）。
/// 路由键 = `profile` 的 `(device_id, point_id)`（点表协议映射单源；
/// 与 [`IssuerIdentity::profile`] 重复即装配错误——路由表禁双绑同一
/// (device,point)）。
pub struct AssetRoute {
    /// 全局资产 id（诊断/观测面；路由判定不消费）。
    pub asset_id: u64,
    /// 同源档案（语义同 [`IssuerIdentity::profile`]）。
    pub profile: ControlProfile,
    /// 授权链实例（语义同 [`IssuerIdentity::chain`]）。
    pub chain: ControlChain,
}

/// SPEC WP-A.2：护栏限值束（工程单位见各字段；测试域可自由构造）。
/// 与引擎内部私有 `SafetyGuardParams` 对齐（后者另含预留阈派生域）。
#[derive(Debug, Clone, Copy)]
pub struct SafetyLimits {
    /// 功率上限（kW）——proposal 校验。
    pub max_power_kw: f64,
    /// SPEC WP-A.2：电压保护窗（V；缺省 = 蓝图默认 (340,420)）。
    pub safety_voltage_window: (f64, f64),
    /// SPEC WP-A.2：频率保护窗（Hz；缺省 = (49.5,50.5)）。
    pub safety_freq_window: (f64, f64),
    /// SPEC WP-A.2：保护配合爬坡率上限（kW/min；缺省 = 校验器默认 200）。
    pub safety_ramp_kw_per_min: f64,
}

/// 决策留痕落点束（蓝图 69.4 WP-69.4.4 分组）：决策审计、SHA-256
/// 事件审计链、durable intent 存储——三者都是「决策留痕落点」。
pub struct AuditSinks {
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
}

/// 投递绑定束（蓝图 69.4 WP-69.4.4 分组）：往哪投 + 怎么回读。
pub struct DeliveryBinding {
    /// 投递目标端点（EIPC 服务名）。
    pub target_endpoint: String,
    /// 投递通道（生产：Sel4EipcChannel；测试：内存通道）。
    pub delivery: Box<dyn DeliveryChannel>,
    /// 反馈点映射（受信任遥测 → FeedbackBundle）。
    pub feedback_mapping: crate::feedback::FeedbackPointMapping,
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
    /// 蓝图 61 v0.314.35：资源注册表视图（装配注入；命令批编译的绑定
    /// 权威——planner 输入只查询 Registry）。
    resources: alloc::boxed::Box<dyn eneros_decision_contract::ResourceRegistry + 'static>,
    /// WP-5.1：控制合同编译视图（批编译合同半边；见 DispatchConfig）。
    contracts: alloc::boxed::Box<dyn crate::compiler::PlanContractView + 'static>,
    /// WP-5.1：编译器策略。
    compiler_policy: crate::compiler::CompilerPolicy,
    /// WP-5.3：唯一生产 CommandArbiter（[10] §4.7：全仓唯一）。
    arbiter: crate::arbiter::CommandArbiter,
    /// WP-5.3：资源控制模式。
    control_mode: crate::arbiter::ControlMode,
    key: Vec<u8>,
    /// 蓝图 70 R10：per-asset 执行路由表（routes[0] = 默认路由 = 装配
    /// issuer；其后为 extra_routes）。信封签发/链复验/SBO/爬坡基线全部
    /// 按命令命中的路由执行——每资产独立链实例，安全语义零放宽。
    routes: alloc::vec::Vec<AssetRoute>,
    fencing_token: u64,
    issuer_service: [u8; 16],
    issuer_epoch: u64,
    target_endpoint: String,
    max_power_kw: f64,
    /// SPEC WP-A.2：电气护栏参数（窗口/爬坡，见 DispatchConfig 同名字段）。
    safety_voltage_window: (f64, f64),
    safety_freq_window: (f64, f64),
    safety_ramp_kw_per_min: f64,
    /// 速率/能量预算闸（观测：used_kwh/rejected 计数；经只读 getter
    /// `gate_used_kwh`/`gate_rejected_energy_kwh` 暴露——蓝图 63 WP-63.1.2
    /// 写序唯一编排点结构收口，无 pub 可变字段）。
    gate: DecisionGate,
    /// 决策审计（观测：链校验/条目；经 `audit_log` 只读暴露）。
    audit: Option<DecisionAudit>,
    /// 47：SHA-256 控制事件审计链。
    control_audit: Option<Box<dyn eneros_audit::AuditLogger>>,
    /// durable intent 存储（观测：find/pending 对账；经 `find_intent`/
    /// `pending_intents` 只读暴露——写只经 settle/push_stage 编排点）。
    intent_log: Box<dyn IntentStore>,
    delivery: Box<dyn DeliveryChannel>,
    /// 61 v0.314.24：执行 journal（执行状态与幂等真值唯一持有者）。
    /// 启动时从 intent store 未决记录播种 Reconciling——旧版/崩溃遗留的
    /// 非终态记录永不以 Committed 复活。
    journal: ExecutionJournal,
    feedback_builder: TrustedFeedbackBuilder,
    now_fn: fn() -> u64,
    /// 蓝图 70.8 WP-70.8.4：可信输入准入策略。
    trusted_input: TrustedInputPolicy,
    /// 蓝图 70.9：独立认证来源与验方 key 环。
    certifier: alloc::boxed::Box<dyn crate::certify::CertificateSource + 'static>,
    certifier_ring: eneros_certifier::proof::CertifierKeyRing,
    seq: u64,
    /// 已提交（Committed）次数（观测/审计；经 `committed_count` 只读）。
    committed_count: u64,
    /// 最近一次过渡 proof digest（蓝图 66 WP-66.2.2 观测面，getter doc
    /// 见 last_transition_proof_digest）。
    last_transition_proof_digest: Option<[u8; 32]>,
    /// 审计丢弃计数。
    audit_drop_count: u64,
    /// 47（Exit 补证）：成功入链条数（append 计数——QEMU 串口
    /// AUDIT-CHAIN 行依据；零计数不打印，杜绝恒 ok 式假证据）。
    audit_append_count: u64,
    /// 47（WP-47.1）：每来源协议已见最大意图序号（重放门——重复
    /// sequence 零写；跨重启由链幂等（意图 correlation 派生）兜底）。
    last_seq: BTreeMap<String, u64>,
    /// 复查 FIX（61 §3.4）：每 (device,point) 最近 AppliedConfirmed
    /// setpoint（值, 确认时刻 ms）——单点爬坡检查的参照基线。仅内存
    /// （重启后无参照 = 首命令放行；跨重启的爬坡基线归对账/点表）。
    last_confirmed: BTreeMap<(u32, u32), (f64, u64)>,
    held_plans: Option<crate::held_plan::DurablePlans>,
    held_store_unavailable: bool,
    /// ADR-0054:29–30：持有输入保留面（None = 未装配——host 测试或
    /// fail-closed 装配由调用方以 NoRetention 显式承担）。
    input_retainer: Option<alloc::boxed::Box<dyn held_engine::HeldInputRetainer>>,
    /// 已解除钉的快照 ID（防重复 release；有界环形回收）。
    released_holds: alloc::vec::Vec<[u8; 16]>,
    /// 已打印终态行的计划 ID（HELD-PLAN-DONE 一次性证据行）。
    held_done_logged: alloc::vec::Vec<eneros_decision_contract::PlanId>,
    /// 钉面失败计数（hold 拒绝/装配缺失面——可观测不静默）。
    pub held_pin_failures: u64,
}

// ==========================================================================
// 62 v0.315.0 WP-62.1.3（S3）：结算决策表。
//
// 每条命令的结局（journal 状态转移 + durable intent 状态 + 回执形态 +
// audit 文案）由纯函数 [`settle`] 单点决定；[`SettleDispatch::settle_command`]
// 是结算路径上 `journal.transition` / `intent_log.update_state` / `audit`
// 的唯一编排点（stage 推进另经 [`SettleDispatch::push_stage`]）。
// 文案经 [`AuditText`] 惰性渲染，串口/audit 词形逐字节不变（audit_chain
// 与 45–47/61 冻结证据链依赖）。写序不变量（61 v0.314.24：Applied =
// durable 终态 ack 先行，成功后才 journal AppliedConfirmed）由表内
// `durable` 序表达，表级单测 `settle_table_*` 锁定。
// ==========================================================================

/// 结算触发事件（execute_core 前置拒绝 + deliver_and_settle 全部分类臂）。
///
/// 新增一个触发（如假想 OutcomeCode）的成本 = 本枚举一臂 + `settle`
/// 表一行 + 表测试一用例 + 流程分类点一行——不动任何既有臂。
#[derive(Debug)]
enum SettleTrigger {
    /// 电气护栏拒绝（步骤 4.5，授权后零投递）。
    SafetyReject { reason: String },
    /// 速率预算拒绝（步骤 5；现状无 audit——保持）。
    BudgetRate,
    /// 能量预算拒绝（步骤 5；现状无 audit——保持）。
    BudgetEnergy,
    /// durable intent append 失败（步骤 6，零投递终局）。
    IntentAppendFailed { err: String },
    /// journal Authorized→Persisted 推进失败（步骤 6.5）。
    JournalPersistFailed { err: String },
    /// 投递 Ok 但 journal Persisted→Sent 推进失败（步骤 7）。
    SentAdvanceFailed { err: String },
    /// 投递 Ok 但 durable mark-sent 失败（步骤 7）。
    MarkDeliveredFailed,
    /// 投递超时（写可能已发生）。
    DeliveryTimeout,
    /// 通道不可达（可证明未投递 → 终局 Failed）。
    DeliveryUnreachable,
    /// 投递通用错误（写可能已发生）。
    DeliveryError { err: String },
    /// 回执帧解码失败（写已发生、结果未知）。
    ResponseDecodeFailed { err: String },
    /// ACK 已解码但 journal Sent→TransportAcked 推进失败。
    AckAdvanceFailed { err: String },
    /// 对端 Applied（两相成功，见 [`Settlement`] 写序）。
    Applied,
    /// Applied 但 durable 终态落盘失败（嵌套触发）。
    AppliedDurableFailed,
    /// Applied 但 journal AppliedConfirmed 推进失败（嵌套触发）。
    AppliedAdvanceFailed { err: String },
    /// 对端幂等命中（本进程不持原结果，绝不升级 Committed）。
    PeerIdempotentHit,
    /// 对端 dry-run（未写设备）。
    PeerDryRun,
    /// 对端拒绝。
    PeerDenied { class: u8 },
    /// 写可能已发生但读回不可用。
    ReadbackUnavailable,
    /// ACK 成功但读回不匹配。
    ReadbackMismatch,
    /// 对端执行失败。
    PeerFailed,
}

/// 单笔 durable 写（结算表行内元素）。
#[derive(Debug, PartialEq, Eq)]
enum SettleOp {
    /// journal 转移（Applied 行的 journal 写携带 sm3(回执) 摘要——
    /// 由 `settle_command` 的 Applied 分支供给；其余行摘要恒 None）。
    Journal {
        from: ExecutionStatus,
        to: ExecutionStatus,
    },
    /// durable intent 状态推进。
    Intent { state: DispatchState },
}

/// audit 文案（惰性渲染：states 调试串在编排点按终态拼接）。
#[derive(Debug)]
enum AuditText {
    /// 预格式文案（含动态 err/class 内容，不含 states）。
    Formatted(String),
    /// `states={states:?};{suffix}`（suffix 形如 `note=…`/`safety-reject=…`）。
    StatesSuffix(String),
    /// `states={states:?}`（Applied 成功终审）。
    States,
}

impl AuditText {
    fn render(&self, states: &[DispatchState]) -> String {
        match self {
            AuditText::Formatted(s) => s.clone(),
            AuditText::StatesSuffix(suffix) => alloc::format!("states={states:?};{suffix}"),
            AuditText::States => alloc::format!("states={states:?}"),
        }
    }
}

/// 回执形态（结算表第四元）。
#[derive(Debug, Clone)]
enum SettleReceipt {
    /// 成功收尾（调用侧做记账/爬坡基线/计数后回 Committed 回执）。
    Committed,
    /// Ok 回执 + 终态（Rejected/Failed）。
    Terminal {
        final_state: DispatchState,
        note: String,
    },
    /// Ok 回执 + Indeterminate（对账，不猜成败）。
    Indeterminate { note: String },
    /// `Err(DeliveryUnreachable)`。
    Unreachable,
    /// `Err(SafetyRejected)`。
    SafetyReject { reason: String },
    /// `Err(BudgetExceeded)`。
    BudgetExceeded { kind: &'static str },
    /// `Err(IntentPersistFailed)`。
    IntentPersistFailed { err: String },
}

/// 结算结局（触发 → 四元组：durable 写序 + audit + 回执形态）。
#[derive(Debug)]
struct Settlement {
    /// durable 写序（按序执行）。Applied 行 = `[Intent(Committed),
    /// Journal(TransportAcked→AppliedConfirmed)]`——durable 终态 ack
    /// 先行（61 v0.314.24 写序不变量的表内表达）。
    durable: Vec<SettleOp>,
    /// audit（None = 该触发现状不写 audit——rate/energy 预算拒绝）。
    audit: Option<(ExecOutcome, SafetyOutcome, AuditText)>,
    receipt: SettleReceipt,
}

/// 结算决策表（纯函数）：触发事件 → 结局。
///
/// 表级单测（`unit_tests::settle_table_*`）逐触发断言四元组与
/// 文案逐字节词形；新增触发事件只需本 match 加一臂。
fn settle(trigger: &SettleTrigger, states: &[DispatchState]) -> Settlement {
    let failed_pass = (ExecOutcome::Failed, SafetyOutcome::Passed);
    let _ = states; // 文案经 AuditText 惰性渲染；签名保留 states 语义位
    let (durable, audit, receipt) = match trigger {
        SettleTrigger::SafetyReject { reason } => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            Some((
                ExecOutcome::Failed,
                SafetyOutcome::Blocked,
                AuditText::StatesSuffix(alloc::format!("safety-reject={reason}")),
            )),
            SettleReceipt::SafetyReject {
                reason: reason.clone(),
            },
        ),
        SettleTrigger::BudgetRate => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            None,
            SettleReceipt::BudgetExceeded { kind: "rate" },
        ),
        SettleTrigger::BudgetEnergy => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            None,
            SettleReceipt::BudgetExceeded { kind: "energy" },
        ),
        SettleTrigger::IntentAppendFailed { err } => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "intent persist failed: {err}（零投递，终局 Failed）"
                )),
            )),
            SettleReceipt::IntentPersistFailed { err: err.clone() },
        ),
        SettleTrigger::JournalPersistFailed { err } => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "execution-journal Authorized→Persisted failed: {err}"
                )),
            )),
            SettleReceipt::IntentPersistFailed {
                err: alloc::format!("execution-journal Authorized→Persisted failed: {err}"),
            },
        ),
        SettleTrigger::SentAdvanceFailed { err } => (
            alloc::vec![SettleOp::Intent {
                state: DispatchState::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "delivery-ok 但 journal Sent 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("delivery-ok 但 journal Sent 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::MarkDeliveredFailed => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from("delivery-ok 但 durable mark-sent 失败——对账")),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("delivery-ok 但 durable mark-sent 失败——对账"),
            },
        ),
        SettleTrigger::DeliveryTimeout => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("delivery-timeout：写可能已发生，需对账"),
            },
        ),
        SettleTrigger::DeliveryUnreachable => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Failed,
                },
                SettleOp::Intent {
                    state: DispatchState::Failed,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from("delivery-unreachable")),
            )),
            SettleReceipt::Unreachable,
        ),
        SettleTrigger::DeliveryError { err } => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "delivery: {err} → Indeterminate（写可能已发生，对账）"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("delivery-error {err}：物理结果未知，需对账"),
            },
        ),
        SettleTrigger::ResponseDecodeFailed { err } => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Sent,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "response-decode: {err} → Indeterminate（对账）"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("response-decode-failed：物理结果未知，需对账"),
            },
        ),
        SettleTrigger::AckAdvanceFailed { err } => (
            alloc::vec![SettleOp::Intent {
                state: DispatchState::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "ack-decoded 但 journal TransportAcked 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("ack-decoded 但 journal TransportAcked 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::Applied => (
            // 写序不变量：durable 终态 ack 先行，成功后才
            // AppliedConfirmed → 外部 Committed（61 v0.314.24）。
            alloc::vec![
                SettleOp::Intent {
                    state: DispatchState::Committed,
                },
                SettleOp::Journal {
                    from: ExecutionStatus::TransportAcked,
                    to: ExecutionStatus::AppliedConfirmed,
                },
            ],
            Some((
                ExecOutcome::Succeeded,
                SafetyOutcome::Passed,
                AuditText::States,
            )),
            SettleReceipt::Committed,
        ),
        SettleTrigger::AppliedDurableFailed => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::TransportAcked,
                to: ExecutionStatus::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）"),
            },
        ),
        SettleTrigger::AppliedAdvanceFailed { err } => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "applied-confirmed 但 journal 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("applied-confirmed 但 journal 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::PeerIdempotentHit => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::TransportAcked,
                    to: ExecutionStatus::Reconciling,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "对端幂等命中：原结果未知——对账，不升级 Committed",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("对端幂等命中：原结果未知——对账，不升级 Committed"),
            },
        ),
        SettleTrigger::PeerDryRun => {
            let note = String::from("对端 dry-run（未写设备）");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Rejected,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Rejected,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Rejected,
                    note,
                },
            )
        }
        SettleTrigger::PeerDenied { class } => {
            let note = alloc::format!("device-denied class={class}");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
        SettleTrigger::ReadbackUnavailable => {
            let note = String::from("readback-unavailable：物理状态未确认，需对账");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Indeterminate,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Indeterminate,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Indeterminate { note },
            )
        }
        SettleTrigger::ReadbackMismatch => {
            let note = String::from("readback-mismatch：ACK 成功但物理状态未达目标");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    ExecOutcome::Failed,
                    SafetyOutcome::Blocked,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
        SettleTrigger::PeerFailed => {
            let note = String::from("device-exec-failed");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
    };
    Settlement {
        durable,
        audit,
        receipt,
    }
}

/// stage 推进（流程继续型 durable 写，区别于终局结算）。
#[derive(Debug, Clone, Copy)]
enum SettleStage {
    /// journal Authorized→Persisted（步骤 6.5）。
    JournalPersisted,
    /// journal Persisted→Sent（投递 Ok 后，步骤 7）。
    JournalSent,
    /// durable intent → Delivered（步骤 7）。
    IntentDelivered,
    /// journal Sent→TransportAcked（ACK 解码后，步骤 8）。
    JournalTransportAcked,
}

/// 结算不变参（蓝图 63 WP-63.1.3）：一次命令结算路径上**不随触发变化**
/// 的上下文——`started`/`command_id`/`correlation`/`energy`/
/// `setpoint_kw`/`response`。`states` 是唯一随臂推进的可变参，仍逐调用
/// 传 `&mut`。
///
/// `response` 两段式（非变异）：投递帧未到位（前置结算）→ `None`；
/// 帧解码后调用点以 `Some(&帧)` 重建 ctx 拷贝。Copy 语义——绝不在
/// 编排点内原地改 ctx。
#[derive(Clone, Copy)]
struct ExecutionCtx<'a> {
    started: u64,
    command_id: [u8; 16],
    correlation: [u8; 16],
    /// 对端回执原文（Applied 行 sm3 摘要供给；前置结算 None）。
    response: Option<&'a [u8]>,
    /// 记账能量（kWh；前置结算路径 0.0——与 execute_core 现状一致）。
    energy: f64,
    /// 连续 setpoint 值（AppliedConfirmed 时更新爬坡基线；离散 None）。
    setpoint_kw: Option<f64>,
    /// 蓝图 70 R10：命令语义目标 (device, point)——AppliedConfirmed 爬坡
    /// 基线按键（多资产各键独立；单资产形态 = 旧 (profile.device,profile.point)
    /// 逐字节一致）。
    target: (u32, u32),
}

/// 结算编排面（`DispatchEngine` 内聚实现；host/target 同源）。
trait SettleDispatch {
    /// stage 推进单点：恰一笔 durable 写，失败返回 `{err:?}` 文案
    /// （与原各内联块 `{e:?}` 词形一致）。
    fn push_stage(
        &mut self,
        stage: SettleStage,
        command_id: &[u8; 16],
        correlation: &[u8; 16],
        started: u64,
    ) -> Result<(), String>;

    /// 结算单点编排：表驱动 durable 写序（Applied 两相含嵌套结算）+
    /// audit + 回执。`ctx.response` = 回执原文（Applied 行 sm3 摘要供给）。
    /// 蓝图 63 WP-63.1.3：原九参（`#[allow(clippy::too_many_arguments)]`）
    /// 六不变列收进 [`ExecutionCtx`]，每调用点 1–3 行。
    fn settle_command(
        &mut self,
        trigger: SettleTrigger,
        states: &mut Vec<DispatchState>,
        ctx: &ExecutionCtx<'_>,
    ) -> Result<DispatchReceipt, DispatchError>;

    /// 嵌套结算（Applied 两相失败；durable 写失败一律容忍——
    /// 61 v0.314.24 `let _` 语义，不再递归）。
    fn settle_nested(
        &mut self,
        s: &Settlement,
        states: &mut Vec<DispatchState>,
        ctx: &ExecutionCtx<'_>,
    ) -> Result<DispatchReceipt, DispatchError>;

    /// 结算回执映射（四元组第四元的落地）。
    fn receipt_result(
        &self,
        receipt: SettleReceipt,
        states: &[DispatchState],
        started: u64,
        command_id: [u8; 16],
        correlation: [u8; 16],
    ) -> Result<DispatchReceipt, DispatchError>;
}

impl SettleDispatch for DispatchEngine {
    fn push_stage(
        &mut self,
        stage: SettleStage,
        command_id: &[u8; 16],
        correlation: &[u8; 16],
        started: u64,
    ) -> Result<(), String> {
        // journal 转移单点：stage → (from, to) 映射后一次调用
        //（62 v0.315.0 WP-62.1.3 收敛判据：transition 字面调用 ≤5）。
        let journal_pair = match stage {
            SettleStage::JournalPersisted => {
                (ExecutionStatus::Authorized, ExecutionStatus::Persisted)
            }
            SettleStage::JournalSent => (ExecutionStatus::Persisted, ExecutionStatus::Sent),
            SettleStage::JournalTransportAcked => {
                (ExecutionStatus::Sent, ExecutionStatus::TransportAcked)
            }
            SettleStage::IntentDelivered => {
                return self
                    .intent_log
                    .update_state(command_id, DispatchState::Delivered)
                    .map(|_| ())
                    .map_err(|e| alloc::format!("{e:?}"));
            }
        };
        self.journal
            .transition(correlation, journal_pair.0, journal_pair.1, started, None)
            .map(|_| ())
            .map_err(|e| alloc::format!("{e:?}"))
    }

    fn settle_command(
        &mut self,
        trigger: SettleTrigger,
        states: &mut Vec<DispatchState>,
        ctx: &ExecutionCtx<'_>,
    ) -> Result<DispatchReceipt, DispatchError> {
        // 63 WP-63.1.3：ctx 解构为既有局部（本体内引用原样——纯机械
        // 收口，零行为/文案变化）。
        let started = ctx.started;
        let command_id = ctx.command_id;
        let correlation = ctx.correlation;
        let settlement = settle(&trigger, states);
        if matches!(trigger, SettleTrigger::Applied) {
            // 61 v0.314.24 写序（表内 durable 序 [Intent, Journal]）：
            // durable 终态 ack 先行（持久确认成功），成功后才 journal
            // AppliedConfirmed → 外部 Committed。durable ack 失败 →
            // Indeterminate（确认无法持久 = 未确认）。
            if self
                .intent_log
                .update_state(&command_id, DispatchState::Committed)
                .is_err()
            {
                return self.settle_nested(
                    &settle(&SettleTrigger::AppliedDurableFailed, states),
                    states,
                    ctx,
                );
            }
            let response = ctx.response.expect("Applied 结算必须携带回执原文");
            let receipt_digest = eneros_crypto::sm3_hash(response);
            if let Err(e) = self.journal.transition(
                &correlation,
                ExecutionStatus::TransportAcked,
                ExecutionStatus::AppliedConfirmed,
                started,
                Some(receipt_digest),
            ) {
                return self.settle_nested(
                    &settle(
                        &SettleTrigger::AppliedAdvanceFailed {
                            err: alloc::format!("{e:?}"),
                        },
                        states,
                    ),
                    states,
                    ctx,
                );
            }
            states.push(DispatchState::ReadbackVerified);
            // 61 v0.314.24：E4 证据 token——串口必须出现 applied-confirmed
            //（唯一 Committed 前置事实；readback 前禁止出现该行）。
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[svc-control-dispatch] EXEC-JOURNAL applied-confirmed（独立回读匹配→Committed）"
            );
            // 复查 FIX（61 §3.4）：记录最近 AppliedConfirmed setpoint
            //（单点爬坡基线；仅 ActivePowerKw 有连续量语义）。蓝图 70 R10：
            // 基线按键 = 命令语义目标 (device,point)（多资产各键独立）。
            if let Some(v) = ctx.setpoint_kw {
                self.last_confirmed.insert(ctx.target, (v, (self.now_fn)()));
            }
            // 步骤 11：confirmed dispatch 才记账。
            self.gate.record(started, ctx.energy);
            states.push(DispatchState::Committed);
            self.committed_count += 1;
        } else {
            // 终局触发的 durable 写一律容忍失败（原 `let _` 语义）。
            for op in &settlement.durable {
                match op {
                    SettleOp::Journal { from, to } => {
                        let _ = self
                            .journal
                            .transition(&correlation, *from, *to, started, None);
                    }
                    SettleOp::Intent { state } => {
                        let _ = self.intent_log.update_state(&command_id, *state);
                    }
                }
            }
        }
        if let Some((exec, safety, text)) = &settlement.audit {
            self.audit(*exec, *safety, &text.render(states), started);
        }
        self.receipt_result(settlement.receipt, states, started, command_id, correlation)
    }

    fn settle_nested(
        &mut self,
        s: &Settlement,
        states: &mut Vec<DispatchState>,
        ctx: &ExecutionCtx<'_>,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = ctx.started;
        let command_id = ctx.command_id;
        let correlation = ctx.correlation;
        for op in &s.durable {
            match op {
                SettleOp::Journal { from, to } => {
                    let _ = self
                        .journal
                        .transition(&correlation, *from, *to, started, None);
                }
                SettleOp::Intent { state } => {
                    let _ = self.intent_log.update_state(&command_id, *state);
                }
            }
        }
        if let Some((exec, safety, text)) = &s.audit {
            self.audit(*exec, *safety, &text.render(states), started);
        }
        self.receipt_result(s.receipt.clone(), states, started, command_id, correlation)
    }

    fn receipt_result(
        &self,
        receipt: SettleReceipt,
        states: &[DispatchState],
        started: u64,
        command_id: [u8; 16],
        correlation: [u8; 16],
    ) -> Result<DispatchReceipt, DispatchError> {
        match receipt {
            SettleReceipt::Committed => Ok(DispatchReceipt {
                command_id,
                correlation_id: correlation,
                final_state: DispatchState::Committed,
                state_path: Vec::from(states),
                committed_at_ms: Some((self.now_fn)()),
                note: None,
                latency_ms: (self.now_fn)().saturating_sub(started),
            }),
            SettleReceipt::Terminal { final_state, note } => Ok(DispatchReceipt {
                command_id,
                correlation_id: correlation,
                final_state,
                state_path: Vec::from(states),
                committed_at_ms: None,
                note: Some(note),
                latency_ms: (self.now_fn)().saturating_sub(started),
            }),
            SettleReceipt::Indeterminate { note } => Ok(self.indeterminate_receipt(
                command_id,
                correlation,
                Vec::from(states),
                started,
                note,
            )),
            SettleReceipt::Unreachable => Err(DispatchError::DeliveryUnreachable),
            SettleReceipt::SafetyReject { reason } => Err(DispatchError::SafetyRejected(reason)),
            SettleReceipt::BudgetExceeded { kind } => {
                Err(DispatchError::BudgetExceeded(String::from(kind)))
            }
            SettleReceipt::IntentPersistFailed { err } => {
                Err(DispatchError::IntentPersistFailed(err))
            }
        }
    }
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
        let pending = config.audit_sinks.intent_log.pending();
        // 蓝图 70 R10：路由表 = 默认路由（issuer）打头 + 扩展路由；
        // (device,point) 双绑 = 装配错误（信封签发面二义）→ 拒绝装配
        // 之外的处理不可行（构造无 Result），此处 panic 与装配期静态
        // 校验同律——错误装配在首次构造即暴露，不带病运行。
        let mut routes = alloc::vec::Vec::with_capacity(1 + config.extra_routes.len());
        routes.push(AssetRoute {
            asset_id: 0, // 默认路由（历史单资产形态；asset id 未入身份束）
            profile: config.issuer.profile,
            chain: config.issuer.chain,
        });
        for r in config.extra_routes {
            assert!(
                routes
                    .iter()
                    .all(|e| e.profile.device_id != r.profile.device_id
                        || e.profile.point_id != r.profile.point_id),
                "extra route duplicates (device,point): ({}, {})",
                r.profile.device_id,
                r.profile.point_id
            );
            routes.push(r);
        }
        let mut engine = Self {
            resources: config.resources,
            contracts: config.contracts,
            compiler_policy: config.compiler_policy,
            arbiter: crate::arbiter::CommandArbiter::new(256),
            control_mode: config.control_mode,
            key: config.issuer.key,
            routes,
            fencing_token: config.issuer.fencing_token,
            issuer_service: config.issuer.issuer_service,
            issuer_epoch: config.issuer.issuer_epoch,
            target_endpoint: config.delivery_binding.target_endpoint,
            max_power_kw: config.safety.max_power_kw,
            safety_voltage_window: config.safety.safety_voltage_window,
            safety_freq_window: config.safety.safety_freq_window,
            safety_ramp_kw_per_min: config.safety.safety_ramp_kw_per_min,
            gate: config.gate,
            audit: config.audit_sinks.audit,
            control_audit: config.audit_sinks.control_audit,
            intent_log: config.audit_sinks.intent_log,
            delivery: config.delivery_binding.delivery,
            // 与 durable 槽位同阶的有界执行 journal（61 v0.314.24）。
            // 蓝图 70.1 WP-70.1.1/70.1.3（I4 字节预算）：HEAP-WM 实弹曲线
            // （evidence/70/70.1 归因窗）显示 cap=256 时 journal 随逐条
            // IdempotencyRecord 入账把 svc-control-dispatch 堆推到 ~423KB
            // 平台期（512KiB 堆余量 <100KB，~850 轮分配抖动后碎裂到无
            // 512B 连续块 → 512B 申请 OOM）。cap 收至 128：幂等/重放窗
            // 减半（在途非终态不受逐出影响——Full 背压语义保持），堆
            // 平台期回落至 ~300KB 级、余量 ≥180KB。
            journal: ExecutionJournal::new(128),
            feedback_builder: TrustedFeedbackBuilder::new(config.delivery_binding.feedback_mapping),
            now_fn: config.now_fn,
            trusted_input: config.trusted_input,
            certifier: config.certifier,
            certifier_ring: config.certifier_ring,
            seq: 0,
            committed_count: 0,
            last_transition_proof_digest: None,
            audit_drop_count: 0,
            audit_append_count: 0,
            last_seq: BTreeMap::new(),
            last_confirmed: BTreeMap::new(),
            held_plans: None,
            held_store_unavailable: false,
            input_retainer: None,
            released_holds: alloc::vec::Vec::new(),
            held_done_logged: alloc::vec::Vec::new(),
            held_pin_failures: 0,
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

    /// 蓝图 70 R10：按 (device,point) 解析执行路由索引（点表协议映射
    /// 键；未注册目标 → None——调用方 fail-closed 拒绝）。
    fn route_index_for(&self, device: u32, point: u32) -> Option<usize> {
        self.routes
            .iter()
            .position(|r| r.profile.device_id == device && r.profile.point_id == point)
    }

    // 蓝图 63 WP-63.1.2：观测/可写分离——以下只读观测面是外部读这六个
    // 字段的唯一入口（字段已私有）。可写面唯一编排点 = `settle_command`
    // / `push_stage`（写序承诺从纪律变结构：任何 `intent_log` 写/`gate`
    // 记账/计数递增都只能经结算路径发生）。

    /// Committed 次数（观测）。
    pub fn committed_count(&self) -> u64 {
        self.committed_count
    }

    /// 最近一次过渡认证发放的 proof digest（蓝图 66 WP-66.2.2 观测面）：
    /// Gateway 侧持钥验证读点——与 keyed::hmac_sha256(key,
    /// "eneros.transition-proof.v2" ‖ plan_wire_sha256) 逐字节比对；
    /// 未发证为 None。生产读者随 certifier 接电接入（ADR-051）。
    pub fn last_transition_proof_digest(&self) -> Option<[u8; 32]> {
        self.last_transition_proof_digest
    }

    /// 审计链成功入链条数（AUDIT-CHAIN 串口行 appended 依据）。
    pub fn audit_append_count(&self) -> u64 {
        self.audit_append_count
    }

    /// 审计丢弃计数。
    pub fn audit_drop_count(&self) -> u64 {
        self.audit_drop_count
    }

    /// 已记账能量（DecisionGate 只读委托）。
    pub fn gate_used_kwh(&self) -> f64 {
        self.gate.used_kwh()
    }

    /// 被拒能量计数（DecisionGate 只读委托；计数单位见 gate）。
    pub fn gate_rejected_energy(&self) -> u64 {
        self.gate.rejected_energy()
    }

    /// 决策审计只读视图（链校验/条目观测）。
    pub fn audit_log(&self) -> Option<&DecisionAudit> {
        self.audit.as_ref()
    }

    /// durable intent 只读查询（对账）。
    pub fn find_intent(&self, id: &[u8; 16]) -> Option<IntentRecord> {
        self.intent_log.find(id)
    }

    /// durable intent 未决集（对账）。
    pub fn pending_intents(&self) -> Vec<IntentRecord> {
        self.intent_log.pending()
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
    /// 迁移期保留：PlanTyped correlation 派生的历史参照（adapter 测试
    /// 以同规则锁定字节兼容）。.32 facade reader 删除时一并清偿。
    #[allow(dead_code)]
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
    /// WP-5.3（[10] §4.1）：投递前唯一 Arbiter 裁决。迁移期映射：
    /// Reject ⇒ NotAuthorized（零投递）；MergeInSolver ⇒ ProposalInvalid
    /// （可叠加服务回 solver）；Preempt ⇒ 对账完成前拒绝新写。新意图的
    /// 幂等键与在途命令不同，不能用同键去重冒充抢占前对账。
    fn arbitrate(
        &mut self,
        source: crate::arbiter::CommandSource,
        valid_now: bool,
        device_id: u32,
    ) -> Result<(), DispatchError> {
        self.arbitrate_owned(source, valid_now, device_id, None)
    }

    fn arbitrate_owned(
        &mut self,
        source: crate::arbiter::CommandSource,
        valid_now: bool,
        device_id: u32,
        owner: Option<eneros_decision_contract::PlanId>,
    ) -> Result<(), DispatchError> {
        if self.held_device_busy(device_id, owner)? {
            return Err(DispatchError::NotAuthorized(String::from(
                "arbiter: device owned by held plan",
            )));
        }
        let in_flight = !self.journal.pending().is_empty();
        if in_flight && self.arbiter.in_flight_source(device_id).is_none() {
            // Boot recovery rebuilds the journal, but cannot prove the old
            // source/device scope from legacy records. An empty arbitration
            // map must not turn that uncertainty into permission for a new key.
            return Err(DispatchError::NotAuthorized(String::from(
                "arbiter: reconcile required for recovered or unscoped pending intent",
            )));
        }
        if !in_flight {
            // journal 真值自愈：无非终态记录 = 在途已收敛，清登记。
            self.arbiter.clear_in_flight(device_id);
        }
        let req = crate::arbiter::ArbitrationRequest {
            source,
            device_id,
            lease_valid: true, // 租约/权限由 12 步链 Gateway 全查（纵深）
            valid_now,
            preemptible_target: true,
            in_flight_sent_or_unknown: in_flight,
        };
        let ctx = crate::arbiter::ResourceContext {
            mode: self.control_mode,
            saga_in_flight: in_flight,
            fresh_snapshot_after_manual: true,
        };
        match self.arbiter.decide(&req, &ctx) {
            crate::arbiter::Decision::Proceed => {
                self.arbiter.register_in_flight(device_id, source);
                Ok(())
            }
            crate::arbiter::Decision::Preempt(_, losers) => {
                // Preserve the existing owner until the unknown physical effect
                // has actually been reconciled. Priority alone is not evidence.
                Err(DispatchError::NotAuthorized(alloc::format!(
                    "arbiter: reconcile required before preempting {:?}",
                    losers.first().map(|l| l.source)
                )))
            }
            crate::arbiter::Decision::Reject(losers) => {
                Err(DispatchError::NotAuthorized(alloc::format!(
                    "arbiter: {}",
                    losers
                        .first()
                        .map(|l| l.reason.as_str())
                        .unwrap_or("rejected")
                )))
            }
            crate::arbiter::Decision::MergeInSolver(losers) => {
                Err(DispatchError::ProposalInvalid(alloc::format!(
                    "arbiter: {}",
                    losers.first().map(|l| l.reason.as_str()).unwrap_or("merge")
                )))
            }
        }
    }

    // 蓝图 69.4 WP-69.4.4：`DispatchEngine::resume_auto` 已删（全仓零
    // 外部调用方——唯一命中是自身转调与 `Arbiter` 同名方法的测试）。
    // 人工→自动切换在生产上只发生在 `Arbiter` 层
    // （`CommandArbiter::resume_auto` 持 fresh snapshot/capability，
    // `tests/arbiter_degrade.rs` 覆盖）；`DispatchEngine.control_mode`
    // 由此成为装配期一次性设定、运行期不变的字段（卷内裁决 ②）。

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

        // 步骤 1：意图校验——目标 (device,point) 必须命中装配绑定注册表
        // （WP-47.2 点表绑定 + 蓝图 70 R10 多资产路由；防协议层越点写）
        // + 值域有限。未注册目标 = 未知资产意图，一律拒绝（fail-closed）。
        let route_idx = self.route_index_for(intent.device_id as u32, intent.point_id);
        let Some(route_idx) = route_idx else {
            return Err(DispatchError::NotAuthorized(format!(
                "intent target device={} point={} 未绑定本 dispatch（绑定注册表无此 (device,point) 路由）",
                intent.device_id, intent.point_id
            )));
        };
        // WP-5.3（[10] §4.1）：协议入站写同经唯一 Arbiter（调度机构类；
        // 在途登记按意图目标设备——多资产各键独立）。
        self.arbitrate(
            crate::arbiter::CommandSource::GridOperatorConfirmed,
            true,
            u32::from(intent.device_id),
        )?;

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
                plan_sha: None,
                idem_note: String::from("idempotent-hit（同 (protocol,seq) 重放——不重复写）"),
            },
            telemetry,
            actor,
            started,
            states,
            deadline_ms,
            route_idx,
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

        // 步骤 1：proposal 校验。
        self.validate_proposal(proposal)?;

        // 步骤 2：telemetry 执行时刻新鲜度。
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }

        // 蓝图 61 v0.314.28：**typed 链收口**——proposal 在 dispatch 边界
        // 经严格 adapter 进入 CandidatePlan（[17] .28"经严格 adapter 进入
        // CandidatePlan"），合核只消费 typed 产物（含 PlanTyped 身份）。
        // 旧 facade reader（70B wire 读臂）已于 v0.329.0 删除（蓝图 69.4）。
        // 蓝图 70 R10：legacy 单资产 facade 绑定默认路由（routes[0]）。
        let plan = crate::adapter::proposal_to_candidate(
            proposal,
            (
                u64::from(self.routes[0].profile.device_id),
                u16::try_from(self.routes[0].profile.device_id).unwrap_or(0),
                self.routes[0].profile.point_id,
                0,
            ),
            eneros_telemetry_model::unit_catalog::code_of(self.routes[0].profile.unit),
            1, // catalog_version（.35 registry 落地前的 profile 单源常量）
        )?;
        let energy = schedule_energy_kwh(&proposal.schedule);
        self.run_typed(
            &plan,
            proposal.target_device.clone(),
            Some(energy),
            telemetry,
            actor,
            deadline_ms,
            None,
        )
    }

    /// 蓝图 61 v0.314.28：typed 入口——CandidatePlan → 过渡认证证书 →
    /// **PlanCompiler 命令批**（WP-5.1，v0.314.41）→ 逐命令 12 步合核。
    ///
    /// [10] §2.7 出口 gate：生产无「取计划第一条 power_kw」路径——全部
    /// assignments 经 `compiler::compile` 确定性编译（计划-证书绑定校验、
    /// 单位换算走 UnitCatalog、分辨率合并留「未动作」证据、同设备串行
    /// DAG、compiler/batch 双摘要），再逐意图进入唯一 Gateway 执行面
    /// （身份 = PlanCommand：sub_index 0 沿用 PlanId 幂等键，Phase 0
    /// 单 PCS 记录兼容）。批内任一命令失败即阻断后继（Saga 收敛归
    /// WP-5.2）。本函数不是第二执行真相：投递只经 `AuthorizedCommand`。
    ///
    /// 63 WP-63.1.3：`states` 恒为 `[Proposed, Validated]`（两调用点
    /// 同构构造、本体内不增删——只 `.clone()` 传入逐命令 execute_core），
    /// 改为体内派生；`started` 两调用点均 `(self.now_fn)()` 现算现传
    /// （μs 级），同样改体内派生。参数 8 → 6（删除
    /// `#[allow(clippy::too_many_arguments)]`）。
    #[allow(clippy::too_many_arguments)] // 蓝图 70.8 WP-70.8.4：新增可信
                                         // 输入事实入参（认证准入前置判据）——参数成束重构留待 70.11 计划持有
                                         // 模块落地时统一收拢（届时 trusted/energy/telemetry 归入单一请求结构）。
    fn run_typed(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        target_device: String,
        energy_kwh: Option<f64>,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
        trusted: Option<&eneros_agent_sdk::snap_input_wire::TrustedInput>,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        if !plan.is_valid_at(started) {
            return Err(DispatchError::ProposalInvalid(String::from("计划已过期")));
        }
        // 蓝图 70.8 WP-70.8.4：**可信输入准入**。入口只验证、绝不改写
        // 候选身份——不匹配即整批拒绝（零投递）。
        self.admit_trusted_input(plan, trusted.map(|input| input.facts()), started)?;
        // 63 WP-63.1.3：states 恒为推进链起点（原调用点同构构造，本体内
        // 不增删）。
        let states: Vec<DispatchState> =
            alloc::vec![DispatchState::Proposed, DispatchState::Validated];

        // —— 过渡认证（[06] §3.5.4：dispatch 入口 CertifiedPlan 路由）——
        // 未绑定分配（asset 0）解析：**认证与编译之前一次性归一**——证书
        // 与命令批绑定同一份计划 wire（只替换占位资产，身份字段不动）。
        let resolved = crate::adapter::resolve_unbound_assets(plan, self.default_asset_id())?;
        let plan = &resolved;
        // —— 独立认证（蓝图 70.9 WP-70.9.1/70.9.4）——
        // plan_sha256 = SHA-256(CandidatePlan V1 wire)（派生单源 =
        // decision-contract `plan_wire_sha256`）。证书**只**来自装配的
        // 独立认证来源（`CertificateSource`）；过渡发放面
        // （`transition-certifier` / `CertifierSeal::issue_transition`）
        // 已按 ADR-051 出口条件删除——本函数无任何本地发放分支。
        let plan_wire = plan.encode_v1();
        let plan_sha: [u8; 32] = eneros_decision_contract::CandidatePlan::plan_wire_sha256(plan);
        let reply = self
            .certifier
            .certify(&plan_wire, started)
            .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("certifier: {e}")))?;
        // 验方环：认证者自带环时以它为准（轮换/退役即刻生效），否则用装配
        // 在引擎侧的环（测试双/无环来源）。
        let certificate = match self.certifier.verifier_ring() {
            Some(ring) => crate::certify::verify_reply(plan, &plan_wire, &reply, ring, started),
            None => crate::certify::verify_reply(
                plan,
                &plan_wire,
                &reply,
                &self.certifier_ring,
                started,
            ),
        }
        .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("certify-reject: {e}")))?;
        // 观测面：最近一次独立认证的 proof digest（Gateway/审计读点；
        // 与证书内证据同一字段，供证据行比对）。
        self.last_transition_proof_digest = Some(*certificate.proof_digest());
        eneros_runtime::println!(
            "[svc-control-dispatch] CERTIFIED plan={:02x?} cert={:02x?} constraints={} key={}/{}",
            &plan_sha[..4],
            &certificate.certificate_id().0[..4],
            certificate.hard_constraint_count(),
            reply.key_id,
            reply.key_version
        );

        // —— PlanCompiler：全 assignments → 确定性命令批（WP-5.1）——
        // 已确认基线（分辨率合并参照）：(device,point) → 最近 AppliedConfirmed
        // 值，经绑定换算到资产。
        let applied = |asset_id: u64| -> Option<f64> {
            let binding = self.resources.asset_control_binding(asset_id)?;
            self.last_confirmed
                .get(&(u32::from(binding.0), binding.1))
                .map(|&(v, _)| v)
        };
        let outcome = crate::compiler::compile(
            &certificate,
            plan,
            self.resources.as_ref(),
            self.contracts.as_ref(),
            &applied,
            &self.compiler_policy,
            started,
            u64::from(self.routes[0].profile.device_id),
        )
        .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("compile: {e}")))?;

        // 批指纹（观测/证据行：compiler/batch 双摘要 + 未动作证据数）。
        let first_digest = outcome
            .batches
            .first()
            .map(|b| *b.batch_digest())
            .unwrap_or([0u8; 32]);
        eneros_runtime::println!(
            "[svc-control-dispatch] PLAN-COMPILED batches={} intents={} skipped={} manual={} compiler={:02x?} batch={:02x?}",
            outcome.batches.len(),
            outcome
                .batches
                .iter()
                .map(|b| b.intents().len())
                .sum::<usize>(),
            outcome.skipped.len(),
            outcome.manual_approval.len(),
            &outcome.compiler_digest[..4],
            &first_digest[..4],
        );

        // An irreversible command needs an explicit approval capability. The
        // automatic plan entry has none: reject the entire batch before writes.
        if !outcome.manual_approval.is_empty() {
            return Err(DispatchError::NotAuthorized(String::from(
                "manual approval required for irreversible plan commands",
            )));
        }

        // 装配预检（fail-closed，不半执行）：全部意图的 (device,point)
        // 必须命中绑定注册表路由（蓝图 70 R10 多资产；未注册目标拒绝，
        // 不半执行）+ 全局功率上限护栏。
        for batch in &outcome.batches {
            for intent in batch.intents() {
                if self
                    .route_index_for(u32::from(intent.device_id()), intent.point_id())
                    .is_none()
                {
                    return Err(DispatchError::ProposalInvalid(alloc::format!(
                        "compile: intent device {} point {} 未绑定本 dispatch（绑定注册表无此路由）",
                        intent.device_id(),
                        intent.point_id()
                    )));
                }
                if intent.power_kw().abs() > self.max_power_kw {
                    return Err(DispatchError::ProposalInvalid(alloc::format!(
                        "power_kw {} 超上限 {}",
                        intent.power_kw(),
                        self.max_power_kw
                    )));
                }
            }
        }

        // The durable entry owns every future step. A production multi-command
        // plan must not fall through to the legacy caller-driven replay loop.
        if plan.step_count() > 1
            || outcome
                .batches
                .iter()
                .map(|b| b.intents().len())
                .sum::<usize>()
                > 1
        {
            if self.held_plans.is_some() {
                let input = trusted.ok_or_else(|| {
                    DispatchError::NotAuthorized(String::from("held plan requires trusted input"))
                })?;
                let horizon_end = plan.horizon_start_ms().saturating_add(
                    plan.step_ns().saturating_mul(u64::from(plan.step_count())) / 1_000_000,
                );
                if horizon_end > telemetry.valid_until_ms
                    || horizon_end > certificate.valid_until_ms()
                    || outcome
                        .batches
                        .iter()
                        .flat_map(|b| b.intents())
                        .any(|i| i.valid_until_ms() > telemetry.valid_until_ms)
                {
                    return Err(DispatchError::ProposalInvalid(String::from(
                        "held horizon exceeds trusted telemetry",
                    )));
                }
                return self.accept_held_plan(
                    plan,
                    reply,
                    input,
                    telemetry,
                    actor,
                    &outcome.batches,
                );
            }
            if matches!(self.trusted_input, TrustedInputPolicy::Required) {
                return Err(DispatchError::IntentPersistFailed(String::from(
                    "held plan storage unavailable",
                )));
            }
        }

        // WP-5.3（[10] §4.1）：投递前唯一 Arbiter 裁决——按批内涉备逐一
        // 裁决并登记在途（蓝图 70 R10 多资产：在途登记按设备键，各键
        // 独立成立；任一设备拒绝 = 整批不投递，语义与单资产一致）。
        let mut involved: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        for batch in &outcome.batches {
            for intent in batch.intents() {
                let d = u32::from(intent.device_id());
                if !involved.contains(&d) {
                    involved.push(d);
                }
            }
        }
        let legacy_single = crate::compiler::is_legacy_single_command_plan(plan);
        // A repeated partial batch must return its unresolved receipt before
        // attempting to reserve devices belonging to still-unsent successors.
        let mut unresolved = None;
        for intent in outcome.batches.iter().flat_map(|batch| batch.intents()) {
            let key = crate::compiler::plan_command_correlation(
                plan.id(),
                intent.intent_id(),
                legacy_single,
            );
            let digest = eneros_protocol_abstract::write::semantic_command_digest(
                u32::from(intent.device_id()),
                intent.point_id(),
                CommandValue::ActivePowerKw(intent.power_kw()),
            );
            match self.journal.lookup(&key, &digest) {
                Some(JournalHit::Conflict) => {
                    return Err(DispatchError::IdempotencyConflict(String::from(
                        "batch replay semantic mismatch",
                    )))
                }
                Some(JournalHit::Current(record)) if !record.status.is_terminal() => {
                    unresolved.get_or_insert(record);
                }
                _ => {}
            }
        }
        if let Some(record) = unresolved {
            return Ok(Self::receipt_from_journal(
                &record,
                record.command_id,
                record.key,
                started,
                (self.now_fn)(),
                "batch replay: reconciliation pending",
            ));
        }
        for &d in &involved {
            // A known command may query its journal while the resource is held.
            // This bypass is only for matching semantic digests; a new command
            // still goes through arbitration and cannot overwrite an unknown effect.
            let replay_only = outcome
                .batches
                .iter()
                .flat_map(|b| b.intents())
                .filter(|i| u32::from(i.device_id()) == d)
                .all(|i| {
                    let key = crate::compiler::plan_command_correlation(
                        plan.id(),
                        i.intent_id(),
                        legacy_single,
                    );
                    let digest = eneros_protocol_abstract::write::semantic_command_digest(
                        d,
                        i.point_id(),
                        eneros_telemetry_model::command::CommandValue::ActivePowerKw(i.power_kw()),
                    );
                    matches!(
                        self.journal.lookup(&key, &digest),
                        Some(JournalHit::Current(_))
                    )
                });
            if !replay_only {
                self.arbitrate(crate::arbiter::CommandSource::SiteOptimizer, true, d)?;
            }
        }

        // —— 逐命令合核（批序 = 编译序；失败阻断后继）——
        // not_before 语义：未来步不当拍投递（留待到期 tick——键已由编译
        // 确定性分配，重编译 + journal 幂等命中 = 无重复投递）；已过期步
        // 跳过并留证据行。
        //
        // **未到期步不是静默丢弃**（2026-09-14）：旧实现对未到期步直接
        // `continue`（零日志零计数），与过期步的 `PLAN-COMMAND-SKIP-EXPIRED`
        // 不对称——于是"多步计划只走了首步"在串口证据里**不可见**
        // （REG-70.FL-MULTISTEP-REPLAY）。现补对称证据行 + 计数：
        // 延期步数逐次可对账，投递面是否真有后续 tick 来接手，
        // 从"看不出来"变成"看得出"。
        let mut delivered_any = false;
        let mut last_receipt = None;
        let mut deferred_steps = 0u32;
        let mut expired_steps = 0u32;
        for batch in &outcome.batches {
            for intent in batch.intents() {
                let now = (self.now_fn)();
                if now < intent.valid_from_ms() {
                    deferred_steps = deferred_steps.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-control-dispatch] PLAN-COMMAND-DEFERRED intent={:02x?} due_in_ms={}",
                        &intent.intent_id().0[..4],
                        intent.valid_from_ms().saturating_sub(now),
                    );
                    continue;
                }
                if now >= intent.valid_until_ms() {
                    expired_steps = expired_steps.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-control-dispatch] PLAN-COMMAND-SKIP-EXPIRED intent={:02x?}",
                        &intent.intent_id().0[..4],
                    );
                    continue;
                }
                let correlation = crate::compiler::plan_command_correlation(
                    plan.id(),
                    intent.intent_id(),
                    legacy_single,
                );
                // typed 路径能量预算估算（WP-64.3.2.3 单源）。
                let (window_secs, energy_est) = intent_window_estimate(intent, now, plan.ttl_ms());
                let energy = energy_kwh.or(Some(energy_est));
                // 蓝图 70 R10：逐命令按其绑定路由执行（预检已保证命中）。
                let route_idx = self
                    .route_index_for(u32::from(intent.device_id()), intent.point_id())
                    .expect("预检已保证 (device,point) 命中路由表");
                let receipt = self.execute_core(
                    CoreCommand {
                        identity: CoreIdentity::PlanCommand { correlation },
                        ttl_secs: window_secs.max(1),
                        power_kw: intent.power_kw(),
                        target_device: target_device.clone(),
                        input_digest: Some(*plan.input_digest()),
                        energy_kwh: energy,
                        plan_sha: Some(plan_sha),
                        idem_note: String::from("idempotent-hit（原结果，不重复写）"),
                    },
                    telemetry,
                    actor,
                    started,
                    states.clone(),
                    deadline_ms,
                    route_idx,
                )?;
                delivered_any = true;
                // 物理结果未知/拒绝是 Ok(receipt)，并不等于命令成功。
                // 必须阻断后继，不能让末条 Committed 覆盖前项失败；未知
                // 目标保留仲裁在途登记，其他尚未投递目标释放登记。
                if !receipt.final_state.is_committed() {
                    for &d in &involved {
                        if d != u32::from(intent.device_id())
                            || receipt.final_state != DispatchState::Indeterminate
                        {
                            self.arbiter.clear_in_flight(d);
                        }
                    }
                    return Ok(receipt);
                }
                last_receipt = Some(receipt);
            }
        }
        // 延期/过期步数一次性对账（0/0 也打印：缺行与"无延期"必须可区分）。
        eneros_runtime::println!(
            "[svc-control-dispatch] PLAN-COMMANDS-DEFERRED deferred={} expired={} delivered={}",
            deferred_steps,
            expired_steps,
            u32::from(delivered_any),
        );
        // 蓝图 70 R10：在途登记按涉备逐一清账（与仲裁登记同键集）。
        for &d in &involved {
            self.arbiter.clear_in_flight(d);
        }
        if !delivered_any {
            return Err(DispatchError::ProposalInvalid(alloc::format!(
                "计划无到期命令（未来步留待到期 tick）：deferred={deferred_steps} expired={expired_steps}"
            )));
        }
        Ok(last_receipt.expect("delivered_any ⇒ 非 None"))
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
    /// 蓝图 70 R10：route_idx 入参（多资产执行路由；参数 7→8——成束重构
    /// 与 run_typed 同律，归 70.11 计划持有模块落地时统一收拢）。
    #[allow(clippy::too_many_arguments)]
    fn execute_core(
        &mut self,
        cmd: CoreCommand,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        started: u64,
        mut states: Vec<DispatchState>,
        deadline_ms: u64,
        route_idx: usize,
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
            CoreIdentity::PlanCommand { correlation } => {
                self.seq += 1;
                let mut cid = [0u8; 16];
                cid[..8].copy_from_slice(&self.seq.to_le_bytes());
                (cid, correlation, self.seq)
            }
        };
        let now_secs = started / 1000;
        // 蓝图 70 R10：本命令的执行路由（预检/准入已保证命中）——信封、
        // 语义摘要、爬坡基线、SBO 与链复验全部按此路由的 (device,point)
        // 与链实例执行（每资产独立链，安全语义零放宽）。
        let (route_device, route_point) = {
            let p = &self.routes[route_idx].profile;
            (p.device_id, p.point_id)
        };
        // 61 v0.314.25 §3.5.8：`power_kw → On/Off` 映射删除——连续功率以
        // ActivePowerKw setpoint 授权与执行（不再伪装成开关）。
        let value = CommandValue::ActivePowerKw(cmd.power_kw);

        // Envelope 装配+签名+幂等键（唯一实现）：幂等键=correlation——
        // 同计划重放 / 同 (protocol,seq) 跨重启重放均返回原结果，不重复写。
        // 61 v0.314.24：ControlEnvelope 提前构建（纯装配，无副作用）——
        // journal 预检需要命令摘要；投递复用同一实例。
        let buf = {
            let route = &self.routes[route_idx];
            EnvelopeBuilder::new(
                command_id,
                route.profile.device_id,
                route.profile.point_id,
                ControlCommand::Single(SingleCommand::On), // legacy 占位；value 覆盖
                now_secs,
                now_secs.saturating_add(cmd.ttl_secs),
                self.issuer_service,
            )
            .lease(route.profile.lease_id, self.fencing_token)
            .policy_version(route.profile.policy_version)
            .sequence(sequence)
            .actor_kind(ActorKind::Ai)
            .idempotency_key(correlation)
            .value(value)
            .build(&self.key)
            .encode()
        };
        let expires_ms = started.saturating_add(cmd.ttl_secs * 1000);
        let envelope = ControlEnvelope::new_v3(
            correlation,
            self.issuer_service,
            self.issuer_epoch,
            started,
            expires_ms,
            &buf,
        );
        // journal 语义摘要（61 v0.314.25）：SHA-256("eneros.command-semantic.v1"
        // ‖ device ‖ point ‖ value_tag ‖ value_bits)——只含同 key 重传的
        // 不变语义内容（不含 sequence/时间戳，防合法重放误判冲突）。
        // AuthorizedCommand.command_digest（canonical V3 头摘要）另由
        // gateway 签发，供 writer 绑定验证——两者用途不同、并存不冲突。
        let command_digest = eneros_protocol_abstract::write::semantic_command_digest(
            route_device,
            route_point,
            value,
        );

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

        // 复查 FIX（61 §3.4）：单点连续 setpoint 爬坡——与最近
        // AppliedConfirmed setpoint 比较，真实 elapsed 换算（合同
        // max_ramp_per_sec）；超限拒绝（零投递）。首命令无参照放行
        // （跨重启基线不持久——归对账/点表，Phase 1 收编）。
        if let CommandValue::ActivePowerKw(v) = value {
            if let Some(&(last_v, last_ms)) = self.last_confirmed.get(&(route_device, route_point))
            {
                let dt_secs = ((self.now_fn)().saturating_sub(last_ms)) as f64 / 1_000.0;
                // dt 下限 1s：同拍/倒挂重发不放大许可，也不除零。
                let rate = (v - last_v).abs() / dt_secs.max(1.0);
                let limit = self.routes[route_idx]
                    .profile
                    .control_contract()
                    .map(|c| c.max_ramp_per_sec)
                    .unwrap_or(f64::INFINITY);
                if rate > limit {
                    let note = alloc::format!(
                        "ramp-limit：|{v}-{last_v}|kW/{:.3}s = {rate:.3} kW/s 超合同限 {limit:.3} kW/s——拒绝（零投递）",
                        dt_secs
                    );
                    self.audit(ExecOutcome::Failed, SafetyOutcome::Passed, &note, started);
                    return Err(DispatchError::SafetyRejected(note));
                }
            }
        }

        // SBO：选择+执行原子（ControlGateway 九步链，不双头授权）。
        // 蓝图 70 R10：选择键与链实例均取本命令路由（每资产独立）。
        let fb: FeedbackBundle = self.feedback_builder.build(telemetry, now_secs);
        let chain_outcome = {
            let route = &mut self.routes[route_idx];
            route.chain.sbo_select(route.profile.point_id, now_secs);
            route.chain.execute(&buf, now_secs, &fb)
        };
        match chain_outcome {
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

        // 63 WP-63.1.3：前置结算不变参 ctx（前置拒绝路径 energy=0.0/
        // setpoint=None/response=None——与既有各调用点逐字一致）。
        let pre_ctx = ExecutionCtx {
            started,
            command_id,
            correlation,
            response: None,
            energy: 0.0,
            setpoint_kw: None,
            target: (route_device, route_point),
        };

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
                // 61 v0.314.24：授权后未投递即终局 → journal Failed（表驱动）。
                let receipt = self.settle_command(
                    SettleTrigger::SafetyReject {
                        reason: reason.clone(),
                    },
                    &mut states,
                    &pre_ctx,
                );
                #[cfg(target_os = "none")]
                eneros_runtime::println!("[svc-control-dispatch] SAFETY-REJECTED reason={reason}");
                return receipt;
            }
        }

        // 步骤 5：rate 预算 reserve（不 record）；energy 仅 AI 计划路径有语义。
        if self.gate.check_rate(started) != GateDecision::Allow {
            return self.settle_command(SettleTrigger::BudgetRate, &mut states, &pre_ctx);
        }
        if let Some(energy) = cmd.energy_kwh {
            if self.gate.check_energy(started, energy) != GateDecision::Allow {
                return self.settle_command(SettleTrigger::BudgetEnergy, &mut states, &pre_ctx);
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
            plan_sha256: cmd.plan_sha,
        };
        // 61 v0.314.24：journal append 失败时不投递（记录保持 Authorized
        // 未决——重启后对账，不会误判为已投递）。
        if let Err(e) = self.intent_log.append(&rec) {
            // 61 v0.314.43（Phase 5 复查）：append 失败 → journal 授权记录
            // 终局 Failed——「授权后未投递即终局」与安全/预算拒绝同律
            // （记录从未落持久面，重启后本就不存在，Authorized 未决只是
            // 进程内垃圾）。原实现遗留悬空 Authorized 非终态：同 key 重放
            // 恒返回 Authorized 零投递（命令永不可执行）、pending() 恒
            // 非空 → WP-5.3 Arbiter 在途登记自愈失效，同源续跑被
            // MergeInSolver 永久锁死（[10] §3.6 磁盘满故障矩阵）。
            return self.settle_command(
                SettleTrigger::IntentAppendFailed {
                    err: alloc::format!("{e:?}"),
                },
                &mut states,
                &pre_ctx,
            );
        }
        if let Err(err) = self.push_stage(
            SettleStage::JournalPersisted,
            &command_id,
            &correlation,
            started,
        ) {
            return self.settle_command(
                SettleTrigger::JournalPersistFailed { err },
                &mut states,
                &pre_ctx,
            );
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
        let setpoint_kw = match value {
            CommandValue::ActivePowerKw(v) => Some(v),
            _ => None,
        };
        // 63 WP-63.1.3：投递路径 ctx（能量 = 命令真实能量——与投递前拒绝
        // 路径的 0.0 不同，`pre_ctx` 不可复用）。
        let deliver_ctx = ExecutionCtx {
            started,
            command_id,
            correlation,
            response: None,
            energy: cmd.energy_kwh.unwrap_or(0.0),
            setpoint_kw,
            target: (route_device, route_point),
        };
        self.deliver_and_settle(response, states, &deliver_ctx)
    }

    /// 步骤 7-12 收尾（run/run_intent 共享）：投递 ControlEnvelope 并分类回执。
    /// 前置失败零投递；投递后超时/结果不明 → `Indeterminate`，不猜成功。
    ///
    /// 61 v0.314.24：journal 写序 `Sent → TransportAcked → AppliedConfirmed`
    /// 全程 expected-previous；投递后任何 journal/持久化失败 →
    /// `Indeterminate`（对账），不伪造成功或失败。`Committed` 只能从
    /// `AppliedConfirmed`（对端独立回读匹配）映射。
    ///
    /// 62 v0.315.0 WP-62.1.3：全部分类臂改调结算决策表（`settle_command`/
    /// `push_stage`）——结局语义单点，本函数只保留流程分类。
    /// 63 WP-63.1.3：六不变参收进 [`ExecutionCtx`]（`#[allow(too_many_
    /// arguments)]` 删除）。
    fn deliver_and_settle(
        &mut self,
        response: Result<Vec<u8>, crate::ChannelError>,
        mut states: Vec<DispatchState>,
        ctx: &ExecutionCtx<'_>,
    ) -> Result<DispatchReceipt, DispatchError> {
        // 63 WP-63.1.3：投递未/已到位两段 ctx（非变异——delivered_ctx 是
        // base_ctx 的结构更新拷贝；帧绑定在 shadow 后存在）。
        let base_ctx = *ctx;
        let started = base_ctx.started;
        let command_id = base_ctx.command_id;
        let correlation = base_ctx.correlation;
        // 步骤 7：投递已由调用方完成（envelope 参数）；此处直接分类回执。
        let response = match response {
            Ok(r) => {
                // 61 v0.314.24：投递返回 Ok → journal Sent + durable
                // 「已发送」落盘。任一失败 → Indeterminate（写可能已发生）。
                if let Err(err) =
                    self.push_stage(SettleStage::JournalSent, &command_id, &correlation, started)
                {
                    return self.settle_command(
                        SettleTrigger::SentAdvanceFailed { err },
                        &mut states,
                        &base_ctx,
                    );
                }
                if self
                    .push_stage(
                        SettleStage::IntentDelivered,
                        &command_id,
                        &correlation,
                        started,
                    )
                    .is_err()
                {
                    return self.settle_command(
                        SettleTrigger::MarkDeliveredFailed,
                        &mut states,
                        &base_ctx,
                    );
                }
                r
            }
            Err(crate::ChannelError::Timeout) => {
                // 投递后超时：写可能已发生——Indeterminate 对账，不猜成功。
                return self.settle_command(SettleTrigger::DeliveryTimeout, &mut states, &base_ctx);
            }
            Err(crate::ChannelError::Unreachable) => {
                // 通道不可达 = 未投递（可证明）→ 终局 Failed。
                return self.settle_command(
                    SettleTrigger::DeliveryUnreachable,
                    &mut states,
                    &base_ctx,
                );
            }
            Err(e) => {
                // 61 v0.314.24（对抗审查 R7）：deliver 已被调用、信封可能已
                // 到达对端（生产 EIPC 通道的发送失败与回执丢失同报 Protocol）
                // ——一律 Indeterminate 对账，不假失败、不假成功。
                // 62 v0.315.0 WP-62.1.1：原此处另有同语义 `if Unreachable`
                // 复制残骸（R7 两轮补丁叠加产物；match 顺序匹配保证 `e`
                // 到达本臂时不可能为 Unreachable——专门臂已承接），删除。
                return self.settle_command(
                    SettleTrigger::DeliveryError {
                        err: alloc::format!("{e:?}"),
                    },
                    &mut states,
                    &base_ctx,
                );
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
                return self.settle_command(
                    SettleTrigger::ResponseDecodeFailed {
                        err: alloc::format!("{e:?}"),
                    },
                    &mut states,
                    &base_ctx,
                );
            }
        };
        // 61 v0.314.24：ACK 帧已解码 → journal TransportAcked；失败 → 对账。
        if let Err(err) = self.push_stage(
            SettleStage::JournalTransportAcked,
            &command_id,
            &correlation,
            started,
        ) {
            return self.settle_command(
                SettleTrigger::AckAdvanceFailed { err },
                &mut states,
                &base_ctx,
            );
        }
        states.push(DispatchState::TransportAcked);
        // 帧已解码——结算 ctx 携带回执原文（sm3 摘要供给；两段式第二段）。
        let delivered_ctx = ExecutionCtx {
            response: Some(&response),
            ..base_ctx
        };
        let outcome = frame
            .outcome()
            .ok_or_else(|| DispatchError::BadResponse("未知结果码".into()))?;
        match outcome {
            OutcomeCode::Applied => {
                self.settle_command(SettleTrigger::Applied, &mut states, &delivered_ctx)
            }
            OutcomeCode::IdempotentHit => self.settle_command(
                SettleTrigger::PeerIdempotentHit,
                &mut states,
                &delivered_ctx,
            ),
            OutcomeCode::DryRun => {
                self.settle_command(SettleTrigger::PeerDryRun, &mut states, &delivered_ctx)
            }
            OutcomeCode::Denied => self.settle_command(
                SettleTrigger::PeerDenied { class: frame.class },
                &mut states,
                &delivered_ctx,
            ),
            OutcomeCode::AppliedReadbackUnavailable => self.settle_command(
                SettleTrigger::ReadbackUnavailable,
                &mut states,
                &delivered_ctx,
            ),
            OutcomeCode::AppliedReadbackMismatch => {
                self.settle_command(SettleTrigger::ReadbackMismatch, &mut states, &delivered_ctx)
            }
            OutcomeCode::Failed => {
                self.settle_command(SettleTrigger::PeerFailed, &mut states, &delivered_ctx)
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
    fn tick_held(
        &mut self,
        input: Option<&eneros_agent_sdk::snap_input_wire::TrustedInput>,
        telemetry: Option<&TrustedTelemetry>,
    ) -> Result<(), DispatchError> {
        self.drive_held_plans(input, telemetry).map(|_| ())
    }

    fn query_held(&self) -> Result<Vec<crate::saga::DriveReport>, DispatchError> {
        self.held_plan_reports()
    }
    fn default_asset_id(&self) -> u64 {
        u64::from(self.routes[0].profile.device_id)
    }

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

    fn submit_typed(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        target_device: &str,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }
        self.run_typed(
            plan,
            alloc::string::String::from(target_device),
            None,
            telemetry,
            actor,
            deadline_ms,
            None,
        )
    }

    fn submit_trusted(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        trusted: &eneros_agent_sdk::snap_input_wire::TrustedInput,
        target_device: &str,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.now_fn)();
        if !telemetry.is_valid_at(started) {
            return Err(DispatchError::TelemetryExpired);
        }
        self.run_typed(
            plan,
            alloc::string::String::from(target_device),
            None,
            telemetry,
            actor,
            deadline_ms,
            Some(trusted),
        )
    }
}

impl DispatchEngine {
    /// 验方 key 环替换（装配/轮换用；生产装配在构造时注入，运行期轮换由
    /// 运维动作驱动——本访问器是唯一改点）。
    pub fn set_certifier_ring(&mut self, ring: eneros_certifier::proof::CertifierKeyRing) {
        self.certifier_ring = ring;
    }

    /// 蓝图 70.8 WP-70.8.4：可信输入准入判据（**只验证、不改写**）。
    ///
    /// 1. 策略：`Required` 下无可信输入的候选一律拒绝（零投递）；
    /// 2. 身份同代：候选 `snapshot_id` / `resource_catalog_version` /
    ///    时域起点必须与可信输入逐字段相等；
    /// 3. 摘要自洽：`objective_digest` 必须等于绑定公式输出（防篡改）；
    /// 4. 完整性：快照必须完整（缺必需点 = 无控制资格）。
    fn admit_trusted_input(
        &self,
        plan: &eneros_decision_contract::CandidatePlan,
        trusted: Option<&eneros_agent_sdk::snap_input_wire::SnapshotFacts>,
        now_ms: u64,
    ) -> Result<(), DispatchError> {
        let Some(facts) = trusted else {
            return match self.trusted_input {
                TrustedInputPolicy::Required => Err(DispatchError::ProposalInvalid(String::from(
                    "trusted-input: 缺可信输入身份（生产策略 Required，零投递）",
                ))),
                TrustedInputPolicy::LegacyMigration => Ok(()),
            };
        };
        if !facts.is_complete() {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 快照不完整（缺必需点）",
            )));
        }
        if plan.snapshot_id().0 != facts.snapshot_id {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 候选快照身份与可信输入不同代",
            )));
        }
        if plan.resource_catalog_version() != facts.catalog_version {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 候选目录版本与可信输入不同代",
            )));
        }
        let expect_horizon = facts.produced_at_ns / 1_000_000;
        if plan.horizon_start_ms() != expect_horizon {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 候选时域起点与快照产出时刻不符",
            )));
        }
        let expect_digest = eneros_decision_contract::objective_binding_v1(
            &facts.snapshot_id,
            facts.catalog_version,
            plan.horizon_start_ms(),
            plan.step_ns(),
            plan.step_count(),
        );
        if plan.objective_digest() != &expect_digest {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 候选目标摘要与绑定公式不符（伪造/陈旧）",
            )));
        }
        // 快照年龄（输入新鲜度在 svc-telemetry 侧判；此处二次防线：滚动
        // 时域窗内的旧快照不得驱动新命令）。
        if now_ms < plan.issued_at_ms() {
            return Err(DispatchError::ProposalInvalid(String::from(
                "trusted-input: 候选签发时刻晚于本机时刻（时钟异常）",
            )));
        }
        Ok(())
    }
}

/// 蓝图 61 Phase 5 WP-5.2（[10] §3.3）：12 步合核 → [`SagaExecutor`]
/// 生产 adapter——命令批经 [`eneros_control_dispatch::saga::BatchSaga`]
/// 编排时，逐命令的授权/投递/读回/记账仍由唯一合核承载（不产生第二
/// 执行真相）。
///
/// > 状态：**deferred（ADR-0052，蓝图 63 v0.316.0）**——零生产消费者冻结
/// > 资产（同 saga.rs 模块头）。`#[allow(clippy::too_many_arguments)]` 因
/// > 冻结纪律保留（构造样板收拢归激活前置，见 ADR-0052 裁决 4）。
///
/// - `execute`：走 [`DispatchEngine::execute_core`]-equivalent 链
///   （PlanCommand 身份：幂等键 = intent_id 派生 / legacy 单命令 = PlanId
///   ——journal 同键防重复投递）；
/// - `reconcile`（[10] §3.4 readback-first）：只调用设备独立读取接口，
///   不重入执行链、不追加意图、不发送写命令；读值匹配后由 driver
///   再次独立读取确认，读失败交人工处理。
/// - `compensate`：补偿派生键（`"comp"` 域分隔）全新执行——新意图、
///   新授权、新 journal 条目，不改写历史。
pub struct EngineSagaExecutor<'a> {
    engine: &'a mut DispatchEngine,
    telemetry: &'a TrustedTelemetry,
    actor: &'a AuthenticatedActor,
    plan_id: eneros_decision_contract::PlanId,
    plan_input_digest: [u8; 32],
    /// CandidatePlan wire SHA-256（62 v0.315.0 WP-62.1.2；构造方经
    /// `CandidatePlan::plan_wire_sha256` 派生后显式传入）。
    plan_sha: [u8; 32],
    plan_ttl_ms: u64,
    legacy_single: bool,
    target_device: String,
    deadline_ms: u64,
}

use crate::saga::{CmdOutcome, ReconcileOutcome, SagaExecutor};

/// 能量窗估算单源（WP-64.3.2.3；R-64.e 边界内——`run_typed` 幂等命中
/// 路径与 [`EngineSagaExecutor::execute_intent`] 两体共用的 crate 内
/// 私有函数；**不**给 executor 增删参数/方法/行为）。
///
/// 语义冻结：`window_secs = (valid_until - now).min(ttl)/1000`（下限
/// `max(1)` 由调用方施加，保持原表达式位置）；能量上限形态 =
/// `|power| × 意图窗（valid_from..valid_until，小时）`。
fn intent_window_estimate(
    intent: &eneros_decision_contract::CommandIntent,
    now_ms: u64,
    ttl_ms: u64,
) -> (u64, f64) {
    let window_secs = intent.valid_until_ms().saturating_sub(now_ms).min(ttl_ms) / 1000;
    let window_h = (intent
        .valid_until_ms()
        .saturating_sub(intent.valid_from_ms()) as f64)
        / 3_600_000.0;
    let energy_kwh = intent.power_kw().abs() * window_h;
    (window_secs, energy_kwh)
}

/// correlation → 事件摘要证据（32B 证据位；SHA-256 域分隔）。
fn request_digest_of(correlation: [u8; 16]) -> [u8; 32] {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"eneros.saga-request.v1");
    h.update(correlation);
    h.finalize().into()
}

impl<'a> EngineSagaExecutor<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: &'a mut DispatchEngine,
        telemetry: &'a TrustedTelemetry,
        actor: &'a AuthenticatedActor,
        plan_id: eneros_decision_contract::PlanId,
        plan_input_digest: [u8; 32],
        // 62 v0.315.0 WP-62.1.2（D-3）：CandidatePlan wire SHA-256 显式
        // 传入（`CandidatePlan::plan_wire_sha256` 单源派生）——与输入
        // provenance 摘要语义分离，禁止互相冒充。
        plan_sha: [u8; 32],
        plan_ttl_ms: u64,
        legacy_single: bool,
        target_device: &str,
        deadline_ms: u64,
    ) -> Self {
        Self {
            engine,
            telemetry,
            actor,
            plan_id,
            plan_input_digest,
            plan_sha,
            plan_ttl_ms,
            legacy_single,
            target_device: alloc::string::String::from(target_device),
            deadline_ms,
        }
    }

    fn execute_intent(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        correlation: [u8; 16],
        power_override_kw: Option<f64>,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.engine.now_fn)();
        if started < intent.valid_from_ms() || started >= intent.valid_until_ms() {
            return Err(DispatchError::ProposalInvalid(String::from(
                "saga command outside validity window",
            )));
        }
        let (window_secs, energy_est) = intent_window_estimate(intent, started, self.plan_ttl_ms);
        // 恢复的批可能引用已移除的路由：与普通意图入口同样拒绝，
        // 不能让未注册目标使整个服务 panic。
        let route_idx = self
            .engine
            .route_index_for(u32::from(intent.device_id()), intent.point_id())
            .ok_or_else(|| {
                DispatchError::NotAuthorized(alloc::format!(
                    "saga target device={} point={} 未绑定本 dispatch",
                    intent.device_id(),
                    intent.point_id()
                ))
            })?;
        self.engine.arbitrate_owned(
            crate::arbiter::CommandSource::SiteOptimizer,
            true,
            u32::from(intent.device_id()),
            Some(self.plan_id),
        )?;
        self.engine.execute_core(
            CoreCommand {
                identity: CoreIdentity::PlanCommand { correlation },
                ttl_secs: window_secs.max(1),
                power_kw: power_override_kw.unwrap_or_else(|| intent.power_kw()),
                target_device: self.target_device.clone(),
                input_digest: Some(self.plan_input_digest),
                energy_kwh: Some(energy_est),
                // 62 v0.315.0 WP-62.1.2（D-3）：wire SHA-256（原此处传
                // plan_input_digest——输入摘要冒充 wire 绑定摘要）。
                plan_sha: Some(self.plan_sha),
                idem_note: String::from("saga idempotent-hit（原结果，不重复写）"),
            },
            self.telemetry,
            self.actor,
            started,
            alloc::vec![DispatchState::Proposed, DispatchState::Validated],
            self.deadline_ms,
            route_idx,
        )
    }

    fn map_receipt(receipt: DispatchReceipt) -> CmdOutcome {
        let note = receipt.note.clone().unwrap_or_default();
        let digest: [u8; 32] = {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update(receipt.correlation_id);
            h.update([receipt.final_state.as_str().len() as u8]);
            h.update(receipt.final_state.as_str().as_bytes());
            h.finalize().into()
        };
        match receipt.final_state {
            DispatchState::Committed => CmdOutcome::Applied {
                receipt_digest: digest,
                readback: None,
            },
            DispatchState::Rejected => CmdOutcome::Rejected { reason: note },
            DispatchState::Failed => CmdOutcome::Failed { reason: note },
            DispatchState::Indeterminate => CmdOutcome::Unknown {
                request_digest: digest,
                reason: note,
            },
            other => CmdOutcome::Unknown {
                request_digest: digest,
                reason: alloc::format!("in-flight journal state: {other:?}"),
            },
        }
    }
}

impl SagaExecutor for EngineSagaExecutor<'_> {
    fn applied_durable(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        readback: Option<f64>,
    ) -> Result<(), String> {
        let correlation = crate::compiler::plan_command_correlation(
            &self.plan_id,
            intent.intent_id(),
            self.legacy_single,
        );
        self.engine
            .settle_saga_applied(intent, correlation, readback)
    }

    fn execute(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        _attempt: u32,
    ) -> CmdOutcome {
        let correlation = crate::compiler::plan_command_correlation(
            &self.plan_id,
            intent.intent_id(),
            self.legacy_single,
        );
        match self.execute_intent(intent, correlation, None) {
            Ok(r) => {
                let mut outcome = Self::map_receipt(r);
                if let CmdOutcome::Applied { readback, .. } = &mut outcome {
                    *readback = self.engine.post_apply_readback(intent, self.deadline_ms);
                }
                outcome
            }
            Err(DispatchError::DeliveryTimeout) => CmdOutcome::Unknown {
                request_digest: request_digest_of(correlation),
                reason: String::from("delivery timeout"),
            },
            Err(e) => CmdOutcome::Rejected {
                reason: alloc::format!("{e:?}"),
            },
        }
    }

    fn reconcile(&mut self, intent: &eneros_decision_contract::CommandIntent) -> ReconcileOutcome {
        self.engine.reconcile_saga_intent(intent, self.deadline_ms)
    }

    fn compensate(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        restore_kw: f64,
    ) -> CmdOutcome {
        // 补偿 = 新意图、新授权、新 journal（[10] §3.3.9）：补偿幂等键
        // 独立派生（"comp" 域分隔），与原命令互不覆盖。
        let base = crate::compiler::plan_command_correlation(
            &self.plan_id,
            intent.intent_id(),
            self.legacy_single,
        );
        let correlation = {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update(b"eneros.saga-compensation.v1");
            h.update(base);
            let d: [u8; 32] = h.finalize().into();
            d[..16].try_into().expect("16B")
        };
        match self.execute_intent(intent, correlation, Some(restore_kw)) {
            Ok(r) => Self::map_receipt(r),
            Err(DispatchError::DeliveryTimeout) => CmdOutcome::Unknown {
                request_digest: request_digest_of(correlation),
                reason: String::from("compensation timeout"),
            },
            Err(e) => CmdOutcome::Rejected {
                reason: alloc::format!("{e:?}"),
            },
        }
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

    /// WP-64.3.2.3 golden（迁移前固化现值；R-64.e）：私有估算函数输出
    /// 与两体迁移前公式逐位一致（ttl 封顶形态 + 未封顶形态）。
    #[test]
    fn intent_window_estimate_golden() {
        let intent = |from: u64, until: u64, kw: f64| {
            eneros_decision_contract::CommandIntent::from_parts(
                eneros_decision_contract::CommandIntentParts {
                    intent_id: eneros_decision_contract::IntentId([3u8; 16]),
                    plan_id: eneros_decision_contract::PlanId([1u8; 16]),
                    plan_sha256: [0xAB; 32],
                    asset_id: 1,
                    device_id: 1,
                    point_id: 10,
                    power_kw: kw,
                    unit_code: 6,
                    valid_from_ms: from,
                    valid_until_ms: until,
                    precondition: eneros_decision_contract::Precondition::None,
                    compensation: eneros_decision_contract::CompensationHint::None,
                },
            )
            .unwrap()
        };
        // 形态一（run_typed 幂等命中原值）：now=2000, ttl=60_000 →
        // window_secs = min(61_000-2_000, 60_000)/1000 = 59；
        // energy = 50 × (61_000-1_000)/3_600_000 = 0.8333333333333334。
        let (secs, energy) = intent_window_estimate(&intent(1_000, 61_000, -50.0), 2_000, 60_000);
        assert_eq!(secs, 59);
        assert_eq!(energy, 0.8333333333333334f64);
        // 形态二（execute_intent 原值）：now=1_000（未封顶）→ 60；
        // energy = 5 × 意图窗。
        let (secs, energy) = intent_window_estimate(&intent(1_000, 61_000, 5.0), 1_000, 60_000);
        assert_eq!(secs, 60);
        assert_eq!(energy, 0.08333333333333333f64);
    }

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

    // ===== 62 v0.315.0 WP-62.1.3 结算决策表表级单测 =====
    //
    // 逐触发断言四元组（durable 写序 / audit 形态与逐字节文案 / 回执形态）。
    // 新增触发事件 = SettleTrigger 一臂 + settle 一臂 + 本节一用例。

    /// ACK 后典型状态路径（OutcomeCode 各臂的 audit states 调试串来源）。
    fn ack_states() -> Vec<DispatchState> {
        alloc::vec![
            DispatchState::Persisted,
            DispatchState::Delivered,
            DispatchState::TransportAcked,
        ]
    }

    /// IntentAppendFailed / JournalPersistFailed 触发点的真实流程位
    /// （execute_core：Authorized/Reserved 已推、Persisted 未推——pre-
    /// persist 阶段）。夹具须与真实位一致，否则未来该触发改 StatesSuffix
    /// 形态时测试夹具会掩盖词形漂移（62 复查 62.1-B 收口）。
    fn pre_persist_states() -> Vec<DispatchState> {
        alloc::vec![
            DispatchState::Proposed,
            DispatchState::Validated,
            DispatchState::Authorized,
            DispatchState::Reserved,
        ]
    }

    fn audit_of(s: &Settlement, states: &[DispatchState]) -> (ExecOutcome, SafetyOutcome, String) {
        let (exec, safety, text) = s.audit.as_ref().expect("audit 存在");
        (*exec, *safety, text.render(states))
    }

    fn journal_op(from: ExecutionStatus, to: ExecutionStatus) -> SettleOp {
        SettleOp::Journal { from, to }
    }

    fn intent_op(state: DispatchState) -> SettleOp {
        SettleOp::Intent { state }
    }

    #[test]
    fn settle_table_safety_reject() {
        let states = alloc::vec![DispatchState::Authorized];
        let s = settle(
            &SettleTrigger::SafetyReject {
                reason: String::from("ov"),
            },
            &states,
        );
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        let (exec, safety, text) = audit_of(&s, &states);
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(matches!(safety, SafetyOutcome::Blocked));
        assert_eq!(text, format!("states={states:?};safety-reject=ov"));
        assert!(matches!(s.receipt, SettleReceipt::SafetyReject { .. }));
    }

    #[test]
    fn settle_table_budget_rate_no_audit() {
        let s = settle(&SettleTrigger::BudgetRate, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        assert!(s.audit.is_none(), "rate 预算拒绝现状无 audit");
        assert!(matches!(
            s.receipt,
            SettleReceipt::BudgetExceeded { kind: "rate" }
        ));
    }

    #[test]
    fn settle_table_budget_energy_no_audit() {
        let s = settle(&SettleTrigger::BudgetEnergy, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        assert!(s.audit.is_none(), "energy 预算拒绝现状无 audit");
        assert!(matches!(
            s.receipt,
            SettleReceipt::BudgetExceeded { kind: "energy" }
        ));
    }

    #[test]
    fn settle_table_intent_append_failed() {
        let s = settle(
            &SettleTrigger::IntentAppendFailed {
                err: String::from("DiskFull"),
            },
            &pre_persist_states(),
        );
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        let (exec, safety, text) = audit_of(&s, &pre_persist_states());
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(matches!(safety, SafetyOutcome::Passed));
        assert_eq!(
            text,
            "intent persist failed: DiskFull（零投递，终局 Failed）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::IntentPersistFailed { .. }
        ));
    }

    #[test]
    fn settle_table_journal_persist_failed() {
        let s = settle(
            &SettleTrigger::JournalPersistFailed {
                err: String::from("NotAuthorized"),
            },
            &pre_persist_states(),
        );
        assert!(s.durable.is_empty(), "推进已试败，表内无补偿写");
        let (_, _, text) = audit_of(&s, &pre_persist_states());
        assert_eq!(
            text,
            "execution-journal Authorized→Persisted failed: NotAuthorized"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::IntentPersistFailed { .. }
        ));
    }

    #[test]
    fn settle_table_sent_advance_failed() {
        let states = ack_states();
        let s = settle(
            &SettleTrigger::SentAdvanceFailed {
                err: String::from("NotAuthorized"),
            },
            &states,
        );
        assert_eq!(
            s.durable,
            alloc::vec![intent_op(DispatchState::Indeterminate)]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            "delivery-ok 但 journal Sent 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-ok 但 journal Sent 推进失败 NotAuthorized——对账"
        ));
    }

    #[test]
    fn settle_table_mark_delivered_failed() {
        let s = settle(&SettleTrigger::MarkDeliveredFailed, &ack_states());
        assert!(s.durable.is_empty());
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "delivery-ok 但 durable mark-sent 失败——对账");
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_delivery_timeout() {
        let s = settle(&SettleTrigger::DeliveryTimeout, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-timeout：写可能已发生，需对账"
        ));
    }

    #[test]
    fn settle_table_delivery_unreachable() {
        let s = settle(&SettleTrigger::DeliveryUnreachable, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "delivery-unreachable");
        assert!(matches!(s.receipt, SettleReceipt::Unreachable));
    }

    #[test]
    fn settle_table_delivery_error() {
        let states = ack_states();
        let s = settle(
            &SettleTrigger::DeliveryError {
                err: String::from("Protocol(\"x\")"),
            },
            &states,
        );
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            "delivery: Protocol(\"x\") → Indeterminate（写可能已发生，对账）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-error Protocol(\"x\")：物理结果未知，需对账"
        ));
    }

    #[test]
    fn settle_table_response_decode_failed() {
        let s = settle(
            &SettleTrigger::ResponseDecodeFailed {
                err: String::from("Truncated"),
            },
            &ack_states(),
        );
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Sent, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "response-decode: Truncated → Indeterminate（对账）");
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "response-decode-failed：物理结果未知，需对账"
        ));
    }

    #[test]
    fn settle_table_ack_advance_failed() {
        let s = settle(
            &SettleTrigger::AckAdvanceFailed {
                err: String::from("NotAuthorized"),
            },
            &ack_states(),
        );
        assert_eq!(
            s.durable,
            alloc::vec![intent_op(DispatchState::Indeterminate)]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "ack-decoded 但 journal TransportAcked 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_applied_write_order_invariant() {
        // 写序不变量：durable 终态 ack 先行，成功后才 journal AppliedConfirmed。
        let s = settle(&SettleTrigger::Applied, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![
                intent_op(DispatchState::Committed),
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::AppliedConfirmed
                ),
            ]
        );
        let (exec, safety, text) = audit_of(&s, &ack_states());
        assert!(matches!(exec, ExecOutcome::Succeeded));
        assert!(matches!(safety, SafetyOutcome::Passed));
        assert_eq!(text, format!("states={:?}", ack_states()));
        assert!(matches!(s.receipt, SettleReceipt::Committed));
    }

    #[test]
    fn settle_table_applied_durable_failed() {
        let s = settle(&SettleTrigger::AppliedDurableFailed, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::TransportAcked,
                ExecutionStatus::Indeterminate
            )]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_applied_advance_failed() {
        let s = settle(
            &SettleTrigger::AppliedAdvanceFailed {
                err: String::from("NotAuthorized"),
            },
            &ack_states(),
        );
        assert!(s.durable.is_empty());
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "applied-confirmed 但 journal 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_peer_idempotent_hit() {
        let s = settle(&SettleTrigger::PeerIdempotentHit, &ack_states());
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Reconciling
                ),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "对端幂等命中：原结果未知——对账，不升级 Committed");
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "对端幂等命中：原结果未知——对账，不升级 Committed"
        ));
    }

    #[test]
    fn settle_table_peer_dry_run() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerDryRun, &states);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Rejected),
                intent_op(DispatchState::Rejected),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=对端 dry-run（未写设备）")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Rejected,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_peer_denied() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerDenied { class: 3 }, &states);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=device-denied class=3")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_readback_unavailable() {
        let states = ack_states();
        let s = settle(&SettleTrigger::ReadbackUnavailable, &states);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Indeterminate
                ),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=readback-unavailable：物理状态未确认，需对账")
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_readback_mismatch_blocked() {
        let states = ack_states();
        let s = settle(&SettleTrigger::ReadbackMismatch, &states);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (exec, safety, text) = audit_of(&s, &states);
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(
            matches!(safety, SafetyOutcome::Blocked),
            "读回不符必须 Blocked"
        );
        assert_eq!(
            text,
            format!("states={states:?};note=readback-mismatch：ACK 成功但物理状态未达目标")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_peer_failed() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerFailed, &states);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(text, format!("states={states:?};note=device-exec-failed"));
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }
}

impl DispatchEngine {
    /// ADR-0054:15–16：新鲜应用的**实际回读**证据（写入 Saga 事件持久化，
    /// `DISP_HELD_QUERY`（HQR1）可查）。经独立只读通道读**命令点**——设备
    /// 真实持有的设定值，非请求回显（传输 ACK = 设备结果码；物理反馈点
    /// （合同 `readback_point_id`）= 对账/恢复路径的验证源——三语义分离）。
    /// 前置 = 资产有控制合同（与对账路径同门）；读失败/非有限值 = None
    /// （诚实缺证，不用设定值代填，不改变已判定的 Applied 终态）。
    fn post_apply_readback(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        deadline_ms: u64,
    ) -> Option<f64> {
        if self
            .contracts
            .plan_contract(intent.asset_id())
            .map(|c| c.readback_point_id == 0)
            .unwrap_or(true)
        {
            return None;
        }
        match self.delivery.readback(
            u32::from(intent.device_id()),
            intent.point_id(),
            deadline_ms,
        ) {
            Ok(actual) if actual.is_finite() => Some(actual),
            _ => None,
        }
    }

    fn reconcile_saga_intent(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        deadline_ms: u64,
    ) -> ReconcileOutcome {
        // Never enter execute_core here: a missing journal is not permission to
        // send, and replaying its receipt twice is not two independent reads.
        let Some(contract) = self.contracts.plan_contract(intent.asset_id()) else {
            return ReconcileOutcome::ResolvedManual {
                reason: String::from("readback contract missing"),
            };
        };
        if self
            .route_index_for(u32::from(intent.device_id()), intent.point_id())
            .is_none()
            || contract.readback_point_id == 0
        {
            return ReconcileOutcome::ResolvedManual {
                reason: String::from("readback route missing"),
            };
        }
        match self.delivery.readback(
            u32::from(intent.device_id()),
            contract.readback_point_id,
            deadline_ms,
        ) {
            Ok(actual) if actual.is_finite() => {
                if (actual - intent.power_kw()).abs()
                    <= eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW
                {
                    ReconcileOutcome::ConfirmedApplied {
                        readback: actual,
                        double_verified: false,
                    }
                } else {
                    ReconcileOutcome::FailedConfirmed {
                        reason: alloc::format!(
                            "independent readback {actual} != {}",
                            intent.power_kw()
                        ),
                    }
                }
            }
            other => ReconcileOutcome::ResolvedManual {
                reason: alloc::format!("independent readback unavailable: {other:?}"),
            },
        }
    }
}

#[path = "held_engine.rs"]
pub mod held_engine;
