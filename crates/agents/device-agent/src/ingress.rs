//! 信封输入状态机（蓝图/24 §6.2/6.3/6.6 —— device-agent 唯一写收口点）。
//!
//! [`EnvelopeIngress`] 是设备写的**唯一裁决收口点**（§1.3 推论 D：远程路径
//! 与本地路径在 device-agent 汇合后走同一个复验函数）：
//!
//! ```text
//! 信封(EIPC) ──▶ 1.帧解码 → 2.ControlChain 复验
//!                (HMAC/租约/幂等/越权/策略PDP/安全)
//!                    │                     │
//!               复验通过                 复验失败
//!                    ▼                     ▼
//!        3.路由: device_id → 适配器   拒绝分类 → 指标+审计
//!        4.点匹配: point_id → mapping （不触碰设备）
//!        5.write_authorized(cmd, value)
//!                    │
//!              ┌────┴────┐
//!           成功│        │失败
//!              ▼        ▼
//!        回读确认    协议错误 → 指标+审计
//!       （可空）    （D14：跳过本条，不中断队列）
//! ```
//!
//! 失败行为表（§6.2）：每一类拒绝都有指标 + 审计，全部路径**不 panic**。
//!
//! # 本地租约与握手（§6.3，D7 修正版静态预置）
//!
//! - `lease_id` 配置预置（两侧同源）；`LeaseService` 以平台重启计数为
//!   epoch 种子启动本地 `grant()`；
//! - 握手请求到达时返回 `{lease_id, fencing_token}`（HMAC-SM3 保护，
//!   见 [`handshake_response`](Self::handshake_response)）；
//! - 签发方（AI runner）经一次启动握手取当前 token（§6.4 投递器消费）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_audit::{AuditEvent, AuditEventType, AuditLogger, AuditLoggerImpl, MemStore};
use eneros_control_gateway::chain::{ChainDenyReason, ChainOutcome, ControlChain, FeedbackBundle};
use eneros_control_gateway::envelope::CommandEnvelope;
use eneros_control_gateway::gateway::GatewayError;
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::Policy;
use eneros_control_gateway::safety::SafetyPoint;
use eneros_crypto::{ct_eq, hmac_sm3};
use eneros_metrics::{
    MemoryMetricsStore, MetricDef, MetricType, MetricsCollector, MetricsCollectorImpl, MetricsStore,
};
use eneros_protocol_abstract::access::WriteError;
use eneros_upa_model::{DeviceId, PointId, PointValue};

use crate::delivery::DeliveryFrame;
use crate::registry::DeviceRegistry;

// ============================================================================
// 指标命名（蓝图/24 §6.6：全部为计数器）
// ============================================================================

/// 写执行成功计数。
pub const METRIC_WRITE_ALLOWED: &str = "eneros_write_allowed_total";
/// 帧解码失败计数。
pub const METRIC_DENIED_DECODE: &str = "eneros_write_denied_decode_total";
/// HMAC 失败计数（疑似伪造）。
pub const METRIC_DENIED_HMAC: &str = "eneros_write_denied_hmac_total";
/// 租约失效/过期/未知计数（含信封时间性/资格性拒绝，见 [`classify_deny`]）。
pub const METRIC_DENIED_LEASE: &str = "eneros_write_denied_lease_total";
/// fencing token 过期计数。
pub const METRIC_DENIED_STALE_TOKEN: &str = "eneros_write_denied_stale_token_total";
/// 策略 PDP 拒绝计数（含越权/流程类拒绝）。
pub const METRIC_DENIED_POLICY: &str = "eneros_write_denied_policy_total";
/// 安全校验拒绝计数。
pub const METRIC_DENIED_SAFETY: &str = "eneros_write_denied_safety_total";
/// 设备/点不匹配计数。
pub const METRIC_DENIED_MISMATCH: &str = "eneros_write_denied_mismatch_total";
/// 旧匿名命令拒绝计数（§6.2 双轨）。
pub const METRIC_DENIED_LEGACY: &str = "eneros_write_denied_legacy_total";
/// 幂等命中计数（exactly-once，返回先前结果）。
pub const METRIC_IDEMPOTENT_HIT: &str = "eneros_write_idempotent_hit_total";
/// 协议写失败计数（D14：跳过本条）。
pub const METRIC_FAILED_PROTOCOL: &str = "eneros_write_failed_protocol_total";

