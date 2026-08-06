//! CLI 命令到管理 API 的映射分发器 (OBS-009).
//!
//! 每个 CLI 命令映射到一个 `ApiRequest`，通过 `ManagementApi::handle` 执行。
//! 确保 CLI 与管理 API 的操作语义一致（API conformance）。

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_mgmt_api::{
    ApiAction, ApiRequest, ApiResponse, ApiVersion, IdempotencyStore, ManagementApi,
    RateLimitConfig, RateLimiter, RbacPolicy, Role, StatusCode, CURRENT_VERSION,
};

use crate::parser::{CliCommand, CliError, CommandObject, CommandVerb};
use crate::CliResult;

/// CLI 分发器。
///
/// 将解析后的 `CliCommand` 映射为 `ApiRequest`，
/// 通过 `ManagementApi` 执行，并格式化响应。
pub struct CliDispatcher {
    api: ManagementApi,
    /// 当前会话角色。
    role: Role,
}

impl CliDispatcher {
    /// 创建分发器。
    ///
    /// 本地控制台默认使用 `Admin` 角色。
    /// 恢复模式下可使用 `Superuser`。
    pub fn new(role: Role) -> Self {
        let policy = RbacPolicy::new();
        // 所有角色的默认权限已通过角色层次继承
        // 这里不添加额外授权
        let idempotency = IdempotencyStore::new(64);
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 60,
            window_secs: 60,
        });
        Self {
            api: ManagementApi::new(policy, idempotency, rate_limiter),
            role,
        }
    }

    /// 分发并执行 CLI 命令。
    ///
    /// `now_unix` 为当前 Unix 时间戳（秒）。
    /// `handler` 接收 API action 和 payload，执行实际操作并返回 (status, body)。
    pub fn dispatch<F>(&mut self, cmd: &CliCommand, now_unix: u64, handler: F) -> CliResult
    where
        F: FnOnce(&ApiAction, &str) -> (StatusCode, String),
    {
        // help 命令直接返回
        if cmd.verb == CommandVerb::Help {
            return CliResult::ok(help_text());
        }

        // 映射 CLI 命令到 API action
        let action = match map_command_to_action(cmd) {
            Ok(a) => a,
            Err(e) => return CliResult::error(&format!("error: {:?}", e)),
        };

        // 构造 payload
        let payload = build_payload(cmd);

        // 构造 API 请求
        // v0.239.0（B5-02）：幂等键改内容哈希（SM3(action ‖ payload)），
        // 相同内容重放命中缓存，不同内容不命中。
        let idempotency_key = if is_mutating(action) {
            Some(eneros_mgmt_api::content_hash_key(action.name(), &payload))
        } else {
            None
        };

        let request = ApiRequest {
            api_version: ApiVersion(CURRENT_VERSION),
            role: self.role,
            action,
            payload,
            idempotency_key,
        };

        // 执行
        let response = self.api.handle(&request, now_unix, handler);

        format_response(&response)
    }
}

/// 将 CLI 命令映射到管理 API action。
fn map_command_to_action(cmd: &CliCommand) -> Result<ApiAction, CliError> {
    match (cmd.verb, cmd.object) {
        // 系统状态
        (CommandVerb::Status, CommandObject::System) => Ok(ApiAction::StatusQuery),
        (CommandVerb::Status, CommandObject::Slo) => Ok(ApiAction::SloQuery),

        // Agent 管理
        (CommandVerb::List, CommandObject::Agent) => Ok(ApiAction::AgentList),
        (CommandVerb::Start, CommandObject::Agent) => Ok(ApiAction::AgentStart),
        (CommandVerb::Stop, CommandObject::Agent) => Ok(ApiAction::AgentStop),
        (CommandVerb::Restart, CommandObject::Agent) => Ok(ApiAction::AgentRestart),

        // 配置管理
        (CommandVerb::Get, CommandObject::Config) => Ok(ApiAction::ConfigGet),
        (CommandVerb::Set, CommandObject::Config) => Ok(ApiAction::ConfigSet),

        // 日志
        (CommandVerb::Logs, CommandObject::System) => Ok(ApiAction::LogQuery),

        // 更新
        (CommandVerb::Check, CommandObject::Update) => Ok(ApiAction::UpdateCheck),
        (CommandVerb::Apply, CommandObject::Update) => Ok(ApiAction::UpdateApply),
        (CommandVerb::Rollback, CommandObject::Update) => Ok(ApiAction::UpdateRollback),

        // 维护
        (CommandVerb::Enter, CommandObject::Maintenance) => Ok(ApiAction::MaintenanceEnter),
        (CommandVerb::Exit, CommandObject::Maintenance) => Ok(ApiAction::MaintenanceExit),

        // 备份
        (CommandVerb::Create, CommandObject::Backup) => Ok(ApiAction::BackupCreate),
        (CommandVerb::Restore, CommandObject::Backup) => Ok(ApiAction::BackupRestore),

        // 支持包
        (CommandVerb::Export, CommandObject::SupportBundle) => Ok(ApiAction::SupportBundleExport),

        // 崩溃转储
        (CommandVerb::Export, CommandObject::CrashDump) => Ok(ApiAction::CrashDumpSymbolize),

        // 安全
        (CommandVerb::Revoke, CommandObject::Security) => Ok(ApiAction::Revoke),
        (CommandVerb::Reset, CommandObject::System) => {
            if !cmd.has_confirm() {
                return Err(CliError::MissingArgument(
                    "--confirm is required for factory reset".to_string(),
                ));
            }
            Ok(ApiAction::FactoryReset)
        }
        (CommandVerb::Erase, CommandObject::Security) => {
            if !cmd.has_confirm() {
                return Err(CliError::MissingArgument(
                    "--confirm is required for secure erase".to_string(),
                ));
            }
            Ok(ApiAction::SecureErase)
        }

        _ => Err(CliError::BadArgument(format!(
            "unsupported command: {:?} {:?}",
            cmd.verb, cmd.object
        ))),
    }
}

