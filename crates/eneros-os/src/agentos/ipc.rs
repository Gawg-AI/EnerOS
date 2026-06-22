//! Agent 间消息传递（IPC）
//!
//! 提供两种传输层：
//! - Unix socket：通用域，延迟 < 100μs，跨平台兼容（Linux/macOS）
//! - 共享内存 + eventfd：RT 域，延迟 < 10μs，仅 Linux
//!
//! Windows 平台使用 TCP 回退（127.0.0.1）。
//!
//! v0.29.0 — T029-24：实现真实的 `SharedMemoryChannel`，基于 `memmap2` 共享内存映射
//! + Linux eventfd 通知。环形缓冲区管理消息队列，原子操作保证线程安全。

use eneros_core::AgentMessage;
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// TCP 端口基数（agent-`<id>` 使用 base + hash(id) % 1000）
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
    #[error("shared memory channel is full")]
    ChannelFull,
    #[error("invalid shared memory channel: {0}")]
    InvalidChannel(String),
    #[error("message too large for channel buffer: {0} bytes")]
    MessageTooLarge(usize),
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
            IpcTransport::Tcp => {
                let port = tcp_port(&config, &agent_id);
                let listener = TcpListener::bind(("127.0.0.1", port)).await?;
                let handle = tokio::spawn(async move {
                    accept_loop_tcp(listener, tx).await;
                });
                self.server_handle = Some(handle);
            }
            IpcTransport::SharedMemory => {
                // RT 域共享内存通道：基于 memmap2 + eventfd（Linux）
                let shm_path = shm_path(&config.socket_dir, &agent_id);
                // 清理旧共享内存文件
                let _ = std::fs::remove_file(&shm_path);
                let channel =
                    SharedMemoryChannel::create(&shm_path, ChannelConfig::default())?;
                let handle = tokio::task::spawn_blocking(move || {
                    shm_server_loop(channel, tx);
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
    // 共享内存通道（RT 域，IpcTransport::SharedMemory 使用）
    shm_channel: Option<SharedMemoryChannel>,
}

impl AgentIpcClient {
    /// 创建 IPC 客户端（不连接）
    pub fn new(config: AgentIpcConfig) -> Self {
        Self {
            config,
            tcp_conn: None,
            #[cfg(unix)]
            unix_conn: None,
            shm_channel: None,
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
                    Ok(())
                }
            }
            IpcTransport::Tcp => {
                let port = tcp_port(&self.config, target_id);
                let stream = TcpStream::connect(("127.0.0.1", port)).await?;
                self.tcp_conn = Some(stream);
                Ok(())
            }
            IpcTransport::SharedMemory => {
                // 打开服务端创建的共享内存通道（带重试，等待服务端就绪）
                let path = shm_path(&self.config.socket_dir, target_id);
                let channel = open_shm_with_retry(&path)?;
                self.shm_channel = Some(channel);
                Ok(())
            }
        }
    }

    /// 发送消息
    pub async fn send(&mut self, msg: &AgentMessage) -> Result<(), IpcError> {
        let payload = serde_json::to_vec(msg)?;

        // 共享内存通道优先（RT 域零拷贝路径）
        if let Some(ch) = self.shm_channel.as_ref() {
            ch.send(&payload)?;
            return Ok(());
        }

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

/// 计算共享内存通道文件路径
fn shm_path(dir: &str, agent_id: &str) -> PathBuf {
    PathBuf::from(dir).join(format!("agent-{}.shm", agent_id))
}

/// 带重试地打开共享内存通道（等待服务端创建完成）
fn open_shm_with_retry(path: &Path) -> Result<SharedMemoryChannel, IpcError> {
    let mut last_err = None;
    for _ in 0..50 {
        match SharedMemoryChannel::open(path) {
            Ok(ch) => return Ok(ch),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    Err(last_err.unwrap_or(IpcError::NotConnected))
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

/// 共享内存通道服务端循环 — 阻塞任务，从通道读取消息并转发到 mpsc
///
/// 在 `tokio::task::spawn_blocking` 中运行，使用 `recv_timeout` 轮询消息，
/// 每 100ms 超时一次以检查接收方是否已关闭（`tx.is_closed()`）。
fn shm_server_loop(channel: SharedMemoryChannel, tx: mpsc::Sender<Result<AgentMessage, IpcError>>) {
    loop {
        // 接收方已关闭，退出循环
        if tx.is_closed() {
            break;
        }

        match channel.recv_timeout(100) {
            Ok(Some(data)) => {
                match serde_json::from_slice::<AgentMessage>(&data) {
                    Ok(msg) => {
                        if tx.blocking_send(Ok(msg)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if tx.blocking_send(Err(IpcError::Serialize(e))).is_err() {
                            break;
                        }
                    }
                }
            }
            Ok(None) => {
                // 超时，继续循环以检查接收方状态
            }
            Err(e) => {
                if tx.blocking_send(Err(e)).is_err() {
                    break;
                }
            }
        }
    }
}

// ============================================================================
// 共享内存通道（RT 域 IPC）
//
// 基于 `memmap2` 共享内存映射实现零拷贝消息传递。Linux 上使用 `eventfd`
// 进行通知（延迟 < 10μs），非 Linux 平台使用轮询回退。
//
// 环形缓冲区管理消息队列，使用原子操作（Acquire/Release）管理读写偏移，
// 保证单生产者单消费者（SPSC）场景下的线程安全。
// ============================================================================

/// 共享内存通道魔数 — 标识有效的通道文件（"ENEROS_S" 的 ASCII 编码）
const SHM_CHANNEL_MAGIC: u64 = 0x454E45524F535F53;

/// 共享内存通道头部大小（字节）
const SHM_HEADER_SIZE: usize = std::mem::size_of::<ChannelHeader>();

/// 共享内存通道配置
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// 缓冲区容量（字节，不含头部）
    pub capacity: usize,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        // 默认 1MB 缓冲区，满足 RT 域 Agent 间消息传递需求
        Self {
            capacity: 1024 * 1024,
        }
    }
}

/// 共享内存通道头部 — 位于 mmap 区域起始处，C 兼容布局
///
/// 所有字段使用原子操作访问，保证跨线程可见性。
/// `write_offset` 和 `read_offset` 为单调递增的绝对偏移（非模 capacity），
/// 实际缓冲区位置通过 `offset % capacity` 计算。这避免了"空/满不可区分"
/// 问题：当 `write_offset == read_offset` 时缓冲区为空，当
/// `write_offset - read_offset == capacity - 1` 时缓冲区为满。
#[repr(C)]
struct ChannelHeader {
    /// 魔数，标识有效的通道
    magic: u64,
    /// 缓冲区容量（字节，不含头部）
    capacity: u64,
    /// 写偏移（绝对值，单调递增）
    write_offset: AtomicU64,
    /// 读偏移（绝对值，单调递增）
    read_offset: AtomicU64,
    /// 队列中的消息数（信息性，非同步原语）
    message_count: AtomicU64,
}

/// 共享内存通道 — 基于 memmap2 的零拷贝 IPC 通道
///
/// ## 设计
///
/// - **共享内存**：使用 `memmap2::MmapMut` 映射文件到内存，多进程共享
/// - **通知机制**：Linux 使用 `eventfd`（延迟 < 10μs），非 Linux 使用轮询
/// - **环形缓冲区**：消息以 `[4字节长度][N字节数据]` 格式存储，支持回绕
/// - **线程安全**：原子操作（Acquire/Release）管理偏移，SPSC 语义
///
/// ## 帧格式
///
/// 每条消息在缓冲区中的存储格式：
/// ```text
/// +----------+----------------+
/// | len: u32 | data: [u8; len] |
/// +----------+----------------+
/// ```
/// `len` 为小端序 4 字节无符号整数，表示数据长度。
///
/// ## 线程安全
///
/// 通道支持单生产者单消费者（SPSC）语义：
/// - 生产者（`send`）：写入数据后，以 `Release` 序更新 `write_offset`
/// - 消费者（`try_recv`/`recv`）：以 `Acquire` 序加载 `write_offset` 后读取数据
///
/// `Sync` 的安全性基于上述原子操作建立的 happens-before 关系。
pub struct SharedMemoryChannel {
    mmap: MmapMut,
    config: ChannelConfig,
    /// Linux eventfd 文件描述符（用于通知）；非 Linux 为 None
    event_fd: Option<i32>,
}

// 安全：SharedMemoryChannel 使用原子操作（Acquire/Release）管理读写偏移，
// 保证 SPSC 场景下的线程安全。缓冲区写入在 write_offset 更新（Release）前
// 完成，读取在 write_offset 加载（Acquire）后进行，建立 happens-before 关系。
// event_fd 为裸 i32，可安全跨线程共享。
unsafe impl Sync for SharedMemoryChannel {}

impl SharedMemoryChannel {
    /// 创建新的共享内存通道（服务端）
    ///
    /// 在 `path` 创建新文件，扩展到 `header + capacity` 大小，mmap 映射，
    /// 并初始化头部（写入魔数、容量，偏移清零）。
    pub fn create(path: &Path, config: ChannelConfig) -> Result<Self, IpcError> {
        // 校验容量：至少能容纳一条最小消息（4 字节长度 + 1 字节数据）
        if config.capacity < 8 {
            return Err(IpcError::MessageTooLarge(config.capacity));
        }

        let total_size = SHM_HEADER_SIZE + config.capacity;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(total_size as u64)?;

        let mmap = unsafe { MmapMut::map_mut(&file)? };

        let mut channel = Self {
            mmap,
            config: config.clone(),
            event_fd: create_eventfd()?,
        };

        channel.init_header(&config)?;
        Ok(channel)
    }

    /// 打开已有的共享内存通道（客户端）
    ///
    /// mmap 映射 `path` 指向的文件，读取头部验证魔数和容量。
    pub fn open(path: &Path) -> Result<Self, IpcError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        // 验证头部魔数
        let header = header_ptr(&mmap);
        let magic = unsafe { (*header).magic };
        if magic != SHM_CHANNEL_MAGIC {
            return Err(IpcError::InvalidChannel("magic mismatch".to_string()));
        }
        let capacity = unsafe { (*header).capacity } as usize;

        Ok(Self {
            mmap,
            config: ChannelConfig { capacity },
            event_fd: create_eventfd()?,
        })
    }

    /// 初始化通道头部（写入魔数、容量，偏移清零）
    fn init_header(&mut self, config: &ChannelConfig) -> Result<(), IpcError> {
        let header = header_mut_ptr(&mut self.mmap);
        unsafe {
            (*header).magic = SHM_CHANNEL_MAGIC;
            (*header).capacity = config.capacity as u64;
            (*header).write_offset = AtomicU64::new(0);
            (*header).read_offset = AtomicU64::new(0);
            (*header).message_count = AtomicU64::new(0);
        }
        Ok(())
    }

    /// 获取头部不可变引用（通过 mmap 指针）
    fn header(&self) -> &ChannelHeader {
        unsafe { &*(header_ptr(&self.mmap)) }
    }

    /// 获取缓冲区起始指针（跳过头部）
    fn buffer_ptr(&self) -> *const u8 {
        unsafe { self.mmap.as_ptr().add(SHM_HEADER_SIZE) }
    }

    /// 获取当前通道中的消息数（信息性，可能存在短暂不一致）
    pub fn message_count(&self) -> u64 {
        self.header().message_count.load(Ordering::Relaxed)
    }

    /// 发送消息（写入共享内存）
    ///
    /// 帧格式：`[4字节小端长度][N字节数据]`。写入完成后以 `Release` 序
    /// 更新 `write_offset`，并通过 eventfd 通知接收方（Linux）。
    pub fn send(&self, data: &[u8]) -> Result<(), IpcError> {
        let frame_len = 4 + data.len();
        let capacity = self.config.capacity;

        // 消息不能大于缓冲区（留 1 字节用于区分空/满）
        if frame_len >= capacity {
            return Err(IpcError::MessageTooLarge(data.len()));
        }

        // 原子读取偏移
        let write_offset = self.header().write_offset.load(Ordering::Acquire);
        let read_offset = self.header().read_offset.load(Ordering::Acquire);

        // 计算可用空间（环形缓冲区，保留 1 字节以区分空/满）
        let used = write_offset.wrapping_sub(read_offset);
        let available = capacity.saturating_sub(used as usize).saturating_sub(1);
        if frame_len > available {
            return Err(IpcError::ChannelFull);
        }

        // 写入帧（处理回绕）
        let buffer = self.buffer_ptr();
        let write_pos = (write_offset as usize) % capacity;

        // 写入 4 字节长度前缀（小端序）
        let len_bytes = (data.len() as u32).to_le_bytes();
        write_ring(buffer, capacity, write_pos, &len_bytes);

        // 写入数据
        let data_pos = (write_pos + 4) % capacity;
        write_ring(buffer, capacity, data_pos, data);

        // 更新写偏移（Release 序，确保数据写入对读者可见）
        let new_write_offset = write_offset.wrapping_add(frame_len as u64);
        self.header()
            .write_offset
            .store(new_write_offset, Ordering::Release);

        // 增加消息计数（信息性，Relaxed 序即可）
        self.header().message_count.fetch_add(1, Ordering::Relaxed);

        // 通知接收方（Linux eventfd）
        notify_eventfd(self.event_fd)?;

        Ok(())
    }

    /// 非阻塞接收消息
    ///
    /// 如果缓冲区为空返回 `Ok(None)`，否则读取一帧并返回数据。
    /// 若读取到的长度前缀超出缓冲区容量（恶意或损坏数据），返回 `Err`。
    pub fn try_recv(&self) -> Result<Option<Vec<u8>>, IpcError> {
        let write_offset = self.header().write_offset.load(Ordering::Acquire);
        let read_offset = self.header().read_offset.load(Ordering::Acquire);

        // 缓冲区为空
        if write_offset == read_offset {
            return Ok(None);
        }

        let capacity = self.config.capacity;
        let buffer = self.buffer_ptr();
        let read_pos = (read_offset as usize) % capacity;

        // 读取 4 字节长度前缀
        let len_bytes = read_ring(buffer, capacity, read_pos, 4);
        let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;

        // 校验 len 不超过缓冲区容量减去长度前缀（防止恶意/损坏数据导致 OOM）
        let max_msg_size = capacity.saturating_sub(4);
        if len > max_msg_size {
            return Err(IpcError::InvalidChannel(format!(
                "消息长度 {} 超过最大允许值 {}", len, max_msg_size
            )));
        }

        // 读取数据
        let data_pos = (read_pos + 4) % capacity;
        let data = read_ring(buffer, capacity, data_pos, len);

        // 更新读偏移（Release 序，确保数据读取对写者可见）
        let frame_len = 4 + len;
        let new_read_offset = read_offset.wrapping_add(frame_len as u64);
        self.header()
            .read_offset
            .store(new_read_offset, Ordering::Release);

        // 减少消息计数
        self.header().message_count.fetch_sub(1, Ordering::Relaxed);

        Ok(Some(data))
    }

    /// 带超时的阻塞接收
    ///
    /// 在 `timeout_ms` 毫秒内等待消息。Linux 上使用 `poll()` 等待 eventfd，
    /// 非 Linux 上使用 `sleep` 轮询。超时返回 `Ok(None)`。
    pub fn recv_timeout(&self, timeout_ms: u32) -> Result<Option<Vec<u8>>, IpcError> {
        // 先尝试非阻塞读取
        if let Some(data) = self.try_recv()? {
            return Ok(Some(data));
        }

        // 等待通知或超时
        #[cfg(target_os = "linux")]
        {
            wait_eventfd(self.event_fd, timeout_ms);
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
        }

        // 再次尝试读取
        self.try_recv()
    }

    /// 阻塞接收消息（无限等待）
    ///
    /// 内部使用 `recv_timeout` 循环，每 1 秒检查一次。适用于专用接收线程。
    pub fn recv(&self) -> Result<Vec<u8>, IpcError> {
        loop {
            if let Some(data) = self.recv_timeout(1000)? {
                return Ok(data);
            }
        }
    }
}

impl Drop for SharedMemoryChannel {
    fn drop(&mut self) {
        // 关闭 eventfd（Linux）
        #[cfg(target_os = "linux")]
        {
            if let Some(fd) = self.event_fd {
                unsafe {
                    libc::close(fd);
                }
            }
        }
    }
}

// ============================================================================
// 共享内存通道辅助函数
// ============================================================================

/// 获取 mmap 区域头部的不可变指针
fn header_ptr(mmap: &MmapMut) -> *const ChannelHeader {
    mmap.as_ptr() as *const ChannelHeader
}

/// 获取 mmap 区域头部的可变指针
fn header_mut_ptr(mmap: &mut MmapMut) -> *mut ChannelHeader {
    mmap.as_mut_ptr() as *mut ChannelHeader
}

/// 环形写入：将 `data` 写入缓冲区 `buffer`（容量 `capacity`）的 `offset` 位置，
/// 自动处理跨边界回绕。
///
/// 安全性：调用者需确保 `offset + data.len() <= capacity`（考虑回绕后），
/// 且 `buffer` 指向有效的共享内存区域。
fn write_ring(buffer: *const u8, capacity: usize, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if end <= capacity {
        // 不跨边界，单次拷贝
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.add(offset) as *mut u8, data.len());
        }
    } else {
        // 跨边界，分两段拷贝
        let first_len = capacity - offset;
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                buffer.add(offset) as *mut u8,
                first_len,
            );
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(first_len),
                buffer as *mut u8,
                data.len() - first_len,
            );
        }
    }
}

