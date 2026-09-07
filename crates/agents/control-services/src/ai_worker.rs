//! 蓝图 40 WP-40.5：svc-ai-worker 服务逻辑（target AI worker）。
//!
//! 只经 EIPC 获取可信输入（TrustedTelemetry wire）并返回 proposal/拒绝。
//! Llama/HiGHS 为 host native 后端——target profile **不声明**（WP-40.4）：
//! 未声明后端时推理请求显式 `Unavailable`（零推理、零新写，不伪造）。
//! host 测试以 mock backend 验证完整决策逻辑。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub use eneros_control_dispatch::adapter::proposal_to_candidate_unbound;
use eneros_control_dispatch::dispatch::AuthenticatedActor;
use eneros_control_gateway::envelope::ActorKind;
use eneros_dual_brain::ProposedControlPlan;
use eneros_safety_validator::TrustedTelemetry;

use crate::telemetry::decode_trusted;

/// 请求码。
pub const AI_PROPOSE: u8 = 1;

/// 响应状态。
pub const AI_OK: u8 = 0; // proposal wire
pub const AI_NOACTION: u8 = 1; // no-action（含拒绝原因串）
pub const AI_UNAVAILABLE: u8 = 2; // 后端未声明/不可用（零推理）
pub const AI_INVALID: u8 = 0xFF;

/// SPEC-SERIES-2026-08-27/E WP-E.4：System1 SOC 死区带**单源**（中点 ± 半宽）。
///
/// 全仓 SOC 死区阈值只许从这里取值（`HeuristicBackend` 出厂值与
/// `learn` 闭环的出厂半宽同源）；标注词汇 PathType/Degraded 沿用
/// `dual_brain::DecisionPath`，不再另立词表。
pub mod soc_band {
    /// 死区带中点（SOC 比例）。
    pub const MID: f64 = 0.4;
    /// 出厂半宽（SOC 比例）——出厂带 = [MID−0.1, MID+0.1] = [0.3, 0.5]。
    pub const FACTORY_HALF_WIDTH: f64 = 0.1;
    /// 学习半宽硬下界（SOC 比例；catalog 实例同值——越界拒绝的权威）。
    pub const LEARN_MIN_HALF_WIDTH: f64 = 0.06;
    /// 学习半宽硬上界（SOC 比例）。
    pub const LEARN_MAX_HALF_WIDTH: f64 = 0.14;
    /// 充电阈值 = 中点 − 半宽。
    pub fn charge_soc(half_width: f64) -> f64 {
        MID - half_width
    }
    /// 放电阈值 = 中点 + 半宽。
    pub fn discharge_soc(half_width: f64) -> f64 {
        MID + half_width
    }
}

/// SPEC-SERIES-2026-08-27/E WP-E.3：学习管理动词 wire 码（svc-ai-worker
/// 消费；AI_PROPOSE=1 / AI_PEER_MSG=0x10 旁新增，EIPC 请求首字节判别）。
pub mod learn_wire {
    /// 学习参数回出厂（learn-reset；EAQ-3 裁决 = 新增动词而非 ConfigSet——
    /// mgmt 总线不达 control-services 域，EIPC 动词是唯一可达面）。
    pub const AI_LEARN_RESET: u8 = 0x20;
    /// 学习状态查询（StatusQuery 等价面——current/min/max/samples/
    /// rejected/applies 原样回显，逐字节可对照）。
    pub const AI_LEARN_STATUS: u8 = 0x21;
    /// LEARN_STATUS 响应定长：code(1)+enabled(1)+current(8)+min(8)+max(8)
    /// +samples(8)+rejected(8)+applies(8) = 50B（LE）。
    pub const LEARN_STATUS_WIRE_LEN: usize = 1 + 1 + 8 * 6;
    /// LEARN_RESET 响应定长：code(1)+status(1)（0=ok / 1=不可用）。
    pub const LEARN_RESET_WIRE_LEN: usize = 2;
}