/// 全部写接缝指标（构造时注册；increment 失败仅在未注册时发生，注册后不可达）。
const WRITE_METRICS: [(&str, &str); 11] = [
    (METRIC_WRITE_ALLOWED, "authorized writes applied to devices"),
    (
        METRIC_DENIED_DECODE,
        "write denied: frame/envelope decode failure",
    ),
    (
        METRIC_DENIED_HMAC,
        "write denied: HMAC mismatch (suspected forgery)",
    ),
    (
        METRIC_DENIED_LEASE,
        "write denied: lease invalid/expired/unknown or envelope time-denial",
    ),
    (
        METRIC_DENIED_STALE_TOKEN,
        "write denied: stale fencing token",
    ),
    (
        METRIC_DENIED_POLICY,
        "write denied: policy PDP / override / sbo",
    ),
    (
        METRIC_DENIED_SAFETY,
        "write denied: independent safety validator",
    ),
    (
        METRIC_DENIED_MISMATCH,
        "write denied: device/point mismatch",
    ),
    (
        METRIC_DENIED_LEGACY,
        "write denied: legacy anonymous command (transition track)",
    ),
    (
        METRIC_IDEMPOTENT_HIT,
        "write idempotent hit (exactly-once, prior result returned)",
    ),
    (
        METRIC_FAILED_PROTOCOL,
        "write failed at protocol layer (skipped, D14)",
    ),
];

// ============================================================================
// 拒绝分类与结果
// ============================================================================

/// 拒绝分类（§6.2 失败行为表的指标桶；审计 detail 始终携带精确原因）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyClass {
    /// 帧解码失败（投递帧/信封结构）。
    Decode,
    /// HMAC 验证失败（疑似伪造）。
    Hmac,
    /// 租约失效/过期/未知（含信封时间性/资格性拒绝：Expired/Future/Replayed）。
    Lease,
    /// fencing token 过期（跨重启单调，StaleFencing）。
    StaleToken,
    /// 策略 PDP / 越权 / SBO 流程拒绝。
    Policy,
    /// 独立安全校验拒绝（含反馈不可信）。
    Safety,
    /// 设备/点不匹配。
    Mismatch,
    /// 协议层写失败。
    Protocol,
    /// 旧匿名命令（双轨：收到不执行）。
    Legacy,
}

impl DenyClass {
    /// 指标名。
    pub const fn metric(self) -> &'static str {
        match self {
            DenyClass::Decode => METRIC_DENIED_DECODE,
            DenyClass::Hmac => METRIC_DENIED_HMAC,
            DenyClass::Lease => METRIC_DENIED_LEASE,
            DenyClass::StaleToken => METRIC_DENIED_STALE_TOKEN,
            DenyClass::Policy => METRIC_DENIED_POLICY,
            DenyClass::Safety => METRIC_DENIED_SAFETY,
            DenyClass::Mismatch => METRIC_DENIED_MISMATCH,
            DenyClass::Protocol => METRIC_FAILED_PROTOCOL,
            DenyClass::Legacy => METRIC_DENIED_LEGACY,
        }
    }

    /// 审计 detail 标签。
    pub const fn label(self) -> &'static str {
        match self {
            DenyClass::Decode => "decode",
            DenyClass::Hmac => "hmac",
            DenyClass::Lease => "lease",
            DenyClass::StaleToken => "stale_token",
            DenyClass::Policy => "policy",
            DenyClass::Safety => "safety",
            DenyClass::Mismatch => "mismatch",
            DenyClass::Protocol => "protocol",
            DenyClass::Legacy => "legacy",
        }
    }
}

/// 信封处理结果（外部可观察行为）。
#[derive(Debug, Clone, PartialEq)]
pub enum IngressOutcome {
    /// 复验通过并落写成功（含回读确认，可空）。
    Applied {
        device_id: DeviceId,
        point_id: PointId,
        power_kw: f64,
    },
    /// 幂等命中：先前结果已返回，未二次执行（exactly-once）。
    IdempotentHit,
    /// 策略允许但为干跑（Obligation::DryRun）：不落写。
    DryRun,
    /// 复验/路由/落写被拒。`reason` 为精确原因（审计 detail 同源）。
    Denied { class: DenyClass, reason: String },
}

/// 链拒绝原因 → 指标桶（§6.2 失败行为表；表未列出的网关拒绝归入
/// 语义最近的桶，审计 detail 携带精确原因——"每类都有指标+审计"）。
fn classify_deny(reason: &ChainDenyReason) -> DenyClass {
    match reason {
        ChainDenyReason::Gateway(e) => match e {
            GatewayError::Decode(_) => DenyClass::Decode,
            GatewayError::Unauthorized => DenyClass::Hmac,
            // 时间性/资格性拒绝（信封过期/未来/无租约/已吊销/重放）：
            // 命令不再有执行资格，归入租约桶；审计 detail 精确区分。
            GatewayError::Expired
            | GatewayError::Future
            | GatewayError::NoLease
            | GatewayError::LeaseRevoked
            | GatewayError::Replayed => DenyClass::Lease,
            GatewayError::StaleFencing => DenyClass::StaleToken,
            GatewayError::NoPolicy => DenyClass::Policy,
            GatewayError::BadDeviceId => DenyClass::Mismatch,
        },
        ChainDenyReason::LeaseRevoked => DenyClass::Lease,
        ChainDenyReason::StaleFencing => DenyClass::StaleToken,
        // 越权接管（CTL-009）与 SBO 流程（CTL-007）属授权类拒绝。
        ChainDenyReason::SupersededByOverride => DenyClass::Policy,
        ChainDenyReason::Policy(_) => DenyClass::Policy,
        ChainDenyReason::NotSelected | ChainDenyReason::SelectExpired => DenyClass::Policy,
        // 独立安全校验（CTL-008）与反馈不可信（CTL-010）→ 安全桶。
        ChainDenyReason::Unsafe(_) | ChainDenyReason::FeedbackUntrusted(_) => DenyClass::Safety,
    }
}

