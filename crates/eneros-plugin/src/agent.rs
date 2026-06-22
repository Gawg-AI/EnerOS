//! Agent 策略插件接口
//!
//! 提供 `AgentPlugin` trait 与 `AgentStrategyInstance` trait，用于在不依赖
//! `eneros-agent`（避免循环依赖）的前提下，让第三方插件以动态库形式注册
//! 自定义 Agent 策略（如负荷均衡、自愈、调度等）。
//!
//! 架构关系：
//! - `eneros-plugin`（本 crate）定义插件接口与注册表
//! - `eneros-agent` 定义内置 `AgentStrategy` trait 与 `AgentAction` 枚举
//! - 插件实现 `AgentPlugin`，由加载器注册到 `AgentPluginRegistry`
//! - Agent 子系统在边界处将 `AgentPluginAction` 适配为内部 `AgentAction`
//!
//! 安全约束：
//! - 插件 Agent 的权限上限为 `AuthorityLevel::Operator`，
//!   `Supervisor` 与 `Emergency` 会被 `enforce_authority_limit` 强制降级
//! - 多个插件 Agent 同时响应同一事件时，由 `resolve_conflict` 按优先级排序

use crate::error::PluginError;
use async_trait::async_trait;
use eneros_core::AuthorityLevel;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Agent 插件配置
///
/// 由 Agent 子系统在创建插件 Agent 实例时传入，包含实例化所需的最小信息集。
/// 协议特定或策略特定参数通过 `custom_config`（JSON）传递，避免接口膨胀。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPluginConfig {
    /// Agent 实例 ID（由 Agent 子系统分配，全局唯一）
    pub agent_id: String,
    /// Agent 类型标签（如 "load-balance"、"self-healing"）
    pub agent_type: String,
    /// 周期性 tick 间隔（毫秒），0 表示不进行周期性 tick
    pub tick_interval_ms: u64,
    /// 策略特定配置（JSON），由插件自行解析
    pub custom_config: serde_json::Value,
}

impl Default for AgentPluginConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            agent_type: "custom".to_string(),
            tick_interval_ms: 1000,
            custom_config: serde_json::Value::Null,
        }
    }
}

/// Agent 策略插件 trait
///
/// 插件以动态库形式加载后，需实现此 trait 并通过 C ABI 入口函数
/// `eneros_plugin_create` 返回 `Box<dyn AgentPlugin>`。
///
/// 每个插件代表一类策略（如负荷均衡），可创建多个 Agent 实例
/// （对应不同区域/馈线）。
#[async_trait]
pub trait AgentPlugin: Send + Sync {
    /// 策略名称（全局唯一，用作注册表键）
    fn strategy_name(&self) -> &str;

    /// 策略描述
    fn description(&self) -> &str {
        ""
    }

    /// 请求的权限级别
    ///
    /// 系统会通过 `enforce_authority_limit` 强制降级到 `Operator` 上限，
    /// 插件即使声明 `Emergency` 或 `Supervisor` 也无法获得高于 `Operator` 的权限。
    fn authority_level(&self) -> AuthorityLevel;

    /// 策略优先级
    ///
    /// 用于多插件冲突解决，高优先级插件的动作优先执行。
    fn priority(&self) -> StrategyPriority {
        StrategyPriority::Normal
    }

    /// 创建 Agent 实例
    ///
    /// 每次调用应返回独立的实例（对应一个 Agent 实体），
    /// 实例间状态相互隔离。
    async fn create_agent(
        &self,
        config: &AgentPluginConfig,
    ) -> Result<Box<dyn AgentStrategyInstance>, PluginError>;
}

/// Agent 策略实例（插件创建的 Agent）
///
/// 这是 `eneros_agent::AgentStrategy` 的简化版本，避免 eneros-plugin
/// 依赖 eneros-agent 造成循环依赖。Agent 子系统可在边界处做适配转换。
#[async_trait]
pub trait AgentStrategyInstance: Send + Sync {
    /// Agent ID
    fn agent_id(&self) -> &str;

    /// Agent 类型
    fn agent_type(&self) -> &str;

    /// 处理事件
    ///
    /// 接收外部事件（遥测变位、告警等），返回需要执行的动作列表。
    async fn handle_event(
        &mut self,
        event: &AgentPluginEvent,
    ) -> Result<Vec<AgentPluginAction>, PluginError>;

