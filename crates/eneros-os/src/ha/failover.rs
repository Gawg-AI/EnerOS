//! 热备切换引擎（v0.26.0 — Task 2）
//!
//! 实现 failover 状态机和切换流程：
//! - 状态机：Standby → TakingOver → Active → FailingBack → Standby（或 Failed）
//! - IP 接管/释放（Linux: `ip addr add/del` + `arping`）
//! - 服务接管通知（[`HaEvent`] 发布）
//! - 切换日志（JSON Lines 追加到 `failover.log`）
//! - 切换历史（最多保留 100 条）

use crate::ha::{HaConfig, NodeRole, NodeState, NodeStateChange, SharedStore, SyncManager};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// 切换历史最大保留条数
const MAX_HISTORY: usize = 100;

/// Failover 状态机状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailoverState {
    #[default]
    Standby,
    TakingOver,
    Active,
    FailingBack,
    Failed,
}

impl FailoverState {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailoverState::Standby => "standby",
            FailoverState::TakingOver => "taking_over",
            FailoverState::Active => "active",
            FailoverState::FailingBack => "failing_back",
            FailoverState::Failed => "failed",
        }
    }
}

/// 恢复策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPolicy {
    #[default]
    AutoPreferPrimary,
    Manual,
}

/// Failover 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    pub vip: String,
    #[serde(default = "default_vip_interface")]
    pub vip_interface: String,
    #[serde(default = "default_true")]
    pub cleanup_arp: bool,
    #[serde(default = "default_takeover_timeout_ms")]
    pub takeover_timeout_ms: u64,
    #[serde(default)]
    pub recovery_policy: RecoveryPolicy,
}

fn default_vip_interface() -> String {
    "eth0".to_string()
}
fn default_true() -> bool {
    true
}
fn default_takeover_timeout_ms() -> u64 {
    3000
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            vip: String::new(),
            vip_interface: default_vip_interface(),
            cleanup_arp: true,
            takeover_timeout_ms: default_takeover_timeout_ms(),
            recovery_policy: RecoveryPolicy::default(),
        }
    }
}

/// 切换记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverRecord {
    pub timestamp: i64,
    pub from_state: FailoverState,
    pub to_state: FailoverState,
    pub reason: String,
    pub duration_ms: u64,
    pub result: String,
    pub node_id: String,
}

/// Failover 状态查询结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverStatus {
    pub current_state: FailoverState,
    pub role: NodeRole,
    pub vip: Option<String>,
    pub is_readonly: bool,
    pub last_failover: Option<FailoverRecord>,
}

