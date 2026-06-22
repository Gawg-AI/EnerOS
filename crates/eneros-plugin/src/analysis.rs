//! 分析模块插件接口
//!
//! 提供 `AnalysisPlugin` trait 与 `AnalysisPluginRegistry`，用于在不依赖
//! `eneros-analysis`（避免循环依赖）的前提下，让第三方插件以动态库形式注册
//! 自定义电力系统分析功能（如可靠性分析、潮流计算、短路计算等）。
//!
//! 架构关系：
//! - `eneros-plugin`（本 crate）定义插件接口与注册表
//! - `eneros-analysis` 定义内置 `AnalysisResult` 与分析算法
//! - 插件实现 `AnalysisPlugin`，由加载器注册到 `AnalysisPluginRegistry`
//! - 分析子系统在边界处将插件结果适配为内部类型
//!
//! 安全约束：
//! - 输入/输出使用 `serde_json::Value` 而非 `ndarray`/`Complex64`，
//!   避免跨动态库 ABI 的不安全类型传递
//! - 分析插件为同步 trait（CPU 密集型计算），由调用方决定是否在线程池中执行

use crate::error::PluginError;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// 分析结果（与 eneros_analysis::AnalysisResult 镜像，避免循环依赖）
///
/// 泛型 `T` 在插件接口边界处固定为 `serde_json::Value`，
/// 内部使用时可特化为具体类型（如潮流结果、可靠性指标）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult<T> {
    /// 是否收敛
    pub converged: bool,
    /// 迭代次数
    pub iterations: u32,
    /// 分析结果数据
    pub result: T,
    /// 分析过程中产生的告警信息
    pub warnings: Vec<String>,
}

impl<T> AnalysisResult<T> {
    /// 创建一个收敛的成功结果（迭代次数为 1，无告警）
    pub fn new(result: T) -> Self {
        Self {
            converged: true,
            iterations: 1,
            result,
            warnings: Vec::new(),
        }
    }

    /// 附加告警信息
    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }

    /// 创建一个失败结果（未收敛，迭代次数为 0）
    ///
    /// 要求 `T: Default` 以构造默认占位结果。
    pub fn failed(warnings: Vec<String>) -> Self
    where
        T: Default,
    {
        Self {
            converged: false,
            iterations: 0,
            result: T::default(),
            warnings,
        }
    }
}

/// 分析插件 trait
///
/// 插件以动态库形式加载后，需实现此 trait 并通过 C ABI 入口函数
/// `eneros_plugin_create` 返回 `Box<dyn AnalysisPlugin>`。
///
/// 每个插件代表一类分析（如可靠性分析、潮流计算），通过 `analyze_type`
/// 标识，注册到 `AnalysisPluginRegistry` 后由 `AnalysisScheduler` 调度。
///
/// 输入/输出使用 `serde_json::Value` 避免 ABI 不安全类型（ndarray/Complex64），
/// 插件内部可自行反序列化为所需结构。
pub trait AnalysisPlugin: Send + Sync {
    /// 分析类型标识（如 "reliability"、"powerflow"），全局唯一
    fn analyze_type(&self) -> &str;

    /// 分析类型描述
    fn description(&self) -> &str {
        ""
    }

    /// 执行分析
    ///
    /// 输入为 JSON Value，输出为 `AnalysisResult<serde_json::Value>`。
    /// 插件应自行解析输入字段并构造输出 JSON。
    fn analyze(
        &self,
        input: &serde_json::Value,
    ) -> Result<AnalysisResult<serde_json::Value>, PluginError>;
}

/// 分析任务
///
/// 用于批量调度，`analyze_type` 指定调用哪个插件，`input` 为插件输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisTask {
    /// 分析类型标识
    pub analyze_type: String,
    /// 分析输入（JSON）
    pub input: serde_json::Value,
}

/// 分析插件注册表
///
/// 线程安全：内部使用 `parking_lot::RwLock` 保护 HashMap，
/// 支持多线程并发注册/查找/注销。
pub struct AnalysisPluginRegistry {
    plugins: RwLock<HashMap<String, Arc<dyn AnalysisPlugin>>>,
}

