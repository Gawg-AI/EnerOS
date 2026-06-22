//! EnerOS 插件框架核心
//!
//! 提供 EnerOS v0.27.0 插件系统的完整框架，包括：
//! - 插件清单（manifest）解析与元数据
//! - 插件生命周期状态机（Loaded → Running → Stopped）
//! - 插件注册表（线程安全）
//! - 依赖解析与拓扑排序（Kahn 算法）
//! - API 版本兼容性检查（语义化版本）
//! - Plugin trait 定义（异步生命周期接口）
//! - 动态库加载器（libloading + C ABI 入口函数）
//! - Ed25519 签名验证（密钥生成/签名/验证）
//! - 插件沙箱（seccomp + cgroups + catch_unwind 崩溃隔离）
//! - 协议适配器插件接口（ProtocolPlugin trait）
//! - Agent 策略插件接口（AgentPlugin trait，权限上限 Operator）
//! - 分析模块插件接口（AnalysisPlugin trait，serde_json::Value 输入/输出）
//! - 插件系统配置（PluginConfig，对应 /etc/eneros/plugin.toml）

pub mod error;
pub mod manifest;
pub mod lifecycle;
pub mod registry;
pub mod dependency;
pub mod version;
pub mod plugin;
pub mod loader;
pub mod signature;
pub mod sandbox;
pub mod protocol;
pub mod agent;
pub mod analysis;
pub mod config;
pub mod ipc;
pub mod market;

pub use error::{PluginError, PluginResult};
pub use manifest::{PluginManifest, PluginType, PluginMetadata};
pub use lifecycle::{PluginState, PluginLifecycle};
pub use registry::{PluginRegistry, PluginEntry};
pub use version::{check_compatibility, CURRENT_API_VERSION};
pub use dependency::{check_dependencies, resolve_load_order};
pub use plugin::Plugin;
pub use ipc::{PluginDaemonClient, DaemonRequest, DaemonResponse};
pub use market::{PluginMarketClient, MarketConfig, RepoConfig, PluginIndexEntry, RepoIndex};