/// Failover 错误
#[derive(Debug, thiserror::Error)]
pub enum FailoverError {
    #[error("IP takeover failed: {0}")]
    IpTakeoverFailed(String),
    #[error("service activation failed: {0}")]
    ServiceActivationFailed(String),
    #[error("operation timeout: {0}")]
    Timeout(String),
    #[error("already active")]
    AlreadyActive,
    #[error("not primary candidate: {0}")]
    NotPrimaryCandidate(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// HA 事件（v0.26.0 — Task 3）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaEvent {
    HaDegraded,
    HaRecovered,
    HaTakeover,
    HaDrillCompleted,
}

impl HaEvent {
    pub fn publish(&self) {
        match self {
            HaEvent::HaDegraded => tracing::warn!("HA 事件：进入降级模式（备节点只读）"),
            HaEvent::HaRecovered => tracing::info!("HA 事件：恢复完成"),
            HaEvent::HaTakeover => tracing::info!("HA 事件：服务接管"),
            HaEvent::HaDrillCompleted => tracing::info!("HA 事件：灾备演练完成"),
        }
    }
}

/// Failover 切换引擎
pub struct FailoverEngine {
    config: HaConfig,
    store: Arc<SharedStore>,
    state: Arc<RwLock<FailoverState>>,
    history: Arc<RwLock<VecDeque<FailoverRecord>>>,
    failover_config: FailoverConfig,
    node_id: String,
    sync_manager: Option<Arc<SyncManager>>,
    last_failover: Arc<RwLock<Option<FailoverRecord>>>,
}

impl FailoverEngine {
    /// 创建 FailoverEngine，初始状态为 Standby
    pub fn new(
        config: HaConfig,
        store: Arc<SharedStore>,
        failover_config: FailoverConfig,
    ) -> Self {
        let node_id = config.node_id.clone();
        Self {
            config,
            store,
            state: Arc::new(RwLock::new(FailoverState::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            failover_config,
            node_id,
            sync_manager: None,
            last_failover: Arc::new(RwLock::new(None)),
        }
    }

    /// 设置同步管理器（builder 方法）
    pub fn with_sync_manager(mut self, sync_manager: Arc<SyncManager>) -> Self {
        self.sync_manager = Some(sync_manager);
        self
    }

    pub fn config(&self) -> &HaConfig {
        &self.config
    }

    pub fn failover_config(&self) -> &FailoverConfig {
        &self.failover_config
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn current_state(&self) -> FailoverState {
        *self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    /// 监听节点状态变更，自动触发 failover/recovery
    pub fn on_node_state_change(&self, change: &NodeStateChange) {
        let current = self.current_state();
        match (current, change.new_state) {
            (FailoverState::Standby, NodeState::Dead) => {
                tracing::warn!(node_id = %change.node_id, "对端节点 Dead，触发 failover");
                if let Err(e) = self.trigger_failover(&format!("peer {} dead", change.node_id)) {
                    tracing::error!(error = %e, "自动 failover 失败");
                }
            }
            (FailoverState::Active, NodeState::Alive) => {
                tracing::info!(node_id = %change.node_id, "对端节点 Alive，触发 recovery");
                if let Err(e) = self.trigger_recovery() {
                    tracing::error!(error = %e, "自动 recovery 失败");
                }
            }
            _ => {
                tracing::debug!(current = ?current, new_state = ?change.new_state, "忽略节点状态变更");
            }
        }
    }

    /// 触发 failover（备 → 主切换）
    ///
    /// 状态转换：Standby → TakingOver → Active
    pub fn trigger_failover(&self, reason: &str) -> Result<FailoverRecord, FailoverError> {
        let start = std::time::Instant::now();
        let from_state = self.current_state();

        if from_state == FailoverState::Active {
            return Err(FailoverError::AlreadyActive);
        }
        if from_state != FailoverState::Standby {
            return Err(FailoverError::NotPrimaryCandidate(format!(
                "当前状态为 {:?}，只有 Standby 状态可触发 failover",
                from_state
            )));
        }

        self.set_state(FailoverState::TakingOver);
        tracing::info!(reason = %reason, "开始接管");

        if let Err(e) = self.takeover_vip() {
            self.set_state(FailoverState::Failed);
            let record = FailoverRecord {
                timestamp: Utc::now().timestamp_millis(),
                from_state,
                to_state: FailoverState::Failed,
                reason: reason.to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
                result: "failed".to_string(),
                node_id: self.node_id.clone(),
            };
            self.log_failover(&record);
            self.record_history(record.clone());
            return Err(e);
        }

        self.notify_service_takeover();
        self.store.update_role(NodeRole::Primary);
        self.store.set_readonly(false);
        self.set_state(FailoverState::Active);
        tracing::info!("接管完成，状态转为 Active");

        let record = FailoverRecord {
            timestamp: Utc::now().timestamp_millis(),
            from_state,
            to_state: FailoverState::Active,
            reason: reason.to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            result: "success".to_string(),
            node_id: self.node_id.clone(),
        };
        self.log_failover(&record);
        self.record_history(record.clone());
        Ok(record)
    }

    /// 触发恢复（主 → 备回切）
    pub fn trigger_recovery(&self) -> Result<FailoverRecord, FailoverError> {
        let start = std::time::Instant::now();
        let from_state = self.current_state();

        if from_state != FailoverState::Active {
            return Err(FailoverError::NotPrimaryCandidate(format!(
                "当前状态为 {:?}，只有 Active 状态可触发 recovery",
                from_state
            )));
        }

        self.set_state(FailoverState::FailingBack);
        tracing::info!("开始回切");

        let (to_state, result) = match self.failover_config.recovery_policy {
            RecoveryPolicy::AutoPreferPrimary => {
                self.store.update_role(NodeRole::Secondary);
                self.store.set_readonly(true);
                HaEvent::HaDegraded.publish();

                if let Err(e) = self.release_vip() {
                    tracing::warn!(error = %e, "释放 VIP 失败（回切继续）");
                }

                if let Some(sync) = &self.sync_manager {
                    if let Err(e) = sync.request_incremental_sync(0) {
                        tracing::warn!(error = %e, "请求增量同步失败");
                    }
                }

                self.set_state(FailoverState::Standby);
                HaEvent::HaRecovered.publish();
                tracing::info!("回切完成，状态转为 Standby");
                (FailoverState::Standby, "success")
            }
            RecoveryPolicy::Manual => {
                self.set_state(FailoverState::Active);
                tracing::info!("手动恢复策略，保持 Active 状态");
                (FailoverState::Active, "manual")
            }
        };

        let record = FailoverRecord {
            timestamp: Utc::now().timestamp_millis(),
            from_state,
            to_state,
            reason: "recovery".to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            result: result.to_string(),
            node_id: self.node_id.clone(),
        };
        self.log_failover(&record);
        self.record_history(record.clone());
        Ok(record)
    }

    /// 接管 VIP（Linux only）
    #[cfg(target_os = "linux")]
    pub fn takeover_vip(&self) -> Result<(), FailoverError> {
        use std::process::Command;
        let vip = &self.failover_config.vip;
        let iface = &self.failover_config.vip_interface;
        tracing::info!(vip = %vip, interface = %iface, "接管 VIP");

        let output = Command::new("ip")
            .args(["addr", "add", vip, "dev", iface])
            .output()
            .map_err(|e| FailoverError::IpTakeoverFailed(format!("执行 ip 命令失败: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(FailoverError::IpTakeoverFailed(format!(
                "ip addr add 失败: {}",
                stderr.trim()
            )));
        }

        if self.failover_config.cleanup_arp {
            let _ = Command::new("arping").args(["-U", "-c", "3", vip]).output();
            tracing::debug!(vip = %vip, "已发送 ARP 广播");
        }
        Ok(())
    }

    /// 接管 VIP（非 Linux 平台 stub）
    #[cfg(not(target_os = "linux"))]
    pub fn takeover_vip(&self) -> Result<(), FailoverError> {
        tracing::debug!(vip = %self.failover_config.vip, "非 Linux 平台，跳过 VIP 接管");
        Ok(())
    }

    /// 释放 VIP（Linux only）
    #[cfg(target_os = "linux")]
    pub fn release_vip(&self) -> Result<(), FailoverError> {
        use std::process::Command;
        let vip = &self.failover_config.vip;
        let iface = &self.failover_config.vip_interface;
        tracing::info!(vip = %vip, interface = %iface, "释放 VIP");

        let output = Command::new("ip")
            .args(["addr", "del", vip, "dev", iface])
            .output()
            .map_err(|e| FailoverError::IpTakeoverFailed(format!("执行 ip 命令失败: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(FailoverError::IpTakeoverFailed(format!(
                "ip addr del 失败: {}",
                stderr.trim()
            )));
        }
        Ok(())
    }

    /// 释放 VIP（非 Linux 平台 stub）
    #[cfg(not(target_os = "linux"))]
    pub fn release_vip(&self) -> Result<(), FailoverError> {
        tracing::debug!(vip = %self.failover_config.vip, "非 Linux 平台，跳过 VIP 释放");
        Ok(())
    }

    /// 通知服务接管（发布 HaTakeover 事件）
    pub fn notify_service_takeover(&self) {
        HaEvent::HaTakeover.publish();
    }

    /// 记录切换日志（JSON Lines 追加到 failover.log）
    pub fn log_failover(&self, record: &FailoverRecord) {
        let log_path = self.failover_log_path();
        if let Some(parent) = log_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "创建日志目录失败");
                return;
            }
        }
        match serde_json::to_string(record) {
            Ok(line) => {
                if let Err(e) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .and_then(|mut f| writeln!(f, "{}", line))
                {
                    tracing::warn!(error = %e, "写入 failover 日志失败");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "序列化 FailoverRecord 失败");
            }
        }
    }

