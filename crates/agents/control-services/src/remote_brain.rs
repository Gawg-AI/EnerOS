//! 蓝图 49 WP-49.2：远程 dual-brain **目标侧子集**（ADR-0045 方案 B）。
//!
//! 职责（蓝图 WP-49.2 #4）：路径选择 / 意图校验委托契约层 / 超时降级 /
//! System1（HeuristicBackend）↔ System2（远程 peer）可用性切换。切换事件
//! 经 [`RemoteBrain::drain_events`] 交给装配层入 DecisionAudit（ADR-0042
//! 角色边界不变：dispatch 仍是唯一投递与最终授权点）。
//!
//! 信任模型：peer 应答 = 不受信输入。本模块不自行信任任何应答——全部
//! 经 `intent-contract::peer` 全量校验（schema 闭集/版本枚举/req_id 配对/
//! sequence 单调/数值域）后才可被消费；坏应答计数丢弃，迟响应单独计数，
//! 均不得进入决策。
//!
//! 宣称纪律（ADR-0045 #4）：本链路语义 = 「AI 辅助决策（需配套站端服务）」。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use eneros_dual_brain::decision::ProposedControlPlan;
use eneros_dual_brain::DecisionPath;
use eneros_intent_contract::peer::{
    budget as peer_budget, parse_and_validate, truncate_for_audit, PeerBudget, PeerRequest,
    PeerTelemetry, ResponseChecks,
};
use eneros_safety_validator::TrustedTelemetry;

use crate::ai_worker::{encode_proposal, DecisionBackend, DecisionReject};

/// 切换/状态迁移事件（审计与串口证据面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchEvent {
    /// 连续超时达阈值——peer 标记 Unhealthy（System1 接管）。
    PeerUnhealthy { consecutive_timeouts: u32 },
    /// 有效应答重新到达——恢复 Healthy（回 AI）。
    PeerRecovered,
}

/// 决策元数据（审计留痕用；每次 decide 后可读）。
#[derive(Debug, Clone)]
pub struct DecisionMeta {
    /// 本次是否采信远程提案（false = System1 启发式/降级）。
    pub used_remote: bool,
    pub path: DecisionPath,
    /// 关联请求 id（远程路径时有值）。
    pub req_id: Option<u64>,
    /// prompt 摘要（≤48 字符，截断策略 ADR-0045 #3）。
    pub prompt_summary: String,
    /// 应答摘要（≤64 字符，截断策略同上）。
    pub resp_summary: String,
}

/// peer 触发链路 seam（svc bin 以 EIPC drv_call 实现；host 测试注入 mock）。
pub trait PeerLink {
    /// 把请求 JSON 交给传输面发布到 ai_request_topic（QoS1）。
    fn send_request(&mut self, req_json: &str) -> Result<(), LinkError>;
}

/// 链路错误（传输失败 ≠ 协议拒绝；只计数，不改健康判定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError(pub String);

/// 状态机配置（来源 configs/ai-peer.toml typed 消费；缺省禁凭记忆——
/// bin 装配必须显式提供）。
#[derive(Debug, Clone, Copy)]
pub struct RemoteBrainConfig {
    /// 在途请求超时（毫秒；产品缺省 5000）。
    pub timeout_ms: u64,
    /// 连续超时多少次后标记 Unhealthy（产品缺省 3）。
    pub unhealthy_after: u32,
    /// Unhealthy 状态下每 N tick 探测一次恢复（产品缺省 5）。
    pub probe_every: u32,
    /// 功率幅值预算 kW（写入请求 + 应答校验上界）。
    pub max_power_kw: f64,
    /// 提案 TTL 上限秒。
    pub ttl_secs: u32,
}

/// 在途请求。
struct PendingReq {
    req_id: u64,
    issued_ms: u64,
}

/// 已接受待消费的有效提案（已过全量校验 + 与在途请求配对）。
struct AcceptedPlan {
    req_id: u64,
    power_kw: f64,
    ttl_secs: u32,
    target_device: String,
    reason: String,
    prompt_summary: String,
}

/// on_peer_message 判定（bin 据此打串口证据行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMsgVerdict {
    /// 校验通过且与在途请求配对——等待下次 decide 消费。
    Accepted,
    /// 迟到（在途已过期/无配对）——不进决策，不计坏应答。
    Late,
    /// 协议违规/越权（schema/域校验失败、重放）——丢弃并计坏应答。
    Bad,
}

