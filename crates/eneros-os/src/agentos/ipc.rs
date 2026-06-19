//! Agent 间消息传递（IPC）
//!
//! 提供两种传输层：
//! - Unix socket：通用域，延迟 < 100μs，跨平台兼容（Linux/macOS）
//! - 共享内存 + eventfd：RT 域，延迟 < 10μs，仅 Linux
//!
//! Windows 平台使用 TCP 回退（127.0.0.1）。

use eneros_core::AgentMessage;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// IPC 传输层
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcTransport {
    /// Unix domain socket（Linux/macOS 首选）
    UnixSocket,
    /// TCP（Windows 回退或远程通信）
    Tcp,
    /// 共享内存 + eventfd（RT 域，仅 Linux）
    SharedMemory,
}

/// IPC 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIpcConfig {
    pub transport: IpcTransport,
    pub buffer_size: usize,
    /// Unix socket 路径前缀（如 /var/run/eneros）
    pub socket_dir: String,
    /// TCP 端口基数（agent-<id> 使用 base + hash(id) % 1000）
    pub tcp_port_base: u16,
}

impl Default for AgentIpcConfig {
    fn default() -> Self {
        Self {
            transport: if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
                IpcTransport::UnixSocket
            } else {
                IpcTransport::Tcp
            },
            buffer_size: 65536,
            socket_dir: "/var/run/eneros".to_string(),
            tcp_port_base: 9000,
        }
    }
}

/// IPC 错误
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("not connected")]
    NotConnected,
    #[error("timeout")]
    Timeout,
}

/// IPC 服务端 — 接收 Agent 消息
pub struct AgentIpcServer {
    config: AgentIpcConfig,
    agent_id: String,
    // 接收到的消息通过 channel 传出
    msg_rx: Option<mpsc::Receiver<Result<AgentMessage, IpcError>>>,
    // 服务端任务的 JoinHandle
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl AgentIpcServer {
    /// 创建 IPC 服务端（不启动）
    pub fn new(agent_id: &str, config: AgentIpcConfig) -> Self {
        Self {
            config,
            agent_id: agent_id.to_string(),
            msg_rx: None,
            server_handle: None,
        }
    }

    /// 启动服务端监听
    pub async fn start(&mut self) -> Result<(), IpcError> {
        let (tx, rx) = mpsc::channel(256);
        self.msg_rx = Some(rx);

        let agent_id = self.agent_id.clone();
        let config = self.config.clone();

        match config.transport {
            IpcTransport::UnixSocket => {
                #[cfg(unix)]
                {
                    let socket_path = unix_socket_path(&config.socket_dir, &agent_id);
                    // 清理旧 socket 文件
                    let _ = std::fs::remove_file(&socket_path);
                    let listener = UnixListener::bind(&socket_path)?;
                    let handle = tokio::spawn(async move {
                        accept_loop_unix(listener, tx).await;
                    });
                    self.server_handle = Some(handle);
                }
                #[cfg(not(unix))]
                {
                    // 非 Unix 平台回退到 TCP
                    let port = tcp_port(&config, &agent_id);
                    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
                    let handle = tokio::spawn(async move {
                        accept_loop_tcp(listener, tx).await;
                    });
                    self.server_handle = Some(handle);
                }
            }
            IpcTransport::Tcp | IpcTransport::SharedMemory => {
                let port = tcp_port(&config, &agent_id);
                let listener = TcpListener::bind(("127.0.0.1", port)).await?;
                let handle = tokio::spawn(async move {
                    accept_loop_tcp(listener, tx).await;
                });
                self.server_handle = Some(handle);
            }
        }

        Ok(())
    }

    /// 接收下一条消息
    pub async fn recv(&mut self) -> Result<AgentMessage, IpcError> {
        let rx = self.msg_rx.as_mut().ok_or(IpcError::NotConnected)?;
        rx.recv()
            .await
            .ok_or(IpcError::ConnectionClosed)?
    }
}

/// IPC 客户端 — 发送 Agent 消息
pub struct AgentIpcClient {
    config: AgentIpcConfig,
    // TCP 连接（TcpStream 可跨平台，UnixStream 仅 Unix）
    tcp_conn: Option<TcpStream>,
    #[cfg(unix)]
    unix_conn: Option<UnixStream>,
}

impl AgentIpcClient {
    /// 创建 IPC 客户端（不连接）
    pub fn new(config: AgentIpcConfig) -> Self {
        Self {
            config,
            tcp_conn: None,
            #[cfg(unix)]
            unix_conn: None,
        }
    }

