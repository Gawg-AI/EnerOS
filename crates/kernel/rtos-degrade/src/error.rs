//! 降级引擎错误类型.
//!
//! 定义 [`DegradeError`]，覆盖点写入失败、设备映射缺失、安全默认值缺失三类错误。

use eneros_protocol_abstract::ProtocolError;
use eneros_upa_model::PointId;

/// 降级引擎错误（3 变体）.
#[derive(Debug)]
pub enum DegradeError {
    /// 点写入失败（协议层返回错误）。
    PointWriteFailed(ProtocolError),
    /// 设备映射缺失（DevicePointMap 为空但需要遍历下发）。
    NoDeviceMap,
    /// 安全默认值缺失（SafeDefaults 中找不到指定点）。
    SafeDefaultMissing(PointId),
}

impl core::fmt::Display for DegradeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DegradeError::PointWriteFailed(e) => {
                write!(f, "point write failed: {:?}", e)
            }
            DegradeError::NoDeviceMap => write!(f, "device point map is empty"),
            DegradeError::SafeDefaultMissing(pid) => {
                write!(f, "safe default missing for point {}", pid)
            }
        }
    }
}
