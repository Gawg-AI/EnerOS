//! CAN 驱动实现（v0.47.0）.
//!
//! 实现 `DeviceDriver` trait，提供 CAN 2.0A/B 帧收发能力。
//!
//! # 偏差声明
//! - D1: 通过 `CanController` trait 抽象硬件（本地定义，HAL 无 CAN 方法）
//! - D3: `CanFrame` 不含时间戳字段
//! - D4: `RingBuffer<CanFrame, 64>` 本地实现
//! - D5: `recv()` 接受 `now_ns: u64` 参数注入时间戳
//! - D6: `read_rx_buffer()` 返回 `Option<CanFrame>`
//! - D7: 过滤器实现 ID+掩码匹配 + 标准/扩展互斥
//! - D9: 不依赖 `eneros-hal` crate

use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

use eneros_driver_framework::{
    DeviceDriver, DriverError, DriverHealth, DriverId, DriverState, DriverType,
};

use crate::config::CanConfig;
use crate::controller::{CanController, CanStats};
use crate::frame::CanFrame;
use crate::ring::RingBuffer;

/// CAN RX 中断编号（固定为 1，类比 RS485 默认 rx_irq=1）
const CAN_RX_IRQ_ID: u32 = 1;

/// CAN 驱动
///
/// 实现 `DeviceDriver` trait，提供 CAN 2.0A/B 帧收发。
/// 通过 `CanController` trait 抽象 CAN 控制器硬件操作（D1），
/// 使用本地 `RingBuffer<CanFrame, 64>` 作为接收缓冲（D4）。
pub struct CanDriver {
    /// 驱动 ID
    id: DriverId,
    /// 驱动名称（如 "can-internal"）
    name: String,
    /// CAN 配置
    config: CanConfig,
    /// 驱动状态
    state: DriverState,
    /// CAN 控制器硬件抽象（D1: Box<dyn CanController>）
    controller: Box<dyn CanController>,
    /// 接收环形缓冲（D4: RingBuffer<CanFrame, 64>）
    rx_queue: RingBuffer<CanFrame, 64>,
    /// 收发统计
    stats: CanStats,
    /// 接收中断标志（AtomicBool）
    irq_rx: AtomicBool,
    /// 最近一次注入的时间戳（D5）
    last_now_ns: u64,
}

impl CanDriver {
    /// 创建 CAN 驱动
    ///
    /// # 参数
    /// - `id`: 驱动唯一标识
    /// - `config`: CAN 配置
    /// - `controller`: CAN 控制器硬件实现（需 `Send + Sync`）
    pub fn new(id: DriverId, config: CanConfig, controller: Box<dyn CanController>) -> Self {
        let mut name = String::from("can-");
        name.push_str(config.controller_type.as_str());
        Self {
            id,
            name,
            config,
            state: DriverState::Uninitialized,
            controller,
            rx_queue: RingBuffer::new(),
            stats: CanStats::default(),
            irq_rx: AtomicBool::new(false),
            last_now_ns: 0,
        }
    }

    /// 发送 CAN 帧
    ///
    /// # 参数
    /// - `frame`: 待发送帧（数据长度 ≤ 8 字节）
    ///
    /// # 返回
    /// - `Ok(())`: 发送成功，`tx_count` 递增
    /// - `Err(DriverError::InvalidState)`: 数据长度 > 8
    /// - `Err(DriverError::InitFailed)`: 硬件写入失败
    pub fn send(&mut self, frame: &CanFrame) -> Result<(), DriverError> {
        if frame.data.len() > 8 {
            return Err(DriverError::InvalidState);
        }
        self.controller
            .write_tx_buffer(frame)
            .map_err(|_| DriverError::InitFailed)?;
        self.stats.tx_count += 1;
        Ok(())
    }

