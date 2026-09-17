//! eneros-planner-peer —— 蓝图 68.5 WP-68.5.1：host planner 分区第一形态。
//!
//! 站端服务器/上位机常驻服务：订阅 agent 侧 broker 的 `ai_request_topic`
//! （RemoteBrain 6 字段投影请求，QoS1），LP 建模 → 真 HiGHS 求解 → 发布
//! PeerResponse JSON 到 `ai_response_topic`（QoS1）。**单在途槽**：单线程
//! 循环逐请求应答（F27 第 11 项）；`seq` 单调经状态文件跨重启持久
//! （peer 重启后 seq 回退 = 目标侧重放拒绝——状态文件消除该面）。
//!
//! QEMU 形态：对 evidence/50 python 假 broker（slirp 10.0.2.2 = host
//! 回环）；真机形态：对站端 mosquitto。部署红线（ADR-050:11-13）：
//! 本 bin 不持有 protocol writer / Gateway key / seL4 capability。
//!
//! 词形（QEMU gate/host 对账锚）：
//! `READY` / `SUBSCRIBED` / `PEER-REQ` / `PLANNER-SOLVE` / `PEER-RESP` /
//! `PLANNER-SKIP` / `RECONNECT`。求解失败**不应答**（目标侧超时降级，
//! fail-operational 既有语义）。
//!
//! 69.2：会话生命周期（建联/keepalive/静默看门狗/重连/主题过滤/seq）
//! 收进 [`session::PeerSession`] 深模块——本文件只剩装配 + 求解编排。

use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

use eneros_intent_contract::peer::{
    parse_and_validate, parse_and_validate_v2, PeerRequest, PeerRequestV2, PlanBinds,
    ResponseChecks, PEER_CONTRACT_VERSION_V2,
};
use eneros_planner::tolerance::ToleranceProfile;
use eneros_planner::Planner;
use eneros_planner_peer::{PeerPlannerConfig, BACKEND_TAG};
use eneros_solver_core::highs::HighsSolver;

mod mqtt;
mod session;
use session::{PeerEvent, PeerSession, RespondError};

const DEFAULT_REQUEST_TOPIC: &str = "eneros/brain/request";
const DEFAULT_RESPONSE_TOPIC: &str = "eneros/brain/response";
const DEFAULT_BROKER: &str = "127.0.0.1:1883";
const DEFAULT_CLIENT_ID: &str = "eneros-planner-peer";

struct Args {
    broker: String,
    config_path: String,
    state_path: String,
    request_topic: String,
    response_topic: String,
    client_id: String,
    /// REG-70.15-R09R10-GATE-DESIGN：负例注入模式（R09 专属门；
    /// 正常求解后按模式变异应答——目标侧契约/认证层必须 fail-closed）。
    negative: Option<NegativeMode>,
    /// 轮转模式：单实例免重启，每 4 响应注入一个负例（四模式轮转）。
    negative_cycle: bool,
}

/// 负例变异模式（R09：跨代/过期/约束变异/伪关联——四层防御各咬一口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NegativeMode {
    /// ttl_secs = 0（超出 [1, budget.ttl_secs] → 契约 Range 拒绝）。
    Expired,
    /// snapshot_id_hex 归零（绑定代次不符 → 契约 Range 拒绝）。
    Crossgen,
    /// 首分配功率越预算钳制（> budget.max_power_kw → 契约预算拒绝）。
    Mutated,
    /// req_id 错位（伪造关联 → Correlation → Late 丢弃，绝不消费）。
    Unsigned,
}

impl NegativeMode {
    /// 四模式轮转序（`--negative-cycle`：单实例免重启覆盖全部负例）。
    /// 周期 8：**首 4 响应 = 四模式各一**（蓝图 70 R10 会话前置——broker→
    /// target 应答投递有损且随窗内时间恶化（REG-70.15-R09R10 投递有损治理：
    /// 首周期 2/2 命中、次周期 0/2），每模式的首注入必须落在链路最新鲜的
    /// 前四拍，判据（每模式目标侧拒绝词形 ≥1）才能脱离投递抽奖）；后 4
    /// 响应正常（链路保持活性）。
    pub const CYCLE: [Option<NegativeMode>; 8] = [
        Some(NegativeMode::Expired),
        Some(NegativeMode::Mutated),
        Some(NegativeMode::Crossgen),
        Some(NegativeMode::Unsigned),
        None,
        None,
        None,
        None,
    ];
}

