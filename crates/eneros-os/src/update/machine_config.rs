//! 声明式机器配置（v0.22.0）
//!
//! 定义 `MachineConfig` 结构体，描述设备的硬件规格、分区布局、网络、启动参数、
//! RT 配置和 Agent 列表。`eneros-imager` 构建时读取此配置生成镜像，
//! `eneros-init` 启动时读取此配置初始化系统。

use crate::update::error::UpdateError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 硬件规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareSpec {
    /// CPU 架构："x86_64" 或 "aarch64"
    pub arch: String,
    /// CPU 核心数
    pub cpu_cores: u32,
    /// 内存大小（MB）
    pub memory_mb: u32,
    /// 目标磁盘设备，如 "/dev/sda"
    pub disk_device: String,
}

/// 分区布局
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionLayout {
    /// EFI 分区大小（MB），默认 512（FAT32，共享）
    pub efi_size_mb: u32,
    /// 每个 Root 分区大小（MB），默认 1536（ext4，A/B 双槽位）
    pub root_size_mb: u32,
    /// Data 分区大小（MB），0 表示使用剩余空间（ext4，共享）
    pub data_size_mb: u32,
    /// Config 分区大小（MB），默认 256（ext4，共享）
    pub config_size_mb: u32,
}

/// 网络接口配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    /// 接口名，如 "eth0"
    pub name: String,
    /// 地址模式："static" 或 "dhcp"
    pub method: String,
    /// static 模式的 IP 地址
    pub address: Option<String>,
    /// static 模式的子网掩码
    pub netmask: Option<String>,
    /// static 模式的网关（可选）
    pub gateway: Option<String>,
}

/// 网络配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSpec {
    /// 主机名
    pub hostname: String,
    /// 网络接口列表
    pub interfaces: Vec<InterfaceConfig>,
}

/// PREEMPT_RT 实时配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtConfig {
    /// 是否启用 RT 配置
    pub enabled: bool,
    /// CPU 隔离列表（isolcpus），如 [2, 3]
    pub isolated_cpus: Vec<u32>,
    /// No-HZ full CPU 列表
    pub nohz_full: Vec<u32>,
    /// RCU no-callbacks CPU 列表
    pub rcu_nocbs: Vec<u32>,
}

/// 启动配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootSpec {
    /// 额外内核启动参数
    #[serde(default)]
    pub kernel_params: Vec<String>,
    /// RT 配置
    pub rt_config: RtConfig,
}

/// Agent 进程配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Agent 名称，如 "dispatch-agent"
    pub name: String,
    /// 是否启用
    pub enabled: bool,
    /// CPU 配额百分比（可选）
    pub cpu_quota: Option<u32>,
    /// 内存限制（MB，可选）
    pub memory_limit_mb: Option<u32>,
}

/// 声明式机器配置根结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    /// 硬件规格
    pub hardware: HardwareSpec,
    /// 分区布局
    pub partitions: PartitionLayout,
    /// 网络配置
    pub network: NetworkSpec,
    /// 启动配置
    pub boot: BootSpec,
    /// Agent 进程列表
    #[serde(default)]
    pub agents: Vec<AgentSpec>,
}