/// 构造请求 payload（JSON 格式）。
fn build_payload(cmd: &CliCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in &cmd.args {
        parts.push(format!(r#""{}":"{}""#, key, value));
    }
    if !cmd.payload.is_empty() {
        parts.push(format!(r#""payload":"{}""#, cmd.payload));
    }
    format!(r#"{{{}}}"#, parts.join(","))
}

/// 判断是否为写操作（需要幂等键）。
fn is_mutating(action: ApiAction) -> bool {
    !matches!(
        action,
        ApiAction::StatusQuery
            | ApiAction::SloQuery
            | ApiAction::AgentList
            | ApiAction::ConfigGet
            | ApiAction::LogQuery
            | ApiAction::UpdateCheck
    )
}

/// 格式化 API 响应为 CLI 输出。
fn format_response(response: &ApiResponse) -> CliResult {
    match response.status {
        StatusCode::Ok => {
            let prefix = if response.replayed {
                format!("[replayed req={}] ", response.request_id)
            } else {
                format!("[req={}] ", response.request_id)
            };
            CliResult::ok(&format!("{}{}", prefix, response.body))
        }
        StatusCode::Forbidden => CliResult::error(&format!(
            "error: permission denied (req={}) — {}",
            response.request_id, response.body
        )),
        StatusCode::TooManyRequests => CliResult::error(&format!(
            "error: rate limited (req={}) — {}",
            response.request_id, response.body
        )),
        StatusCode::UnsupportedVersion => CliResult::error(&format!(
            "error: unsupported API version (req={}) — {}",
            response.request_id, response.body
        )),
        status => CliResult::error(&format!(
            "error: HTTP {} (req={}) — {}",
            status.code(),
            response.request_id,
            response.body
        )),
    }
}

/// 帮助文本。
fn help_text() -> &'static str {
    "EnerOS Local CLI — Commands:\n\
     \n\
     System:\n\
       status              — query system status\n\
       slo                 — query SLO metrics\n\
       logs                — query system logs\n\
     \n\
     Agent:\n\
       agent list          — list all agents\n\
       agent start --id N  — start agent\n\
       agent stop --id N   — stop agent\n\
       agent restart --id N — restart agent\n\
     \n\
     Config:\n\
       config get --key K  — read config value\n\
       config set --key K --value V — write config value\n\
     \n\
     Update:\n\
       update check        — check for updates\n\
       update apply --version V — apply update\n\
       update rollback     — rollback last update\n\
     \n\
     Maintenance:\n\
       maintenance enter   — enter maintenance window\n\
       maintenance exit    — exit maintenance window\n\
     \n\
     Backup:\n\
       backup create       — create backup snapshot\n\
       backup restore --id B — restore from backup\n\
     \n\
     Diagnostics:\n\
       support-bundle export — export encrypted support bundle\n\
       crash-dump export --file F — symbolize crash dump\n\
     \n\
     Security:\n\
       security revoke --cert-id C — revoke certificate/capability\n\
       factory-reset       — factory reset (requires --confirm)\n\
       secure-erase        — secure erase (requires --confirm)\n\
     \n\
     help                 — show this help"
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;
    use crate::parse_command;

    fn make_dispatcher() -> CliDispatcher {
        CliDispatcher::new(Role::Admin)
    }

    #[test]
    fn dispatch_status() {
        let mut d = make_dispatcher();
        let cmd = parse_command("status").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::StatusQuery);
            (
                StatusCode::Ok,
                r#"{"uptime":3600,"version":"0.200.0"}"#.to_string(),
            )
        });
        assert!(result.success);
        assert!(result.output.contains("uptime"));
        assert!(result.output.contains("[req="));
    }

    #[test]
    fn dispatch_agent_list() {
        let mut d = make_dispatcher();
        let cmd = parse_command("agent list").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::AgentList);
            (
                StatusCode::Ok,
                r#"[{"id":"edge-1","status":"running"}]"#.to_string(),
            )
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_config_set() {
        let mut d = make_dispatcher();
        let cmd = parse_command("config set --key node_id --value node-01").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, payload| {
            assert_eq!(*action, ApiAction::ConfigSet);
            assert!(payload.contains(r#""key":"node_id""#));
            assert!(payload.contains(r#""value":"node-01""#));
            (StatusCode::Ok, r#"{"updated":true}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_factory_reset_requires_confirm() {
        let mut d = make_dispatcher();
        // 不带 --confirm
        let cmd = CliCommand {
            verb: CommandVerb::Reset,
            object: CommandObject::System,
            args: vec![],
            payload: String::new(),
        };
        let result = d.dispatch(&cmd, 1000, |_, _| panic!("should not execute"));
        assert!(!result.success);
        assert!(result.output.contains("confirm"));
    }

    #[test]
    fn dispatch_factory_reset_with_confirm() {
        let mut d = make_dispatcher();
        let cmd = parse_command("reset system --confirm").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::FactoryReset);
            (StatusCode::Ok, r#"{"reset":true}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_rbac_denied_for_viewer() {
        let mut d = CliDispatcher::new(Role::Viewer);
        let cmd = parse_command("config set --key k --value v").unwrap();
        let result = d.dispatch(&cmd, 1000, |_, _| panic!("should not execute"));
        assert!(!result.success);
        assert!(result.output.contains("permission denied"));
    }

    #[test]
    fn dispatch_rbac_allowed_for_viewer_read() {
        let mut d = CliDispatcher::new(Role::Viewer);
        let cmd = parse_command("status").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::StatusQuery);
            (StatusCode::Ok, r#"{"ok":true}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_idempotency_replay() {
        let mut d = make_dispatcher();
        let cmd = parse_command("config set --key k --value v").unwrap();
        let result1 = d.dispatch(&cmd, 1000, |_, _| {
            (StatusCode::Ok, r#"{"updated":true}"#.to_string())
        });
        assert!(result1.success);

        // 相同幂等键应返回缓存
        let result2 = d.dispatch(&cmd, 1001, |_, _| {
            (
                StatusCode::Ok,
                r#"{"updated":"SHOULD NOT SEE"}"#.to_string(),
            )
        });
        assert!(result2.success);
        assert!(result2.output.contains("replayed"));
    }

    #[test]
    fn dispatch_help() {
        let mut d = make_dispatcher();
        let cmd = parse_command("help").unwrap();
        let result = d.dispatch(&cmd, 1000, |_, _| panic!("help should not call handler"));
        assert!(result.success);
        assert!(result.output.contains("Commands:"));
    }

    #[test]
    fn dispatch_rate_limit() {
        let role = Role::Admin;
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, eneros_mgmt_api::Permission::SystemRead);
        let idempotency = IdempotencyStore::new(32);
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
        });
        let mut api = ManagementApi::new(policy, idempotency, rate_limiter);
        let mut d = CliDispatcher {
            api: core::mem::replace(
                &mut api,
                ManagementApi::new(
                    RbacPolicy::new(),
                    IdempotencyStore::new(1),
                    RateLimiter::new(RateLimitConfig {
                        max_requests: 1,
                        window_secs: 1,
                    }),
                ),
            ),
            role,
        };

        let cmd = parse_command("status").unwrap();
        d.dispatch(&cmd, 1000, |_, _| (StatusCode::Ok, "ok".to_string()));
        d.dispatch(&cmd, 1001, |_, _| (StatusCode::Ok, "ok".to_string()));
        let result = d.dispatch(&cmd, 1002, |_, _| (StatusCode::Ok, "ok".to_string()));
        assert!(!result.success);
        assert!(result.output.contains("rate limited"));
    }

    #[test]
    fn dispatch_backup_create() {
        let mut d = make_dispatcher();
        let cmd = parse_command("backup create").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::BackupCreate);
            (StatusCode::Ok, r#"{"backup_id":"b-001"}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_update_apply() {
        let mut d = make_dispatcher();
        let cmd = parse_command("update apply --version 0.201.0").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, payload| {
            assert_eq!(*action, ApiAction::UpdateApply);
            assert!(payload.contains(r#""version":"0.201.0""#));
            (StatusCode::Ok, r#"{"applied":true}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_maintenance_enter() {
        let mut d = make_dispatcher();
        let cmd = parse_command("maintenance enter").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::MaintenanceEnter);
            (StatusCode::Ok, r#"{"maintenance":true}"#.to_string())
        });
        assert!(result.success);
    }

    #[test]
    fn dispatch_support_bundle_export() {
        let mut d = make_dispatcher();
        let cmd = parse_command("support-bundle export").unwrap();
        let result = d.dispatch(&cmd, 1000, |action, _| {
            assert_eq!(*action, ApiAction::SupportBundleExport);
            (
                StatusCode::Ok,
                r#"{"bundle":"support-2026-08-04.ensb"}"#.to_string(),
            )
        });
        assert!(result.success);
    }
}
