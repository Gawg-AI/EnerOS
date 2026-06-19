//! EnerOS 管理 CLI (enerosctl)
//!
//! 用于查询和管理 Agent 进程状态、EventBus 状态的管理工具。
//! 通过 TCP 控制通道（127.0.0.1:9876）或本地状态文件与 EnerOS 内核交互。
//!
//! 子命令:
//! - `agent list / start / stop / status / restart` — Agent 进程管理
//! - `eventbus status / subscribe` — EventBus 事件总线管理
//! - `system info` — 系统信息汇总

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;
mod format;

/// EnerOS 管理 CLI
#[derive(Parser, Debug)]
#[command(
    name = "enerosctl",
    version,
    about = "EnerOS 管理 CLI — 查询和管理 Agent 进程与 EventBus"
)]
struct Cli {
    /// IPC 控制 socket 路径（当前使用 TCP 控制通道，此选项保留供未来 Unix socket 使用）
    #[arg(long, global = true, default_value = "/var/run/eneros/control.sock")]
    socket: String,

    /// 详细输出（启用 debug 日志）
    #[arg(short, long, global = true)]
    verbose: bool,

    /// 子命令
    #[command(subcommand)]
    command: Commands,
}

/// 顶层子命令
#[derive(Subcommand, Debug)]
enum Commands {
    /// Agent 进程管理
    Agent {
        /// Agent 子命令
        #[command(subcommand)]
        action: AgentCommands,
    },
    /// EventBus 事件总线管理
    Eventbus {
        /// EventBus 子命令
        #[command(subcommand)]
        action: EventbusCommands,
    },
    /// 系统信息
    System {
        /// System 子命令
        #[command(subcommand)]
        action: SystemCommands,
    },
    /// 网络配置管理
    Network {
        /// Network 子命令
        #[command(subcommand)]
        action: NetworkCommands,
    },
    /// 日志管理
    Log {
        /// Log 子命令
        #[command(subcommand)]
        action: LogCommands,
    },
    /// 设备管理
    Device {
        /// Device 子命令
        #[command(subcommand)]
        action: DeviceCommands,
    },
    /// 审计日志管理
    Audit {
        /// Audit 子命令
        #[command(subcommand)]
        action: AuditCommands,
    },
    /// 时间同步管理
    Time {
        /// Time 子命令
        #[command(subcommand)]
        action: TimeCommands,
    },
    /// OTA 更新管理
    #[command(subcommand)]
    Update(UpdateCommands),
}

/// Agent 子命令
#[derive(Subcommand, Debug)]
enum AgentCommands {
    /// 列出所有注册的 Agent
    List,
    /// 启动指定 Agent
    Start {
        /// Agent ID
        agent_id: String,
    },
    /// 停止指定 Agent
    Stop {
        /// Agent ID
        agent_id: String,
    },
    /// 查询指定 Agent 状态
    Status {
        /// Agent ID
        agent_id: String,
    },
    /// 重启指定 Agent
    Restart {
        /// Agent ID
        agent_id: String,
    },
}

/// EventBus 子命令
#[derive(Subcommand, Debug)]
enum EventbusCommands {
    /// 查询 EventBusBroker 状态
    Status,
    /// 订阅事件（实时打印，按 Ctrl+C 退出）
    Subscribe {
        /// 订阅主题（可选，不指定则订阅所有事件）
        topic: Option<String>,
    },
}

/// System 子命令
#[derive(Subcommand, Debug)]
enum SystemCommands {
    /// 显示系统信息（Agent 数量、状态分布、EventBus 连接状态）
    Info,
}

/// Network 子命令
#[derive(Subcommand, Debug)]
enum NetworkCommands {
    /// 显示所有网络接口状态
    Status,
    /// 显示接口配置
    Config {
        /// 接口名称（可选，不指定则显示所有接口配置）
        interface: Option<String>,
    },
    /// 显示防火墙规则
    Firewall {
        /// Firewall 子命令
        #[command(subcommand)]
        action: Option<FirewallCommands>,
    },
    /// 显示 bonding 状态
    Bond {
        /// Bond 接口名称（可选，不指定则显示所有 bond）
        interface: Option<String>,
    },
}

/// Firewall 子命令
#[derive(Subcommand, Debug)]
enum FirewallCommands {
    /// 列出所有防火墙规则
    List,
    /// 显示防火墙默认策略
    Policy,
}