impl MachineConfig {
    /// 从 YAML 文件加载配置
    pub fn load_from_yaml(path: &Path) -> Result<Self, UpdateError> {
        let content = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&content).map_err(|e| UpdateError::Serialize(e.to_string()))
    }

    /// 保存配置到 YAML 文件
    pub fn save_to_yaml(&self, path: &Path) -> Result<(), UpdateError> {
        let content =
            serde_yaml::to_string(self).map_err(|e| UpdateError::Serialize(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 校验配置
    pub fn validate(&self) -> Result<(), UpdateError> {
        // 硬件校验
        if self.hardware.arch != "x86_64" && self.hardware.arch != "aarch64" {
            return Err(UpdateError::Config(format!(
                "invalid arch '{}': must be 'x86_64' or 'aarch64'",
                self.hardware.arch
            )));
        }
        if self.hardware.cpu_cores < 1 {
            return Err(UpdateError::Config("cpu_cores must be >= 1".to_string()));
        }
        if self.hardware.memory_mb < 256 {
            return Err(UpdateError::Config("memory_mb must be >= 256".to_string()));
        }

        // 分区校验
        if self.partitions.efi_size_mb < 64 {
            return Err(UpdateError::Config("efi_size_mb must be >= 64".to_string()));
        }
        if self.partitions.root_size_mb < 512 {
            return Err(UpdateError::Config(
                "root_size_mb must be >= 512".to_string(),
            ));
        }
        if self.partitions.config_size_mb < 64 {
            return Err(UpdateError::Config(
                "config_size_mb must be >= 64".to_string(),
            ));
        }

        // 网络校验
        if self.network.hostname.is_empty() {
            return Err(UpdateError::Config(
                "hostname must not be empty".to_string(),
            ));
        }
        if self.network.interfaces.is_empty() {
            return Err(UpdateError::Config(
                "at least one network interface is required".to_string(),
            ));
        }
        for iface in &self.network.interfaces {
            if iface.name.is_empty() {
                return Err(UpdateError::Config(
                    "interface name must not be empty".to_string(),
                ));
            }
            if iface.method != "static" && iface.method != "dhcp" {
                return Err(UpdateError::Config(format!(
                    "interface '{}' method must be 'static' or 'dhcp'",
                    iface.name
                )));
            }
            if iface.method == "static" {
                if iface.address.is_none() {
                    return Err(UpdateError::Config(format!(
                        "interface '{}' uses static mode but has no address",
                        iface.name
                    )));
                }
                if iface.netmask.is_none() {
                    return Err(UpdateError::Config(format!(
                        "interface '{}' uses static mode but has no netmask",
                        iface.name
                    )));
                }
            }
        }

        Ok(())
    }

    /// 生成 init.toml 格式的 TOML 字符串
    ///
    /// 包含标准系统服务（network/timesync/syslog/devmgr/power-app）和
    /// 根据 `agents` 列表生成的 Agent 进程配置。
    pub fn generate_init_config(&self) -> String {
        let mut out = String::new();
        out.push_str("# Generated by EnerOS machine_config from eneros-machine.yaml\n\n");

        // 标准系统服务
        let services: [(&str, &str, &str, &[&str]); 5] = [
            ("network", "/bin/eneros-netcfg", "always", &[]),
            ("timesync", "/bin/eneros-timesync", "always", &["network"]),
            ("syslog", "/bin/eneros-syslog", "always", &[]),
            ("devmgr", "/bin/eneros-devmgr", "always", &[]),
            (
                "power-app",
                "/bin/eneros-api",
                "on_failure",
                &["network", "timesync", "syslog", "devmgr"],
            ),
        ];

        for (name, binary, policy, deps) in services {
            out.push_str("[[services]]\n");
            out.push_str(&format!("name = \"{}\"\n", name));
            out.push_str(&format!("binary = \"{}\"\n", binary));
            out.push_str(&format!("restart_policy = \"{}\"\n", policy));
            if deps.is_empty() {
                out.push_str("dependencies = []\n");
            } else {
                let deps_str: Vec<String> = deps.iter().map(|d| format!("\"{}\"", d)).collect();
                out.push_str(&format!("dependencies = [{}]\n", deps_str.join(", ")));
            }
            out.push_str("graceful_timeout_secs = 10\n\n");
        }

        // Agent 进程（仅启用的）
        for agent in &self.agents {
            if !agent.enabled {
                continue;
            }
            let (agent_id, agent_type, authority, binary) = agent_init_fields(&agent.name);
            out.push_str("[[agents]]\n");
            out.push_str(&format!("agent_id = \"{}\"\n", agent_id));
            out.push_str(&format!("agent_type = \"{}\"\n", agent_type));
            out.push_str(&format!("authority = \"{}\"\n", authority));
            out.push_str(&format!("binary = \"{}\"\n", binary));
            out.push_str(&format!(
                "args = [\"--agent-id\", \"{}\", \"--eventbus-addr\", \"127.0.0.1:9876\", \"--gateway-addr\", \"127.0.0.1:9877\"]\n",
                agent_id
            ));
            out.push_str("dependencies = [\"power-app\"]\n\n");

            let cpu = agent.cpu_quota.unwrap_or(50);
            let mem = agent.memory_limit_mb.unwrap_or(512);
            out.push_str("[agents.resource_quota]\n");
            out.push_str(&format!("cpu_percent = {}\n", cpu));
            out.push_str(&format!("memory_mb = {}\n", mem));
            out.push_str("max_pids = 100\n\n");
        }

        out
    }

    /// 生成 network.toml 格式的 TOML 字符串
    pub fn generate_network_config(&self) -> String {
        let mut out = String::new();
        out.push_str("# Generated by EnerOS machine_config from eneros-machine.yaml\n\n");

        for iface in &self.network.interfaces {
            out.push_str("[[interfaces]]\n");
            out.push_str(&format!("name = \"{}\"\n", iface.name));
            match iface.method.as_str() {
                "dhcp" => {
                    out.push_str("ipv4 = { mode = \"dhcp\" }\n");
                }
                "static" => {
                    let addr = iface.address.as_deref().unwrap_or("");
                    let prefix = iface
                        .netmask
                        .as_deref()
                        .and_then(netmask_to_prefix)
                        .unwrap_or(24);
                    let cidr = format!("{}/{}", addr, prefix);
                    if let Some(gw) = &iface.gateway {
                        out.push_str(&format!(
                            "ipv4 = {{ mode = \"static\", address = \"{}\", gateway = \"{}\" }}\n",
                            cidr, gw
                        ));
                    } else {
                        out.push_str(&format!(
                            "ipv4 = {{ mode = \"static\", address = \"{}\" }}\n",
                            cidr
                        ));
                    }
                }
                _ => {
                    out.push_str(&format!("method = \"{}\"\n", iface.method));
                }
            }
            out.push('\n');
        }

        out
    }

    /// 生成内核启动参数字符串
    ///
    /// RT 启用时生成：`isolcpus=2,3 nohz_full=2,3 rcu_nocbs=2,3 mlock=1`
    /// 加上 `kernel_params` 中的额外参数。
    pub fn generate_kernel_cmdline(&self) -> String {
        let mut params: Vec<String> = Vec::new();

        // RT 配置
        if self.boot.rt_config.enabled {
            if !self.boot.rt_config.isolated_cpus.is_empty() {
                let cpus: Vec<String> =
                    self.boot.rt_config.isolated_cpus.iter().map(|c| c.to_string()).collect();
                params.push(format!("isolcpus={}", cpus.join(",")));
            }
            if !self.boot.rt_config.nohz_full.is_empty() {
                let cpus: Vec<String> =
                    self.boot.rt_config.nohz_full.iter().map(|c| c.to_string()).collect();
                params.push(format!("nohz_full={}", cpus.join(",")));
            }
            if !self.boot.rt_config.rcu_nocbs.is_empty() {
                let cpus: Vec<String> =
                    self.boot.rt_config.rcu_nocbs.iter().map(|c| c.to_string()).collect();
                params.push(format!("rcu_nocbs={}", cpus.join(",")));
            }
            params.push("mlock=1".to_string());
        }

        // 额外内核参数
        for p in &self.boot.kernel_params {
            params.push(p.clone());
        }

        params.join(" ")
    }
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            hardware: HardwareSpec {
                arch: "x86_64".to_string(),
                cpu_cores: 4,
                memory_mb: 4096,
                disk_device: "/dev/sda".to_string(),
            },
            partitions: PartitionLayout {
                efi_size_mb: 512,
                root_size_mb: 1536,
                data_size_mb: 0,
                config_size_mb: 256,
            },
            network: NetworkSpec {
                hostname: "eneros-node".to_string(),
                interfaces: vec![InterfaceConfig {
                    name: "eth0".to_string(),
                    method: "dhcp".to_string(),
                    address: None,
                    netmask: None,
                    gateway: None,
                }],
            },
            boot: BootSpec {
                kernel_params: Vec::new(),
                rt_config: RtConfig {
                    enabled: false,
                    isolated_cpus: Vec::new(),
                    nohz_full: Vec::new(),
                    rcu_nocbs: Vec::new(),
                },
            },
            agents: Vec::new(),
        }
    }
}