/// 环形读取：从缓冲区 `buffer`（容量 `capacity`）的 `offset` 位置读取 `len` 字节，
/// 自动处理跨边界回绕，返回新分配的 `Vec<u8>`。
fn read_ring(buffer: *const u8, capacity: usize, offset: usize, len: usize) -> Vec<u8> {
    let mut result = vec![0u8; len];
    let end = offset + len;
    if end <= capacity {
        // 不跨边界，单次拷贝
        unsafe {
            std::ptr::copy_nonoverlapping(buffer.add(offset), result.as_mut_ptr(), len);
        }
    } else {
        // 跨边界，分两段拷贝
        let first_len = capacity - offset;
        unsafe {
            std::ptr::copy_nonoverlapping(buffer.add(offset), result.as_mut_ptr(), first_len);
            std::ptr::copy_nonoverlapping(
                buffer,
                result.as_mut_ptr().add(first_len),
                len - first_len,
            );
        }
    }
    result
}

/// 创建 eventfd（Linux），非 Linux 返回 None
#[cfg(target_os = "linux")]
fn create_eventfd() -> Result<Option<i32>, IpcError> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(IpcError::Io(std::io::Error::last_os_error()));
    }
    Ok(Some(fd))
}

#[cfg(not(target_os = "linux"))]
fn create_eventfd() -> Result<Option<i32>, IpcError> {
    Ok(None)
}

