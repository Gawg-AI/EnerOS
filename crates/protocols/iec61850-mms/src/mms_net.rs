//! MMS 真实 TCP 传输（v0.222.0，B2-05）。
//!
//! [`MmsTransport`] 此前仅 [`MockTransport`](crate::mms_client::MockTransport)。
//! 本模块提供基于 eneros-net（smoltcp）的 [`NetMmsTransport`]，经真实
//! DMA 环收发 MMS PDU（MMS over TCP，默认端口 102）。

use alloc::string::String;

use crate::mms_client::MmsTransport;
use crate::MmsError;

/// 读缓冲大小。
const RX_BUF: usize = 4096;
/// 发送缓冲大小。
const TX_BUF: usize = 4096;

/// 基于 eneros-net TCP 的 MMS 传输（feature `net`）。
pub struct NetMmsTransport<D: eneros_net::NetDevice> {
    /// 网络接口（内含 socket 集）。
    iface: eneros_net::tcpip::NetworkInterface<D>,
    /// TCP socket 句柄。
    tcp: eneros_net::tcpip::TcpSocket,
    /// 对端地址（IP 字符串）。
    #[allow(dead_code)]
    peer: String,
    /// 对端端口。
    #[allow(dead_code)]
    port: u16,
    /// 已连接。
    connected: bool,
    /// 单调时钟（ms）。
    now_ms: u64,
}

impl<D: eneros_net::NetDevice> NetMmsTransport<D> {
    /// 创建 MMS 传输（绑定网卡设备与接口配置）。
    pub fn new(
        device: D,
        config: eneros_net::tcpip::InterfaceConfig,
        peer: &str,
        port: u16,
    ) -> Result<Self, MmsError> {
        let mut iface = eneros_net::tcpip::NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        let tcp = eneros_net::tcpip::TcpSocket::new(handle);
        Ok(Self {
            iface,
            tcp,
            peer: String::from(peer),
            port,
            connected: false,
            now_ms: 0,
        })
    }

    /// 驱动协议栈（调用方周期调用）。
    pub fn poll(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        let _ = self.iface.poll(now_ms);
    }
}

impl<D: eneros_net::NetDevice> MmsTransport for NetMmsTransport<D> {
    fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError> {
        if self.connected {
            return Ok(());
        }
        let ip: eneros_net::tcpip::Ipv4Addr = addr.parse().map_err(|_| MmsError::ConnRefused)?;
        let remote = eneros_net::tcpip::ip_endpoint(ip, port);
        let local_port = 40000u16.wrapping_add(
            addr.as_bytes()
                .iter()
                .fold(0u16, |acc, b| acc.wrapping_add(*b as u16))
                % 10000,
        );
        self.tcp
            .connect(&mut self.iface, remote, local_port)
            .map_err(|_| MmsError::ConnRefused)?;
        for _ in 0..100 {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            if self.tcp.state(&self.iface).is_connected() {
                self.connected = true;
                return Ok(());
            }
        }
        Err(MmsError::Timeout)
    }

    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError> {
        if !self.connected {
            return Err(MmsError::NotConnected);
        }
        let mut sent = 0usize;
        while sent < pdu.len() {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            let n = self
                .tcp
                .send(&mut self.iface, &pdu[sent..])
                .map_err(|_| MmsError::TransportError)?;
            if n == 0 {
                self.now_ms += 1;
                let _ = self.iface.poll(self.now_ms);
                continue;
            }
            sent += n;
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError> {
        if !self.connected {
            return Err(MmsError::NotConnected);
        }
        for _ in 0..100 {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            match self.tcp.recv(&mut self.iface, buf) {
                Ok(n) if n > 0 => return Ok(n),
                Ok(_) => continue,
                Err(_) => return Err(MmsError::TransportError),
            }
        }
        Err(MmsError::Timeout)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_net::mac::NetDevice;
    use eneros_net::{NetError, NetStats};

    use super::*;

    /// 链路断开模拟设备：send/recv 均失败（无对端）。
    struct DeadNetDevice;

    impl NetDevice for DeadNetDevice {
        fn send(&mut self, _frame: &[u8]) -> Result<(), NetError> {
            Err(NetError::LinkDown)
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, NetError> {
            Err(NetError::LinkDown)
        }
        fn mac_address(&self) -> [u8; 6] {
            [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]
        }
        fn mtu(&self) -> usize {
            1500
        }
        fn link_up(&self) -> bool {
            false
        }
        fn set_promiscuous(&mut self, _on: bool) {}
        fn stats(&self) -> NetStats {
            NetStats::default()
        }
    }

    #[test]
    fn test_rx_tx_buf_sizes() {
        // 常量语义：读/写缓冲 4KiB（MMS PDU 最大帧）。
        assert_eq!(RX_BUF, 4096);
        assert_eq!(TX_BUF, 4096);
    }

    #[test]
    fn test_net_transport_fail_closed_on_dead_link() {
        // 蓝图/20 R2-5：真实 TCP 传输（smoltcp）fail-closed——
        // 链路断开时 connect 必须失败（不静默成功/不悬挂）。
        let device = DeadNetDevice;
        let config = eneros_net::tcpip::InterfaceConfig {
            mac_addr: device.mac_address(),
            ipv4_addr: Some(eneros_net::tcpip::Ipv4Cidr::new(
                [192, 168, 1, 10].into(),
                24,
            )),
            gateway: Some([192, 168, 1, 1].into()),
            dhcp: false,
        };
        let mut t = NetMmsTransport::new(device, config, "192.168.1.20", 102).unwrap();
        assert_eq!(
            t.connect("192.168.1.20", 102),
            Err(crate::MmsError::Timeout),
            "无链路必须超时失败"
        );
        assert_eq!(t.send(b"x"), Err(crate::MmsError::NotConnected));
    }

    #[test]
    fn test_mms_client_generic_compiles_with_net_transport() {
        // MmsClient<T: MmsTransport> 泛型接线（生产可注入 NetMmsTransport）。
        use crate::mms_client::MmsClient;
        let _ = core::mem::size_of::<MmsClient<NetMmsTransport<DeadNetDevice>>>();
    }
}
