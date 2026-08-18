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
#[cfg(test)]
use crate::dispatch_service::decode_receipt;
use crate::dispatch_service::DISP_SUBMIT;
#[cfg(test)]
use crate::telemetry::decode_trusted;
use crate::telemetry::SNAPSHOT_OK;

/// 请求码。
pub const EA_TICK: u8 = 1;

/// 响应状态。
pub const EA_OK: u8 = 0;
pub const EA_NOACTION: u8 = 1;
pub const EA_UNAVAILABLE: u8 = 2;
pub const EA_INVALID: u8 = 0xFF;

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
    /// 构造。
    pub fn new(
        telemetry: alloc::boxed::Box<dyn TelemetryClient>,
        worker: alloc::boxed::Box<dyn WorkerClient>,
        dispatch: alloc::boxed::Box<dyn DispatchClient>,
    ) -> Self {
        Self {
            telemetry: TelemetryClientBox(telemetry),
            worker: WorkerClientBox(worker),
            dispatch: DispatchClientBox(dispatch),
            tick_count: 0,
            last_result: None,
        }
    }

    /// 处理一次 tick：telemetry → worker → dispatch。
    pub fn tick(&mut self) -> Vec<u8> {
        // 1. 可信遥测（缺失 → 零推理零新写）。
        let tel_wire = match self.telemetry.0.fetch_telemetry() {
            Ok(w) => w,
            Err(e) => return self.noaction(&e),
        };
        if tel_wire.first() != Some(&SNAPSHOT_OK) {
            return self.noaction("telemetry rejected");
        }
        let tel = &tel_wire[1..];
        // 2. 决策（worker 不可用 → 结构化降级）。
        let mut worker_req = Vec::with_capacity(1 + tel.len());
        worker_req.push(AI_PROPOSE);
        worker_req.extend_from_slice(tel);
        let worker_resp = match self.worker.0.propose(&worker_req) {
            Ok(r) => r,
            Err(e) => return self.noaction(&e),
        };
        if worker_resp.first() == Some(&AI_UNAVAILABLE) {
            return self.unavailable();
        }
        if worker_resp.first() == Some(&crate::ai_worker::AI_NOACTION) {
            // 死区无动作（有能力不动作）——零投递，结构化返回。
            let reason = core::str::from_utf8(&worker_resp[1..]).unwrap_or("no-action");
            return self.noaction(reason);
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
                let out = vec![EA_OK];
                let mut full = out;
                full.extend_from_slice(&receipt_wire);
                self.last_result = Some(full.clone());
                full
            }
            Err(e) => self.noaction(&e),
        }
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
            out.extend_from_slice(&crate::ai_worker::encode_proposal(&plan));
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
        let resp = svc.tick();
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
        let resp = svc.tick();
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
        let resp = svc.tick();
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
        let resp = svc.tick();
        assert_eq!(resp[0], EA_OK, "投递回执透传（Failed 由回执表达）");
        let (_, state, ..) = decode_receipt(&resp[1..]).expect("receipt");
        assert_eq!(state, eneros_control_dispatch::DispatchState::Failed);
        assert_eq!(svc.tick_count, 1);
    }
}