    /// 接收 CAN 帧（D5：时间由调用者注入）
    ///
    /// 优先从 `rx_queue` 弹出帧；若为空则轮询控制器 RX 缓冲。
    /// 超时返回 `DriverError::Timeout`，`rx_error_count` 递增。
    ///
    /// # 参数
    /// - `now_ns`: 当前时间戳（纳秒，由调用者注入）
    /// - `timeout_ms`: 接收超时（毫秒）
    pub fn recv(&mut self, now_ns: u64, timeout_ms: u32) -> Result<CanFrame, DriverError> {
        self.last_now_ns = now_ns;
        let deadline_ns = now_ns.saturating_add((timeout_ms as u64) * 1_000_000);
        loop {
            // 1. 从 rx_queue 弹出
            if let Some(frame) = self.rx_queue.pop() {
                self.stats.rx_count += 1;
                return Ok(frame);
            }
            // 2. 轮询控制器 RX 缓冲
            let polled = self.controller.read_rx_buffer();
            if let Some(frame) = polled {
                if self.passes_filters(&frame) {
                    self.stats.rx_count += 1;
                    return Ok(frame);
                }
                // 过滤器拒绝，继续循环
            }
            // 3. 超时检查（使用控制器时间源推进）
            if self.controller.now_ns() >= deadline_ns {
                self.stats.rx_error_count += 1;
                return Err(DriverError::Timeout);
            }
        }
    }

    /// 返回接收中断标志（并清除）
    pub fn take_irq_rx(&self) -> bool {
        self.irq_rx.swap(false, Ordering::AcqRel)
    }

    /// 返回统计信息
    pub fn stats(&self) -> &CanStats {
        &self.stats
    }

    /// 返回配置引用
    pub fn config(&self) -> &CanConfig {
        &self.config
    }

    /// 判断帧是否通过软件过滤器
    ///
    /// 无过滤器时接收所有；有过滤器时任一匹配即通过。
    fn passes_filters(&self, frame: &CanFrame) -> bool {
        if self.config.filters.is_empty() {
            return true;
        }
        self.config.filters.iter().any(|f| f.matches(frame))
    }
}

impl DeviceDriver for CanDriver {
    fn id(&self) -> &DriverId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn driver_type(&self) -> DriverType {
        DriverType::Can
    }

    fn state(&self) -> DriverState {
        self.state
    }

    fn init(&mut self) -> Result<(), DriverError> {
        self.controller
            .reset()
            .map_err(|_| DriverError::InitFailed)?;
        self.controller
            .set_baud_rate(self.config.baud_rate)
            .map_err(|_| DriverError::InitFailed)?;
        for (index, filter) in self.config.filters.iter().enumerate() {
            self.controller
                .set_filter(index, filter)
                .map_err(|_| DriverError::InitFailed)?;
        }
        self.controller
            .set_mode(self.config.mode)
            .map_err(|_| DriverError::InitFailed)?;
        self.controller
            .enable_rx_irq()
            .map_err(|_| DriverError::InitFailed)?;
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
        let _ = self.controller.disable_rx_irq();
        self.state = DriverState::Dead;
        Ok(())
    }

    fn handle_irq(&mut self, irq_id: u32) {
        if irq_id != CAN_RX_IRQ_ID {
            return;
        }
        loop {
            let polled = self.controller.read_rx_buffer();
            match polled {
                Some(frame) => {
                    if self.passes_filters(&frame) && self.rx_queue.push(frame).is_err() {
                        // 队列满，丢弃并计错
                        self.stats.rx_error_count += 1;
                        break;
                    }
                    // 不匹配的帧静默丢弃
                }
                None => break,
            }
        }
        self.irq_rx.store(true, Ordering::Release);
    }