impl NegativeMode {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "expired" => Some(Self::Expired),
            "crossgen" => Some(Self::Crossgen),
            "mutated" => Some(Self::Mutated),
            "unsigned" => Some(Self::Unsigned),
            _ => None,
        }
    }
    fn word(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Crossgen => "crossgen",
            Self::Mutated => "mutated",
            Self::Unsigned => "unsigned",
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args {
        broker: String::from(DEFAULT_BROKER),
        config_path: String::new(),
        state_path: String::new(),
        request_topic: String::from(DEFAULT_REQUEST_TOPIC),
        response_topic: String::from(DEFAULT_RESPONSE_TOPIC),
        client_id: String::from(DEFAULT_CLIENT_ID),
        negative: None,
        negative_cycle: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        macro_rules! val {
            () => {
                it.next()
                    .unwrap_or_else(|| exit_with_usage(&format!("{arg} 需要值")))
            };
        }
        match arg.as_str() {
            "--broker" => a.broker = val!(),
            "--config" => a.config_path = val!(),
            "--state" => a.state_path = val!(),
            "--request-topic" => a.request_topic = val!(),
            "--response-topic" => a.response_topic = val!(),
            "--client-id" => a.client_id = val!(),
            "--negative" => {
                let v = val!();
                a.negative = Some(
                    NegativeMode::parse(&v)
                        .unwrap_or_else(|| exit_with_usage(&format!("--negative 未知模式 {v}"))),
                );
            }
            "--negative-cycle" => a.negative_cycle = true,
            other => exit_with_usage(&format!("未知参数 {other}")),
        }
    }
    if a.config_path.is_empty() || a.state_path.is_empty() {
        exit_with_usage("--config 与 --state 必配");
    }
    a
}

