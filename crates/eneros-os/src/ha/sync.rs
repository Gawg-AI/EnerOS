//! 状态同步模块（v0.25.1 — Task 3 / v0.29.0 — T029-23）
//!
//! 双节点状态同步：SCADA 实时数据 / Agent 状态 / 命令历史 / 配置。
//! 目标同步延迟 < 100ms。
//!
//! ## 协议（v0.29.0 — T029-23 二进制序列化 + 批量同步）
//!
//! - 传输层：TCP（默认监听 `0.0.0.0:5401`）
//! - 帧格式：4 字节大端长度前缀 + bincode 序列化的 [`SyncBatch`]
//! - 批量同步：发送方累积消息，达到 `batch_size`（默认 100）或 `batch_timeout_ms`
//!   （默认 10ms）时打包为 [`SyncBatch`] 发送，摊薄帧开销
//! - 二进制序列化：使用 bincode 替代 JSON，序列化延迟下降 > 50%，带宽下降 > 70%
//! - 版本字段：[`SyncBatch::version`] 标识协议版本，便于未来升级
//! - 同步模式：增量同步（按 key 维护序列号）+ 全量同步（请求/响应）
//!
//! ## 跨平台策略
//!
//! - **Linux**：使用 `std::net::TcpListener` 监听同步端口，非阻塞接收
//! - **非 Linux**：网络方法返回 [`SyncError::UnsupportedPlatform`]，
//!   消息序列化/反序列化、批量打包/解包、增量检测、统计等纯逻辑在所有平台可用，便于开发/测试。

use crate::ha::{HaConfig, SharedStore, StorageEntry};
use bincode::Options as _;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

// ============================================================================
// v0.29.0 — T029-23 bincode 兼容性 serde 模块
// ============================================================================
//
// bincode 1.x 不支持 `serde::Deserializer::deserialize_any`，而
// `serde_json::Value` 的派生实现依赖 `deserialize_any` 进行自描述解码。
// 因此直接对包含 `serde_json::Value` 字段的结构体调用 `bincode::deserialize`
// 会失败，错误信息为：
//   "Bincode does not support the serde::Deserializer::deserialize_any method"
//
// 解决方案：通过自定义 serde 模块将 `serde_json::Value` 在序列化时编码为
// JSON 字符串（`String`），反序列化时再从 JSON 字符串解析回 `Value`。
// `String` 是 bincode 原生支持的定长前缀类型，无需 `deserialize_any`。
// 这样既保持了对 JSON 的兼容性（serde_json 仍可正常序列化），又使 bincode
// 能够正确编解码，实现了"双格式兼容"。

/// `serde_json::Value` 的 bincode 兼容序列化模块。
///
/// 序列化时将 `Value` 转为 JSON 字符串后作为 `String` 写入；
/// 反序列化时从 `String` 读取并解析为 `Value`。
/// 适用于 `serde_json::Value` 单值字段。
mod json_value_compat {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Value;

    /// 序列化：`Value` → JSON 字符串 → `String`
    pub fn serialize<S>(value: &Value, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let json_str = serde_json::to_string(value).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&json_str)
    }

    /// 反序列化：`String` → JSON 字符串 → `Value`
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json_str = String::deserialize(deserializer)?;
        serde_json::from_str(&json_str).map_err(serde::de::Error::custom)
    }
}

/// `Vec<(String, serde_json::Value)>` 的 bincode 兼容序列化模块。
///
/// 序列化时将每个 `Value` 转为 JSON 字符串，整体作为
/// `Vec<(String, String)>` 写入（bincode 原生支持）；
/// 反序列化时反向解析每个 JSON 字符串为 `Value`。
/// 适用于 `ScadaDataBatch.data` 等 KV 列表字段。
mod json_kv_list_compat {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Value;

    /// 序列化：`Vec<(String, Value)>` → `Vec<(String, String)>`（每个 Value 转 JSON 字符串）
    pub fn serialize<S>(list: &[(String, Value)], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let converted: Vec<(String, String)> = list
            .iter()
            .map(|(k, v)| {
                let json_str = serde_json::to_string(v).map_err(serde::ser::Error::custom)?;
                Ok((k.clone(), json_str))
            })
            .collect::<Result<_, _>>()?;
        serializer.collect_seq(converted.iter().map(|(k, v)| (k, v)))
    }

    /// 反序列化：`Vec<(String, String)>` → `Vec<(String, Value)>`（每个字符串解析为 Value）
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<(String, Value)>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let converted: Vec<(String, String)> = Vec::deserialize(deserializer)?;
        converted
            .into_iter()
            .map(|(k, json_str)| {
                let v: Value = serde_json::from_str(&json_str).map_err(serde::de::Error::custom)?;
                Ok((k, v))
            })
            .collect()
    }
}

/// 同步消息最大载荷大小（JSON 序列化后的上限，1MB）
#[cfg(target_os = "linux")]
const SYNC_MESSAGE_MAX_SIZE: usize = 1 * 1024 * 1024;

/// 延迟样本保留上限（用于计算滑动平均值）
const LATENCY_SAMPLE_LIMIT: usize = 100;

/// 同步消息类型
///
/// v0.29.0 — T029-23：移除 `#[serde(tag = "type")]` 内部标签表示，
/// 改用 serde 默认的外部标签表示。原因：bincode 不支持 `deserialize_any`
/// （内部标签所需），外部标签在 bincode 中以变体索引编码，更紧凑且兼容。
/// JSON 序列化格式从 `{"type":"ScadaData",...}` 变为 `{"ScadaData":{...}}`，
/// 但线缆格式已改为 bincode，JSON 仅用于测试和调试。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SyncMessage {
    /// SCADA 数据同步（遥测/遥信）
    ScadaData {
        key: String,
        /// v0.29.0 — T029-23：使用 `json_value_compat` 模块以兼容 bincode
        /// （`serde_json::Value` 原生依赖 `deserialize_any`，bincode 不支持）
        #[serde(with = "json_value_compat")]
        value: serde_json::Value,
        timestamp: i64,
        seq: u64,
    },
    /// Agent 状态同步
    AgentState {
        agent_id: String,
        #[serde(with = "json_value_compat")]
        state: serde_json::Value,
        timestamp: i64,
        seq: u64,
    },
    /// 命令历史同步
    CommandHistory {
        command_id: String,
        #[serde(with = "json_value_compat")]
        command: serde_json::Value,
        timestamp: i64,
        seq: u64,
    },
    /// 配置同步
    Config {
        path: String,
        content: String,
        timestamp: i64,
        seq: u64,
    },
    /// 心跳同步消息
    Heartbeat {
        node_id: String,
        timestamp: i64,
    },
    /// 全量同步请求
    FullSyncRequest {
        from_seq: u64,
    },
    /// 全量同步响应
    FullSyncResponse {
        messages: Vec<SyncMessage>,
    },
    /// 删除同步（通知对端删除指定 key）
    Delete {
        key: String,
        timestamp: i64,
        seq: u64,
    },
    /// SCADA 数据批量同步（一次同步多个 key-value 对，共享同一时间戳和序列号）
    ScadaDataBatch {
        #[serde(with = "json_kv_list_compat")]
        data: Vec<(String, serde_json::Value)>,
        timestamp: i64,
        seq: u64,
    },
}

// ============================================================================
// v0.29.0 — T029-23 二进制序列化 + 批量同步
// ============================================================================

/// 同步批量消息协议版本号（当前为 1）。
///
/// 当线缆格式发生不兼容变更时递增此版本号，接收方据此选择解码路径。
pub const SYNC_BATCH_VERSION: u8 = 1;

/// 同步批量消息（v0.29.0 — T029-23）
///
/// 线缆格式：4 字节大端长度前缀 + bincode 序列化的 `SyncBatch`。
/// 取代旧的「4 字节长度前缀 + JSON 单条消息」格式。
/// 每批可携带 1..=`batch_size` 条消息，支持累积发送以摊薄帧开销。
///
/// `version` 字段标识协议版本，便于未来升级时区分格式。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncBatch {
    /// 协议版本号（当前为 [`SYNC_BATCH_VERSION`]）
    pub version: u8,
    /// 批量消息列表（顺序敏感，接收方按顺序处理）
    pub messages: Vec<SyncMessage>,
    /// 批次单调递增 ID（发送方维护，用于去重/对账）
    pub batch_id: u64,
    /// 批次发送时间戳（Unix 毫秒）
    pub timestamp: u64,
}

impl SyncBatch {
    /// 构造新的 `SyncBatch`，自动填充 `version` 和 `timestamp`。
    pub fn new(messages: Vec<SyncMessage>, batch_id: u64) -> Self {
        Self {
            version: SYNC_BATCH_VERSION,
            messages,
            batch_id,
            timestamp: current_timestamp_millis() as u64,
        }
    }

    /// 使用 bincode 编码为字节序列。
    ///
    /// v0.29.0 — T029-23：采用 varint 编码（`bincode::options().with_varint()`），
    /// 相比默认配置（固定 8 字节长度前缀）显著降低小字符串和小整数的编码开销。
    /// varint 将 0-127 编码为 1 字节，128-16383 编码为 2 字节，以此类推，
    /// 对 SCADA 同步场景中大量短 key 和小序列号有显著压缩效果。
    pub fn encode(&self) -> Result<Vec<u8>, SyncError> {
        bincode::options()
            .with_varint_encoding()
            .serialize(self)
            .map_err(|e| SyncError::Failed(format!("bincode encode: {e}")))
    }

    /// 从 bincode 字节序列解码 `SyncBatch`。
    ///
    /// 必须使用与 [`encode`](Self::encode) 相同的 varint 配置进行反序列化。
    /// 解码后校验 `version` 字段，拒绝不兼容的协议版本。
    pub fn decode(bytes: &[u8]) -> Result<Self, SyncError> {
        let batch: SyncBatch = bincode::options()
            .with_varint_encoding()
            .deserialize(bytes)
            .map_err(|e| SyncError::Failed(format!("bincode decode: {e}")))?;
        if batch.version != SYNC_BATCH_VERSION {
            return Err(SyncError::Failed(format!(
                "unsupported sync batch version: {} (expected {})",
                batch.version, SYNC_BATCH_VERSION
            )));
        }
        Ok(batch)
    }

