//! CLI 命令解析器 (OBS-009).
//!
//! 将文本行解析为结构化命令。支持动词-对象语法和命名参数。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

/// CLI 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// 空输入。
    EmptyInput,
    /// 未知命令。
    UnknownVerb(String),
    /// 未知对象。
    UnknownObject(String),
    /// 缺少必需参数。
    MissingArgument(String),
    /// 参数格式错误。
    BadArgument(String),
    /// 参数值缺失。
    MissingValue(String),
}

/// 命令动词。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandVerb {
    /// 查询状态（不带对象时为系统状态）。
    Status,
    /// 列出资源。
    List,
    /// 启动资源。
    Start,
    /// 停止资源。
    Stop,
    /// 重启资源。
    Restart,
    /// 读取配置。
    Get,
    /// 设置配置。
    Set,
    /// 查询日志。
    Logs,
    /// 检查更新。
    Check,
    /// 应用更新。
    Apply,
    /// 回滚更新。
    Rollback,
    /// 创建资源（备份/支持包）。
    Create,
    /// 恢复资源（备份恢复）。
    Restore,
    /// 导出资源。
    Export,
    /// 进入维护模式。
    Enter,
    /// 退出维护模式。
    Exit,
    /// 吊销。
    Revoke,
    /// 工厂重置。
    Reset,
    /// 安全擦除。
    Erase,
    /// 帮助。
    Help,
}

impl CommandVerb {
    /// 从文本解析动词。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "status" => Some(Self::Status),
            "list" | "ls" => Some(Self::List),
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            "get" => Some(Self::Get),
            "set" => Some(Self::Set),
            "logs" => Some(Self::Logs),
            "check" => Some(Self::Check),
            "apply" => Some(Self::Apply),
            "rollback" => Some(Self::Rollback),
            "create" => Some(Self::Create),
            "restore" => Some(Self::Restore),
            "export" => Some(Self::Export),
            "enter" => Some(Self::Enter),
            "exit" => Some(Self::Exit),
            "revoke" => Some(Self::Revoke),
            "reset" => Some(Self::Reset),
            "erase" => Some(Self::Erase),
            "help" => Some(Self::Help),
            _ => None,
        }
    }
}

/// 命令对象（操作目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandObject {
    /// 系统级（无特定对象）。
    System,
    /// Agent。
    Agent,
    /// 配置。
    Config,
    /// SLO 指标。
    Slo,
    /// 更新。
    Update,
    /// 维护窗口。
    Maintenance,
    /// 备份。
    Backup,
    /// 支持包。
    SupportBundle,
    /// 崩溃转储。
    CrashDump,
    /// 安全（证书/能力）。
    Security,
}

impl CommandObject {
    /// 从文本解析对象。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "agent" | "agents" => Some(Self::Agent),
            "config" | "cfg" => Some(Self::Config),
            "slo" => Some(Self::Slo),
            "update" | "updates" => Some(Self::Update),
            "maintenance" | "maint" => Some(Self::Maintenance),
            "backup" => Some(Self::Backup),
            "support-bundle" | "bundle" => Some(Self::SupportBundle),
            "crash-dump" | "crash" => Some(Self::CrashDump),
            "security" | "sec" => Some(Self::Security),
            _ => None,
        }
    }
}

/// 解析后的 CLI 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCommand {
    /// 动词。
    pub verb: CommandVerb,
    /// 对象。
    pub object: CommandObject,
    /// 命名参数（--key value 对）。
    pub args: Vec<(String, String)>,
    /// 位置参数（payload）。
    pub payload: String,
}

