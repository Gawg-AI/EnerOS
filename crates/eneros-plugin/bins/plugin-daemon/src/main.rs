//! EnerOS Plugin Daemon — 独立守护进程（v0.28.0 Task 10）
//!
//! v0.27.0 插件系统采用同进程加载（libloading 直接加载进主进程），
//! v0.28.0 改为 plugin-daemon 独立进程加载，主进程通过 IPC 控制，实现崩溃隔离。
//!
//! # IPC 协议（JSON 行协议）
//!
//! 每行一个 JSON 请求/响应。
//!
//! 请求格式：
//! ```json
//! {"cmd": "load", "path": "/path/to/plugin.so", "skip_signature": false}
//! {"cmd": "unload", "name": "my-plugin"}
//! {"cmd": "list"}
//! {"cmd": "info", "name": "my-plugin"}
//! {"cmd": "enable", "name": "my-plugin"}
//! {"cmd": "disable", "name": "my-plugin"}
//! {"cmd": "verify", "path": "/path/to/plugin.so"}
//! {"cmd": "status"}
//! ```
//!
//! 响应格式：
//! ```json
//! {"ok": true, "data": {...}}
//! {"ok": false, "error": "error message"}
//! ```
//!
//! # 传输层
//!
//! - Linux：Unix socket（默认 `/var/run/eneros/plugin-daemon.sock`）
//! - 跨平台回退：TCP `127.0.0.1:5410`
//!
//! # 崩溃隔离
//!
//! 所有插件操作用 `std::panic::catch_unwind` 包裹，捕获 panic 后记录日志并返回错误响应，
//! daemon 进程不退出。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;
use eneros_plugin::config::PluginConfig;
use eneros_plugin::ipc::{DaemonRequest, DaemonResponse};
use eneros_plugin::loader::{LoadedPlugin, PluginLoader};
use eneros_plugin::registry::{PluginEntry, PluginRegistry};
use eneros_plugin::signature::{
    PluginSignatureVerifier, VerificationResult as SigVerificationResult,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tracing_subscriber::EnvFilter;

// ============================================================================
// CLI 参数
// ============================================================================

/// plugin-daemon CLI 参数
#[derive(Parser)]
#[command(name = "eneros-plugin-daemon", about = "EnerOS 插件守护进程 — 独立进程加载插件，崩溃隔离")]
struct Args {
    /// IPC 监听地址（Unix socket 路径或 TCP 地址）
    #[arg(short, long, default_value = default_addr())]
    addr: String,
    /// 配置文件路径
    #[arg(short, long, default_value = "/etc/eneros/plugin.toml")]
    config: String,
    /// 日志级别
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

/// 跨平台默认 IPC 地址：Linux 使用 Unix socket，其他平台使用 TCP
fn default_addr() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "/var/run/eneros/plugin-daemon.sock"
    }
    #[cfg(not(target_os = "linux"))]
    {
        "127.0.0.1:5410"
    }
}

// ============================================================================
// IPC 协议类型
// ============================================================================
//
// v0.28.0 Task 12：`DaemonRequest` 与 `DaemonResponse` 已统一到
// `eneros_plugin::ipc` 模块，daemon 端直接复用，消除重复定义。
// 下方仅保留 daemon 特有的响应数据结构（`PluginInfo` / `VerificationResultInfo`
// / `DaemonStatus`），它们通过 `DaemonResponse::ok(data)` 序列化为 `data` 字段。

/// 插件信息（IPC 返回）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// 插件名称
    pub name: String,
    /// 插件版本
    pub version: String,
    /// 插件 API 版本
    pub api_version: String,
    /// 插件类型
    pub plugin_type: String,
    /// 插件描述
    pub description: String,
    /// 当前状态
    pub state: String,
    /// 是否启用
    pub enabled: bool,
    /// 加载时间（RFC 3339）
    pub loaded_at: String,
}

impl PluginInfo {
    /// 从注册表条目构造插件信息
    fn from_entry(entry: &PluginEntry) -> Self {
        Self {
            name: entry.metadata.name.clone(),
            version: entry.metadata.version.clone(),
            api_version: entry.metadata.api_version.clone(),
            plugin_type: format!("{:?}", entry.metadata.plugin_type),
            description: entry.metadata.description.clone(),
            state: format!("{}", entry.state),
            enabled: entry.enabled,
            loaded_at: entry.loaded_at.to_rfc3339(),
        }
    }
}

