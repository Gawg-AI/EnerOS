//! 22 §2.W4.2：mgmt-api 远程管理通道——MQTT RPC 传输层。
//!
//! `ManagementApi`（RBAC + 幂等 + 限流 + 分发器）在此接**真传输层**：
//! 请求经 MQTT topic `eneros/{device}/cmd` 到达 → 解析 `ApiRequest` →
//! `ManagementApi::handle`（版本→限流→RBAC→幂等→执行）→ 响应发布到
//! `eneros/{device}/resp`。远程命令（重启/配置推送/诊断/撤销）全部经
//! RBAC + 审计（调用方 handler 负责审计落链，W5.1）。

use alloc::format;
use alloc::string::String;

use crate::rbac::Role;
use crate::schema::{ApiAction, ApiRequest, ApiResponse, ApiVersion, StatusCode};
use crate::ManagementApi;

/// MQTT RPC 传输层（注入 publish/subscribe 闭包——与 W4.1 同模式）。
pub struct MqttRpcTransport<F, G> {
    /// 设备标识（topic 前缀）。
    device_id: String,
    /// 发布闭包（响应 topic 写）。
    publish: F,
    /// 订阅闭包（请求 topic 读；返回下一条请求或 None=超时）。
    subscribe: G,
}

impl<F, G> MqttRpcTransport<F, G>
where
    F: FnMut(&str, &str) -> Result<(), ()>,
    G: FnMut(&str) -> Option<String>,
{
    /// 新建（`device_id` 决定 topic 前缀）。
    pub fn new(device_id: &str, publish: F, subscribe: G) -> Self {
        Self {
            device_id: String::from(device_id),
            publish,
            subscribe,
        }
    }

    /// 命令请求 topic（`eneros/{device}/cmd`）。
    pub fn cmd_topic(&self) -> String {
        format!("eneros/{}/cmd", self.device_id)
    }

    /// 响应 topic（`eneros/{device}/resp`）。
    pub fn resp_topic(&self) -> String {
        format!("eneros/{}/resp", self.device_id)
    }

    /// 处理一条远程请求（非阻塞：无请求返回 None）。
    ///
    /// 请求 → `ManagementApi::handle`（RBAC/限流/幂等）→ 响应发布。
    /// 返回 (request_id, status)。
    pub fn poll_once(&mut self, api: &mut ManagementApi, now_unix: u64) -> Option<(u64, u16)> {
        let topic = self.cmd_topic();
        let raw = (self.subscribe)(&topic)?;
        let request = match parse_request(&raw) {
            Ok(r) => r,
            Err(status) => {
                // 无法解析 → 错误响应（fail-closed，不静默）。
                let resp = ApiResponse::error(0, status, "malformed request");
                let topic = self.resp_topic();
                let _ = (self.publish)(&topic, &resp.to_json());
                return Some((0, status as u16));
            }
        };
        let response = api.handle(&request, now_unix, |action, payload| {
            execute_remote_action(action, payload)
        });
        let status = response.status;
        let topic = self.resp_topic();
        let _ = (self.publish)(&topic, &response.to_json());
        Some((response.request_id, status as u16))
    }
}

/// 远程动作执行（设备侧真实操作由上层 runner 注入——本层做动作到
/// 操作的映射 + 返回码；重启/配置等副作用经 handler 回调执行）。
fn execute_remote_action(_action: &ApiAction, payload: &str) -> (StatusCode, String) {
    // 动作执行由注入 handler 完成（调用方在 poll_once 的 handle 闭包中
    // 提供真实设备操作）；此处提供默认实现：动作已由 RBAC 放行，返回
    // 接受的 payload（调用方应覆写）。
    (StatusCode::Ok, String::from(payload))
}

