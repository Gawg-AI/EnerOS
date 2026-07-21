//! MMS 客户端：连接状态机 + Read/Write 服务（泛型传输，D4/D11）.
//!
//! 关联时序（蓝图 §4.3）：`COTP CR` → `COTP CC` → `ACSE AARQ` → `ACSE AARE`，
//! 成功后进入 `Connected`；COTP 数据 TPDU 头（`[0x02, 0xF0, 0x80]`）在本模块内联（D9）。
//!
//! 错误语义（D10/D11）：
//! - `connect` 至多尝试 3 次（无 sleep，超时语义归传输层），3 次全败 → 最后一次错误 + `Error`
//! - 未连接调用 `read`/`write` → `NotConnected`（状态不变）
//! - 发送/接收/解码失败 → `state = Error`；再次 `connect` 成功后可恢复
//! - 对端返回 ConfirmedErrorPDU（0xA2）→ `MmsResponse::Error { code }`（不置 Error 状态）

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_iec61850_model::{DaValue, Quality};

use crate::acse::{decode_aare, decode_cotp_cc, encode_aarq, encode_cotp_cr, COTP_DT_HEADER_LEN};
use crate::ber_decode::{
    decode_read_response, decode_write_response, read_tag_length, TAG_CONFIRMED_ERROR,
};
use crate::ber_encode::BerEncoder;
use crate::MmsError;

/// 连接重试次数上限（蓝图 §4.4，D11）。
const MAX_CONNECT_ATTEMPTS: usize = 3;
/// 接收缓冲区大小（简化栈定长）。
const RECV_BUF_LEN: usize = 4096;

/// MMS 传输层抽象（D4：v0.29.0 Socket 真实接线在集成层）。
pub trait MmsTransport {
    /// 建立底层 TCP 连接（超时语义由实现内部决定）。
    fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError>;
    /// 发送一段字节。
    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError>;
    /// 接收一段字节到 `buf`，返回实际长度。
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError>;
}

/// MMS 连接信息。
#[derive(Debug, Clone, PartialEq)]
pub struct MmsConnection {
    /// 对端地址。
    pub peer_addr: String,
    /// 对端端口（MMS over TCP 默认 102）。
    pub peer_port: u16,
    /// 本地 AP-title。
    pub local_ap_title: String,
    /// 连接状态。
    pub state: ConnState,
}

/// 连接状态机。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnState {
    /// 未连接。
    Idle,
    /// 连接建立中（COTP/ACSE 握手）。
    Connecting,
    /// 已连接（可读写）。
    Connected,
    /// 错误（需重连恢复）。
    Error,
}

/// 变量访问规格（domain = LD 名，item = LN.DO.DA 路径）。
#[derive(Debug, Clone, PartialEq)]
pub struct VarAccessSpec {
    /// LD 名（如 "IED1_LD0"）。
    pub domain: String,
    /// LN.DO.DA 路径（如 "XCBR1.Pos.stVal"）。
    pub item: String,
}

/// MMS 请求（蓝图 §4.1 全量，含前瞻变体，D5）。
#[derive(Debug, Clone, PartialEq)]
pub enum MmsRequest {
    /// Read 服务。
    Read {
        /// 变量访问规格列表。
        variable_access: Vec<VarAccessSpec>,
    },
    /// Write 服务。
    Write {
        /// （变量访问规格, 写入值）列表。
        variable_access: Vec<(VarAccessSpec, DaValue)>,
    },
    /// GetVariableAccessAttributes（前瞻，模型消费在后续版本接入）。
    GetVariableAccessAttributes {
        /// LD 名。
        domain: String,
        /// LN.DO 路径。
        item: String,
    },
    /// DefineNamedVariableList（前瞻）。
    DefineNamedVariableList {
        /// 变量列表名。
        name: String,
        /// 条目列表。
        entries: Vec<VarAccessSpec>,
    },
}

/// MMS 响应。
#[derive(Debug, Clone, PartialEq)]
pub enum MmsResponse {
    /// Read 结果。
    ReadResult {
        /// 结果列表（保序）。
        results: Vec<MmsReadResult>,
    },
    /// Write 结果。
    WriteResult {
        /// 结果列表（保序）。
        results: Vec<MmsWriteResult>,
    },
    /// 对端 ConfirmedErrorPDU。
    Error {
        /// 错误码。
        code: MmsErrorCode,
    },
}

