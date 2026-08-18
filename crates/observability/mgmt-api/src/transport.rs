//! 22 §2.W4.2：mgmt-api 远程管理通道——MQTT RPC 传输层。
//!
//! 蓝图 41 WP-41.11 修真（35-00 P0-13）：
//! - **身份在 transport session**：`poll_once` 接收 transport 层认证的
//!   `role`（TLS/mTLS/session token 验证后才 decode action），请求
//!   payload 中的 `role` 字段**忽略并拒绝冲突**（不再信任自报角色，
//!   "security" 不再映射 Operator）；
//! - **malformed/unknown action 返回错误**（400），不再默认
//!   Operator/StatusQuery；
//! - **执行错误传播**：publish 失败返回 Err（不吞 `let _ =`）；
//!   `execute_remote_action` 固定 Ok 的默认实现删除——调用方必须注入
//!   真实 executor，未注入 → 显式 501/500（不空成功）。

use alloc::format;
use alloc::string::String;

use crate::rbac::Role;
use crate::schema::{ApiAction, ApiRequest, ApiResponse, ApiVersion, StatusCode};
use crate::ManagementApi;

/// MQTT RPC 传输层（注入 publish/subscribe 闭包 + 真实动作执行器）。
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
    /// `session_role` = transport 层认证的角色（payload 自报 role 无效）；
    /// `executor` = 真实动作执行器（每种 action 有实现；未接线的动作
    /// 由调用方返回显式错误，不得固定 Ok）。
    ///
    /// 返回 (request_id, status)；**publish 失败 → `Ok(None)` 之上的
    /// `Err(())`**——执行成功但响应未达 = 调用方可见的失败，不吞。
    pub fn poll_once(
        &mut self,
        api: &mut ManagementApi,
        session_role: Role,
        now_unix: u64,
        executor: &mut dyn FnMut(&ApiAction, &str) -> (StatusCode, String),
    ) -> Option<Result<(u64, u16), ()>> {
        let topic = self.cmd_topic();
        let raw = (self.subscribe)(&topic)?;
        let request = match parse_request(&raw, session_role) {
            Ok(r) => r,
            Err(status) => {
                // 无法解析/未知 action → 错误响应（fail-closed，不静默、
                // 不默认 StatusQuery）。
                let resp = ApiResponse::error(0, status, "malformed or unknown request");
                let topic = self.resp_topic();
                if (self.publish)(&topic, &resp.to_json()).is_err() {
                    return Some(Err(()));
                }
                return Some(Ok((0, status.code())));
            }
        };
        let response = api.handle(&request, now_unix, executor);
        let status = response.status;
        let topic = self.resp_topic();
        if (self.publish)(&topic, &response.to_json()).is_err() {
            return Some(Err(()));
        }
        Some(Ok((response.request_id, status.code())))
    }
}

