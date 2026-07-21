//! 校准系数持久化抽象（v0.51.1）.
//!
//! 定义 [`CalibStore`] trait — 按 meter_id 存取校准系数的抽象接口，
//! 与 [`InMemoryCalibStore`] — 基于内存 BTreeMap 的默认实现（测试用）。
//!
//! > D9 偏差：`CalibStore` 不直接依赖文件系统，文件系统实现后置。

use alloc::collections::BTreeMap;

use crate::coeffs::CalibCoeffs;

/// 校准系数存储抽象
///
/// 按 `meter_id` 存取校准系数。具体后端可以是：
/// - 内存（[`InMemoryCalibStore`]，测试/启动期）
/// - 文件系统（littlefs2，后置实现）
/// - EEPROM/Flash（设备级，后置实现）
pub trait CalibStore {
    /// 读取指定电表的校准系数，不存在返回 `None`。
    fn load(&self, meter_id: u32) -> Option<CalibCoeffs>;

    /// 保存指定电表的校准系数（覆盖写）。
    fn save(&mut self, meter_id: u32, coeffs: &CalibCoeffs);
}

/// 内存校准系数存储（测试/启动期用）
///
/// 基于 `BTreeMap<u32, CalibCoeffs>` 的简单实现，重启后数据丢失。
/// 生产环境应替换为文件系统或 Flash 后端。
pub struct InMemoryCalibStore {
    store: BTreeMap<u32, CalibCoeffs>,
}

impl InMemoryCalibStore {
    /// 创建空存储。
    pub fn new() -> Self {
        Self {
            store: BTreeMap::new(),
        }
    }
}

impl Default for InMemoryCalibStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CalibStore for InMemoryCalibStore {
    fn load(&self, meter_id: u32) -> Option<CalibCoeffs> {
        self.store.get(&meter_id).cloned()
    }

    fn save(&mut self, meter_id: u32, coeffs: &CalibCoeffs) {
        self.store.insert(meter_id, coeffs.clone());
    }
}
