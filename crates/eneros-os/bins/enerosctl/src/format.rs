//! 输出格式化模块
//!
//! 提供 Agent 列表、Agent 详情、系统信息的格式化输出。
//! 表格使用手动对齐（不依赖外部 crate），兼容 Windows 10+ 终端。

use chrono::{DateTime, Local, Utc};
use eneros_core::AuthorityLevel;
use eneros_os::agentos::{AgentInfo, AgentStatus, AgentType};

/// 系统信息汇总（供 `system info` 命令输出）
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// 已注册 Agent 总数
    pub registered_agents: usize,
    /// Running 状态数
    pub running: usize,
    /// Stopped 状态数
    pub stopped: usize,
    /// Crashed 状态数
    pub crashed: usize,
    /// Degraded 状态数
    pub degraded: usize,
    /// RT Agent（SelfHealing 类型）ID 列表
    pub rt_agents: Vec<String>,
    /// EventBusBroker 是否已连接
    pub eventbus_connected: bool,
    /// EventBusBroker 地址
    pub eventbus_addr: String,
}

/// 格式化 Agent 类型为可读字符串
pub fn format_agent_type(t: &AgentType) -> String {
    match t {
        AgentType::Dispatch => "Dispatch".to_string(),
        AgentType::Forecast => "Forecast".to_string(),
        AgentType::Operation => "Operation".to_string(),
        AgentType::SelfHealing => "SelfHealing".to_string(),
        AgentType::Trading => "Trading".to_string(),
        AgentType::Planning => "Planning".to_string(),
        AgentType::Custom(name) => format!("Custom({})", name),
    }
}

/// 格式化 Agent 状态为可读字符串
pub fn format_agent_status(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Starting => "Starting",
        AgentStatus::Running => "Running",
        AgentStatus::Stopped => "Stopped",
        AgentStatus::Crashed => "Crashed",
        AgentStatus::Degraded => "Degraded",
    }
}

/// 格式化权限级别为可读字符串
pub fn format_authority(a: AuthorityLevel) -> &'static str {
    match a {
        AuthorityLevel::Observer => "Observer",
        AuthorityLevel::Operator => "Operator",
        AuthorityLevel::Supervisor => "Supervisor",
        AuthorityLevel::Emergency => "Emergency",
    }
}

/// 将 UTC 时间转换为本地时间字符串（用于表格显示）
fn format_local_time(t: &DateTime<Utc>) -> String {
    t.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// 将 UTC 时间格式化为带 UTC 后缀的字符串（用于详情显示）
fn format_utc_time(t: &DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// 右侧填充空格到指定显示宽度（按字符数计算，兼容非 ASCII）
fn pad_right(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count >= width {
        s.to_string()
    } else {
        let mut result = String::with_capacity(s.len() + (width - char_count));
        result.push_str(s);
        result.push_str(&" ".repeat(width - char_count));
        result
    }
}

/// 格式化 Agent 列表为对齐表格
///
/// 输出示例：
/// ```text
/// AGENT ID    PID  TYPE      STATUS   AUTHORITY  STARTED
/// dispatch-1  1024 Dispatch  Running  Operator   2026-06-18 10:30:00
/// ```
pub fn format_agent_table(agents: &[AgentInfo]) -> String {
    let header = ["AGENT ID", "PID", "TYPE", "STATUS", "AUTHORITY", "STARTED"];

    // 构建数据行
    let rows: Vec<Vec<String>> = agents
        .iter()
        .map(|a| {
            vec![
                a.agent_id.clone(),
                a.pid.to_string(),
                format_agent_type(&a.agent_type),
                format_agent_status(a.status).to_string(),
                format_authority(a.authority).to_string(),
                format_local_time(&a.started_at),
            ]
        })
        .collect();

    // 计算每列最大宽度（表头与数据取最大值）
    let mut widths = header.iter().map(|h| h.chars().count()).collect::<Vec<_>>();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = String::new();

    // 表头行
    for (i, h) in header.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(&pad_right(h, widths[i]));
    }
    out.push('\n');

    // 数据行
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(&pad_right(cell, widths[i]));
        }
        out.push('\n');
    }

    out
}

/// 格式化单个 Agent 详细信息
///
/// 输出示例：
/// ```text
/// Agent: dispatch-1
///   PID:        1024
///   Type:       Dispatch
///   Status:     Running
///   Authority:  Operator
///   Binary:     /usr/bin/eneros-dispatch-agent
///   Started:    2026-06-18 10:30:00 UTC
///   Crashes:    0
/// ```
pub fn format_agent_info(info: &AgentInfo) -> String {
    format!(
        "Agent: {}\n  PID:        {}\n  Type:       {}\n  Status:     {}\n  Authority:  {}\n  Binary:     {}\n  Started:    {}\n  Crashes:    {}\n",
        info.agent_id,
        info.pid,
        format_agent_type(&info.agent_type),
        format_agent_status(info.status),
        format_authority(info.authority),
        info.binary,
        format_utc_time(&info.started_at),
        info.crash_count,
    )
}

/// 格式化系统信息
///
/// 输出示例：
/// ```text
/// EnerOS System Status
/// =====================
/// Registered Agents: 3
///   Running:    3
///   Stopped:    0
///   Crashed:    0
///   Degraded:   0
/// RT Agents:        1 (self-heal-1)
/// EventBus Broker:  Connected (127.0.0.1:9876)
/// ```
pub fn format_system_info(info: &SystemInfo) -> String {
    let rt_agents_str = if info.rt_agents.is_empty() {
        "0".to_string()
    } else {
        format!("{} ({})", info.rt_agents.len(), info.rt_agents.join(", "))
    };

    let eventbus_str = if info.eventbus_connected {
        format!("Connected ({})", info.eventbus_addr)
    } else {
        format!("Disconnected ({})", info.eventbus_addr)
    };

    format!(
        "EnerOS System Status\n=====================\nRegistered Agents: {}\n  Running:    {}\n  Stopped:    {}\n  Crashed:    {}\n  Degraded:   {}\nRT Agents:        {}\nEventBus Broker:  {}\n",
        info.registered_agents,
        info.running,
        info.stopped,
        info.crashed,
        info.degraded,
        rt_agents_str,
        eventbus_str,
    )
}
