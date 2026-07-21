//! RS485 驱动实现（v0.44.0）.
//!
//! 实现 `DeviceDriver` trait，提供 RS485 半双工串口数据帧收发能力。
//!
//! # 偏差声明
//! - D3 修正: `recv()` 通过 `UartHw::now_ns()` 获取时间，不接受外部 `now_ns` 参数
//! - D7 修正: DE/RE 控制通过 `UartHw::set_de_re()` 实现，不持有 `HalGpio` 引用
//! - D9: 无 `tx_buffer` 字段（同步发送）
//! - D10: `recv()` 返回 `alloc::vec::Vec<u8>`

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use eneros_driver_framework::{
    DeviceDriver, DriverError, DriverHealth, DriverId, DriverState, DriverType,
};

use crate::config::{Rs485Config, UartPort};
use crate::ring::RingBuffer;
use crate::uart_hw::UartHw;

/// RS485 收发统计
#[derive(Clone, Debug, Default)]
pub struct Rs485Stats {
    /// 发送帧数
    pub tx_count: u32,
    /// 接收帧数
    pub rx_count: u32,
    /// 接收错误次数
    pub rx_error_count: u32,
    /// 最近一次接收错误
    pub last_rx_error: Option<DriverError>,
}

/// RS485 串口驱动
///
/// 实现 `DeviceDriver` trait，提供半双工 RS485 数据帧收发。
/// 通过 `UartHw` trait 抽象 UART 硬件操作（D1），DE/RE 方向控制
/// 委托给 `UartHw` 实现（D7 修正）。
pub struct Rs485Driver {
    /// 驱动 ID
    id: DriverId,
    /// 驱动名称（如 "rs485-uart0"）
    name: String,
    /// RS485 配置
    config: Rs485Config,
    /// 驱动状态
    state: DriverState,
    /// UART 硬件抽象（D1: Box<dyn UartHw>）
    uart: Box<dyn UartHw>,
    /// 接收环形缓冲（D4: RingBuffer<u8, 512>）
    rx_buffer: RingBuffer<u8, 512>,
    /// 收发统计
    stats: Rs485Stats,
    /// 接收中断标志（D6: AtomicBool）
    irq_rx: AtomicBool,
}

impl Rs485Driver {
    /// 创建 RS485 驱动
    ///
    /// # 参数
    /// - `id`: 驱动唯一标识
    /// - `config`: RS485 配置
    /// - `uart`: UART 硬件实现（需 `Send + Sync`）
    pub fn new(id: DriverId, config: Rs485Config, uart: Box<dyn UartHw>) -> Self {
        let name = match config.port {
            UartPort::Uart0 => String::from("rs485-uart0"),
            UartPort::Uart1 => String::from("rs485-uart1"),
            UartPort::Uart2 => String::from("rs485-uart2"),
            UartPort::Uart3 => String::from("rs485-uart3"),
        };
        Self {
            id,
            name,
            config,
            state: DriverState::Uninitialized,
            uart,
            rx_buffer: RingBuffer::new(),
            stats: Rs485Stats::default(),
            irq_rx: AtomicBool::new(false),
        }
    }

    /// 发送数据帧
    ///
    /// 流程：DE 拉高 → 写入 UART → 等待发送完成 → DE 拉低
    ///
    /// # 参数
    /// - `data`: 待发送数据
    pub fn send(&mut self, data: &[u8]) -> Result<(), DriverError> {
        // 1. 切换为发送模式
        self.uart.set_de_re(true)?;

        // 2. 发送数据
        let written = self.uart.write_bytes(data)?;
        if written < data.len() {
            // 未完全写入，恢复接收模式并返回错误
            let _ = self.uart.set_de_re(false);
            self.stats.rx_error_count += 1;
            self.stats.last_rx_error = Some(DriverError::InitFailed);
            return Err(DriverError::InitFailed);
        }

        // 3. 等待发送完成
        if let Err(e) = self.uart.wait_tx_done(self.config.response_timeout_ms) {
            // 超时或错误，恢复接收模式
            let _ = self.uart.set_de_re(false);
            self.stats.rx_error_count += 1;
            self.stats.last_rx_error = Some(e.clone());
            return Err(e);
        }

        // 4. 切换回接收模式
        self.uart.set_de_re(false)?;

        self.stats.tx_count += 1;
        Ok(())
    }