impl AnalysisPluginRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// 注册分析插件
    ///
    /// 若同名分析类型已注册，返回 `PluginError::AlreadyLoaded`。
    pub fn register(&self, plugin: Arc<dyn AnalysisPlugin>) -> Result<(), PluginError> {
        let name = plugin.analyze_type().to_string();
        let mut plugins = self.plugins.write();
        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }
        plugins.insert(name, plugin);
        Ok(())
    }

    /// 注销分析插件
    ///
    /// 若分析类型未注册，返回 `PluginError::NotLoaded`。
    pub fn unregister(&self, analyze_type: &str) -> Result<Arc<dyn AnalysisPlugin>, PluginError> {
        let mut plugins = self.plugins.write();
        plugins
            .remove(analyze_type)
            .ok_or_else(|| PluginError::NotLoaded(analyze_type.to_string()))
    }

    /// 查找分析插件
    pub fn lookup(&self, analyze_type: &str) -> Option<Arc<dyn AnalysisPlugin>> {
        self.plugins.read().get(analyze_type).cloned()
    }

    /// 列出所有分析类型标识
    pub fn list(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }

    /// 列出所有分析插件（带详情）
    pub fn list_with_info(&self) -> Vec<AnalysisPluginInfo> {
        self.plugins
            .read()
            .values()
            .map(|p| AnalysisPluginInfo {
                analyze_type: p.analyze_type().to_string(),
                description: p.description().to_string(),
            })
            .collect()
    }

    /// 是否包含指定分析类型
    pub fn contains(&self, analyze_type: &str) -> bool {
        self.plugins.read().contains_key(analyze_type)
    }

    /// 注册的分析插件数量
    pub fn count(&self) -> usize {
        self.plugins.read().len()
    }
}

impl Default for AnalysisPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 分析插件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisPluginInfo {
    /// 分析类型标识
    pub analyze_type: String,
    /// 分析类型描述
    pub description: String,
}

/// 分析任务调度器
///
/// 封装 `AnalysisPluginRegistry`，提供单任务与批量任务调度接口。
/// 调度器持有注册表的 `Arc` 引用，可与加载器共享同一注册表实例。
pub struct AnalysisScheduler {
    registry: Arc<AnalysisPluginRegistry>,
}

impl AnalysisScheduler {
    /// 创建调度器
    pub fn new(registry: Arc<AnalysisPluginRegistry>) -> Self {
        Self { registry }
    }

    /// 调度单个分析任务
    ///
    /// 若指定分析类型未注册，返回 `PluginError::NotLoaded`。
    pub fn schedule(
        &self,
        analyze_type: &str,
        input: serde_json::Value,
    ) -> Result<AnalysisResult<serde_json::Value>, PluginError> {
        let plugin = self
            .registry
            .lookup(analyze_type)
            .ok_or_else(|| PluginError::NotLoaded(format!("analysis plugin '{}' not found", analyze_type)))?;
        plugin.analyze(&input)
    }