/// 决策后端（服务注入 seam——target 无声明后端时为 Unavailable）。
pub trait DecisionBackend {
    /// 执行决策：返回 Proposal wire 或结构化拒绝。
    fn decide(
        &mut self,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject>;
}

/// 后端拒绝（结构化区分——WP-40.3 语义：不可用≠无动作）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionReject {
    /// 后端未声明/不可用（零推理）。
    Unavailable(String),
    /// 有决策能力但本输入下无动作（如 SOC 死区）。
    NoAction(String),
}

/// 未声明后端（target 默认）：显式 Unavailable（WP-40.4/40.5）。
pub struct UnavailableBackend;

impl DecisionBackend for UnavailableBackend {
    fn decide(
        &mut self,
        _telemetry: &TrustedTelemetry,
        _now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        Err(DecisionReject::Unavailable(String::from(
            "ai-backend-unavailable: profile 未声明 Llama/HiGHS（WP-40.4）",
        )))
    }
}

/// 静态启发式后端（target 产品路径，WP-40.4："静态 solver/heuristic 具
/// bounded work"）：SOC 死区规则——高于放电阈值放电、低于充电阈值充电、
/// 死区内 NoAction。无 native 库依赖（Llama/HiGHS 仍由 profile 声明），
/// 决策为 O(1) 有界工作；只产生 proposal（经唯一 ControlDispatch 投递）。
pub struct HeuristicBackend {
    seq: u64,
    /// SOC 放电阈值（≥ 放电）。
    discharge_soc: f64,
    /// SOC 充电阈值（≤ 充电）。
    charge_soc: f64,
    /// 功率幅值 kW（安全目录范围内）。
    power_kw: f64,
    /// 决策路径标注（审计用）。
    path: eneros_dual_brain::DecisionPath,
    /// 蓝图 61 v0.314.35：目录合同版本（候选计划 resource_catalog_version
    /// 与 dispatch PlannerView 对照；bin 启动从目录装载，缺省 0）。
    catalog_version: u64,
    /// 蓝图 61 v0.314.42（WP-5.3）：D3 SolverFallback 降级 lease 门——
    /// 武装后每次 decide 逐次授权（验签/纪元/时效/白名单/限额/次数）；
    /// 未武装 = D0 正常主路径。
    degradation: Option<(
        eneros_control_dispatch::degrade::DegradationAuthority,
        eneros_control_dispatch::degrade::DegradationLease,
    )>,
}

/// 68.5 WP-68.5.3：D3 降级 lease 单发使用次数上界（逐次授权记账；耗尽 =
/// fail-closed "lease exhausted"——兜底不是无限额度）。
pub const DEGRADE_LEASE_MAX_USES: u32 = 100_000;

/// 68.5 WP-68.5.3：降级武装 seam——RemoteBrain 对 System1 本地后端的
/// lease 执行面（D3 SolverFallback 语义"兜底也要持签"；degrade.rs 机制面
/// 零改动）。生产唯一 RemoteBrain 本地形态 = [`HeuristicBackend`]；新
/// 后端接入 RemoteBrain 须显式实现（武装语义是决策点，不给静默缺省）。
pub trait DegradeArmSeam {
    /// 武装查询（学习冻结判据消费面）。
    fn degradation_armed(&self) -> bool;
    /// 68.5 复查修复：当前武装 lease 是否已过有效期（复位/重武装判据；
    /// 未武装恒 false）。authorize 对过期 lease 返回 Err 且状态不动——
    /// 若无此判据，武装态在 lease 到期后既不能授权（decide 恒
    /// Unavailable）也不能重武装（已武装守卫短路），peer 长期失联时
    /// 决策链永久停供。
    fn degradation_lease_expired(&self, now_ms: u64) -> bool;
    /// 武装（issue + 验签 arm，fail-closed；成功返回 lease_id 供审计事件）。
    fn arm_for_degrade(
        &mut self,
        key: &[u8; 32],
        now_ms: u64,
        ttl_secs: u32,
        max_power_kw: f64,
    ) -> Result<[u8; 16], alloc::string::String>;
    /// 撤销（超时 episode 结束/恢复 D0）。
    fn revoke_degradation(&mut self);
}

impl DegradeArmSeam for HeuristicBackend {
    fn degradation_armed(&self) -> bool {
        self.degradation.is_some()
    }

