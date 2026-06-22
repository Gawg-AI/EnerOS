//! EventBusBroker — 独立进程的事件发布/订阅 broker
//!
//! 作为独立进程运行，接收来自多个客户端的 publish/subscribe 请求。
//! - 跨平台：Unix socket（Linux/macOS）+ TCP 回退（Windows）
//! - 长度前缀 JSON 帧格式（4字节 LE 长度 + JSON payload）
//! - 支持 EventType 过滤订阅
//! - 保留 urgent/normal 优先级语义

use eneros_core::{Event, EventType};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::broadcast;

/// Broker 错误
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("not connected")]
    NotConnected,
    #[error("broker full: too many subscribers")]
    TooManySubscribers,
    #[error("invalid message: {0}")]
    InvalidMessage(String),
}

/// Broker 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerConfig {
    /// TCP 绑定地址（如 "127.0.0.1:9876"）
    pub tcp_addr: String,
    /// Unix socket 路径（仅 Unix；None 表示不使用 Unix socket）
    pub unix_socket: Option<String>,
    /// broadcast channel 容量（每个订阅者）
    pub channel_capacity: usize,
    /// 最大订阅者数
    pub max_subscribers: usize,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            tcp_addr: "127.0.0.1:9876".to_string(),
            unix_socket: None,
            channel_capacity: 1024,
            max_subscribers: 256,
        }
    }
}

/// 事件过滤器
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    /// 仅接收这些 EventType（空表示接收全部）
    pub event_types: Vec<EventType>,
    /// 仅接收来自这些 source 的事件（空表示接收全部）
    pub sources: Vec<String>,
}

impl EventFilter {
    /// 创建空过滤器（接收所有事件）
    pub fn new() -> Self {
        Self::default()
    }

    /// 按 EventType 过滤
    pub fn by_types(types: Vec<EventType>) -> Self {
        Self {
            event_types: types,
            sources: Vec::new(),
        }
    }

    /// 按 source 过滤
    pub fn by_sources(sources: Vec<String>) -> Self {
        Self {
            event_types: Vec::new(),
            sources,
        }
    }

    /// 检查事件是否匹配过滤器
    pub fn matches(&self, event: &Event) -> bool {
        if !self.event_types.is_empty() && !self.event_types.contains(&event.event_type) {
            return false;
        }
        if !self.sources.is_empty() && !self.sources.contains(&event.source) {
            return false;
        }
        true
    }
}

/// Broker 消息（客户端 ↔ broker 通信协议）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BrokerMessage {
    /// 客户端 → broker：发布事件
    Publish { event: Event },
    /// 客户端 → broker：订阅
    Subscribe { filter: Option<EventFilter> },
    /// 客户端 → broker：取消订阅
    Unsubscribe,
    /// 客户端 → broker：请求统计
    GetStats,
    /// broker → 客户端：事件推送
    Event { event: Event },
    /// broker → 客户端：统计响应
    Stats { stats: BrokerStats },
    /// broker → 客户端：确认
    Ack { message: String },
    /// broker → 客户端：错误
    Error { message: String },
}

/// Broker 统计信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrokerStats {
    /// 总发布事件数
    pub total_published: u64,
    /// 总投递事件数
    pub total_delivered: u64,
    /// 当前活跃订阅者数
    pub active_subscribers: usize,
    /// 启动时间（Unix 时间戳秒）
    pub started_at: i64,
}

/// EventBusBroker — 独立进程的事件 broker
pub struct EventBusBroker {
    config: BrokerConfig,
    /// 事件广播通道（所有订阅者共享）
    event_tx: broadcast::Sender<Event>,
    /// 统计
    total_published: Arc<AtomicU64>,
    total_delivered: Arc<AtomicU64>,
    active_subscribers: Arc<AtomicU64>,
    started_at: i64,
}

