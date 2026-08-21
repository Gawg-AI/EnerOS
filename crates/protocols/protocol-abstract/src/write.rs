//! 协议写意图（蓝图 39 WP-39.10）。
//!
//! 协议层把**写请求**标准化为 [`PointWriteIntent`] 输出——真正设备写
//! 仍必须经蓝图 40 的 ControlDispatch（validate → authorize → dispatch →
//! ACK → readback）。本类型是协议与安全写链之间的唯一意图载体；没有
//! 授权命令（`AuthorizedCommand`），意图无法执行（编译期 + 运行期双门）。

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
}
