//! 插件注册表
//!
//! 线程安全的插件注册表，记录已加载插件的元数据、状态与启用标志。
//! 使用 `parking_lot::RwLock` 提供并发读写访问。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::error::PluginError;
use crate::lifecycle::PluginState;
use crate::manifest::PluginMetadata;

/// 注册表中的插件条目
#[derive(Debug, Clone)]
pub struct PluginEntry {
    /// 插件元数据
    pub metadata: PluginMetadata,
    /// 当前状态
    pub state: PluginState,
    /// 加载时间
    pub loaded_at: DateTime<Utc>,
    /// 是否启用
    pub enabled: bool,
}

impl PluginEntry {
    /// 创建新的插件条目，初始状态为 `Loaded`，默认启用
    pub fn new(metadata: PluginMetadata) -> Self {
        Self {
            metadata,
            state: PluginState::Loaded,
            loaded_at: Utc::now(),
            enabled: true,
        }
    }
}

/// 插件注册表（线程安全）
#[derive(Debug)]
pub struct PluginRegistry {
    plugins: RwLock<HashMap<String, PluginEntry>>,
}

impl PluginRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// 注册插件条目，若同名插件已存在返回 `AlreadyLoaded`
    pub fn register(&self, entry: PluginEntry) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write();
        let name = entry.metadata.name.clone();
        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }
        plugins.insert(name, entry);
        Ok(())
    }

    /// 注销插件，若不存在返回 `NotLoaded`
    pub fn unregister(&self, name: &str) -> Result<PluginEntry, PluginError> {
        let mut plugins = self.plugins.write();
        plugins
            .remove(name)
            .ok_or_else(|| PluginError::NotLoaded(name.to_string()))
    }

    /// 查找插件，返回克隆的条目
    pub fn lookup(&self, name: &str) -> Option<PluginEntry> {
        self.plugins.read().get(name).cloned()
    }

    /// 列出所有插件条目
    pub fn list(&self) -> Vec<PluginEntry> {
        self.plugins.read().values().cloned().collect()
    }

    /// 更新插件状态，若不存在返回 `NotLoaded`
    pub fn update_state(&self, name: &str, state: PluginState) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write();
        let entry = plugins
            .get_mut(name)
            .ok_or_else(|| PluginError::NotLoaded(name.to_string()))?;
        entry.state = state;
        Ok(())
    }

    /// 设置插件启用/禁用，若不存在返回 `NotLoaded`
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write();
        let entry = plugins
            .get_mut(name)
            .ok_or_else(|| PluginError::NotLoaded(name.to_string()))?;
        entry.enabled = enabled;
        Ok(())
    }

    /// 判断插件是否已注册
    pub fn contains(&self, name: &str) -> bool {
        self.plugins.read().contains_key(name)
    }

    /// 返回已注册插件数量
    pub fn count(&self) -> usize {
        self.plugins.read().len()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PluginType;

    fn make_metadata(name: &str) -> PluginMetadata {
        PluginMetadata {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.27.0".to_string(),
            plugin_type: PluginType::Agent,
            description: String::new(),
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let registry = PluginRegistry::new();
        let entry = PluginEntry::new(make_metadata("plugin-a"));
        assert!(registry.register(entry).is_ok());
        assert_eq!(registry.count(), 1);
        assert!(registry.contains("plugin-a"));

        let found = registry.lookup("plugin-a").unwrap();
        assert_eq!(found.metadata.name, "plugin-a");
        assert_eq!(found.state, PluginState::Loaded);
        assert!(found.enabled);
    }

    #[test]
    fn test_register_duplicate() {
        let registry = PluginRegistry::new();
        let entry = PluginEntry::new(make_metadata("plugin-a"));
        assert!(registry.register(entry).is_ok());
        let entry2 = PluginEntry::new(make_metadata("plugin-a"));
        let result = registry.register(entry2);
        assert!(matches!(result, Err(PluginError::AlreadyLoaded(_))));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_unregister() {
        let registry = PluginRegistry::new();
        registry
            .register(PluginEntry::new(make_metadata("plugin-a")))
            .unwrap();
        let removed = registry.unregister("plugin-a").unwrap();
        assert_eq!(removed.metadata.name, "plugin-a");
        assert_eq!(registry.count(), 0);
        assert!(!registry.contains("plugin-a"));
    }

    #[test]
    fn test_unregister_missing() {
        let registry = PluginRegistry::new();
        let result = registry.unregister("nonexistent");
        assert!(matches!(result, Err(PluginError::NotLoaded(_))));
    }

    #[test]
    fn test_update_state() {
        let registry = PluginRegistry::new();
        registry
            .register(PluginEntry::new(make_metadata("plugin-a")))
            .unwrap();
        assert!(registry
            .update_state("plugin-a", PluginState::Running)
            .is_ok());
        let found = registry.lookup("plugin-a").unwrap();
        assert_eq!(found.state, PluginState::Running);
    }

    #[test]
    fn test_update_state_missing() {
        let registry = PluginRegistry::new();
        let result = registry.update_state("nonexistent", PluginState::Running);
        assert!(matches!(result, Err(PluginError::NotLoaded(_))));
    }

    #[test]
    fn test_set_enabled() {
        let registry = PluginRegistry::new();
        registry
            .register(PluginEntry::new(make_metadata("plugin-a")))
            .unwrap();
        assert!(registry.set_enabled("plugin-a", false).is_ok());
        let found = registry.lookup("plugin-a").unwrap();
        assert!(!found.enabled);
        assert!(registry.set_enabled("plugin-a", true).is_ok());
        let found = registry.lookup("plugin-a").unwrap();
        assert!(found.enabled);
    }

    #[test]
    fn test_set_enabled_missing() {
        let registry = PluginRegistry::new();
        let result = registry.set_enabled("nonexistent", true);
        assert!(matches!(result, Err(PluginError::NotLoaded(_))));
    }

    #[test]
    fn test_list() {
        let registry = PluginRegistry::new();
        registry
            .register(PluginEntry::new(make_metadata("plugin-a")))
            .unwrap();
        registry
            .register(PluginEntry::new(make_metadata("plugin-b")))
            .unwrap();
        let list = registry.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_lookup_missing() {
        let registry = PluginRegistry::new();
        assert!(registry.lookup("nonexistent").is_none());
    }

    #[test]
    fn test_default() {
        let registry = PluginRegistry::default();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_entry_new_defaults() {
        let entry = PluginEntry::new(make_metadata("p"));
        assert_eq!(entry.state, PluginState::Loaded);
        assert!(entry.enabled);
    }
}
