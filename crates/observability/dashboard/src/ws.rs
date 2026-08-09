//! 22 §2.W4.5：dashboard 真 WebSocket 服务——RFC 6455 协议实现。
//!
//! 替换 `MockWsServer`（移入 `#[cfg(test)]`）为真 WebSocket：
//!
//! - **握手**：HTTP/1.1 Upgrade 解析 + `Sec-WebSocket-Accept`
//!   （SHA-1 + base64，RFC 6455 §4.2.2；SHA-1 仅用于协议握手，
//!   非安全用途——EnerOS 安全哈希一律 SM3）；
//! - **帧**：server→client 文本帧构造 / client→server 掩码帧解析
//!   （RFC 6455 §5.2；未掩码帧、RSV 非零、超长帧一律拒绝）；
//! - **事件推送**：`DashboardEvent` → JSON 文本帧，经注入发送闭包
//!   写出（真 TCP/MQTT 桥由集成层注入，与 W4.1 同模式）。
//!
//! 浏览器兼容性由 RFC 6455 官方测试向量（key→accept）+ 字节级帧
//! 往返测试保证。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use crate::render::{DashboardEvent, DashboardWsServer};
use crate::types::{EdgeKind, FlowDirection, NodeKind, NodeStatus, PowerFlowSample, TopologyGraph};
use crate::ObsError;

// ---------------------------------------------------------------------------
// opcode（RFC 6455 §5.2）
// ---------------------------------------------------------------------------

/// 文本帧。
pub const OP_TEXT: u8 = 0x1;
/// 二进制帧。
pub const OP_BINARY: u8 = 0x2;
/// 关闭帧。
pub const OP_CLOSE: u8 = 0x8;
/// ping 帧。
pub const OP_PING: u8 = 0x9;
/// pong 帧。
pub const OP_PONG: u8 = 0xA;

/// 单帧负载上限（1 MiB；防超大帧内存炸弹）。
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// WebSocket 握手 GUID（RFC 6455 §1.3）。
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

// ---------------------------------------------------------------------------
// 帧（RFC 6455 §5.2）
// ---------------------------------------------------------------------------

/// 解析后的帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsFrame {
    /// 结束标志。
    pub fin: bool,
    /// 帧类型。
    pub opcode: u8,
    /// 去掩码后的负载。
    pub payload: Vec<u8>,
}

/// 构造一帧（FIN=1 单帧消息；`mask=None` 为 server→client 帧）。
pub fn encode_frame(opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode); // FIN=1, RSV=0
    let len = payload.len() as u64;
    let masked_bit: u8 = if mask.is_some() { 0x80 } else { 0 };
    if len < 126 {
        out.push(masked_bit | len as u8);
    } else if len <= 0xFFFF {
        out.push(masked_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(masked_bit | 127);
        out.extend_from_slice(&len.to_be_bytes());
    }
    if let Some(m) = mask {
        out.extend_from_slice(&m);
        for (i, b) in payload.iter().enumerate() {
            out.push(b ^ m[i % 4]);
        }
    } else {
        out.extend_from_slice(payload);
    }
    out
}

/// 解析一帧（client→server；RFC 6455 §5.1 服务器必须拒绝未掩码帧）。
///
/// 拒绝：RSV 非零、未掩码、长度超 `MAX_FRAME_BYTES`、截断帧、127 长度
/// MSB 非零——全部 `Err`（fail-closed，红线4）。
#[allow(clippy::result_unit_err)]
pub fn parse_frame(buf: &[u8]) -> Result<WsFrame, ()> {
    if buf.len() < 2 {
        return Err(());
    }
    let b0 = buf[0];
    let b1 = buf[1];
    if (b0 & 0x70) != 0 {
        return Err(()); // RSV 必须 0
    }
    let fin = (b0 & 0x80) != 0;
    let opcode = b0 & 0x0F;
    if (b1 & 0x80) == 0 {
        return Err(()); // client→server 必须掩码
    }
    let mut len = (b1 & 0x7F) as u64;
    let mut idx = 2usize;
    if len == 126 {
        if buf.len() < 4 {
            return Err(());
        }
        len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        idx = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Err(());
        }
        if (buf[2] & 0x80) != 0 {
            return Err(()); // 最高位必须 0（RFC 6455 §5.2）
        }
        len = u64::from_be_bytes(buf[2..10].try_into().unwrap());
        idx = 10;
    }
    if len as usize > MAX_FRAME_BYTES {
        return Err(()); // 防内存炸弹
    }
    if buf.len() < idx + 4 + len as usize {
        return Err(()); // 不完整帧
    }
    let mask = [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]];
    idx += 4;
    let mut payload = buf[idx..idx + len as usize].to_vec();
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Ok(WsFrame { fin, opcode, payload })
}

