//! SPEC-SERIES-2026-08-28/B WP-B.3：AI tick 编排单源（svc-ai-worker 双 handler 收编）。
//!
//! 旧 bin 内 `RemoteAiHandler` / `HeuristicOnlyHandler` 互相复制：tick 编排
//! （SOC 提取→阈值注入→propose→审计→observe）两份、LEARN_STATUS 50B 组装
//! 两份、LearnOutcome 串口匹配两份、LEARN_RESET 两份（且词形已漂移）。本模块
//! 把编排收敛为 [`AiTickHandler`] 单源；两 handler 的**真实差异**收窄为
//! [`TickAudit`] 钩子（remote 留痕/peer 推送 vs 固定启发式留痕）。
//!
//! # 注入路径等价性分析（WP-B.3 红线 3，统一前必证）
//!
//! 旧 bin 两条阈值注入路径：
//! 1. remote：`AiWorkerService<RemoteBrain<HeuristicBackend, P>>::
//!    backend_mut().local_mut().set_deadband(...)` —— [`RemoteBrain::
//!    local_mut`](crate::remote_brain::RemoteBrain::local_mut) 返回其
//!    System1 本地后端（`self.local`，即 decide 启发式回退所用实例）；
//! 2. heuristic-only：`AiWorkerService<HeuristicBackend>::backend_mut()
//!    .set_deadband(...)`。
//!
//! 两条路径终止于**同一对象**（本 tick `propose→decide` 所用的那个
//! [`HeuristicBackend`]）的同一方法 `set_deadband`；差异纯为访问器链
//! （`RemoteBrain` 是包装而非另一个后端）。何时等价：注入发生在
//! `propose` 之前——本模块 tick 编排恒满足。何时不等价：若
//! `RemoteBrain::decide` 远程路径不咨询 `self.local`，注入在该 tick 无
//! 观察效果——但这是 RemoteBrain 的决策语义（两路径共有），不构成注入
//! 路径差异。**结论：等价，统一为 [`DeadbandTarget`] 单点**；该结论有
//! [`tests::deadband_target_wrapped_and_direct_are_equivalent`] 锁定。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::ai_relay::AI_PEER_MSG;
use eneros_dual_brain::audit::{DecisionAudit, DecisionRecord, ExecOutcome, SafetyOutcome};
use eneros_durable_state::PersistIo;
use eneros_hex::encode_lower as hex_encode_lower;

use crate::ai_worker::{
    learn_wire, AiWorkerService, DecisionBackend, DegradeObservation, HeuristicBackend, AI_OK,
    AI_PROPOSE,
};
use crate::learn::{DeadbandLearner, LearnOutcome};
use crate::remote_brain::{PeerLink, PeerMsgVerdict, RemoteBrain, SwitchEvent};
use crate::service::ControlServiceHandler;

/// System1 死区注入面（WP-B.3 单点——两条历史注入路径的收编目标）。
///
/// 语义等价性分析见模块头；实现必须注入到 decide 实际使用的
/// `HeuristicBackend` 实例上。
pub trait DeadbandTarget {
    /// 注入当前自适应死区阈值（界内不变式由学习侧 catalog 保证，
    /// `HeuristicBackend::set_deadband` 防御性钳制兜底）。
    fn apply_deadband(&mut self, charge_soc: f64, discharge_soc: f64);
}

impl DeadbandTarget for HeuristicBackend {
    fn apply_deadband(&mut self, charge_soc: f64, discharge_soc: f64) {
        self.set_deadband(charge_soc, discharge_soc);
    }
}

impl<P: PeerLink> DeadbandTarget for RemoteBrain<HeuristicBackend, P> {
    fn apply_deadband(&mut self, charge_soc: f64, discharge_soc: f64) {
        self.local_mut().set_deadband(charge_soc, discharge_soc);
    }
}

/// 68.5 WP-68.5.3：降级武装查询面（tick 编排的学习冻结判据——武装期
/// 死区参数必须冻结：学习继续改阈值 ⇒ strategy digest 漂移 ⇒ lease
/// 白名单失配 ⇒ System1 断供。次生面处置，机制面零改动）。
pub trait DegradeArmQuery {
    fn is_degradation_armed(&self) -> bool;
}

impl DegradeArmQuery for HeuristicBackend {
    fn is_degradation_armed(&self) -> bool {
        HeuristicBackend::degradation_armed(self)
    }
}

impl<P: PeerLink> DegradeArmQuery for RemoteBrain<HeuristicBackend, P> {
    fn is_degradation_armed(&self) -> bool {
        self.local_degradation_armed()
    }
}

/// 蓝图 69 卷 69.5（WP-69.5.1）：降级/学习**只读**观测查询面。
///
/// 与 [`DegradeArmQuery`] 分立（不改既有 trait——订正 ⑧ 纪律：判据面
/// 与观测面是两件事）；实现只允许装配既有量的只读投影
/// （[`DegradeObservation`]），**不得**在此面引入任何 setter/写路径
/// （Z3/Z5/Z2）。计数器字段（applies/frozen_skips）由持有 learn 的
/// [`AiTickHandler`] 装配时填入——本面回 0 值占位。
pub trait DegradeObserveQuery {
    /// 只读快照（`now_ms` 仅用于既有 `degradation_lease_expired` 判定
    /// 的透传；本方法自身不改任何状态）。
    fn degrade_observation(&self, now_ms: u64) -> DegradeObservation;
}

impl DegradeObserveQuery for HeuristicBackend {
    fn degrade_observation(&self, now_ms: u64) -> DegradeObservation {
        HeuristicBackend::degrade_observation(self, now_ms)
    }
}

impl<P: PeerLink> DegradeObserveQuery for RemoteBrain<HeuristicBackend, P> {
    fn degrade_observation(&self, now_ms: u64) -> DegradeObservation {
        self.local().degrade_observation(now_ms)
    }
}

/// tick 留痕钩子（remote 与 heuristic-only 的行为差异收窄点）。
pub trait TickAudit<B: DecisionBackend> {
    /// propose 成功（AI_OK）后的决策留痕：切换事件/路径标记入审计链
    /// 与串口（词形逐字节保留自旧 bin，R9）。
    fn on_proposal_ok(&mut self, backend: &mut B, now_ms: u64);

    /// 未识别请求码扩展面（默认不消费 → 调用方 0xFF 兜底）；remote
    /// 路径用于 AI_PEER_MSG 推送（不受信 JSON 全量校验 + 配对）。
    fn on_other(
        &mut self,
        _backend: &mut B,
        _req: &[u8],
        _resp: &mut Vec<u8>,
        _now_ms: u64,
    ) -> bool {
        false
    }
}

/// 决策审计入链 + 串口证据行（词形逐字节保留自旧 bin `log_decision`）。
pub(crate) fn log_ai_decision(audit: &mut DecisionAudit, intent: &str, lp: &str, now_ms: u64) {
    let _ = audit.log_decision(&DecisionRecord {
        actor: "ai-worker",
        target: "pcs",
        intent,
        lp_solution: lp,
        safety: SafetyOutcome::Passed,
        exec: ExecOutcome::Proposed,
        now_ms,
    });
    eneros_runtime::println!(
        "[svc-ai-worker] AUDIT-AI appended={} dropped={}",
        audit.chain_state().entry_count,
        audit.dropped_count()
    );
}

