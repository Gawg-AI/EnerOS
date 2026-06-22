//! 协议适配器插件接口
//!
//! 提供 `ProtocolPlugin` trait 与 `ProtocolAdapterInstance` trait，用于在不依赖
//! `eneros-device`（避免循环依赖）的前提下，让第三方插件以动态库形式注册
//! 自定义电力协议适配器（如 IEC 60870-5-103、CDT 等）。
//!
//! 架构关系：
//! - `eneros-plugin`（本 crate）定义插件接口与注册表
//! - `eneros-device` 定义内置 `ProtocolAdapter` trait 与 `ProtocolType` 枚举
//! - 插件实现 `ProtocolPlugin`，由加载器注册到 `ProtocolPluginRegistry`
//! - 设备层通过 `ProtocolType::Custom(name)` 引用插件协议

use crate::error::PluginError;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// 协议适配器插件 trait
///
/// 插件以动态库形式加载后，需实现此 trait 并通过 C ABI 入口函数
/// `eneros_plugin_create` 返回 `Box<dyn ProtocolPlugin>`。
#[async_trait]
pub trait ProtocolPlugin: Send + Sync {
    /// 协议名称（如 "iec103"）
    fn protocol_name(&self) -> &str;

    /// 协议类型字符串（如 "custom:iec103"）
    ///
    /// 与 `eneros_device::ProtocolType::Custom(name)` 的 serde 表示一致，
    /// 便于设备层通过协议类型字符串查找对应插件。
    fn protocol_type_str(&self) -> String {
        format!("custom:{}", self.protocol_name())
    }

    /// 协议描述
    fn description(&self) -> &str {
        ""
    }

    /// 创建协议适配器实例
    ///
    /// 返回一个 trait object，具体类型由插件实现决定。
    /// 每次调用应返回独立的适配器实例（对应一次设备连接）。
    async fn create_adapter(
        &self,
        config: &ProtocolPluginConfig,
    ) -> Result<Box<dyn ProtocolAdapterInstance>, PluginError>;
}

/// 协议插件配置（通用，避免依赖 eneros-device 的 ConnectionConfig）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProtocolPluginConfig {
    /// 连接地址（IP:Port / 串口设备 / 网卡名）
    pub address: String,
    /// 协议特定配置（JSON）
    pub protocol_config: serde_json::Value,
    /// 超时（毫秒）
    pub timeout_ms: u64,
}

impl Default for ProtocolPluginConfig {
    fn default() -> Self {
        Self {
            address: String::new(),
            protocol_config: serde_json::Value::Null,
            timeout_ms: 5000,
        }
    }
}

/// 协议适配器实例（插件创建的适配器）
///
/// 这是 `eneros_device::ProtocolAdapter` 的简化版本，避免 eneros-plugin
/// 依赖 eneros-device 造成循环依赖。设备层可在边界处做适配转换。
#[async_trait]
pub trait ProtocolAdapterInstance: Send + Sync {
    /// 连接设备
    async fn connect(&mut self) -> Result<(), PluginError>;
    /// 断开连接
    async fn disconnect(&mut self) -> Result<(), PluginError>;
    /// 读取数据点
    async fn read(&self, address: &str) -> Result<PluginDataPoint, PluginError>;
    /// 写入数据
    async fn write(&mut self, address: &str, value: &PluginDataValue) -> Result<(), PluginError>;
    /// 适配器名称
    fn name(&self) -> &str;
    /// 是否已连接
    fn is_connected(&self) -> bool;
}

/// 插件数据值（与 eneros-device DataValue 镜像，避免循环依赖）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PluginDataValue {
    /// 布尔量（开关状态、告警标志）
    Bool(bool),
    /// 16 位整数
    Int16(i16),
    /// 32 位整数
    Int32(i32),
    /// 64 位整数（电度累计量）
    Int64(i64),
    /// 32 位浮点（遥测值）
    Float32(f32),
    /// 64 位浮点（高精度计量）
    Float64(f64),
    /// 字符串
    String(String),
    /// 字节序列（原始报文）
    Bytes(Vec<u8>),
}

/// 插件数据点
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginDataPoint {
    /// 数据点地址（协议相关，如 Modbus 寄存器号、IEC 103 ASDU 地址）
    pub address: String,
    /// 数据值
    pub value: PluginDataValue,
    /// Unix 毫秒时间戳
    pub timestamp: u64,
    /// 数据质量
    pub quality: PluginDataQuality,
}

/// 数据质量
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginDataQuality {
    /// 有效
    Good,
    /// 不确定
    Uncertain,
    /// 无效
    Bad,
    /// 设备离线
    Offline,
}