/// 远程 dual-brain 目标侧子集。实现 [`DecisionBackend`] 可直接装入
/// [`crate::ai_worker::AiWorkerService`]。
pub struct RemoteBrain<L: DecisionBackend, P: PeerLink> {
    local: L,
    link: P,
    cfg: RemoteBrainConfig,
    healthy: bool,
    next_req_id: u64,
    pending: Option<PendingReq>,
    /// 在途请求的 prompt 摘要（请求发出时暂存，应答接受时随行入审计）。
    pending_prompt_summary: Option<String>,
    last_seq: Option<u64>,
    accepted: Option<AcceptedPlan>,
    consecutive_timeouts: u32,
    ticks: u64,
    // 计数（遥测可见面——串口状态行 + 审计摘要）。
    pub timeouts_total: u64,
    pub bad_responses_total: u64,
    pub late_total: u64,
    pub remote_used_total: u64,
    pub triggers_sent: u64,
    pub link_errors: u64,
    events: Vec<SwitchEvent>,
    last_meta: Option<DecisionMeta>,
    /// 最近一次拒绝的契约错误摘要（诊断面；bin 状态行透传）。
    pub last_reject_detail: Option<String>,
}

impl<L: DecisionBackend, P: PeerLink> RemoteBrain<L, P> {
    pub fn new(local: L, link: P, cfg: RemoteBrainConfig) -> Self {
        Self {
            local,
            link,
            cfg,
            healthy: true,
            next_req_id: 0,
            pending: None,
            pending_prompt_summary: None,
            last_seq: None,
            accepted: None,
            consecutive_timeouts: 0,
            ticks: 0,
            timeouts_total: 0,
            bad_responses_total: 0,
            late_total: 0,
            remote_used_total: 0,
            triggers_sent: 0,
            link_errors: 0,
            events: Vec::new(),
            last_meta: None,
            last_reject_detail: None,
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// SPEC-E WP-E.3：System1（本地后端）可变访问——学习闭环在 decide
    /// 前经此注入自适应死区阈值（下游消费点；不影响切换状态机语义）。
    pub fn local_mut(&mut self) -> &mut L {
        &mut self.local
    }

    /// 取上次 decide 的元数据（审计留痕）。
    pub fn last_meta(&self) -> Option<&DecisionMeta> {
        self.last_meta.as_ref()
    }

    /// 取走累积的切换事件（装配层入审计链 + 串口证据行）。
    pub fn drain_events(&mut self) -> Vec<SwitchEvent> {
        core::mem::take(&mut self.events)
    }

    /// 收到传输面推来的 peer 应答（不受信 JSON）→ 全量校验 + 配对。
    pub fn on_peer_message(&mut self, json: &str, now_ms: u64) -> PeerMsgVerdict {
        // 无在途请求 → 一切应答皆迟到（含主动推送的垃圾流量）。
        let Some(pending) = &self.pending else {
            self.late_total = self.late_total.saturating_add(1);
            return PeerMsgVerdict::Late;
        };
        let expect_req_id = pending.req_id;
        let issued_ms = pending.issued_ms;
        let checks = ResponseChecks {
            expect_req_id,
            expect_seq_gt: self.last_seq,
            budget: &self.peer_budget(),
        };
        match parse_and_validate(json, &checks) {
            Ok(resp) => {
                // 新鲜度：配对成功但已过超时窗 → 迟到（遥测快照陈旧不可用）。
                if now_ms.saturating_sub(issued_ms) > self.cfg.timeout_ms {
                    self.late_total = self.late_total.saturating_add(1);
                    return PeerMsgVerdict::Late;
                }
                self.last_seq = Some(resp.seq);
                self.accepted = Some(AcceptedPlan {
                    req_id: resp.req_id,
                    power_kw: resp.plan.power_kw,
                    ttl_secs: resp.plan.ttl_secs,
                    target_device: resp.plan.target_device.clone(),
                    reason: resp.reason.clone(),
                    prompt_summary: self.pending_prompt_summary.take().unwrap_or_default(),
                });
                self.pending = None;
                if !self.healthy {
                    self.healthy = true;
                    self.events.push(SwitchEvent::PeerRecovered);
                }
                PeerMsgVerdict::Accepted
            }
            Err(eneros_intent_contract::PeerContractError::Correlation { .. }) => {
                // 配对失败的典型场景 = 在途已被超时清除后的旧应答。
                self.late_total = self.late_total.saturating_add(1);
                PeerMsgVerdict::Late
            }
            Err(e) => {
                self.bad_responses_total = self.bad_responses_total.saturating_add(1);
                self.last_reject_detail = Some(alloc::format!("{e:?}"));
                PeerMsgVerdict::Bad
            }
        }
    }

    /// 决策主入口（带审计元数据；[`DecisionBackend::decide`] 的富化版）。
    pub fn decide_full(
        &mut self,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        self.ticks = self.ticks.saturating_add(1);

        // 1. 在途请求超时裁决（fail-operational：本次 tick 即刻可降级）。
        if let Some(p) = &self.pending {
            if now_ms.saturating_sub(p.issued_ms) > self.cfg.timeout_ms {
                self.pending = None;
                self.timeouts_total = self.timeouts_total.saturating_add(1);
                self.consecutive_timeouts = self.consecutive_timeouts.saturating_add(1);
                if self.healthy && self.consecutive_timeouts >= self.cfg.unhealthy_after {
                    self.healthy = false;
                    self.events.push(SwitchEvent::PeerUnhealthy {
                        consecutive_timeouts: self.consecutive_timeouts,
                    });
                }
            }
        }

        // 2. 消费已接受的有效远程提案（System2 路径）。
        if let Some(a) = self.accepted.take() {
            self.consecutive_timeouts = 0;
            self.remote_used_total = self.remote_used_total.saturating_add(1);
            let resp_summary = truncate_for_audit(
                &alloc::format!(
                    "power={}kW ttl={}s dev={} why={}",
                    a.power_kw,
                    a.ttl_secs,
                    a.target_device,
                    a.reason
                ),
                64,
            );
            let plan = Self::build_plan(&a, telemetry, now_ms);
            let meta = DecisionMeta {
                used_remote: true,
                path: DecisionPath::Slow,
                req_id: Some(a.req_id),
                prompt_summary: a.prompt_summary,
                resp_summary,
            };
            self.last_meta = Some(meta);
            return Ok(encode_proposal(&plan).to_vec());
        }

        // 3. 需要新请求？Healthy 常态发起；Unhealthy 仅周期探测。
        let probe_due = !self.healthy && self.ticks % u64::from(self.cfg.probe_every.max(1)) == 0;
        if self.pending.is_none() && (self.healthy || probe_due) {
            self.next_req_id = self.next_req_id.saturating_add(1);
            let req_id = self.next_req_id;
            let req = PeerRequest::new(
                req_id,
                now_ms,
                PeerTelemetry {
                    soc_pct: telemetry.state.soc_pct,
                    voltage_v: telemetry.state.voltage_v,
                    current_a: telemetry.state.current_a,
                    frequency_hz: telemetry.state.frequency_hz,
                    generated_at_ms: telemetry.generated_at_ms,
                    digest_hex: hex_encode(&telemetry.provenance_digest),
                },
                self.peer_budget(),
            );
            match req.encode() {
                Ok(json) => match self.link.send_request(&json) {
                    Ok(()) => {
                        self.triggers_sent = self.triggers_sent.saturating_add(1);
                        self.pending_prompt_summary =
                            Some(truncate_for_audit(&prompt_summary(req_id, telemetry), 48));
                        self.pending = Some(PendingReq {
                            req_id,
                            issued_ms: now_ms,
                        });
                    }
                    Err(_) => {
                        self.link_errors = self.link_errors.saturating_add(1);
                        self.next_req_id = self.next_req_id.saturating_sub(1);
                    }
                },
                Err(_) => {
                    self.link_errors = self.link_errors.saturating_add(1);
                }
            }
        }

        // 4. System1 兜底（启发式；NoAction/Unavailable 语义原样透传）。
        let wire = self.local.decide(telemetry, now_ms)?;
        self.last_meta = Some(DecisionMeta {
            used_remote: false,
            path: DecisionPath::Fast,
            req_id: None,
            prompt_summary: String::new(),
            resp_summary: String::new(),
        });
        Ok(wire)
    }

    fn peer_budget(&self) -> PeerBudget {
        peer_budget(self.cfg.max_power_kw, self.cfg.ttl_secs)
    }

    fn build_plan(
        a: &AcceptedPlan,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> ProposedControlPlan {
        use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
        use eneros_solver_core::result::SolveStatus;
        let power = a.power_kw;
        ProposedControlPlan {
            plan_id: alloc::format!("remote-{}", a.req_id),
            path: DecisionPath::Slow,
            schedule: ScheduleResult {
                schedule: vec![ScheduleEntry {
                    period: 0,
                    charge_power_kw: if power < 0.0 { -power } else { 0.0 },
                    discharge_power_kw: if power > 0.0 { power } else { 0.0 },
                    net_power_kw: power,
                    soc_pct: telemetry.state.soc_pct,
                    revenue_yuan: 0.0,
                }],
                total_revenue_yuan: 0.0,
                objective_value: 0.0,
                solve_status: SolveStatus::Suboptimal,
            },
            power_kw: power,
            target_device: a.target_device.clone(),
            ttl_secs: a.ttl_secs,
            issued_at_ms: now_ms,
            input_digest: Some(telemetry.provenance_digest),
            model_digest: None,
            latency: Default::default(),
            safety_violations: 0,
            safety_clamped: false,
            feedback: None,
        }
    }
}

impl<L: DecisionBackend, P: PeerLink> DecisionBackend for RemoteBrain<L, P> {
    fn decide(
        &mut self,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        self.decide_full(telemetry, now_ms)
    }
}

/// prompt 摘要（请求要素投影；截断策略见调用点）。
fn prompt_summary(req_id: u64, t: &TrustedTelemetry) -> String {
    alloc::format!(
        "req={} soc={:.2} v={:.0}V",
        req_id,
        t.state.soc_pct,
        t.state.voltage_v
    )
}

/// 小写 hex（digest → 契约字段；no_std 无 {:#x} 补零便利）。
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xF) as usize] as char);
    }
    s
}

