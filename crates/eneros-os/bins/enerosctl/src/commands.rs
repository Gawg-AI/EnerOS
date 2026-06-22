//! enerosctl 子命令实现
//!
//! 每个命令尝试通过 TCP（127.0.0.1:9876）连接 EventBusBroker 或控制通道。
//! 对于只读命令（agent list / agent status / system info），在 TCP 连接失败时
//! 回退到读取本地状态文件 `/var/run/eneros/agents.json`。
//! 所有连接失败均输出友好错误信息，不 panic。

use anyhow::{anyhow, Context, Result};
use eneros_os::agentos::{AgentInfo, AgentStatus, AgentType};
use eneros_os::init::syslog::{LogLevel, SyslogConfig};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::format::{self, SystemInfo};

// Plugin 子命令依赖（v0.27.0 — 跨平台，eneros-plugin 库本身跨平台）
// v0.28.0 Task 15: plugin 命令改为通过 PluginDaemonClient IPC 调用 plugin-daemon
use eneros_plugin::ipc::{DaemonResponse, PluginDaemonClient};
use eneros_plugin::manifest::PluginType;

// Simulator 子命令依赖（v0.28.0 — Task 15）
use eneros_simulator::fault::FaultScenarioLibrary;
use eneros_simulator::{Scenario, ScenarioRunner};

// Device 子命令依赖（仅 Linux 平台编译）
#[cfg(target_os = "linux")]
use eneros_os::hal::{FlowControl, Parity};
#[cfg(target_os = "linux")]
use eneros_os::init::audit::{
    AuditAction, AuditEntry, AuditLogger, AuditResult, IntegrityViolation, ViolationType,
};
#[cfg(target_os = "linux")]
use eneros_os::init::devmgr::{DeviceManager, DeviceStatus, DeviceType};
#[cfg(target_os = "linux")]
use eneros_os::init::serial_mgr::{SerialAccessControl, SerialHealth, SerialMonitor, SerialPreset};
#[cfg(target_os = "linux")]
use eneros_os::init::timesync::{ClockSource, TimeSyncConfig, TimeSyncManager};
#[cfg(target_os = "linux")]
use eneros_os::init::usb_mgr::UsbWhitelist;
#[cfg(target_os = "linux")]
use eneros_os::update::{AbPartition, OtaConfig, OtaManager, Slot};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// 默认控制通道地址（EventBusBroker / 控制服务）
const CONTROL_ADDR: &str = "127.0.0.1:9876";
/// 默认本地状态文件路径（eneros-init 写入的 Agent 注册表快照）
const STATE_FILE: &str = "/var/run/eneros/agents.json";
/// TCP 连接超时
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// 默认插件目录（v0.27.0 — v0.28.0 Task 15 后仅保留供参考，实际由 daemon 管理）
#[allow(dead_code)]
const PLUGIN_DIR: &str = "/var/lib/eneros/plugins";
/// 默认可信公钥目录
const PLUGIN_KEYS_DIR: &str = "/etc/eneros/keys";

/// 控制通道请求（JSON 行协议）
#[derive(Serialize)]
struct ControlRequest<'a> {
    command: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<&'a str>,
}

/// 控制通道响应（JSON 行协议）
#[derive(Deserialize)]
struct ControlResponse {
    /// "ok" 或 "error"
    status: String,
    /// 附加消息（成功时可能为空，失败时为错误描述）
    #[serde(default)]
    message: String,
    /// Agent 列表（agent_list / agent_status 响应）
    #[serde(default)]
    agents: Vec<AgentInfo>,
    /// EventBusBroker 连接状态（eventbus_status 响应）
    #[serde(default)]
    eventbus_connected: bool,
}

// ---------------------------------------------------------------------------
// 内部辅助函数
// ---------------------------------------------------------------------------

/// 尝试连接控制通道（TCP），带超时
async fn connect_control() -> Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(CONTROL_ADDR))
        .await
        .map_err(|_| anyhow!("连接控制通道 {} 超时", CONTROL_ADDR))?
        .map_err(|e| anyhow!("连接控制通道 {} 失败: {}", CONTROL_ADDR, e))?;
    Ok(stream)
}

/// 发送请求并读取单行 JSON 响应
async fn request_response(req: &ControlRequest<'_>) -> Result<ControlResponse> {
    let mut stream = connect_control().await?;
    let json = serde_json::to_string(req)?;
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // 读取响应设置 3 秒超时，避免控制通道无响应时永久阻塞
    let _ = tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line))
        .await
        .map_err(|_| anyhow!("读取响应超时"))??;

    let resp: ControlResponse = serde_json::from_str(&line)?;
    Ok(resp)
}

/// 从本地状态文件读取 Agent 列表
fn read_agents_from_state_file() -> Result<Vec<AgentInfo>> {
    let path = Path::new(STATE_FILE);
    if !path.exists() {
        return Err(anyhow!("状态文件 {} 不存在", STATE_FILE));
    }
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("读取状态文件 {} 失败", STATE_FILE))?;
    let agents: Vec<AgentInfo> = serde_json::from_str(&data)
        .with_context(|| format!("解析状态文件 {} 失败", STATE_FILE))?;
    Ok(agents)
}

/// 尝试获取 Agent 列表：先走 TCP 控制通道，失败则回退到本地状态文件
async fn try_get_agents() -> Result<Vec<AgentInfo>> {
    // 1. 尝试 TCP 控制通道
    let req = ControlRequest {
        command: "agent_list",
        agent_id: None,
        topic: None,
    };
    if let Ok(resp) = request_response(&req).await {
        if resp.status == "ok" {
            return Ok(resp.agents);
        }
    }
    // 2. 回退：本地状态文件
    read_agents_from_state_file()
}

/// 生成连接失败的友好错误信息
fn connection_error(action: &str) -> anyhow::Error {
    anyhow!(
        "无法{}。\n\n请确认:\n  1. EnerOS 内核 (eneros-init) 已启动\n  2. 控制通道 {} 正在监听\n  3. 或本地状态文件 {} 存在且可读",
        action,
        CONTROL_ADDR,
        STATE_FILE
    )
}

// ---------------------------------------------------------------------------
// Agent 子命令
// ---------------------------------------------------------------------------

/// 列出所有注册的 Agent
pub async fn cmd_agent_list(_socket: &str) -> Result<()> {
    match try_get_agents().await {
        Ok(agents) => {
            if agents.is_empty() {
                println!("当前没有已注册的 Agent。");
            } else {
                print!("{}", format::format_agent_table(&agents));
            }
            Ok(())
        }
        Err(_) => Err(connection_error("获取 Agent 列表")),
    }
}

/// 启动指定 Agent
pub async fn cmd_agent_start(_socket: &str, agent_id: &str) -> Result<()> {
    let req = ControlRequest {
        command: "agent_start",
        agent_id: Some(agent_id),
        topic: None,
    };
    match request_response(&req).await {
        Ok(resp) if resp.status == "ok" => {
            println!("Agent '{}' 已启动。", agent_id);
            Ok(())
        }
        Ok(resp) => Err(anyhow!("启动 Agent '{}' 失败: {}", agent_id, resp.message)),
        Err(_) => Err(connection_error(&format!("启动 Agent '{}'", agent_id))),
    }
}

/// 停止指定 Agent
pub async fn cmd_agent_stop(_socket: &str, agent_id: &str) -> Result<()> {
    let req = ControlRequest {
        command: "agent_stop",
        agent_id: Some(agent_id),
        topic: None,
    };
    match request_response(&req).await {
        Ok(resp) if resp.status == "ok" => {
            println!("Agent '{}' 已停止。", agent_id);
            Ok(())
        }
        Ok(resp) => Err(anyhow!("停止 Agent '{}' 失败: {}", agent_id, resp.message)),
        Err(_) => Err(connection_error(&format!("停止 Agent '{}'", agent_id))),
    }
}

/// 查询指定 Agent 状态
pub async fn cmd_agent_status(_socket: &str, agent_id: &str) -> Result<()> {
    match try_get_agents().await {
        Ok(agents) => {
            let agent = agents.iter().find(|a| a.agent_id == agent_id);
            match agent {
                Some(info) => {
                    print!("{}", format::format_agent_info(info));
                    Ok(())
                }
                None => Err(anyhow!("Agent '{}' 未找到。", agent_id)),
            }
        }
        Err(_) => Err(connection_error("查询 Agent 状态")),
    }
}

/// 重启指定 Agent
pub async fn cmd_agent_restart(_socket: &str, agent_id: &str) -> Result<()> {
    let req = ControlRequest {
        command: "agent_restart",
        agent_id: Some(agent_id),
        topic: None,
    };
    match request_response(&req).await {
        Ok(resp) if resp.status == "ok" => {
            println!("Agent '{}' 已重启。", agent_id);
            Ok(())
        }
        Ok(resp) => Err(anyhow!("重启 Agent '{}' 失败: {}", agent_id, resp.message)),
        Err(_) => Err(connection_error(&format!("重启 Agent '{}'", agent_id))),
    }
}

// ---------------------------------------------------------------------------
// EventBus 子命令
// ---------------------------------------------------------------------------

/// 查询 EventBusBroker 状态
pub async fn cmd_eventbus_status(_socket: &str) -> Result<()> {
    let req = ControlRequest {
        command: "eventbus_status",
        agent_id: None,
        topic: None,
    };
    match request_response(&req).await {
        Ok(resp) if resp.status == "ok" => {
            let status = if resp.eventbus_connected {
                "已连接"
            } else {
                "未连接"
            };
            println!("EventBus Broker: {}", status);
            Ok(())
        }
        Ok(resp) => Err(anyhow!("查询 EventBus 状态失败: {}", resp.message)),
        Err(_) => Err(connection_error("查询 EventBus 状态")),
    }
}

/// 订阅事件流（实时打印，按 Ctrl+C 退出）
pub async fn cmd_eventbus_subscribe(_socket: &str, topic: Option<&str>) -> Result<()> {
    let req = ControlRequest {
        command: "eventbus_subscribe",
        agent_id: None,
        topic,
    };
    let json = serde_json::to_string(&req)?;

    let mut stream = connect_control()
        .await
        .map_err(|_| connection_error("订阅事件"))?;
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    println!("已订阅事件流 (按 Ctrl+C 退出)...");
    if let Some(t) = topic {
        println!("主题过滤: {}", t);
    }

    let mut line = String::new();
    loop {
        line.clear();
        tokio::select! {
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => break,
                    Ok(_) => print!("{}", line),
                    Err(e) => {
                        eprintln!("读取事件流错误: {}", e);
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n停止订阅。");
                break;
            }
        }
    }

    println!("事件流已结束。");
    Ok(())
}

// ---------------------------------------------------------------------------
// System 子命令
// ---------------------------------------------------------------------------

/// 显示系统信息（已注册 Agent 数、状态分布、RT Agent、EventBus 连接状态）
pub async fn cmd_system_info(_socket: &str) -> Result<()> {
    // 获取 Agent 列表（TCP 或状态文件，失败则视为空）
    let agents = try_get_agents().await.unwrap_or_default();

    // 检查 EventBusBroker 连接状态
    let eventbus_connected = connect_control().await.is_ok();

    // 统计各状态数量
    let running = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Running)
        .count();
    let stopped = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Stopped)
        .count();
    let crashed = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Crashed)
        .count();
    let degraded = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Degraded)
        .count();

    // 识别 RT Agent（SelfHealing 类型，使用 SCHED_FIFO 调度）
    let rt_agents: Vec<String> = agents
        .iter()
        .filter(|a| matches!(a.agent_type, AgentType::SelfHealing))
        .map(|a| a.agent_id.clone())
        .collect();

    let info = SystemInfo {
        registered_agents: agents.len(),
        running,
        stopped,
        crashed,
        degraded,
        rt_agents,
        eventbus_connected,
        eventbus_addr: CONTROL_ADDR.to_string(),
    };

    print!("{}", format::format_system_info(&info));
    Ok(())
}

// ---------------------------------------------------------------------------
// Network 子命令
// ---------------------------------------------------------------------------

/// 右侧填充空格到指定显示宽度
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

/// 格式化对齐表格
fn format_table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.chars().count();
            if i < widths.len() && w > widths[i] {
                widths[i] = w;
            }
        }
    }
    let mut out = String::new();
    for (i, h) in header.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(&pad_right(h, widths[i]));
    }
    out.push('\n');
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            let w = widths.get(i).copied().unwrap_or(0);
            out.push_str(&pad_right(cell, w));
        }
        out.push('\n');
    }
    out
}