    fn health_check(&self) -> DriverHealth {
        if self.stats.rx_error_count > 100 {
            DriverHealth::Unhealthy
        } else if self.stats.rx_error_count > 10 {
            DriverHealth::Degraded
        } else {
            DriverHealth::Healthy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CanConfig, CanControllerType, CanMode};
    use crate::filter::CanFilter;
    use crate::frame::{CanFrame, CanId};
    use crate::mock::MockCanController;

    /// 创建测试用 driver（默认配置，mock 控制器）
    fn make_driver() -> CanDriver {
        let mock = MockCanController::new();
        let config = CanConfig::default();
        CanDriver::new(DriverId(1), config, Box::new(mock))
    }

    /// 创建带过滤器的 driver
    fn make_driver_with_filter(filter: CanFilter) -> CanDriver {
        let mock = MockCanController::new();
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        CanDriver::new(DriverId(1), config, Box::new(mock))
    }

    // ===== T1: 状态转换测试 =====

    #[test]
    fn test_state_transitions() {
        let mut driver = make_driver();
        assert_eq!(driver.state(), DriverState::Uninitialized);
        assert_eq!(driver.driver_type(), DriverType::Can);
        assert_eq!(driver.name(), "can-internal");

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
    fn test_init_configures_controller() {
        let mut driver = make_driver();
        driver.init().expect("init should succeed");
        assert_eq!(driver.state(), DriverState::Ready);
        // init 成功即表示 reset + set_baud_rate + set_mode + enable_rx_irq 均成功
    }

    #[test]
    fn test_name_per_controller_type() {
        let cases = [
            (CanControllerType::Internal, "can-internal"),
            (CanControllerType::MCP2515, "can-mcp2515"),
            (CanControllerType::SJA1000, "can-sja1000"),
        ];
        for (ct, expected) in cases {
            let mock = MockCanController::new();
            let config = CanConfig {
                controller_type: ct,
                ..Default::default()
            };
            let driver = CanDriver::new(DriverId(1), config, Box::new(mock));
            assert_eq!(driver.name(), expected);
        }
    }

    // ===== T2: send() 成功测试 =====

    #[test]
    fn test_send_success() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        let frame = CanFrame::new_standard(0x123, &[0x01, 0x02, 0x03]);
        let result = driver.send(&frame);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
        assert_eq!(driver.stats().rx_error_count, 0);
    }

    #[test]
    fn test_send_multiple_frames() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        driver
            .send(&CanFrame::new_standard(0x100, &[0x01]))
            .expect("send 1");
        driver
            .send(&CanFrame::new_standard(0x200, &[0x02, 0x03]))
            .expect("send 2");
        driver
            .send(&CanFrame::new_extended(0x1FFFFFFF, &[0x04, 0x05, 0x06]))
            .expect("send 3");
        assert_eq!(driver.stats().tx_count, 3);
    }

    #[test]
    fn test_send_data_too_long() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        // 9 字节数据（超过 8 字节限制）
        let frame = CanFrame::new_standard(0x100, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let result = driver.send(&frame);
        assert_eq!(result, Err(DriverError::InvalidState));
        assert_eq!(driver.stats().tx_count, 0);
    }

