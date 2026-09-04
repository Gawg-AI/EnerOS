//! 蓝图 40：svc-energy-agent 编排服务逻辑。
//!
//! 消息路由：可信遥测（svc-telemetry）→ 决策（svc-ai-worker）→ 唯一
//! ControlDispatch（svc-control-dispatch）。各环节为可替换客户端 seam
//! （host 测试用进程内实现；target 用 Sel4IpcClient）。任何环节不可用 →
//! 结构化降级（零新写，不伪造）。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ai_worker::{AI_PROPOSE, AI_UNAVAILABLE};
use crate::dispatch_service::decode_receipt;
use crate::dispatch_service::DISP_SUBMIT;
#[cfg(test)]
use crate::telemetry::decode_trusted;
use crate::telemetry::SNAPSHOT_OK;

/// 请求码。
pub const EA_TICK: u8 = 1;

// 蓝图 64 卷 64.2（WP-64.2.1b）：EA 响应状态码族迁 agent-sdk `ea_wire`
// 单源（root 手写 4 臂镜像收编）；本处 re-export 保持既有调用点零改动。
pub use eneros_agent_sdk::ea_wire::{EA_INVALID, EA_NOACTION, EA_OK, EA_UNAVAILABLE};

/// 熔断默认冷却时长（毫秒；蓝图 57 D2：预算耗尽后跳过推理的止损窗口，
/// 到期放行单次探针，Committed/NoAction 复位）。
pub const DEFAULT_COOLDOWN_MS: u64 = 30_000;

/// 遥测客户端（svc-telemetry 消费 seam）。
pub trait TelemetryClient {
    /// 取受信任遥测 wire（TEL_SNAPSHOT 响应）。
    fn fetch_telemetry(&mut self) -> Result<Vec<u8>, String>;
}

/// 决策客户端（svc-ai-worker 消费 seam）。
pub trait WorkerClient {
    /// 请求决策（可信遥测 wire → proposal wire）。
    fn propose(&mut self, telemetry_wire: &[u8]) -> Result<Vec<u8>, String>;
}

/// 投递客户端（svc-control-dispatch 消费 seam）。
pub trait DispatchClient {
    /// 提交（proposal wire + telemetry wire + actor）→ 回执 wire。
    fn submit(&mut self, req: &[u8]) -> Result<Vec<u8>, String>;
}

/// 编排服务逻辑。
pub struct EnergyAgentService {
    /// 遥测客户端。
    pub telemetry: crate::energy_agent_service::TelemetryClientBox,
    /// 决策客户端。
    pub worker: crate::energy_agent_service::WorkerClientBox,
    /// 投递客户端。
    pub dispatch: crate::energy_agent_service::DispatchClientBox,
    /// 蓝图 57：tick 记账单源（错误预算/分类/升级——与进程内 EnergyAgent
    /// 同一实现；学习钩子不入目标侧，wire 无诚实学习输入，蓝图 57 D3）。
    pub policy: eneros_dual_brain::TickPolicy,
    /// 熔断冷却时长（毫秒）。
    pub cooldown_ms: u64,
    /// 冷却截止时刻（0 = 无熔断；预算耗尽时 = now + cooldown_ms）。
    cooldown_until_ms: u64,
    /// tick 计数。
    pub tick_count: u64,
    /// 最近一次编排结果（诊断）。
    pub last_result: Option<Vec<u8>>,
}

/// trait 对象包装（no_std 无 dyn 限制问题）。
pub struct TelemetryClientBox(pub alloc::boxed::Box<dyn TelemetryClient>);
pub struct WorkerClientBox(pub alloc::boxed::Box<dyn WorkerClient>);
pub struct DispatchClientBox(pub alloc::boxed::Box<dyn DispatchClient>);

impl EnergyAgentService {
    /// 构造（冷却时长取 [`DEFAULT_COOLDOWN_MS`]）。
    pub fn new(
        telemetry: alloc::boxed::Box<dyn TelemetryClient>,
        worker: alloc::boxed::Box<dyn WorkerClient>,
        dispatch: alloc::boxed::Box<dyn DispatchClient>,
    ) -> Self {
        Self::with_cooldown(telemetry, worker, dispatch, DEFAULT_COOLDOWN_MS)
    }

