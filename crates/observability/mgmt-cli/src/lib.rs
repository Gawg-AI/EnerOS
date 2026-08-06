//! EnerOS 本地管理 CLI — 全关键操作/API conformance (OBS-009).
//!
//! MASTER §21.13 OBS-009: "local CLI — 全关键操作/API conformance"。
//! G8 Exit: "本地控制台可执行全关键操作，且每个命令映射到管理 API"。
//!
//! 本 crate 提供：
//! - [`CliCommand`] — 解析后的 CLI 命令。
//! - [`parse_command`] — 从文本行解析命令。
//! - [`CliDispatcher`] — 将 CLI 命令映射到管理 API 请求并执行。
//! - [`CliError`] — CLI 错误。
//!
//! # 设计
//!
//! 1. 本地控制台读取一行文本输入。
//! 2. `parse_command` 解析为 `CliCommand`（动词 + 对象 + 参数）。
//! 3. `CliDispatcher::dispatch` 将命令映射为 `ApiRequest`。
//! 4. 调用 `ManagementApi::handle` 执行请求。
//! 5. 格式化响应为文本输出。
//!
//! # 命令格式
//!
//! ```text
//! <verb> <object> [--<flag> <value>...] [payload]
//! ```
//!
//! 示例：
//! - `status` — 查询系统状态
//! - `agent list` — 列出 Agent
//! - `agent start --id edge-agent` — 启动指定 Agent
//! - `config get --key system.node_id` — 读取配置
//! - `config set --key system.node_id --value node-01` — 写入配置
//! - `update check` — 检查更新
//! - `update apply --version 0.200.0` — 应用更新
//! - `backup create` — 创建备份
//! - `support-bundle export` — 导出支持包
//! - `factory-reset --confirm` — 工厂重置
//! - `help` — 显示帮助

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dispatcher;
pub mod parser;

use alloc::string::String;

pub use dispatcher::CliDispatcher;
pub use parser::{parse_command, CliCommand, CliError, CommandObject, CommandVerb};

/// CLI 执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliResult {
    /// 输出文本。
    pub output: String,
    /// 是否成功。
    pub success: bool,
}

impl CliResult {
    /// 成功结果。
    pub fn ok(output: &str) -> Self {
        Self {
            output: String::from(output),
            success: true,
        }
    }

    /// 错误结果。
    pub fn error(message: &str) -> Self {
        Self {
            output: String::from(message),
            success: false,
        }
    }
}
