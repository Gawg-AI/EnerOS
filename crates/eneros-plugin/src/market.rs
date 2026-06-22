//! 插件市场客户端 — 远程仓库搜索、下载、签名验证、本地缓存
//!
//! 提供插件市场的基础功能，包括：
//! - 多仓库配置管理（`RepoConfig`、`MarketConfig`）
//! - 仓库索引加载与缓存（`RepoIndex`、`PluginIndexEntry`）
//! - 插件搜索（按名称、描述、作者，不区分大小写）
//! - 插件下载到本地缓存（简化实现，实际 HTTP 下载需扩展）
//! - 缓存 LRU 清理
//!
//! # 典型流程
//! 1. `PluginMarketClient::with_defaults()` 创建客户端
//! 2. `load_repo_index()` 加载远程仓库索引（TOML 格式）
//! 3. `search()` 在已缓存索引中搜索插件
//! 4. `download()` 下载插件到本地缓存（简化实现）
//! 5. `clean_cache()` 按 LRU 策略清理缓存

use crate::error::{PluginError, PluginResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 仓库配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    /// 仓库名称
    pub name: String,
    /// 仓库 URL（指向 TOML 索引文件）
    pub url: String,
    /// 是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 优先级（数字越小优先级越高）
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_true() -> bool {
    true
}

fn default_priority() -> u32 {
    100
}

/// 插件市场配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketConfig {
    /// 仓库列表
    pub repos: Vec<RepoConfig>,
    /// 本地缓存目录（支持 `~` 前缀展开）
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    /// 缓存上限（字节）
    #[serde(default = "default_cache_limit")]
    pub cache_limit_bytes: u64,
}

fn default_cache_dir() -> String {
    "~/.eneros/plugins/cache".to_string()
}

fn default_cache_limit() -> u64 {
    1024 * 1024 * 1024 // 1GB
}

/// 远程插件索引条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    /// 插件名称
    pub name: String,
    /// 版本（语义化版本）
    pub version: String,
    /// 描述
    pub description: String,
    /// 作者
    pub author: String,
    /// 插件类型（protocol / agent / analysis）
    pub plugin_type: String,
    /// 下载 URL
    pub download_url: String,
    /// 文件校验和（SHA-256）
    pub checksum: String,
    /// 签名 URL
    pub signature_url: String,
}

/// 仓库索引
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    /// 仓库名称
    pub repo_name: String,
    /// 插件列表
    pub plugins: Vec<PluginIndexEntry>,
}

/// 搜索结果
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// 匹配的插件列表（repo_name, entry）
    pub entries: Vec<(String, PluginIndexEntry)>,
}

/// 下载结果
#[derive(Debug, Clone)]
pub struct DownloadResult {
    /// 本地缓存路径
    pub local_path: PathBuf,
    /// 文件大小（字节）
    pub size_bytes: u64,
    /// 校验和验证通过
    pub checksum_verified: bool,
}

/// 插件市场客户端
///
/// 维护多仓库配置与已加载的仓库索引，支持搜索、下载与缓存管理。
/// 客户端本身不持有网络连接，下载为简化实现。
pub struct PluginMarketClient {
    /// 市场配置
    config: MarketConfig,
    /// 已缓存的仓库索引（repo_name -> RepoIndex）
    cached_indices: HashMap<String, RepoIndex>,
}

impl PluginMarketClient {
    /// 创建市场客户端
    pub fn new(config: MarketConfig) -> Self {
        Self {
            config,
            cached_indices: HashMap::new(),
        }
    }

    /// 使用默认配置创建（官方仓库）
    pub fn with_defaults() -> Self {
        Self::new(MarketConfig {
            repos: vec![RepoConfig {
                name: "official".to_string(),
                url: "https://plugins.eneros.io/index.toml".to_string(),
                enabled: true,
                priority: 100,
            }],
            cache_dir: default_cache_dir(),
            cache_limit_bytes: default_cache_limit(),
        })
    }

