//! 蓝图 68.5 WP-68.5.1：最小 MQTT 3.1.1 客户端（host std；planner-peer
//! 传输面）。
//!
//! 覆盖面（对真 mosquitto 与 evidence/50 python 假 broker 同话）：CONNECT/
//! CONNACK、SUBSCRIBE/SUBACK、PUBLISH QoS0/1 + PUBACK、PINGREQ/PINGRESP、
//! DISCONNECT。读路径 [`MqttClient::poll`] 单包粒度——bin 循环按 keepalive
//! 节拍驱动；**单在途槽**（F27 第 11 项：逐请求应答，不并发）由 bin 的
//! 单线程循环承载，本模块不引入线程/缓冲扩展。
//!
//! QoS1 出站等待 PUBACK（bounded 重读）；inbound QoS1 PUBLISH 立即回
//! PUBACK（broker 重传消除）。会话语义 = clean session（seq 单调由服务
//! 侧状态文件承载，见 bin）。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

/// 连接/协议错误。
#[derive(Debug, Clone, PartialEq)]
pub struct MqttError(pub String);

impl std::fmt::Display for MqttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MqttError {}

const CONNECT: u8 = 1;
const CONNACK: u8 = 2;
const PUBLISH: u8 = 3;
const PUBACK: u8 = 4;
const SUBSCRIBE: u8 = 8;
const SUBACK: u8 = 9;
const PINGREQ: u8 = 12;
const PINGRESP: u8 = 13;
const DISCONNECT: u8 = 14;

fn err<T>(msg: impl Into<String>) -> Result<T, MqttError> {
    Err(MqttError(msg.into()))
}

fn encode_len(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut d = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            d |= 0x80;
        }
        out.push(d);
        if n == 0 {
            return out;
        }
    }
}

