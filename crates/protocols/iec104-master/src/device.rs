//! IEC 104 主站远端设备描述与连接状态.
//!
//! D8：IP 地址用 `[u8; 4]` 表示 IPv4（无 `std::net::IpAddr`，与 v0.46.0 `TcpDevice` 一致）。

/// 远端设备（IEC 104 从站）描述
///
/// 标识一个被轮询的从站设备：IP、端口、公共地址、轮询周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteDevice {
    /// IPv4 地址（D8）
    pub ip: [u8; 4],
    /// TCP 端口
    pub port: u16,
    /// 公共地址（ASDU 地址）
    pub common_addr: u16,
    /// 轮询周期（毫秒）
    pub poll_interval_ms: u32,
}

impl RemoteDevice {
    /// 创建远端设备描述。
    pub const fn new(ip: [u8; 4], port: u16, common_addr: u16, poll_interval_ms: u32) -> Self {
        Self {
            ip,
            port,
            common_addr,
            poll_interval_ms,
        }
    }
}

/// 主站连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// 空闲（未连接）
    Idle,
    /// 连接中（传输层 connect 进行中）
    Connecting,
    /// STARTDT 等待确认
    StartDtPending,
    /// 已连接（数据传输中）
    Connected,
    /// 总召唤进行中
    Interrogating,
    /// 错误
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_device_new() {
        let dev = RemoteDevice::new([192, 168, 1, 1], 2404, 1, 30_000);
        assert_eq!(dev.ip, [192, 168, 1, 1]);
        assert_eq!(dev.port, 2404);
        assert_eq!(dev.common_addr, 1);
        assert_eq!(dev.poll_interval_ms, 30_000);
    }

    #[test]
    fn test_remote_device_eq() {
        let d1 = RemoteDevice::new([10, 0, 0, 1], 2404, 1, 30_000);
        let d2 = RemoteDevice::new([10, 0, 0, 1], 2404, 1, 30_000);
        let d3 = RemoteDevice::new([10, 0, 0, 2], 2404, 1, 30_000);
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
    }

    #[test]
    fn test_conn_state_eq() {
        assert_eq!(ConnState::Idle, ConnState::Idle);
        assert_ne!(ConnState::Connected, ConnState::Interrogating);
    }
}