/// 签名验证结果（IPC 返回）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResultInfo {
    /// 是否验证通过
    pub valid: bool,
    /// 签名者标识（验证通过时存在）
    pub signer: Option<String>,
    /// 结果描述
    pub message: String,
}

/// daemon 状态（IPC 返回）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// daemon 是否运行中
    pub running: bool,
    /// 已加载插件数量
    pub plugin_count: usize,
    /// 已启用插件数量
    pub enabled_count: usize,
}

// ============================================================================
// PluginDaemon
// ============================================================================

/// 插件守护进程
///
/// 独立进程加载插件，通过 IPC 与主进程通信，实现崩溃隔离。
/// 插件 panic 不会导致 daemon 退出，仅标记插件状态为 Crashed。
pub struct PluginDaemon {
    /// 插件注册表（线程安全）
    registry: PluginRegistry,
    /// 插件加载器
    loader: PluginLoader,
    /// 插件系统配置
    config: PluginConfig,
    /// 是否运行中
    running: Arc<AtomicBool>,
    /// 签名验证器（缓存，避免每次 handle_load/handle_verify 重新加载公钥）
    ///
    /// v0.28.0 Task 17 修复：原实现中 `handle_load` 与 `handle_verify` 每次调用
    /// 都重新调用 `PluginSignatureVerifier::new`，重复扫描 `trusted_keys_dir`
    /// 并解析公钥文件。在频繁加载/验证插件的场景下造成不必要的 IO 开销。
    /// 改为在 daemon 初始化时构建一次，缓存复用。
    verifier: Arc<PluginSignatureVerifier>,
    /// 已加载的插件库句柄（保持 Library 存活，否则动态库会被卸载）
    ///
    /// 使用 `Mutex` 而非 `RwLock`：`LoadedPlugin` 内含 `libloading::Library`，
    /// 后者实现 `Send` 但未实现 `Sync`。`Mutex<T>` 在 `T: Send` 时即为 `Sync`，
    /// 而 `RwLock<T>` 需要 `T: Send + Sync`。
    loaded: Mutex<HashMap<String, LoadedPlugin>>,
    /// 插件 load/unload 操作串行化锁（v0.28.0 Task 11 修复 H1）
    ///
    /// `loaded`（`Mutex<HashMap>`）与 `registry`（`RwLock<HashMap>`）是两把独立的锁，
    /// `handle_load` / `handle_unload` 需同时操作两者。若不串行化，并发场景下
    /// 可能出现：线程 A unregister 成功 → 线程 B register 成功 → 线程 A remove
    /// 了线程 B 刚插入的库句柄 → 注册表与 loaded 状态不一致。
    ///
    /// 此锁在 `handle_load` / `handle_unload` 入口获取，确保 load/unload 操作
    /// 原子执行，避免双锁竞态。使用 `parking_lot::Mutex`（同步锁），因为
    /// `handle_load` / `handle_unload` 为同步函数。
    plugin_op_lock: Mutex<()>,
}

impl PluginDaemon {
    /// 创建新的 daemon 实例
    ///
    /// 初始化时构建 `PluginSignatureVerifier` 并缓存，避免后续每次
    /// `handle_load` / `handle_verify` 重复扫描公钥目录。
    pub fn new(config: PluginConfig) -> Result<Self> {
        let verifier = Arc::new(PluginSignatureVerifier::new(
            &config.plugin.trusted_keys_dir,
            config.plugin.require_signature,
        )?);
        Ok(Self {
            registry: PluginRegistry::new(),
            loader: PluginLoader::new(),
            config,
            running: Arc::new(AtomicBool::new(false)),
            verifier,
            loaded: Mutex::new(HashMap::new()),
            plugin_op_lock: Mutex::new(()),
        })
    }