/// 链拒绝原因 → 可读字符串（审计/指标 detail）。
fn deny_reason_str(reason: &ChainDenyReason) -> String {
    match reason {
        ChainDenyReason::Gateway(e) => format!("gateway:{e:?}"),
        ChainDenyReason::LeaseRevoked => String::from("lease-revoked"),
        ChainDenyReason::StaleFencing => String::from("stale-fencing"),
        ChainDenyReason::SupersededByOverride => String::from("superseded-by-override"),
        ChainDenyReason::Policy(d) => format!("policy:{d:?}"),
        ChainDenyReason::NotSelected => String::from("not-selected"),
        ChainDenyReason::SelectExpired => String::from("select-expired"),
        ChainDenyReason::Unsafe(s) => format!("unsafe:{s}"),
        ChainDenyReason::FeedbackUntrusted(s) => format!("feedback-untrusted:{s}"),
    }
}

// ============================================================================
// 信封输入状态机
// ============================================================================

/// 握手响应长度：lease_id(16) + fencing_token(8) + HMAC(32)。
pub const HANDSHAKE_LEN: usize = 16 + 8 + 32;

/// 收口错误（构造/握手失败；运行期失败一律经 `IngressOutcome::Denied` 表达，
/// 不 panic）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressError {
    /// 本地租约签发失败（lease_id 与吊销墓碑冲突等，fail-closed）。
    LeaseIssueFailed,
}

/// 信封输入状态机（蓝图/24 §6.2）。
pub struct EnvelopeIngress {
    /// 复验链（HMAC/租约/幂等/越权/策略/安全；default-deny）。
    chain: ControlChain,
    /// 复验 HMAC 密钥（装配层注入；握手响应 HMAC 同源）。
    key: Vec<u8>,
    /// 本地预置租约 ID（两侧同源）。
    lease_id: [u8; 16],
    /// 拒绝分类指标（§6.6 计数器，构造时注册）。
    metrics: MetricsCollectorImpl<MemoryMetricsStore>,
    /// 审计链（§6.6：写执行/每类拒绝/握手/租约签发）。
    audit: AuditLoggerImpl<MemStore>,
    /// 成功落写计数（外部可观测）。
    write_count: u64,
    /// 本地租约是否签发成功（失败 → 全部拒绝，fail-closed）。
    lease_ok: bool,
}