/// 协议插件注册表
///
/// 线程安全：内部使用 `parking_lot::RwLock` 保护 HashMap，
/// 支持多线程并发注册/查找/注销。
pub struct ProtocolPluginRegistry {
    plugins: RwLock<HashMap<String, Arc<dyn ProtocolPlugin>>>,
}

impl ProtocolPluginRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// 注册协议插件
    ///
    /// 若同名协议已注册，返回 `PluginError::AlreadyLoaded`。
    pub fn register(&self, plugin: Arc<dyn ProtocolPlugin>) -> Result<(), PluginError> {
        let name = plugin.protocol_name().to_string();
        let mut plugins = self.plugins.write();
        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }
        plugins.insert(name, plugin);
        Ok(())
    }

    /// 注销协议插件
    ///
    /// 若协议未注册，返回 `PluginError::NotLoaded`。
    pub fn unregister(&self, name: &str) -> Result<Arc<dyn ProtocolPlugin>, PluginError> {
        let mut plugins = self.plugins.write();
        plugins
            .remove(name)
            .ok_or_else(|| PluginError::NotLoaded(name.to_string()))
    }

    /// 查找协议插件
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn ProtocolPlugin>> {
        self.plugins.read().get(name).cloned()
    }

    /// 列出所有协议插件名称
    pub fn list(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }

    /// 列出所有协议插件（带详情）
    pub fn list_with_info(&self) -> Vec<ProtocolPluginInfo> {
        self.plugins
            .read()
            .values()
            .map(|p| ProtocolPluginInfo {
                name: p.protocol_name().to_string(),
                protocol_type: p.protocol_type_str(),
                description: p.description().to_string(),
            })
            .collect()
    }

    /// 是否包含指定协议
    pub fn contains(&self, name: &str) -> bool {
        self.plugins.read().contains_key(name)
    }

    /// 注册的协议数量
    pub fn count(&self) -> usize {
        self.plugins.read().len()
    }
}

impl Default for ProtocolPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 协议插件信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProtocolPluginInfo {
    /// 协议名称
    pub name: String,
    /// 协议类型字符串（"custom:`<name>`"）
    pub protocol_type: String,
    /// 协议描述
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 Mock 协议插件
    struct MockProtocolPlugin {
        name: String,
    }

    #[async_trait]
    impl ProtocolPlugin for MockProtocolPlugin {
        fn protocol_name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "mock protocol plugin for testing"
        }

