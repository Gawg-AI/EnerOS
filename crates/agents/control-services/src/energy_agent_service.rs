//! 蓝图 40：svc-energy-agent 编排服务逻辑。
//!
//! 消息路由：可信遥测（svc-telemetry）→ 决策（svc-ai-worker）→ 唯一
//! ControlDispatch（svc-control-dispatch）。各环节为可替换客户端 seam
//! （host 测试用进程内实现；target 用 Sel4IpcClient）。任何环节不可用 →
//! 结构化降级（零新写，不伪造）。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ai_worker::{AI_PROPOSE_SNAP, AI_UNAVAILABLE};
use crate::dispatch_service::decode_receipt;
use crate::dispatch_service::DISP_SUBMIT_TRUSTED;
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

/// recheck 轮（R10 实弹）：持有计划终态后的 System1 兜底静默窗（毫秒）。
/// 依据：最坏设定点跳变爬坡清算 = power_max 5kW ÷ 合同 3.333 kW/s = 1.5s，
/// 加 peer 往返余量取 3s——窗内新远程计划的步 1 不再撞上一计划间歇期
/// System1 写下的设定点基线（run4 实弹：连续 6 计划 |0-5|kW/0.3~0.9s
/// 限拒死亡）。
pub const SYSTEM1_POST_PLAN_HOLD_MS: u64 = 3_000;

/// 遥测客户端（svc-telemetry 消费 seam）。
///
/// 蓝图 70.8 WP-70.8.1：请求码 = `TEL_REQ_SNAPSHOT_INPUT`，应答 =
/// `[SNAPSHOT_OK][TrustedInput wire]`（身份 + 144B 状态）或
/// `[TEL_RESP_ERR, 快照拒绝码]`。旧 `TEL_REQ_SNAPSHOT`（144B 裸遥测）
/// 自此为差分轨——生产决策输入必须携带冻结快照身份。
pub trait TelemetryClient {
    /// 取可信快照输入 wire（身份 + 状态一体）。
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
    /// recheck 轮（R10 实弹）：计划终态后 System1 兜底静默截止时刻
    /// （0 = 无保持）。见 [`SYSTEM1_POST_PLAN_HOLD_MS`]。
    system1_hold_until_ms: u64,
    /// 上一 tick 的在途计划数（终态迁移检测：>0 → 0 = 刚完成）。
    last_held_active: usize,
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
            system1_hold_until_ms: 0,
            last_held_active: 0,
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
        // 1. 可信快照输入（缺失/不可得 → 零推理零新写）。
        let tel_wire = match self.telemetry.0.fetch_telemetry() {
            Ok(w) => w,
            Err(e) => {
                let _ = self.drive_held(None);
                let resp = self.noaction(&e);
                return self.settle(TickOutcome::NoTrustedInput, resp, now_ms);
            }
        };
        if tel_wire.first() != Some(&SNAPSHOT_OK) {
            let _ = self.drive_held(None);
            // 蓝图 70.8：服务端结构化拒绝码（无冻结快照/不完整/过期/背压）
            // ——如实入降级统计，绝不回退旧评估器伪身份。
            let code = tel_wire.get(1).copied().unwrap_or(0);
            let reason = alloc::format!(
                "trusted-input:{}",
                eneros_agent_sdk::snap_input_wire::reject_word(code)
            );
            let resp = self.noaction(&reason);
            return self.settle(TickOutcome::NoTrustedInput, resp, now_ms);
        }
        let input = &tel_wire[1..];
        if eneros_agent_sdk::snap_input_wire::TrustedInput::decode(input).is_none() {
            let _ = self.drive_held(None);
            let resp = self.noaction("trusted-input:malformed");
            return self.settle(TickOutcome::NoTrustedInput, resp, now_ms);
        }
        // Existing plans progress before cooldown/worker/no-action branches.
        // A failed owner tick cannot be bypassed by submitting another plan.
        let held_active = match self.drive_held(Some(input)) {
            Ok(active) => active,
            Err(error) => {
                let resp = self.noaction(&error);
                return self.settle(
                    TickOutcome::PreDispatchError { detail: error },
                    resp,
                    now_ms,
                );
            }
        };
        // ADR-0054（2026-09-19 R10 实弹根因修复）：**计划在途不发新控制
        // 命令**——在途持有计划拥有全部可控设备期间，本地 System1 兜底
        // 单命令与计划步的交错写（实弹 |0-5|kW/0.9s = 5.5 kW/s）触发合同
        // 爬坡限拒（3.333 kW/s）⇒ 计划步终态失败级联。计划终态/取消后的
        // 下个 tick 自动恢复兜底（active 归零）。
        if held_active > 0 {
            let resp = self.noaction("held-plan-active: System1 suppressed");
            return self.settle(TickOutcome::NoAction, resp, now_ms);
        }
        // recheck 轮（R10 run3/4 实弹根因）：**计划终态后的短保持窗**——
        // 旧实现「active 归零即恢复兜底」，System1 在计划间歇立刻写
        // 5kW，0.3~0.9s 后到达的新远程计划步 1（0kW）撞爬坡限拒
        //（|0-5|kW/0.3s）⇒ 计划失败 ⇒ 兜底再写 ⇒ 自持振荡（run4 实弹
        // 连续 6 计划同签名死亡）。保持窗 ≥ 最坏爬坡清算（5kW/3.333kW/s
        // = 1.5s）+ peer 往返余量；窗内远程计划提交顺延（TTL 60s 无碍），
        // 窗后兜底照常。
        let just_finished = self.last_held_active > 0 && held_active == 0;
        if just_finished {
            self.system1_hold_until_ms = now_ms.saturating_add(SYSTEM1_POST_PLAN_HOLD_MS);
        }
        self.last_held_active = held_active;
        if now_ms < self.system1_hold_until_ms {
            let resp = self.noaction("post-plan-ramp-hold: System1 suppressed");
            return self.settle(TickOutcome::NoAction, resp, now_ms);
        }
        if self.policy.in_error() && now_ms < self.cooldown_until_ms {
            return self.noaction("error-budget-exhausted");
        }
        // 2. 决策（worker 不可用 → 结构化降级）。
        // 身份随输入一体传给 worker 与 dispatch（同一次冻结产物）。
        let mut worker_req = Vec::with_capacity(1 + input.len());
        worker_req.push(AI_PROPOSE_SNAP);
        worker_req.extend_from_slice(input);
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
        // 3. 投递（唯一 ControlDispatch；可信输入入口——引擎在入口验证
        //    候选与输入同代，不匹配整批拒绝）。
        let mut disp_req = Vec::with_capacity(1 + proposal_wire.len() + input.len() + 1);
        disp_req.push(DISP_SUBMIT_TRUSTED);
        disp_req.extend_from_slice(proposal_wire);
        disp_req.extend_from_slice(input);
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

