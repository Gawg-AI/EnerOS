//! OBS1 可观测上报 wire（agent-sdk 共享模块，蓝图 58 C3）。
//!
//! 单一事实：root-task telemetry_report_tick 的手写 ENQUEUE/FLUSH 帧构造
//! 与 svc-observability handler 的手写解析收归一处（WP-45.5 sample_wire
//! 先例疗法）。
//!
//! 布局（小端；magic = `OBS1`）：
//! - 0x01 ENQUEUE：magic‖0x01‖class u8‖service 16B‖schema 8B‖created u64‖
//!   plen u8‖payload（payload ≤ 255——调用方截断，codec 校验拒绝超长）；
//!   resp = [1] 接受（其余 = 拒绝）
//! - 0x02 FLUSH：magic‖0x02‖batch u32；resp = exported u64 或 `ERR:...`
//!   （50 实弹语义随迁：ERR 前缀不得误读为导出数字）

use alloc::string::String;
use alloc::vec::Vec;

/// wire magic。
pub const MAGIC: [u8; 4] = *b"OBS1";
/// ENQUEUE 子码。
pub const SUB_ENQUEUE: u8 = 0x01;
/// FLUSH 子码。
pub const SUB_FLUSH: u8 = 0x02;
/// ENQUEUE 接受响应字节。
pub const ENQ_OK: u8 = 1;
/// service 字段宽。
pub const SERVICE_LEN: usize = 16;
/// schema 字段宽。
pub const SCHEMA_LEN: usize = 8;

/// 解析失败类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsWireError {
    /// 帧短于布局要求。
    Short(usize),
    /// magic 不符。
    BadMagic,
    /// payload 超过 u8 容量。
    PayloadTooLong(usize),
    /// FLUSH 响应被服务侧以 `ERR:` 拒绝。
    FlushRejected(String),
}

/// NUL 填充定长字段编码（超长截断）。
fn fill_field(buf: &mut Vec<u8>, value: &[u8], width: usize) {
    let n = core::cmp::min(value.len(), width);
    buf.extend_from_slice(&value[..n]);
    buf.resize(buf.len() + (width - n), 0);
}

/// 0x01 ENQUEUE 请求帧。
///
/// `payload` 必须 ≤ 255 字节（u8 长度前缀容量）；超长返回错误——截断策略
/// 属调用方编排决策（root 侧健康摘要当前在构造处截断），codec 只诚实拒绝。
pub fn encode_enqueue(
    class: u8,
    service: &[u8],
    schema: &[u8],
    created_ms: u64,
    payload: &[u8],
) -> Result<Vec<u8>, ObsWireError> {
    if payload.len() > 255 {
        return Err(ObsWireError::PayloadTooLong(payload.len()));
    }
    let mut req = Vec::from(MAGIC);
    req.push(SUB_ENQUEUE);
    req.push(class);
    fill_field(&mut req, service, SERVICE_LEN);
    fill_field(&mut req, schema, SCHEMA_LEN);
    req.extend_from_slice(&created_ms.to_le_bytes());
    req.push(payload.len() as u8);
    req.extend_from_slice(payload);
    Ok(req)
}

/// 服务侧解析 ENQUEUE 请求 → 借用视图（payload 零拷贝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnqueueFrame<'a> {
    /// 遥测类别。
    pub class: u8,
    /// 来源服务名（NUL 去 除后的原字节）。
    pub service: [u8; SERVICE_LEN],
    /// schema 名。
    pub schema: [u8; SCHEMA_LEN],
    /// 创建时刻（毫秒）。
    pub created_ms: u64,
    /// 载荷。
    pub payload: &'a [u8],
}

/// 服务侧解析 ENQUEUE 请求。
pub fn parse_enqueue_req(req: &[u8]) -> Result<EnqueueFrame<'_>, ObsWireError> {
    if req.len() < 5 || req[..4] != MAGIC {
        return Err(ObsWireError::BadMagic);
    }
    if req[4] != SUB_ENQUEUE {
        return Err(ObsWireError::Short(req.len()));
    }
    // class(1)+service(16)+schema(8)+created(8)+plen(1)
    if req.len() < 5 + 34 {
        return Err(ObsWireError::Short(req.len()));
    }
    let p = &req[5..];
    let mut frame = EnqueueFrame {
        class: p[0],
        service: [0; SERVICE_LEN],
        schema: [0; SCHEMA_LEN],
        created_ms: u64::from_le_bytes(p[25..33].try_into().expect("u64")),
        payload: &[],
    };
    frame.service.copy_from_slice(&p[1..17]);
    frame.schema.copy_from_slice(&p[17..25]);
    let plen = p[33] as usize;
    let payload = &p[34..];
    if payload.len() < plen {
        return Err(ObsWireError::Short(req.len()));
    }
    frame.payload = &payload[..plen];
    Ok(frame)
}

