//! Packet capture and analysis types (D11: host-side std).
//!
//! 定义 [`Packet`] / [`PacketDirection`] / [`CaptureConfig`] /
//! [`PacketCapture`] / [`CaptureStats`]，封装抓包会话的状态与统计。
//! 当前为骨架实现（`capture` 返回 0 包），真实抓包由后续版本补全。

use std::collections::HashMap;
use std::fmt;

/// 抓包方向（相对被测设备）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDirection {
    /// 被测设备接收（设备 ← 主机/控制器）。
    Rx,
    /// 被测设备发送（设备 → 主机/控制器）。
    Tx,
}

/// 单个被抓取的协议报文。
#[derive(Debug, Clone)]
pub struct Packet {
    /// 自抓包开始计的毫秒时间戳。
    pub timestamp_ms: u64,
    /// 协议名（`modbus` / `iec104` / `can`）。
    pub protocol: String,
    /// 源端点（IP:port / CAN id / RTU 从站地址 ...）。
    pub source: String,
    /// 目的端点。
    pub destination: String,
    /// 原始负载字节。
    pub data: Vec<u8>,
    /// 相对被测设备的方向。
    pub direction: PacketDirection,
}

/// 抓包会话配置。
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// 抓包网卡（如 `eth0`）。
    pub interface: String,
    /// TCP/UDP 端口过滤（0 = 抓取所有端口）。
    pub port: u16,
    /// 解码协议（`modbus` / `iec104` / `can`）。
    pub protocol: String,
    /// 缓冲区保留的最大报文数。
    pub max_packets: u32,
}

/// 抓包会话的聚合统计。
#[derive(Debug, Clone)]
pub struct CaptureStats {
    pub total_packets: u32,
    pub rx_count: u32,
    pub tx_count: u32,
    /// 按协议分组的报文计数。
    pub protocol_breakdown: HashMap<String, u32>,
}

/// 有状态的抓包缓冲区。
pub struct PacketCapture {
    /// 抓包配置。
    pub config: CaptureConfig,
    /// 已抓取的报文（按到达顺序）。
    pub packets: Vec<Packet>,
}

impl PacketCapture {
    /// 创建新的（空）抓包会话。
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            packets: Vec::new(),
        }
    }

    /// 抓包 `duration_ms` 毫秒。
    ///
    /// 骨架实现始终返回 0 包。真实实现将在 `config.interface` 上打开
    /// raw socket / pcap 句柄，并按 `config.protocol` 解码帧。
    pub fn capture(&mut self, _duration_ms: u64) -> Result<usize, String> {
        // Stub: 骨架工具链阶段不做真实抓包。
        Ok(0)
    }

    /// 借用已抓取的报文缓冲区。
    pub fn packets(&self) -> &[Packet] {
        &self.packets
    }

    /// 对已抓取报文计算聚合统计。
    pub fn analyze(&self) -> CaptureStats {
        let mut rx_count = 0u32;
        let mut tx_count = 0u32;
        let mut protocol_breakdown: HashMap<String, u32> = HashMap::new();

        for p in &self.packets {
            match p.direction {
                PacketDirection::Rx => rx_count += 1,
                PacketDirection::Tx => tx_count += 1,
            }
            *protocol_breakdown.entry(p.protocol.clone()).or_insert(0) += 1;
        }

        CaptureStats {
            total_packets: self.packets.len() as u32,
            rx_count,
            tx_count,
            protocol_breakdown,
        }
    }
}

impl fmt::Display for CaptureStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CaptureStats {{ total: {}, rx: {}, tx: {}, protocols: {:?} }}",
            self.total_packets, self.rx_count, self.tx_count, self.protocol_breakdown
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CaptureConfig {
        CaptureConfig {
            interface: "eth0".to_string(),
            port: 502,
            protocol: "modbus".to_string(),
            max_packets: 16,
        }
    }

    #[test]
    fn capture_stub_returns_zero() {
        let mut cap = PacketCapture::new(cfg());
        assert_eq!(cap.capture(100).unwrap(), 0);
        assert!(cap.packets().is_empty());
    }

    #[test]
    fn analyze_empty() {
        let cap = PacketCapture::new(cfg());
        let stats = cap.analyze();
        assert_eq!(stats.total_packets, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.tx_count, 0);
        assert!(stats.protocol_breakdown.is_empty());
    }

    #[test]
    fn analyze_with_packets() {
        let mut cap = PacketCapture::new(cfg());
        cap.packets.push(Packet {
            timestamp_ms: 1,
            protocol: "modbus".to_string(),
            source: "1.2.3.4:502".to_string(),
            destination: "5.6.7.8:502".to_string(),
            data: vec![0x01],
            direction: PacketDirection::Rx,
        });
        cap.packets.push(Packet {
            timestamp_ms: 2,
            protocol: "iec104".to_string(),
            source: "5.6.7.8:2404".to_string(),
            destination: "1.2.3.4:2404".to_string(),
            data: vec![0x02],
            direction: PacketDirection::Tx,
        });
        cap.packets.push(Packet {
            timestamp_ms: 3,
            protocol: "modbus".to_string(),
            source: "1.2.3.4:502".to_string(),
            destination: "5.6.7.8:502".to_string(),
            data: vec![0x03],
            direction: PacketDirection::Tx,
        });

        let stats = cap.analyze();
        assert_eq!(stats.total_packets, 3);
        assert_eq!(stats.rx_count, 1);
        assert_eq!(stats.tx_count, 2);
        assert_eq!(*stats.protocol_breakdown.get("modbus").unwrap(), 2);
        assert_eq!(*stats.protocol_breakdown.get("iec104").unwrap(), 1);
    }
}
