//! nftables 防火墙管理（Firewall Management）
//!
//! 提供基于 nftables 的防火墙规则管理，默认策略保护电力通信网络。
//! 入站仅允许 IEC 104/61850/SSH/EventBus，出站仅允许 NTP/PTP/syslog。

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// 防火墙错误
#[derive(Debug, Error)]
pub enum FirewallError {
    #[error("nft command failed: {0}")]
    NftFailed(String),
    #[error("config parse error: {0}")]
    ParseError(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 防火墙规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FirewallRule {
    pub direction: RuleDirection,
    pub protocol: Protocol,
    pub port: u16,
    #[serde(default)]
    pub source: Option<String>,
    pub action: Action,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuleDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Accept,
    #[default]
    Drop,
}

/// 防火墙配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FirewallConfig {
    #[serde(default)]
    pub rules: Vec<FirewallRule>,
    #[serde(default = "default_drop")]
    pub default_input_policy: Action,
    #[serde(default = "default_drop")]
    pub default_output_policy: Action,
}

fn default_drop() -> Action {
    Action::Drop
}

/// 防火墙管理器
pub struct FirewallManager {
    config: FirewallConfig,
}

impl FirewallManager {
    /// 从配置文件加载
    pub fn load(path: &Path) -> Result<Self, FirewallError> {
        let content = std::fs::read_to_string(path)?;
        let config: FirewallConfig =
            toml::from_str(&content).map_err(|e| FirewallError::ParseError(e.to_string()))?;
        Ok(Self { config })
    }

    /// 使用默认电力通信安全策略创建
    pub fn with_default_policy() -> Self {
        let mut config = FirewallConfig {
            rules: Vec::new(),
            default_input_policy: Action::Drop,
            default_output_policy: Action::Drop,
        };
        // 入站规则
        config.rules.push(FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 22,
            source: None,
            action: Action::Accept,
            comment: "SSH management".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 102,
            source: None,
            action: Action::Accept,
            comment: "IEC 61850 MMS".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 2404,
            source: None,
            action: Action::Accept,
            comment: "IEC 104".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 9876,
            source: None,
            action: Action::Accept,
            comment: "EventBus internal".to_string(),
        });
        // 出站规则
        config.rules.push(FirewallRule {
            direction: RuleDirection::Output,
            protocol: Protocol::Udp,
            port: 123,
            source: None,
            action: Action::Accept,
            comment: "NTP".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Output,
            protocol: Protocol::Udp,
            port: 319,
            source: None,
            action: Action::Accept,
            comment: "PTP event".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Output,
            protocol: Protocol::Udp,
            port: 320,
            source: None,
            action: Action::Accept,
            comment: "PTP general".to_string(),
        });
        config.rules.push(FirewallRule {
            direction: RuleDirection::Output,
            protocol: Protocol::Udp,
            port: 514,
            source: None,
            action: Action::Accept,
            comment: "syslog".to_string(),
        });
        Self { config }
    }

    /// 应用规则到系统
    #[cfg(target_os = "linux")]
    pub fn apply(&self) -> Result<(), FirewallError> {
        let conf = self.to_nftables_conf();
        let output = std::process::Command::new("nft")
            .args(["-f", "-"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        use std::io::Write;
        let mut child = output;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(conf.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(FirewallError::NftFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply(&self) -> Result<(), FirewallError> {
        Err(FirewallError::UnsupportedPlatform)
    }

    /// 持久化到配置文件
    pub fn save(&self, path: &Path) -> Result<(), FirewallError> {
        let content =
            toml::to_string_pretty(&self.config).map_err(|e| FirewallError::ParseError(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 添加规则
    pub fn add_rule(&mut self, rule: FirewallRule) {
        self.config.rules.push(rule);
    }

    /// 列出当前规则
    pub fn list_rules(&self) -> &[FirewallRule] {
        &self.config.rules
    }

    /// 生成 nftables 配置文本
    pub fn to_nftables_conf(&self) -> String {
        let mut out = String::new();
        out.push_str("#!/usr/sbin/nft -f\n");
        out.push_str("# EnerOS 防火墙配置 — 电力通信安全策略\n\n");
        out.push_str("flush ruleset\n\n");
        out.push_str("table inet eneros {\n");

        // input chain
        out.push_str("    chain input {\n");
        out.push_str(&format!(
            "        type filter hook input priority 0; policy {};\n",
            policy_keyword(self.config.default_input_policy)
        ));
        out.push_str("        ct state established,related accept\n");
        out.push_str("        iif \"lo\" accept\n");
        for rule in &self.config.rules {
            if rule.direction == RuleDirection::Input {
                out.push_str(&format!(
                    "        {} dport {} {} comment \"{}\"\n",
                    protocol_keyword(rule.protocol),
                    rule.port,
                    action_keyword(rule.action),
                    rule.comment
                ));
            }
        }
        out.push_str("    }\n\n");

        // output chain
        out.push_str("    chain output {\n");
        out.push_str(&format!(
            "        type filter hook output priority 0; policy {};\n",
            policy_keyword(self.config.default_output_policy)
        ));
        out.push_str("        oif \"lo\" accept\n");
        for rule in &self.config.rules {
            if rule.direction == RuleDirection::Output {
                out.push_str(&format!(
                    "        {} dport {} {} comment \"{}\"\n",
                    protocol_keyword(rule.protocol),
                    rule.port,
                    action_keyword(rule.action),
                    rule.comment
                ));
            }
        }
        out.push_str("    }\n");
        out.push_str("}\n");
        out
    }
}

fn policy_keyword(action: Action) -> &'static str {
    match action {
        Action::Accept => "accept",
        Action::Drop => "drop",
    }
}

fn protocol_keyword(p: Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
    }
}

fn action_keyword(a: Action) -> &'static str {
    match a {
        Action::Accept => "accept",
        Action::Drop => "drop",
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_has_iec_ports() {
        let mgr = FirewallManager::with_default_policy();
        let rules = mgr.list_rules();
        let ports: Vec<u16> = rules.iter().map(|r| r.port).collect();
        assert!(ports.contains(&2404), "missing IEC 104 port 2404");
        assert!(ports.contains(&102), "missing IEC 61850 MMS port 102");
        assert!(ports.contains(&22), "missing SSH port 22");
    }

    #[test]
    fn test_nftables_conf_generation() {
        let mgr = FirewallManager::with_default_policy();
        let conf = mgr.to_nftables_conf();
        assert!(conf.contains("table inet eneros"));
        assert!(conf.contains("chain input"));
        assert!(conf.contains("chain output"));
        assert!(conf.contains("policy drop"));
        assert!(conf.contains("tcp dport 2404 accept"));
        assert!(conf.contains("udp dport 123 accept"));
    }

    #[test]
    fn test_rule_serialization() {
        let rule = FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 2404,
            source: None,
            action: Action::Accept,
            comment: "IEC 104".to_string(),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: FirewallRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, deserialized);
    }

    #[test]
    fn test_add_rule() {
        let mut mgr = FirewallManager::with_default_policy();
        let initial_count = mgr.list_rules().len();
        mgr.add_rule(FirewallRule {
            direction: RuleDirection::Input,
            protocol: Protocol::Tcp,
            port: 8080,
            source: None,
            action: Action::Accept,
            comment: "test".to_string(),
        });
        assert_eq!(mgr.list_rules().len(), initial_count + 1);
    }

    #[test]
    fn test_default_policy_drop() {
        let mgr = FirewallManager::with_default_policy();
        let conf = mgr.to_nftables_conf();
        assert!(conf.contains("policy drop"));
    }
}