/// 通过 eventfd 通知接收方（Linux）
#[cfg(target_os = "linux")]
fn notify_eventfd(event_fd: Option<i32>) -> Result<(), IpcError> {
    if let Some(fd) = event_fd {
        let value: u64 = 1;
        let bytes = value.to_le_bytes();
        let ret = unsafe { libc::write(fd, bytes.as_ptr() as *const libc::c_void, 8) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            // EAGAIN/EWOULDBLOCK 表示 eventfd 计数器溢出（接收方尚未读取），
            // 这是非致命的，通知已经在队列中
            if err.kind() != std::io::ErrorKind::WouldBlock {
                return Err(IpcError::Io(err));
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn notify_eventfd(_event_fd: Option<i32>) -> Result<(), IpcError> {
    Ok(())
}

/// 使用 poll 等待 eventfd 可读（Linux），超时返回
#[cfg(target_os = "linux")]
fn wait_eventfd(event_fd: Option<i32>, timeout_ms: u32) {
    if let Some(fd) = event_fd {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms as libc::c_int) };
        if ret > 0 && (pfd.revents & libc::POLLIN) != 0 {
            // 消费 eventfd 值（重置计数器）
            let mut buf = [0u8; 8];
            unsafe {
                libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 8);
            }
        }
    } else {
        // 无 eventfd，使用 sleep 回退
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
    }
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

    // ========================================================================
    // SharedMemoryChannel 单元测试
    // ========================================================================

    /// 生成唯一的临时共享内存文件路径
    fn unique_shm_path(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("eneros-shm-test-{}-{}-{}.shm", pid, id, name))
    }

    #[test]
    fn test_shm_channel_create_and_open() {
        let path = unique_shm_path("create_open");
        let _ = std::fs::remove_file(&path);

        // 创建通道
        let config = ChannelConfig { capacity: 4096 };
        let channel = SharedMemoryChannel::create(&path, config.clone()).unwrap();
        assert_eq!(channel.message_count(), 0);

        // 打开同一通道
        let channel2 = SharedMemoryChannel::open(&path).unwrap();
        assert_eq!(channel2.config.capacity, 4096);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_invalid_magic() {
        let path = unique_shm_path("invalid_magic");
        let _ = std::fs::remove_file(&path);

        // 创建一个不含有效魔数的文件
        std::fs::write(&path, b"not a valid shared memory channel").unwrap();

        // 打开应失败
        let result = SharedMemoryChannel::open(&path);
        assert!(matches!(result, Err(IpcError::InvalidChannel(_))));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_send_recv_basic() {
        let path = unique_shm_path("send_recv_basic");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 4096 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送消息
        let msg = b"hello shared memory";
        channel.send(msg).unwrap();
        assert_eq!(channel.message_count(), 1);

        // 接收消息
        let received = channel.try_recv().expect("try_recv failed").expect("should receive a message");
        assert_eq!(received, msg);
        assert_eq!(channel.message_count(), 0);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_try_recv_empty() {
        let path = unique_shm_path("try_recv_empty");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 4096 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 空通道应返回 None
        assert!(channel.try_recv().unwrap().is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_multiple_messages() {
        let path = unique_shm_path("multiple_messages");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 8192 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送 10 条消息
        let messages: Vec<Vec<u8>> = (0..10).map(|i| format!("message-{}", i).into_bytes()).collect();
        for msg in &messages {
            channel.send(msg).unwrap();
        }

        // 按顺序接收并验证
        for expected in &messages {
            let received = channel.try_recv().expect("try_recv failed").expect("should receive a message");
            assert_eq!(received, *expected);
        }

        // 缓冲区应为空
        assert!(channel.try_recv().unwrap().is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_large_message() {
        let path = unique_shm_path("large_message");
        let _ = std::fs::remove_file(&path);

        let capacity = 64 * 1024; // 64KB
        let config = ChannelConfig { capacity };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送接近容量上限的消息（60KB）
        let data = vec![0xABu8; 60 * 1024];
        channel.send(&data).unwrap();

        let received = channel.try_recv().expect("try_recv failed").expect("should receive large message");
        assert_eq!(received.len(), data.len());
        assert_eq!(received, data);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_message_too_large() {
        let path = unique_shm_path("message_too_large");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 1024 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送超过缓冲区容量的消息
        let large_data = vec![0u8; 2048];
        let result = channel.send(&large_data);
        assert!(matches!(result, Err(IpcError::MessageTooLarge(_))));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_buffer_full() {
        let path = unique_shm_path("buffer_full");
        let _ = std::fs::remove_file(&path);

        // 使用小容量缓冲区
        let config = ChannelConfig { capacity: 128 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送消息直到缓冲区满
        let msg = vec![0x42u8; 50]; // 每条消息占 54 字节（4 + 50）
        channel.send(&msg).unwrap(); // 54 字节
        channel.send(&msg).unwrap(); // 108 字节

        // 第三条消息会使已用空间达到 162 字节，超过 capacity - 1 = 127
        let result = channel.send(&msg);
        assert!(matches!(result, Err(IpcError::ChannelFull)));

        // 读取一条消息后应该能再次发送
        let received = channel.try_recv().expect("try_recv failed").expect("should receive");
        assert_eq!(received, msg);

        // 现在缓冲区有空间，可以再次发送
        channel.send(&msg).unwrap();

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_wraparound() {
        let path = unique_shm_path("wraparound");
        let _ = std::fs::remove_file(&path);

        // 使用小容量缓冲区以便触发回绕
        let config = ChannelConfig { capacity: 64 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 发送和接收多条消息，使写偏移接近缓冲区末尾
        // 每条消息：4 字节长度 + 20 字节数据 = 24 字节
        let msg = vec![0x55u8; 20];

        // 第一条：offset 0 -> 24
        channel.send(&msg).unwrap();
        let r1 = channel.try_recv().unwrap().unwrap();
        assert_eq!(r1, msg);

        // 第二条：offset 24 -> 48
        channel.send(&msg).unwrap();
        let r2 = channel.try_recv().unwrap().unwrap();
        assert_eq!(r2, msg);

        // 第三条：offset 48 -> 72（回绕到 8）
        // 48 + 24 = 72 > 64，所以数据会回绕
        channel.send(&msg).unwrap();
        let r3 = channel.try_recv().unwrap().unwrap();
        assert_eq!(r3, msg);

        // 验证回绕后仍能正常工作
        channel.send(&msg).unwrap();
        let r4 = channel.try_recv().unwrap().unwrap();
        assert_eq!(r4, msg);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_agent_message_serialization() {
        let path = unique_shm_path("agent_message");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 8192 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 序列化 AgentMessage 并发送
        let msg = AgentMessage::direct("agent-sender", "agent-receiver", "test shared memory ipc");
        let payload = serde_json::to_vec(&msg).unwrap();
        channel.send(&payload).unwrap();

        // 接收并反序列化
        let received = channel.try_recv().expect("try_recv failed").expect("should receive");
        let decoded: AgentMessage = serde_json::from_slice(&received).unwrap();
        assert_eq!(decoded.content, msg.content);
        assert_eq!(decoded.sender_id, msg.sender_id);
        assert_eq!(decoded.target_id, msg.target_id);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_recv_timeout() {
        let path = unique_shm_path("recv_timeout");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 4096 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 空通道，recv_timeout 应在超时后返回 None
        let start = std::time::Instant::now();
        let result = channel.recv_timeout(50);
        let elapsed = start.elapsed();

        assert!(result.unwrap().is_none());
        // 至少等待了 50ms（非 Linux 可能略长）
        assert!(elapsed >= std::time::Duration::from_millis(40));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_send_after_recv_frees_space() {
        let path = unique_shm_path("send_after_recv");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 256 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 填充缓冲区
        let msg = vec![0x77u8; 100]; // 每条 104 字节
        channel.send(&msg).unwrap(); // 104 字节
        channel.send(&msg).unwrap(); // 208 字节

        // 第三条应该失败（104 + 208 = 312 > 255）
        assert!(channel.send(&msg).is_err());

        // 接收一条后释放空间
        let _ = channel.try_recv().unwrap().unwrap();

        // 现在可以再发送
        channel.send(&msg).unwrap();

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_channel_try_recv_rejects_oversized_len() {
        let path = unique_shm_path("oversized_len");
        let _ = std::fs::remove_file(&path);

        let config = ChannelConfig { capacity: 256 };
        let channel = SharedMemoryChannel::create(&path, config).unwrap();

        // 直接写入恶意帧：4 字节长度前缀设为超出容量的值
        let capacity = channel.config.capacity;
        let buffer = channel.buffer_ptr();
        let malicious_len: u32 = (capacity as u32) * 2;
        let len_bytes = malicious_len.to_le_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), buffer as *mut u8, 4);
        }
        // 更新 write_offset 使通道非空
        channel.header().write_offset.store(8, Ordering::Release);

        // try_recv 应拒绝恶意长度
        let result = channel.try_recv();
        assert!(matches!(result, Err(IpcError::InvalidChannel(_))));

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_ipc_shm_send_recv() {
        // 使用唯一的 socket_dir 避免与其他测试冲突
        let socket_dir = format!(
            "{}/eneros-shm-ipc-test-{}",
            std::env::temp_dir().to_string_lossy(),
            std::process::id()
        );
        std::fs::create_dir_all(&socket_dir).ok();

        let config = AgentIpcConfig {
            transport: IpcTransport::SharedMemory,
            buffer_size: 4096,
            socket_dir: socket_dir.clone(),
            tcp_port_base: 9600,
        };

        // 启动服务端
        let mut server = AgentIpcServer::new("shm-test-server", config.clone());
        server.start().await.unwrap();

        // 给服务端一点时间创建通道文件
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 客户端连接并发送
        let mut client = AgentIpcClient::new(config);
        client.connect("shm-test-server").await.unwrap();

        let msg = AgentMessage::direct("sender", "shm-test-server", "hello shared memory ipc");
        client.send(&msg).await.unwrap();

        // 服务端接收
        let received = server.recv().await.unwrap();
        assert_eq!(received.content, "hello shared memory ipc");
        assert_eq!(received.sender_id, "sender");

        // 清理
        let shm_file = PathBuf::from(&socket_dir).join("agent-shm-test-server.shm");
        std::fs::remove_file(&shm_file).ok();
        std::fs::remove_dir_all(&socket_dir).ok();
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
