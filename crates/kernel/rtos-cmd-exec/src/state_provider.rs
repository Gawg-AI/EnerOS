//! DeviceStateProvider trait — 设备状态来源抽象（D3）.

use eneros_controlbus::{DeviceId, DeviceState};

/// 设备状态来源抽象.
///
/// 蓝图未定义此 trait（D3）；由调用方提供设备状态来源
/// （如采样快照、协议读取、缓存等），将 [`DeviceId`] 映射到 [`DeviceState`]，
/// 供 [`crate::executor::CommandExecutor`] 在约束检查时查询.
pub trait DeviceStateProvider {
    /// 返回指定设备的当前状态.
    fn device_state(&self, device: DeviceId) -> DeviceState;
}
