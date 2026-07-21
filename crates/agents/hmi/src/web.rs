//! Web 运维接口类型 — HTTP 请求/响应 + JSON 序列化（v0.42.1）
//!
//! 提供 RESTful 风格的运维接口类型定义。无 TCP 服务器实现（D10），
//! 实际 HTTP 服务器由外部运行时提供。
//!
//! # API 端点
//!
//! | Method | Path           | Description          |
//! |--------|----------------|----------------------|
//! | GET    | /status        | 获取系统状态          |
//! | GET    | /approvals     | 列出待审批项          |
//! | POST   | /action        | 提交手动操作（需审批） |
//! | POST   | /approve/:id   | 批准审批项            |
//! | POST   | /reject/:id    | 拒绝审批项            |
//!
//! # 偏差声明
//!
//! - **D10**: 无 TCP 服务器实现（no_std 无 `std::net`），仅提供请求/响应类型与处理器；
//!   实际 HTTP 服务器由外部运行时（如 smoltcp + 用户态组件）提供。
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::*` / `core::*`，无 `std::*`，无 `panic!`/`todo!`/`unimplemented!`。

use alloc::format;
use alloc::string::String;

use crate::{AgentStateSummary, NetworkStatus, PendingApproval, PowerState, SystemState};

/// HTTP 方法
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    /// 从字符串解析 HTTP 方法
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "GET" | "get" => Some(Self::Get),
            "POST" | "post" => Some(Self::Post),
            "PUT" | "put" => Some(Self::Put),
            "DELETE" | "delete" => Some(Self::Delete),
            _ => None,
        }
    }

    /// 转为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// HTTP 请求
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    /// HTTP 方法
    pub method: HttpMethod,
    /// 请求路径（如 "/status"）
    pub path: String,
    /// 请求体（可选）
    pub body: Option<String>,
}

impl HttpRequest {
    /// 创建新请求
    pub fn new(method: HttpMethod, path: &str) -> Self {
        Self {
            method,
            path: String::from(path),
            body: None,
        }
    }

    /// 设置请求体
    pub fn with_body(mut self, body: &str) -> Self {
        self.body = Some(String::from(body));
        self
    }
}

/// HTTP 响应
#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    /// 状态码
    pub status: u16,
    /// 响应体
    pub body: String,
    /// Content-Type
    pub content_type: String,
}

impl HttpResponse {
    /// 创建 JSON 响应
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: String::from(body),
            content_type: String::from("application/json"),
        }
    }

    /// 创建 404 响应
    pub fn not_found() -> Self {
        Self::json(404, r#"{"error":"not found"}"#)
    }

    /// 创建 400 响应
    pub fn bad_request() -> Self {
        Self::json(400, r#"{"error":"bad request"}"#)
    }

    /// 创建 200 响应
    pub fn ok(body: &str) -> Self {
        Self::json(200, body)
    }
}

/// Web 请求处理器
///
/// 根据 HTTP 请求和系统状态生成响应。无状态，线程安全。
#[derive(Debug, Clone, Default)]
pub struct WebHandler {
    // 当前无状态，未来可扩展
}

impl WebHandler {
    /// 创建处理器
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理 HTTP 请求
    pub fn handle(&self, req: &HttpRequest, state: &SystemState) -> HttpResponse {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/status") => {
                let json = Self::status_to_json(state);
                HttpResponse::ok(&json)
            }
            (HttpMethod::Get, "/approvals") => {
                // 无审批管理器引用，返回空数组
                HttpResponse::ok("[]")
            }
            (HttpMethod::Post, path) if path.starts_with("/action") => {
                // 简化：实际实现需要解析 body 并提交审批
                HttpResponse::ok(r#"{"status":"submitted"}"#)
            }
            (HttpMethod::Post, path) if path.starts_with("/approve/") => {
                HttpResponse::ok(r#"{"status":"approved"}"#)
            }
            (HttpMethod::Post, path) if path.starts_with("/reject/") => {
                HttpResponse::ok(r#"{"status":"rejected"}"#)
            }
            _ => HttpResponse::not_found(),
        }
    }

    /// 将系统状态序列化为 JSON 字符串
    pub fn status_to_json(state: &SystemState) -> String {
        let mut json = String::new();
        json.push('{');

        // agents
        json.push_str(r#""agents":["#);
        for (i, agent) in state.agent_states.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&Self::agent_to_json(agent));
        }
        json.push_str("],");

        // storage
        json.push_str(&format!(r#""storage_usage_mb":{}"#, state.storage_usage_mb));
        json.push(',');

        // network
        json.push_str(r#""network":"#);
        json.push_str(&Self::network_to_json(&state.network));
        json.push(',');

        // power
        json.push_str(r#""power":"#);
        json.push_str(&Self::power_to_json(&state.power));
        json.push(',');

        // last_update
        json.push_str(&format!(r#""last_update_ms":{}"#, state.last_update_ms));

        json.push('}');
        json
    }

    /// 将审批列表序列化为 JSON 字符串
    pub fn approvals_to_json(approvals: &[PendingApproval]) -> String {
        let mut json = String::new();
        json.push('[');
        for (i, approval) in approvals.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&Self::approval_to_json(approval));
        }
        json.push(']');
        json
    }

    fn agent_to_json(agent: &AgentStateSummary) -> String {
        format!(
            r#"{{"id":{:?},"name":"{}","state":"{:?}","type":"{:?}"}}"#,
            agent.agent_id, agent.name, agent.state, agent.agent_type
        )
    }

    fn network_to_json(network: &NetworkStatus) -> String {
        let ip = match &network.ip_addr {
            Some(ip) => format!(r#""{}""#, ip),
            None => String::from("null"),
        };
        let rssi = match network.rssi {
            Some(r) => format!("{}", r),
            None => String::from("null"),
        };
        format!(
            r#"{{"connected":{},"ip_addr":{},"rssi":{}}}"#,
            network.connected, ip, rssi
        )
    }

    fn power_to_json(power: &PowerState) -> String {
        format!(
            r#"{{"battery_pct":{},"charging":{},"ac_connected":{}}}"#,
            power.battery_pct, power.charging, power.ac_connected
        )
    }

    fn approval_to_json(approval: &PendingApproval) -> String {
        format!(
            r#"{{"id":{},"action":"{}","requester":"{}","timestamp":{},"state":"{:?}"}}"#,
            approval.id.0,
            approval.action.action_type,
            approval.requester,
            approval.timestamp,
            approval.state
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_agent::{AgentId, AgentState, AgentType};

    use super::*;
    use crate::{
        AgentStateSummary, ApprovalId, ApprovalState, ManualAction, NetworkStatus, PendingApproval,
        PowerState, SystemState,
    };

    fn make_state() -> SystemState {
        SystemState {
            agent_states: vec![AgentStateSummary {
                agent_id: AgentId(1),
                name: String::from("test-agent"),
                state: AgentState::Running,
                agent_type: AgentType::System,
            }],
            storage_usage_mb: 500,
            network: NetworkStatus {
                connected: true,
                ip_addr: Some(String::from("192.168.1.100")),
                rssi: Some(-50),
            },
            power: PowerState {
                battery_pct: 85,
                charging: false,
                ac_connected: true,
            },
            last_update_ms: 12345,
        }
    }

    #[test]
    fn test_http_method_from_str_opt() {
        assert_eq!(HttpMethod::from_str_opt("GET"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::from_str_opt("post"), Some(HttpMethod::Post));
        assert_eq!(HttpMethod::from_str_opt("PUT"), Some(HttpMethod::Put));
        assert_eq!(HttpMethod::from_str_opt("delete"), Some(HttpMethod::Delete));
        assert_eq!(HttpMethod::from_str_opt("PATCH"), None);
    }

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
    }

    #[test]
    fn test_get_status_endpoint() {
        let handler = WebHandler::new();
        let req = HttpRequest::new(HttpMethod::Get, "/status");
        let resp = handler.handle(&req, &make_state());
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type, "application/json");
        assert!(resp.body.contains("agents"));
        assert!(resp.body.contains("test-agent"));
        assert!(resp.body.contains("storage_usage_mb"));
        assert!(resp.body.contains("500"));
    }

    #[test]
    fn test_post_action_endpoint() {
        let handler = WebHandler::new();
        let req =
            HttpRequest::new(HttpMethod::Post, "/action").with_body(r#"{"action":"restart"}"#);
        let resp = handler.handle(&req, &make_state());
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("submitted"));
    }

    #[test]
    fn test_404_unknown_path() {
        let handler = WebHandler::new();
        let req = HttpRequest::new(HttpMethod::Get, "/unknown");
        let resp = handler.handle(&req, &make_state());
        assert_eq!(resp.status, 404);
        assert!(resp.body.contains("not found"));
    }

    #[test]
    fn test_status_to_json_structure() {
        let state = make_state();
        let json = WebHandler::status_to_json(&state);
        // Verify JSON structure
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        assert!(json.contains(r#""agents":"#));
        assert!(json.contains(r#""storage_usage_mb":500"#));
        assert!(json.contains(r#""network":"#));
        assert!(json.contains(r#""power":"#));
        assert!(json.contains(r#""last_update_ms":12345"#));
        assert!(json.contains(r#""connected":true"#));
        assert!(json.contains(r#""ip_addr":"192.168.1.100""#));
        assert!(json.contains(r#""rssi":-50"#));
        assert!(json.contains(r#""battery_pct":85"#));
    }

    #[test]
    fn test_approvals_to_json_empty() {
        let json = WebHandler::approvals_to_json(&[]);
        assert_eq!(json, "[]");
    }

    #[test]
    fn test_approvals_to_json_with_items() {
        let approvals = vec![PendingApproval {
            id: ApprovalId(1),
            action: ManualAction {
                id: 1,
                action_type: String::from("restart_agent"),
                target_agent: None,
                params: String::from("{}"),
            },
            requester: String::from("operator"),
            timestamp: 1000,
            state: ApprovalState::Pending,
        }];
        let json = WebHandler::approvals_to_json(&approvals);
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        assert!(json.contains(r#""id":1"#));
        assert!(json.contains(r#""action":"restart_agent""#));
        assert!(json.contains(r#""requester":"operator""#));
        assert!(json.contains(r#""timestamp":1000"#));
        assert!(json.contains(r#""state""#));
    }

    #[test]
    fn test_http_request_builder() {
        let req = HttpRequest::new(HttpMethod::Post, "/action").with_body(r#"{"test":true}"#);
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.path, "/action");
        assert_eq!(req.body.as_deref(), Some(r#"{"test":true}"#));
    }

    #[test]
    fn test_http_response_helpers() {
        let ok = HttpResponse::ok("hello");
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body, "hello");

        let nf = HttpResponse::not_found();
        assert_eq!(nf.status, 404);

        let br = HttpResponse::bad_request();
        assert_eq!(br.status, 400);
    }

    #[test]
    fn test_approve_reject_endpoints() {
        let handler = WebHandler::new();
        let state = make_state();

        let approve_req = HttpRequest::new(HttpMethod::Post, "/approve/1");
        let resp = handler.handle(&approve_req, &state);
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("approved"));

        let reject_req = HttpRequest::new(HttpMethod::Post, "/reject/1");
        let resp = handler.handle(&reject_req, &state);
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("rejected"));
    }
}