    fn degradation_lease_expired(&self, now_ms: u64) -> bool {
        matches!(&self.degradation, Some((_, l)) if !l.is_valid_at(now_ms))
    }

    fn arm_for_degrade(
        &mut self,
        key: &[u8; 32],
        now_ms: u64,
        ttl_secs: u32,
        max_power_kw: f64,
    ) -> Result<[u8; 16], alloc::string::String> {
        use eneros_control_dispatch::degrade::{DegradationLease, DegradationLevel};
        if self.degradation.is_some() {
            return Err(alloc::string::String::from("already-armed"));
        }
        // lease_id = 武装时刻 u128 LE（单调可读可对账）。
        let lease_id = (now_ms as u128).to_le_bytes();
        // 资源作用域摘要 = 服务域标识常量（授权面不消费——仅签名覆盖）。
        let scope = eneros_crypto::sm3_hash(b"eneros.svc-ai-worker.degrade-scope.v1");
        // 限额代理语义（Phase 5 复查登记同源）：本装配面无动作时长/爬坡
        // 输入，energy/ramp 以功率幅值代理（authorize 调用同口径）。
        let lease = DegradationLease::issue(
            key,
            lease_id,
            0,
            DegradationLevel::SolverFallback,
            self.strategy_digest(),
            scope,
            max_power_kw,
            max_power_kw,
            max_power_kw,
            DEGRADE_LEASE_MAX_USES,
            now_ms,
            now_ms.saturating_add(u64::from(ttl_secs).saturating_mul(1_000)),
            0,
            0,
        )?;
        self.arm_degradation_lease(key, lease)?;
        Ok(lease_id)
    }

    fn revoke_degradation(&mut self) {
        HeuristicBackend::revoke_degradation(self);
    }
}

impl HeuristicBackend {
    /// 构造（出厂死区 = [`soc_band`] 单源：MID±FACTORY_HALF_WIDTH =
    /// [0.3, 0.5]：≥0.5 放电 / ≤0.3 充电 / 死区 NoAction）。
    pub fn new() -> Self {
        Self {
            seq: 0,
            discharge_soc: soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH),
            charge_soc: soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH),
            power_kw: 5.0,
            path: eneros_dual_brain::DecisionPath::Fast,
            catalog_version: 0,
            degradation: None,
        }
    }

    /// WP-5.3（[10] §4.7）：本后端的策略摘要（降级 lease 白名单锚——
    /// 装配侧签发 lease 时引用同一摘要；死区参数漂移 = 摘要漂移）。
    pub fn strategy_digest(&self) -> [u8; 32] {
        Self::strategy_digest_of(self.charge_soc, self.discharge_soc, self.power_kw)
    }

    /// 策略摘要纯函数（SM3/SHA 输入域固定；参数漂移 = 摘要漂移）。
    fn strategy_digest_of(charge_soc: f64, discharge_soc: f64, power_kw: f64) -> [u8; 32] {
        let mut buf = alloc::vec::Vec::with_capacity(40);
        buf.extend_from_slice(b"eneros.heur-soc-band.v1");
        buf.extend_from_slice(&charge_soc.to_le_bytes());
        buf.extend_from_slice(&discharge_soc.to_le_bytes());
        buf.extend_from_slice(&power_kw.to_le_bytes());
        eneros_crypto::sm3_hash(buf.as_slice())
    }

    /// 武装 D3 降级 lease（装配在降级态调用；签名不合法即拒绝武装——
    /// fail-closed）。
    pub fn arm_degradation_lease(
        &mut self,
        key: &[u8],
        lease: eneros_control_dispatch::degrade::DegradationLease,
    ) -> Result<(), alloc::string::String> {
        if !lease.verify(key) {
            return Err(alloc::string::String::from(
                "degradation lease signature invalid",
            ));
        }
        self.degradation = Some((
            eneros_control_dispatch::degrade::DegradationAuthority::new(
                key,
                lease.revocation_epoch,
            ),
            lease,
        ));
        Ok(())
    }

    /// 撤销降级 lease（恢复 D0 或换发新 lease 前）。
    pub fn revoke_degradation(&mut self) {
        if let Some((mut auth, lease)) = self.degradation.take() {
            auth.revoke_all();
            let _ = (auth, lease);
        }
    }

    /// 68.5 WP-68.5.3：降级武装查询（装配观测 + 学习冻结判据——武装期
    /// 死区参数必须冻结，否则 strategy digest 漂移击穿 lease 白名单）。
    pub fn degradation_armed(&self) -> bool {
        self.degradation.is_some()
    }

    /// 目录版本注入（bin 启动装载；[08] §4.8 planner 输入与 Registry 同代）。
    pub fn with_catalog_version(mut self, version: u64) -> Self {
        self.catalog_version = version;
        self
    }

    /// SPEC-SERIES-2026-08-27/E WP-E.3：运行期死区重设（学习闭环下游
    /// 消费点——学到的参数在此真实改变拒绝/放行边界）。界内不变式
    /// （charge < discharge、均落 (0,1)）由学习侧 catalog 保证；此处
    /// 防御性钳制保证任何调用都不产生倒挂死区。
    pub fn set_deadband(&mut self, charge_soc: f64, discharge_soc: f64) {
        let (lo, hi) = if charge_soc <= discharge_soc {
            (charge_soc, discharge_soc)
        } else {
            (discharge_soc, charge_soc)
        };
        self.charge_soc = lo.clamp(0.01, 0.98);
        self.discharge_soc = hi.clamp(self.charge_soc + 0.01, 0.99);
    }
}