impl CliCommand {
    /// 获取命名参数值。
    pub fn get_arg(&self, key: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// 检查是否有 `--confirm` 标志。
    pub fn has_confirm(&self) -> bool {
        self.args.iter().any(|(k, _)| k == "confirm")
    }
}

/// 解析文本行为 CLI 命令。
///
/// 支持两种格式：
/// 1. 裸动词（系统命令）：`status`, `help`, `logs`, `slo`
/// 2. 对象 + 动词：`<object> <verb> [--<key> <value>...] [payload]`
///    例如：`agent list`, `config set --key K --value V`, `update apply --version V`
///
/// 特殊别名：
/// - `factory-reset` → `system reset --confirm`
/// - `secure-erase` → `security erase --confirm`
pub fn parse_command(input: &str) -> Result<CliCommand, CliError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CliError::EmptyInput);
    }

    // 特殊别名
    let input = match input {
        "factory-reset" => {
            return Ok(CliCommand {
                verb: CommandVerb::Reset,
                object: CommandObject::System,
                args: vec![("confirm".to_string(), "true".to_string())],
                payload: String::new(),
            })
        }
        "secure-erase" => {
            return Ok(CliCommand {
                verb: CommandVerb::Erase,
                object: CommandObject::Security,
                args: vec![("confirm".to_string(), "true".to_string())],
                payload: String::new(),
            })
        }
        s => s,
    };

    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(CliError::EmptyInput);
    }

    let mut iter = tokens.into_iter().peekable();

    let first = iter.next().ok_or(CliError::EmptyInput)?;

    // 尝试解析为裸动词（系统命令）
    if let Some(verb) = CommandVerb::from_str(&first) {
        let object = match verb {
            CommandVerb::Status => CommandObject::System,
            CommandVerb::Help => CommandObject::System,
            CommandVerb::Logs => CommandObject::System,
            _ => CommandObject::System,
        };
        // 剩余 token 作为参数
        let (args, payload) = parse_args(iter);
        return Ok(CliCommand {
            verb,
            object,
            args,
            payload,
        });
    }

    // 尝试解析为对象 + 动词
    if let Some(object) = CommandObject::from_str(&first) {
        // 特殊：slo 作为独立命令 → verb=Status, object=Slo
        if first == "slo" && iter.peek().is_none() {
            return Ok(CliCommand {
                verb: CommandVerb::Status,
                object: CommandObject::Slo,
                args: Vec::new(),
                payload: String::new(),
            });
        }

        // 下一个 token 是动词
        let verb_str = iter.next().unwrap_or_default();
        if verb_str.is_empty() {
            // 只有对象，无动词 → 默认 List 或 Status
            let verb = match object {
                CommandObject::Agent => CommandVerb::List,
                _ => CommandVerb::Status,
            };
            let (args, payload) = parse_args(iter);
            return Ok(CliCommand {
                verb,
                object,
                args,
                payload,
            });
        }

        let verb = CommandVerb::from_str(&verb_str)
            .ok_or_else(|| CliError::UnknownVerb(verb_str.clone()))?;
        let (args, payload) = parse_args(iter);
        return Ok(CliCommand {
            verb,
            object,
            args,
            payload,
        });
    }

    Err(CliError::UnknownVerb(first))
}

/// 解析命名参数和位置参数。
fn parse_args<I>(iter: I) -> (Vec<(String, String)>, String)
where
    I: Iterator<Item = String>,
{
    let mut args: Vec<(String, String)> = Vec::new();
    let mut payload_parts: Vec<String> = Vec::new();
    let mut iter = iter.peekable();

    while let Some(token) = iter.next() {
        if token.starts_with("--") {
            let key = token.trim_start_matches("--").to_string();
            if key == "confirm" {
                args.push((key, "true".to_string()));
                continue;
            }
            if let Some(value) = iter.next() {
                args.push((key, value));
            } else {
                args.push((key, String::new()));
            }
        } else {
            payload_parts.push(token);
        }
    }

    (args, payload_parts.join(" "))
}