/// Read 单条结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MmsReadResult {
    /// 值（未知数据类型 → None）。
    pub value: Option<DaValue>,
    /// 品质（解码侧默认 Good，无时间语义）。
    pub quality: Quality,
    /// 时间戳（解码侧默认 0，由集成层注入）。
    pub timestamp: u64,
}

/// Write 单条结果。
#[derive(Debug, Clone, PartialEq)]
pub enum MmsWriteResult {
    /// 成功。
    Success,
    /// 失败（携带 DataAccessError 描述）。
    Failed(String),
}

/// MMS 错误码（对端错误语义）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MmsErrorCode {
    /// 超时。
    Timeout,
    /// 拒绝。
    Refused,
    /// 对象不存在。
    NotFound,
    /// 类型不匹配。
    TypeMismatch,
    /// 未知错误码。
    Unknown(u16),
}

/// MMS 客户端（泛型传输，D4）。
pub struct MmsClient<T: MmsTransport> {
    conn: MmsConnection,
    transport: T,
    /// 超时预算（D11：重试无 sleep，超时语义归传输层；字段供集成层读取）。
    #[allow(dead_code)]
    timeout_ms: u32,
    invoke_id: u32,
}

impl<T: MmsTransport> MmsClient<T> {
    /// 创建客户端（初始 `state = Idle`，`invoke_id = 0`）。
    pub fn new(transport: T, local_ap_title: &str, timeout_ms: u32) -> Self {
        Self {
            conn: MmsConnection {
                peer_addr: String::new(),
                peer_port: 0,
                local_ap_title: String::from(local_ap_title),
                state: ConnState::Idle,
            },
            transport,
            timeout_ms,
            invoke_id: 0,
        }
    }

    /// 建立连接：COTP CR/CC + ACSE AARQ/AARE，重试至多 3 次（D11）。
    pub fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError> {
        self.conn.state = ConnState::Connecting;
        self.conn.peer_addr = String::from(addr);
        self.conn.peer_port = port;
        let mut last_err = MmsError::Timeout;
        for _ in 0..MAX_CONNECT_ATTEMPTS {
            match self.try_handshake() {
                Ok(()) => {
                    self.conn.state = ConnState::Connected;
                    return Ok(());
                }
                Err(e) => last_err = e,
            }
        }
        self.conn.state = ConnState::Error;
        Err(last_err)
    }

    /// Read 服务：编码 → 发送 → 接收 → 解码（保序）。
    pub fn read(&mut self, vars: &[VarAccessSpec]) -> Result<MmsResponse, MmsError> {
        if self.conn.state != ConnState::Connected {
            return Err(MmsError::NotConnected);
        }
        self.invoke_id = self.invoke_id.wrapping_add(1);
        let mut enc = BerEncoder::new();
        let pdu = wrap_cotp_data(enc.encode_read_request(self.invoke_id, vars));
        self.send_pdu(&pdu)?;
        let mut buf = [0u8; RECV_BUF_LEN];
        let n = self.recv_pdu(&mut buf)?;
        if is_error_pdu(&buf[..n]) {
            let code = decode_error_pdu(&buf[..n])?;
            return Ok(MmsResponse::Error { code });
        }
        match decode_read_response(&buf[..n]) {
            Ok(results) => Ok(MmsResponse::ReadResult { results }),
            Err(e) => {
                self.conn.state = ConnState::Error;
                Err(e)
            }
        }
    }

    /// Write 服务：同构（编码 → 发送 → 接收 → 解码）。
    pub fn write(&mut self, vars: &[(VarAccessSpec, DaValue)]) -> Result<MmsResponse, MmsError> {
        if self.conn.state != ConnState::Connected {
            return Err(MmsError::NotConnected);
        }
        self.invoke_id = self.invoke_id.wrapping_add(1);
        let mut enc = BerEncoder::new();
        let pdu = wrap_cotp_data(enc.encode_write_request(self.invoke_id, vars));
        self.send_pdu(&pdu)?;
        let mut buf = [0u8; RECV_BUF_LEN];
        let n = self.recv_pdu(&mut buf)?;
        if is_error_pdu(&buf[..n]) {
            let code = decode_error_pdu(&buf[..n])?;
            return Ok(MmsResponse::Error { code });
        }
        match decode_write_response(&buf[..n]) {
            Ok(results) => Ok(MmsResponse::WriteResult { results }),
            Err(e) => {
                self.conn.state = ConnState::Error;
                Err(e)
            }
        }
    }