    /// 返回切换历史（按时间倒序，最新在前）
    pub fn history(&self) -> Vec<FailoverRecord> {
        let history = self.history.read().unwrap_or_else(|e| e.into_inner());
        history.iter().rev().cloned().collect()
    }

    /// 返回当前 failover 状态
    pub fn status(&self) -> FailoverStatus {
        let current_state = self.current_state();
        let vip = if current_state == FailoverState::Active {
            Some(self.failover_config.vip.clone())
        } else {
            None
        };
        FailoverStatus {
            current_state,
            role: self.store.local_role(),
            vip,
            is_readonly: self.store.is_readonly(),
            last_failover: self
                .last_failover
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    fn set_state(&self, new_state: FailoverState) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        tracing::info!(from = ?*state, to = ?new_state, "状态转换");
        *state = new_state;
    }

    fn record_history(&self, record: FailoverRecord) {
        let mut history = self.history.write().unwrap_or_else(|e| e.into_inner());
        if history.len() >= MAX_HISTORY {
            history.pop_front();
        }
        history.push_back(record.clone());
        let mut last = self
            .last_failover
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *last = Some(record);
    }

    fn failover_log_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            PathBuf::from("/var/log/eneros/failover.log")
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::env::temp_dir().join("eneros").join("failover.log")
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::{ConflictResolution, FencingStrategy, StorageQuota, SyncScope};

    fn test_config() -> HaConfig {
        HaConfig {
            node_id: "node-2".to_string(),
            role: NodeRole::Secondary,
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
        }
    }

    fn test_store() -> Arc<SharedStore> {
        Arc::new(SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::default(),
            StorageQuota::default(),
        ))
    }

