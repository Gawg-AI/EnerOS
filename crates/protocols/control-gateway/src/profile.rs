//! 控制链「同源档案」（ControlProfile）——蓝图 55 C1 候选落地。
//!
//! 签发侧（[`ChainCommandSink`](crate::chain)、DispatchEngine 信封构造）与
//! 复验侧（[`EnvelopeIngress`](https://docs.rs)、[`ControlChain`] gateway）
//! 必须派生自**同一份档案**：租约 ID、fencing epoch 种子、策略代次、绑定
//! 目标、安全点表与链调优参数，任何一项不同源即互相拒绝（QEMU 实弹语义，
//! 原「两端必须同源」仅由三处注释承载——本模块把不变量搬进类型）。
//!
//! 档案**不含**两类事实：
//! - HMAC 密钥：装配层注入（§2.6 密钥不落明文配置；47.3 KMS 注入链）；
//! - issuer 字节：签发方身份属署名/审计语义，gateway 不校验（AI sink 与
//!   dispatch 引擎本就不同），不属授权事实。
//!
//! 规范策略习语收敛为一种：`Policy::new(v-1)` + `replace_rules` ⇒
//! `generation == policy_version`（`replace_rules` 恰 bump +1 且下限 1；
//! 旧的 `with_rule` 不 bump 习语退役）。

use alloc::vec;
use alloc::vec::Vec;

use eneros_telemetry_model::command::{CommandValue, ControlContract, REQUIRED_FEEDBACK_SET_PROD};
use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
use eneros_telemetry_model::unit::PhysicalUnit;

use crate::chain::ControlChain;
use crate::envelope::ActorKind;
use crate::lease::LeaseScope;
use crate::policy::{CommandClass, Policy, PolicyRule};
use crate::safety::{SafetyPoint, SafetyRole};

/// PCS 连续 setpoint 的独立回读绝对容差（kW；工程量）。
/// 单源：writer 侧（svc-modbus-tcp 回读比对）与签发侧合同
/// （[`ControlProfile::control_contract`]）都从这里取值。
pub const PCS_SETPOINT_READBACK_TOL_KW: f64 = 0.1;

/// PCS 连续 setpoint 的逐点爬坡合同上限（kW/s）。
/// = 200 kW/min（与 `DispatchConfig.safety_ramp_kw_per_min` 保护缺省同源
/// 的保护意图）；调度相邻时段路径继续用后者，单点命令路径用本值。
pub const PCS_SETPOINT_MAX_RAMP_PER_SEC: f64 = 200.0 / 60.0;

/// 链调优参数束——原八参 `ControlChain::new` 调用点的六个逐字魔数
/// （key 与 policy 之外的全部数值），产品默认值收拢于此。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainTuning {
    /// Gateway 时间偏移容忍（秒）。
    pub max_skew_secs: u64,
    /// 重放缓存容量。
    pub replay_cache_cap: usize,
    /// 反馈样本最大年龄（秒，CTL-010）。
    pub feedback_max_age_secs: u64,
    /// 幂等缓存 TTL（秒，CTL-007）。
    pub idempotency_ttl_secs: u64,
    /// 幂等缓存容量。
    pub idempotency_cap: usize,
    /// SBO 选择窗口（秒，CTL-007）。
    pub sbo_select_ttl_secs: u64,
}

impl Default for ChainTuning {
    fn default() -> Self {
        Self {
            max_skew_secs: 5,
            replay_cache_cap: 64,
            feedback_max_age_secs: 60,
            idempotency_ttl_secs: 300,
            idempotency_cap: 64,
            sbo_select_ttl_secs: 30,
        }
    }
}