/// SPEC-SERIES-2026-08-28/B WP-B.3：peer 链路 EIPC 适配（自 svc-ai-worker
/// bin 下沉——请求 JSON → svc-mqtt 分片 EXECUTE；drv wire 单帧 ≤64B）。
/// host 不可达（seL4 IPC）；wire 语义与词形逐字节保留自旧 bin 实现。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub struct PeerLinkEipc {
    /// svc-mqtt 客户端（授予槽经 WP-54.4 按名解析）。
    pub mqtt: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    /// drv_call 关联号（逐帧递增）。
    pub correlation: u32,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl PeerLink for PeerLinkEipc {
    fn send_request(&mut self, req_json: &str) -> Result<(), LinkError> {
        use eneros_agent_sdk::ai_relay::fragment_request;
        let (_total, frames) = fragment_request(req_json.as_bytes())
            .map_err(|e| LinkError(alloc::format!("{e:?}")))?;
        // 本地 IPC 正常亚毫秒；2s 上限防 mqtt 未就绪时长阻塞（心跳面安全边际）。
        let timeout_ns = 2_000_000_000u64;
        for frame in frames {
            self.correlation = self.correlation.wrapping_add(1);
            let mut req = eneros_drv_svc::runtime::DriverRequest::new(
                self.correlation,
                eneros_drv_svc::runtime::OP_EXECUTE,
            );
            req.payload[..frame.len()].copy_from_slice(&frame);
            req.payload_len = frame.len() as u16;
            let raw = eneros_drv_svc::runtime::wire::encode_request(&req);
            // drv-svc 服务端按 4B/word 回 MR（47 卷 padding 坑同款）——
            // 接受尾部补零后按精确长度解码。
            match self.mqtt.drv_call(&raw, timeout_ns) {
                Ok(resp) => {
                    let decode_len =
                        core::cmp::min(resp.len(), eneros_drv_svc::runtime::wire::RESPONSE_LEN);
                    let resp = eneros_drv_svc::runtime::wire::decode_response(&resp[..decode_len])
                        .ok_or_else(|| LinkError(String::from("bad-resp")))?;
                    if resp.status != eneros_drv_svc::runtime::ST_OK
                        || resp.payload.first() != Some(&0)
                    {
                        return Err(LinkError(alloc::format!(
                            "mqtt-reject status={}",
                            resp.status
                        )));
                    }
                }
                Err(e) => return Err(LinkError(alloc::format!("drv:{e:?}"))),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_worker::tests_helpers::minimal_trusted_at;
    use crate::ai_worker::{decode_proposal, HeuristicBackend};

    struct MockLink {
        fail: bool,
        sent: Vec<String>,
    }
    impl MockLink {
        fn ok() -> Self {
            Self {
                fail: false,
                sent: Vec::new(),
            }
        }
    }
    impl PeerLink for MockLink {
        fn send_request(&mut self, req_json: &str) -> Result<(), LinkError> {
            if self.fail {
                return Err(LinkError(String::from("eipc-down")));
            }
            self.sent.push(String::from(req_json));
            Ok(())
        }
    }

    fn cfg() -> RemoteBrainConfig {
        RemoteBrainConfig {
            timeout_ms: 5_000,
            unhealthy_after: 3,
            probe_every: 5,
            max_power_kw: 5.0,
            ttl_secs: 60,
        }
    }

    fn brain() -> RemoteBrain<HeuristicBackend, MockLink> {
        RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg())
    }

    fn resp_json(req_id: u64, seq: u64, power: f64) -> String {
        alloc::format!(
            r#"{{"v":1,"req_id":{req_id},"seq":{seq},"plan":{{"power_kw":{power},"target_device":"pcs","ttl_secs":60}},"reason":"soc"}}"#
        )
    }

    #[test]
    fn happy_path_remote_proposal() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        // tick1：Healthy 常态发起请求（req_id=1）+ 本 tick 启发式兜底。
        let w = b.decide_full(&t, 10_000).expect("wire");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.plan_id, "heur-1");
        assert_eq!(b.triggers_sent, 1);
        assert!(b.link.sent[0].contains("\"req_id\":1"));
        // 应答到达（配对 req=1）。
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -3.0), 10_500),
            PeerMsgVerdict::Accepted
        );
        // tick2：消费远程提案。
        let w = b.decide_full(&t, 11_000).expect("wire");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.plan_id, "remote-1");
        assert_eq!(p.path, DecisionPath::Slow);
        assert_eq!(p.power_kw, -3.0);
        assert_eq!(p.input_digest, Some(t.provenance_digest));
        assert_eq!(b.remote_used_total, 1);
        let meta = b.last_meta().expect("meta");
        assert!(meta.used_remote);
        assert_eq!(meta.req_id, Some(1));
        assert!(meta.prompt_summary.len() <= 48);
        assert!(meta.resp_summary.len() <= 64);
        assert!(b.drain_events().is_empty(), "未离开 Healthy 无迁移事件");
    }

    #[test]
    fn timeout_degrades_to_heuristic_this_tick() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&t, 10_000).expect("tick1");
        // 超时后 tick：pending 清除 + 计数 + 本 tick 走启发式。
        let w = b.decide_full(&t, 16_000).expect("tick2");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.path, DecisionPath::Fast);
        assert_eq!(b.timeouts_total, 1);
        assert!(!b.last_meta().expect("meta").used_remote);
        // 同 tick 已重新发起新请求（Healthy 常态）。
        assert_eq!(b.triggers_sent, 2);
    }

    #[test]
    fn consecutive_timeouts_mark_unhealthy_with_event() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        let mut now = 10_000;
        // 第 1 tick 仅发起请求（无在途可超时）；其后 3 tick 各超时一次。
        for _ in 0..4 {
            now += 6_000;
            b.decide_full(&t, now).expect("tick");
        }
        assert!(!b.is_healthy());
        assert_eq!(b.timeouts_total, 3);
        let events = b.drain_events();
        assert!(
            events.contains(&SwitchEvent::PeerUnhealthy {
                consecutive_timeouts: 3
            }),
            "须有 Unhealthy 迁移事件，got {events:?}"
        );
    }

    #[test]
    fn unhealthy_probes_periodically_and_recovers() {
        let mut c = cfg();
        c.probe_every = 3;
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), c);
        let t = minimal_trusted_at(10_000);
        let mut now = 10_000;
        // 4 tick：t1 发起，t2-t4 各超时一次 → Unhealthy（consecutive=3）。
        for _ in 0..4 {
            now += 6_000;
            b.decide_full(&t, now).expect("degrade ticks");
        }
        assert!(!b.is_healthy());
        let sent_after_unhealthy = b.triggers_sent;
        // Unhealthy：非探测 tick（ticks=5，5%3≠0）不发请求。
        now += 6_000;
        b.decide_full(&t, now).expect("tick");
        assert_eq!(
            b.triggers_sent, sent_after_unhealthy,
            "非探测 tick 不得发请求"
        );
        // 探测 tick（ticks=6，6%3=0）发请求。
        now += 6_000;
        b.decide_full(&t, now).expect("probe tick");
        assert_eq!(
            b.triggers_sent,
            sent_after_unhealthy + 1,
            "探测 tick 须发请求"
        );
        let probe_req = b.next_req_id;
        // 探测应答到达 → 恢复事件 + 下 tick 回 AI。
        assert_eq!(
            b.on_peer_message(&resp_json(probe_req, 10, 2.5), now + 100),
            PeerMsgVerdict::Accepted
        );
        assert!(b.drain_events().contains(&SwitchEvent::PeerRecovered));
        now += 1_000;
        let w = b.decide_full(&t, now).expect("tick");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.plan_id, alloc::format!("remote-{probe_req}"));
        assert_eq!(b.remote_used_total, 1);
        assert!(b.is_healthy());
    }

    #[test]
    fn over_budget_response_rejected_as_bad() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&t, 10_000).expect("tick1");
        // 越权提案（|power| > 5kW 预算）→ Bad + 不消费 + 继续降级。
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -5.5), 10_500),
            PeerMsgVerdict::Bad
        );
        assert_eq!(b.bad_responses_total, 1);
        assert!(b.accepted.is_none());
        let w = b.decide_full(&t, 11_000).expect("tick2");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.path, DecisionPath::Fast, "坏应答不得进入决策");
    }

    #[test]
    fn replay_and_regression_seq_rejected() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&t, 10_000).expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_json(1, 4, -3.0), 10_500),
            PeerMsgVerdict::Accepted
        );
        // tick2 消费远程提案（消费后同 tick 不再发新请求）。
        b.decide_full(&t, 11_000).expect("consume");
        // tick3 发起新请求（req=2）。
        b.decide_full(&t, 12_000).expect("tick3");
        // 对 req=2 重放旧 seq（≤4）→ Bad。
        assert_eq!(
            b.on_peer_message(&resp_json(2, 4, -3.0), 12_100),
            PeerMsgVerdict::Bad
        );
        assert_eq!(b.bad_responses_total, 1);
    }

    #[test]
    fn late_response_after_expiry_counted_late_not_bad() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&t, 10_000).expect("tick1");
        // 超时清除在途后，旧 req 的应答才到达 → Late（非协议违规）。
        b.decide_full(&t, 16_000).expect("expire tick");
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -3.0), 16_100),
            PeerMsgVerdict::Late
        );
        assert_eq!(b.late_total, 1);
        assert_eq!(b.bad_responses_total, 0);
        // 无在途时的主动推送同样 Late。
        b.decide_full(&t, 17_000).expect("tick");
        b.decide_full(&t, 18_000).expect("tick");
        assert_eq!(
            b.on_peer_message(&resp_json(99, 50, 1.0), 18_100),
            PeerMsgVerdict::Late
        );
        assert_eq!(b.late_total, 2);
    }

    #[test]
    fn link_error_degrades_without_panic() {
        let mut b = RemoteBrain::new(
            HeuristicBackend::new(),
            MockLink {
                fail: true,
                sent: Vec::new(),
            },
            cfg(),
        );
        let t = minimal_trusted_at(10_000);
        let w = b.decide_full(&t, 10_000).expect("wire");
        let p = decode_proposal(&w).expect("plan");
        assert_eq!(p.path, DecisionPath::Fast);
        assert_eq!(b.link_errors, 1);
        assert_eq!(b.triggers_sent, 0);
        assert!(b.pending.is_none());
    }

    #[test]
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免先例 ai_worker.rs:618）
    fn deadband_noaction_passthrough_when_no_remote() {
        let mut b = brain();
        let mut t = minimal_trusted_at(10_000);
        t.state.soc_pct = 0.4; // 死区
        match b.decide_full(&t, 10_000) {
            Err(crate::ai_worker::DecisionReject::NoAction(_)) => {}
            other => panic!("死区须透传 NoAction，got {other:?}"),
        }
    }

    #[test]
    fn remote_plan_ids_unique_across_cycles() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        let mut now = 10_000u64;
        let mut ids = Vec::new();
        for i in 1..=3u64 {
            b.decide_full(&t, now).expect("tick");
            assert_eq!(
                b.on_peer_message(&resp_json(i, i, -1.0), now + 100),
                PeerMsgVerdict::Accepted
            );
            now += 1_000;
            let w = b.decide_full(&t, now).expect("tick");
            ids.push(decode_proposal(&w).expect("plan").plan_id);
            now += 1_000;
        }
        assert_eq!(
            ids,
            vec![
                "remote-1".to_string(),
                "remote-2".to_string(),
                "remote-3".to_string()
            ]
        );
    }
}
