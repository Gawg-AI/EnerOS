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

/// 读超时哨兵（`poll` 以此区分"无数据"与真错误；69.2 提为具名常量——
/// 字面值是行为锁，不得改动）。
pub(crate) const TIMEOUT_SENTINEL: &str = "timeout";

/// 传输面抽象（69.2 WP-69.2.0）。
///
/// codebase-design 判据："One adapter means a hypothetical seam. Two
/// adapters means a real one." 本 trait 立即有 **2 个真实 adapter**：
/// `TcpStream`（生产）与 `LoopbackTransport`（session.rs 测试，内存双端
/// 队列）⇒ 真缝，非假想缝；且它是唯一能让会话策略（看门狗/keepalive/
/// 重连）获得测试覆盖的手段——原 55 行主循环零测试。
pub trait PeerTransport {
    /// 精确读满 `buf`。读超时（无入站数据）→ `MqttError(TIMEOUT_SENTINEL)`；
    /// EOF/其他 IO 错 → 描述性错误。
    fn read_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), MqttError>;
    fn write_all_bytes(&mut self, buf: &[u8]) -> Result<(), MqttError>;
    fn flush_bytes(&mut self) -> Result<(), MqttError>;
    fn set_read_timeout(&mut self, d: Duration) -> Result<(), MqttError>;
}

impl PeerTransport for TcpStream {
    fn read_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), MqttError> {
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
        Read::read_exact(self, buf).map_err(map)
    }

    fn write_all_bytes(&mut self, buf: &[u8]) -> Result<(), MqttError> {
        Write::write_all(self, buf).map_err(|e| MqttError(format!("write: {e}")))
    }

    fn flush_bytes(&mut self) -> Result<(), MqttError> {
        Write::flush(self).map_err(|e| MqttError(format!("flush: {e}")))
    }

    fn set_read_timeout(&mut self, d: Duration) -> Result<(), MqttError> {
        TcpStream::set_read_timeout(self, Some(d)).map_err(|e| MqttError(format!("opt: {e}")))
    }
}

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

/// 等待 PUBACK 期间入站 PUBLISH 的有界保留队列上限（蓝图 70.4
/// WP-70.4.3；单请求 ≤1024B → 上限内内存有界 ≈ 16KiB）。
pub(crate) const PENDING_INBOUND_CAP: usize = 16;

pub struct MqttClient<T: PeerTransport = TcpStream> {
    stream: T,
    /// 最近入站活动（keepalive 判据；bin 按 ms 供给）。
    last_activity_ms: u64,
    /// 70.4（WP-70.4.3）：等待 PUBACK 期间接到的 PUBLISH（已回 QoS1
    /// PUBACK）经此有界队列保留，业务层经 [`Self::poll`] 依次取得——
    /// 不再"ACK 后丢弃"。满队：QoS1 不回 PUBACK（broker 按重传策略
    /// 重投 = 显式不接纳，不静默丢弃）；QoS0 只能丢弃（无重投语义）
    /// 且 [`Self::pending_dropped`] 计数可见。
    pending_inbound: std::vec::Vec<InboundPublish>,
    /// QoS0 交错 PUBLISH 满队丢弃计数（观测面）。
    pending_dropped: u32,
    /// 出站包 id 计数器（70.4：PUBACK/SUBACK 必须核对对应 pid——
    /// 单在途槽下陈旧 ACK 不得完结当前请求）。
    next_pid: u16,
}

impl<T: PeerTransport> MqttClient<T> {
    fn allocate_pid(&mut self) -> u16 {
        let pid = self.next_pid;
        self.next_pid = if self.next_pid == u16::MAX {
            1
        } else {
            self.next_pid + 1
        };
        pid
    }

    /// 满队丢弃计数（观测面；QoS0 交错帧无法重投只能可见地丢）。
    /// host 会话测试消费（session::tests 满队合同）。
    #[allow(dead_code)]
    pub fn pending_dropped(&self) -> u32 {
        self.pending_dropped
    }