// ---------------------------------------------------------------------------
// 握手（RFC 6455 §4.2.2）
// ---------------------------------------------------------------------------

/// 生成服务端握手响应（101 Switching Protocols）。
///
/// 解析请求头：`Sec-WebSocket-Key` 存在 + `Upgrade: websocket` +
/// `Sec-WebSocket-Version: 13`，否则 `Err`（非 WS 请求拒绝）。
#[allow(clippy::result_unit_err)]
pub fn handshake_response(request: &str) -> Result<String, ()> {
    let mut key: Option<&str> = None;
    let mut upgrade = false;
    let mut version_ok = false;
    for line in request.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("sec-websocket-key") {
            key = Some(value);
        } else if name.eq_ignore_ascii_case("upgrade") {
            upgrade = value.eq_ignore_ascii_case("websocket");
        } else if name.eq_ignore_ascii_case("sec-websocket-version") {
            version_ok = value == "13";
        }
    }
    let key = key.ok_or(())?;
    if !upgrade || !version_ok {
        return Err(());
    }
    let mut accept_input = Vec::with_capacity(key.len() + WS_GUID.len());
    accept_input.extend_from_slice(key.as_bytes());
    accept_input.extend_from_slice(WS_GUID.as_bytes());
    let accept = base64_encode(&sha1(&accept_input));
    Ok(format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept
    ))
}

/// SHA-1（RFC 3174）——仅用于 WS 握手（Sec-WebSocket-Accept），
/// 非安全用途；EnerOS 安全哈希一律 SM3（crypto 栈）。
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    // 填充：0x80 + 0x00... + 64-bit 位长（big-endian）。
    let ml = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 64);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, b) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(b.try_into().unwrap());
        }
        // w 需随机索引（SHA 轮函数依赖 w[i-3..i-16]），clippy 建议不适用。
        #[allow(clippy::needless_range_loop)]
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        // 主轮循环依赖 i（相位常量 + w[i] 随机索引），clippy 建议不适用。
        #[allow(clippy::needless_range_loop)]
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

