//! EnerOS RS485 串口驱动（v0.44.0）.
//!
//! 基于 v0.43.0 驱动框架（`DeviceDriver` trait）实现 RS485 半双工串口驱动，
//! 为 v0.45.0 Modbus RTU 主站提供物理层/链路层传输。
//!
//! # 核心类型
//! - [`driver::Rs485Driver`] — RS485 驱动，实现 `DeviceDriver` trait
//! - [`config::Rs485Config`] — RS485 配置结构（波特率/数据位/停止位/校验/地址）
//! - [`uart_hw::UartHw`] — UART 硬件抽象 trait（D1 偏差：HAL 无 HalUart）
//! - [`ring::RingBuffer`] — no_std 环形缓冲（D4 偏差：无外部依赖）
//!
//! # 偏差声明
//! - D1: 定义本地 `UartHw` trait（HAL 仅有 `HalSerial`，无 UART 专有方法）
//! - D2: `UartPort`/`StopBits`/`Parity`/`GpioPin` 在本 crate 内定义
//! - D3: `recv()` 接受 `now_ns: u64` 参数注入时间戳
//! - D4: `RingBuffer<T, const N: usize>` 本地实现
//! - D5: `DriverError::Timeout` 已在 v0.44.0 框架中添加
//! - D6: 使用 `core::sync::atomic::AtomicBool`
//! - D7: DE/RE 通过 `&'static dyn HalGpio` + `de_re_pin: Option<u32>` 控制
//! - D8: 延时由 `UartHw` 实现负责
//! - D9: 无 `tx_buffer` 字段（同步发送）
//! - D10: `recv()` 返回 `alloc::vec::Vec<u8>`
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零外部依赖（除 eneros-driver-framework/eneros-hal）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod driver;
pub mod ring;
pub mod uart_hw;

#[cfg(test)]
pub mod mock;