impl EnvelopeIngress {
    /// 构造收口点：装配复验链（密钥/策略/安全点/租约），注册指标。
    ///
    /// - `key`：信封 HMAC 密钥（装配层注入，不落明文配置，§2.6）；
    /// - `lease_id`：本地预置租约 ID（配置预置，两侧同源，§6.7）；
    /// - `epoch_seed`：平台重启计数（fencing epoch 种子，§1.5 推论 G）；
    /// - `lease_ttl_secs`/`lease_scope`：本地 grant 参数；
    /// - `policy`/`safety_points`：复验链策略与安全点（与签发侧同源装配）。
    ///
    /// 租约签发失败时本收口点生而为 default-deny（fail-closed，
    /// 见 [`lease_ok`](Self::lease_ok)）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: &[u8],
        lease_id: [u8; 16],
        lease_scope: LeaseScope,
        lease_ttl_secs: u64,
        epoch_seed: u64,
        policy: Policy,
        feedback_max_age_secs: u64,
        safety_points: &[(u32, u32, SafetyPoint)],
        now_ms: u64,
    ) -> Self {
        let mut chain = ControlChain::new(key, 5, 64, policy, feedback_max_age_secs, 300, 64, 30);
        // D7：fencing epoch 以平台重启计数为种子（签发任何租约之前）。
        chain.lease_service_mut().seed_epoch(epoch_seed);
        let lease_ok = chain
            .lease_service_mut()
            .issue(lease_id, lease_scope, lease_ttl_secs, now_ms / 1000)
            .is_some();
        for (device, point, spec) in safety_points {
            chain.safety_mut().register(*device, *point, *spec);
        }

        let mut metrics = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 1000);
        for (name, help) in WRITE_METRICS {
            // 名称/help 均为编译期常量，定容转换不可达失败；注册冲突静默跳过。
            if let (Ok(n), Ok(h), Ok(u)) = (
                heapless::String::<64>::try_from(name),
                heapless::String::<128>::try_from(help),
                heapless::String::<16>::try_from("count"),
            ) {
                let _ = metrics.register(MetricDef {
                    name: n,
                    metric_type: MetricType::Counter,
                    help: h,
                    unit: u,
                });
            }
        }

        // 租约签发入审计（§6.6：租约本地签发）。
        let mut audit = AuditLoggerImpl::new(MemStore::new(), [0u8; 32]);
        let _ = audit.log(AuditEvent::new(
            AuditEventType::LeaseGranted,
            "device-agent",
            "local-lease",
            &format!("lease_id={:02x?} epoch_seed={epoch_seed}", lease_id),
        ));

        Self {
            chain,
            key: key.to_vec(),
            lease_id,
            metrics,
            audit,
            write_count: 0,
            lease_ok,
        }
    }

    /// 本地租约是否签发成功（`false` = 生而为 default-deny）。
    pub fn lease_ok(&self) -> bool {
        self.lease_ok
    }

    /// 已成功落写计数。
    pub fn write_count(&self) -> u64 {
        self.write_count
    }

    /// 本地预置租约 ID（两侧同源，§6.7）。
    pub fn lease_id(&self) -> [u8; 16] {
        self.lease_id
    }

    /// 暴露复验链租约服务（装配/监督吊销用；吊销立即生效，墓碑不可逆，
    /// §1.5 单调性）。
    pub fn lease_service_mut(&mut self) -> &mut eneros_control_gateway::lease::LeaseService {
        self.chain.lease_service_mut()
    }

    /// 处理一帧信封投递（§6.2 状态机）。`payload` 为 EIPC 载荷
    /// （[`DeliveryFrame`] 定长 180 字节）；`now_ms` 注入时钟。
    ///
    /// 每一类失败都计数 + 审计，绝不 panic（错误纪律）。
    pub fn process(
        &mut self,
        devices: &mut DeviceRegistry,
        payload: &[u8],
        now_ms: u64,
    ) -> IngressOutcome {
        let now_secs = now_ms / 1000;

        // 1. 帧解码：投递帧长度不符 → 丢弃 + 计数（不喂给解码器）。
        let frame = match DeliveryFrame::decode(payload) {
            Ok(f) => f,
            Err(e) => return self.deny(DenyClass::Decode, &format!("frame:{e:?}"), None, now_ms),
        };

        // 信封结构解码（供 SBO 选择与审计 detail；HMAC 由链复验）。
        let env = match CommandEnvelope::decode(&frame.envelope) {
            Ok(e) => e,
            Err(e) => {
                return self.deny(DenyClass::Decode, &format!("envelope:{e:?}"), None, now_ms)
            }
        };
        let audit_ctx = AuditCtx {
            actor_kind: env.actor_kind as u8,
            issuer: &env.issuer,
            device_id: env.device_id as u32,
            point_id: env.point_id,
            correlation_id: &env.correlation_id,
        };

        // 2. ControlChain 复验（HMAC/租约/幂等/越权/策略PDP/安全，default-deny）。
        //    SBO 以"信封到达即选择+执行原子"处理（与签发侧 ChainCommandSink
        //    语义一致；两阶段人工 SBO 属就地操作员流程，非本路径）。
        self.chain.sbo_select(env.point_id, now_secs);
        let feedback = FeedbackBundle::fresh(true, 50.0, now_secs);
        match self.chain.execute(&frame.envelope, now_secs, &feedback) {
            ChainOutcome::Executed { cmd, .. } => {
                // 3. 路由：device_id → 适配器。
                let Some(info) = devices.find_by_device_id(cmd.device_id) else {
                    return self.deny(
                        DenyClass::Mismatch,
                        &format!("device_id={}", cmd.device_id),
                        Some(&audit_ctx),
                        now_ms,
                    );
                };
                // 4. 点匹配：point_id 必须在设备 point_map 内。
                if !info.point_map.values().any(|&v| v == cmd.point_id) {
                    return self.deny(
                        DenyClass::Mismatch,
                        &format!("point_id={}", cmd.point_id),
                        Some(&audit_ctx),
                        now_ms,
                    );
                }
                // 5. 授权落写（适配器二次匹配校验；功率载荷 ADR-029）。
                match info
                    .adapter
                    .write_authorized(&cmd, PointValue::Float(frame.power_kw))
                {
                    Ok(_outcome) => {
                        // 回读确认：WriteOutcome.echoed 可空；以适配器协议应答为准。
                        self.write_count += 1;
                        let _ = self.metrics.increment(METRIC_WRITE_ALLOWED, 1, &[], now_ms);
                        let _ = self.audit.log(AuditEvent::new(
                            AuditEventType::WriteExecuted,
                            actor_str_byte(cmd.actor_kind as u8),
                            &format!("device={} point={}", cmd.device_id, cmd.point_id),
                            &audit_ctx.detail("outcome=applied"),
                        ));
                        IngressOutcome::Applied {
                            device_id: cmd.device_id,
                            point_id: cmd.point_id,
                            power_kw: frame.power_kw,
                        }
                    }
                    Err(e) => {
                        // 适配器级拒绝：不匹配/未授权 → mismatch 桶；
                        // 协议失败 → failed_protocol 桶（D14：跳过本条）。
                        let class = match e {
                            WriteError::CommandMismatch | WriteError::Unauthorized => {
                                DenyClass::Mismatch
                            }
                            WriteError::Protocol(_) => DenyClass::Protocol,
                        };
                        self.deny(
                            class,
                            &format!("write_authorized:{e:?}"),
                            Some(&audit_ctx),
                            now_ms,
                        )
                    }
                }
            }
            ChainOutcome::IdempotentHit(_) => {
                // 幂等命中：返回先前结果，不二次执行（exactly-once）。
                let _ = self
                    .metrics
                    .increment(METRIC_IDEMPOTENT_HIT, 1, &[], now_ms);
                let _ = self.audit.log(AuditEvent::new(
                    AuditEventType::WriteExecuted,
                    actor_str_byte(env.actor_kind as u8),
                    &format!("device={} point={}", env.device_id, env.point_id),
                    &audit_ctx.detail("outcome=idempotent-hit"),
                ));
                IngressOutcome::IdempotentHit
            }
            ChainOutcome::DryRun { cmd, .. } => {
                // 策略允许但为干跑：不落写（PEP 执行 DryRun obligation）。
                let _ = self.audit.log(AuditEvent::new(
                    AuditEventType::WriteExecuted,
                    actor_str_byte(cmd.actor_kind as u8),
                    &format!("device={} point={}", cmd.device_id, cmd.point_id),
                    &audit_ctx.detail("outcome=dry-run"),
                ));
                IngressOutcome::DryRun
            }
            ChainOutcome::Denied(reason) => {
                let class = classify_deny(&reason);
                self.deny(class, &deny_reason_str(&reason), Some(&audit_ctx), now_ms)
            }
        }
    }

    /// 拒绝公共路径：指标 + 审计（detail 键值编码，含精确原因），绝不 panic。
    fn deny(
        &mut self,
        class: DenyClass,
        reason: &str,
        ctx: Option<&AuditCtx<'_>>,
        now_ms: u64,
    ) -> IngressOutcome {
        let _ = self.metrics.increment(class.metric(), 1, &[], now_ms);
        let detail = match ctx {
            Some(c) => format!("{} deny_reason={}", c.detail("outcome=denied"), reason),
            None => format!("deny_reason={reason}"),
        };
        let actor = ctx
            .map(|c| actor_str_byte(c.actor_kind))
            .unwrap_or("unknown");
        let target = ctx
            .map(|c| format!("device={} point={}", c.device_id, c.point_id))
            .unwrap_or_default();
        let _ = self.audit.log(AuditEvent::new(
            AuditEventType::WriteDenied,
            actor,
            &target,
            &detail,
        ));
        IngressOutcome::Denied {
            class,
            reason: String::from(reason),
        }
    }

    /// 旧匿名命令双轨（§6.2/6.6/D12）：收到不执行，记拒绝指标 + 审计。
    pub fn record_legacy_denied(&mut self, now_ms: u64) {
        let _ = self.metrics.increment(METRIC_DENIED_LEGACY, 1, &[], now_ms);
        let _ = self.audit.log(AuditEvent::new(
            AuditEventType::WriteDenied,
            "legacy",
            "device-agent",
            "outcome=denied deny_reason=legacy-anonymous",
        ));
    }

    /// 启动握手（§6.3）：返回 `{lease_id, fencing_token}` 的 HMAC 保护响应。
    ///
    /// 响应布局：`lease_id(16) ‖ fencing_token(u64 LE, 8) ‖ HMAC-SM3(32)`，
    /// HMAC 覆盖前 24 字节（与信封同源密钥）。租约未签发（fail-closed）
    /// 时返回 [`IngressError::LeaseIssueFailed`]。
    pub fn handshake_response(&mut self) -> Result<[u8; HANDSHAKE_LEN], IngressError> {
        if !self.lease_ok {
            return Err(IngressError::LeaseIssueFailed);
        }
        let token = self
            .chain
            .lease_service_mut()
            .token_of(&self.lease_id)
            .unwrap_or(0);
        if token == 0 {
            return Err(IngressError::LeaseIssueFailed);
        }
        let mut payload = [0u8; 24];
        payload[..16].copy_from_slice(&self.lease_id);
        payload[16..24].copy_from_slice(&token.to_le_bytes());
        let hmac = hmac_sm3(&self.key, &payload);
        let mut resp = [0u8; HANDSHAKE_LEN];
        resp[..24].copy_from_slice(&payload);
        resp[24..].copy_from_slice(&hmac);
        let _ = self.audit.log(AuditEvent::new(
            AuditEventType::HandshakeCompleted,
            "device-agent",
            "handshake",
            &format!("lease_id={:02x?} token={token}", self.lease_id),
        ));
        Ok(resp)
    }

    /// 指标当前值（刷入存储后读取；测试/观测用）。
    pub fn metric_total(&mut self, name: &str, now_ms: u64) -> u64 {
        let _ = self.metrics.maybe_flush(now_ms);
        self.metrics.store().get_latest(name).unwrap_or(0)
    }

    /// 审计链条目数（外部留痕可观测）。
    pub fn audit_entry_count(&self) -> u64 {
        self.audit.chain_state().entry_count
    }
}