impl Default for HeuristicBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DecisionBackend for HeuristicBackend {
    fn decide(
        &mut self,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        // WP-5.3（[10] §4.7）：降级 lease 门——武装后每次 decide 逐次授权，
        // 任何失败拒绝决策（fail-closed，不产生未授权 fallback 动作）。
        // 68.5 复查修复：兜底提案功率钳到 lease 授权上界内（站点
        // max_power_kw 是可配旋钮，可小于后端常量功率 5.0——不钳制则
        // 武装期逐次授权恒 "action beyond lease bounds"，兜底全拒断供）；
        // digest 仍按策略原值计算，签名面不变。
        let mut proposal_power = self.power_kw;
        if let Some((auth, lease)) = &mut self.degradation {
            let digest = HeuristicBackend::strategy_digest_of(
                self.charge_soc,
                self.discharge_soc,
                self.power_kw,
            );
            proposal_power = proposal_power.min(lease.max_abs_power_kw);
            let power = proposal_power.abs();
            // 复查登记（Phase 5）：能量/爬坡限额以功率绝对值代理——本
            // 后端无动作时长/爬坡形态输入，逐次授权仅能作名义包络校验
            // （超功率拒绝仍是硬门）；真实能量/爬坡语义随 D3 降级运行
            // 手册与动作描述面落地（Phase 6 前置）。
            auth.authorize(lease, now_ms, &digest, power, power, power)
                .map_err(|e| {
                    DecisionReject::Unavailable(alloc::format!("degradation-lease: {e}"))
                })?;
        }
        let soc = telemetry.state.soc_pct;
        if !soc.is_finite() {
            return Err(DecisionReject::NoAction(String::from("soc-non-finite")));
        }
        // 死区：无动作（明确语义——非 unavailable）。
        if soc < self.discharge_soc && soc > self.charge_soc {
            return Err(DecisionReject::NoAction(alloc::format!(
                "soc-deadband:{soc:.3}"
            )));
        }
        let power = if soc >= self.discharge_soc {
            proposal_power
        } else {
            -proposal_power
        };
        self.seq = self.seq.saturating_add(1);
        // plan_id 唯一（seq 递增）——dispatch 幂等键派生源，每次真实投递。
        // 蓝图 55：单时段计划构造器单源（TTL 60 = heuristic 快路径语义）。
        let plan = ProposedControlPlan::single_entry(
            alloc::format!("heur-{}", self.seq),
            self.path,
            power,
            soc,
            eneros_solver_core::result::SolveStatus::Suboptimal, // 启发式解（无最优性声明）
            60,
            now_ms,
            Some(telemetry.provenance_digest),
        );
        let cand = crate::dispatch_service::candidate_from_proposal(&plan, self.catalog_version)
            .map_err(|e| DecisionReject::NoAction(alloc::format!("candidate-adapter: {e:?}")))?;
        Ok(encode_candidate_payload(&cand))
    }
}

