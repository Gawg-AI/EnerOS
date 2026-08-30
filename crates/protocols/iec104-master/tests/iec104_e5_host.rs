//! 蓝图 61 [07] §3.6.3 / §3.7（v0.314.31）：IEC104 E5 host 级标准 peer
//! 互操作——真实 TCP 字节流路径（非 mock bus 作 E4）。
//!
//! 对端 = 测试内手写 APDU 的 `std::net::TcpListener` 假从站（与 crate 的
//! 编解码代码**零共享**：帧按 IEC 60870-5-104 规范逐字节手写，46 L2
//! fixture `evidence/46/iec104_slave.py` 同纪律的 Rust 内嵌形态；QEMU 级
//! fixture = `evidence/61/iec104_slave61.py` 留给主会话实弹）。
//!
//! 主站侧走与生产 `StreamMasterTransport` 同构的 std 传输（0x68 len 粘包
//! 重组——46.5 根因面）：收发全程真实 TCP 字节流。覆盖 [07] §3.3 判据：
//! STARTDT 握手 + 总召唤（TI=100）、TI=36 带 CP56Time2a 时标浮点（时标
//! 保留）、IV=true 时钟未同步变体、品质 invalid 帧、重复 ASDU（同内容
//! 新 N(S)——协议合法重发）、断连重连（对端关闭后 accept 新连接）。
#![allow(clippy::disallowed_macros)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use eneros_iec104_master::adapter::{Iec104Adapter, IecPointMapping};
use eneros_iec104_master::config::MasterConfig;
use eneros_iec104_master::device::{ConnState, RemoteDevice};
use eneros_iec104_master::master::Iec104Master;
use eneros_iec104_master::transport::{ConnId, MasterTransport};
use eneros_iec104_master::MasterError;
// ProtocolAdapter trait（init/start/poll——数据面驱动入口）。
use eneros_protocol_abstract::ProtocolAdapter as _;
use eneros_protocol_abstract::{AdapterConfig, DeviceConfig, ProtocolAddress, ProtocolType};

// ===========================================================================
// 主站侧 std 传输（与生产 StreamMasterTransport 同构：0x68 len 粘包重组）
// ===========================================================================

/// 互操作测试专用 std 传输（真实 TCP 字节流 + APDU 重组）。
struct StdTcpTransport {
    stream: Option<TcpStream>,
    /// 粘包缓冲：一次 read 可能到多帧——按 0x68 len 边界切帧，余量保留。
    pending: Vec<u8>,
    started: Instant,
}

impl StdTcpTransport {
    fn new() -> Self {
        Self {
            stream: None,
            pending: Vec::new(),
            started: Instant::now(),
        }
    }
}

/// 从缓冲取出一个完整 APDU（0x68 打头 + 长度一致）；垃圾/坏长度逐字节
/// 重同步（与生产 StreamMasterTransport::take_frame 同约定）。
fn take_frame(pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        if pending.is_empty() {
            return None;
        }
        if pending[0] != 0x68 {
            pending.remove(0);
            continue;
        }
        if pending.len() < 2 {
            return None;
        }
        let len = pending[1] as usize;
        if len < 4 {
            pending.remove(0);
            continue;
        }
        if pending.len() < 2 + len {
            return None;
        }
        return Some(pending.drain(..2 + len).collect());
    }
}

impl MasterTransport for StdTcpTransport {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError> {
        let addr = SocketAddr::from((ip, port));
        let stream = TcpStream::connect(addr).map_err(|_| MasterError::ConnectFailed)?;
        // 非阻塞：process_rx 轮询语义（无帧 → Ok(None)）。
        stream
            .set_nonblocking(true)
            .map_err(|_| MasterError::ConnectFailed)?;
        self.stream = Some(stream);
        self.pending.clear();
        Ok(1)
    }

    fn send(&mut self, _conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        self.stream
            .as_mut()
            .ok_or(MasterError::NotConnected)?
            .write_all(data)
            .map_err(|_| MasterError::SendFailed)
    }

    fn recv(&mut self, _conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        // 先消化粘包缓冲（上一轮 read 剩余整帧）。
        if let Some(f) = take_frame(&mut self.pending) {
            return Ok(Some(f));
        }
        let mut buf = [0u8; 2048];
        match self
            .stream
            .as_mut()
            .ok_or(MasterError::NotConnected)?
            .read(&mut buf)
        {
            Ok(0) => Ok(None), // EOF（对端关闭）——poll 语义无帧
            Ok(n) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(take_frame(&mut self.pending))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(_) => Err(MasterError::RecvFailed),
        }
    }

