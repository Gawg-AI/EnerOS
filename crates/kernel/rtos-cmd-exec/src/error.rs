//! ExecutorError — 命令执行器错误类型.

use eneros_controlbus::DeviceId;
use eneros_protocol_abstract::ProtocolError;

/// 命令执行器错误.
#[derive(Debug)]
pub enum ExecutorError {
    /// 点写入失败（协议层返回错误）.
    PointWriteFailed(ProtocolError),
    /// 设备未在 [`crate::device_map::DevicePointMap`] 中映射.
    DeviceNotMapped(DeviceId),
    /// 设备状态不可用.
    StateUnavailable(DeviceId),
}

impl core::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExecutorError::PointWriteFailed(e) => write!(f, "point write failed: {:?}", e),
            ExecutorError::DeviceNotMapped(d) => write!(f, "device {} not mapped", d.0),
            ExecutorError::StateUnavailable(d) => {
                write!(f, "state unavailable for device {}", d.0)
            }
        }
    }
}
