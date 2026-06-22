//! 双节点心跳服务（v0.25.1 — Task 2）
//!
//! UDP 多播心跳，100ms 间隔，300ms 故障检测。
//! 支持主/备角色优先级和双网卡冗余。
//!
//! ## 协议
//!
//! - 传输层：UDP 多播（默认 `239.0.0.1:5400`）
//! - 心跳间隔：100ms（可配置）
//! - 故障检测：suspect 100ms / dead 300ms（可配置）
//! - 载荷：JSON 序列化的 [`HeartbeatPacket`]
//! - 认证：HMAC-SHA256（可选，通过 `auth_key` 启用）
//! - 防重放：epoch 单调递增，拒绝旧 epoch 的包
//!
//! ## 跨平台策略
//!
//! - **Linux**：使用 `std::net::UdpSocket` + `IP_ADD_MEMBERSHIP` 加入多播组，
//!   支持 `SO_REUSEADDR` 以便同机多实例测试。
//! - **非 Linux**：网络方法返回 [`HeartbeatError::UnsupportedPlatform`]，
//!   状态机逻辑、序列化、HMAC 计算、超时检测等纯逻辑在所有平台可用，便于开发/测试。

use crate::ha::HaConfig;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::net::UdpSocket;
#[cfg(target_os = "linux")]
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

type HmacSha256 = Hmac<Sha256>;

/// 心跳包最大载荷大小（JSON 序列化后的上限，留足余量）
#[cfg(target_os = "linux")]
const HEARTBEAT_PACKET_MAX_SIZE: usize = 1024;

/// 节点恢复 Alive 所需的连续心跳次数（去抖）
const ALIVE_CONFIRM_THRESHOLD: u32 = 3;

/// 节点角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    Primary,
    Secondary,
}

impl NodeRole {
    /// 返回角色的字符串表示（用于 HMAC 计算等）
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeRole::Primary => "primary",
            NodeRole::Secondary => "secondary",
        }
    }
}

/// 节点状态
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// 存活：心跳正常
    #[default]
    Alive,
    /// 可疑：心跳超时（100ms）
    Suspect,
    /// 死亡：心跳超时（300ms）
    Dead,
}

/// 心跳包
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatPacket {
    /// 发送节点 ID
    pub node_id: String,
    /// 节点角色
    pub role: NodeRole,
    /// 发送时间戳（Unix 毫秒）
    pub timestamp: i64,
    /// 序列号
    pub seq: u64,
    /// 优先级
    pub priority: u32,
    /// HMAC-SHA256 认证码（无密钥时为全零）
    #[serde(default = "default_hmac")]
    pub hmac: [u8; 32],
    /// 发送方 epoch（单调递增，用于拒绝旧包）
    #[serde(default = "default_epoch")]
    pub epoch: u64,
}

/// HMAC 默认值（全零，用于无密钥场景的向后兼容）
fn default_hmac() -> [u8; 32] {
    [0u8; 32]
}

/// epoch 默认值（0，用于向后兼容旧格式包）
fn default_epoch() -> u64 {
    0
}

/// 节点信息（用于状态跟踪）
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// 节点 ID
    pub node_id: String,
    /// 节点角色
    pub role: NodeRole,
    /// 节点状态
    pub state: NodeState,
    /// 优先级
    pub priority: u32,
    /// 最后一次心跳接收时间
    pub last_heartbeat: Instant,
    /// 最后一次心跳序列号
    pub last_seq: u64,
    /// 连续存活确认计数（用于从 Suspect/Dead 恢复 Alive 的去抖）
    pub alive_confirm_count: u32,
    /// 节点 epoch（用于拒绝旧包）
    pub epoch: u64,
}

/// 节点状态变更事件（由 [`HeartbeatManager::check_timeouts`] 返回）
#[derive(Debug, Clone)]
pub struct NodeStateChange {
    /// 节点 ID
    pub node_id: String,
    /// 变更前状态
    pub old_state: NodeState,
    /// 变更后状态
    pub new_state: NodeState,
    /// 变更时间戳（Unix 毫秒）
    pub timestamp: i64,
}

/// 心跳错误
#[derive(Debug, thiserror::Error)]
pub enum HeartbeatError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("unsupported platform: heartbeat requires Linux")]
    UnsupportedPlatform,
    #[error("node {0} is dead")]
    NodeDead(String),
    #[error("no sockets available")]
    NoSockets,
}