fn write_utf(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// inbound PUBLISH（已回 QoS1 PUBACK）。
#[derive(Debug, Clone)]
pub struct InboundPublish {
    pub topic: String,
    pub payload: Vec<u8>,
}

pub struct MqttClient {
    stream: TcpStream,
    /// 最近入站活动（keepalive 判据；bin 按 ms 供给）。
    last_activity_ms: u64,
}

impl MqttClient {
    /// 建联 + CONNECT/CONNACK（clean session；keep_alive_secs 上限 65535）。
    pub fn connect(
        host: &str,
        client_id: &str,
        keep_alive_secs: u16,
        timeout: Duration,
    ) -> Result<Self, MqttError> {
        let stream = TcpStream::connect(host).map_err(|e| MqttError(format!("tcp: {e}")))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| MqttError(format!("opt: {e}")))?;
        let mut body = Vec::new();
        write_utf(&mut body, client_id);
        // flags: clean session = 0x02；无用户名/口令/will（peer 通道匿名，
        // broker 侧访问控制归部署面）。
        body.push(0x02);
        body.extend_from_slice(&keep_alive_secs.to_be_bytes());
        let mut pkt = vec![CONNECT << 4]; // flags 0（无 will/用户名）
        pkt.extend_from_slice(&encode_len(body.len()));
        pkt.extend_from_slice(&body);
        let mut c = Self {
            stream,
            last_activity_ms: 0,
        };
        c.write_raw(&pkt)?;
        let (hdr, _body) = c.read_packet()?;
        if hdr >> 4 != CONNACK {
            return err(format!("期望 CONNACK 得 {:#x}", hdr >> 4));
        }
        Ok(c)
    }

    /// 订阅（QoS 上限 1；等待 SUBACK）。
    pub fn subscribe(&mut self, topic: &str, qos: u8) -> Result<(), MqttError> {
        let mut body = Vec::new();
        let pid = 1u16; // 单在途槽：包 id 常量即可（同步等 SUBACK）
        body.extend_from_slice(&pid.to_be_bytes());
        write_utf(&mut body, topic);
        body.push(qos.min(1));
        let mut pkt = vec![(SUBSCRIBE << 4) | 0x2];
        pkt.extend_from_slice(&encode_len(body.len()));
        pkt.extend_from_slice(&body);
        self.write_raw(&pkt)?;
        let (hdr, body) = self.read_packet()?;
        if hdr >> 4 != SUBACK {
            return err(format!("期望 SUBACK 得 {:#x}", hdr >> 4));
        }
        if body.len() < 3 {
            return err(String::from("SUBACK 短帧"));
        }
        if body[2] == 0x80 {
            return err(String::from("SUBACK 拒绝"));
        }
        Ok(())
    }

    /// 发布（QoS1：同步等 PUBACK；QoS0：即返）。
    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: u8) -> Result<(), MqttError> {
        let qos = qos.min(1);
        let mut body = Vec::new();
        write_utf(&mut body, topic);
        let pid = 1u16;
        if qos > 0 {
            body.extend_from_slice(&pid.to_be_bytes());
        }
        body.extend_from_slice(payload);
        let mut pkt = vec![(PUBLISH << 4) | (qos << 1)];
        pkt.extend_from_slice(&encode_len(body.len()));
        pkt.extend_from_slice(&body);
        self.write_raw(&pkt)?;
        if qos == 0 {
            return Ok(());
        }
        // 单在途：循环读直到 PUBACK（PINGRESP 等旁路包吞掉）。
        for _ in 0..32 {
            let (hdr, b) = self.read_packet()?;
            match hdr >> 4 {
                PUBACK => return Ok(()),
                PINGRESP => continue,
                PUBLISH => {
                    // broker 转发先到：回 PUBACK 后继续等本条 ACK。
                    self.ack_publish(&hdr, &b)?;
                    continue;
                }
                k => return err(format!("PUBACK 等待中得 {k:#x}")),
            }
        }
        err(String::from("PUBACK 超时（32 轮无确认）"))
    }

    /// 读一包；inbound PUBLISH（QoS1）即时回 PUBACK 并上抛内容。
    /// 无数据（读超时）→ Ok(None)。
    pub fn poll(&mut self, now_ms: u64) -> Result<Option<InboundPublish>, MqttError> {
        match self.read_packet() {
            Ok((hdr, body)) => {
                self.last_activity_ms = now_ms;
                match hdr >> 4 {
                    PUBLISH => {
                        let qos = (hdr >> 1) & 0x03;
                        let (topic, payload) = parse_publish(&body, qos)?;
                        if qos > 0 {
                            self.ack_publish(&hdr, &body)?;
                        }
                        Ok(Some(InboundPublish { topic, payload }))
                    }
                    PINGRESP => Ok(None),
                    _ => Ok(None),
                }
            }
            Err(e) if e.0 == "timeout" => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// keepalive（bin 按半 keepalive 节拍调用）。
    pub fn ping(&mut self) -> Result<(), MqttError> {
        self.write_raw(&[PINGREQ << 4, 0x00])
    }

    /// 入站活性打点（连接建立/重连后调用——置零"静默时长"起点；不带此
    /// 打点则构造期 0 值会被看门狗立即判死）。
    pub fn mark_activity(&mut self, now_ms: u64) {
        self.last_activity_ms = now_ms;
    }

    /// 距最近入站（PINGRESP/数据帧）的静默毫秒数。68.5 复查修复：host
    /// 侧"半死 broker"盲区——TCP 存活但对端永不回包时，poll 恒
    /// Ok(None)、ping 写恒成功（进内核缓冲），连接永不重建；bin 以
    /// "静默 > 1.5×keepalive"判死重连。
    pub fn inbound_silent_for(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_activity_ms)
    }

    /// 优雅断开（对端观测面；bin 循环当前不主动断开——进程退出即断，
    /// broker 侧 clean session 清扫。测试消费）。
    #[allow(dead_code)]
    pub fn disconnect(&mut self) {
        let _ = self.write_raw(&[DISCONNECT << 4, 0x00]);
    }

    fn ack_publish(&mut self, hdr: &u8, body: &[u8]) -> Result<(), MqttError> {
        let qos = (hdr >> 1) & 0x03;
        if qos == 0 {
            return Ok(());
        }
        if body.len() < 4 {
            return err(String::from("PUBLISH 短帧（无 packet id）"));
        }
        let mut pkt = vec![PUBACK << 4, 0x02];
        pkt.extend_from_slice(&body[2..4]);
        self.write_raw(&pkt)
    }

    /// 入站帧上限（防御不受信 broker 的 varint 超大剩余长度；协议请求
    /// ≤1024B + 主题，1 MiB 量级余量）。
    const MAX_INBOUND_FRAME: usize = 1024 * 1024;

    fn write_raw(&mut self, pkt: &[u8]) -> Result<(), MqttError> {
        self.stream
            .write_all(pkt)
            .map_err(|e| MqttError(format!("write: {e}")))?;
        self.stream
            .flush()
            .map_err(|e| MqttError(format!("flush: {e}")))
    }

    /// 读一完整包（fixed header + varint 剩余长度 + body）。
    /// 读超时（无入站数据）→ `Err(MqttError(TIMEOUT_SENTINEL))`。
    fn read_packet(&mut self) -> Result<(u8, Vec<u8>), MqttError> {
        const TIMEOUT_SENTINEL: &str = "timeout";
        let map = |e: std::io::Error| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) {
                MqttError(String::from(TIMEOUT_SENTINEL))
            } else {
                MqttError(format!("read: {e}"))
            }
        };
        let mut hdr = [0u8; 1];
        self.stream.read_exact(&mut hdr).map_err(map)?;
        let mut rem = 0usize;
        let mut mult = 1usize;
        loop {
            let mut b = [0u8; 1];
            self.stream.read_exact(&mut b).map_err(map)?;
            rem += (b[0] & 0x7F) as usize * mult;
            if b[0] & 0x80 == 0 {
                break;
            }
            mult *= 128;
            if mult > 128 * 128 * 128 * 128 {
                return err(String::from("varint 溢出"));
            }
        }
        // 68 全卷复查修复：剩余长度上限（不受信 broker 防御）——MQTT
        // 规范外 4 字节 varint（0xFF×4）现解码接受，rem 可达 ~539 MiB →
        // 分配 OOM；本客户端协议帧 ~1 KiB 级，1 MiB 上限远宽且全防恶意
        // 帧（5 字节形由上方 mult 检查拦，4 字节形由本检查拦）。
        if rem > Self::MAX_INBOUND_FRAME {
            return err(String::from("剩余长度超限"));
        }
        let mut body = vec![0u8; rem];
        if rem > 0 {
            self.stream.read_exact(&mut body).map_err(map)?;
        }
        Ok((hdr[0], body))
    }
}