/// 0x02 FLUSH 请求。
pub fn encode_flush(batch: u32) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_FLUSH);
    req.extend_from_slice(&batch.to_le_bytes());
    req
}

/// FLUSH 响应解析 → 导出条数（`ERR:` 前缀 → FlushRejected，不误读为数字）。
pub fn parse_flush_resp(resp: &[u8]) -> Result<u64, ObsWireError> {
    if resp.starts_with(b"ERR") {
        return Err(ObsWireError::FlushRejected(alloc::string::String::from(
            core::str::from_utf8(resp).unwrap_or("<bad-utf8>"),
        )));
    }
    if resp.len() < 8 {
        return Err(ObsWireError::Short(resp.len()));
    }
    Ok(u64::from_le_bytes(resp[..8].try_into().expect("u64")))
}

/// ENQUEUE 应答拒绝形态（蓝图 64 卷 64.2 WP-64.2.1c——携带原始字节供
/// 调用方按原打印面呈现）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsReject<'a> {
    /// 首字节非 [`ENQ_OK`]（含空应答——0 兜底）。
    NotAck(u8),
    /// `ERR:` 前缀拒绝（原始应答字节）。
    Err(&'a [u8]),
}

/// ENQUEUE 应答解析（约定单源：ack=`ENQ_OK`(0x01) 受理；其余值 / `ERR:`
/// 前缀 = 拒绝——原 root 手解 `first() != Some(&1)` / `starts_with(b"ERR")`
/// 收编，与 [`parse_flush_resp`] 的 ERR 约定同源）。
pub fn parse_enqueue_resp(resp: &[u8]) -> Result<(), ObsReject<'_>> {
    match resp.first() {
        Some(&ENQ_OK) => Ok(()),
        Some(_) if resp.starts_with(b"ERR") => Err(ObsReject::Err(resp)),
        Some(&b) => Err(ObsReject::NotAck(b)),
        None => Err(ObsReject::NotAck(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_round_trip_both_sides_one_type() {
        // root 编码 → 服务侧解析（同一模块 = 布局不可能漂移）。
        let req = encode_enqueue(
            3,
            b"root-task",
            b"health",
            123456789,
            b"health:req=20 opt=9 total=29",
        )
        .expect("encode");
        let f = parse_enqueue_req(&req).expect("parse");
        assert_eq!(f.class, 3);
        assert_eq!(&f.service[..9], b"root-task");
        assert_eq!(&f.schema[..6], b"health");
        assert_eq!(f.created_ms, 123456789);
        assert_eq!(f.payload, b"health:req=20 opt=9 total=29");
    }

    #[test]
    fn enqueue_rejects_oversize_payload() {
        let big = [0u8; 256];
        assert_eq!(
            encode_enqueue(3, b"s", b"x", 0, &big),
            Err(ObsWireError::PayloadTooLong(256))
        );
    }

    #[test]
    fn enqueue_parse_short_and_bad_magic() {
        assert_eq!(
            parse_enqueue_req(b"XXXX\x01\x03"),
            Err(ObsWireError::BadMagic)
        );
        let short = vec![b'O', b'B', b'S', b'1', 0x01, 3];
        assert_eq!(parse_enqueue_req(&short), Err(ObsWireError::Short(6)));
    }

    #[test]
    fn flush_round_trip_and_err_prefix_not_misread_as_number() {
        let exported = [42u64.to_le_bytes().as_slice(), b""].concat();
        assert_eq!(parse_flush_resp(&exported), Ok(42));
        assert_eq!(
            parse_flush_resp(b"ERR:exporter-offline"),
            Err(ObsWireError::FlushRejected(String::from(
                "ERR:exporter-offline"
            ))),
            "ERR 前缀不得误读为导出数字（50 实弹语义）"
        );
        assert_eq!(parse_flush_resp(&[1, 2]), Err(ObsWireError::Short(2)));
    }

    #[test]
    fn flush_request_shape() {
        assert_eq!(encode_flush(8), Vec::from(&b"OBS1\x02\x08\0\0\0"[..]));
    }

    /// WP-64.2.1c：ENQUEUE 应答约定锁（正负例——ack=1 受理 / ack=2 /
    /// ERR 前缀 / 空应答）。
    #[test]
    fn enqueue_resp_ack_and_rejection_conventions() {
        assert_eq!(parse_enqueue_resp(&[1]), Ok(()));
        assert_eq!(
            parse_enqueue_resp(&[1, 0, 0]),
            Ok(()),
            "首字节判据，尾部忽略"
        );
        assert_eq!(parse_enqueue_resp(&[2]), Err(ObsReject::NotAck(2)));
        assert_eq!(parse_enqueue_resp(&[]), Err(ObsReject::NotAck(0)));
        let err = b"ERR:queue-full";
        assert_eq!(parse_enqueue_resp(err), Err(ObsReject::Err(err)));
        assert_eq!(parse_enqueue_resp(&[0]), Err(ObsReject::NotAck(0)));
    }
}
