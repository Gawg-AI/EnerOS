//! EnerOS HMI 集成测试（v0.42.1）
//!
//! 验证本地人机接口的公共 API：控制台渲染、审批状态机、Web 运维接口。
//!
//! 集成测试位于 `tests/` 目录，编译为独立 crate，可使用 `std`。
//! 测试目标 crate（`eneros-hmi`）以 no_std 库形式链接。

use eneros_agent::{AgentId, AgentState, AgentType};
use eneros_hmi::{
    render_hmi_screen, AgentStateSummary, AlarmSeverity, AlarmSummary, ApprovalId, ApprovalManager,
    ApprovalState, ConsoleRenderer, HmiError, HmiFrame, HttpMethod, HttpRequest, ManualAction,
    NetworkStatus, PendingApproval, PowerState, SystemState, WebHandler,
};

/// 构造一个包含单个 Agent 的非空系统状态，用于多数测试。
fn make_state() -> SystemState {
    SystemState {
        agent_states: vec![AgentStateSummary {
            agent_id: AgentId(1),
            name: String::from("grid-agent"),
            state: AgentState::Running,
            agent_type: AgentType::Grid,
        }],
        storage_usage_mb: 512,
        network: NetworkStatus {
            connected: true,
            ip_addr: Some(String::from("192.168.1.50")),
            rssi: Some(-55),
        },
        power: PowerState {
            battery_pct: 90,
            charging: false,
            ac_connected: true,
        },
        last_update_ms: 99_999,
    }
}

/// 构造一个手动操作（重启 Agent）。
fn make_action(id: u64) -> ManualAction {
    ManualAction {
        id,
        action_type: String::from("restart_agent"),
        target_agent: Some(AgentId(1)),
        params: String::from(r#"{"force":false}"#),
    }
}

// ===========================================================================
// 1. render_hmi_screen 结构验证
// ===========================================================================

#[test]
fn test_render_hmi_screen() {
    let state = make_state();
    let frame = render_hmi_screen(&state);

    // 系统状态应被克隆进帧
    assert_eq!(frame.system_state, state);
    assert_eq!(frame.system_state.storage_usage_mb, 512);
    assert_eq!(frame.system_state.agent_states.len(), 1);
    assert_eq!(frame.system_state.agent_states[0].name, "grid-agent");

    // render_hmi_screen 默认构造空列表（无告警/审批/操作注入接口）
    assert!(frame.active_alarms.is_empty());
    assert!(frame.pending_approvals.is_empty());
    assert!(frame.manual_actions.is_empty());
}

// ===========================================================================
// 2. 审批：提交 + 批准
// ===========================================================================

#[test]
fn test_approval_submit_and_approve() {
    let mut mgr = ApprovalManager::new();
    let id = mgr.submit(make_action(1), "operator-a", 1_000);

    // 初始状态为 Pending
    let pending = mgr.get(id).expect("approval should exist after submit");
    assert_eq!(pending.state, ApprovalState::Pending);
    assert_eq!(pending.requester, "operator-a");
    assert_eq!(pending.timestamp, 1_000);
    assert_eq!(pending.action.action_type, "restart_agent");

    // 批准
    let result = mgr.approve(id);
    assert!(result.is_ok());
    assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Approved);

    // 批准后不再出现在 pending 列表
    assert!(mgr.list_pending().is_empty());
}

// ===========================================================================
// 3. 审批：拒绝
// ===========================================================================

#[test]
fn test_approval_reject() {
    let mut mgr = ApprovalManager::new();
    let id = mgr.submit(make_action(2), "operator-b", 2_000);

    // 拒绝
    let result = mgr.reject(id);
    assert!(result.is_ok());
    assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Rejected);

    // 拒绝后不可再次批准
    let err = mgr.approve(id).unwrap_err();
    assert!(matches!(
        err,
        HmiError::InvalidStateTransition {
            from: ApprovalState::Rejected,
            to: ApprovalState::Approved
        }
    ));
}

// ===========================================================================
// 4. 审批：提交 + 批准 + 执行
// ===========================================================================

#[test]
fn test_approval_execute() {
    let mut mgr = ApprovalManager::new();
    let id = mgr.submit(make_action(3), "operator-c", 3_000);

    // 未经批准不可执行
    let err = mgr.execute(id).unwrap_err();
    assert!(matches!(
        err,
        HmiError::InvalidStateTransition {
            from: ApprovalState::Pending,
            to: ApprovalState::Executed
        }
    ));

    // 批准后执行
    mgr.approve(id).expect("approve should succeed");
    let action = mgr
        .execute(id)
        .expect("execute should succeed after approve");
    assert_eq!(action.id, 3);
    assert_eq!(action.action_type, "restart_agent");
    assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Executed);

    // 已执行不可重复执行
    assert!(mgr.execute(id).is_err());
}