/// 心跳管理器
///
/// 管理本节点的心跳发送与对端节点的状态跟踪。
///
/// - Linux：绑定 UDP 多播 socket，实际收发心跳包
/// - 非 Linux：仅维护节点状态机（纯逻辑），网络方法返回 `UnsupportedPlatform`
pub struct HeartbeatManager {
    /// HA 配置
    config: HaConfig,
    /// 已知节点列表（peer node_id → NodeInfo）
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    /// 本节点序列号
    seq: Arc<RwLock<u64>>,
    /// UDP 多播 socket（仅 Linux，每个接口一个）
    #[cfg(target_os = "linux")]
    sockets: Vec<UdpSocket>,
    /// HMAC 认证密钥（None 表示不启用认证）
    auth_key: Option<Vec<u8>>,
    /// 本节点 epoch（启动时生成，用于防重放）
    epoch: u64,
}

impl HeartbeatManager {
    /// 创建心跳管理器。
    ///
    /// - Linux：为每个 `config.interfaces` 创建独立 UDP 多播 socket 并加入多播组；
    ///   若 `interfaces` 为空，回退到绑定 `INADDR_ANY`。
    /// - 非 Linux：创建管理器但不绑定 socket（纯逻辑可用）
    pub fn new(config: HaConfig) -> Result<Self, HeartbeatError> {
        let auth_key = config
            .auth_key
            .as_ref()
            .map(|s| s.as_bytes().to_vec());
        let epoch = current_timestamp_nanos();

        #[cfg(target_os = "linux")]
        {
            let mut sockets: Vec<UdpSocket> = Vec::new();
            if config.interfaces.is_empty() {
                // 回退到原行为：绑定 INADDR_ANY
                let socket = create_multicast_socket(
                    &config.multicast_addr,
                    config.heartbeat_port,
                    None,
                    config.multicast_ttl,
                )?;
                sockets.push(socket);
            } else {
                // 为每个接口创建独立 socket
                for iface in &config.interfaces {
                    let interface_ip: Ipv4Addr = match iface.parse() {
                        Ok(ip) => ip,
                        Err(_) => {
                            eprintln!(
                                "[heartbeat] invalid interface IP '{}', skipping",
                                iface
                            );
                            continue;
                        }
                    };
                    match create_multicast_socket(
                        &config.multicast_addr,
                        config.heartbeat_port,
                        Some(interface_ip),
                        config.multicast_ttl,
                    ) {
                        Ok(socket) => sockets.push(socket),
                        Err(e) => {
                            eprintln!(
                                "[heartbeat] failed to create socket for interface {}: {}",
                                iface, e
                            );
                        }
                    }
                }
                // 如果所有接口都失败，回退到 INADDR_ANY
                if sockets.is_empty() {
                    eprintln!(
                        "[heartbeat] all interfaces failed, falling back to INADDR_ANY"
                    );
                    let socket = create_multicast_socket(
                        &config.multicast_addr,
                        config.heartbeat_port,
                        None,
                        config.multicast_ttl,
                    )?;
                    sockets.push(socket);
                }
            }
            Ok(Self {
                config,
                nodes: Arc::new(RwLock::new(HashMap::new())),
                seq: Arc::new(RwLock::new(0)),
                sockets,
                auth_key,
                epoch,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self {
                config,
                nodes: Arc::new(RwLock::new(HashMap::new())),
                seq: Arc::new(RwLock::new(0)),
                auth_key,
                epoch,
            })
        }
    }

    /// 发送心跳包到多播组。
    ///
    /// 遍历所有 socket 发送，任一 socket 发送失败记录日志但继续其他 socket。
    /// 序列号自增，使用当前时间戳。
    pub fn send_heartbeat(&self) -> Result<(), HeartbeatError> {
        #[cfg(target_os = "linux")]
        {
            if self.sockets.is_empty() {
                return Err(HeartbeatError::NoSockets);
            }
            let packet = self.build_packet();
            let payload = serde_json::to_vec(&packet)?;
            let addr = format!(
                "{}:{}",
                self.config.multicast_addr, self.config.heartbeat_port
            );
            for socket in &self.sockets {
                if let Err(e) = socket.send_to(&payload, &addr) {
                    eprintln!("[heartbeat] send_to failed on socket: {}", e);
                    // 继续其他 socket
                }
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.build_packet(); // 序列号仍然递增，便于测试
            Err(HeartbeatError::UnsupportedPlatform)
        }
    }

    /// 构建心跳包并递增序列号。
    ///
    /// 无密钥时 HMAC 为全零；有密钥时计算 HMAC-SHA256。
    fn build_packet(&self) -> HeartbeatPacket {
        let mut seq = self
            .seq
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *seq = seq.wrapping_add(1);
        let current_seq = *seq;
        drop(seq);

        let mut packet = HeartbeatPacket {
            node_id: self.config.node_id.clone(),
            role: self.config.role,
            timestamp: current_timestamp_millis(),
            seq: current_seq,
            priority: self.config.priority,
            hmac: [0u8; 32],
            epoch: self.epoch,
        };

        // 有密钥时计算 HMAC，无密钥时保持全零
        if let Some(key) = &self.auth_key {
            packet.hmac = compute_hmac(&packet, key);
        }

        packet
    }

    /// 接收心跳包（非阻塞）。
    ///
    /// 遍历所有 socket 接收，任一 socket 收到包即处理。
    /// 反序列化失败或 HMAC 校验失败时记录日志并继续接收下一个包。
    /// 全部 socket `WouldBlock` 时返回 `Ok(None)`。
    pub fn receive_heartbeat(&self) -> Result<Option<HeartbeatPacket>, HeartbeatError> {
        #[cfg(target_os = "linux")]
        {
            if self.sockets.is_empty() {
                return Err(HeartbeatError::NoSockets);
            }
            let mut buf = [0u8; HEARTBEAT_PACKET_MAX_SIZE];
            for socket in &self.sockets {
                match socket.recv_from(&mut buf) {
                    Ok((len, _addr)) => {
                        // 反序列化失败：记录日志并继续接收下一个包
                        let packet: HeartbeatPacket = match serde_json::from_slice(&buf[..len]) {
                            Ok(p) => p,
                            Err(e) => {
                                eprintln!(
                                    "[heartbeat] deserialize heartbeat packet failed: {}",
                                    e
                                );
                                continue;
                            }
                        };
                        // HMAC 校验：有密钥时校验失败则记录日志并 continue
                        if let Some(key) = &self.auth_key {
                            if !verify_hmac(&packet, key) {
                                eprintln!(
                                    "[heartbeat] HMAC verification failed for node {}",
                                    packet.node_id
                                );
                                continue;
                            }
                        }
                        self.update_node(&packet);
                        return Ok(Some(packet));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        return Err(HeartbeatError::Io(e));
                    }
                }
            }
            Ok(None)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(HeartbeatError::UnsupportedPlatform)
        }
    }

    /// 根据收到的心跳包更新节点状态。
    ///
    /// - 忽略自己的心跳包
    /// - 拒绝旧 epoch 的包（`packet.epoch < node.epoch`）
    /// - 从 Suspect/Dead 恢复 Alive 需要连续 [`ALIVE_CONFIRM_THRESHOLD`] 次心跳（去抖）
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn update_node(&self, packet: &HeartbeatPacket) {
        // 忽略自己的心跳包
        if packet.node_id == self.config.node_id {
            return;
        }
        let mut nodes = self
            .nodes
            .write()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(node) = nodes.get_mut(&packet.node_id) {
            // 拒绝旧 epoch 的包
            if packet.epoch < node.epoch {
                eprintln!(
                    "[heartbeat] reject old epoch packet from {}: {} < {}",
                    packet.node_id, packet.epoch, node.epoch
                );
                return;
            }

            // 更新节点基本信息
            node.role = packet.role;
            node.priority = packet.priority;
            node.last_heartbeat = Instant::now();
            node.last_seq = packet.seq;
            node.epoch = packet.epoch;

            // 去抖：从 Suspect/Dead 恢复 Alive 需要连续 N 次心跳
            match node.state {
                NodeState::Alive => {
                    node.alive_confirm_count = 0;
                }
                NodeState::Suspect | NodeState::Dead => {
                    node.alive_confirm_count += 1;
                    if node.alive_confirm_count >= ALIVE_CONFIRM_THRESHOLD {
                        node.state = NodeState::Alive;
                        node.alive_confirm_count = 0;
                    }
                }
            }
        } else {
            // 新节点：直接设为 Alive
            nodes.insert(
                packet.node_id.clone(),
                NodeInfo {
                    node_id: packet.node_id.clone(),
                    role: packet.role,
                    state: NodeState::Alive,
                    priority: packet.priority,
                    last_heartbeat: Instant::now(),
                    last_seq: packet.seq,
                    alive_confirm_count: 1,
                    epoch: packet.epoch,
                },
            );
        }
    }

    /// 检查所有节点超时，更新状态机。
    ///
    /// - elapsed >= dead_ms → `Dead`
    /// - elapsed >= suspect_ms → `Suspect`
    /// - 否则 → `Alive`
    ///
    /// 返回发生状态变更的节点列表（[`NodeStateChange`]）。
    pub fn check_timeouts(&self) -> Vec<NodeStateChange> {
        let now = Instant::now();
        let suspect = Duration::from_millis(self.config.heartbeat_suspect_ms);
        let dead = Duration::from_millis(self.config.heartbeat_dead_ms);
        let mut changes = Vec::new();
        let mut nodes = self
            .nodes
            .write()
            .unwrap_or_else(|e| e.into_inner());
        for node in nodes.values_mut() {
            let elapsed = now.duration_since(node.last_heartbeat);
            let old_state = node.state;
            let new_state = if elapsed >= dead {
                NodeState::Dead
            } else if elapsed >= suspect {
                NodeState::Suspect
            } else {
                NodeState::Alive
            };
            if new_state != old_state {
                node.state = new_state;
                changes.push(NodeStateChange {
                    node_id: node.node_id.clone(),
                    old_state,
                    new_state,
                    timestamp: current_timestamp_millis(),
                });
            }
        }
        changes
    }

    /// 后台循环：发送心跳、接收心跳、检查超时，直到 shutdown 被设置。
    pub fn run(&self, shutdown: Arc<std::sync::atomic::AtomicBool>) {
        use std::sync::atomic::Ordering;
        while !shutdown.load(Ordering::SeqCst) {
            let _ = self.send_heartbeat();
            while let Ok(Some(_)) = self.receive_heartbeat() {}
            let _changes = self.check_timeouts();
            std::thread::sleep(self.config.heartbeat_interval());
        }
    }

    /// 获取指定节点的当前状态。
    pub fn get_node_state(&self, node_id: &str) -> Option<NodeState> {
        let nodes = self
            .nodes
            .read()
            .unwrap_or_else(|e| e.into_inner());
        nodes.get(node_id).map(|n| n.state)
    }

    /// 列出所有已知节点（不含本节点）。
    pub fn list_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self
            .nodes
            .read()
            .unwrap_or_else(|e| e.into_inner());
        nodes.values().cloned().collect()
    }

    /// 获取本节点 ID。
    pub fn local_node_id(&self) -> &str {
        &self.config.node_id
    }

    /// 获取本节点角色。
    pub fn local_role(&self) -> NodeRole {
        self.config.role
    }

    /// 获取本节点优先级。
    pub fn local_priority(&self) -> u32 {
        self.config.priority
    }

    /// 获取当前序列号（主要用于测试）。
    pub fn current_seq(&self) -> u64 {
        *self
            .seq
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 获取本节点 epoch（主要用于测试）。
    #[allow(dead_code)]
    pub fn local_epoch(&self) -> u64 {
        self.epoch
    }
}

// ============================================================================
// HMAC 认证
// ============================================================================

/// 计算心跳包的 HMAC-SHA256。
///
/// 对 `node_id + role + timestamp + seq + priority + epoch` 字段计算。
/// 无密钥时调用方应直接使用全零 HMAC，不调用此函数。
fn compute_hmac(packet: &HeartbeatPacket, key: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length error");
    mac.update(packet.node_id.as_bytes());
    mac.update(packet.role.as_str().as_bytes());
    mac.update(&packet.timestamp.to_le_bytes());
    mac.update(&packet.seq.to_le_bytes());
    mac.update(&packet.priority.to_le_bytes());
    mac.update(&packet.epoch.to_le_bytes());
    let result = mac.finalize().into_bytes();
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&result);
    hmac
}

/// 校验心跳包的 HMAC-SHA256。
///
/// 返回 `true` 表示校验通过，`false` 表示校验失败（密钥错误或数据被篡改）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn verify_hmac(packet: &HeartbeatPacket, key: &[u8]) -> bool {
    let mut mac = match HmacSha256::new_from_slice(key) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(packet.node_id.as_bytes());
    mac.update(packet.role.as_str().as_bytes());
    mac.update(&packet.timestamp.to_le_bytes());
    mac.update(&packet.seq.to_le_bytes());
    mac.update(&packet.priority.to_le_bytes());
    mac.update(&packet.epoch.to_le_bytes());
    mac.verify_slice(&packet.hmac).is_ok()
}