/// 握手响应验证（签发方侧，WP-24.4 投递器复用）：校验 HMAC 后返回
/// `(lease_id, fencing_token)`；篡改/长度不符 → `None`。
pub fn verify_handshake_response(key: &[u8], resp: &[u8]) -> Option<([u8; 16], u64)> {
    if resp.len() != HANDSHAKE_LEN {
        return None;
    }
    let expected = hmac_sm3(key, &resp[..24]);
    if !ct_eq(&expected, &resp[24..]) {
        return None;
    }
    let mut lease_id = [0u8; 16];
    lease_id.copy_from_slice(&resp[..16]);
    let mut token = [0u8; 8];
    token.copy_from_slice(&resp[16..24]);
    Some((lease_id, u64::from_le_bytes(token)))
}

/// 审计上下文（信封已解码字段；detail 键值编码，§6.6）。
struct AuditCtx<'a> {
    actor_kind: u8,
    issuer: &'a [u8; 16],
    device_id: u32,
    point_id: PointId,
    correlation_id: &'a [u8; 16],
}

impl AuditCtx<'_> {
    fn detail(&self, outcome: &str) -> String {
        format!(
            "actor_kind={} issuer={:02x?} device_id={} point_id={} correlation_id={:02x?} {}",
            self.actor_kind,
            self.issuer,
            self.device_id,
            self.point_id,
            self.correlation_id,
            outcome
        )
    }
}

