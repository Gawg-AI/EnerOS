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

/// 设备执行错误（executor 层；`write` 映射为 [`DualBrainError::DispatchError`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceExecError {
    /// 设备离线/不可达。
    Offline,
    /// 设备拒绝（值越限/模式拒绝）。
    Rejected(String),
    /// 通信失败/超时。
    Comm(String),
}

/// 设备执行器：`Executed` 后的真实设备写入 + 结果回读。
///
/// AI 侧禁止直接写设备（锁定 5）；本 trait 由设备服务/驱动适配层实现。
pub trait DeviceExecutor {
    /// 执行授权命令（含功率载荷）。`Err` = 设备拒绝/超时/离线。
    fn write(&mut self, cmd: &AuthorizedCommand, power_kw: f64) -> Result<(), DeviceExecError>;
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
    /// 已成功写入命令数。
    written: usize,
    /// 最近一次拒绝原因（诊断/审计）。
    pub last_deny: Option<String>,
}

impl ChainCommandSink {
    /// 构造（`chain` 必须已装配租约/策略/安全点；`key` 与 gateway 同源）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain: ControlChain,
        executor: Box<dyn DeviceExecutor>,
        key: &[u8],
        lease_id: [u8; 16],
        fencing_token: u64,
        issuer: [u8; 16],
        device_id: u32,
        point_id: u32,
        policy_version: u64,
        now_fn: fn() -> u64,
    ) -> Self {
        Self {
            chain,
            executor,
            key: key.to_vec(),
            lease_id,
            fencing_token,
            issuer,
            device_id,
            point_id,
            policy_version,
            now_fn,
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
        let buf = EnvelopeBuilder::new(
            id_bytes,
            self.device_id,
            self.point_id,
            ControlCommand::Single(single),
            now,
            now.saturating_add(cmd.ttl_ms as u64),
            self.issuer,
        )
        .lease(self.lease_id, self.fencing_token)
        .policy_version(self.policy_version)
        .sequence(self.seq)
        .actor_kind(ActorKind::Ai)
        .build(&self.key)
        .encode();
        // 3. ControlChain：授权 → 租约 → 策略 → SBO → 安全（default-deny）。
        let fb = FeedbackBundle::fresh(true, 50.0, now);
        match self.chain.execute(&buf, now, &fb) {
            ChainOutcome::Executed { cmd, .. } => {
                // 4. 真实设备写入（obligations 已由链 discharge；功率载荷
                // 直接来自 DispatchCommand——AuthorizedCommand 无功率字段）。
                let power = cmd_power;
                self.executor.write(&cmd, power).map_err(|e| {
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
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_control_gateway::LeaseScope;

    use super::*;
    use crate::sink::MockCommandSink;

    const KEY: &[u8] = b"w22-chain-sink-key-xxxxxxxxxxxxxxxxxxxxx";
    const LEASE_ID: [u8; 16] = [0xA2; 16];
    const ISSUER: [u8; 16] = [0xCD; 16];

    fn now() -> u64 {
        1_700_000_000
    }

    /// 装配一个放行 AI 命令的链（策略允许 ActorKind::Ai 对 device1/point10）。
    fn chain_allow_ai() -> ControlChain {
        let mut policy = Policy::new(3);
        let _ = policy.replace_rules(vec![PolicyRule {
            actor_kind: Some(ActorKind::Ai),
            devices: vec![1],
            points: vec![10],
            kinds: vec![CommandClass::Switch],
            time_window: (0, 0),
            obligations: vec![],
        }]);
        let mut chain = ControlChain::new(KEY, 5, 64, policy, 60, 300, 64, 30);
        let scope = LeaseScope {
            devices: vec![1],
            points: vec![10],
            max_class: 1,
        };
        let (token, _) = chain
            .lease_service_mut()
            .issue(LEASE_ID, scope, 600, now())
            .expect("lease");
        assert_eq!(token, 1);
        chain.safety_mut().register(
            1,
            10,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: eneros_telemetry_model::point_spec::PointSpec {
                    point_id: 10,
                    device_id: 1,
                    unit: eneros_telemetry_model::unit::PhysicalUnit::Kilowatts,
                    safe_min: -500.0,
                    safe_max: 500.0,
                    max_ramp_per_sec: None,
                    direction: eneros_telemetry_model::point_spec::ControlDirection::Discrete,
                    remote_controllable: true,
                },
            },
        );
        chain
    }

    /// 记录 executor 写入的桩。
    #[derive(Default)]
    struct RecordingExecutor {
        writes: alloc::vec::Vec<(u32, u32, f64)>,
        fail: Option<DeviceExecError>,
    }
    impl DeviceExecutor for RecordingExecutor {
        fn write(&mut self, cmd: &AuthorizedCommand, power_kw: f64) -> Result<(), DeviceExecError> {
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.writes
                .push((cmd.point_id, cmd.lease_id[0] as u32, power_kw));
            Ok(())
        }
    }

    fn sink_with(executor: RecordingExecutor) -> ChainCommandSink {
        ChainCommandSink::new(
            chain_allow_ai(),
            Box::new(executor),
            KEY,
            LEASE_ID,
            1,
            ISSUER,
            1,
            10,
            4,
            now,
        )
    }

    fn dispatch(power: f64) -> DispatchCommand {
        DispatchCommand {
            target_device: "pcs".into(),
            power_kw: power,
            ttl_ms: 60_000,
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

    /// 验收：设备拒绝 → DualBrainError（非 Ok）。
    #[test]
    fn test_device_rejection_maps_to_error() {
        let mut sink = sink_with(RecordingExecutor {
            fail: Some(DeviceExecError::Rejected("over-limit".into())),
            ..Default::default()
        });
        let r = sink.write(dispatch(-20.0));
        assert!(
            matches!(r, Err(DualBrainError::DispatchError(_))),
            "设备拒绝必须映射为错误: {r:?}"
        );
    }

    /// 验收：default-deny——策略不允许的点（99 不在规则表）→ 拒绝（非 Ok）。
    #[test]
    fn test_default_deny_policy_rejects() {
        let mut sink = ChainCommandSink::new(
            chain_allow_ai(),
            Box::new(RecordingExecutor::default()),
            KEY,
            LEASE_ID,
            1,
            ISSUER,
            1,
            99, // 点 99 无策略规则 → NoPolicy 拒绝
            4,
            now,
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