    fn test_failover_config() -> FailoverConfig {
        FailoverConfig {
            vip: "192.168.1.100".to_string(),
            vip_interface: "eth0".to_string(),
            cleanup_arp: false,
            takeover_timeout_ms: 3000,
            recovery_policy: RecoveryPolicy::AutoPreferPrimary,
        }
    }

    fn test_engine() -> FailoverEngine {
        FailoverEngine::new(test_config(), test_store(), test_failover_config())
    }

    #[test]
    fn test_failover_state_default() {
        assert_eq!(FailoverState::default(), FailoverState::Standby);
    }

    #[test]
    fn test_failover_state_as_str() {
        assert_eq!(FailoverState::Standby.as_str(), "standby");
        assert_eq!(FailoverState::TakingOver.as_str(), "taking_over");
        assert_eq!(FailoverState::Active.as_str(), "active");
        assert_eq!(FailoverState::FailingBack.as_str(), "failing_back");
        assert_eq!(FailoverState::Failed.as_str(), "failed");
    }

    #[test]
    fn test_failover_state_serde() {
        let json = serde_json::to_string(&FailoverState::TakingOver).expect("serialize");
        assert_eq!(json, "\"taking_over\"");
        let state: FailoverState = serde_json::from_str("\"active\"").expect("deserialize");
        assert_eq!(state, FailoverState::Active);
    }

    #[test]
    fn test_recovery_policy_default() {
        assert_eq!(RecoveryPolicy::default(), RecoveryPolicy::AutoPreferPrimary);
    }

    #[test]
    fn test_recovery_policy_serde() {
        let json = serde_json::to_string(&RecoveryPolicy::Manual).expect("serialize");
        assert_eq!(json, "\"manual\"");
        let policy: RecoveryPolicy =
            serde_json::from_str("\"auto_prefer_primary\"").expect("deserialize");
        assert_eq!(policy, RecoveryPolicy::AutoPreferPrimary);
    }

    #[test]
    fn test_failover_config_default() {
        let config = FailoverConfig::default();
        assert!(config.vip.is_empty());
        assert_eq!(config.vip_interface, "eth0");
        assert!(config.cleanup_arp);
        assert_eq!(config.takeover_timeout_ms, 3000);
        assert_eq!(config.recovery_policy, RecoveryPolicy::AutoPreferPrimary);
    }