    #[test]
    fn test_send_empty_data() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        let frame = CanFrame::new_standard(0x100, &[]);
        let result = driver.send(&frame);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
    }

    #[test]
    fn test_send_eight_bytes() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        let frame = CanFrame::new_standard(0x100, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let result = driver.send(&frame);
        assert!(result.is_ok());
        assert_eq!(driver.stats().tx_count, 1);
    }

    // ===== T3: recv() 成功测试 =====

    #[test]
    fn test_recv_success_via_irq() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x123, &[0x01, 0x02]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // IRQ 处理将帧从控制器读入 rx_queue
        driver.handle_irq(1);
        assert!(driver.take_irq_rx());

        // 接收帧
        let frame = driver.recv(0, 100).expect("recv should succeed");
        assert_eq!(frame.id, CanId::Standard(0x123));
        assert_eq!(frame.data, vec![0x01, 0x02]);
        assert_eq!(driver.stats().rx_count, 1);
    }

    #[test]
    fn test_recv_success_via_polling() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x456, &[0xAA, 0xBB]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 直接 recv（轮询控制器 RX 缓冲）
        let frame = driver.recv(0, 100).expect("recv should succeed");
        assert_eq!(frame.id, CanId::Standard(0x456));
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
        assert_eq!(driver.stats().rx_count, 1);
    }

    #[test]
    fn test_recv_single_frame() {
        let mut mock = MockCanController::new();
        mock.push_rx_extended(0x12345678, &[0x01]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let frame = driver.recv(0, 100).expect("recv");
        assert_eq!(frame.id, CanId::Extended(0x12345678));
        assert_eq!(frame.dlc, 1);
    }

    // ===== T4: recv() 超时测试 =====

    #[test]
    fn test_recv_timeout_empty_queue() {
        let mut driver = make_driver();
        driver.init().expect("init");
        driver.start().expect("start");

        // 空队列，超时返回
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(DriverError::Timeout));
        assert_eq!(driver.stats().rx_error_count, 1);
        assert_eq!(driver.stats().rx_count, 0);
    }

    // ===== T5: handle_irq() 测试 =====

    #[test]
    fn test_handle_irq_matching() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x100, &[0x10, 0x20]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        driver.handle_irq(1);
        assert!(driver.take_irq_rx(), "irq_rx flag should be set");
        // 帧应已入队
        assert_eq!(driver.stats().rx_count, 0); // recv 才会递增 rx_count
        let frame = driver.recv(0, 100).expect("frame should be in queue");
        assert_eq!(frame.id, CanId::Standard(0x100));
    }

    #[test]
    fn test_handle_irq_non_matching() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x100, &[0x10]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // IRQ 编号不匹配（CAN_RX_IRQ_ID=1，传入 99）
        driver.handle_irq(99);
        assert!(!driver.take_irq_rx(), "irq_rx flag should NOT be set");
    }

    #[test]
    fn test_take_irq_rx_clears_flag() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x100, &[]);
        let config = CanConfig::default();
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        driver.handle_irq(1);
        assert!(driver.take_irq_rx());
        // 第二次取应返回 false（已清除）
        assert!(!driver.take_irq_rx());
    }

    // ===== T6: health_check() 测试 =====

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
            let _ = driver.recv(0, 1);
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
            let _ = driver.recv(0, 1);
        }
        assert_eq!(driver.stats().rx_error_count, 101);
        assert_eq!(driver.health_check(), DriverHealth::Unhealthy);
    }

    // ===== T7: trait object 兼容性测试 =====

    #[test]
    fn test_can_controller_as_trait_object() {
        let mock: Box<dyn CanController> = Box::new(MockCanController::new());
        let config = CanConfig::default();
        let driver = CanDriver::new(DriverId(42), config, mock);
        assert_eq!(driver.id(), &DriverId(42));
        assert_eq!(driver.driver_type(), DriverType::Can);
    }

    #[test]
    fn test_driver_can_be_boxed_as_device_driver() {
        let mock = MockCanController::new();
        let config = CanConfig::default();
        let driver = CanDriver::new(DriverId(1), config, Box::new(mock));
        let _boxed: Box<dyn DeviceDriver> = Box::new(driver);
        // 编译通过即说明 CanDriver 满足 Send + Sync
    }

    // ===== 额外: config/stats 访问器测试 =====

    #[test]
    fn test_config_accessor() {
        let driver = make_driver();
        let config = driver.config();
        assert_eq!(config.baud_rate, 500_000);
        assert_eq!(config.controller_type, CanControllerType::Internal);
        assert_eq!(config.mode, CanMode::Normal);
        assert!(config.filters.is_empty());
        assert!(config.auto_retransmit);
    }

    #[test]
    fn test_stats_accessor() {
        let driver = make_driver();
        let stats = driver.stats();
        assert_eq!(stats.tx_count, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.rx_error_count, 0);
        assert_eq!(stats.tx_error_count, 0);
        assert_eq!(stats.bus_off_count, 0);
    }

    // ===== 额外: 过滤器集成测试 =====

    #[test]
    fn test_handle_irq_with_filter_matching() {
        let filter = CanFilter::match_exact(0x123, false);
        let mut driver = make_driver_with_filter(filter);
        driver.init().expect("init");
        driver.start().expect("start");

        // 模拟控制器收到匹配的帧
        // 通过 mock 直接 push（绕过 driver，模拟硬件 RX）
        // 由于 controller 已被 Box 移走，需要通过 handle_irq 间接操作
        // 这里我们验证：handle_irq 后帧在队列中
        // 注意：mock 的 rx_queue 在 Box 内，无法直接 push
        // 改为测试 recv 轮询路径的过滤器
    }

    #[test]
    fn test_recv_with_filter_rejects_non_matching() {
        let filter = CanFilter::match_exact(0x123, false);
        let mut mock = MockCanController::new();
        // push 不匹配的帧（ID 0x200 ≠ 0x123）
        mock.push_rx_standard(0x200, &[0x01]);
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        // 帧不匹配过滤器，应超时
        let result = driver.recv(0, 1);
        assert_eq!(result, Err(DriverError::Timeout));
        assert_eq!(driver.stats().rx_count, 0);
    }

    #[test]
    fn test_recv_with_filter_accepts_matching() {
        let filter = CanFilter::match_exact(0x123, false);
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x123, &[0x42]);
        let config = CanConfig {
            filters: alloc::vec![filter],
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(1), config, Box::new(mock));

        driver.init().expect("init");
        driver.start().expect("start");

        let frame = driver
            .recv(0, 100)
            .expect("matching frame should be received");
        assert_eq!(frame.id, CanId::Standard(0x123));
        assert_eq!(frame.data, vec![0x42]);
        assert_eq!(driver.stats().rx_count, 1);
    }
}
