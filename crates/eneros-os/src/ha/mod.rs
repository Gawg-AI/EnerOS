//! EnerOS 高可用（HA）模块
//!
//! 提供双节点高可用基础：
//! - heartbeat: UDP 多播心跳检测（100ms 间隔，300ms 故障检测）
//! - sync: 状态同步（SCADA/Agent/命令历史/配置，延迟 < 100ms）
//! - storage: 共享状态存储（应用级复制引擎）
//! - fencing: 脑裂防护（Fencing 框架）

pub mod heartbeat;
pub mod sync;
pub mod storage;
pub mod fencing;
pub mod failover;
pub mod cluster;
pub mod drill;

// 重新导出主要类型
pub use heartbeat::{HeartbeatManager, HeartbeatPacket, NodeState, NodeStateChange, NodeRole};
pub use sync::{
    BatchConfig, SyncBatch, SyncError, SyncManager, SyncMessage, SyncStats, SyncStatus,
    SYNC_BATCH_VERSION,
};
pub use storage::{SharedStore, StorageEntry, StorageError, StorageQuota, ConflictResolution};
pub use fencing::{
    FencingError, FencingManager, FencingRecord, FencingResult, FencingStrategy,
    QuorumState, SplitBrainConfig, SplitBrainResult,
};
pub use failover::{
    FailoverConfig, FailoverEngine, FailoverError, FailoverRecord, FailoverState, FailoverStatus,
    HaEvent, RecoveryPolicy,
};
pub use cluster::{
    ClusterConfig, ClusterManager, ClusterMember, ClusterMemberRole, MemberCallback, MemberEvent,
    MemberStatus, QuorumPolicy, QuorumResult,
};
pub use drill::{
    DrillConfig, DrillError, DrillResult, DrillScenario, DrillSchedule, DrillScheduler,
};

use serde::{Deserialize, Serialize};

/// HA 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaConfig {
    /// 本节点 ID
    pub node_id: String,
    /// 节点角色（primary/secondary）
    pub role: NodeRole,
    /// 心跳间隔（毫秒）
    #[serde(default = "default_heartbeat_interval_ms")]
    pub heartbeat_interval_ms: u64,
    /// 心跳超时（毫秒，suspect 阈值）
    #[serde(default = "default_heartbeat_suspect_ms")]
    pub heartbeat_suspect_ms: u64,
    /// 心跳超时（毫秒，dead 阈值）
    #[serde(default = "default_heartbeat_dead_ms")]
    pub heartbeat_dead_ms: u64,
    /// UDP 多播地址
    #[serde(default = "default_multicast_addr")]
    pub multicast_addr: String,
    /// 心跳端口
    #[serde(default = "default_heartbeat_port")]
    pub heartbeat_port: u16,
    /// 同步端口
    #[serde(default = "default_sync_port")]
    pub sync_port: u16,
    /// 网络接口列表（双网卡冗余）
    #[serde(default)]
    pub interfaces: Vec<String>,
    /// 节点优先级（数字越大优先级越高）
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Fencing 策略
    #[serde(default)]
    pub fencing_strategy: FencingStrategy,
    /// 同步范围
    #[serde(default)]
    pub sync_scope: SyncScope,
    /// HMAC 认证密钥（用于心跳/同步消息认证）
    #[serde(default)]
    pub auth_key: Option<String>,
    /// 多播 TTL（Time-To-Live），默认 32
    #[serde(default = "default_multicast_ttl")]
    pub multicast_ttl: u8,
    /// 是否为生产环境（生产环境强制启用 fencing，测试环境可设为 false）
    #[serde(default = "default_is_production")]
    pub is_production: bool,
    /// Failover 配置（v0.26.0 — Task 9）
    #[serde(default)]
    pub failover: Option<FailoverConfig>,
    /// 集群配置（v0.26.0 — Task 9）
    #[serde(default)]
    pub cluster: Option<ClusterConfig>,
    /// 灾备演练配置（v0.26.0 — Task 9）
    #[serde(default)]
    pub drill: Option<DrillConfig>,
}

fn default_heartbeat_interval_ms() -> u64 {
    100
}
fn default_heartbeat_suspect_ms() -> u64 {
    100
}
fn default_heartbeat_dead_ms() -> u64 {
    300
}
fn default_multicast_addr() -> String {
    "239.0.0.1".to_string()
}
fn default_heartbeat_port() -> u16 {
    5400
}
fn default_sync_port() -> u16 {
    5401
}
fn default_priority() -> u32 {
    100
}
fn default_multicast_ttl() -> u8 {
    32
}
fn default_is_production() -> bool {
    true
}

