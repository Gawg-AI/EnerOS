//! v0.43.0 用户态驱动框架集成测试.
//!
//! 验证 MockDriver 生命周期、DriverRegistry 注册/发现/能力校验、
//! 驱动注销与统计查询等端到端行为。

use eneros_driver_framework::{
    DeviceDriver, DriverCapability, DriverHandle, DriverId, DriverPermission, DriverRegistry,
    DriverState, DriverType, MockDriver,
};

#[test]
fn test_mock_driver_lifecycle() {
    // init -> start -> stop -> deinit 全流程状态转换
    let mut driver = MockDriver::new(DriverId(1), "mock-uart0", DriverType::Serial);
    assert_eq!(driver.state(), DriverState::Uninitialized);

    driver.init().expect("init should succeed");
    assert_eq!(driver.state(), DriverState::Ready);

    driver.start().expect("start should succeed");
    assert_eq!(driver.state(), DriverState::Running);

    driver.stop().expect("stop should succeed");
    assert_eq!(driver.state(), DriverState::Stopped);

    driver.deinit().expect("deinit should succeed");
    assert_eq!(driver.state(), DriverState::Dead);
}

#[test]
fn test_registry_register_and_find_by_id() {
    let mut registry = DriverRegistry::new();
    let driver = MockDriver::new(DriverId(10), "uart0", DriverType::Serial);
    let id = registry
        .register(Box::new(driver), DriverPermission::OPEN, 1000)
        .expect("register should succeed");
    assert_eq!(id, DriverId(10));
    assert_eq!(registry.find_by_id(&DriverId(10)), Some(DriverId(10)));
    assert_eq!(registry.find_by_id(&DriverId(99)), None);
}

#[test]
fn test_registry_find_by_type() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(1), "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(2), "uart1", DriverType::Serial)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(3), "can0", DriverType::Can)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();

    let serials = registry.find_by_type(DriverType::Serial);
    assert_eq!(serials.len(), 2);
    assert!(serials.contains(&DriverId(1)));
    assert!(serials.contains(&DriverId(2)));

    let cans = registry.find_by_type(DriverType::Can);
    assert_eq!(cans.len(), 1);
    assert_eq!(cans[0], DriverId(3));
}

#[test]
fn test_registry_find_by_name() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(5), "eth0", DriverType::Network)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();

    assert_eq!(registry.find_by_name("eth0"), Some(DriverId(5)));
    assert_eq!(registry.find_by_name("eth1"), None);
}

#[test]
fn test_registry_duplicate_register() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(7), "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();
    // 重复注册同一 ID 应返回 AlreadyRegistered
    let result = registry.register(
        Box::new(MockDriver::new(
            DriverId(7),
            "uart0_dup",
            DriverType::Serial,
        )),
        DriverPermission::OPEN,
        0,
    );
    assert!(result.is_err());
    use eneros_driver_framework::DriverError;
    assert_eq!(result.unwrap_err(), DriverError::AlreadyRegistered);
}

#[test]
fn test_registry_open_with_capability() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(20), "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();

    // 拥有 OPEN 权限的令牌可打开
    let cap = DriverCapability::new(42, DriverPermission::OPEN);
    let handle: DriverHandle = registry
        .open(&DriverId(20), &cap)
        .expect("open with OPEN permission should succeed");
    assert_eq!(handle.id(), DriverId(20));
    assert_eq!(handle.cap().owner(), 42);
}

#[test]
fn test_registry_open_permission_denied() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(21), "uart1", DriverType::Serial)),
            DriverPermission::ALL,
            0,
        )
        .unwrap();

    // 无权限令牌无法打开要求 ALL 权限的驱动
    let empty_cap = DriverCapability::new_empty(99);
    let result = registry.open(&DriverId(21), &empty_cap);
    assert!(result.is_err());
    use eneros_driver_framework::DriverError;
    assert_eq!(result.unwrap_err(), DriverError::PermissionDenied);
}

#[test]
fn test_registry_open_not_found() {
    let registry = DriverRegistry::new();
    let cap = DriverCapability::new_full(1);
    let result = registry.open(&DriverId(999), &cap);
    assert!(result.is_err());
    use eneros_driver_framework::DriverError;
    assert_eq!(result.unwrap_err(), DriverError::NotFound);
}

#[test]
fn test_registry_unregister() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(30), "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            0,
        )
        .unwrap();
    assert_eq!(registry.count(), 1);

    // 注销后 find_by_id 返回 None
    registry
        .unregister(&DriverId(30))
        .expect("unregister should succeed");
    assert_eq!(registry.find_by_id(&DriverId(30)), None);
    assert_eq!(registry.count(), 0);

    // 再次注销返回 NotRegistered
    use eneros_driver_framework::DriverError;
    let result = registry.unregister(&DriverId(30));
    assert_eq!(result.unwrap_err(), DriverError::NotRegistered);
}

#[test]
fn test_registry_stats_and_list() {
    let mut registry = DriverRegistry::new();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(1), "uart0", DriverType::Serial)),
            DriverPermission::OPEN,
            5000,
        )
        .unwrap();
    registry
        .register(
            Box::new(MockDriver::new(DriverId(2), "uart1", DriverType::Serial)),
            DriverPermission::OPEN,
            6000,
        )
        .unwrap();

    // list 列出所有驱动
    let list = registry.list();
    assert_eq!(list.len(), 2);
    assert!(list.contains(&DriverId(1)));
    assert!(list.contains(&DriverId(2)));

    // count
    assert_eq!(registry.count(), 2);

    // stats 查询
    let stats = registry.stats(&DriverId(1)).expect("stats should exist");
    assert_eq!(stats.open_count, 0);

    // record_open 递增 open_count
    registry.record_open(&DriverId(1));
    let stats = registry.stats(&DriverId(1)).unwrap();
    assert_eq!(stats.open_count, 1);

    // stats 查询不存在的驱动返回 None
    assert!(registry.stats(&DriverId(99)).is_none());
}