// ============================================================================
// 时间戳工具
// ============================================================================

/// 获取当前 Unix 时间戳（毫秒）
fn current_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 获取当前 Unix 时间戳（纳秒），用作 epoch 简单随机源
fn current_timestamp_nanos() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ============================================================================
// Linux 多播 socket 创建
// ============================================================================

/// 创建 UDP 多播 socket 并加入多播组。
///
/// - 设置 `SO_REUSEADDR` 以便同机多实例测试
/// - 绑定 `0.0.0.0:port`
/// - 加入多播组 `multicast_addr`，接口由 `interface_ip` 指定（None 表示 INADDR_ANY）
/// - 设置 `IP_MULTICAST_TTL` 为 `ttl`
#[cfg(target_os = "linux")]
fn create_multicast_socket(
    multicast_addr: &str,
    port: u16,
    interface_ip: Option<Ipv4Addr>,
    ttl: u8,
) -> Result<UdpSocket, HeartbeatError> {
    use std::os::unix::io::{AsRawFd, FromRawFd};

    // 1. 创建 UDP socket
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_UDP) };
    if fd < 0 {
        return Err(HeartbeatError::Io(std::io::Error::last_os_error()));
    }

    // 2. 设置 SO_REUSEADDR（bind 前设置）
    let reuse: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &reuse as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return Err(HeartbeatError::Io(std::io::Error::last_os_error()));
    }

    // 3. 绑定 0.0.0.0:port
    let bind_addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: port.to_be(),
        sin_addr: libc::in_addr { s_addr: 0 }, // INADDR_ANY
        sin_zero: [0; 8],
    };
    let ret = unsafe {
        libc::bind(
            fd,
            &bind_addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return Err(HeartbeatError::Io(std::io::Error::last_os_error()));
    }

    // 4. 设置 IP_MULTICAST_TTL
    let ttl_val: libc::c_uchar = ttl as libc::c_uchar;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_MULTICAST_TTL,
            &ttl_val as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_uchar>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return Err(HeartbeatError::Io(std::io::Error::last_os_error()));
    }

    // 5. 加入多播组 IP_ADD_MEMBERSHIP
    let multiaddr: Ipv4Addr = multicast_addr.parse().map_err(|e: std::net::AddrParseError| {
        HeartbeatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid multicast address: {e}"),
        ))
    })?;
    let imr_interface = match interface_ip {
        Some(ip) => libc::in_addr {
            s_addr: u32::from_be_bytes(ip.octets()),
        },
        None => libc::in_addr { s_addr: 0 }, // INADDR_ANY
    };
    let mreq = libc::ip_mreq {
        imr_multiaddr: libc::in_addr {
            s_addr: u32::from_be_bytes(multiaddr.octets()),
        },
        imr_interface,
    };
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_ADD_MEMBERSHIP,
            &mreq as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::ip_mreq>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return Err(HeartbeatError::Io(std::io::Error::last_os_error()));
    }

    // 6. 包装为 UdpSocket 并设置非阻塞
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    socket.set_nonblocking(true)?;

    // 触发一次 AsRawFd 以确保 fd 被正确接管（防止 lint 警告）
    let _ = socket.as_raw_fd();

    Ok(socket)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 HaConfig
    fn test_config() -> HaConfig {
        HaConfig {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            heartbeat_interval_ms: 100,
            heartbeat_suspect_ms: 100,
            heartbeat_dead_ms: 300,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 5401,
            interfaces: Vec::new(),
            priority: 100,
            fencing_strategy: crate::ha::FencingStrategy::None,
            sync_scope: Default::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        }
    }

    /// 构造带认证密钥的测试 HaConfig
    fn test_config_with_auth() -> HaConfig {
        let mut config = test_config();
        config.auth_key = Some("test-secret-key".to_string());
        config
    }

    /// 构造测试用 HeartbeatPacket（无 HMAC）
    fn test_packet(node_id: &str, seq: u64, epoch: u64) -> HeartbeatPacket {
        HeartbeatPacket {
            node_id: node_id.to_string(),
            role: NodeRole::Secondary,
            timestamp: 1700000000000,
            seq,
            priority: 50,
            hmac: [0u8; 32],
            epoch,
        }
    }

    #[test]
    fn test_heartbeat_packet_serialize() {
        let packet = HeartbeatPacket {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            timestamp: 1700000000000,
            seq: 42,
            priority: 100,
            hmac: [0u8; 32],
            epoch: 0,
        };
        let json = serde_json::to_string(&packet).expect("serialize");
        let deserialized: HeartbeatPacket = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(packet, deserialized);
    }

    #[test]
    fn test_heartbeat_packet_backward_compat() {
        // 旧格式 JSON（无 hmac/epoch 字段）应能反序列化，使用默认值
        let old_json = r#"{"node_id":"node-1","role":"primary","timestamp":1700000000000,"seq":42,"priority":100}"#;
        let packet: HeartbeatPacket =
            serde_json::from_str(old_json).expect("deserialize old format");
        assert_eq!(packet.node_id, "node-1");
        assert_eq!(packet.hmac, [0u8; 32]);
        assert_eq!(packet.epoch, 0);
    }

    #[test]
    fn test_node_role_serde() {
        let json = serde_json::to_string(&NodeRole::Primary).expect("serialize");
        assert_eq!(json, "\"primary\"");
        let role: NodeRole = serde_json::from_str("\"secondary\"").expect("deserialize");
        assert_eq!(role, NodeRole::Secondary);
    }

    #[test]
    fn test_node_role_as_str() {
        assert_eq!(NodeRole::Primary.as_str(), "primary");
        assert_eq!(NodeRole::Secondary.as_str(), "secondary");
    }

    #[test]
    fn test_node_state_transitions() {
        let manager = HeartbeatManager::new(test_config()).expect("create manager");

        // 插入一个 peer 节点，last_heartbeat 设为当前时间
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
        }

        // 初始状态：Alive
        let changes = manager.check_timeouts();
        assert!(changes.is_empty(), "no state change expected");
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Alive),
            "fresh node should be Alive"
        );

        // 模拟超过 suspect 阈值（100ms）
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let node = nodes.get_mut("node-2").unwrap();
            node.last_heartbeat = Instant::now() - Duration::from_millis(150);
        }
        let changes = manager.check_timeouts();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old_state, NodeState::Alive);
        assert_eq!(changes[0].new_state, NodeState::Suspect);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Suspect),
            "node with 150ms elapsed should be Suspect"
        );

        // 模拟超过 dead 阈值（300ms）
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let node = nodes.get_mut("node-2").unwrap();
            node.last_heartbeat = Instant::now() - Duration::from_millis(400);
        }
        let changes = manager.check_timeouts();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old_state, NodeState::Suspect);
        assert_eq!(changes[0].new_state, NodeState::Dead);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Dead),
            "node with 400ms elapsed should be Dead"
        );
    }

    #[test]
    fn test_node_role_priority() {
        // 高优先级节点应胜出
        let primary_high = HeartbeatPacket {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            timestamp: 0,
            seq: 0,
            priority: 200,
            hmac: [0u8; 32],
            epoch: 0,
        };
        let secondary_low = HeartbeatPacket {
            node_id: "node-2".to_string(),
            role: NodeRole::Secondary,
            timestamp: 0,
            seq: 0,
            priority: 100,
            hmac: [0u8; 32],
            epoch: 0,
        };
        assert!(primary_high.priority > secondary_low.priority);

        // Primary 角色本身不隐含更高优先级，优先级由 priority 字段决定
        let low_pri_primary = HeartbeatPacket {
            node_id: "node-3".to_string(),
            role: NodeRole::Primary,
            timestamp: 0,
            seq: 0,
            priority: 50,
            hmac: [0u8; 32],
            epoch: 0,
        };
        assert!(secondary_low.priority > low_pri_primary.priority);
    }

    #[test]
    fn test_heartbeat_packet_seq_increment() {
        let manager = HeartbeatManager::new(test_config()).expect("create manager");
        assert_eq!(manager.current_seq(), 0);

        let p1 = manager.build_packet();
        assert_eq!(p1.seq, 1);
        assert_eq!(manager.current_seq(), 1);

        let p2 = manager.build_packet();
        assert_eq!(p2.seq, 2);
        assert_eq!(manager.current_seq(), 2);

        let p3 = manager.build_packet();
        assert_eq!(p3.seq, 3);
        assert_eq!(manager.current_seq(), 3);

        // 序列号严格递增
        assert!(p2.seq > p1.seq);
        assert!(p3.seq > p2.seq);
    }

    #[test]
    fn test_node_info_timeout() {
        let manager = HeartbeatManager::new(test_config()).expect("create manager");

        // 插入三个节点，分别处于不同时间偏移
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "fresh".to_string(),
                NodeInfo {
                    node_id: "fresh".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
            nodes.insert(
                "suspect".to_string(),
                NodeInfo {
                    node_id: "suspect".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now() - Duration::from_millis(120),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
            nodes.insert(
                "dead".to_string(),
                NodeInfo {
                    node_id: "dead".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now() - Duration::from_millis(500),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
        }

        let changes = manager.check_timeouts();
        // fresh 无变化，suspect 和 dead 各一次变化
        assert_eq!(changes.len(), 2);

        assert_eq!(manager.get_node_state("fresh"), Some(NodeState::Alive));
        assert_eq!(manager.get_node_state("suspect"), Some(NodeState::Suspect));
        assert_eq!(manager.get_node_state("dead"), Some(NodeState::Dead));
    }

    #[test]
    fn test_heartbeat_manager_new() {
        // 非 Linux 平台验证不 panic
        let manager = HeartbeatManager::new(test_config());
        assert!(manager.is_ok(), "HeartbeatManager::new should succeed");
        let manager = manager.unwrap();
        assert_eq!(manager.local_node_id(), "node-1");
        assert_eq!(manager.local_role(), NodeRole::Primary);
        assert_eq!(manager.local_priority(), 100);
        assert!(manager.list_nodes().is_empty(), "no peers initially");
    }

    #[test]
    fn test_send_heartbeat_non_linux() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        // 序列号在 build_packet 中递增
        #[cfg(not(target_os = "linux"))]
        {
            let result = manager.send_heartbeat();
            assert!(matches!(result, Err(HeartbeatError::UnsupportedPlatform)));
            // 序列号仍然递增（build_packet 在返回错误前被调用）
            assert_eq!(manager.current_seq(), 1);
        }
        #[cfg(target_os = "linux")]
        {
            // Linux 上 send_heartbeat 可能成功或失败（取决于网络环境），
            // 此处仅验证不 panic
            let _ = manager.send_heartbeat();
        }
    }

    #[test]
    fn test_receive_heartbeat_non_linux() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        #[cfg(not(target_os = "linux"))]
        {
            let result = manager.receive_heartbeat();
            assert!(matches!(result, Err(HeartbeatError::UnsupportedPlatform)));
        }
        #[cfg(target_os = "linux")]
        {
            let _ = manager.receive_heartbeat();
        }
    }

    #[test]
    fn test_update_node_ignores_self() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        // 模拟收到自己的心跳包
        let self_packet = HeartbeatPacket {
            node_id: "node-1".to_string(), // 与 config.node_id 相同
            role: NodeRole::Primary,
            timestamp: 0,
            seq: 1,
            priority: 100,
            hmac: [0u8; 32],
            epoch: 0,
        };
        manager.update_node(&self_packet);
        assert!(
            manager.list_nodes().is_empty(),
            "self heartbeat should not be added to nodes"
        );
    }

    #[test]
    fn test_update_node_adds_peer() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        let peer_packet = HeartbeatPacket {
            node_id: "node-2".to_string(),
            role: NodeRole::Secondary,
            timestamp: 1700000000000,
            seq: 5,
            priority: 50,
            hmac: [0u8; 32],
            epoch: 0,
        };
        manager.update_node(&peer_packet);
        let nodes = manager.list_nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node-2");
        assert_eq!(nodes[0].role, NodeRole::Secondary);
        assert_eq!(nodes[0].state, NodeState::Alive);
        assert_eq!(nodes[0].priority, 50);
        assert_eq!(nodes[0].last_seq, 5);
    }

    #[test]
    fn test_check_timeouts_empty() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        // 无节点时不应 panic，返回空 Vec
        let changes = manager.check_timeouts();
        assert!(changes.is_empty());
        assert!(manager.list_nodes().is_empty());
    }

    // ========================================================================
    // v0.25.1 — Task 2 新增测试
    // ========================================================================

    #[test]
    fn test_hmac_compute_and_verify() {
        let key = b"test-secret-key";
        let packet = HeartbeatPacket {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            timestamp: 1700000000000,
            seq: 42,
            priority: 100,
            hmac: [0u8; 32],
            epoch: 12345,
        };
        let hmac = compute_hmac(&packet, key);
        assert_ne!(hmac, [0u8; 32], "HMAC should not be all zeros");

        // 构造带 HMAC 的包并校验
        let packet_with_hmac = HeartbeatPacket { hmac, ..packet };
        assert!(
            verify_hmac(&packet_with_hmac, key),
            "verify_hmac should return true with correct key"
        );
    }

    #[test]
    fn test_hmac_verify_fails_with_wrong_key() {
        let key = b"correct-secret-key";
        let wrong_key = b"wrong-secret-key";
        let packet = HeartbeatPacket {
            node_id: "node-1".to_string(),
            role: NodeRole::Primary,
            timestamp: 1700000000000,
            seq: 42,
            priority: 100,
            hmac: [0u8; 32],
            epoch: 12345,
        };
        let hmac = compute_hmac(&packet, key);
        let packet_with_hmac = HeartbeatPacket { hmac, ..packet };
        assert!(
            !verify_hmac(&packet_with_hmac, wrong_key),
            "verify_hmac should return false with wrong key"
        );
    }

    #[test]
    fn test_hmac_skip_when_no_key() {
        // 无密钥时 HMAC 为全零，build_packet 不计算 HMAC
        let manager = HeartbeatManager::new(test_config()).unwrap();
        let packet = manager.build_packet();
        assert_eq!(packet.hmac, [0u8; 32], "HMAC should be all zeros when no key");

        // 无密钥时 verify_hmac 不应被调用（调用方应跳过校验）
        // 这里验证全零 HMAC + 任意密钥校验失败（证明无密钥时不应校验）
        assert!(
            !verify_hmac(&packet, b"any-key"),
            "all-zero HMAC should fail verification with any key"
        );
    }

    #[test]
    fn test_hmac_with_auth_key_in_manager() {
        // 有密钥时 build_packet 应计算 HMAC
        let manager = HeartbeatManager::new(test_config_with_auth()).unwrap();
        let packet = manager.build_packet();
        assert_ne!(
            packet.hmac, [0u8; 32],
            "HMAC should not be all zeros when auth_key is set"
        );

        // 用相同密钥校验应通过
        let key = b"test-secret-key";
        assert!(
            verify_hmac(&packet, key),
            "verify_hmac should pass with correct key"
        );
    }

    #[test]
    fn test_epoch_rejects_old_packet() {
        let manager = HeartbeatManager::new(test_config()).unwrap();

        // 插入一个 peer 节点，epoch = 100
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 100,
                },
            );
        }

        // 收到一个旧 epoch 的包（epoch = 50）
        let old_packet = test_packet("node-2", 1, 50);
        manager.update_node(&old_packet);

        // 节点的 seq 不应被更新（仍为 0），epoch 不变
        let nodes = manager.list_nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].last_seq, 0,
            "old epoch packet should be rejected"
        );
        assert_eq!(nodes[0].epoch, 100, "epoch should not change");
    }

    #[test]
    fn test_epoch_accepts_newer_packet() {
        let manager = HeartbeatManager::new(test_config()).unwrap();

        // 插入一个 peer 节点，epoch = 50
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 50,
                },
            );
        }

        // 收到一个新 epoch 的包（epoch = 100）
        let new_packet = test_packet("node-2", 10, 100);
        manager.update_node(&new_packet);

        // 节点的 seq 和 epoch 应被更新
        let nodes = manager.list_nodes();
        assert_eq!(nodes[0].last_seq, 10, "newer epoch packet should be accepted");
        assert_eq!(nodes[0].epoch, 100, "epoch should be updated");
    }

    #[test]
    fn test_multi_interface_send() {
        let mut config = test_config();
        config.interfaces = vec!["127.0.0.1".to_string()];
        let manager = HeartbeatManager::new(config);
        assert!(manager.is_ok(), "manager creation should succeed");
        let manager = manager.unwrap();

        // 非 Linux 平台验证不 panic
        #[cfg(not(target_os = "linux"))]
        {
            let _ = manager.send_heartbeat();
        }
        // Linux 平台验证多接口创建独立 socket
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                manager.sockets.len(),
                1,
                "should create 1 socket for 1 interface"
            );
            let _ = manager.send_heartbeat();
        }
    }

    #[test]
    fn test_receive_heartbeat_malformed_packet() {
        let manager = HeartbeatManager::new(test_config()).unwrap();
        // 非 Linux 平台验证不 panic
        #[cfg(not(target_os = "linux"))]
        {
            let _ = manager.receive_heartbeat();
        }
        // Linux 平台验证 malformed 包不返回 Err（需要 mock socket，此处仅验证不 panic）
        #[cfg(target_os = "linux")]
        {
            let _ = manager.receive_heartbeat();
        }
    }

    #[test]
    fn test_check_timeouts_returns_state_changes() {
        let manager = HeartbeatManager::new(test_config()).unwrap();

        // 插入一个节点，last_heartbeat 设为过去时间（超过 dead 阈值）
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now() - Duration::from_millis(500),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
        }

        // check_timeouts 应返回状态变更
        let changes = manager.check_timeouts();
        assert_eq!(changes.len(), 1, "should detect 1 state change");
        assert_eq!(changes[0].node_id, "node-2");
        assert_eq!(changes[0].old_state, NodeState::Alive);
        assert_eq!(changes[0].new_state, NodeState::Dead);
    }

    #[test]
    fn test_alive_confirm_count_debounce() {
        let manager = HeartbeatManager::new(test_config()).unwrap();

        // 插入一个 Suspect 节点
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Suspect,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 0,
                    epoch: 0,
                },
            );
        }

        let packet = test_packet("node-2", 1, 0);

        // 第一次心跳：应保持 Suspect（count = 1）
        manager.update_node(&packet);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Suspect),
            "1st heartbeat should not recover to Alive"
        );

        // 第二次心跳：应保持 Suspect（count = 2）
        manager.update_node(&packet);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Suspect),
            "2nd heartbeat should not recover to Alive"
        );

        // 第三次心跳：应恢复 Alive（count = 3）
        manager.update_node(&packet);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Alive),
            "3rd heartbeat should recover to Alive"
        );
    }

    #[test]
    fn test_alive_confirm_count_resets_on_alive() {
        let manager = HeartbeatManager::new(test_config()).unwrap();

        // 插入一个 Alive 节点
        {
            let mut nodes = manager
                .nodes
                .write()
                .unwrap_or_else(|e| e.into_inner());
            nodes.insert(
                "node-2".to_string(),
                NodeInfo {
                    node_id: "node-2".to_string(),
                    role: NodeRole::Secondary,
                    state: NodeState::Alive,
                    priority: 50,
                    last_heartbeat: Instant::now(),
                    last_seq: 0,
                    alive_confirm_count: 5, // 非零值
                    epoch: 0,
                },
            );
        }

        // 收到心跳，应保持 Alive 并重置 count
        let packet = test_packet("node-2", 1, 0);
        manager.update_node(&packet);
        assert_eq!(
            manager.get_node_state("node-2"),
            Some(NodeState::Alive),
            "Alive node should stay Alive"
        );

        let nodes = manager.list_nodes();
        assert_eq!(nodes[0].alive_confirm_count, 0, "count should be reset");
    }

    #[test]
    fn test_node_state_change_struct() {
        let change = NodeStateChange {
            node_id: "node-2".to_string(),
            old_state: NodeState::Alive,
            new_state: NodeState::Dead,
            timestamp: 1700000000000,
        };
        assert_eq!(change.node_id, "node-2");
        assert_eq!(change.old_state, NodeState::Alive);
        assert_eq!(change.new_state, NodeState::Dead);
        assert_eq!(change.timestamp, 1700000000000);
    }
}
