//! 驱动注册表（v0.43.0）.
//!
//! 提供全局驱动注册表，支持驱动的注册、发现（按 ID/类型/名称）与
//! 能力校验的访问控制。
//!
//! # 偏差声明
//! - D2: 使用 BTreeMap/BTreeSet（no_std 无 HashMap）
//! - D3: register() 接受 now: u64 参数注入时间戳（no_std 无系统时钟）
//! - D5: DriverStats 在框架内定义（蓝图引用但未定义）

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::handle::{DriverCapability, DriverHandle, DriverPermission};
use crate::{DeviceDriver, DriverError, DriverId, DriverType};

/// 驱动运行统计（D5）
#[derive(Clone, Debug, Default)]
pub struct DriverStats {
    /// open() 调用次数
    pub open_count: u32,
    /// 错误次数
    pub error_count: u32,
    /// 最近一次错误
    pub last_error: Option<DriverError>,
    /// IRQ 处理次数
    pub irq_count: u32,
}

impl DriverStats {
    /// 记录一次 open 调用
    pub fn record_open(&mut self) {
        self.open_count += 1;
    }
    /// 记录一次错误
    pub fn record_error(&mut self, err: DriverError) {
        self.error_count += 1;
        self.last_error = Some(err);
    }
    /// 记录一次中断
    pub fn record_irq(&mut self) {
        self.irq_count += 1;
    }
}

/// 注册表条目（内部结构）
struct DriverEntry {
    /// 驱动实例
    driver: Box<dyn DeviceDriver>,
    /// 访问所需权限
    required_perms: DriverPermission,
    /// 创建时间戳（D3：注入）
    created_at: u64,
    /// 运行统计
    stats: DriverStats,
}

/// 全局驱动注册表（D2：BTreeMap）
pub struct DriverRegistry {
    /// 驱动表：DriverId -> DriverEntry
    drivers: BTreeMap<DriverId, DriverEntry>,
    /// 类型索引：DriverType -> Vec<DriverId>
    type_index: BTreeMap<DriverType, Vec<DriverId>>,
    /// 名称索引：name -> DriverId
    name_index: BTreeMap<String, DriverId>,
}

impl DriverRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            drivers: BTreeMap::new(),
            type_index: BTreeMap::new(),
            name_index: BTreeMap::new(),
        }
    }

    /// 注册驱动（D3：now 参数注入时间戳）
    pub fn register(
        &mut self,
        driver: Box<dyn DeviceDriver>,
        required_perms: DriverPermission,
        now: u64,
    ) -> Result<DriverId, DriverError> {
        let id = *driver.id();
        let dtype = driver.driver_type();
        let name = driver.name().to_string();
        if self.drivers.contains_key(&id) {
            return Err(DriverError::AlreadyRegistered);
        }
        let entry = DriverEntry {
            driver,
            required_perms,
            created_at: now,
            stats: DriverStats::default(),
        };
        self.drivers.insert(id, entry);
        self.type_index.entry(dtype).or_default().push(id);
        self.name_index.insert(name, id);
        Ok(id)
    }

    /// 按 ID 查找
    pub fn find_by_id(&self, id: &DriverId) -> Option<DriverId> {
        self.drivers.contains_key(id).then_some(*id)
    }

    /// 按类型查找
    pub fn find_by_type(&self, dtype: DriverType) -> Vec<DriverId> {
        self.type_index.get(&dtype).cloned().unwrap_or_default()
    }

    /// 按名称查找
    pub fn find_by_name(&self, name: &str) -> Option<DriverId> {
        self.name_index.get(name).copied()
    }

    /// 打开驱动（能力校验）
    pub fn open(&self, id: &DriverId, cap: &DriverCapability) -> Result<DriverHandle, DriverError> {
        let entry = self.drivers.get(id).ok_or(DriverError::NotFound)?;
        if !cap.can_access(entry.required_perms) {
            return Err(DriverError::PermissionDenied);
        }
        // 注意：此处不可变借用 self，无法对 entry.stats 调用 record_open
        // open_count 统计需在调用方通过 record_open 更新，或此处省略
        Ok(DriverHandle::new(*id, *cap))
    }

    /// 注销驱动
    pub fn unregister(&mut self, id: &DriverId) -> Result<(), DriverError> {
        let entry = self.drivers.remove(id).ok_or(DriverError::NotRegistered)?;
        // 清理 type_index
        if let Some(vec) = self.type_index.get_mut(&entry.driver.driver_type()) {
            vec.retain(|x| x != id);
            if vec.is_empty() {
                self.type_index.remove(&entry.driver.driver_type());
            }
        }
        // 清理 name_index
        self.name_index.retain(|_, v| v != id);
        Ok(())
    }

    /// 列出所有驱动 ID
    pub fn list(&self) -> Vec<DriverId> {
        self.drivers.keys().copied().collect()
    }

    /// 驱动数量
    pub fn count(&self) -> usize {
        self.drivers.len()
    }

    /// 查询驱动统计
    pub fn stats(&self, id: &DriverId) -> Option<&DriverStats> {
        self.drivers.get(id).map(|e| &e.stats)
    }

    /// 获取驱动的创建时间戳
    pub fn created_at(&self, id: &DriverId) -> Option<u64> {
        self.drivers.get(id).map(|e| e.created_at)
    }

    /// 获取驱动所需权限
    pub fn required_perms(&self, id: &DriverId) -> Option<DriverPermission> {
        self.drivers.get(id).map(|e| e.required_perms)
    }

    /// 记录一次 open 调用（需可变借用）
    pub fn record_open(&mut self, id: &DriverId) {
        if let Some(entry) = self.drivers.get_mut(id) {
            entry.stats.record_open();
        }
    }
}