impl EventBusBroker {
    /// 创建新 broker
    pub fn new(config: BrokerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.channel_capacity);
        Self {
            config,
            event_tx,
            total_published: Arc::new(AtomicU64::new(0)),
            total_delivered: Arc::new(AtomicU64::new(0)),
            active_subscribers: Arc::new(AtomicU64::new(0)),
            started_at: chrono::Utc::now().timestamp(),
        }
    }

    /// 启动 broker（阻塞，监听传入连接）
    pub async fn run(self) -> Result<(), BrokerError> {
        let self_arc = Arc::new(self);

        // TCP 监听
        let tcp_addr = self_arc.config.tcp_addr.clone();
        let tcp_listener = TcpListener::bind(&tcp_addr).await?;
        tracing::info!("EventBusBroker TCP listening on {}", tcp_addr);

        let broker_for_tcp = self_arc.clone();
        let tcp_handle = tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((stream, peer)) => {
                        tracing::debug!("TCP client connected: {}", peer);
                        let broker = broker_for_tcp.clone();
                        tokio::spawn(async move {
                            if let Err(e) = broker.handle_client_tcp(stream).await {
                                tracing::warn!("TCP client error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("TCP accept error: {}", e);
                    }
                }
            }
        });

        // Unix socket 监听（仅 Unix）
        #[cfg(unix)]
        let unix_handle = {
            let unix_socket = self_arc.config.unix_socket.clone();
            if let Some(path) = unix_socket {
                let _ = std::fs::remove_file(&path);
                let listener = UnixListener::bind(&path)?;
                tracing::info!("EventBusBroker Unix socket listening on {}", path);
                let broker_for_unix = self_arc.clone();
                Some(tokio::spawn(async move {
                    loop {
                        match listener.accept().await {
                            Ok((stream, _)) => {
                                let broker = broker_for_unix.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = broker.handle_client_unix(stream).await {
                                        tracing::warn!("Unix client error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::error!("Unix accept error: {}", e);
                            }
                        }
                    }
                }))
            } else {
                None
            }
        };

        // 等待（broker 会一直运行直到进程终止）
        tcp_handle.await.map_err(|e| {
            BrokerError::Io(std::io::Error::other(format!("tcp task join: {}", e)))
        })?;

        #[cfg(unix)]
        {
            if let Some(h) = unix_handle {
                let _ = h.await;
            }
        }

        Ok(())
    }

    /// 获取统计快照
    pub fn stats(&self) -> BrokerStats {
        BrokerStats {
            total_published: self.total_published.load(Ordering::Relaxed),
            total_delivered: self.total_delivered.load(Ordering::Relaxed),
            active_subscribers: self.active_subscribers.load(Ordering::Relaxed) as usize,
            started_at: self.started_at,
        }
    }

    /// 处理 TCP 客户端连接
    async fn handle_client_tcp(self: Arc<Self>, mut stream: tokio::net::TcpStream) -> Result<(), BrokerError> {
        self.handle_client(&mut stream).await
    }

    /// 处理 Unix socket 客户端连接
    #[cfg(unix)]
    async fn handle_client_unix(self: Arc<Self>, mut stream: tokio::net::UnixStream) -> Result<(), BrokerError> {
        self.handle_client(&mut stream).await
    }

    /// 通用客户端处理逻辑
    ///
    /// 使用泛型 + AsyncReadExt/AsyncWriteExt trait，TCP 和 Unix stream 都适用
    ///
    /// 客户端模式：
    /// - 发布者：发送 Publish 消息，broker 广播给订阅者
    /// - 订阅者：先发送 Subscribe，然后接收推送的事件
    /// - 混合：订阅后也可以继续发送 Publish
    async fn handle_client<R>(self: Arc<Self>, stream: &mut R) -> Result<(), BrokerError>
    where
        R: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        // 读取第一条消息决定客户端模式
        let first_msg = read_message(stream).await?;

        match first_msg {
            BrokerMessage::Subscribe { filter } => {
                // 订阅者模式
                self.handle_subscriber(stream, filter.unwrap_or_default()).await
            }
            BrokerMessage::Publish { event } => {
                // 发布者模式：先处理第一条 Publish
                self.total_published.fetch_add(1, Ordering::Relaxed);
                let _ = self.event_tx.send(event);
                // 然后进入发布者循环
                self.handle_publisher(stream).await
            }
            BrokerMessage::GetStats => {
                // 统计查询
                let _ = write_message(stream, &BrokerMessage::Stats {
                    stats: self.stats(),
                }).await;
                Ok(())
            }
            _ => {
                // 其他消息类型，忽略
                Ok(())
            }
        }
    }

    /// 处理订阅者客户端
    async fn handle_subscriber<R>(
        self: Arc<Self>,
        stream: &mut R,
        filter: EventFilter,
    ) -> Result<(), BrokerError>
    where
        R: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        // 检查订阅者上限
        let current = self.active_subscribers.load(Ordering::Relaxed);
        if current as usize >= self.config.max_subscribers {
            let _ = write_message(stream, &BrokerMessage::Error {
                message: "too many subscribers".to_string(),
            }).await;
            return Ok(());
        }

        self.active_subscribers.fetch_add(1, Ordering::Relaxed);
        let mut rx = self.event_tx.subscribe();

        // 发送确认
        write_message(stream, &BrokerMessage::Ack {
            message: "subscribed".to_string(),
        }).await?;

        // 主循环：接收客户端消息 + 推送事件
        loop {
            tokio::select! {
                // 接收客户端消息
                msg = read_message_optional(stream) => {
                    match msg {
                        Ok(Some(BrokerMessage::Publish { event })) => {
                            self.total_published.fetch_add(1, Ordering::Relaxed);
                            let _ = self.event_tx.send(event);
                        }
                        Ok(Some(BrokerMessage::Unsubscribe)) => {
                            break;
                        }
                        Ok(Some(BrokerMessage::GetStats)) => {
                            let _ = write_message(stream, &BrokerMessage::Stats {
                                stats: self.stats(),
                            }).await;
                        }
                        Ok(Some(_)) => {
                            // 其他消息忽略
                        }
                        Ok(None) => {
                            break;
                        }
                        Err(e) => {
                            tracing::debug!("Subscriber read error: {}", e);
                            break;
                        }
                    }
                }
                // 推送事件给客户端
                event_result = rx.recv() => {
                    match event_result {
                        Ok(event) => {
                            if filter.matches(&event) {
                                self.total_delivered.fetch_add(1, Ordering::Relaxed);
                                if write_message(stream, &BrokerMessage::Event { event }).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Subscriber lagged by {} events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            }
        }

        self.active_subscribers.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// 处理发布者客户端（仅发布，不订阅）
    async fn handle_publisher<R>(
        self: Arc<Self>,
        stream: &mut R,
    ) -> Result<(), BrokerError>
    where
        R: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        loop {
            match read_message_optional(stream).await {
                Ok(Some(BrokerMessage::Publish { event })) => {
                    self.total_published.fetch_add(1, Ordering::Relaxed);
                    let _ = self.event_tx.send(event);
                }
                Ok(Some(BrokerMessage::GetStats)) => {
                    let _ = write_message(stream, &BrokerMessage::Stats {
                        stats: self.stats(),
                    }).await;
                }
                Ok(Some(_)) => {
                    // 其他消息忽略
                }
                Ok(None) => {
                    break;
                }
                Err(e) => {
                    tracing::debug!("Publisher read error: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }
}

/// 读取一条 BrokerMessage（长度前缀 JSON 帧）
async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<BrokerMessage, BrokerError> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes) as usize;

    if len > 16 * 1024 * 1024 {
        return Err(BrokerError::InvalidMessage(format!("message too large: {} bytes", len)));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;

    let msg: BrokerMessage = serde_json::from_slice(&payload)?;
    Ok(msg)
}

/// 读取一条 BrokerMessage（可选，EOF 返回 None）
async fn read_message_optional<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<BrokerMessage>, BrokerError> {
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_bytes) as usize;

    if len > 16 * 1024 * 1024 {
        return Err(BrokerError::InvalidMessage(format!("message too large: {} bytes", len)));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;

    let msg: BrokerMessage = serde_json::from_slice(&payload)?;
    Ok(Some(msg))
}

/// 写入一条 BrokerMessage（长度前缀 JSON 帧）
pub(crate) async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &BrokerMessage,
) -> Result<(), BrokerError> {
    let payload = serde_json::to_vec(msg)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{Event, EventPayload, EventType};

    fn test_event(source: &str) -> Event {
        Event::new(
            EventType::SystemAlarm,
            source,
            EventPayload::Message("test".to_string()),
        )
    }

    #[test]
    fn test_broker_config_default() {
        let config = BrokerConfig::default();
        assert_eq!(config.tcp_addr, "127.0.0.1:9876");
        assert!(config.channel_capacity > 0);
        assert!(config.max_subscribers > 0);
    }

    #[test]
    fn test_event_filter_matches_all() {
        let filter = EventFilter::new();
        let event = test_event("source-1");
        assert!(filter.matches(&event));
    }

    #[test]
    fn test_event_filter_by_type() {
        let filter = EventFilter::by_types(vec![EventType::SystemAlarm]);
        let alarm = test_event("src");
        let topology = Event::new(
            EventType::TopologyChanged,
            "src",
            EventPayload::Message("".to_string()),
        );
        assert!(filter.matches(&alarm));
        assert!(!filter.matches(&topology));
    }

    #[test]
    fn test_event_filter_by_source() {
        let filter = EventFilter::by_sources(vec!["source-1".to_string()]);
        let matching = test_event("source-1");
        let non_matching = test_event("source-2");
        assert!(filter.matches(&matching));
        assert!(!filter.matches(&non_matching));
    }

    #[test]
    fn test_broker_message_serde_roundtrip() {
        let event = test_event("test");
        let msg = BrokerMessage::Publish { event };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: BrokerMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            BrokerMessage::Publish { event: e } => {
                assert_eq!(e.source, "test");
            }
            _ => panic!("expected Publish"),
        }
    }

    #[tokio::test]
    async fn test_broker_publish_subscribe_tcp() {
        // 先绑定获取实际端口
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let actual_addr = listener.local_addr().unwrap();
        drop(listener);

        let config = BrokerConfig {
            tcp_addr: actual_addr.to_string(),
            ..Default::default()
        };

        let broker = EventBusBroker::new(config.clone());
        let broker_handle = tokio::spawn(async move {
            let _ = broker.run().await;
        });

        // 给 broker 启动时间
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 客户端订阅
        use tokio::net::TcpStream;
        let mut sub_stream = TcpStream::connect(&actual_addr).await.unwrap();

        let sub_msg = BrokerMessage::Subscribe { filter: None };
        write_message(&mut sub_stream, &sub_msg).await.unwrap();

        // 读取确认
        let ack = read_message(&mut sub_stream).await.unwrap();
        assert!(matches!(ack, BrokerMessage::Ack { .. }));

        // 另一个客户端发布事件
        let mut pub_stream = TcpStream::connect(&actual_addr).await.unwrap();
        let event = test_event("publisher");
        let pub_msg = BrokerMessage::Publish { event: event.clone() };
        write_message(&mut pub_stream, &pub_msg).await.unwrap();

        // 订阅者应收到事件
        let received = read_message(&mut sub_stream).await.unwrap();
        match received {
            BrokerMessage::Event { event: e } => {
                assert_eq!(e.source, "publisher");
            }
            _ => panic!("expected Event message"),
        }

        broker_handle.abort();
    }

    #[test]
    fn test_broker_stats() {
        let broker = EventBusBroker::new(BrokerConfig::default());
        let stats = broker.stats();
        assert_eq!(stats.total_published, 0);
        assert_eq!(stats.active_subscribers, 0);
    }
}
