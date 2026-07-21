//! Modbus TCP 从设备描述.

/// Modbus TCP 从设备.
///
/// 描述一个 TCP 从设备的连接参数：
/// - `ip`: IPv4 地址（4 字节，D4：不使用 smoltcp 的 Ipv4Addr）
/// - `port`: TCP 端口（Modbus 默认 502）
/// - `unit_id`: 单元 ID（对应 RTU 的从站地址语义）
/// - `timeout_ms`: 单次接收超时（毫秒）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpDevice {
    /// IPv4 地址（4 字节，如 `[192, 168, 1, 10]`）
    pub ip: [u8; 4],
    /// TCP 端口
    pub port: u16,
    /// 单元 ID（D3：语义等同 RTU slave_addr）
    pub unit_id: u8,
    /// 单次接收超时（毫秒）
    pub timeout_ms: u32,
}

impl TcpDevice {
    /// 创建 TCP 从设备，默认 `timeout_ms = 3000`。
    pub fn new(ip: [u8; 4], port: u16, unit_id: u8) -> Self {
        Self {
            ip,
            port,
            unit_id,
            timeout_ms: 3000,
        }
    }

    /// 创建 TCP 从设备，使用默认端口 502 与默认超时 3000ms。
    pub fn new_default(ip: [u8; 4], unit_id: u8) -> Self {
        Self {
            ip,
            port: Self::default_port(),
            unit_id,
            timeout_ms: 3000,
        }
    }

    /// Modbus TCP 默认端口（502）。
    pub fn default_port() -> u16 {
        502
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_custom_port() {
        let dev = TcpDevice::new([192, 168, 1, 10], 5020, 5);
        assert_eq!(dev.ip, [192, 168, 1, 10]);
        assert_eq!(dev.port, 5020);
        assert_eq!(dev.unit_id, 5);
        assert_eq!(dev.timeout_ms, 3000);
    }

    #[test]
    fn test_new_default_port_502() {
        let dev = TcpDevice::new_default([10, 0, 0, 1], 1);
        assert_eq!(dev.ip, [10, 0, 0, 1]);
        assert_eq!(dev.port, 502);
        assert_eq!(dev.unit_id, 1);
        assert_eq!(dev.timeout_ms, 3000);
    }

    #[test]
    fn test_default_port_constant() {
        assert_eq!(TcpDevice::default_port(), 502);
    }

    #[test]
    fn test_eq_and_copy() {
        let a = TcpDevice::new([192, 168, 0, 1], 502, 2);
        let b = a; // Copy
        assert_eq!(a, b);
        let c = TcpDevice::new([192, 168, 0, 2], 502, 2);
        assert_ne!(a, c);
    }

    #[test]
    fn test_clone() {
        let a = TcpDevice::new([172, 16, 0, 5], 1502, 3);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn test_zero_unit_id() {
        // unit_id 可为 0
        let dev = TcpDevice::new_default([127, 0, 0, 1], 0);
        assert_eq!(dev.unit_id, 0);
    }

    #[test]
    fn test_max_unit_id() {
        let dev = TcpDevice::new_default([127, 0, 0, 1], 255);
        assert_eq!(dev.unit_id, 255);
    }
}