        async fn create_adapter(
            &self,
            _config: &ProtocolPluginConfig,
        ) -> Result<Box<dyn ProtocolAdapterInstance>, PluginError> {
            Ok(Box::new(MockAdapter { connected: false }))
        }
    }

    /// 测试用 Mock 适配器实例
    struct MockAdapter {
        connected: bool,
    }

    #[async_trait]
    impl ProtocolAdapterInstance for MockAdapter {
        async fn connect(&mut self) -> Result<(), PluginError> {
            self.connected = true;
            Ok(())
        }

        async fn disconnect(&mut self) -> Result<(), PluginError> {
            self.connected = false;
            Ok(())
        }

        async fn read(&self, address: &str) -> Result<PluginDataPoint, PluginError> {
            Ok(PluginDataPoint {
                address: address.to_string(),
                value: PluginDataValue::Bool(true),
                timestamp: 1000,
                quality: PluginDataQuality::Good,
            })
        }

        async fn write(
            &mut self,
            _address: &str,
            _value: &PluginDataValue,
        ) -> Result<(), PluginError> {
            Ok(())
        }

        fn name(&self) -> &str {
            "mock-adapter"
        }

        fn is_connected(&self) -> bool {
            self.connected
        }
    }

    fn make_plugin(name: &str) -> Arc<dyn ProtocolPlugin> {
        Arc::new(MockProtocolPlugin {
            name: name.to_string(),
        })
    }

    #[test]
    fn test_protocol_plugin_config_default() {
        let cfg = ProtocolPluginConfig::default();
        assert!(cfg.address.is_empty());
        assert_eq!(cfg.timeout_ms, 5000);
        assert!(cfg.protocol_config.is_null());
    }

    #[test]
    fn test_protocol_plugin_type_str() {
        let plugin = MockProtocolPlugin {
            name: "iec103".to_string(),
        };
        assert_eq!(plugin.protocol_name(), "iec103");
        assert_eq!(plugin.protocol_type_str(), "custom:iec103");
        assert_eq!(plugin.description(), "mock protocol plugin for testing");
    }

    #[test]
    fn test_registry_register_unregister() {
        let registry = ProtocolPluginRegistry::new();
        let plugin = make_plugin("iec103");
        assert!(registry.register(plugin).is_ok());
        assert!(registry.contains("iec103"));

        let unregistered = registry.unregister("iec103");
        assert!(unregistered.is_ok());
        assert!(!registry.contains("iec103"));
    }

    #[test]
    fn test_registry_lookup() {
        let registry = ProtocolPluginRegistry::new();
        assert!(registry.lookup("iec103").is_none());

        let plugin = make_plugin("iec103");
        registry.register(plugin).unwrap();
        assert!(registry.lookup("iec103").is_some());
        assert!(registry.lookup("modbus-rtu").is_none());
    }

    #[test]
    fn test_registry_list() {
        let registry = ProtocolPluginRegistry::new();
        registry.register(make_plugin("iec103")).unwrap();
        registry.register(make_plugin("cdt")).unwrap();

        let mut names = registry.list();
        names.sort();
        assert_eq!(names, vec!["cdt".to_string(), "iec103".to_string()]);
    }

    #[test]
    fn test_registry_already_loaded() {
        let registry = ProtocolPluginRegistry::new();
        registry.register(make_plugin("iec103")).unwrap();
        let err = registry.register(make_plugin("iec103")).unwrap_err();
        assert!(matches!(err, PluginError::AlreadyLoaded(_)));
        assert_eq!(err.to_string(), "plugin already loaded: iec103");
    }

    #[test]
    fn test_registry_not_loaded() {
        let registry = ProtocolPluginRegistry::new();
        // unregister 返回 Arc<dyn ProtocolPlugin>，未实现 Debug，
        // 故用 .err().unwrap() 而非 .unwrap_err() 提取错误
        let err = registry.unregister("iec103").err().unwrap();
        assert!(matches!(err, PluginError::NotLoaded(_)));
        assert_eq!(err.to_string(), "plugin not loaded: iec103");
    }

    #[test]
    fn test_registry_contains() {
        let registry = ProtocolPluginRegistry::new();
        assert!(!registry.contains("iec103"));
        registry.register(make_plugin("iec103")).unwrap();
        assert!(registry.contains("iec103"));
        assert!(!registry.contains("cdt"));
    }

    #[test]
    fn test_registry_count() {
        let registry = ProtocolPluginRegistry::new();
        assert_eq!(registry.count(), 0);
        registry.register(make_plugin("iec103")).unwrap();
        assert_eq!(registry.count(), 1);
        registry.register(make_plugin("cdt")).unwrap();
        assert_eq!(registry.count(), 2);
        registry.unregister("iec103").unwrap();
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_protocol_plugin_info() {
        let registry = ProtocolPluginRegistry::new();
        registry.register(make_plugin("iec103")).unwrap();
        registry.register(make_plugin("cdt")).unwrap();

        let mut infos = registry.list_with_info();
        infos.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "cdt");
        assert_eq!(infos[0].protocol_type, "custom:cdt");
        assert_eq!(infos[0].description, "mock protocol plugin for testing");
        assert_eq!(infos[1].name, "iec103");
        assert_eq!(infos[1].protocol_type, "custom:iec103");
    }

    #[tokio::test]
    async fn test_mock_adapter_lifecycle() {
        let plugin = make_plugin("iec103");
        let config = ProtocolPluginConfig {
            address: "COM1".to_string(),
            protocol_config: serde_json::json!({"baud_rate": 9600}),
            timeout_ms: 3000,
        };
        let mut adapter = plugin.create_adapter(&config).await.unwrap();
        assert_eq!(adapter.name(), "mock-adapter");
        assert!(!adapter.is_connected());

        adapter.connect().await.unwrap();
        assert!(adapter.is_connected());

        let dp = adapter.read("1.2.3").await.unwrap();
        assert_eq!(dp.address, "1.2.3");
        assert_eq!(dp.value, PluginDataValue::Bool(true));
        assert_eq!(dp.quality, PluginDataQuality::Good);

        adapter
            .write("1.2.3", &PluginDataValue::Bool(false))
            .await
            .unwrap();

        adapter.disconnect().await.unwrap();
        assert!(!adapter.is_connected());
    }

    #[test]
    fn test_plugin_data_value_serde() {
        let v = PluginDataValue::Float32(2.5);
        let json = serde_json::to_string(&v).unwrap();
        let de: PluginDataValue = serde_json::from_str(&json).unwrap();
        assert_eq!(v, de);
    }

    #[test]
    fn test_plugin_data_quality_serde() {
        let q = PluginDataQuality::Bad;
        let json = serde_json::to_string(&q).unwrap();
        let de: PluginDataQuality = serde_json::from_str(&json).unwrap();
        assert_eq!(q, de);
    }

    #[test]
    fn test_protocol_plugin_config_serde() {
        let cfg = ProtocolPluginConfig {
            address: "192.168.1.1:2404".to_string(),
            protocol_config: serde_json::json!({"ca": 1}),
            timeout_ms: 10000,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let de: ProtocolPluginConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.address, "192.168.1.1:2404");
        assert_eq!(de.timeout_ms, 10000);
    }
}