/// 解析 MQTT 请求 payload → ApiRequest（极简 JSON Lines 兼容）。
///
/// 蓝图 41：`role` 不得来自 payload——`session_role`（transport 认证）
/// 是唯一角色来源；payload 中出现的 `role` 键若与 session 冲突 → 400。
/// 未知 action/role → Err（不再默认 Operator/StatusQuery）。
fn parse_request(raw: &str, session_role: Role) -> Result<ApiRequest, StatusCode> {
    let mut version = 1u32;
    let mut action = None;
    let mut payload = String::new();
    let mut payload_role: Option<Role> = None;
    for part in raw.trim_matches(['{', '}']).split(',') {
        let mut it = part.splitn(2, ':');
        let (k, v) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        let v = v.trim_matches('"');
        match k.trim().trim_matches('"') {
            "version" => version = v.parse().unwrap_or(1),
            "role" => {
                // payload 自报角色：仅接受与 session 一致的显式声明；
                // 未知角色值一律 400。
                payload_role = Some(match v {
                    "operator" => Role::Operator,
                    "viewer" => Role::Viewer,
                    "admin" => Role::Admin,
                    "superuser" => Role::Superuser,
                    _ => return Err(StatusCode::BadRequest),
                });
            }
            "action" => {
                action = Some(match v {
                    "StatusQuery" => ApiAction::StatusQuery,
                    "SloQuery" => ApiAction::SloQuery,
                    "AgentList" => ApiAction::AgentList,
                    "AgentStart" => ApiAction::AgentStart,
                    "AgentStop" => ApiAction::AgentStop,
                    "AgentRestart" => ApiAction::AgentRestart,
                    "ConfigGet" => ApiAction::ConfigGet,
                    "ConfigSet" => ApiAction::ConfigSet,
                    "LogQuery" => ApiAction::LogQuery,
                    "SupportBundleExport" => ApiAction::SupportBundleExport,
                    "CrashDumpSymbolize" => ApiAction::CrashDumpSymbolize,
                    "UpdateCheck" => ApiAction::UpdateCheck,
                    "UpdateApply" => ApiAction::UpdateApply,
                    "UpdateRollback" => ApiAction::UpdateRollback,
                    "MaintenanceEnter" => ApiAction::MaintenanceEnter,
                    "MaintenanceExit" => ApiAction::MaintenanceExit,
                    "BackupCreate" => ApiAction::BackupCreate,
                    "Restore" => ApiAction::BackupRestore,
                    "Revoke" => ApiAction::Revoke,
                    "FactoryReset" => ApiAction::FactoryReset,
                    "SecureErase" => ApiAction::SecureErase,
                    // 未知/拼错 action → 400（不再映射 StatusQuery）。
                    _ => return Err(StatusCode::BadRequest),
                });
            }
            "payload" => payload = String::from(v),
            _ => {}
        }
    }
    if let Some(payload_role) = payload_role {
        if payload_role != session_role {
            // payload 自报角色与认证会话冲突 → 拒绝（防自报提权）。
            return Err(StatusCode::BadRequest);
        }
    }
    Ok(ApiRequest {
        api_version: ApiVersion(version),
        role: session_role,
        action: action.ok_or(StatusCode::BadRequest)?,
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

    /// 真实执行器（固定 Ok 已删除——调用方必须注入；失败显式返回）。
    fn executor(
        executed: &std::rc::Rc<core::cell::RefCell<Vec<String>>>,
    ) -> impl FnMut(&ApiAction, &str) -> (StatusCode, String) + '_ {
        let state = std::rc::Rc::clone(executed);
        move |action: &ApiAction, payload: &str| {
            state.borrow_mut().push(format!("{action:?}"));
            (StatusCode::Ok, String::from(payload))
        }
    }

    /// 验收：远程命令经 MQTT RPC 端到端（请求→RBAC→真实执行→响应发布）。
    #[test]
    fn test_remote_command_roundtrip() {
        let published: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let executed: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let pub_state = std::rc::Rc::clone(&published);
        let publish = move |_t: &str, payload: &str| {
            pub_state.borrow_mut().push(payload.to_string());
            Ok(())
        };
        // 一条 ConfigSet 请求。
        let requests: Vec<String> = vec![String::from(
            r#"{"version":1,"action":"ConfigSet","payload":"{\"param\":1}"}"#,
        )];
        let mut idx = 0;
        let subscribe = move |_t: &str| {
            let r = requests.get(idx).cloned();
            idx += 1;
            r
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let mut exec = executor(&executed);
        let result = transport
            .poll_once(&mut api, Role::Operator, 1_700_000_000, &mut exec)
            .expect("响应")
            .expect("发布成功");
        assert_eq!(result.1, 200, "Ok=200");
        assert_eq!(published.borrow().len(), 1, "响应必须发布到 resp topic");
        assert_eq!(executed.borrow().len(), 1, "真实 executor 必须被调用");
    }

    /// 验收：RBAC 拒绝未授权命令（Viewer 的 ConfigSet → 403）。
    #[test]
    fn test_rbac_rejects_unauthorized() {
        let publish = |_t: &str, _p: &str| Ok(());
        let subscribe = |_t: &str| {
            Some(String::from(
                r#"{"version":1,"action":"ConfigSet","payload":"x"}"#,
            ))
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let mut exec = |_a: &ApiAction, _p: &str| (StatusCode::InternalError, String::new());
        let (_, status) = transport
            .poll_once(&mut api, Role::Viewer, 1_700_000_000, &mut exec)
            .expect("拒绝响应")
            .expect("发布成功");
        assert_eq!(status, 403, "Forbidden=403——Viewer 必须被 RBAC 拒绝");
    }

    /// 验收：畸形/未知 action fail-closed（400，不默认 StatusQuery）。
    #[test]
    fn test_unknown_action_fail_closed() {
        let published: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let pub_state = std::rc::Rc::clone(&published);
        let publish = move |_t: &str, payload: &str| {
            pub_state.borrow_mut().push(payload.to_string());
            Ok(())
        };
        let subscribe = |_t: &str| {
            Some(String::from(
                r#"{"version":1,"action":"HackTheSystem","payload":"x"}"#,
            ))
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let mut exec = |_a: &ApiAction, _p: &str| (StatusCode::InternalError, String::new());
        let (_, status) = transport
            .poll_once(&mut api, Role::Operator, 1_700_000_000, &mut exec)
            .expect("响应")
            .expect("发布成功");
        assert_eq!(status, 400, "未知 action 必须 400（不默认 StatusQuery）");
    }

    /// 验收：payload 自报 role 冲突 → 400（不再信任自报角色）。
    #[test]
    fn test_payload_role_conflict_rejected() {
        let published: std::rc::Rc<core::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let pub_state = std::rc::Rc::clone(&published);
        let publish = move |_t: &str, payload: &str| {
            pub_state.borrow_mut().push(payload.to_string());
            Ok(())
        };
        // payload 自称 superuser，但 transport 会话是 viewer。
        let subscribe = |_t: &str| {
            Some(String::from(
                r#"{"version":1,"role":"superuser","action":"FactoryReset","payload":"x"}"#,
            ))
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let mut exec = |_a: &ApiAction, _p: &str| (StatusCode::InternalError, String::new());
        let (_, status) = transport
            .poll_once(&mut api, Role::Viewer, 1_700_000_000, &mut exec)
            .expect("响应")
            .expect("发布成功");
        assert_eq!(status, 400, "自报角色与认证会话冲突必须 400");
    }

    /// 验收：publish 失败传播（不吞错误）。
    #[test]
    fn test_publish_failure_propagates() {
        let publish = |_t: &str, _p: &str| Err(());
        let subscribe = |_t: &str| {
            Some(String::from(
                r#"{"version":1,"action":"StatusQuery","payload":""}"#,
            ))
        };
        let mut transport = MqttRpcTransport::new("dev-1", publish, subscribe);
        let mut api = api();
        let mut exec = |_a: &ApiAction, _p: &str| (StatusCode::Ok, String::new());
        let result = transport
            .poll_once(&mut api, Role::Operator, 1_700_000_000, &mut exec)
            .expect("有请求");
        assert!(result.is_err(), "publish 失败必须可见（Err），不伪装成功");
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
        let mut exec = |_a: &ApiAction, _p: &str| (StatusCode::InternalError, String::new());
        let result = transport
            .poll_once(&mut api, Role::Operator, 1_700_000_000, &mut exec)
            .expect("畸形请求必须产生响应（不静默）");
        assert!(result.is_ok());
        assert_eq!(published.borrow().len(), 1);
    }
}