    /// 驱动持有计划；成功时返回**在途计划数**（tick 应答第 2 字节，
    /// 2026-09-19 三资源实弹接线）。
    fn drive_held(&mut self, input: Option<&[u8]>) -> Result<usize, String> {
        let mut request = vec![crate::dispatch_service::DISP_HELD_TICK];
        if let Some(input) = input {
            request.extend_from_slice(input);
        }
        let response = self.dispatch.0.submit(&request)?;
        if response.first() != Some(&crate::dispatch_service::DISP_OK) {
            return Err(String::from("held-plan-tick-failed"));
        }
        Ok(response.get(1).copied().unwrap_or(0) as usize)
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

    /// 冻结快照输入 wire（测试：[SNAPSHOT_OK][TrustedInput]——生产同形）。
    fn stub_input_wire() -> (Vec<u8>, eneros_agent_sdk::snap_input_wire::TrustedInput) {
        use eneros_agent_sdk::snap_input_wire::{
            SnapshotFacts, TrustedInput, COMPLETE, SNAP_INPUT_TELEMETRY_LEN,
        };
        let t = crate::ai_worker::tests_helpers::minimal_trusted();
        let payload = crate::telemetry::encode_trusted(&t);
        let mut telemetry = [0u8; SNAP_INPUT_TELEMETRY_LEN];
        telemetry.copy_from_slice(&payload);
        let facts = SnapshotFacts {
            snapshot_id: [0x5C; 16],
            provenance_root: t.provenance_digest,
            cut_time_ns: t.generated_at_ms * 1_000_000,
            produced_at_ns: t.generated_at_ms * 1_000_000,
            catalog_version: 1,
            catalog_digest8: [9, 9, 9, 9, 9, 9, 9, 9],
            accepted: 7,
            missing: 0,
            completeness: COMPLETE,
        };
        let input = TrustedInput::new(facts, telemetry);
        let mut out = vec![SNAPSHOT_OK];
        out.extend_from_slice(&input.encode());
        (out, input)
    }

    struct StubTelemetry {
        ok: bool,
    }
    impl TelemetryClient for StubTelemetry {
        fn fetch_telemetry(&mut self) -> Result<Vec<u8>, String> {
            if self.ok {
                Ok(stub_input_wire().0)
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
            // 用 ai_worker 的 mock 后端逻辑（固定 proposal）。请求形态 =
            // [AI_PROPOSE_SNAP][TrustedInput]（蓝图 70.8 生产形态）。
            assert_eq!(telemetry_wire.first(), Some(&AI_PROPOSE_SNAP));
            let input =
                eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&telemetry_wire[1..])
                    .ok_or_else(|| String::from("bad input"))?;
            let t = decode_trusted(input.telemetry()).ok_or_else(|| String::from("bad tel"))?;
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
                safety_clamped: false,
                feedback: None,
            };
            // 蓝图 70.8：候选绑定冻结快照身份（生产同形）。
            use eneros_control_dispatch::adapter::{proposal_to_candidate_bound, CandidateInputs};
            let inputs = CandidateInputs::from_snapshot(
                input.facts().snapshot_id,
                input.facts().catalog_version,
                input.facts().produced_at_ns / 1_000_000,
                0,
                1,
            );
            let cand = proposal_to_candidate_bound(&plan, &inputs, (0, 0, 0, 0), 0)
                .map_err(|e| alloc::format!("{e:?}"))?;
            let mut out = vec![crate::ai_worker::AI_OK];
            out.extend_from_slice(&crate::ai_worker::encode_candidate_payload(&cand));
            Ok(out)
        }
    }