    /// 构造（指定熔断冷却时长；测试注入小窗口用）。
    pub fn with_cooldown(
        telemetry: alloc::boxed::Box<dyn TelemetryClient>,
        worker: alloc::boxed::Box<dyn WorkerClient>,
        dispatch: alloc::boxed::Box<dyn DispatchClient>,
        cooldown_ms: u64,
    ) -> Self {
        Self {
            telemetry: TelemetryClientBox(telemetry),
            worker: WorkerClientBox(worker),
            dispatch: DispatchClientBox(dispatch),
            policy: eneros_dual_brain::TickPolicy::new(),
            cooldown_ms,
            cooldown_until_ms: 0,
            tick_count: 0,
            last_result: None,
        }
    }

    /// 处理一次 tick：telemetry → worker → dispatch。
    ///
    /// 蓝图 57 D2：错误预算耗尽 → 冷却期内跳过推理直接
    /// `EA_NOACTION("error-budget-exhausted")`（止损：不烧推理、不再撞
    /// dispatch）；冷却到期放行单次探针，Committed/NoAction 复位恢复。
    /// 响应码词汇与原实现不变。
    pub fn tick(&mut self, now_ms: u64) -> Vec<u8> {
        use eneros_dual_brain::TickOutcome;
        if self.policy.in_error() && now_ms < self.cooldown_until_ms {
            return self.noaction("error-budget-exhausted");
        }
        // 1. 可信遥测（缺失 → 零推理零新写）。
        let tel_wire = match self.telemetry.0.fetch_telemetry() {
            Ok(w) => w,
            Err(e) => {
                let resp = self.noaction(&e);
                return self.settle(TickOutcome::NoTrustedInput, resp, now_ms);
            }
        };
        if tel_wire.first() != Some(&SNAPSHOT_OK) {
            let resp = self.noaction("telemetry rejected");
            return self.settle(TickOutcome::NoTrustedInput, resp, now_ms);
        }
        let tel = &tel_wire[1..];
        // 2. 决策（worker 不可用 → 结构化降级）。
        let mut worker_req = Vec::with_capacity(1 + tel.len());
        worker_req.push(AI_PROPOSE);
        worker_req.extend_from_slice(tel);
        let worker_resp = match self.worker.0.propose(&worker_req) {
            Ok(r) => r,
            Err(e) => {
                let resp = self.noaction(&e);
                return self.settle(
                    TickOutcome::PreDispatchError { detail: e.clone() },
                    resp,
                    now_ms,
                );
            }
        };
        if worker_resp.first() == Some(&AI_UNAVAILABLE) {
            let resp = self.unavailable();
            return self.settle(TickOutcome::Unavailable, resp, now_ms);
        }
        if worker_resp.first() == Some(&crate::ai_worker::AI_NOACTION) {
            // 死区无动作（有能力不动作）——零投递，结构化返回。
            let reason = core::str::from_utf8(&worker_resp[1..]).unwrap_or("no-action");
            let resp = self.noaction(reason);
            return self.settle(TickOutcome::NoAction, resp, now_ms);
        }
        let proposal_wire = &worker_resp[1..];
        // 3. 投递（唯一 ControlDispatch）。
        let mut disp_req = Vec::with_capacity(1 + proposal_wire.len() + tel.len() + 1);
        disp_req.push(DISP_SUBMIT);
        disp_req.extend_from_slice(proposal_wire);
        disp_req.extend_from_slice(tel);
        disp_req.push(2); // ActorKind::Ai
        match self.dispatch.0.submit(&disp_req) {
            Ok(receipt_wire) => {
                self.tick_count += 1;
                // 回执终态入账（蓝图 57 D2 增益：Failed 不再零记账）。
                let decoded = decode_receipt(&receipt_wire);
                let committed = matches!(
                    decoded.as_ref().map(|(_, s, ..)| s),
                    Some(eneros_control_dispatch::DispatchState::Committed)
                );
                let outcome = if committed {
                    TickOutcome::Committed
                } else {
                    let note = decoded
                        .map(|(_, _, _, _, n)| n)
                        .unwrap_or_else(|| alloc::string::String::from("receipt-uncommitted"));
                    TickOutcome::DispatchRejected { note }
                };
                let mut full = vec![EA_OK];
                full.extend_from_slice(&receipt_wire);
                self.settle(outcome, full, now_ms)
            }
            Err(e) => {
                let resp = self.noaction(&e);
                self.settle(
                    TickOutcome::PreDispatchError { detail: e.clone() },
                    resp,
                    now_ms,
                )
            }
        }
    }

    /// 结算单点：结果入 TickPolicy；升级时重设熔断冷却截止。
    fn settle(
        &mut self,
        outcome: eneros_dual_brain::TickOutcome,
        resp: Vec<u8>,
        now_ms: u64,
    ) -> Vec<u8> {
        if self.policy.record(&outcome) == eneros_dual_brain::TickVerdict::Escalated {
            self.cooldown_until_ms = now_ms.saturating_add(self.cooldown_ms);
        }
        self.last_result = Some(resp.clone());
        resp
    }