    /// 断开连接（状态回到 `Idle`）。
    pub fn disconnect(&mut self) {
        self.conn.state = ConnState::Idle;
    }

    /// 查询当前连接状态。
    pub fn conn_state(&self) -> ConnState {
        self.conn.state
    }

    /// 读取传输层（测试断言 / 集成层接线用）。
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// 可变读取传输层（测试脚本注入用）。
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// 单次握手尝试：transport.connect → COTP CR → CC → AARQ → AARE。
    fn try_handshake(&mut self) -> Result<(), MmsError> {
        self.transport
            .connect(&self.conn.peer_addr, self.conn.peer_port)?;
        let cr = encode_cotp_cr();
        self.transport.send(&cr)?;
        let mut buf = [0u8; 256];
        let n = self.transport.recv(&mut buf)?;
        decode_cotp_cc(&buf[..n])?;
        let aarq = encode_aarq(&self.conn.local_ap_title);
        let aarq_pdu = wrap_cotp_data(&aarq);
        self.transport.send(&aarq_pdu)?;
        let n = self.transport.recv(&mut buf)?;
        decode_aare(&buf[..n])?;
        Ok(())
    }

    /// 发送 PDU（失败置 `Error` 状态）。
    fn send_pdu(&mut self, pdu: &[u8]) -> Result<(), MmsError> {
        match self.transport.send(pdu) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.conn.state = ConnState::Error;
                Err(e)
            }
        }
    }

    /// 接收一段响应到 `buf`（失败置 `Error` 状态），返回字节数。
    fn recv_pdu(&mut self, buf: &mut [u8]) -> Result<usize, MmsError> {
        match self.transport.recv(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.conn.state = ConnState::Error;
                Err(e)
            }
        }
    }
}

/// 包装 COTP 数据 TPDU 头（D9：`[0x02, 0xF0, 0x80]` + payload）。
fn wrap_cotp_data(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + COTP_DT_HEADER_LEN);
    out.push(0x02); // LI
    out.push(0xF0); // DT
    out.push(0x80); // TPDU-NR / eot
    out.extend_from_slice(payload);
    out
}

/// 是否 ConfirmedErrorPDU（0xA2）。
fn is_error_pdu(data: &[u8]) -> bool {
    !data.is_empty() && data[0] == TAG_CONFIRMED_ERROR
}

/// 解码 ConfirmedErrorPDU 错误码（0xA2 → 首个 INTEGER/0x85 内容）。
fn decode_error_pdu(data: &[u8]) -> Result<MmsErrorCode, MmsError> {
    let mut pos = 0usize;
    let (tag, _len) = read_tag_length(data, &mut pos)?;
    if tag != TAG_CONFIRMED_ERROR {
        return Err(MmsError::BerDecodeError);
    }
    while pos < data.len() {
        let (t, l) = read_tag_length(data, &mut pos)?;
        if t == 0x02 || t == 0x85 {
            let mut code: u16 = 0;
            for i in 0..l.min(2) {
                code = (code << 8) | u16::from(data[pos + i]);
            }
            return Ok(map_error_code(code));
        }
        pos += l;
    }
    Err(MmsError::BerDecodeError)
}

/// 对端错误码映射（简化栈约定：1~4 已知，其余 Unknown）。
fn map_error_code(code: u16) -> MmsErrorCode {
    match code {
        1 => MmsErrorCode::NotFound,
        2 => MmsErrorCode::TypeMismatch,
        3 => MmsErrorCode::Refused,
        4 => MmsErrorCode::Timeout,
        other => MmsErrorCode::Unknown(other),
    }
}