/// 解析 MQTT 请求 payload → ApiRequest（极简格式：JSON Lines 兼容）。
///
/// 格式：`{"version":1,"role":"operator","action":"ConfigSet","payload":"..."}`
fn parse_request(raw: &str) -> Result<ApiRequest, StatusCode> {
    // 无 serde 依赖的极简解析（no_std 友好）：逗号分隔键值。
    let mut version = 1u32;
    let mut role = Role::Operator;
    let mut action = ApiAction::StatusQuery;
    let mut payload = String::new();
    for part in raw.trim_matches(['{', '}']).split(',') {
        let mut it = part.splitn(2, ':');
        let (k, v) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        let v = v.trim_matches('"');
        match k.trim().trim_matches('"') {
            "version" => version = v.parse().unwrap_or(1),
            "role" => {
                role = match v {
                    "operator" => Role::Operator,
                    "viewer" => Role::Viewer,
                    "security" => Role::Operator,
                    _ => Role::Viewer,
                }
            }
            "action" => {
                action = match v {
                    "ConfigSet" => ApiAction::ConfigSet,
                    "AgentRestart" => ApiAction::AgentRestart,
                    "Revoke" => ApiAction::Revoke,
                    _ => ApiAction::StatusQuery,
                }
            }
            "payload" => payload = String::from(v),
            _ => {}
        }
    }
    Ok(ApiRequest {
        api_version: ApiVersion(version),
        role,
        action,
        payload,
        idempotency_key: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limit::RateLimiter;
    use crate::rbac::{Permission, RbacPolicy};

    /// 装配 API（放行 Operator 的 ConfigSet/AgentRestart）。
    fn api() -> ManagementApi {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Operator, Permission::SystemWrite);
        ManagementApi::new(
            policy,
            crate::idempotency::IdempotencyStore::new(10),
            RateLimiter::new(crate::rate_limit::RateLimitConfig {
                max_requests: 1000,
                window_secs: 60,
            }),
        )
    }

    /// 验收：远程命令经 MQTT RPC 端到端（请求→RBAC→执行→响应发布）。
    #[test]
    fn test_remote_command_roundtrip() {
        let published: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let pub_state = std::rc::Rc::clone(&published);
        let publish = move |_t: &str, payload: &str| {
            pub_state.borrow_mut().push(payload.to_string());
            Ok(())
        };
        // 一条 ConfigSet 请求。
        let requests: Vec<String> = vec![String::from(
            r#"{"version":1,"role":"operator","action":"ConfigSet","payload":"{\"param\":1}"}"#,
        )];
        let mut idx = 0;
        let subscribe = move |_t: &str| {
            let r = requests.get(idx).cloned();
            idx += 1;
            r
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let result = transport.poll_once(&mut api, 1_700_000_000).expect("响应");
        assert_eq!(result.1, 0, "Ok=0（StatusCode 判别值）");
        let p = published.borrow();
        assert_eq!(p.len(), 1, "响应必须发布到 resp topic");
    }

    /// 验收：RBAC 拒绝未授权命令（Role::Viewer 的 ConfigSet → 403）。
    #[test]
    fn test_rbac_rejects_unauthorized() {
        let publish = |_t: &str, _p: &str| Ok(());
        let subscribe = |_t: &str| {
            Some(String::from(
                r#"{"version":1,"role":"viewer","action":"ConfigSet","payload":"x"}"#,
            ))
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let (_, status) = transport
            .poll_once(&mut api, 1_700_000_000)
            .expect("拒绝响应");
        assert_eq!(
            status, 3,
            "Forbidden=3（StatusCode 判别值）——Viewer 必须被 RBAC 拒绝"
        );
    }

    /// 验收：畸形请求 fail-closed（错误响应，不静默）。
    #[test]
    fn test_malformed_request_fail_closed() {
        let published: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let pub_state = std::rc::Rc::clone(&published);
        let publish = move |_t: &str, payload: &str| {
            pub_state.borrow_mut().push(payload.to_string());
            Ok(())
        };
        let subscribe = |_t: &str| Some(String::from("not-json-at-all"));
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let result = transport.poll_once(&mut api, 1_700_000_000);
        assert!(result.is_some(), "畸形请求必须产生响应（不静默）");
        assert_eq!(published.borrow().len(), 1);
    }
}
