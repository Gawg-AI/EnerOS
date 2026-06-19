//! 网络配置服务（Network Configuration Service）
//!
//! 提供静态 IP、VLAN（802.1Q）、网桥、bonding、DNS 配置能力。
//! 所有 Linux 特性通过 `ip` 命令实现，非 Linux 平台返回 `UnsupportedPlatform`。

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// 网络配置错误
#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("config parse error: {0}")]
    ParseError(String),
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

// ---------------------------------------------------------------------------
// 配置结构体
// ---------------------------------------------------------------------------

/// 网络配置根结构
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub interfaces: Vec<InterfaceConfig>,
    #[serde(default)]
    pub bonds: Vec<BondConfig>,
    #[serde(default)]
    pub bridges: Vec<BridgeConfig>,
    #[serde(default)]
    pub vlans: Vec<VlanConfig>,
    #[serde(default)]
    pub dns: DnsConfig,
}

/// 接口配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub name: String,
    #[serde(default)]
    pub ipv4: Option<IpConfig>,
    #[serde(default)]
    pub ipv6: Option<IpConfig>,
    #[serde(default = "default_true")]
    pub up: bool,
    #[serde(default)]
    pub mtu: Option<u32>,
}

fn default_true() -> bool {
    true
}

/// IP 配置（静态或 DHCP）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum IpConfig {
    #[serde(rename = "static")]
    Static {
        address: String,
        gateway: Option<String>,
    },
    #[serde(rename = "dhcp")]
    Dhcp,
}

/// Bonding 配置（链路聚合）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BondConfig {
    pub name: String,
    pub mode: BondMode,
    pub interfaces: Vec<String>,
    #[serde(default = "default_mii_monitor_ms")]
    pub miimon_ms: u32,
    #[serde(default)]
    pub primary: Option<String>,
}

fn default_mii_monitor_ms() -> u32 {
    100
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BondMode {
    #[default]
    ActiveBackup,
    Lacp,
    BalanceTlb,
}

impl BondMode {
    fn linux_mode(&self) -> &'static str {
        match self {
            BondMode::ActiveBackup => "active-backup",
            BondMode::Lacp => "802.3ad",
            BondMode::BalanceTlb => "balance-tlb",
        }
    }
}

/// 网桥配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub name: String,
    pub ports: Vec<String>,
    #[serde(default)]
    pub ipv4: Option<IpConfig>,
}

/// VLAN 配置（802.1Q）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanConfig {
    pub name: String,
    pub parent: String,
    pub id: u16,
    #[serde(default)]
    pub ipv4: Option<IpConfig>,
}

/// DNS 配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnsConfig {
    #[serde(default)]
    pub nameservers: Vec<String>,
    #[serde(default)]
    pub search: Vec<String>,
}

// ---------------------------------------------------------------------------
// NetworkConfig 实现
// ---------------------------------------------------------------------------

impl NetworkConfig {
    /// 从 TOML 文件加载配置
    pub fn load(path: &Path) -> Result<Self, NetworkError> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(|e| NetworkError::ParseError(e.to_string()))
    }

    /// 热重载（SIGHUP 触发时调用）
    pub fn reload(path: &Path) -> Result<Self, NetworkError> {
        Self::load(path)
    }

    /// 应用配置到系统
    #[cfg(target_os = "linux")]
    pub fn apply(&self) -> Result<(), NetworkError> {
        // 应用顺序：bonds → VLANs → bridges → 接口 IP → DNS
        for bond in &self.bonds {
            apply_bond(bond)?;
        }
        for vlan in &self.vlans {
            apply_vlan(vlan)?;
        }
        for bridge in &self.bridges {
            apply_bridge(bridge)?;
        }
        for iface in &self.interfaces {
            apply_interface(iface)?;
        }
        apply_dns(&self.dns)?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply(&self) -> Result<(), NetworkError> {
        Err(NetworkError::UnsupportedPlatform)
    }
}

// ---------------------------------------------------------------------------
// 网络接口状态查询
// ---------------------------------------------------------------------------

/// 网络接口状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    pub name: String,
    pub is_up: bool,
    pub mac: Option<String>,
    pub mtu: u32,
    pub ipv4_addresses: Vec<String>,
    pub ipv6_addresses: Vec<String>,
    pub interface_type: InterfaceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceType {
    Ethernet,
    Vlan,
    Bridge,
    Bond,
    Loopback,
    Other,
}

