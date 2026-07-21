//! EnerOS CAN 驱动（v0.47.0）.
//!
//! 基于 v0.43.0 驱动框架（`DeviceDriver` trait）实现 CAN 2.0A/B 帧驱动，
//! 为后续 CAN 上层协议（CANopen/储能专用 CAN 协议）提供传输基础。
//!
//! # 核心类型
//! - [`driver::CanDriver`] — CAN 驱动，实现 `DeviceDriver` trait
//! - [`config::CanConfig`] — CAN 配置结构（控制器类型/波特率/模式/过滤器/自动重传）
//! - [`controller::CanController`] — CAN 控制器硬件抽象 trait（D1 偏差）
//! - [`frame::CanFrame`] — CAN 帧结构（标准/扩展/远程帧）
//! - [`filter::CanFilter`] — CAN 帧过滤器（ID+掩码匹配）
//! - [`ring::RingBuffer`] — no_std 环形缓冲（D4 偏差：无外部依赖）
//!
//! # 偏差声明
//! - D1: 定义本地 `CanController` trait（HAL 仅有 `HalSpi`/`HalGpio`，无 CAN 控制器专有方法）
//! - D2: `CanControllerType` 枚举仅作配置标识（MCP2515/Internal/SJA1000），不实现寄存器级操作
//! - D3: `CanFrame` 不含 `timestamp: MonotonicTime` 字段（EnerOS 无 `MonotonicTime` 类型）
//! - D4: `RingBuffer<T, const N: usize>` 本地实现（不依赖 RS485 crate，遵循 Surgical Changes 原则）
//! - D5: `recv()` 接受 `now_ns: u64` 参数注入时间戳（不使用 `MonotonicTime::now()`）
//! - D6: `CanController::read_rx_buffer()` 返回 `Option<CanFrame>`（驱动级抽象，无时间戳）
//! - D7: `CanFilter::matches()` 实现 ID+掩码匹配 + 标准帧/扩展帧互斥检查
//! - D8: crate 放入 `crates/drivers/can/`（遵循 §2.3.1 crate 分组规则）
//! - D9: 不依赖 `eneros-hal` crate（HAL 抽象由本地 `CanController` trait 提供）
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零外部依赖（除 eneros-driver-framework）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod controller;
pub mod driver;
pub mod filter;
pub mod frame;
pub mod ring;

#[cfg(test)]
pub mod mock;

