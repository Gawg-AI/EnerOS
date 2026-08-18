//! IEC 104 主站真实传输层（31 WP-31.1 / 吸收 WP-27.8）。
//!
//! 基于 eneros-net（smoltcp 封装）的 TCP 客户端传输，签名对照
//! `eneros-iec104-slave` 的 `SmoltcpSlaveTransport`（transport.rs 模式）：
//! connect/send/recv/close 内部驱动协议栈轮询；`now_ms` 来自单调时钟
//! （eneros-time）。connect 失败 fail-closed。

use alloc::vec::Vec;

use eneros_net::tcpip::{
    ip_endpoint, ipv4_addr, InterfaceConfig, NetworkInterface, SocketAddr, TcpSocket,
};
use eneros_net::NetDevice;

use crate::error::MasterError;
use crate::transport::{ConnId, MasterTransport};

/// TCP RX 缓冲（字节）。
const RX_BUF: usize = 4096;
/// TCP TX 缓冲（字节）。
const TX_BUF: usize = 4096;

/// 基于 eneros-net（smoltcp）的 IEC 104 主站传输。
///
/// 生产实现：`connect/send/recv/close/now_ms` 内部驱动协议栈轮询，
/// 面向主站（主动 connect）语义；与 `MockMasterTransport` 同 trait、
/// 不同类型（21-B B1：生产实现与 mock 并列，mock 仅测试）。
#[cfg(feature = "net")]
pub struct RealMasterTransport<D: NetDevice> {
    /// 网络接口（协议栈状态；内含 socket 集）。
    iface: NetworkInterface<D>,
    /// TCP socket 句柄。
    tcp: TcpSocket,
    /// 当前连接标识（0 = 未连接）。
    conn_id: ConnId,
    /// 31-review（B2）：TCP RX 重组缓冲——按 0x68 len 帧边界累积/切分。
    rx_buf: Vec<u8>,
}

#[cfg(feature = "net")]
impl<D: NetDevice> RealMasterTransport<D> {
    /// 创建主站传输（`device` = 网卡；`config` = 网络接口配置）。
    /// 对端地址在 `MasterTransport::connect` 时由装配层注入，
    /// 禁止生产路径字面量 IP（PR-14）。
    pub fn new(device: D, config: InterfaceConfig) -> Result<Self, MasterError> {
        let mut iface = NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        let tcp = TcpSocket::new(handle);
        Ok(Self {
            iface,
            tcp,
            conn_id: 0,
            rx_buf: Vec::new(),
        })
    }