/// Base64（RFC 4648 标准表，带 padding）。
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        out.push(B64[(b[0] >> 2) as usize] as char);
        out.push(B64[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[(((b[1] & 0x0F) << 2) | (b[2] >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(b[2] & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 事件 → JSON 文本帧
// ---------------------------------------------------------------------------

/// `DashboardEvent` → JSON 文本（手写编码，零 serde 依赖）。
///
/// 浮点量纲检查（NaN/负值归零，与 real_render 同契约）；
/// 字符串转义（label 可含引号/控制字符）。
pub fn encode_event_json(event: &DashboardEvent) -> String {
    match event {
        DashboardEvent::TopologyUpdate(g) => encode_topology(g),
        DashboardEvent::PowerFlowUpdate(samples) => encode_powerflow(samples),
        DashboardEvent::NodeStatusChange(id, status) => format!(
            "{{\"type\":\"node_status\",\"id\":{},\"status\":\"{}\"}}",
            id,
            node_status_str(*status)
        ),
    }
}

fn encode_topology(g: &TopologyGraph) -> String {
    let mut nodes = String::new();
    for n in &g.nodes {
        nodes.push_str(&format!(
            "{{\"id\":{},\"label\":\"{}\",\"kind\":\"{}\",\"status\":\"{}\",\"x\":{:.2},\"y\":{:.2},\"z\":{:.2}}},",
            n.node_id,
            esc(&n.label),
            node_kind_str(n.kind),
            node_status_str(n.status),
            n.position.x,
            n.position.y,
            n.position.z
        ));
    }
    if nodes.ends_with(',') {
        nodes.pop();
    }
    let mut edges = String::new();
    for e in &g.edges {
        edges.push_str(&format!(
            "{{\"from\":{},\"to\":{},\"kind\":\"{}\",\"flow\":{:.3},\"direction\":\"{}\"}},",
            e.from,
            e.to,
            edge_kind_str(e.kind),
            finite_or_zero(e.flow),
            flow_dir_str(e.direction)
        ));
    }
    if edges.ends_with(',') {
        edges.pop();
    }
    format!(
        "{{\"type\":\"topology\",\"timestamp\":{},\"nodes\":[{}],\"edges\":[{}]}}",
        g.timestamp, nodes, edges
    )
}

fn encode_powerflow(samples: &[PowerFlowSample]) -> String {
    let mut out = String::from("{\"type\":\"powerflow\",\"samples\":[");
    for s in samples {
        out.push_str(&format!(
            "{{\"from\":{},\"to\":{},\"kw\":{:.3},\"v\":{:.1},\"a\":{:.2}}},",
            s.from,
            s.to,
            finite_or_zero(s.power_kw),
            finite_or_zero(s.voltage),
            finite_or_zero(s.current)
        ));
    }
    if out.ends_with(',') {
        out.pop();
    }
    out.push_str("]}");
    out
}

fn node_kind_str(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Agent => "agent",
        NodeKind::Asset => "asset",
        NodeKind::Market => "market",
        NodeKind::Grid => "grid",
        NodeKind::Substation => "substation",
    }
}

fn node_status_str(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Healthy => "healthy",
        NodeStatus::Warn => "warn",
        NodeStatus::Fault => "fault",
        NodeStatus::Offline => "offline",
    }
}

fn edge_kind_str(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::Power => "power",
        EdgeKind::Data => "data",
        EdgeKind::Trade => "trade",
    }
}

fn flow_dir_str(d: FlowDirection) -> &'static str {
    match d {
        FlowDirection::Forward => "forward",
        FlowDirection::Reverse => "reverse",
        FlowDirection::Bidirectional => "bidirectional",
    }
}

/// NaN/负值 → 0（量纲检查；与 real_render 同契约）。
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        0.0
    }
}

/// JSON 字符串转义（引号/反斜杠/控制字符）。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 真 WebSocket 服务端（DashboardWsServer 实现）
// ---------------------------------------------------------------------------

/// 真 WebSocket 服务端：`DashboardEvent` → JSON 文本帧 → 注入发送闭包。
///
/// 发送闭包 `FnMut(&[u8]) -> Result<(), ()>` 由集成层注入（真 TCP/MQTT
/// 桥）；失败 → `Err(BroadcastFailed)` + 失败计数（红线4 不静默）。
/// 多客户端由集成层每连接一个实例管理。
pub struct WsServer<F> {
    /// 帧写出闭包（当前连接）。
    send_fn: RefCell<F>,
    /// 成功发送帧数。
    frames_sent: Cell<u64>,
    /// 失败发送帧数。
    frames_failed: Cell<u64>,
}

impl<F> WsServer<F>
where
    F: FnMut(&[u8]) -> Result<(), ()>,
{
    /// 新建服务端（连接已握手完成；`send_fn` 写出帧字节）。
    pub fn new(send_fn: F) -> Self {
        Self {
            send_fn: RefCell::new(send_fn),
            frames_sent: Cell::new(0),
            frames_failed: Cell::new(0),
        }
    }

    /// 成功发送帧数。
    pub fn frames_sent(&self) -> u64 {
        self.frames_sent.get()
    }

    /// 失败发送帧数。
    pub fn frames_failed(&self) -> u64 {
        self.frames_failed.get()
    }

    /// 发送心跳 ping（空文本帧保活）。
    pub fn send_ping(&self) -> Result<(), ObsError> {
        self.write_frame(OP_PING, &[])
    }

    /// 发送关闭帧。
    pub fn send_close(&self) -> Result<(), ObsError> {
        self.write_frame(OP_CLOSE, &[])
    }

    /// 写入一帧（内部：编码 + 写出 + 计数）。
    fn write_frame(&self, opcode: u8, payload: &[u8]) -> Result<(), ObsError> {
        let frame = encode_frame(opcode, payload, None);
        match self.send_fn.borrow_mut()(&frame) {
            Ok(()) => {
                self.frames_sent.set(self.frames_sent.get() + 1);
                Ok(())
            }
            Err(()) => {
                self.frames_failed.set(self.frames_failed.get() + 1);
                Err(ObsError::BroadcastFailed)
            }
        }
    }
}