/// 蓝图 61 v0.314.29 §4.5.6：决策 wire 唯一 writer = CandidatePlan V1
/// （`CAND` magic；decision-contract 单源）。旧 70B writer 已删除。
pub fn encode_candidate_payload(
    c: &eneros_decision_contract::CandidatePlan,
) -> alloc::vec::Vec<u8> {
    c.encode_v1()
}

/// AI worker 服务逻辑。
pub struct AiWorkerService<B: DecisionBackend> {
    backend: B,
    /// 当前 actor（服务装配期注入——Ai 角色 + 服务标识）。
    #[allow(dead_code)] // 44.2：装配期注入字段，当前读路径未消费（44.4 冻结登记）
    actor: AuthenticatedActor,
    /// 决策计数（观测）。
    pub decisions: u64,
}

impl<B: DecisionBackend> AiWorkerService<B> {
    /// 构造。
    pub fn new(backend: B, service_id: [u8; 16]) -> Self {
        Self {
            backend,
            actor: AuthenticatedActor {
                kind: ActorKind::Ai,
                service: service_id,
                session_epoch: 0,
            },
            decisions: 0,
        }
    }

    /// 49（WP-49.2）：后端可变访问（装配层读决策元数据/切换事件做审计）。
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// 处理一次推理请求（可信遥测 wire → proposal wire/结构化拒绝）。
    pub fn propose(&mut self, req: &[u8], now_ms: u64) -> Vec<u8> {
        let Some(&code) = req.first() else {
            return vec![AI_INVALID];
        };
        if code != AI_PROPOSE {
            return vec![AI_INVALID];
        }
        let Some(telemetry) = decode_trusted(&req[1..]) else {
            return vec![AI_INVALID];
        };
        // 后端不可用 → 显式 Unavailable（零推理、零新写）；
        // 死区无动作 → NoAction（有能力，本输入不动作）。
        match self.backend.decide(&telemetry, now_ms) {
            Ok(proposal_wire) => {
                self.decisions += 1;
                let mut out = vec![AI_OK];
                out.extend_from_slice(&proposal_wire);
                out
            }
            Err(DecisionReject::Unavailable(reason)) => {
                let mut out = vec![AI_UNAVAILABLE];
                out.extend_from_slice(reason.as_bytes());
                out
            }
            Err(DecisionReject::NoAction(reason)) => {
                let mut out = vec![AI_NOACTION];
                out.extend_from_slice(reason.as_bytes());
                out
            }
        }
    }
}

/// 结果解码（测试/消费侧）：AI_OK 后 = CandidatePlan V1 wire（蓝图 61
/// v0.314.29 唯一形态）。
pub fn decode_response(resp: &[u8]) -> Result<eneros_decision_contract::CandidatePlan, String> {
    match resp.first() {
        Some(&AI_OK) => eneros_decision_contract::CandidatePlan::decode_v1(&resp[1..])
            .map_err(|e| alloc::format!("bad candidate: {e:?}")),
        Some(&AI_UNAVAILABLE) => Err(String::from_utf8_lossy(&resp[1..]).into_owned()),
        Some(&AI_NOACTION) => Err(String::from_utf8_lossy(&resp[1..]).into_owned()),
        _ => Err(String::from("invalid response")),
    }
}

/// host 测试辅助（可信遥测构造——服务逻辑测试共用）。
#[cfg(test)]
pub mod tests_helpers {
    use super::*;

    pub fn minimal_trusted() -> TrustedTelemetry {
        minimal_trusted_at(10_000)
    }

