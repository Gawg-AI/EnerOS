//! 协议写意图（蓝图 39 WP-39.10）。
//!
//! 协议层把**写请求**标准化为 [`PointWriteIntent`] 输出——真正设备写
//! 仍必须经蓝图 40 的 ControlDispatch（validate → authorize → dispatch →
//! ACK → readback）。本类型是协议与安全写链之间的唯一意图载体；没有
//! 授权命令（`AuthorizedCommand`），意图无法执行（编译期 + 运行期双门）。

use eneros_telemetry_model::command::CommandValue;
use eneros_upa_model::{DeviceId, PointId, PointValue};

/// 标准化写意图（协议层输出；设备写由 40 ControlDispatch 执行）。
#[derive(Debug, Clone, PartialEq)]
pub struct PointWriteIntent {
    /// 来源协议 id（descriptor.id）。
    pub protocol_id: alloc::string::String,
    /// 目标设备。
    pub device_id: DeviceId,
    /// 目标点。
    pub point_id: PointId,
    /// 写入值。
    pub value: PointValue,
    /// 单调序号（防重放；跨重启由上层持久化）。
    pub sequence: u64,
    /// 签发时间（毫秒）。
    pub issued_ms: u64,
}

impl PointWriteIntent {
    /// 构造（字段全显式——防止漏填）。
    pub fn new(
        protocol_id: &str,
        device_id: DeviceId,
        point_id: PointId,
        value: PointValue,
        sequence: u64,
        issued_ms: u64,
    ) -> Self {
        Self {
            protocol_id: alloc::string::String::from(protocol_id),
            device_id,
            point_id,
            value,
            sequence,
            issued_ms,
        }
    }
}

/// 47（WP-47.1 协议出站写）：主站写请求操作码（drv-svc 信封；payload =
/// device_id u32 LE ‖ point_id u32 LE ‖ power_kw f64 LE，16B）。
/// svc 间共享常量（客户端 = svc-control-dispatch，服务端 = svc-modbus-tcp）。
pub const OP_MASTER_WRITE: u8 = 0x11;

/// 主站写请求载荷定长：`[子码 OP_MASTER_WRITE]‖device u32 LE‖point u32 LE‖
/// power f64 LE` = 17B（SPEC-B WP-B.2：编/解码单源收编——历史双端各自
/// 手抄，见体检候选 6）。
pub const MASTER_WRITE_PAYLOAD_LEN: usize = 1 + 4 + 4 + 8;

/// 编码主站写请求载荷（17B；子码 + 三字段 LE）。
pub fn encode_master_write_payload(
    device_id: u32,
    point_id: u32,
    power_kw: f64,
) -> [u8; MASTER_WRITE_PAYLOAD_LEN] {
    let mut out = [0u8; MASTER_WRITE_PAYLOAD_LEN];
    out[0] = OP_MASTER_WRITE;
    out[1..5].copy_from_slice(&device_id.to_le_bytes());
    out[5..9].copy_from_slice(&point_id.to_le_bytes());
    out[9..17].copy_from_slice(&power_kw.to_le_bytes());
    out
}

/// 解码主站写请求载荷（长度/子码不符 → None，fail-closed 不猜）。
pub fn decode_master_write_payload(buf: &[u8]) -> Option<(u32, u32, f64)> {
    if buf.len() != MASTER_WRITE_PAYLOAD_LEN || buf[0] != OP_MASTER_WRITE {
        return None;
    }
    let device_id = u32::from_le_bytes(buf[1..5].try_into().ok()?);
    let point_id = u32::from_le_bytes(buf[5..9].try_into().ok()?);
    let power_kw = f64::from_le_bytes(buf[9..17].try_into().ok()?);
    Some((device_id, point_id, power_kw))
}

/// 61 v0.314.25 §3.3：命令语义摘要——签发侧（dispatch journal）与
/// writer 侧（svc-modbus-tcp 重算比对）共用同一实现（单一事实源）。
/// `SHA-256("eneros.command-semantic.v1" ‖ device u32 LE ‖ point u32 LE ‖
/// value_tag u8 ‖ value_bits 8B)`——只含同 key 重传不变语义内容。
pub fn semantic_command_digest(device_id: u32, point_id: u32, value: CommandValue) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut h = Sha256::new();
    h.update(b"eneros.command-semantic.v1");
    h.update(device_id.to_le_bytes());
    h.update(point_id.to_le_bytes());
    h.update(value.value_tag().to_le_bytes());
    h.update(value.value_bits());
    h.finalize().into()
}

