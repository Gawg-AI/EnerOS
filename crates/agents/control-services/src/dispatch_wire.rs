//! C3（2026-09-24 架构走查收口）：控制链出站/钉面 wire 的 **codec 半边**
//!——host 可测纯函数（`&[u8]` 进出）。
//!
//! 迁移前这些字节映射全部住在 `dispatch_assembly.rs` 的
//! `cfg(target_os = "none")` 区（`EipcMasterChannel` 的信封→drv 主站写
//! 请求 + readback 帧解析、`TelemetrySnapRetainer` 的钉面 17B 编解码与
//! 应答判真），host 测试结构性不可达——「Committed 是否真走过独立
//! readback」的判别逻辑只在 QEMU 可验。现字节逻辑全部收进本模块：
//! cfg(none) 区只留 IPC 拨号与证据行打印。判据逐条保持（SPEC-B 纪律：
//! 只统一实现，不动语义）。
//!
//! 同源（蓝图 76 WP-76.2.7）：钉面 17B 请求（`[op, snapshot_id ×16]`）的
//! 布局事实单源 = agent-sdk `tel_wire`——本模块构帧 `snap_request` 与服务端
//! `telemetry::handle_request` 三臂的解帧 `tel_wire::parse_snap_request`
//! 共用同一 [`SNAP_REQ_LEN`]（帧长与 id 偏移两侧各抄一遍的日子结束）。
//! 判别力不变：布局漂移两侧同时失效——锁测试在本模块
//! `snap_wire_two_side_interlock`（客户端构帧 ↔ 共享解帧往返）与
//! telemetry.rs `snap_wire_dual_side_lock`（端到端应答判真）。

use eneros_agent_sdk::tel_wire::{
    SNAP_REQ_LEN, TEL_REQ_SNAP_HOLD, TEL_REQ_SNAP_RELEASE, TEL_RESP_OK,
};
use eneros_control_gateway::envelope::CommandEnvelope;
use eneros_control_gateway::wire::ControlEnvelope;
use eneros_drv_svc::runtime::{wire, DriverRequest, OP_EXECUTE, ST_OK};

/// 信封→主站写请求的错误面（与通道层 `ChannelError::Protocol` 的证据词
/// 一一对应——adapter 侧据此重组，词形逐字不变）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterWriteRequestError {
    /// 外层信封解码失败（证据词 `env:{e:?}`）。
    Envelope(alloc::string::String),
    /// payload digest 复验失败（证据词 `payload-digest`）。
    PayloadDigest,
    /// 内层 CommandEnvelope 解码失败（证据词 `inner:{e:?}`）。
    Inner(alloc::string::String),
}

/// 主站写应答错误（与通道层 Rejected/BadResponse 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterOutcomeError {
    /// 服务端非 ST_OK（命令被拒）。
    Rejected,
    /// 应答帧畸形（解码失败 / payload_len ≠ 2）。
    BadResponse,
}

/// readback 请求帧：`OP_EXECUTE` + `encode_master_read_payload(device, point)`
///（correlation 由调用方管理——通道层单调自增语义不变）。
pub fn readback_request(correlation: u32, device: u32, point: u32) -> alloc::vec::Vec<u8> {
    let mut req = DriverRequest::new(correlation, OP_EXECUTE);
    let payload = eneros_protocol_abstract::write::encode_master_read_payload(device, point);
    req.payload[..payload.len()].copy_from_slice(&payload);
    req.payload_len = payload.len() as u16;
    wire::encode_request(&req).to_vec()
}

/// readback 应答 → 值。判据（与迁移前逐条相同）：容差解码
///（`decode_response_padded`，QEMU 4B/word 尾部 padding 单源）；ST_OK；
/// payload 恰 8B；小端 f64 且**有限**。
// 接缝契约同 telemetry_history::open_disk_history：错误面为 ()（通道层
// 统一折叠为 ChannelError::BadResponse，不消费细节）。
#[allow(clippy::result_unit_err)]
pub fn readback_value(resp_bytes: &[u8]) -> Result<f64, ()> {
    let resp = wire::decode_response_padded(resp_bytes).ok_or(())?;
    if resp.status != ST_OK || resp.payload_len != 8 {
        return Err(());
    }
    let value = f64::from_le_bytes(resp.payload[..8].try_into().map_err(|_| ())?);
    if !value.is_finite() {
        return Err(());
    }
    Ok(value)
}