    pub fn minimal_trusted_at(now_ms: u64) -> TrustedTelemetry {
        let now = now_ms;
        let mut svc =
            crate::telemetry::TelemetryService::new(eneros_safety_validator::EvaluatorConfig {
                coverage: eneros_safety_validator::RequiredPointCoverage {
                    points: vec![
                        eneros_safety_validator::RequiredPoint {
                            point: 1,
                            min: 0.0,
                            max: 1000.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Volt,
                        },
                        eneros_safety_validator::RequiredPoint {
                            point: 2,
                            min: -1000.0,
                            max: 1000.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Ampere,
                        },
                        eneros_safety_validator::RequiredPoint {
                            point: 3,
                            min: 45.0,
                            max: 55.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Hertz,
                        },
                        eneros_safety_validator::RequiredPoint {
                            point: 4,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Ratio,
                        },
                        eneros_safety_validator::RequiredPoint {
                            point: 5,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Other,
                        },
                        eneros_safety_validator::RequiredPoint {
                            point: 6,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Other,
                        },
                    ],
                },
                bindings: eneros_safety_validator::StateBindings {
                    voltage: 1,
                    current: 2,
                    frequency: 3,
                    soc: 4,
                },
                mapping_version: 1,
                max_skew_ms: 1000,
                allowed_sources: vec![1],
                require_integrity: true,
                control_horizon_ms: 30_000,
            });
        let good = |point: u32, value: f64, seq: u64| eneros_safety_validator::TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms: now.saturating_sub(50),
            received_at_ms: now,
            quality: eneros_safety_validator::SampleQuality::Good,
            sequence: seq,
            integrity: eneros_safety_validator::IntegrityStatus::Verified,
            mapping_version: 1,
        };
        svc.ingest(&[
            good(1, 380.0, 1),
            good(2, 10.0, 1),
            good(3, 50.0, 1),
            good(4, 0.6, 1),
            good(5, 1.0, 1),
            good(6, 0.0, 1),
        ]);
        svc.snapshot(now).expect("trusted")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试断言 panic!（mgmt-cli 先例）
    use super::*;

    /// mock 后端（host 测试）：返回固定 proposal。
    struct MockBackend {
        fail: bool,
    }
    impl DecisionBackend for MockBackend {
        fn decide(
            &mut self,
            telemetry: &TrustedTelemetry,
            now_ms: u64,
        ) -> Result<Vec<u8>, DecisionReject> {
            if self.fail {
                return Err(DecisionReject::Unavailable(String::from("backend-broken")));
            }
            // 蓝图 55：单时段计划构造器单源。
            let plan = ProposedControlPlan::single_entry(
                String::from("req-1"),
                eneros_dual_brain::DecisionPath::Slow,
                5.0,
                telemetry.state.soc_pct,
                eneros_solver_core::result::SolveStatus::Optimal,
                60,
                now_ms,
                Some(telemetry.provenance_digest),
            );
            Ok(encode_candidate_payload(
                &proposal_to_candidate_unbound(&plan).expect("stub cand"),
            ))
        }
    }

    fn trusted() -> TrustedTelemetry {
        tests_helpers::minimal_trusted()
    }

    #[test]
    fn propose_with_backend_returns_proposal() {
        let mut worker = AiWorkerService::new(MockBackend { fail: false }, [0xAA; 16]);
        let t = trusted();
        let mut req = vec![AI_PROPOSE];
        req.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        let resp = worker.propose(&req, 10_000);
        let plan = decode_response(&resp).expect("proposal");
        assert_eq!(plan.assignments()[0].power_kw, 5.0);
        assert_eq!(plan.input_digest(), &t.provenance_digest);
        assert_eq!(worker.decisions, 1);
    }

    #[test]
    fn backend_unavailable_is_structured() {
        let mut worker = AiWorkerService::new(MockBackend { fail: true }, [0xAA; 16]);
        let t = trusted();
        let mut req = vec![AI_PROPOSE];
        req.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        let resp = worker.propose(&req, 10_000);
        assert_eq!(resp[0], AI_UNAVAILABLE);
        assert!(decode_response(&resp).is_err());
        assert_eq!(worker.decisions, 0, "后端不可用不得计数决策");
    }