    /// 周期性 tick
    ///
    /// 由 Agent 调度器按 `tick_interval_ms` 间隔调用，
    /// 返回需要执行的动作列表。
    async fn tick(&mut self) -> Result<Vec<AgentPluginAction>, PluginError>;

    /// tick 间隔（毫秒）
    ///
    /// 默认 1000ms，插件可在创建实例时根据配置覆盖。
    fn tick_interval_ms(&self) -> u64 {
        1000
    }
}

/// 策略优先级
///
/// 用于多插件冲突解决。`Critical` > `High` > `Normal` > `Low`。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum StrategyPriority {
    /// 低优先级
    Low,
    /// 普通优先级（默认）
    #[default]
    Normal,
    /// 高优先级
    High,
    /// 关键优先级（仅用于安全相关策略）
    Critical,
}

/// Agent 插件事件（简化版，避免依赖 eneros-core 的 Event）
///
/// 通过 `event_type` 字符串区分事件类型，payload 由插件自行解析。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPluginEvent {
    /// 事件类型（如 "telemetry_update"、"alarm_raised"）
    pub event_type: String,
    /// 事件负载（JSON）
    pub payload: serde_json::Value,
    /// Unix 毫秒时间戳
    pub timestamp: u64,
}

/// Agent 插件动作（简化版，避免依赖 eneros-agent 的 AgentAction）
///
/// Agent 子系统在边界处将此枚举适配为内部 `AgentAction`。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AgentPluginAction {
    /// 发布事件到事件总线
    PublishEvent {
        /// 事件类型
        event_type: String,
        /// 事件负载（JSON）
        payload: serde_json::Value,
    },
    /// 执行控制命令
    ExecuteCommand {
        /// 命令（JSON，由 Agent 子系统解析为具体 Command）
        command: serde_json::Value,
    },
    /// 记录日志
    LogMessage {
        /// 日志级别（"info"、"warn"、"error" 等）
        level: String,
        /// 日志消息
        message: String,
    },
    /// 空操作
    NoOp,
}

/// Agent 插件注册表
///
/// 线程安全：内部使用 `parking_lot::RwLock` 保护 HashMap，
/// 支持多线程并发注册/查找/注销。
pub struct AgentPluginRegistry {
    plugins: RwLock<HashMap<String, Arc<dyn AgentPlugin>>>,
}

impl AgentPluginRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// 注册 Agent 插件
    ///
    /// 若同名策略已注册，返回 `PluginError::AlreadyLoaded`。
    pub fn register(&self, plugin: Arc<dyn AgentPlugin>) -> Result<(), PluginError> {
        let name = plugin.strategy_name().to_string();
        let mut plugins = self.plugins.write();
        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }
        plugins.insert(name, plugin);
        Ok(())
    }

    /// 注销 Agent 插件
    ///
    /// 若策略未注册，返回 `PluginError::NotLoaded`。
    pub fn unregister(&self, name: &str) -> Result<Arc<dyn AgentPlugin>, PluginError> {
        let mut plugins = self.plugins.write();
        plugins
            .remove(name)
            .ok_or_else(|| PluginError::NotLoaded(name.to_string()))
    }

    /// 查找 Agent 插件
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn AgentPlugin>> {
        self.plugins.read().get(name).cloned()
    }

    /// 列出所有 Agent 插件名称
    pub fn list(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }

    /// 列出所有 Agent 插件（带详情）
    pub fn list_with_info(&self) -> Vec<AgentPluginInfo> {
        self.plugins
            .read()
            .values()
            .map(|p| AgentPluginInfo {
                name: p.strategy_name().to_string(),
                description: p.description().to_string(),
                authority_level: p.authority_level(),
                priority: p.priority(),
            })
            .collect()
    }

    /// 是否包含指定策略
    pub fn contains(&self, name: &str) -> bool {
        self.plugins.read().contains_key(name)
    }

    /// 注册的策略数量
    pub fn count(&self) -> usize {
        self.plugins.read().len()
    }
}

impl Default for AgentPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent 插件信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPluginInfo {
    /// 策略名称
    pub name: String,
    /// 策略描述
    pub description: String,
    /// 请求的权限级别（未经强制降级）
    pub authority_level: AuthorityLevel,
    /// 策略优先级
    pub priority: StrategyPriority,
}