/// 投递信封 → drv 主站写请求帧（V2 载荷 55B——typed value + 语义摘要，
/// codec 单源 = `protocol_abstract::write`）。判据：ControlEnvelope V2
/// 解码 → payload digest 复验 → 内层 CommandEnvelope 解码 →
/// `encode_master_write_payload_v2(device, point, value)` 装配。
pub fn master_write_request(
    correlation: u32,
    payload: &[u8],
) -> Result<alloc::vec::Vec<u8>, MasterWriteRequestError> {
    let ce = ControlEnvelope::decode(payload)
        .map_err(|e| MasterWriteRequestError::Envelope(alloc::format!("env:{e:?}")))?;
    if !ce.verify_payload_digest() {
        return Err(MasterWriteRequestError::PayloadDigest);
    }
    let env = CommandEnvelope::decode(&ce.envelope)
        .map_err(|e| MasterWriteRequestError::Inner(alloc::format!("inner:{e:?}")))?;
    let mut req = DriverRequest::new(correlation, OP_EXECUTE);
    let pld = eneros_protocol_abstract::write::encode_master_write_payload_v2(
        env.device_id,
        env.point_id,
        env.value,
    );
    req.payload[..pld.len()].copy_from_slice(&pld);
    req.payload_len = pld.len() as u16;
    Ok(wire::encode_request(&req).to_vec())
}

/// 主站写应答 → 2B `OutcomeFrame`。判据顺序（与迁移前逐条相同）：
/// 容差解码失败 → BadResponse；status ≠ ST_OK → Rejected（即使长度
/// 同时也不符——拒绝优先，与「命令被拒」证据面一致）；payload_len ≠ 2
/// → BadResponse。
pub fn master_write_outcome(resp_bytes: &[u8]) -> Result<[u8; 2], MasterOutcomeError> {
    let resp = wire::decode_response_padded(resp_bytes).ok_or(MasterOutcomeError::BadResponse)?;
    if resp.status != ST_OK {
        return Err(MasterOutcomeError::Rejected);
    }
    if resp.payload_len != 2 {
        return Err(MasterOutcomeError::BadResponse);
    }
    Ok([resp.payload[0], resp.payload[1]])
}

/// 钉面请求（[`SNAP_REQ_LEN`]B = `[op, snapshot_id×16]`）——hold/release
/// 共用装配，与服务端解帧 `tel_wire::parse_snap_request` 的长度门同源
/// （蓝图 76 WP-76.2.7：帧长单一事实 = `SNAP_REQ_LEN`）。
pub fn snap_hold_request(snapshot_id: [u8; 16]) -> alloc::vec::Vec<u8> {
    snap_request(TEL_REQ_SNAP_HOLD, snapshot_id)
}

/// 钉面释放请求（同 [`SNAP_REQ_LEN`]B 布局）。
pub fn snap_release_request(snapshot_id: [u8; 16]) -> alloc::vec::Vec<u8> {
    snap_request(TEL_REQ_SNAP_RELEASE, snapshot_id)
}

fn snap_request(op: u8, snapshot_id: [u8; 16]) -> alloc::vec::Vec<u8> {
    let mut req = alloc::vec::Vec::with_capacity(SNAP_REQ_LEN);
    req.push(op);
    req.extend_from_slice(&snapshot_id);
    req
}