/// 控制链同源档案。全 Copy 数据，零分配存储；派生方法按需分配。
///
/// 一份档案同时喂两侧：
/// - 签发侧：信封字段（lease_id/fencing token/策略代次/device/point）；
/// - 复验侧：[`ControlChain::from_profile`] 装配的 gateway/lease/policy/safety。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlProfile {
    /// 本地预置租约 ID（两侧同源，§6.7）。
    pub lease_id: [u8; 16],
    /// fencing epoch 种子（平台重启计数；首签 token = seed + 1）。
    pub epoch_seed: u64,
    /// 策略代次（生效 generation 恰等于此值）。
    pub policy_version: u64,
    /// 租约 TTL（秒）。
    pub lease_ttl_secs: u64,
    /// 授权角色类别（驱动 PDP 规则匹配）。
    pub actor_kind: ActorKind,
    /// 绑定设备 ID（信封域 u32；点模型域截取低 16 位）。
    pub device_id: u32,
    /// 绑定点 ID。
    pub point_id: u32,
    /// 放行命令类（PDP 规则）。
    pub command_class: CommandClass,
    /// 租约 scope 的 max_class 提示。
    pub lease_max_class: u8,
    /// 安全角色（独立验证器点表）。
    pub safety_role: SafetyRole,
    /// 受控量纲。
    pub unit: PhysicalUnit,
    /// 安全下限。
    pub safe_min: f64,
    /// 安全上限。
    pub safe_max: f64,
    /// 最大变化率（None = 不强制）。
    pub max_ramp_per_sec: Option<f64>,
    /// 控制方向。
    pub direction: ControlDirection,
    /// 是否可远程控制。
    pub remote_controllable: bool,
    /// 链调优参数。
    pub tuning: ChainTuning,
}

impl ControlProfile {
    /// 规范 AI 单点档案：PccBreaker ±500 kW Kilowatts Discrete 可远程，
    /// Switch 类放行，max_class=1，TTL 3600 s，tuning 产品默认。
    /// 这是全仓 13 处逐字装配块的唯一事实源。
    pub fn ai_pcc_single(
        device_id: u32,
        point_id: u32,
        policy_version: u64,
        lease_id: [u8; 16],
        epoch_seed: u64,
    ) -> Self {
        Self {
            lease_id,
            epoch_seed,
            policy_version,
            lease_ttl_secs: 3_600,
            actor_kind: ActorKind::Ai,
            device_id,
            point_id,
            command_class: CommandClass::Switch,
            lease_max_class: 1,
            safety_role: SafetyRole::PccBreaker,
            unit: PhysicalUnit::Kilowatts,
            safe_min: -500.0,
            safe_max: 500.0,
            max_ramp_per_sec: None,
            direction: ControlDirection::Discrete,
            remote_controllable: true,
            tuning: ChainTuning::default(),
        }
    }

    /// 规范 AI 连续设定点档案（61 v0.314.25 §3.7：替代 `ai_pcc_single`
    /// 的语义错配）——PCS 点 10 = `ActivePowerKw/Continuous`，Setpoint 类，
    /// Kilowatts ±500 可远程。PCC 开关另用真实开关点 + `ai_pcc_single`；
    /// 没有真实开关点就不开放该控制。
    pub fn ai_pcs_setpoint(
        device_id: u32,
        point_id: u32,
        policy_version: u64,
        lease_id: [u8; 16],
        epoch_seed: u64,
    ) -> Self {
        Self {
            lease_id,
            epoch_seed,
            policy_version,
            lease_ttl_secs: 3_600,
            actor_kind: ActorKind::Ai,
            device_id,
            point_id,
            command_class: CommandClass::Setpoint,
            lease_max_class: 1,
            safety_role: SafetyRole::PccBreaker,
            unit: PhysicalUnit::Kilowatts,
            safe_min: -500.0,
            safe_max: 500.0,
            // 复查 FIX（61 §3.4）：单点 setpoint 爬坡合同化——与保护配合
            // 同一保护意图（DispatchConfig.safety_ramp_kw_per_min 缺省
            // 200 kW/min），换算为逐点合同值（单位/秒）；执行侧由
            // dispatch 引擎对「最近 AppliedConfirmed setpoint」强制。
            max_ramp_per_sec: Some(PCS_SETPOINT_MAX_RAMP_PER_SEC),
            direction: ControlDirection::Increase,
            remote_controllable: true,
            tuning: ChainTuning::default(),
        }
    }

