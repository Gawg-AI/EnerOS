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
    reader.read_line(&mut line).await?;

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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

    writeln!(writer, "\n--- 导出完成: {} 条日志 ---", exported)?;
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
