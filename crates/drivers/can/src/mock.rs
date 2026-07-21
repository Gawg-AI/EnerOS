//! MockCanController 测试桩（v0.47.0）.
//!
//! 实现 `CanController` trait，用于 CAN 驱动的单元测试与集成测试。
//! 支持预填充接收队列、记录发送帧、可配置 TX 错误、模拟时间推进。

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::config::CanMode;
use crate::controller::CanController;
use crate::filter::CanFilter;
use crate::frame::{CanFrame, CanId};

/// CAN 控制器硬件模拟器
///
/// 实现 `CanController` trait 用于测试。通过 `now_ns()` 提供可推进的时间源，
/// 使 `CanDriver::recv()` 的超时逻辑可测试。
pub struct MockCanController {
    /// 预填充的接收帧队列
    rx_queue: VecDeque<CanFrame>,
    /// 已发送帧记录
    tx_frames: Vec<CanFrame>,
    /// `write_tx_buffer` 是否返回错误
    tx_error: bool,
    /// 模拟当前时间（纳秒），原子以支持 `&self` 的 `now_ns()`
    current_time_ns: AtomicU64,
    /// 每次 `now_ns()` 调用自动推进的时间量（纳秒）
    time_step_ns: u64,
    /// 是否已调用 `reset`
    reset_called: AtomicBool,
    /// 最近一次设置的波特率
    last_baud_rate: u32,
    /// 是否已调用 `enable_rx_irq`
    rx_irq_enabled: AtomicBool,
    /// 已设置的过滤器记录
    set_filter_calls: Vec<(usize, CanFilter)>,
    /// 最近一次设置的模式
    last_mode: Option<CanMode>,
}

impl MockCanController {
    /// 创建默认 mock（time_step=1ms）
    pub fn new() -> Self {
        Self {
            rx_queue: VecDeque::new(),
            tx_frames: Vec::new(),
            tx_error: false,
            current_time_ns: AtomicU64::new(0),
            time_step_ns: 1_000_000, // 1ms per now_ns() call
            reset_called: AtomicBool::new(false),
            last_baud_rate: 0,
            rx_irq_enabled: AtomicBool::new(false),
            set_filter_calls: Vec::new(),
            last_mode: None,
        }
    }

    /// 预填充接收帧
    pub fn push_rx_frame(&mut self, frame: CanFrame) {
        self.rx_queue.push_back(frame);
    }

    /// 预填充标准数据帧
    pub fn push_rx_standard(&mut self, id: u16, data: &[u8]) {
        self.rx_queue.push_back(CanFrame::new_standard(id, data));
    }

    /// 预填充扩展数据帧
    pub fn push_rx_extended(&mut self, id: u32, data: &[u8]) {
        self.rx_queue.push_back(CanFrame::new_extended(id, data));
    }

    /// 设置当前时间（纳秒）
    pub fn set_now_ns(&self, ns: u64) {
        self.current_time_ns.store(ns, Ordering::Relaxed);
    }

    /// 设置每次 `now_ns()` 调用的时间推进量（纳秒）
    pub fn set_time_step_ns(&mut self, step: u64) {
        self.time_step_ns = step;
    }

    /// 手动推进时间（纳秒）
    pub fn advance_now_ns(&self, delta: u64) {
        self.current_time_ns.fetch_add(delta, Ordering::Relaxed);
    }

    /// 设置 `write_tx_buffer` 是否返回错误
    pub fn set_tx_error(&mut self, err: bool) {
        self.tx_error = err;
    }

    /// 返回已发送帧记录
    pub fn tx_frames(&self) -> &[CanFrame] {
        &self.tx_frames
    }

    /// 返回是否已调用 `reset`
    pub fn is_reset_called(&self) -> bool {
        self.reset_called.load(Ordering::Relaxed)
    }

    /// 返回最近一次设置的波特率
    pub fn last_baud_rate(&self) -> u32 {
        self.last_baud_rate
    }

    /// 返回 RX IRQ 是否已启用
    pub fn is_rx_irq_enabled(&self) -> bool {
        self.rx_irq_enabled.load(Ordering::Relaxed)
    }

    /// 返回已设置的过滤器记录
    pub fn set_filter_calls(&self) -> &[(usize, CanFilter)] {
        &self.set_filter_calls
    }

    /// 返回最近一次设置的模式
    pub fn last_mode(&self) -> Option<CanMode> {
        self.last_mode
    }

    /// 返回接收队列剩余帧数
    pub fn rx_queue_len(&self) -> usize {
        self.rx_queue.len()
    }

    /// 清空所有状态
    pub fn clear(&mut self) {
        self.rx_queue.clear();
        self.tx_frames.clear();
        self.tx_error = false;
        self.current_time_ns.store(0, Ordering::Relaxed);
        self.reset_called.store(false, Ordering::Relaxed);
        self.last_baud_rate = 0;
        self.rx_irq_enabled.store(false, Ordering::Relaxed);
        self.set_filter_calls.clear();
        self.last_mode = None;
    }
}

impl Default for MockCanController {
    fn default() -> Self {
        Self::new()
    }
}

