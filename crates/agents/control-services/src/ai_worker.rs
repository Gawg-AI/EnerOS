//! 蓝图 40 WP-40.5：svc-ai-worker 服务逻辑（target AI worker）。
//!
//! 只经 EIPC 获取可信输入（TrustedTelemetry wire）并返回 proposal/拒绝。
//! Llama/HiGHS 为 host native 后端——target profile **不声明**（WP-40.4）：
//! 未声明后端时推理请求显式 `Unavailable`（零推理、零新写，不伪造）。
//! host 测试以 mock backend 验证完整决策逻辑。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

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

/// proposal wire（定长）：plan_id 16 + path 1 + power 8 + ttl 4 + issued 8 +
/// input_digest 32。
pub const PROPOSAL_WIRE_LEN: usize = 16 + 1 + 8 + 4 + 8 + 32;

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
}

impl HeuristicBackend {
    /// 构造（默认死区 [0.3, 0.5]：≥0.5 放电 / ≤0.3 充电 / 死区 NoAction）。
    pub fn new() -> Self {
        Self {
            seq: 0,
            discharge_soc: 0.5,
            charge_soc: 0.3,
            power_kw: 5.0,
            path: eneros_dual_brain::DecisionPath::Fast,
        }
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
            self.power_kw
        } else {
            -self.power_kw
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
        Ok(encode_proposal(&plan).to_vec())
    }
}

/// proposal → wire。
pub fn encode_proposal(p: &ProposedControlPlan) -> [u8; PROPOSAL_WIRE_LEN] {
    let mut out = [0u8; PROPOSAL_WIRE_LEN];
    let id = p.plan_id.as_bytes();
    let n = core::cmp::min(16, id.len());
    out[..n].copy_from_slice(&id[..n]);
    out[16] = match p.path {
        eneros_dual_brain::DecisionPath::Slow => 0,
        eneros_dual_brain::DecisionPath::Fast => 1,
        eneros_dual_brain::DecisionPath::Fallback => 2,
        eneros_dual_brain::DecisionPath::Degraded => 3,
    };
    out[17..25].copy_from_slice(&p.power_kw.to_le_bytes());
    out[25..29].copy_from_slice(&p.ttl_secs.to_le_bytes());
    out[29..37].copy_from_slice(&p.issued_at_ms.to_le_bytes());
    let digest = p.input_digest.unwrap_or([0u8; 32]);
    out[37..69].copy_from_slice(&digest);
    out
}

/// wire → proposal（长度/字段校验；digest 全零 → None）。
pub fn decode_proposal(buf: &[u8]) -> Option<ProposedControlPlan> {
    if buf.len() != PROPOSAL_WIRE_LEN {
        return None;
    }
    let mut plan_id = [0u8; 16];
    plan_id.copy_from_slice(&buf[..16]);
    // 去尾零。
    let end = plan_id
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let plan_id = String::from_utf8(plan_id[..end].to_vec()).ok()?;
    let path = match buf[16] {
        0 => eneros_dual_brain::DecisionPath::Slow,
        1 => eneros_dual_brain::DecisionPath::Fast,
        2 => eneros_dual_brain::DecisionPath::Fallback,
        3 => eneros_dual_brain::DecisionPath::Degraded,
        _ => return None,
    };
    let power_kw = f64::from_le_bytes(buf[17..25].try_into().ok()?);
    let ttl_secs = u32::from_le_bytes(buf[25..29].try_into().ok()?);
    let issued_at_ms = u64::from_le_bytes(buf[29..37].try_into().ok()?);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&buf[37..69]);
    let input_digest = if digest == [0u8; 32] {
        None
    } else {
        Some(digest)
    };
    // 蓝图 55：wire 重建走单时段计划构造器（soc 固定 0.5/Optimal 为
    // wire 契约既有语义）。
    Some(ProposedControlPlan::single_entry(
        plan_id,
        path,
        power_kw,
        0.5,
        eneros_solver_core::result::SolveStatus::Optimal,
        ttl_secs,
        issued_at_ms,
        input_digest,
    ))
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

/// 结果解码（测试/消费侧）。
pub fn decode_response(resp: &[u8]) -> Result<ProposedControlPlan, String> {
    match resp.first() {
        Some(&AI_OK) => decode_proposal(&resp[1..]).ok_or_else(|| String::from("bad proposal")),
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
            Ok(encode_proposal(&plan).to_vec())
        }
    }

    fn trusted() -> TrustedTelemetry {
        tests_helpers::minimal_trusted()
    }

    fn _trusted_impl() -> TrustedTelemetry {
        let now = 10_000u64;
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

    #[test]
    fn propose_with_backend_returns_proposal() {
        let mut worker = AiWorkerService::new(MockBackend { fail: false }, [0xAA; 16]);
        let t = trusted();
        let mut req = vec![AI_PROPOSE];
        req.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        let resp = worker.propose(&req, 10_000);
        let plan = decode_response(&resp).expect("proposal");
        assert_eq!(plan.power_kw, 5.0);
        assert_eq!(plan.input_digest, Some(t.provenance_digest));
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
    fn proposal_wire_roundtrip() {
        let t = trusted();
        let p = ProposedControlPlan {
            plan_id: String::from("req-170000000000"),
            path: eneros_dual_brain::DecisionPath::Fallback,
            schedule: eneros_energy_lp_model::result::ScheduleResult {
                schedule: vec![],
                total_revenue_yuan: 0.0,
                objective_value: 0.0,
                solve_status: eneros_solver_core::result::SolveStatus::Optimal,
            },
            power_kw: -2.5,
            target_device: String::from("pcs"),
            ttl_secs: 300,
            issued_at_ms: 7,
            input_digest: Some(t.provenance_digest),
            model_digest: None,
            latency: Default::default(),
            safety_violations: 0,
            safety_clamped: false,
            feedback: None,
        };
        let d = decode_proposal(&encode_proposal(&p)).expect("decode");
        assert_eq!(d.plan_id, "req-170000000000");
        assert_eq!(d.path, eneros_dual_brain::DecisionPath::Fallback);
        assert_eq!(d.power_kw, -2.5);
        assert_eq!(d.ttl_secs, 300);
        assert_eq!(d.input_digest, Some(t.provenance_digest));
    }

    #[test]
    fn heuristic_discharges_high_soc_with_unique_plan_ids() {
        // SOC 0.6 ≥ 0.5 → 放电 +5kW；plan_id 唯一（幂等键派生源）。
        let mut backend = HeuristicBackend::new();
        let t = trusted(); // soc 0.6
        let w1 = backend.decide(&t, 10_000).expect("discharge");
        let plan1 = decode_proposal(&w1).expect("plan");
        assert_eq!(plan1.power_kw, 5.0);
        assert_eq!(plan1.plan_id, "heur-1");
        let w2 = backend.decide(&t, 10_000).expect("discharge 2");
        let plan2 = decode_proposal(&w2).expect("plan");
        assert_ne!(plan1.plan_id, plan2.plan_id, "plan_id 必须唯一");
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
        let plan = decode_proposal(&w).expect("plan");
        assert_eq!(plan.power_kw, -5.0, "充电为负功率");
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
}
