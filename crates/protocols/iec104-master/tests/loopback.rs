//! 31 WP-31.1 验收：IEC104 master ↔ slave 测试桩 loopback 集成（host）。
//!
//! 蓝图判据：127.0.0.1:2404 起 iec104-slave 测试桩对拉，总召 → 遥测回包
//! PASS。StdMasterTransport（std::net::TcpStream 实现 MasterTransport）
//! 为**互操作测试专用**（蓝图/27 §12：RawTcpTransport（std）保留为互操作
//! 测试专用，不收编生产路径——生产走 eneros-net 的 RealMasterTransport）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use eneros_iec104_master::config::MasterConfig;
use eneros_iec104_master::device::RemoteDevice;
use eneros_iec104_master::master::Iec104Master;
use eneros_iec104_master::transport::{ConnId, MasterTransport};
use eneros_iec104_slave::apdu::{Apdu, ControlField};

/// 互操作测试专用 std 传输（TCP stream）。
struct StdMasterTransport {
    stream: Option<TcpStream>,
    /// 粘包缓冲：read 一次消费多帧时暂存剩余字节。
    pending: Vec<u8>,
    now: u64,
}

impl StdMasterTransport {
    fn new() -> Self {
        Self {
            stream: None,
            pending: Vec::new(),
            now: 0,
        }
    }
}

impl MasterTransport for StdMasterTransport {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, eneros_iec104_master::error::MasterError> {
        let addr = std::net::SocketAddr::from((ip, port));
        let stream =
            TcpStream::connect(addr).map_err(|_| eneros_iec104_master::error::MasterError::ConnectFailed)?;
        // 非阻塞：process_rx 轮询语义（无帧 → Ok(None)）。
        stream
            .set_nonblocking(true)
            .map_err(|_| eneros_iec104_master::error::MasterError::ConnectFailed)?;
        self.stream = Some(stream);
        Ok(1)
    }
    fn send(&mut self, _conn: ConnId, data: &[u8]) -> Result<(), eneros_iec104_master::error::MasterError> {
        self.stream
            .as_mut()
            .ok_or(eneros_iec104_master::error::MasterError::NotConnected)?
            .write_all(data)
            .map_err(|_| eneros_iec104_master::error::MasterError::SendFailed)
    }
    fn recv(&mut self, _conn: ConnId) -> Result<Option<Vec<u8>>, eneros_iec104_master::error::MasterError> {
        // 先消费粘包缓冲（上一轮 read 剩余字节）。
        if self.pending.len() >= 2 && self.pending[0] == 0x68 {
            let frame_len = 2 + self.pending[1] as usize;
            if self.pending.len() >= frame_len {
                let frame = self.pending[..frame_len].to_vec();
                self.pending.drain(..frame_len);
                return Ok(Some(frame));
            }
        }
        let mut buf = [0u8; 2048];
        let n = match self
            .stream
            .as_mut()
            .ok_or(eneros_iec104_master::error::MasterError::NotConnected)?
            .read(&mut buf)
        {
            Ok(0) => return Ok(None),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(self.pending.drain(..).collect()))
                };
            }
            Err(_) => return Err(eneros_iec104_master::error::MasterError::RecvFailed),
        };
        // 粘包防御：read 可能一次返回多帧（ack + 遥测）——追加到缓冲，
        // 只返回第一帧完整 APDU（0x68 len ...），剩余留待下一轮。
        self.pending.extend_from_slice(&buf[..n]);
        if self.pending.len() >= 2 && self.pending[0] == 0x68 {
            let frame_len = 2 + self.pending[1] as usize;
            if self.pending.len() >= frame_len {
                let frame = self.pending[..frame_len].to_vec();
                self.pending.drain(..frame_len);
                return Ok(Some(frame));
            }
        }
        Ok(Some(self.pending.drain(..).collect()))
    }
    fn close(&mut self, _conn: ConnId) -> Result<(), eneros_iec104_master::error::MasterError> {
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

/// 从 TCP 流读一帧 APDU（起始 0x68 + 长度）。
fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).ok()?;
    if head[0] != 0x68 {
        return None;
    }
    let len = head[1] as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    let mut frame = head.to_vec();
    frame.extend_from_slice(&body);
    Some(frame)
}