    /// 批量调度分析任务
    ///
    /// 按顺序执行每个任务，返回与任务列表一一对应的结果列表。
    /// 单个任务失败不影响其他任务，失败项以 `Err` 形式返回。
    pub fn schedule_batch(
        &self,
        tasks: Vec<AnalysisTask>,
    ) -> Vec<Result<AnalysisResult<serde_json::Value>, PluginError>> {
        tasks
            .into_iter()
            .map(|task| self.schedule(&task.analyze_type, task.input))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 Mock 分析插件
    struct MockAnalysisPlugin {
        analyze_type: String,
        description: String,
    }

    impl AnalysisPlugin for MockAnalysisPlugin {
        fn analyze_type(&self) -> &str {
            &self.analyze_type
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn analyze(
            &self,
            input: &serde_json::Value,
        ) -> Result<AnalysisResult<serde_json::Value>, PluginError> {
            // 简单回显输入作为结果，用于测试
            Ok(AnalysisResult::new(input.clone()))
        }
    }

    /// 构造默认 Mock 插件
    fn make_plugin(analyze_type: &str) -> Arc<dyn AnalysisPlugin> {
        Arc::new(MockAnalysisPlugin {
            analyze_type: analyze_type.to_string(),
            description: "mock analysis plugin for testing".to_string(),
        })
    }

    /// 构造可定制描述的 Mock 插件
    fn make_plugin_with_desc(analyze_type: &str, description: &str) -> Arc<dyn AnalysisPlugin> {
        Arc::new(MockAnalysisPlugin {
            analyze_type: analyze_type.to_string(),
            description: description.to_string(),
        })
    }

    #[test]
    fn test_analysis_result_new() {
        let result: AnalysisResult<i32> = AnalysisResult::new(42);
        assert!(result.converged);
        assert_eq!(result.iterations, 1);
        assert_eq!(result.result, 42);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_analysis_result_with_warnings() {
        let warnings = vec!["near voltage limit".to_string(), "high loading".to_string()];
        let result: AnalysisResult<i32> = AnalysisResult::new(10).with_warnings(warnings.clone());
        assert!(result.converged);
        assert_eq!(result.result, 10);
        assert_eq!(result.warnings, warnings);
    }

    #[test]
    fn test_analysis_result_failed() {
        let warnings = vec!["no convergence".to_string()];
        let result: AnalysisResult<i32> = AnalysisResult::failed(warnings.clone());
        assert!(!result.converged);
        assert_eq!(result.iterations, 0);
        assert_eq!(result.result, 0);
        assert_eq!(result.warnings, warnings);
    }

    #[test]
    fn test_registry_register_unregister() {
        let registry = AnalysisPluginRegistry::new();
        let plugin = make_plugin("reliability");
        assert!(registry.register(plugin).is_ok());
        assert!(registry.contains("reliability"));

        let unregistered = registry.unregister("reliability");
        assert!(unregistered.is_ok());
        assert!(!registry.contains("reliability"));
    }

    #[test]
    fn test_registry_lookup() {
        let registry = AnalysisPluginRegistry::new();
        assert!(registry.lookup("reliability").is_none());

        let plugin = make_plugin("reliability");
        registry.register(plugin).unwrap();
        assert!(registry.lookup("reliability").is_some());
        assert!(registry.lookup("powerflow").is_none());
    }

    #[test]
    fn test_registry_list() {
        let registry = AnalysisPluginRegistry::new();
        registry.register(make_plugin("reliability")).unwrap();
        registry.register(make_plugin("powerflow")).unwrap();

        let mut names = registry.list();
        names.sort();
        assert_eq!(
            names,
            vec!["powerflow".to_string(), "reliability".to_string()]
        );
    }

    #[test]
    fn test_registry_already_loaded() {
        let registry = AnalysisPluginRegistry::new();
        registry.register(make_plugin("reliability")).unwrap();
        let err = registry.register(make_plugin("reliability")).unwrap_err();
        assert!(matches!(err, PluginError::AlreadyLoaded(_)));
        assert_eq!(err.to_string(), "plugin already loaded: reliability");
    }

    #[test]
    fn test_registry_not_loaded() {
        let registry = AnalysisPluginRegistry::new();
        // unregister 返回 Arc<dyn AnalysisPlugin>，未实现 Debug，
        // 故用 .err().unwrap() 而非 .unwrap_err() 提取错误
        let err = registry.unregister("reliability").err().unwrap();
        assert!(matches!(err, PluginError::NotLoaded(_)));
        assert_eq!(err.to_string(), "plugin not loaded: reliability");
    }

    #[test]
    fn test_registry_contains() {
        let registry = AnalysisPluginRegistry::new();
        assert!(!registry.contains("reliability"));
        registry.register(make_plugin("reliability")).unwrap();
        assert!(registry.contains("reliability"));
        assert!(!registry.contains("powerflow"));
    }

    #[test]
    fn test_registry_count() {
        let registry = AnalysisPluginRegistry::new();
        assert_eq!(registry.count(), 0);
        registry.register(make_plugin("reliability")).unwrap();
        assert_eq!(registry.count(), 1);
        registry.register(make_plugin("powerflow")).unwrap();
        assert_eq!(registry.count(), 2);
        registry.unregister("reliability").unwrap();
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_scheduler_schedule_success() {
        let registry = Arc::new(AnalysisPluginRegistry::new());
        registry.register(make_plugin("reliability")).unwrap();
        let scheduler = AnalysisScheduler::new(registry);

        let input = serde_json::json!({"customers_affected": 100});
        let result = scheduler.schedule("reliability", input.clone()).unwrap();
        assert!(result.converged);
        assert_eq!(result.result, input);
    }

    #[test]
    fn test_scheduler_schedule_not_found() {
        let registry = Arc::new(AnalysisPluginRegistry::new());
        let scheduler = AnalysisScheduler::new(registry);

        let err = scheduler
            .schedule("nonexistent", serde_json::Value::Null)
            .unwrap_err();
        assert!(matches!(err, PluginError::NotLoaded(_)));
        assert_eq!(
            err.to_string(),
            "plugin not loaded: analysis plugin 'nonexistent' not found"
        );
    }

    #[test]
    fn test_scheduler_schedule_batch() {
        let registry = Arc::new(AnalysisPluginRegistry::new());
        registry.register(make_plugin("reliability")).unwrap();
        registry.register(make_plugin("powerflow")).unwrap();
        let scheduler = AnalysisScheduler::new(registry);

        let tasks = vec![
            AnalysisTask {
                analyze_type: "reliability".to_string(),
                input: serde_json::json!({"a": 1}),
            },
            AnalysisTask {
                analyze_type: "powerflow".to_string(),
                input: serde_json::json!({"b": 2}),
            },
            AnalysisTask {
                analyze_type: "nonexistent".to_string(),
                input: serde_json::Value::Null,
            },
        ];

        let results = scheduler.schedule_batch(tasks);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());
    }

    #[test]
    fn test_analysis_plugin_info() {
        let registry = AnalysisPluginRegistry::new();
        registry
            .register(make_plugin_with_desc(
                "reliability",
                "Power grid reliability analysis",
            ))
            .unwrap();
        registry
            .register(make_plugin_with_desc(
                "powerflow",
                "Newton-Raphson power flow",
            ))
            .unwrap();

        let mut infos = registry.list_with_info();
        infos.sort_by(|a, b| a.analyze_type.cmp(&b.analyze_type));

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].analyze_type, "powerflow");
        assert_eq!(infos[0].description, "Newton-Raphson power flow");
        assert_eq!(infos[1].analyze_type, "reliability");
        assert_eq!(infos[1].description, "Power grid reliability analysis");
    }

    #[test]
    fn test_analysis_result_serde() {
        let result: AnalysisResult<i32> = AnalysisResult::new(42)
            .with_warnings(vec!["warning1".to_string()]);
        let json = serde_json::to_string(&result).unwrap();
        let de: AnalysisResult<i32> = serde_json::from_str(&json).unwrap();
        assert!(de.converged);
        assert_eq!(de.iterations, 1);
        assert_eq!(de.result, 42);
        assert_eq!(de.warnings, vec!["warning1".to_string()]);
    }

    #[test]
    fn test_analysis_task_serde() {
        let task = AnalysisTask {
            analyze_type: "reliability".to_string(),
            input: serde_json::json!({"customers": 100}),
        };
        let json = serde_json::to_string(&task).unwrap();
        let de: AnalysisTask = serde_json::from_str(&json).unwrap();
        assert_eq!(de.analyze_type, "reliability");
        assert_eq!(de.input, serde_json::json!({"customers": 100}));
    }

    #[test]
    fn test_registry_default() {
        let registry = AnalysisPluginRegistry::default();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_analysis_plugin_default_description() {
        // 测试默认 description 返回空字符串
        struct MinimalPlugin;
        impl AnalysisPlugin for MinimalPlugin {
            fn analyze_type(&self) -> &str {
                "minimal"
            }
            fn analyze(
                &self,
                _input: &serde_json::Value,
            ) -> Result<AnalysisResult<serde_json::Value>, PluginError> {
                Ok(AnalysisResult::new(serde_json::Value::Null))
            }
        }
        let plugin = MinimalPlugin;
        assert_eq!(plugin.description(), "");
    }
}
