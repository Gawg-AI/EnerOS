//! 测试桩驱动（v0.43.0）.
//!
//! 提供 [`MockDriver`]，实现 [`DeviceDriver`] trait，用于驱动框架的单元测试
//! 与集成测试。支持可配置的状态转换失败与中断调用记录。

use alloc::string::String;
use alloc::vec::Vec;

use crate::{DeviceDriver, DriverError, DriverHealth, DriverId, DriverState, DriverType};

/// 测试桩驱动
///
/// 实现 [`DeviceDriver`] trait，用于驱动框架测试。
/// 支持配置 init/start 是否失败，并记录 IRQ 调用历史。
#[derive(Debug)]
pub struct MockDriver {
    id: DriverId,
    name: String,
    driver_type: DriverType,
    state: DriverState,
    irq_log: Vec<u32>,
    health: DriverHealth,
    init_fails: bool,
    start_fails: bool,
}

impl MockDriver {
    /// 创建 mock 驱动（初始状态 Uninitialized）
    pub fn new(id: DriverId, name: &str, driver_type: DriverType) -> Self {
        Self {
            id,
            name: String::from(name),
            driver_type,
            state: DriverState::Uninitialized,
            irq_log: Vec::new(),
            health: DriverHealth::Healthy,
            init_fails: false,
            start_fails: false,
        }
    }

    /// 设置健康状态
    pub fn set_health(&mut self, health: DriverHealth) {
        self.health = health;
    }

    /// 设置 init 是否失败
    pub fn set_init_fails(&mut self, fails: bool) {
        self.init_fails = fails;
    }

    /// 设置 start 是否失败
    pub fn set_start_fails(&mut self, fails: bool) {
        self.start_fails = fails;
    }

    /// 返回 IRQ 调用历史
    pub fn irq_log(&self) -> &[u32] {
        &self.irq_log
    }

    /// 返回当前状态（与 trait state() 一致，提供直接访问）
    pub fn current_state(&self) -> DriverState {
        self.state
    }
}

impl DeviceDriver for MockDriver {
    fn id(&self) -> &DriverId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn driver_type(&self) -> DriverType {
        self.driver_type
    }

    fn state(&self) -> DriverState {
        self.state
    }

    fn init(&mut self) -> Result<(), DriverError> {
        if self.init_fails {
            return Err(DriverError::InitFailed);
        }
        self.state = DriverState::Ready;
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        if self.start_fails {
            return Err(DriverError::StartFailed);
        }
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

    fn handle_irq(&mut self, irq_id: u32) {
        self.irq_log.push(irq_id);
    }

    fn health_check(&self) -> DriverHealth {
        self.health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_initial_state() {
        let driver = MockDriver::new(DriverId(1), "mock-uart", DriverType::Serial);
        assert_eq!(driver.id(), &DriverId(1));
        assert_eq!(driver.name(), "mock-uart");
        assert_eq!(driver.driver_type(), DriverType::Serial);
        assert_eq!(driver.state(), DriverState::Uninitialized);
        assert_eq!(driver.current_state(), DriverState::Uninitialized);
        assert_eq!(driver.health_check(), DriverHealth::Healthy);
        assert!(driver.irq_log().is_empty());
    }

    #[test]
    fn test_init_success() {
        let mut driver = MockDriver::new(DriverId(2), "mock-net", DriverType::Network);
        assert_eq!(driver.state(), DriverState::Uninitialized);
        let result = driver.init();
        assert!(result.is_ok());
        assert_eq!(driver.state(), DriverState::Ready);
        assert_eq!(driver.current_state(), DriverState::Ready);
    }

    #[test]
    fn test_init_fails() {
        let mut driver = MockDriver::new(DriverId(3), "mock-can", DriverType::Can);
        driver.set_init_fails(true);
        let result = driver.init();
        assert_eq!(result, Err(DriverError::InitFailed));
        // 失败时状态保持 Uninitialized
        assert_eq!(driver.state(), DriverState::Uninitialized);
    }

    #[test]
    fn test_start_success() {
        let mut driver = MockDriver::new(DriverId(4), "mock-storage", DriverType::Storage);
        driver.init().expect("init should succeed");
        assert_eq!(driver.state(), DriverState::Ready);
        let result = driver.start();
        assert!(result.is_ok());
        assert_eq!(driver.state(), DriverState::Running);
    }

    #[test]
    fn test_start_fails() {
        let mut driver = MockDriver::new(DriverId(5), "mock-gpio", DriverType::Gpio);
        driver.set_start_fails(true);
        let result = driver.start();
        assert_eq!(result, Err(DriverError::StartFailed));
    }

    #[test]
    fn test_stop() {
        let mut driver = MockDriver::new(DriverId(6), "mock-i2c", DriverType::I2c);
        driver.init().expect("init should succeed");
        driver.start().expect("start should succeed");
        assert_eq!(driver.state(), DriverState::Running);
        let result = driver.stop();
        assert!(result.is_ok());
        assert_eq!(driver.state(), DriverState::Stopped);
    }

    #[test]
    fn test_deinit() {
        let mut driver = MockDriver::new(DriverId(7), "mock-spi", DriverType::Spi);
        driver.init().expect("init should succeed");
        driver.start().expect("start should succeed");
        driver.stop().expect("stop should succeed");
        assert_eq!(driver.state(), DriverState::Stopped);
        let result = driver.deinit();
        assert!(result.is_ok());
        assert_eq!(driver.state(), DriverState::Dead);
    }

    #[test]
    fn test_handle_irq_records() {
        let mut driver = MockDriver::new(DriverId(8), "mock-custom", DriverType::Custom(100));
        assert!(driver.irq_log().is_empty());
        driver.handle_irq(1);
        driver.handle_irq(2);
        assert_eq!(driver.irq_log(), &[1, 2]);
    }

    #[test]
    fn test_health_check() {
        let mut driver = MockDriver::new(DriverId(9), "mock-health", DriverType::Serial);
        assert_eq!(driver.health_check(), DriverHealth::Healthy);
        driver.set_health(DriverHealth::Unhealthy);
        assert_eq!(driver.health_check(), DriverHealth::Unhealthy);
        driver.set_health(DriverHealth::Degraded);
        assert_eq!(driver.health_check(), DriverHealth::Degraded);
        driver.set_health(DriverHealth::Unknown);
        assert_eq!(driver.health_check(), DriverHealth::Unknown);
    }

    #[test]
    fn test_custom_driver_type_variant() {
        // 覆盖 Custom(u16) 携带值的场景
        let driver = MockDriver::new(DriverId(10), "mock-custom-42", DriverType::Custom(42));
        assert_eq!(driver.driver_type(), DriverType::Custom(42));
        assert_ne!(driver.driver_type(), DriverType::Custom(43));
    }
}