    #[test]
    fn malformed_request_rejected() {
        let mut worker = AiWorkerService::new(MockBackend { fail: false }, [0xAA; 16]);
        assert_eq!(worker.propose(&[], 1), vec![AI_INVALID]);
        assert_eq!(worker.propose(&[99], 1), vec![AI_INVALID]);
        assert_eq!(worker.propose(&[AI_PROPOSE, 1, 2, 3], 1), vec![AI_INVALID]);
    }

    #[test]
    fn heuristic_discharges_high_soc_with_unique_plan_ids() {
        // SOC 0.6 ≥ 0.5 → 放电 +5kW；plan_id 唯一（幂等键派生源）。
        let mut backend = HeuristicBackend::new();
        let t = trusted(); // soc 0.6
        let w1 = backend.decide(&t, 10_000).expect("discharge");
        let cand1 = eneros_decision_contract::CandidatePlan::decode_v1(&w1).expect("cand");
        assert_eq!(cand1.assignments()[0].power_kw, 5.0);
        let id1 = *cand1.id();
        let w2 = backend.decide(&t, 10_000).expect("discharge 2");
        let cand2 = eneros_decision_contract::CandidatePlan::decode_v1(&w2).expect("cand");
        assert_ne!(id1, *cand2.id(), "plan_id 必须唯一");
    }

    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn heuristic_deadband_is_noaction_not_unavailable() {
        // SOC 0.4 ∈ (0.3, 0.5) 死区 → NoAction（有能力不动作 ≠ 不可用）。
        let mut backend = HeuristicBackend::new();
        let mut t = trusted();
        t.state.soc_pct = 0.4;
        match backend.decide(&t, 10_000) {
            Err(DecisionReject::NoAction(_)) => {}
            other => panic!("死区必须 NoAction，got {other:?}"),
        }
    }

    #[test]
    fn heuristic_charges_low_soc_negative_power() {
        let mut backend = HeuristicBackend::new();
        let mut t = trusted();
        t.state.soc_pct = 0.2;
        let w = backend.decide(&t, 10_000).expect("charge");
        let cand = eneros_decision_contract::CandidatePlan::decode_v1(&w).expect("cand");
        assert_eq!(cand.assignments()[0].power_kw, -5.0, "充电为负功率");
    }

    #[test]
    fn heuristic_nonfinite_soc_noaction() {
        let mut backend = HeuristicBackend::new();
        let mut t = trusted();
        t.state.soc_pct = f64::NAN;
        assert!(matches!(
            backend.decide(&t, 10_000),
            Err(DecisionReject::NoAction(_))
        ));
    }

    /// SPEC-E WP-E.3 wire KAT：学习管理动词码与定长布局（对端对照基线）。
    #[test]
    fn e3_learn_wire_layout_kat() {
        use super::learn_wire::{
            AI_LEARN_RESET, AI_LEARN_STATUS, LEARN_RESET_WIRE_LEN, LEARN_STATUS_WIRE_LEN,
        };
        assert_eq!(AI_LEARN_RESET, 0x20);
        assert_eq!(AI_LEARN_STATUS, 0x21);
        // code(1)+enabled(1)+current(8)+min(8)+max(8)+samples(8)+rejected(8)+applies(8)。
        assert_eq!(LEARN_STATUS_WIRE_LEN, 50);
        assert_eq!(LEARN_RESET_WIRE_LEN, 2);
    }

    /// SPEC-E WP-E.3：自适应阈值真实改变拒绝/放行边界（下游消费判据
    /// host 侧）——同 SOC=0.52，出厂带外 ⇒ 放电；加宽带内 ⇒ NoAction。
    #[test]
    fn e3_set_deadband_changes_boundary() {
        let mut b = HeuristicBackend::new();
        let mut t = trusted();
        t.state.soc_pct = 0.52; // 出厂带 [0.3, 0.5] 之外 ⇒ 放电。
        assert!(b.decide(&t, 10_000).is_ok());
        // 半宽加宽到 0.14 ⇒ 带 [0.26, 0.54] ⇒ 0.52 落入死区。
        b.set_deadband(
            soc_band::charge_soc(soc_band::LEARN_MAX_HALF_WIDTH),
            soc_band::discharge_soc(soc_band::LEARN_MAX_HALF_WIDTH),
        );
        match b.decide(&t, 10_000) {
            Err(DecisionReject::NoAction(reason)) => {
                assert!(reason.contains("soc-deadband"), "{reason}");
            }
            other => panic!("加宽后 0.52 必须落入死区，got {other:?}"),
        }
    }