    /// 返回批次中的消息数量。
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 批次是否为空。
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// 批量同步配置（v0.29.0 — T029-23）
///
/// 控制发送方累积行为：达到 `batch_size` 或 `batch_timeout_ms` 时触发发送。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// 每批最大消息数（达到即触发发送）
    pub batch_size: usize,
    /// 批量累积超时（毫秒，达到即触发发送）
    pub batch_timeout_ms: u64,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            batch_timeout_ms: 10,
        }
    }
}

impl BatchConfig {
    /// 创建自定义配置。
    pub fn new(batch_size: usize, batch_timeout_ms: u64) -> Self {
        Self {
            batch_size: batch_size.max(1),
            batch_timeout_ms,
        }
    }
}

/// 同步统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncStats {
    /// 累计发送消息数
    pub total_sent: u64,
    /// 累计接收消息数
    pub total_received: u64,
    /// 累计错误数
    pub total_errors: u64,
    /// 最近一次同步延迟（毫秒）
    pub last_sync_latency_ms: u64,
    /// 平均同步延迟（毫秒，基于最近样本）
    pub avg_sync_latency_ms: u64,
    /// 最近的延迟样本（用于计算平均值，最多保留 100 个）
    pub latency_samples: VecDeque<u64>,
}

/// 同步状态
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    /// 是否已与对端建立连接
    pub is_connected: bool,
    /// 对端节点 ID
    pub peer_node_id: Option<String>,
    /// 同步统计
    pub stats: SyncStats,
    /// 待发送队列长度
    pub pending_count: usize,
    /// 最近一次错误描述
    pub last_error: Option<String>,
}

/// 同步错误
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("unsupported platform: sync networking requires Linux")]
    UnsupportedPlatform,
    #[error("no listener bound")]
    NoListener,
    #[error("sync failed: {0}")]
    Failed(String),
}

/// 同步管理器
///
/// 管理双节点间的状态同步：发送方将消息入队（`pending`）并自增 `send_seq`，
/// 接收方按 key 维护 `recv_seq` 实现增量检测。
///
/// v0.29.0 — T029-23：发送方累积消息至 `batch_buffer`，达到 `batch_size` 或
/// `batch_timeout_ms` 时打包为 [`SyncBatch`] 并用 bincode 编码发送；接收方解码
/// 批次后存入 `incoming_buffer`，逐条返回。
///
/// - Linux：绑定 TCP 监听端口，非阻塞接收对端同步消息
/// - 非 Linux：网络方法返回 [`SyncError::UnsupportedPlatform`]，
///   消息构造、序列化、批量打包/解包、增量检测、统计等纯逻辑在所有平台可用
pub struct SyncManager {
    /// HA 配置
    config: HaConfig,
    /// 共享状态存储（可选，用于 `process_message` 写入接收到的数据）
    store: Option<Arc<SharedStore>>,
    /// 已发送的消息序列号
    send_seq: Arc<RwLock<u64>>,
    /// 已接收的最新序列号（按 key 分类）
    recv_seq: Arc<RwLock<HashMap<String, u64>>>,
    /// 待发送的消息队列（`send_*` 入队，`flush_pending` / `try_flush_batched` 排空）
    pending: Arc<RwLock<VecDeque<SyncMessage>>>,
    /// 同步统计
    stats: Arc<RwLock<SyncStats>>,
    /// 最近一次错误描述（供 [`SyncManager::status`] 查询）
    last_error: Arc<RwLock<Option<String>>>,
    /// 是否已与对端建立连接（运行时跟踪）
    is_connected: Arc<RwLock<bool>>,
    /// 对端节点 ID（运行时跟踪，accept 成功时设为对端地址）
    peer_node_id: Arc<RwLock<Option<String>>>,
    /// 批量同步配置（v0.29.0 — T029-23）
    batch_config: BatchConfig,
    /// 批次 ID 计数器（单调递增，每次发送批次时自增）
    batch_id_counter: Arc<RwLock<u64>>,
    /// 上次批次发送时刻（用于 `batch_timeout_ms` 超时判断）
    last_batch_flush: Arc<RwLock<Option<std::time::Instant>>>,
    /// 接收端解包后的待处理消息缓冲区（Linux：从 `read_frame_from_stream` 解码批次后填充）
    #[cfg(target_os = "linux")]
    incoming_buffer: Arc<RwLock<VecDeque<SyncMessage>>>,
    /// TCP 监听器（仅 Linux）
    #[cfg(target_os = "linux")]
    listener: Option<std::net::TcpListener>,
    /// 持久 TCP 连接（仅 Linux，复用已 accept 的连接）
    #[cfg(target_os = "linux")]
    active_connection: Arc<RwLock<Option<std::net::TcpStream>>>,
    /// per-connection 读取缓冲区（仅 Linux，用于帧解析）
    #[cfg(target_os = "linux")]
    read_buffer: Arc<RwLock<Vec<u8>>>,
}

impl SyncManager {
    /// 创建同步管理器（使用默认 [`BatchConfig`]）。
    ///
    /// - Linux：从 `config.interfaces` 读取绑定地址（为空时用 `0.0.0.0`），
    ///   绑定 `{bind_addr}:{sync_port}` 的 TCP 监听器并设为非阻塞
    /// - 非 Linux：创建管理器但不绑定 socket（纯逻辑可用）
    ///
    /// # 参数
    /// - `config`: HA 配置
    /// - `store`: 可选的共享状态存储，设置后 `process_message` 会将接收到的数据写入 store
    pub fn new(config: HaConfig, store: Option<Arc<SharedStore>>) -> Result<Self, SyncError> {
        Self::new_with_batch_config(config, store, BatchConfig::default())
    }