impl NetworkInterface {
    /// 枚举所有网络接口
    #[cfg(target_os = "linux")]
    pub fn list() -> Result<Vec<NetworkInterface>, NetworkError> {
        let entries = std::fs::read_dir("/sys/class/net")?;
        let mut interfaces = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Ok(iface) = Self::get(&name) {
                interfaces.push(iface);
            }
        }
        Ok(interfaces)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list() -> Result<Vec<NetworkInterface>, NetworkError> {
        Err(NetworkError::UnsupportedPlatform)
    }

    /// 查询单个接口状态
    #[cfg(target_os = "linux")]
    pub fn get(name: &str) -> Result<NetworkInterface, NetworkError> {
        let base = format!("/sys/class/net/{}", name);
        let operstate = std::fs::read_to_string(format!("{}/operstate", base))
            .unwrap_or_default()
            .trim()
            .to_string();
        let is_up = operstate == "up" || name == "lo";
        let mac = std::fs::read_to_string(format!("{}/address", base))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mtu = std::fs::read_to_string(format!("{}/mtu", base))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1500);

        let interface_type = if name == "lo" {
            InterfaceType::Loopback
        } else if name.starts_with("bond") {
            InterfaceType::Bond
        } else if name.contains('.') {
            InterfaceType::Vlan
        } else if std::path::Path::new(&format!("{}/bridge", base)).exists() {
            InterfaceType::Bridge
        } else {
            InterfaceType::Ethernet
        };

        // 获取 IP 地址（通过 ip 命令）
        let (ipv4_addresses, ipv6_addresses) = get_ip_addresses(name);