/// 蓝图 69 卷 69.5（WP-69.5.1）：`DEGRADE-OBS` 串口行**纯函数**（词形
/// 单点——`degrade_observe` 只负责装配快照与落盘输出；t30b 饱和语义
/// 锁的测试锚：`expires_in_ms` 用 saturating 减，到期后必须为 0 而非
/// 回绕巨大值——观测行的数字供词形基线脚本解析，负数/巨大值即假证）。
fn degrade_obs_line(obs: &DegradeObservation, now_ms: u64) -> alloc::string::String {
    alloc::format!(
        "[svc-ai-worker] DEGRADE-OBS armed={} expired={} level={} lease={} uses={}/{} expires_in_ms={} deadband=({:.6},{:.6}) applies={} frozen_skips={}",
        if obs.armed { "true" } else { "false" },
        if obs.expired { "true" } else { "false" },
        obs.level,
        hex_encode_lower(&obs.lease_id[..8]),
        obs.uses_left,
        obs.uses_max,
        obs.expires_at_ms.saturating_sub(now_ms),
        obs.charge_soc,
        obs.discharge_soc,
        obs.deadband_applies,
        obs.deadband_frozen_skips,
    )
}

/// 纯启发式路径留痕（固定 sys1 标记——词形保留自旧 bin）。
pub struct HeuristicAudit<'a> {
    /// 决策审计链（ADR-0042 门面）。
    pub audit: &'a mut DecisionAudit,
}

impl TickAudit<HeuristicBackend> for HeuristicAudit<'_> {
    fn on_proposal_ok(&mut self, _backend: &mut HeuristicBackend, now_ms: u64) {
        log_ai_decision(self.audit, "sys1-heuristic", "deadband ±5kW", now_ms);
    }
}

/// remote dual-brain 路径留痕（切换事件 + REMOTE-BRAIN 元数据 +
/// AI_PEER_MSG 推送——词形逐字节保留自旧 bin `log_proposal`/peer 臂）。
pub struct RemoteAudit<'a> {
    /// 决策审计链（ADR-0042 门面）。
    pub audit: &'a mut DecisionAudit,
}

impl<P: PeerLink> TickAudit<RemoteBrain<HeuristicBackend, P>> for RemoteAudit<'_> {
    fn on_proposal_ok(&mut self, backend: &mut RemoteBrain<HeuristicBackend, P>, now_ms: u64) {
        let events: Vec<SwitchEvent> = backend.drain_events();
        for ev in &events {
            match ev {
                SwitchEvent::PeerUnhealthy {
                    consecutive_timeouts,
                } => {
                    eneros_runtime::println!(
                        "[svc-ai-worker] REMOTE-BRAIN event=peer-unhealthy consecutive={consecutive_timeouts}"
                    );
                    log_ai_decision(self.audit, "event:peer-unhealthy", "", now_ms);
                }
                SwitchEvent::PeerRecovered => {
                    eneros_runtime::println!("[svc-ai-worker] REMOTE-BRAIN event=peer-recovered");
                    log_ai_decision(self.audit, "event:peer-recovered", "", now_ms);
                }
                SwitchEvent::LeaseArmed { lease_id } => {
                    eneros_runtime::println!(
                        "[svc-ai-worker] LEASE-ARMED level=D3 lease_id={} —— peer 失联，System1 兜底持签降级（68.5 WP-68.5.3）",
                        hex_encode_lower(lease_id)
                    );
                    log_ai_decision(self.audit, "event:lease-armed", "", now_ms);
                }
                SwitchEvent::LeaseArmFailed => {
                    eneros_runtime::println!(
                        "[svc-ai-worker] LEASE-ARM-FAIL —— 降级武装被拒（结构性不可达面：全零键/TTL=0 已被装配守卫拦、自签验签必过；若达此面=无签兜底继续=68.5 前行为，须查配置链）"
                    );
                    log_ai_decision(self.audit, "event:lease-arm-fail", "", now_ms);
                }
                SwitchEvent::LeaseRevoked { reason } => {
                    match *reason {
                        "peer-accepted" => {
                            eneros_runtime::println!(
                                "[svc-ai-worker] LEASE-REVOKED reason=peer-accepted —— 有效应答消费，超时 episode 结束（回 D0）"
                            );
                        }
                        "lease-expired" => {
                            eneros_runtime::println!(
                                "[svc-ai-worker] LEASE-REVOKED reason=lease-expired —— lease 到期复位，episode 自然终止（后续超时裁决重武装）"
                            );
                        }
                        other => {
                            eneros_runtime::println!(
                                "[svc-ai-worker] LEASE-REVOKED reason={other}"
                            );
                        }
                    }
                    log_ai_decision(self.audit, "event:lease-revoked", "", now_ms);
                }
            }
        }
        let healthy = backend.is_healthy();
        let timeouts = backend.timeouts_total;
        let bad = backend.bad_responses_total;
        let late = backend.late_total;
        let link_err = backend.link_errors;
        if let Some(meta) = backend.last_meta() {
            eneros_runtime::println!(
                "[svc-ai-worker] REMOTE-BRAIN used={} path={:?} req={:?} prompt=\"{}\" resp=\"{}\" healthy={} timeouts={} bad={} late={} link_err={}",
                meta.used_remote,
                meta.path,
                meta.req_id,
                meta.prompt_summary,
                meta.resp_summary,
                healthy,
                timeouts,
                bad,
                late,
                link_err,
            );
            let intent = if meta.used_remote {
                format!("sys2-remote {}", meta.prompt_summary)
            } else {
                String::from("sys1-heuristic")
            };
            let sol = if meta.used_remote {
                meta.resp_summary.clone()
            } else {
                String::from("deadband ±5kW")
            };
            log_ai_decision(self.audit, &intent, &sol, now_ms);
        }
    }

    fn on_other(
        &mut self,
        backend: &mut RemoteBrain<HeuristicBackend, P>,
        req: &[u8],
        resp: &mut Vec<u8>,
        now_ms: u64,
    ) -> bool {
        if req.first() != Some(&AI_PEER_MSG) {
            return false;
        }
        // peer 应答推送（不受信 JSON）→ 全量校验 + 配对。
        let detail;
        let verdict = match core::str::from_utf8(&req[1..]) {
            Ok(j) => {
                let v = backend.on_peer_message(j, now_ms);
                detail = backend.last_reject_detail.clone().unwrap_or_default();
                v
            }
            Err(e) => {
                backend.bad_responses_total = backend.bad_responses_total.saturating_add(1);
                detail = format!("utf8:{e}");
                PeerMsgVerdict::Bad
            }
        };
        eneros_runtime::println!(
            "[svc-ai-worker] PEER-MSG verdict={verdict:?} bad={} late={} healthy={} detail={}",
            backend.bad_responses_total,
            backend.late_total,
            backend.is_healthy(),
            detail
        );
        *resp = alloc::vec![AI_PEER_MSG, 0];
        true
    }
}

/// AI tick 编排单源 handler（WP-B.3：SOC 提取→阈值注入→propose→留痕→observe）。
///
/// 时序不变式（红线 2，[`tests::tick_n_observe_tick_n_plus_1_uses_learned_thresholds`]
/// 锁定）：decide **前**注入当前阈值、decide **后**观测学习——tick N 的观测
/// 在 tick N 的 decide 不可见，tick N+1 的 decide 必须可见。
pub struct AiTickHandler<'a, B: DecisionBackend, Io: PersistIo, A: TickAudit<B>> {
    worker: &'a mut AiWorkerService<B>,
    audit: A,
    /// SPEC-E WP-E.3：学习器（闸/授予任一不满足 ⇒ None=出厂行为）。
    learn: Option<DeadbandLearner<Io>>,
    /// 蓝图 69 卷 69.5（WP-69.5.1）：自启动以来成功注入死区的次数
    /// （观测计数——saturating：观测面读到 MAX 比回绕成小值更诚实，
    /// 计数不参与任何判定）。
    deadband_applies: u32,
    /// 自启动以来武装期冻结跳过次数（注入早退 + 复位拒绝两处；观测
    /// 计数，同 saturating 纪律）。
    deadband_frozen_skips: u32,
}