/// 主站写请求载荷 V2（61 v0.314.25）：`[子码][ver=2][device u32][point u32]
/// [value_tag][value_bits 8B][command_digest 32B]` = 55B（≤ PAYLOAD_CAP 64）。
/// writer 侧以同函数重算语义摘要并比对——授权 50 写 60 必然检出。
pub const MASTER_WRITE_PAYLOAD_LEN_V2: usize = 1 + 1 + 4 + 4 + 1 + 8 + 32;

/// 编码 V2 主站写载荷（55B）。
pub fn encode_master_write_payload_v2(
    device_id: u32,
    point_id: u32,
    value: CommandValue,
) -> [u8; MASTER_WRITE_PAYLOAD_LEN_V2] {
    let mut out = [0u8; MASTER_WRITE_PAYLOAD_LEN_V2];
    out[0] = OP_MASTER_WRITE;
    out[1] = 2;
    out[2..6].copy_from_slice(&device_id.to_le_bytes());
    out[6..10].copy_from_slice(&point_id.to_le_bytes());
    out[10] = value.value_tag();
    out[11..19].copy_from_slice(&value.value_bits());
    let digest = semantic_command_digest(device_id, point_id, value);
    out[19..51].copy_from_slice(&digest);
    out
}

/// 解码 V2 主站写载荷（长度/版本/子码不符 → None；非有限浮点 → None）。
pub fn decode_master_write_payload_v2(buf: &[u8]) -> Option<(u32, u32, CommandValue, [u8; 32])> {
    if buf.len() != MASTER_WRITE_PAYLOAD_LEN_V2 || buf[0] != OP_MASTER_WRITE || buf[1] != 2 {
        return None;
    }
    let device_id = u32::from_le_bytes(buf[2..6].try_into().ok()?);
    let point_id = u32::from_le_bytes(buf[6..10].try_into().ok()?);
    let mut bits = [0u8; 8];
    bits.copy_from_slice(&buf[11..19]);
    let value = CommandValue::from_tag_bits(buf[10], bits)?;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&buf[19..51]);
    Some((device_id, point_id, value, digest))
}

/// writer 侧工程值投影（V2 载荷 → 设备点值；61 v0.314.25）。
pub fn command_value_to_point_value(value: CommandValue) -> PointValue {
    value.to_point_value()
}

/// 意图线码（47 WP-47.1：svc 间 EIPC 传输的定长载荷）。
///
/// 布局：protocol_id 16B 填充 ‖ device_id u32 LE ‖ point_id u32 LE ‖
/// value tag 1B（0=Float/1=Int/2=Bool，其余无效）‖ value 8B LE ‖
/// sequence u64 LE ‖ issued_ms u64 LE = 49B 定长。
pub const WRITE_INTENT_WIRE_LEN: usize = 16 + 4 + 4 + 1 + 8 + 8 + 8;

/// 意图线码（定长 49B；长度不符 → None，fail-closed 不猜）。
pub fn encode_intent(intent: &PointWriteIntent) -> [u8; WRITE_INTENT_WIRE_LEN] {
    let mut out = [0u8; WRITE_INTENT_WIRE_LEN];
    let id = intent.protocol_id.as_bytes();
    let n = core::cmp::min(16, id.len());
    out[..n].copy_from_slice(&id[..n]);
    out[16..20].copy_from_slice(&(intent.device_id as u32).to_le_bytes());
    out[20..24].copy_from_slice(&intent.point_id.to_le_bytes());
    let (tag, bytes): (u8, [u8; 8]) = match intent.value {
        PointValue::Float(f) => (0u8, f.to_le_bytes()),
        PointValue::Int(i) => (1u8, i.to_le_bytes()),
        PointValue::Bool(b) => (2u8, (if b { 1u64 } else { 0u64 }).to_le_bytes()),
        _ => return out, // 其他类型不可线传（协议写值域：数值/开关）。
    };
    out[24] = tag;
    out[25..33].copy_from_slice(&bytes);
    out[33..41].copy_from_slice(&intent.sequence.to_le_bytes());
    out[41..49].copy_from_slice(&intent.issued_ms.to_le_bytes());
    out
}

/// 解码意图线码（长度/值 tag 校验；无效 → None）。
pub fn decode_intent(buf: &[u8]) -> Option<PointWriteIntent> {
    if buf.len() != WRITE_INTENT_WIRE_LEN {
        return None;
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&buf[..16]);
    let end = id.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);
    let protocol_id = core::str::from_utf8(&id[..end]).ok()?;
    let device_id = u32::from_le_bytes(buf[16..20].try_into().ok()?)
        .try_into()
        .ok()?;
    let point_id = u32::from_le_bytes(buf[20..24].try_into().ok()?);
    let value = match buf[24] {
        0 => PointValue::Float(f64::from_le_bytes(buf[25..33].try_into().ok()?)),
        1 => PointValue::Int(i64::from_le_bytes(buf[25..33].try_into().ok()?)),
        2 => PointValue::Bool(buf[25] != 0),
        _ => return None,
    };
    let sequence = u64::from_le_bytes(buf[33..41].try_into().ok()?);
    let issued_ms = u64::from_le_bytes(buf[41..49].try_into().ok()?);
    Some(PointWriteIntent {
        protocol_id: alloc::string::String::from(protocol_id),
        device_id,
        point_id,
        value,
        sequence,
        issued_ms,
    })
}