/// 显示所有网络接口状态
#[cfg(target_os = "linux")]
pub async fn cmd_network_status() -> Result<()> {
    let output = tokio::process::Command::new("ip")
        .args(["-j", "addr"])
        .output()
        .await
        .context("执行 `ip -j addr` 失败")?;
    if !output.status.success() {
        return Err(anyhow!(
            "`ip -j addr` 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).context("解析 `ip -j addr` 输出失败")?;

    let header = ["Interface", "Type", "State", "MAC", "MTU", "IPv4"];
    let mut rows = Vec::new();
    for entry in &entries {
        let name = entry.get("ifname").and_then(|v| v.as_str()).unwrap_or("");
        let mtu = entry.get("mtu").and_then(|v| v.as_u64()).unwrap_or(0);
        let mac = entry.get("address").and_then(|v| v.as_str()).unwrap_or("");
        let flags = entry.get("flags").and_then(|v| v.as_array());
        let is_up = flags
            .map(|f| f.iter().any(|x| x.as_str() == Some("UP")))
            .unwrap_or(false);
        let iface_type = if name == "lo" {
            "loopback"
        } else if name.starts_with("bond") {
            "bond"
        } else if name.contains('.') {
            "vlan"
        } else {
            "ethernet"
        };
        let mut ipv4 = String::new();
        if let Some(addr_info) = entry.get("addr_info").and_then(|v| v.as_array()) {
            for addr in addr_info {
                if addr.get("family").and_then(|v| v.as_str()) == Some("inet") {
                    let local = addr.get("local").and_then(|v| v.as_str()).unwrap_or("");
                    let prefix = addr.get("prefixlen").and_then(|v| v.as_u64()).unwrap_or(0);
                    if !ipv4.is_empty() {
                        ipv4.push_str(", ");
                    }
                    ipv4.push_str(&format!("{}/{}", local, prefix));
                }
            }
        }
        rows.push(vec![
            name.to_string(),
            iface_type.to_string(),
            if is_up { "up" } else { "down" }.to_string(),
            mac.to_string(),
            mtu.to_string(),
            ipv4,
        ]);
    }
    if rows.is_empty() {
        println!("未发现网络接口。");
    } else {
        print!("{}", format_table(&header, &rows));
    }
    Ok(())
}

/// 显示接口配置
#[cfg(target_os = "linux")]
pub async fn cmd_network_config(interface: Option<&str>) -> Result<()> {
    let path = "/etc/eneros/network.toml";
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => {
            println!("网络配置文件 {} 不存在。", path);
            return Ok(());
        }
    };
    let config: serde_json::Value =
        toml::from_str(&data).with_context(|| format!("解析 {} 失败", path))?;

    let header = ["Interface", "IPv4", "MTU"];
    let mut rows = Vec::new();
    if let Some(interfaces) = config.get("interfaces").and_then(|v| v.as_array()) {
        for iface in interfaces {
            let name = iface.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(filter) = interface {
                if name != filter {
                    continue;
                }
            }
            let ipv4 = iface
                .get("ipv4")
                .and_then(|v| v.get("address"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mtu = iface.get("mtu").and_then(|v| v.as_u64()).unwrap_or(0);
            rows.push(vec![name.to_string(), ipv4.to_string(), mtu.to_string()]);
        }
    }
    if rows.is_empty() {
        println!("没有匹配的接口配置。");
    } else {
        print!("{}", format_table(&header, &rows));
    }
    Ok(())
}

/// 列出防火墙规则
#[cfg(target_os = "linux")]
pub async fn cmd_network_firewall_list() -> Result<()> {
    let output = tokio::process::Command::new("nft")
        .args(["list", "ruleset"])
        .output()
        .await
        .context("执行 `nft list ruleset` 失败")?;
    if !output.status.success() {
        return Err(anyhow!(
            "`nft list ruleset` 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let header = ["Chain", "Rule"];
    let mut rows = Vec::new();
    let mut current_chain = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("chain ") && line.ends_with('{') {
            current_chain = line
                .trim_start_matches("chain ")
                .trim_end_matches('{')
                .trim()
                .to_string();
        } else if line == "}" {
            current_chain.clear();
        } else if !current_chain.is_empty() && !line.is_empty() {
            if line.contains("dport") || line.contains("policy") {
                rows.push(vec![current_chain.clone(), line.to_string()]);
            }
        }
    }
    if rows.is_empty() {
        println!("没有防火墙规则。");
    } else {
        print!("{}", format_table(&header, &rows));
    }
    Ok(())
}

/// 显示防火墙默认策略
#[cfg(target_os = "linux")]
pub async fn cmd_network_firewall_policy() -> Result<()> {
    let output = tokio::process::Command::new("nft")
        .args(["list", "ruleset"])
        .output()
        .await
        .context("执行 `nft list ruleset` 失败")?;
    if !output.status.success() {
        return Err(anyhow!("`nft list ruleset` 失败"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let header = ["Chain", "Policy"];
    let mut rows = Vec::new();
    let mut current_chain = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("chain ") && line.ends_with('{') {
            current_chain = line
                .trim_start_matches("chain ")
                .trim_end_matches('{')
                .trim()
                .to_string();
        }
        if !current_chain.is_empty() {
            if let Some(pos) = line.find("policy ") {
                let policy = line[pos + 7..]
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("");
                if !policy.is_empty() {
                    rows.push(vec![current_chain.clone(), policy.to_string()]);
                    current_chain.clear();
                }
            }
        }
    }
    if rows.is_empty() {
        println!("没有防火墙链定义。");
    } else {
        print!("{}", format_table(&header, &rows));
    }
    Ok(())
}

/// 显示 bonding 状态
#[cfg(target_os = "linux")]
pub async fn cmd_network_bond_status(interface: Option<&str>) -> Result<()> {
    let bond_dir = "/proc/net/bonding";
    let bond_names: Vec<String> = if let Some(name) = interface {
        vec![name.to_string()]
    } else {
        match std::fs::read_dir(bond_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
            Err(_) => {
                println!("没有 bonding 接口。");
                return Ok(());
            }
        }
    };
    let header = ["Bond", "Mode", "Active Slave", "Slaves", "Link"];
    let mut rows = Vec::new();
    for name in &bond_names {
        let path = format!("{}/{}", bond_dir, name);
        if let Ok(data) = std::fs::read_to_string(&path) {
            let mut mode = String::new();
            let mut active = String::new();
            let mut slaves = Vec::new();
            let mut link = "down".to_string();
            let mut in_slave = false;
            for line in data.lines() {
                let line = line.trim();
                if line.starts_with("Bonding Mode:") {
                    mode = line.trim_start_matches("Bonding Mode:").trim().to_string();
                } else if line.starts_with("Currently Active Slave:") {
                    active = line
                        .trim_start_matches("Currently Active Slave:")
                        .trim()
                        .to_string();
                } else if line.starts_with("MII Status:") {
                    if !in_slave {
                        link = line.trim_start_matches("MII Status:").trim().to_string();
                    }
                } else if line.starts_with("Slave Interface:") {
                    in_slave = true;
                    slaves.push(
                        line.trim_start_matches("Slave Interface:")
                            .trim()
                            .to_string(),
                    );
                }
            }
            rows.push(vec![
                name.clone(),
                mode,
                active,
                slaves.join(", "),
                link,
            ]);
        }
    }
    if rows.is_empty() {
        println!("没有 bonding 接口。");
    } else {
        print!("{}", format_table(&header, &rows));
    }
    Ok(())
}

// 非 Linux 平台桩实现
#[cfg(not(target_os = "linux"))]
pub async fn cmd_network_status() -> Result<()> {
    Err(anyhow!("Network commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_network_config(_interface: Option<&str>) -> Result<()> {
    Err(anyhow!("Network commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_network_firewall_list() -> Result<()> {
    Err(anyhow!("Network commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_network_firewall_policy() -> Result<()> {
    Err(anyhow!("Network commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_network_bond_status(_interface: Option<&str>) -> Result<()> {
    Err(anyhow!("Network commands require Linux"))
}

// ---------------------------------------------------------------------------
// Log 子命令
// ---------------------------------------------------------------------------

/// 默认日志目录
#[cfg(target_os = "linux")]
const LOG_DIR: &str = "/var/log/eneros";

/// syslog 配置文件路径（跨平台，非 Linux 下也用于读写配置）
const SYSLOG_CONFIG_PATH: &str = "/etc/eneros/syslog.toml";

/// 合法日志分类（与 syslog.rs LogCategory 对应）
const VALID_CATEGORIES: &[&str] = &["system", "agent", "protocol", "security", "audit"];

/// 校验分类白名单并返回日志文件路径（防路径遍历 + audit 子目录对齐）
#[cfg(target_os = "linux")]
fn resolve_log_file(category: Option<&str>) -> Result<String> {
    let cat = category.unwrap_or("system");
    if !VALID_CATEGORIES.contains(&cat) {
        return Err(anyhow!(
            "无效分类 '{}'，可选: {}",
            cat,
            VALID_CATEGORIES.join(", ")
        ));
    }
    // audit 分类存储在子目录 /var/log/eneros/audit/audit.log（与 audit.rs 一致）
    let path = if cat == "audit" {
        format!("{}/audit/audit.log", LOG_DIR)
    } else {
        format!("{}/{}.log", LOG_DIR, cat)
    };
    Ok(path)
}

/// 格式化 JSONL 日志行为统一输出（含 category 字段）
#[cfg(target_os = "linux")]
fn format_log_line(v: &serde_json::Value) -> String {
    let ts = v.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
    let level = v.get("level").and_then(|v| v.as_str()).unwrap_or("?");
    let category = v
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let source = v.get("source").and_then(|v| v.as_str()).unwrap_or("?");
    let msg = v.get("message").and_then(|v| v.as_str()).unwrap_or("");
    format!("{} [{}] [{}] {} — {}", ts, level, category, source, msg)
}

/// 检查日志文件是否存在，不存在时输出友好提示
#[cfg(target_os = "linux")]
fn check_log_file_exists(log_file: &str, category: &str) -> Result<()> {
    if !Path::new(log_file).exists() {
        return Err(anyhow!(
            "日志文件 {} 不存在（分类 {} 可能尚未产生日志）",
            log_file,
            category
        ));
    }
    Ok(())
}

/// 查看最近 N 行日志（支持 --follow 实时跟踪和 --json 原始输出）
#[cfg(target_os = "linux")]
pub async fn cmd_log_tail(
    category: Option<&str>,
    lines: usize,
    follow: bool,
    json: bool,
) -> Result<()> {
    let log_file = resolve_log_file(category)?;
    let cat = category.unwrap_or("system");
    check_log_file_exists(&log_file, cat)?;

    if follow {
        // 实时跟踪模式：tail -n N -f + Ctrl+C 退出
        let mut child = tokio::process::Command::new("tail")
            .args(["-n", &lines.to_string(), "-f", &log_file])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("执行 tail -f 失败")?;

        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout).lines();

        println!("（实时跟踪日志，按 Ctrl+C 退出）");

        loop {
            tokio::select! {
                result = reader.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            if json {
                                println!("{}", line);
                            } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                println!("{}", format_log_line(&v));
                            } else {
                                println!("{}", line);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("读取日志错误: {}", e);
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    println!("\n停止跟踪。");
                    let _ = child.kill().await;
                    break;
                }
            }
        }
    } else {
        // 一次性读取模式：tail -n N
        let output = tokio::process::Command::new("tail")
            .args(["-n", &lines.to_string(), &log_file])
            .output()
            .await
            .context("执行 tail 失败")?;

        if !output.status.success() {
            return Err(anyhow!(
                "读取日志失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let content = String::from_utf8_lossy(&output.stdout);
        for line in content.lines() {
            if json {
                println!("{}", line);
            } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                println!("{}", format_log_line(&v));
            } else {
                println!("{}", line);
            }
        }
    }
    Ok(())
}

/// 搜索日志（支持级别/时间/来源过滤，--category all 跨分类搜索，--json 原始输出）
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
pub async fn cmd_log_search(
    pattern: &str,
    category: Option<&str>,
    level: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    source: Option<&str>,
    json: bool,
) -> Result<()> {
    let start_time = since.and_then(parse_time);
    let end_time = until.and_then(parse_time);

    // 确定搜索文件列表（--category all 跨分类搜索）
    let files: Vec<(String, String)> = if category == Some("all") {
        VALID_CATEGORIES
            .iter()
            .filter_map(|c| resolve_log_file(Some(c)).ok().map(|f| (f, c.to_string())))
            .collect()
    } else {
        let cat = category.unwrap_or("system");
        vec![(resolve_log_file(category)?, cat.to_string())]
    };

    let mut found = 0;
    for (log_file, cat) in &files {
        if !Path::new(log_file).exists() {
            continue; // 跳过不存在的文件
        }

        let file = std::fs::File::open(log_file)
            .with_context(|| format!("打开日志文件失败: {}", log_file))?;
        let reader = std::io::BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            // 关键字匹配（子串匹配）
            if !line.contains(pattern) {
                continue;
            }

            // 解析 JSON 以应用过滤
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => {
                    // 非 JSON 行：仅当无过滤条件时输出
                    if json {
                        println!("{}", line);
                    }
                    continue;
                }
            };

            // 级别过滤
            if let Some(lvl) = level {
                let entry_level = v.get("level").and_then(|v| v.as_str()).unwrap_or("");
                if entry_level != lvl {
                    continue;
                }
            }

            // 来源过滤
            if let Some(src) = source {
                let entry_source = v.get("source").and_then(|v| v.as_str()).unwrap_or("");
                if entry_source != src {
                    continue;
                }
            }

            // 时间范围过滤
            if let Some(ts_str) = v.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(ts) = ts_str.parse::<chrono::DateTime<chrono::Utc>>() {
                    if let Some(s) = start_time {
                        if ts < s {
                            continue;
                        }
                    }
                    if let Some(e) = end_time {
                        if ts > e {
                            continue;
                        }
                    }
                }
            }

            if json {
                println!("{}", line);
            } else {
                println!("{}", format_log_line(&v));
            }
            found += 1;
        }
    }

    if found == 0 {
        println!("未找到匹配 '{}' 的日志", pattern);
    } else {
        eprintln!("共找到 {} 条匹配", found);
    }
    Ok(())
}

/// 动态调整或查询日志级别（跨平台：修改 syslog.toml 配置文件，Linux 下发送 SIGHUP）
///
/// - `level = None`：查询当前级别
/// - `level = Some(level)`：设置级别并通知 eneros-init 重载
pub async fn cmd_log_level(target: &str, level: Option<&str>) -> Result<()> {
    // 校验 target 为 global 或合法分类名
    if target != "global" && !VALID_CATEGORIES.contains(&target) {
        return Err(anyhow!(
            "无效目标 '{}'，可选: global, {}",
            target,
            VALID_CATEGORIES.join(", ")
        ));
    }

    let config_path = Path::new(SYSLOG_CONFIG_PATH);

    // 读取现有配置
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => {
            return Err(anyhow!(
                "syslog 配置文件 {} 不存在或不可读",
                SYSLOG_CONFIG_PATH
            ));
        }
    };

    let mut config: SyslogConfig =
        toml::from_str(&content).with_context(|| format!("解析 {} 失败", SYSLOG_CONFIG_PATH))?;

    // 查询模式
    if let Some(level_str) = level {
        // 设置模式
        let new_level = LogLevel::parse_level(level_str).ok_or_else(|| {
            anyhow!(
                "无效日志级别 '{}'，可选: trace, debug, info, warn, error",
                level_str
            )
        })?;

        if target == "global" {
            config.global_level = new_level;
        } else {
            config
                .category_levels
                .insert(target.to_string(), new_level);
        }

        // 序列化并写回配置文件
        let new_content = toml::to_string(&config).context("序列化 syslog 配置失败")?;
        std::fs::write(config_path, new_content)
            .with_context(|| format!("写入 {} 失败", SYSLOG_CONFIG_PATH))?;

        // Linux 下发送 SIGHUP 给 eneros-init (PID 1) 触发 reload
        #[cfg(target_os = "linux")]
        {
            let sighup_result = tokio::process::Command::new("kill")
                .args(["-HUP", "1"])
                .output()
                .await;
            match sighup_result {
                Ok(output) if output.status.success() => {
                    println!(
                        "日志级别已设置: {} → {}（已通知 eneros-init 重载）",
                        target, level_str
                    );
                }
                Ok(output) => {
                    println!(
                        "日志级别已设置: {} → {}（配置已写入，但 SIGHUP 失败: {}）",
                        target,
                        level_str,
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => {
                    println!(
                        "日志级别已设置: {} → {}（配置已写入，但 SIGHUP 失败: {}）",
                        target, level_str, e
                    );
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            println!("日志级别已设置: {} → {}（配置已写入）", target, level_str);
        }
    } else {
        // 查询模式
        if target == "global" {
            println!("{}: {}", target, config.global_level.as_str());
        } else if let Some(lvl) = config.category_levels.get(target) {
            println!("{}: {}", target, lvl.as_str());
        } else {
            println!(
                "{}: 未设置（使用全局级别 {}）",
                target,
                config.global_level.as_str()
            );
        }
    }
    Ok(())
}

/// 导出日志（支持 --output 文件输出和 BufReader 流式处理）
#[cfg(target_os = "linux")]
pub async fn cmd_log_export(
    start: Option<&str>,
    end: Option<&str>,
    format: &str,
    category: Option<&str>,
    output: Option<&Path>,
) -> Result<()> {
    // 校验导出格式
    if format != "json" && format != "text" {
        return Err(anyhow!("无效格式 '{}'，可选: json, text", format));
    }

    let log_file = resolve_log_file(category)?;
    let cat = category.unwrap_or("system");
    check_log_file_exists(&log_file, cat)?;

    let file = std::fs::File::open(&log_file)
        .with_context(|| format!("打开日志文件失败: {}", log_file))?;
    let reader = std::io::BufReader::new(file);

    // 输出目标：文件或 stdout
    let mut writer: Box<dyn Write> = if let Some(path) = output {
        Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).with_context(|| format!("创建输出文件失败: {}", path.display()))?,
        ))
    } else {
        Box::new(std::io::BufWriter::new(std::io::stdout()))
    };

    let start_time = start.and_then(parse_time);
    let end_time = end.and_then(parse_time);

    let mut exported = 0;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 时间范围过滤（严格模式：时间戳解析失败时跳过该行）
        if let Some(ts_str) = v.get("timestamp").and_then(|v| v.as_str()) {
            match ts_str.parse::<chrono::DateTime<chrono::Utc>>() {
                Ok(ts) => {
                    if let Some(s) = start_time {
                        if ts < s {
                            continue;
                        }
                    }
                    if let Some(e) = end_time {
                        if ts > e {
                            continue;
                        }
                    }
                }
                Err(_) => continue, // 严格模式：跳过无法解析的时间戳
            }
        } else {
            continue; // 严格模式：跳过无时间戳的行
        }

        match format {
            "json" => writeln!(writer, "{}", line)?,
            "text" => writeln!(writer, "{}", format_log_line(&v))?,
            _ => writeln!(writer, "{}", line)?,
        }
        exported += 1;
    }

    // 摘要信息输出到 stderr，避免污染导出文件内容
    eprintln!("--- 导出完成: {} 条日志 ---", exported);
    writer.flush()?;
    Ok(())
}

/// 触发日志轮转（通过 SIGHUP 通知 eneros-init 重载）
#[cfg(target_os = "linux")]
pub async fn cmd_log_rotate(category: &str) -> Result<()> {
    if !VALID_CATEGORIES.contains(&category) {
        return Err(anyhow!(
            "无效分类 '{}'，可选: {}",
            category,
            VALID_CATEGORIES.join(", ")
        ));
    }

    // 发送 SIGHUP 给 eneros-init (PID 1) 触发 reload，syslog.rs 在 reload 时检查轮转条件
    let output = tokio::process::Command::new("kill")
        .args(["-HUP", "1"])
        .output()
        .await
        .context("发送 SIGHUP 信号失败")?;

    if !output.status.success() {
        return Err(anyhow!(
            "发送 SIGHUP 信号失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    println!("已发送 SIGHUP 信号通知 eneros-init 重载（分类: {}）", category);
    println!("日志轮转将在 eneros-init 重载时触发");
    Ok(())
}

/// 解析时间字符串（纯函数，无平台依赖；仅 Linux 命令调用）
#[cfg(target_os = "linux")]
fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    // 尝试 ISO 8601
    if let Ok(dt) = s.parse::<chrono::DateTime<chrono::Utc>>() {
        return Some(dt);
    }
    // 尝试 YYYY-MM-DD
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).map(|dt| {
            chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc)
        });
    }
    None
}

// 非 Linux 平台 stub
#[cfg(not(target_os = "linux"))]
pub async fn cmd_log_tail(
    _category: Option<&str>,
    _lines: usize,
    _follow: bool,
    _json: bool,
) -> Result<()> {
    Err(anyhow!("Log commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
pub async fn cmd_log_search(
    _pattern: &str,
    _category: Option<&str>,
    _level: Option<&str>,
    _since: Option<&str>,
    _until: Option<&str>,
    _source: Option<&str>,
    _json: bool,
) -> Result<()> {
    Err(anyhow!("Log commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_log_export(
    _start: Option<&str>,
    _end: Option<&str>,
    _format: &str,
    _category: Option<&str>,
    _output: Option<&Path>,
) -> Result<()> {
    Err(anyhow!("Log commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_log_rotate(_category: &str) -> Result<()> {
    Err(anyhow!("Log commands require Linux"))
}

// ---------------------------------------------------------------------------
// Audit 子命令
// ---------------------------------------------------------------------------

/// 审计配置文件路径
#[cfg(target_os = "linux")]
const AUDIT_CONFIG_PATH: &str = "/etc/eneros/audit.toml";

/// 格式化审计结果为可读字符串
#[cfg(target_os = "linux")]
fn format_audit_result(r: AuditResult) -> &'static str {
    match r {
        AuditResult::Success => "success",
        AuditResult::Failure => "failure",
        AuditResult::Denied => "denied",
    }
}

/// 格式化审计日志条目为单行输出
#[cfg(target_os = "linux")]
fn format_audit_entry(entry: &AuditEntry) -> String {
    format!(
        "[{}] {} {} actor={} target={} result={} ip={:?} detail={}",
        entry.seq,
        entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
        entry.action.as_str(),
        entry.actor,
        entry.target,
        format_audit_result(entry.result),
        entry.source_ip,
        entry.detail
    )
}

/// 解析审计结果字符串
#[cfg(target_os = "linux")]
fn parse_audit_result(s: &str) -> Option<AuditResult> {
    match s.to_lowercase().as_str() {
        "success" => Some(AuditResult::Success),
        "failure" | "failed" => Some(AuditResult::Failure),
        "denied" => Some(AuditResult::Denied),
        _ => None,
    }
}

/// 格式化完整性违规类型
#[cfg(target_os = "linux")]
fn format_violation_type(v: &ViolationType) -> &'static str {
    match v {
        ViolationType::SignatureMismatch => "签名不匹配",
        ViolationType::SeqGap { .. } => "序号间隙",
        ViolationType::HashChainBroken { .. } => "哈希链断裂",
        ViolationType::Unparseable => "无法解析",
    }
}

/// 列出审计日志
#[cfg(target_os = "linux")]
pub async fn cmd_audit_list(
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let config_path = Path::new(AUDIT_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!("审计配置文件 {} 不存在", AUDIT_CONFIG_PATH));
    }

    let logger = AuditLogger::load(config_path).context("加载审计配置失败")?;

    let start = since.and_then(parse_time);
    let end = until.and_then(parse_time);

    let entries = logger
        .query(start, end, None, None, None, None, limit)
        .context("查询审计日志失败")?;

    if entries.is_empty() {
        println!("没有匹配的审计日志。");
        return Ok(());
    }

    let header = ["SEQ", "TIMESTAMP", "ACTION", "ACTOR", "TARGET", "RESULT", "SOURCE_IP"];
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.seq.to_string(),
                e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                e.action.as_str().to_string(),
                e.actor.clone(),
                e.target.clone(),
                format_audit_result(e.result).to_string(),
                e.source_ip.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 条审计日志", entries.len());
    Ok(())
}

/// 验证审计日志完整性
#[cfg(target_os = "linux")]
pub async fn cmd_audit_verify() -> Result<()> {
    let config_path = Path::new(AUDIT_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!("审计配置文件 {} 不存在", AUDIT_CONFIG_PATH));
    }

    let logger = AuditLogger::load(config_path).context("加载审计配置失败")?;

    let violations = logger
        .verify_integrity()
        .context("验证审计日志完整性失败")?;

    // 查询总记录数
    let entries = logger
        .query(None, None, None, None, None, None, None)
        .context("查询审计日志失败")?;

    let total = entries.len();
    let violation_count = violations.len();
    let passed = total.saturating_sub(violation_count);

    println!("审计日志完整性验证");
    println!("====================");
    println!("总记录数:    {}", total);
    println!("验证通过:    {}", passed);
    println!("违规数:      {}", violation_count);

    if !violations.is_empty() {
        println!("\n违规详情:");
        for v in &violations {
            println!(
                "  行 {} (seq {}): {} — {}",
                v.line_number,
                v.seq,
                format_violation_type(&v.violation_type),
                v.detail
            );
        }
    } else {
        println!("\n✓ 审计日志完整性验证通过");
    }
    Ok(())
}

/// 搜索审计日志
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
pub async fn cmd_audit_search(
    actor: Option<&str>,
    action: Option<&str>,
    result: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let config_path = Path::new(AUDIT_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!("审计配置文件 {} 不存在", AUDIT_CONFIG_PATH));
    }

    let logger = AuditLogger::load(config_path).context("加载审计配置失败")?;

    // 解析 action 过滤条件
    let action_filter = if let Some(a) = action {
        Some(
            AuditAction::from_str(a)
                .ok_or_else(|| anyhow!("无效审计动作 '{}'，可选: login, logout, config_change, agent_control, permission_change, update, emergency, command_exec, data_access, other", a))?,
        )
    } else {
        None
    };

    // 解析 result 过滤条件
    let result_filter = if let Some(r) = result {
        Some(parse_audit_result(r).ok_or_else(|| anyhow!("无效审计结果 '{}'，可选: success, failure, denied", r))?)
    } else {
        None
    };

    let start = since.and_then(parse_time);
    let end = until.and_then(parse_time);

    let entries = logger
        .query(
            start,
            end,
            action_filter.as_ref(),
            actor,
            result_filter.as_ref(),
            None,
            limit,
        )
        .context("查询审计日志失败")?;

    if entries.is_empty() {
        println!("没有匹配的审计日志。");
        return Ok(());
    }

    for entry in &entries {
        println!("{}", format_audit_entry(entry));
    }
    println!("\n共 {} 条匹配", entries.len());
    Ok(())
}

// Audit 非 Linux 平台 stub
#[cfg(not(target_os = "linux"))]
pub async fn cmd_audit_list(
    _since: Option<&str>,
    _until: Option<&str>,
    _limit: Option<usize>,
) -> Result<()> {
    Err(anyhow!("Audit commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_audit_verify() -> Result<()> {
    Err(anyhow!("Audit commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
pub async fn cmd_audit_search(
    _actor: Option<&str>,
    _action: Option<&str>,
    _result: Option<&str>,
    _since: Option<&str>,
    _until: Option<&str>,
    _limit: Option<usize>,
) -> Result<()> {
    Err(anyhow!("Audit commands require Linux"))
}

// ---------------------------------------------------------------------------
// Time 子命令
// ---------------------------------------------------------------------------

/// 时间同步配置文件路径
#[cfg(target_os = "linux")]
const TIMESYNC_CONFIG_PATH: &str = "/etc/eneros/timesync.toml";

/// 格式化时钟源为可读字符串
#[cfg(target_os = "linux")]
fn format_clock_source(s: ClockSource) -> &'static str {
    match s {
        ClockSource::Ptp => "PTP",
        ClockSource::Ntp => "NTP",
        ClockSource::LocalClock => "LocalClock",
    }
}

/// 解析时钟源字符串
#[cfg(target_os = "linux")]
fn parse_clock_source(s: &str) -> Option<ClockSource> {
    match s.to_lowercase().as_str() {
        "ptp" => Some(ClockSource::Ptp),
        "ntp" => Some(ClockSource::Ntp),
        "local" | "local_clock" | "localclock" => Some(ClockSource::LocalClock),
        _ => None,
    }
}

/// 显示时间同步状态
#[cfg(target_os = "linux")]
pub async fn cmd_time_status() -> Result<()> {
    let config_path = Path::new(TIMESYNC_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!(
            "时间同步配置文件 {} 不存在",
            TIMESYNC_CONFIG_PATH
        ));
    }

    let manager = TimeSyncManager::load(config_path).context("加载时间同步配置失败")?;

    let status = manager.status();

    println!("时间同步状态");
    println!("============");
    println!("时钟源:       {}", format_clock_source(status.source));
    println!(
        "同步状态:     {}",
        if status.locked { "已锁定" } else { "未锁定" }
    );
    println!("偏差:         {} μs", status.offset_micros);
    if let Some(gm) = &status.grandmaster_id {
        println!("Grandmaster:  {}", gm);
    } else {
        println!("Grandmaster:  (无)");
    }
    if let Some(err) = &status.last_error {
        println!("最后错误:     {}", err);
    } else {
        println!("最后错误:     无");
    }
    println!(
        "最后同步:     {}",
        status.last_sync.format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

/// 设置时钟源
#[cfg(target_os = "linux")]
pub async fn cmd_time_set_source(source: &str) -> Result<()> {
    let config_path = Path::new(TIMESYNC_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!(
            "时间同步配置文件 {} 不存在",
            TIMESYNC_CONFIG_PATH
        ));
    }

    let clock_source = parse_clock_source(source)
        .ok_or_else(|| anyhow!("无效时钟源 '{}'，可选: ptp, ntp, local", source))?;

    // 加载配置，修改 enabled_sources，保存
    let content = std::fs::read_to_string(config_path)?;
    let mut config: TimeSyncConfig = toml::from_str(&content)
        .with_context(|| format!("解析 {} 失败", TIMESYNC_CONFIG_PATH))?;

    config.enabled_sources = vec![clock_source];

    let new_content =
        toml::to_string(&config).context("序列化时间同步配置失败")?;
    std::fs::write(config_path, new_content)?;

    // 重新加载并应用
    let mut manager = TimeSyncManager::load(config_path).context("加载时间同步配置失败")?;
    manager.apply().context("应用时间同步配置失败")?;

    println!("时钟源已切换为: {}", source);
    Ok(())
}

/// 手动触发时间同步
#[cfg(target_os = "linux")]
pub async fn cmd_time_sync() -> Result<()> {
    let config_path = Path::new(TIMESYNC_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!(
            "时间同步配置文件 {} 不存在",
            TIMESYNC_CONFIG_PATH
        ));
    }

    let mut manager = TimeSyncManager::load(config_path).context("加载时间同步配置失败")?;
    manager.apply().context("触发时间同步失败")?;

    println!("时间同步已触发");
    Ok(())
}

// Time 非 Linux 平台 stub
#[cfg(not(target_os = "linux"))]
pub async fn cmd_time_status() -> Result<()> {
    Err(anyhow!("Time commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_time_set_source(_source: &str) -> Result<()> {
    Err(anyhow!("Time commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_time_sync() -> Result<()> {
    Err(anyhow!("Time commands require Linux"))
}

// ---------------------------------------------------------------------------
// Device 子命令
// ---------------------------------------------------------------------------

/// 解析设备类型字符串
#[cfg(target_os = "linux")]
fn parse_device_type(s: &str) -> Option<DeviceType> {
    match s.to_lowercase().as_str() {
        "serial" | "tty" => Some(DeviceType::Serial),
        "usb" => Some(DeviceType::Usb),
        "gpio" => Some(DeviceType::Gpio),
        "i2c" => Some(DeviceType::I2c),
        "spi" => Some(DeviceType::Spi),
        "net" => Some(DeviceType::Net),
        "block" => Some(DeviceType::Block),
        _ => None,
    }
}

/// 格式化设备类型
#[cfg(target_os = "linux")]
fn format_device_type(t: &DeviceType) -> &'static str {
    match t {
        DeviceType::Net => "Net",
        DeviceType::Block => "Block",
        DeviceType::Usb => "USB",
        DeviceType::Serial => "Serial",
        DeviceType::Gpio => "GPIO",
        DeviceType::I2c => "I2C",
        DeviceType::Spi => "SPI",
        DeviceType::Unknown => "Unknown",
    }
}

/// 格式化设备状态
#[cfg(target_os = "linux")]
fn format_device_status(s: &DeviceStatus) -> &'static str {
    match s {
        DeviceStatus::Online => "Online",
        DeviceStatus::Offline => "Offline",
        DeviceStatus::Error => "Error",
    }
}

/// 格式化串口健康状态
#[cfg(target_os = "linux")]
fn format_serial_health(h: &SerialHealth) -> &'static str {
    match h {
        SerialHealth::Healthy => "Healthy",
        SerialHealth::Degraded => "Degraded",
        SerialHealth::Failed => "Failed",
    }
}

/// 格式化校验位
#[cfg(target_os = "linux")]
fn format_parity(p: &Parity) -> &'static str {
    match p {
        Parity::None => "None",
        Parity::Even => "Even",
        Parity::Odd => "Odd",
    }
}

/// 格式化流控
#[cfg(target_os = "linux")]
fn format_flow_control(fc: &FlowControl) -> &'static str {
    match fc {
        FlowControl::None => "None",
        FlowControl::Hardware => "Hardware",
        FlowControl::Software => "Software",
    }
}

/// 列出所有设备
#[cfg(target_os = "linux")]
pub async fn cmd_device_list(type_filter: Option<&str>) -> Result<()> {
    // 解析类型过滤器
    let filter = if let Some(t) = type_filter {
        Some(parse_device_type(t).ok_or_else(|| {
            anyhow!(
                "无效设备类型 '{}'，可选: serial, usb, gpio, i2c, spi, net, block",
                t
            )
        })?)
    } else {
        None
    };

    let devices = DeviceManager::list_all_devices().context("枚举设备失败")?;

    let filtered: Vec<_> = devices
        .iter()
        .filter(|d| filter.as_ref().is_none_or(|f| &d.device_type == f))
        .collect();

    if filtered.is_empty() {
        println!("未找到匹配设备");
        return Ok(());
    }

    let header = ["NAME", "TYPE", "STATUS", "PATH", "DRIVER"];
    let rows: Vec<Vec<String>> = filtered
        .iter()
        .map(|d| {
            vec![
                d.name.clone(),
                format_device_type(&d.device_type).to_string(),
                format_device_status(&d.status).to_string(),
                d.path.clone(),
                d.driver.clone(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 台设备", filtered.len());
    Ok(())
}

/// 显示设备详情
#[cfg(target_os = "linux")]
pub async fn cmd_device_info(device: &str) -> Result<()> {
    let devices = DeviceManager::list_all_devices().context("枚举设备失败")?;

    // 查找匹配 name 或 path 的设备
    let info = devices
        .iter()
        .find(|d| d.name == device || d.path == device)
        .ok_or_else(|| anyhow!("未找到设备 '{}'", device))?;

    println!("Device: {}", info.name);
    println!("  Type:       {}", format_device_type(&info.device_type));
    println!("  Status:     {}", format_device_status(&info.status));
    println!("  Path:       {}", info.path);
    println!("  Driver:     {}", info.driver);
    if let Some(ref last_seen) = info.last_seen {
        println!("  Last Seen:  {}", last_seen);
    }

    // 串口设备：显示锁定与健康状态
    // 注意：CLI 一次性调用，SerialAccessControl/SerialMonitor 为新建实例，
    // is_locked 恒为 false、health 恒为 Healthy（仅反映本进程视图）
    if info.device_type == DeviceType::Serial {
        let ac = SerialAccessControl::new();
        println!("  Locked:     {}", ac.is_locked(&info.path));
        let monitor = SerialMonitor::new();
        println!(
            "  Health:     {}",
            format_serial_health(&monitor.health(&info.path))
        );
    }

    // USB 设备：显示白名单授权状态
    if info.device_type == DeviceType::Usb {
        let sysfs_path = &info.path;
        let vid = std::fs::read_to_string(format!("{}/idVendor", sysfs_path))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let pid = std::fs::read_to_string(format!("{}/idProduct", sysfs_path))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        println!("  VID:PID:    {}:{}", vid, pid);

        let wl_path = std::path::Path::new("/etc/eneros/usb-whitelist.toml");
        if wl_path.exists() {
            match UsbWhitelist::load(wl_path) {
                Ok(wl) => {
                    let authorized = wl.is_authorized(&vid, &pid);
                    println!(
                        "  Whitelist:  {}",
                        if authorized { "Authorized" } else { "Not authorized" }
                    );
                }
                Err(e) => {
                    println!("  Whitelist:  (读取失败: {})", e);
                }
            }
        } else {
            println!("  Whitelist:  (未配置)");
        }
    }

    Ok(())
}

/// 配置设备参数（显示配置方案，不实际修改）
#[cfg(target_os = "linux")]
pub async fn cmd_device_config(
    device: &str,
    preset: Option<&str>,
    baud: Option<u32>,
) -> Result<()> {
    // 解析预设
    let preset = if let Some(p) = preset {
        match p {
            "iec104_ft12" => SerialPreset::Iec104Ft12,
            "modbus_rtu" => SerialPreset::ModbusRtu,
            "modbus_rtu_high" => SerialPreset::ModbusRtuHigh,
            _ => {
                return Err(anyhow!(
                    "无效预设 '{}'，可选: iec104_ft12, modbus_rtu, modbus_rtu_high",
                    p
                ));
            }
        }
    } else {
        SerialPreset::Iec104Ft12
    };

    let mut config = preset.to_config();

    // 波特率覆盖
    if let Some(b) = baud {
        config.baud_rate = b;
    }

    println!("Device: {}", device);
    println!("  Baud Rate:    {}", config.baud_rate);
    println!("  Data Bits:    {}", config.data_bits);
    println!("  Stop Bits:    {}", config.stop_bits);
    println!("  Parity:       {}", format_parity(&config.parity));
    println!("  Flow Control: {}", format_flow_control(&config.flow_control));
    match config.timeout_ms {
        Some(t) => println!("  Timeout:      {} ms", t),
        None => println!("  Timeout:      (blocking)"),
    }
    println!("\n（仅显示配置方案，未实际修改设备）");
    Ok(())
}

/// 实时监控设备状态（每 2 秒刷新，Ctrl+C 退出）
#[cfg(target_os = "linux")]
pub async fn cmd_device_monitor() -> Result<()> {
    println!("监控设备状态中（按 Ctrl+C 退出）...\n");

    loop {
        // 清屏并复位光标
        print!("\x1b[2J\x1b[H");

        match DeviceManager::list_all_devices() {
            Ok(devices) => {
                let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                println!("EnerOS Device Monitor  [{}]", now);
                println!("========================");

                if devices.is_empty() {
                    println!("（无设备）");
                } else {
                    let header = ["NAME", "TYPE", "STATUS", "PATH", "DRIVER"];
                    let rows: Vec<Vec<String>> = devices
                        .iter()
                        .map(|d| {
                            vec![
                                d.name.clone(),
                                format_device_type(&d.device_type).to_string(),
                                format_device_status(&d.status).to_string(),
                                d.path.clone(),
                                d.driver.clone(),
                            ]
                        })
                        .collect();
                    print!("{}", format_table(&header, &rows));
                    println!("共 {} 台设备", devices.len());
                }
            }
            Err(e) => {
                println!("枚举设备失败: {}", e);
            }
        }

        // 等待 2 秒或 Ctrl+C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n已退出监控");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }

    Ok(())
}

// 非 Linux 平台 stub
#[cfg(not(target_os = "linux"))]
pub async fn cmd_device_list(_type_filter: Option<&str>) -> Result<()> {
    Err(anyhow!("Device commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_device_info(_device: &str) -> Result<()> {
    Err(anyhow!("Device commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_device_config(
    _device: &str,
    _preset: Option<&str>,
    _baud: Option<u32>,
) -> Result<()> {
    Err(anyhow!("Device commands require Linux"))
}

#[cfg(not(target_os = "linux"))]
pub async fn cmd_device_monitor() -> Result<()> {
    Err(anyhow!("Device commands require Linux"))
}

// ---------------------------------------------------------------------------
// Update 子命令
// ---------------------------------------------------------------------------

/// 槽位状态文件路径
#[cfg(target_os = "linux")]
const SLOT_STATE_PATH: &str = "/etc/eneros/slot-state.json";

/// 格式化时间为可读字符串（YYYY-MM-DD HH:MM:SS），None 时返回 "-"
#[cfg(target_os = "linux")]
fn format_datetime(dt: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match dt {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "-".to_string(),
    }
}

/// 格式化文件大小为人类可读字符串
#[cfg(target_os = "linux")]
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// 创建默认 OtaConfig
#[cfg(target_os = "linux")]
fn default_ota_config() -> OtaConfig {
    OtaConfig {
        download_dir: PathBuf::from("/data/updates/"),
        update_server_url: None,
        verify_keys_path: PathBuf::from("/etc/eneros/keys/signing.pub"),
        slot_state_path: PathBuf::from(SLOT_STATE_PATH),
    }
}

/// Update 子命令分发
#[cfg(target_os = "linux")]
pub async fn cmd_update(cmd: crate::UpdateCommands) -> Result<()> {
    match cmd {
        crate::UpdateCommands::Status => cmd_update_status().await,
        crate::UpdateCommands::Apply { bundle } => cmd_update_apply(&bundle).await,
        crate::UpdateCommands::Rollback => cmd_update_rollback().await,
        crate::UpdateCommands::List => cmd_update_list().await,
        crate::UpdateCommands::GenKeys { output } => cmd_update_gen_keys(&output).await,
    }
}

/// 查询当前槽位状态
#[cfg(target_os = "linux")]
async fn cmd_update_status() -> Result<()> {
    let path = Path::new(SLOT_STATE_PATH);
    let ab = AbPartition::load_from_file(path).context("加载槽位状态失败")?;

    println!("EnerOS A/B Slot Status");
    println!("======================");
    println!("Active Slot: {:?}", ab.active_slot);
    println!();

    // Slot A
    println!("Slot A:");
    println!("  Status:      {:?}", ab.slot_a_status);
    println!("  Boot Count:  {}", ab.boot_count_a);
    let last_boot_a = if ab.active_slot == Slot::A {
        format_datetime(ab.last_boot)
    } else {
        "-".to_string()
    };
    println!("  Last Boot:   {}", last_boot_a);

    // Slot B
    println!();
    println!("Slot B:");
    println!("  Status:      {:?}", ab.slot_b_status);
    println!("  Boot Count:  {}", ab.boot_count_b);
    let last_boot_b = if ab.active_slot == Slot::B {
        format_datetime(ab.last_boot)
    } else {
        "-".to_string()
    };
    println!("  Last Boot:   {}", last_boot_b);

    println!();
    println!("Last Update: {}", format_datetime(ab.last_update));
    Ok(())
}

/// 应用 OTA 更新包
#[cfg(target_os = "linux")]
async fn cmd_update_apply(bundle: &str) -> Result<()> {
    let config = default_ota_config();
    let mut manager = OtaManager::new(config).context("创建 OTA 管理器失败")?;

    println!("正在应用 OTA 更新: {}", bundle);
    manager.apply(bundle).context("OTA 更新失败")?;
    println!("OTA 更新成功，请重启系统以激活新槽位。");
    Ok(())
}

/// 回滚到上一已知良好槽位
#[cfg(target_os = "linux")]
async fn cmd_update_rollback() -> Result<()> {
    let config = default_ota_config();
    let mut manager = OtaManager::new(config).context("创建 OTA 管理器失败")?;

    manager.rollback().context("回滚失败")?;
    println!("已回滚到上一已知良好槽位，请重启系统。");
    Ok(())
}

/// 列出可用的更新包
#[cfg(target_os = "linux")]
async fn cmd_update_list() -> Result<()> {
    let config = default_ota_config();
    let manager = OtaManager::new(config).context("创建 OTA 管理器失败")?;

    let updates = manager.list_updates();
    if updates.is_empty() {
        println!("没有可用的更新包。");
        return Ok(());
    }

    let header = ["名称", "大小", "路径"];
    let rows: Vec<Vec<String>> = updates
        .iter()
        .map(|u| {
            vec![
                u.name.clone(),
                format_size(u.size),
                u.path.display().to_string(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个更新包", updates.len());
    Ok(())
}

/// 生成 Ed25519 密钥对
#[cfg(target_os = "linux")]
async fn cmd_update_gen_keys(output: &str) -> Result<()> {
    use eneros_os::update::signer;

    let output_dir = Path::new(output);
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("创建目录 {} 失败", output))?;

    let (signing_key, verifying_key) = signer::generate_keypair()
        .context("生成密钥对失败")?;

    let key_path = output_dir.join("signing.key");
    let pub_path = output_dir.join("signing.pub");

    signer::save_signing_key(&signing_key, &key_path).context("保存私钥失败")?;
    signer::save_verifying_key(&verifying_key, &pub_path).context("保存公钥失败")?;

    println!("Ed25519 密钥对已生成:");
    println!("  私钥: {}", key_path.display());
    println!("  公钥: {}", pub_path.display());
    println!("\n请妥善保管私钥，公钥用于 OTA 更新包验证。");
    Ok(())
}

// Update 非 Linux 平台 stub
#[cfg(not(target_os = "linux"))]
pub async fn cmd_update(_cmd: crate::UpdateCommands) -> Result<()> {
    Err(anyhow!("update commands require Linux"))
}

// ---------------------------------------------------------------------------
// Protocol 子命令（v0.23.0）
// ---------------------------------------------------------------------------

use eneros_device::ProtocolType;

/// 协议连通性测试超时
const PROTO_TEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Protocol 子命令分发
pub async fn cmd_protocol(cmd: crate::ProtocolCommands) -> Result<()> {
    match cmd {
        crate::ProtocolCommands::Status => cmd_protocol_status().await,
        crate::ProtocolCommands::List => cmd_protocol_list().await,
        crate::ProtocolCommands::Test { protocol, address } => {
            cmd_protocol_test(&protocol, &address).await
        }
    }
}

/// 显示所有协议适配器状态（支持协议列表 + 传输层能力）
async fn cmd_protocol_status() -> Result<()> {
    println!("EnerOS 协议适配器状态");
    println!("====================");
    println!();

    let header = ["协议", "传输层", "OSI 层", "端口/EtherType", "状态"];
    let rows: Vec<Vec<String>> = vec![
        vec!["GOOSE".to_string(), "AF_PACKET (L2)".to_string(), "2".to_string(), "0x88B8".to_string(), "可用".to_string()],
        vec!["SV".to_string(), "AF_PACKET (L2)".to_string(), "2".to_string(), "0x88BA".to_string(), "可用".to_string()],
        vec!["IEC 104".to_string(), "TCP / FT 1.2 串口".to_string(), "4-7".to_string(), "2404".to_string(), "可用".to_string()],
        vec!["Modbus TCP".to_string(), "TCP".to_string(), "4-7".to_string(), "502".to_string(), "可用".to_string()],
        vec!["Modbus RTU".to_string(), "串口 (termios)".to_string(), "2".to_string(), "-".to_string(), "可用".to_string()],
        vec!["MQTT".to_string(), "TCP".to_string(), "4-7".to_string(), "1883".to_string(), "可用".to_string()],
        vec!["OPC UA".to_string(), "TCP".to_string(), "4-7".to_string(), "4840".to_string(), "可用".to_string()],
        vec!["DNP3".to_string(), "TCP".to_string(), "4-7".to_string(), "20000".to_string(), "可用".to_string()],
        vec!["IEC 61850 MMS".to_string(), "TCP".to_string(), "4-7".to_string(), "102".to_string(), "可用".to_string()],
    ];

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个协议适配器", rows.len());
    Ok(())
}

/// 列出已注册协议适配器及配置
async fn cmd_protocol_list() -> Result<()> {
    println!("EnerOS 支持的协议类型");
    println!("=====================");
    println!();

    let protocols = [
        ("goose", ProtocolType::Goose),
        ("sv", ProtocolType::Sv),
        ("iec104", ProtocolType::Iec104),
        ("modbus", ProtocolType::Modbus),
        ("mqtt", ProtocolType::Mqtt),
        ("opcua", ProtocolType::OpcUa),
        ("dnp3", ProtocolType::Dnp3),
        ("iec61850", ProtocolType::Iec61850),
    ];

    let header = ["名称", "协议类型", "Layer2", "UDP", "TCP", "默认端口"];
    let rows: Vec<Vec<String>> = protocols
        .iter()
        .map(|(name, pt)| {
            vec![
                name.to_string(),
                format!("{:?}", pt),
                if pt.uses_layer2() { "是" } else { "否" }.to_string(),
                if pt.uses_udp() { "是" } else { "否" }.to_string(),
                if pt.uses_tcp() { "是" } else { "否" }.to_string(),
                pt.default_port().to_string(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个协议类型", protocols.len());
    Ok(())
}

/// 测试指定协议连通性
async fn cmd_protocol_test(protocol: &str, address: &str) -> Result<()> {
    println!("测试协议连通性: {} → {}", protocol, address);
    println!("----------------------------");

    match protocol.to_lowercase().as_str() {
        "goose" | "sv" => cmd_protocol_test_layer2(protocol, address).await,
        "iec104" | "modbus" | "modbus_tcp" | "mqtt" | "opcua" | "dnp3" | "iec61850" => {
            cmd_protocol_test_tcp(protocol, address).await
        }
        "modbus_rtu" => cmd_protocol_test_serial(address).await,
        _ => Err(anyhow!(
            "未知协议 '{}'，支持: goose, sv, iec104, modbus, modbus_rtu, mqtt, opcua, dnp3, iec61850",
            protocol
        )),
    }
}

/// 测试 Layer 2 协议（GOOSE/SV）连通性 — Linux AF_PACKET
#[cfg(target_os = "linux")]
async fn cmd_protocol_test_layer2(protocol: &str, interface: &str) -> Result<()> {
    use eneros_device::{AfPacketConfig, AfPacketTransport, GooseTransport};

    println!("协议: {} (Layer 2)", protocol.to_uppercase());
    println!("网卡: {}", interface);
    println!();

    let src_mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    let config = match protocol.to_lowercase().as_str() {
        "goose" => AfPacketConfig::for_goose(interface, src_mac),
        "sv" => AfPacketConfig::for_sv(interface, src_mac),
        _ => unreachable!(),
    };

    println!("正在打开 AF_PACKET socket...");
    let transport = match AfPacketTransport::new(config) {
        Ok(t) => t,
        Err(e) => {
            println!("✗ 打开 AF_PACKET 失败: {:?}", e);
            return Err(anyhow!("AF_PACKET socket 创建失败: {:?}", e));
        }
    };
    println!("✓ AF_PACKET socket 已打开");

    println!("正在监听 {} 帧（超时 3 秒）...", protocol.to_uppercase());
    let recv_result = tokio::time::timeout(PROTO_TEST_TIMEOUT, transport.receive()).await;

    match recv_result {
        Ok(Ok(frame)) => {
            println!("✓ 收到 {} 帧: {} 字节", protocol.to_uppercase(), frame.len());
            println!("  前 16 字节: {:02X?}", &frame[..frame.len().min(16)]);
            Ok(())
        }
        Ok(Err(e)) => {
            println!("✗ 接收失败: {:?}", e);
            Err(anyhow!("接收 {} 帧失败: {:?}", protocol, e))
        }
        Err(_) => {
            println!("⚠ 3 秒内未收到 {} 帧（可能无发布者）", protocol.to_uppercase());
            Err(anyhow!("3 秒内未收到 {} 帧", protocol.to_uppercase()))
        }
    }
}

/// 非 Linux 平台 Layer 2 测试 stub
#[cfg(not(target_os = "linux"))]
async fn cmd_protocol_test_layer2(protocol: &str, _interface: &str) -> Result<()> {
    Err(anyhow!(
        "{} 协议测试需要 Linux AF_PACKET 支持",
        protocol
    ))
}

/// 测试 TCP 协议连通性
async fn cmd_protocol_test_tcp(protocol: &str, address: &str) -> Result<()> {
    let default_port = match protocol.to_lowercase().as_str() {
        "iec104" => 2404u16,
        "modbus" | "modbus_tcp" => 502,
        "mqtt" => 1883,
        "opcua" => 4840,
        "dnp3" => 20000,
        "iec61850" => 102,
        _ => unreachable!(),
    };

    // 解析地址（支持 host:port / host / [IPv6]:port / [IPv6] / 裸 IPv6）
    let target = if address.starts_with('[') {
        // IPv6 字面量格式: [::1]:port 或 [::1]
        if let Some(close) = address.find(']') {
            let host = &address[1..close];
            let rest = &address[close + 1..];
            if rest.starts_with(':') {
                address.to_string() // 已含端口
            } else {
                format!("[{}]:{}", host, default_port) // 补全默认端口
            }
        } else {
            // 格式异常（[ 无 ]），原样返回交由连接阶段报错
            address.to_string()
        }
    } else if address.matches(':').count() == 1 {
        // host:port 格式（IPv4 或 hostname）
        address.to_string()
    } else if address.matches(':').count() > 1 {
        // 裸 IPv6 地址（无端口，含多个 :）
        format!("[{}]:{}", address, default_port)
    } else {
        // 仅 host（无端口）
        format!("{}:{}", address, default_port)
    };

    println!("协议: {}", protocol.to_uppercase());
    println!("目标: {}", target);
    println!();

    println!("正在连接（超时 3 秒）...");
    let connect_result = tokio::time::timeout(
        PROTO_TEST_TIMEOUT,
        tokio::net::TcpStream::connect(&target),
    )
    .await;

    match connect_result {
        Ok(Ok(_stream)) => {
            println!("✓ TCP 连接成功: {}", target);
            Ok(())
        }
        Ok(Err(e)) => {
            println!("✗ TCP 连接失败: {}", e);
            Err(anyhow!("连接 {} 失败: {}", target, e))
        }
        Err(_) => {
            println!("✗ TCP 连接超时（3 秒）");
            Err(anyhow!("连接 {} 超时", target))
        }
    }
}

/// 测试串口协议连通性 — Linux termios
#[cfg(target_os = "linux")]
async fn cmd_protocol_test_serial(device: &str) -> Result<()> {
    println!("协议: Modbus RTU (串口)");
    println!("设备: {}", device);
    println!();

    // 检查设备文件是否存在
    let dev_path = std::path::Path::new(device);
    if !dev_path.exists() {
        println!("✗ 串口设备不存在: {}", device);
        return Err(anyhow!("串口设备不存在: {}", device));
    }
    println!("✓ 串口设备存在");

    // 尝试打开设备文件验证可访问性
    println!("正在打开串口...");
    match std::fs::OpenOptions::new().read(true).write(true).open(device) {
        Ok(_file) => {
            println!("✓ 串口已打开（可读写）");
            println!("  默认配置: 9600/8/E/1, 从站地址 1");
            println!("\n（连通性测试通过，实际 Modbus RTU 通信需配合从站设备）");
            Ok(())
        }
        Err(e) => {
            println!("✗ 打开串口失败: {}", e);
            Err(anyhow!("打开串口 {} 失败: {}", device, e))
        }
    }
}

/// 非 Linux 平台串口测试 stub
#[cfg(not(target_os = "linux"))]
async fn cmd_protocol_test_serial(_device: &str) -> Result<()> {
    Err(anyhow!("Modbus RTU 串口测试需要 Linux termios 支持"))
}

// ---------------------------------------------------------------------------
// Security 子命令（v0.24.0）
// ---------------------------------------------------------------------------

/// KMS 配置文件路径
#[cfg(target_os = "linux")]
const KMS_CONFIG_PATH: &str = "/etc/eneros/kms.toml";

/// Security 子命令分发
pub async fn cmd_security(cmd: crate::SecurityCommands) -> Result<()> {
    match cmd {
        crate::SecurityCommands::Status => cmd_security_status().await,
        crate::SecurityCommands::Audit { action } => match action {
            crate::SecurityAuditCommands::List {
                since,
                until,
                limit,
            } => cmd_security_audit_list(since.as_deref(), until.as_deref(), limit).await,
            crate::SecurityAuditCommands::Search {
                actor,
                action,
                result,
                since,
                until,
                limit,
            } => {
                cmd_security_audit_search(
                    actor.as_deref(),
                    action.as_deref(),
                    result.as_deref(),
                    since.as_deref(),
                    until.as_deref(),
                    limit,
                )
                .await
            }
            crate::SecurityAuditCommands::Verify => cmd_security_audit_verify().await,
        },
        crate::SecurityCommands::Keys { action } => match action {
            crate::SecurityKeysCommands::List => cmd_security_keys_list().await,
            crate::SecurityKeysCommands::Rotate { key_id } => {
                cmd_security_keys_rotate(&key_id).await
            }
            crate::SecurityKeysCommands::Info { key_id } => {
                cmd_security_keys_info(&key_id).await
            }
        },
    }
}

/// 显示安全状态汇总（Secure Boot + 内核加固 + seccomp + 审计 + KMS）
#[cfg(target_os = "linux")]
async fn cmd_security_status() -> Result<()> {
    use eneros_os::init::security::SecureBootManager;

    println!("EnerOS 安全状态");
    println!("================");
    println!();

    // Secure Boot 状态
    let mgr = SecureBootManager::new();
    let status = mgr.full_status().context("查询安全状态失败")?;

    println!("Secure Boot:");
    println!("  已启用:       {}", if status.secure_boot.enabled { "是" } else { "否" });
    println!("  设置模式:     {}", if status.secure_boot.setup_mode { "是" } else { "否" });
    println!("  PK 已设置:    {}", if status.secure_boot.pk_set { "是" } else { "否" });
    println!("  KEK 已设置:   {}", if status.secure_boot.kek_set { "是" } else { "否" });
    println!("  db 条目数:    {}", status.secure_boot.db_count);
    println!("  dbx 条目数:   {}", status.secure_boot.dbx_count);
    println!();

    // 内核命令行加固参数
    println!("内核命令行加固参数:");
    if status.missing_kernel_cmdline_params.is_empty() {
        println!("  ✓ 所有加固参数已应用");
    } else {
        for p in &status.missing_kernel_cmdline_params {
            println!("  ✗ 缺失: {}", p);
        }
    }
    println!();

    // 内核配置加固选项
    println!("内核配置加固选项:");
    if status.missing_kernel_config_options.is_empty() {
        println!("  ✓ 所有加固选项已启用");
    } else {
        for o in &status.missing_kernel_config_options {
            println!("  ✗ 缺失: {}", o);
        }
    }
    println!();

    // seccomp
    println!("seccomp 沙箱:");
    println!("  可用: {}", if status.seccomp_available { "是" } else { "否" });
    println!();

    // 审计日志
    println!("审计日志:");
    println!("  已初始化: {}", if status.audit_initialized { "是" } else { "否" });
    println!();

    // KMS 密钥库状态
    println!("密钥管理服务 (KMS):");
    let kms_config_path = Path::new(KMS_CONFIG_PATH);
    if kms_config_path.exists() {
        match load_kms_status() {
            Ok(kms_status) => {
                println!("  密钥总数:     {}", kms_status.key_count);
                println!("  活跃密钥:     {}", kms_status.active_keys);
                println!("  需轮换:       {}", kms_status.keys_needing_rotation);
                println!("  存储后端:     {}", kms_status.backend);
            }
            Err(e) => {
                println!("  ✗ 查询 KMS 状态失败: {}", e);
            }
        }
    } else {
        println!("  ✗ KMS 配置文件不存在 ({})", KMS_CONFIG_PATH);
    }

    Ok(())
}

/// 非 Linux 平台 security status stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_status() -> Result<()> {
    Err(anyhow!("security status requires Linux"))
}

/// 加载 KMS 密钥库（读取并解析配置）
#[cfg(target_os = "linux")]
fn load_kms_store() -> Result<eneros_os::init::kms::KeyStore> {
    use eneros_os::init::kms::{KeyStore, KmsConfig};
    let config_path = Path::new(KMS_CONFIG_PATH);
    if !config_path.exists() {
        return Err(anyhow!("KMS 配置文件 {} 不存在", KMS_CONFIG_PATH));
    }
    let config_content = std::fs::read_to_string(config_path).context("读取 KMS 配置失败")?;
    let config: KmsConfig = toml::from_str(&config_content)
        .map_err(|e| anyhow!("解析 KMS 配置失败: {}", e))?;
    let store = KeyStore::load(config).context("加载密钥库失败")?;
    Ok(store)
}

/// 加载 KMS 状态
#[cfg(target_os = "linux")]
fn load_kms_status() -> Result<eneros_os::init::kms::KeyStoreStatus> {
    let store = load_kms_store()?;
    store.status().map_err(|e| anyhow!("查询密钥库状态失败: {}", e))
}

/// 列出审计规则（复用 audit list）
#[cfg(target_os = "linux")]
async fn cmd_security_audit_list(
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    cmd_audit_list(since, until, limit).await
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_audit_list(
    _since: Option<&str>,
    _until: Option<&str>,
    _limit: Option<usize>,
) -> Result<()> {
    Err(anyhow!("security audit commands require Linux"))
}

/// 搜索审计日志（复用 audit search）
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
async fn cmd_security_audit_search(
    actor: Option<&str>,
    action: Option<&str>,
    result: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    cmd_audit_search(actor, action, result, since, until, limit).await
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
async fn cmd_security_audit_search(
    _actor: Option<&str>,
    _action: Option<&str>,
    _result: Option<&str>,
    _since: Option<&str>,
    _until: Option<&str>,
    _limit: Option<usize>,
) -> Result<()> {
    Err(anyhow!("security audit commands require Linux"))
}

/// 验证审计日志完整性（复用 audit verify）
#[cfg(target_os = "linux")]
async fn cmd_security_audit_verify() -> Result<()> {
    cmd_audit_verify().await
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_audit_verify() -> Result<()> {
    Err(anyhow!("security audit commands require Linux"))
}

/// 列出所有密钥元数据
#[cfg(target_os = "linux")]
async fn cmd_security_keys_list() -> Result<()> {
    let store = load_kms_store()?;

    let keys = store.list_keys().context("列出密钥失败")?;

    if keys.is_empty() {
        println!("密钥库为空。");
        return Ok(());
    }

    let header = ["KEY_ID", "类型", "用途", "版本", "创建时间", "使用次数", "状态"];
    let rows: Vec<Vec<String>> = keys
        .iter()
        .map(|k| {
            vec![
                k.key_id.clone(),
                k.key_type.as_str().to_string(),
                k.purpose.clone(),
                k.version.to_string(),
                k.created_at.format("%Y-%m-%d %H:%M").to_string(),
                k.use_count.to_string(),
                if k.revoked {
                    "已撤销".to_string()
                } else if k.is_expired() {
                    "已过期".to_string()
                } else {
                    "活跃".to_string()
                },
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个密钥", keys.len());
    Ok(())
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_keys_list() -> Result<()> {
    Err(anyhow!("security keys commands require Linux"))
}

/// 显示密钥详情
#[cfg(target_os = "linux")]
async fn cmd_security_keys_info(key_id: &str) -> Result<()> {
    let store = load_kms_store()?;

    let meta = store
        .get_metadata(key_id)
        .map_err(|e| anyhow!("查询密钥 {} 失败: {}", key_id, e))?;

    println!("密钥详情: {}", meta.key_id);
    println!("================");
    println!("类型:          {}", meta.key_type.as_str());
    println!("用途:          {}", meta.purpose);
    println!("版本:          {}", meta.version);
    println!("创建时间:      {}", meta.created_at.format("%Y-%m-%d %H:%M:%S"));
    if let Some(exp) = meta.expires_at {
        println!("过期时间:      {}", exp.format("%Y-%m-%d %H:%M:%S"));
    } else {
        println!("过期时间:      永不过期");
    }
    if let Some(last) = meta.last_rotated_at {
        println!("最后轮换:      {}", last.format("%Y-%m-%d %H:%M:%S"));
    } else {
        println!("最后轮换:      从未");
    }
    println!("使用次数:      {}", meta.use_count);
    if let Some(max) = meta.max_uses {
        println!("最大使用次数:  {}", max);
    } else {
        println!("最大使用次数:  不限");
    }
    println!("已撤销:        {}", if meta.revoked { "是" } else { "否" });
    println!("已过期:        {}", if meta.is_expired() { "是" } else { "否" });
    if meta.allowed_consumers.is_empty() {
        println!("允许消费者:    所有人");
    } else {
        println!("允许消费者:    {}", meta.allowed_consumers.join(", "));
    }

    Ok(())
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_keys_info(_key_id: &str) -> Result<()> {
    Err(anyhow!("security keys commands require Linux"))
}

/// 轮换密钥
#[cfg(target_os = "linux")]
async fn cmd_security_keys_rotate(key_id: &str) -> Result<()> {
    let store = load_kms_store()?;

    let rotated = store
        .rotate_key(key_id)
        .map_err(|e| anyhow!("轮换密钥 {} 失败: {}", key_id, e))?;

    println!("✓ 密钥轮换成功");
    println!("  密钥 ID:    {}", rotated.metadata.key_id);
    println!("  旧版本:     {}", rotated.metadata.version.saturating_sub(1));
    println!("  新版本:     {}", rotated.metadata.version);
    println!("  轮换时间:   {}", rotated.metadata.last_rotated_at.unwrap_or_else(|| chrono::Utc::now()).format("%Y-%m-%d %H:%M:%S"));
    println!("  使用计数:   已重置为 0");

    Ok(())
}

/// 非 Linux stub
#[cfg(not(target_os = "linux"))]
async fn cmd_security_keys_rotate(_key_id: &str) -> Result<()> {
    Err(anyhow!("security keys commands require Linux"))
}

// ---------------------------------------------------------------------------
// HA 子命令（v0.26.0 — 通过 IPC 查询 ha-daemon）
// ---------------------------------------------------------------------------

/// HA 守护进程 IPC 地址
const HA_DAEMON_ADDR: &str = "127.0.0.1:5402";

/// HA 守护进程 IPC 响应（JSON 行协议）
#[derive(Deserialize)]
struct HaResponse {
    /// "ok" 或 "error"
    status: String,
    /// 响应数据（成功时为具体 payload，失败时可能为 null）
    #[serde(default)]
    data: serde_json::Value,
    /// 附加消息（成功时可能为空，失败时为错误描述）
    #[serde(default)]
    message: String,
}

/// 连接到指定地址的 ha-daemon（带 500ms 超时）
async fn ha_connect_to(addr: &str) -> Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("连接 ha-daemon {} 超时", addr))?
        .map_err(|e| anyhow!("连接 ha-daemon {} 失败: {}", addr, e))?;
    Ok(stream)
}

/// 发送 HA IPC 请求，返回响应 data 字段
///
/// 协议：发送一行 JSON `{"command":"...","args":...}`，接收一行 JSON 响应。
async fn ha_request(command: &str, args: Option<&serde_json::Value>) -> Result<serde_json::Value> {
    ha_request_to(HA_DAEMON_ADDR, command, args).await
}

/// 发送 HA IPC 请求到指定地址，返回响应 data 字段
async fn ha_request_to(
    addr: &str,
    command: &str,
    args: Option<&serde_json::Value>,
) -> Result<serde_json::Value> {
    let mut stream = ha_connect_to(addr).await?;
    let req = serde_json::json!({
        "command": command,
        "args": args,
    });
    let json = serde_json::to_string(&req)?;
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let resp: HaResponse = serde_json::from_str(&line)?;
    if resp.status == "ok" {
        Ok(resp.data)
    } else {
        Err(anyhow!("HA 守护进程返回错误: {}", resp.message))
    }
}

/// 发送 HA IPC 请求，连接失败时返回友好错误
async fn ha_query(command: &str, args: Option<&serde_json::Value>) -> Result<serde_json::Value> {
    ha_request(command, args)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("连接 ha-daemon") {
                anyhow!("错误：ha-daemon 未运行，请检查 eneros-ha 服务状态")
            } else {
                e
            }
        })
}

/// HA 子命令分发
pub async fn cmd_ha(action: crate::HaCommands) -> Result<()> {
    match action {
        crate::HaCommands::Status => cmd_ha_status().await,
        crate::HaCommands::Nodes => cmd_ha_nodes().await,
        crate::HaCommands::SyncStatus => cmd_ha_sync_status().await,
        crate::HaCommands::FailoverStatus => cmd_failover_status().await,
        crate::HaCommands::FailoverTrigger { force } => cmd_failover_trigger(force).await,
        crate::HaCommands::FailoverHistory => cmd_failover_history().await,
        crate::HaCommands::FailoverDrill { scenario } => cmd_failover_drill(scenario).await,
    }
}

/// 显示 HA 状态（节点角色、心跳、同步、failover）
async fn cmd_ha_status() -> Result<()> {
    let data = ha_query("ha_status", None).await?;

    println!("EnerOS 高可用状态");
    println!("================");
    println!();

    // 本节点信息
    println!("本节点:");
    println!(
        "  节点 ID:    {}",
        data.get("node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );
    println!(
        "  角色:       {}",
        data.get("role").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "  优先级:     {}",
        data.get("priority")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    );
    println!();

    // 心跳状态
    println!("心跳状态:");
    if let Some(local_hb) = data.get("local_heartbeat") {
        println!(
            "  本地间隔:   {} ms",
            local_hb
                .get("interval_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  本地状态:   {}",
            local_hb
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
        );
    }
    if let Some(peers) = data.get("peer_nodes").and_then(|v| v.as_array()) {
        println!("  对端节点 ({}):", peers.len());
        for peer in peers {
            println!(
                "    {} — {} ({}, 优先级 {})",
                peer.get("node_id").and_then(|v| v.as_str()).unwrap_or("-"),
                peer.get("state").and_then(|v| v.as_str()).unwrap_or("-"),
                peer.get("role").and_then(|v| v.as_str()).unwrap_or("-"),
                peer.get("priority").and_then(|v| v.as_u64()).unwrap_or(0),
            );
        }
    }
    println!();

    // 同步状态
    println!("同步状态:");
    if let Some(sync) = data.get("sync") {
        println!(
            "  已连接:     {}",
            if sync
                .get("is_connected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "是"
            } else {
                "否"
            }
        );
        println!(
            "  对端节点:   {}",
            sync.get("peer_node_id")
                .and_then(|v| v.as_str())
                .unwrap_or("(未知)")
        );
        if let Some(stats) = sync.get("stats") {
            println!(
                "  已发送:     {}",
                stats
                    .get("total_sent")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  已接收:     {}",
                stats
                    .get("total_received")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
        }
    }
    println!();

    // Failover 状态
    println!("Failover 状态:");
    if let Some(fo) = data.get("failover") {
        println!(
            "  当前状态:   {}",
            fo.get("current_state")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
        );
        println!(
            "  只读:       {}",
            if fo
                .get("is_readonly")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "是"
            } else {
                "否"
            }
        );
        println!(
            "  VIP:        {}",
            fo.get("vip").and_then(|v| v.as_str()).unwrap_or("-")
        );
    }

    Ok(())
}

/// 列出集群节点
async fn cmd_ha_nodes() -> Result<()> {
    let data = ha_query("ha_nodes", None).await?;

    println!("EnerOS HA 集群节点");
    println!("==================");
    println!();

    let nodes = data
        .get("nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if nodes.is_empty() {
        println!("(无已知节点)");
        return Ok(());
    }

    let header = ["节点 ID", "角色", "状态", "优先级", "最后心跳"];
    let rows: Vec<Vec<String>> = nodes
        .iter()
        .map(|n| {
            vec![
                n.get("node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                n.get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                n.get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                n.get("priority")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .to_string(),
                n.get("last_heartbeat")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个节点", nodes.len());

    Ok(())
}

/// 显示同步状态
async fn cmd_ha_sync_status() -> Result<()> {
    let data = ha_query("ha_sync_status", None).await?;

    println!("EnerOS HA 同步状态");
    println!("==================");
    println!();

    println!("连接状态:");
    println!(
        "  已连接:     {}",
        if data
            .get("is_connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "是"
        } else {
            "否"
        }
    );
    println!(
        "  对端节点:   {}",
        data.get("peer_node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("(未知)")
    );
    println!();

    println!("同步统计:");
    if let Some(stats) = data.get("stats") {
        println!(
            "  已发送:     {}",
            stats
                .get("total_sent")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  已接收:     {}",
            stats
                .get("total_received")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  错误数:     {}",
            stats
                .get("errors")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  最近延迟:   {} ms",
            stats
                .get("latency_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
    }

    Ok(())
}

/// 显示 failover 状态（状态机、VIP、上次切换）
async fn cmd_failover_status() -> Result<()> {
    let data = ha_query("failover_status", None).await?;

    println!("EnerOS HA Failover 状态");
    println!("=======================");
    println!();

    println!(
        "当前状态:   {}",
        data.get("current_state")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );
    println!(
        "节点角色:   {}",
        data.get("role").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "VIP:        {}",
        data.get("vip").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "只读:       {}",
        if data
            .get("is_readonly")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "是"
        } else {
            "否"
        }
    );
    println!(
        "上次切换:   {}",
        data.get("last_failover")
            .and_then(|v| v.as_str())
            .unwrap_or("(无)")
    );

    Ok(())
}

/// 手动触发 failover 切换
async fn cmd_failover_trigger(force: bool) -> Result<()> {
    // 交互式确认（未指定 --force 时要求输入 yes）
    if !force {
        println!("警告：failover 切换是高风险操作，将触发主备倒换。");
        println!("确认执行请输入 yes，否则输入其他任意内容取消：");
        let mut input = String::new();
        // async 函数中应使用 tokio 异步 stdin，避免阻塞 runtime 线程
        let mut stdin = BufReader::new(tokio::io::stdin());
        stdin
            .read_line(&mut input)
            .await
            .map_err(|e| anyhow!("读取输入失败: {}", e))?;
        if input.trim() != "yes" {
            println!("已取消 failover 切换。");
            return Ok(());
        }
    }

    let start = std::time::Instant::now();
    let data = ha_query("failover_trigger", None).await?;
    let elapsed = start.elapsed();

    let success = data
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let message = data
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if success {
        println!("✓ Failover 切换成功（耗时 {} ms）", elapsed.as_millis());
    } else {
        println!("✗ Failover 切换失败（耗时 {} ms）", elapsed.as_millis());
    }
    if !message.is_empty() {
        println!("  {}", message);
    }

    Ok(())
}

/// 显示 failover 切换历史
async fn cmd_failover_history() -> Result<()> {
    let data = ha_query("failover_history", None).await?;

    println!("EnerOS HA Failover 切换历史");
    println!("===========================");
    println!();

    let history = data
        .get("history")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if history.is_empty() {
        println!("(无切换记录)");
        return Ok(());
    }

    let header = ["时间", "从状态", "到状态", "原因", "耗时(ms)", "结果"];
    let rows: Vec<Vec<String>> = history
        .iter()
        .map(|h| {
            vec![
                h.get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                h.get("from_state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                h.get("to_state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                h.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                h.get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .to_string(),
                h.get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 条切换记录", history.len());

    Ok(())
}

/// 触发灾备演练
async fn cmd_failover_drill(scenario: String) -> Result<()> {
    // 校验场景
    let valid_scenarios = ["primary_down", "network_partition", "disk_failure"];
    if !valid_scenarios.contains(&scenario.as_str()) {
        return Err(anyhow!(
            "无效演练场景 '{}'，可选: {}",
            scenario,
            valid_scenarios.join(", ")
        ));
    }

    println!("触发灾备演练: {}", scenario);
    println!("================");
    println!();

    let args = serde_json::json!({ "scenario": scenario });
    let data = ha_query("failover_drill", Some(&args)).await?;

    let success = data
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let message = data
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if success {
        println!("✓ 演练成功");
    } else {
        println!("✗ 演练失败");
    }
    if !message.is_empty() {
        println!("  {}", message);
    }

    // 显示演练详情
    if let Some(details) = data.get("details") {
        println!();
        println!("演练详情:");
        println!(
            "  耗时:       {} ms",
            details
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        );
        if let Some(steps) = details.get("steps").and_then(|v| v.as_array()) {
            println!("  步骤 ({}):", steps.len());
            for step in steps {
                println!(
                    "    {} — {}",
                    step.get("name").and_then(|v| v.as_str()).unwrap_or("-"),
                    step.get("result").and_then(|v| v.as_str()).unwrap_or("-"),
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin 子命令（v0.27.0 — v0.28.0 Task 15: 改为通过 IPC 调用 plugin-daemon）
// ---------------------------------------------------------------------------

/// plugin-daemon IPC 默认地址（Unix socket 或 TCP 回退）
#[allow(dead_code)]
const PLUGIN_DAEMON_ADDR: &str = "/var/run/eneros/plugin-daemon.sock";
/// plugin-daemon TCP 回退地址（非 Unix 平台）
const PLUGIN_DAEMON_TCP_ADDR: &str = "127.0.0.1:5410";

/// 获取 plugin-daemon IPC 客户端
///
/// 尝试连接默认地址（Unix socket: /var/run/eneros/plugin-daemon.sock 或 TCP: 127.0.0.1:5410），
/// 通过 `is_reachable()` 检查连通性。无 daemon 运行时返回友好错误提示。
pub fn get_daemon_client() -> Result<PluginDaemonClient, String> {
    // 优先尝试 Unix socket（Linux），回退到 TCP
    #[cfg(unix)]
    {
        let client = PluginDaemonClient::new(PLUGIN_DAEMON_ADDR);
        if client.is_reachable() {
            return Ok(client);
        }
    }

    // TCP 回退（所有平台）
    let client = PluginDaemonClient::new(PLUGIN_DAEMON_TCP_ADDR);
    if client.is_reachable() {
        return Ok(client);
    }

    Err("plugin-daemon 未运行，请先启动：enerosctl service start plugin-daemon".to_string())
}

/// Plugin 子命令分发
pub async fn cmd_plugin(cmd: crate::PluginCommands) -> Result<()> {
    match cmd {
        crate::PluginCommands::List => cmd_plugin_list().await,
        crate::PluginCommands::Load {
            path,
            skip_signature,
        } => cmd_plugin_load(&path, skip_signature).await,
        crate::PluginCommands::Unload { name } => cmd_plugin_unload(&name).await,
        crate::PluginCommands::Info { name } => cmd_plugin_info(&name).await,
        crate::PluginCommands::Verify { path, sig } => {
            cmd_plugin_verify(&path, sig.as_deref()).await
        }
        crate::PluginCommands::Enable { name } => cmd_plugin_enable(&name).await,
        crate::PluginCommands::Disable { name } => cmd_plugin_disable(&name).await,
        crate::PluginCommands::GenKeys { output } => cmd_plugin_genkeys(&output).await,
        crate::PluginCommands::Sign { plugin, key } => cmd_plugin_sign(&plugin, &key).await,
    }
}

/// 格式化插件类型为可读字符串
#[allow(dead_code)]
fn format_plugin_type(t: &PluginType) -> &'static str {
    match t {
        PluginType::Protocol => "Protocol",
        PluginType::Agent => "Agent",
        PluginType::Analysis => "Analysis",
    }
}

/// 扫描插件目录，返回所有 manifest.toml 的路径列表
///
/// 查找两种布局：
/// - `<dir>/<name>/manifest.toml`（每个插件一个子目录）
/// - `<dir>/<name>.toml`（扁平 manifest 文件）
#[allow(dead_code)]
fn collect_plugin_manifests(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut manifests = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return manifests;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let m = path.join("manifest.toml");
            if m.exists() {
                manifests.push(m);
            }
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext == "toml" {
                    manifests.push(path);
                }
            }
        }
    }
    manifests
}

/// 列出已加载的插件（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程，
/// 不再直接扫描本地插件目录。
async fn cmd_plugin_list() -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .list()
        .map_err(|e| anyhow!("IPC 调用 list 失败: {}", e))?;
    handle_plugin_list_response(&resp)
}

/// 处理 list 命令的 IPC 响应（提取为独立函数便于单元测试 mock）
fn handle_plugin_list_response(resp: &DaemonResponse) -> Result<()> {
    if !resp.ok {
        return Err(anyhow!(
            "列出插件失败: {}",
            resp.error.as_deref().unwrap_or("未知错误")
        ));
    }

    let plugins = resp
        .data
        .as_ref()
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("响应数据格式错误：期望插件数组"))?;

    if plugins.is_empty() {
        println!("当前没有已加载的插件。");
        return Ok(());
    }

    let header = ["名称", "版本", "类型", "状态", "启用"];
    let mut rows: Vec<Vec<String>> = Vec::new();
    for p in plugins {
        rows.push(vec![
            p.get("name").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            p.get("version").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            p.get("type").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            p.get("state").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            p.get("enabled")
                .and_then(|v| v.as_bool())
                .map(|b| if b { "是" } else { "否" }.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }

    println!("EnerOS 插件列表（via plugin-daemon IPC）");
    println!("===============");
    println!();
    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个插件", plugins.len());
    Ok(())
}

/// 加载插件（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程，
/// daemon 负责：验证签名 → 加载库 → 初始化 → 启动。
async fn cmd_plugin_load(path: &str, skip_signature: bool) -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .load(path, skip_signature)
        .map_err(|e| anyhow!("IPC 调用 load 失败: {}", e))?;
    handle_plugin_load_response(&resp, path, skip_signature)
}

/// 处理 load 命令的 IPC 响应（提取为独立函数便于单元测试 mock）
fn handle_plugin_load_response(resp: &DaemonResponse, path: &str, skip_signature: bool) -> Result<()> {
    if !resp.ok {
        return Err(anyhow!(
            "加载插件失败: {}",
            resp.error.as_deref().unwrap_or("未知错误")
        ));
    }

    println!("插件加载请求已发送至 plugin-daemon:");
    println!("  路径:         {}", path);
    println!("  跳过签名验证: {}", if skip_signature { "是" } else { "否" });
    if let Some(data) = &resp.data {
        if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
            println!("  插件名称:     {}", name);
        }
        if let Some(version) = data.get("version").and_then(|v| v.as_str()) {
            println!("  插件版本:     {}", version);
        }
        if let Some(state) = data.get("state").and_then(|v| v.as_str()) {
            println!("  当前状态:     {}", state);
        }
    }
    Ok(())
}

/// 解析插件路径，返回 (manifest_path, lib_path)
///
/// - 目录：查找目录下的 manifest.toml，动态库为目录下同名的 .so/.dll/.dylib
/// - .toml 文件：manifest 即此文件，动态库为同目录下同名（去 .toml）的 .so/.dll/.dylib
/// - 动态库文件：lib 即此文件，manifest 为同目录下的 manifest.toml
#[allow(dead_code)]
fn resolve_plugin_paths(input: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    if !input.exists() {
        return Err(anyhow!("路径不存在: {}", input.display()));
    }

    if input.is_dir() {
        let manifest = input.join("manifest.toml");
        if !manifest.exists() {
            return Err(anyhow!(
                "目录中未找到 manifest.toml: {}",
                input.display()
            ));
        }
        let lib = find_library_in_dir(input)
            .ok_or_else(|| anyhow!("目录中未找到动态库: {}", input.display()))?;
        return Ok((manifest, lib));
    }

    // 文件：根据扩展名判断
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if ext == "toml" {
        // manifest 文件
        let manifest = input.to_path_buf();
        let dir = input
            .parent()
            .ok_or_else(|| anyhow!("无法获取父目录: {}", input.display()))?;
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("无法获取文件名: {}", input.display()))?;
        let lib = find_library_by_name(dir, stem)
            .ok_or_else(|| anyhow!("未找到对应的动态库: {}", input.display()))?;
        return Ok((manifest, lib));
    }

    if matches!(ext, "so" | "dll" | "dylib") {
        // 动态库文件
        let lib = input.to_path_buf();
        let dir = input
            .parent()
            .ok_or_else(|| anyhow!("无法获取父目录: {}", input.display()))?;
        let manifest = dir.join("manifest.toml");
        if !manifest.exists() {
            return Err(anyhow!(
                "未找到 manifest.toml（动态库同目录下）: {}",
                manifest.display()
            ));
        }
        return Ok((manifest, lib));
    }

    Err(anyhow!(
        "无法识别的路径类型（期望目录/.toml/.so/.dll/.dylib）: {}",
        input.display()
    ))
}

/// 在目录中查找动态库文件（.so/.dll/.dylib）
#[allow(dead_code)]
fn find_library_in_dir(dir: &Path) -> Option<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "so" | "dll" | "dylib") {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// 在目录中按名称查找动态库文件
#[allow(dead_code)]
fn find_library_by_name(dir: &Path, stem: &str) -> Option<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                if file_stem == stem {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if matches!(ext, "so" | "dll" | "dylib") {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

/// 卸载插件（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程。
async fn cmd_plugin_unload(name: &str) -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .unload(name)
        .map_err(|e| anyhow!("IPC 调用 unload 失败: {}", e))?;
    handle_simple_response(&resp, &format!("卸载插件 {}", name))
}

/// 显示插件详情（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程，
/// 返回 manifest + state + statistics。
async fn cmd_plugin_info(name: &str) -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .info(name)
        .map_err(|e| anyhow!("IPC 调用 info 失败: {}", e))?;
    handle_plugin_info_response(&resp, name)
}

/// 处理 info 命令的 IPC 响应（提取为独立函数便于单元测试 mock）
fn handle_plugin_info_response(resp: &DaemonResponse, name: &str) -> Result<()> {
    if !resp.ok {
        return Err(anyhow!(
            "查询插件 {} 信息失败: {}",
            name,
            resp.error.as_deref().unwrap_or("未知错误")
        ));
    }

    let data = resp
        .data
        .as_ref()
        .ok_or_else(|| anyhow!("响应数据为空"))?;

    println!("EnerOS 插件详情（via plugin-daemon IPC）");
    println!("===============");
    println!();
    println!("基本信息:");
    println!("  名称:       {}", data.get("name").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  版本:       {}", data.get("version").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  API 版本:   {}", data.get("api_version").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  类型:       {}", data.get("type").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  描述:       {}", data.get("description").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  作者:       {}", data.get("author").and_then(|v| v.as_str()).unwrap_or("-"));
    println!();
    println!("运行时状态:");
    println!("  状态:       {}", data.get("state").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  启用:       {}", data.get("enabled").and_then(|v| v.as_bool()).map(|b| if b { "是" } else { "否" }).unwrap_or("-"));
    if let Some(stats) = data.get("statistics").and_then(|v| v.as_object()) {
        println!("  统计:");
        for (k, v) in stats {
            println!("    {}: {}", k, v);
        }
    }
    Ok(())
}

/// 验证插件签名（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程。
/// 注意：IPC verify 不支持自定义签名文件路径（--sig），如指定 --sig 将提示忽略。
async fn cmd_plugin_verify(path: &str, sig_path: Option<&str>) -> Result<()> {
    if sig_path.is_some() {
        println!("提示：--sig 选项在 IPC 模式下被忽略，plugin-daemon 使用默认签名文件路径。如需自定义签名路径，请配置 plugin-daemon。");
    }
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .verify(path)
        .map_err(|e| anyhow!("IPC 调用 verify 失败: {}", e))?;
    handle_plugin_verify_response(&resp, path)
}

/// 处理 verify 命令的 IPC 响应（提取为独立函数便于单元测试 mock）
fn handle_plugin_verify_response(resp: &DaemonResponse, path: &str) -> Result<()> {
    if !resp.ok {
        return Err(anyhow!(
            "验证插件签名失败: {}",
            resp.error.as_deref().unwrap_or("未知错误")
        ));
    }

    let data = resp
        .data
        .as_ref()
        .ok_or_else(|| anyhow!("响应数据为空"))?;

    let result = data
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    println!("签名验证结果（via plugin-daemon IPC）:");
    println!("  插件文件:   {}", path);
    match result {
        "valid" => {
            println!("  结果:       有效");
            if let Some(signer) = data.get("signer").and_then(|v| v.as_str()) {
                println!("  签名者:     {}", signer);
            }
        }
        "missing" => {
            println!("  结果:       签名文件缺失");
        }
        "invalid" => {
            println!("  结果:       无效");
            if let Some(reason) = data.get("reason").and_then(|v| v.as_str()) {
                println!("  原因:       {}", reason);
            }
        }
        "untrusted" => {
            println!("  结果:       不可信签名者");
            if let Some(signer) = data.get("signer").and_then(|v| v.as_str()) {
                println!("  签名者:     {}", signer);
            }
        }
        _ => {
            println!("  结果:       {}", result);
        }
    }
    Ok(())
}

/// 启用插件（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程。
async fn cmd_plugin_enable(name: &str) -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .enable(name)
        .map_err(|e| anyhow!("IPC 调用 enable 失败: {}", e))?;
    handle_simple_response(&resp, &format!("启用插件 {}", name))
}

/// 禁用插件（通过 plugin-daemon IPC 调用）
///
/// v0.28.0 Task 15: 改为通过 PluginDaemonClient IPC 调用 plugin-daemon 守护进程。
async fn cmd_plugin_disable(name: &str) -> Result<()> {
    let client = get_daemon_client().map_err(anyhow::Error::msg)?;
    let resp = client
        .disable(name)
        .map_err(|e| anyhow!("IPC 调用 disable 失败: {}", e))?;
    handle_simple_response(&resp, &format!("禁用插件 {}", name))
}

/// 处理简单的成功/失败 IPC 响应（unload/enable/disable 等无复杂数据的命令）
fn handle_simple_response(resp: &DaemonResponse, action: &str) -> Result<()> {
    if !resp.ok {
        return Err(anyhow!(
            "{}失败: {}",
            action,
            resp.error.as_deref().unwrap_or("未知错误")
        ));
    }
    println!("{}成功", action);
    Ok(())
}

/// 生成插件签名密钥对（Ed25519）
async fn cmd_plugin_genkeys(output: &str) -> Result<()> {
    let output_dir = Path::new(output);
    let (priv_path, pub_path) = eneros_plugin::signature::generate_keypair(output_dir)
        .context("生成密钥对失败")?;

    println!("Ed25519 密钥对已生成:");
    println!("  私钥: {}", priv_path.display());
    println!("  公钥: {}", pub_path.display());
    println!();
    println!("请妥善保管私钥，公钥用于插件签名验证。");
    println!(
        "将公钥复制到 {} 目录以使其成为可信签名者。",
        PLUGIN_KEYS_DIR
    );
    Ok(())
}

/// 对插件文件签名
async fn cmd_plugin_sign(plugin: &str, key: &str) -> Result<()> {
    let plugin_path = Path::new(plugin);
    let key_path = Path::new(key);

    if !plugin_path.exists() {
        return Err(anyhow!("插件文件不存在: {}", plugin_path.display()));
    }
    if !key_path.exists() {
        return Err(anyhow!("私钥文件不存在: {}", key_path.display()));
    }

    let sig_path = eneros_plugin::signature::sign_plugin(plugin_path, key_path)
        .context("签名插件失败")?;

    println!("插件签名成功:");
    println!("  插件文件:   {}", plugin_path.display());
    println!("  私钥文件:   {}", key_path.display());
    println!("  签名文件:   {}", sig_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulator 子命令（v0.28.0 — Task 15）
// ---------------------------------------------------------------------------

/// Simulator 子命令分发
///
/// simulator 命令直接调用 eneros-simulator 库，不需要 plugin-daemon 守护进程。
pub async fn cmd_simulator(action: crate::SimulatorAction) -> Result<()> {
    match action {
        crate::SimulatorAction::Run { path } => cmd_simulator_run(&path).await,
        crate::SimulatorAction::Validate { path } => cmd_simulator_validate(&path).await,
        crate::SimulatorAction::List => cmd_simulator_list_scenarios().await,
    }
}

/// 运行场景脚本
///
/// 加载 TOML 场景脚本，调用 ScenarioRunner 按时间线执行事件，输出 RunResult 摘要。
/// simulator 命令直接调用 eneros-simulator 库，不需要 daemon。
pub async fn cmd_simulator_run(scenario_path: &Path) -> Result<()> {
    let scenario = Scenario::load_from_file(&scenario_path.to_string_lossy())
        .map_err(|e| anyhow!("加载场景失败: {}", e))?;

    println!("正在运行场景: {}", scenario.name);
    println!("描述: {}", scenario.description);
    println!(
        "时长: {:.1}s, 步长: {:.1}s, 事件数: {}",
        scenario.duration,
        scenario.time_step,
        scenario.timeline.len()
    );
    println!();
    println!("事件时间线:");

    let runner = ScenarioRunner::new(scenario);
    let result = runner
        .run(|time, event| {
            println!("  [{:>7.2}s] {:?}", time, event.action);
        })
        .map_err(|e| anyhow!("场景运行失败: {}", e))?;

    println!();
    println!("运行结果摘要:");
    println!("  已执行事件: {}", result.events_executed);
    println!("  场景时长:   {:.1}s", result.duration);
    println!("  观察记录点: {}", result.observations.len());

    if !result.observations.is_empty() {
        println!();
        println!("观察记录:");
        for obs in &result.observations {
            println!(
                "  [{:>7.2}s] {} 个状态参数",
                obs.time,
                obs.state.len()
            );
        }
    }

    Ok(())
}

/// 验证场景脚本语法（解析 + 类型检查）
///
/// 解析 TOML 场景脚本并调用 validate() 检查配置完整性，输出验证结果。
/// simulator 命令直接调用 eneros-simulator 库，不需要 daemon。
pub async fn cmd_simulator_validate(scenario_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(scenario_path)
        .with_context(|| format!("读取场景文件失败: {}", scenario_path.display()))?;

    match Scenario::load_from_str(&content) {
        Ok(scenario) => match scenario.validate() {
            Ok(()) => {
                println!("场景验证通过: {}", scenario_path.display());
                println!("  名称:     {}", scenario.name);
                println!("  描述:     {}", scenario.description);
                println!("  时长:     {:.1}s", scenario.duration);
                println!("  步长:     {:.1}s", scenario.time_step);
                println!("  事件数:   {}", scenario.timeline.len());
                Ok(())
            }
            Err(e) => Err(anyhow!("场景校验失败: {}", e)),
        },
        Err(e) => Err(anyhow!("场景解析失败: {}", e)),
    }
}

/// 列出 FaultScenarioLibrary 内置故障场景
///
/// 输出 5 个预置故障场景：N-1/N-2/级联/保护拒动/保护误动。
/// simulator 命令直接调用 eneros-simulator 库，不需要 daemon。
pub async fn cmd_simulator_list_scenarios() -> Result<()> {
    let library = FaultScenarioLibrary::new();
    let scenarios = library.list();

    println!("EnerOS 模拟器内置故障场景");
    println!("===============");
    println!();

    let header = ["名称", "类型", "故障数", "描述"];
    let rows: Vec<Vec<String>> = scenarios
        .iter()
        .map(|s| {
            vec![
                s.name.clone(),
                format!("{:?}", s.scenario_type),
                s.faults.len().to_string(),
                s.description.clone(),
            ]
        })
        .collect();

    print!("{}", format_table(&header, &rows));
    println!("\n共 {} 个内置场景", scenarios.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// 命令分发（供 main 和交互式 shell 共用，避免 shell 递归）
// ---------------------------------------------------------------------------

/// 分发顶层命令到对应的处理函数。
///
/// main 函数和交互式 shell 都调用此函数，确保命令执行逻辑只有一份。
/// 交互式 shell 在调用前会拦截 `Commands::Shell` 以避免递归。
pub async fn dispatch_command(command: crate::Commands, socket: &str) -> Result<()> {
    use crate::{
        AgentCommands, AuditCommands, Commands, DeviceCommands, EventbusCommands,
        FirewallCommands, LogCommands, NetworkCommands, SystemCommands, TimeCommands,
    };

    match command {
        Commands::Agent { action } => match action {
            AgentCommands::List => cmd_agent_list(socket).await,
            AgentCommands::Start { agent_id } => cmd_agent_start(socket, &agent_id).await,
            AgentCommands::Stop { agent_id } => cmd_agent_stop(socket, &agent_id).await,
            AgentCommands::Status { agent_id } => cmd_agent_status(socket, &agent_id).await,
            AgentCommands::Restart { agent_id } => cmd_agent_restart(socket, &agent_id).await,
        },
        Commands::Eventbus { action } => match action {
            EventbusCommands::Status => cmd_eventbus_status(socket).await,
            EventbusCommands::Subscribe { topic } => {
                cmd_eventbus_subscribe(socket, topic.as_deref()).await
            }
        },
        Commands::System { action } => match action {
            SystemCommands::Info => cmd_system_info(socket).await,
        },
        Commands::Network { action } => match action {
            NetworkCommands::Status => cmd_network_status().await,
            NetworkCommands::Config { interface } => {
                cmd_network_config(interface.as_deref()).await
            }
            NetworkCommands::Firewall { action } => match action {
                Some(FirewallCommands::List) => cmd_network_firewall_list().await,
                Some(FirewallCommands::Policy) => cmd_network_firewall_policy().await,
                None => cmd_network_firewall_list().await,
            },
            NetworkCommands::Bond { interface } => {
                cmd_network_bond_status(interface.as_deref()).await
            }
        },
        Commands::Log { action } => match action {
            LogCommands::Tail {
                category,
                lines,
                follow,
                json,
            } => cmd_log_tail(category.as_deref(), lines, follow, json).await,
            LogCommands::Search {
                pattern,
                category,
                level,
                since,
                until,
                source,
                json,
            } => {
                cmd_log_search(
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
                cmd_log_level(&target, level.as_deref()).await
            }
            LogCommands::Export {
                start,
                end,
                format,
                category,
                output,
            } => {
                cmd_log_export(
                    start.as_deref(),
                    end.as_deref(),
                    &format,
                    category.as_deref(),
                    output.as_deref(),
                )
                .await
            }
            LogCommands::Rotate { category } => cmd_log_rotate(&category).await,
        },
        Commands::Device { action } => match action {
            DeviceCommands::List { r#type } => cmd_device_list(r#type.as_deref()).await,
            DeviceCommands::Info { device } => cmd_device_info(&device).await,
            DeviceCommands::Config { device, preset, baud } => {
                cmd_device_config(&device, preset.as_deref(), baud).await
            }
            DeviceCommands::Monitor => cmd_device_monitor().await,
        },
        Commands::Audit { action } => match action {
            AuditCommands::List { since, until, limit } => {
                cmd_audit_list(since.as_deref(), until.as_deref(), limit).await
            }
            AuditCommands::Verify => cmd_audit_verify().await,
            AuditCommands::Search {
                actor,
                action,
                result,
                since,
                until,
                limit,
            } => {
                cmd_audit_search(
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
            TimeCommands::Status => cmd_time_status().await,
            TimeCommands::SetSource { source } => cmd_time_set_source(&source).await,
            TimeCommands::Sync => cmd_time_sync().await,
        },
        Commands::Update(cmd) => cmd_update(cmd).await,
        Commands::Protocol(cmd) => cmd_protocol(cmd).await,
        Commands::Security(cmd) => cmd_security(cmd).await,
        Commands::Ha(action) => cmd_ha(action).await,
        Commands::Plugin(cmd) => cmd_plugin(cmd).await,
        Commands::Simulator { action } => cmd_simulator(action).await,
        Commands::Shell => cmd_shell(socket).await,
        Commands::Completions { shell } => cmd_completions(shell),
        Commands::Config { action } => cmd_config(action).await,
        Commands::Service { action } => cmd_service(action).await,
        Commands::Doctor => cmd_doctor().await,
    }
}

// ---------------------------------------------------------------------------
// Shell / Completions 子命令（v0.28.0 — Task 13）
// ---------------------------------------------------------------------------

/// 启动交互式 shell
pub async fn cmd_shell(socket: &str) -> Result<()> {
    let mut shell = crate::shell::InteractiveShell::new(socket)
        .map_err(|e| anyhow!("启动交互式 shell 失败: {}", e))?;
    shell.run().await.map_err(|e| anyhow!("shell 运行错误: {}", e))?;
    Ok(())
}

/// 生成 shell 补全脚本并输出到 stdout
pub fn cmd_completions(shell: clap_complete::Shell) -> Result<()> {
    let script = generate_completions(shell);
    print!("{}", script);
    Ok(())
}

/// 生成补全脚本为字符串（供 cmd_completions 和测试共用）
pub fn generate_completions(shell: clap_complete::Shell) -> String {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, "enerosctl", &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
// ---------------------------------------------------------------------------
// Config 子命令（v0.29.0 — Task 14）
// ---------------------------------------------------------------------------

/// EnerOS 配置目录
const CONFIG_DIR: &str = "/etc/eneros";

/// Config 子命令分发
pub async fn cmd_config(action: crate::ConfigAction) -> Result<()> {
    match action {
        crate::ConfigAction::Get { key } => cmd_config_get(&key),
        crate::ConfigAction::Set { key, value } => cmd_config_set(&key, &value),
        crate::ConfigAction::Edit { file } => cmd_config_edit(&file),
        crate::ConfigAction::List => cmd_config_list(),
    }
}

/// 解析配置键（格式：file.field，如 plugin.require_signature）
///
/// 返回 (file_name, field_path)，file_name 不含扩展名。
/// 若键中不含 `.`，则将整个键视为 file_name，field_path 为空。
pub fn parse_config_key(key: &str) -> (String, String) {
    match key.find('.') {
        Some(idx) => {
            let (file, field) = key.split_at(idx);
            (file.to_string(), field[1..].to_string())
        }
        None => (key.to_string(), String::new()),
    }
}

/// 校验配置文件名，防止路径遍历攻击
///
/// 拒绝空字符串、包含路径分隔符（`/` 或 `\`）、包含 `..` 或包含 NUL 字节的文件名。
/// 这些字符可让攻击者跳出 CONFIG_DIR，读取/写入/编辑任意 `.toml` 文件。
fn validate_config_file_name(file: &str) -> Result<()> {
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains('\0')
    {
        return Err(anyhow!("非法配置文件名 '{}'", file));
    }
    Ok(())
}

/// 查看配置项
fn cmd_config_get(key: &str) -> Result<()> {
    let (file, field) = parse_config_key(key);
    validate_config_file_name(&file)?;
    if field.is_empty() {
        return Err(anyhow!(
            "配置键格式应为 file.field，如 plugin.require_signature"
        ));
    }
    let path = format!("{}/{}.toml", CONFIG_DIR, file);
    let path_obj = Path::new(&path);
    if !path_obj.exists() {
        return Err(anyhow!("配置文件 {} 不存在", path));
    }
    let data = std::fs::read_to_string(path_obj)
        .with_context(|| format!("读取配置文件 {} 失败", path))?;
    let value: toml::Value = toml::from_str(&data)
        .with_context(|| format!("解析配置文件 {} 失败", path))?;

    // 按点分路径查找嵌套字段
    let mut current = &value;
    for part in field.split('.') {
        match current.get(part) {
            Some(v) => current = v,
            None => return Err(anyhow!("配置项 {} 不存在", key)),
        }
    }
    println!("{}", current);
    Ok(())
}

/// 设置配置项
fn cmd_config_set(key: &str, value: &str) -> Result<()> {
    let (file, field) = parse_config_key(key);
    validate_config_file_name(&file)?;
    if field.is_empty() {
        return Err(anyhow!(
            "配置键格式应为 file.field，如 plugin.require_signature"
        ));
    }
    let path = format!("{}/{}.toml", CONFIG_DIR, file);
    let path_obj = Path::new(&path);
    if !path_obj.exists() {
        return Err(anyhow!("配置文件 {} 不存在", path));
    }
    let data = std::fs::read_to_string(path_obj)
        .with_context(|| format!("读取配置文件 {} 失败", path))?;
    let mut root: toml::Value = toml::from_str(&data)
        .with_context(|| format!("解析配置文件 {} 失败", path))?;

    // 按点分路径定位到父表，设置叶子字段
    let parts: Vec<&str> = field.split('.').collect();
    let leaf = parts.last().unwrap();
    let ancestors = &parts[..parts.len() - 1];

    let mut current = &mut root;
    for part in ancestors {
        current = current
            .as_table_mut()
            .and_then(|t| t.get_mut(*part))
            .ok_or_else(|| anyhow!("配置路径 {} 不存在", key))?;
    }

    if let Some(table) = current.as_table_mut() {
        // 尝试将值解析为 bool/integer，否则作为字符串
        let parsed: toml::Value = if value == "true" {
            toml::Value::Boolean(true)
        } else if value == "false" {
            toml::Value::Boolean(false)
        } else if let Ok(n) = value.parse::<i64>() {
            toml::Value::Integer(n)
        } else {
            toml::Value::String(value.to_string())
        };
        table.insert(leaf.to_string(), parsed);
    } else {
        return Err(anyhow!("配置路径 {} 不是表，无法设置字段", key));
    }

    let serialized = toml::to_string_pretty(&root)
        .with_context(|| format!("序列化配置文件 {} 失败", path))?;
    std::fs::write(path_obj, serialized)
        .with_context(|| format!("写入配置文件 {} 失败", path))?;
    println!("已设置 {} = {}", key, value);
    Ok(())
}

/// 编辑配置文件（使用 $EDITOR）
fn cmd_config_edit(file: &str) -> Result<()> {
    validate_config_file_name(file)?;
    let path = format!("{}/{}.toml", CONFIG_DIR, file);
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("启动编辑器 {} 失败", editor))?;
    if !status.success() {
        return Err(anyhow!("编辑器 {} 退出码非零", editor));
    }
    Ok(())
}

/// 列出所有配置文件
fn cmd_config_list() -> Result<()> {
    let dir = Path::new(CONFIG_DIR);
    if !dir.exists() {
        println!("配置目录 {} 不存在", CONFIG_DIR);
        return Ok(());
    }
    let mut files: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "toml" {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            files.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    files.sort();
    if files.is_empty() {
        println!("配置目录 {} 下没有 .toml 文件", CONFIG_DIR);
    } else {
        println!("配置文件列表（{}）：", CONFIG_DIR);
        for f in &files {
            println!("  {}", f);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Service 子命令（v0.29.0 — Task 14）
// ---------------------------------------------------------------------------

/// 已知的 EnerOS 服务列表
const KNOWN_SERVICES: &[&str] = &[
    "eneros-init",
    "ha-daemon",
    "plugin-daemon",
    "eventbus-broker",
    "gateway",
];

/// Service 子命令分发
pub async fn cmd_service(action: crate::ServiceAction) -> Result<()> {
    match action {
        crate::ServiceAction::Start { name } => cmd_service_start(&name),
        crate::ServiceAction::Stop { name } => cmd_service_stop(&name),
        crate::ServiceAction::Restart { name } => cmd_service_restart(&name),
        crate::ServiceAction::Status { name } => cmd_service_status(&name),
        crate::ServiceAction::List => cmd_service_list(),
    }
}

/// 校验服务名是否在已知列表中
fn validate_service(name: &str) -> Result<()> {
    if !KNOWN_SERVICES.contains(&name) {
        return Err(anyhow!(
            "未知服务 '{}'，已知服务: {}",
            name,
            KNOWN_SERVICES.join(", ")
        ));
    }
    Ok(())
}

/// 启动服务（通过 systemctl）
fn cmd_service_start(name: &str) -> Result<()> {
    validate_service(name)?;
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["start", name])
            .status()
            .with_context(|| format!("调用 systemctl start {} 失败", name))?;
        if !status.success() {
            return Err(anyhow!("启动服务 {} 失败", name));
        }
        println!("服务 {} 已启动", name);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("（非 Linux 环境）服务管理需要 Linux 环境与 systemctl");
        println!("将在 Linux 环境下执行: systemctl start {}", name);
        Ok(())
    }
}

/// 停止服务
fn cmd_service_stop(name: &str) -> Result<()> {
    validate_service(name)?;
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["stop", name])
            .status()
            .with_context(|| format!("调用 systemctl stop {} 失败", name))?;
        if !status.success() {
            return Err(anyhow!("停止服务 {} 失败", name));
        }
        println!("服务 {} 已停止", name);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("（非 Linux 环境）服务管理需要 Linux 环境与 systemctl");
        println!("将在 Linux 环境下执行: systemctl stop {}", name);
        Ok(())
    }
}

/// 重启服务
fn cmd_service_restart(name: &str) -> Result<()> {
    validate_service(name)?;
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["restart", name])
            .status()
            .with_context(|| format!("调用 systemctl restart {} 失败", name))?;
        if !status.success() {
            return Err(anyhow!("重启服务 {} 失败", name));
        }
        println!("服务 {} 已重启", name);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("（非 Linux 环境）服务管理需要 Linux 环境与 systemctl");
        println!("将在 Linux 环境下执行: systemctl restart {}", name);
        Ok(())
    }
}

/// 查询服务状态
fn cmd_service_status(name: &str) -> Result<()> {
    validate_service(name)?;
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["status", name])
            .status()
            .with_context(|| format!("调用 systemctl status {} 失败", name))?;
        if !status.success() {
            println!("服务 {} 未运行或状态异常", name);
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("（非 Linux 环境）服务管理需要 Linux 环境与 systemctl");
        println!("将在 Linux 环境下执行: systemctl status {}", name);
        Ok(())
    }
}

/// 列出所有已知服务及其状态
fn cmd_service_list() -> Result<()> {
    println!("EnerOS 服务列表");
    println!("{}", "-".repeat(40));
    for name in KNOWN_SERVICES {
        #[cfg(target_os = "linux")]
        {
            let output = std::process::Command::new("systemctl")
                .args(["is-active", name])
                .output();
            let state = match output {
                Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Err(_) => "unknown".to_string(),
            };
            println!("  {:<20} {}", name, state);
        }
        #[cfg(not(target_os = "linux"))]
        {
            println!("  {:<20} (需要 Linux 环境查询状态)", name);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Doctor 子命令（v0.29.0 — Task 14）
// ---------------------------------------------------------------------------

/// 诊断检查结果
#[derive(Debug)]
pub struct CheckResult {
    /// 是否通过
    pub ok: bool,
    /// 结果描述
    pub message: String,
}

impl CheckResult {
    /// 构造通过的检查结果
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
        }
    }

    /// 构造失败的检查结果
    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
        }
    }
}

/// 打印单项检查结果
pub fn print_check(name: &str, result: &CheckResult, all_ok: &mut bool) {
    let status = if result.ok { "✓" } else { "✗" };
    println!("{} {}: {}", status, name, result.message);
    if !result.ok {
        *all_ok = false;
    }
}

/// 系统诊断
pub async fn cmd_doctor() -> Result<()> {
    println!("EnerOS 系统诊断");
    println!("{}", "=".repeat(60));

    let mut all_ok = true;

    // 1. 内核版本检查
    print_check("内核版本", &check_kernel_version(), &mut all_ok);

    // 2. 控制通道连通性
    print_check(
        "控制通道（TCP）",
        &check_control_channel(),
        &mut all_ok,
    );

    // 3. 状态文件完整性
    print_check("状态文件", &check_state_files(), &mut all_ok);

    // 4. 权限检查
    print_check("权限检查", &check_permissions(), &mut all_ok);

    // 5. 依赖服务状态
    print_check("依赖服务", &check_services(), &mut all_ok);

    println!("{}", "=".repeat(60));
    if all_ok {
        println!("✓ 所有检查通过");
    } else {
        println!("✗ 部分检查未通过，请查看上方详情");
    }
    Ok(())
}

/// 检查内核版本
fn check_kernel_version() -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
            Ok(v) => {
                let v = v.trim();
                if v.starts_with("5.") || v.starts_with("6.") {
                    CheckResult::ok(format!("Linux {}", v))
                } else {
                    CheckResult::fail(format!(
                        "Linux {}（建议 5.x 或更高版本）",
                        v
                    ))
                }
            }
            Err(e) => CheckResult::fail(format!("无法读取内核版本: {}", e)),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::fail("跳过（需要 Linux 环境）")
    }
}

/// 检查控制通道（TCP 127.0.0.1:9876）连通性
///
/// EnerOS 控制通道使用 TCP 而非 Unix socket，此处通过 TCP 连接探测
/// eneros-init 是否在监听。TCP 探测跨平台，无需条件编译。
fn check_control_channel() -> CheckResult {
    let addr = CONTROL_ADDR;
    match addr.parse() {
        Ok(socket_addr) => match std::net::TcpStream::connect_timeout(
            &socket_addr,
            Duration::from_secs(2),
        ) {
            Ok(_) => CheckResult::ok(format!("控制通道可达（{}）", addr)),
            Err(_) => {
                CheckResult::fail(format!("控制通道不可达（{}，eneros-init 可能未启动）", addr))
            }
        },
        Err(_) => CheckResult::fail(format!("控制通道地址 {} 解析失败", addr)),
    }
}

/// 检查状态文件完整性
fn check_state_files() -> CheckResult {
    let state = Path::new(STATE_FILE);
    if state.exists() {
        match std::fs::read_to_string(state) {
            Ok(data) => {
                if data.trim().is_empty() {
                    CheckResult::fail("状态文件为空".to_string())
                } else {
                    CheckResult::ok(format!("状态文件正常（{} 字节）", data.len()))
                }
            }
            Err(e) => CheckResult::fail(format!("读取状态文件失败: {}", e)),
        }
    } else {
        CheckResult::fail(format!("状态文件 {} 不存在", STATE_FILE))
    }
}

/// 检查权限（当前用户是否为 root 或有 sudo 权限）
fn check_permissions() -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("id")
            .arg("-u")
            .output();
        match output {
            Ok(o) => {
                let uid_str = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if uid_str == "0" {
                    CheckResult::ok("以 root 运行".to_string())
                } else {
                    CheckResult::fail(format!(
                        "当前 UID={}（建议以 root 运行）",
                        uid_str
                    ))
                }
            }
            Err(e) => CheckResult::fail(format!("无法获取 UID: {}", e)),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::fail("跳过（需要 Linux 环境）")
    }
}

/// 检查依赖服务状态
fn check_services() -> CheckResult {
    #[cfg(target_os = "linux")]
    {
        let mut running = 0;
        let mut total = 0;
        for name in KNOWN_SERVICES {
            total += 1;
            let output = std::process::Command::new("systemctl")
                .args(["is-active", name])
                .output();
            if let Ok(o) = output {
                let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if state == "active" {
                    running += 1;
                }
            }
        }
        if running == total {
            CheckResult::ok(format!("{}/{} 服务运行中", running, total))
        } else {
            CheckResult::fail(format!("{}/{} 服务运行中", running, total))
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CheckResult::fail("跳过（需要 Linux 环境）")
    }
}


// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    /// 测试 HA IPC 请求构造（无参数）
    #[test]
    fn test_ha_request_serialization() {
        let req = serde_json::json!({
            "command": "ha_status",
            "args": null,
        });
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"command\":\"ha_status\""));
        assert!(json.contains("\"args\":null"));
    }

    /// 测试 HA IPC 请求构造（带参数）
    #[test]
    fn test_ha_request_with_args_serialization() {
        let args = serde_json::json!({"scenario": "primary_down"});
        let req = serde_json::json!({
            "command": "failover_drill",
            "args": args,
        });
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"command\":\"failover_drill\""));
        assert!(json.contains("\"scenario\":\"primary_down\""));
    }

    /// 测试 HA IPC 响应解析（成功响应）
    #[test]
    fn test_ha_response_deserialization_ok() {
        let json = r#"{"status":"ok","data":{"node_id":"node-1","role":"Primary"},"message":""}"#;
        let resp: HaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.data["node_id"], "node-1");
        assert_eq!(resp.data["role"], "Primary");
        assert!(resp.message.is_empty());
    }

    /// 测试 HA IPC 响应解析（错误响应）
    #[test]
    fn test_ha_response_deserialization_error() {
        let json = r#"{"status":"error","data":null,"message":"failover in progress"}"#;
        let resp: HaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "error");
        assert!(resp.data.is_null());
        assert_eq!(resp.message, "failover in progress");
    }

    /// 测试 HA IPC 响应解析（缺少 message 字段时使用默认值）
    #[test]
    fn test_ha_response_deserialization_missing_message() {
        let json = r#"{"status":"ok","data":{"key":"value"}}"#;
        let resp: HaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.data["key"], "value");
        assert!(resp.message.is_empty());
    }

    /// 测试连接失败错误处理（无 ha-daemon 运行时应失败）
    #[tokio::test]
    async fn test_ha_connect_failure() {
        let result = ha_connect_to(HA_DAEMON_ADDR).await;
        // 测试环境中 ha-daemon 通常未运行；若恰好运行则跳过
        if result.is_ok() {
            eprintln!("跳过测试：ha-daemon 正在运行");
            return;
        }
        assert!(result.is_err());
    }

    /// 测试 ha_query 连接失败时返回友好错误
    #[tokio::test]
    async fn test_ha_query_connection_error_message() {
        // 先检查 ha-daemon 是否在运行
        if ha_connect_to(HA_DAEMON_ADDR).await.is_ok() {
            eprintln!("跳过测试：ha-daemon 正在运行");
            return;
        }

        // ha-daemon 未运行，ha_query 应返回友好错误
        let result = ha_query("ha_status", None).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("ha-daemon 未运行"),
            "错误消息应包含 'ha-daemon 未运行'，实际: {}",
            err_msg
        );
    }

    /// 测试命令分发逻辑（通过 mock 服务器验证 IPC 请求）
    ///
    /// 使用随机端口避免与其他 HA 测试（test_ha_connect_failure 等）并行冲突。
    #[tokio::test]
    async fn test_ha_dispatch_with_mock_server() {
        use tokio::net::TcpListener;

        // 绑定随机端口（127.0.0.1:0），避免与 HA_DAEMON_ADDR 冲突
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        // 启动 mock 服务器
        let server = tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                // 使用 split 分离读写半部，避免 BufReader 借用 socket 后无法 write
                let (mut reader, mut writer) = tokio::io::split(socket);
                let mut buf_reader = BufReader::new(&mut reader);
                let mut line = String::new();
                match buf_reader.read_line(&mut line).await {
                    Ok(n) if n > 0 => {
                        // 验证请求命令
                        let req: serde_json::Value =
                            serde_json::from_str(&line).unwrap_or_default();
                        assert_eq!(req["command"], "ha_status");

                        // 发送响应
                        let resp = serde_json::json!({
                            "status": "ok",
                            "data": {"node_id": "test-node", "role": "Primary"},
                            "message": ""
                        });
                        let resp_json = serde_json::to_string(&resp).unwrap();
                        let _ = writer.write_all(resp_json.as_bytes()).await;
                        let _ = writer.write_all(b"\n").await;
                    }
                    _ => {
                        eprintln!("服务器未收到数据或读取失败");
                    }
                }
            }
        });

        // 调用 ha_request_to（验证完整的 IPC 流程，使用随机端口）
        let result = ha_request_to(&addr, "ha_status", None).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data["node_id"], "test-node");
        assert_eq!(data["role"], "Primary");

        // 等待服务器任务完成
        let _ = server.await;
    }

    // -----------------------------------------------------------------------
    // Plugin 子命令测试（v0.27.0）
    // -----------------------------------------------------------------------

    /// 测试 PluginCommands 枚举解析（list 子命令）
    #[test]
    fn test_plugin_list_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "list"]);
        assert!(matches!(cli.command, crate::PluginCommands::List));
    }

    /// 测试 PluginCommands 枚举解析（load 子命令）
    #[test]
    fn test_plugin_load_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "load", "/tmp/foo.so"]);
        assert!(matches!(
            cli.command,
            crate::PluginCommands::Load { .. }
        ));
    }

    /// 测试 PluginCommands 枚举解析（load --skip-signature）
    #[test]
    fn test_plugin_load_skip_signature_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "load", "/tmp/foo.so", "--skip-signature"]);
        match cli.command {
            crate::PluginCommands::Load {
                path,
                skip_signature,
            } => {
                assert_eq!(path, "/tmp/foo.so");
                assert!(skip_signature);
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 PluginCommands 枚举解析（genkeys --output）
    #[test]
    fn test_plugin_genkeys_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "gen-keys", "--output", "/tmp/keys"]);
        match cli.command {
            crate::PluginCommands::GenKeys { output } => assert_eq!(output, "/tmp/keys"),
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 PluginCommands 枚举解析（verify --sig）
    #[test]
    fn test_plugin_verify_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "verify", "/tmp/foo.so", "--sig", "/tmp/foo.sig"]);
        match cli.command {
            crate::PluginCommands::Verify { path, sig } => {
                assert_eq!(path, "/tmp/foo.so");
                assert_eq!(sig.as_deref(), Some("/tmp/foo.sig"));
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 PluginCommands 枚举解析（sign 子命令）
    #[test]
    fn test_plugin_sign_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::PluginCommands,
        }

        let cli = TestCli::parse_from(["test", "sign", "/tmp/foo.so", "/tmp/key"]);
        match cli.command {
            crate::PluginCommands::Sign { plugin, key } => {
                assert_eq!(plugin, "/tmp/foo.so");
                assert_eq!(key, "/tmp/key");
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 cmd_plugin_list 在无 daemon 运行时的行为（应返回 Err 并输出友好提示）
    ///
    /// v0.28.0 Task 15: cmd_plugin_list 改为通过 IPC 调用 plugin-daemon，
    /// 无 daemon 运行时应返回友好错误提示。
    #[tokio::test]
    async fn test_plugin_list_nonexistent_dir() {
        // cmd_plugin_list 在无 daemon 时应返回 Err（友好错误提示）
        let result = cmd_plugin_list().await;
        assert!(
            result.is_err(),
            "cmd_plugin_list 应在无 daemon 时返回 Err"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("plugin-daemon 未运行"),
            "错误信息应包含 'plugin-daemon 未运行'，实际: {}",
            err_msg
        );
    }

    /// 测试 format_plugin_type 辅助函数
    #[test]
    fn test_format_plugin_type() {
        assert_eq!(format_plugin_type(&PluginType::Protocol), "Protocol");
        assert_eq!(format_plugin_type(&PluginType::Agent), "Agent");
        assert_eq!(format_plugin_type(&PluginType::Analysis), "Analysis");
    }

    /// 测试 collect_plugin_manifests 在不存在的目录上返回空列表
    #[test]
    fn test_collect_plugin_manifests_nonexistent() {
        let manifests = collect_plugin_manifests(Path::new("/nonexistent/path/12345"));
        assert!(
            manifests.is_empty(),
            "不存在的目录应返回空 manifest 列表"
        );
    }

    /// 测试 collect_plugin_manifests 在临时目录上的行为
    #[test]
    fn test_collect_plugin_manifests_tempdir() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("enerosctl-test-{}-{}", std::process::id(), id));
        // 清理可能残留的旧目录
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 创建 <dir>/plugin-a/manifest.toml
        let sub = dir.join("plugin-a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("manifest.toml"), "# placeholder").unwrap();

        // 创建 <dir>/plugin-b.toml
        std::fs::write(dir.join("plugin-b.toml"), "# placeholder").unwrap();

        let manifests = collect_plugin_manifests(&dir);
        assert_eq!(
            manifests.len(),
            2,
            "应找到 2 个 manifest 文件，实际: {:?}",
            manifests
        );

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }
    // -----------------------------------------------------------------------
    // Config / Service / Doctor 子命令测试（v0.29.0 — Task 14）
    // -----------------------------------------------------------------------

    /// 测试 ConfigAction::Get 解析
    #[test]
    fn test_config_action_get_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ConfigAction,
        }

        let cli = TestCli::parse_from(["test", "get", "plugin.require_signature"]);
        match cli.command {
            crate::ConfigAction::Get { key } => {
                assert_eq!(key, "plugin.require_signature");
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 ConfigAction::Set 解析
    #[test]
    fn test_config_action_set_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ConfigAction,
        }

        let cli = TestCli::parse_from(["test", "set", "plugin.require_signature", "true"]);
        match cli.command {
            crate::ConfigAction::Set { key, value } => {
                assert_eq!(key, "plugin.require_signature");
                assert_eq!(value, "true");
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 ConfigAction::List 解析
    #[test]
    fn test_config_action_list_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ConfigAction,
        }

        let cli = TestCli::parse_from(["test", "list"]);
        assert!(matches!(cli.command, crate::ConfigAction::List));
    }

    /// 测试 ServiceAction::Start 解析
    #[test]
    fn test_service_action_start_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ServiceAction,
        }

        let cli = TestCli::parse_from(["test", "start", "eneros-init"]);
        match cli.command {
            crate::ServiceAction::Start { name } => {
                assert_eq!(name, "eneros-init");
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 ServiceAction::Status 解析
    #[test]
    fn test_service_action_status_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ServiceAction,
        }

        let cli = TestCli::parse_from(["test", "status", "ha-daemon"]);
        match cli.command {
            crate::ServiceAction::Status { name } => {
                assert_eq!(name, "ha-daemon");
            }
            _ => panic!("wrong variant"),
        }
    }

    /// 测试 ServiceAction::List 解析
    #[test]
    fn test_service_action_list_parse() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: crate::ServiceAction,
        }

        let cli = TestCli::parse_from(["test", "list"]);
        assert!(matches!(cli.command, crate::ServiceAction::List));
    }

    /// 测试 CheckResult ok 变体
    #[test]
    fn test_doctor_check_result_ok() {
        let result = CheckResult::ok("检查通过");
        assert!(result.ok);
        assert_eq!(result.message, "检查通过");
    }

    /// 测试 CheckResult fail 变体
    #[test]
    fn test_doctor_check_result_fail() {
        let result = CheckResult::fail("检查失败");
        assert!(!result.ok);
        assert_eq!(result.message, "检查失败");
    }

    /// 测试 print_check 输出格式
    #[test]
    fn test_doctor_print_check() {
        // 验证 print_check 在 ok 和 fail 两种情况下正确更新 all_ok 标志
        let mut all_ok = true;

        // ok 情况：不应改变 all_ok
        print_check("测试通过项", &CheckResult::ok("正常"), &mut all_ok);
        assert!(all_ok, "ok 检查不应将 all_ok 设为 false");

        // fail 情况：应将 all_ok 设为 false
        print_check("测试失败项", &CheckResult::fail("异常"), &mut all_ok);
        assert!(!all_ok, "fail 检查应将 all_ok 设为 false");
    }

    /// 测试 validate_config_file_name 拒绝路径遍历攻击
    ///
    /// 验证各种恶意输入都被拒绝：`..`、路径分隔符、空字符串、NUL 字节
    #[test]
    fn test_validate_config_file_name_rejects_traversal() {
        // 路径遍历攻击
        assert!(
            validate_config_file_name("../../etc/passwd").is_err(),
            "应拒绝路径遍历攻击 ../../etc/passwd"
        );
        // 单独的 ..
        assert!(
            validate_config_file_name("..").is_err(),
            "应拒绝 .."
        );
        // 包含正斜杠
        assert!(
            validate_config_file_name("a/b").is_err(),
            "应拒绝包含 / 的文件名 a/b"
        );
        // 包含反斜杠
        assert!(
            validate_config_file_name("a\\b").is_err(),
            "应拒绝包含 \\ 的文件名 a\\b"
        );
        // 空字符串
        assert!(
            validate_config_file_name("").is_err(),
            "应拒绝空字符串"
        );
        // 包含 NUL 字节
        assert!(
            validate_config_file_name("a\0b").is_err(),
            "应拒绝包含 NUL 字节的文件名"
        );
    }

    /// 测试 validate_config_file_name 接受合法文件名
    ///
    /// 验证正常的配置文件名（不含扩展名）被接受
    #[test]
    fn test_validate_config_file_name_accepts_valid() {
        assert!(
            validate_config_file_name("syslog").is_ok(),
            "应接受 syslog"
        );
        assert!(
            validate_config_file_name("agent").is_ok(),
            "应接受 agent"
        );
        assert!(
            validate_config_file_name("plugin").is_ok(),
            "应接受 plugin"
        );
        // 包含点号但非 .. 的合法文件名（如版本号风格）
        assert!(
            validate_config_file_name("ha_config").is_ok(),
            "应接受 ha_config"
        );
    }

    /// 测试 check_control_channel 返回 CheckResult（不 panic）
    ///
    /// 在测试环境中（无 eneros-init 监听 127.0.0.1:9876），函数应返回
    /// fail 结果而非 panic。本测试只验证返回类型正确，不假设具体结果。
    #[test]
    fn test_check_control_channel_returns_result() {
        // 调用函数应不 panic
        let result = check_control_channel();
        // 验证返回的是 CheckResult（通过访问字段确认类型）
        let _ = &result.ok;
        let _ = &result.message;
        // 在测试环境中（无 daemon 监听），预期为 fail；但若 CI 环境恰好
        // 有进程占用该端口，ok 也算通过——关键是函数不 panic。
        // 这里仅断言 message 非空，确保有可读的诊断信息。
        assert!(
            !result.message.is_empty(),
            "check_control_channel 应返回非空诊断信息"
        );
    }

    /// 测试配置键解析（file.field 格式）
    #[test]
    fn test_config_key_parse() {
        // 标准格式：file.field
        let (file, field) = parse_config_key("plugin.require_signature");
        assert_eq!(file, "plugin");
        assert_eq!(field, "require_signature");

        // 嵌套格式：file.section.field
        let (file, field) = parse_config_key("syslog.rotation.max_size");
        assert_eq!(file, "syslog");
        assert_eq!(field, "rotation.max_size");

        // 无点号格式：整个键作为 file，field 为空
        let (file, field) = parse_config_key("plugin");
        assert_eq!(file, "plugin");
        assert_eq!(field, "");
    }

    // -----------------------------------------------------------------------
    // Plugin IPC / Simulator 子命令测试（v0.28.0 — Task 15）
    // -----------------------------------------------------------------------

    /// 测试 1: get_daemon_client 在无 daemon 运行时返回错误
    ///
    /// 在测试环境中（无 plugin-daemon 运行），get_daemon_client 应返回 Err，
    /// 且错误信息包含友好提示 "plugin-daemon 未运行"。
    #[test]
    fn test_get_daemon_client_unreachable() {
        let result = get_daemon_client();
        assert!(
            result.is_err(),
            "无 daemon 运行时 get_daemon_client 应返回 Err"
        );
        let err_msg = match result {
            Err(e) => e,
            Ok(_) => panic!("应返回 Err"),
        };
        assert!(
            err_msg.contains("plugin-daemon 未运行"),
            "错误信息应包含 'plugin-daemon 未运行'，实际: {}",
            err_msg
        );
    }

    /// 测试 2: handle_plugin_list_response 处理 mock 的 IPC 响应
    ///
    /// 构造一个成功的 DaemonResponse（包含插件数组），验证 handle_plugin_list_response 正确解析并返回 Ok。
    #[test]
    fn test_plugin_list_via_ipc_mock() {
        let resp = DaemonResponse {
            ok: true,
            data: Some(serde_json::json!([
                {
                    "name": "iec103-driver",
                    "version": "1.0.0",
                    "type": "protocol",
                    "state": "running",
                    "enabled": true
                },
                {
                    "name": "analysis-module",
                    "version": "0.5.0",
                    "type": "analysis",
                    "state": "stopped",
                    "enabled": false
                }
            ])),
            error: None,
        };
        let result = handle_plugin_list_response(&resp);
        assert!(result.is_ok(), "处理成功的 list 响应应返回 Ok");
    }

    /// 测试 3: handle_plugin_info_response 处理 mock 的 IPC 响应
    ///
    /// 构造一个成功的 DaemonResponse（包含插件详情），验证 handle_plugin_info_response 正确解析并返回 Ok。
    #[test]
    fn test_plugin_info_via_ipc_mock() {
        let resp = DaemonResponse {
            ok: true,
            data: Some(serde_json::json!({
                "name": "iec103-driver",
                "version": "1.0.0",
                "api_version": "0.27.0",
                "type": "protocol",
                "description": "IEC 60870-5-103 协议适配器",
                "author": "EnerOS Team",
                "state": "running",
                "enabled": true
            })),
            error: None,
        };
        let result = handle_plugin_info_response(&resp, "iec103-driver");
        assert!(result.is_ok(), "处理成功的 info 响应应返回 Ok");
    }

    /// 测试 4: cmd_simulator_validate 解析有效的 TOML 场景脚本
    ///
    /// 创建一个有效的 TOML 场景文件，验证 cmd_simulator_validate 返回 Ok。
    #[tokio::test]
    async fn test_simulator_validate_valid_scenario() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "enerosctl-sim-valid-{}-{}.toml",
            std::process::id(),
            id
        ));

        let toml_content = r#"
name = "test_validate"
description = "验证测试场景"
duration = 10.0
time_step = 0.1

[[timeline]]
time = 0.0
action = { type = "observe" }

[[timeline]]
time = 5.0
action = { type = "inject_fault" }
params = { bus_id = "bus_1" }

[[timeline]]
time = 10.0
action = { type = "observe" }
"#;
        std::fs::write(&path, toml_content).expect("写入测试场景文件失败");

        let result = cmd_simulator_validate(&path).await;
        assert!(
            result.is_ok(),
            "验证有效场景应返回 Ok，实际: {:?}",
            result.err()
        );

        let _ = std::fs::remove_file(&path);
    }

    /// 测试 5: cmd_simulator_validate 解析无效的 TOML 场景脚本
    ///
    /// 创建一个无效的 TOML 场景文件（duration <= 0），验证 cmd_simulator_validate 返回 Err。
    #[tokio::test]
    async fn test_simulator_validate_invalid_scenario() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "enerosctl-sim-invalid-{}-{}.toml",
            std::process::id(),
            id
        ));

        // duration 为负数，校验应失败
        let toml_content = r#"
name = "invalid_scenario"
description = "无效场景"
duration = -1.0
time_step = 0.1

[[timeline]]
time = 0.0
action = { type = "observe" }
"#;
        std::fs::write(&path, toml_content).expect("写入测试场景文件失败");

        let result = cmd_simulator_validate(&path).await;
        assert!(
            result.is_err(),
            "验证无效场景（duration <= 0）应返回 Err"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// 测试 6: cmd_simulator_list_scenarios 列出 5 个内置故障场景
    ///
    /// 验证 FaultScenarioLibrary 包含 5 个预置场景：N-1/N-2/级联/保护拒动/保护误动。
    #[tokio::test]
    async fn test_simulator_list_scenarios() {
        let result = cmd_simulator_list_scenarios().await;
        assert!(
            result.is_ok(),
            "列出内置场景应返回 Ok，实际: {:?}",
            result.err()
        );

        // 直接验证 FaultScenarioLibrary 包含 5 个场景
        let library = FaultScenarioLibrary::new();
        let scenarios = library.list();
        assert_eq!(
            scenarios.len(),
            5,
            "应包含 5 个内置故障场景，实际: {}",
            scenarios.len()
        );

        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"n1_bus_fault"), "应包含 N-1 场景");
        assert!(names.contains(&"n2_double_line"), "应包含 N-2 场景");
        assert!(names.contains(&"cascading_failure"), "应包含级联故障场景");
        assert!(
            names.contains(&"protection_failure"),
            "应包含保护拒动场景"
        );
        assert!(
            names.contains(&"protection_maloperation"),
            "应包含保护误动场景"
        );
    }

    /// 测试 7: cmd_simulator_run 运行简单场景，验证 RunResult
    ///
    /// 创建一个简单的场景文件，运行并验证 RunResult 中的 events_executed 和 duration。
    #[tokio::test]
    async fn test_simulator_run_dry() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "enerosctl-sim-run-{}-{}.toml",
            std::process::id(),
            id
        ));

        let toml_content = r#"
name = "dry_run_test"
description = "干运行测试场景"
duration = 5.0
time_step = 0.1

[[timeline]]
time = 0.0
action = { type = "observe" }

[[timeline]]
time = 2.5
action = { type = "inject_fault" }
params = { bus_id = "bus_1" }

[[timeline]]
time = 5.0
action = { type = "observe" }
"#;
        std::fs::write(&path, toml_content).expect("写入测试场景文件失败");

        let result = cmd_simulator_run(&path).await;
        assert!(
            result.is_ok(),
            "运行简单场景应返回 Ok，实际: {:?}",
            result.err()
        );

        // 验证 ScenarioRunner 直接运行的 RunResult
        let scenario = Scenario::load_from_file(&path.to_string_lossy())
            .expect("加载场景失败");
        let runner = ScenarioRunner::new(scenario);
        let run_result = runner
            .run(|_t, _e| {})
            .expect("运行场景失败");
        assert_eq!(
            run_result.events_executed, 3,
            "应执行 3 个事件"
        );
        assert!(
            (run_result.duration - 5.0).abs() < 1e-9,
            "场景时长应为 5.0s"
        );
        assert_eq!(
            run_result.observations.len(),
            2,
            "应有 2 个观察记录点"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// 测试 8: SimulatorAction 枚举分发（Commands 枚举解析）
    ///
    /// 验证 SimulatorAction 的 Run/Validate/List 变体能被 clap 正确解析，
    /// 且 cmd_simulator 能正确分发 List 动作。
    #[tokio::test]
    async fn test_simulator_command_dispatch() {
        use clap::Parser;
        use std::path::PathBuf;

        // 验证 SimulatorAction::Run 解析
        {
            #[derive(Parser, Debug)]
            struct TestCli {
                #[command(subcommand)]
                command: crate::SimulatorAction,
            }
            let cli = TestCli::parse_from(["test", "run", "/tmp/scenario.toml"]);
            match cli.command {
                crate::SimulatorAction::Run { path } => {
                    assert_eq!(path, PathBuf::from("/tmp/scenario.toml"));
                }
                _ => panic!("应解析为 Run 变体"),
            }
        }

        // 验证 SimulatorAction::Validate 解析
        {
            #[derive(Parser, Debug)]
            struct TestCli {
                #[command(subcommand)]
                command: crate::SimulatorAction,
            }
            let cli = TestCli::parse_from(["test", "validate", "/tmp/scenario.toml"]);
            match cli.command {
                crate::SimulatorAction::Validate { path } => {
                    assert_eq!(path, PathBuf::from("/tmp/scenario.toml"));
                }
                _ => panic!("应解析为 Validate 变体"),
            }
        }

        // 验证 SimulatorAction::List 解析
        {
            #[derive(Parser, Debug)]
            struct TestCli {
                #[command(subcommand)]
                command: crate::SimulatorAction,
            }
            let cli = TestCli::parse_from(["test", "list"]);
            assert!(matches!(cli.command, crate::SimulatorAction::List));
        }

        // 验证 cmd_simulator 分发 List 动作成功执行
        let result = cmd_simulator(crate::SimulatorAction::List).await;
        assert!(
            result.is_ok(),
            "cmd_simulator 分发 List 应返回 Ok，实际: {:?}",
            result.err()
        );
    }

}