/// 退出码表（69.2 O5 全量穷举：`rg "exit\(" crates/ai/planner-peer/src/`，
/// 共 2 处调用点、1 个码）——**2** = 需运维干预的致命错（用法/配置错与
/// seq 状态损坏同码，stderr 词形区分：用法文案 / `SEQ-STATE-CORRUPT`）。
/// 无其他退出码（0 = 进程正常退出，未显式使用）。
fn exit_with_usage(msg: &str) -> ! {
    eprintln!(
        "planner-peer: {msg}\n用法：eneros-planner-peer --config <路径/to/<站点配置>.toml> \
         --state <seq-state.json> [--broker 127.0.0.1:1883] \
         [--request-topic eneros/brain/request] [--response-topic eneros/brain/response] \
         [--client-id eneros-planner-peer]"
    );
    exit(2);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn main() {
    let args = parse_args();
    let cfg_text = std::fs::read_to_string(&args.config_path)
        .unwrap_or_else(|e| exit_with_usage(&format!("读 config: {e}")));
    let cfg = PeerPlannerConfig::parse(&cfg_text)
        .unwrap_or_else(|e| exit_with_usage(&format!("config 解析: {e}")));

    let negative = args.negative;
    let negative_cycle = args.negative_cycle;
    if negative_cycle {
        println!("[planner-peer] NEGATIVE-CYCLE armed（每 4 响应轮转一负例）");
    }
    if let Some(mode) = negative {
        println!(
            "[planner-peer] NEGATIVE-ARMED mode={}（R09 专属门负例注入）",
            mode.word()
        );
    }
    println!(
        "[planner-peer] READY broker={} request={} response={} battery={}kWh@{}kW steps={} kind={} state={}",
        args.broker,
        args.request_topic,
        args.response_topic,
        cfg.battery.capacity_kwh,
        cfg.battery.power_max_kw,
        cfg.objective.steps,
        cfg.objective.kind,
        args.state_path,
    );

    // 求解器常驻（同进程复用；L2 门只收 Optimal——ADR-050 求解门）。
    let mut planner = Planner::new(ToleranceProfile::l2(), 100);
    let mut highs =
        HighsSolver::new().unwrap_or_else(|e| exit_with_usage(&format!("HiGHS 初始化失败: {e}")));
    let mut session = PeerSession::new(args).unwrap_or_else(|e| exit_with_usage(&e));
    loop {
        match session.next_event() {
            Some(PeerEvent::Request { json }) => {
                handle_request(
                    &json,
                    &cfg,
                    negative,
                    negative_cycle,
                    &mut planner,
                    &mut highs,
                    &mut session,
                );
            }
            // 重连词形已在会话内落——观测事件，无处置。
            Some(PeerEvent::Reconnected { .. }) | None => {}
        }
    }
}

/// 单请求处理：解析 → 求解 → 组装 → 发布。任何失败 = PLANNER-SKIP +
/// **不应答**（目标侧超时降级——诚实语义）。发布失败由
/// [`PeerSession::respond`] 内部重连（publish-failed 词形在会话侧落）。
fn handle_request<T: crate::mqtt::PeerTransport>(
    json: &str,
    cfg: &PeerPlannerConfig,
    negative: Option<NegativeMode>,
    negative_cycle: bool,
    planner: &mut Planner,
    highs: &mut HighsSolver,
    session: &mut PeerSession<T>,
) {
    // 版本判别：**先探 `v` 再分派**（v1/v2 schema 不同——v1 解析器带
    // `deny_unknown_fields` 且要求 `telemetry`，v2 请求在 v1 解析器下
    // 必然失败，靠解析失败猜版本无法区分"版本不同"与"真坏包"）。
    //
    // 蓝图 70.10 复查修复：本 bin 原只走 v1 ——目标侧一旦配置结构化计划
    // 参数（`ai-peer` 字段 9）就发 v2 请求，host 侧恒 PLANNER-SKIP，
    // 目标每周期超时降级（结构化通道**端到端不可用**）。v2 与 v1 共用
    // 同一求解器常驻实例（seq/状态文件面同源）。
    match route_version(json) {
        VersionRoute::V2 => {
            handle_request_v2(json, negative, negative_cycle, planner, highs, session);
            return;
        }
        VersionRoute::V1 => {}
        VersionRoute::Unsupported(other) => {
            println!("[planner-peer] PLANNER-SKIP reason=unsupported-version ({other})");
            return;
        }
        VersionRoute::Malformed(e) => {
            println!("[planner-peer] PLANNER-SKIP reason=parse ({e})");
            return;
        }
    }
    // 不受信输入：schema/域全检在 solve_request 内（求解权威域检）；此处
    // 仅反序列化预检——坏请求不值得一次求解。
    let req: PeerRequest = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(e) => {
            println!("[planner-peer] PLANNER-SKIP reason=parse ({e})");
            return;
        }
    };
    println!(
        "[planner-peer] PEER-REQ {}",
        eneros_planner_peer::request_summary(&req)
    );
    let (power, meta) =
        match eneros_planner_peer::solve_request(cfg, planner, highs, &req, now_ms()) {
            Ok(out) => out,
            Err(e) => {
                println!("[planner-peer] PLANNER-SKIP reason={}", e.0);
                return;
            }
        };
    println!(
        "[planner-peer] PLANNER-SOLVE status={} power={power:.6}kW model={} elapsed={}ms obj={:.4}",
        meta.status_word, meta.model_digest_hex, meta.elapsed_ms, meta.objective_value
    );
    // 蓝图 70.4（WP-70.4.3）：seq **先持久预留再发布**（respond_reserved
    // 单点承载）——组装/自检/发布任一失败都产生有解释的序号间隙并如实
    // 记账（PEER-RESP-ABANDONED）；持久失败 = 不发布（fail-closed）。
    // 观测字段（字节数/backend-tag）经 RefCell 由 compose 闭包带出。
    let meta_out: std::cell::RefCell<Option<(usize, bool)>> = std::cell::RefCell::new(None);
    let seqn = match session.respond_reserved(|seq| {
        let resp = eneros_planner_peer::build_response(
            req.req_id,
            seq,
            power,
            req.budget.ttl_secs,
            &cfg.target_device,
            &meta,
        );
        let out = serde_json::to_string(&resp).map_err(|e| format!("encode: {e}"))?;
        // 出站自检：目标侧 parse_and_validate 同链预演（t1 语义的运行时
        // 面；失败不发布——发一条必被拒的应答只会烧目标侧 bad 计数）。
        // 配对（expect_req_id）本地可判；seq 递增判据目标侧会话状态 peer
        // 不可知——置 None 只查本地可判定域。
        let checks = ResponseChecks {
            expect_req_id: req.req_id,
            expect_seq_gt: None,
            budget: &req.budget,
        };
        parse_and_validate(&out, &checks).map_err(|e| format!("self-check: {e:?}"))?;
        *meta_out.borrow_mut() = Some((out.len(), resp.reason.starts_with(BACKEND_TAG)));
        Ok(out.into_bytes())
    }) {
        Ok(seqn) => seqn,
        Err(RespondError::Compose(e)) => {
            println!("[planner-peer] PEER-RESP-ABANDONED reason={e}（序号间隙，如实记账）");
            return;
        }
        Err(RespondError::Persist(e)) => {
            println!("[planner-peer] PLANNER-SKIP reason=seq-persist ({e})——未发布");
            return;
        }
        Err(RespondError::Publish(e)) => {
            println!("[planner-peer] PLANNER-SKIP reason=publish ({e})");
            return;
        }
    };
    let (bytes, tag_ok) = meta_out.into_inner().unwrap_or((0, false));
    println!("[planner-peer] PEER-RESP seq={seqn} bytes={bytes} backend-tag-ok={tag_ok}");
}