/// 强制权限上限：插件 Agent 权限最高为 `Operator`
///
/// 出于安全考虑，插件无法获得 `Supervisor` 或 `Emergency` 权限。
/// 即使插件声明了更高权限，系统也会强制降级到 `Operator`。
pub fn enforce_authority_limit(plugin: &dyn AgentPlugin) -> AuthorityLevel {
    let requested = plugin.authority_level();
    match requested {
        AuthorityLevel::Emergency | AuthorityLevel::Supervisor => AuthorityLevel::Operator,
        other => other,
    }
}

/// 按优先级解决冲突：返回按优先级排序的插件列表（高优先级在前）
///
/// 稳定排序：相同优先级的插件保持原始相对顺序。
pub fn resolve_conflict(plugins: &[Arc<dyn AgentPlugin>]) -> Vec<Arc<dyn AgentPlugin>> {
    let mut sorted: Vec<_> = plugins.to_vec();
    // sort_by_key 为稳定排序，相同优先级保持原序；
    // 使用 Reverse 使高优先级（Ord 中更大）排在前面
    sorted.sort_by_key(|p| std::cmp::Reverse(p.priority()));
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 Mock Agent 插件
    struct MockAgentPlugin {
        name: String,
        authority: AuthorityLevel,
        priority: StrategyPriority,
    }

    #[async_trait]
    impl AgentPlugin for MockAgentPlugin {
        fn strategy_name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "mock agent plugin for testing"
        }

        fn authority_level(&self) -> AuthorityLevel {
            self.authority
        }

        fn priority(&self) -> StrategyPriority {
            self.priority
        }

        async fn create_agent(
            &self,
            config: &AgentPluginConfig,
        ) -> Result<Box<dyn AgentStrategyInstance>, PluginError> {
            Ok(Box::new(MockAgentInstance {
                agent_id: config.agent_id.clone(),
                agent_type: config.agent_type.clone(),
                tick_interval_ms: config.tick_interval_ms,
            }))
        }
    }

    /// 测试用 Mock Agent 实例
    struct MockAgentInstance {
        agent_id: String,
        agent_type: String,
        tick_interval_ms: u64,
    }

    #[async_trait]
    impl AgentStrategyInstance for MockAgentInstance {
        fn agent_id(&self) -> &str {
            &self.agent_id
        }

        fn agent_type(&self) -> &str {
            &self.agent_type
        }

        async fn handle_event(
            &mut self,
            _event: &AgentPluginEvent,
        ) -> Result<Vec<AgentPluginAction>, PluginError> {
            Ok(vec![AgentPluginAction::NoOp])
        }

        async fn tick(&mut self) -> Result<Vec<AgentPluginAction>, PluginError> {
            Ok(vec![AgentPluginAction::LogMessage {
                level: "info".to_string(),
                message: "mock tick".to_string(),
            }])
        }

        fn tick_interval_ms(&self) -> u64 {
            self.tick_interval_ms
        }
    }

    /// 构造默认 Mock 插件（Normal 优先级、Operator 权限）
    fn make_plugin(name: &str) -> Arc<dyn AgentPlugin> {
        Arc::new(MockAgentPlugin {
            name: name.to_string(),
            authority: AuthorityLevel::Operator,
            priority: StrategyPriority::Normal,
        })
    }

    /// 构造可定制权限与优先级的 Mock 插件
    fn make_plugin_with(
        name: &str,
        authority: AuthorityLevel,
        priority: StrategyPriority,
    ) -> Arc<dyn AgentPlugin> {
        Arc::new(MockAgentPlugin {
            name: name.to_string(),
            authority,
            priority,
        })
    }

    #[test]
    fn test_agent_plugin_config_default() {
        let cfg = AgentPluginConfig::default();
        assert!(cfg.agent_id.is_empty());
        assert_eq!(cfg.agent_type, "custom");
        assert_eq!(cfg.tick_interval_ms, 1000);
        assert!(cfg.custom_config.is_null());
    }

    #[test]
    fn test_strategy_priority_ordering() {
        // Low < Normal < High < Critical
        assert!(StrategyPriority::Low < StrategyPriority::Normal);
        assert!(StrategyPriority::Normal < StrategyPriority::High);
        assert!(StrategyPriority::High < StrategyPriority::Critical);
        assert!(StrategyPriority::Low < StrategyPriority::Critical);
    }

    #[test]
    fn test_registry_register_unregister() {
        let registry = AgentPluginRegistry::new();
        let plugin = make_plugin("load-balance");
        assert!(registry.register(plugin).is_ok());
        assert!(registry.contains("load-balance"));

        let unregistered = registry.unregister("load-balance");
        assert!(unregistered.is_ok());
        assert!(!registry.contains("load-balance"));
    }

    #[test]
    fn test_registry_lookup() {
        let registry = AgentPluginRegistry::new();
        assert!(registry.lookup("load-balance").is_none());

        let plugin = make_plugin("load-balance");
        registry.register(plugin).unwrap();
        assert!(registry.lookup("load-balance").is_some());
        assert!(registry.lookup("self-healing").is_none());
    }

    #[test]
    fn test_registry_list() {
        let registry = AgentPluginRegistry::new();
        registry.register(make_plugin("load-balance")).unwrap();
        registry.register(make_plugin("self-healing")).unwrap();

        let mut names = registry.list();
        names.sort();
        assert_eq!(
            names,
            vec!["load-balance".to_string(), "self-healing".to_string()]
        );
    }

    #[test]
    fn test_registry_already_loaded() {
        let registry = AgentPluginRegistry::new();
        registry.register(make_plugin("load-balance")).unwrap();
        let err = registry.register(make_plugin("load-balance")).unwrap_err();
        assert!(matches!(err, PluginError::AlreadyLoaded(_)));
        assert_eq!(err.to_string(), "plugin already loaded: load-balance");
    }

    #[test]
    fn test_registry_not_loaded() {
        let registry = AgentPluginRegistry::new();
        // unregister 返回 Arc<dyn AgentPlugin>，未实现 Debug，
        // 故用 .err().unwrap() 而非 .unwrap_err() 提取错误
        let err = registry.unregister("load-balance").err().unwrap();
        assert!(matches!(err, PluginError::NotLoaded(_)));
        assert_eq!(err.to_string(), "plugin not loaded: load-balance");
    }

    #[test]
    fn test_enforce_authority_limit_emergency() {
        let plugin = make_plugin_with(
            "emergency-strategy",
            AuthorityLevel::Emergency,
            StrategyPriority::Normal,
        );
        let enforced = enforce_authority_limit(&*plugin);
        assert_eq!(enforced, AuthorityLevel::Operator);
    }

    #[test]
    fn test_enforce_authority_limit_supervisor() {
        let plugin = make_plugin_with(
            "supervisor-strategy",
            AuthorityLevel::Supervisor,
            StrategyPriority::Normal,
        );
        let enforced = enforce_authority_limit(&*plugin);
        assert_eq!(enforced, AuthorityLevel::Operator);
    }

    #[test]
    fn test_enforce_authority_limit_operator() {
        let plugin = make_plugin_with(
            "operator-strategy",
            AuthorityLevel::Operator,
            StrategyPriority::Normal,
        );
        let enforced = enforce_authority_limit(&*plugin);
        assert_eq!(enforced, AuthorityLevel::Operator);
    }

    #[test]
    fn test_enforce_authority_limit_observer() {
        let plugin = make_plugin_with(
            "observer-strategy",
            AuthorityLevel::Observer,
            StrategyPriority::Normal,
        );
        let enforced = enforce_authority_limit(&*plugin);
        assert_eq!(enforced, AuthorityLevel::Observer);
    }

    #[test]
    fn test_resolve_conflict_orders_by_priority() {
        let low = make_plugin_with("low", AuthorityLevel::Operator, StrategyPriority::Low);
        let critical = make_plugin_with(
            "critical",
            AuthorityLevel::Operator,
            StrategyPriority::Critical,
        );
        let normal = make_plugin_with(
            "normal",
            AuthorityLevel::Operator,
            StrategyPriority::Normal,
        );
        let high = make_plugin_with("high", AuthorityLevel::Operator, StrategyPriority::High);

        let ordered = resolve_conflict(&[low.clone(), critical.clone(), normal, high]);
        // 高优先级在前
        assert_eq!(ordered.len(), 4);
        assert_eq!(ordered[0].strategy_name(), "critical");
        assert_eq!(ordered[1].strategy_name(), "high");
        assert_eq!(ordered[2].strategy_name(), "normal");
        assert_eq!(ordered[3].strategy_name(), "low");
    }

    #[test]
    fn test_agent_plugin_info() {
        let registry = AgentPluginRegistry::new();
        registry
            .register(make_plugin_with(
                "load-balance",
                AuthorityLevel::Operator,
                StrategyPriority::High,
            ))
            .unwrap();
        registry
            .register(make_plugin_with(
                "observer",
                AuthorityLevel::Observer,
                StrategyPriority::Low,
            ))
            .unwrap();

        let mut infos = registry.list_with_info();
        infos.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "load-balance");
        assert_eq!(infos[0].description, "mock agent plugin for testing");
        assert_eq!(infos[0].authority_level, AuthorityLevel::Operator);
        assert_eq!(infos[0].priority, StrategyPriority::High);

        assert_eq!(infos[1].name, "observer");
        assert_eq!(infos[1].authority_level, AuthorityLevel::Observer);
        assert_eq!(infos[1].priority, StrategyPriority::Low);
    }

    #[tokio::test]
    async fn test_mock_agent_instance_lifecycle() {
        let plugin = make_plugin("load-balance");
        let config = AgentPluginConfig {
            agent_id: "agent-001".to_string(),
            agent_type: "load-balance".to_string(),
            tick_interval_ms: 500,
            custom_config: serde_json::json!({"zone": "z1"}),
        };
        let mut agent = plugin.create_agent(&config).await.unwrap();
        assert_eq!(agent.agent_id(), "agent-001");
        assert_eq!(agent.agent_type(), "load-balance");
        assert_eq!(agent.tick_interval_ms(), 500);

        let event = AgentPluginEvent {
            event_type: "telemetry_update".to_string(),
            payload: serde_json::json!({"v": 220.0}),
            timestamp: 1000,
        };
        let actions = agent.handle_event(&event).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentPluginAction::NoOp));

        let tick_actions = agent.tick().await.unwrap();
        assert_eq!(tick_actions.len(), 1);
        match &tick_actions[0] {
            AgentPluginAction::LogMessage { level, message } => {
                assert_eq!(level, "info");
                assert_eq!(message, "mock tick");
            }
            other => panic!("expected LogMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_agent_plugin_config_serde() {
        let cfg = AgentPluginConfig {
            agent_id: "agent-002".to_string(),
            agent_type: "self-healing".to_string(),
            tick_interval_ms: 2000,
            custom_config: serde_json::json!({"threshold": 0.8}),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let de: AgentPluginConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.agent_id, "agent-002");
        assert_eq!(de.agent_type, "self-healing");
        assert_eq!(de.tick_interval_ms, 2000);
    }

    #[test]
    fn test_strategy_priority_default() {
        assert_eq!(StrategyPriority::default(), StrategyPriority::Normal);
    }

    #[test]
    fn test_agent_plugin_action_serde() {
        let action = AgentPluginAction::LogMessage {
            level: "warn".to_string(),
            message: "high load".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let de: AgentPluginAction = serde_json::from_str(&json).unwrap();
        match de {
            AgentPluginAction::LogMessage { level, message } => {
                assert_eq!(level, "warn");
                assert_eq!(message, "high load");
            }
            other => panic!("expected LogMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_agent_plugin_event_serde() {
        let event = AgentPluginEvent {
            event_type: "alarm_raised".to_string(),
            payload: serde_json::json!({"severity": "high"}),
            timestamp: 12345,
        };
        let json = serde_json::to_string(&event).unwrap();
        let de: AgentPluginEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(de.event_type, "alarm_raised");
        assert_eq!(de.timestamp, 12345);
    }

    #[test]
    fn test_registry_count() {
        let registry = AgentPluginRegistry::new();
        assert_eq!(registry.count(), 0);
        registry.register(make_plugin("a")).unwrap();
        assert_eq!(registry.count(), 1);
        registry.register(make_plugin("b")).unwrap();
        assert_eq!(registry.count(), 2);
        registry.unregister("a").unwrap();
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_registry_contains() {
        let registry = AgentPluginRegistry::new();
        assert!(!registry.contains("a"));
        registry.register(make_plugin("a")).unwrap();
        assert!(registry.contains("a"));
        assert!(!registry.contains("b"));
    }
}
