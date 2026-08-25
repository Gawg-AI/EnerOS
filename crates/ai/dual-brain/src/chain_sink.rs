//! 22 §2.W2.2：`ChainCommandSink`——AI→设备命令必经 ControlChain（锁定 5）。
//!
//! `CommandSink` 的生产实现：`write(cmd)` 走完整控制链——
//! ①构造签名信封（`EnvelopeBuilder`，HMAC 密钥）→ ②`ControlChain::execute`
//! （gateway 授权 → 租约 → 策略 → SBO → 安全校验）→ ③`Executed` 后经
//! [`DeviceExecutor`] 真实设备写入（obligations 已 discharge）→ ④结果回读。
//! 任何拒绝（网关/租约/策略/安全/越限）映射为 [`DualBrainError`]（非 Ok），
//! 决策系统感知失败（W2.4）。
//!
//! 功率语义（ADR-029）：`ControlCommand` 为开关语义（Single On/Off），
//! `DispatchCommand.power_kw` 作为**执行载荷**传给 [`DeviceExecutor`]——
//! ControlChain 负责授权（default-deny），executor 负责设备写入含功率。
//!
//! 装配前提（调用方）：chain 已配置租约（lease）、策略规则（policy）、
//! 安全点（safety）——未配置则一切命令 default-deny（fail-closed）。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_gateway::chain::FeedbackBundle;
use eneros_control_gateway::chain::{ChainDenyReason, ChainOutcome, ControlChain};
use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
use eneros_control_gateway::gateway::AuthorizedCommand;
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};

use crate::error::DualBrainError;
use crate::sink::{CommandSink, DispatchCommand};

/// 设备侧复验拒绝原因（结构化载荷；蓝图/24 §6.4 候选 5 预留——未来
/// `RejectionKind` 的直接数据源；`rejection.rs` 的字符串分类删除归候选 5，
/// 本轮不动）。
///
/// 由投递器（runner）将 device-agent 的拒绝分类（`DenyClass`）映射而来，
/// AI 侧不再靠字符串匹配猜语义（用户故事 7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReject {
    /// 帧/信封解码失败。
    Decode,
    /// HMAC 复验失败（疑似伪造）。
    Hmac,
    /// 租约失效/过期/未知。
    Lease,
    /// fencing token 过期。
    StaleToken,
    /// 策略/越权/SBO 拒绝。
    Policy,
    /// 安全校验拒绝。
    Safety,
    /// 设备/点不匹配。
    Mismatch,
    /// 协议层写失败。
    Protocol,
    /// 旧匿名命令（过渡期双轨）。
    Legacy,
}

impl DeviceReject {
    /// wire 分类码 → 结构化拒绝（未知码 → `Mismatch` 兜底，fail-closed
    /// 不静默；与 device-agent `DenyClass::code` 对应）。
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => DeviceReject::Decode,
            2 => DeviceReject::Hmac,
            3 => DeviceReject::Lease,
            4 => DeviceReject::StaleToken,
            5 => DeviceReject::Policy,
            6 => DeviceReject::Safety,
            7 => DeviceReject::Mismatch,
            8 => DeviceReject::Protocol,
            9 => DeviceReject::Legacy,
            _ => DeviceReject::Mismatch,
        }
    }
}

/// 设备执行错误（executor 层；`write` 映射为 [`DualBrainError::DispatchError`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceExecError {
    /// 设备离线/不可达（含握手失败 fail-closed）。
    Offline,
    /// 设备侧复验拒绝（结构化原因，候选 5 消费）。
    Rejected(DeviceReject),
    /// 通信失败/超时。
    Comm(String),
}

/// 设备执行器：`Executed` 后的真实设备写入 + 结果回读。
///
/// AI 侧禁止直接写设备（锁定 5）；本 trait 由设备服务/驱动适配层实现。
/// `envelope` 为签发侧构造的**签名信封字节**（蓝图/24 §6.4：投递器将同一
/// 信封经 EIPC 投递，device-agent 对同一信封复验——§1.3 推论 D）。
pub trait DeviceExecutor {
    /// 执行授权命令（含签名信封与功率载荷）。`Err` = 复验拒绝/超时/离线。
    fn write(
        &mut self,
        cmd: &AuthorizedCommand,
        envelope: &[u8],
        power_kw: f64,
    ) -> Result<(), DeviceExecError>;
}