/// 同步范围配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncScope {
    #[serde(default = "default_true")]
    pub sync_scada: bool,
    #[serde(default = "default_true")]
    pub sync_agent_state: bool,
    #[serde(default = "default_true")]
    pub sync_command_history: bool,
    #[serde(default = "default_true")]
    pub sync_config: bool,
}

impl Default for SyncScope {
    fn default() -> Self {
        Self {
            sync_scada: true,
            sync_agent_state: true,
            sync_command_history: true,
            sync_config: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// HA 配置错误
#[derive(Debug, thiserror::Error)]
pub enum HaConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid config: {0}")]
    Invalid(String),
}

impl HaConfig {
    /// 从文件加载 HA 配置
    pub fn load<P: AsRef<std::path::Path>>(path: P) -> Result<Self, HaConfigError> {
        let content = std::fs::read_to_string(path)?;
        Self::load_from_str(&content)
    }

    /// 从字符串加载 HA 配置
    pub fn load_from_str(content: &str) -> Result<Self, HaConfigError> {
        let config: Self = toml::from_str(content)?;
        config.validate()?;
        Ok(config)
    }

    /// 校验配置约束
    ///
    /// 检查以下约束，违反则返回 [`HaConfigError::Invalid`]：
    /// - `heartbeat_suspect_ms < heartbeat_dead_ms`（suspect 必须小于 dead）
    /// - `heartbeat_interval_ms > 0`（interval 必须大于 0）
    /// - `heartbeat_suspect_ms >= heartbeat_interval_ms`（suspect 必须 >= interval）
    /// - `multicast_addr` 在 224.0.0.0/4 范围（octets\[0\] >= 224 && octets\[0\] <= 239）
    /// - `heartbeat_port != sync_port`（端口不能冲突）
    /// - `node_id` 非空
    /// - 生产环境（`is_production == true`）`fencing_strategy != None`
    pub fn validate(&self) -> Result<(), HaConfigError> {
        // suspect 必须小于 dead
        if self.heartbeat_suspect_ms >= self.heartbeat_dead_ms {
            return Err(HaConfigError::Invalid(format!(
                "heartbeat_suspect_ms ({}) must be less than heartbeat_dead_ms ({})",
                self.heartbeat_suspect_ms, self.heartbeat_dead_ms
            )));
        }
        // interval 必须大于 0
        if self.heartbeat_interval_ms == 0 {
            return Err(HaConfigError::Invalid(
                "heartbeat_interval_ms must be greater than 0".to_string(),
            ));
        }
        // suspect 必须 >= interval
        if self.heartbeat_suspect_ms < self.heartbeat_interval_ms {
            return Err(HaConfigError::Invalid(format!(
                "heartbeat_suspect_ms ({}) must be >= heartbeat_interval_ms ({})",
                self.heartbeat_suspect_ms, self.heartbeat_interval_ms
            )));
        }
        // multicast_addr 在 224.0.0.0/4 范围
        let octets = self
            .multicast_addr
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| {
                HaConfigError::Invalid(format!(
                    "multicast_addr '{}' is not a valid IPv4 address",
                    self.multicast_addr
                ))
            })?
            .octets();
        if octets[0] < 224 || octets[0] > 239 {
            return Err(HaConfigError::Invalid(format!(
                "multicast_addr '{}' is not in 224.0.0.0/4 multicast range",
                self.multicast_addr
            )));
        }
        // 端口不能冲突
        if self.heartbeat_port == self.sync_port {
            return Err(HaConfigError::Invalid(format!(
                "heartbeat_port ({}) conflicts with sync_port ({})",
                self.heartbeat_port, self.sync_port
            )));
        }
        // node_id 非空
        if self.node_id.is_empty() {
            return Err(HaConfigError::Invalid(
                "node_id must not be empty".to_string(),
            ));
        }
        // 生产环境 fencing_strategy != None
        if self.is_production && self.fencing_strategy == FencingStrategy::None {
            return Err(HaConfigError::Invalid(
                "fencing_strategy must not be None in production environment".to_string(),
            ));
        }
        // v0.26.0 — failover 配置校验
        if let Some(ref failover) = self.failover {
            // takeover_timeout_ms 必须 > 0
            if failover.takeover_timeout_ms == 0 {
                return Err(HaConfigError::Invalid(
                    "failover.takeover_timeout_ms must be greater than 0".to_string(),
                ));
            }
            // vip 非空时校验为合法 IPv4/IPv6 地址（支持 CIDR 表示法如 192.168.1.100/24）
            if !failover.vip.is_empty() {
                // 去除 CIDR 前缀长度（如 192.168.1.100/24 → 192.168.1.100）
                let ip_str = failover.vip.split('/').next().unwrap_or(&failover.vip);
                let vip_valid = ip_str.parse::<std::net::IpAddr>().is_ok();
                if !vip_valid {
                    return Err(HaConfigError::Invalid(format!(
                        "failover.vip '{}' is not a valid IP address",
                        failover.vip
                    )));
                }
            }
        }
        // v0.26.0 — Task 9 cluster 配置校验
        if let Some(ref cluster) = self.cluster {
            // members 非空
            if cluster.members.is_empty() {
                return Err(HaConfigError::Invalid(
                    "cluster.members must not be empty".to_string(),
                ));
            }
            // witness 不在 members 中（witness 列表中的节点 ID 不应出现在 members 列表中）
            let member_ids: std::collections::HashSet<&str> =
                cluster.members.iter().map(|m| m.node_id.as_str()).collect();
            for w in &cluster.witness {
                if member_ids.contains(w.as_str()) {
                    return Err(HaConfigError::Invalid(format!(
                        "cluster.witness '{}' is also in cluster.members, witness must be separate",
                        w
                    )));
                }
            }
            // local_node_id（node_id）必须在 members 中
            if !member_ids.contains(self.node_id.as_str()) {
                return Err(HaConfigError::Invalid(format!(
                    "node_id '{}' must be in cluster.members",
                    self.node_id
                )));
            }
        }
        // v0.26.0 — Task 9 drill 配置校验
        if let Some(ref drill) = self.drill {
            // 如果启用演练，scenarios 非空
            if drill.enabled && drill.scenarios.is_empty() {
                return Err(HaConfigError::Invalid(
                    "drill.scenarios must not be empty when drill is enabled".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// 获取心跳间隔
    pub fn heartbeat_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.heartbeat_interval_ms)
    }

    /// 获取 suspect 超时
    pub fn heartbeat_suspect_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.heartbeat_suspect_ms)
    }

    /// 获取 dead 超时
    pub fn heartbeat_dead_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.heartbeat_dead_ms)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ha_config_default() {
        // 验证默认值函数
        assert_eq!(default_heartbeat_interval_ms(), 100);
        assert_eq!(default_heartbeat_suspect_ms(), 100);
        assert_eq!(default_heartbeat_dead_ms(), 300);
        assert_eq!(default_multicast_addr(), "239.0.0.1");
        assert_eq!(default_heartbeat_port(), 5400);
        assert_eq!(default_sync_port(), 5401);
        assert_eq!(default_priority(), 100);
        assert_eq!(default_multicast_ttl(), 32);
        assert!(default_is_production());

        // 验证 SyncScope 默认值（所有同步范围默认开启）
        let scope = SyncScope::default();
        assert!(scope.sync_scada);
        assert!(scope.sync_agent_state);
        assert!(scope.sync_command_history);
        assert!(scope.sync_config);

        // 验证 FencingStrategy 默认值为 None
        assert_eq!(FencingStrategy::default(), FencingStrategy::None);
    }

    #[test]
    fn test_ha_config_load_from_str() {
        let content = r#"
# EnerOS 高可用配置
node_id = "node-1"
role = "primary"
heartbeat_interval_ms = 100
heartbeat_suspect_ms = 100
heartbeat_dead_ms = 300
multicast_addr = "239.0.0.1"
heartbeat_port = 5400
sync_port = 5401
priority = 100
fencing_strategy = "none"
is_production = false

interfaces = ["eth0", "eth1"]

[sync_scope]
sync_scada = true
sync_agent_state = true
sync_command_history = true
sync_config = true
"#;
        let config = HaConfig::load_from_str(content).expect("parse full config");
        assert_eq!(config.node_id, "node-1");
        assert_eq!(config.role, NodeRole::Primary);
        assert_eq!(config.heartbeat_interval_ms, 100);
        assert_eq!(config.heartbeat_suspect_ms, 100);
        assert_eq!(config.heartbeat_dead_ms, 300);
        assert_eq!(config.multicast_addr, "239.0.0.1");
        assert_eq!(config.heartbeat_port, 5400);
        assert_eq!(config.sync_port, 5401);
        assert_eq!(config.priority, 100);
        assert_eq!(config.fencing_strategy, FencingStrategy::None);
        assert_eq!(config.interfaces, vec!["eth0".to_string(), "eth1".to_string()]);
        assert!(config.sync_scope.sync_scada);
        assert!(config.sync_scope.sync_agent_state);
        assert!(config.sync_scope.sync_command_history);
        assert!(config.sync_scope.sync_config);
        assert_eq!(config.multicast_ttl, 32);
        assert!(!config.is_production);
        assert!(config.auth_key.is_none());
    }

    #[test]
    fn test_ha_config_load_from_str_minimal() {
        // 最小配置：只有 node_id 和 role，其余字段使用 serde 默认值
        // is_production 默认为 true，但生产环境要求 fencing_strategy != None，
        // 因此测试时显式设为 false 以允许 fencing_strategy = none
        let content = r#"
node_id = "node-2"
role = "secondary"
is_production = false
"#;
        let config = HaConfig::load_from_str(content).expect("parse minimal config");
        assert_eq!(config.node_id, "node-2");
        assert_eq!(config.role, NodeRole::Secondary);
        // 验证默认值被正确应用
        assert_eq!(config.heartbeat_interval_ms, 100);
        assert_eq!(config.heartbeat_suspect_ms, 100);
        assert_eq!(config.heartbeat_dead_ms, 300);
        assert_eq!(config.multicast_addr, "239.0.0.1");
        assert_eq!(config.heartbeat_port, 5400);
        assert_eq!(config.sync_port, 5401);
        assert_eq!(config.priority, 100);
        assert_eq!(config.fencing_strategy, FencingStrategy::None);
        assert!(config.interfaces.is_empty());
        assert!(config.sync_scope.sync_scada);
        assert!(config.sync_scope.sync_agent_state);
        assert!(config.sync_scope.sync_command_history);
        assert!(config.sync_scope.sync_config);
        assert_eq!(config.multicast_ttl, 32);
        assert!(!config.is_production);
        assert!(config.auth_key.is_none());
    }

    #[test]
    fn test_ha_config_load_invalid() {
        // 无效的 role 值（只接受 primary/secondary）
        let content = r#"
node_id = "node-1"
role = "invalid_role"
"#;
        let result = HaConfig::load_from_str(content);
        assert!(result.is_err(), "invalid role should fail");
        assert!(
            matches!(result.unwrap_err(), HaConfigError::Parse(_)),
            "invalid role should return Parse error"
        );

        // 缺少必填字段 node_id
        let content2 = r#"
role = "primary"
"#;
        let result2 = HaConfig::load_from_str(content2);
        assert!(result2.is_err(), "missing node_id should fail");
        assert!(
            matches!(result2.unwrap_err(), HaConfigError::Parse(_)),
            "missing node_id should return Parse error"
        );

        // 无效的 TOML 语法
        let content3 = "node_id = = =";
        let result3 = HaConfig::load_from_str(content3);
        assert!(result3.is_err(), "invalid toml syntax should fail");
        assert!(
            matches!(result3.unwrap_err(), HaConfigError::Parse(_)),
            "invalid toml syntax should return Parse error"
        );
    }

    #[test]
    fn test_ha_config_heartbeat_intervals() {
        let config = HaConfig {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            heartbeat_interval_ms: 100,
            heartbeat_suspect_ms: 100,
            heartbeat_dead_ms: 300,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 5401,
            interfaces: Vec::new(),
            priority: 100,
            fencing_strategy: FencingStrategy::None,
            sync_scope: SyncScope::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        };
        assert_eq!(
            config.heartbeat_interval(),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            config.heartbeat_suspect_timeout(),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            config.heartbeat_dead_timeout(),
            std::time::Duration::from_millis(300)
        );

        // 验证自定义值
        let config2 = HaConfig {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            heartbeat_interval_ms: 200,
            heartbeat_suspect_ms: 250,
            heartbeat_dead_ms: 500,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 5401,
            interfaces: Vec::new(),
            priority: 100,
            fencing_strategy: FencingStrategy::None,
            sync_scope: SyncScope::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        };
        assert_eq!(
            config2.heartbeat_interval(),
            std::time::Duration::from_millis(200)
        );
        assert_eq!(
            config2.heartbeat_suspect_timeout(),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            config2.heartbeat_dead_timeout(),
            std::time::Duration::from_millis(500)
        );
    }
}