    /// 从配置文件加载
    ///
    /// 读取 TOML 格式的市场配置文件。IO 错误映射为 `PluginError::Io`，
    /// 解析错误映射为 `PluginError::InvalidManifest`。
    pub fn load_from_file(path: &str) -> PluginResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: MarketConfig = toml::from_str(&content).map_err(|e| {
            PluginError::InvalidManifest(format!("market config parse failed: {}", e))
        })?;
        Ok(Self::new(config))
    }

    /// 搜索插件（在已缓存的索引中搜索）
    ///
    /// 匹配插件名称、描述或作者（不区分大小写）。
    /// 仅搜索已启用仓库的已加载索引。
    pub fn search(&self, query: &str) -> SearchResult {
        let query_lower = query.to_lowercase();
        let mut entries = Vec::new();

        for repo in &self.config.repos {
            if !repo.enabled {
                continue;
            }
            if let Some(index) = self.cached_indices.get(&repo.name) {
                for plugin in &index.plugins {
                    if plugin.name.to_lowercase().contains(&query_lower)
                        || plugin.description.to_lowercase().contains(&query_lower)
                        || plugin.author.to_lowercase().contains(&query_lower)
                    {
                        entries.push((repo.name.clone(), plugin.clone()));
                    }
                }
            }
        }

        SearchResult { entries }
    }

    /// 加载仓库索引
    ///
    /// 解析 TOML 格式的索引内容并缓存到内存。解析错误映射为 `PluginError::InvalidManifest`。
    pub fn load_repo_index(&mut self, repo_name: &str, index_content: &str) -> PluginResult<()> {
        let index: RepoIndex = toml::from_str(index_content)
            .map_err(|e| PluginError::InvalidManifest(format!("repo index parse failed: {}", e)))?;
        self.cached_indices.insert(repo_name.to_string(), index);
        Ok(())
    }

    /// 下载插件到本地缓存
    ///
    /// 简化实现：创建占位文件。实际生产环境应：
    /// 1. 从 `entry.download_url` 下载文件
    /// 2. 计算 SHA-256 校验和并与 `entry.checksum` 对比
    /// 3. 下载 `entry.signature_url` 签名文件并调用
    ///    `signature::PluginSignatureVerifier` 验证
    pub fn download(
        &self,
        repo_name: &str,
        plugin_name: &str,
        version: &str,
    ) -> PluginResult<DownloadResult> {
        // 验证插件存在于索引中（实际下载需使用 entry 的 download_url/checksum/signature_url）
        let _ = self
            .find_plugin(repo_name, plugin_name, version)
            .ok_or_else(|| {
                PluginError::DependencyMissing(format!(
                    "plugin {}@{} not found in repo {}",
                    plugin_name, version, repo_name
                ))
            })?;

        // 确保缓存目录存在
        let cache_dir = self.expand_cache_dir();
        std::fs::create_dir_all(&cache_dir)?;

        // 生成本地缓存路径
        let filename = format!("{}-{}.so", plugin_name, version);
        let local_path = cache_dir.join(filename);

        // 简化实现：写入占位文件
        let placeholder = b"# placeholder plugin binary";
        std::fs::write(&local_path, placeholder)?;

        Ok(DownloadResult {
            local_path,
            size_bytes: placeholder.len() as u64,
            // 占位实现未实际校验校验和，必须返回 false 以避免安全误导
            checksum_verified: false,
        })
    }

    /// 查找插件
    fn find_plugin(
        &self,
        repo_name: &str,
        plugin_name: &str,
        version: &str,
    ) -> Option<&PluginIndexEntry> {
        self.cached_indices
            .get(repo_name)?
            .plugins
            .iter()
            .find(|p| p.name == plugin_name && p.version == version)
    }

    /// 展开缓存目录路径（处理 `~` 前缀）
    ///
    /// 仅当路径以 `~` 开头时才展开为 home 目录：
    /// - Linux 下使用 `HOME` 环境变量，Windows 下回退到 `USERPROFILE`
    /// - 若环境变量均未设置，则原样返回配置路径
    /// - 绝对路径（如 `/tmp/eneros-cache`）原样返回，不拼接 home 目录
    fn expand_cache_dir(&self) -> PathBuf {
        if let Some(rest) = self.config.cache_dir.strip_prefix('~') {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .ok();
            match home {
                Some(h) => PathBuf::from(h).join(rest.trim_start_matches('/')),
                None => PathBuf::from(&self.config.cache_dir),
            }
        } else {
            PathBuf::from(&self.config.cache_dir)
        }
    }

    /// 列出所有已配置的仓库
    pub fn list_repos(&self) -> &[RepoConfig] {
        &self.config.repos
    }

    /// 列出所有已索引的插件
    ///
    /// 返回 `(repo_name, &PluginIndexEntry)` 列表，仅包含已加载索引的插件。
    pub fn list_plugins(&self) -> Vec<(String, &PluginIndexEntry)> {
        let mut result = Vec::new();
        for (repo_name, index) in &self.cached_indices {
            for plugin in &index.plugins {
                result.push((repo_name.clone(), plugin));
            }
        }
        result
    }

    /// 清理缓存（LRU 淘汰）
    ///
    /// 遍历缓存目录，若总大小超过 `cache_limit_bytes`，
    /// 按修改时间从旧到新删除文件直到满足上限。
    ///
    /// 返回清理后的缓存总大小（字节）。
    pub fn clean_cache(&self) -> PluginResult<u64> {
        let cache_dir = self.expand_cache_dir();
        if !cache_dir.exists() {
            return Ok(0);
        }

        let mut total_size = 0u64;
        let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();

        // 收集缓存文件
        if let Ok(rd) = std::fs::read_dir(&cache_dir) {
            for entry in rd.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        let mtime = meta
                            .modified()
                            .unwrap_or_else(|_| std::time::SystemTime::now());
                        let size = meta.len();
                        total_size += size;
                        entries.push((entry.path(), mtime, size));
                    }
                }
            }
        }

        // 超过上限时按最旧优先删除
        if total_size > self.config.cache_limit_bytes {
            entries.sort_by_key(|(_, mtime, _)| *mtime);
            for (path, _, size) in &entries {
                if total_size <= self.config.cache_limit_bytes {
                    break;
                }
                if std::fs::remove_file(path).is_ok() {
                    total_size -= *size;
                }
            }
        }

        Ok(total_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用仓库索引 TOML
    const SAMPLE_INDEX_TOML: &str = r#"
repo_name = "official"

[[plugins]]
name = "iec103-adapter"
version = "1.0.0"
description = "IEC 103 协议适配器"
author = "EnerOS Team"
plugin_type = "protocol"
download_url = "https://plugins.eneros.io/iec103-adapter-1.0.0.so"
checksum = "sha256:abc123"
signature_url = "https://plugins.eneros.io/iec103-adapter-1.0.0.sig"

[[plugins]]
name = "custom-strategy"
version = "0.9.0"
description = "自定义策略 Agent"
author = "ThirdParty"
plugin_type = "agent"
download_url = "https://plugins.eneros.io/custom-strategy-0.9.0.so"
checksum = "sha256:def456"
signature_url = "https://plugins.eneros.io/custom-strategy-0.9.0.sig"
"#;

    #[test]
    fn test_market_client_new() {
        let config = MarketConfig {
            repos: vec![
                RepoConfig {
                    name: "official".to_string(),
                    url: "https://plugins.eneros.io/index.toml".to_string(),
                    enabled: true,
                    priority: 100,
                },
                RepoConfig {
                    name: "community".to_string(),
                    url: "https://community.eneros.io/index.toml".to_string(),
                    enabled: false,
                    priority: 200,
                },
            ],
            cache_dir: "/tmp/eneros-cache".to_string(),
            cache_limit_bytes: 512 * 1024 * 1024,
        };
        let client = PluginMarketClient::new(config);
        // 通过 list_repos 验证配置已加载
        let repos = client.list_repos();
        assert_eq!(repos.len(), 2, "应包含 2 个仓库");
        assert_eq!(repos[0].name, "official");
        assert_eq!(repos[1].name, "community");
        assert!(repos[0].enabled, "official 应启用");
        assert!(!repos[1].enabled, "community 应禁用");
        assert_eq!(repos[0].priority, 100);
        assert_eq!(repos[1].priority, 200);
    }

    #[test]
    fn test_market_client_with_defaults() {
        let client = PluginMarketClient::with_defaults();
        let repos = client.list_repos();
        assert_eq!(repos.len(), 1, "默认配置应包含 1 个仓库");
        assert_eq!(repos[0].name, "official");
        assert!(repos[0].enabled, "默认仓库应启用");
        assert_eq!(repos[0].url, "https://plugins.eneros.io/index.toml");
        assert_eq!(repos[0].priority, 100);
    }

    #[test]
    fn test_market_search() {
        let mut client = PluginMarketClient::with_defaults();
        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");

        // 按名称搜索
        let result = client.search("iec103");
        assert_eq!(result.entries.len(), 1, "应匹配 iec103-adapter");
        assert_eq!(result.entries[0].1.name, "iec103-adapter");
        assert_eq!(result.entries[0].0, "official");

        // 按描述搜索（中文关键词）
        let result = client.search("策略");
        assert_eq!(result.entries.len(), 1, "应匹配 custom-strategy");
        assert_eq!(result.entries[0].1.name, "custom-strategy");

        // 按作者搜索
        let result = client.search("ThirdParty");
        assert_eq!(result.entries.len(), 1, "应匹配 ThirdParty 作者");
        assert_eq!(result.entries[0].1.name, "custom-strategy");

        // 无匹配
        let result = client.search("nonexistent");
        assert!(result.entries.is_empty(), "应无匹配结果");

        // 大小写不敏感
        let result = client.search("IEC103");
        assert_eq!(result.entries.len(), 1, "搜索应不区分大小写");
    }

    #[test]
    fn test_market_load_repo_index() {
        let mut client = PluginMarketClient::with_defaults();
        // 加载前无插件
        assert!(client.list_plugins().is_empty(), "加载前应无插件");

        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");

        // 加载后有 2 个插件
        let plugins = client.list_plugins();
        assert_eq!(plugins.len(), 2, "加载后应有 2 个插件");

        // 验证索引内容正确解析
        let iec103 = plugins
            .iter()
            .find(|(_, p)| p.name == "iec103-adapter")
            .expect("应包含 iec103-adapter");
        assert_eq!(iec103.0, "official");
        assert_eq!(iec103.1.version, "1.0.0");
        assert_eq!(iec103.1.checksum, "sha256:abc123");
    }

    #[test]
    fn test_market_list_repos() {
        let config = MarketConfig {
            repos: vec![
                RepoConfig {
                    name: "official".to_string(),
                    url: "https://plugins.eneros.io/index.toml".to_string(),
                    enabled: true,
                    priority: 10,
                },
                RepoConfig {
                    name: "staging".to_string(),
                    url: "https://staging.eneros.io/index.toml".to_string(),
                    enabled: true,
                    priority: 50,
                },
                RepoConfig {
                    name: "community".to_string(),
                    url: "https://community.eneros.io/index.toml".to_string(),
                    enabled: false,
                    priority: 200,
                },
            ],
            cache_dir: "/tmp/eneros-cache".to_string(),
            cache_limit_bytes: default_cache_limit(),
        };
        let client = PluginMarketClient::new(config);
        let repos = client.list_repos();
        assert_eq!(repos.len(), 3, "应列出 3 个仓库");
        assert_eq!(repos[0].name, "official");
        assert_eq!(repos[1].name, "staging");
        assert_eq!(repos[2].name, "community");
        assert_eq!(repos[0].priority, 10);
        assert_eq!(repos[2].priority, 200);
    }

    #[test]
    fn test_market_list_plugins() {
        let mut client = PluginMarketClient::with_defaults();
        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");

        let plugins = client.list_plugins();
        assert_eq!(plugins.len(), 2, "应列出 2 个插件");

        // 验证 iec103-adapter 插件内容
        let iec103 = plugins
            .iter()
            .find(|(_, p)| p.name == "iec103-adapter")
            .expect("应包含 iec103-adapter");
        assert_eq!(iec103.0, "official");
        assert_eq!(iec103.1.version, "1.0.0");
        assert_eq!(iec103.1.plugin_type, "protocol");
        assert_eq!(iec103.1.author, "EnerOS Team");
        assert_eq!(
            iec103.1.download_url,
            "https://plugins.eneros.io/iec103-adapter-1.0.0.so"
        );

        // 验证 custom-strategy 插件内容
        let custom = plugins
            .iter()
            .find(|(_, p)| p.name == "custom-strategy")
            .expect("应包含 custom-strategy");
        assert_eq!(custom.1.version, "0.9.0");
        assert_eq!(custom.1.plugin_type, "agent");
        assert_eq!(custom.1.author, "ThirdParty");
    }

    #[test]
    fn test_expand_cache_dir_absolute_path() {
        // 绝对路径不应拼接 home 目录，应原样返回
        let config = MarketConfig {
            repos: vec![],
            cache_dir: "/tmp/eneros-cache".to_string(),
            cache_limit_bytes: default_cache_limit(),
        };
        let client = PluginMarketClient::new(config);
        let expanded = client.expand_cache_dir();
        assert_eq!(
            expanded,
            PathBuf::from("/tmp/eneros-cache"),
            "绝对路径应原样返回，不拼接 home 目录"
        );
    }

    #[test]
    fn test_expand_cache_dir_tilde() {
        // `~/cache` 应展开为 home/cache
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .expect("HOME 或 USERPROFILE 环境变量应存在");
        let config = MarketConfig {
            repos: vec![],
            cache_dir: "~/cache".to_string(),
            cache_limit_bytes: default_cache_limit(),
        };
        let client = PluginMarketClient::new(config);
        let expanded = client.expand_cache_dir();
        let expected = PathBuf::from(&home).join("cache");
        assert_eq!(expanded, expected, "~/cache 应展开为 home/cache");
    }

    #[test]
    fn test_download_returns_unverified() {
        // 占位实现未实际校验校验和，应返回 checksum_verified: false
        let temp_dir = std::env::temp_dir().join("eneros_test_download_unverified");
        let config = MarketConfig {
            repos: vec![RepoConfig {
                name: "official".to_string(),
                url: "https://plugins.eneros.io/index.toml".to_string(),
                enabled: true,
                priority: 100,
            }],
            cache_dir: temp_dir.to_string_lossy().to_string(),
            cache_limit_bytes: default_cache_limit(),
        };
        let mut client = PluginMarketClient::new(config);
        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");
        let result = client
            .download("official", "iec103-adapter", "1.0.0")
            .expect("下载应成功");
        assert!(
            !result.checksum_verified,
            "占位实现未校验校验和，应返回 false"
        );
        // 清理临时目录
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
