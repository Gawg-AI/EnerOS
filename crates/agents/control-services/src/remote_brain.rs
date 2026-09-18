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

use alloc::boxed::Box;
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

use crate::ai_worker::{encode_candidate_payload, DecisionBackend, DecisionReject, DegradeArmSeam};

/// 切换/状态迁移事件（审计与串口证据面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchEvent {
    /// 连续超时达阈值——peer 标记 Unhealthy（System1 接管）。
    PeerUnhealthy { consecutive_timeouts: u32 },
    /// 有效应答重新到达——恢复 Healthy（回 AI）。
    PeerRecovered,
    /// 68.5 WP-68.5.3：D3 降级 lease 武装（peer 超时裁决后，System1 兜底
    /// 逐次持签授权——"兜底也要持签"）。
    LeaseArmed { lease_id: [u8; 16] },
    /// 68.5：lease 武装失败（issue/验签拒绝）。如实注记：当前装配该臂
    /// 结构性不可达（全零键/TTL=0 被 cfg 守卫拦截、自签验签必过、
    /// already-armed 被前置判据挡）——若达此面 = 无签兜底继续（68.5
    /// 前行为），非决策停供。
    LeaseArmFailed,
    /// 68.5：episode 结束，lease 撤销（D0 回归）。reason：`peer-accepted`
    /// = 有效应答消费；`lease-expired` = lease 到期复位（68.5 复查修复
    /// ——复位后随本次超时裁决重武装，见 [`RemoteBrain::arm_lease_wiring`]）。
    LeaseRevoked { reason: &'static str },
}

/// WP-68.5.2（F27 第 5 项修）：应答来源后端标签——reason 前缀词形单源。
///
/// 对端约定（planner-peer bin `BACKEND_TAG` 同源）：`backend=planner` 前缀
/// = host planner 分区真 HiGHS 产物；其余非空 reason = remote-llm（旧
/// LLM peer 形态）。System1 启发式不经此函数（used_remote=false 即
/// heuristic）。词形漂移 = 审计不可区分（t3 锁）。
pub fn backend_tag(reason: &str) -> &'static str {
    if reason.starts_with("backend=planner") {
        "planner"
    } else {
        "remote-llm"
    }
}

/// reason 字段取值（" key=" 定界；无则 None）。
fn reason_field<'a>(reason: &'a str, key: &str) -> Option<&'a str> {
    let needle = alloc::format!(" {key}=");
    let start = reason.find(&needle)? + needle.len();
    let rest = &reason[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(&rest[..end])
}

/// 求解状态诚实映射：仅字面 `status=optimal` 可声明 Optimal（对端 L2 求解
/// 门只产 optimal）；其余一律 Suboptimal（无声明不升格——解码侧同纪律）。
fn reason_solve_status(reason: &str) -> eneros_solver_core::result::SolveStatus {
    match reason_field(reason, "status") {
        Some("optimal") => eneros_solver_core::result::SolveStatus::Optimal,
        _ => eneros_solver_core::result::SolveStatus::Suboptimal,
    }
}

/// 模型 digest 诚实映射：`model=<64hex>` → 32B；缺/形不符 = None
/// （不伪造——remote-llm/heuristic 形态本就无模型）。
fn reason_model_digest(reason: &str) -> Option<[u8; 32]> {
    reason_field(reason, "model").and_then(eneros_hex::decode_fixed::<32>)
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
    /// 68.5 WP-68.5.3：D3 降级 lease HMAC 键（ai-peer 配置注入链供给；
    /// **全零 = 未装配**——不武装，行为与 68.5 前一致，host 测试缺省态）。
    pub lease_key: [u8; 32],
    /// D3 lease 有效时长秒（0 = 未装配；到期不复位即拒降级维持——D3
    /// fail-closed 语义，t5 锁）。
    pub lease_ttl_secs: u32,
}

/// 在途请求。
struct PendingReq {
    req_id: u64,
    issued_ms: u64,
    /// 蓝图 70.10 WP-70.10.3：在途 v2 请求的身份绑定（应答必须逐字段
    /// 相符——晚答/跨代答一律拒）。`None` = v1 迁移轨（无结构化绑定）。
    binds: Option<eneros_intent_contract::peer::PlanBinds>,
    /// 在途 v2 请求的资源绑定（应答分配**必须**落在该集合内，且绑定
    /// 字段以请求为准——不采信应答回带的 device/point/unit）。
    assets: Vec<eneros_intent_contract::peer::PeerAssetSpec>,
}

/// 目标侧结构化计划输入源（蓝图 70.10 WP-70.10.2：host 消费**目标侧
/// 事实**而不是自持 fixture）。
///
/// 生产实现由目录（铭牌/合同）+ 冻结快照（SOC → 初始能量）+ 现有
/// envelope 视图装配；返回 `None` = 无结构化输入源 → 走 v1 迁移轨
/// （v1 产物不具新控制资格，`TrustedInputPolicy` 已在执行入口拦）。
pub trait PlanSpecSource {
    /// 由可信输入身份与当前 SOC 装配一次请求规格。
    fn specs(
        &mut self,
        facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
        soc_pct: f64,
    ) -> Option<PlanSpecs>;
}

/// 结构化请求规格（目标侧事实投影）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanSpecs {
    pub assets: Vec<eneros_intent_contract::peer::PeerAssetSpec>,
    pub grid_asset_id: u64,
    pub objective_kind: String,
    pub prices_per_kwh: Vec<f64>,
    pub step_ns: u64,
    pub step_count: u32,
}

/// 已接受待消费的有效提案（已过全量校验 + 与在途请求配对）。
enum AcceptedPlan {
    /// v1 迁移轨（单值提案；不具新控制资格）。
    V1 {
        req_id: u64,
        power_kw: f64,
        ttl_secs: u32,
        target_device: String,
        reason: String,
        prompt_summary: String,
    },
    /// v2 结构化轨（全量分配；身份已与在途请求逐字段绑定）。
    V2(Box<AcceptedPlanV2>),
}

/// 已接受的 v2 结构化计划。
struct AcceptedPlanV2 {
    req_id: u64,
    plan: eneros_intent_contract::peer::PeerPlanV2,
    /// 请求侧资源绑定（应答回带的 device/point/unit 不被采信）。
    assets: Vec<eneros_intent_contract::peer::PeerAssetSpec>,
    /// 请求侧身份绑定（接纳时的代次；消费前复核——复查修复）。
    binds: eneros_intent_contract::peer::PlanBinds,
    /// 应答 reason（含 `backend=` 词形；审计标签单源——复查修复：
    /// 原实现用 `solver_status` 当 reason 的替身，`backend_tag` 恒返回
    /// `remote-llm`，planner 与 LLM 在审计面不可区分）。
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
    /// 蓝图 61 v0.314.35：目录合同版本（候选 stamped；0 = 未装配）。
    pub catalog_version: u64,
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
    /// 蓝图 70.10：结构化计划输入源（None = v1 迁移轨）。
    spec_source: Option<alloc::boxed::Box<dyn PlanSpecSource + 'static>>,
    /// 观测：在途请求因会话重连/输入换代被取消的次数。
    pub pending_cancelled: u64,
    /// 观测：接受的 v2 结构化应答数。
    pub structured_accepted: u64,
    /// 观测：已接纳但因**输入换代**在消费前丢弃的 v2 应答数（复查修复：
    /// 换代后消费会拼出杂交候选——dispatch 必拒并烧 EA 错误预算）。
    pub stale_generation_accepted_total: u64,
}

impl<L: DecisionBackend + DegradeArmSeam, P: PeerLink> RemoteBrain<L, P> {
    pub fn new(local: L, link: P, cfg: RemoteBrainConfig) -> Self {
        Self::with_catalog_ver(local, link, cfg, 0)
    }

    /// 目录版本注入（[08] §4.8：远程候选与 Registry 同代）。
    pub fn with_catalog_ver(
        local: L,
        link: P,
        cfg: RemoteBrainConfig,
        catalog_version: u64,
    ) -> Self {
        Self {
            catalog_version,
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
            spec_source: None,
            pending_cancelled: 0,
            structured_accepted: 0,
            stale_generation_accepted_total: 0,
        }
    }

    /// 蓝图 70.10 WP-70.10.2：注入结构化计划输入源（生产 = 目录+快照
    /// 装配；缺省 None = v1 迁移轨）。
    pub fn set_plan_spec_source(&mut self, src: alloc::boxed::Box<dyn PlanSpecSource + 'static>) {
        self.spec_source = Some(src);
    }

