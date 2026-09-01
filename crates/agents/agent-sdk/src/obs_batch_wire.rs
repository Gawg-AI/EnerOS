//! 蓝图 62 卷 62.2 WP-62.2.2（S2b）：OBS 观测批次 wire（TEL_INGEST_OBS）
//! —— agent-sdk 单一事实源（sample_wire / history_wire 先例）。
//!
//! 单一事实：svc-modbus-tcp / svc-iec104 两 bin 的请求手拼
//! （`req.push(4u8)` + LE 头）、控制服务 telemetry 的请求手解与响应手拼、
//! 以及"单次 chunk 传输帧上限"算式（隐式依赖 7B 头知识）此前各写一遍，
//! 消费侧权威常量（telemetry `OBS_REQ_HEADER_LEN`/`OBS_RESP_LEN`）零引用。
//! 本模块把编码/解码/分批预算收归一处，**布局逐字节不变**。
//!
//! 布局（小端）：
//! - 请求：`[opcode=4][source_id u32][count u16][n × 128B 帧]`；
//! - 响应：`[OK=0][accepted u16][rejected u16]`（其余首字节/短帧 = 拒绝）；
//! - 单次 chunk 传输帧上限 =
//!   (CHUNK_COUNT_MAX × CHUNK_DATA_MAX − 请求头 7B) / 128B = 12 帧
//!   （超限由 [`encode_obs_request`] 结构性拒绝，不再依赖调用方编排自觉）。

use alloc::vec::Vec;

use eneros_telemetry_model::event::OBSERVATION_WIRE_LEN;

use crate::chunked;

/// TEL_INGEST_OBS opcode（telemetry 分发表权威码位；协议服务/控制服务
/// 共同消费本值——两 bin 手拼点删除后各自引用此处）。
pub const OPCODE_TEL_INGEST_OBS: u8 = 4;

/// 请求头长（opcode 1B + source u32 + count u16 = 7B；原
/// telemetry `OBS_REQ_HEADER_LEN` 值，单源迁入）。
pub const REQ_HEADER_LEN: usize = 1 + 4 + 2;

/// 响应帧长（OK 1B + accepted u16 + rejected u16 = 5B；原
/// telemetry `OBS_RESP_LEN` 值，单源迁入）。
pub const RESP_LEN: usize = 1 + 2 + 2;

/// 单帧 wire 长度 = ObservationEvent wire（单一事实 re-export）。
pub const FRAME_WIRE_LEN: usize = OBSERVATION_WIRE_LEN;

/// 单次 chunk 传输帧上限（原两 bin `OBS_FRAMES_PER_TRANSFER` 双抄的
/// 单源收编；算式显式依赖 [`REQ_HEADER_LEN`]，不再藏魔数 7）。
pub const FRAMES_PER_TRANSFER: usize =
    (chunked::CHUNK_COUNT_MAX as usize * chunked::CHUNK_DATA_MAX - REQ_HEADER_LEN) / FRAME_WIRE_LEN;

const _: () = assert!(FRAMES_PER_TRANSFER >= 1);

/// OBS 批次 wire 错误（诚实拒绝，不截断不静默）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsBatchError {
    /// 编码：帧数超单次 chunk 传输预算。
    TooManyFrames(usize),
    /// 编码（流门面）：输入长度非帧长整除。
    NotFrameAligned(usize),
    /// 解码请求：短于请求头。
    ShortRequest(usize),
    /// 解码请求：count 声明与实际帧字节不符。
    CountMismatch { declared: usize, have: usize },
    /// 解码响应：短于响应帧。
    ShortResponse(usize),
}

/// 编码 OBS 批次请求（逐帧引用形态）。
///
/// `frames.len() > [`FRAMES_PER_TRANSFER`]` →
/// [`ObsBatchError::TooManyFrames`]（单次 chunk 传输 1664B 逻辑上限的
/// 结构性防线；分批是调用方编排，但超限请求编码侧直接拒绝）。
pub fn encode_obs_request(
    source_id: u32,
    frames: &[&[u8; FRAME_WIRE_LEN]],
) -> Result<Vec<u8>, ObsBatchError> {
    if frames.len() > FRAMES_PER_TRANSFER {
        return Err(ObsBatchError::TooManyFrames(frames.len()));
    }
    let mut req = Vec::with_capacity(REQ_HEADER_LEN + frames.len() * FRAME_WIRE_LEN);
    req.push(OPCODE_TEL_INGEST_OBS);
    req.extend_from_slice(&source_id.to_le_bytes());
    req.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    for f in frames {
        req.extend_from_slice(f.as_slice());
    }
    Ok(req)
}