    /// 类型化控制合同（61 v0.314.25 §3.3；复查 2026-08-30 落地）——
    /// 全部字段由本档案单源派生（值域=安全窗、爬坡=逐点合同、tag/unit=
    /// 命令类别），签发侧与 writer 侧据此对齐。仅对可执行命令类开放：
    /// `Read`/`Emergency` 无设备执行合同 → `None`。
    /// asset_id 暂绑定 device_id（资产级绑定随 Phase 1 点表编译收编）。
    pub fn control_contract(&self) -> Option<ControlContract> {
        let (command_tag, unit_code, max_ramp_per_sec) = match (self.command_class, self.unit) {
            (CommandClass::Switch, _) => (
                CommandValue::SwitchPosition(0).value_tag(),
                CommandValue::SwitchPosition(0).unit_code(),
                f64::INFINITY,
            ),
            (CommandClass::Setpoint, PhysicalUnit::Kilowatts) => (
                CommandValue::ActivePowerKw(0.0).value_tag(),
                CommandValue::ActivePowerKw(0.0).unit_code(),
                self.max_ramp_per_sec.unwrap_or(f64::INFINITY),
            ),
            _ => return None,
        };
        Some(ControlContract {
            contract_id: self.point_id,
            asset_id: u64::from(self.device_id),
            device_id: u16::try_from(self.device_id).ok()?,
            point_id: self.point_id,
            command_tag,
            unit_code,
            min: self.safe_min,
            max: self.safe_max,
            max_ramp_per_sec,
            readback_point_id: self.point_id,
            tolerance_abs: PCS_SETPOINT_READBACK_TOL_KW,
            tolerance_rel: 0.0,
            settle_timeout_ms: 3_000,
            required_feedback_set_id: REQUIRED_FEEDBACK_SET_PROD,
        })
    }

    /// 规范策略习语：`Policy::new(v-1)` + `replace_rules` ⇒ 生效
    /// `generation == policy_version`（信封 `policy_version` 与链 generation
    /// 同值，QEMU 实弹 StalePolicyVersion 教训的制度化）。
    ///
    /// 要求 `policy_version >= 1`（generation 非零不变量）；违反时 panic
    /// 属装配期错误，不得静默降级。
    pub fn policy(&self) -> Policy {
        assert!(
            self.policy_version >= 1,
            "control profile policy_version must be >= 1"
        );
        let mut policy = Policy::new(self.policy_version - 1);
        let _ = policy.replace_rules(vec![PolicyRule {
            actor_kind: Some(self.actor_kind),
            devices: vec![self.device_id],
            points: vec![self.point_id],
            kinds: vec![self.command_class],
            time_window: (0, 0),
            obligations: vec![],
        }]);
        policy
    }

    /// 绑定目标的租约 scope。
    pub fn lease_scope(&self) -> LeaseScope {
        LeaseScope {
            devices: vec![self.device_id],
            points: vec![self.point_id],
            max_class: self.lease_max_class,
        }
    }

    /// 独立验证器安全点表（恰一条绑定条目）。
    pub fn safety_points(&self) -> Vec<(u32, u32, SafetyPoint)> {
        vec![(
            self.device_id,
            self.point_id,
            SafetyPoint {
                role: self.safety_role,
                spec: PointSpec {
                    point_id: self.point_id,
                    // 点模型域 DeviceId=u16；信封域 u32。绑定来自 45 卷点表
                    // Control 点（u16 域），此处截取与 prod_safety_points 先例一致。
                    device_id: self.device_id as u16,
                    unit: self.unit,
                    safe_min: self.safe_min,
                    safe_max: self.safe_max,
                    max_ramp_per_sec: self.max_ramp_per_sec,
                    direction: self.direction,
                    remote_controllable: self.remote_controllable,
                },
            },
        )]
    }

    /// 在已装配链上签发本地租约；返回 fencing token（首签 = seed + 1）。
    /// 返回 `None` = 租约被吊销过（该 ID 永不可再授予）——调用方 fail-closed。
    pub fn issue_lease(&self, chain: &mut ControlChain, now_secs: u64) -> Option<u64> {
        let (token, _expiry) = chain.lease_service_mut().issue(
            self.lease_id,
            self.lease_scope(),
            self.lease_ttl_secs,
            now_secs,
        )?;
        Some(token)
    }
}

