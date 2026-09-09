//! svc-config GET_ENTRY 客户端单源（SPEC-SERIES-2026-08-28/B WP-B.1）。
//!
//! 历史病灶（体检候选 5）：CFG1 wire 知识（请求构造 → ChunkedClient →
//! 4B LE 长度前缀剥离）在 4 个 crate 约 10 个调用点逐字手抄——服务端单源
//! 无恙，客户端无模块。本模块把请求构造与响应前缀解析收编一处，调用方
//! 只负责传输/解码错误的**呈现**（各自措辞与 fail-closed 策略）。

use alloc::vec::Vec;

use crate::chunked::RawIpc;
use crate::endpoint::ChunkedEndpoint;

/// GET_ENTRY 协议魔数（与服务端 `config_service.rs::PROTO_MAGIC` 同值）。
pub const CFG_MAGIC: &[u8; 4] = b"CFG1";
/// GET_ENTRY 子命令。
pub const CMD_GET_ENTRY: u8 = 0x01;
/// GET_DIGEST 子命令（服务端 config_service.rs 同值）。
pub const CMD_GET_DIGEST: u8 = 0x02;
/// GET_GENERATION 子命令（蓝图 61 v0.314.27：观测映射世代单源）。
pub const CMD_GET_GENERATION: u8 = 0x03;
/// svc-config 端点名（所有配置客户端共用）。
pub const CONFIG_ENDPOINT: &str = "svc-config";

/// GET_ENTRY 客户端错误面（传输/前缀解析；canonical 解码错误属调用方域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// EIPC/分块通道失败（服务死亡/超时/断连）。
    Transport,
    /// 响应不足 4B 长度前缀。
    TooShort,
    /// 4B 长度前缀越出响应体。
    LengthPrefix,
}

/// 构造 GET_ENTRY 请求帧：`CFG1 ‖ 0x01 ‖ entry 字节`。
pub fn encode_get_entry(entry: &str) -> Vec<u8> {
    let mut req = Vec::with_capacity(5 + entry.len());
    req.extend_from_slice(CFG_MAGIC);
    req.push(CMD_GET_ENTRY);
    req.extend_from_slice(entry.as_bytes());
    req
}

/// 解析 GET_ENTRY 响应：`4B LE 长度前缀 + canonical payload`。
pub fn decode_response_body(resp: &[u8]) -> Result<&[u8], ConfigError> {
    if resp.len() < 4 {
        return Err(ConfigError::TooShort);
    }
    let plen =
        u32::from_le_bytes(resp[..4].try_into().map_err(|_| ConfigError::TooShort)?) as usize;
    if 4 + plen > resp.len() {
        return Err(ConfigError::LengthPrefix);
    }
    Ok(&resp[4..4 + plen])
}

/// GET_ENTRY 一次往返：请求构造 + 分块传输，返回**原始响应**字节（含
/// 4B 长度前缀；取 canonical payload 用 [`decode_response_body`]）。
///
/// 蓝图 67 WP-67.2.2 类型化改造：传输/epoch 纪律收 [`ChunkedEndpoint`]
/// 单点（超时 = 常量默认 5_000，与原全调用方实传值一致）——调用方不再
/// 手传 epoch（原"调用方维护页序单调性"参数面删除；svc-mgmt 手递增 ×2
/// 于本卷收编）。
pub fn get_entry<C: RawIpc>(
    endpoint: &mut ChunkedEndpoint<C>,
    entry: &str,
) -> Result<Vec<u8>, ConfigError> {
    raw_command(endpoint, CMD_GET_ENTRY, entry.as_bytes())
}

/// GET_GENERATION 一次往返（蓝图 61）：返回 active bundle generation。
///
/// 协议服务用其作观测事件 `mapping_version`（点表生效世代——签名配置
/// 分配，binary 零硬编码）。服务端响应 = generation 8B LE。
pub fn get_generation<C: RawIpc>(endpoint: &mut ChunkedEndpoint<C>) -> Result<u64, ConfigError> {
    let resp = raw_command(endpoint, CMD_GET_GENERATION, &[])?;
    let body = decode_response_body(&resp)?;
    if body.len() != 8 {
        return Err(ConfigError::LengthPrefix);
    }
    Ok(u64::from_le_bytes(body.try_into().unwrap()))
}