impl<F> DashboardWsServer for WsServer<F>
where
    F: FnMut(&[u8]) -> Result<(), ()>,
{
    fn broadcast(&self, event: &DashboardEvent) -> Result<(), ObsError> {
        let json = encode_event_json(event);
        self.write_frame(OP_TEXT, json.as_bytes())
    }
}

// ============================================================================
// Tests（RFC 6455 官方向量 + 字节级往返 + 事件编码）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FlowDirection, Position3D, TopologyEdge, TopologyNode};

    fn sample_graph() -> TopologyGraph {
        TopologyGraph {
            nodes: vec![
                TopologyNode {
                    node_id: 1,
                    label: String::from("PV \"roof\""),
                    kind: NodeKind::Grid,
                    position: Position3D::new(1.0, 2.0, 0.0),
                    status: NodeStatus::Healthy,
                },
                TopologyNode {
                    node_id: 2,
                    label: String::from("Load"),
                    kind: NodeKind::Asset,
                    position: Position3D::new(3.0, 4.0, 0.0),
                    status: NodeStatus::Fault,
                },
            ],
            edges: vec![TopologyEdge {
                from: 1,
                to: 2,
                kind: EdgeKind::Power,
                flow: 150.5,
                direction: FlowDirection::Forward,
            }],
            timestamp: 1000,
        }
    }

    #[test]
    fn test_sha1_rfc_vector() {
        // RFC 3174 测试向量："abc"。
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e,
                0x25, 0x71, 0x78, 0x50, 0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
    }

    #[test]
    fn test_base64_vectors() {
        // RFC 4648 标准向量。
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"the sample nonce"), "dGhlIHNhbXBsZSBub25jZQ==");
    }

    #[test]
    fn test_handshake_rfc_vector() {
        // RFC 6455 官方示例：key → accept。
        let req = "GET /chat HTTP/1.1\r\n\
                   Host: server.example.com\r\n\
                   Upgrade: websocket\r\n\
                   Connection: Upgrade\r\n\
                   Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                   Sec-WebSocket-Version: 13\r\n\
                   \r\n";
        let resp = handshake_response(req).expect("握手必须成功");
        assert!(resp.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        assert!(resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
    }

    #[test]
    fn test_handshake_rejects_non_ws() {
        // 缺 Upgrade 头。
        assert!(handshake_response("GET / HTTP/1.1\r\nHost: x\r\n\r\n").is_err());
        // 缺 Sec-WebSocket-Key。
        assert!(handshake_response(
            "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .is_err());
        // 版本不对。
        assert!(handshake_response(
            "GET / HTTP/1.1\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 12\r\n\r\n"
        )
        .is_err());
        // 非 websocket upgrade。
        assert!(handshake_response(
            "GET / HTTP/1.1\r\nUpgrade: h2c\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .is_err());
    }

    #[test]
    fn test_encode_small_text_frame() {
        // "Hello" → 0x81 0x05 + payload（server→client 无掩码）。
        let f = encode_frame(OP_TEXT, b"Hello", None);
        assert_eq!(f, vec![0x81, 0x05, b'H', b'e', b'l', b'l', b'o']);
    }

    #[test]
    fn test_encode_medium_and_long_frames() {
        // 126 长度：>125 字节。
        let payload = vec![0xAAu8; 300];
        let f = encode_frame(OP_TEXT, &payload, None);
        assert_eq!(f[0], 0x81);
        assert_eq!(f[1], 126);
        assert_eq!(u16::from_be_bytes([f[2], f[3]]), 300);
        assert_eq!(&f[4..], &payload[..]);
        // 127 长度：>65535 字节。
        let payload = vec![0xBBu8; 70_000];
        let f = encode_frame(OP_TEXT, &payload, None);
        assert_eq!(f[1], 127);
        assert_eq!(u64::from_be_bytes(f[2..10].try_into().unwrap()), 70_000);
    }

    #[test]
    fn test_parse_masked_client_frame_roundtrip() {
        // client 掩码帧 → 解析 → 原文一致。
        let mask = [0x37, 0xFA, 0x21, 0x3D];
        let raw = encode_frame(OP_TEXT, b"Hello, WS", Some(mask));
        assert_eq!(raw[1] & 0x80, 0x80, "client 帧必须带掩码位");
        let frame = parse_frame(&raw).expect("解析成功");
        assert!(frame.fin);
        assert_eq!(frame.opcode, OP_TEXT);
        assert_eq!(frame.payload, b"Hello, WS");
    }

    #[test]
    fn test_parse_rejects_invalid_frames() {
        // 未掩码帧（server 必须拒绝）。
        assert!(parse_frame(&[0x81, 0x05, b'H', b'i']).is_err());
        // RSV 非零。
        let masked = encode_frame(OP_TEXT, b"x", Some([1, 2, 3, 4]));
        assert!(parse_frame(&[0xC1, masked[1], masked[2], masked[3]]).is_err());
        // 截断帧。
        let raw = encode_frame(OP_TEXT, b"hello", Some([1, 2, 3, 4]));
        assert!(parse_frame(&raw[..raw.len() - 3]).is_err());
        // 超长帧（127 长度超上限）。
        let mut hdr = vec![0x81, 0xFF];
        hdr.extend_from_slice(&(MAX_FRAME_BYTES as u64 + 1).to_be_bytes());
        hdr.extend_from_slice(&[0, 0, 0, 0]);
        assert!(parse_frame(&hdr).is_err());
    }

    #[test]
    fn test_ws_server_broadcast_text_frame() {
        let sent: RefCell<Vec<u8>> = RefCell::new(Vec::new());
        let srv = WsServer::new(|frame: &[u8]| {
            sent.borrow_mut().extend_from_slice(frame);
            Ok(())
        });
        let event = DashboardEvent::TopologyUpdate(sample_graph());
        srv.broadcast(&event).expect("广播成功");
        assert_eq!(srv.frames_sent(), 1);
        let bytes = sent.borrow();
        assert_eq!(bytes[0], 0x81, "FIN+文本帧");
        assert_eq!(bytes[1] & 0x80, 0, "server→client 不掩码");
        let payload = String::from_utf8(bytes[2..].to_vec()).expect("UTF-8 JSON");
        assert!(payload.contains("\"type\":\"topology\""));
        assert!(payload.contains("\"id\":1"));
        assert!(payload.contains("\"label\":\"PV \\\"roof\\\"\""), "label 转义: {payload}");
        assert!(payload.contains("\"flow\":150.500"));
        assert!(payload.contains("\"status\":\"fault\""));
    }

    #[test]
    fn test_ws_server_broadcast_failure_counts() {
        let srv = WsServer::new(|_f: &[u8]| -> Result<(), ()> { Err(()) });
        let err = srv.broadcast(&DashboardEvent::NodeStatusChange(3, NodeStatus::Warn));
        assert_eq!(err, Err(ObsError::BroadcastFailed));
        assert_eq!(srv.frames_failed(), 1);
        assert_eq!(srv.frames_sent(), 0);
    }

    #[test]
    fn test_ws_server_ping_close() {
        let sent: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
        let srv = WsServer::new(|frame: &[u8]| {
            sent.borrow_mut().push(frame.to_vec());
            Ok(())
        });
        srv.send_ping().expect("ping");
        srv.send_close().expect("close");
        let frames = sent.borrow();
        assert_eq!(frames[0][0], 0x89, "ping 帧");
        assert_eq!(frames[1][0], 0x88, "close 帧");
    }

    #[test]
    fn test_encode_powerflow_json() {
        let samples = vec![
            PowerFlowSample {
                from: 1,
                to: 2,
                power_kw: 150.5,
                voltage: 230.0,
                current: 0.65,
            },
            PowerFlowSample {
                from: 2,
                to: 1,
                power_kw: f64::NAN,
                voltage: 0.0,
                current: 0.0,
            },
        ];
        let json = encode_event_json(&DashboardEvent::PowerFlowUpdate(samples));
        assert!(json.contains("\"type\":\"powerflow\""));
        assert!(json.contains("\"kw\":150.500"));
        assert!(json.contains("\"kw\":0.000"), "NaN 归零");
    }

    #[test]
    fn test_encode_node_status_json() {
        let json = encode_event_json(&DashboardEvent::NodeStatusChange(7, NodeStatus::Offline));
        assert_eq!(json, "{\"type\":\"node_status\",\"id\":7,\"status\":\"offline\"}");
    }
}
