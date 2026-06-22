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
mod shell;

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
    /// 协议适配器管理（v0.23.0）
    #[command(subcommand)]
    Protocol(ProtocolCommands),
    /// 安全管理（v0.24.0）
    #[command(subcommand)]
    Security(SecurityCommands),
    /// 高可用管理（v0.25.0）
    #[command(subcommand)]
    Ha(HaCommands),
    /// 插件管理（v0.27.0）
    #[command(subcommand)]
    Plugin(PluginCommands),
    /// 模拟器管理（v0.28.0 — Task 15）
    Simulator {
        /// Simulator 子命令
        #[command(subcommand)]
        action: SimulatorAction,
    },
    /// 启动交互式 shell（v0.28.0 — Task 13）
    Shell,
    /// 生成 shell 补全脚本（v0.28.0 — Task 13）
    Completions {
        /// 目标 shell 类型（bash/zsh/fish/powershell/elvish）
        shell: clap_complete::Shell,
    },
    /// 配置管理（v0.29.0 — Task 14）
    Config {
        /// Config 子命令
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// 服务管理（v0.29.0 — Task 14）
    Service {
        /// Service 子命令
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// 系统诊断（v0.29.0 — Task 14）
    Doctor,
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

/// Protocol 子命令（v0.23.0）
#[derive(Subcommand, Debug)]
enum ProtocolCommands {
    /// 显示所有协议适配器状态（支持协议列表 + 传输层能力）
    Status,
    /// 列出已注册协议适配器及配置
    List,
    /// 测试指定协议连通性
    Test {
        /// 协议类型（goose/sv/iec104/modbus_tcp/modbus_rtu/mqtt/opcua/dnp3/iec61850）
        protocol: String,
        /// 目标地址（IP:Port / 串口设备 / 网卡名）
        address: String,
    },
}

/// Security 子命令（v0.24.0）
#[derive(Subcommand, Debug)]
enum SecurityCommands {
    /// 显示安全状态汇总（Secure Boot + 内核加固 + seccomp + 审计 + KMS）
    Status,
    /// 审计日志管理
    Audit {
        /// Audit 子命令
        #[command(subcommand)]
        action: SecurityAuditCommands,
    },
    /// 密钥管理
    Keys {
        /// Keys 子命令
        #[command(subcommand)]
        action: SecurityKeysCommands,
    },
}

/// Security Audit 子命令
#[derive(Subcommand, Debug)]
enum SecurityAuditCommands {
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
    /// 验证审计日志完整性
    Verify,
}

/// Security Keys 子命令
#[derive(Subcommand, Debug)]
enum SecurityKeysCommands {
    /// 列出所有密钥
    List,
    /// 显示密钥详情
    Info {
        /// 密钥 ID
        key_id: String,
    },
    /// 轮换密钥
    Rotate {
        /// 密钥 ID
        key_id: String,
    },
}

/// HA 子命令（v0.26.0 — 通过 IPC 查询 ha-daemon）
#[derive(Subcommand, Debug)]
enum HaCommands {
    /// 显示 HA 状态（节点角色、心跳、同步、failover）
    Status,
    /// 列出集群节点
    Nodes,
    /// 显示同步状态
    SyncStatus,
    /// 显示 failover 状态（状态机、VIP、上次切换）
    FailoverStatus,
    /// 手动触发 failover 切换
    FailoverTrigger {
        /// 跳过交互式确认（高风险操作）
        #[arg(long)]
        force: bool,
    },
    /// 显示 failover 切换历史
    FailoverHistory,
    /// 触发灾备演练
    FailoverDrill {
        /// 演练场景（primary_down / network_partition / disk_failure）
        #[arg(long, default_value = "primary_down")]
        scenario: String,
    },
}

/// Plugin 子命令（v0.27.0 — 直接调用 eneros-plugin 库，IPC 推迟到 v0.28.0）
#[derive(Subcommand, Debug)]
enum PluginCommands {
    /// 列出已加载的插件
    List,
    /// 加载插件（验证签名 → 加载库 → 初始化 → 启动）
    Load {
        /// 插件动态库路径或 manifest.toml 路径
        path: String,
        /// 跳过签名验证（仅开发/测试环境使用）
        #[arg(long)]
        skip_signature: bool,
    },
    /// 卸载插件（停止 → 卸载库）
    Unload {
        /// 插件名称
        name: String,
    },
    /// 显示插件详情（manifest + state + statistics）
    Info {
        /// 插件名称
        name: String,
    },
    /// 验证插件签名（不加载）
    Verify {
        /// 插件动态库路径
        path: String,
        /// 签名文件路径（默认为 `<plugin>.sig`）
        #[arg(long)]
        sig: Option<String>,
    },
    /// 启用插件
    Enable {
        /// 插件名称
        name: String,
    },
    /// 禁用插件
    Disable {
        /// 插件名称
        name: String,
    },
    /// 生成插件签名密钥对（Ed25519）
    GenKeys {
        /// 密钥输出目录（默认 /etc/eneros/keys/）
        #[arg(long, default_value = "/etc/eneros/keys/")]
        output: String,
    },
    /// 对插件文件签名
    Sign {
        /// 插件动态库路径
        plugin: String,
        /// 私钥路径
        key: String,
    },
}

/// Simulator 子命令（v0.28.0 — Task 15）
#[derive(Subcommand, Debug)]
pub enum SimulatorAction {
    /// 运行场景脚本
    Run {
        /// 场景脚本文件路径（TOML 格式）
        path: PathBuf,
    },
    /// 验证场景脚本语法（解析 + 类型检查）
    Validate {
        /// 场景脚本文件路径（TOML 格式）
        path: PathBuf,
    },
    /// 列出内置故障场景
    List,
}

/// Config 子命令（v0.29.0 — Task 14）
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 查看配置项
    Get {
        /// 配置键（格式：file.field，如 plugin.require_signature）
        key: String,
    },
    /// 设置配置项
    Set {
        /// 配置键（格式：file.field）
        key: String,
        /// 配置值
        value: String,
    },
    /// 编辑配置文件
    Edit {
        /// 配置文件名（不含扩展名，如 plugin、syslog）
        file: String,
    },
    /// 列出所有配置文件
    List,
}

/// Service 子命令（v0.29.0 — Task 14）
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// 启动服务
    Start {
        /// 服务名称（eneros-init / ha-daemon / plugin-daemon / eventbus-broker / gateway）
        name: String,
    },
    /// 停止服务
    Stop {
        /// 服务名称
        name: String,
    },
    /// 重启服务
    Restart {
        /// 服务名称
        name: String,
    },
    /// 查询服务状态
    Status {
        /// 服务名称
        name: String,
    },
    /// 列出所有服务
    List,
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

    // 分发到对应子命令（dispatch_command 同时供交互式 shell 使用）
    commands::dispatch_command(cli.command, socket).await
}