    /// 蓝图 70.10 WP-70.10.3：会话重连——**取消旧 pending**（旧应答不再
    /// 能被采信：其绑定的是重连前的输入代次/会话序号）。返回是否确有取消。
    pub fn on_session_reconnect(&mut self) -> bool {
        if self.pending.take().is_some() {
            self.pending_cancelled = self.pending_cancelled.saturating_add(1);
            self.pending_prompt_summary = None;
            true
        } else {
            false
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

    /// 蓝图 69 卷 69.5（WP-69.5.1）：System1 本地后端**只读**访问——
    /// 降级/学习观测快照经此透传（`DegradeObserveQuery` 的 RemoteBrain
    /// 臂委托本地后端既有读面）；纯读，零状态接触。
    pub fn local(&self) -> &L {
        &self.local
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
        use eneros_intent_contract::peer::{parse_and_validate_v2, PEER_CONTRACT_VERSION_V2};
        // 无在途请求 → 一切应答皆迟到（含主动推送的垃圾流量）。
        let Some(pending) = &self.pending else {
            self.late_total = self.late_total.saturating_add(1);
            return PeerMsgVerdict::Late;
        };
        let expect_req_id = pending.req_id;
        let issued_ms = pending.issued_ms;
        let binds = pending.binds;
        // 版本判别：**先探 v 再分派**（v1/v2 的 schema 不同，不能靠解析失败
        // 猜测版本——v1 应答解析成 v2 结构会以 Schema 错收场，无法区分
        // "版本不同"与"真坏包"）。
        #[derive(serde::Deserialize)]
        struct VersionProbe {
            v: u64,
        }
        let probed = serde_json::from_str::<VersionProbe>(json).map(|p| p.v);
        let use_v2 = matches!(probed, Ok(v) if v == PEER_CONTRACT_VERSION_V2);
        if matches!(probed, Ok(v) if v != 1 && v != PEER_CONTRACT_VERSION_V2) {
            self.bad_responses_total = self.bad_responses_total.saturating_add(1);
            self.last_reject_detail =
                Some(alloc::format!("unsupported contract version: {probed:?}"));
            eneros_runtime::println!("[svc-ai-worker] PEER-V2-REJECT unsupported-version");
            return PeerMsgVerdict::Bad;
        }
        let checks = ResponseChecks {
            expect_req_id,
            expect_seq_gt: self.last_seq,
            budget: &self.peer_budget(),
        };
        if use_v2 {
            // v2 需要绑定；在途无绑定（v1 请求）却收到 v2 应答 = 协议错。
            let Some(binds) = binds else {
                self.bad_responses_total = self.bad_responses_total.saturating_add(1);
                self.last_reject_detail = Some(String::from("v2 response without bound pending"));
                eneros_runtime::println!("[svc-ai-worker] PEER-V2-REJECT unbound-pending");
                return PeerMsgVerdict::Bad;
            };
            return match parse_and_validate_v2(json, &checks, &binds) {
                Ok(resp) => {
                    // 复查修复：**请求资产集合必须被应答全覆盖**（"全量
                    // 分配/轨迹完整"是 v2 通道的判据本体）。缺任一请求资产
                    // = 该资产被静默留在不可控状态而计划被记为"全量"——
                    // 契约违规按坏应答处置（不做部分采纳：部分计划在
                    // dispatch 侧被认证为整体覆盖，语义上是错的）。
                    if let Some(missing) =
                        Self::missing_response_assets(&pending.assets, &resp.plan.assignments)
                    {
                        self.bad_responses_total = self.bad_responses_total.saturating_add(1);
                        self.last_reject_detail = Some(alloc::format!(
                            "v2 response misses requested asset {missing}"
                        ));
                        eneros_runtime::println!(
                            "[svc-ai-worker] PEER-V2-REJECT missing-asset {missing}"
                        );
                        return PeerMsgVerdict::Bad;
                    }
                    if now_ms.saturating_sub(issued_ms) > self.cfg.timeout_ms {
                        self.late_total = self.late_total.saturating_add(1);
                        return PeerMsgVerdict::Late;
                    }
                    self.last_seq = Some(resp.seq);
                    let assets = pending.assets.clone();
                    self.accepted = Some(AcceptedPlan::V2(Box::new(AcceptedPlanV2 {
                        req_id: resp.req_id,
                        plan: resp.plan,
                        assets,
                        binds,
                        reason: resp.reason.clone(),
                        prompt_summary: self.pending_prompt_summary.take().unwrap_or_default(),
                    })));
                    self.structured_accepted = self.structured_accepted.saturating_add(1);
                    self.pending = None;
                    if !self.healthy {
                        self.healthy = true;
                        self.events.push(SwitchEvent::PeerRecovered);
                    }
                    PeerMsgVerdict::Accepted
                }
                Err(eneros_intent_contract::PeerContractError::Correlation { .. }) => {
                    self.late_total = self.late_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-ai-worker] PEER-V2-LATE correlation（伪造/错位关联应答——丢弃不消费）"
                    );
                    PeerMsgVerdict::Late
                }
                Err(e) => {
                    self.bad_responses_total = self.bad_responses_total.saturating_add(1);
                    self.last_reject_detail = Some(alloc::format!("{e:?}"));
                    eneros_runtime::println!("[svc-ai-worker] PEER-V2-REJECT {e:?}");
                    PeerMsgVerdict::Bad
                }
            };
        }
        if binds.is_some() {
            // 复查修复：v2 在途请求不得被 v1 应答"顶替"（反向 t8d）。
            // 旧 schema 单值提案不具结构化绑定，消费它会清掉在途并推进
            // seq——真正的 v2 应答随后变 Late，远程往返被静默作废。
            self.bad_responses_total = self.bad_responses_total.saturating_add(1);
            self.last_reject_detail = Some(String::from("v1 response on bound (v2) pending"));
            return PeerMsgVerdict::Bad;
        }
        match parse_and_validate(json, &checks) {
            Ok(resp) => {
                // 新鲜度：配对成功但已过超时窗 → 迟到（遥测快照陈旧不可用）。
                if now_ms.saturating_sub(issued_ms) > self.cfg.timeout_ms {
                    self.late_total = self.late_total.saturating_add(1);
                    return PeerMsgVerdict::Late;
                }
                self.last_seq = Some(resp.seq);
                self.accepted = Some(AcceptedPlan::V1 {
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
    ///
    /// 蓝图 70.8：入参为 [`DecisionInput`]（身份 + 状态一体）——由可信
    /// 快照派生的候选身份在其内，本函数不再自造身份。
    pub fn decide_full(
        &mut self,
        input: &crate::ai_worker::DecisionInput<'_>,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        let telemetry = input.telemetry;
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
                // 68.5 WP-68.5.3：peer 失联（超时裁决）即武装 D3 lease——
                // 后续每个 System1 兜底决策逐次持签授权（"兜底也要持签"，
                // D3 SolverFallback 语义；degrade.rs 机制面零改动，本处
                // 只接线）。同 tick 的 System1 兜底即在武装后执行。
                self.arm_lease_wiring(now_ms);
            }
        }

        // 2. 消费已接受的有效远程提案（System2 路径）。
        //    复查修复：**消费前复核输入代次**——应答在途期间输入换代时，
        //    以当前身份 + 应答时域拼出的候选是杂交体（dispatch 入口以
        //    "候选时域起点与快照产出时刻不符"必拒，EA 记 DispatchRejected
        //    并烧错误预算）。代次不符 = 丢弃该应答，走 System1（本地兜底
        //    绑定当前输入身份），绝不把旧代计划伪造成新代决策。
        let stale_accepted = match &self.accepted {
            Some(AcceptedPlan::V2(v2)) => !Self::catalog_generation_matches(input, &v2.binds),
            _ => false,
        };
        if stale_accepted {
            self.accepted = None;
            self.stale_generation_accepted_total =
                self.stale_generation_accepted_total.saturating_add(1);
            self.last_reject_detail = Some(String::from(
                "accepted plan generation changed before consume",
            ));
            if let Some(AcceptedPlan::V2(v2)) = &self.accepted {
                eneros_runtime::println!(
                    "[svc-ai-worker] PEER-V2-STALE-DROP catalog-mismatch binds_cv={} input_cv={}",
                    v2.binds.catalog_version,
                    input.facts.catalog_version,
                );
            }
        }
        if let Some(a) = self.accepted.take() {
            self.consecutive_timeouts = 0;
            self.remote_used_total = self.remote_used_total.saturating_add(1);
            // 68.5：有效应答消费 ⇒ 超时 episode 结束——lease 撤销（D0
            // 回归；healthy 常态的 fail-operational 兜底回到无 lease 行为）。
            if self.local.degradation_armed() {
                self.local.revoke_degradation();
                self.events.push(SwitchEvent::LeaseRevoked {
                    reason: "peer-accepted",
                });
            }
            let (cand, meta) = match a {
                // —— v1 迁移轨（单值提案；不具新控制资格）——
                AcceptedPlan::V1 {
                    req_id,
                    power_kw,
                    ttl_secs,
                    target_device,
                    reason,
                    prompt_summary,
                } => {
                    // WP-68.5.2：后端标签前置（[backend=X] 在 64 字符截断
                    // 前缀内恒可读——审计截断 48/64 维持，F27 第 9 项登记不扩）。
                    let resp_summary = truncate_for_audit(
                        &alloc::format!(
                            "[backend={}] power={}kW ttl={}s dev={} why={}",
                            backend_tag(&reason),
                            power_kw,
                            ttl_secs,
                            target_device,
                            reason
                        ),
                        64,
                    );
                    let plan = Self::build_plan(
                        req_id,
                        power_kw,
                        ttl_secs,
                        &target_device,
                        &reason,
                        telemetry,
                        now_ms,
                    );
                    let meta = DecisionMeta {
                        used_remote: true,
                        path: DecisionPath::Slow,
                        req_id: Some(req_id),
                        prompt_summary,
                        resp_summary,
                    };
                    let cand = crate::dispatch_service::candidate_from_proposal(
                        &plan,
                        self.catalog_version,
                    )
                    .map_err(|e| {
                        DecisionReject::NoAction(alloc::format!("candidate-adapter: {e:?}"))
                    })?;
                    (cand, meta)
                }
                // —— v2 结构化轨（全量分配；身份已与在途请求逐字段绑定）——
                AcceptedPlan::V2(v2) => {
                    let n = v2.plan.assignments.len();
                    let resp_summary = truncate_for_audit(
                        &alloc::format!(
                            "[backend={}] assignments={} assets={} status={} model={} why={}",
                            backend_tag(&v2.reason),
                            n,
                            v2.assets.len(),
                            v2.plan.solver_status,
                            &v2.plan.model_digest_hex[..8.min(v2.plan.model_digest_hex.len())],
                            "structured-plan"
                        ),
                        64,
                    );
                    let req_id = v2.req_id;
                    let prompt_summary = v2.prompt_summary.clone();
                    // 绑定 = 在途请求的身份（快照/目录/时域）——与请求同一
                    // 生成器公式；验证方（dispatch 入口）重算同一公式。
                    if !input.bound() {
                        // 结构化计划必须绑定到冻结快照身份（无身份 = 无控制
                        // 资格）：宁可 NoAction，也不造伪身份。
                        eneros_runtime::println!(
                            "[svc-ai-worker] PEER-V2-CONSUME-REJECT unbound-trusted-input"
                        );
                        return Err(DecisionReject::NoAction(String::from(
                            "structured-plan: unbound trusted input",
                        )));
                    }
                    // 目录版本取**可信输入**携带的代次（不是本进程的配置
                    // 值）——与在途请求的绑定同一来源，dispatch 入口重算即
                    // 判同代；两者不一致时由契约绑定比对先行拒绝。
                    // 重锚定：时域起点取**当前**快照产刻（非应答携带的旧
                    // 值）——候选身份/时域一体当前代（dispatch 四字段复核
                    // 可过），远程分配值保留由认证器全量物理复核兜底。
                    let inputs = eneros_control_dispatch::adapter::CandidateInputs::from_snapshot(
                        input.facts.snapshot_id,
                        input.facts.catalog_version,
                        input.facts.produced_at_ns / 1_000_000,
                        v2.plan.step_ns,
                        v2.plan.step_count,
                    );
                    let cand = crate::structured_plan::candidate_from_structured_plan(
                        &v2.plan, &inputs, &v2.assets,
                    )
                    .map_err(|e| {
                        eneros_runtime::println!(
                            "[svc-ai-worker] PEER-V2-CONSUME-REJECT candidate-build {e}"
                        );
                        DecisionReject::NoAction(alloc::format!("structured-plan: {e}"))
                    })?;
                    let meta = DecisionMeta {
                        used_remote: true,
                        path: DecisionPath::Slow,
                        req_id: Some(req_id),
                        prompt_summary,
                        resp_summary,
                    };
                    (cand, meta)
                }
            };
            self.last_meta = Some(meta);
            return Ok(encode_candidate_payload(&cand));
        }

        // 3. 需要新请求？Healthy 常态发起；Unhealthy 仅周期探测。
        let probe_due = !self.healthy && self.ticks % u64::from(self.cfg.probe_every.max(1)) == 0;
        if self.pending.is_none() && (self.healthy || probe_due) {
            self.next_req_id = self.next_req_id.saturating_add(1);
            let req_id = self.next_req_id;
            // 蓝图 70.10 WP-70.10.2/70.10.3：输入已绑定 + 有规格来源 ⇒
            // 结构化 v2 请求（host 消费目标侧事实；应答按绑定接纳）。
            if input.bound() {
                if let Some(src) = self.spec_source.as_mut() {
                    if let Some(specs) = src.specs(input.facts, telemetry.state.soc_pct) {
                        if !self.send_structured_request(
                            req_id,
                            input.facts,
                            &specs,
                            now_ms,
                            telemetry,
                        ) {
                            self.next_req_id = self.next_req_id.saturating_sub(1);
                        }
                        // 已发（含 pending/binds/计数）——继续走 System1
                        // 兜底路径语义（本 tick 仍返回兜底提案，应答下
                        // 一 tick 消费）。
                    } else {
                        // 规格源存在但**本代次无输入**（目录代次不符/无可控
                        // 资产/价格或预算不可用）⇒ **不发任何请求**
                        // （fail-closed：宁可不决策）。复查修复：原实现此处
                        // 落到 v1 迁移轨——而 v1 产物在生产
                        // `TrustedInputPolicy::Required` 下必被 dispatch 拒，
                        // 白烧一次往返 + EA 错误预算，且违背文档承诺。
                        eneros_runtime::println!(
                            "[svc-ai-worker] STRUCTURED-SKIP 本代次无结构化输入——不发请求（fail-closed）"
                        );
                    }
                    // 结构化轨已选定：不再回退 v1（避免同一代次上两条不同
                    // schema 的在途请求）。
                    return self.fallback_tick(input, now_ms);
                }
                // 70.14 复查修复：**可信输入已绑定但本机没有规格源**（站点未给
                // `ai-peer` 计划参数 = 实测的 `PLAN-SPEC-NONE`，也是当前生产
                // 默认装配）⇒ 同样不发 v1 请求。
                //
                // 旧实现落下去发 v1：v1 产物是**伪身份**候选（`snapshot_id` =
                // 请求摘要前 16B、`objective_digest` = 0），在
                // `TrustedInputPolicy::Required` 下 dispatch 入口必拒 ⇒ 每轮
                // 白烧一次往返 + EA 记 `DispatchRejected`；连续 3 次即
                // `Escalated` + 30s 冷却，冷却期内 EA 连 System1 启发式候选也
                // 一并压制（`error-budget-exhausted` ⇒ NOACTION）——把"这条轨
                // 不产物"变成"整条控制面间歇停摆"。
                // 与 `Some(src)/None` 同一处置：**宁可不决策**（fail-closed），
                // 不产生注定被拒的请求。
                eneros_runtime::println!(
                    "[svc-ai-worker] V1-SKIP 可信输入已绑定但无结构化规格源——不发 v1 请求（产物必被 Required 准入拒绝，避免烧错误预算）"
                );
                return self.fallback_tick(input, now_ms);
            }
            let req = PeerRequest::new(
                req_id,
                now_ms,
                PeerTelemetry {
                    soc_pct: telemetry.state.soc_pct,
                    voltage_v: telemetry.state.voltage_v,
                    current_a: telemetry.state.current_a,
                    frequency_hz: telemetry.state.frequency_hz,
                    generated_at_ms: telemetry.generated_at_ms,
                    digest_hex: eneros_hex::encode_lower(&telemetry.provenance_digest),
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
                            binds: None,
                            assets: Vec::new(),
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

        // 4. System1 兜底（启发式；NoAction/Unavailable 语义原样透传——
        // 与远程轨共用同一可信输入身份，兜底候选同样绑定冻结快照）。
        self.fallback_tick(input, now_ms)
    }

    /// System1 兜底单点（启发式；NoAction/Unavailable 透传）。
    fn fallback_tick(
        &mut self,
        input: &crate::ai_worker::DecisionInput<'_>,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        let wire = self.local.decide(input, now_ms)?;
        self.last_meta = Some(DecisionMeta {
            used_remote: false,
            path: DecisionPath::Fast,
            req_id: None,
            prompt_summary: String::new(),
            resp_summary: String::new(),
        });
        Ok(wire)
    }

    /// 蓝图 70.10 WP-70.10.2：发出结构化（v2）请求并把身份绑定记入在途。
    ///
    /// 返回 `false` = 未发出（编码/链路失败；调用方回退 req_id **但不**
    /// 回退 schema——同一代次上不得同时存在两条不同 schema 的在途请求）。
    fn send_structured_request(
        &mut self,
        req_id: u64,
        facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
        specs: &PlanSpecs,
        now_ms: u64,
        telemetry: &TrustedTelemetry,
    ) -> bool {
        use eneros_intent_contract::peer::{hex16, hex32, PeerRequestV2, PEER_CONTRACT_VERSION_V2};
        // 时域起点 = 冻结快照产出时刻（与候选绑定公式同一约定）。
        let horizon_start_ms = facts.produced_at_ns / 1_000_000;
        let req = PeerRequestV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id,
            ts_ms: now_ms,
            snapshot_id_hex: hex16(&facts.snapshot_id),
            provenance_hex: hex32(&facts.provenance_root),
            catalog_version: facts.catalog_version,
            horizon_start_ms,
            step_ns: specs.step_ns,
            step_count: specs.step_count,
            assets: specs.assets.clone(),
            grid_asset_id: specs.grid_asset_id,
            objective_kind: specs.objective_kind.clone(),
            prices_per_kwh: specs.prices_per_kwh.clone(),
            budget: self.peer_budget(),
        };
        let Ok(json) = req.encode() else {
            self.link_errors = self.link_errors.saturating_add(1);
            return false;
        };
        // 诊断行（2026-09-18 三资源定位）：发送前置——成功/失败都可观测。
        eneros_runtime::println!(
            "[svc-ai-worker] PLAN-REQ assets={} steps={} bytes={}",
            specs.assets.len(),
            specs.step_count,
            json.len()
        );
        if let Err(e) = self.link.send_request(&json) {
            self.link_errors = self.link_errors.saturating_add(1);
            eneros_runtime::println!(
                "[svc-ai-worker] PLAN-REQ-SEND-FAIL err={}",
                truncate_for_audit(&e.0, 96)
            );
            return false;
        }
        self.triggers_sent = self.triggers_sent.saturating_add(1);
        self.pending_prompt_summary = Some(truncate_for_audit(
            &alloc::format!(
                "structured req_id={req_id} assets={} steps={} soc={:.3}",
                specs.assets.len(),
                specs.step_count,
                telemetry.state.soc_pct
            ),
            48,
        ));
        self.pending = Some(PendingReq {
            req_id,
            issued_ms: now_ms,
            binds: Some(eneros_intent_contract::peer::PlanBinds {
                snapshot_id: facts.snapshot_id,
                catalog_version: facts.catalog_version,
                horizon_start_ms,
                step_ns: specs.step_ns,
                step_count: specs.step_count,
            }),
            assets: specs.assets.clone(),
        });
        true
    }

    fn peer_budget(&self) -> PeerBudget {
        peer_budget(self.cfg.max_power_kw, self.cfg.ttl_secs)
    }

    /// 应答未覆盖的请求资产（`Some(asset_id)` = 违规）。空 = 全覆盖。
    fn missing_response_assets(
        requested: &[eneros_intent_contract::peer::PeerAssetSpec],
        assignments: &[eneros_intent_contract::peer::PeerAssignment],
    ) -> Option<u64> {
        requested
            .iter()
            .find(|spec| !assignments.iter().any(|a| a.asset_id == spec.asset_id))
            .map(|spec| spec.asset_id)
    }

    /// 在途/已接纳绑定与**当前**可信输入是否同代（复查修复单源）：
    /// 快照 id + 目录版本 + 时域起点三元组——与 dispatch 入口
    /// `admit_trusted_input` 的同代判据同集合（答案必须一致；差异 =
    /// 候选在中途被拒）。
    /// R09 激活收口（2026-09-15）：消费侧绑定**放宽至目录代次**——
    /// 快照推进（同目录代次）不再丢弃远程计划，消费时**重锚定**当前代
    /// （候选身份/时域全取当前输入；远程分配值保留），认证器对重锚计划
    /// 做全量物理复核、dispatch 入口四字段复核照旧（fail-closed 兜底
    /// 双层不变）。目录代次 = 合同授权基础，仍逐字节绑定。
    fn catalog_generation_matches(
        input: &crate::ai_worker::DecisionInput<'_>,
        binds: &eneros_intent_contract::peer::PlanBinds,
    ) -> bool {
        if !input.bound() {
            return false; // 无可信身份 ⇒ 结构化绑定不可维持
        }
        binds.catalog_version == input.facts.catalog_version
    }

    /// 68.5 WP-68.5.3：D3 lease 武装（接线级——seam issue + 验签 arm；
    /// 失败事件化 fail-closed，绝不裸兜底）。键全零 = 未装配（host 测试
    /// 缺省 /68.5 前行为）。lease_id = 武装时刻 u128 LE（可读可对账）。
    fn arm_lease_wiring(&mut self, now_ms: u64) {
        if self.cfg.lease_key == [0u8; 32] || self.cfg.lease_ttl_secs == 0 {
            return;
        }
        if self.local.degradation_armed() {
            // 68.5 复查修复：lease 到期 = episode 自然终止——复位（审计
            // 可见）后走下方流程重武装。键在目标侧（附加裁决 7 自签），
            // 重签不越任何安全边界，每 episode 仍受 TTL/次数有界约束；
            // 无此复位路径时 authorize 恒 "lease expired" 且重武装被
            // 已武装守卫短路——peer 长期失联（>TTL）时决策链永久停供
            // （68.5 前启发式兜底可无限期工作，属可用性回归）。未过期
            // = episode 进行中，不重复武装。
            if !self.local.degradation_lease_expired(now_ms) {
                return;
            }
            self.local.revoke_degradation();
            self.events.push(SwitchEvent::LeaseRevoked {
                reason: "lease-expired",
            });
        }
        match self.local.arm_for_degrade(
            &self.cfg.lease_key,
            now_ms,
            self.cfg.lease_ttl_secs,
            self.cfg.max_power_kw,
        ) {
            Ok(lease_id) => self.events.push(SwitchEvent::LeaseArmed { lease_id }),
            Err(_) => self.events.push(SwitchEvent::LeaseArmFailed),
        }
    }

    /// 68.5：System1 本地后端降级武装查询（学习冻结判据消费面）。
    pub fn local_degradation_armed(&self) -> bool {
        self.local.degradation_armed()
    }

    /// v1 单值提案 → `ProposedControlPlan`（迁移轨专用；v2 结构化轨**直接**
    /// 构造 `CandidatePlan`，不经本函数——否则等于把完整计划再压扁成单值）。
    fn build_plan(
        req_id: u64,
        power_kw: f64,
        ttl_secs: u32,
        target_device: &str,
        reason: &str,
        telemetry: &TrustedTelemetry,
        now_ms: u64,
    ) -> ProposedControlPlan {
        use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
        let power = power_kw;
        ProposedControlPlan {
            plan_id: alloc::format!("remote-{req_id}"),
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
                // WP-68.5.2：不再硬编码 Suboptimal——按应答 reason 词形
                // 诚实映射（planner 对端 L2 门产 optimal；无声明不升格）。
                solve_status: reason_solve_status(reason),
            },
            power_kw: power,
            target_device: alloc::string::String::from(target_device),
            ttl_secs,
            issued_at_ms: now_ms,
            input_digest: Some(telemetry.provenance_digest),
            // WP-68.5.2：不再恒 None——planner 对端随 reason 携带模型
            // digest（64 hex → 32B）；remote-llm/heuristic 本就无模型 = None。
            model_digest: reason_model_digest(reason),
            latency: Default::default(),
            safety_clamped: false,
            feedback: None,
        }
    }
}

impl<L: DecisionBackend + DegradeArmSeam, P: PeerLink> DecisionBackend for RemoteBrain<L, P> {
    fn decide(
        &mut self,
        input: &crate::ai_worker::DecisionInput<'_>,
        now_ms: u64,
    ) -> Result<Vec<u8>, DecisionReject> {
        self.decide_full(input, now_ms)
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
    use crate::ai_worker::HeuristicBackend;

    /// CAND wire → (plan_id 字符串, power_kw) 测试视图（蓝图 61 wire 切换）。
    fn decode_cand(w: &[u8]) -> (String, f64) {
        let c = eneros_decision_contract::CandidatePlan::decode_v1(w).expect("cand");
        let bytes = c.id().0;
        let end = bytes
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        (
            String::from_utf8(bytes[..end].to_vec()).expect("utf8 id"),
            c.assignments()[0].power_kw,
        )
    }

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
            lease_key: [0u8; 32], // 缺省 = 未装配（不武装——既有行为锁）
            lease_ttl_secs: 0,
        }
    }

    /// 68.5：lease 装配态配置（t4/t5 判据面）。
    fn cfg_leased(key: [u8; 32], ttl: u32) -> RemoteBrainConfig {
        RemoteBrainConfig {
            lease_key: key,
            lease_ttl_secs: ttl,
            ..cfg()
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

    /// t3：自定义 reason 应答（后端词形注入）。
    fn resp_json_reason(req_id: u64, seq: u64, power: f64, reason: &str) -> String {
        alloc::format!(
            r#"{{"v":1,"req_id":{req_id},"seq":{seq},"plan":{{"power_kw":{power},"target_device":"pcs","ttl_secs":60}},"reason":"{reason}"}}"#
        )
    }

    // ========================================================================
    // 蓝图 70.10：结构化（v2）通道——绑定接纳 / 跨代拒收 / 重连取消
    // ========================================================================

    /// 结构化请求规格来源（测试双：1 资产 × 2 时段）。
    struct StubSpecs;
    impl PlanSpecSource for StubSpecs {
        fn specs(
            &mut self,
            _facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
            soc_pct: f64,
        ) -> Option<PlanSpecs> {
            Some(PlanSpecs {
                assets: vec![eneros_intent_contract::peer::PeerAssetSpec {
                    asset_id: 1,
                    capacity_kwh: 100.0,
                    eta_c: 0.95,
                    eta_d: 0.95,
                    initial_kwh: 100.0 * soc_pct,
                    soc_min: 0.05,
                    soc_max: 0.95,
                    p_min_kw: -5.0,
                    p_max_kw: 5.0,
                    ramp_up_kw_s: 1e6,
                    ramp_down_kw_s: 1e6,
                    device_id: 2,
                    point_id: 10,
                    unit_code: 6,
                }],
                grid_asset_id: 0,
                objective_kind: String::from("max_revenue"),
                prices_per_kwh: vec![1.0, 10.0],
                step_ns: 60_000_000_000,
                step_count: 2,
            })
        }
    }

    /// 身份绑定态输入（生产形态：facts 非零 + 完整）——遥测体用共享夹具
    /// 泄漏为 `'static`（测试生命周期，非生产形态）。
    fn bound_input() -> crate::ai_worker::DecisionInput<'static> {
        static FACTS: eneros_agent_sdk::snap_input_wire::SnapshotFacts =
            eneros_agent_sdk::snap_input_wire::SnapshotFacts {
                snapshot_id: [0x5A; 16],
                provenance_root: [0x3D; 32],
                cut_time_ns: 1_700_000_000_000_000_000,
                produced_at_ns: 1_700_000_000_000_000_000,
                catalog_version: 3,
                catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
                accepted: 7,
                missing: 0,
                completeness: eneros_agent_sdk::snap_input_wire::COMPLETE,
            };
        static TEL: std::sync::OnceLock<&'static TrustedTelemetry> = std::sync::OnceLock::new();
        let t = TEL.get_or_init(|| {
            Box::leak(Box::new(
                crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000),
            ))
        });
        crate::ai_worker::DecisionInput {
            telemetry: t,
            facts: &FACTS,
        }
    }

    /// 任意代次的身份绑定输入（换代场景用；泄漏为 `'static` = 测试生命周期）。
    fn input_gen(
        snapshot_id: [u8; 16],
        produced_ms: u64,
    ) -> crate::ai_worker::DecisionInput<'static> {
        let facts: &'static eneros_agent_sdk::snap_input_wire::SnapshotFacts =
            Box::leak(Box::new(eneros_agent_sdk::snap_input_wire::SnapshotFacts {
                snapshot_id,
                provenance_root: [0x3D; 32],
                cut_time_ns: produced_ms.saturating_mul(1_000_000),
                produced_at_ns: produced_ms.saturating_mul(1_000_000),
                catalog_version: 3,
                catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
                accepted: 7,
                missing: 0,
                completeness: eneros_agent_sdk::snap_input_wire::COMPLETE,
            }));
        let t: &'static TrustedTelemetry = Box::leak(Box::new(
            crate::ai_worker::tests_helpers::minimal_trusted_at(produced_ms),
        ));
        crate::ai_worker::DecisionInput {
            telemetry: t,
            facts,
        }
    }