    fn noaction(&mut self, reason: &str) -> Vec<u8> {
        let mut out = vec![EA_NOACTION];
        out.extend_from_slice(reason.as_bytes());
        self.last_result = Some(out.clone());
        out
    }

    fn unavailable(&mut self) -> Vec<u8> {
        let mut out = vec![EA_UNAVAILABLE];
        out.extend_from_slice(b"ai-backend-unavailable");
        self.last_result = Some(out.clone());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubTelemetry {
        ok: bool,
    }
    impl TelemetryClient for StubTelemetry {
        fn fetch_telemetry(&mut self) -> Result<Vec<u8>, String> {
            if self.ok {
                let t = crate::ai_worker::tests_helpers::minimal_trusted();
                let mut out = vec![SNAPSHOT_OK];
                out.extend_from_slice(&crate::telemetry::encode_trusted(&t));
                Ok(out)
            } else {
                Err(String::from("telemetry-unavailable"))
            }
        }
    }

    struct StubWorker {
        fail: bool,
    }
    impl WorkerClient for StubWorker {
        fn propose(&mut self, telemetry_wire: &[u8]) -> Result<Vec<u8>, String> {
            if self.fail {
                return Ok(vec![AI_UNAVAILABLE]);
            }
            // 用 ai_worker 的 mock 后端逻辑（固定 proposal）。
            let t = decode_trusted(&telemetry_wire[1..]).ok_or_else(|| String::from("bad tel"))?;
            let plan = eneros_dual_brain::ProposedControlPlan {
                plan_id: String::from("req-orch-1"),
                path: eneros_dual_brain::DecisionPath::Fast,
                schedule: eneros_energy_lp_model::result::ScheduleResult {
                    schedule: vec![eneros_energy_lp_model::result::ScheduleEntry {
                        period: 0,
                        charge_power_kw: 0.0,
                        discharge_power_kw: 5.0,
                        net_power_kw: 5.0,
                        soc_pct: t.state.soc_pct,
                        revenue_yuan: 0.0,
                    }],
                    total_revenue_yuan: 0.0,
                    objective_value: 0.0,
                    solve_status: eneros_solver_core::result::SolveStatus::Optimal,
                },
                power_kw: 5.0,
                target_device: String::from("pcs"),
                ttl_secs: 60,
                issued_at_ms: t.generated_at_ms,
                input_digest: Some(t.provenance_digest),
                model_digest: None,
                latency: Default::default(),
                safety_violations: 0,
                safety_clamped: false,
                feedback: None,
            };
            let mut out = vec![crate::ai_worker::AI_OK];
            out.extend_from_slice(&crate::ai_worker::encode_candidate_payload(
                &crate::ai_worker::proposal_to_candidate_unbound(&plan).expect("stub cand"),
            ));
            Ok(out)
        }
    }

    struct StubDispatch {
        committed: bool,
    }
    impl DispatchClient for StubDispatch {
        fn submit(&mut self, _req: &[u8]) -> Result<Vec<u8>, String> {
            let state = if self.committed {
                eneros_control_dispatch::DispatchState::Committed
            } else {
                eneros_control_dispatch::DispatchState::Failed
            };
            let r = eneros_control_dispatch::DispatchReceipt {
                command_id: [1; 16],
                correlation_id: [2; 16],
                final_state: state,
                state_path: vec![state],
                committed_at_ms: if self.committed { Some(1) } else { None },
                note: if self.committed {
                    None
                } else {
                    Some(String::from("device-denied"))
                },
                latency_ms: 1,
            };
            Ok(crate::dispatch_service::encode_receipt(&r))
        }
    }

    #[test]
    fn tick_full_chain_commits() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch { committed: true }),
        );
        let resp = svc.tick(0);
        assert_eq!(resp[0], EA_OK);
        assert_eq!(svc.tick_count, 1);
        let (status, state, ..) = decode_receipt(&resp[1..]).expect("receipt");
        assert_eq!(status, 0);
        assert_eq!(state, eneros_control_dispatch::DispatchState::Committed);
    }

    #[test]
    fn tick_without_telemetry_noaction() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: false }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch { committed: true }),
        );
        let resp = svc.tick(0);
        assert_eq!(resp[0], EA_NOACTION);
        assert_eq!(svc.tick_count, 0, "无遥测不得推进决策");
    }

    #[test]
    fn tick_backend_unavailable_degraded() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: true }),
            alloc::boxed::Box::new(StubDispatch { committed: true }),
        );
        let resp = svc.tick(0);
        assert_eq!(resp[0], EA_UNAVAILABLE);
        assert_eq!(svc.tick_count, 0);
    }

    #[test]
    fn tick_dispatch_failure_propagates() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch { committed: false }),
        );
        let resp = svc.tick(0);
        assert_eq!(resp[0], EA_OK, "投递回执透传（Failed 由回执表达）");
        let (_, state, ..) = decode_receipt(&resp[1..]).expect("receipt");
        assert_eq!(state, eneros_control_dispatch::DispatchState::Failed);
        assert_eq!(svc.tick_count, 1);
    }
    /// 蓝图 57 D2 验收：连续失败达预算 → 熔断（冷却期内跳过推理）。
    #[test]
    fn error_budget_exhaustion_trips_breaker() {
        let mut svc = EnergyAgentService::with_cooldown(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: true }),
            alloc::boxed::Box::new(StubDispatch { committed: true }),
            1_000,
        );
        // 连续 3 次 Unavailable → Escalated + 冷却设定。
        for t in 0..3u64 {
            let resp = svc.tick(t * 100);
            assert_eq!(resp[0], EA_UNAVAILABLE);
        }
        assert!(svc.policy.in_error());
        // 冷却期内：零推理直接 NOACTION（worker 不再被调用——fail=true 时
        // 若被调用会再次返回 UNAVAILABLE 而非 NOACTION，可作判据）。
        let resp = svc.tick(500);
        assert_eq!(resp[0], EA_NOACTION);
        assert_eq!(&resp[1..], b"error-budget-exhausted");
    }

    /// 蓝图 57 D2 验收：冷却到期探针失败 → 重设冷却；成功 → 复位恢复。
    #[test]
    fn breaker_probe_and_recovery() {
        let mut svc = EnergyAgentService::with_cooldown(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch { committed: false }),
            1_000,
        );
        // Failed 回执连续 3 次 → 熔断（EA_OK 透传回执但预算记账）。
        for t in 0..3u64 {
            svc.tick(t * 100);
        }
        assert!(svc.policy.in_error());
        // 冷却内跳过。
        assert_eq!(svc.tick(500)[0], EA_NOACTION);
        // 冷却到期（now=1500 ≥ 200+1000）：探针放行——Failed 再次升级，
        // 冷却重设为 1500+1000。
        let resp = svc.tick(1_500);
        assert_eq!(resp[0], EA_OK, "探针 tick 正常编排并透传回执");
        assert!(svc.policy.in_error(), "探针仍 Failed → 保持错误态");
        assert_eq!(svc.policy.consecutive_errors(), 4);
        // 再冷却内跳过；换 Committed dispatch 后到期探针成功 → 复位。
        assert_eq!(svc.tick(2_000)[0], EA_NOACTION);
        svc.dispatch.0 = alloc::boxed::Box::new(StubDispatch { committed: true });
        let resp = svc.tick(2_600);
        assert_eq!(resp[0], EA_OK);
        assert!(!svc.policy.in_error(), "Committed 复位错误态");
        assert_eq!(svc.policy.consecutive_errors(), 0);
        // 恢复后正常编排（不再受冷却拦截）。
        assert_eq!(svc.tick(2_700)[0], EA_OK);
    }
}

#[cfg(test)]
mod ea_display_guard {
    /// 蓝图 66 WP-66.6.4：svc-energy-agent 证据行词形 golden 守卫
    /// （root service.rs 既有守卫 token 复制——bin 侧 4 臂手写 match
    /// 已换 ea_wire::ea_display 单源；status= 行逐字节不变）。
    #[test]
    fn svc_energy_agent_status_line_uses_ea_display() {
        let src = include_str!("bin/svc-energy-agent.rs");
        let t_noact = concat!("\"NO", "ACTION\"");
        let t_unavail = concat!("\"UNAV", "AILABLE\"");
        assert!(
            !src.contains(t_noact) && !src.contains(t_unavail),
            "EA 手写臂回归（应走 ea_wire::ea_display）"
        );
        assert!(
            src.contains("ea_wire::ea_display"),
            "EA 显示面必须经 agent-sdk ea_wire 单源"
        );
        assert!(
            src.contains("status={status}"),
            "证据行词形漂移：[svc-energy-agent] tick=… status=… 须逐字节保持"
        );
    }
}