    #[test]
    fn test_failover_engine_new() {
        let engine = test_engine();
        assert_eq!(engine.current_state(), FailoverState::Standby);
        assert_eq!(engine.node_id(), "node-2");
        assert!(engine.history().is_empty());
        let status = engine.status();
        assert_eq!(status.current_state, FailoverState::Standby);
        assert_eq!(status.role, NodeRole::Secondary);
        assert!(status.vip.is_none());
        assert!(status.is_readonly);
        assert!(status.last_failover.is_none());
    }

    #[test]
    fn test_with_sync_manager() {
        let mut config = test_config();
        config.sync_port = 0;
        let store = test_store();
        let sync_manager = Arc::new(
            SyncManager::new(config.clone(), Some(store.clone())).expect("create sync manager"),
        );
        let engine = FailoverEngine::new(config, store, test_failover_config())
            .with_sync_manager(sync_manager);
        assert_eq!(engine.current_state(), FailoverState::Standby);
    }

    #[test]
    fn test_trigger_failover_success() {
        let engine = test_engine();
        assert!(engine.store.is_readonly());

        let record = engine
            .trigger_failover("manual test")
            .expect("failover should succeed");

        assert_eq!(engine.current_state(), FailoverState::Active);
        assert!(!engine.store.is_readonly());
        assert_eq!(engine.store.local_role(), NodeRole::Primary);
        assert_eq!(record.from_state, FailoverState::Standby);
        assert_eq!(record.to_state, FailoverState::Active);
        assert_eq!(record.result, "success");
        assert_eq!(record.reason, "manual test");
    }

    #[test]
    fn test_trigger_failover_already_active() {
        let engine = test_engine();
        engine
            .trigger_failover("first")
            .expect("first failover should succeed");

        let result = engine.trigger_failover("second");
        assert!(matches!(result, Err(FailoverError::AlreadyActive)));
    }

    #[test]
    fn test_trigger_recovery_auto() {
        let engine = test_engine();
        engine.trigger_failover("setup").expect("failover to active");
        assert_eq!(engine.current_state(), FailoverState::Active);
        assert!(!engine.store.is_readonly());

        let record = engine.trigger_recovery().expect("recovery should succeed");

        assert_eq!(engine.current_state(), FailoverState::Standby);
        assert!(engine.store.is_readonly());
        assert_eq!(engine.store.local_role(), NodeRole::Secondary);
        assert_eq!(record.from_state, FailoverState::Active);
        assert_eq!(record.to_state, FailoverState::Standby);
        assert_eq!(record.result, "success");
    }

    #[test]
    fn test_trigger_recovery_manual() {
        let config = test_config();
        let store = test_store();
        let failover_config = FailoverConfig {
            recovery_policy: RecoveryPolicy::Manual,
            ..test_failover_config()
        };
        let engine = FailoverEngine::new(config, store, failover_config);

        engine.trigger_failover("setup").expect("failover to active");
        assert_eq!(engine.current_state(), FailoverState::Active);

        let record = engine.trigger_recovery().expect("recovery should succeed");

        assert_eq!(engine.current_state(), FailoverState::Active);
        assert_eq!(record.to_state, FailoverState::Active);
        assert_eq!(record.result, "manual");
    }

    #[test]
    fn test_trigger_recovery_not_active() {
        let engine = test_engine();
        let result = engine.trigger_recovery();
        assert!(matches!(result, Err(FailoverError::NotPrimaryCandidate(_))));
    }

    #[test]
    fn test_takeover_vip_non_linux() {
        let engine = test_engine();
        let result = engine.takeover_vip();
        assert!(result.is_ok());
    }

    #[test]
    fn test_release_vip_non_linux() {
        let engine = test_engine();
        let result = engine.release_vip();
        assert!(result.is_ok());
    }

    #[test]
    fn test_on_node_state_change_dead_triggers_failover() {
        let engine = test_engine();
        let change = NodeStateChange {
            node_id: "node-1".to_string(),
            old_state: NodeState::Alive,
            new_state: NodeState::Dead,
            timestamp: 1700000000000,
        };
        engine.on_node_state_change(&change);
        assert_eq!(engine.current_state(), FailoverState::Active);
    }

