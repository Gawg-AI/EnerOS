//! 协议点映射 — raw 值与工程量转换.
//!
//! [`ProtocolPointMapping`] 描述一个 UPA 点与底层协议地址的绑定关系，
//! 并提供线性工程量变换（`scale` / `offset`）：
//!
//! - `to_engineering(raw)` = `raw * scale + offset`
//! - `from_engineering(value)` = `((value - offset) / scale)` 取整
//!
//! 蓝图 61 [06] §2.2：映射同时承载身份绑定（[`PointIdentityBinding`] —
//! 单位、schema、资产绑定与映射世代），使任何可参与决策的观测能回答
//! "以什么 schema/单位/映射、绑定到哪个资产产生"。

use eneros_upa_model::{DeviceId, PointId, PointType};

use crate::address::ProtocolAddress;

/// 点映射身份绑定（蓝图 61 [06] §2.2 / [02] §4.1）.
///
/// `UNKNOWN`（全零）= 未绑定：观测可存档，但 `unit_code=0`/`asset_id=0`/
/// `schema_id=0` 的观测不得进入控制快照（[02] §3.1 硬门）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointIdentityBinding {
    /// `UnitCatalog` 封闭单位码（0 = 未知）。
    pub unit_code: u16,
    /// 注册 schema id（0 = 未注册）。
    pub schema_id: u32,
    /// schema 版本。
    pub schema_version: u16,
    /// 全局资产 ID（0 = 未绑定，永不进控制快照）。
    pub asset_id: u64,
    /// 点表/映射单调世代（0 = 未定代——装配期必须赋值才可控）。
    pub mapping_generation: u64,
}

impl PointIdentityBinding {
    /// 未绑定常量（测试设施/参考插件默认；生产路径必须由配置赋值）。
    pub const UNKNOWN: Self = Self {
        unit_code: 0,
        schema_id: 0,
        schema_version: 0,
        asset_id: 0,
        mapping_generation: 0,
    };

    /// 是否满足控制快照最低身份要求（单位已知 + 资产已绑 + schema 已注 +
    /// 世代已定）。
    pub const fn is_control_eligible(&self) -> bool {
        self.unit_code != 0
            && self.asset_id != 0
            && self.schema_id != 0
            && self.mapping_generation != 0
    }
}

/// 协议点映射（点 ID ↔ 协议地址 + 工程量变换 + 身份绑定）.
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
    /// 身份绑定（单位/schema/资产/世代；蓝图 61 v0.314.27）。
    pub identity: PointIdentityBinding,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_binding_not_control_eligible() {
        assert!(!PointIdentityBinding::UNKNOWN.is_control_eligible());
        let full = PointIdentityBinding {
            unit_code: 6,
            schema_id: 1,
            schema_version: 1,
            asset_id: 1,
            mapping_generation: 1,
        };
        assert!(full.is_control_eligible());
        // 任一缺项即失格。
        let no_asset = PointIdentityBinding {
            asset_id: 0,
            ..full
        };
        assert!(!no_asset.is_control_eligible());
    }
}