/// 编码 OBS 批次请求（连续帧流门面——采集交付半环（WP-62.2.3
/// telemetry_uplink）上行持有 `drain_frames` 拼接流；逐字节一致性与
/// [`encode_obs_request`] 由 roundtrip 测试锁定）。
pub fn encode_obs_request_stream(source_id: u32, wire: &[u8]) -> Result<Vec<u8>, ObsBatchError> {
    if wire.len() % FRAME_WIRE_LEN != 0 {
        return Err(ObsBatchError::NotFrameAligned(wire.len()));
    }
    let n = wire.len() / FRAME_WIRE_LEN;
    if n > FRAMES_PER_TRANSFER {
        return Err(ObsBatchError::TooManyFrames(n));
    }
    let mut req = Vec::with_capacity(REQ_HEADER_LEN + wire.len());
    req.push(OPCODE_TEL_INGEST_OBS);
    req.extend_from_slice(&source_id.to_le_bytes());
    req.extend_from_slice(&(n as u16).to_le_bytes());
    req.extend_from_slice(wire);
    Ok(req)
}

/// 解码 OBS 批次请求（控制服务分发臂消费）。
///
/// 返回 `(source_id, 帧流)`；帧流长度恒为 `count × FRAME_WIRE_LEN`（布局
/// 校验失败 → [`ObsBatchError`]，对应服务端原 0xFF 拒绝语义）。
pub fn decode_obs_request(req: &[u8]) -> Result<(u32, &[u8]), ObsBatchError> {
    let Some(body) = req.get(1..) else {
        return Err(ObsBatchError::ShortRequest(req.len()));
    };
    if body.len() < 6 {
        return Err(ObsBatchError::ShortRequest(req.len()));
    }
    let source_id = u32::from_le_bytes(body[0..4].try_into().expect("4B 切片"));
    let count = u16::from_le_bytes(body[4..6].try_into().expect("2B 切片")) as usize;
    let frames = &body[6..];
    if frames.len() != count * FRAME_WIRE_LEN {
        return Err(ObsBatchError::CountMismatch {
            declared: count,
            have: frames.len(),
        });
    }
    Ok((source_id, frames))
}

/// 编码 OBS 批次响应（控制服务 obs_ingest 出口消费）。
pub fn encode_obs_response(accepted: u16, rejected: u16) -> [u8; RESP_LEN] {
    let mut out = [0u8; RESP_LEN];
    out[0] = 0; // OK
    out[1..3].copy_from_slice(&accepted.to_le_bytes());
    out[3..5].copy_from_slice(&rejected.to_le_bytes());
    out
}

/// 解码 OBS 批次响应（协议服务上行消费）：`Some((accepted, rejected))` =
/// OK 应答；`None` = 短帧/非 OK 首字节（调用方按原词形打 OBS-ROUND
/// rejected/eipc 行）。
pub fn decode_obs_response(resp: &[u8]) -> Option<(u32, u32)> {
    if resp.len() < RESP_LEN || resp[0] != 0 {
        return None;
    }
    Some((
        u32::from(u16::from_le_bytes([resp[1], resp[2]])),
        u32::from(u16::from_le_bytes([resp[3], resp[4]])),
    ))
}

