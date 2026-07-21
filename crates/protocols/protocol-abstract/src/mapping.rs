//! 协议点映射 — raw 值与工程量转换.
//!
//! [`ProtocolPointMapping`] 描述一个 UPA 点与底层协议地址的绑定关系，
//! 并提供线性工程量变换（`scale` / `offset`）：
//!
//! - `to_engineering(raw)` = `raw * scale + offset`
//! - `from_engineering(value)` = `((value - offset) / scale)` 取整

use eneros_upa_model::{DeviceId, PointId, PointType};

use crate::address::ProtocolAddress;

/// 协议点映射（点 ID ↔ 协议地址 + 工程量变换）.
///
/// 线性变换公式：`engineering = raw * scale + offset`。
#[derive(Debug, Clone)]
pub struct ProtocolPointMapping {
    /// 点唯一标识。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 底层协议地址。
    pub protocol_addr: ProtocolAddress,
    /// 点数据类型。
    pub data_type: PointType,
    /// 缩放系数（raw → engineering）。
    pub scale: f64,
    /// 零点偏移（raw → engineering）。
    pub offset: f64,
}

impl ProtocolPointMapping {
    /// 将原始整数值转换为工程量：`raw as f64 * scale + offset`。
    pub fn to_engineering(&self, raw: i64) -> f64 {
        raw as f64 * self.scale + self.offset
    }

    /// 将工程量转换回原始整数值：`((value - offset) / scale) as i64`。
    ///
    /// 注意：`scale` 为 0 时会得到 `inf`/`NaN`，调用方应确保 `scale != 0`。
    pub fn from_engineering(&self, value: f64) -> i64 {
        ((value - self.offset) / self.scale) as i64
    }
}
