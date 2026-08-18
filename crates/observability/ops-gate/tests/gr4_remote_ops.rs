//! 22 §2.G-R4 闸门：远程能看 / 能管 / 能升级——五通道端到端会话。
//!
//! 同一 `RemoteOpsSession`（模拟 MQTT broker）串起 W4.1-W4.5：
//!
//! 1. **能看**：metrics 导出器 → `eneros/{device}/metrics`（运维侧收到行）；
//! 2. **能看（告警）**：SLO 超阈值 → `MqttAlertSink` → `alerts` topic；
//! 3. **能管**：运维侧发 `cmd` → `ManagementApi`（RBAC/限流）→ `resp`
//!    回运维侧；未授权命令被拒绝（Forbidden）；
//! 4. **能升级**：镜像下发 → `OtaDownloader` 分块下载 → `OtaPipeline`
//!    回路（write/verify/activate/health）→ 升级成功；
//! 5. **能看（实时）**：`WsServer` 广播 RFC 6455 文本帧 → 运维侧
//!    `parse_frame` 还原 JSON 事件。
//!
//! 全部断言通过 = G-R4 判据「W4.1-W4.5 全 PASS + 远程能看/能管/能升级」。

use eneros_dashboard::ws::{parse_server_frame, OP_TEXT};
use eneros_dashboard::{
    DashboardEvent, DashboardWsServer, EdgeKind, FlowDirection, NodeKind, NodeStatus, Position3D,
    TopologyEdge, TopologyGraph, TopologyNode, WsServer,
};
use eneros_mgmt_api::rate_limit::{RateLimitConfig, RateLimiter};
use eneros_mgmt_api::rbac::{Permission, RbacPolicy};
use eneros_mgmt_api::transport::MqttRpcTransport;
use eneros_mgmt_api::{IdempotencyStore, ManagementApi, Role, StatusCode};
use eneros_ops_gate::{attach_rpc_transport, RemoteOpsSession};
use eneros_ota_ab::downloader::{OtaDownloader, OtaOutcome, OtaPipeline};
use eneros_slo_alert::{AlertState, TriggerEngine};

/// 装配管理 API（放行 Operator 的 SystemWrite——ConfigSet 命令）。
fn build_mgmt_api() -> ManagementApi {
    let mut policy = RbacPolicy::new();
    policy.grant(Role::Operator, Permission::SystemWrite);
    ManagementApi::new(
        policy,
        IdempotencyStore::new(10),
        RateLimiter::new(RateLimitConfig {
            max_requests: 1000,
            window_secs: 60,
        }),
    )
}