/// Log 子命令
#[derive(Subcommand, Debug)]
enum LogCommands {
    /// 查看最近 N 行日志
    Tail {
        /// 日志分类（system/agent/protocol/security/audit，可选）
        category: Option<String>,
        /// 显示行数
        #[arg(short, long, default_value = "50")]
        lines: usize,
        /// 实时跟踪日志输出（按 Ctrl+C 退出）
        #[arg(short = 'f', long)]
        follow: bool,
        /// 输出原始 JSONL 行（不经过格式化）
        #[arg(long)]
        json: bool,
    },
    /// 搜索日志
    Search {
        /// 搜索模式（关键字）
        pattern: String,
        /// 日志分类（可选，指定 all 跨分类搜索）
        #[arg(short, long)]
        category: Option<String>,
        /// 按日志级别过滤（trace/debug/info/warn/error）
        #[arg(short = 'l', long)]
        level: Option<String>,
        /// 起始时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        since: Option<String>,
        /// 结束时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        until: Option<String>,
        /// 按来源过滤
        #[arg(short = 's', long)]
        source: Option<String>,
        /// 输出原始 JSONL 行（不经过格式化）
        #[arg(long)]
        json: bool,
    },
    /// 动态调整日志级别（不指定 level 时查询当前级别）
    Level {
        /// 目标（global 或分类名）
        target: String,
        /// 日志级别（trace/debug/info/warn/error），不指定则查询当前级别
        level: Option<String>,
    },
    /// 导出日志
    Export {
        /// 开始时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        start: Option<String>,
        /// 结束时间
        #[arg(long)]
        end: Option<String>,
        /// 输出格式（json/text）
        #[arg(long, default_value = "json")]
        format: String,
        /// 日志分类（可选）
        #[arg(short, long)]
        category: Option<String>,
        /// 输出文件路径（不指定则输出到 stdout）
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// 触发日志轮转（通过 SIGHUP 通知 eneros-init 重载）
    Rotate {
        /// 日志分类（system/agent/protocol/security/audit）
        category: String,
    },
}

/// Device 子命令
#[derive(Subcommand, Debug)]
enum DeviceCommands {
    /// 列出所有设备
    List {
        /// 按类型过滤（serial/usb/gpio/i2c/spi/net，可选）
        #[arg(short, long)]
        r#type: Option<String>,
    },
    /// 显示设备详情
    Info {
        /// 设备名称或路径
        device: String,
    },
    /// 配置设备参数
    Config {
        /// 设备名称或路径
        device: String,
        /// 串口预设（iec104_ft12/modbus_rtu/modbus_rtu_high）
        #[arg(long)]
        preset: Option<String>,
        /// 波特率
        #[arg(long)]
        baud: Option<u32>,
    },
    /// 实时监控设备状态（按 Ctrl+C 退出）
    Monitor,
}

/// Audit 子命令
#[derive(Subcommand, Debug)]
enum AuditCommands {
    /// 列出审计日志
    List {
        /// 起始时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        since: Option<String>,
        /// 结束时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        until: Option<String>,
        /// 最大返回条数
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 验证审计日志完整性
    Verify,
    /// 搜索审计日志
    Search {
        /// 按操作者过滤
        #[arg(long)]
        actor: Option<String>,
        /// 按动作类型过滤（login/logout/config_change/agent_control/...）
        #[arg(long)]
        action: Option<String>,
        /// 按结果过滤（success/failure/denied）
        #[arg(long)]
        result: Option<String>,
        /// 起始时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        since: Option<String>,
        /// 结束时间（ISO 8601 或 YYYY-MM-DD）
        #[arg(long)]
        until: Option<String>,
        /// 最大返回条数
        #[arg(long)]
        limit: Option<usize>,
    },
}

/// Time 子命令
#[derive(Subcommand, Debug)]
enum TimeCommands {
    /// 显示时间同步状态
    Status,
    /// 设置时钟源
    SetSource {
        /// 时钟源（ptp/ntp/local）
        source: String,
    },
    /// 手动触发时间同步
    Sync,
}

/// Update 子命令
#[derive(Subcommand, Debug)]
enum UpdateCommands {
    /// 查询当前槽位状态
    Status,
    /// 应用 OTA 更新包
    Apply {
        /// 更新包路径或 URL
        bundle: String,
    },
    /// 回滚到上一已知良好槽位
    Rollback,
    /// 列出可用的更新包
    List,
    /// 生成 Ed25519 密钥对
    GenKeys {
        /// 密钥输出目录（默认 /etc/eneros/keys/）
        #[arg(long, default_value = "/etc/eneros/keys/")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // 初始化日志：verbose 模式输出 debug，否则只输出 warn 以上
    let filter_level = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter_level)),
        )
        .init();