/// actor_kind → 审计 actor 字符串。
fn actor_str_byte(kind: u8) -> &'static str {
    match kind {
        0 => "operator",
        1 => "agent",
        2 => "ai",
        3 => "system",
        4 => "emergency",
        _ => "unknown",
    }
}

// ============================================================================
// 第二层收口测试（蓝图/24 §8.2：真 HMAC 密钥、禁 mock 验签——全部经真实
// ControlChain 复验路径；MockPointDevice 仅为测试设备）
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
    use eneros_control_gateway::lease::LeaseScope;
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;

    use super::*;
    use crate::device_type::DeviceType;
    use crate::registry::MockPointDevice;

    const KEY: &[u8] = b"w24-ingress-test-key-000000000000000000";
    const LEASE_ID: [u8; 16] = [0x33; 16];
    const ISSUER: [u8; 16] = [0xCD; 16];
    /// 模拟平台重启计数（fencing epoch 种子，§6.3）。
    const EPOCH_SEED: u64 = 100;
    const NOW_MS: u64 = 1_700_000_000_000;

    fn now_secs() -> u64 {
        NOW_MS / 1000
    }

    fn policy_allow(device: u16, point: u32) -> Policy {
        Policy::new(3).with_rule(PolicyRule {
            actor_kind: Some(ActorKind::Ai),
            devices: vec![device as u32],
            points: vec![point],
            kinds: vec![CommandClass::Switch],
            time_window: (0, 0),
            obligations: vec![],
        })
    }

    fn safety_for(device: u16, point: u32) -> Vec<(u32, u32, SafetyPoint)> {
        vec![(
            device as u32,
            point,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: PointSpec {
                    point_id: point,
                    device_id: device,
                    unit: PhysicalUnit::Kilowatts,
                    safe_min: -500.0,
                    safe_max: 500.0,
                    max_ramp_per_sec: None,
                    direction: ControlDirection::Discrete,
                    remote_controllable: true,
                },
            },
        )]
    }

    fn ingress_with(device: u16, point: u32) -> EnvelopeIngress {
        EnvelopeIngress::new(
            KEY,
            LEASE_ID,
            LeaseScope {
                devices: vec![device as u32],
                points: vec![point],
                max_class: 1,
            },
            3_600,
            EPOCH_SEED,
            policy_allow(device, point),
            60,
            &safety_for(device, point),
            NOW_MS,
        )
    }

    /// 注册含 power_setpoint 点的测试设备（device_id 可定制）。
    fn registry_with(device_id: u16, offline: bool) -> DeviceRegistry {
        let mut dev = MockPointDevice::new(device_id).with_point("power_setpoint", 0.0);
        dev.set_offline(offline);
        let mut pm = BTreeMap::new();
        if let Some(id) = dev.point_id_of("power_setpoint") {
            pm.insert(String::from("power_setpoint"), id);
        }
        let mut reg = DeviceRegistry::new();
        reg.register("pcs", DeviceType::Pcs, device_id, Box::new(dev), pm);
        reg
    }

    /// 构造签名信封（真 HMAC；幂等键/过期偏移可定制）。
    #[allow(clippy::too_many_arguments)]
    fn signed_envelope(
        key: &[u8],
        device: u32,
        point: u32,
        token: u64,
        idem_key: [u8; 16],
        expiry_offset_secs: u64,
        seq: u64,
    ) -> [u8; 172] {
        let mut id_bytes = [0u8; 16];
        id_bytes[..8].copy_from_slice(&seq.to_le_bytes());
        EnvelopeBuilder::new(
            id_bytes,
            device,
            point,
            ControlCommand::Single(SingleCommand::On),
            now_secs(),
            now_secs().saturating_add(expiry_offset_secs),
            ISSUER,
        )
        .lease(LEASE_ID, token)
        .policy_version(3)
        .sequence(seq)
        .actor_kind(ActorKind::Ai)
        .idempotency_key(idem_key)
        .build(key)
        .encode()
    }

    fn delivery(envelope: [u8; 172], power_kw: f64) -> Vec<u8> {
        DeliveryFrame { envelope, power_kw }.encode().to_vec()
    }

    // ===== 用例 1：有效信封落写（含回读） =====
    #[test]
    fn valid_envelope_applied_with_readback() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        assert!(ingress.lease_ok());

        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .expect("lease issued");
        let env = signed_envelope(KEY, 1, 1, token, [0x11; 16], 30, 1);
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Applied {
                device_id: 1,
                point_id: 1,
                power_kw: 50.0,
            }
        );
        // 回读确认：设备点值已更新。
        let info = devices.get_mut("pcs").expect("pcs");
        let id = *info.point_map.get("power_setpoint").expect("point");
        let dp = info.adapter.read_point(id).expect("read");
        assert_eq!(dp.value, PointValue::Float(50.0));
        // 指标 + 审计留痕。
        assert_eq!(ingress.metric_total(METRIC_WRITE_ALLOWED, NOW_MS), 1);
        assert_eq!(ingress.write_count(), 1);
        assert_eq!(ingress.audit_entry_count(), 2); // LeaseGranted + WriteExecuted
    }

    // ===== 用例 2：篡改信封被拒（HMAC，疑似伪造）+ 审计留痕 =====
    #[test]
    fn tampered_envelope_denied_hmac() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();

        let mut env = signed_envelope(KEY, 1, 1, token, [0x12; 16], 30, 1);
        env[45] ^= 0xFF; // 篡改 device_id 字节（HMAC 覆盖区）
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::Hmac,
                reason: String::from("gateway:Unauthorized"),
            }
        );
        assert_eq!(ingress.metric_total(METRIC_DENIED_HMAC, NOW_MS), 1);
        // 设备状态不变。
        let info = devices.get_mut("pcs").unwrap();
        let id = *info.point_map.get("power_setpoint").unwrap();
        assert_eq!(
            info.adapter.read_point(id).unwrap().value,
            PointValue::Float(0.0)
        );
        assert_eq!(ingress.audit_entry_count(), 2); // LeaseGranted + WriteDenied
    }

    // ===== 用例 3：过期信封被拒（时间性拒绝 → lease 桶） =====
    #[test]
    fn expired_envelope_denied() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();
        let env = signed_envelope(KEY, 1, 1, token, [0x13; 16], 0, 1); // 立即过期
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS + 20_000);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::Lease,
                reason: String::from("gateway:Expired")
            }
        );
        assert_eq!(
            ingress.metric_total(METRIC_DENIED_LEASE, NOW_MS + 20_000),
            1
        );
        assert_eq!(ingress.write_count(), 0);
    }

    // ===== 用例 4：吊销租约拒（立即生效，墓碑） =====
    #[test]
    fn revoked_lease_denied_immediately() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();

        // 先放行一条（基线）。
        let env1 = signed_envelope(KEY, 1, 1, token, [0x14; 16], 30, 1);
        assert!(matches!(
            ingress.process(&mut devices, &delivery(env1, 10.0), NOW_MS),
            IngressOutcome::Applied { .. }
        ));

        // 吊销 → 立即生效；同一 token 重发被拒。
        assert!(ingress.lease_service_mut().revoke(&LEASE_ID));
        let env2 = signed_envelope(KEY, 1, 1, token, [0x15; 16], 30, 2);
        let out = ingress.process(&mut devices, &delivery(env2, 20.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::Lease,
                reason: String::from("lease-revoked")
            }
        );
        assert_eq!(ingress.metric_total(METRIC_DENIED_LEASE, NOW_MS), 1);
        // 设备值保持第一条（10.0），未被第二条触碰。
        let info = devices.get_mut("pcs").unwrap();
        let id = *info.point_map.get("power_setpoint").unwrap();
        assert_eq!(
            info.adapter.read_point(id).unwrap().value,
            PointValue::Float(10.0)
        );
    }

    // ===== 用例 5：stale token 拒（fencing 单调性） =====
    #[test]
    fn stale_token_denied() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();
        let env = signed_envelope(KEY, 1, 1, token - 1, [0x16; 16], 30, 1);
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::StaleToken,
                reason: String::from("stale-fencing")
            }
        );
        assert_eq!(ingress.metric_total(METRIC_DENIED_STALE_TOKEN, NOW_MS), 1);
        assert_eq!(ingress.write_count(), 0);
    }

    // ===== 用例 6：幂等重放返回先前结果，不二次执行（exactly-once） =====
    #[test]
    fn idempotent_replay_returns_prior_result() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, false);
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();

        let env1 = signed_envelope(KEY, 1, 1, token, [0x21; 16], 30, 1);
        assert!(matches!(
            ingress.process(&mut devices, &delivery(env1, 50.0), NOW_MS),
            IngressOutcome::Applied { .. }
        ));

        // 重放：新 command_id、同 idempotency_key、不同功率 → 幂等命中。
        let env2 = signed_envelope(KEY, 1, 1, token, [0x21; 16], 30, 2);
        let out = ingress.process(&mut devices, &delivery(env2, 99.0), NOW_MS);
        assert_eq!(out, IngressOutcome::IdempotentHit);
        assert_eq!(ingress.metric_total(METRIC_IDEMPOTENT_HIT, NOW_MS), 1);
        // 不二次执行：设备值保持 50.0。
        let info = devices.get_mut("pcs").unwrap();
        let id = *info.point_map.get("power_setpoint").unwrap();
        assert_eq!(
            info.adapter.read_point(id).unwrap().value,
            PointValue::Float(50.0)
        );
        assert_eq!(ingress.write_count(), 1);
    }

    // ===== 用例 7：设备/点不匹配拒（路由层） =====
    #[test]
    fn mismatched_device_denied() {
        // 链放行 device 5/point 1，但注册表无 device 5 → 路由不匹配。
        let mut ingress = ingress_with(5, 1);
        let mut devices = registry_with(1, false); // 只有 device 1
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();
        let env = signed_envelope(KEY, 5, 1, token, [0x17; 16], 30, 1);
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::Mismatch,
                reason: String::from("device_id=5"),
            }
        );
        assert_eq!(ingress.metric_total(METRIC_DENIED_MISMATCH, NOW_MS), 1);
        assert_eq!(ingress.write_count(), 0);
    }

    // ===== 用例 8：协议写失败（离线设备）→ failed_protocol，跳过本条 =====
    #[test]
    fn protocol_failure_denied_and_skipped() {
        let mut ingress = ingress_with(1, 1);
        let mut devices = registry_with(1, true); // 离线
        let token = ingress
            .chain
            .lease_service_mut()
            .token_of(&LEASE_ID)
            .unwrap();
        let env = signed_envelope(KEY, 1, 1, token, [0x18; 16], 30, 1);
        let out = ingress.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::Protocol,
                reason: String::from("write_authorized:Protocol(WriteFailed)")
            }
        );
        assert_eq!(ingress.metric_total(METRIC_FAILED_PROTOCOL, NOW_MS), 1);
        assert_eq!(ingress.write_count(), 0);
    }

    // ===== 用例 9：握手返回 {lease_id, fencing_token} 且 HMAC 保护 =====
    #[test]
    fn handshake_returns_lease_and_token_hmac_protected() {
        let mut ingress = ingress_with(1, 1);
        let resp = ingress.handshake_response().expect("handshake");
        assert_eq!(resp.len(), HANDSHAKE_LEN);
        let (lease_id, token) = verify_handshake_response(KEY, &resp).expect("verify");
        assert_eq!(lease_id, LEASE_ID);
        // D7：token 以重启计数为种子铸造（epoch_seed=100 → 首签 token 101）。
        assert_eq!(token, EPOCH_SEED + 1);
        // 篡改响应 → 验证失败；错误密钥 → 验证失败。
        let mut tampered = resp;
        tampered[0] ^= 0xFF;
        assert_eq!(verify_handshake_response(KEY, &tampered), None);
        assert_eq!(verify_handshake_response(b"wrong-key", &resp), None);
        // 握手完成入审计。
        assert_eq!(ingress.audit_entry_count(), 2); // LeaseGranted + HandshakeCompleted
    }

    // ===== 用例 10：租约 epoch 单调性——重启后新 token 高于旧 token =====
    #[test]
    fn epoch_seed_makes_tokens_monotonic_across_restarts() {
        let mut boot1 = ingress_with(1, 1);
        let t1 = boot1.chain.lease_service_mut().token_of(&LEASE_ID).unwrap();
        // 重启（重启计数 +1）：同一 lease_id 重新 grant（吊销旧墓碑语义由
        // 监督层保证；此处验证 token 严格递增）。
        let mut boot2 = EnvelopeIngress::new(
            KEY,
            LEASE_ID,
            LeaseScope {
                devices: vec![1],
                points: vec![1],
                max_class: 1,
            },
            3_600,
            EPOCH_SEED + 1,
            policy_allow(1, 1),
            60,
            &safety_for(1, 1),
            NOW_MS,
        );
        let t2 = boot2.chain.lease_service_mut().token_of(&LEASE_ID).unwrap();
        assert!(t2 > t1, "重启计数种子必须使 fencing token 单调递增");
        // 旧 token 在新 epoch 下为 stale。
        let mut devices = registry_with(1, false);
        let env = signed_envelope(KEY, 1, 1, t1, [0x19; 16], 30, 1);
        let out = boot2.process(&mut devices, &delivery(env, 50.0), NOW_MS);
        assert_eq!(
            out,
            IngressOutcome::Denied {
                class: DenyClass::StaleToken,
                reason: String::from("stale-fencing")
            }
        );
    }
}