// ===========================================================================
// 5. ConsoleRenderer::render 系统状态渲染
// ===========================================================================

#[test]
fn test_console_render_state() {
    let renderer = ConsoleRenderer::new();
    let output = renderer.render(&make_state());

    // 标题与关键字段
    assert!(output.contains("=== EnerOS System Status ==="));
    assert!(output.contains("Last Update: 99999 ms"));
    assert!(output.contains("Storage: 512 MB"));
    // 网络
    assert!(output.contains("Network: Connected"));
    assert!(output.contains("192.168.1.50"));
    assert!(output.contains("RSSI=-55dBm"));
    // 电源
    assert!(output.contains("Power: Battery 90%"));
    assert!(output.contains("[AC]"));
    // Agent 列表
    assert!(output.contains("Agents (1):"));
    assert!(output.contains("grid-agent"));
}

// ===========================================================================
// 6. ConsoleRenderer::render_frame 帧渲染（含告警与审批）
// ===========================================================================

#[test]
fn test_console_render_frame() {
    let renderer = ConsoleRenderer::new();
    let state = make_state();
    let frame = HmiFrame {
        system_state: state,
        active_alarms: vec![AlarmSummary {
            id: 7,
            severity: AlarmSeverity::Critical,
            message: String::from("Agent grid-agent crashed"),
            timestamp: 8_888,
        }],
        pending_approvals: vec![PendingApproval {
            id: ApprovalId(1),
            action: make_action(1),
            requester: String::from("operator-d"),
            timestamp: 1_000,
            state: ApprovalState::Pending,
        }],
        manual_actions: vec![make_action(9)],
    };
    let output = renderer.render_frame(&frame);

    // 告警段
    assert!(output.contains("=== Alarms (1) ==="));
    assert!(output.contains("[CRIT]"));
    assert!(output.contains("Agent grid-agent crashed"));
    // 审批段
    assert!(output.contains("=== Pending Approvals (1) ==="));
    assert!(output.contains("[Pending]"));
    assert!(output.contains("restart_agent"));
    assert!(output.contains("operator-d"));
    // 手动操作段
    assert!(output.contains("=== Manual Actions (1) ==="));
}

// ===========================================================================
// 7. Web GET /status 端点
// ===========================================================================

#[test]
fn test_web_status_endpoint() {
    let handler = WebHandler::new();
    let req = HttpRequest::new(HttpMethod::Get, "/status");
    let resp = handler.handle(&req, &make_state());

    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_type, "application/json");
    // JSON 结构关键片段
    assert!(resp.body.contains(r#""agents":"#));
    assert!(resp.body.contains("grid-agent"));
    assert!(resp.body.contains(r#""storage_usage_mb":512"#));
    assert!(resp.body.contains(r#""last_update_ms":99999"#));
    assert!(resp.body.contains(r#""connected":true"#));
    assert!(resp.body.contains(r#""battery_pct":90"#));
}

// ===========================================================================
// 8. Web POST /action 端点
// ===========================================================================

#[test]
fn test_web_action_endpoint() {
    let handler = WebHandler::new();
    let req = HttpRequest::new(HttpMethod::Post, "/action")
        .with_body(r#"{"action":"restart_agent","target":1}"#);
    let resp = handler.handle(&req, &make_state());

    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_type, "application/json");
    assert!(resp.body.contains("submitted"));
}

// ===========================================================================
// 9. Web 未知路径返回 404
// ===========================================================================

#[test]
fn test_web_404() {
    let handler = WebHandler::new();
    let req = HttpRequest::new(HttpMethod::Get, "/unknown");
    let resp = handler.handle(&req, &make_state());

    assert_eq!(resp.status, 404);
    assert_eq!(resp.content_type, "application/json");
    assert!(resp.body.contains("not found"));
}

// ===========================================================================
// 10. render_hmi_screen 空状态
// ===========================================================================

#[test]
fn test_hmi_frame_empty_state() {
    let empty_state = SystemState {
        agent_states: Vec::new(),
        storage_usage_mb: 0,
        network: NetworkStatus {
            connected: false,
            ip_addr: None,
            rssi: None,
        },
        power: PowerState {
            battery_pct: 0,
            charging: false,
            ac_connected: false,
        },
        last_update_ms: 0,
    };
    let frame = render_hmi_screen(&empty_state);

    // 帧字段一致性
    assert_eq!(frame.system_state, empty_state);
    assert!(frame.system_state.agent_states.is_empty());
    assert!(!frame.system_state.network.connected);
    // 默认空列表
    assert!(frame.active_alarms.is_empty());
    assert!(frame.pending_approvals.is_empty());
    assert!(frame.manual_actions.is_empty());

    // 空帧可被控制台渲染（不 panic）
    let renderer = ConsoleRenderer::new();
    let text = renderer.render_frame(&frame);
    assert!(text.contains("Agents (0):"));
    assert!(text.contains("Disconnected"));
}