impl<'a, B, Io, A> AiTickHandler<'a, B, Io, A>
where
    B: DecisionBackend + DeadbandTarget + DegradeArmQuery + DegradeObserveQuery,
    Io: PersistIo,
    A: TickAudit<B>,
{
    /// 构造（worker/audit/learn 三件——与旧 bin handler 字段一致）。
    pub fn new(
        worker: &'a mut AiWorkerService<B>,
        audit: A,
        learn: Option<DeadbandLearner<Io>>,
    ) -> Self {
        Self {
            worker,
            audit,
            learn,
            deadband_applies: 0,
            deadband_frozen_skips: 0,
        }
    }

    /// SPEC-E WP-E.3（decide 前）：阈值注入单点（[`DeadbandTarget`]）。
    /// 68.5 WP-68.5.3 次生面处置：降级武装期**冻结注入**——学习继续改
    /// 死区 ⇒ strategy digest 漂移 ⇒ lease 白名单失配 ⇒ System1 断供；
    /// 恢复（LEASE-REVOKED）后自动解冻。
    fn apply_learn_thresholds(&mut self) {
        let Some(l) = self.learn.as_mut() else {
            return;
        };
        if self.worker.backend_mut().is_degradation_armed() {
            // 69.5（WP-69.5.1）：武装期冻结跳过留痕（观测计数；本分支
            // 结构原样保留——与 learn_reset 拒绝分支守不同动作，订正 ⑧）。
            self.deadband_frozen_skips = self.deadband_frozen_skips.saturating_add(1);
            return;
        }
        let (charge, discharge) = l.thresholds();
        self.worker.backend_mut().apply_deadband(charge, discharge);
        // 69.5（WP-69.5.1）：成功注入计数（观测面；学习闭环"自启动以来
        // 被学习改动 N 次"的读出根）。
        self.deadband_applies = self.deadband_applies.saturating_add(1);
    }

    /// SPEC-E WP-E.3（decide 后）：以本帧受信 SOC 观测学习（阈值下一
    /// tick 生效——节拍确定，前后对照可证）；串口证据行单源（旧 bin
    /// 两 handler 词形漂移归一到 remote 全形，登记 BC-B3）。
    fn learn_observe(&mut self, telemetry_soc: Option<f64>) {
        let Some(l) = self.learn.as_mut() else {
            return;
        };
        let Some(soc) = telemetry_soc else {
            return;
        };
        match l.observe(soc) {
            LearnOutcome::Applied { current } => {
                eneros_runtime::println!("[svc-ai-worker] LEARN-APPLY p={current:.6}");
            }
            LearnOutcome::RejectCap { proposed } => {
                eneros_runtime::println!("[svc-ai-worker] LEARN-REJECT-CAP p={proposed:.6}");
            }
            LearnOutcome::PersistFailed => {
                eneros_runtime::println!(
                    "[svc-ai-worker] LEARN-PERSIST-FAIL n={} —— 本步回滚（行为退回上一稳定值）",
                    l.persist_failures
                );
            }
            LearnOutcome::Anchored | LearnOutcome::Disabled => {}
        }
    }

    /// LEARN_STATUS wire（StatusQuery 等价面）：50B 定长回显单源
    /// （旧 bin 两份副本收编；字节布局由 `LEARN_STATUS_WIRE_LEN` 守）。
    fn learn_status_wire(&mut self, out: &mut Vec<u8>) {
        out.clear();
        out.push(learn_wire::AI_LEARN_STATUS);
        match self.learn.as_ref() {
            Some(l) => {
                out.push(1);
                let (min, max) = l.bounds();
                let snap = l.snapshot_view();
                out.extend_from_slice(&snap.current.to_le_bytes());
                out.extend_from_slice(&min.to_le_bytes());
                out.extend_from_slice(&max.to_le_bytes());
                out.extend_from_slice(&snap.samples.to_le_bytes());
                out.extend_from_slice(&snap.rejected_updates.to_le_bytes());
                out.extend_from_slice(&l.applies.to_le_bytes());
            }
            None => {
                // 未装配：enabled=0 + 全零字段（长度不变，便于逐字节对照）。
                out.push(0);
                out.resize(learn_wire::LEARN_STATUS_WIRE_LEN, 0);
            }
        }
    }

    /// LEARN_RESET 处理单源：回出厂 + **立即回注**（旧 heuristic 路径
    /// 缺回注——下一 tick 注入同值，决策等价；登记 BC-B3）+ 串口词形。
    fn learn_reset(&mut self, resp: &mut Vec<u8>) {
        // 68.5 复查修复：武装期拒绝复位——回厂改变死区 ⇒ strategy digest
        // 漂移 ⇒ lease 白名单失配 ⇒ System1 断供（与 apply_learn_thresholds
        // 冻结同判据同理由；附加裁决 8 防线的注入点补口）。
        if self.worker.backend_mut().is_degradation_armed() {
            // 69.5（WP-69.5.1）：复位拒绝同属"武装期冻结跳过"（观测计数；
            // 本 if 与 apply_learn_thresholds 的早退守不同动作、行为本就
            // 不同——拒绝还要回 AI_LEARN_RESET,1 回执，不得合并，订正 ⑧）。
            self.deadband_frozen_skips = self.deadband_frozen_skips.saturating_add(1);
            eneros_runtime::println!(
                "[svc-ai-worker] LEARN-RESET REJECTED reason=degrade-armed —— 武装期死区冻结（digest 漂移防线），peer 恢复后可复位"
            );
            *resp = alloc::vec![learn_wire::AI_LEARN_RESET, 1];
            return;
        }
        match self.learn.as_mut() {
            Some(l) => match l.reset() {
                Ok(()) => {
                    let (charge, discharge) = l.thresholds();
                    self.worker.backend_mut().apply_deadband(charge, discharge);
                    let snap = l.snapshot_view();
                    eneros_runtime::println!(
                        "[svc-ai-worker] LEARN-RESET ok current={:.6} samples={} rejected={} —— 参数回出厂并已持久化",
                        snap.current,
                        snap.samples,
                        snap.rejected_updates
                    );
                    *resp = alloc::vec![learn_wire::AI_LEARN_RESET, 0];
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-ai-worker] LEARN-RESET FAILED {e:?} —— 保持旧态（fail-closed）"
                    );
                    *resp = alloc::vec![learn_wire::AI_LEARN_RESET, 1];
                }
            },
            None => {
                eneros_runtime::println!(
                    "[svc-ai-worker] LEARN-RESET unavailable —— 学习未装配（闸/授予）"
                );
                *resp = alloc::vec![learn_wire::AI_LEARN_RESET, 1];
            }
        }
    }

    /// 蓝图 69 卷 69.5（WP-69.5.1）：DEGRADE-OBSERVE 只读观测处理——
    /// 装配快照（后端既有量 + 本 handler 计数器）→ 串口 `DEGRADE-OBS`
    /// 行 + 68B 定长响应。**只读纪律**：本方法不改任何状态（计数器只在
    /// 注入/冻结路径推进，本路径零接触——t31/Q5 的对象）。
    ///
    /// 词形（Z2，随卷 69.5 登记基线）：
    /// `[svc-ai-worker] DEGRADE-OBS armed={} expired={} level={} lease={}
    /// uses={}/{} expires_in_ms={} deadband=({},{}) applies={} frozen_skips={}`
    /// - `lease` = lease_id 前 8 字节 hex（武装时刻 u128 LE 的高对账半段；
    ///   16 字节全打超行预算）。未武装 = 全 0 hex——不打非数字占位
    ///   （词形基线脚本按 token 解析）；
    /// - `expires_in_ms` = saturating 减（不得负数/回绕）；
    /// - `uses` = 剩余/总额度。
    fn degrade_observe(&mut self, resp: &mut Vec<u8>, now_ms: u64) {
        let mut obs = self.worker.backend_mut().degrade_observation(now_ms);
        obs.deadband_applies = self.deadband_applies;
        obs.deadband_frozen_skips = self.deadband_frozen_skips;
        eneros_runtime::println!("{}", degrade_obs_line(&obs, now_ms));
        resp.clear();
        resp.push(learn_wire::AI_DEGRADE_OBSERVE);
        resp.push(u8::from(obs.armed));
        resp.push(u8::from(obs.expired));
        resp.push(obs.level);
        resp.extend_from_slice(&obs.lease_id);
        resp.extend_from_slice(&obs.issued_at_ms.to_le_bytes());
        resp.extend_from_slice(&obs.expires_at_ms.to_le_bytes());
        resp.extend_from_slice(&obs.uses_max.to_le_bytes());
        resp.extend_from_slice(&obs.uses_left.to_le_bytes());
        resp.extend_from_slice(&obs.charge_soc.to_le_bytes());
        resp.extend_from_slice(&obs.discharge_soc.to_le_bytes());
        resp.extend_from_slice(&obs.deadband_applies.to_le_bytes());
        resp.extend_from_slice(&obs.deadband_frozen_skips.to_le_bytes());
        debug_assert_eq!(resp.len(), learn_wire::DEGRADE_OBS_WIRE_LEN);
    }
}