        Ok(NetworkInterface {
            name: name.to_string(),
            is_up,
            mac,
            mtu,
            ipv4_addresses,
            ipv6_addresses,
            interface_type,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn get(_name: &str) -> Result<NetworkInterface, NetworkError> {
        Err(NetworkError::UnsupportedPlatform)
    }
}

// ---------------------------------------------------------------------------
// Bond 状态查询
// ---------------------------------------------------------------------------

/// Bond 接口状态
#[derive(Debug, Clone)]
pub struct BondStatus {
    pub name: String,
    pub mode: String,
    pub active_slave: Option<String>,
    pub slaves: Vec<String>,
    pub link_up: bool,
}

impl BondStatus {
    #[cfg(target_os = "linux")]
    pub fn list() -> Result<Vec<BondStatus>, NetworkError> {
        let bonding_dir = Path::new("/proc/net/bonding");
        if !bonding_dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(bonding_dir)?;
        let mut bonds = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                bonds.push(parse_bond_status(&name, &content));
            }
        }
        Ok(bonds)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list() -> Result<Vec<BondStatus>, NetworkError> {
        Err(NetworkError::UnsupportedPlatform)
    }
}

// ---------------------------------------------------------------------------
// Linux 实现辅助函数
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn run_ip(args: &[&str]) -> Result<(), NetworkError> {
    let output = std::process::Command::new("ip").args(args).output()?;
    if !output.status.success() {
        return Err(NetworkError::CommandFailed(format!(
            "ip {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn link_exists(name: &str) -> bool {
    Path::new(&format!("/sys/class/net/{}", name)).exists()
}

#[cfg(target_os = "linux")]
fn apply_bond(bond: &BondConfig) -> Result<(), NetworkError> {
    if !link_exists(&bond.name) {
        run_ip(&["link", "add", &bond.name, "type", "bond"])?;
        // 配置 bond 模式
        let mode_path = format!("/sys/class/net/{}/bonding/mode", bond.name);
        let _ = std::fs::write(&mode_path, bond.mode.linux_mode());
        // 配置 miimon
        let miimon_path = format!("/sys/class/net/{}/bonding/miimon", bond.name);
        let _ = std::fs::write(&miimon_path, bond.miimon_ms.to_string());
    }
    // 添加子接口
    for iface in &bond.interfaces {
        run_ip(&["link", "set", iface, "down"])?;
        run_ip(&["link", "set", iface, "master", &bond.name])?;
    }
    run_ip(&["link", "set", &bond.name, "up"])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_vlan(vlan: &VlanConfig) -> Result<(), NetworkError> {
    if !link_exists(&vlan.name) {
        run_ip(&[
            "link", "add", "link", &vlan.parent, "name", &vlan.name, "type",
            "vlan", "id", &vlan.id.to_string(),
        ])?;
    }
    run_ip(&["link", "set", &vlan.name, "up"])?;
    if let Some(IpConfig::Static { address, .. }) = &vlan.ipv4 {
        run_ip(&["addr", "add", address, "dev", &vlan.name])?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_bridge(bridge: &BridgeConfig) -> Result<(), NetworkError> {
    if !link_exists(&bridge.name) {
        run_ip(&["link", "add", &bridge.name, "type", "bridge"])?;
    }
    for port in &bridge.ports {
        run_ip(&["link", "set", port, "master", &bridge.name])?;
    }
    run_ip(&["link", "set", &bridge.name, "up"])?;
    if let Some(IpConfig::Static { address, .. }) = &bridge.ipv4 {
        run_ip(&["addr", "add", address, "dev", &bridge.name])?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_interface(iface: &InterfaceConfig) -> Result<(), NetworkError> {
    if let Some(mtu) = iface.mtu {
        run_ip(&["link", "set", "dev", &iface.name, "mtu", &mtu.to_string()])?;
    }
    if let Some(IpConfig::Static { address, gateway }) = &iface.ipv4 {
        run_ip(&["addr", "add", address, "dev", &iface.name])?;
        if let Some(gw) = gateway {
            run_ip(&["route", "add", "default", "via", gw])?;
        }
    }
    if iface.up {
        run_ip(&["link", "set", &iface.name, "up"])?;
    } else {
        run_ip(&["link", "set", &iface.name, "down"])?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_dns(dns: &DnsConfig) -> Result<(), NetworkError> {
    if dns.nameservers.is_empty() && dns.search.is_empty() {
        return Ok(());
    }
    let mut content = String::new();
    content.push_str("# Generated by EnerOS netcfg\n");
    for ns in &dns.nameservers {
        content.push_str(&format!("nameserver {}\n", ns));
    }
    if !dns.search.is_empty() {
        content.push_str(&format!("search {}\n", dns.search.join(" ")));
    }
    std::fs::write("/etc/resolv.conf", content)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn get_ip_addresses(name: &str) -> (Vec<String>, Vec<String>) {
    let output = std::process::Command::new("ip")
        .args(["-j", "addr", "show", "dev", name])
        .output();
    let Ok(output) = output else {
        return (Vec::new(), Vec::new());
    };
    if !output.status.success() {
        return (Vec::new(), Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // 简单解析 JSON 数组
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    if let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
        for entry in entries {
            if let Some(addr_info) = entry.get("addr_info").and_then(|v| v.as_array()) {
                for addr in addr_info {
                    let family = addr.get("family").and_then(|v| v.as_str()).unwrap_or("");
                    let local = addr.get("local").and_then(|v| v.as_str()).unwrap_or("");
                    let prefix = addr.get("prefixlen").and_then(|v| v.as_u64()).unwrap_or(0);
                    let addr_str = format!("{}/{}", local, prefix);
                    if family == "inet" {
                        ipv4.push(addr_str);
                    } else if family == "inet6" {
                        ipv6.push(addr_str);
                    }
                }
            }
        }
    }
    (ipv4, ipv6)
}

#[cfg(target_os = "linux")]
fn parse_bond_status(name: &str, content: &str) -> BondStatus {
    let mut mode = String::new();
    let mut active_slave = None;
    let mut slaves = Vec::new();
    let mut link_up = false;
    let mut in_slave_section = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("Bonding Mode:") {
            mode = line.trim_start_matches("Bonding Mode:").trim().to_string();
        } else if line.starts_with("Currently Active Slave:") {
            let slave = line.trim_start_matches("Currently Active Slave:").trim().to_string();
            if !slave.is_empty() {
                active_slave = Some(slave);
            }
        } else if line.starts_with("MII Status:") {
            let status = line.trim_start_matches("MII Status:").trim();
            if !in_slave_section {
                link_up = status == "up";
            }
        } else if line.starts_with("Slave Interface:") {
            in_slave_section = true;
            let slave = line.trim_start_matches("Slave Interface:").trim().to_string();
            if !slave.is_empty() {
                slaves.push(slave);
            }
        }
    }

    BondStatus {
        name: name.to_string(),
        mode,
        active_slave,
        slaves,
        link_up,
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_config_default() {
        let config = NetworkConfig::default();
        assert!(config.interfaces.is_empty());
        assert!(config.bonds.is_empty());
        assert!(config.bridges.is_empty());
        assert!(config.vlans.is_empty());
        assert!(config.dns.nameservers.is_empty());
    }

    #[test]
    fn test_network_config_parse() {
        let toml_str = r#"
[[interfaces]]
name = "eth0"
ipv4 = { mode = "static", address = "192.168.1.100/24", gateway = "192.168.1.1" }
mtu = 1500

[[bonds]]
name = "bond0"
mode = "active_backup"
interfaces = ["eth1", "eth2"]
miimon_ms = 100

[[vlans]]
name = "bond0.10"
parent = "bond0"
id = 10

[dns]
nameservers = ["10.0.0.1", "10.0.0.2"]
search = ["eneros.local"]
"#;
        let config: NetworkConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.interfaces.len(), 1);
        assert_eq!(config.interfaces[0].name, "eth0");
        assert_eq!(config.bonds.len(), 1);
        assert_eq!(config.bonds[0].name, "bond0");
        assert_eq!(config.bonds[0].mode, BondMode::ActiveBackup);
        assert_eq!(config.vlans.len(), 1);
        assert_eq!(config.vlans[0].id, 10);
        assert_eq!(config.dns.nameservers.len(), 2);
    }

    #[test]
    fn test_ip_config_serialization() {
        let config = IpConfig::Static {
            address: "10.0.0.1/24".to_string(),
            gateway: Some("10.0.0.254".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: IpConfig = serde_json::from_str(&json).unwrap();
        match deserialized {
            IpConfig::Static { address, gateway } => {
                assert_eq!(address, "10.0.0.1/24");
                assert_eq!(gateway, Some("10.0.0.254".to_string()));
            }
            _ => panic!("expected Static"),
        }
    }

    #[test]
    fn test_bond_mode_serialization() {
        let modes = vec![
            (BondMode::ActiveBackup, "active_backup"),
            (BondMode::Lacp, "lacp"),
            (BondMode::BalanceTlb, "balance_tlb"),
        ];
        for (mode, expected) in modes {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", expected));
            let deserialized: BondMode = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, mode);
        }
    }

    #[test]
    fn test_bond_config_defaults() {
        let toml_str = r#"
name = "bond0"
mode = "active_backup"
interfaces = ["eth1", "eth2"]
"#;
        let bond: BondConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(bond.miimon_ms, 100); // 默认值
        assert_eq!(bond.primary, None);
    }

    #[test]
    fn test_vlan_config_parse() {
        let toml_str = r#"
name = "eth0.100"
parent = "eth0"
id = 100
"#;
        let vlan: VlanConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(vlan.name, "eth0.100");
        assert_eq!(vlan.parent, "eth0");
        assert_eq!(vlan.id, 100);
    }

    #[test]
    fn test_dns_config_default() {
        let dns = DnsConfig::default();
        assert!(dns.nameservers.is_empty());
        assert!(dns.search.is_empty());
    }

    #[test]
    fn test_interface_config_up_default() {
        let toml_str = r#"
name = "eth0"
"#;
        let iface: InterfaceConfig = toml::from_str(toml_str).unwrap();
        assert!(iface.up); // 默认 true
    }

    #[test]
    fn test_dhcp_config_parse() {
        let toml_str = r#"
[[interfaces]]
name = "eth0"
ipv4 = { mode = "dhcp" }
"#;
        let config: NetworkConfig = toml::from_str(toml_str).unwrap();
        match &config.interfaces[0].ipv4 {
            Some(IpConfig::Dhcp) => {}
            _ => panic!("expected Dhcp"),
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_list_interfaces_unsupported() {
        let result = NetworkInterface::list();
        assert!(matches!(result, Err(NetworkError::UnsupportedPlatform)));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_apply_unsupported() {
        let config = NetworkConfig::default();
        let result = config.apply();
        assert!(matches!(result, Err(NetworkError::UnsupportedPlatform)));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_bond_status_list_unsupported() {
        let result = BondStatus::list();
        assert!(matches!(result, Err(NetworkError::UnsupportedPlatform)));
    }
}