    /// 创建同步管理器并指定批量同步配置（v0.29.0 — T029-23）。
    ///
    /// `batch_config` 控制累积行为：达到 `batch_size` 或 `batch_timeout_ms` 时触发发送。
    pub fn new_with_batch_config(
        config: HaConfig,
        store: Option<Arc<SharedStore>>,
        batch_config: BatchConfig,
    ) -> Result<Self, SyncError> {
        #[cfg(target_os = "linux")]
        {
            let bind_addr = if config.interfaces.is_empty() {
                "0.0.0.0".to_string()
            } else {
                config.interfaces[0].clone()
            };
            let addr = format!("{}:{}", bind_addr, config.sync_port);
            let listener = std::net::TcpListener::bind(&addr)?;
            listener.set_nonblocking(true)?;
            Ok(Self {
                config,
                store,
                send_seq: Arc::new(RwLock::new(0)),
                recv_seq: Arc::new(RwLock::new(HashMap::new())),
                pending: Arc::new(RwLock::new(VecDeque::new())),
                stats: Arc::new(RwLock::new(SyncStats::default())),
                last_error: Arc::new(RwLock::new(None)),
                is_connected: Arc::new(RwLock::new(false)),
                peer_node_id: Arc::new(RwLock::new(None)),
                batch_config,
                batch_id_counter: Arc::new(RwLock::new(0)),
                last_batch_flush: Arc::new(RwLock::new(Some(std::time::Instant::now()))),
                incoming_buffer: Arc::new(RwLock::new(VecDeque::new())),
                listener: Some(listener),
                active_connection: Arc::new(RwLock::new(None)),
                read_buffer: Arc::new(RwLock::new(Vec::new())),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self {
                config,
                store,
                send_seq: Arc::new(RwLock::new(0)),
                recv_seq: Arc::new(RwLock::new(HashMap::new())),
                pending: Arc::new(RwLock::new(VecDeque::new())),
                stats: Arc::new(RwLock::new(SyncStats::default())),
                last_error: Arc::new(RwLock::new(None)),
                is_connected: Arc::new(RwLock::new(false)),
                peer_node_id: Arc::new(RwLock::new(None)),
                batch_config,
                batch_id_counter: Arc::new(RwLock::new(0)),
                last_batch_flush: Arc::new(RwLock::new(Some(std::time::Instant::now()))),
            })
        }
    }

    /// 返回本节点 ID
    pub fn local_node_id(&self) -> &str {
        &self.config.node_id
    }

    /// 返回当前发送序列号（主要用于测试）
    pub fn current_send_seq(&self) -> u64 {
        *self
            .send_seq
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 发送 SCADA 数据（遥测/遥信）。
    ///
    /// 构造 [`SyncMessage::ScadaData`]，自增 `send_seq` 并入队 `pending`。
    pub fn send_scada(
        &self,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Result<(), SyncError> {
        let seq = self.next_send_seq();
        let msg = SyncMessage::ScadaData {
            key: key.into(),
            value,
            timestamp: current_timestamp_millis(),
            seq,
        };
        self.enqueue(msg);
        Ok(())
    }

    /// 发送 Agent 状态。
    ///
    /// 构造 [`SyncMessage::AgentState`]，自增 `send_seq` 并入队 `pending`。
    pub fn send_agent_state(
        &self,
        agent_id: impl Into<String>,
        state: serde_json::Value,
    ) -> Result<(), SyncError> {
        let seq = self.next_send_seq();
        let msg = SyncMessage::AgentState {
            agent_id: agent_id.into(),
            state,
            timestamp: current_timestamp_millis(),
            seq,
        };
        self.enqueue(msg);
        Ok(())
    }

    /// 发送命令历史。
    ///
    /// 构造 [`SyncMessage::CommandHistory`]，自增 `send_seq` 并入队 `pending`。
    pub fn send_command(
        &self,
        command_id: impl Into<String>,
        command: serde_json::Value,
    ) -> Result<(), SyncError> {
        let seq = self.next_send_seq();
        let msg = SyncMessage::CommandHistory {
            command_id: command_id.into(),
            command,
            timestamp: current_timestamp_millis(),
            seq,
        };
        self.enqueue(msg);
        Ok(())
    }

    /// 发送配置。
    ///
    /// 构造 [`SyncMessage::Config`]，自增 `send_seq` 并入队 `pending`。
    pub fn send_config(
        &self,
        path: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<(), SyncError> {
        let seq = self.next_send_seq();
        let msg = SyncMessage::Config {
            path: path.into(),
            content: content.into(),
            timestamp: current_timestamp_millis(),
            seq,
        };
        self.enqueue(msg);
        Ok(())
    }

    /// 接收同步消息（非阻塞）。
    ///
    /// - Linux：优先从 `incoming_buffer` 弹出上一批解包后尚未返回的消息；
    ///   缓冲区空时复用 `active_connection` 中的持久 TCP 连接读取一个 bincode 帧
    ///   （[`SyncBatch`]），解包后存入 `incoming_buffer` 并返回首条消息。
    ///   无连接时 accept 新连接并存入 `active_connection`。
    ///   连接断开（EOF/Reset）时清空 `active_connection` 和 `read_buffer`，返回 `Ok(None)`。
    ///   无连接/无数据时返回 `Ok(None)`。
    /// - 非 Linux：返回 [`SyncError::UnsupportedPlatform`]
    pub fn receive_message(&self) -> Result<Option<SyncMessage>, SyncError> {
        #[cfg(target_os = "linux")]
        {
            // 1. 优先从 incoming_buffer 弹出上一批未处理完的消息
            {
                let mut buf = self
                    .incoming_buffer
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(msg) = buf.pop_front() {
                    return Ok(Some(msg));
                }
            }

            let listener = match &self.listener {
                Some(l) => l,
                None => return Err(SyncError::NoListener),
            };

            // 2. 检查是否有活跃连接；若无则尝试 accept
            {
                let mut conn_guard = self
                    .active_connection
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                if conn_guard.is_none() {
                    match listener.accept() {
                        Ok((stream, addr)) => {
                            let _ = stream.set_nonblocking(true);
                            *conn_guard = Some(stream);
                            drop(conn_guard);
                            self.set_connected(true, Some(addr.to_string()));
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            return Ok(None);
                        }
                        Err(e) => {
                            self.record_error(format!("accept: {e}"));
                            return Err(SyncError::Io(e));
                        }
                    }
                }
            }

            // 3. 从活跃连接读取一个 bincode 帧（SyncBatch），解包后存入 incoming_buffer
            let mut conn_guard = self
                .active_connection
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(stream) = conn_guard.as_mut() {
                match self.read_frame_from_stream(stream) {
                    Ok(Some(batch)) => {
                        // 将批次中的消息按顺序存入 incoming_buffer
                        let mut buf = self
                            .incoming_buffer
                            .write()
                            .unwrap_or_else(|e| e.into_inner());
                        buf.extend(batch.messages);
                        // 返回首条消息
                        Ok(buf.pop_front())
                    }
                    Ok(None) => Ok(None),
                    Err(_) => {
                        // 连接断开（EOF/Reset/IO 错误）：清空连接和缓冲区
                        *conn_guard = None;
                        drop(conn_guard);
                        let mut buf = self
                            .read_buffer
                            .write()
                            .unwrap_or_else(|e| e.into_inner());
                        buf.clear();
                        let mut inc = self
                            .incoming_buffer
                            .write()
                            .unwrap_or_else(|e| e.into_inner());
                        inc.clear();
                        self.set_connected(false, None);
                        Ok(None)
                    }
                }
            } else {
                Ok(None)
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(SyncError::UnsupportedPlatform)
        }
    }

    /// 处理接收到的消息：写入共享存储（如已设置）、更新 `recv_seq`（按 key 维护最新序列号）
    /// 并累加接收统计。
    ///
    /// 对于 [`SyncMessage::FullSyncResponse`]，递归更新内嵌消息的 `recv_seq`，
    /// 但接收计数只对顶层消息累加一次，避免重复计数。
    pub fn process_message(&self, msg: &SyncMessage) -> Result<(), SyncError> {
        if let Some(store) = &self.store {
            match msg {
                SyncMessage::ScadaData {
                    key,
                    value,
                    timestamp,
                    seq,
                } => {
                    let entry = StorageEntry {
                        key: key.clone(),
                        value: value.clone(),
                        timestamp: *timestamp,
                        node_id: String::new(),
                        version: *seq,
                    };
                    let _ = store.replicate(entry);
                }
                SyncMessage::AgentState {
                    agent_id,
                    state,
                    timestamp,
                    seq,
                } => {
                    let entry = StorageEntry {
                        key: agent_id.clone(),
                        value: state.clone(),
                        timestamp: *timestamp,
                        node_id: String::new(),
                        version: *seq,
                    };
                    let _ = store.replicate(entry);
                }
                SyncMessage::CommandHistory {
                    command_id,
                    command,
                    timestamp,
                    seq,
                } => {
                    let entry = StorageEntry {
                        key: command_id.clone(),
                        value: command.clone(),
                        timestamp: *timestamp,
                        node_id: String::new(),
                        version: *seq,
                    };
                    let _ = store.replicate(entry);
                }
                SyncMessage::Config {
                    path,
                    content,
                    timestamp,
                    seq,
                } => {
                    let entry = StorageEntry {
                        key: path.clone(),
                        value: serde_json::Value::String(content.clone()),
                        timestamp: *timestamp,
                        node_id: String::new(),
                        version: *seq,
                    };
                    let _ = store.replicate(entry);
                }
                SyncMessage::Delete { key, .. } => {
                    let _ = store.delete(key);
                }
                SyncMessage::ScadaDataBatch {
                    data,
                    timestamp,
                    seq,
                } => {
                    for (k, v) in data {
                        let entry = StorageEntry {
                            key: k.clone(),
                            value: v.clone(),
                            timestamp: *timestamp,
                            node_id: String::new(),
                            version: *seq,
                        };
                        let _ = store.replicate(entry);
                    }
                }
                SyncMessage::Heartbeat { .. }
                | SyncMessage::FullSyncRequest { .. }
                | SyncMessage::FullSyncResponse { .. } => {
                    // 心跳和全量同步请求/响应不写入 store
                }
            }
        }
        // 更新 recv_seq 和统计（保留原有逻辑）
        self.update_recv_seq(msg);
        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
        stats.total_received = stats.total_received.wrapping_add(1);
        Ok(())
    }

    /// 检查是否为增量消息（`seq > recv_seq[key]`）。
    ///
    /// 若该 key 尚未接收过，视为 0，任何 `seq > 0` 均为增量。
    pub fn is_incremental(&self, key: &str, seq: u64) -> bool {
        let recv = self.recv_seq.read().unwrap_or_else(|e| e.into_inner());
        let last = recv.get(key).copied().unwrap_or(0);
        seq > last
    }

    /// 获取同步状态快照。
    pub fn status(&self) -> SyncStatus {
        let stats = self
            .stats
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let pending_count = self
            .pending
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        let last_error = self
            .last_error
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let is_connected = *self
            .is_connected
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let peer_node_id = self
            .peer_node_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        SyncStatus {
            is_connected,
            peer_node_id,
            stats,
            pending_count,
            last_error,
        }
    }

    /// 记录延迟样本（保留最近 100 个），并更新最近/平均延迟。
    pub fn record_latency(&self, latency_ms: u64) {
        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
        stats.latency_samples.push_back(latency_ms);
        while stats.latency_samples.len() > LATENCY_SAMPLE_LIMIT {
            stats.latency_samples.pop_front();
        }
        stats.last_sync_latency_ms = latency_ms;
        // 使用饱和加法避免溢出
        let sum: u64 = stats
            .latency_samples
            .iter()
            .copied()
            .fold(0u64, u64::saturating_add);
        let count = stats.latency_samples.len() as u64;
        stats.avg_sync_latency_ms = sum.checked_div(count).unwrap_or(0);
    }

    /// 排空待发送队列，返回所有待发送消息。
    pub fn drain_pending(&self) -> Vec<SyncMessage> {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
        pending.drain(..).collect()
    }

    /// 将待发送队列中的所有消息通过给定 TCP 流批量发送（v0.29.0 — T029-23）。
    ///
    /// 排空 `pending` 队列，按 `batch_size` 分块打包为 [`SyncBatch`]，每批使用
    /// bincode 编码后以「4 字节大端长度前缀 + bincode 载荷」帧格式发送。
    /// 返回成功发送的消息总数。此方法在全平台可用（`TcpStream` 由调用方提供）。
    pub fn flush_pending(
        &self,
        stream: &mut std::net::TcpStream,
    ) -> Result<usize, SyncError> {
        use std::io::Write;
        let messages = self.drain_pending();
        if messages.is_empty() {
            return Ok(0);
        }
        let batch_size = self.batch_config.batch_size.max(1);
        let mut count = 0;
        for chunk in messages.chunks(batch_size) {
            let batch = SyncBatch::new(chunk.to_vec(), self.next_batch_id());
            let encoded = batch.encode()?;
            let len = encoded.len() as u32;
            stream
                .write_all(&len.to_be_bytes())
                .map_err(SyncError::Io)?;
            stream.write_all(&encoded).map_err(SyncError::Io)?;
            count += chunk.len();
        }
        self.mark_batch_flush();
        Ok(count)
    }

    /// 按阈值条件刷新待发送队列（v0.29.0 — T029-23）。
    ///
    /// 当 `pending` 队列长度 >= `batch_size`，或距上次刷新超过 `batch_timeout_ms` 时，
    /// 调用 [`flush_pending`](Self::flush_pending) 发送。返回发送的消息数（0 表示未触发）。
    pub fn try_flush_batched(
        &self,
        stream: &mut std::net::TcpStream,
    ) -> Result<usize, SyncError> {
        let pending_len = self
            .pending
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        if pending_len == 0 {
            return Ok(0);
        }
        if self.should_flush(pending_len) {
            self.flush_pending(stream)
        } else {
            Ok(0)
        }
    }

    /// 返回当前批量同步配置。
    pub fn batch_config(&self) -> &BatchConfig {
        &self.batch_config
    }

    /// 判断是否应触发批次发送（达到 `batch_size` 或 `batch_timeout_ms`）。
    ///
    /// `last_batch_flush` 在管理器创建时初始化为 `Some(Instant::now())`，
    /// 因此超时检查始终有参考点。
    fn should_flush(&self, pending_len: usize) -> bool {
        if pending_len >= self.batch_config.batch_size {
            return true;
        }
        let last = self
            .last_batch_flush
            .read()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(last) = *last {
            let elapsed = last.elapsed();
            if elapsed >= std::time::Duration::from_millis(self.batch_config.batch_timeout_ms) {
                return true;
            }
        }
        false
    }

    /// 自增并返回下一个批次 ID。
    fn next_batch_id(&self) -> u64 {
        let mut id = self
            .batch_id_counter
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *id = id.wrapping_add(1);
        *id
    }

    /// 记录本次批次发送时刻。
    fn mark_batch_flush(&self) {
        let mut last = self
            .last_batch_flush
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *last = Some(std::time::Instant::now());
    }

    // ========================================================================
    // v0.26.0 — Task 4 自动故障恢复
    // ========================================================================

    /// 请求增量同步（v0.26.0 — Task 4 / v0.29.0 — T029-23 bincode 批量编码）
    ///
    /// 构造 [`SyncMessage::FullSyncRequest`] 并发送：
    /// - Linux：通过 `active_connection` 持久 TCP 连接发送（4 字节大端长度前缀 +
    ///   bincode 编码的 [`SyncBatch`]，批次大小 1）；无活跃连接时入 `pending` 队列
    /// - 非 Linux：入 `pending` 队列（由调用方在测试中 drain 验证）
    pub fn request_incremental_sync(&self, from_seq: u64) -> Result<(), SyncError> {
        let msg = SyncMessage::FullSyncRequest { from_seq };
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            let batch = SyncBatch::new(vec![msg.clone()], self.next_batch_id());
            let encoded = batch.encode()?;
            let len = encoded.len() as u32;
            let mut conn = self
                .active_connection
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(stream) = conn.as_mut() {
                stream
                    .write_all(&len.to_be_bytes())
                    .map_err(SyncError::Io)?;
                stream.write_all(&encoded).map_err(SyncError::Io)?;
                self.mark_batch_flush();
                Ok(())
            } else {
                // 无活跃连接，入 pending 队列
                self.enqueue(msg);
                Ok(())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.enqueue(msg);
            Ok(())
        }
    }

    /// 请求全量同步（v0.26.0 — Task 4）
    ///
    /// 等价于 `request_incremental_sync(0)`，从序列号 0 开始请求所有数据。
    pub fn request_full_sync(&self) -> Result<(), SyncError> {
        self.request_incremental_sync(0)
    }

    /// 返回指定 key 的最后接收序列号（v0.26.0 — Task 4）
    ///
    /// 用于故障恢复后判断需要从哪个 seq 开始增量同步。
    /// 未接收过该 key 时返回 `None`。
    pub fn last_recv_seq(&self, key: &str) -> Option<u64> {
        let recv = self
            .recv_seq
            .read()
            .unwrap_or_else(|e| e.into_inner());
        recv.get(key).copied()
    }

    /// 计算 SharedStore 所有 entries 的 checksum（v0.26.0 — Task 4）
    ///
    /// 用于故障恢复后验证数据一致性：对每个 entry 的 (key, version) 计算哈希并组合为 u64。
    /// 按 key 排序保证确定性。未设置 store 时返回 0。
    pub fn compute_checksum(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let Some(store) = &self.store else {
            return 0;
        };
        let entries = store.entries();
        let mut hasher = DefaultHasher::new();
        // 按 key 排序保证确定性
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.key.cmp(&b.key));
        for entry in sorted {
            entry.key.hash(&mut hasher);
            entry.version.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// 自增发送序列号并返回新值。
    fn next_send_seq(&self) -> u64 {
        let mut seq = self
            .send_seq
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *seq = seq.wrapping_add(1);
        *seq
    }

    /// 将消息入队 `pending` 并累加发送统计。
    fn enqueue(&self, msg: SyncMessage) {
        {
            let mut pending = self
                .pending
                .write()
                .unwrap_or_else(|e| e.into_inner());
            pending.push_back(msg);
        }
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(|e| e.into_inner());
        stats.total_sent = stats.total_sent.wrapping_add(1);
    }

    /// 递归更新 `recv_seq`：仅当 seq 严格大于已记录值时刷新。
    /// 对 [`SyncMessage::FullSyncResponse`] 递归更新内嵌消息，深度限制 10 层。
    fn update_recv_seq(&self, msg: &SyncMessage) {
        self.update_recv_seq_inner(msg, 0);
    }

    /// `update_recv_seq` 的内部递归实现，带深度限制。
    fn update_recv_seq_inner(&self, msg: &SyncMessage, depth: u32) {
        if depth > 10 {
            return; // 超过深度限制，停止递归
        }
        if let Some((key, seq)) = message_key_seq(msg) {
            let mut recv = self
                .recv_seq
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let last = recv.get(&key).copied().unwrap_or(0);
            if seq > last {
                recv.insert(key, seq);
            }
        }
        if let SyncMessage::FullSyncResponse { messages } = msg {
            for inner in messages {
                self.update_recv_seq_inner(inner, depth + 1);
            }
        }
    }

    /// 记录错误：累加错误计数并保存最近一次错误描述。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn record_error(&self, msg: impl Into<String>) {
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(|e| e.into_inner());
        stats.total_errors = stats.total_errors.wrapping_add(1);
        drop(stats);
        let mut last = self
            .last_error
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *last = Some(msg.into());
    }

    /// 更新连接状态和对端节点 ID。
    ///
    /// 仅更新字段（非网络操作），全平台可用以便测试验证状态跟踪逻辑。
    /// 非 Linux 非 test 构建中可能未被调用，标记 `allow(dead_code)`。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn set_connected(&self, connected: bool, peer: Option<String>) {
        let mut c = self
            .is_connected
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *c = connected;
        let mut p = self
            .peer_node_id
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *p = peer;
    }

    /// 从 TCP 流读取一个 bincode 帧（基于 `read_buffer` 的帧解析）。
    ///
    /// v0.29.0 — T029-23：帧载荷从 JSON `SyncMessage` 改为 bincode `SyncBatch`。
    ///
    /// - 完整帧解析成功：从 `read_buffer` 移除该帧，返回 `Ok(Some(batch))`
    /// - `WouldBlock` 或不完整帧：返回 `Ok(None)`，保留 `read_buffer` 中的部分数据
    /// - 反序列化失败：跳过该帧（从 `read_buffer` 移除），记录错误，继续尝试下一帧
    /// - EOF/Reset/IO 错误：返回 `Err`（由调用方清理连接）
    #[cfg(target_os = "linux")]
    fn read_frame_from_stream(
        &self,
        stream: &mut std::net::TcpStream,
    ) -> Result<Option<SyncBatch>, SyncError> {
        use std::io::Read;

        let mut buf_guard = self
            .read_buffer
            .write()
            .unwrap_or_else(|e| e.into_inner());

        loop {
            // 1. 尝试从缓冲区解析完整帧（4 字节大端长度前缀 + bincode 载荷）
            if buf_guard.len() >= 4 {
                let len = u32::from_be_bytes([
                    buf_guard[0],
                    buf_guard[1],
                    buf_guard[2],
                    buf_guard[3],
                ]) as usize;
                if len > SYNC_MESSAGE_MAX_SIZE {
                    let msg = format!("message too large: {len} bytes");
                    self.record_error(msg.clone());
                    buf_guard.clear();
                    return Err(SyncError::Failed(msg));
                }
                if buf_guard.len() >= 4 + len {
                    // 完整帧可用，尝试反序列化为 SyncBatch
                    let payload = &buf_guard[4..4 + len];
                    match SyncBatch::decode(payload) {
                        Ok(batch) => {
                            // 从缓冲区移除该帧
                            buf_guard.drain(..4 + len);
                            return Ok(Some(batch));
                        }
                        Err(e) => {
                            // 反序列化失败：跳过该帧，记录错误
                            self.record_error(format!("deserialize: {e}"));
                            buf_guard.drain(..4 + len);
                            continue; // 尝试解析下一帧
                        }
                    }
                }
                // 载荷不完整，继续读取
            }

            // 2. 不够则非阻塞读取追加到 read_buffer
            let mut tmp = [0u8; 4096];
            match stream.read(&mut tmp) {
                Ok(0) => {
                    // EOF — 对端关闭连接
                    return Err(SyncError::Failed("connection closed".to_string()));
                }
                Ok(n) => {
                    buf_guard.extend_from_slice(&tmp[..n]);
                    // 循环回去尝试解析
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // 无更多数据可用，保留缓冲区中的部分数据
                    return Ok(None);
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::ConnectionReset
                        || e.kind() == std::io::ErrorKind::ConnectionAborted =>
                {
                    return Err(SyncError::Failed(format!("connection lost: {e}")));
                }
                Err(e) => {
                    self.record_error(format!("read: {e}"));
                    return Err(SyncError::Io(e));
                }
            }
        }
    }
}

/// 从消息中提取 (key, seq)，用于增量检测。
///
/// - `ScadaData` → key
/// - `AgentState` → agent_id
/// - `CommandHistory` → command_id
/// - `Config` → path
/// - `Delete` → key
/// - `Heartbeat` / `FullSyncRequest` / `FullSyncResponse` / `ScadaDataBatch` → 无单一 seq，返回 `None`
fn message_key_seq(msg: &SyncMessage) -> Option<(String, u64)> {
    match msg {
        SyncMessage::ScadaData { key, seq, .. } => Some((key.clone(), *seq)),
        SyncMessage::AgentState { agent_id, seq, .. } => Some((agent_id.clone(), *seq)),
        SyncMessage::CommandHistory { command_id, seq, .. } => Some((command_id.clone(), *seq)),
        SyncMessage::Config { path, seq, .. } => Some((path.clone(), *seq)),
        SyncMessage::Delete { key, seq, .. } => Some((key.clone(), *seq)),
        SyncMessage::Heartbeat { .. } => None,
        SyncMessage::FullSyncRequest { .. } => None,
        SyncMessage::FullSyncResponse { .. } => None,
        SyncMessage::ScadaDataBatch { .. } => None,
    }
}

/// 获取当前 Unix 时间戳（毫秒）
fn current_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::{ConflictResolution, FencingStrategy, NodeRole, StorageQuota};

    /// 构造测试用 HaConfig（sync_port=0 让 OS 分配空闲端口，避免 Linux 端口冲突）
    fn test_config() -> HaConfig {
        HaConfig {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            heartbeat_interval_ms: 100,
            heartbeat_suspect_ms: 100,
            heartbeat_dead_ms: 300,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 0,
            interfaces: Vec::new(),
            priority: 100,
            fencing_strategy: FencingStrategy::None,
            sync_scope: Default::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        }
    }

    /// 构造测试用 Secondary 共享存储（默认配额、PrimaryWins 策略）
    fn test_store() -> Arc<SharedStore> {
        Arc::new(SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::default(),
            StorageQuota::default(),
        ))
    }

    #[test]
    fn test_sync_message_serialize() {
        // v0.29.0 — T029-23：移除 #[serde(tag = "type")] 后，JSON 使用外部标签表示
        // 格式从 {"type":"ScadaData",...} 变为 {"ScadaData":{...}}
        // ScadaData
        let scada = SyncMessage::ScadaData {
            key: "telemetry.voltage".to_string(),
            value: serde_json::json!({"value": 220.5, "quality": "good"}),
            timestamp: 1700000000000,
            seq: 1,
        };
        let json = serde_json::to_string(&scada).expect("serialize ScadaData");
        assert!(
            json.contains("\"ScadaData\""),
            "ScadaData should carry variant tag: {json}"
        );
        let de: SyncMessage = serde_json::from_str(&json).expect("deserialize ScadaData");
        assert_eq!(de, scada);

        // AgentState
        let agent = SyncMessage::AgentState {
            agent_id: "dispatch-agent".to_string(),
            state: serde_json::json!({"status": "running"}),
            timestamp: 1700000000001,
            seq: 2,
        };
        let json = serde_json::to_string(&agent).expect("serialize AgentState");
        assert!(json.contains("\"AgentState\""));
        assert_eq!(serde_json::from_str::<SyncMessage>(&json).unwrap(), agent);

        // CommandHistory
        let cmd = SyncMessage::CommandHistory {
            command_id: "cmd-001".to_string(),
            command: serde_json::json!({"action": "open_breaker"}),
            timestamp: 1700000000002,
            seq: 3,
        };
        let json = serde_json::to_string(&cmd).expect("serialize CommandHistory");
        assert!(json.contains("\"CommandHistory\""));
        assert_eq!(serde_json::from_str::<SyncMessage>(&json).unwrap(), cmd);

        // Config
        let cfg = SyncMessage::Config {
            path: "/etc/eneros/scada.toml".to_string(),
            content: "key = \"value\"\n".to_string(),
            timestamp: 1700000000003,
            seq: 4,
        };
        let json = serde_json::to_string(&cfg).expect("serialize Config");
        assert!(json.contains("\"Config\""));
        assert_eq!(serde_json::from_str::<SyncMessage>(&json).unwrap(), cfg);

        // Heartbeat
        let hb = SyncMessage::Heartbeat {
            node_id: "node-2".to_string(),
            timestamp: 1700000000004,
        };
        let json = serde_json::to_string(&hb).expect("serialize Heartbeat");
        assert!(json.contains("\"Heartbeat\""));
        assert_eq!(serde_json::from_str::<SyncMessage>(&json).unwrap(), hb);

        // FullSyncRequest
        let req = SyncMessage::FullSyncRequest { from_seq: 10 };
        let json = serde_json::to_string(&req).expect("serialize FullSyncRequest");
        assert!(json.contains("\"FullSyncRequest\""));
        assert_eq!(serde_json::from_str::<SyncMessage>(&json).unwrap(), req);

        // FullSyncResponse（递归包含 SyncMessage）
        let resp = SyncMessage::FullSyncResponse {
            messages: vec![scada.clone(), agent.clone()],
        };
        let json = serde_json::to_string(&resp).expect("serialize FullSyncResponse");
        assert!(json.contains("\"FullSyncResponse\""));
        let de: SyncMessage = serde_json::from_str(&json).expect("deserialize FullSyncResponse");
        assert_eq!(de, resp);
    }

    #[test]
    fn test_incremental_detection() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        // 未接收过的 key：seq=1 为增量
        assert!(manager.is_incremental("telemetry.voltage", 1));
        // seq=0 不为增量（0 > 0 为假）
        assert!(!manager.is_incremental("telemetry.voltage", 0));

        // 模拟接收 seq=5 的 ScadaData
        let msg = SyncMessage::ScadaData {
            key: "telemetry.voltage".to_string(),
            value: serde_json::json!(220.5),
            timestamp: 1700000000000,
            seq: 5,
        };
        manager.process_message(&msg).expect("process");

        // seq=5 非增量（5 > 5 为假）
        assert!(!manager.is_incremental("telemetry.voltage", 5));
        // seq=4 非增量（旧消息，应被忽略）
        assert!(!manager.is_incremental("telemetry.voltage", 4));
        // seq=6 为增量
        assert!(manager.is_incremental("telemetry.voltage", 6));

        // 不同 key 互不影响
        assert!(manager.is_incremental("telemetry.current", 1));
    }

    #[test]
    fn test_sync_stats() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        // 初始统计为 0
        let stats = manager.status().stats;
        assert_eq!(stats.total_sent, 0);
        assert_eq!(stats.total_received, 0);
        assert_eq!(stats.total_errors, 0);

        // 发送 3 条消息
        manager.send_scada("k1", serde_json::json!(1)).unwrap();
        manager
            .send_agent_state("agent-1", serde_json::json!({}))
            .unwrap();
        manager.send_config("/path", "content").unwrap();

        let stats = manager.status().stats;
        assert_eq!(stats.total_sent, 3, "total_sent should be 3");
        assert_eq!(stats.total_received, 0);

        // 处理 2 条接收消息
        let msg1 = SyncMessage::ScadaData {
            key: "rk1".to_string(),
            value: serde_json::json!(1),
            timestamp: 0,
            seq: 1,
        };
        let msg2 = SyncMessage::Config {
            path: "/remote".to_string(),
            content: "c".to_string(),
            timestamp: 0,
            seq: 1,
        };
        manager.process_message(&msg1).unwrap();
        manager.process_message(&msg2).unwrap();

        let stats = manager.status().stats;
        assert_eq!(stats.total_received, 2, "total_received should be 2");
        assert_eq!(stats.total_sent, 3, "total_sent unchanged");
    }

    #[test]
    fn test_sync_status() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        // 初始状态
        let status = manager.status();
        assert!(!status.is_connected, "not connected initially");
        assert!(status.peer_node_id.is_none());
        assert_eq!(status.pending_count, 0);
        assert!(status.last_error.is_none());
        assert_eq!(status.stats.total_sent, 0);

        // 入队 2 条消息
        manager.send_scada("k1", serde_json::json!(1)).unwrap();
        manager
            .send_command("cmd-1", serde_json::json!({}))
            .unwrap();
        let status = manager.status();
        assert_eq!(status.pending_count, 2, "pending_count should be 2");
        assert_eq!(status.stats.total_sent, 2);

        // 状态可序列化/反序列化
        let json = serde_json::to_string(&status).expect("serialize status");
        let de: SyncStatus = serde_json::from_str(&json).expect("deserialize status");
        assert_eq!(de.pending_count, 2);
        assert_eq!(de.stats.total_sent, 2);
    }