    /// SPEC-E WP-E.4：出厂阈值 = soc_band 单源派生（grep 只剩一处定义）。
    #[test]
    fn e4_factory_thresholds_from_single_source() {
        let b = HeuristicBackend::new();
        assert_eq!(
            b.charge_soc,
            soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH)
        );
        assert_eq!(
            b.discharge_soc,
            soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH)
        );
        assert_eq!(soc_band::MID, 0.4);
        assert_eq!(soc_band::FACTORY_HALF_WIDTH, 0.1);
    }

    /// 68.5 WP-68.5.3 t5（红测先行）：过期 lease 不得作为降级凭证——
    /// arm 接受（签名合法）但 decide 逐次授权的时效门拒绝 → System1
    /// Unavailable（fail-closed：降级维持必须有活 lease）。
    #[test]
    fn t5_lease_expiry_blocks_stale_fallback() {
        use eneros_control_dispatch::degrade::{DegradationLease, DegradationLevel};
        let key = b"station-degrade-lease-key-000000001";
        let factory = HeuristicBackend::new();
        let lease = DegradationLease::issue(
            key,
            [7u8; 16],
            0,
            DegradationLevel::SolverFallback,
            factory.strategy_digest(),
            eneros_crypto::sm3_hash(b"eneros.svc-ai-worker.degrade-scope.v1"),
            5.0,
            5.0,
            5.0,
            100,
            1_000,
            2_000,
            0,
            0,
        )
        .expect("issue");
        let mut backend = factory;
        backend.arm_degradation_lease(key, lease).expect("arm");
        assert!(backend.degradation_armed());
        let t = trusted();
        // 窗内（1_500 ≤ 2_000）：授权成功 → 兜底产出。
        assert!(backend.decide(&t, 1_500).is_ok(), "窗内兜底持签产出");
        // 过期（3_000 > 2_000）：时效门拒绝 → Unavailable（不是 NoAction）。
        match backend.decide(&t, 3_000) {
            Err(DecisionReject::Unavailable(reason)) => {
                assert!(reason.contains("expired"), "{reason}");
            }
            other => panic!("过期 lease 必须拒兜底，got {other:?}"),
        }
    }

    /// 68.5 复查修复 t5b（红测先行）：lease 功率上界 < 后端常量功率时，
    /// 兜底提案须钳到上界内产出（修复前 authorize 恒
    /// "action beyond lease bounds" → Unavailable 全拒断供）。变异靶：
    /// decide 去掉 proposal_power 钳制 → 本测红。
    #[test]
    fn t5b_fallback_power_clamped_to_lease_bound() {
        use eneros_control_dispatch::degrade::{DegradationLease, DegradationLevel};
        let key = b"station-degrade-lease-key-000000001";
        let factory = HeuristicBackend::new();
        let lease = DegradationLease::issue(
            key,
            [8u8; 16],
            0,
            DegradationLevel::SolverFallback,
            factory.strategy_digest(),
            eneros_crypto::sm3_hash(b"eneros.svc-ai-worker.degrade-scope.v1"),
            4.0,
            4.0,
            4.0, // 上界 4.0 < 后端常量 5.0
            100,
            1_000,
            2_000,
            0,
            0,
        )
        .expect("issue");
        let mut backend = factory;
        backend.arm_degradation_lease(key, lease).expect("arm");
        let mut t = trusted();
        t.state.soc_pct = 0.52; // 出厂带 [0.3, 0.5] 带外 ⇒ 兜底放电提案
        let w = backend
            .decide(&t, 1_500)
            .expect("兜底提案须钳到 lease 上界内产出（修复前恒 Unavailable）");
        let cand = eneros_decision_contract::CandidatePlan::decode_v1(&w).expect("cand");
        let power = cand.assignments()[0].power_kw;
        assert!(power > 0.0 && power <= 4.0, "放电提案须钳到上界内: {power}");
    }
}