impl<B, Io, A> ControlServiceHandler for AiTickHandler<'_, B, Io, A>
where
    B: DecisionBackend + DeadbandTarget + DegradeArmQuery + DegradeObserveQuery,
    Io: PersistIo,
    A: TickAudit<B>,
{
    fn handle(&mut self, req: &[u8], resp: &mut Vec<u8>, now_ms: u64) -> bool {
        match req.first() {
            Some(&AI_PROPOSE) => {
                // 受信 SOC 提取（trust 链解码失败 ⇒ None=不观测）。
                let soc = req
                    .get(1..)
                    .and_then(crate::telemetry::decode_trusted)
                    .map(|t| t.state.soc_pct);
                self.apply_learn_thresholds();
                *resp = self.worker.propose(req, now_ms);
                if resp.first() == Some(&AI_OK) {
                    self.audit.on_proposal_ok(self.worker.backend_mut(), now_ms);
                }
                self.learn_observe(soc);
                true
            }
            Some(&learn_wire::AI_LEARN_STATUS) => {
                self.learn_status_wire(resp);
                true
            }
            Some(&learn_wire::AI_LEARN_RESET) => {
                self.learn_reset(resp);
                true
            }
            // 69.5（WP-69.5.1）：只读观测——只产生响应，不改任何状态
            // （默认臂 0xFF 兜底行为不变，TickAudit::on_other doc 承诺）。
            Some(&learn_wire::AI_DEGRADE_OBSERVE) => {
                self.degrade_observe(resp, now_ms);
                true
            }
            _ => {
                // AI_PEER_MSG（remote）等扩展码由留痕钩子消费；未消费 → 0xFF。
                let handled = !req.is_empty()
                    && self
                        .audit
                        .on_other(self.worker.backend_mut(), req, resp, now_ms);
                if !handled {
                    *resp = alloc::vec![0xFF];
                }
                true
            }
        }
    }
}

// ============================================================================
// target 装配（svc-ai-worker bin 下沉——bin 只剩能力探测与判定行）
// ============================================================================

#[cfg(all(target_os = "none", feature = "sel4"))]
pub mod target {
    use eneros_agent_sdk::block_io::{eipc_block_io, BlockIo};
    use eneros_agent_sdk::config_client;
    use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
    use eneros_config_bundle::ai_peer::AiPeerConfig;
    use eneros_runtime::println;

    use super::*;
    use crate::remote_brain::RemoteBrainConfig;

    /// 学习器装配（target）：storage 授予 + 配置闸双条件；任一不满足 ⇒
    /// None（行为出厂值，串口如实登记——fail-closed 诚实降级）。
    pub type TargetLearner = DeadbandLearner<BlockIo<Sel4IpcClient>>;

    /// remote 路径 worker 型别（PeerLink 实现在 remote_brain 单元）。
    pub type RemoteWorker =
        AiWorkerService<RemoteBrain<HeuristicBackend, crate::remote_brain::PeerLinkEipc>>;