/// 请求版本路由（**单点**；复查修复：v2 必须在此可达，否则目标侧配置
/// 结构化计划参数后 host 恒 SKIP = 通道端到端不可用）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionRoute {
    /// v1 迁移轨（单值提案）。
    V1,
    /// v2 结构化全计划。
    V2,
    /// 其他版本号（闭集之外——不应答）。
    Unsupported(u64),
    /// 无法解析/无 `v` 字段。
    Malformed(String),
}

/// `v` 探测与路由判定（纯函数；bin 单测锁在此）。
fn route_version(json: &str) -> VersionRoute {
    #[derive(serde::Deserialize)]
    struct VersionProbe {
        v: u64,
    }
    match serde_json::from_str::<VersionProbe>(json) {
        Ok(p) if p.v == PEER_CONTRACT_VERSION_V2 => VersionRoute::V2,
        Ok(p) if p.v == 1 => VersionRoute::V1,
        Ok(p) => VersionRoute::Unsupported(p.v),
        Err(e) => VersionRoute::Malformed(e.to_string()),
    }
}

/// 结构化（v2）请求处理（蓝图 70.10）：解析 → 求解 → 组装 → 自检 → 发布。
///
/// 与 v1 同律：任何失败 = PLANNER-SKIP + **不应答**（目标侧超时降级），
/// 发布失败由 `respond_reserved` 内部重连。出站自检走目标侧同链
/// （`parse_and_validate_v2` + 请求侧绑定）——发一条必被拒的应答只会烧
/// 目标侧坏应答计数。
fn handle_request_v2<T: crate::mqtt::PeerTransport>(
    json: &str,
    negative: Option<NegativeMode>,
    negative_cycle: bool,
    planner: &mut Planner,
    highs: &mut HighsSolver,
    session: &mut PeerSession<T>,
) {
    let req: PeerRequestV2 = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(e) => {
            println!("[planner-peer] PLANNER-SKIP reason=parse-v2 ({e})");
            return;
        }
    };
    println!(
        "[planner-peer] PEER-REQ-V2 req_id={} assets={} steps={} prices={} objective={}",
        req.req_id,
        req.assets.len(),
        req.step_count,
        req.prices_per_kwh.len(),
        req.objective_kind
    );
    let (plan, meta) = match eneros_planner_peer::solve_request_v2(planner, highs, &req, now_ms()) {
        Ok(out) => out,
        Err(e) => {
            println!("[planner-peer] PLANNER-SKIP reason={}", e.0);
            return;
        }
    };
    println!(
        "[planner-peer] PLANNER-SOLVE-V2 status={} assignments={} model={} elapsed={}ms obj={:.4}",
        meta.status_word,
        plan.assignments.len(),
        meta.model_digest_hex,
        meta.elapsed_ms,
        meta.objective_value
    );
    // 请求侧绑定（自检用；与目标侧 `PendingReq.binds` 同源公式）。
    let sid = match eneros_hex::decode_vec(&req.snapshot_id_hex) {
        Some(v) if v.len() == 16 => v,
        _ => {
            println!("[planner-peer] PLANNER-SKIP reason=snapshot_id_hex");
            return;
        }
    };
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(&sid);
    let binds = PlanBinds {
        snapshot_id,
        catalog_version: req.catalog_version,
        horizon_start_ms: req.horizon_start_ms,
        step_ns: req.step_ns,
        step_count: req.step_count,
    };
    let req_id = req.req_id;
    let meta_out: std::cell::RefCell<Option<(usize, bool)>> = std::cell::RefCell::new(None);
    let seqn = match session.respond_reserved(|seq| {
        let mut resp = eneros_planner_peer::build_response_v2(req_id, seq, plan.clone(), &meta);
        // 轮转模式：单实例免重启，每 4 响应注入一个负例（四模式轮转）。
        let mode = negative.or(if negative_cycle {
            NegativeMode::CYCLE[(seq as usize) % NegativeMode::CYCLE.len()]
        } else {
            None
        });
        if let Some(mode) = mode {
            // R09 专属门负例注入：正常求解后按模式变异，**跳过宿主自检**
            //（负例自证必败——发布它正是门的受试对象）。目标侧契约/认证
            // 层必须 fail-closed：PEER-V2-REJECT / late 计数 / 零误受理。
            match mode {
                NegativeMode::Expired => resp.plan.ttl_secs = 0,
                NegativeMode::Crossgen => resp.plan.snapshot_id_hex = "0".repeat(32),
                NegativeMode::Mutated => {
                    if let Some(a) = resp.plan.assignments.first_mut() {
                        a.power_kw = req.budget.max_power_kw + 2.0;
                    }
                }
                NegativeMode::Unsigned => {
                    resp.req_id = resp.req_id.wrapping_add(0x9E37_79B9_7F4A_7C15)
                }
            }
            println!(
                "[planner-peer] PEER-NEGATIVE mode={} injected（目标侧必须拒绝）",
                mode.word()
            );
            let bytes = serde_json::to_vec(&resp).map_err(|e| format!("encode: {e}"))?;
            return Ok(bytes);
        }
        let out = serde_json::to_string(&resp).map_err(|e| format!("encode: {e}"))?;
        let checks = ResponseChecks {
            expect_req_id: req_id,
            expect_seq_gt: None,
            budget: &req.budget,
        };
        parse_and_validate_v2(&out, &checks, &binds).map_err(|e| format!("self-check: {e:?}"))?;
        *meta_out.borrow_mut() = Some((out.len(), resp.reason.starts_with(BACKEND_TAG)));
        Ok(out.into_bytes())
    }) {
        Ok(seqn) => seqn,
        Err(RespondError::Compose(e)) => {
            println!("[planner-peer] PEER-RESP-ABANDONED reason={e}（序号间隙，如实记账）");
            return;
        }
        Err(RespondError::Persist(e)) => {
            println!("[planner-peer] PLANNER-SKIP reason=seq-persist ({e})——未发布");
            return;
        }
        Err(RespondError::Publish(e)) => {
            println!("[planner-peer] PLANNER-SKIP reason=publish ({e})");
            return;
        }
    };
    let (bytes, tag_ok) = meta_out.into_inner().unwrap_or((0, false));
    println!("[planner-peer] PEER-RESP-V2 seq={seqn} bytes={bytes} backend-tag-ok={tag_ok}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 蓝图 70.10 复查修复锁：**v2 请求必须被路由到结构化处理器**。
    ///
    /// 缺陷形态（修复前）：`handle_request` 只解析 v1 → 目标侧配置
    /// `ai-peer` 计划参数后发出的 v2 请求恒 `PLANNER-SKIP reason=parse`
    /// （v1 schema 要求 `telemetry`/拒绝未知字段）→ host 永不应答 →
    /// 目标每周期超时降级 = 结构化通道端到端不可用。
    ///
    /// 变异靶：删 `Route::V2` 臂（或把 v2 归入 V1）→ 本测红。
    #[test]
    fn v2_request_routes_to_structured_handler() {
        let v1 = r#"{"v":1,"req_id":7,"seq":0,"telemetry":{"soc_pct":0.5},"budget":{"max_power_kw":5.0,"ttl_secs":60},"assets":[]}"#;
        let v2 = r#"{"v":2,"req_id":7,"snapshot_id_hex":"5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"}"#;
        assert_eq!(route_version(v1), VersionRoute::V1, "v1 迁移轨");
        assert_eq!(
            route_version(v2),
            VersionRoute::V2,
            "v2 结构化请求必须路由（修复前 v2 请求在 v1 解析器下必失败）"
        );
        assert_eq!(
            route_version(r#"{"v":3,"req_id":1}"#),
            VersionRoute::Unsupported(3),
            "闭集外版本不得猜"
        );
        assert!(matches!(
            route_version("not json"),
            VersionRoute::Malformed(_)
        ));
    }
}