    let socket = cli.socket.as_str();

    match cli.command {
        Commands::Agent { action } => match action {
            AgentCommands::List => commands::cmd_agent_list(socket).await,
            AgentCommands::Start { agent_id } => {
                commands::cmd_agent_start(socket, &agent_id).await
            }
            AgentCommands::Stop { agent_id } => {
                commands::cmd_agent_stop(socket, &agent_id).await
            }
            AgentCommands::Status { agent_id } => {
                commands::cmd_agent_status(socket, &agent_id).await
            }
            AgentCommands::Restart { agent_id } => {
                commands::cmd_agent_restart(socket, &agent_id).await
            }
        },
        Commands::Eventbus { action } => match action {
            EventbusCommands::Status => commands::cmd_eventbus_status(socket).await,
            EventbusCommands::Subscribe { topic } => {
                commands::cmd_eventbus_subscribe(socket, topic.as_deref()).await
            }
        },
        Commands::System { action } => match action {
            SystemCommands::Info => commands::cmd_system_info(socket).await,
        },
        Commands::Network { action } => match action {
            NetworkCommands::Status => commands::cmd_network_status().await,
            NetworkCommands::Config { interface } => {
                commands::cmd_network_config(interface.as_deref()).await
            }
            NetworkCommands::Firewall { action } => match action {
                Some(FirewallCommands::List) => commands::cmd_network_firewall_list().await,
                Some(FirewallCommands::Policy) => commands::cmd_network_firewall_policy().await,
                None => commands::cmd_network_firewall_list().await,
            },
            NetworkCommands::Bond { interface } => {
                commands::cmd_network_bond_status(interface.as_deref()).await
            }
        },
        Commands::Log { action } => match action {
            LogCommands::Tail {
                category,
                lines,
                follow,
                json,
            } => commands::cmd_log_tail(category.as_deref(), lines, follow, json).await,
            LogCommands::Search {
                pattern,
                category,
                level,
                since,
                until,
                source,
                json,
            } => {
                commands::cmd_log_search(
                    &pattern,
                    category.as_deref(),
                    level.as_deref(),
                    since.as_deref(),
                    until.as_deref(),
                    source.as_deref(),
                    json,
                )
                .await
            }
            LogCommands::Level { target, level } => {
                commands::cmd_log_level(&target, level.as_deref()).await
            }
            LogCommands::Export {
                start,
                end,
                format,
                category,
                output,
            } => {
                commands::cmd_log_export(
                    start.as_deref(),
                    end.as_deref(),
                    &format,
                    category.as_deref(),
                    output.as_deref(),
                )
                .await
            }
            LogCommands::Rotate { category } => commands::cmd_log_rotate(&category).await,
        },
        Commands::Device { action } => match action {
            DeviceCommands::List { r#type } => commands::cmd_device_list(r#type.as_deref()).await,
            DeviceCommands::Info { device } => commands::cmd_device_info(&device).await,
            DeviceCommands::Config { device, preset, baud } => {
                commands::cmd_device_config(&device, preset.as_deref(), baud).await
            }
            DeviceCommands::Monitor => commands::cmd_device_monitor().await,
        },
        Commands::Audit { action } => match action {
            AuditCommands::List { since, until, limit } => {
                commands::cmd_audit_list(since.as_deref(), until.as_deref(), limit).await
            }
            AuditCommands::Verify => commands::cmd_audit_verify().await,
            AuditCommands::Search {
                actor,
                action,
                result,
                since,
                until,
                limit,
            } => {
                commands::cmd_audit_search(
                    actor.as_deref(),
                    action.as_deref(),
                    result.as_deref(),
                    since.as_deref(),
                    until.as_deref(),
                    limit,
                )
                .await
            }
        },
        Commands::Time { action } => match action {
            TimeCommands::Status => commands::cmd_time_status().await,
            TimeCommands::SetSource { source } => commands::cmd_time_set_source(&source).await,
            TimeCommands::Sync => commands::cmd_time_sync().await,
        },
        Commands::Update(cmd) => commands::cmd_update(cmd).await,
    }
}