    /// 接收数据帧（D3 修正：通过 `UartHw::now_ns()` 获取时间）
    ///
    /// 流程：从 rx_buffer 弹出字节，检测帧间隔（静默超过 frame_gap_ms 则帧结束），
    /// 超过 timeout_ms 则超时返回。
    ///
    /// # 参数
    /// - `timeout_ms`: 接收超时（毫秒）
    ///
    /// # 返回
    /// - `Ok(Vec<u8>)`: 接收到的完整帧
    /// - `Err(DriverError::Timeout)`: 超时未收到数据
    pub fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError> {
        let start_ns = self.uart.now_ns();
        let deadline_ns = start_ns + (timeout_ms as u64) * 1_000_000;
        let frame_gap_ns = (self.config.frame_gap_ms as u64) * 1_000_000;

        let mut frame: Vec<u8> = Vec::new();
        let mut last_byte_ns: Option<u64> = None;

        loop {
            let now = self.uart.now_ns();

            // 超时检查
            if now >= deadline_ns {
                break;
            }

            // 尝试从缓冲读取字节
            if let Some(byte) = self.rx_buffer.pop() {
                frame.push(byte);
                last_byte_ns = Some(now);
            } else if let Some(last) = last_byte_ns {
                // 无新数据，检查帧间隔
                if now.saturating_sub(last) >= frame_gap_ns {
                    break; // 帧结束
                }
            }
            // 无数据且无 last_byte_ns：继续等待
        }

        if frame.is_empty() {
            self.stats.rx_error_count += 1;
            self.stats.last_rx_error = Some(DriverError::Timeout);
            return Err(DriverError::Timeout);
        }

        self.stats.rx_count += 1;
        Ok(frame)
    }

    /// 返回统计信息
    pub fn stats(&self) -> &Rs485Stats {
        &self.stats
    }

    /// 返回配置引用
    pub fn config(&self) -> &Rs485Config {
        &self.config
    }

    /// 返回接收中断标志（并清除）
    pub fn take_irq_rx(&self) -> bool {
        self.irq_rx.swap(false, Ordering::AcqRel)
    }
}

impl DeviceDriver for Rs485Driver {
    fn id(&self) -> &DriverId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn driver_type(&self) -> DriverType {
        DriverType::Serial
    }

    fn state(&self) -> DriverState {
        self.state
    }

    fn init(&mut self) -> Result<(), DriverError> {
        // 配置 UART 硬件
        self.uart.configure(
            self.config.baud_rate,
            self.config.data_bits,
            self.config.stop_bits,
            self.config.parity,
        )?;

        // 配置 DE/RE 方向控制 GPIO（D7 修正：委托给 UartHw）
        self.uart.configure_de_re(self.config.de_re_pin)?;

        // 默认接收模式（DE=0）
        self.uart.set_de_re(false)?;

        self.state = DriverState::Ready;
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        self.uart.enable_rx_irq()?;
        self.state = DriverState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        self.uart.disable_rx_irq()?;
        self.state = DriverState::Stopped;
        Ok(())
    }

    fn deinit(&mut self) -> Result<(), DriverError> {
        self.state = DriverState::Dead;
        Ok(())
    }

    fn handle_irq(&mut self, irq_id: u32) {
        if irq_id == self.uart.rx_irq_id() {
            // 读取所有可用字节
            while let Some(byte) = self.uart.read_byte() {
                if self.rx_buffer.push(byte).is_err() {
                    // 缓冲满，丢弃
                    self.stats.rx_error_count += 1;
                    break;
                }
            }
            self.irq_rx.store(true, Ordering::Release);
        }
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
