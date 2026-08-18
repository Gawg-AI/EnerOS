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
    pub protocol_id: &'static str,
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
        protocol_id: &'static str,
        device_id: DeviceId,
        point_id: PointId,
        value: PointValue,
        sequence: u64,
        issued_ms: u64,
    ) -> Self {
        Self {
            protocol_id,
            device_id,
            point_id,
            value,
            sequence,
            issued_ms,
        }
    }
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
}
