//! 设备注册表 — DeviceAdapter trait + MockDevice + DeviceInfo + DeviceRegistry.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::device_type::DeviceType;
use crate::error::DeviceError;

/// 设备适配器 trait（D6：字符串点名读写）.
///
/// 定义设备数据访问接口。`read_point` / `write_point` 使用字符串点名，
/// 避免 v0.51.0 `PointAccess` 的 `PointId`/`DataPoint` 类型化 API 复杂性。
pub trait DeviceAdapter {
    /// 读取点位值.
    fn read_point(&mut self, name: &str) -> Result<f64, DeviceError>;
    /// 写入点位值.
    fn write_point(&mut self, name: &str, value: f64) -> Result<(), DeviceError>;
    /// 返回设备类型.
    fn device_type(&self) -> DeviceType;
    /// 设备是否在线.
    fn is_online(&self) -> bool;
}

/// Mock 设备（BTreeMap-backed，D10）.
pub struct MockDevice {
    /// 设备类型.
    device_type: DeviceType,
    /// 点位值映射.
    points: BTreeMap<String, f64>,
    /// 在线状态.
    online: bool,
}

impl MockDevice {
    /// 创建空设备（无点位，默认在线）.
    pub fn new(device_type: DeviceType) -> Self {
        Self {
            device_type,
            points: BTreeMap::new(),
            online: true,
        }
    }

    /// 链式添加点位.
    pub fn with_point(mut self, name: &str, value: f64) -> Self {
        self.points.insert(String::from(name), value);
        self
    }

    /// 设置点位值.
    pub fn set_point(&mut self, name: &str, value: f64) {
        self.points.insert(String::from(name), value);
    }

    /// 设置在线状态.
    pub fn set_online(&mut self, online: bool) {
        self.online = online;
    }
}

impl DeviceAdapter for MockDevice {
    fn read_point(&mut self, name: &str) -> Result<f64, DeviceError> {
        if !self.online {
            return Err(DeviceError::DeviceOffline(alloc::format!(
                "{:?}",
                self.device_type
            )));
        }
        self.points
            .get(name)
            .copied()
            .ok_or_else(|| DeviceError::PointNotFound(String::from(name)))
    }

    fn write_point(&mut self, name: &str, value: f64) -> Result<(), DeviceError> {
        if !self.online {
            return Err(DeviceError::DeviceOffline(alloc::format!(
                "{:?}",
                self.device_type
            )));
        }
        self.points.insert(String::from(name), value);
        Ok(())
    }

    fn device_type(&self) -> DeviceType {
        self.device_type
    }

    fn is_online(&self) -> bool {
        self.online
    }
}

/// 设备元信息（D11：简化，仅 device_type + adapter）.
pub struct DeviceInfo {
    /// 设备类型.
    pub device_type: DeviceType,
    /// 设备适配器.
    pub adapter: Box<dyn DeviceAdapter>,
}

/// 设备注册表（D12：BTreeMap-backed）.
pub struct DeviceRegistry {
    /// 设备名 → 设备信息映射.
    devices: BTreeMap<String, DeviceInfo>,
}

impl DeviceRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    /// 注册设备.
    pub fn register(
        &mut self,
        name: &str,
        device_type: DeviceType,
        adapter: Box<dyn DeviceAdapter>,
    ) {
        self.devices.insert(
            String::from(name),
            DeviceInfo {
                device_type,
                adapter,
            },
        );
    }

    /// 获取设备可变引用.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut DeviceInfo> {
        self.devices.get_mut(name)
    }

    /// 设备数量.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 可变迭代.
    pub fn iter_mut(&mut self) -> alloc::collections::btree_map::IterMut<'_, String, DeviceInfo> {
        self.devices.iter_mut()
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