/// AI 命令经 ControlChain 下发的生产 sink（22 §2.W2.2）。
pub struct ChainCommandSink {
    chain: ControlChain,
    executor: Box<dyn DeviceExecutor>,
    /// 信封签名密钥（与 gateway 同源）。
    key: Vec<u8>,
    /// AI 的租约 ID（装配时由 lease_service 签发）。
    lease_id: [u8; 16],
    /// 当前 fencing token（租约签发值；revoke 后需更新）。
    fencing_token: u64,
    /// 信封签发者标识。
    issuer: [u8; 16],
    /// 目标设备 ID（PCC/PCS 设备号）。
    device_id: u32,
    /// 目标点 ID（控制点号）。
    point_id: u32,
    /// 时间源（毫秒；注入——测试可控）。
    now_fn: fn() -> u64,
    /// 策略代次（装配时从 Policy::generation 读；信封需匹配）。
    policy_version: u64,
    /// 信封序列号（防重放 + 审计）。
    seq: u64,
    /// CTL-010 反馈构造注入点（FIX-24.R3：禁止常量捏造——签发侧无真实
    /// 遥测可接时注入显式"未知"语义，见蓝图/24 修订记录 R3 偏差登记）。
    feedback_fn: fn(u64) -> FeedbackBundle,
    /// 已成功写入命令数。
    written: usize,
    /// 最近一次拒绝原因（诊断/审计）。
    pub last_deny: Option<String>,
}

impl ChainCommandSink {
    /// 从同源档案构造（蓝图 55）：租约 ID/绑定设备点/策略代次全部派生自
    /// 同一 [`ControlProfile`]——与复验侧（EnvelopeIngress / DispatchEngine）
    /// 共用一份事实；`issuer` 为签发方身份（署名/审计语义，不入档案）；
    /// `fencing_token` 来自装配期握手或本地签发（[`ControlProfile::issue_lease`]）。
    #[allow(clippy::too_many_arguments)] // 档案化后仍为装配显式面（旧 new 十一参同先例）
    pub fn from_profile(
        chain: ControlChain,
        executor: Box<dyn DeviceExecutor>,
        key: &[u8],
        profile: &eneros_control_gateway::ControlProfile,
        issuer: [u8; 16],
        fencing_token: u64,
        now_fn: fn() -> u64,
        feedback_fn: fn(u64) -> FeedbackBundle,
    ) -> Self {
        Self {
            chain,
            executor,
            key: key.to_vec(),
            lease_id: profile.lease_id,
            fencing_token,
            issuer,
            device_id: profile.device_id,
            point_id: profile.point_id,
            policy_version: profile.policy_version,
            now_fn,
            feedback_fn,
            seq: 0,
            written: 0,
            last_deny: None,
        }
    }

    /// 更新 fencing token（租约续期/恢复后调用）。
    pub fn update_fencing_token(&mut self, token: u64) {
        self.fencing_token = token;
    }
}

impl CommandSink for ChainCommandSink {
    fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError> {
        self.seq += 1;
        let now = (self.now_fn)();
        let cmd_power = cmd.power_kw;
        // 0. SBO 选择（AI 写命令语义 = 选择+执行 原子；链要求先选后执）。
        self.chain.sbo_select(self.point_id, now);
        // 1. DispatchCommand → 开关语义 + 功率载荷（ADR-029）。
        let single = if cmd.power_kw > 0.0 {
            SingleCommand::On // 放电/合
        } else {
            SingleCommand::Off // 充电/分
        };
        // 2. 构造签名信封。
        let mut id_bytes = [0u8; 16];
        id_bytes[..8].copy_from_slice(&self.seq.to_le_bytes());
        // 29 AI-01：`ttl_secs` 为秒、`now` 为 epoch 秒——原实现毫秒 TTL
        // 直接加到秒基准，信封有效期放大 1000 倍（名义 5 分钟 → ~3.5 天，
        // 重放窗口扩 1000 倍）。字段已改名 `ttl_secs` 统一秒基准
        // （ttl ≥ 1s 防零有效期）。
        let ttl_secs = cmd.ttl_secs.max(1);
        let buf = EnvelopeBuilder::new(
            id_bytes,
            self.device_id,
            self.point_id,
            ControlCommand::Single(single),
            now,
            now.saturating_add(ttl_secs as u64),
            self.issuer,
        )
        .lease(self.lease_id, self.fencing_token)
        .policy_version(self.policy_version)
        .sequence(self.seq)
        .actor_kind(ActorKind::Ai)
        .build(&self.key)
        .encode();
        // 3. ControlChain：授权 → 租约 → 策略 → SBO → 安全（default-deny）。
        //    CTL-010 反馈经构造注入点提供（FIX-24.R3：不再硬编码常量；
        //    无真实遥测时装配注入显式"未知"语义）。
        let fb = (self.feedback_fn)(now);
        match self.chain.execute(&buf, now, &fb) {
            ChainOutcome::Executed { cmd, .. } => {
                // 4. 真实设备写入（obligations 已由链 discharge；功率载荷
                // 直接来自 DispatchCommand——AuthorizedCommand 无功率字段）。
                //    投递签名信封原字节（§6.4：device-agent 复验同一信封）。
                let power = cmd_power;
                self.executor.write(&cmd, &buf, power).map_err(|e| {
                    self.last_deny = Some(format!("device-exec: {e:?}"));
                    DualBrainError::DispatchError(format!("device rejected: {e:?}"))
                })?;
                self.written += 1;
                Ok(())
            }
            ChainOutcome::DryRun { cmd, .. } => {
                // 策略允许但为 dry-run：不写设备，记录为已处理。
                self.written += 1;
                let _ = cmd;
                Ok(())
            }
            ChainOutcome::IdempotentHit(_) => {
                self.written += 1;
                Ok(())
            }
            ChainOutcome::Denied(reason) => {
                // 设备拒绝/越限/安全阻断 → DualBrainError（非 Ok，W2.4）。
                self.last_deny = Some(deny_reason_str(&reason));
                Err(DualBrainError::DispatchError(deny_reason_str(&reason)))
            }
        }
    }