    struct StubDispatch {
        committed: bool,
        /// 在途计划数（DISP_HELD_TICK 应答第 2 字节注入面；默认 0）。
        held_active: u8,
    }
    impl DispatchClient for StubDispatch {
        fn submit(&mut self, req: &[u8]) -> Result<Vec<u8>, String> {
            if req.first() == Some(&crate::dispatch_service::DISP_HELD_TICK) {
                assert!(
                    req.len() == 1
                        || eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&req[1..])
                            .is_some()
                );
                return Ok(alloc::vec![
                    crate::dispatch_service::DISP_OK,
                    self.held_active,
                ]);
            }
            assert_eq!(
                req.first(),
                Some(&DISP_SUBMIT_TRUSTED),
                "EA 必须以可信输入入口提交（蓝图 70.8 WP-70.8.4）"
            );
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

    /// ADR-0054（2026-09-19 R10 实弹根因）：在途持有计划期间，System1 兜底
    /// 单命令必须被抑制（计划与兜底交错写触发合同爬坡限拒——实弹
    /// |0-5|kW/0.9s=5.5kW/s > 3.333kW/s 计划步终态失败级联）。
    /// 变异靶：去掉抑制 → 本测红（兜底提交到达 dispatch）。
    #[test]
    fn tick_suppresses_system1_while_held_plan_active() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 1,
            }),
        );
        let resp = svc.tick(0);
        assert_eq!(
            resp.first(),
            Some(&EA_NOACTION),
            "在途计划 tick 必须 NOACTION（抑制 System1）"
        );
        assert!(
            String::from_utf8_lossy(&resp).contains("held-plan-active"),
            "抑制理由可观测：{:?}",
            String::from_utf8_lossy(&resp)
        );
    }

    /// recheck 轮（R10 run3/4 实弹根因）：计划终态后的**短保持窗**——
    /// 计划刚完成（active 1→0）时，System1 兜底必须再静默
    /// [`SYSTEM1_POST_PLAN_HOLD_MS`]（否则兜底立即写 5kW，0.3~0.9s 后
    /// 到达的新远程计划步 1（0kW）撞爬坡限拒——自持振荡杀死每个后继
    /// 计划）。窗后恢复兜底。变异靶：去掉保持窗 → 本测红（间歇 tick
    /// 的兜底提交到达 dispatch）。
    #[test]
    fn tick_holds_system1_after_plan_terminal_within_ramp_clearance() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 1,
            }),
        );
        // tick0：计划在途 → 抑制。
        assert_eq!(svc.tick(0).first(), Some(&EA_NOACTION));
        // 终态迁移：上一 tick 在途（last_held_active=1）、本 tick 归零——
        // 计划刚完成的间歇。
        let mut svc2 = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 0,
            }),
        );
        svc2.last_held_active = 1;
        let resp = svc2.tick(100);
        assert_eq!(
            resp.first(),
            Some(&EA_NOACTION),
            "计划刚完成：保持窗内兜底必须静默"
        );
        assert!(
            String::from_utf8_lossy(&resp).contains("post-plan-ramp-hold"),
            "保持窗理由可观测：{:?}",
            String::from_utf8_lossy(&resp)
        );
        // 窗内中途仍静默。
        assert_eq!(
            svc2.tick(100 + SYSTEM1_POST_PLAN_HOLD_MS / 2).first(),
            Some(&EA_NOACTION)
        );
        // 窗后恢复兜底（到达 dispatch，EA_OK 回执）。
        let after = svc2.tick(100 + SYSTEM1_POST_PLAN_HOLD_MS + 1);
        assert_eq!(after[0], EA_OK, "保持窗到期恢复兜底");
        let (status, ..) = decode_receipt(&after[1..]).expect("receipt");
        assert_eq!(status, 0);
    }

    #[test]
    fn tick_full_chain_commits() {
        let mut svc = EnergyAgentService::new(
            alloc::boxed::Box::new(StubTelemetry { ok: true }),
            alloc::boxed::Box::new(StubWorker { fail: false }),
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 0,
            }),
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
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 0,
            }),
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
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 0,
            }),
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
            alloc::boxed::Box::new(StubDispatch {
                committed: false,
                held_active: 0,
            }),
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
            alloc::boxed::Box::new(StubDispatch {
                committed: true,
                held_active: 0,
            }),
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
            alloc::boxed::Box::new(StubDispatch {
                committed: false,
                held_active: 0,
            }),
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
        svc.dispatch.0 = alloc::boxed::Box::new(StubDispatch {
            committed: true,
            held_active: 0,
        });
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