impl Default for DriverRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::string::String;

    use super::*;
    use crate::{DriverHealth, DriverState};

    /// 测试用最小 mock 驱动
    struct TestDriver {
        id: DriverId,
        name: String,
        dtype: DriverType,
        state: DriverState,
    }

    impl TestDriver {
        fn new(id: u64, name: &str, dtype: DriverType) -> Self {
            Self {
                id: DriverId(id),
                name: String::from(name),
                dtype,
                state: DriverState::Uninitialized,
            }
        }
    }

    impl DeviceDriver for TestDriver {
        fn id(&self) -> &DriverId {
            &self.id
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn driver_type(&self) -> DriverType {
            self.dtype
        }
        fn state(&self) -> DriverState {
            self.state
        }
        fn init(&mut self) -> Result<(), DriverError> {
            self.state = DriverState::Ready;
            Ok(())
        }
        fn start(&mut self) -> Result<(), DriverError> {
            self.state = DriverState::Running;
            Ok(())
        }
        fn stop(&mut self) -> Result<(), DriverError> {
            self.state = DriverState::Stopped;
            Ok(())
        }
        fn deinit(&mut self) -> Result<(), DriverError> {
            self.state = DriverState::Dead;
            Ok(())
        }
        fn handle_irq(&mut self, _irq_id: u32) {}
        fn health_check(&self) -> DriverHealth {
            DriverHealth::Healthy
        }
    }

    #[test]
    fn test_empty_registry() {
        let reg = DriverRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.list().is_empty());
    }

    #[test]
    fn test_register_success() {
        let mut reg = DriverRegistry::new();
        let driver = Box::new(TestDriver::new(1, "uart0", DriverType::Serial));
        let result = reg.register(driver, DriverPermission::OPEN, 100);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), DriverId(1));
        assert_eq!(reg.count(), 1);
        assert_eq!(reg.find_by_id(&DriverId(1)), Some(DriverId(1)));
    }

    #[test]
    fn test_register_duplicate() {
        let mut reg = DriverRegistry::new();
        let d1 = Box::new(TestDriver::new(1, "uart0", DriverType::Serial));
        let d2 = Box::new(TestDriver::new(1, "uart0_dup", DriverType::Serial));
        reg.register(d1, DriverPermission::OPEN, 100).unwrap();
        let result = reg.register(d2, DriverPermission::OPEN, 100);
        assert_eq!(result, Err(DriverError::AlreadyRegistered));
    }

    #[test]
    fn test_find_by_id_hit() {
        let mut reg = DriverRegistry::new();
        let driver = Box::new(TestDriver::new(5, "net0", DriverType::Network));
        reg.register(driver, DriverPermission::OPEN, 100).unwrap();
        assert_eq!(reg.find_by_id(&DriverId(5)), Some(DriverId(5)));
    }

    #[test]
    fn test_find_by_id_miss() {
        let reg = DriverRegistry::new();
        assert_eq!(reg.find_by_id(&DriverId(99)), None);
    }

    #[test]
    fn test_find_by_type() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        reg.register(
            Box::new(TestDriver::new(2, "uart1", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        reg.register(
            Box::new(TestDriver::new(3, "net0", DriverType::Network)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        let serials = reg.find_by_type(DriverType::Serial);
        assert_eq!(serials.len(), 2);
        assert!(serials.contains(&DriverId(1)));
        assert!(serials.contains(&DriverId(2)));
    }

    #[test]
    fn test_find_by_name() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        assert_eq!(reg.find_by_name("uart0"), Some(DriverId(1)));
        assert_eq!(reg.find_by_name("uart1"), None);
    }

    #[test]
    fn test_open_success() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        let cap = DriverCapability::new_full(1);
        let result = reg.open(&DriverId(1), &cap);
        assert!(result.is_ok());
        let handle = result.unwrap();
        assert_eq!(handle.id(), DriverId(1));
    }

    #[test]
    fn test_open_permission_denied() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::ALL,
            100,
        )
        .unwrap();
        let cap = DriverCapability::new_empty(1);
        let result = reg.open(&DriverId(1), &cap);
        assert_eq!(result, Err(DriverError::PermissionDenied));
    }

    #[test]
    fn test_open_not_found() {
        let reg = DriverRegistry::new();
        let cap = DriverCapability::new_full(1);
        let result = reg.open(&DriverId(99), &cap);
        assert_eq!(result, Err(DriverError::NotFound));
    }

    #[test]
    fn test_unregister_success() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        assert!(reg.unregister(&DriverId(1)).is_ok());
        assert_eq!(reg.find_by_id(&DriverId(1)), None);
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_unregister_not_registered() {
        let mut reg = DriverRegistry::new();
        let result = reg.unregister(&DriverId(99));
        assert_eq!(result, Err(DriverError::NotRegistered));
    }

    #[test]
    fn test_list_and_count() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        reg.register(
            Box::new(TestDriver::new(2, "uart1", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        reg.register(
            Box::new(TestDriver::new(3, "net0", DriverType::Network)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        let list = reg.list();
        assert_eq!(list.len(), 3);
        assert_eq!(reg.count(), 3);
    }

    #[test]
    fn test_stats_query() {
        let mut reg = DriverRegistry::new();
        reg.register(
            Box::new(TestDriver::new(1, "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            100,
        )
        .unwrap();
        let stats = reg.stats(&DriverId(1));
        assert!(stats.is_some());
        assert_eq!(stats.unwrap().open_count, 0);
        reg.record_open(&DriverId(1));
        assert_eq!(reg.stats(&DriverId(1)).unwrap().open_count, 1);
    }
}