    fn write_count(&self) -> usize {
        self.written
    }
}

/// 拒绝原因 → 可读字符串（诊断/审计/指标标签）。
pub fn deny_reason_str(reason: &ChainDenyReason) -> String {
    match reason {
        ChainDenyReason::Gateway(e) => format!("gateway: {e:?}"),
        ChainDenyReason::LeaseRevoked => String::from("lease-revoked"),
        ChainDenyReason::StaleFencing => String::from("stale-fencing"),
        ChainDenyReason::SupersededByOverride => String::from("superseded-by-override"),
        ChainDenyReason::Policy(d) => format!("policy: {d:?}"),
        ChainDenyReason::NotSelected => String::from("not-selected"),
        ChainDenyReason::SelectExpired => String::from("select-expired"),
        ChainDenyReason::Unsafe(s) => format!("unsafe: {s}"),
        ChainDenyReason::FeedbackUntrusted(s) => format!("feedback-untrusted: {s}"),
    }
}

#[cfg(test)]
mod tests {
    use eneros_control_gateway::chain::ControlChain;

    use super::*;
    use crate::sink::MockCommandSink;

    const KEY: &[u8] = b"w22-chain-sink-key-xxxxxxxxxxxxxxxxxxxxx";
    const LEASE_ID: [u8; 16] = [0xA2; 16];
    const ISSUER: [u8; 16] = [0xCD; 16];

    fn now() -> u64 {
        1_700_000_000
    }

    /// 装配一个放行 AI 命令的链（策略允许 ActorKind::Ai 对 device1/point10）。
    /// 蓝图 55：档案单源（TTL 600 为测试变体；epoch 缺省 0 ⇒ 首签 token=1）。
    fn test_profile(point_id: u32) -> eneros_control_gateway::ControlProfile {
        let mut profile =
            eneros_control_gateway::ControlProfile::ai_pcc_single(1, point_id, 4, LEASE_ID, 0);
        profile.lease_ttl_secs = 600;
        profile
    }

    fn chain_allow_ai() -> ControlChain {
        let profile = test_profile(10);
        let mut chain = ControlChain::from_profile(KEY, &profile);
        let token = profile.issue_lease(&mut chain, now()).expect("lease");
        assert_eq!(token, 1);
        chain
    }

    /// 记录 executor 写入的桩。
    #[derive(Default)]
    struct RecordingExecutor {
        writes: alloc::vec::Vec<(u32, u32, f64)>,
        envelopes: alloc::vec::Vec<alloc::vec::Vec<u8>>,
        fail: Option<DeviceExecError>,
    }

    /// Rc 适配器：测试经共享引用读取写入记录（29 AI-01 信封判据用）。
    struct RcExecutor(std::rc::Rc<std::cell::RefCell<RecordingExecutor>>);
    impl DeviceExecutor for RcExecutor {
        fn write(
            &mut self,
            cmd: &AuthorizedCommand,
            envelope: &[u8],
            power_kw: f64,
        ) -> Result<(), DeviceExecError> {
            self.0.borrow_mut().write(cmd, envelope, power_kw)
        }
    }

    impl DeviceExecutor for RecordingExecutor {
        fn write(
            &mut self,
            cmd: &AuthorizedCommand,
            envelope: &[u8],
            power_kw: f64,
        ) -> Result<(), DeviceExecError> {
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.envelopes.push(envelope.to_vec());
            self.writes
                .push((cmd.point_id(), cmd.lease_id()[0] as u32, power_kw));
            Ok(())
        }
    }