    fn close(&mut self, _conn: ConnId) -> Result<(), MasterError> {
        self.stream = None;
        self.pending.clear();
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

// ===========================================================================
// 对端：手写 APDU 假从站（零 crate 编解码共享——46 fixture 纪律）
// ===========================================================================

/// 手写 APDU 帧构造（IEC 60870-5-104 规范字节序）。
mod wire {
    /// U 格式：68 04 func 00 00 00。
    pub fn u_frame(func: u8) -> [u8; 6] {
        [0x68, 0x04, func, 0, 0, 0]
    }

    /// I 格式：68 len [N(S)<<1 (2B LE)] [N(R)<<1 (2B LE)] asdu。
    pub fn i_frame(ssn: u16, rsn: u16, asdu: &[u8]) -> Vec<u8> {
        let mut f = vec![0x68u8, (4 + asdu.len()) as u8];
        f.push(((ssn << 1) & 0xFF) as u8);
        f.push((((ssn << 1) >> 8) & 0x7F) as u8);
        f.push(((rsn << 1) & 0xFF) as u8);
        f.push((((rsn << 1) >> 8) & 0x7F) as u8);
        f.extend_from_slice(asdu);
        f
    }

    /// CP56Time2a 7 字节（ minute|IV / hour / day / month / year + 毫秒
    /// 16B LE = second×1000+ms）。
    #[allow(clippy::too_many_arguments)] // 时标字段 = 协议格式固有
    pub fn cp56(
        second: u32,
        ms: u16,
        minute: u8,
        hour: u8,
        day: u8,
        month: u8,
        year: u8,
        iv: bool,
    ) -> [u8; 7] {
        let total = (second as u16) * 1000 + ms;
        [
            (total & 0xFF) as u8,
            (total >> 8) as u8,
            minute | if iv { 0x40 } else { 0x00 },
            hour,
            day,
            month,
            year,
        ]
    }

    /// TI=36（M_ME_TD_1 短浮点带 CP56 时标）：type VSQ COT OA CA IOA(3B)
    /// f32(4B LE) QDS CP56(7B)。
    pub fn asdu_float_time_tagged(
        ca: u16,
        ioa: u32,
        v: f32,
        qds: u8,
        cot: u8,
        tag: [u8; 7],
    ) -> Vec<u8> {
        let mut a = vec![36u8, 1, cot, 0];
        a.extend_from_slice(&ca.to_le_bytes());
        a.extend_from_slice(&ioa.to_le_bytes()[..3]);
        a.extend_from_slice(&v.to_le_bytes());
        a.push(qds);
        a.extend_from_slice(&tag);
        a
    }

    /// TI=13（M_ME_NC 短浮点无时标）：同上但无 CP56。
    pub fn asdu_float(ca: u16, ioa: u32, v: f32, qds: u8, cot: u8) -> Vec<u8> {
        let mut a = vec![13u8, 1, cot, 0];
        a.extend_from_slice(&ca.to_le_bytes());
        a.extend_from_slice(&ioa.to_le_bytes()[..3]);
        a.extend_from_slice(&v.to_le_bytes());
        a.push(qds);
        a
    }

    /// 总召唤激活确认（TI=100 COT=7 QOI=20）。
    pub fn asdu_interrogation_con(ca: u16) -> Vec<u8> {
        let mut a = vec![100u8, 1, 7, 0];
        a.extend_from_slice(&ca.to_le_bytes());
        a.extend_from_slice(&[0x00, 0x00, 0x00, 20]);
        a
    }
}

/// 一个会话的响应脚本（STARTDT_CON + 总召确认 + 数据帧序列）。
struct SessionScript {
    /// ioa=100 的值（会话 2 用不同值区分重连前后）。
    ioa100_value: f32,
    /// 发送后是否主动断开（模拟对端断连——驱动重连路径）。
    close_after: bool,
}

/// 从 TCP 流读一帧 APDU（阻塞 read_exact——对端节奏由主站驱动）。
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

/// 假从站：服务 `sessions` 个顺序会话。每会话：accept → STARTDT_CON →
/// 收总召（TI=100 COT=6）→ 回激活确认 + 数据突发（见 burst）→ 按脚本
/// 断开。经 mpsc 上报已服务会话数（供重连同步）。
///
/// listener 在调用线程 bind（消除 accept 前竞态——loopback.rs 同纪律），
/// 返回端口与已服务会话接收端。
fn spawn_slave_stub(sessions: u32) -> (u16, mpsc::Receiver<u32>) {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for session in 0..sessions {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            // 每会话独立 N(S) 序列——主站重连后新连接期望从 0 重新开始。
            let mut ssn = 0u16;
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout");
            let script = if session == 0 {
                SessionScript {
                    ioa100_value: 50.5,
                    close_after: true,
                }
            } else {
                SessionScript {
                    ioa100_value: 88.25,
                    close_after: false,
                }
            };
            // STARTDT 握手：收 STARTDT_ACT（68 04 07 …）→ 回 STARTDT_CON。
            while let Some(frame) = read_frame(&mut stream) {
                if frame.len() == 6 && frame[2] == 0x07 {
                    let _ = stream.write_all(&wire::u_frame(0x0B));
                    break;
                }
            }
            // 总召 + 数据突发。
            while let Some(frame) = read_frame(&mut stream) {
                if frame.len() < 6 || (frame[2] & 0x03) != 0x00 {
                    continue; // S/U 帧：忽略（主站确认/保活）
                }
                let asdu = &frame[6..];
                if asdu.len() >= 10 && asdu[0] == 100 && asdu[2] == 6 {
                    // 总召激活（TI=100 COT=6）→ 确认 + 数据突发。
                    let mut out: Vec<u8> = Vec::new();
                    out.extend_from_slice(&wire::i_frame(ssn, 1, &wire::asdu_interrogation_con(1)));
                    ssn += 1;
                    // TI=36 可信时标（2026-08-30 12:00:00.500，iv=0）。
                    out.extend_from_slice(&wire::i_frame(
                        ssn,
                        1,
                        &wire::asdu_float_time_tagged(
                            1,
                            100,
                            script.ioa100_value,
                            0x00,
                            20,
                            wire::cp56(0, 500, 0, 12, 30, 8, 26, false),
                        ),
                    ));
                    ssn += 1;
                    // TI=36 IV=true 变体（时钟未同步）。
                    out.extend_from_slice(&wire::i_frame(
                        ssn,
                        1,
                        &wire::asdu_float_time_tagged(
                            1,
                            101,
                            61.5,
                            0x00,
                            20,
                            wire::cp56(0, 500, 0, 12, 30, 8, 26, true),
                        ),
                    ));
                    ssn += 1;
                    // TI=13 品质 invalid（QDS bit4=IV）自发（COT=3）。
                    out.extend_from_slice(&wire::i_frame(
                        ssn,
                        1,
                        &wire::asdu_float(1, 102, 7.25, 0x10, 3),
                    ));
                    ssn += 1;
                    // TI=36 ioa=103（13:00:01.250）。
                    let dup = wire::asdu_float_time_tagged(
                        1,
                        103,
                        9.75,
                        0x00,
                        20,
                        wire::cp56(1, 250, 0, 13, 30, 8, 26, false),
                    );
                    out.extend_from_slice(&wire::i_frame(ssn, 1, &dup));
                    ssn += 1;
                    // 重复帧：同 ASDU（同内容）以**新 N(S)** 协议合法重发
                    // ——主站按内容/时标去重（非按 N(S) 重传）。
                    out.extend_from_slice(&wire::i_frame(ssn, 1, &dup));
                    let _ = ssn; // 帧已发出——ssn 断言留待后续帧使用
                    let _ = stream.write_all(&out);
                    break;
                }
            }
            if script.close_after {
                // 对端主动断连（驱动主站重连路径）。
                drop(stream);
            } else {
                // 末会话：留连接等主站收尾。
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = tx.send(session + 1);
        }
    });
    (port, rx)
}

fn master_config(port: u16) -> MasterConfig {
    MasterConfig {
        peer_ip: [127, 0, 0, 1],
        peer_port: port,
        poll_interval_ms: 100,
        ..Default::default()
    }
}

/// E5 判据 1：STARTDT + 总召 + 数据突发全走真实 TCP；CP56 时标保留为
/// Unix 纳秒、IV 位保留、品质 invalid 逐位映射、重复 ASDU 协议合法接受。
#[test]
fn e5_host_interop_adapter_data_plane_over_real_tcp() {
    let (port, _rx) = spawn_slave_stub(1);
    let transport = StdTcpTransport::new();
    let mut adapter =
        Iec104Adapter::new(master_config(port), Box::new(transport)).with_mappings(vec![
            IecPointMapping {
                point_id: 1,
                common_addr: 1,
                ioa: 100,
            },
            IecPointMapping {
                point_id: 2,
                common_addr: 1,
                ioa: 101,
            },
            IecPointMapping {
                point_id: 3,
                common_addr: 1,
                ioa: 102,
            },
            IecPointMapping {
                point_id: 4,
                common_addr: 1,
                ioa: 103,
            },
        ]);
    let cfg = AdapterConfig {
        name: "e5-host".to_string(),
        protocol_type: ProtocolType::Iec104,
        device_configs: vec![DeviceConfig {
            device_id: 1,
            name: "peer-1".to_string(),
            address: ProtocolAddress::Iec104 {
                common_addr: 1,
                ioa: 0,
                type_id: 13,
            },
        }],
    };
    adapter.init(&cfg).expect("init");
    adapter
        .start()
        .expect("start（真实 TCP connect + STARTDT_ACT）");

    // 驱动状态机至数据面填充（总召调度 100ms 周期 + 真实 RTT）。
    let mut got = 0usize;
    for i in 0..600u32 {
        let _ = adapter.poll(u64::from(i) * 20);
        got = adapter.master().latest_values().count();
        if got >= 4 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        got >= 4,
        "总召后数据面须填充 4 点（实际 {got}；stats={:?}）",
        adapter.master().stats()
    );
    // 继续驱动若干轮排空突发尾帧（重复帧为突发最后一帧）。
    for i in 600..660u32 {
        let _ = adapter.poll(u64::from(i) * 20);
        thread::sleep(Duration::from_millis(5));
    }

    // 判据：CP56Time2a → Unix 纳秒（2026-08-30 12:00:00.500Z 手算常量）。
    let lv100 = adapter
        .master()
        .latest_value(1, 100)
        .expect("ioa=100 未入数据面");
    assert_eq!(
        lv100.time_tag_ns,
        Some(1_788_091_200_500_000_000),
        "CP56 时标必须保留为 Unix 纳秒"
    );
    assert!(!lv100.time_tag_iv, "可信时标 IV=false");
    assert!(matches!(lv100.value, eneros_iec104_master::IoValue::Float(v) if v == 50.5));
    assert!(!lv100.quality.invalid, "QDS=0x00 → 全良好");

    // 判据：IV=true 变体——时标保留且时钟未同步位可见。
    let lv101 = adapter
        .master()
        .latest_value(1, 101)
        .expect("ioa=101 未入数据面");
    assert!(lv101.time_tag_ns.is_some(), "IV 帧仍带时标");
    assert!(lv101.time_tag_iv, "IV 位必须保留（时钟质量）");

    // 判据：品质 invalid 帧逐位可见。
    let lv102 = adapter
        .master()
        .latest_value(1, 102)
        .expect("ioa=102 未入数据面");
    assert!(lv102.quality.invalid, "QDS=0x10 → invalid");
    assert_eq!(lv102.time_tag_ns, None, "TI=13 无时标 → None");

    // 判据：重复 ASDU（同内容新 N(S)）协议合法接受，数据面内容一致，
    // 零协议违规（重放拒绝路径不误伤）。
    let lv103 = adapter
        .master()
        .latest_value(1, 103)
        .expect("ioa=103 未入数据面");
    assert!(matches!(lv103.value, eneros_iec104_master::IoValue::Float(v) if v == 9.75));
    assert_eq!(
        adapter.master().stats().rx_error_count,
        0,
        "重复帧以新 N(S) 发送——协议合法，零违规"
    );
    assert!(
        adapter.master().stats().rx_point_count >= 5,
        "5 个数据 IO（含重复帧）全部入数据面"
    );
}

/// E5 判据 2：断连重连——对端关闭会话后主站经真实 TCP 重建连接
/// （accept 新连接），新会话数据可见（connection_epoch 语义的传输面证明；
/// adapter Err→Ok 计数折进 module.note_reconnect 由 iec104_ingest 覆盖）。
#[test]
fn e5_host_reconnect_after_peer_close() {
    let (port, rx) = spawn_slave_stub(2);
    let transport = StdTcpTransport::new();
    let mut master = Iec104Master::new(master_config(port), Box::new(transport));
    let dev = RemoteDevice::new([127, 0, 0, 1], port, 1, 100);
    master.connect(&dev).expect("session-1 connect");

    // 会话 1：驱动至数据面填充。
    let mut ok1 = false;
    for i in 0..600u32 {
        master.poll(u64::from(i) * 20);
        if master.latest_value(1, 100).is_some() {
            ok1 = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ok1, "会话 1 数据面须填充");
    // 对端断连（stub close_after → EOF），等 accept 回位。
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(1));
    thread::sleep(Duration::from_millis(50));

    // 会话 2：重建连接（真实 TCP accept 新连接）→ 新数据可见。
    master.connect(&dev).expect("session-2 connect");
    let mut ok2 = false;
    for i in 600..1200u32 {
        master.poll(u64::from(i) * 20);
        if let Some(lv) = master.latest_value(1, 100) {
            if matches!(lv.value, eneros_iec104_master::IoValue::Float(v) if v == 88.25) {
                ok2 = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ok2,
        "重连后新会话数据（88.25）必须可见；stats={:?}",
        master.stats()
    );
    assert_eq!(
        master.device_state(1),
        Some(ConnState::Connected),
        "重连后须 Connected"
    );
    assert_eq!(master.stats().connect_count, 2, "两次真实 TCP 连接建立");
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(2));
}