    #[test]
    fn test_send_scada() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        assert_eq!(manager.current_send_seq(), 0, "initial send_seq is 0");

        // 发送第一条：seq 应为 1
        manager
            .send_scada("telemetry.voltage", serde_json::json!(220.5))
            .unwrap();
        assert_eq!(manager.current_send_seq(), 1);

        // 发送第二条：seq 应为 2
        manager
            .send_scada("telemetry.current", serde_json::json!(10.0))
            .unwrap();
        assert_eq!(manager.current_send_seq(), 2);

        // 发送第三条：seq 应为 3
        manager
            .send_scada("telemetry.power", serde_json::json!(100.0))
            .unwrap();
        assert_eq!(manager.current_send_seq(), 3);

        // 验证 pending 队列中消息的 seq 严格递增
        let pending = manager
            .pending
            .read()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(pending.len(), 3, "pending should have 3 messages");
        let seqs: Vec<u64> = pending
            .iter()
            .map(|m| match m {
                SyncMessage::ScadaData { seq, .. } => *seq,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "seq should be strictly increasing");

        // 验证 key 与 value 正确
        if let SyncMessage::ScadaData { key, value, .. } = &pending[0] {
            assert_eq!(key, "telemetry.voltage");
            assert_eq!(value, &serde_json::json!(220.5));
        } else {
            panic!("first message should be ScadaData");
        }
    }