/// 钉面应答判真（首字节 `TEL_RESP_OK`——hold/release 服务端应答形状
/// `[TEL_RESP_OK]` / `[TEL_RESP_ERR]` 的客户端判据）。
pub fn snap_resp_ok(resp: &[u8]) -> bool {
    resp.first() == Some(&TEL_RESP_OK)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_control_gateway::envelope::EnvelopeBuilder;
    use eneros_drv_svc::runtime::{wire, DriverResponse, OP_EXECUTE, ST_INVALID, ST_OK};
    use eneros_telemetry_model::command::{CommandValue, ControlCommand, SingleCommand};

    use super::*;

    /// readback 请求帧装配 + 应答判据五臂（迁移前 cfg(none) 逐条同款，
    /// host 首次可测）。
    #[test]
    fn readback_request_roundtrip_and_value_judgments() {
        // 请求：OP_EXECUTE + master read payload，correlation 透传。
        let raw = readback_request(41, 7, 1001);
        let req = wire::decode_request(&raw).expect("请求帧可解码");
        assert_eq!(req.correlation, 41);
        assert_eq!(req.opcode, OP_EXECUTE);
        let expect = eneros_protocol_abstract::write::encode_master_read_payload(7, 1001);
        assert_eq!(&req.payload[..expect.len()], &expect[..]);
        assert_eq!(req.payload_len as usize, expect.len());

        // 应答判据：ST_OK + 恰 8B + 有限 f64，任一不满足 = Err。
        fn resp_of(status: u8, payload: &[u8]) -> alloc::vec::Vec<u8> {
            let mut r = DriverResponse::new(1, 0, 1);
            r.status = status;
            r.payload_len = payload.len() as u16;
            r.payload[..payload.len()].copy_from_slice(payload);
            wire::encode_response(&r).to_vec()
        }
        // ① OK + 8B 有限值。
        assert_eq!(
            readback_value(&resp_of(ST_OK, &42.5f64.to_le_bytes())),
            Ok(42.5)
        );
        // ② 非 OK。
        assert!(readback_value(&resp_of(ST_INVALID, &42.5f64.to_le_bytes())).is_err());
        // ③ OK 但长度 ≠ 8。
        assert!(readback_value(&resp_of(ST_OK, &[1u8; 4])).is_err());
        // ④ OK + 8B 但 NaN（非有限）——读回值不可信。
        assert!(readback_value(&resp_of(ST_OK, &f64::NAN.to_le_bytes())).is_err());
        // ⑤ 畸形帧。
        assert!(readback_value(&[0xA5u8; 5]).is_err());
    }

    /// 信封→主站写请求：字段映射 + digest 篡改负例（host 首次可测——
    /// 迁移前此映射只在 cfg(none) 区）。
    #[test]
    fn master_write_request_maps_envelope_and_rejects_tampered_digest() {
        let inner = EnvelopeBuilder::new(
            [9u8; 16],
            1,
            10,
            ControlCommand::Single(SingleCommand::On),
            1_000,
            60_000,
            [0x41u8; 16],
        )
        .value(CommandValue::ActivePowerKw(-12.5))
        .build(b"wire-test-key")
        .encode();
        let ce = ControlEnvelope::new_v3([7u8; 16], [0x41; 16], 0, 1_000, 61_000, &inner);
        let raw = master_write_request(9, &ce.encode()).expect("合法信封可装配");

        let req = wire::decode_request(&raw).expect("请求帧可解码");
        assert_eq!(req.correlation, 9);
        assert_eq!(req.opcode, OP_EXECUTE);
        let expect =
            eneros_protocol_abstract::write::encode_master_write_payload_v2(1, 10, env_value());
        assert_eq!(
            &req.payload[..expect.len()],
            &expect[..],
            "device/point/value → V2 载荷"
        );

        // 篡改 digest 域内一字节 → 专用证据词（payload-digest）。
        let mut tampered = ce.encode();
        tampered[60] ^= 0xFF; // payload_digest 域（offsets 58..90）
        assert_eq!(
            master_write_request(9, &tampered),
            Err(MasterWriteRequestError::PayloadDigest)
        );

        // 非信封字节 → Envelope 臂（证据词 `env:` 前缀）。
        let err = master_write_request(9, &[0u8; 8]).unwrap_err();
        assert!(
            matches!(err, MasterWriteRequestError::Envelope(ref s) if s.starts_with("env:")),
            "{err:?}"
        );
    }

    /// 写应答判据：拒优先于长度（与迁移前顺序逐条相同）。
    #[test]
    fn master_write_outcome_rejected_beats_bad_length() {
        fn resp_of(status: u8, payload: &[u8]) -> alloc::vec::Vec<u8> {
            let mut r = DriverResponse::new(1, 0, 1);
            r.status = status;
            r.payload_len = payload.len() as u16;
            r.payload[..payload.len()].copy_from_slice(payload);
            wire::encode_response(&r).to_vec()
        }
        assert_eq!(
            master_write_outcome(&resp_of(ST_OK, &[0x11, 0x22])),
            Ok([0x11, 0x22])
        );
        // 非 OK 且长度也不符 → Rejected（不是 BadResponse）。
        assert_eq!(
            master_write_outcome(&resp_of(ST_INVALID, &[0x11])),
            Err(MasterOutcomeError::Rejected)
        );
        // OK 但长度 ≠ 2 → BadResponse。
        assert_eq!(
            master_write_outcome(&resp_of(ST_OK, &[0x11])),
            Err(MasterOutcomeError::BadResponse)
        );
        // 畸形帧。
        assert_eq!(
            master_write_outcome(&[0xA5u8; 5]),
            Err(MasterOutcomeError::BadResponse)
        );
    }

    /// 钉面 17B 字节形状 + 客户端判真（服务端往返半边在 telemetry.rs
    /// `snap_wire_dual_side_lock`——两侧共用本模块编码器）。
    #[test]
    fn snap_wire_client_side_shape_and_judgment() {
        let id = [0x5Au8; 16];
        let hold = snap_hold_request(id);
        assert_eq!(hold.len(), 17);
        assert_eq!(hold[0], TEL_REQ_SNAP_HOLD);
        assert_eq!(&hold[1..], &id[..]);
        let release = snap_release_request(id);
        assert_eq!(release.len(), 17);
        assert_eq!(release[0], TEL_REQ_SNAP_RELEASE);
        assert_eq!(&release[1..], &id[..]);
        // 判真：首字节 OK；ERR / 空应答为假。
        assert!(snap_resp_ok(&[TEL_RESP_OK]));
        assert!(!snap_resp_ok(&[eneros_agent_sdk::tel_wire::TEL_RESP_ERR]));
        assert!(!snap_resp_ok(&[]));
    }

    /// 两面互锁（蓝图 76 WP-76.2.7）：客户端构帧喂**同一份**共享解帧
    /// `tel_wire::parse_snap_request`（服务端三臂用它）必须原样取回 id，
    /// hold/release 两码各自逐字节等于手抄 `[u8; 17]` 字面量（wire 冻结）。
    /// 布局任一侧漂移（帧长 / op / id 偏移）本测即红——双面「各抄一遍后
    /// 碰巧一致」的旧形态不复存在。
    #[test]
    fn snap_wire_two_side_interlock() {
        use eneros_agent_sdk::tel_wire::parse_snap_request;
        let id: [u8; 16] = [
            0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18, 0x29, 0x3A, 0x4B, 0x5C, 0x6D, 0x7E,
            0x8F, 0x90,
        ];
        // 手抄期望帧（与 tel_wire `snap_request_byte_layout_frozen` 同值）。
        const EXPECTED_HOLD: [u8; 17] = [
            0x07, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18, 0x29, 0x3A, 0x4B, 0x5C, 0x6D,
            0x7E, 0x8F, 0x90,
        ];
        const EXPECTED_RELEASE: [u8; 17] = [
            0x08, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18, 0x29, 0x3A, 0x4B, 0x5C, 0x6D,
            0x7E, 0x8F, 0x90,
        ];

        let hold = snap_hold_request(id);
        assert_eq!(&hold[..], &EXPECTED_HOLD[..], "hold 请求逐字节冻结");
        assert_eq!(parse_snap_request(&hold), Some(id), "构帧 → 解帧往返");

        let release = snap_release_request(id);
        assert_eq!(
            &release[..],
            &EXPECTED_RELEASE[..],
            "release 请求逐字节冻结"
        );
        assert_eq!(parse_snap_request(&release), Some(id));

        // 负例：帧长漂移（少一字节）→ 共享解帧 None → 服务端三臂回 ERR。
        assert_eq!(parse_snap_request(&hold[..hold.len() - 1]), None);
    }

    /// 与装配侧一致的 CommandValue（V2 载荷对照单源——同构造不同处断言
    /// 时用同一值）。
    fn env_value() -> CommandValue {
        CommandValue::ActivePowerKw(-12.5)
    }
}
