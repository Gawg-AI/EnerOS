//! ARC-007：IEC104 主站独立对端互操作证据。
//!
//! 独立对端 = 测试内手写 APDU 的 raw `std::net::TcpListener`（与 crate 的
//! 编解码代码**零共享**：帧按 IEC 60870-5-104 规范逐字节手写）。
//! 测试经 `RawTcpTransport`（仅实现 `MasterTransport` trait 的 std 适配层）
//! 驱动真实主站栈连接对端：STARTDT 握手 + 总召唤 + 遥测应答报文实录。
#![allow(clippy::disallowed_macros)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use eneros_iec104_master::transport::{ConnId, MasterTransport};
use eneros_iec104_master::{MasterConfig, MasterError, RemoteDevice};

/// Raw TCP 传输层（仅字节搬运，不含 IEC104 编解码）。
struct RawTcpTransport {
    stream: Option<TcpStream>,
    rx: Vec<Vec<u8>>,
    tx: Vec<Vec<u8>>,
    now: u64,
}

impl RawTcpTransport {
    fn new() -> Self {
        Self { stream: None, rx: Vec::new(), tx: Vec::new(), now: 0 }
    }
}

impl MasterTransport for RawTcpTransport {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError> {
        let addr = format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port);
        self.stream =
            Some(TcpStream::connect(&addr).map_err(|_| MasterError::ConnectFailed)?);
        Ok(1)
    }
    fn send(&mut self, _conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        self.tx.push(data.to_vec());
        if let Some(s) = &mut self.stream {
            s.write_all(data).map_err(|_| MasterError::SendFailed)?;
        }
        Ok(())
    }
    fn recv(&mut self, _conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        if let Some(f) = self.rx.pop() {
            return Ok(Some(f));
        }
        if let Some(s) = &mut self.stream {
            let mut buf = [0u8; 512];
            let n = s.read(&mut buf).map_err(|_| MasterError::RecvFailed)?;
            if n > 0 {
                return Ok(Some(buf[..n].to_vec()));
            }
        }
        Ok(None)
    }
    fn close(&mut self, _conn: ConnId) -> Result<(), MasterError> {
        self.stream = None;
        Ok(())
    }
    fn now_ms(&self) -> u64 {
        self.now
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// 手写 APDU（IEC 60870-5-104 规范，独立于 crate 实现）。
mod apdu {
    /// STARTDT 激活（U-frame）：68 04 07 00 00 00
    pub const STARTDT_ACT: [u8; 6] = [0x68, 0x04, 0x07, 0x00, 0x00, 0x00];
    /// STARTDT 确认（U-frame）：68 04 0B 00 00 00
    pub const STARTDT_CON: [u8; 6] = [0x68, 0x04, 0x0B, 0x00, 0x00, 0x00];

    /// I-frame 总召唤激活（C_IC_NA 无时标, cot=6 激活, 地址 1, QOI=20）。
    /// 报文实录（主站实际发送）：68 0E 00 00 00 00 64 01 06 00 01 00 00 00 00 20
    /// APDU 长 0x0E=14：控制域 4 + ASDU 10（type/num/cot2/addr2/ioa2/qoi1）。
    pub fn interrogate_act() -> [u8; 16] {
        [
            0x68, 0x0E, 0x00, 0x00, 0x00, 0x00, 0x64, 0x01, 0x06, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x14,
        ]
    }

    /// 总召唤确认（I-frame, cot=7 激活确认, N(R)=1）。
    /// 68 0E 00 00 01 00 64 01 07 00 01 00 00 00 00 14
    pub fn interrogate_con() -> [u8; 16] {
        [
            0x68, 0x0E, 0x00, 0x00, 0x01, 0x00, 0x64, 0x01, 0x07, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x14,
        ]
    }

    /// 遥测数据（M_ME_NC 浮点带时标, ioa=1, value=25.5f32, q=0x00, cot=20 响应总召唤）。
    /// 68 16 00 00 01 00 0D 01 14 00 01 00 01 00 00 00 CC 41 00 00 00 00 00 00 00 00
    /// ASDU：type=0x0D, num=1, cot=20, addr=1, ioa=1, f32(00 00 CC 41), q=00, CP56Time2a×7
    pub fn telemetry() -> [u8; 26] {
        [
            0x68, 0x16, 0x00, 0x00, 0x01, 0x00, 0x0D, 0x01, 0x14, 0x00, 0x01, 0x00, 0x01, 0x00,
            0x00, 0x00, 0xCC, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    }
}

/// 独立对端：raw TCP 服务，手写 APDU 应答 STARTDT 与总召唤。
fn spawn_independent_peer() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut received: Vec<u8> = Vec::new();
        let mut buf = [0u8; 6];
        sock.read_exact(&mut buf).unwrap();
        received.extend_from_slice(&buf);
        assert_eq!(buf, apdu::STARTDT_ACT, "对端收到 STARTDT 激活（报文实录）");
        sock.write_all(&apdu::STARTDT_CON).unwrap();
        let mut ibuf = [0u8; 16];
        sock.read_exact(&mut ibuf).unwrap();
        received.extend_from_slice(&ibuf);
        assert_eq!(ibuf, apdu::interrogate_act(), "对端收到总召唤激活");
        sock.write_all(&apdu::interrogate_con()).unwrap();
        sock.write_all(&apdu::telemetry()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        received
    });
    (port, handle)
}

#[test]
fn arc007_iec104_independent_peer_interop() {
    let (port, peer) = spawn_independent_peer();
    let transport = RawTcpTransport::new();
    let mut master = eneros_iec104_master::master::Iec104Master::new(
        MasterConfig::default(),
        Box::new(transport),
    );
    let device = RemoteDevice::new([127, 0, 0, 1], port, 1, 1000);
    master.connect(&device).unwrap();

    // 第一轮：处理 STARTDT_CON；第二轮：总召唤 + 遥测
    master.poll(100);
    master.poll(1100);
    std::thread::sleep(std::time::Duration::from_millis(100));
    master.poll(2100);

    let rx = peer.join().unwrap();
    assert_eq!(&rx[..6], &apdu::STARTDT_ACT, "对端报文实录：STARTDT 激活");
    assert_eq!(&rx[6..], &apdu::interrogate_act()[..], "对端报文实录：总召唤激活");
    assert!(
        master.stats().rx_point_count > 0,
        "主站解析对端遥测：rx_point_count={}",
        master.stats().rx_point_count
    );
}