#[test]
fn gr4_remote_ops_session_full() {
    let session = RemoteOpsSession::new("dev-42");
    let mut api = build_mgmt_api();
    let cmd_topic = "eneros/dev-42/cmd";

    // ── 1. 能看：metrics 经 MQTT 到达运维侧 ─────────────────────────
    let mut exporter = session.metrics_exporter();
    exporter
        .export("{\"metric\":\"cpu\",\"v\":0.42}")
        .expect("导出");
    let line = session.take_metrics().expect("运维侧收到 metrics");
    assert!(line.contains("cpu"), "metrics 行内容: {line}");

    // ── 2. 能看（告警）：SLO 超阈值 → alerts topic ──────────────────
    let mut engine = TriggerEngine::new(session.alert_sink());
    assert!(
        engine.evaluate("ipc", 6_000_000, 2_000_000, 5_000_000, 1000),
        "触发告警"
    );
    let alert = session.take_alert().expect("运维侧收到告警");
    assert!(alert.contains("state=Critical"), "告警内容: {alert}");

    // ── 3. 能管：cmd → RBAC → 执行 → resp 回运维侧 ──────────────────
    let mut transport: MqttRpcTransport<_, _> = attach_rpc_transport(&session);
    // Operator 发 ConfigSet（授权）。
    let cmd = r#"{"version":1,"role":"operator","action":"ConfigSet","payload":"{\"param\":1}"}"#;
    session.broker.publish(cmd_topic, cmd).expect("发布 cmd");
    let handled = transport
        .poll_once(
            &mut api,
            Role::Operator,
            1_700_000_000,
            &mut |_action, _arg| (StatusCode::Ok, "ok".into()),
        )
        .expect("设备侧处理远程命令")
        .expect("发布成功");
    assert_eq!(handled.1, 200, "Ok=200（HTTP 风格判别值）");
    let resp = session.take_response().expect("运维侧收到响应");
    assert!(resp.contains("\"request_id\":"), "响应: {resp}");
    assert!(resp.contains("\"status\":200"), "响应状态: {resp}");

    // Viewer 发 AgentRestart（未授权）→ Forbidden=3，运维侧收到拒绝。
    let bad = r#"{"version":1,"role":"viewer","action":"AgentRestart","payload":"x"}"#;
    session.broker.publish(cmd_topic, bad).expect("发布 cmd");
    let (_, status) = transport
        .poll_once(
            &mut api,
            Role::Viewer,
            1_700_000_001,
            &mut |_action, _arg| (StatusCode::Ok, "ok".into()),
        )
        .expect("设备侧处理")
        .expect("发布成功");
    assert_eq!(status, 403, "Forbidden=403——RBAC 拒绝未授权命令");
    let resp = session.take_response().expect("运维侧收到拒绝响应");
    assert!(resp.contains("\"status\":403"), "拒绝响应: {resp}");

    // ── 4. 能升级：镜像下载 + OTA 回路 → Success ────────────────────
    let mut dl = OtaDownloader::new("http://ota/fw.bin", 1000, 256, 4096, |off, want| {
        Ok(vec![(off % 256) as u8; want as usize])
    })
    .expect("构造器校验通过");
    let (image, progress) = dl.download_all().expect("下载完成");
    assert_eq!(progress.last().unwrap().percent(), 100);
    let mut pipe = OtaPipeline::new(
        |_img: &[u8]| Ok(()),
        |_img: &[u8]| Ok(()), // 验签通过
        || true,              // healthcheck 通过
    );
    let outcome = pipe.run(&image);
    assert_eq!(outcome, OtaOutcome::Success { slot: 1 }, "升级成功");

    // ── 5. 能看（实时）：WS 广播 → RFC 6455 帧 → 运维侧解析 ──────────
    let frames: std::cell::RefCell<Vec<Vec<u8>>> = std::cell::RefCell::new(Vec::new());
    let ws = WsServer::new(|frame: &[u8]| {
        frames.borrow_mut().push(frame.to_vec());
        Ok(())
    });
    let graph = TopologyGraph {
        nodes: vec![TopologyNode {
            node_id: 1,
            label: "PV".into(),
            kind: NodeKind::Grid,
            position: Position3D::new(0.0, 0.0, 0.0),
            status: NodeStatus::Healthy,
        }],
        edges: vec![TopologyEdge {
            from: 1,
            to: 2,
            kind: EdgeKind::Power,
            flow: 150.5,
            direction: FlowDirection::Forward,
        }],
        timestamp: 2000,
    };
    ws.broadcast(&DashboardEvent::TopologyUpdate(graph))
        .expect("广播");
    let raw = frames.borrow();
    let frame = parse_server_frame(&raw[0]).expect("运维侧解析 WS 帧");
    assert_eq!(frame.opcode, OP_TEXT, "文本帧");
    let json = String::from_utf8(frame.payload).expect("UTF-8");
    assert!(json.contains("\"type\":\"topology\""), "实时事件: {json}");
    assert!(json.contains("\"flow\":150.500"));
    assert!(json.contains("\"status\":\"healthy\""));
}

/// 验收：告警恢复也经 MQTT 发出（运维侧能收到恢复事件）。
#[test]
fn gr4_alert_recovery_published() {
    let session = RemoteOpsSession::new("dev-42");
    let mut engine = TriggerEngine::new(session.alert_sink());
    engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
    engine.evaluate("ipc", 100_000, 2_000_000, 5_000_000, 2000);
    let alert = session.take_alert().expect("告警");
    assert!(alert.contains("state=Warning"));
    let recovery = session.take_alert().expect("恢复事件");
    assert!(recovery.contains("state=Normal"), "恢复: {recovery}");
    assert_eq!(engine.state(), AlertState::Normal);
}

/// 验收：OTA 下载失败（短分块）不进入回路（fail-closed）。
#[test]
fn gr4_ota_download_fail_closed() {
    let mut dl = OtaDownloader::new("http://ota/bad.bin", 1000, 256, 4096, |_o, _w| {
        Ok(Vec::new())
    })
    .expect("构造器校验通过");
    assert!(dl.download_all().is_err(), "空分块必须 Err（防死循环）");
}