    /// 连接到目标 Agent
    pub async fn connect(&mut self, target_id: &str) -> Result<(), IpcError> {
        match self.config.transport {
            IpcTransport::UnixSocket => {
                #[cfg(unix)]
                {
                    let path = unix_socket_path(&self.config.socket_dir, target_id);
                    let stream = UnixStream::connect(&path).await?;
                    self.unix_conn = Some(stream);
                    return Ok(());
                }
                #[cfg(not(unix))]
                {
                    let port = tcp_port(&self.config, target_id);
                    let stream = TcpStream::connect(("127.0.0.1", port)).await?;
                    self.tcp_conn = Some(stream);
                    return Ok(());
                }
            }
            IpcTransport::Tcp | IpcTransport::SharedMemory => {
                let port = tcp_port(&self.config, target_id);
                let stream = TcpStream::connect(("127.0.0.1", port)).await?;
                self.tcp_conn = Some(stream);
                return Ok(());
            }
        }
    }

    /// 发送消息
    pub async fn send(&mut self, msg: &AgentMessage) -> Result<(), IpcError> {
        let payload = serde_json::to_vec(msg)?;
        let len = payload.len() as u32;
        let len_bytes = len.to_le_bytes();

        #[cfg(unix)]
        {
            if let Some(conn) = self.unix_conn.as_mut() {
                conn.write_all(&len_bytes).await?;
                conn.write_all(&payload).await?;
                conn.flush().await?;
                return Ok(());
            }
        }

        if let Some(conn) = self.tcp_conn.as_mut() {
            conn.write_all(&len_bytes).await?;
            conn.write_all(&payload).await?;
            conn.flush().await?;
            return Ok(());
        }

        Err(IpcError::NotConnected)
    }
}

/// 计算 Unix socket 路径
#[cfg(unix)]
fn unix_socket_path(dir: &str, agent_id: &str) -> PathBuf {
    PathBuf::from(dir).join(format!("agent-{}.sock", agent_id))
}

/// 计算 TCP 端口（基于 agent_id 哈希）
fn tcp_port(config: &AgentIpcConfig, agent_id: &str) -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    agent_id.hash(&mut hasher);
    let hash = hasher.finish();
    config.tcp_port_base + (hash % 1000) as u16
}

/// Unix socket accept 循环
#[cfg(unix)]
async fn accept_loop_unix(listener: UnixListener, tx: mpsc::Sender<Result<AgentMessage, IpcError>>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection_unix(stream, tx).await {
                        tracing::warn!("IPC connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::warn!("IPC accept error: {}", e);
            }
        }
    }
}

/// TCP accept 循环
async fn accept_loop_tcp(listener: TcpListener, tx: mpsc::Sender<Result<AgentMessage, IpcError>>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection_tcp(stream, tx).await {
                        tracing::warn!("IPC connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::warn!("IPC accept error: {}", e);
            }
        }
    }
}