fn parse_publish(body: &[u8], qos: u8) -> Result<(String, Vec<u8>), MqttError> {
    if body.len() < 2 {
        return err(String::from("PUBLISH 无主题"));
    }
    let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
    if body.len() < 2 + tlen {
        return err(String::from("PUBLISH 主题越界"));
    }
    let topic = String::from_utf8_lossy(&body[2..2 + tlen]).into_owned();
    let mut off = 2 + tlen;
    if qos > 0 {
        off += 2; // packet id（ack 用 body 原始切片，这里跳过）
    }
    let payload = body.get(off..).unwrap_or(&[]).to_vec();
    Ok((topic, payload))
}

// ============================================================================
// host 测试（对假 broker 的最小协议往返）
// ============================================================================

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// 进程内假 broker（测试专用）：接受一客户端，按脚本应答。
    /// CONNECT→CONNACK；SUBSCRIBE→SUBACK；首条 PUBLISH→原样路由回；
    /// PINGREQ→PINGRESP。
    fn fake_broker(script_route: bool) -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut seen = Vec::new();
            loop {
                let mut hdr = [0u8; 1];
                if sock.read_exact(&mut hdr).is_err() {
                    break;
                }
                let kind = hdr[0] >> 4;
                let mut rem = 0usize;
                let mut mult = 1usize;
                loop {
                    let mut b = [0u8; 1];
                    if sock.read_exact(&mut b).is_err() {
                        return;
                    }
                    rem += (b[0] & 0x7F) as usize * mult;
                    if b[0] & 0x80 == 0 {
                        break;
                    }
                    mult *= 128;
                }
                let mut body = vec![0u8; rem];
                if rem > 0 && sock.read_exact(&mut body).is_err() {
                    return;
                }
                match kind {
                    CONNECT => {
                        let _ = sock.write_all(&[0x20, 0x02, 0x00, 0x00]);
                    }
                    SUBSCRIBE => {
                        let mut resp = vec![0x90];
                        resp.extend_from_slice(&encode_len(body.len()));
                        resp.extend_from_slice(&body);
                        let _ = sock.write_all(&resp);
                    }
                    PUBLISH => {
                        seen.push(body.clone());
                        let _ = tx.send(body.clone());
                        if script_route {
                            // 原样路由回（QoS 位保留）。
                            let mut pkt = vec![hdr[0]];
                            pkt.extend_from_slice(&encode_len(body.len()));
                            pkt.extend_from_slice(&body);
                            let _ = sock.write_all(&pkt);
                        }
                        // PUBACK 给发布方（QoS1）。
                        if (hdr[0] >> 1) & 0x03 > 0 && body.len() >= 4 {
                            let mut ack = vec![PUBACK << 4, 0x02];
                            ack.extend_from_slice(&body[2..4]);
                            let _ = sock.write_all(&ack);
                        }
                    }
                    PINGREQ => {
                        let _ = sock.write_all(&[0xD0, 0x00]);
                    }
                    _ => break,
                }
            }
        });
        (addr, rx)
    }

    #[test]
    fn connect_subscribe_ping_roundtrip() {
        let (addr, _rx) = fake_broker(false);
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_secs(2))
            .expect("connect");
        c.subscribe("t/req", 1).expect("subscribe");
        c.ping().expect("ping");
        c.disconnect();
    }

    #[test]
    fn publish_qos1_and_inbound_route() {
        let (addr, rx) = fake_broker(true);
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_secs(2))
            .expect("connect");
        c.subscribe("t/req", 1).expect("subscribe");
        c.publish("t/resp", br#"{"v":1}"#, 1).expect("publish qos1");
        // broker 原样路由回一条（订阅通配不在假件里——直接收到路由帧）。
        let got = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("broker saw publish");
        assert!(got.windows(6).any(|w| w == b"t/resp"));
        c.disconnect();
    }

    /// 读超时 = Ok(None)（bin 循环的静默 tick 语义）。
    #[test]
    fn poll_timeout_is_none() {
        let (addr, _rx) = fake_broker(false);
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_millis(200))
            .expect("connect");
        let out = c.poll(0).expect("poll");
        assert!(out.is_none());
        c.disconnect();
    }

    /// 68 全卷复查修复：不受信 broker 的 varint 超大剩余长度（规范外
    /// 4 字节 0xFF×4 → rem≈539 MiB）须在分配前拒绝——OOM 面防御。
    /// 变异靶：去掉 MAX_INBOUND_FRAME 检查 → 本测真分配 539 MiB 致进程
    /// OOM 崩溃（红）。
    #[test]
    #[allow(clippy::disallowed_macros)] // 仓例先例（68.2 卡 D.6）：负例断言用 panic!
    fn oversized_varint_rejected_before_alloc() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().unwrap().to_string();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().expect("accept");
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf).unwrap(); // CONNECT
            s.write_all(&[0x20, 0x02, 0x00, 0x00]).unwrap(); // CONNACK ok
            let _ = s.read(&mut buf).unwrap(); // SUBSCRIBE
            s.write_all(&[0x90, 0x03, 0x00, 0x01, 0x00]).unwrap(); // SUBACK pid=1
                                                                   // 恶意帧：PINGRESP 头 + 4 字节 varint 顶格形 0xFF×3+0x7F
                                                                   //（≈171 MiB——第 4 字节无 continuation 位，解码器在 4 字节停，
                                                                   // 由剩余长度上限检查拦；0xFF×4 续位会读第 5 字节属另一分支）。
            s.write_all(&[0xD0, 0xFF, 0xFF, 0xFF, 0x7F]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_millis(500))
            .expect("connect");
        c.subscribe("t/x", 1).expect("subscribe");
        std::thread::sleep(std::time::Duration::from_millis(150));
        match c.poll(0) {
            Err(e) => assert!(e.0.contains("剩余长度超限"), "须分配前拒恶意帧: {e:?}"),
            other => panic!("超大剩余长度帧必须拒绝，got {other:?}"),
        }
        h.join().unwrap();
    }

    /// 68.5 复查：入站静默看门狗判据——poll 读到入站即刷新活性；
    /// mark_activity 重置起点（重连后不误判）。变异靶：poll 去掉
    /// last_activity_ms 刷新 → 本测红。
    #[test]
    fn inbound_silence_watchdog_semantics() {
        let (addr, _rx) = fake_broker(true);
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_millis(200))
            .expect("connect");
        // 连接期零入站（mark 前）：静默 = now - 0（起点未置）——bin 在
        // CONNACK/SUBACK 后 mark_activity 打点重置。
        assert_eq!(c.inbound_silent_for(10_000), 10_000);
        c.mark_activity(10_000);
        assert_eq!(c.inbound_silent_for(10_500), 500);
        // QoS0 publish（假 broker 原样路由回，无 PUBACK 消费）→ poll 读到
        // 入站帧 → 活性刷新。
        c.publish("t/resp", b"x", 0).expect("publish qos0");
        assert!(c.poll(11_000).expect("poll inbound").is_some());
        assert_eq!(c.inbound_silent_for(11_500), 500, "入站须刷新活性");
        c.disconnect();
    }
}