/// 子命令一次往返（请求构造 + 分块传输单一实现；GET_ENTRY/GET_GENERATION
/// 共用——服务端同 magic 同前缀词形）。
fn raw_command<C: RawIpc>(
    endpoint: &mut ChunkedEndpoint<C>,
    cmd: u8,
    body: &[u8],
) -> Result<Vec<u8>, ConfigError> {
    let mut req = Vec::with_capacity(5 + body.len());
    req.extend_from_slice(CFG_MAGIC);
    req.push(cmd);
    req.extend_from_slice(body);
    endpoint
        .call(CONFIG_ENDPOINT, &req)
        .map_err(|_| ConfigError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunked::{
        ack_bytes, is_pull_request, ChunkHeader, ChunkedEvent, PagedServerV2, CHUNK_MAGIC_FINAL,
        CHUNK_MAGIC_HEADER,
    };

    /// svc-config 内存对端：chunked 分块状态机（同 control_service_main）+
    /// GET_ENTRY 回显（4B 长度前缀 + 条目字节）。分页帧律（v2 判别帧）
    /// 由 [`PagedServerV2`] 承载（70.3 收编）。
    struct FakeConfigServer {
        rx: crate::chunked::ChunkedReceiver,
        pending: Option<Vec<u8>>,
        pages: PagedServerV2,
    }

    impl RawIpc for FakeConfigServer {
        fn request(
            &mut self,
            _t: &str,
            payload: &[u8],
            _to: u64,
        ) -> Result<Vec<u8>, crate::AgentError> {
            if &payload[..4] == CHUNK_MAGIC_HEADER {
                let h = ChunkHeader::decode(payload).unwrap();
                if let ChunkedEvent::Header { epoch, .. } = self.rx.on_message(&h.encode(), true) {
                    self.pages.set_txn(epoch);
                }
                return Ok(ack_bytes().to_vec());
            }
            if &payload[..4] == CHUNK_MAGIC_FINAL {
                let resp = match self.pending.take() {
                    Some(full) => {
                        assert_eq!(&full[..4], b"CFG1", "请求帧 magic");
                        // 响应体（4B 前缀在下方统一加——单前缀词形）。
                        let body: Vec<u8> = if full[4] == CMD_GET_GENERATION {
                            let mut b = Vec::new();
                            b.extend_from_slice(&42u64.to_le_bytes());
                            b
                        } else {
                            assert_eq!(full[4], CMD_GET_ENTRY);
                            full[5..].to_vec()
                        };
                        let mut resp = Vec::new();
                        resp.extend_from_slice(&(body.len() as u32).to_le_bytes());
                        resp.extend_from_slice(&body);
                        resp
                    }
                    // Final 无完整请求 → 保持显式拒绝词形（不装空响应）。
                    None => return Ok(b"ERR:NoTransfer".to_vec()),
                };
                return self.pages.on_request(payload, resp).ok_or_else(|| {
                    crate::AgentError::IpcError(
                        heapless::String::try_from("paged-reject").unwrap_or_default(),
                    )
                });
            }
            if is_pull_request(payload) {
                return self.pages.on_request(payload, Vec::new()).ok_or_else(|| {
                    crate::AgentError::IpcError(
                        heapless::String::try_from("paged-reject").unwrap_or_default(),
                    )
                });
            }
            Err(crate::AgentError::IpcError(
                heapless::String::try_from("unexpected").unwrap_or_default(),
            ))
        }

        fn send(&mut self, _t: &str, payload: &[u8]) -> Result<(), crate::AgentError> {
            if let ChunkedEvent::Complete(full) = self.rx.on_message(payload, false) {
                self.pending = Some(full);
            }
            Ok(())
        }
    }

    #[test]
    fn request_frame_is_cfg1_get_entry() {
        let req = encode_get_entry("points");
        assert_eq!(&req[..4], b"CFG1");
        assert_eq!(req[4], CMD_GET_ENTRY);
        assert_eq!(&req[5..], b"points");
        // 长条目与空条目（长度自洽）。
        let long = encode_get_entry("protocol-modbus-tcp");
        assert_eq!(&long[5..], b"protocol-modbus-tcp");
        assert_eq!(encode_get_entry(""), &[b'C', b'F', b'G', b'1', 0x01]);
    }

    #[test]
    fn response_body_strips_prefix_and_validates() {
        let mut resp: Vec<u8> = Vec::new();
        resp.extend_from_slice(&5u32.to_le_bytes());
        resp.extend_from_slice(b"hello");
        assert_eq!(decode_response_body(&resp).unwrap(), b"hello");
        // 截断：前缀声明 5 但 body 只有 3。
        let mut trunc = resp.clone();
        trunc.truncate(7);
        assert_eq!(decode_response_body(&trunc), Err(ConfigError::LengthPrefix));
        // 不足 4B。
        assert_eq!(decode_response_body(&[1, 2, 3]), Err(ConfigError::TooShort));
        // 空 body 合法（plen=0）。
        let empty = alloc::vec![0u8; 4];
        assert_eq!(decode_response_body(&empty).unwrap(), &[]);
    }

    #[test]
    fn get_entry_roundtrip_via_chunked() {
        // 蓝图 67 WP-67.2.2：调用面 = ChunkedEndpoint（epoch 纪律单点，
        // 端点持有对端）。
        let mut ep = crate::endpoint::ChunkedEndpoint::wrap(FakeConfigServer {
            rx: Default::default(),
            pending: None,
            pages: PagedServerV2::new(),
        });
        let resp = get_entry(&mut ep, "points").expect("transfer");
        let body = decode_response_body(&resp).unwrap();
        assert_eq!(body, b"points");
        assert_eq!(ep.epoch_for_test(), 1, "config 往返同样消耗恰好 1 epoch");
    }

    /// 蓝图 61：GET_GENERATION 往返（8B LE 响应体）。
    #[test]
    fn get_generation_roundtrip_via_chunked() {
        let mut ep = crate::endpoint::ChunkedEndpoint::wrap(FakeConfigServer {
            rx: Default::default(),
            pending: None,
            pages: PagedServerV2::new(),
        });
        let gen = get_generation(&mut ep).expect("transfer");
        assert_eq!(gen, 42);
    }
}