    #[test]
    fn test_on_node_state_change_alive_triggers_recovery() {
        let engine = test_engine();
        engine.trigger_failover("setup").expect("failover");

        let change = NodeStateChange {
            node_id: "node-1".to_string(),
            old_state: NodeState::Dead,
            new_state: NodeState::Alive,
            timestamp: 1700000000000,
        };
        engine.on_node_state_change(&change);
        assert_eq!(engine.current_state(), FailoverState::Standby);
    }

    #[test]
    fn test_on_node_state_change_ignored() {
        let engine = test_engine();
        let change = NodeStateChange {
            node_id: "node-1".to_string(),
            old_state: NodeState::Dead,
            new_state: NodeState::Alive,
            timestamp: 1700000000000,
        };
        engine.on_node_state_change(&change);
        assert_eq!(engine.current_state(), FailoverState::Standby);
    }

    #[test]
    fn test_history_empty() {
        let engine = test_engine();
        assert!(engine.history().is_empty());
    }

    #[test]
    fn test_history_after_failover() {
        let engine = test_engine();
        engine.trigger_failover("test").expect("failover");
        let history = engine.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].to_state, FailoverState::Active);
        assert_eq!(history[0].result, "success");
    }

    #[test]
    fn test_history_after_failover_and_recovery() {
        let engine = test_engine();
        engine.trigger_failover("test").expect("failover");
        engine.trigger_recovery().expect("recovery");
        let history = engine.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].to_state, FailoverState::Standby);
        assert_eq!(history[1].to_state, FailoverState::Active);
    }

    #[test]
    fn test_history_max_100() {
        let engine = test_engine();
        for i in 0..150 {
            engine
                .trigger_failover(&format!("failover-{}", i))
                .expect("failover");
            engine.trigger_recovery().expect("recovery");
        }
        let history = engine.history();
        assert_eq!(history.len(), MAX_HISTORY);
    }

    #[test]
    fn test_status_standby() {
        let engine = test_engine();
        let status = engine.status();
        assert_eq!(status.current_state, FailoverState::Standby);
        assert_eq!(status.role, NodeRole::Secondary);
        assert!(status.vip.is_none());
        assert!(status.is_readonly);
        assert!(status.last_failover.is_none());
    }

    #[test]
    fn test_status_active() {
        let engine = test_engine();
        engine.trigger_failover("test").expect("failover");
        let status = engine.status();
        assert_eq!(status.current_state, FailoverState::Active);
        assert_eq!(status.role, NodeRole::Primary);
        assert_eq!(status.vip, Some("192.168.1.100".to_string()));
        assert!(!status.is_readonly);
        assert!(status.last_failover.is_some());
    }

    #[test]
    fn test_log_failover() {
        let engine = test_engine();
        let record = FailoverRecord {
            timestamp: Utc::now().timestamp_millis(),
            from_state: FailoverState::Standby,
            to_state: FailoverState::Active,
            reason: "test log".to_string(),
            duration_ms: 100,
            result: "success".to_string(),
            node_id: "node-2".to_string(),
        };
        engine.log_failover(&record);

        let log_path = engine.failover_log_path();
        assert!(log_path.exists(), "日志文件应存在");
        let content = std::fs::read_to_string(&log_path).expect("read log");
        assert!(content.contains("test log"), "日志应包含 reason");
        assert!(content.contains("success"), "日志应包含 result");
    }

    #[test]
    fn test_ha_event_publish() {
        HaEvent::HaDegraded.publish();
        HaEvent::HaRecovered.publish();
        HaEvent::HaTakeover.publish();
        HaEvent::HaDrillCompleted.publish();
    }

    #[test]
    fn test_notify_service_takeover() {
        let engine = test_engine();
        engine.notify_service_takeover();
    }

    #[test]
    fn test_failover_record_serde() {
        let record = FailoverRecord {
            timestamp: 1700000000000,
            from_state: FailoverState::Standby,
            to_state: FailoverState::Active,
            reason: "peer dead".to_string(),
            duration_ms: 500,
            result: "success".to_string(),
            node_id: "node-2".to_string(),
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let deserialized: FailoverRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.timestamp, record.timestamp);
        assert_eq!(deserialized.from_state, record.from_state);
        assert_eq!(deserialized.to_state, record.to_state);
        assert_eq!(deserialized.reason, record.reason);
        assert_eq!(deserialized.duration_ms, record.duration_ms);
        assert_eq!(deserialized.result, record.result);
        assert_eq!(deserialized.node_id, record.node_id);
    }

    #[test]
    fn test_failover_status_serde() {
        let status = FailoverStatus {
            current_state: FailoverState::Active,
            role: NodeRole::Primary,
            vip: Some("192.168.1.100".to_string()),
            is_readonly: false,
            last_failover: None,
        };
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("\"current_state\":\"active\""));
        assert!(json.contains("\"role\":\"primary\""));
    }

    #[test]
    fn test_failover_config_serde() {
        let config = FailoverConfig {
            vip: "10.0.0.1".to_string(),
            vip_interface: "bond0".to_string(),
            cleanup_arp: false,
            takeover_timeout_ms: 5000,
            recovery_policy: RecoveryPolicy::Manual,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: FailoverConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.vip, "10.0.0.1");
        assert_eq!(deserialized.vip_interface, "bond0");
        assert!(!deserialized.cleanup_arp);
        assert_eq!(deserialized.takeover_timeout_ms, 5000);
        assert_eq!(deserialized.recovery_policy, RecoveryPolicy::Manual);
    }

    #[test]
    fn test_full_failover_recovery_cycle() {
        let engine = test_engine();

        assert_eq!(engine.current_state(), FailoverState::Standby);
        assert_eq!(engine.store.local_role(), NodeRole::Secondary);
        assert!(engine.store.is_readonly());

        engine.trigger_failover("primary dead").expect("failover");
        assert_eq!(engine.current_state(), FailoverState::Active);
        assert_eq!(engine.store.local_role(), NodeRole::Primary);
        assert!(!engine.store.is_readonly());

        engine.trigger_recovery().expect("recovery");
        assert_eq!(engine.current_state(), FailoverState::Standby);
        assert_eq!(engine.store.local_role(), NodeRole::Secondary);
        assert!(engine.store.is_readonly());

        assert_eq!(engine.history().len(), 2);
    }

    // ===== T030-07: 覆盖率补充测试 =====

    /// 验证 `failover_config()` 访问器返回正确的配置引用。
    #[test]
    fn test_failover_config_accessor() {
        let engine = test_engine();
        let config = engine.failover_config();
        // test_failover_config() 设置 vip 为 "192.168.1.100"
        assert_eq!(config.vip, "192.168.1.100");
        assert_eq!(config.vip_interface, "eth0");
        assert!(!config.cleanup_arp);
        assert_eq!(config.takeover_timeout_ms, 3000);
    }

    /// 验证 `node_id()` 访问器返回正确的节点 ID。
    #[test]
    fn test_node_id_accessor() {
        let engine = test_engine();
        // test_config() 设置 node_id 为 "node-2"
        assert_eq!(engine.node_id(), "node-2");
    }

    /// 验证 `config()` 访问器返回 HaConfig 引用。
    #[test]
    fn test_config_accessor() {
        let engine = test_engine();
        let config = engine.config();
        assert_eq!(config.node_id, "node-2");
    }

    /// 验证 `FailoverState::Failed` 的 `as_str()` 返回正确字符串。
    #[test]
    fn test_failover_state_failed_as_str() {
        assert_eq!(FailoverState::Failed.as_str(), "failed");
        assert_eq!(FailoverState::FailingBack.as_str(), "failing_back");
        assert_eq!(FailoverState::TakingOver.as_str(), "taking_over");
    }

    /// 验证 `RecoveryPolicy::Manual` 的 serde 序列化/反序列化。
    #[test]
    fn test_recovery_policy_manual_serde() {
        let policy = RecoveryPolicy::Manual;
        let json = serde_json::to_string(&policy).expect("serialize");
        let deserialized: RecoveryPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(policy, deserialized);
    }
}