/// 脚本化 Mock 传输层（测试/集成占位，D4）。
///
/// - `connect`：前 `fail_connects` 次尝试返回 `Timeout`，其后成功；计数可查
/// - `send`：记录已发字节序列（供时序断言：COTP CR 在 AARQ 之前）
/// - `recv`：依次弹出预置响应；无响应 → `Timeout`；可注入一次性错误
pub struct MockTransport {
    responses: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    connect_attempts: usize,
    fail_connects: usize,
    recv_error: Option<MmsError>,
    send_error: Option<MmsError>,
}

impl MockTransport {
    /// 创建空 mock。
    pub fn new() -> Self {
        Self {
            responses: VecDeque::new(),
            sent: Vec::new(),
            connect_attempts: 0,
            fail_connects: 0,
            recv_error: None,
            send_error: None,
        }
    }

    /// 预置一段 recv 响应（按弹出顺序被消费）。
    pub fn push_response(&mut self, bytes: &[u8]) {
        self.responses.push_back(Vec::from(bytes));
    }

    /// 设置前 `n` 次 connect 尝试返回 `Timeout`（D11 重试脚本）。
    pub fn fail_first_connects(&mut self, n: usize) {
        self.fail_connects = n;
    }

    /// connect 尝试计数。
    pub fn connect_attempts(&self) -> usize {
        self.connect_attempts
    }

    /// 已发送字节序列（供时序断言）。
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// 注入一次性 recv 错误。
    pub fn inject_recv_error(&mut self, e: MmsError) {
        self.recv_error = Some(e);
    }