/// 意图请求帧壳（蓝图 66 WP-66.3.1 单源；历史三处手拼收编：
/// protocol-services mqtt_io 编码侧 + control-services dispatch_service
/// 测试 helper）。字节面冻结（[00] §3.4）：
/// `[INTENT_REQUEST_OPCODE=2][intent wire 49B][actor = ActorKind::Ai as u8]`。
pub fn encode_intent_request(intent: &PointWriteIntent) -> alloc::vec::Vec<u8> {
    use eneros_telemetry_model::command::{ActorKind, INTENT_REQUEST_OPCODE};
    let mut out = alloc::vec::Vec::with_capacity(1 + WRITE_INTENT_WIRE_LEN + 1);
    out.push(INTENT_REQUEST_OPCODE);
    out.extend_from_slice(&encode_intent(intent));
    out.push(ActorKind::Ai as u8);
    out
}

/// 解码意图请求帧（opcode/长度/actor 语义校验；非法 → None，fail-closed）。
///
/// 返回 `(intent, actor byte)`——actor 语义裁决（Ai 唯一签发面）归调用方
/// （dispatch 侧错误分型词形保持逐字节不变：unknown opcode / intent
/// request length / bad intent wire / actor must be Ai）。
pub fn decode_intent_request(buf: &[u8]) -> Option<(PointWriteIntent, u8)> {
    use eneros_telemetry_model::command::INTENT_REQUEST_OPCODE;
    if buf.len() != 1 + WRITE_INTENT_WIRE_LEN + 1 || buf[0] != INTENT_REQUEST_OPCODE {
        return None;
    }
    let intent = decode_intent(&buf[1..1 + WRITE_INTENT_WIRE_LEN])?;
    Some((intent, buf[1 + WRITE_INTENT_WIRE_LEN]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_carries_all_fields() {
        let intent = PointWriteIntent::new(
            "modbus-rtu",
            3,
            PointId::from(10u32),
            PointValue::Float(2.5),
            7,
            1234,
        );
        assert_eq!(intent.protocol_id, "modbus-rtu");
        assert_eq!(intent.device_id, 3);
        assert_eq!(intent.point_id, PointId::from(10u32));
        assert_eq!(intent.value, PointValue::Float(2.5));
        assert_eq!(intent.sequence, 7);
        assert_eq!(intent.issued_ms, 1234);
    }

    /// 蓝图 66 WP-66.3.1 golden（R-66.d 词形锁先行）：请求帧字节面 =
    /// 旧手拼 `[2] ++ intent wire ++ [2]`（改造前布局逐字节固化；
    /// 51B 定长；mutation 敏感——任一字节漂移即红）。
    #[test]
    fn intent_request_frame_golden_bytes() {
        let intent = PointWriteIntent::new(
            "m",
            3,
            PointId::from(10u32),
            PointValue::Float(2.5),
            7,
            1234,
        );
        let frame = encode_intent_request(&intent);
        assert_eq!(frame.len(), 1 + WRITE_INTENT_WIRE_LEN + 1);
        let expected_hex = concat!(
            "02",                             // opcode = INTENT_REQUEST_OPCODE（值冻结）
            "6d",                             // protocol_id "m"
            "000000000000000000000000000000", // 16B 填充余量 15B
            "03000000",                       // device 3 LE
            "0a000000",                       // point 10 LE
            "00",                             // tag Float
            "0000000000000440",               // 2.5 f64 LE
            "0700000000000000",               // seq 7 LE
            "d204000000000000",               // issued_ms 1234 LE
            "02",                             // actor = ActorKind::Ai as u8
        );
        let mut got = alloc::string::String::new();
        for b in &frame {
            use core::fmt::Write as _;
            let _ = write!(got, "{b:02x}");
        }
        assert_eq!(got, expected_hex);
    }

    /// 蓝图 66 WP-66.3.1：帧壳往返（encode/decode 单源互逆）。
    #[test]
    fn intent_request_roundtrip() {
        use eneros_telemetry_model::command::ActorKind;
        let intent =
            PointWriteIntent::new("mqtt", 1, PointId::from(9u32), PointValue::Int(-3), 11, 77);
        let frame = encode_intent_request(&intent);
        let (back, actor) = decode_intent_request(&frame).expect("decode");
        assert_eq!(back.protocol_id, "mqtt");
        assert_eq!(back.device_id, 1);
        assert_eq!(back.point_id, PointId::from(9u32));
        assert_eq!(back.value, PointValue::Int(-3));
        assert_eq!(back.sequence, 11);
        assert_eq!(back.issued_ms, 77);
        assert_eq!(actor, ActorKind::Ai as u8);
    }

    /// 蓝图 66 WP-66.3.1：解码负例（opcode 漂移 / 短帧 / 长帧）。
    #[test]
    fn intent_request_decode_rejects_malformed() {
        let intent =
            PointWriteIntent::new("m", 1, PointId::from(1u32), PointValue::Bool(true), 1, 1);
        let mut frame = encode_intent_request(&intent);
        assert!(decode_intent_request(&frame).is_some());
        frame[0] = 3; // opcode 漂移
        assert!(decode_intent_request(&frame).is_none());
        assert!(decode_intent_request(&frame[..frame.len() - 1]).is_none());
        let mut long = frame;
        long[0] = 2;
        long.push(0);
        assert!(decode_intent_request(&long).is_none());
    }

    /// 47 WP-47.1：意图线码往返（Float/Int/Bool 三值域）。
    #[test]
    fn intent_wire_roundtrip_all_value_kinds() {
        for value in [
            PointValue::Float(50.0),
            PointValue::Float(-12.5),
            PointValue::Int(42),
            PointValue::Bool(true),
            PointValue::Bool(false),
        ] {
            let intent = PointWriteIntent::new("mqtt", 1, PointId::from(10u32), value, 3, 999);
            let buf = encode_intent(&intent);
            assert_eq!(buf.len(), WRITE_INTENT_WIRE_LEN);
            let back = decode_intent(&buf).expect("decode");
            assert_eq!(back.protocol_id, "mqtt");
            assert_eq!(back.device_id, 1);
            assert_eq!(back.point_id, intent.point_id);
            assert_eq!(back.value, intent.value);
            assert_eq!(back.sequence, 3);
            assert_eq!(back.issued_ms, 999);
        }
    }

    #[test]
    fn intent_wire_rejects_bad_length_and_tag() {
        assert!(decode_intent(&[0u8; WRITE_INTENT_WIRE_LEN - 1]).is_none());
        assert!(decode_intent(&[0u8; WRITE_INTENT_WIRE_LEN + 1]).is_none());
        let mut buf = encode_intent(&PointWriteIntent::new(
            "mqtt",
            1,
            PointId::from(10u32),
            PointValue::Float(1.0),
            1,
            1,
        ));
        buf[24] = 9; // 无效值 tag
        assert!(decode_intent(&buf).is_none());
    }

    /// SPEC-B（WP-B.2）：主站写 17B 载荷 codec 单源——双端（dispatch 编 /
    /// modbus-tcp 解）共用，长度与字段布局逐字节锁定（golden）。
    #[test]
    fn master_write_payload_roundtrip_golden() {
        let buf = encode_master_write_payload(0x1122_3344, 42, 12.5);
        assert_eq!(buf.len(), MASTER_WRITE_PAYLOAD_LEN);
        assert_eq!(buf[0], OP_MASTER_WRITE);
        assert_eq!(
            decode_master_write_payload(&buf),
            Some((0x1122_3344, 42, 12.5))
        );
        // golden 字段布局：子码+4B+4B+8B 逐字节。
        assert_eq!(buf[1..5], 0x1122_3344u32.to_le_bytes());
        assert_eq!(buf[5..9], 42u32.to_le_bytes());
        assert_eq!(buf[9..17], 12.5f64.to_le_bytes());
        // 负值/NaN 保真（wire 不做浮点裁决）。
        let neg = encode_master_write_payload(1, 2, -3.25);
        assert_eq!(decode_master_write_payload(&neg), Some((1, 2, -3.25)));
        let nan = encode_master_write_payload(1, 2, f64::NAN);
        assert!(decode_master_write_payload(&nan).unwrap().2.is_nan());
    }

    #[test]
    fn master_write_payload_rejects_bad_length_and_subcode() {
        assert!(decode_master_write_payload(&[0u8; MASTER_WRITE_PAYLOAD_LEN - 1]).is_none());
        assert!(decode_master_write_payload(&[0u8; MASTER_WRITE_PAYLOAD_LEN + 1]).is_none());
        let mut buf = encode_master_write_payload(1, 2, 3.0);
        buf[0] = 0x99; // 子码不符
        assert!(decode_master_write_payload(&buf).is_none());
    }
}