    /// 推进协议栈（单调时钟，毫秒）。
    fn poll(&mut self) {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = self.iface.poll(now_ms);
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice + 'static> MasterTransport for RealMasterTransport<D> {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError> {
        let remote: SocketAddr = ip_endpoint(ipv4_addr(ip[0], ip[1], ip[2], ip[3]), port);
        // 本地端口：自动选择（从 20000 起扫描可用）。
        for local in 20000u16..20100 {
            if self.tcp.connect(&mut self.iface, remote, local).is_err() {
                continue;
            }
            // fail-closed（PR-02）：smoltcp connect 仅发起握手，
            // 必须轮询到 Established 才算连接成功；有界防 SYN 重试死等。
            for _ in 0..5000 {
                self.poll();
                let st = self.tcp.state(&self.iface);
                if st.is_connected() {
                    self.conn_id = 1;
                    return Ok(1);
                }
                if st.is_closed() {
                    break; // 握手失败 → 换本地端口重试
                }
            }
        }
        Err(MasterError::ConnectFailed)
    }

    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        let mut sent = 0usize;
        // 阻塞语义：轮询推进直到全部字节写入 TX 缓冲。
        for _ in 0..2000 {
            self.poll();
            match self.tcp.send(&mut self.iface, &data[sent..]) {
                Ok(0) => continue,
                Ok(n) => sent += n,
                Err(_) => return Err(MasterError::SendFailed),
            }
            if sent >= data.len() {
                return Ok(());
            }
        }
        Err(MasterError::SendFailed)
    }

    fn recv(&mut self, conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        // 连接已关闭（对端 FIN/RST）→ 清连接（fail-closed）。
        if self.tcp.state(&self.iface).is_closed() {
            self.conn_id = 0;
            return Err(MasterError::RecvFailed);
        }
        self.poll();
        // 31-review（B2）：TCP 是字节流——累积到重组缓冲，按 0x68 len 帧边界
        // 取一个完整 APDU（旧实现直喂 Apdu::decode，粘包丢尾帧/半帧致永久失步）。
        let mut buf = [0u8; 2048];
        match self.tcp.recv(&mut self.iface, &mut buf) {
            Ok(n) => self.rx_buf.extend_from_slice(&buf[..n]),
            Err(_) => return Err(MasterError::RecvFailed),
        }
        loop {
            if self.rx_buf.is_empty() {
                return Ok(None);
            }
            if self.rx_buf[0] != 0x68 {
                // 重同步：丢弃到下一个起始字节。
                let pos = self
                    .rx_buf
                    .iter()
                    .position(|&b| b == 0x68)
                    .unwrap_or(self.rx_buf.len());
                self.rx_buf.drain(..pos);
                continue;
            }
            if self.rx_buf.len() < 2 {
                return Ok(None); // 半包（等起始+长度）
            }
            let len = self.rx_buf[1] as usize;
            if len < 4 || 2 + len > 4096 {
                self.rx_buf.drain(..1); // 坏长度：丢起始字节重同步
                continue;
            }
            if self.rx_buf.len() < 2 + len {
                return Ok(None); // 半包（等帧体）
            }
            let frame: Vec<u8> = self.rx_buf.drain(..2 + len).collect();
            return Ok(Some(frame));
        }
    }

    fn close(&mut self, conn: ConnId) -> Result<(), MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        self.tcp.close(&mut self.iface);
        for _ in 0..100 {
            self.poll();
        }
        self.conn_id = 0;
        self.rx_buf.clear(); // 31-review（B2）：清重组缓冲，防重连后残留旧帧。
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        eneros_time::get_monotonic_ns() / 1_000_000
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

#[cfg(all(test, feature = "net"))]
mod tests {
    use eneros_net::{NetError, NetStats};

    use super::*;

    /// 最小测试网卡（无帧进出）——eneros-net 的 MockNetDevice 仅
    /// cfg(test) 内部可见不跨 crate，此处自含 stub（仅测试装配）。
    struct NullNetDevice;

    impl NetDevice for NullNetDevice {
        fn send(&mut self, _frame: &[u8]) -> Result<(), NetError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, NetError> {
            Err(NetError::NoBuffer)
        }
        fn mac_address(&self) -> [u8; 6] {
            [0x02, 0, 0, 0, 0, 0x02]
        }
        fn mtu(&self) -> usize {
            1500
        }
        fn link_up(&self) -> bool {
            true
        }
        fn set_promiscuous(&mut self, _on: bool) {}
        fn stats(&self) -> NetStats {
            NetStats::new()
        }
    }

    /// 接线断言（S2）：`RealMasterTransport` 可构造并装入
    /// `Box<dyn MasterTransport>`（`Iec104Master` 装配面）。
    /// 行为判据（总召对拉）在 `tests/loopback.rs` host 真 TCP 集成覆盖。
    #[test]
    fn real_transport_constructs_as_trait_object() {
        let cfg = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x02])
            .with_ipv4(eneros_net::tcpip::ipv4_cidr(ipv4_addr(10, 0, 0, 2), 24))
            .with_gateway(ipv4_addr(10, 0, 0, 1));
        let t = RealMasterTransport::new(NullNetDevice, cfg).expect("transport");
        let boxed: alloc::boxed::Box<dyn MasterTransport> = alloc::boxed::Box::new(t);
        let _ = boxed.now_ms(); // trait object 可调度
    }

    /// connect 无对端（无帧回应的空网卡）→ 扫描窗口耗尽后
    /// ConnectFailed（fail-closed，PR-02 判据）。
    #[test]
    fn connect_unreachable_fails_closed() {
        let cfg = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x02])
            .with_ipv4(eneros_net::tcpip::ipv4_cidr(ipv4_addr(10, 0, 0, 2), 24));
        let mut t = RealMasterTransport::new(NullNetDevice, cfg).expect("transport");
        let r = t.connect([10, 0, 0, 9], 2404);
        assert!(r.is_err(), "不可达对端必须 ConnectFailed（fail-closed）");
    }
}
