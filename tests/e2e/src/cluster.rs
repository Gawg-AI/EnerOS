//! 集群启动器 — 管理 EnerOS 多进程集群的生命周期。
//!
//! 在本地模式下，通过 `tokio::process::Command` 启动 API、Gateway、Broker
//! 二进制，并轮询健康检查端点直到集群就绪。关闭时优雅终止所有子进程。
//!
//! ## 端口分配
//!
//! 从 `18000` 开始递增分配端口，避免与开发环境常驻服务冲突。
//! 每个集群实例占用 3 个端口（API、Gateway、Broker）。

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};

/// 集群部署模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterMode {
    /// 本地进程组 — 直接在本机启动各组件二进制。
    Local,
    /// Docker 容器组 — 通过 testcontainers 启动（feature gate）。
    #[allow(dead_code)]
    Docker,
}

/// 集群配置 — 控制节点数、端口范围和部署模式。
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// 部署模式
    pub mode: ClusterMode,
    /// 起始端口号（默认 18000）
    pub base_port: u16,
    /// 启动超时
    pub startup_timeout: Duration,
    /// 健康检查轮询间隔
    pub health_poll_interval: Duration,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            mode: ClusterMode::Local,
            base_port: 18000,
            startup_timeout: Duration::from_secs(30),
            health_poll_interval: Duration::from_millis(500),
        }
    }
}

/// 运行中的测试集群 — 持有所有子进程的句柄。
///
/// 实现 `Drop` trait 确保进程不泄漏：即使测试 panic，也会尝试终止子进程。
pub struct TestCluster {
    api_process: Option<Child>,
    gateway_process: Option<Child>,
    broker_process: Option<Child>,
    api_endpoint: String,
    gateway_endpoint: String,
    broker_endpoint: String,
}

impl TestCluster {
    /// 启动集群。
    ///
    /// 在本地模式下，依次启动 Broker → Gateway → API 二进制，
    /// 然后轮询 API `/health` 端点直到返回 200 或超时。
    ///
    /// 如果二进制不存在，返回 `Err`，调用方可选择跳过测试。
    pub async fn start(config: ClusterConfig) -> Result<Self> {
        let api_port = config.base_port;
        let gateway_port = config.base_port + 1;
        let broker_port = config.base_port + 2;

        let api_endpoint = format!("http://127.0.0.1:{}", api_port);
        let gateway_endpoint = format!("127.0.0.1:{}", gateway_port);
        let broker_endpoint = format!("127.0.0.1:{}", broker_port);

        let broker_bin = resolve_binary("eneros-broker")?;
        let gateway_bin = resolve_binary("eneros-gateway")?;
        let api_bin = resolve_binary("eneros-api")?;

        // 1. 启动 Broker
        let broker_process = Command::new(&broker_bin)
            .arg("--bind")
            .arg(&broker_endpoint)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn {}", broker_bin.display()))?;

        // 2. 启动 Gateway
        let gateway_process = Command::new(&gateway_bin)
            .arg("--bind")
            .arg(&gateway_endpoint)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn {}", gateway_bin.display()))?;

        // 3. 启动 API
        let api_process = Command::new(&api_bin)
            .arg("run")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(api_port.to_string())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn {}", api_bin.display()))?;

        let mut cluster = Self {
            api_process: Some(api_process),
            gateway_process: Some(gateway_process),
            broker_process: Some(broker_process),
            api_endpoint,
            gateway_endpoint,
            broker_endpoint,
        };

        match cluster
            .wait_for_health(config.startup_timeout, config.health_poll_interval)
            .await
        {
            Ok(()) => Ok(cluster),
            Err(e) => {
                cluster.shutdown().await;
                Err(e)
            }
        }
    }

    /// 轮询 API `/health` 端点直到返回 200 或超时。
    async fn wait_for_health(
        &self,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;

        let deadline = tokio::time::Instant::now() + timeout;
        let health_url = format!("{}/health", self.api_endpoint);

        loop {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "API health check timed out after {:?} (endpoint={})",
                    timeout,
                    health_url
                );
            }

            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                _ => {}
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// 返回 API HTTP 端点 URL（如 `http://127.0.0.1:18000`）。
    pub fn api_endpoint(&self) -> &str {
        &self.api_endpoint
    }

    /// 返回 Gateway IPC 端点地址（如 `127.0.0.1:18001`）。
    pub fn gateway_endpoint(&self) -> &str {
        &self.gateway_endpoint
    }

    /// 返回 Broker TCP 端点地址（如 `127.0.0.1:18002`）。
    pub fn broker_endpoint(&self) -> &str {
        &self.broker_endpoint
    }

    /// 优雅关闭所有进程。
    ///
    /// 先尝试发送终止信号，等待 3 秒后强制杀死。
    pub async fn shutdown(&mut self) {
        // 按启动的逆序关闭：API → Gateway → Broker
        if let Some(mut child) = self.api_process.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = child.kill().await;
        }
        if let Some(mut child) = self.gateway_process.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = child.kill().await;
        }
        if let Some(mut child) = self.broker_process.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = child.kill().await;
        }
    }
}

impl Drop for TestCluster {
    fn drop(&mut self) {
        // 同步 Drop 中无法 .await，直接 start_kill 作为兜底
        if let Some(mut child) = self.api_process.take() {
            let _ = child.start_kill();
        }
        if let Some(mut child) = self.gateway_process.take() {
            let _ = child.start_kill();
        }
        if let Some(mut child) = self.broker_process.take() {
            let _ = child.start_kill();
        }
    }
}

/// 解析工作空间构建产物的二进制路径。
///
/// 查找顺序：`target/release/` → `target/debug/`。
/// Windows 下自动追加 `.exe` 后缀。
fn resolve_binary(name: &str) -> Result<PathBuf> {
    let exe_name = if cfg!(windows) {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .context("failed to resolve workspace root")?;

    for profile in &["release", "debug"] {
        let candidate = workspace_root.join("target").join(profile).join(&exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "binary '{}' not found in target/release or target/debug (run `cargo build --workspace` first)",
        name
    )
}