pub use config::{GpioPin, Parity, Rs485Config, StopBits, UartPort};
pub use driver::{Rs485Driver, Rs485Stats};
pub use ring::RingBuffer;
pub use uart_hw::UartHw;

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_driver_framework::{DeviceDriver, DriverHealth, DriverId, DriverState, DriverType};

    use super::*;
    use crate::mock::MockUartHw;

    /// 创建测试用 driver（默认配置，mock UART，无 DE/RE 引脚）
    fn make_driver() -> Rs485Driver {
        let mock = MockUartHw::new();
        let config = Rs485Config::default();
        Rs485Driver::new(DriverId(1), config, Box::new(mock))
    }

    /// 创建带 DE/RE 引脚的 driver
    fn make_driver_with_de_re() -> Rs485Driver {
        let mock = MockUartHw::new();
        let config = Rs485Config {
            de_re_pin: Some(42),
            ..Default::default()
        };
        Rs485Driver::new(DriverId(1), config, Box::new(mock))
    }

    // ===== T1: 状态转换测试 =====

    #[test]
    fn test_state_transitions() {
        let mut driver = make_driver();
        // 初始状态
        assert_eq!(driver.state(), DriverState::Uninitialized);
        assert_eq!(driver.driver_type(), DriverType::Serial);
        assert_eq!(driver.name(), "rs485-uart0");

        // init → Ready
        driver.init().expect("init should succeed");
        assert_eq!(driver.state(), DriverState::Ready);

        // start → Running
        driver.start().expect("start should succeed");
        assert_eq!(driver.state(), DriverState::Running);

        // stop → Stopped
        driver.stop().expect("stop should succeed");
        assert_eq!(driver.state(), DriverState::Stopped);

        // deinit → Dead
        driver.deinit().expect("deinit should succeed");
        assert_eq!(driver.state(), DriverState::Dead);
    }

    #[test]
    fn test_init_configures_uart_and_de_re() {
        let mut driver = make_driver_with_de_re();
        driver.init().expect("init should succeed");
        // init 成功即表示 configure + configure_de_re + set_de_re(false) 均成功
        assert_eq!(driver.state(), DriverState::Ready);
    }

    #[test]
    fn test_name_per_port() {
        let configs = [
            (UartPort::Uart0, "rs485-uart0"),
            (UartPort::Uart1, "rs485-uart1"),
            (UartPort::Uart2, "rs485-uart2"),
            (UartPort::Uart3, "rs485-uart3"),
        ];
        for (port, expected_name) in configs {
            let mock = MockUartHw::new();
            let config = Rs485Config {
                port,
                ..Default::default()
            };
            let driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));
            assert_eq!(driver.name(), expected_name);
        }
    }

    // ===== T2: send() 成功测试 =====

    #[test]
    fn test_send_success() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let result = driver.send(&data);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
        assert_eq!(driver.stats().rx_error_count, 0);
    }

    #[test]
    fn test_send_multiple_frames() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        driver.send(&[0x01]).expect("send 1");
        driver.send(&[0x02, 0x03]).expect("send 2");
        driver.send(&[0x04, 0x05, 0x06]).expect("send 3");
        assert_eq!(driver.stats().tx_count, 3);
    }

    #[test]
    fn test_send_with_de_re() {
        let mut driver = make_driver_with_de_re();
        driver.init().expect("init");
        driver.start().expect("start");

        let result = driver.send(&[0xAB, 0xCD]);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
    }

    #[test]
    fn test_send_empty_data() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        let result = driver.send(&[]);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
    }

    // ===== T3: send() 超时测试 =====

    #[test]
    fn test_send_timeout() {
        let mut mock = MockUartHw::new();
        mock.set_tx_timeout(true);
        let config = Rs485Config::default();
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let result = driver.send(&[0x01, 0x02]);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
        assert_eq!(driver.stats().rx_error_count, 1);
        assert_eq!(
            driver.stats().last_rx_error,
            Some(eneros_driver_framework::DriverError::Timeout)
        );
        assert_eq!(driver.stats().tx_count, 0); // 未成功发送
    }

    // ===== T4: recv() 成功测试 =====

    #[test]
    fn test_recv_success() {
        let mut mock = MockUartHw::new();
        mock.push_rx_slice(&[0x01, 0x02, 0x03]);
        let config = Rs485Config {
            frame_gap_ms: 4,
            ..Default::default()
        };
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 模拟 IRQ 接收数据到 rx_buffer
        driver.handle_irq(1);
        assert!(driver.take_irq_rx());

        // 接收帧（时间会自动推进超过 frame_gap_ms）
        let frame = driver.recv(100).expect("recv should succeed");
        assert_eq!(frame, vec![0x01, 0x02, 0x03]);
        assert_eq!(driver.stats().rx_count, 1);
    }

    #[test]
    fn test_recv_single_byte() {
        let mut mock = MockUartHw::new();
        mock.push_rx(0x42);
        let config = Rs485Config::default();
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");
        driver.handle_irq(1);

        let frame = driver.recv(100).expect("recv");
        assert_eq!(frame, vec![0x42]);
    }

    // ===== T5: recv() 超时测试 =====

    #[test]
    fn test_recv_timeout_empty_buffer() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        // 空缓冲，超时返回
        let result = driver.recv(1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
        assert_eq!(driver.stats().rx_error_count, 1);
        assert_eq!(driver.stats().rx_count, 0);
    }

    // ===== T6: handle_irq() 测试 =====

    #[test]
    fn test_handle_irq_matching() {
        let mut mock = MockUartHw::new();
        mock.push_rx_slice(&[0x10, 0x20, 0x30]);
        let config = Rs485Config::default();
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // IRQ 编号匹配（默认 rx_irq=1）
        driver.handle_irq(1);
        assert!(driver.take_irq_rx(), "irq_rx flag should be set");
    }

    #[test]
    fn test_handle_irq_non_matching() {
        let mut mock = MockUartHw::new();
        mock.push_rx_slice(&[0x10]);
        let config = Rs485Config::default();
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // IRQ 编号不匹配（rx_irq=1, 但传入 99）
        driver.handle_irq(99);
        assert!(!driver.take_irq_rx(), "irq_rx flag should NOT be set");
    }

    #[test]
    fn test_take_irq_rx_clears_flag() {
        let mut mock = MockUartHw::new();
        mock.push_rx(0x01);
        let config = Rs485Config::default();
        let mut driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        driver.handle_irq(1);
        assert!(driver.take_irq_rx());
        // 第二次取应该返回 false（已清除）
        assert!(!driver.take_irq_rx());
    }

    // ===== T7: health_check() 测试 =====

    #[test]
    fn test_health_check_healthy() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        assert_eq!(driver.health_check(), DriverHealth::Healthy);
    }

    #[test]
    fn test_health_check_degraded() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        // 触发 11 次错误（recv 超时）
        for _ in 0..11 {
            let _ = driver.recv(1);
        }
        assert_eq!(driver.stats().rx_error_count, 11);
        assert_eq!(driver.health_check(), DriverHealth::Degraded);
    }

    #[test]
    fn test_health_check_unhealthy() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        // 触发 101 次错误
        for _ in 0..101 {
            let _ = driver.recv(1);
        }
        assert_eq!(driver.stats().rx_error_count, 101);
        assert_eq!(driver.health_check(), DriverHealth::Unhealthy);
    }

    // ===== T9: trait object 兼容性测试 =====

    #[test]
    fn test_uart_hw_as_trait_object() {
        let mock: Box<dyn UartHw> = Box::new(MockUartHw::new());
        let config = Rs485Config::default();
        let driver = Rs485Driver::new(DriverId(42), config, mock);
        assert_eq!(driver.id(), &DriverId(42));
        assert_eq!(driver.driver_type(), DriverType::Serial);
    }

    #[test]
    fn test_driver_can_be_boxed_as_device_driver() {
        let mock = MockUartHw::new();
        let config = Rs485Config::default();
        let driver = Rs485Driver::new(DriverId(1), config, Box::new(mock));
        let _boxed: Box<dyn eneros_driver_framework::DeviceDriver> = Box::new(driver);
        // 如果编译通过，说明 Rs485Driver 满足 Send + Sync
    }

    // ===== 额外: config 测试 =====

    #[test]
    fn test_config_accessor() {
        let driver = make_driver();
        let config = driver.config();
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.local_addr, 1);
        assert_eq!(config.frame_gap_ms, 4);
    }

    #[test]
    fn test_stats_accessor() {
        let driver = make_driver();
        let stats = driver.stats();
        assert_eq!(stats.tx_count, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.rx_error_count, 0);
        assert_eq!(stats.last_rx_error, None);
    }
}