    /// 学习器装配（词形逐字节保留自旧 bin `build_learner`）。
    pub fn build_learner(eipc_mask: u64, learn_enabled: bool) -> Option<TargetLearner> {
        // SPEC-B：掩码位判定收编 eipc_granted_slot（WP-54.4 按名解析之上）。
        let slot_of = |target: &'static str| -> Option<u64> {
            eneros_agent_sdk::eipc_granted_slot("svc-ai-worker", eipc_mask, target)
        };
        if !learn_enabled {
            println!("[svc-ai-worker] LEARN disabled（ai-peer learn_enabled=false）—— 阈值恒出厂");
            return None;
        }
        let Some(slot) = slot_of("svc-storage") else {
            println!("[svc-ai-worker] LEARN-SNAPSHOT-UNAVAILABLE storage NOT granted —— 学习不可用（出厂行为）");
            return None;
        };
        let Some(io) = eipc_block_io(slot, 0, 5_000) else {
            println!("[svc-ai-worker] LEARN-SNAPSHOT-UNAVAILABLE storage bind FAILED —— 学习不可用（出厂行为）");
            return None;
        };
        match TargetLearner::open(true, io) {
            Ok(l) => {
                let (charge, discharge) = l.thresholds();
                let (min, max) = l.bounds();
                let snap = l.snapshot_view();
                println!(
                    "[svc-ai-worker] LEARN-STATE restored=1 current={:.6} min={min:.6} max={max:.6} samples={} rejected={}",
                    snap.current, snap.samples, snap.rejected_updates
                );
                println!(
                    "[svc-ai-worker] LEARN-THRESHOLDS charge={charge:.6} discharge={discharge:.6}"
                );
                Some(l)
            }
            Err(e) => {
                println!(
                    "[svc-ai-worker] LEARN-SNAPSHOT-CORRUPT {e:?} —— 学习不可用（出厂行为，fail-closed 不静默重建）"
                );
                None
            }
        }
    }

    /// ai-peer 配置拉取（svc-config CFG1；全必配 fail-closed；词形保留）。
    /// 蓝图 67 WP-67.2.3：传输面收 `ChunkedEndpoint`（epoch 单点纪律）。
    pub fn fetch_ai_peer_config(
        client: &mut eneros_agent_sdk::endpoint::ChunkedEndpoint<Sel4IpcClient>,
    ) -> Result<AiPeerConfig, &'static str> {
        let payload = config_client::get_entry(client, "ai-peer")
            .map_err(|_| "config-eipc-transfer-failed")?;
        if payload.starts_with(b"ERR:") {
            // svc-config 结构化拒绝——透传原因串（诊断面）。
            let reason = core::str::from_utf8(&payload[..payload.len().min(40)]).unwrap_or("ERR:");
            println!("[svc-ai-worker] AI-PEER-CFG server {reason}");
            return Err("config-server-error");
        }
        // S1 WP-S1.2：前缀剥离单点 = config_gateway::response_body（ERR:
        // 结构化拒绝需先检视原始载荷，故不走 config_get 管道）。
        let body = eneros_agent_sdk::config_gateway::response_body(&payload)
            .map_err(|_| "config-response-too-short")?;
        AiPeerConfig::decode(body).map_err(|_| "config-decode-failed")
    }

    /// remote dual-brain 主路径装配进服务循环（词形保留自旧 bin）。
    pub fn run_remote(
        ep: u64,
        ready_ntfn: u64,
        heartbeat_slot: u64,
        cfg: RemoteBrainConfig,
        mqtt: Sel4IpcClient,
        learn: Option<TargetLearner>,
        catalog_version: u64,
    ) -> ! {
        // 决策审计（ADR-0042 边界：dual-brain/audit 为决策语义门面）。
        // 蓝图 54 WP-54.1：同源魔数 [0x47;32] → 唯一入口按本服务域派生。
        // 蓝图 70.1 WP-70.1.2：决策审计字节预算显式声明（BoundedMemStore
        // 构造期预分配，扩容峰值 = 0，满载丢新计数——69.5 窗 OOM 根因消除）。
        let mut audit = DecisionAudit::with_budget(
            eneros_audit::genesis(b"svc-ai-worker-decision"),
            eneros_dual_brain::audit::DECISION_AUDIT_BUDGET_BYTES,
        );
        let link = crate::remote_brain::PeerLinkEipc {
            mqtt,
            correlation: 0,
        };
        let brain = RemoteBrain::with_catalog_ver(
            HeuristicBackend::new().with_catalog_version(catalog_version),
            link,
            cfg,
            catalog_version,
        );
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let mut handler = AiTickHandler::new(&mut worker, RemoteAudit { audit: &mut audit }, learn);
        crate::control_service_main(
            ep,
            "ai-worker",
            ready_ntfn,
            eneros_drv_svc::now_ms,
            eneros_drv_svc::instance_epoch(),
            heartbeat_slot,
            &mut handler,
        )
    }

    /// 纯启发式兜底装配（config/mqtt 未授予；词形保留自旧 bin）。
    pub fn run_heuristic_only(
        ep: u64,
        ready_ntfn: u64,
        heartbeat_slot: u64,
        learn: Option<TargetLearner>,
        catalog_version: u64,
    ) -> ! {
        // 蓝图 70.1 WP-70.1.2：决策审计字节预算显式声明（BoundedMemStore
        // 构造期预分配，扩容峰值 = 0，满载丢新计数——69.5 窗 OOM 根因消除）。
        let mut audit = DecisionAudit::with_budget(
            eneros_audit::genesis(b"svc-ai-worker-decision"),
            eneros_dual_brain::audit::DECISION_AUDIT_BUDGET_BYTES,
        );
        let mut worker = AiWorkerService::new(
            HeuristicBackend::new().with_catalog_version(catalog_version),
            [0x41; 16],
        );
        let mut handler =
            AiTickHandler::new(&mut worker, HeuristicAudit { audit: &mut audit }, learn);
        crate::control_service_main(
            ep,
            "ai-worker",
            ready_ntfn,
            eneros_drv_svc::now_ms,
            eneros_drv_svc::instance_epoch(),
            heartbeat_slot,
            &mut handler,
        )
    }
}

// ============================================================================
// host 测试（tick 时序不变式 + wire 单源 + 注入路径等价性锁定）
// ============================================================================

#[cfg(test)]
mod tests {
    use core::cell::RefCell;

    use eneros_durable_state::StoreError;

    use super::*;
    use crate::ai_worker::tests_helpers::minimal_trusted;
    use crate::ai_worker::AI_NOACTION;
    use crate::remote_brain::{LinkError, RemoteBrainConfig};
    use crate::telemetry::encode_trusted;

    /// 共享内存持久介质（测试版 PersistIo；偏移 = svc-storage 整盘视图，
    /// 以 ota-ab `DataRegion::LearnState.base_byte()` 平移）。
    struct MemIo {
        media: RefCell<Vec<u8>>,
    }

    const MEDIA_TAIL: u64 = 4096;

    impl MemIo {
        fn new() -> Self {
            let base = learn_base();
            Self {
                media: RefCell::new(vec![0u8; (base + MEDIA_TAIL) as usize]),
            }
        }

        fn window(&self, offset: u64, len: usize) -> Result<usize, StoreError> {
            offset
                .checked_sub(learn_base())
                .filter(|off| off + len as u64 <= self.media.borrow().len() as u64)
                .map(|off| off as usize)
                .ok_or(StoreError::Unavailable)
        }
    }

    fn learn_base() -> u64 {
        eneros_ota_ab::ab_partition::DataRegion::LearnState.base_byte()
    }

