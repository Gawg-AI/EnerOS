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

use std::process::exit;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eneros_intent_contract::peer::{parse_and_validate, PeerRequest, ResponseChecks};
use eneros_planner::tolerance::ToleranceProfile;
use eneros_planner::Planner;
use eneros_planner_peer::{PeerPlannerConfig, BACKEND_TAG};
use eneros_solver_core::highs::HighsSolver;

mod mqtt;
use mqtt::MqttClient;

const DEFAULT_REQUEST_TOPIC: &str = "eneros/brain/request";
const DEFAULT_RESPONSE_TOPIC: &str = "eneros/brain/response";
const DEFAULT_BROKER: &str = "127.0.0.1:1883";
const DEFAULT_CLIENT_ID: &str = "eneros-planner-peer";
const POLL_TICK_MS: u64 = 200;

struct Args {
    broker: String,
    config_path: String,
    state_path: String,
    request_topic: String,
    response_topic: String,
    client_id: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        broker: String::from(DEFAULT_BROKER),
        config_path: String::new(),
        state_path: String::new(),
        request_topic: String::from(DEFAULT_REQUEST_TOPIC),
        response_topic: String::from(DEFAULT_RESPONSE_TOPIC),
        client_id: String::from(DEFAULT_CLIENT_ID),
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
            other => exit_with_usage(&format!("未知参数 {other}")),
        }
    }
    if a.config_path.is_empty() || a.state_path.is_empty() {
        exit_with_usage("--config 与 --state 必配");
    }
    a
}

fn exit_with_usage(msg: &str) -> ! {
    eprintln!(
        "planner-peer: {msg}\n用法：eneros-planner-peer --config <planner-peer.toml> \
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

/// seq 状态（跨重启单调；peer 重启 seq 回退会被目标侧按重放拒绝）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct SeqState(u64);

impl SeqState {
    fn load(path: &str) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => SeqState(v.get("seq").and_then(|s| s.as_u64()).unwrap_or(0)),
                Err(_) => SeqState(0),
            },
            Err(_) => SeqState(0),
        }
    }

    /// 原子落盘（temp + rename——半写状态文件不产生伪 seq）。
    fn store(&self, path: &str) -> std::io::Result<()> {
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, serde_json::json!({ "seq": self.0 }).to_string())?;
        std::fs::rename(&tmp, path)
    }
}

fn main() {
    let args = parse_args();
    let cfg_text = std::fs::read_to_string(&args.config_path)
        .unwrap_or_else(|e| exit_with_usage(&format!("读 config: {e}")));
    let cfg = PeerPlannerConfig::parse(&cfg_text)
        .unwrap_or_else(|e| exit_with_usage(&format!("config 解析: {e}")));
    let mut seq = SeqState::load(&args.state_path);

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

    let mut client = connect(&args);
    let mut last_ping = Instant::now();
    loop {
        // keepalive 节拍（半窗）。
        if last_ping.elapsed() >= Duration::from_millis(15_000) {
            if let Err(e) = client.ping() {
                eprintln!("[planner-peer] RECONNECT reason=ping-failed ({e})");
                client = connect(&args);
            }
            last_ping = Instant::now();
        }
        match client.poll(now_ms()) {
            Ok(Some(pub_)) => {
                if pub_.topic != args.request_topic {
                    continue;
                }
                let json = match std::str::from_utf8(&pub_.payload) {
                    Ok(s) => s,
                    Err(_) => {
                        println!("[planner-peer] PLANNER-SKIP reason=utf8");
                        continue;
                    }
                };
                handle_request(
                    json,
                    &cfg,
                    &mut planner,
                    &mut highs,
                    &mut seq,
                    &args,
                    &mut client,
                );
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[planner-peer] RECONNECT reason=poll-failed ({e})");
                client = connect(&args);
                last_ping = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(POLL_TICK_MS));
    }
}

fn connect(args: &Args) -> MqttClient {
    loop {
        match try_connect(args) {
            Ok(c) => return c,
            Err(e) => {
                eprintln!("[planner-peer] RECONNECT reason={} (retry 2s)", e.0);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

fn try_connect(args: &Args) -> Result<MqttClient, mqtt::MqttError> {
    let mut c = MqttClient::connect(&args.broker, &args.client_id, 30, Duration::from_secs(5))?;
    c.subscribe(&args.request_topic, 1)?;
    println!(
        "[planner-peer] SUBSCRIBED topic={} broker={}",
        args.request_topic, args.broker
    );
    Ok(c)
}

/// 单请求处理：解析 → 求解 → 组装 → 发布。任何失败 = PLANNER-SKIP +
/// **不应答**（目标侧超时降级——诚实语义）。
fn handle_request(
    json: &str,
    cfg: &PeerPlannerConfig,
    planner: &mut Planner,
    highs: &mut HighsSolver,
    seq: &mut SeqState,
    args: &Args,
    client: &mut MqttClient,
) {
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
    seq.0 = seq.0.saturating_add(1);
    let resp = eneros_planner_peer::build_response(
        req.req_id,
        seq.0,
        power,
        req.budget.ttl_secs,
        &cfg.target_device,
        &meta,
    );
    let out = match serde_json::to_string(&resp) {
        Ok(s) => s,
        Err(e) => {
            println!("[planner-peer] PLANNER-SKIP reason=encode ({e})");
            return;
        }
    };
    // 出站自检：目标侧 parse_and_validate 同链预演（t1 语义的运行时面；
    // 失败不发布——发一条必被拒的应答只会烧目标侧 bad 计数）。配对
    // （expect_req_id）本地可判；seq 递增判据目标侧会话状态 peer 不可知
    // ——置 None 只查本地可判定域。
    let checks = ResponseChecks {
        expect_req_id: req.req_id,
        expect_seq_gt: None,
        budget: &req.budget,
    };
    if let Err(e) = parse_and_validate(&out, &checks) {
        println!("[planner-peer] PLANNER-SKIP reason=self-check ({e:?})");
        return;
    }
    if let Err(e) = client.publish(&args.response_topic, out.as_bytes(), 1) {
        println!("[planner-peer] PLANNER-SKIP reason=publish ({e})");
        return;
    }
    if let Err(e) = seq.store(&args.state_path) {
        println!("[planner-peer] SEQ-PERSIST-FAIL {e}（重启后 seq 回退将由目标侧拒绝——诚实可见）");
    }
    println!(
        "[planner-peer] PEER-RESP seq={} bytes={} backend-tag-ok={}",
        seq.0,
        out.len(),
        resp.reason.starts_with(BACKEND_TAG)
    );
}
