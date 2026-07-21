//! CAN 控制器 HAL 抽象（v0.47.0）.
//!
//! 定义 CAN 控制器硬件访问 trait 与收发统计结构。
//!
//! # 偏差声明
//! - D1: 定义本地 `CanController` trait（HAL 仅有 `HalSpi`/`HalGpio`，无 CAN 控制器专有方法）
//! - D5: `recv()` 接受 `now_ns: u64` 参数注入时间戳；此处 `CanController::now_ns()` 提供时间源
//! - D6: `read_rx_buffer()` 返回 `Option<CanFrame>`（驱动级抽象，无时间戳）
//! - D9: 不依赖 `eneros-hal` crate，HAL 抽象由本地 trait 提供

use crate::config::CanMode;
use crate::filter::CanFilter;
use crate::frame::CanFrame;

/// CAN 控制器硬件抽象 trait（D1）
///
/// 抽象 CAN 控制器的寄存器级操作，由具体硬件实现（如 MCP2515/内部 CAN/SJA1000）。
/// 所有方法返回 `Result<(), ()>`：成功 `Ok(())`，硬件错误 `Err(())`。
/// 驱动层（`CanDriver`）将 `Err(())` 映射为对应的 `DriverError`。
#[allow(clippy::result_unit_err)]
pub trait CanController: Send + Sync {
    /// 硬件复位
    fn reset(&mut self) -> Result<(), ()>;

    /// 设置波特率
    fn set_baud_rate(&mut self, baud: u32) -> Result<(), ()>;

    /// 设置工作模式
    fn set_mode(&mut self, mode: CanMode) -> Result<(), ()>;

    /// 设置硬件过滤器（index 指定过滤器槽位）
    fn set_filter(&mut self, index: usize, filter: &CanFilter) -> Result<(), ()>;

    /// 启用接收中断
    fn enable_rx_irq(&mut self) -> Result<(), ()>;

    /// 禁用接收中断
    fn disable_rx_irq(&mut self) -> Result<(), ()>;

    /// 读取 RX 缓冲中的下一帧（D6）
    ///
    /// # 返回
    /// - `Some(CanFrame)`: 成功读取一帧
    /// - `None`: RX 缓冲为空
    fn read_rx_buffer(&mut self) -> Option<CanFrame>;

    /// 写入 TX 缓冲发送一帧
    fn write_tx_buffer(&mut self, frame: &CanFrame) -> Result<(), ()>;

    /// 返回当前时间戳（纳秒）（D5：时间源由控制器实现提供）
    fn now_ns(&self) -> u64;
}

/// CAN 收发统计
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanStats {
    /// 发送帧数
    pub tx_count: u32,
    /// 接收帧数
    pub rx_count: u32,
    /// 接收错误次数
    pub rx_error_count: u32,
    /// 发送错误次数
    pub tx_error_count: u32,
    /// 总线关闭（Bus-Off）次数
    pub bus_off_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_stats_default() {
        let stats = CanStats::default();
        assert_eq!(stats.tx_count, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.rx_error_count, 0);
        assert_eq!(stats.tx_error_count, 0);
        assert_eq!(stats.bus_off_count, 0);
    }

    #[test]
    fn test_can_stats_increment_tx() {
        let mut stats = CanStats::default();
        stats.tx_count += 1;
        assert_eq!(stats.tx_count, 1);
        stats.tx_count += 5;
        assert_eq!(stats.tx_count, 6);
    }

    #[test]
    fn test_can_stats_increment_rx() {
        let mut stats = CanStats::default();
        stats.rx_count += 1;
        assert_eq!(stats.rx_count, 1);
        stats.rx_count += 10;
        assert_eq!(stats.rx_count, 11);
    }

    #[test]
    fn test_can_stats_increment_errors() {
        let mut stats = CanStats::default();
        stats.rx_error_count += 1;
        stats.tx_error_count += 2;
        stats.bus_off_count += 3;
        assert_eq!(stats.rx_error_count, 1);
        assert_eq!(stats.tx_error_count, 2);
        assert_eq!(stats.bus_off_count, 3);
    }

    #[test]
    fn test_can_stats_clone_eq() {
        let stats = CanStats {
            tx_count: 5,
            rx_count: 3,
            ..Default::default()
        };
        let stats_clone = stats.clone();
        assert_eq!(stats, stats_clone);
    }

    #[test]
    fn test_can_stats_partial_eq() {
        let mut a = CanStats::default();
        let mut b = CanStats::default();
        assert_eq!(a, b);
        a.tx_count = 1;
        assert_ne!(a, b);
        b.tx_count = 1;
        assert_eq!(a, b);
    }
}