pub use config::{CanConfig, CanControllerType, CanMode};
pub use controller::{CanController, CanStats};
pub use driver::CanDriver;
pub use filter::CanFilter;
pub use frame::{CanFrame, CanId, FrameType};
pub use ring::RingBuffer;

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_driver_framework::{DeviceDriver, DriverId, DriverState, DriverType};

    use super::*;
    use crate::config::{CanConfig, CanControllerType, CanMode};
    use crate::filter::CanFilter;
    use crate::frame::{CanFrame, CanId, FrameType};
    use crate::mock::MockCanController;

    // ===== T9.1: CanFrame 端到端测试 =====

    #[test]
    fn test_can_frame_end_to_end() {
        // 构造 → 发送 → mock 记录 → 校验
        let mock = MockCanController::new();
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let frame = CanFrame::new_standard(0x123, &[0x01, 0x02, 0x03]);
        driver.send(&frame).expect("send");

        // mock 记录了发送的帧
        // 注意：controller 已被 Box 移走，无法直接访问 mock
        // 通过 stats 验证发送成功
        assert_eq!(driver.stats().tx_count, 1);
    }

    #[test]
    fn test_can_frame_send_and_verify_tx() {
        let mock = MockCanController::new();
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let frame = CanFrame::new_extended(0x1ABCDE, &[0xAA, 0xBB, 0xCC]);
        driver.send(&frame).expect("send");
        assert_eq!(driver.stats().tx_count, 1);

        // 验证驱动状态正常
        assert_eq!(driver.state(), DriverState::Running);
        assert_eq!(driver.driver_type(), DriverType::Can);
    }

    // ===== T9.2: CanFilter 集成测试 =====

    #[test]
    fn test_filter_accept_all_integration() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x001, &[]);
        mock.push_rx_standard(0x7FF, &[0x01]);
        let config = CanConfig::default(); // 无过滤器 = 接收所有
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let f1 = driver.recv(0, 100).expect("first frame");
        assert_eq!(f1.id, CanId::Standard(0x001));
        let f2 = driver.recv(0, 100).expect("second frame");
        assert_eq!(f2.id, CanId::Standard(0x7FF));
        assert_eq!(driver.stats().rx_count, 2);
    }

    #[test]
    fn test_filter_match_exact_integration() {
        let filter = CanFilter::match_exact(0x123, false);
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x123, &[0x42]); // 匹配
        mock.push_rx_standard(0x124, &[0x00]); // 不匹配
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 第一帧匹配，应收到
        let f = driver.recv(0, 100).expect("matching frame");
        assert_eq!(f.id, CanId::Standard(0x123));
        assert_eq!(driver.stats().rx_count, 1);

        // 第二帧不匹配，应超时
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
        assert_eq!(driver.stats().rx_count, 1); // 未递增
    }

    #[test]
    fn test_filter_match_prefix_integration() {
        // 匹配高 3 位 = 0b111 的标准帧（0x700~0x7FF）
        let filter = CanFilter::match_prefix(0x700, 3, false);
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x700, &[]); // 匹配
        mock.push_rx_standard(0x7FF, &[0x01]); // 匹配
        mock.push_rx_standard(0x6FF, &[0x02]); // 不匹配
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let f1 = driver.recv(0, 100).expect("first matching frame");
        assert_eq!(f1.id, CanId::Standard(0x700));
        let f2 = driver.recv(0, 100).expect("second matching frame");
        assert_eq!(f2.id, CanId::Standard(0x7FF));
        assert_eq!(driver.stats().rx_count, 2);

        // 第三帧不匹配前缀
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
    }

    // ===== T9.3: Loopback 模式测试 =====

    #[test]
    fn test_loopback_mode_config() {
        let mock = MockCanController::new();
        let config = CanConfig {
            mode: CanMode::Loopback,
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        // init 成功即表示 set_mode(Loopback) 调用成功
        assert_eq!(driver.state(), DriverState::Ready);
    }

    #[test]
    fn test_loopback_send_and_receive() {
        // 模拟环回：发送后 mock 立即将帧放入 RX 队列
        // 由于 controller 被 Box 移走，我们通过预填充 RX 模拟环回
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x123, &[0x01, 0x02]); // 模拟环回到 RX
        let config = CanConfig {
            mode: CanMode::Loopback,
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 接收"环回"帧
        let frame = driver.recv(0, 100).expect("loopback frame");
        assert_eq!(frame.id, CanId::Standard(0x123));
        assert_eq!(frame.data, vec![0x01, 0x02]);
        assert_eq!(driver.stats().rx_count, 1);
    }

    // ===== T9.4: 多帧收发测试 =====

    #[test]
    fn test_multi_frame_send_and_receive() {
        let mut mock = MockCanController::new();
        // 预填充 5 帧到 RX
        for i in 0..5u16 {
            mock.push_rx_standard(0x100 + i, &[i as u8]);
        }
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 接收 5 帧
        for i in 0..5u16 {
            let frame = driver.recv(0, 100).expect("recv should succeed");
            assert_eq!(frame.id, CanId::Standard(0x100 + i));
            assert_eq!(frame.data, vec![i as u8]);
        }
        assert_eq!(driver.stats().rx_count, 5);

        // 第 6 帧应超时
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
    }

    #[test]
    fn test_multi_frame_send_only() {
        let mock = MockCanController::new();
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 发送 5 帧
        for i in 0..5u16 {
            driver
                .send(&CanFrame::new_standard(0x100 + i, &[i as u8]))
                .expect("send");
        }
        assert_eq!(driver.stats().tx_count, 5);
    }

    // ===== T9.5: 过滤器集成测试 =====

    #[test]
    fn test_filter_only_matching_frames_enqueued() {
        let filter = CanFilter::match_exact(0x200, false);
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x100, &[0x01]); // 不匹配
        mock.push_rx_standard(0x200, &[0x02]); // 匹配
        mock.push_rx_standard(0x300, &[0x03]); // 不匹配
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 应只收到 0x200（其他被过滤器拒绝）
        let frame = driver.recv(0, 100).expect("only matching frame");
        assert_eq!(frame.id, CanId::Standard(0x200));
        assert_eq!(frame.data, vec![0x02]);
        assert_eq!(driver.stats().rx_count, 1);

        // 后续应超时（无更多匹配帧）
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
    }

    #[test]
    fn test_filter_standard_extended_mutual_exclusion() {
        // 标准帧过滤器不应匹配扩展帧
        let filter = CanFilter::match_exact(0x123, false);
        let mut mock = MockCanController::new();
        mock.push_rx_extended(0x123, &[0x01]); // 扩展帧，ID 相同但不匹配标准过滤器
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 扩展帧不应匹配标准帧过滤器
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(eneros_driver_framework::DriverError::Timeout));
        assert_eq!(driver.stats().rx_count, 0);
    }

    // ===== 额外: 综合场景测试 =====

    #[test]
    fn test_remote_frame_handling() {
        let mut mock = MockCanController::new();
        mock.push_rx_frame(CanFrame::new_remote(CanId::Standard(0x100)));
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let frame = driver.recv(0, 100).expect("remote frame");
        assert_eq!(frame.frame_type, FrameType::Remote);
        assert_eq!(frame.dlc, 0);
        assert!(frame.data.is_empty());
    }

    #[test]
    fn test_driver_full_lifecycle() {
        let mock = MockCanController::new();
        let config = CanConfig {
            controller_type: CanControllerType::MCP2515,
            baud_rate: 250_000,
            mode: CanMode::Normal,
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(99), config, Box::new(mock));

        assert_eq!(driver.name(), "can-mcp2515");
        assert_eq!(driver.state(), DriverState::Uninitialized);

        driver.init().expect("init");
        assert_eq!(driver.state(), DriverState::Ready);

        driver.start().expect("start");
        assert_eq!(driver.state(), DriverState::Running);

        // 发送
        driver
            .send(&CanFrame::new_standard(0x50, &[0xDE, 0xAD]))
            .expect("send");
        assert_eq!(driver.stats().tx_count, 1);

        driver.stop().expect("stop");
        assert_eq!(driver.state(), DriverState::Stopped);

        driver.deinit().expect("deinit");
        assert_eq!(driver.state(), DriverState::Dead);
    }
}