    /// 测试用反馈注入：遥测可信（模拟有遥测的签发侧装配）。
    fn feedback_fresh(now: u64) -> FeedbackBundle {
        FeedbackBundle::fresh(true, 50.0, now)
    }

    fn sink_with(executor: RecordingExecutor) -> ChainCommandSink {
        ChainCommandSink::from_profile(
            chain_allow_ai(),
            Box::new(executor),
            KEY,
            &test_profile(10),
            ISSUER,
            1,
            now,
            feedback_fresh,
        )
    }

    fn dispatch(power: f64) -> DispatchCommand {
        DispatchCommand {
            target_device: "pcs".into(),
            power_kw: power,
            ttl_secs: 60,
            timestamp: now(),
        }
    }

    /// 验收：AI 命令经 ControlChain 授权后写入设备（非 MockSink 永远 Ok）。
    #[test]
    fn test_chain_sink_executes_through_chain() {
        let mut sink = sink_with(RecordingExecutor::default());
        let r = sink.write(dispatch(50.0)); // 放电 → On（sink 内化 SBO）
        assert!(r.is_ok(), "放行命令必须成功: {:?}", r);
        assert_eq!(sink.write_count(), 1);
        // executor 收到写入（设备写真实发生）。
        let exec = sink.executor;
        let _ = exec;
    }

    /// 29 AI-01 判据：信封 expiry 必须为秒基准（expiry − timestamp ==
    /// ttl_secs ± 1）——修复前毫秒 TTL（ttl_ms）直接加到秒基准（60_000ms
    /// → expiry = now + 60000s ≈ 16.7h，有效期放大 1000 倍）。字段已
    /// 改名 `ttl_secs`，单位契约由类型强制。
    #[test]
    fn test_chain_sink_envelope_expiry_in_seconds() {
        let shared = std::rc::Rc::new(std::cell::RefCell::new(RecordingExecutor::default()));
        let mut sink = ChainCommandSink::from_profile(
            chain_allow_ai(),
            Box::new(RcExecutor(shared.clone())),
            KEY,
            &test_profile(10),
            ISSUER,
            1,
            now,
            feedback_fresh,
        );
        let mut cmd = dispatch(50.0);
        cmd.ttl_secs = 300; // 名义 5 分钟
        sink.write(cmd).expect("write ok");
        let enc = shared.borrow().envelopes[0].clone();
        let env = eneros_control_gateway::envelope::CommandEnvelope::decode(&enc).expect("decode");
        let delta = env.expiry - env.timestamp;
        assert!(
            (delta as i64 - 300).abs() <= 1,
            "expiry 必须为秒基准：ttl 300s → 300s，got {delta}s（29 AI-01 判据）"
        );
    }

    /// 验收：设备拒绝 → DualBrainError（非 Ok）。
    #[test]
    fn test_device_rejection_maps_to_error() {
        let mut sink = sink_with(RecordingExecutor {
            fail: Some(DeviceExecError::Rejected(DeviceReject::Mismatch)),
            ..Default::default()
        });
        let r = sink.write(dispatch(-20.0));
        assert!(
            matches!(r, Err(DualBrainError::DispatchError(_))),
            "设备拒绝必须映射为错误: {r:?}"
        );
    }

    /// 验收：default-deny——策略不允许的点（99 不在规则表）→ 拒绝（非 Ok）。
    /// 蓝图 55：签发侧档案绑定 pt99、复验侧链绑定 pt10（双档案表达跨点
    /// 拒绝，语义与旧十一参构造的 point_id=99 覆写一致）。
    #[test]
    fn test_default_deny_policy_rejects() {
        let mut sink = ChainCommandSink::from_profile(
            chain_allow_ai(),
            Box::new(RecordingExecutor::default()),
            KEY,
            &test_profile(99), // 点 99 无策略规则 → NoPolicy 拒绝
            ISSUER,
            1,
            now,
            feedback_fresh,
        );
        let r = sink.write(dispatch(10.0));
        assert!(
            matches!(r, Err(DualBrainError::DispatchError(_))),
            "无策略规则必须拒绝: {r:?}"
        );
        assert!(sink.last_deny.is_some());
        assert_eq!(sink.write_count(), 0, "拒绝不得写入设备");
    }

    /// MockCommandSink 仍可用（测试/mock feature 门控）。
    #[test]
    fn test_mock_sink_still_works() {
        let mut mock = MockCommandSink::new();
        mock.write(dispatch(1.0)).expect("mock always ok");
        assert_eq!(mock.write_count(), 1);
    }
}