/// 分词器：按空格分割，支持引号。
fn tokenize(input: &str) -> Result<Vec<String>, CliError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(core::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_quotes {
        return Err(CliError::BadArgument("unclosed quote".to_string()));
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status() {
        let cmd = parse_command("status").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Status);
        assert_eq!(cmd.object, CommandObject::System);
    }

    #[test]
    fn parse_agent_list() {
        let cmd = parse_command("agent list").unwrap();
        assert_eq!(cmd.verb, CommandVerb::List);
        assert_eq!(cmd.object, CommandObject::Agent);
    }

    #[test]
    fn parse_agent_start_with_id() {
        let cmd = parse_command("agent start --id edge-agent").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Start);
        assert_eq!(cmd.object, CommandObject::Agent);
        assert_eq!(cmd.get_arg("id"), Some("edge-agent"));
    }

    #[test]
    fn parse_config_set() {
        let cmd = parse_command("config set --key system.node_id --value node-01").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Set);
        assert_eq!(cmd.object, CommandObject::Config);
        assert_eq!(cmd.get_arg("key"), Some("system.node_id"));
        assert_eq!(cmd.get_arg("value"), Some("node-01"));
    }

    #[test]
    fn parse_config_get() {
        let cmd = parse_command("config get --key system.node_id").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Get);
        assert_eq!(cmd.object, CommandObject::Config);
        assert_eq!(cmd.get_arg("key"), Some("system.node_id"));
    }

    #[test]
    fn parse_update_check() {
        let cmd = parse_command("update check").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Check);
        assert_eq!(cmd.object, CommandObject::Update);
    }

    #[test]
    fn parse_update_apply_with_version() {
        let cmd = parse_command("update apply --version 0.200.0").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Apply);
        assert_eq!(cmd.object, CommandObject::Update);
        assert_eq!(cmd.get_arg("version"), Some("0.200.0"));
    }

    #[test]
    fn parse_backup_create() {
        let cmd = parse_command("backup create").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Create);
        assert_eq!(cmd.object, CommandObject::Backup);
    }

    #[test]
    fn parse_support_bundle_export() {
        let cmd = parse_command("support-bundle export").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Export);
        assert_eq!(cmd.object, CommandObject::SupportBundle);
    }

    #[test]
    fn parse_factory_reset_alias() {
        let cmd = parse_command("factory-reset").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Reset);
        assert_eq!(cmd.object, CommandObject::System);
        assert!(cmd.has_confirm());
    }

    #[test]
    fn parse_secure_erase_alias() {
        let cmd = parse_command("secure-erase").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Erase);
        assert_eq!(cmd.object, CommandObject::Security);
        assert!(cmd.has_confirm());
    }

    #[test]
    fn parse_maintenance_enter() {
        let cmd = parse_command("maintenance enter").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Enter);
        assert_eq!(cmd.object, CommandObject::Maintenance);
    }

    #[test]
    fn parse_slo_command() {
        let cmd = parse_command("slo").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Status);
        assert_eq!(cmd.object, CommandObject::Slo);
    }

    #[test]
    fn parse_logs_command() {
        let cmd = parse_command("logs").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Logs);
        assert_eq!(cmd.object, CommandObject::System);
    }

    #[test]
    fn parse_help() {
        let cmd = parse_command("help").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Help);
    }

    #[test]
    fn parse_empty_input_rejected() {
        assert_eq!(parse_command(""), Err(CliError::EmptyInput));
        assert_eq!(parse_command("   "), Err(CliError::EmptyInput));
    }

    #[test]
    fn parse_unknown_verb_rejected() {
        assert!(matches!(
            parse_command("foobar"),
            Err(CliError::UnknownVerb(_))
        ));
    }

    #[test]
    fn parse_quoted_value() {
        let cmd = parse_command(r#"config set --key name --value "my node""#).unwrap();
        assert_eq!(cmd.get_arg("value"), Some("my node"));
    }

    #[test]
    fn parse_revoke_command() {
        let cmd = parse_command("security revoke --cert-id agent-001").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Revoke);
        assert_eq!(cmd.object, CommandObject::Security);
        assert_eq!(cmd.get_arg("cert-id"), Some("agent-001"));
    }

    #[test]
    fn parse_backup_restore() {
        let cmd = parse_command("backup restore --id backup-2026-08-04").unwrap();
        assert_eq!(cmd.verb, CommandVerb::Restore);
        assert_eq!(cmd.object, CommandObject::Backup);
        assert_eq!(cmd.get_arg("id"), Some("backup-2026-08-04"));
    }
}