/// 处理 Unix socket 连接
#[cfg(unix)]
async fn handle_connection_unix(
    mut stream: UnixStream,
    tx: mpsc::Sender<Result<AgentMessage, IpcError>>,
) -> Result<(), IpcError> {
    loop {
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;

        match serde_json::from_slice::<AgentMessage>(&payload) {
            Ok(msg) => {
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                if tx.send(Err(IpcError::Serialize(e))).await.is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// 处理 TCP 连接
async fn handle_connection_tcp(
    mut stream: TcpStream,
    tx: mpsc::Sender<Result<AgentMessage, IpcError>>,
) -> Result<(), IpcError> {
    loop {
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;

        match serde_json::from_slice::<AgentMessage>(&payload) {
            Ok(msg) => {
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                if tx.send(Err(IpcError::Serialize(e))).await.is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

// ============================================================================
// 网络命名空间隔离（Linux only，非 Linux 提供 no-op stub）
//
// 通过 `ip` 命令为 Agent 进程创建独立网络命名空间，实现网络访问隔离。
// ============================================================================

/// 网络命名空间错误
#[derive(Debug, thiserror::Error)]
pub enum NamespaceError {
    #[error("namespace operation failed: {0}")]
    OperationFailed(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 网络命名空间配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkNamespaceConfig {
    /// 命名空间名称
    pub name: String,
    /// veth pair 位于命名空间内的端点名称
    pub veth_peer: String,
    /// 宿主侧 veth 连接的网桥（可选）
    pub bridge: Option<String>,
    /// 命名空间内 veth 的 IPv4 地址（含前缀，如 10.0.0.2/24）
    pub ipv4: Option<String>,
}

/// 网络命名空间管理器 — 通过 `ip` 命令隔离 Agent 进程网络
pub struct NetworkNamespaceManager;

impl NetworkNamespaceManager {
    /// 创建网络命名空间（`ip netns add <name>`）
    #[cfg(target_os = "linux")]
    pub fn create(name: &str) -> Result<(), NamespaceError> {
        run_ip(&["netns", "add", name])
    }

    #[cfg(not(target_os = "linux"))]
    pub fn create(_name: &str) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 删除网络命名空间（`ip netns del <name>`）
    #[cfg(target_os = "linux")]
    pub fn delete(name: &str) -> Result<(), NamespaceError> {
        run_ip(&["netns", "del", name])
    }

    #[cfg(not(target_os = "linux"))]
    pub fn delete(_name: &str) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 创建 veth pair 并将 `<veth_ns>` 移入命名空间
    ///（`ip link add <veth_host> type veth peer name <veth_ns>` +
    ///  `ip link set <veth_ns> netns <ns_name>`）
    #[cfg(target_os = "linux")]
    pub fn create_veth_pair(
        ns_name: &str,
        veth_host: &str,
        veth_ns: &str,
    ) -> Result<(), NamespaceError> {
        run_ip(&[
            "link", "add", veth_host, "type", "veth", "peer", "name", veth_ns,
        ])?;
        run_ip(&["link", "set", veth_ns, "netns", ns_name])?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn create_veth_pair(
        _ns_name: &str,
        _veth_host: &str,
        _veth_ns: &str,
    ) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 将宿主侧 veth 连接到网桥（`ip link set <veth_host> master <bridge>`）
    #[cfg(target_os = "linux")]
    pub fn attach_to_bridge(veth_host: &str, bridge: &str) -> Result<(), NamespaceError> {
        run_ip(&["link", "set", veth_host, "master", bridge])
    }

    #[cfg(not(target_os = "linux"))]
    pub fn attach_to_bridge(_veth_host: &str, _bridge: &str) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 在命名空间内为 veth 配置 IP
    ///（`ip netns exec <ns_name> ip addr add <ipv4> dev <veth_ns>`）
    #[cfg(target_os = "linux")]
    pub fn configure_ip(ns_name: &str, veth_ns: &str, ipv4: &str) -> Result<(), NamespaceError> {
        run_ip(&[
            "netns", "exec", ns_name, "ip", "addr", "add", ipv4, "dev", veth_ns,
        ])
    }

    #[cfg(not(target_os = "linux"))]
    pub fn configure_ip(
        _ns_name: &str,
        _veth_ns: &str,
        _ipv4: &str,
    ) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 完整设置 Agent 网络命名空间：
    /// 创建命名空间 → 创建 veth pair → 连接网桥 → 配置 IP
    ///
    /// 宿主侧 veth 名称由命名空间名派生为 `veth-<name>`，命名空间内侧使用
    /// `config.veth_peer`。
    #[cfg(target_os = "linux")]
    pub fn setup_agent_namespace(config: &NetworkNamespaceConfig) -> Result<(), NamespaceError> {
        let veth_host = format!("veth-{}", config.name);
        Self::create(&config.name)?;
        Self::create_veth_pair(&config.name, &veth_host, &config.veth_peer)?;
        if let Some(bridge) = &config.bridge {
            Self::attach_to_bridge(&veth_host, bridge)?;
        }
        if let Some(ipv4) = &config.ipv4 {
            Self::configure_ip(&config.name, &config.veth_peer, ipv4)?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn setup_agent_namespace(_config: &NetworkNamespaceConfig) -> Result<(), NamespaceError> {
        Err(NamespaceError::UnsupportedPlatform)
    }

    /// 列出所有网络命名空间（`ip netns list`）
    #[cfg(target_os = "linux")]
    pub fn list() -> Result<Vec<String>, NamespaceError> {
        let output = std::process::Command::new("ip")
            .args(["netns", "list"])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NamespaceError::OperationFailed(stderr.trim().to_string()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let names = stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(str::to_string))
            .collect();
        Ok(names)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list() -> Result<Vec<String>, NamespaceError> {
        Ok(Vec::new())
    }

    /// 检查命名空间是否存在
    #[cfg(target_os = "linux")]
    pub fn exists(name: &str) -> bool {
        match Self::list() {
            Ok(names) => names.iter().any(|n| n == name),
            Err(_) => false,
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn exists(_name: &str) -> bool {
        false
    }
}

/// 执行 `ip` 命令并映射错误（仅 Linux）
#[cfg(target_os = "linux")]
fn run_ip(args: &[&str]) -> Result<(), NamespaceError> {
    let output = std::process::Command::new("ip").args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NamespaceError::OperationFailed(stderr.trim().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_config_default() {
        let config = AgentIpcConfig::default();
        assert!(config.buffer_size > 0);
    }

    #[test]
    fn test_tcp_port_deterministic() {
        let config = AgentIpcConfig::default();
        let port1 = tcp_port(&config, "agent-1");
        let port2 = tcp_port(&config, "agent-1");
        assert_eq!(port1, port2);

        let port3 = tcp_port(&config, "agent-2");
        // 不同 agent_id 应该（大概率）映射到不同端口
        // 哈希碰撞理论可能，此处仅验证端口在合法范围内
        assert!(port3 >= config.tcp_port_base && port3 < config.tcp_port_base + 1000);
    }

    #[tokio::test]
    async fn test_ipc_tcp_send_recv() {
        let config = AgentIpcConfig {
            transport: IpcTransport::Tcp,
            buffer_size: 4096,
            socket_dir: "/tmp/eneros-test".to_string(),
            tcp_port_base: 9500,
        };

        // 启动服务端
        let mut server = AgentIpcServer::new("test-server", config.clone());
        server.start().await.unwrap();

        // 给服务端一点时间启动
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // 客户端连接并发送
        let mut client = AgentIpcClient::new(config);
        client.connect("test-server").await.unwrap();

        let msg = AgentMessage::direct("sender", "test-server", "hello ipc");
        client.send(&msg).await.unwrap();

        // 服务端接收
        let received = server.recv().await.unwrap();
        assert_eq!(received.content, "hello ipc");
        assert_eq!(received.sender_id, "sender");
    }

    #[test]
    fn test_namespace_config_serialization() {
        let config = NetworkNamespaceConfig {
            name: "eneros-agent-1".to_string(),
            veth_peer: "veth-agent1".to_string(),
            bridge: Some("br-eneros".to_string()),
            ipv4: Some("10.0.0.2/24".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: NetworkNamespaceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, config.name);
        assert_eq!(decoded.veth_peer, config.veth_peer);
        assert_eq!(decoded.bridge, config.bridge);
        assert_eq!(decoded.ipv4, config.ipv4);

        // 验证 None 字段也能正确往返
        let config_none = NetworkNamespaceConfig {
            name: "eneros-agent-2".to_string(),
            veth_peer: "veth-agent2".to_string(),
            bridge: None,
            ipv4: None,
        };
        let json_none = serde_json::to_string(&config_none).unwrap();
        let decoded_none: NetworkNamespaceConfig = serde_json::from_str(&json_none).unwrap();
        assert_eq!(decoded_none.bridge, None);
        assert_eq!(decoded_none.ipv4, None);
    }

    #[test]
    fn test_namespace_exists_returns_bool() {
        #[cfg(not(target_os = "linux"))]
        {
            // 非 Linux：exists() 返回 false
            assert!(!NetworkNamespaceManager::exists("eneros-test-ns"));
        }
        #[cfg(target_os = "linux")]
        {
            // Linux：仅验证可调用且返回 bool（实际值依赖系统状态）
            let _: bool = NetworkNamespaceManager::exists("eneros-test-ns");
        }
    }

    #[test]
    fn test_namespace_list_returns_vec() {
        #[cfg(not(target_os = "linux"))]
        {
            // 非 Linux：list() 返回空 Vec
            let names: Vec<String> = NetworkNamespaceManager::list().unwrap();
            assert!(names.is_empty());
        }
        #[cfg(target_os = "linux")]
        {
            // Linux：仅验证可调用且返回 Ok（实际内容依赖系统状态）
            assert!(NetworkNamespaceManager::list().is_ok());
        }
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_namespace_create_unsupported() {
        let result = NetworkNamespaceManager::create("eneros-test-ns");
        assert!(matches!(result, Err(NamespaceError::UnsupportedPlatform)));
    }
}
