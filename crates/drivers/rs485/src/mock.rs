//! MockUartHw 测试桩（v0.44.0）.
//!
//! 实现 `UartHw` trait，用于 RS485 驱动的单元测试与集成测试。
//! 支持预填充接收缓冲、记录发送数据、可配置 TX 超时、模拟时间推进。

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use eneros_driver_framework::DriverError;

use crate::config::{Parity, StopBits};
use crate::uart_hw::UartHw;

/// UART 硬件模拟器
///
/// 实现 `UartHw` trait 用于测试。通过 `now_ns()` 的自动时间推进机制
/// 模拟时间流逝，使 `Rs485Driver::recv()` 的帧间隔检测和超时逻辑可测试。
pub struct MockUartHw {
    /// 预填充的接收数据队列
    rx_data: VecDeque<u8>,
    /// 已发送数据记录
    written: Vec<u8>,
    /// wait_tx_done 是否返回超时
    tx_timeout: bool,
    /// 模拟当前时间（纳秒），原子以支持 `&self` 的 `now_ns()`
    current_time_ns: AtomicU64,
    /// 每次 `now_ns()` 调用自动推进的时间量（纳秒）
    time_step_ns: u64,
    /// 是否已调用 `configure`
    configured: AtomicBool,
    /// RX IRQ 是否已启用
    rx_irq_enabled: AtomicBool,
    /// 模拟的 RX IRQ 编号
    rx_irq: u32,
    /// DE/RE 当前状态（true=发送模式，false=接收模式）
    de_re_high: AtomicBool,
    /// `configure_de_re` 是否已调用
    de_re_configured: AtomicBool,
}

impl MockUartHw {
    /// 创建默认 mock（rx_irq=1, time_step=1ms）
    pub fn new() -> Self {
        Self {
            rx_data: VecDeque::new(),
            written: Vec::new(),
            tx_timeout: false,
            current_time_ns: AtomicU64::new(0),
            time_step_ns: 1_000_000, // 1ms per now_ns() call
            configured: AtomicBool::new(false),
            rx_irq_enabled: AtomicBool::new(false),
            rx_irq: 1,
            de_re_high: AtomicBool::new(false),
            de_re_configured: AtomicBool::new(false),
        }
    }

    /// 预填充接收数据
    pub fn push_rx(&mut self, byte: u8) {
        self.rx_data.push_back(byte);
    }

    /// 预填充接收数据（多字节）
    pub fn push_rx_slice(&mut self, data: &[u8]) {
        for &b in data {
            self.rx_data.push_back(b);
        }
    }

    /// 返回已发送数据
    pub fn written(&self) -> &[u8] {
        &self.written
    }

    /// 设置 `wait_tx_done` 是否返回超时
    pub fn set_tx_timeout(&mut self, timeout: bool) {
        self.tx_timeout = timeout;
    }

    /// 设置初始时间（纳秒）
    pub fn set_time_ns(&self, ns: u64) {
        self.current_time_ns.store(ns, Ordering::Relaxed);
    }

    /// 设置每次 `now_ns()` 调用的时间推进量（纳秒）
    pub fn set_time_step_ns(&mut self, step: u64) {
        self.time_step_ns = step;
    }

    /// 手动推进时间（纳秒）
    pub fn advance_time_ns(&self, delta: u64) {
        self.current_time_ns.fetch_add(delta, Ordering::Relaxed);
    }

    /// 返回 DE/RE 当前状态
    pub fn de_re_high(&self) -> bool {
        self.de_re_high.load(Ordering::Relaxed)
    }

    /// 返回是否已调用 `configure`
    pub fn is_configured(&self) -> bool {
        self.configured.load(Ordering::Relaxed)
    }

    /// 返回 RX IRQ 是否已启用
    pub fn is_rx_irq_enabled(&self) -> bool {
        self.rx_irq_enabled.load(Ordering::Relaxed)
    }

    /// 返回 `configure_de_re` 是否已被调用
    pub fn de_re_configured(&self) -> bool {
        self.de_re_configured.load(Ordering::Relaxed)
    }

    /// 设置模拟的 RX IRQ 编号
    pub fn set_rx_irq(&mut self, irq: u32) {
        self.rx_irq = irq;
    }

    /// 清空已发送数据记录
    pub fn clear_written(&mut self) {
        self.written.clear();
    }
}

impl Default for MockUartHw {
    fn default() -> Self {
        Self::new()
    }
}

impl UartHw for MockUartHw {
    fn configure(
        &mut self,
        _baud_rate: u32,
        _data_bits: u8,
        _stop_bits: StopBits,
        _parity: Parity,
    ) -> Result<(), DriverError> {
        self.configured.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn enable_rx_irq(&mut self) -> Result<(), DriverError> {
        self.rx_irq_enabled.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn disable_rx_irq(&mut self) -> Result<(), DriverError> {
        self.rx_irq_enabled.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn read_byte(&mut self) -> Option<u8> {
        self.rx_data.pop_front()
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError> {
        self.written.extend_from_slice(data);
        Ok(data.len())
    }

    fn wait_tx_done(&mut self, _timeout_ms: u32) -> Result<(), DriverError> {
        if self.tx_timeout {
            Err(DriverError::Timeout)
        } else {
            Ok(())
        }
    }

    fn rx_irq_id(&self) -> u32 {
        self.rx_irq
    }

    fn now_ns(&self) -> u64 {
        self.current_time_ns
            .fetch_add(self.time_step_ns, Ordering::Relaxed)
    }

    fn configure_de_re(&mut self, _pin: Option<u32>) -> Result<(), DriverError> {
        self.de_re_configured.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_de_re(&mut self, high: bool) -> Result<(), DriverError> {
        self.de_re_high.store(high, Ordering::Relaxed);
        Ok(())
    }
}
