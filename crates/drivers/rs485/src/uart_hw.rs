//! UART 硬件抽象 trait（D1 偏差）.
//!
//! 蓝图假设 HAL 提供 `HalUart` trait（含 `configure()`/`enable_rx_irq()`/
//! `read_byte()`/`wait_tx_done()` 等 UART 专有方法），但实际 HAL 仅提供
//! `HalSerial`（`write`/`read`/`flush` 三方法）。
//!
//! 因此在本 crate 内定义 `UartHw` trait 抽象 UART 硬件操作，由 BSP 或
//! 测试桩（`MockUartHw`）实现。`Rs485Driver` 通过此 trait 访问 UART 硬件。

use eneros_driver_framework::DriverError;

use crate::config::{Parity, StopBits};

/// UART 硬件抽象 trait（D1 偏差）
///
/// 提供 UART 硬件的配置、中断控制、字节级读写、发送完成等待与 DE/RE 方向控制。
/// 由 BSP（真实硬件）或 `MockUartHw`（测试桩）实现。
///
/// `Send + Sync` 超级 trait 确保 `Rs485Driver` 可实现 `DeviceDriver: Send + Sync`，
/// 从而可注册到 `DriverRegistry`。
///
/// # D7 修正
/// 蓝图原设计通过 `&'static dyn HalGpio` 控制 DE/RE，但 `HalGpio` 无 `Send + Sync`
/// 超级 trait，导致 `Rs485Driver` 无法满足 `DeviceDriver: Send + Sync`。
/// 现将 DE/RE 控制方法合并到 `UartHw` trait 中，由实现方内部处理 GPIO。
pub trait UartHw: Send + Sync {
    /// 配置 UART 硬件参数（波特率/数据位/停止位/校验）
    ///
    /// # 参数
    /// - `baud_rate`: 波特率（如 9600/115200）
    /// - `data_bits`: 数据位（7 或 8）
    /// - `stop_bits`: 停止位（1 或 2）
    /// - `parity`: 校验位
    fn configure(
        &mut self,
        baud_rate: u32,
        data_bits: u8,
        stop_bits: StopBits,
        parity: Parity,
    ) -> Result<(), DriverError>;

    /// 启用接收中断
    fn enable_rx_irq(&mut self) -> Result<(), DriverError>;

    /// 禁用接收中断
    fn disable_rx_irq(&mut self) -> Result<(), DriverError>;

    /// 读取单个字节（非阻塞）
    ///
    /// # 返回
    /// - `Some(u8)`: 读取到一字节
    /// - `None`: 无数据可读
    fn read_byte(&mut self) -> Option<u8>;

    /// 写入多字节数据（阻塞直到全部写入或出错）
    ///
    /// # 返回
    /// 成功写入的字节数
    fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError>;

    /// 等待发送完成
    ///
    /// # 参数
    /// - `timeout_ms`: 超时时间（毫秒）
    ///
    /// # 返回
    /// - `Ok(())`: 发送完成
    /// - `Err(DriverError::Timeout)`: 超时
    fn wait_tx_done(&mut self, timeout_ms: u32) -> Result<(), DriverError>;

    /// 返回接收中断的 IRQ 编号
    fn rx_irq_id(&self) -> u32;

    /// 返回当前单调时间（纳秒）
    ///
    /// 用于 `Rs485Driver::recv()` 中的超时与帧间隔检测。
    /// 实现方通常委托给 `HalClock::now_ns()`。
    fn now_ns(&self) -> u64;

    /// 配置 DE/RE 方向控制 GPIO 引脚（D7 修正：DE/RE 由 UartHw 管理）
    ///
    /// 在 `Rs485Driver::init()` 中调用。实现方应将指定引脚配置为输出并置低
    /// （接收模式）。`pin = None` 表示无 DE/RE 方向控制（如环回测试）。
    ///
    /// # 参数
    /// - `pin`: DE/RE 控制的 GPIO 引脚号，或 `None` 表示不使用
    fn configure_de_re(&mut self, pin: Option<u32>) -> Result<(), DriverError>;

    /// 设置 DE/RE 方向（D7 修正）
    ///
    /// # 参数
    /// - `high`: `true` = 发送模式（DE=1, RE=0），`false` = 接收模式（DE=0, RE=1）
    fn set_de_re(&mut self, high: bool) -> Result<(), DriverError>;
}