    /// 注入一次性 send 错误。
    pub fn inject_send_error(&mut self, e: MmsError) {
        self.send_error = Some(e);
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MmsTransport for MockTransport {
    fn connect(&mut self, _addr: &str, _port: u16) -> Result<(), MmsError> {
        self.connect_attempts += 1;
        if self.connect_attempts <= self.fail_connects {
            Err(MmsError::Timeout)
        } else {
            Ok(())
        }
    }

    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError> {
        if let Some(e) = self.send_error.take() {
            return Err(e);
        }
        self.sent.push(Vec::from(pdu));
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError> {
        if let Some(e) = self.recv_error.take() {
            return Err(e);
        }
        let resp = self.responses.pop_front().ok_or(MmsError::Timeout)?;
        let n = resp.len().min(buf.len());
        buf[..n].copy_from_slice(&resp[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    const CC: &[u8] = &[0x09, 0xD0, 0x00, 0x01, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A];
    const AARE_OK: &[u8] = &[0x61, 0x03, 0x02, 0x01, 0x00];

    fn spec(domain: &str, item: &str) -> VarAccessSpec {
        VarAccessSpec {
            domain: String::from(domain),
            item: String::from(item),
        }
    }

    fn push_len(buf: &mut Vec<u8>, len: usize) {
        if len < 0x80 {
            buf.push(len as u8);
        } else {
            buf.push(0x82);
            buf.extend_from_slice(&(len as u16).to_be_bytes());
        }
    }

    /// 构造 Read 响应（条目为 u8 整数列表，0x85 01 xx）。
    fn build_int_read_response(vals: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        for v in vals {
            content.extend_from_slice(&[0x85, 0x01, *v]);
        }
        let mut list = vec![0xA0];
        push_len(&mut list, content.len());
        list.extend_from_slice(&content);
        let mut rr = vec![0xA5];
        push_len(&mut rr, list.len());
        rr.extend_from_slice(&list);
        let mut inner = vec![0x02, 0x01, 0x01];
        inner.extend_from_slice(&rr);
        let mut pdu = vec![0xA1];
        push_len(&mut pdu, inner.len());
        pdu.extend_from_slice(&inner);
        pdu
    }

    fn build_write_response(entries: &[&[u8]]) -> Vec<u8> {
        let mut content = Vec::new();
        for e in entries {
            content.extend_from_slice(e);
        }
        let mut wr = vec![0xA6];
        push_len(&mut wr, content.len());
        wr.extend_from_slice(&content);
        let mut inner = vec![0x02, 0x01, 0x01];
        inner.extend_from_slice(&wr);
        let mut pdu = vec![0xA1];
        push_len(&mut pdu, inner.len());
        pdu.extend_from_slice(&inner);
        pdu
    }

    /// 创建已连接的客户端（mock 预置 CC + AARE）。
    fn connected_client() -> MmsClient<MockTransport> {
        let mut mock = MockTransport::new();
        mock.push_response(CC);
        mock.push_response(AARE_OK);
        let mut client = MmsClient::new(mock, "1.1.1.999", 3000);
        client.connect("192.168.0.10", 102).unwrap();
        client
    }

    // ===== MC27：new 初始状态 Idle =====
    #[test]
    fn test_mc27_new_initial_idle() {
        let client = MmsClient::new(MockTransport::new(), "1.1.1.999", 3000);
        assert_eq!(client.conn_state(), ConnState::Idle);
    }

    // ===== MC28：connect 成功状态机 Idle → Connected =====
    #[test]
    fn test_mc28_connect_success_state() {
        let client = connected_client();
        assert_eq!(client.conn_state(), ConnState::Connected);
    }

    // ===== MC29：时序 — 先发 COTP CR 再发 AARQ（mock 记录）=====
    #[test]
    fn test_mc29_send_sequence_cr_before_aarq() {
        let client = connected_client();
        let sent = client.transport().sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0][1], 0xE0); // COTP CR
                                      // AARQ 经 COTP DT 头内联包装：[0x02, 0xF0, 0x80, 0x60, ...]
        assert_eq!(&sent[1][..3], &[0x02, 0xF0, 0x80]);
        assert_eq!(sent[1][3], 0x60); // AARQ
    }

    // ===== MC30：重试 2 次后第 3 次成功（D11）=====
    #[test]
    fn test_mc30_retry_third_attempt_succeeds() {
        let mut mock = MockTransport::new();
        mock.fail_first_connects(2);
        mock.push_response(CC);
        mock.push_response(AARE_OK);
        let mut client = MmsClient::new(mock, "1.1.1.999", 3000);
        client.connect("192.168.0.10", 102).unwrap();
        assert_eq!(client.conn_state(), ConnState::Connected);
        assert_eq!(client.transport().connect_attempts(), 3);
    }

    // ===== MC31：3 次全超时 → Timeout + Error =====
    #[test]
    fn test_mc31_all_attempts_timeout() {
        let mut mock = MockTransport::new();
        mock.fail_first_connects(3);
        let mut client = MmsClient::new(mock, "1.1.1.999", 3000);
        assert_eq!(client.connect("192.168.0.10", 102), Err(MmsError::Timeout));
        assert_eq!(client.conn_state(), ConnState::Error);
        assert_eq!(client.transport().connect_attempts(), 3);
    }

    // ===== MC32：read mock 回路结果 =====
    #[test]
    fn test_mc32_read_mock_loop() {
        let mut client = connected_client();
        let resp = build_int_read_response(&[7, 42]);
        client.transport_mut().push_response(&resp);
        let vars = [
            spec("IED1_LD0", "XCBR1.Pos.stVal"),
            spec("IED1_LD0", "MMXU1.Hz.mag"),
        ];
        match client.read(&vars).unwrap() {
            MmsResponse::ReadResult { results } => {
                assert_eq!(results.len(), 2);
                assert_eq!(results[0].value, Some(DaValue::Int32(7)));
                assert_eq!(results[1].value, Some(DaValue::Int32(42)));
            }
            other => panic!("expect ReadResult, got {:?}", other),
        }
        // 发送的 Read 请求经 COTP DT 包装，第 4 字节为 0xA0
        let sent = client.transport().sent();
        assert_eq!(sent[2][3], 0xA0);
    }

    // ===== MC33：未连接 read/write → NotConnected =====
    #[test]
    fn test_mc33_read_write_not_connected() {
        let mut client = MmsClient::new(MockTransport::new(), "1.1.1.999", 3000);
        let vars = [spec("D", "I")];
        assert_eq!(client.read(&vars), Err(MmsError::NotConnected));
        let wvars = [(spec("D", "I"), DaValue::Bool(true))];
        assert_eq!(client.write(&wvars), Err(MmsError::NotConnected));
        assert_eq!(client.conn_state(), ConnState::Idle);
    }

    // ===== MC34：write Success + Failed =====
    #[test]
    fn test_mc34_write_success_and_failed() {
        let mut client = connected_client();
        let resp = build_write_response(&[&[0x80, 0x00], &[0x81, 0x01, 0x0A]]);
        client.transport_mut().push_response(&resp);
        let vars = [
            (spec("D", "I1"), DaValue::Bool(true)),
            (spec("D", "I2"), DaValue::Int32(1)),
        ];
        match client.write(&vars).unwrap() {
            MmsResponse::WriteResult { results } => {
                assert_eq!(results[0], MmsWriteResult::Success);
                assert!(matches!(results[1], MmsWriteResult::Failed(_)));
            }
            other => panic!("expect WriteResult, got {:?}", other),
        }
    }

    // ===== MC35：disconnect → Idle =====
    #[test]
    fn test_mc35_disconnect_to_idle() {
        let mut client = connected_client();
        assert_eq!(client.conn_state(), ConnState::Connected);
        client.disconnect();
        assert_eq!(client.conn_state(), ConnState::Idle);
        // 断开后 read → NotConnected
        let vars = [spec("D", "I")];
        assert_eq!(client.read(&vars), Err(MmsError::NotConnected));
    }

    // ===== MC36：recv 错误 → state Error → 重连恢复 =====
    #[test]
    fn test_mc36_recv_error_then_reconnect() {
        let mut client = connected_client();
        client
            .transport_mut()
            .inject_recv_error(MmsError::TransportError);
        let vars = [spec("D", "I")];
        assert_eq!(client.read(&vars), Err(MmsError::TransportError));
        assert_eq!(client.conn_state(), ConnState::Error);
        // 重连：预置 CC + AARE + Read 响应
        client.transport_mut().push_response(CC);
        client.transport_mut().push_response(AARE_OK);
        let resp = build_int_read_response(&[1]);
        client.transport_mut().push_response(&resp);
        client.connect("192.168.0.10", 102).unwrap();
        assert_eq!(client.conn_state(), ConnState::Connected);
        match client.read(&vars).unwrap() {
            MmsResponse::ReadResult { results } => {
                assert_eq!(results[0].value, Some(DaValue::Int32(1)));
            }
            other => panic!("expect ReadResult, got {:?}", other),
        }
    }

    // ===== MC37：100 点 read < 50ms 且保序（D12，cfg(test) Instant）=====
    #[test]
    fn test_mc37_read_100_points_perf() {
        let mut client = connected_client();
        let vals: Vec<u8> = (0..100).collect();
        let resp = build_int_read_response(&vals);
        client.transport_mut().push_response(&resp);
        let vars: Vec<VarAccessSpec> = (0..100)
            .map(|i| spec("IED1_LD0", &alloc::format!("MMXU1.P{}.mag", i)))
            .collect();
        let start = std::time::Instant::now();
        let resp = client.read(&vars).unwrap();
        let elapsed = start.elapsed();
        match resp {
            MmsResponse::ReadResult { results } => {
                assert_eq!(results.len(), 100);
                for (i, r) in results.iter().enumerate() {
                    assert_eq!(r.value, Some(DaValue::Int32(i as i32))); // 保序
                }
            }
            other => panic!("expect ReadResult, got {:?}", other),
        }
        assert!(
            elapsed.as_millis() < 50,
            "read 100 points too slow: {:?}",
            elapsed
        );
    }

    // ===== MC38：ConfirmedErrorPDU → MmsResponse::Error code 映射 =====
    #[test]
    fn test_mc38_error_pdu_code_mapping() {
        let mut client = connected_client();
        // 0xA2 → invokeID → 0x85 01 01（NotFound）
        let err_pdu = [0xA2, 0x06, 0x02, 0x01, 0x01, 0x85, 0x01, 0x01];
        client.transport_mut().push_response(&err_pdu);
        let vars = [spec("D", "I")];
        assert_eq!(
            client.read(&vars),
            Ok(MmsResponse::Error {
                code: MmsErrorCode::NotFound
            })
        );
        // Error 响应不置 Error 状态，连接仍可用
        assert_eq!(client.conn_state(), ConnState::Connected);
    }
}