/// 从 Agent 名称推导 init.toml 字段（agent_id, agent_type, authority, binary）
fn agent_init_fields(name: &str) -> (String, String, &'static str, String) {
    let base = name.strip_suffix("-agent").unwrap_or(name);
    let agent_type = kebab_to_pascal(base);
    let agent_id = format!("{}-1", base);
    let binary = format!("/bin/eneros-{}", name);
    let authority = authority_for_agent_type(&agent_type);
    (agent_id, agent_type, authority, binary)
}

/// kebab-case 转 PascalCase（如 "self-healing" → "SelfHealing"）
fn kebab_to_pascal(s: &str) -> String {
    s.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// 根据 Agent 类型返回默认权限级别
fn authority_for_agent_type(agent_type: &str) -> &'static str {
    match agent_type {
        "Dispatch" => "Supervisor",
        "Forecast" => "Observer",
        "Operation" => "Supervisor",
        "SelfHealing" => "Emergency",
        "Planning" => "Operator",
        "Trading" => "Operator",
        _ => "Observer",
    }
}

/// 将子网掩码转换为 CIDR 前缀长度（如 "255.255.255.0" → 24）
fn netmask_to_prefix(netmask: &str) -> Option<u8> {
    let parts: Vec<u8> = netmask.split('.').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 4 {
        return None;
    }
    let bits: u32 = ((parts[0] as u32) << 24)
        | ((parts[1] as u32) << 16)
        | ((parts[2] as u32) << 8)
        | (parts[3] as u32);
    let prefix = bits.count_ones() as u8;
    // 验证是连续的 1 后跟 0
    let mask = if prefix == 0 { 0 } else { (!0u32) << (32 - prefix) };
    if bits != mask {
        return None;
    }
    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yaml_roundtrip() {
        let config = MachineConfig::default();
        let tmp = std::env::temp_dir().join("eneros_machine_config_roundtrip.yaml");
        config.save_to_yaml(&tmp).unwrap();
        let loaded = MachineConfig::load_from_yaml(&tmp).unwrap();
        assert_eq!(loaded.hardware.arch, config.hardware.arch);
        assert_eq!(loaded.hardware.cpu_cores, config.hardware.cpu_cores);
        assert_eq!(loaded.hardware.memory_mb, config.hardware.memory_mb);
        assert_eq!(loaded.network.hostname, config.network.hostname);
        assert_eq!(
            loaded.network.interfaces.len(),
            config.network.interfaces.len()
        );
        assert_eq!(
            loaded.network.interfaces[0].name,
            config.network.interfaces[0].name
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_validate_ok() {
        let config = MachineConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_bad_arch() {
        let mut config = MachineConfig::default();
        config.hardware.arch = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_no_interfaces() {
        let mut config = MachineConfig::default();
        config.network.interfaces.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_static_missing_address() {
        let mut config = MachineConfig::default();
        config.network.interfaces[0].method = "static".to_string();
        config.network.interfaces[0].address = None;
        config.network.interfaces[0].netmask = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_generate_init_config() {
        let mut config = MachineConfig::default();
        config.agents.push(AgentSpec {
            name: "dispatch-agent".to_string(),
            enabled: true,
            cpu_quota: Some(25),
            memory_limit_mb: Some(512),
        });
        let toml = config.generate_init_config();
        assert!(toml.contains("[[services]]"));
        assert!(toml.contains("[[agents]]"));
        assert!(toml.contains("dispatch-1"));
        assert!(toml.contains("Dispatch"));
        assert!(toml.contains("/bin/eneros-dispatch-agent"));
    }

    #[test]
    fn test_generate_init_config_self_healing() {
        let mut config = MachineConfig::default();
        config.agents.push(AgentSpec {
            name: "self-healing-agent".to_string(),
            enabled: true,
            cpu_quota: Some(80),
            memory_limit_mb: Some(1024),
        });
        let toml = config.generate_init_config();
        assert!(toml.contains("self-healing-1"));
        assert!(toml.contains("SelfHealing"));
        assert!(toml.contains("Emergency"));
    }

    #[test]
    fn test_generate_init_config_disabled_agent() {
        let mut config = MachineConfig::default();
        config.agents.push(AgentSpec {
            name: "dispatch-agent".to_string(),
            enabled: false,
            cpu_quota: None,
            memory_limit_mb: None,
        });
        let toml = config.generate_init_config();
        assert!(toml.contains("[[services]]"));
        assert!(!toml.contains("[[agents]]"));
    }

    #[test]
    fn test_generate_network_config() {
        let config = MachineConfig::default();
        let toml = config.generate_network_config();
        assert!(toml.contains("[[interfaces]]"));
        assert!(toml.contains("eth0"));
        assert!(toml.contains("dhcp"));
    }

    #[test]
    fn test_generate_network_config_static() {
        let mut config = MachineConfig::default();
        config.network.interfaces[0].method = "static".to_string();
        config.network.interfaces[0].address = Some("192.168.1.100".to_string());
        config.network.interfaces[0].netmask = Some("255.255.255.0".to_string());
        config.network.interfaces[0].gateway = Some("192.168.1.1".to_string());
        let toml = config.generate_network_config();
        assert!(toml.contains("static"));
        assert!(toml.contains("192.168.1.100/24"));
        assert!(toml.contains("192.168.1.1"));
    }

    #[test]
    fn test_generate_kernel_cmdline_rt() {
        let mut config = MachineConfig::default();
        config.boot.rt_config.enabled = true;
        config.boot.rt_config.isolated_cpus = vec![2, 3];
        config.boot.rt_config.nohz_full = vec![2, 3];
        config.boot.rt_config.rcu_nocbs = vec![2, 3];
        let cmdline = config.generate_kernel_cmdline();
        assert!(cmdline.contains("isolcpus=2,3"));
        assert!(cmdline.contains("nohz_full=2,3"));
        assert!(cmdline.contains("rcu_nocbs=2,3"));
        assert!(cmdline.contains("mlock=1"));
    }

    #[test]
    fn test_generate_kernel_cmdline_no_rt() {
        let config = MachineConfig::default();
        let cmdline = config.generate_kernel_cmdline();
        assert!(!cmdline.contains("isolcpus"));
        assert!(!cmdline.contains("mlock=1"));
    }

    #[test]
    fn test_generate_kernel_cmdline_extra_params() {
        let mut config = MachineConfig::default();
        config.boot.kernel_params.push("console=ttyS0,115200".to_string());
        config.boot.kernel_params.push("panic=10".to_string());
        let cmdline = config.generate_kernel_cmdline();
        assert!(cmdline.contains("console=ttyS0,115200"));
        assert!(cmdline.contains("panic=10"));
    }

    #[test]
    fn test_default() {
        let config = MachineConfig::default();
        assert_eq!(config.hardware.arch, "x86_64");
        assert_eq!(config.hardware.cpu_cores, 4);
        assert_eq!(config.hardware.memory_mb, 4096);
        assert_eq!(config.hardware.disk_device, "/dev/sda");
        assert_eq!(config.partitions.efi_size_mb, 512);
        assert_eq!(config.partitions.root_size_mb, 1536);
        assert_eq!(config.partitions.data_size_mb, 0);
        assert_eq!(config.partitions.config_size_mb, 256);
        assert_eq!(config.network.hostname, "eneros-node");
        assert_eq!(config.network.interfaces.len(), 1);
        assert_eq!(config.network.interfaces[0].name, "eth0");
        assert_eq!(config.network.interfaces[0].method, "dhcp");
        assert!(!config.boot.rt_config.enabled);
        assert!(config.agents.is_empty());
    }

    #[test]
    fn test_netmask_to_prefix() {
        assert_eq!(netmask_to_prefix("255.255.255.0"), Some(24));
        assert_eq!(netmask_to_prefix("255.255.0.0"), Some(16));
        assert_eq!(netmask_to_prefix("255.0.0.0"), Some(8));
        assert_eq!(netmask_to_prefix("255.255.255.128"), Some(25));
        assert_eq!(netmask_to_prefix("0.0.0.0"), Some(0));
        assert_eq!(netmask_to_prefix("invalid"), None);
    }

    #[test]
    fn test_kebab_to_pascal() {
        assert_eq!(kebab_to_pascal("dispatch"), "Dispatch");
        assert_eq!(kebab_to_pascal("self-healing"), "SelfHealing");
        assert_eq!(kebab_to_pascal("forecast"), "Forecast");
    }

    #[test]
    fn test_load_from_yaml_file() {
        let config = MachineConfig::default();
        let tmp = std::env::temp_dir().join("eneros_machine_config_load_test.yaml");
        config.save_to_yaml(&tmp).unwrap();
        let loaded = MachineConfig::load_from_yaml(&tmp).unwrap();
        assert!(loaded.validate().is_ok());
        let _ = std::fs::remove_file(&tmp);
    }
}