    /// 入队一条交错 PUBLISH；满队时 QoS1 返回 false（调用方不回 PUBACK
    /// → broker 重投），QoS0 计数后丢弃。
    fn queue_inbound(&mut self, pub_: InboundPublish, qos: u8) -> bool {
        if self.pending_inbound.len() < PENDING_INBOUND_CAP {
            self.pending_inbound.push(pub_);
            return true;
        }
        if qos == 0 {
            self.pending_dropped = self.pending_dropped.saturating_add(1);
        }
        false
    }
}

impl MqttClient<TcpStream> {
    /// 建联 + CONNECT/CONNACK（clean session；keep_alive_secs 上限 65535）。
    pub fn connect(
        host: &str,
        client_id: &str,
        keep_alive_secs: u16,
        timeout: Duration,
    ) -> Result<Self, MqttError> {
        let stream = TcpStream::connect(host).map_err(|e| MqttError(format!("tcp: {e}")))?;
        Self::over(stream, client_id, keep_alive_secs, timeout)
    }
}

impl<T: PeerTransport> MqttClient<T> {
    /// 在既有传输上完成 CONNECT/CONNACK 握手（TCP 生产路径与测试假件
    /// 共用；69.2 WP-69.2.0 泛型化——协议/会话语义零变更）。
    pub fn over(
        mut transport: T,
        client_id: &str,
        keep_alive_secs: u16,
        timeout: Duration,
    ) -> Result<Self, MqttError> {
        transport.set_read_timeout(timeout)?;
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
            stream: transport,
            last_activity_ms: 0,
            pending_inbound: std::vec::Vec::new(),
            pending_dropped: 0,
            next_pid: 1,
        };
        c.write_raw(&pkt)?;
        let (hdr, _body) = c.read_packet()?;
        if hdr >> 4 != CONNACK {
            return err(format!("期望 CONNACK 得 {:#x}", hdr >> 4));
        }
        Ok(c)
    }

    /// 订阅（QoS 上限 1；等待 SUBACK）。
    ///
    /// 70.4（WP-70.4.3）：SUBACK 必须回带本次 SUBSCRIBE 的 pid（失配 =
    /// 陈旧/错配 ACK，不完结本次订阅）；等待期间到达的 PUBLISH 照
    /// [`Self::publish`] 同律入有界队列保留。
    pub fn subscribe(&mut self, topic: &str, qos: u8) -> Result<(), MqttError> {
        let mut body = Vec::new();
        let pid = self.allocate_pid();
        body.extend_from_slice(&pid.to_be_bytes());
        write_utf(&mut body, topic);
        body.push(qos.min(1));
        let mut pkt = vec![(SUBSCRIBE << 4) | 0x2];
        pkt.extend_from_slice(&encode_len(body.len()));
        pkt.extend_from_slice(&body);
        self.write_raw(&pkt)?;
        // 单在途：循环读直到本 pid 的 SUBACK（旁路包按律处置）。
        for _ in 0..32 {
            let (hdr, body) = self.read_packet()?;
            match hdr >> 4 {
                SUBACK => {
                    if body.len() < 3 {
                        return err(String::from("SUBACK 短帧"));
                    }
                    let acked = u16::from_be_bytes([body[0], body[1]]);
                    if acked != pid {
                        continue; // 陈旧 SUBACK：不完结本次订阅
                    }
                    if body[2] == 0x80 {
                        return err(String::from("SUBACK 拒绝"));
                    }
                    return Ok(());
                }
                PINGRESP => continue,
                PUBLISH => {
                    self.stash_inbound(&hdr, &body)?;
                    continue;
                }
                k => return err(format!("SUBACK 等待中得 {k:#x}")),
            }
        }
        err(String::from("SUBACK 超时（32 轮无确认）"))
    }

    /// 解析并保留一条交错 PUBLISH（publish/subscribe 等待环共用）：
    /// 有界队列未满 → QoS1 即回 PUBACK 后入队；满队 → QoS1 **不回
    /// PUBACK**（broker 重投 = 显式不接纳）；QoS0 计数丢弃（无重投）。
    fn stash_inbound(&mut self, hdr: &u8, body: &[u8]) -> Result<(), MqttError> {
        let qos = (hdr >> 1) & 0x03;
        let (topic, payload) = parse_publish(body, qos)?;
        let retained = {
            let pub_ = InboundPublish {
                topic,
                payload: payload.clone(),
            };
            self.queue_inbound(pub_, qos)
        };
        if !retained {
            // 满队：QoS1 不 ACK（重投）；QoS0 已计数（queue_inbound 内）。
            return Ok(());
        }
        if qos > 0 {
            self.ack_publish(hdr, body)?;
        }
        Ok(())
    }

    /// 发布（QoS1：同步等 PUBACK；QoS0：即返）。
    ///
    /// 70.4（WP-70.4.3 / N14 修复）：PUBACK 必须回带本次发布 pid——
    /// 失配 = 陈旧 ACK，继续等待不完结；等待期间到达的 PUBLISH 经有界
    /// 队列保留（满队 QoS1 不 ACK = broker 重投），业务层经 [`Self::poll`]
    /// 取得，不再"确认后丢弃"。
    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: u8) -> Result<(), MqttError> {
        let qos = qos.min(1);
        let mut body = Vec::new();
        write_utf(&mut body, topic);
        let pid = self.allocate_pid();
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
        // 单在途：循环读直到本 pid 的 PUBACK（旁路包按律处置）。
        for _ in 0..32 {
            let (hdr, b) = self.read_packet()?;
            match hdr >> 4 {
                PUBACK => {
                    if b.len() >= 2 && u16::from_be_bytes([b[0], b[1]]) == pid {
                        return Ok(());
                    }
                    continue; // 陈旧/错配 PUBACK：不完结本次发布
                }
                PINGRESP => continue,
                PUBLISH => {
                    // broker 转发先到：保留入队（容量允许时回 PUBACK）。
                    self.stash_inbound(&hdr, &b)?;
                    continue;
                }
                k => return err(format!("PUBACK 等待中得 {k:#x}")),
            }
        }
        err(String::from("PUBACK 超时（32 轮无确认）"))
    }

    /// 读一包；inbound PUBLISH（QoS1）即时回 PUBACK 并上抛内容。
    /// 无数据（读超时）→ Ok(None)。
    ///
    /// 70.4：等待 PUBACK 期间保留的交错 PUBLISH 优先交付（FIFO）——
    /// 业务层仍经本方法取得，交付语义与直读一致。
    pub fn poll(&mut self, now_ms: u64) -> Result<Option<InboundPublish>, MqttError> {
        if !self.pending_inbound.is_empty() {
            return Ok(Some(self.pending_inbound.remove(0)));
        }
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
            Err(e) if e.0 == TIMEOUT_SENTINEL => Ok(None),
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
        // MQTT 3.1.1 §3.3.1/3.3.2：可变头 = 2B 长度前缀主题 + packet id
        // （QoS>0 时）。68 三轮复查修复：旧实现取 body[2..4]（主题前两
        // 字节）当 pid——真 broker（mosquitto）按 pid 记 inflight，错 pid
        // 使消息永不过窗（max_inflight_messages 默认 20 → 20 条后请求停
        // 发），且 PINGRESP 持续刷新活性令静默看门狗失效。
        if body.len() < 2 {
            return err(String::from("PUBLISH 短帧（无主题）"));
        }
        let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
        let pid_off = 2 + tlen;
        if body.len() < pid_off + 2 {
            return err(String::from("PUBLISH 短帧（无 packet id）"));
        }
        let mut pkt = vec![PUBACK << 4, 0x02];
        pkt.extend_from_slice(&body[pid_off..pid_off + 2]);
        self.write_raw(&pkt)
    }

    /// 入站帧上限（防御不受信 broker 的 varint 超大剩余长度；协议请求
    /// ≤1024B + 主题，1 MiB 量级余量）。
    const MAX_INBOUND_FRAME: usize = 1024 * 1024;

    fn write_raw(&mut self, pkt: &[u8]) -> Result<(), MqttError> {
        self.stream.write_all_bytes(pkt)?;
        self.stream.flush_bytes()
    }

    /// 读一完整包（fixed header + varint 剩余长度 + body）。
    /// 读超时（无入站数据）→ `Err(MqttError(TIMEOUT_SENTINEL))`。
    fn read_packet(&mut self) -> Result<(u8, Vec<u8>), MqttError> {
        let mut hdr = [0u8; 1];
        self.stream.read_exact_bytes(&mut hdr)?;
        let mut rem = 0usize;
        let mut mult = 1usize;
        loop {
            let mut b = [0u8; 1];
            self.stream.read_exact_bytes(&mut b)?;
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
            self.stream.read_exact_bytes(&mut body)?;
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
                        // PUBACK 给发布方（QoS1）——pid 位于 2+主题长度
                        // （与真 broker 同律；ack_publish 修复的同族保真）。
                        if (hdr[0] >> 1) & 0x03 > 0 && body.len() >= 2 {
                            let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
                            let off = 2 + tlen;
                            if body.len() >= off + 2 {
                                let mut ack = vec![PUBACK << 4, 0x02];
                                ack.extend_from_slice(&body[off..off + 2]);
                                let _ = sock.write_all(&ack);
                            }
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

    /// 68 全卷三轮复查（红测先行）：入站 QoS1 PUBLISH 的 PUBACK 必须回显
    /// 真实 packet id（位于 2+主题长度 处——非 body[2..4]）。错 pid 对真
    /// broker = 消息永不过 inflight 窗（mosquitto max_inflight_messages
    /// 默认 20 → 20 条后请求停发），且 PINGRESP 持续刷新活性使入站静默
    /// 看门狗永不触发——host 端 AI 链静默饿死。变异靶：ack_publish 退回
    /// body[2..4] → 本测红（收到 0x656E 而非 0x1234）。
    #[test]
    fn puback_echoes_real_packet_id() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().unwrap().to_string();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().expect("accept");
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf).unwrap(); // CONNECT
            s.write_all(&[0x20, 0x02, 0x00, 0x00]).unwrap(); // CONNACK
            let _ = s.read(&mut buf).unwrap(); // SUBSCRIBE
            s.write_all(&[0x90, 0x03, 0x00, 0x01, 0x01]).unwrap(); // SUBACK pid=1 qos=1
                                                                   // 入站 QoS1 PUBLISH：topic="eneros/brain/request"（tlen=20），
                                                                   // packet id=0x1234（位于 2+20=22 处），payload="{}"。
            let topic = b"eneros/brain/request";
            let mut body = Vec::new();
            body.extend_from_slice(&[0x00, 0x14]);
            body.extend_from_slice(topic);
            body.extend_from_slice(&[0x12, 0x34]);
            body.extend_from_slice(b"{}");
            let mut pkt = vec![0x32]; // PUBLISH QoS1
            pkt.extend_from_slice(&encode_len(body.len()));
            pkt.extend_from_slice(&body);
            s.write_all(&pkt).unwrap();
            // 读客户端应答：须为 PUBACK 且 pid=0x1234。
            s.read_exact(&mut buf[..2]).unwrap(); // fixed header
            assert_eq!(buf[0], 0x40, "须为 PUBACK，got {:#x}", buf[0]);
            assert_eq!(buf[1], 0x02);
            s.read_exact(&mut buf[..2]).unwrap();
            let acked = u16::from_be_bytes([buf[0], buf[1]]);
            assert_eq!(acked, 0x1234, "PUBACK 须回显真实 packet id（非主题字节）");
        });
        let mut c = MqttClient::connect(&addr, "planner-peer-test", 60, Duration::from_millis(500))
            .expect("connect");
        c.subscribe("t/x", 1).expect("subscribe");
        let got = c
            .poll(0)
            .expect("poll inbound")
            .expect("须收到入站 PUBLISH");
        assert_eq!(got.topic, "eneros/brain/request");
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