    impl PersistIo for MemIo {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            let start = self.window(offset, buf.len())?;
            let m = self.media.borrow();
            buf.copy_from_slice(&m[start..start + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let start = self.window(offset, data.len())?;
            let mut m = self.media.borrow_mut();
            m[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.media.borrow().len() as u64
        }
    }

    /// AI_PROPOSE 请求构造（指定 SOC）。
    fn propose_req(soc: f64) -> Vec<u8> {
        let mut t = minimal_trusted();
        t.state.soc_pct = soc;
        let mut req = alloc::vec![AI_PROPOSE];
        req.extend_from_slice(&encode_trusted(&t));
        req
    }

    /// 时序不变式（WP-B.3 红线 2 的化身）：tick N 观测 → tick N decide
    /// 不可见 → tick N+1 decide 必须可见。
    ///
    /// 序列（出厂带 [0.3, 0.5]；锚定 0.425 后 0.52 帧 dev=0.095 ⇒
    /// EMA=0.0457 ⇒ 3σ=0.137 界内生效 ⇒ 带 [0.263, 0.537]）：
    /// - tick1 soc=0.425：出厂死区 NoAction；observe 锚定（阈值不变）；
    /// - tick2 soc=0.52：decide 用**出厂**带 → 放电（若 observe 先于
    ///   decide 则此刻即死区——顺序反证）；observe 后带变 [0.263, 0.537]；
    /// - tick3 soc=0.52：decide 用**学习后**带 → NoAction。
    #[test]
    fn tick_n_observe_tick_n_plus_1_uses_learned_thresholds() {
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let mut worker = AiWorkerService::new(HeuristicBackend::new(), [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            HeuristicAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();

        // tick1：出厂死区拒绝；首帧锚定。
        resp.clear();
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert_eq!(resp.first(), Some(&AI_NOACTION), "0.425 ∈ 出厂死区");
        // LEARN-STATUS：锚定不学习，current 仍出厂 0.1。
        resp.clear();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_STATUS], &mut resp, 1_001));
        assert_eq!(resp.len(), learn_wire::LEARN_STATUS_WIRE_LEN);
        let current = f64::from_le_bytes(resp[2..10].try_into().expect("50B"));
        assert!((current - 0.1).abs() < 1e-9, "锚定不改阈值: {current}");

        // tick2：decide 用出厂带（0.52 ≥ 0.5 放电）——observe 未提前生效。
        resp.clear();
        assert!(handler.handle(&propose_req(0.52), &mut resp, 2_000));
        assert_eq!(resp.first(), Some(&AI_OK), "出厂带外放电");
        // 蓝图 61：CAND wire（变长）——魔数自证。
        assert_eq!(&resp[1..5], b"CAND");
        // observe(0.52)：dev=0.095 → current=0.137 Applied（下一 tick 生效）。
        resp.clear();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_STATUS], &mut resp, 2_001));
        let current = f64::from_le_bytes(resp[2..10].try_into().expect("50B"));
        assert!((current - 0.137).abs() < 1e-9, "学习后 current: {current}");

        // tick3：decide 必须用 tick2 学到的带 [0.263, 0.537] → 0.52 死区。
        resp.clear();
        assert!(handler.handle(&propose_req(0.52), &mut resp, 3_000));
        assert_eq!(resp.first(), Some(&AI_NOACTION), "学习后 0.52 必须落入死区");
    }

    /// LEARN_STATUS wire：未装配（None）= enabled 0 + 全零，50B 定长
    /// （与旧 bin 两副本逐字节同布局）。
    #[test]
    fn learn_status_wire_unavailable_is_zero_padded_50b() {
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let mut worker = AiWorkerService::new(HeuristicBackend::new(), [0x41; 16]);
        let mut handler = AiTickHandler::new(
            &mut worker,
            HeuristicAudit { audit: &mut audit },
            None::<DeadbandLearner<MemIo>>,
        );
        let mut resp = Vec::new();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_STATUS], &mut resp, 1));
        assert_eq!(resp.len(), 50);
        assert_eq!(resp[0], learn_wire::AI_LEARN_STATUS);
        assert_eq!(resp[1], 0);
        assert!(resp[2..].iter().all(|&b| b == 0));
    }

    /// LEARN_RESET：回出厂 + wire [0x20, 0] + 阈值还原（0.52 重新放电影
    /// 响应边界复位）+ applies 归零。
    #[test]
    fn learn_reset_restores_factory_boundary() {
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let mut worker = AiWorkerService::new(HeuristicBackend::new(), [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            HeuristicAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();
        // 先把带学宽（复用时序测试序列）。
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 2_000));
        resp.clear();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_STATUS], &mut resp, 2_001));
        let current = f64::from_le_bytes(resp[2..10].try_into().expect("50B"));
        assert!((current - 0.137).abs() < 1e-9);

        // reset：wire 两字节 ok；current 回出厂；applies 归零。
        resp.clear();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_RESET], &mut resp, 3_000));
        assert_eq!(resp, alloc::vec![learn_wire::AI_LEARN_RESET, 0]);
        resp.clear();
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_STATUS], &mut resp, 3_001));
        let current = f64::from_le_bytes(resp[2..10].try_into().expect("50B"));
        assert!((current - 0.1).abs() < 1e-9, "reset 回出厂: {current}");
        let applies = u64::from_le_bytes(resp[42..50].try_into().expect("50B"));
        assert_eq!(applies, 0);
        // 边界复位：0.52 重新放电影响响应（出厂带外）。
        resp.clear();
        assert!(handler.handle(&propose_req(0.52), &mut resp, 4_000));
        assert_eq!(resp.first(), Some(&AI_OK));
    }

    /// 注入路径等价性锁定（红线 3 结论的可执行化）：直连 HeuristicBackend
    /// 与包装于 RemoteBrain 的 local 后端，同一 apply_deadband 后决策
    /// 行为逐判定一致。
    #[test]
    fn deadband_target_wrapped_and_direct_are_equivalent() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0u8; 32], // 68.5：未装配（不武装——既有行为锁）
            lease_ttl_secs: 0,
        };
        let mut direct = HeuristicBackend::new();
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);

        direct.apply_deadband(0.26, 0.54);
        worker.backend_mut().apply_deadband(0.26, 0.54);

        // 同 SOC 判定逐条一致：0.52 ∈ 加宽带 → 双双死区；0.6 ≥ 0.54 → 双双放电。
        let mut t = minimal_trusted();
        t.state.soc_pct = 0.52;
        assert!(direct.decide(&t, 1_000).is_err());
        assert!(worker.backend_mut().local_mut().decide(&t, 1_000).is_err());
        t.state.soc_pct = 0.6;
        assert!(direct.decide(&t, 1_001).is_ok());
        assert!(worker.backend_mut().local_mut().decide(&t, 1_001).is_ok());
    }

    /// heuristic 全 tick：AI_OK 提案入决策审计链（actor/target/intent 词形
    /// 与旧 bin 一致）。
    #[test]
    fn heuristic_full_tick_appends_audit_record() {
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let entries_before = audit.chain_state().entry_count;
        let mut worker = AiWorkerService::new(HeuristicBackend::new(), [0x41; 16]);
        let mut handler = AiTickHandler::new(
            &mut worker,
            HeuristicAudit { audit: &mut audit },
            None::<DeadbandLearner<MemIo>>,
        );
        let mut resp = Vec::new();
        assert!(handler.handle(&propose_req(0.6), &mut resp, 7_000));
        assert_eq!(resp.first(), Some(&AI_OK));
        assert_eq!(
            audit.chain_state().entry_count,
            entries_before + 1,
            "成功提案必须留痕"
        );
    }

    /// remote 扩展码钩子：AI_PEER_MSG 经 RemoteAudit 消费（Bad/Late 判定 +
    /// 计数 + wire 回执）；未识别码仍 0xFF 兜底。
    #[test]
    fn remote_peer_msg_hook_and_unknown_fallback() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0u8; 32], // 68.5：未装配（不武装——既有行为锁）
            lease_ttl_secs: 0,
        };
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            None::<DeadbandLearner<MemIo>>,
        );
        let mut resp = Vec::new();
        // 坏 UTF-8 推送 → Bad 判定 + 计数上行 + 回执 [0x10, 0]。
        let bad = [AI_PEER_MSG, 0xFF, 0xFE];
        assert!(handler.handle(&bad, &mut resp, 1_000));
        assert_eq!(resp, alloc::vec![AI_PEER_MSG, 0]);
        assert_eq!(handler.worker.backend_mut().bad_responses_total, 1);
        // 无在途请求的合法文本推送 → Late + late_total 上行。
        resp.clear();
        assert!(handler.handle(&[AI_PEER_MSG, b'{', b'}'], &mut resp, 1_001));
        assert_eq!(resp, alloc::vec![AI_PEER_MSG, 0]);
        assert_eq!(handler.worker.backend_mut().late_total, 1);
        // 未识别码 → 0xFF 兜底（on_other 默认不消费）。
        resp.clear();
        assert!(handler.handle(&[0x77], &mut resp, 1_002));
        assert_eq!(resp, alloc::vec![0xFF]);
    }

    /// 68.5 WP-68.5.3 t6（次生面处置锁）：降级武装期学习阈值注入冻结
    /// （学习继续改死区 ⇒ strategy digest 漂移 ⇒ lease 白名单失配 ⇒
    /// System1 断供）；peer 恢复消费后自动解冻。变异靶：删
    /// apply_learn_thresholds 的武装短路 → 本测红。
    #[test]
    fn t6_learn_thresholds_frozen_while_degrade_armed() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();

        // tick1（soc 0.425）：发请求 + 出厂带死区 NoAction；观测锚定。
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert_eq!(resp.first(), Some(&AI_NOACTION));
        // tick2（soc 0.52，now 超时窗）：超时裁决 → LEASE 武装；0.52 出厂
        // 带外放电；观测 → 学到 current=0.137。
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        assert_eq!(resp.first(), Some(&AI_OK));
        // tick3（soc 0.52）：武装期注入冻结 → 工厂带 [0.3, 0.5] 生效 →
        // 0.52 放电（若未冻结则学到的带 [0.263, 0.537] 生效 → NoAction=红）。
        assert!(handler.handle(&propose_req(0.52), &mut resp, 7_000));
        assert_eq!(resp.first(), Some(&AI_OK), "武装期阈值冻结在武装时刻形态");

        // peer 应答到达（AI_PEER_MSG 推送；配对超时后重发的 req_id=2）
        // → 校验接受；下一 tick 消费。
        let mut peer = alloc::vec![AI_PEER_MSG];
        peer.extend_from_slice(
            r#"{"v":1,"req_id":2,"seq":5,"plan":{"power_kw":-3.0,"target_device":"pcs","ttl_secs":60},"reason":"soc"}"#.as_bytes(),
        );
        assert!(handler.handle(&peer, &mut resp, 7_100));
        // tick4（soc 0.60）：消费远程提案（System2 路径）→ LEASE-REVOKED
        //（撤销在此 tick decide 内发生——apply 时仍武装）；观测 0.60 →
        // 学到 current=0.118（带 [0.282, 0.518]）。
        assert!(handler.handle(&propose_req(0.60), &mut resp, 8_000));
        assert_eq!(resp.first(), Some(&AI_OK), "远程提案消费（System2 路径）");
        // tick 解冻（soc 0.52）：学到的带 [0.282, 0.518] 生效 → 死区
        // NoAction（若仍冻结则工厂带 → 0.52 放电 = 红）。
        assert!(handler.handle(&propose_req(0.52), &mut resp, 9_000));
        assert_eq!(resp.first(), Some(&AI_NOACTION), "解冻后学到的带重新生效");
    }

    /// 68.5 复查修复 t7（红测先行）：武装期 LEARN_RESET 须被拒——回厂
    /// 改变死区 ⇒ strategy digest 漂移 ⇒ lease 白名单失配 ⇒ System1
    /// 断供（与 apply_learn_thresholds 冻结同判据；附加裁决 8 防线的
    /// 注入点补口）。变异靶：learn_reset 去掉武装判据 → 本测红
    /// （reset 得逞 status=0 + 下一 tick 兜底 Unavailable）。
    #[test]
    fn t7_learn_reset_rejected_while_degrade_armed() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();

        // tick1/tick2：超时裁决 → 武装（t6 同款时序；学习照常观测）。
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        assert_eq!(resp.first(), Some(&AI_OK));
        resp.clear();
        // 武装期 LEARN_RESET：拒绝（status=1），学习态与死区均保持。
        assert!(handler.handle(&alloc::vec![learn_wire::AI_LEARN_RESET], &mut resp, 6_100));
        assert_eq!(
            resp,
            alloc::vec![learn_wire::AI_LEARN_RESET, 1],
            "武装期复位须被拒（fail-closed）"
        );
        resp.clear();
        // 齿：复位未得逞 ⇒ digest 未漂移 ⇒ 下一 tick 兜底仍持签产出
        //（若复位得逞：白名单失配 → Unavailable）。
        assert!(handler.handle(&propose_req(0.52), &mut resp, 7_000));
        assert_eq!(resp.first(), Some(&AI_OK), "武装期复位被拒后兜底须照常产出");
    }

    // ========================================================================
    // 蓝图 69 卷 69.5（WP-69.5.1）：只读观测窗 t30-t35
    // ========================================================================

    /// DEGRADE-OBSERVE 68B 定长响应解码（测试面；字节布局见
    /// `learn_wire::DEGRADE_OBS_WIRE_LEN` 注记）。
    struct ObsWire {
        armed: bool,
        expired: bool,
        level: u8,
        lease_id: [u8; 16],
        issued_at_ms: u64,
        expires_at_ms: u64,
        uses_max: u32,
        uses_left: u32,
        charge_soc: f64,
        discharge_soc: f64,
        deadband_applies: u32,
        deadband_frozen_skips: u32,
    }

    fn decode_obs(resp: &[u8]) -> ObsWire {
        #![allow(clippy::disallowed_macros)]
        assert_eq!(resp.len(), learn_wire::DEGRADE_OBS_WIRE_LEN, "{resp:?}");
        assert_eq!(resp[0], learn_wire::AI_DEGRADE_OBSERVE);
        let arr = <&[u8; 16]>::try_from(&resp[4..20]).expect("lease_id");
        let f64_at = |off: usize| f64::from_le_bytes(resp[off..off + 8].try_into().expect("f64"));
        ObsWire {
            armed: resp[1] != 0,
            expired: resp[2] != 0,
            level: resp[3],
            lease_id: *arr,
            issued_at_ms: u64::from_le_bytes(resp[20..28].try_into().expect("u64")),
            expires_at_ms: u64::from_le_bytes(resp[28..36].try_into().expect("u64")),
            uses_max: u32::from_le_bytes(resp[36..40].try_into().expect("u32")),
            uses_left: u32::from_le_bytes(resp[40..44].try_into().expect("u32")),
            charge_soc: f64_at(44),
            discharge_soc: f64_at(52),
            deadband_applies: u32::from_le_bytes(resp[60..64].try_into().expect("u32")),
            deadband_frozen_skips: u32::from_le_bytes(resp[64..68].try_into().expect("u32")),
        }
    }

    /// t30：武装后读快照——lease 字段与签发 lease 逐字段相等（WP-69.5.1
    /// 步骤 5；M2 四行确认表的 host 层化身）。变异靶：字段错配/残值/
    /// saturating 换回绕减 → 本测红。
    #[test]
    fn t30_degrade_observe_reports_armed_lease_fields() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();
        // tick1（锚定）/ tick2（超时裁决 → 武装 + 兜底授权记账 1 次）。
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        assert_eq!(resp.first(), Some(&AI_OK));
        resp.clear();
        // 观测（6_100 < expires 3_606_000：未过期）。
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut resp, 6_100));
        let o = decode_obs(&resp);
        assert!(o.armed);
        assert!(!o.expired);
        assert_eq!(
            o.lease_id,
            6_000u128.to_le_bytes(),
            "lease_id=武装时刻 u128 LE"
        );
        assert_eq!(o.level, 3, "SolverFallback = D3 码值");
        assert_eq!(o.issued_at_ms, 6_000);
        assert_eq!(o.expires_at_ms, 6_000 + 3_600 * 1_000);
        assert_eq!(o.uses_max, crate::ai_worker::DEGRADE_LEASE_MAX_USES);
        assert_eq!(o.uses_left, o.uses_max - 1, "tick2 兜底授权记账 1 次");
        // 武装期注入冻结 ⇒ 后端死区保持出厂带（观测=实际生效值）。
        assert!((o.charge_soc - 0.3).abs() < 1e-9);
        assert!((o.discharge_soc - 0.5).abs() < 1e-9);
        // tick1/tick2 均在武装前注入成功 ⇒ applies=2；尚未有冻结。
        assert_eq!(o.deadband_applies, 2);
        assert_eq!(o.deadband_frozen_skips, 0);
    }

    /// t30b（WP-69.5.1 变异靶 1 的牙）：`expires_in_ms` 饱和语义——
    /// 已过期 lease 的观测行必须打 `expires_in_ms=0`（saturating 减），
    /// 不得回绕成巨大值（词形数字供基线脚本解析，回绕即假证）。
    #[test]
    fn t30b_expires_in_ms_saturates_not_wraps() {
        let obs = DegradeObservation {
            armed: true,
            expired: true,
            lease_id: [1; 16],
            level: 3,
            issued_at_ms: 1_000,
            expires_at_ms: 2_000,
            uses_max: 5,
            uses_left: 4,
            charge_soc: 0.3,
            discharge_soc: 0.5,
            deadband_applies: 1,
            deadband_frozen_skips: 0,
        };
        let line = degrade_obs_line(&obs, 9_000);
        assert!(line.contains("expired=true"), "{line}");
        assert!(line.contains("expires_in_ms=0"), "过期须饱和为 0: {line}");
        assert!(
            !line.contains("expires_in_ms=18"),
            "不得出现无饱和回绕的巨大值: {line}"
        );
        // 未武装（全 0 快照）：expires_in_ms=0、uses=0/0、全零 lease hex
        // ——词形基线的未武装形态锚（不打 N/A 类非数字占位）。
        let unarmed = DegradeObservation {
            armed: false,
            expired: false,
            lease_id: [0; 16],
            level: 0,
            issued_at_ms: 0,
            expires_at_ms: 0,
            uses_max: 0,
            uses_left: 0,
            charge_soc: 0.3,
            discharge_soc: 0.5,
            deadband_applies: 0,
            deadband_frozen_skips: 0,
        };
        let line = degrade_obs_line(&unarmed, 1_000);
        assert!(line.contains("lease=0000000000000000"), "{line}");
        assert!(line.contains("uses=0/0"), "{line}");
        assert!(line.contains("level=0"), "{line}");
        assert!(line.contains("armed=false"), "{line}");
    }

    /// t31：观测是只读的——同刻连调两次，两次响应逐字节相同（含计数器
    /// 与 armed；本卷防次生 BUG 的第一牙——观测面自己改状态是最易犯的
    /// 错）。变异靶 2（观测路径顺手 +=1）→ 本测红。
    #[test]
    fn t31_degrade_observe_is_read_only() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        let mut r1 = Vec::new();
        let mut r2 = Vec::new();
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut r1, 6_100));
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut r2, 6_100));
        assert_eq!(r1, r2, "观测两次必须逐字节相同（只读性）");
        let o1 = decode_obs(&r1);
        let o2 = decode_obs(&r2);
        assert_eq!(o1.deadband_applies, o2.deadband_applies);
        assert_eq!(o1.deadband_frozen_skips, o2.deadband_frozen_skips);
        assert_eq!(o1.armed, o2.armed);
        assert_eq!(o1.uses_left, o2.uses_left, "观测不得消耗 lease 额度");
    }

    /// t32：非武装期注入计数——N 次 tick（learn 装配、未武装）⇒
    /// `deadband_applies == N`。
    #[test]
    fn t32_deadband_apply_count_increments() {
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let mut worker = AiWorkerService::new(HeuristicBackend::new(), [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            HeuristicAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();
        for (i, now) in [1_000u64, 2_000, 3_000].into_iter().enumerate() {
            assert!(handler.handle(&propose_req(0.6), &mut resp, now));
            resp.clear();
            assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut resp, now + 1));
            let o = decode_obs(&resp);
            assert_eq!(o.deadband_applies, (i + 1) as u32);
            assert_eq!(o.deadband_frozen_skips, 0);
            assert!(!o.armed);
            resp.clear();
        }
    }

    /// t33（本卷最重要的牙）：武装期 注入早退 + 复位拒绝 各计 1 次 ⇒
    /// `deadband_frozen_skips == 2`，且复位**仍返回 AI_LEARN_RESET,1 拒绝
    /// 回执**（证明只加观测、没改判定）。变异靶 3（删武装拒绝分支）/
    /// 变异靶 4（合并两处 if 丢回执）→ 本测红。
    #[test]
    fn t33_deadband_frozen_skip_count_increments() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let learner = DeadbandLearner::open(true, MemIo::new()).expect("learner open");
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            Some(learner),
        );
        let mut resp = Vec::new();
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        // 武装期注入（tick3）：早退 +1。
        assert!(handler.handle(&propose_req(0.52), &mut resp, 7_000));
        // 武装期复位：拒绝 +1，回执保持 [0x20, 1]（判定未变）。
        resp.clear();
        assert!(handler.handle(&[learn_wire::AI_LEARN_RESET], &mut resp, 7_100));
        assert_eq!(
            resp,
            alloc::vec![learn_wire::AI_LEARN_RESET, 1],
            "武装期复位仍须拒绝（观测加了自己，没动守卫）"
        );
        // 观测：frozen=2；applies 停在 2（武装后的 tick3 不再注入）。
        resp.clear();
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut resp, 7_200));
        let o = decode_obs(&resp);
        assert_eq!(o.deadband_frozen_skips, 2);
        assert_eq!(o.deadband_applies, 2);
        assert!(o.armed);
    }

    /// t35：未武装观测全 0——不是上一次武装的残值（arm → peer 消费撤销
    /// 后再观测，lease 字段必须全 0）。
    #[test]
    fn t35_unarmed_observation_is_all_zero_not_garbage() {
        struct NullLink;
        impl PeerLink for NullLink {
            fn send_request(&mut self, _req_json: &str) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let cfg = RemoteBrainConfig {
            timeout_ms: 2_000,
            unhealthy_after: 3,
            probe_every: 1,
            max_power_kw: 10.0,
            ttl_secs: 60,
            lease_key: [0x5Bu8; 32],
            lease_ttl_secs: 3_600,
        };
        let mut audit = DecisionAudit::new(eneros_audit::genesis(b"ai-handler-test"));
        let brain = RemoteBrain::new(HeuristicBackend::new(), NullLink, cfg);
        let mut worker = AiWorkerService::new(brain, [0x41; 16]);
        let mut handler = AiTickHandler::new(
            &mut worker,
            RemoteAudit { audit: &mut audit },
            None::<DeadbandLearner<MemIo>>,
        );
        let mut resp = Vec::new();
        // 全新 handler：未武装全 0。
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut resp, 100));
        let o = decode_obs(&resp);
        assert!(!o.armed && !o.expired);
        assert_eq!(o.level, 0);
        assert_eq!(o.lease_id, [0; 16]);
        assert_eq!(o.issued_at_ms, 0);
        assert_eq!(o.expires_at_ms, 0);
        assert_eq!(o.uses_max, 0);
        assert_eq!(o.uses_left, 0);
        // 武装（t6 时序）→ peer 应答消费 → 撤销 → 观测不得见残值。
        assert!(handler.handle(&propose_req(0.425), &mut resp, 1_000));
        assert!(handler.handle(&propose_req(0.52), &mut resp, 6_000));
        let mut peer = alloc::vec![AI_PEER_MSG];
        peer.extend_from_slice(
            r#"{"v":1,"req_id":2,"seq":5,"plan":{"power_kw":-3.0,"target_device":"pcs","ttl_secs":60},"reason":"soc"}"#
                .as_bytes(),
        );
        assert!(handler.handle(&peer, &mut resp, 7_100));
        assert!(handler.handle(&propose_req(0.60), &mut resp, 8_000));
        resp.clear();
        assert!(handler.handle(&[learn_wire::AI_DEGRADE_OBSERVE], &mut resp, 8_100));
        let o = decode_obs(&resp);
        assert!(!o.armed, "peer 消费后已撤销");
        assert_eq!(o.lease_id, [0; 16], "不得残留上一次武装的 lease_id");
        assert_eq!(o.level, 0);
        assert_eq!(o.issued_at_ms, 0);
        assert_eq!(o.expires_at_ms, 0);
        assert_eq!(o.uses_max, 0);
        assert_eq!(o.uses_left, 0);
    }
}