impl ControlChain {
    /// 从同源档案装配链：tuning + 规范策略 + epoch 种子 + 安全点注册。
    ///
    /// **不签租约**——复验侧需要区分「签发失败」以进入生而 default-deny
    /// （fail-closed，见 device-agent `lease_ok`）；签发统一走
    /// [`ControlProfile::issue_lease`]。种子先于一切签发的次序不变量在此固化。
    pub fn from_profile(key: &[u8], profile: &ControlProfile) -> Self {
        let t = profile.tuning;
        let mut chain = ControlChain::new(
            key,
            t.max_skew_secs,
            t.replay_cache_cap,
            profile.policy(),
            t.feedback_max_age_secs,
            t.idempotency_ttl_secs,
            t.idempotency_cap,
            t.sbo_select_ttl_secs,
        );
        chain.lease_service_mut().seed_epoch(profile.epoch_seed);
        for (device, point, spec) in profile.safety_points() {
            chain.safety_mut().register(device, point, spec);
        }
        chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE_A: [u8; 16] = [0x40; 16];
    const LEASE_B: [u8; 16] = [0xE1; 16];

    fn profile_v3() -> ControlProfile {
        ControlProfile::ai_pcc_single(1, 10, 3, LEASE_A, 7)
    }

    #[test]
    fn policy_generation_equals_profile_version() {
        // 规范习语：v=1 与 v=3 都必须产出 generation==v（旧双习语等价性收拢）。
        for v in [1u64, 3] {
            let p = ControlProfile::ai_pcc_single(1, 10, v, LEASE_A, 7).policy();
            assert_eq!(p.generation, v);
        }
    }

    #[test]
    fn policy_rule_matches_binding() {
        let p = profile_v3().policy();
        let rules = p.rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].devices, vec![1]);
        assert_eq!(rules[0].points, vec![10]);
    }

    #[test]
    fn from_profile_seeds_epoch_before_issue_and_first_token_is_seed_plus_one() {
        let prof = profile_v3();
        let mut chain = ControlChain::from_profile(b"test-key-0000000000000000000000", &prof);
        // 未签租约：token 未知（复验侧 fail-closed 面）。
        assert_eq!(chain.lease_service_mut().token_of(&prof.lease_id), None);
        let token = prof.issue_lease(&mut chain, 1_700_000_000).expect("lease");
        assert_eq!(token, prof.epoch_seed + 1);
        assert_eq!(
            chain.lease_service_mut().token_of(&prof.lease_id),
            Some(token)
        );
    }

    #[test]
    fn profiles_with_different_lease_ids_do_not_see_each_other_grants() {
        // 同源不变量的反面：两份不同源档案互不可见对方租约（LeaseRevoked 面）。
        let key = b"mismatch-key-00000000000000000000";
        let pa = profile_v3();
        let pb = ControlProfile::ai_pcc_single(1, 10, 3, LEASE_B, 9);
        let mut ca = ControlChain::from_profile(key, &pa);
        pa.issue_lease(&mut ca, 1_700_000_000).expect("lease a");
        assert_eq!(ca.lease_service_mut().token_of(&pb.lease_id), None);
    }

    #[test]
    fn safety_points_derive_from_profile_fields() {
        let pts = profile_v3().safety_points();
        assert_eq!(pts.len(), 1);
        let (dev, pt, sp) = &pts[0];
        assert_eq!(*dev, 1);
        assert_eq!(*pt, 10);
        assert_eq!(sp.spec.safe_min, -500.0);
        assert_eq!(sp.spec.safe_max, 500.0);
        assert_eq!(sp.role, SafetyRole::PccBreaker);
    }

    #[test]
    #[should_panic(expected = "policy_version must be >= 1")]
    fn zero_policy_version_is_assembly_error() {
        ControlProfile::ai_pcc_single(1, 10, 0, LEASE_A, 7).policy();
    }
}