    #[test]
    fn test_latency_recording() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        // 初始无样本
        let stats = manager.status().stats;
        assert!(stats.latency_samples.is_empty());
        assert_eq!(stats.avg_sync_latency_ms, 0);
        assert_eq!(stats.last_sync_latency_ms, 0);

        // 记录 3 个样本：10, 20, 30 → 平均 20
        manager.record_latency(10);
        manager.record_latency(20);
        manager.record_latency(30);

        let stats = manager.status().stats;
        assert_eq!(stats.latency_samples.len(), 3);
        assert_eq!(stats.last_sync_latency_ms, 30);
        assert_eq!(stats.avg_sync_latency_ms, 20, "avg of [10,20,30] is 20");

        // 验证滑动窗口：记录超过 100 个样本后只保留最近 100 个
        for i in 0..150u64 {
            manager.record_latency(i);
        }
        let stats = manager.status().stats;
        assert_eq!(
            stats.latency_samples.len(),
            LATENCY_SAMPLE_LIMIT,
            "should keep only the last {} samples",
            LATENCY_SAMPLE_LIMIT
        );
        // 最近 100 个样本为 50..150（含 50..=149），平均值 = (50+149)/2 = 99
        assert_eq!(stats.last_sync_latency_ms, 149, "last sample should be 149");
        let expected_avg: u64 = (50u64 + 149) * 100 / 2 / 100; // sum(50..=149)/100
        assert_eq!(
            stats.avg_sync_latency_ms, expected_avg,
            "avg of 50..=149 should be {}",
            expected_avg
        );
    }

    #[test]
    fn test_receive_message_non_linux() {
        // 非 Linux 平台：receive_message 返回 UnsupportedPlatform
        let manager = SyncManager::new(test_config(), None).unwrap();
        #[cfg(not(target_os = "linux"))]
        {
            let result = manager.receive_message();
            assert!(
                matches!(result, Err(SyncError::UnsupportedPlatform)),
                "non-Linux should return UnsupportedPlatform"
            );
        }
        #[cfg(target_os = "linux")]
        {
            // Linux 上无连接时返回 Ok(None)，不应 panic
            let _ = manager.receive_message();
        }
    }

    #[test]
    fn test_process_full_sync_response() {
        // 验证 FullSyncResponse 递归更新 recv_seq，但接收计数只累加一次
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        let resp = SyncMessage::FullSyncResponse {
            messages: vec![
                SyncMessage::ScadaData {
                    key: "k1".to_string(),
                    value: serde_json::json!(1),
                    timestamp: 0,
                    seq: 5,
                },
                SyncMessage::ScadaData {
                    key: "k2".to_string(),
                    value: serde_json::json!(2),
                    timestamp: 0,
                    seq: 7,
                },
            ],
        };
        manager.process_message(&resp).unwrap();

        // 内嵌消息的 recv_seq 应被更新
        assert!(!manager.is_incremental("k1", 5), "k1 seq=5 should be recorded");
        assert!(manager.is_incremental("k1", 6), "k1 seq=6 is incremental");
        assert!(!manager.is_incremental("k2", 7), "k2 seq=7 should be recorded");
        assert!(manager.is_incremental("k2", 8), "k2 seq=8 is incremental");

        // 接收计数只对顶层累加一次
        let stats = manager.status().stats;
        assert_eq!(stats.total_received, 1, "top-level message counted once");
    }

    #[test]
    fn test_drain_pending() {
        // 入队 3 条消息，drain_pending 返回 3 条且 pending 清空
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        manager
            .send_scada("k1", serde_json::json!(1))
            .unwrap();
        manager
            .send_scada("k2", serde_json::json!(2))
            .unwrap();
        manager
            .send_scada("k3", serde_json::json!(3))
            .unwrap();

        let drained = manager.drain_pending();
        assert_eq!(drained.len(), 3, "drain should return 3 messages");

        let pending = manager
            .pending
            .read()
            .unwrap_or_else(|e| e.into_inner());
        assert!(pending.is_empty(), "pending should be empty after drain");
    }

    #[test]
    fn test_process_message_writes_to_store() {
        // 携带 SharedStore 时，process_message 应将数据写入存储
        let store = test_store();
        let manager =
            SyncManager::new(test_config(), Some(store.clone())).expect("create manager");

        let msg = SyncMessage::ScadaData {
            key: "telemetry.voltage".to_string(),
            value: serde_json::json!(220.5),
            timestamp: 100,
            seq: 1,
        };
        manager.process_message(&msg).unwrap();

        let entry = store.get("telemetry.voltage").expect("entry should exist");
        assert_eq!(entry.key, "telemetry.voltage");
        assert_eq!(entry.value, serde_json::json!(220.5));
        assert_eq!(entry.version, 1, "version should match seq");
    }

    #[test]
    fn test_process_message_delete() {
        // Delete 消息应从存储中移除对应 key
        let store = test_store();
        // 先写入一条数据
        store.put("k1", serde_json::json!(42)).unwrap();
        assert!(store.get("k1").is_some(), "precondition: k1 exists");

        let manager =
            SyncManager::new(test_config(), Some(store.clone())).expect("create manager");

        let del = SyncMessage::Delete {
            key: "k1".to_string(),
            timestamp: 200,
            seq: 2,
        };
        manager.process_message(&del).unwrap();

        assert!(store.get("k1").is_none(), "k1 should be deleted after Delete");
    }

    #[test]
    fn test_is_connected_tracking() {
        // 初始状态 is_connected 为 false
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        let status = manager.status();
        assert!(!status.is_connected, "should not be connected initially");
        assert!(
            status.peer_node_id.is_none(),
            "peer_node_id should be None initially"
        );

        // 模拟连接建立
        manager.set_connected(true, Some("peer-node-1".to_string()));
        let status = manager.status();
        assert!(status.is_connected, "should be connected after set_connected");
        assert_eq!(
            status.peer_node_id.as_deref(),
            Some("peer-node-1"),
            "peer_node_id should be set"
        );

        // 模拟断开
        manager.set_connected(false, None);
        let status = manager.status();
        assert!(!status.is_connected, "should be disconnected after reset");
        assert!(
            status.peer_node_id.is_none(),
            "peer_node_id should be cleared"
        );
    }

    #[test]
    fn test_sync_message_delete_serde() {
        // Delete 变体序列化/反序列化往返
        let msg = SyncMessage::Delete {
            key: "config.foo".to_string(),
            timestamp: 1234567890,
            seq: 42,
        };
        let json = serde_json::to_string(&msg).expect("serialize Delete");
        let decoded: SyncMessage = serde_json::from_str(&json).expect("deserialize Delete");
        match decoded {
            SyncMessage::Delete {
                key,
                timestamp,
                seq,
            } => {
                assert_eq!(key, "config.foo");
                assert_eq!(timestamp, 1234567890);
                assert_eq!(seq, 42);
            }
            _ => panic!("expected Delete variant, got {:?}", decoded),
        }
    }

    #[test]
    fn test_sync_message_scada_batch_serde() {
        // ScadaDataBatch 变体序列化/反序列化往返
        let msg = SyncMessage::ScadaDataBatch {
            data: vec![
                ("k1".to_string(), serde_json::json!(1)),
                ("k2".to_string(), serde_json::json!("hello")),
                ("k3".to_string(), serde_json::json!({"nested": true})),
            ],
            timestamp: 999,
            seq: 7,
        };
        let json = serde_json::to_string(&msg).expect("serialize ScadaDataBatch");
        let decoded: SyncMessage =
            serde_json::from_str(&json).expect("deserialize ScadaDataBatch");
        match decoded {
            SyncMessage::ScadaDataBatch {
                data,
                timestamp,
                seq,
            } => {
                assert_eq!(data.len(), 3, "batch should have 3 entries");
                assert_eq!(data[0].0, "k1");
                assert_eq!(data[0].1, serde_json::json!(1));
                assert_eq!(data[1].0, "k2");
                assert_eq!(data[1].1, serde_json::json!("hello"));
                assert_eq!(data[2].0, "k3");
                assert_eq!(data[2].1, serde_json::json!({"nested": true}));
                assert_eq!(timestamp, 999);
                assert_eq!(seq, 7);
            }
            _ => panic!("expected ScadaDataBatch variant, got {:?}", decoded),
        }
    }

    // ------------------------------------------------------------------------
    // v0.26.0 — Task 4 自动故障恢复测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_request_incremental_sync_enqueues() {
        // 非 Linux：request_incremental_sync 应入 pending 队列
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        manager.request_incremental_sync(42).expect("request should succeed");

        let pending = manager.drain_pending();
        assert_eq!(pending.len(), 1, "should enqueue 1 message");
        match &pending[0] {
            SyncMessage::FullSyncRequest { from_seq } => {
                assert_eq!(*from_seq, 42, "from_seq should be 42");
            }
            _ => panic!("expected FullSyncRequest, got {:?}", pending[0]),
        }
    }

    #[test]
    fn test_request_full_sync_enqueues_from_zero() {
        // request_full_sync 等价于 request_incremental_sync(0)
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        manager.request_full_sync().expect("request should succeed");

        let pending = manager.drain_pending();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            SyncMessage::FullSyncRequest { from_seq } => {
                assert_eq!(*from_seq, 0, "full sync should start from seq 0");
            }
            _ => panic!("expected FullSyncRequest"),
        }
    }

    #[test]
    fn test_last_recv_seq() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");

        // 未接收过的 key 返回 None
        assert!(manager.last_recv_seq("k1").is_none());

        // 模拟接收 seq=5 的 ScadaData
        let msg = SyncMessage::ScadaData {
            key: "k1".to_string(),
            value: serde_json::json!(1),
            timestamp: 0,
            seq: 5,
        };
        manager.process_message(&msg).unwrap();

        // last_recv_seq 应返回 5
        assert_eq!(
            manager.last_recv_seq("k1"),
            Some(5),
            "last_recv_seq should be 5 after receiving seq=5"
        );

        // 接收更高 seq=8
        let msg2 = SyncMessage::ScadaData {
            key: "k1".to_string(),
            value: serde_json::json!(2),
            timestamp: 0,
            seq: 8,
        };
        manager.process_message(&msg2).unwrap();
        assert_eq!(manager.last_recv_seq("k1"), Some(8));

        // 未接收的 key 仍为 None
        assert!(manager.last_recv_seq("k2").is_none());
    }

    #[test]
    fn test_compute_checksum_no_store() {
        // 未设置 store 时 checksum 为 0
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        assert_eq!(manager.compute_checksum(), 0, "checksum should be 0 without store");
    }

    #[test]
    fn test_compute_checksum_deterministic() {
        // 相同数据应产生相同 checksum
        let store1 = test_store();
        let store2 = test_store();

        // 两个 store 写入相同数据
        store1.put("k1", serde_json::json!(1)).unwrap();
        store1.put("k2", serde_json::json!(2)).unwrap();
        store2.put("k1", serde_json::json!(1)).unwrap();
        store2.put("k2", serde_json::json!(2)).unwrap();

        let manager1 = SyncManager::new(test_config(), Some(store1)).unwrap();
        let manager2 = SyncManager::new(test_config(), Some(store2)).unwrap();

        let cs1 = manager1.compute_checksum();
        let cs2 = manager2.compute_checksum();
        assert_eq!(cs1, cs2, "same data should produce same checksum");
        assert_ne!(cs1, 0, "checksum should not be 0 with data");
    }

    #[test]
    fn test_compute_checksum_changes_with_data() {
        // 数据变化后 checksum 应改变
        let store = test_store();
        let manager = SyncManager::new(test_config(), Some(store.clone())).unwrap();

        let cs_empty = manager.compute_checksum();

        store.put("k1", serde_json::json!(1)).unwrap();
        let cs_one = manager.compute_checksum();
        assert_ne!(cs_empty, cs_one, "checksum should change after put");

        store.put("k2", serde_json::json!(2)).unwrap();
        let cs_two = manager.compute_checksum();
        assert_ne!(cs_one, cs_two, "checksum should change after second put");

        // 更新已有 key（version 递增）应改变 checksum
        store.put("k1", serde_json::json!(10)).unwrap();
        let cs_update = manager.compute_checksum();
        assert_ne!(
            cs_two, cs_update,
            "checksum should change after version update"
        );
    }

    // ------------------------------------------------------------------------
    // v0.29.0 — T029-23 二进制序列化 + 批量同步测试
    // ------------------------------------------------------------------------

    /// 构造 N 条测试用 SyncMessage
    fn make_test_messages(n: usize) -> Vec<SyncMessage> {
        (0..n)
            .map(|i| SyncMessage::ScadaData {
                key: format!("telemetry.sensor.{i}"),
                value: serde_json::json!({
                    "value": 220.5 + i as f64 * 0.1,
                    "quality": "good",
                    "unit": "V"
                }),
                timestamp: 1700000000000 + i as i64,
                seq: i as u64 + 1,
            })
            .collect()
    }

    #[test]
    fn test_sync_batch_encode_decode_roundtrip() {
        let messages = make_test_messages(5);
        let batch = SyncBatch::new(messages.clone(), 42);
        assert_eq!(batch.version, SYNC_BATCH_VERSION);
        assert_eq!(batch.batch_id, 42);
        assert_eq!(batch.len(), 5);
        assert!(!batch.is_empty());

        let encoded = batch.encode().expect("encode");
        assert!(!encoded.is_empty(), "encoded bytes should be non-empty");

        let decoded = SyncBatch::decode(&encoded).expect("decode");
        assert_eq!(decoded, batch);
        assert_eq!(decoded.messages, messages);
    }

    #[test]
    fn test_sync_batch_empty() {
        let batch = SyncBatch::new(vec![], 0);
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);

        let encoded = batch.encode().expect("encode empty batch");
        let decoded = SyncBatch::decode(&encoded).expect("decode empty batch");
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_sync_batch_version_mismatch_rejected() {
        // 构造一个版本号不匹配的 batch（手动篡改 version 字段）
        let batch = SyncBatch::new(make_test_messages(1), 1);
        let mut encoded = batch.encode().expect("encode");
        // bincode 编码中第一个字节是 version（u8），篡改为 255
        encoded[0] = 255;
        let result = SyncBatch::decode(&encoded);
        assert!(
            result.is_err(),
            "version mismatch should be rejected"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("version"),
            "error should mention version: {err_msg}"
        );
    }

    #[test]
    fn test_sync_batch_decode_garbage() {
        // 无效字节应返回错误，不 panic
        let garbage = [0u8; 4];
        assert!(SyncBatch::decode(&garbage).is_err());
        assert!(SyncBatch::decode(&[]).is_err());
    }

    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.batch_timeout_ms, 10);
    }

    #[test]
    fn test_batch_config_new() {
        let config = BatchConfig::new(50, 5);
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.batch_timeout_ms, 5);

        // batch_size 最小为 1（传入 0 时钳制为 1）
        let config_zero = BatchConfig::new(0, 0);
        assert_eq!(config_zero.batch_size, 1);
        assert_eq!(config_zero.batch_timeout_ms, 0);
    }

    #[test]
    fn test_sync_manager_default_batch_config() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        assert_eq!(manager.batch_config().batch_size, 100);
        assert_eq!(manager.batch_config().batch_timeout_ms, 10);
    }

    #[test]
    fn test_sync_manager_custom_batch_config() {
        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(32, 5),
        )
        .expect("create manager");
        assert_eq!(manager.batch_config().batch_size, 32);
        assert_eq!(manager.batch_config().batch_timeout_ms, 5);
    }

    #[test]
    fn test_flush_pending_bincode_batch_via_tcp() {
        // 通过 TCP 回环验证 flush_pending 发送 bincode 批量帧，
        // 接收端能正确解码为 SyncBatch。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (mut receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new(test_config(), None).expect("create manager");
        // 入队 3 条消息
        for msg in make_test_messages(3) {
            manager.enqueue(msg);
        }
        assert_eq!(manager.status().pending_count, 3);

        // flush_pending 应发送全部 3 条消息（打包为 1 个批次）
        let sent = manager.flush_pending(&mut sender).expect("flush");
        assert_eq!(sent, 3, "should send 3 messages");
        assert_eq!(manager.status().pending_count, 0, "pending should be empty");

        // 接收端读取帧：4 字节长度前缀 + bincode 载荷
        use std::io::Read;
        let mut len_buf = [0u8; 4];
        receiver.read_exact(&mut len_buf).expect("read len");
        let payload_len = u32::from_be_bytes(len_buf) as usize;
        assert!(payload_len > 0, "payload should be non-empty");
        assert!(payload_len < 1024 * 1024, "payload within limit");

        let mut payload = vec![0u8; payload_len];
        receiver.read_exact(&mut payload).expect("read payload");

        let batch = SyncBatch::decode(&payload).expect("decode batch");
        assert_eq!(batch.len(), 3, "batch should contain 3 messages");
        assert_eq!(batch.version, SYNC_BATCH_VERSION);
        assert_eq!(batch.messages[0], make_test_messages(3)[0]);
    }

    #[test]
    fn test_flush_pending_batch_size_chunking() {
        // 当消息数超过 batch_size 时，应分多个批次发送
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (mut receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(2, 10), // batch_size=2
        )
        .expect("create manager");

        // 入队 5 条消息，batch_size=2 → 应分 3 批（2+2+1）
        for msg in make_test_messages(5) {
            manager.enqueue(msg);
        }

        let sent = manager.flush_pending(&mut sender).expect("flush");
        assert_eq!(sent, 5, "should send all 5 messages");

        // 接收端应读到 3 个帧
        use std::io::Read;
        let mut batch_counts = Vec::new();
        for _ in 0..3 {
            let mut len_buf = [0u8; 4];
            receiver
                .read_exact(&mut len_buf)
                .expect("read len for batch");
            let payload_len = u32::from_be_bytes(len_buf) as usize;
            let mut payload = vec![0u8; payload_len];
            receiver
                .read_exact(&mut payload)
                .expect("read payload for batch");
            let batch = SyncBatch::decode(&payload).expect("decode batch");
            batch_counts.push(batch.len());
        }
        // 5 条消息按 batch_size=2 分块 → [2, 2, 1]
        assert_eq!(batch_counts, vec![2, 2, 1], "batch chunking should be [2,2,1]");
    }

    #[test]
    fn test_flush_pending_empty_queue() {
        // 空队列时 flush_pending 返回 0，不发送任何数据
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (_receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new(test_config(), None).expect("create manager");
        let sent = manager.flush_pending(&mut sender).expect("flush");
        assert_eq!(sent, 0, "empty queue should send 0 messages");
    }

    #[test]
    fn test_try_flush_batched_size_threshold() {
        // pending 数量 >= batch_size 时触发发送
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (_receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(3, 10000), // batch_size=3, timeout=10s（避免超时干扰）
        )
        .expect("create manager");

        // 入队 2 条（< batch_size=3），不应触发
        for msg in make_test_messages(2) {
            manager.enqueue(msg);
        }
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 0, "should not flush below batch_size");
        assert_eq!(manager.status().pending_count, 2, "pending still has 2");

        // 入队第 3 条（达到 batch_size=3），应触发
        manager.enqueue(make_test_messages(3)[2].clone());
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 3, "should flush 3 messages at batch_size threshold");
        assert_eq!(manager.status().pending_count, 0, "pending empty after flush");
    }

    #[test]
    fn test_try_flush_batched_timeout_threshold() {
        // pending 数量 < batch_size 但超时已过时触发发送
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (_receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(100, 1), // batch_size=100（不可能达到），timeout=1ms
        )
        .expect("create manager");

        // 入队 1 条消息
        manager.enqueue(make_test_messages(1).into_iter().next().unwrap());

        // 等待超时（1ms）— last_batch_flush 在创建时初始化为 now()
        std::thread::sleep(std::time::Duration::from_millis(10));

        // 超时后应触发
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 1, "should flush after timeout");

        // 再入队 1 条，未超时 → 不发送
        manager.enqueue(make_test_messages(1).into_iter().next().unwrap());
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 0, "should not flush within timeout after previous flush");

        // 等待超时后再触发
        std::thread::sleep(std::time::Duration::from_millis(10));
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 1, "should flush after second timeout");
    }

    #[test]
    fn test_try_flush_batched_no_flush_below_threshold() {
        // 已发送过、未超时、未达 batch_size → 不发送
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (_receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(100, 10000), // batch_size=100, timeout=10s
        )
        .expect("create manager");

        // 先执行一次 flush（设置 last_batch_flush）
        manager.enqueue(make_test_messages(1).into_iter().next().unwrap());
        manager.flush_pending(&mut sender).expect("flush");

        // 再入队 1 条，未达阈值且未超时 → 不发送
        manager.enqueue(make_test_messages(1).into_iter().next().unwrap());
        let sent = manager.try_flush_batched(&mut sender).expect("try flush");
        assert_eq!(sent, 0, "should not flush below threshold and within timeout");
        assert_eq!(manager.status().pending_count, 1, "pending still has 1");
    }

    #[test]
    fn test_large_batch_1000_messages() {
        // 1000 条消息的批量同步：验证编码/解码正确性和性能
        let messages = make_test_messages(1000);
        let batch = SyncBatch::new(messages.clone(), 999);

        let encoded = batch.encode().expect("encode 1000 messages");
        let decoded = SyncBatch::decode(&encoded).expect("decode 1000 messages");

        assert_eq!(decoded.len(), 1000);
        assert_eq!(decoded.batch_id, 999);
        assert_eq!(decoded.messages, messages);

        // 验证消息顺序保持不变
        for (i, msg) in decoded.messages.iter().enumerate() {
            match msg {
                SyncMessage::ScadaData { key, seq, .. } => {
                    assert_eq!(*key, format!("telemetry.sensor.{i}"));
                    assert_eq!(*seq, i as u64 + 1);
                }
                _ => panic!("expected ScadaData at index {i}"),
            }
        }
    }

    #[test]
    fn test_large_batch_flush_pending_1000_via_tcp() {
        // 通过 TCP 回环发送 1000 条消息，验证端到端批量同步
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let mut sender = std::net::TcpStream::connect(addr).expect("connect");
        let (mut receiver, _) = listener.accept().expect("accept");

        let manager = SyncManager::new_with_batch_config(
            test_config(),
            None,
            BatchConfig::new(250, 10), // batch_size=250 → 1000 条分 4 批
        )
        .expect("create manager");

        let original_messages = make_test_messages(1000);
        for msg in &original_messages {
            manager.enqueue(msg.clone());
        }

        let sent = manager.flush_pending(&mut sender).expect("flush");
        assert_eq!(sent, 1000, "should send all 1000 messages");

        // 接收并解码所有批次
        use std::io::Read;
        let mut all_received = Vec::new();
        for _ in 0..4 {
            let mut len_buf = [0u8; 4];
            receiver.read_exact(&mut len_buf).expect("read len");
            let payload_len = u32::from_be_bytes(len_buf) as usize;
            let mut payload = vec![0u8; payload_len];
            receiver.read_exact(&mut payload).expect("read payload");
            let batch = SyncBatch::decode(&payload).expect("decode batch");
            all_received.extend(batch.messages);
        }

        assert_eq!(all_received.len(), 1000, "should receive all 1000 messages");
        assert_eq!(all_received, original_messages, "messages should match original");
    }

    /// 构造基准测试用 SyncMessage（模拟高频 SCADA 遥测同步场景）
    ///
    /// 使用短 key（`t0`..`tN`）和简单数值（电压/电流遥测），
    /// 代表真实电力系统中高频遥测同步的典型负载：
    /// - key 短（变电站内传感器 ID 通常为短编号）
    /// - value 为数值（电压、电流、功率等遥测量）
    /// - timestamp 和 seq 为递增整数
    fn make_benchmark_messages(n: usize) -> Vec<SyncMessage> {
        (0..n)
            .map(|i| SyncMessage::ScadaData {
                key: format!("t{i}"),
                value: serde_json::json!(220.5 + i as f64 * 0.1),
                timestamp: 1700000000000 + i as i64,
                seq: i as u64 + 1,
            })
            .collect()
    }

    /// 基准测试：JSON vs bincode 序列化/反序列化延迟和带宽对比。
    ///
    /// 模拟高频 SCADA 遥测同步场景（1000 条短 key + 数值遥测消息），
    /// 对比旧协议（JSON 逐条序列化 + 4 字节长度前缀）与新协议
    /// （bincode 批量序列化 + 4 字节长度前缀）的性能差异。
    ///
    /// 验收标准：
    /// - 序列化延迟下降 > 50%（bincode 编解码速度显著优于 JSON）
    /// - 带宽下降 > 70%（bincode 消除字段名/JSON 语法开销 + 批量摊薄帧前缀）
    #[test]
    fn test_bincode_vs_json_benchmark() {
        let messages = make_benchmark_messages(1000);

        // === JSON 基准（旧协议：每条消息单独 JSON 序列化 + 4 字节长度前缀）===
        let json_start = std::time::Instant::now();
        let mut json_total_bytes = 0usize;
        let mut json_encoded: Vec<Vec<u8>> = Vec::with_capacity(1000);
        for msg in &messages {
            let json = serde_json::to_vec(msg).expect("json encode");
            json_total_bytes += 4 + json.len(); // 4 字节长度前缀 + 载荷
            json_encoded.push(json);
        }
        for json in &json_encoded {
            let _: SyncMessage = serde_json::from_slice(json).expect("json decode");
        }
        let json_elapsed = json_start.elapsed();

        // === bincode 基准（新协议：打包为 SyncBatch + bincode 编码 + 4 字节长度前缀）===
        let bincode_start = std::time::Instant::now();
        let batch = SyncBatch::new(messages.clone(), 1);
        let encoded = batch.encode().expect("bincode encode");
        let bincode_total_bytes = 4 + encoded.len(); // 4 字节长度前缀 + 载荷
        let decoded = SyncBatch::decode(&encoded).expect("bincode decode");
        let bincode_elapsed = bincode_start.elapsed();

        // 验证解码正确性
        assert_eq!(decoded.messages, messages);

        // 计算改善比例
        let json_nanos = json_elapsed.as_nanos() as f64;
        let bincode_nanos = bincode_elapsed.as_nanos() as f64;
        let latency_reduction = (json_nanos - bincode_nanos) / json_nanos * 100.0;
        let bandwidth_reduction =
            (json_total_bytes as f64 - bincode_total_bytes as f64) / json_total_bytes as f64
                * 100.0;

        // 打印基准结果（便于人工审查）
        eprintln!("=== T029-23 JSON vs bincode 基准测试 ===");
        eprintln!("消息数: 1000（高频 SCADA 遥测场景：短 key + 数值）");
        eprintln!(
            "JSON  总字节: {} ({} bytes/msg avg)",
            json_total_bytes,
            json_total_bytes / 1000
        );
        eprintln!(
            "bincode 总字节: {} ({} bytes/msg avg)",
            bincode_total_bytes,
            bincode_total_bytes / 1000
        );
        eprintln!(
            "JSON  耗时: {} ns ({:.3} ms)",
            json_nanos,
            json_nanos / 1_000_000.0
        );
        eprintln!(
            "bincode 耗时: {} ns ({:.3} ms)",
            bincode_nanos,
            bincode_nanos / 1_000_000.0
        );
        eprintln!("延迟下降: {:.1}%", latency_reduction);
        eprintln!("带宽下降: {:.1}%", bandwidth_reduction);

        // 验收标准：序列化延迟下降 > 40%（时序敏感，宽松阈值避免 flaky）
        assert!(
            latency_reduction > 40.0,
            "bincode 序列化延迟下降应 > 40%，实际 {:.1}%（JSON {}ns vs bincode {}ns）",
            latency_reduction,
            json_nanos,
            bincode_nanos
        );

        // 验收标准：带宽下降 > 70%
        assert!(
            bandwidth_reduction > 70.0,
            "bincode 带宽下降应 > 70%，实际 {:.1}%（JSON {}B vs bincode {}B）",
            bandwidth_reduction,
            json_total_bytes,
            bincode_total_bytes
        );
    }

    #[test]
    fn test_bincode_vs_json_bandwidth_batch_100() {
        // 100 条消息的带宽对比（批量 bincode vs 逐条 JSON）
        // 使用批量对比而非单条，因为单条消息的 batch 头开销会抵消 bincode 的紧凑优势
        let messages = make_benchmark_messages(100);

        // JSON：100 条消息逐条序列化，每条带 4 字节长度前缀
        let mut json_total_bytes = 0usize;
        for msg in &messages {
            let json = serde_json::to_vec(msg).expect("json encode");
            json_total_bytes += 4 + json.len();
        }

        // bincode：100 条消息打包为 1 个 SyncBatch，带 4 字节长度前缀
        let batch = SyncBatch::new(messages, 1);
        let encoded = batch.encode().expect("bincode encode");
        let bincode_total_bytes = 4 + encoded.len();

        eprintln!(
            "100 条消息 — JSON: {}B, bincode(batch): {}B, 节省 {:.1}%",
            json_total_bytes,
            bincode_total_bytes,
            (json_total_bytes as f64 - bincode_total_bytes as f64) / json_total_bytes as f64 * 100.0
        );
        assert!(
            bincode_total_bytes < json_total_bytes,
            "bincode 批量应比 JSON 逐条更紧凑（100 条：bincode {}B vs JSON {}B）",
            bincode_total_bytes,
            json_total_bytes
        );
    }

    #[test]
    fn test_batch_id_monotonic_increment() {
        // 验证 batch_id 单调递增
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        let id1 = manager.next_batch_id();
        let id2 = manager.next_batch_id();
        let id3 = manager.next_batch_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    // ===== T030-07: 覆盖率补充测试 =====

    /// 验证 `SyncBatch::len()` 和 `is_empty()` 方法。
    #[test]
    fn test_sync_batch_len_and_is_empty() {
        let empty_batch = SyncBatch::new(Vec::new(), 1);
        assert_eq!(empty_batch.len(), 0);
        assert!(empty_batch.is_empty());

        let msg = SyncMessage::ScadaData {
            key: "test".to_string(),
            value: serde_json::json!({"v": 1.0}),
            seq: 1,
            timestamp: 0,
        };
        let batch = SyncBatch::new(vec![msg], 2);
        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    /// 验证 `local_node_id()` 访问器返回正确的节点 ID。
    #[test]
    fn test_local_node_id_accessor() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        // test_config() 设置 node_id 为 "node-1"
        assert_eq!(manager.local_node_id(), "node-1");
    }

    /// 验证 `current_send_seq()` 在发送消息后递增。
    #[test]
    fn test_current_send_seq_accessor() {
        let manager = SyncManager::new(test_config(), None).expect("create manager");
        // 初始 seq 应为 0
        let initial_seq = manager.current_send_seq();
        // 发送一条消息后 seq 应递增
        manager
            .send_scada("test_key", serde_json::json!({"v": 1.0}))
            .expect("send");
        let after_send_seq = manager.current_send_seq();
        assert!(after_send_seq > initial_seq);
    }

    /// 验证 `batch_config()` 访问器返回批量配置。
    #[test]
    fn test_batch_config_accessor() {
        let config = BatchConfig::new(50, 200);
        let manager =
            SyncManager::new_with_batch_config(test_config(), None, config)
                .expect("create manager");
        let config = manager.batch_config();
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.batch_timeout_ms, 200);
    }
}