impl CanController for MockCanController {
    fn reset(&mut self) -> Result<(), ()> {
        self.reset_called.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_baud_rate(&mut self, baud: u32) -> Result<(), ()> {
        self.last_baud_rate = baud;
        Ok(())
    }

    fn set_mode(&mut self, mode: CanMode) -> Result<(), ()> {
        self.last_mode = Some(mode);
        Ok(())
    }

    fn set_filter(&mut self, index: usize, filter: &CanFilter) -> Result<(), ()> {
        self.set_filter_calls.push((index, filter.clone()));
        Ok(())
    }

    fn enable_rx_irq(&mut self) -> Result<(), ()> {
        self.rx_irq_enabled.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn disable_rx_irq(&mut self) -> Result<(), ()> {
        self.rx_irq_enabled.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn read_rx_buffer(&mut self) -> Option<CanFrame> {
        self.rx_queue.pop_front()
    }

    fn write_tx_buffer(&mut self, frame: &CanFrame) -> Result<(), ()> {
        if self.tx_error {
            Err(())
        } else {
            self.tx_frames.push(frame.clone());
            Ok(())
        }
    }

    fn now_ns(&self) -> u64 {
        self.current_time_ns
            .fetch_add(self.time_step_ns, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_new_empty() {
        let mock = MockCanController::new();
        assert_eq!(mock.tx_frames().len(), 0);
        assert_eq!(mock.rx_queue_len(), 0);
        assert!(!mock.is_reset_called());
        assert!(!mock.is_rx_irq_enabled());
    }

    #[test]
    fn test_mock_default() {
        let mock = MockCanController::default();
        assert_eq!(mock.tx_frames().len(), 0);
    }

    #[test]
    fn test_push_rx_standard_and_read() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x123, &[0x01, 0x02]);
        let frame = mock.read_rx_buffer().expect("frame should exist");
        assert_eq!(frame.id, CanId::Standard(0x123));
        assert_eq!(frame.data, vec![0x01, 0x02]);
        assert!(mock.read_rx_buffer().is_none());
    }

    #[test]
    fn test_push_rx_extended_and_read() {
        let mut mock = MockCanController::new();
        mock.push_rx_extended(0x1FFFFFFF, &[0xAA]);
        let frame = mock.read_rx_buffer().expect("frame should exist");
        assert_eq!(frame.id, CanId::Extended(0x1FFFFFFF));
    }

    #[test]
    fn test_write_tx_buffer_records() {
        let mut mock = MockCanController::new();
        let frame = CanFrame::new_standard(0x100, &[0x01]);
        assert!(mock.write_tx_buffer(&frame).is_ok());
        assert_eq!(mock.tx_frames().len(), 1);
        assert_eq!(mock.tx_frames()[0].id, CanId::Standard(0x100));
    }

    #[test]
    fn test_write_tx_buffer_error() {
        let mut mock = MockCanController::new();
        mock.set_tx_error(true);
        let frame = CanFrame::new_standard(0x100, &[]);
        assert!(mock.write_tx_buffer(&frame).is_err());
        assert_eq!(mock.tx_frames().len(), 0);
    }

    #[test]
    fn test_reset_called() {
        let mut mock = MockCanController::new();
        assert!(!mock.is_reset_called());
        mock.reset().unwrap();
        assert!(mock.is_reset_called());
    }

    #[test]
    fn test_set_baud_rate_recorded() {
        let mut mock = MockCanController::new();
        mock.set_baud_rate(250_000).unwrap();
        assert_eq!(mock.last_baud_rate(), 250_000);
    }

    #[test]
    fn test_set_mode_recorded() {
        let mut mock = MockCanController::new();
        mock.set_mode(CanMode::Loopback).unwrap();
        assert_eq!(mock.last_mode(), Some(CanMode::Loopback));
    }

    #[test]
    fn test_set_filter_recorded() {
        let mut mock = MockCanController::new();
        let filter = CanFilter::match_exact(0x123, false);
        mock.set_filter(0, &filter).unwrap();
        assert_eq!(mock.set_filter_calls().len(), 1);
        assert_eq!(mock.set_filter_calls()[0].0, 0);
    }

    #[test]
    fn test_enable_disable_rx_irq() {
        let mut mock = MockCanController::new();
        mock.enable_rx_irq().unwrap();
        assert!(mock.is_rx_irq_enabled());
        mock.disable_rx_irq().unwrap();
        assert!(!mock.is_rx_irq_enabled());
    }

    #[test]
    fn test_now_ns_advances() {
        let mock = MockCanController::new();
        let t0 = mock.now_ns();
        let t1 = mock.now_ns();
        // 默认 time_step = 1ms = 1_000_000 ns
        assert_eq!(t1 - t0, 1_000_000);
    }

    #[test]
    fn test_set_and_advance_now_ns() {
        let mock = MockCanController::new();
        mock.set_now_ns(1_000_000_000);
        let t = mock.now_ns();
        assert_eq!(t, 1_000_000_000);
        mock.advance_now_ns(5_000_000);
        let t2 = mock.now_ns();
        assert!(t2 >= 1_005_000_000);
    }

    #[test]
    fn test_clear() {
        let mut mock = MockCanController::new();
        mock.push_rx_standard(0x100, &[]);
        mock.write_tx_buffer(&CanFrame::new_standard(0x100, &[]))
            .ok();
        mock.reset().ok();
        mock.clear();
        assert_eq!(mock.rx_queue_len(), 0);
        assert_eq!(mock.tx_frames().len(), 0);
        assert!(!mock.is_reset_called());
    }
}