// ============================================================================
// 红测→定义锁（WP-62.2.2 步骤 1：roundtrip + 分批上限边界）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(i: u8) -> [u8; FRAME_WIRE_LEN] {
        let mut f = [0u8; FRAME_WIRE_LEN];
        f[0] = i;
        f[FRAME_WIRE_LEN - 1] = 0xA5 ^ i;
        f
    }

    #[test]
    fn request_roundtrip_preserves_layout_byte_exact() {
        // 引用数组形态 → 流形态逐字节一致（双门面单实现）。
        let frames = [frame(1), frame(2), frame(3)];
        let refs: Vec<&[u8; FRAME_WIRE_LEN]> = frames.iter().collect();
        let by_refs = encode_obs_request(7, &refs).expect("3 帧 ≤ 上限");
        let mut stream = Vec::new();
        for f in &frames {
            stream.extend_from_slice(f);
        }
        let by_stream = encode_obs_request_stream(7, &stream).expect("流形态合法");
        assert_eq!(by_refs, by_stream);

        // 布局逐字节锁：[4][7 u32 LE][3 u16 LE][帧 × 3]。
        assert_eq!(by_refs[0], OPCODE_TEL_INGEST_OBS);
        assert_eq!(&by_refs[1..5], &7u32.to_le_bytes());
        assert_eq!(&by_refs[5..7], &3u16.to_le_bytes());
        assert_eq!(&by_refs[7..], &stream[..]);

        // 解码往返：source/count/帧字节全保真。
        let (source, wire) = decode_obs_request(&by_refs).expect("合法请求");
        assert_eq!(source, 7);
        assert_eq!(wire.len(), 3 * FRAME_WIRE_LEN);
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(&wire[i * FRAME_WIRE_LEN..(i + 1) * FRAME_WIRE_LEN], f);
        }
    }

    #[test]
    fn batch_limit_boundary_accepts_max_rejects_max_plus_one() {
        // 帧数 = 上限：编码通过（chunk 预算内：7B 头 + 12×128 ≤ 1664）。
        let max_frames: Vec<[u8; FRAME_WIRE_LEN]> =
            (0..FRAMES_PER_TRANSFER).map(|i| frame(i as u8)).collect();
        let refs: Vec<&[u8; FRAME_WIRE_LEN]> = max_frames.iter().collect();
        let req = encode_obs_request(1, &refs).expect("上限帧数必须通过");
        assert_eq!(
            req.len(),
            REQ_HEADER_LEN + FRAMES_PER_TRANSFER * FRAME_WIRE_LEN
        );

        // 上限 + 1：编码侧结构性拒绝（不依赖调用方分批自觉）。
        let over: Vec<[u8; FRAME_WIRE_LEN]> =
            (0..=FRAMES_PER_TRANSFER).map(|i| frame(i as u8)).collect();
        let refs: Vec<&[u8; FRAME_WIRE_LEN]> = over.iter().collect();
        assert_eq!(
            encode_obs_request(1, &refs),
            Err(ObsBatchError::TooManyFrames(FRAMES_PER_TRANSFER + 1))
        );
        let mut over_stream = Vec::new();
        for f in &over {
            over_stream.extend_from_slice(f);
        }
        assert_eq!(
            encode_obs_request_stream(1, &over_stream),
            Err(ObsBatchError::TooManyFrames(FRAMES_PER_TRANSFER + 1))
        );
    }

    #[test]
    fn decode_request_rejects_short_and_count_mismatch() {
        // 短于请求头（含空帧）。
        assert_eq!(decode_obs_request(&[]), Err(ObsBatchError::ShortRequest(0)));
        assert_eq!(
            decode_obs_request(&[OPCODE_TEL_INGEST_OBS, 1, 2, 3]),
            Err(ObsBatchError::ShortRequest(4))
        );
        // count 声明 2 但只带 1 帧。
        let mut req = vec![OPCODE_TEL_INGEST_OBS];
        req.extend_from_slice(&1u32.to_le_bytes());
        req.extend_from_slice(&2u16.to_le_bytes());
        req.extend_from_slice(&frame(9));
        assert_eq!(
            decode_obs_request(&req),
            Err(ObsBatchError::CountMismatch {
                declared: 2,
                have: FRAME_WIRE_LEN
            })
        );
    }

    #[test]
    fn response_roundtrip_and_reject_semantics() {
        // OK 响应往返。
        let resp = encode_obs_response(5, 7);
        assert_eq!(resp.len(), RESP_LEN);
        assert_eq!(decode_obs_response(&resp), Some((5, 7)));

        // 短帧 / 非 OK 首字节 → None（调用方按原词形打 rejected 行）。
        assert_eq!(decode_obs_response(&resp[..4]), None);
        let mut bad = resp;
        bad[0] = 0xFF;
        assert_eq!(decode_obs_response(&bad), None);
    }

    #[test]
    fn frames_per_transfer_formula_matches_retired_bin_constant() {
        // 退役常量算式锁：(16×104 − 7) / 128 = 12（两 bin 双抄值，值不变）。
        assert_eq!(REQ_HEADER_LEN, 7);
        assert_eq!(
            FRAMES_PER_TRANSFER,
            (chunked::CHUNK_COUNT_MAX as usize * chunked::CHUNK_DATA_MAX - 7) / FRAME_WIRE_LEN
        );
        assert_eq!(FRAMES_PER_TRANSFER, 12);
        // chunk 预算不越界：头 + 满批帧 ≤ 逻辑上限 1664B。
        assert!(
            REQ_HEADER_LEN + FRAMES_PER_TRANSFER * FRAME_WIRE_LEN
                <= chunked::CHUNK_COUNT_MAX as usize * chunked::CHUNK_DATA_MAX
        );
    }
}