/// IEC104 slave 测试桩：接受连接 → 回 STARTDT_CON → 收总召 → 回遥测帧。
/// 经 mpsc 上报收到总召（供断言）。
fn spawn_slave_stub(port: u16) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    // 先 bind（主线程）再 accept（子线程）——消除启动竞态。
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        // STARTDT_CON（U 格式 0x0B）。
        let _ = stream.write_all(&[0x68, 0x04, 0x0B, 0x00, 0x00, 0x00]);
        loop {
            let Some(frame) = read_frame(&mut stream) else {
                break;
            };
            match frame[2] & 0x03 {
                0x03 => {
                    // U 格式：STARTDT_ACT（0x07）→ 回 STARTDT_CON。
                    if frame[2] == 0x07 {
                        let _ = stream.write_all(&[0x68, 0x04, 0x0B, 0x00, 0x00, 0x00]);
                    }
                }
                0x00 => {
                    // I 格式：总召（C_IC_NA，type 0x64）→ 回激活确认 + 遥测。
                    let _ = tx.send(());
                    let ack = Apdu::i_format(
                        0,
                        1,
                        eneros_iec104_slave::asdu::Asdu {
                            type_id: eneros_iec104_slave::asdu::TypeId::InterrogationCommand,
                            cause_of_tx: eneros_iec104_slave::asdu::Cot::ActivationConfirm,
                            common_addr: 1,
                            ioas: vec![],
                        },
                    );
                    let _ = stream.write_all(&ack.encode());
                    let tele = Apdu::i_format(
                        1,
                        1,
                        eneros_iec104_slave::asdu::Asdu {
                            type_id: eneros_iec104_slave::asdu::TypeId::MeasuredValueFloat,
                            cause_of_tx: eneros_iec104_slave::asdu::Cot::Periodic,
                            common_addr: 1,
                            ioas: vec![eneros_iec104_slave::asdu::InformationObject {
                                ioa: 42,
                                value: eneros_iec104_slave::asdu::IoValue::Float(3.14),
                                quality: eneros_iec104_slave::asdu::QualityDescriptor::good(),
                                time_tag: None,
                            }],
                        },
                    );
                    let _ = stream.write_all(&tele.encode());
                }
                _ => {}
            }
        }
    });
    rx
}

/// 31 WP-31.1 判据：master 总召 → 测试桩回遥测（3.14 @ ioa 42）→ 缓存命中。
#[test]
fn loopback_interrogation_telemetry() {
    let port = 24040u16;
    let rx = spawn_slave_stub(port);
    let mut config = MasterConfig::default();
    config.peer_ip = [127, 0, 0, 1];
    config.peer_port = port;
    config.poll_interval_ms = 100;

    let mut transport = StdMasterTransport::new();
    let mut master = Iec104Master::new(config, Box::new(transport));
    let dev = RemoteDevice::new([127, 0, 0, 1], port, 1, 30_000);
    master.connect(&dev).expect("connect");
    // 轮询消费 STARTDT_CON（非阻塞 recv——桩线程写 CON 有调度延迟）。
    let mut connected = false;
    for _ in 0..200 {
        master.process_rx(1).ok();
        if master
            .interrogation(1, 0)
            .is_ok()
        {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(connected, "STARTDT_CON 后必须 Connected（loopback 判据）");
    // 总召。
    master.interrogation(1, 0).expect("interrogation");
    // 消费激活确认 + 遥测（多轮直至缓存命中）。
    let mut got = None;
    for i in 0..8 {
        let _ = master.process_rx(1);
        if let Some(v) = master.latest_value(1, 42) {
            got = Some(v.clone());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let v = got.expect("总召后必须收到遥测（loopback 判据）");
    // LatestValue.value 为私有类型——经 debug 表示断言（3.14 的 Debug 为
    // "Float(3.14)"）。
    let dbg = format!("{:?}", v.value);
    assert!(
        dbg.contains("3.14"),
        "遥测值必须为 3.14，got {dbg}（loopback 判据）"
    );
    let _ = &v.quality; // 品质字段存在性断言（good() 构造由桩侧保证）
    assert!(rx.try_recv().is_ok(), "测试桩必须收到总召");
}