    /// 任意代次 + **任意目录代次**的绑定输入（目录代次负例用）。
    fn input_gen_cv(
        snapshot_id: [u8; 16],
        produced_ms: u64,
        catalog_version: u64,
    ) -> crate::ai_worker::DecisionInput<'static> {
        let facts: &'static eneros_agent_sdk::snap_input_wire::SnapshotFacts =
            Box::leak(Box::new(eneros_agent_sdk::snap_input_wire::SnapshotFacts {
                snapshot_id,
                provenance_root: [0x3D; 32],
                cut_time_ns: produced_ms.saturating_mul(1_000_000),
                produced_at_ns: produced_ms.saturating_mul(1_000_000),
                catalog_version,
                catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
                accepted: 7,
                missing: 0,
                completeness: eneros_agent_sdk::snap_input_wire::COMPLETE,
            }));
        let t: &'static TrustedTelemetry = Box::leak(Box::new(
            crate::ai_worker::tests_helpers::minimal_trusted_at(produced_ms),
        ));
        crate::ai_worker::DecisionInput {
            telemetry: t,
            facts,
        }
    }

    /// v2 应答（绑定字段与 in-flight 请求一致）。
    fn resp_v2_json(req_id: u64, seq: u64, snapshot_id: [u8; 16], seq_power: (f64, f64)) -> String {
        alloc::format!(
            r#"{{"v":2,"req_id":{req_id},"seq":{seq},"plan":{{"snapshot_id_hex":"{}","catalog_version":3,"horizon_start_ms":1700000000000,"step_ns":60000000000,"step_count":2,"solver_status":"optimal","model_digest_hex":"{}","input_digest_hex":"{}","objective_value":-1.5,"relative_gap":0.0,"elapsed_ms":7,"assignments":[{{"asset_id":1,"device_id":999,"point_id":999,"unit_code":999,"step_index":1,"power_kw":{}}},{{"asset_id":1,"device_id":999,"point_id":999,"unit_code":999,"step_index":2,"power_kw":{}}}],"target_device":"pcs","ttl_secs":60}},"reason":"backend=planner status=optimal"}}"#,
            eneros_intent_contract::peer::hex16(&snapshot_id),
            eneros_intent_contract::peer::hex32(&[0x11; 32]),
            eneros_intent_contract::peer::hex32(&[0x22; 32]),
            seq_power.0,
            seq_power.1
        )
    }

    /// t8（红测先行）：结构化请求发出（v2 wire）→ 绑定应答接纳 → 候选
    /// 携带**全量分配**且身份 = 冻结快照（生成者无法自造身份）。
    /// 变异靶：回到 v1 单值（`plan.power_kw`）→ 本测红（无 2 条分配）。
    #[test]
    fn t8_structured_v2_request_and_acceptance() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000)
            .expect("tick1 兜底");
        // 发出的请求必须是 v2 结构化 schema（含身份/时域/逐资产规格）。
        let sent = b.link.sent.last().expect("必须发出请求").clone();
        assert!(sent.contains(r#""v":2"#), "结构化轨必须发 v2: {sent}");
        assert!(sent.contains(r#""snapshot_id_hex":"5a5a"#));
        assert!(sent.contains(r#""assets""#));
        assert!(sent.contains(r#""prices_per_kwh""#));
        // 绑定应答接纳。
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Accepted
        );
        assert_eq!(b.structured_accepted, 1);
        let wire = b.decide_full(&input, 1_700_000_200).expect("消费");
        let cand = eneros_decision_contract::CandidatePlan::decode_v1(&wire).expect("cand");
        assert_eq!(cand.assignments().len(), 2, "全量分配（无 first 投影）");
        assert_eq!(cand.snapshot_id().0, [0x5A; 16], "身份 = 冻结快照");
        assert_eq!(cand.resource_catalog_version(), 3);
        assert_eq!(cand.step_count(), 2);
        // 绑定字段取自请求（应答回带 999 不可信）。
        assert!(cand
            .assignments()
            .iter()
            .all(|a| a.device_id == 2 && a.point_id == 10 && a.unit_code == 6));
        assert_eq!(b.remote_used_total, 1);
    }

    /// t8b：跨代 v2 应答（快照身份不同）→ 坏应答（零消费）。
    /// 变异靶：去掉 v2 绑定比对 → 跨代应答被接纳 → 本测红。
    #[test]
    fn t8b_structured_v2_cross_generation_rejected() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x77; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Bad
        );
        assert_eq!(b.bad_responses_total, 1);
        assert_eq!(b.structured_accepted, 0);
        // 跨代应答被拒后，在途请求仍在（下次仍可接受正确的应答）。
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 2, [0x5A; 16], (-2.0, 3.0)), 1_700_000_150),
            PeerMsgVerdict::Accepted
        );
    }

    /// t8c：会话重连 → 旧 pending 取消（旧应答不再可采信）。
    /// 变异靶：`on_session_reconnect` 不取走 pending → 重连后旧应答仍被
    /// 接纳 → 本测红。
    #[test]
    fn t8c_reconnect_cancels_pending() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert!(b.on_session_reconnect(), "确有在途 → 取消");
        assert_eq!(b.pending_cancelled, 1);
        // 取消后旧应答 = 迟到（无在途配对），不进决策。
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Late
        );
        assert!(!b.on_session_reconnect(), "无在途 → 不计数");
        assert_eq!(b.pending_cancelled, 1);
    }

    /// t8d：v1 在途收到 v2 应答（协议错位）→ 坏应答（不得混用两种 schema）。
    #[test]
    fn t8d_v2_response_on_v1_pending_rejected() {
        let mut b = brain(); // 无规格来源 → v1 迁移轨
        let t = minimal_trusted_at(1_700_000_000_000);
        b.decide_full(&crate::ai_worker::dinp(&t), 1_700_000_000_000)
            .expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Bad
        );
        assert!(b
            .last_reject_detail
            .as_deref()
            .unwrap_or("")
            .contains("v2 response without bound pending"));
    }

    /// 复查修复（t8d 反向）：**v2 在途收到 v1 应答**（协议错位）→ 坏应答。
    ///
    /// 修复前：v1 分支不校验在途是否有绑定 → 旧 schema 单值提案被当作
    /// "应答"消费（在途被清、seq 前进），真正的 v2 应答随后变 `Late`，
    /// legacy 候选在生产入口（`TrustedInputPolicy::Required`）被拒——
    /// 一次往返被静默作废且记账污染（`remote_used_total` 计入、结构化计数
    /// 不计）。变异靶：v1 分支不看 `pending.binds` → 本测红。
    #[test]
    fn v1_response_on_v2_pending_rejected() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert!(b.pending.is_some(), "结构化请求在途");
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -2.0), 1_700_000_100),
            PeerMsgVerdict::Bad,
            "v2 在途不得接受 v1 应答"
        );
        assert_eq!(b.bad_responses_total, 1);
        assert!(b.pending.is_some(), "错位应答不得清掉在途请求");
        assert_eq!(b.structured_accepted, 0);
        // 真正的 v2 应答仍可接纳（错位应答不污染状态机）。
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 2, [0x5A; 16], (-2.0, 3.0)), 1_700_000_150),
            PeerMsgVerdict::Accepted
        );
    }

    /// R09 激活收口（2026-09-15，DEC-70-13 后继裁决）：**快照代次放宽至
    /// 目录代次**——应答接纳后、消费前快照推进（同目录代次）⇒ 计划
    /// **重锚定**当前代消费：候选身份/时域全取当前输入（非杂交：身份与
    /// 时域一体同代），远程分配值保留；认证器对重锚计划做全量物理复核
    /// （fail-closed 兜底不变），dispatch 入口四字段复核照旧兜底。
    /// 变异靶：消费侧仍按快照逐字节绑定（旧 facts_match_binds）→ 本测红
    /// （stale_generation_accepted_total=1 且候选功率为本地兜底值）。
    #[test]
    fn accepted_v2_plan_rebased_on_same_catalog_generation() {
        use eneros_decision_contract::CandidatePlan;
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input(); // 代 W（[0x5A] / horizon 1_700_000_000_000）
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Accepted
        );
        // 快照换代（新快照/新产刻；同目录代次 3）后消费 ⇒ 重锚。
        let next = input_gen([0x5B; 16], 1_700_000_030_000);
        let w = b
            .decide_full(&next, 1_700_000_030_100)
            .expect("同目录代次必须重锚消费远程计划");
        assert_eq!(b.stale_generation_accepted_total, 0, "同目录代次不再丢弃");
        let cand = CandidatePlan::decode_v1(&w).expect("remote candidate");
        assert_eq!(
            cand.snapshot_id().0,
            [0x5B; 16],
            "候选身份 = 当前输入（重锚非杂交：身份/时域一体当前代）"
        );
        assert_eq!(
            cand.horizon_start_ms(),
            1_700_000_030_000,
            "时域重锚 = 当前快照产刻（dispatch 四字段复核可过）"
        );
        assert!(
            cand.assignments().iter().any(|a| a.power_kw == -2.0),
            "远程分配值必须进入重锚候选：{:?}",
            cand.assignments()
        );
    }

    /// 目录代次（合同授权基础）变化 ⇒ 仍丢弃（fail-closed 不变）。
    /// 变异靶：放宽时把目录代次一并放开 → 本测红。
    #[test]
    fn accepted_v2_plan_dropped_on_catalog_generation_change() {
        use eneros_decision_contract::CandidatePlan;
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Accepted
        );
        // 目录换代（contract 授权基础变了）：远程计划不得消费。
        let next = input_gen_cv([0x5C; 16], 1_700_000_030_000, 4);
        let w = b
            .decide_full(&next, 1_700_000_030_100)
            .expect("目录换代走本地兜底");
        assert_eq!(
            b.stale_generation_accepted_total, 1,
            "目录代次不符仍丢弃可数"
        );
        let cand = CandidatePlan::decode_v1(&w).expect("local candidate");
        assert!(
            cand.assignments().iter().all(|a| a.power_kw != -2.0),
            "目录换代后远程分配不得进入候选"
        );
    }

    /// 68.5 WP-68.5.3 t4（红测先行）：peer 超时 → D3 lease 武装
    /// （LEASE-ARMED 事件 + 同 tick System1 兜底持签成功）；有效应答
    /// 消费 → 撤销（LEASE-REVOKED，回 D0）。变异靶：删 arm_lease_wiring
    /// 调用 → 本测红。
    #[test]
    fn t4_lease_armed_on_degrade_revoked_on_recover() {
        let key = [0x5Bu8; 32];
        let mut b = RemoteBrain::new(
            HeuristicBackend::new(),
            MockLink::ok(),
            cfg_leased(key, 3_600),
        );
        let t = minimal_trusted_at(10_000);
        // tick1：发请求，无超时 → 未武装（healthy 常态 fail-operational
        // 兜底与 68.5 前行为一致）。
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        assert!(!b.local_degradation_armed(), "healthy 常态不武装");
        assert!(b.drain_events().is_empty());
        // tick2：首次超时裁决（16_000 > 10_000+5_000）→ 武装 + 同 tick
        // System1 兜底在 lease 授权下产出（授权逐次记账成功）。
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 16_000)
            .expect("fallback tick");
        assert!(b.local_degradation_armed(), "超时裁决即武装");
        assert!(
            b.drain_events()
                .iter()
                .any(|e| matches!(e, SwitchEvent::LeaseArmed { .. })),
            "须有 LEASE-ARMED 事件"
        );
        let (plan_id, _) = decode_cand(&w);
        assert_eq!(plan_id, "heur-2", "持签兜底照常产出（tick1=heur-1）");
        // 应答到达（超时裁决后已重发 req_id=2——配对走新在途）→ 消费 →
        // 撤销（episode 结束，回 D0）。
        assert_eq!(
            b.on_peer_message(&resp_json(2, 1, -3.0), 16_100),
            PeerMsgVerdict::Accepted
        );
        b.decide_full(&crate::ai_worker::dinp(&t), 17_000)
            .expect("consume");
        assert!(!b.local_degradation_armed(), "有效应答消费即撤销");
        assert!(b.drain_events().contains(&SwitchEvent::LeaseRevoked {
            reason: "peer-accepted"
        }));
    }

    /// 68.5 复查修复 t7（红测先行）：lease 到期后下一超时裁决须复位重
    /// 武装——authorize 对过期 lease 恒 Err 且状态不动，重武装又被
    /// "已武装"守卫短路，peer 长期失联（>TTL）时决策链永久停供（68.5
    /// 前启发式兜底可无限期工作）。变异靶：arm_lease_wiring 去掉过期
    /// 复位分支 → 本测红（tick4 Unavailable）。
    #[test]
    fn t7_lease_expiry_rearms_on_subsequent_timeout() {
        let key = [0x5Bu8; 32];
        let mut b = RemoteBrain::new(
            HeuristicBackend::new(),
            MockLink::ok(),
            cfg_leased(key, 60), // TTL 60s
        );
        let t = minimal_trusted_at(10_000);
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        // tick2：超时裁决 → 武装（有效期 [16s, 76s]）。
        b.decide_full(&crate::ai_worker::dinp(&t), 16_000)
            .expect("armed fallback");
        assert!(b.local_degradation_armed());
        b.drain_events();
        // tick3：仍在期内 → 兜底继续持签产出。
        let w3 = b
            .decide_full(&crate::ai_worker::dinp(&t), 70_000)
            .expect("in-validity fallback");
        assert!(decode_cand(&w3).0.starts_with("heur-"));
        b.drain_events();
        // tick4：已过期（>76s）——复位（reason=lease-expired）+ 重武装，
        // 兜底产出（修复前恒 Unavailable）。
        let w4 = b
            .decide_full(&crate::ai_worker::dinp(&t), 80_000)
            .expect("过期复位重武装后兜底须产出（修复前恒 Unavailable）");
        assert!(decode_cand(&w4).0.starts_with("heur-"));
        let ev = b.drain_events();
        assert!(
            ev.contains(&SwitchEvent::LeaseRevoked {
                reason: "lease-expired"
            }),
            "须有过期复位事件: {ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, SwitchEvent::LeaseArmed { .. })),
            "复位后须重武装: {ev:?}"
        );
        // tick5：新 episode 有效期内（80s 起算）——兜底继续产出。
        b.decide_full(&crate::ai_worker::dinp(&t), 100_000)
            .expect("new-episode fallback");
    }

    /// 68.5：未装配键（全零）不武装——行为与 68.5 前逐字节一致。
    #[test]
    fn t4b_lease_unconfigured_never_arms() {
        let mut b = brain(); // lease_key 全零
        let t = minimal_trusted_at(10_000);
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        b.decide_full(&crate::ai_worker::dinp(&t), 16_000)
            .expect("timeout tick");
        assert!(!b.local_degradation_armed());
        assert!(b.drain_events().is_empty(), "无武装事件");
    }

    /// WP-68.5.2 t3（红测先行）：三后端审计词形可区分 + 64 字符截断前缀
    /// 内标签恒可读 + build_plan 诚实标注（status/model_digest 按 reason
    /// 词形映射，不再硬编码伪造）。变异靶：backend_tag 把 planner 标成
    /// remote-llm → 本测红。
    #[test]
    fn t3_backend_tag_honest_audit_forms() {
        let t = minimal_trusted_at(10_000);
        let model_hex = eneros_hex::encode_lower(&[0x5Au8; 32]);

        // ── planner 形态：reason 带词形前缀 ──
        let mut b = brain();
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        assert_eq!(
            b.on_peer_message(
                &resp_json_reason(
                    1,
                    1,
                    -3.0,
                    &alloc::format!("backend=planner status=optimal model={model_hex}")
                ),
                10_500
            ),
            PeerMsgVerdict::Accepted
        );
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 11_000)
            .expect("consume");
        let meta = b.last_meta().expect("meta");
        assert!(
            meta.resp_summary.starts_with("[backend=planner] "),
            "标签前置可读：{}",
            meta.resp_summary
        );
        assert!(
            meta.resp_summary.chars().count() <= 65,
            "审计截断维持（64B + 省略号）：{}",
            meta.resp_summary
        );
        let cand = eneros_decision_contract::CandidatePlan::decode_v1(&w).expect("cand");
        assert_eq!(
            cand.solver_status(),
            eneros_decision_contract::SolveStatusKind::Optimal,
            "status=optimal 诚实透传"
        );
        assert_eq!(cand.model_digest(), &[0x5Au8; 32], "model digest 透传");

        // ── remote-llm 形态：普通 reason ──
        let mut b2 = brain();
        b2.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        assert_eq!(
            b2.on_peer_message(&resp_json_reason(1, 2, 2.0, "price arb"), 10_500),
            PeerMsgVerdict::Accepted
        );
        let w2 = b2
            .decide_full(&crate::ai_worker::dinp(&t), 11_000)
            .expect("consume");
        let meta2 = b2.last_meta().expect("meta");
        assert!(meta2.resp_summary.starts_with("[backend=remote-llm] "));
        let cand2 = eneros_decision_contract::CandidatePlan::decode_v1(&w2).expect("cand2");
        assert_eq!(
            cand2.solver_status(),
            eneros_decision_contract::SolveStatusKind::Suboptimal,
            "无声明不升格"
        );
        assert!(
            cand2.model_digest().iter().all(|&b| b == 0),
            "无模型形态 digest = 零（adapter 既有语义）"
        );

        // ── heuristic（System1）：used_remote=false，独立词形 ──
        let mut b3 = brain();
        b3.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("sys1 tick");
        let meta3 = b3.last_meta().expect("meta3");
        assert!(!meta3.used_remote);
        assert_eq!(meta3.resp_summary, "");
    }

    /// backend_tag / reason 字段解析单源单测（词形漂移即红）。
    #[test]
    fn t3b_backend_tag_parsing_unit() {
        assert_eq!(backend_tag("backend=planner status=optimal"), "planner");
        assert_eq!(backend_tag("backend=plannerX"), "planner", "前缀匹配");
        assert_eq!(backend_tag("soc low"), "remote-llm");
        assert_eq!(backend_tag(""), "remote-llm");
        assert_eq!(
            reason_field("backend=planner status=optimal m=1", "status"),
            Some("optimal")
        );
        assert_eq!(
            reason_field("a=1 status=suboptimal", "status"),
            Some("suboptimal")
        );
        assert_eq!(reason_field("no fields", "status"), None);
        assert_eq!(reason_model_digest("model=00ff"), None, "形不符 = None");
        assert_eq!(
            reason_model_digest(&alloc::format!(
                "backend=planner status=optimal model={}",
                eneros_hex::encode_lower(&[7u8; 32])
            )),
            Some([7u8; 32])
        );
    }

    #[test]
    fn happy_path_remote_proposal() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        // tick1：Healthy 常态发起请求（req_id=1）+ 本 tick 启发式兜底。
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("wire");
        let (plan_id, power) = decode_cand(&w);
        assert_eq!(plan_id, "heur-1");
        assert!(power.is_finite());
        assert_eq!(b.triggers_sent, 1);
        assert!(b.link.sent[0].contains("\"req_id\":1"));
        // 应答到达（配对 req=1）。
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -3.0), 10_500),
            PeerMsgVerdict::Accepted
        );
        // tick2：消费远程提案。
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 11_000)
            .expect("wire");
        let (plan_id, power) = decode_cand(&w);
        assert_eq!(plan_id, "remote-1");
        assert_eq!(power, -3.0);
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
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        // 超时后 tick：pending 清除 + 计数 + 本 tick 走启发式。
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 16_000)
            .expect("tick2");
        let (_plan_id, _power) = decode_cand(&w);
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
            b.decide_full(&crate::ai_worker::dinp(&t), now)
                .expect("tick");
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
            b.decide_full(&crate::ai_worker::dinp(&t), now)
                .expect("degrade ticks");
        }
        assert!(!b.is_healthy());
        let sent_after_unhealthy = b.triggers_sent;
        // Unhealthy：非探测 tick（ticks=5，5%3≠0）不发请求。
        now += 6_000;
        b.decide_full(&crate::ai_worker::dinp(&t), now)
            .expect("tick");
        assert_eq!(
            b.triggers_sent, sent_after_unhealthy,
            "非探测 tick 不得发请求"
        );
        // 探测 tick（ticks=6，6%3=0）发请求。
        now += 6_000;
        b.decide_full(&crate::ai_worker::dinp(&t), now)
            .expect("probe tick");
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
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), now)
            .expect("tick");
        let (plan_id, _) = decode_cand(&w);
        assert_eq!(plan_id, alloc::format!("remote-{probe_req}"));
        assert_eq!(b.remote_used_total, 1);
        assert!(b.is_healthy());
    }

    #[test]
    fn over_budget_response_rejected_as_bad() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        // 越权提案（|power| > 5kW 预算）→ Bad + 不消费 + 继续降级。
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -5.5), 10_500),
            PeerMsgVerdict::Bad
        );
        assert_eq!(b.bad_responses_total, 1);
        assert!(b.accepted.is_none());
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 11_000)
            .expect("tick2");
        let (plan_id, _) = decode_cand(&w);
        assert_eq!(plan_id, "heur-2", "坏应答不得进入决策（本地启发式兜底）");
    }

    #[test]
    fn replay_and_regression_seq_rejected() {
        let mut b = brain();
        let t = minimal_trusted_at(10_000);
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_json(1, 4, -3.0), 10_500),
            PeerMsgVerdict::Accepted
        );
        // tick2 消费远程提案（消费后同 tick 不再发新请求）。
        b.decide_full(&crate::ai_worker::dinp(&t), 11_000)
            .expect("consume");
        // tick3 发起新请求（req=2）。
        b.decide_full(&crate::ai_worker::dinp(&t), 12_000)
            .expect("tick3");
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
        b.decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("tick1");
        // 超时清除在途后，旧 req 的应答才到达 → Late（非协议违规）。
        b.decide_full(&crate::ai_worker::dinp(&t), 16_000)
            .expect("expire tick");
        assert_eq!(
            b.on_peer_message(&resp_json(1, 1, -3.0), 16_100),
            PeerMsgVerdict::Late
        );
        assert_eq!(b.late_total, 1);
        assert_eq!(b.bad_responses_total, 0);
        // 无在途时的主动推送同样 Late。
        b.decide_full(&crate::ai_worker::dinp(&t), 17_000)
            .expect("tick");
        b.decide_full(&crate::ai_worker::dinp(&t), 18_000)
            .expect("tick");
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
        let w = b
            .decide_full(&crate::ai_worker::dinp(&t), 10_000)
            .expect("wire");
        let (_plan_id, _power) = decode_cand(&w);
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
        match b.decide_full(&crate::ai_worker::dinp(&t), 10_000) {
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
            b.decide_full(&crate::ai_worker::dinp(&t), now)
                .expect("tick");
            assert_eq!(
                b.on_peer_message(&resp_json(i, i, -1.0), now + 100),
                PeerMsgVerdict::Accepted
            );
            now += 1_000;
            let w = b
                .decide_full(&crate::ai_worker::dinp(&t), now)
                .expect("tick");
            ids.push(decode_cand(&w).0);
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

    /// 复查修复：**规格源拒绝本代次时不发任何请求**（fail-closed）。
    /// 修复前落到 v1 迁移轨——v1 产物在生产 `Required` 策略下必被 dispatch
    /// 拒，白烧往返与 EA 错误预算，且违背"宁可不决策"的文档承诺。
    ///
    /// 变异靶：把 `return self.fallback_tick(...)` 移回 `Some(specs)` 分支
    /// 内 → 本测红（link.sent 非空 = 发了 v1）。
    #[test]
    fn structured_source_none_sends_nothing() {
        struct NoSpecs;
        impl PlanSpecSource for NoSpecs {
            fn specs(
                &mut self,
                _facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
                _soc_pct: f64,
            ) -> Option<PlanSpecs> {
                None // 目录代次不符/无可控资产/价格或预算不可用
            }
        }
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(NoSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("兜底");
        assert!(
            b.link.sent.is_empty(),
            "规格源拒绝 ⇒ 不得发任何请求（含 v1 迁移轨），实发 {} 条",
            b.link.sent.len()
        );
        assert_eq!(b.triggers_sent, 0);
        assert_eq!(
            b.pending_cancelled + u64::from(b.pending.is_some()),
            0,
            "无在途"
        );
    }

    /// 70.14 复查红测：**可信输入已绑定但本机没有规格源**（`PLAN-SPEC-NONE`，
    /// 当前生产默认装配）⇒ 不得发 v1 请求。
    ///
    /// 缺陷形态（旧实现）：只有 `Some(src)/None` 那条臂 fail-closed，`spec_source
    /// 整体缺失`直接落到 v1 发送——而 v1 产物是伪身份候选（`snapshot_id` =
    /// 请求摘要前 16B、`objective_digest` = 0），`Required` 准入必拒 ⇒ 每轮白烧
    /// 一次往返 + EA 记 `DispatchRejected`；连续 3 次触发 `Escalated` + 30s
    /// 冷却，冷却期连 System1 启发式候选也被压制（控制面间歇停摆）。
    #[test]
    fn bound_input_without_spec_source_sends_nothing() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        assert!(b.spec_source.is_none(), "本测前提 = 无规格源");
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("兜底");
        assert!(
            b.link.sent.is_empty(),
            "无结构化规格源 ⇒ 不得发 v1（产物必被 Required 拒），实发 {} 条",
            b.link.sent.len()
        );
        assert_eq!(b.triggers_sent, 0);
        assert!(b.pending.is_none(), "不得留下在途 v1 请求");
    }

    /// 反向锁：**未绑定**输入（v1 迁移轨）仍照发——本次修复不关掉迁移轨。
    #[test]
    fn unbound_input_still_uses_the_v1_migration_track() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        let t = crate::ai_worker::tests_helpers::minimal_trusted_at(10_000);
        let input = crate::ai_worker::dinp(&t);
        assert!(!input.bound(), "本测前提 = 未绑定输入");
        b.decide_full(&input, 10_000).expect("兜底");
        assert_eq!(b.triggers_sent, 1, "未绑定输入仍走 v1 迁移轨");
        assert!(!b.link.sent.is_empty());
    }

    /// 复查修复：v2 应答必须**覆盖全部请求资产**（"全量分配"判据本体）。
    /// 缺任一请求资产 = 部分计划被记为全量 ⇒ 坏应答（不做部分采纳）。
    ///
    /// 变异靶：去掉 `missing_response_assets` 检查 → 本测红（应答被接纳）。
    #[test]
    fn v2_response_missing_requested_asset_is_bad() {
        struct TwoAssets;
        impl PlanSpecSource for TwoAssets {
            fn specs(
                &mut self,
                _facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
                _soc_pct: f64,
            ) -> Option<PlanSpecs> {
                let mut p = StubSpecs.specs(_facts, _soc_pct)?;
                p.assets.push(eneros_intent_contract::peer::PeerAssetSpec {
                    asset_id: 2,
                    capacity_kwh: 8.0,
                    eta_c: 0.95,
                    eta_d: 0.95,
                    initial_kwh: 4.0,
                    soc_min: 0.05,
                    soc_max: 0.95,
                    p_min_kw: -4.0,
                    p_max_kw: 4.0,
                    ramp_up_kw_s: 1e6,
                    ramp_down_kw_s: 1e6,
                    device_id: 3,
                    point_id: 11,
                    unit_code: 6,
                });
                p.step_count = 1;
                p.prices_per_kwh = alloc::vec![1.0];
                Some(p)
            }
        }
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(TwoAssets));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        // 应答只覆盖 asset 1（缺 asset 2）→ 坏应答。
        let one_asset = r#"{"v":2,"req_id":1,"seq":1,"plan":{"snapshot_id_hex":"5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a","catalog_version":3,"horizon_start_ms":1700000000000,"step_ns":60000000000,"step_count":1,"solver_status":"optimal","model_digest_hex":"1111111111111111111111111111111111111111111111111111111111111111","input_digest_hex":"2222222222222222222222222222222222222222222222222222222222222222","objective_value":-1.5,"relative_gap":0.0,"elapsed_ms":7,"assignments":[{"asset_id":1,"device_id":999,"point_id":999,"unit_code":999,"step_index":1,"power_kw":-2.0}],"target_device":"pcs","ttl_secs":60},"reason":"backend=planner status=optimal"}"#;
        assert_eq!(
            b.on_peer_message(one_asset, 1_700_000_100),
            PeerMsgVerdict::Bad,
            "缺请求资产的应答必须拒绝"
        );
        assert!(b
            .last_reject_detail
            .as_deref()
            .unwrap_or("")
            .contains("misses requested asset"));
    }

    /// 复查修复：v2 审计标签取自应答 `reason`（`backend=planner`）——
    /// 原实现用 `solver_status` 当替身 ⇒ `backend_tag` 恒 `remote-llm`，
    /// planner 与 LLM 在审计面不可区分。
    #[test]
    fn v2_decision_meta_tags_planner_from_response_reason() {
        let mut b = RemoteBrain::new(HeuristicBackend::new(), MockLink::ok(), cfg());
        b.set_plan_spec_source(Box::new(StubSpecs));
        let input = bound_input();
        b.decide_full(&input, 1_700_000_000_000).expect("tick1");
        assert_eq!(
            b.on_peer_message(&resp_v2_json(1, 1, [0x5A; 16], (-2.0, 3.0)), 1_700_000_100),
            PeerMsgVerdict::Accepted
        );
        let _ = b.decide_full(&input, 1_700_000_150).expect("消费");
        let meta = b.last_meta().expect("meta");
        assert!(
            meta.resp_summary.contains("[backend=planner]"),
            "v2 审计标签必须识别 planner：{}",
            meta.resp_summary
        );
    }
}
