//! Device Agent 错误类型（D8：本地定义，映射到 AgentRuntimeError）.

use alloc::string::String;
use core::fmt;

use eneros_energy_market_agent::AgentRuntimeError;

/// 设备错误.
#[derive(Debug)]
pub enum DeviceError {
    /// 设备未找到.
    DeviceNotFound(String),
    /// 点位未找到.
    PointNotFound(String),
    /// 设备离线.
    DeviceOffline(String),
    /// 写入失败.
    WriteFailed(String),
    /// 读取失败.
    ReadFailed(String),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::DeviceNotFound(s) => write!(f, "device not found: {}", s),
            DeviceError::PointNotFound(s) => write!(f, "point not found: {}", s),
            DeviceError::DeviceOffline(s) => write!(f, "device offline: {}", s),
            DeviceError::WriteFailed(s) => write!(f, "write failed: {}", s),
            DeviceError::ReadFailed(s) => write!(f, "read failed: {}", s),
        }
    }
}

impl From<DeviceError> for AgentRuntimeError {
    fn from(e: DeviceError) -> Self {
        AgentRuntimeError::DeviceError(alloc::format!("{:?}", e))
    }
}