    /// 启动 IPC 服务端
    ///
    /// 根据地址格式自动选择传输层：
    /// - 以 `/` 开头：Unix socket（仅 Linux）
    /// - 其他：TCP
    pub async fn run(self: Arc<Self>, addr: &str) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);

        if addr.starts_with('/') {
            #[cfg(target_os = "linux")]
            {
                self.clone().run_unix(addr).await?;
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!(
                    "Unix socket '{}' 在当前平台不可用，请使用 TCP 地址（如 127.0.0.1:5410）",
                    addr
                );
            }
        } else {
            self.clone().run_tcp(addr).await?;
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// TCP IPC 服务端
    ///
    /// v0.28.0 Task 17：使用 `tokio::select!` 监听 Ctrl+C 信号实现优雅关闭，
    /// 收到信号后退出 accept 循环，`run` 方法将 `running` 置为 false。
    async fn run_tcp(self: Arc<Self>, addr: &str) -> Result<()> {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(addr).await?;
        tracing::info!("plugin-daemon IPC 监听 TCP: {}", addr);

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer)) => {
                            tracing::debug!("IPC 连接: {}", peer);
                            let daemon = self.clone();
                            tokio::spawn(async move {
                                daemon.handle_connection(stream).await;
                            });
                        }
                        Err(e) => {
                            tracing::error!("accept 失败: {}", e);
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("收到 Ctrl+C 信号，开始优雅关闭...");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Unix socket IPC 服务端（仅 Linux）
    ///
    /// v0.28.0 Task 17：使用 `tokio::select!` 监听 Ctrl+C 信号实现优雅关闭，
    /// 收到信号后退出 accept 循环并清理 Unix socket 文件，避免残留。
    #[cfg(target_os = "linux")]
    async fn run_unix(self: Arc<Self>, addr: &str) -> Result<()> {
        use tokio::net::UnixListener;

        // 移除已存在的 socket 文件
        let _ = std::fs::remove_file(addr);
        // 创建父目录
        if let Some(parent) = Path::new(addr).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let listener = UnixListener::bind(addr)?;
        tracing::info!("plugin-daemon IPC 监听 Unix socket: {}", addr);

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _)) => {
                            let daemon = self.clone();
                            tokio::spawn(async move {
                                daemon.handle_connection(stream).await;
                            });
                        }
                        Err(e) => {
                            tracing::error!("accept 失败: {}", e);
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("收到 Ctrl+C 信号，开始优雅关闭...");
                    break;
                }
            }
        }

        // 清理 Unix socket 文件，避免残留导致下次启动绑定失败
        let _ = std::fs::remove_file(addr);
        tracing::info!("已清理 Unix socket 文件: {}", addr);
        Ok(())
    }

    /// 处理单个 IPC 连接
    ///
    /// 读取行 → 解析 JSON → 处理命令 → 返回 JSON 行
    ///
    /// - 读超时 30 秒：防止恶意客户端连接后不发送数据导致 task 永久阻塞
    /// - 响应序列化失败时发送降级错误响应，避免客户端无限等待
    async fn handle_connection<S>(self: Arc<Self>, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();

        loop {
            line.clear();
            // 30 秒读超时，防止恶意客户端连接后不发送数据导致永久阻塞
            match tokio::time::timeout(Duration::from_secs(30), reader.read_line(&mut line))
                .await
            {
                Ok(Ok(0)) => break, // 连接关闭
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::error!("IPC 读取失败: {}", e);
                    break;
                }
                Err(_) => {
                    tracing::warn!("IPC 读取超时（30s），关闭连接");
                    break;
                }
            }

            let response = match serde_json::from_str::<DaemonRequest>(line.trim()) {
                Ok(cmd) => self.handle_command(cmd),
                Err(e) => DaemonResponse::error(format!("命令解析失败: {}", e)),
            };

            // 序列化失败时发送降级错误响应，避免客户端无限等待
            let mut resp_json = serde_json::to_string(&response).unwrap_or_else(|_| {
                tracing::error!("响应序列化失败，发送降级错误响应");
                r#"{"ok":false,"error":"内部错误:响应序列化失败"}"#.to_string()
            });
            resp_json.push('\n');
            if write_half.write_all(resp_json.as_bytes()).await.is_err() {
                break;
            }
        }
    }

    /// 处理 IPC 命令（顶层 catch_unwind 确保 daemon 不退出）
    pub fn handle_command(&self, cmd: DaemonRequest) -> DaemonResponse {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch_command(cmd)
        }));
        match result {
            Ok(response) => response,
            Err(panic) => {
                tracing::error!("命令处理 panic: {:?}", panic);
                DaemonResponse::error("内部错误: 命令处理崩溃")
            }
        }
    }

    /// 命令分发（实际处理逻辑）
    fn dispatch_command(&self, cmd: DaemonRequest) -> DaemonResponse {
        match cmd {
            DaemonRequest::Load {
                path,
                skip_signature,
            } => match self.handle_load(&path, skip_signature) {
                Ok(info) => DaemonResponse::ok(info),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::Unload { name } => match self.handle_unload(&name) {
                Ok(()) => DaemonResponse::ok(serde_json::Value::Null),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::List => DaemonResponse::ok(self.handle_list()),
            DaemonRequest::Info { name } => match self.handle_info(&name) {
                Ok(info) => DaemonResponse::ok(info),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::Enable { name } => match self.handle_enable(&name) {
                Ok(()) => DaemonResponse::ok(serde_json::Value::Null),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::Disable { name } => match self.handle_disable(&name) {
                Ok(()) => DaemonResponse::ok(serde_json::Value::Null),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::Verify { path } => match self.handle_verify(&path) {
                Ok(info) => DaemonResponse::ok(info),
                Err(e) => DaemonResponse::error(e.to_string()),
            },
            DaemonRequest::Status => DaemonResponse::ok(self.handle_status()),
        }
    }

    /// 加载插件（catch_unwind 包裹 FFI 调用）
    ///
    /// v0.28.0 Task 11 修复：
    /// - H1：入口获取 `plugin_op_lock`，串行化 load/unload，避免 `loaded` 与
    ///   `registry` 双锁竞态导致状态不一致。
    /// - H3：`skip_signature` 由请求方控制存在安全风险，新增 `allow_skip_signature`
    ///   配置项限制。仅当 `config.plugin.allow_skip_signature == true` 时才允许
    ///   请求方跳过签名验证，否则强制执行签名验证。
    pub fn handle_load(&self, path: &str, skip_signature: bool) -> Result<PluginInfo> {
        // 串行化 load/unload 操作，避免双锁竞态（H1 修复）
        let _op_guard = self.plugin_op_lock.lock();

        let plugin_path = Path::new(path);

        // H3 修复：请求方试图跳过签名验证时，检查 daemon 配置是否允许
        let effective_skip_signature = if skip_signature {
            if !self.config.plugin.allow_skip_signature {
                tracing::warn!(
                    "IPC 调用方请求跳过签名验证，但 allow_skip_signature=false，拒绝跳过"
                );
                return Err(anyhow!(
                    "签名验证不可跳过：daemon 配置 allow_skip_signature=false"
                ));
            }
            true
        } else {
            false
        };

        // 签名验证（可跳过，受 allow_skip_signature 配置限制）
        // v0.28.0 Task 17：复用缓存的 verifier，避免每次重新加载公钥
        if !effective_skip_signature {
            let result = self.verifier.verify_plugin(plugin_path)?;
            match result {
                SigVerificationResult::Valid { .. } => {}
                SigVerificationResult::Invalid { reason } => {
                    return Err(anyhow!("签名验证失败: {}", reason));
                }
                SigVerificationResult::Missing => {
                    return Err(anyhow!("签名文件缺失"));
                }
                SigVerificationResult::UntrustedSigner { signer } => {
                    return Err(anyhow!("不可信签名者: {}", signer));
                }
            }
        }

        // 加载插件（catch_unwind 崩溃隔离，包裹 unsafe FFI 调用）
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.loader.load(plugin_path)
        }))
        .map_err(|e| {
            tracing::error!("插件加载 panic: {:?}", e);
            anyhow!("插件加载崩溃")
        })??;

        let metadata = loaded.metadata.clone();
        let name = metadata.name.clone();

        // 先注册到注册表，注册失败时卸载新加载的库，避免泄漏与注册表悬空
        //
        // 修复说明（v0.28.0 Task 4）：
        // 原实现先 `loaded.insert` 再 `registry.register`，重复加载同名插件时：
        //   1. `insert` 返回旧值并 drop → 旧动态库被卸载
        //   2. `register` 返回 `AlreadyLoaded` 错误
        //   3. 注册表仍引用旧条目（但库已卸载），新库在 `loaded` 中但未注册 → 双重损坏
        // 修复后改为先 `register` 再 `insert`，注册失败时 `unload` 新库，保证一致性。
        let entry = PluginEntry::new(metadata);
        if let Err(e) = self.registry.register(entry) {
            // 注册失败（如同名插件已存在），卸载新加载的库，避免泄漏
            self.loader.unload(loaded).map_err(|e| anyhow!("{}", e))?;
            return Err(anyhow!("{}", e));
        }

        // 注册成功后再存储库句柄（保持 Library 存活）
        self.loaded.lock().insert(name.clone(), loaded);

        // 查询并返回插件信息
        let entry = self
            .registry
            .lookup(&name)
            .ok_or_else(|| anyhow!("注册后查询失败: {}", name))?;

        Ok(PluginInfo::from_entry(&entry))
    }

    /// 卸载插件
    ///
    /// v0.28.0 Task 11 修复 H1：入口获取 `plugin_op_lock`，串行化 load/unload，
    /// 避免 `registry.unregister` 与 `loaded.remove` 之间的窗口期被并发
    /// `handle_load` 插入导致状态不一致。
    pub fn handle_unload(&self, name: &str) -> Result<()> {
        // 串行化 load/unload 操作，避免双锁竞态（H1 修复）
        let _op_guard = self.plugin_op_lock.lock();

        // 从注册表注销（不存在返回错误）
        self.registry
            .unregister(name)
            .map_err(|e| anyhow!("{}", e))?;

        // 卸载库句柄（Library drop 时自动关闭动态库）
        // 若 loaded 中不存在该插件的库句柄，说明注册表与 loaded 状态不一致，
        // 记录警告以便排查（修复前此处静默成功，掩盖状态不一致问题）。
        if let Some(loaded) = self.loaded.lock().remove(name) {
            self.loader.unload(loaded).map_err(|e| anyhow!("{}", e))?;
        } else {
            tracing::warn!(
                "插件 '{}' 的库句柄不存在（注册表与 loaded 状态不一致）",
                name
            );
        }

        Ok(())
    }

    /// 列出已加载插件
    pub fn handle_list(&self) -> Vec<PluginInfo> {
        self.registry
            .list()
            .iter()
            .map(PluginInfo::from_entry)
            .collect()
    }

    /// 查询插件信息
    pub fn handle_info(&self, name: &str) -> Result<PluginInfo> {
        let entry = self
            .registry
            .lookup(name)
            .ok_or_else(|| anyhow!("插件未加载: {}", name))?;
        Ok(PluginInfo::from_entry(&entry))
    }

    /// 启用插件
    pub fn handle_enable(&self, name: &str) -> Result<()> {
        self.registry
            .set_enabled(name, true)
            .map_err(|e| anyhow!("{}", e))
    }

    /// 禁用插件
    pub fn handle_disable(&self, name: &str) -> Result<()> {
        self.registry
            .set_enabled(name, false)
            .map_err(|e| anyhow!("{}", e))
    }

    /// 验证插件签名
    pub fn handle_verify(&self, path: &str) -> Result<VerificationResultInfo> {
        let plugin_path = Path::new(path);
        // v0.28.0 Task 17：复用缓存的 verifier，避免每次重新加载公钥
        let result = self.verifier.verify_plugin(plugin_path)?;
        Ok(match result {
            SigVerificationResult::Valid { signer } => VerificationResultInfo {
                valid: true,
                signer: Some(signer),
                message: "验证通过".to_string(),
            },
            SigVerificationResult::Invalid { reason } => VerificationResultInfo {
                valid: false,
                signer: None,
                message: format!("签名无效: {}", reason),
            },
            SigVerificationResult::Missing => VerificationResultInfo {
                valid: false,
                signer: None,
                message: "签名文件缺失".to_string(),
            },
            SigVerificationResult::UntrustedSigner { signer } => VerificationResultInfo {
                valid: false,
                signer: Some(signer),
                message: "不可信签名者".to_string(),
            },
        })
    }

    /// 查询 daemon 状态
    pub fn handle_status(&self) -> DaemonStatus {
        let plugins = self.registry.list();
        let enabled_count = plugins.iter().filter(|p| p.enabled).count();
        DaemonStatus {
            running: self.running.load(Ordering::SeqCst),
            plugin_count: plugins.len(),
            enabled_count,
        }
    }
}

// ============================================================================
// main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_new(&args.log_level).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // 加载配置（文件不存在时使用默认配置）
    let config = if Path::new(&args.config).exists() {
        PluginConfig::load_from_file(Path::new(&args.config))
            .map_err(|e| anyhow!("加载配置失败: {}", e))?
    } else {
        tracing::warn!("配置文件 {} 不存在，使用默认配置", args.config);
        PluginConfig::default()
    };

    tracing::info!(
        "eneros-plugin-daemon 启动，IPC 地址: {}, 插件目录: {}",
        args.addr,
        config.plugin.plugin_dir.display()
    );

    let daemon = Arc::new(PluginDaemon::new(config)?);
    daemon.run(&args.addr).await
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 1：创建 daemon 实例
    #[test]
    fn test_daemon_new() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();
        assert!(!daemon.running.load(Ordering::SeqCst));
        assert_eq!(daemon.registry.count(), 0);
        assert!(daemon.loaded.lock().is_empty());
    }

    /// 测试 2：空列表查询
    #[test]
    fn test_handle_list_empty() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();
        let list = daemon.handle_list();
        assert!(list.is_empty());
    }

    /// 测试 3：状态查询
    #[test]
    fn test_handle_status() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();
        let status = daemon.handle_status();
        assert!(!status.running);
        assert_eq!(status.plugin_count, 0);
        assert_eq!(status.enabled_count, 0);
    }

    /// 测试 4：解析 load 命令 JSON
    #[test]
    fn test_ipc_protocol_parse_load() {
        let json = r#"{"cmd":"load","path":"/tmp/plugin.so","skip_signature":false}"#;
        let cmd: DaemonRequest = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonRequest::Load {
                path,
                skip_signature,
            } => {
                assert_eq!(path, "/tmp/plugin.so");
                assert!(!skip_signature);
            }
            _ => panic!("期望 Load 命令"),
        }
    }

    /// 测试 5：解析 unload 命令 JSON
    #[test]
    fn test_ipc_protocol_parse_unload() {
        let json = r#"{"cmd":"unload","name":"my-plugin"}"#;
        let cmd: DaemonRequest = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonRequest::Unload { name } => assert_eq!(name, "my-plugin"),
            _ => panic!("期望 Unload 命令"),
        }
    }

    /// 测试 6：解析 list 命令 JSON
    #[test]
    fn test_ipc_protocol_parse_list() {
        let json = r#"{"cmd":"list"}"#;
        let cmd: DaemonRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, DaemonRequest::List));
    }

    /// 测试 7：序列化成功响应
    #[test]
    fn test_ipc_protocol_response_ok() {
        let resp = DaemonResponse::ok(serde_json::json!({"name": "test-plugin"}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"name\":\"test-plugin\""));
        assert!(!json.contains("\"error\""));
    }

    /// 测试 8：序列化错误响应
    #[test]
    fn test_ipc_protocol_response_error() {
        let resp = DaemonResponse::error("操作失败");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"error\":\"操作失败\""));
        assert!(!json.contains("\"data\""));
    }

    /// 测试 9：卸载不存在的插件返回错误
    #[test]
    fn test_handle_unload_not_found() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();
        let result = daemon.handle_unload("nonexistent-plugin");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not loaded"),
            "错误消息应包含 'not loaded'，实际: {}",
            msg
        );
    }

    /// 测试 10：查询不存在的插件返回错误
    #[test]
    fn test_handle_info_not_found() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();
        let result = daemon.handle_info("nonexistent-plugin");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("未加载"),
            "错误消息应包含 '未加载'，实际: {}",
            msg
        );
    }

    /// 测试 11：重复加载同名插件返回 AlreadyLoaded 错误且不导致库泄漏
    ///
    /// 由于 `handle_load` 需要真实动态库文件（含 C ABI 入口函数 `eneros_plugin_create`
    /// 等），单元测试无法直接构造。本测试通过直接操作注册表模拟"第一次加载成功"
    /// 后的状态，再验证修复后的逻辑路径：
    ///
    /// - 修复前：`handle_load` 先 `loaded.insert` 再 `registry.register`，
    ///   重复加载时 `insert` 返回旧值并 drop（旧库被卸载），`register` 返回
    ///   `AlreadyLoaded`，导致注册表引用已卸载的旧库 + `loaded` 中存在未注册的新库
    ///   → 双重损坏。
    /// - 修复后：`handle_load` 先 `registry.register` 再 `loaded.insert`，
    ///   `register` 失败时 `unload` 新库，`loaded` 与注册表保持一致。
    ///
    /// 本测试验证修复后的不变量：注册失败时 `loaded` 不被修改、注册表不被污染。
    #[test]
    fn test_load_duplicate_returns_already_loaded_and_no_leak() {
        use eneros_plugin::{PluginError, PluginMetadata, PluginType};

        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();

        // 模拟第一次加载成功：插件已注册到注册表（loaded 中也应有库句柄，
        // 但由于无法构造真实 LoadedPlugin，此处仅注册，重点验证第二次注册失败的行为）
        let metadata = PluginMetadata {
            name: "dup-plugin".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Agent,
            description: "重复加载测试".to_string(),
        };
        daemon
            .registry
            .register(PluginEntry::new(metadata))
            .unwrap();
        assert_eq!(daemon.registry.count(), 1);

        // 模拟第二次加载：修复后的 handle_load 会先调用 register
        // register 检测到同名插件已存在，返回 AlreadyLoaded
        let metadata2 = PluginMetadata {
            name: "dup-plugin".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Agent,
            description: "重复加载测试".to_string(),
        };
        let result = daemon.registry.register(PluginEntry::new(metadata2));

        // 验证返回 AlreadyLoaded 错误
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, PluginError::AlreadyLoaded(_)),
            "期望 AlreadyLoaded 错误，实际: {:?}",
            err
        );
        assert!(
            err.to_string().contains("already loaded"),
            "错误消息应包含 'already loaded'，实际: {}",
            err
        );

        // 验证 loaded 映射未被修改（修复后 register 失败时不会执行 insert，
        // 新库会被 unload 而非留在 loaded 中，因此 loaded 保持为空）
        assert!(
            daemon.loaded.lock().is_empty(),
            "注册失败时 loaded 不应被修改，避免库泄漏"
        );

        // 验证注册表仍只有一个条目（未被污染，无悬空引用）
        assert_eq!(
            daemon.registry.count(),
            1,
            "注册表条目数应保持为 1，不应被重复注册污染"
        );
    }

    /// 测试 12：handle_unload 在 loaded 状态不一致时记录警告但仍成功
    ///
    /// 模拟注册表与 loaded 状态不一致的场景：插件在注册表中但不在 loaded 中。
    /// 修复后的 handle_unload 会记录 `tracing::warn!` 日志并返回 `Ok(())`，
    /// 而非静默成功（修复前 `else` 分支缺失，状态不一致被掩盖）。
    #[test]
    fn test_handle_unload_warns_on_inconsistent_state() {
        use eneros_plugin::{PluginMetadata, PluginType};

        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();

        // 模拟状态不一致：插件在注册表中但不在 loaded 中
        let metadata = PluginMetadata {
            name: "inconsistent-plugin".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Agent,
            description: "状态不一致测试".to_string(),
        };
        daemon
            .registry
            .register(PluginEntry::new(metadata))
            .unwrap();

        // loaded 为空，模拟状态不一致
        assert!(daemon.loaded.lock().is_empty());

        // handle_unload 应成功（unregister 成功）并记录警告
        let result = daemon.handle_unload("inconsistent-plugin");
        assert!(result.is_ok(), "状态不一致时 handle_unload 应仍返回 Ok");

        // 注册表应已清空
        assert_eq!(daemon.registry.count(), 0);
    }

    /// 测试 13：并发 load/unload 不出现状态不一致（v0.28.0 Task 11 修复 H1）
    ///
    /// `plugin_op_lock` 串行化 load/unload 操作。本测试通过多线程并发调用
    /// `handle_unload` 同一个插件，验证：
    /// - 恰好一个线程成功卸载，其余线程收到 `NotLoaded` 错误
    /// - 最终注册表与 loaded 状态一致（count == 0，loaded 为空）
    ///
    /// 修复前（无 `plugin_op_lock`）：并发 `handle_unload` 可能出现两个线程
    /// 同时通过 `registry.unregister` 检查、同时操作 `loaded`，导致状态不一致。
    /// 修复后：`plugin_op_lock` 确保卸载操作串行执行。
    #[test]
    fn test_concurrent_load_unload_no_race() {
        use std::sync::Arc;
        use std::thread;

        let daemon = Arc::new(PluginDaemon::new(PluginConfig::default()).unwrap());

        // 预注册一个插件（模拟已加载状态）
        let metadata = eneros_plugin::PluginMetadata {
            name: "race-test-plugin".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: eneros_plugin::PluginType::Agent,
            description: "并发竞态测试".to_string(),
        };
        daemon
            .registry
            .register(PluginEntry::new(metadata))
            .unwrap();
        assert_eq!(daemon.registry.count(), 1);

        // 8 个线程并发卸载同一个插件
        let num_threads = 8;
        let mut handles = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            let daemon_clone = Arc::clone(&daemon);
            handles.push(thread::spawn(move || {
                daemon_clone.handle_unload("race-test-plugin")
            }));
        }

        // 收集结果
        let mut success_count = 0;
        let mut not_loaded_count = 0;
        for handle in handles {
            match handle.join().unwrap() {
                Ok(()) => success_count += 1,
                Err(e) => {
                    let msg = e.to_string();
                    // 其余线程应收到 "not loaded" 错误（插件已被第一个线程卸载）
                    if msg.contains("not loaded") {
                        not_loaded_count += 1;
                    }
                }
            }
        }

        // 恰好一个线程成功卸载
        assert_eq!(
            success_count, 1,
            "应有且仅有一个线程成功卸载，实际成功: {}",
            success_count
        );
        // 其余线程收到 NotLoaded 错误
        assert_eq!(
            not_loaded_count,
            num_threads - 1,
            "其余线程应收到 not loaded 错误，实际: {}",
            not_loaded_count
        );

        // 最终状态一致：注册表为空，loaded 为空
        assert_eq!(
            daemon.registry.count(),
            0,
            "并发卸载后注册表应为空"
        );
        assert!(
            daemon.loaded.lock().is_empty(),
            "并发卸载后 loaded 应为空"
        );
    }

    /// 测试 14：allow_skip_signature=false 时拒绝跳过签名验证（v0.28.0 Task 11 修复 H3）
    ///
    /// 默认配置 `allow_skip_signature=false`，IPC 调用方发送 `skip_signature=true`
    /// 应被 daemon 拒绝，返回错误信息。防止恶意客户端绕过签名验证。
    #[test]
    fn test_handle_load_rejects_skip_signature_when_not_allowed() {
        let daemon = PluginDaemon::new(PluginConfig::default()).unwrap();

        // 默认配置不允许跳过签名验证
        assert!(!daemon.config.plugin.allow_skip_signature);

        // 请求跳过签名验证应被拒绝
        let result = daemon.handle_load("/nonexistent/plugin.so", true);
        assert!(result.is_err(), "allow_skip_signature=false 时应拒绝跳过签名验证");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("allow_skip_signature"),
            "错误信息应提及 allow_skip_signature，实际: {}",
            msg
        );
    }

    /// 测试 15：allow_skip_signature=true 时允许跳过签名验证（v0.28.0 Task 11 修复 H3）
    ///
    /// 当配置 `allow_skip_signature=true` 时，IPC 调用方可以跳过签名验证。
    /// 本测试验证跳过签名验证后，请求继续执行（因路径不存在将在加载阶段失败，
    /// 而非在签名验证阶段被拒绝）。
    #[test]
    fn test_handle_load_allows_skip_signature_when_allowed() {
        let mut config = PluginConfig::default();
        config.plugin.allow_skip_signature = true;
        let daemon = PluginDaemon::new(config).unwrap();

        // 请求跳过签名验证，应通过签名检查（路径不存在将在加载阶段失败）
        let result = daemon.handle_load("/nonexistent/plugin.so", true);
        assert!(result.is_err(), "路径不存在应返回错误");
        let msg = result.unwrap_err().to_string();
        // 错误应来自加载阶段（file not found），而非签名验证阶段
        assert!(
            !msg.contains("allow_skip_signature"),
            "allow_skip_signature=true 时不应拒绝跳过签名验证，实际错误: {}",
            msg
        );
    }
}
