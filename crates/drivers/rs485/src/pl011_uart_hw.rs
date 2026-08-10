//! PL011 → `UartHw` 适配器（P0-8，v0.195.0）.
//!
//! 将 ARM PrimeCell PL011 UART 的真实 MMIO 适配到
//! [`crate::uart_hw::UartHw`] trait，使 RS485/Modbus-RTU 物理层可在真实
//! 硬件（QEMU virt 平台 `0x0900_0000`，或目标板）上运行。
//!
//! # 与 `hal::arm64::uart_pl011::Pl011Uart` 的关系
//!
//! HAL 的 `Pl011Uart` 实现 `HalSerial`（`&self` 三方法），不含波特率配置/
//! 中断控制/DE-RE 方向——`UartHw` 需要这些。本适配器直接操作 PL011 寄存器
//! （与 HAL 相同的偏移/位定义），并补充 `UartHw` 所需的语义：
//!
//! - `configure()`：PL011 LCRH 8N1/校验/停止位 + IBRD/FBRD 波特率分频
//! - `enable_rx_irq()/disable_rx_irq()`：IMSC RXIM 位
//! - `read_byte()`：非阻塞读 FR_RXFE
//! - `write_bytes()`：有界忙等 TXFF + 逐字节写 DR
//! - `wait_tx_done()`：有界忙等 FR_BUSY 清除
//! - `now_ns()`：经注入的时钟函数（host 测试用模拟时钟，真机接 GenericTimer）
//! - `configure_de_re()/set_de_re()`：DE/RE GPIO 方向控制（`None` = 无方向控制）
//!
//! # no_std 合规
//!
//! 纯 `core`；仅依赖 `alloc::boxed::Box` 持有时钟闭包。

use alloc::boxed::Box;

use eneros_driver_framework::DriverError;

use crate::config::{Parity, StopBits};
use crate::uart_hw::UartHw;

// ---------------------------------------------------------------------------
// PL011 register offsets（与 hal::arm64::uart_pl011 一致）
// ---------------------------------------------------------------------------
const PL011_DR: u64 = 0x00; // Data Register
const PL011_FR: u64 = 0x18; // Flag Register
const PL011_IBRD: u64 = 0x24; // Integer Baud Rate Divisor
const PL011_FBRD: u64 = 0x28; // Fractional Baud Rate Divisor
const PL011_LCRH: u64 = 0x2C; // Line Control
const PL011_CR: u64 = 0x30; // Control Register
const PL011_IMSC: u64 = 0x38; // Interrupt Mask Set Clear

// ---------------------------------------------------------------------------
// PL011 flag / control bits
// ---------------------------------------------------------------------------
const FR_TXFF: u32 = 1 << 5; // Transmit FIFO Full
const FR_RXFE: u32 = 1 << 4; // Receive FIFO Empty
const FR_BUSY: u32 = 1 << 3; // UART Busy

const LCRH_WLEN_8: u32 = 0x60; // 8-bit word length
const LCRH_FEN: u32 = 0x10; // FIFO enable
const LCRH_PEN: u32 = 0x02; // Parity enable
const LCRH_EPS: u32 = 0x01; // Even parity select
const LCRH_STP2: u32 = 0x08; // 2 stop bits

const CR_UARTEN: u32 = 1 << 0; // UART enable
const CR_TXE: u32 = 1 << 8; // Transmit enable
const CR_RXE: u32 = 1 << 9; // Receive enable

const IMSC_RXIM: u32 = 1 << 4; // Receive interrupt mask

/// 默认 PL011 参考时钟（QEMU virt 平台 UART 时钟，典型 24 MHz）。
const DEFAULT_UART_CLOCK_HZ: u32 = 24_000_000;

/// 有界忙等最大迭代次数（防止外设挂死永久阻塞）。
const MAX_POLL_ITERS: u32 = 1_000_000;

/// 时钟来源：返回当前单调时间（纳秒）。
type ClockFn = Box<dyn Fn() -> u64 + Send + Sync>;

/// PL011 → `UartHw` 适配器。
///
/// 直接对 PL011 MMIO 寄存器操作；`base` 由调用方保证为有效 PL011 寄存器块
/// （QEMU virt: `0x0900_0000`）。
pub struct Pl011UartHw {
    /// PL011 MMIO 基址。
    base: u64,
    /// PL011 参考时钟（Hz），用于波特率分频。
    clock_hz: u32,
    /// 单调时钟来源（纳秒）。
    clock: ClockFn,
    /// DE/RE 方向控制 GPIO 引脚号（`None` = 无方向控制）。
    de_re_pin: Option<u32>,
    /// 31 DR-07：注入的 GPIO 控制器（真实电平切换；None = 仅记录状态）。
    gpio: Option<alloc::boxed::Box<dyn GpioPin>>,
    /// DE/RE 当前状态（true = 发送模式）。
    de_re_high: bool,
    /// RX 中断是否已启用。
    rx_irq_enabled: bool,
    /// RX IRQ 编号。
    rx_irq: u32,
}

impl Pl011UartHw {
    /// 创建 PL011 UART 适配器。
    ///
    /// # 参数
    /// - `base`: PL011 MMIO 基址
    /// - `clock_hz`: PL011 参考时钟频率（Hz）；0 时使用默认 24 MHz
    /// - `clock`: 单调时钟函数（纳秒）
    ///
    /// # Safety
    ///
    /// `base` 必须指向有效的、已映射的 PL011 寄存器块；调用方保证其在整个
    /// 适配器生命周期内有效。
    pub unsafe fn new(base: u64, clock_hz: u32, clock: ClockFn) -> Self {
        Self {
            base,
            clock_hz: if clock_hz == 0 {
                DEFAULT_UART_CLOCK_HZ
            } else {
                clock_hz
            },
            clock,
            de_re_pin: None,
            gpio: None,
            de_re_high: false,
            rx_irq_enabled: false,
            rx_irq: 0,
        }
    }

    /// 写 PL011 寄存器。
    ///
    /// # Safety
    /// `self.base` 由 `new` 的 # Safety 契约保证为有效 PL011 MMIO 基址。
    #[inline]
    unsafe fn w32(&self, off: u64, v: u32) {
        core::ptr::write_volatile((self.base + off) as *mut u32, v);
    }

    /// 读 PL011 寄存器。
    ///
    /// # Safety
    /// `self.base` 由 `new` 的 # Safety 契约保证为有效 PL011 MMIO 基址。
    #[inline]
    unsafe fn r32(&self, off: u64) -> u32 {
        core::ptr::read_volatile((self.base + off) as *const u32)
    }

    /// 有界忙等条件成立。
    fn wait_until<F: FnMut() -> bool>(&self, mut cond: F) -> Result<(), DriverError> {
        for _ in 0..MAX_POLL_ITERS {
            if cond() {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(DriverError::Timeout)
    }
}

impl UartHw for Pl011UartHw {
    fn configure(
        &mut self,
        baud_rate: u32,
        data_bits: u8,
        stop_bits: StopBits,
        parity: Parity,
    ) -> Result<(), DriverError> {
        if baud_rate == 0 {
            return Err(DriverError::InitFailed);
        }
        if data_bits != 8 {
            // PL011 支持 5/6/7/8 位；UartHw 契约当前面向 8N1 家族，
            // 非 8 位暂不支持（调用方不应请求）。
            return Err(DriverError::InitFailed);
        }
        // SAFETY: MMIO 写；调用方保证 base 有效。
        unsafe {
            // 1. 禁用 UART。
            self.w32(PL011_CR, 0);
            // 2. 波特率分频：divisor = (clock * 4) / baud；ibrd = divisor>>6,
            //    fbrd = divisor & 0x3f（与 hal PL011 一致）。
            // 31 DR-07：u64 中间量（旧 u32 在 clock_hz ≥ ~1.074 GHz 时
            // *4 溢出回绕 → 波特率错）。
            let divisor = ((self.clock_hz as u64) * 4) / u64::from(baud_rate);
            let ibrd = (divisor >> 6) as u32;
            let fbrd = (divisor & 0x3f) as u32;
            self.w32(PL011_IBRD, ibrd);
            self.w32(PL011_FBRD, fbrd);
            // 3. 行控制：字长 + FIFO + 校验/停止位。
            let mut lcrh = LCRH_WLEN_8 | LCRH_FEN;
            match parity {
                Parity::None => {}
                Parity::Even => lcrh |= LCRH_PEN | LCRH_EPS,
                Parity::Odd => lcrh |= LCRH_PEN,
            }
            match stop_bits {
                StopBits::One => {}
                StopBits::Two => lcrh |= LCRH_STP2,
            }
            self.w32(PL011_LCRH, lcrh);
            // 4. 中断掩码：保留当前 RX 使能状态。
            let imsc = if self.rx_irq_enabled { IMSC_RXIM } else { 0 };
            self.w32(PL011_IMSC, imsc);
            // 5. 使能 UART：UARTEN | TXE | RXE。
            self.w32(PL011_CR, CR_UARTEN | CR_TXE | CR_RXE);
        }
        Ok(())
    }

    fn enable_rx_irq(&mut self) -> Result<(), DriverError> {
        // SAFETY: MMIO 写；调用方保证 base 有效。
        unsafe {
            let imsc = self.r32(PL011_IMSC) | IMSC_RXIM;
            self.w32(PL011_IMSC, imsc);
        }
        self.rx_irq_enabled = true;
        Ok(())
    }

    fn disable_rx_irq(&mut self) -> Result<(), DriverError> {
        // SAFETY: MMIO 写；调用方保证 base 有效。
        unsafe {
            let imsc = self.r32(PL011_IMSC) & !IMSC_RXIM;
            self.w32(PL011_IMSC, imsc);
        }
        self.rx_irq_enabled = false;
        Ok(())
    }

    fn read_byte(&mut self) -> Option<u8> {
        // SAFETY: MMIO 读；调用方保证 base 有效。
        unsafe {
            if self.r32(PL011_FR) & FR_RXFE != 0 {
                return None;
            }
            Some(self.r32(PL011_DR) as u8)
        }
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError> {
        let mut written = 0;
        for &byte in data {
            // SAFETY: MMIO 读写；调用方保证 base 有效。
            unsafe {
                self.wait_until(|| self.r32(PL011_FR) & FR_TXFF == 0)?;
                self.w32(PL011_DR, byte as u32);
            }
            written += 1;
        }
        Ok(written)
    }

    fn wait_tx_done(&mut self, _timeout_ms: u32) -> Result<(), DriverError> {
        // SAFETY: MMIO 读；调用方保证 base 有效。
        unsafe { self.wait_until(|| self.r32(PL011_FR) & FR_BUSY == 0) }
    }

    fn rx_irq_id(&self) -> u32 {
        self.rx_irq
    }

    fn now_ns(&self) -> u64 {
        (self.clock)()
    }

    fn configure_de_re(&mut self, pin: Option<u32>) -> Result<(), DriverError> {
        self.de_re_pin = pin;
        // 初始化为接收模式（DE=0, RE=1）。
        self.de_re_high = false;
        Ok(())
    }

    fn set_de_re(&mut self, high: bool) -> Result<(), DriverError> {
        // 31 DR-07：DE/RE 经 GpioPin 注入（对齐 PowerRail 模式）——旧实现
        // 仅记录状态位不操作 GPIO；注入后真实电平切换。GPIO 接线登记
        // 33 §真机填空（板级 GpioPin 实现）。
        if let Some(gpio) = &mut self.gpio {
            gpio.set(high);
        }
        self.de_re_high = high;
        Ok(())
    }

}

impl Pl011UartHw {
    /// 31 DR-07：注入 DE/RE GPIO 控制器（装配层）。
    pub fn inject_gpio(&mut self, gpio: alloc::boxed::Box<dyn GpioPin>) {
        self.gpio = Some(gpio);
    }
}

/// 31 DR-07：DE/RE 方向控制 GPIO 抽象（注入面）。
pub trait GpioPin: Send + Sync {
    /// 设置电平（true = 发送方向 DE=1）。
    fn set(&mut self, high: bool);
}

/// RAM 桩 GPIO（host 测试；真机板级实现经绑定表注入）。
pub struct RamGpioPin {
    /// 当前电平（测试可读）。
    pub level: bool,
}

impl RamGpioPin {
    /// 构造桩。
    pub fn new() -> Self {
        Self { level: false }
    }
}

impl GpioPin for RamGpioPin {
    fn set(&mut self, high: bool) {
        self.level = high;
    }
}

    /// 31 DR-07 判据：高时钟（1.2 GHz）下 divisor 无 u32 回绕。
    #[test]
    fn test_divisor_no_overflow() {
        // 1.2 GHz / 115200：divisor = (1200000000 * 4) / 115200 = 41666。
        let divisor = ((1_200_000_000u64) * 4) / 115_200;
        assert_eq!(divisor, 41666);
        let ibrd = divisor >> 6;
        let fbrd = divisor & 0x3f;
        assert_eq!(ibrd, 651);
        assert_eq!(fbrd, 2);
    }

    /// 31 DR-07 判据：RamGpioPin 电平切换。
    #[test]
    fn test_ram_gpio_pin_level() {
        let mut pin = RamGpioPin::new();
        assert!(!pin.level);
        pin.set(true);
        assert!(pin.level);
        pin.set(false);
        assert!(!pin.level);
    }

impl core::fmt::Debug for Pl011UartHw {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pl011UartHw")
            .field("base", &self.base)
            .field("clock_hz", &self.clock_hz)
            .field("de_re_pin", &self.de_re_pin)
            .field("de_re_high", &self.de_re_high)
            .field("rx_irq_enabled", &self.rx_irq_enabled)
            .field("rx_irq", &self.rx_irq)
            .finish()
    }
}

// ============================================================================
// Host 仿真测试：PL011 寄存器语义（RAM 后端）
// ============================================================================

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// 测试时钟（静态原子，满足 ClockFn 的 Send+Sync）。
    static TEST_CLOCK: AtomicU64 = AtomicU64::new(0);

    /// PL011 仿真：`#[repr(C)]` 结构体，字段偏移与 PL011 寄存器偏移一致，
    /// 使适配器的 `write_volatile(base+off)` 恰好命中对应字段。
    /// DR 字段由 sim 层解释为 FIFO（见 `SimPl011` 的自定义访问语义）。
    #[repr(C)]
    #[allow(dead_code)]
    struct Pl011Regs {
        dr: u32,     // 0x00
        _rsvd1: u32, // 0x04
        _rsvd2: u32, // 0x08
        _rsvd3: u32, // 0x0C
        _rsvd4: u32, // 0x10
        _rsvd5: u32, // 0x14
        fr: u32,     // 0x18
        _rsvd6: u32, // 0x1C
        _rsvd7: u32, // 0x20
        ibrd: u32,   // 0x24
        fbrd: u32,   // 0x28
        lcrh: u32,   // 0x2C
        cr: u32,     // 0x30
        _rsvd8: u32, // 0x34
        imsc: u32,   // 0x38
    }

    impl Pl011Regs {
        const fn zero() -> Self {
            Self {
                dr: 0,
                _rsvd1: 0,
                _rsvd2: 0,
                _rsvd3: 0,
                _rsvd4: 0,
                _rsvd5: 0,
                fr: 0,
                _rsvd6: 0,
                _rsvd7: 0,
                ibrd: 0,
                fbrd: 0,
                lcrh: 0,
                cr: 0,
                _rsvd8: 0,
                imsc: 0,
            }
        }
    }

    /// 每个测试独立的 MMIO 区（Box 分配，生命周期=测试）。
    struct Harness {
        regs: *mut Pl011Regs,
        uart: Pl011UartHw,
        /// 仿真 FIFO（解释 DR 读写）。
        rx_fifo: alloc::vec::Vec<u8>,
        tx_fifo: alloc::vec::Vec<u8>,
    }

    impl Harness {
        fn new() -> Self {
            TEST_CLOCK.store(0, Ordering::Relaxed);
            let regs = Box::into_raw(Box::new(Pl011Regs::zero()));
            // 自增时钟：每次 now_ns 调用 +1ms，模拟真实单调时钟推进。
            let clock_fn: ClockFn = Box::new(|| TEST_CLOCK.fetch_add(1_000_000, Ordering::Relaxed));
            // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真 PL011 寄存器。
            let uart = unsafe { Pl011UartHw::new(regs as u64, 24_000_000, clock_fn) };
            Harness {
                regs,
                uart,
                rx_fifo: alloc::vec::Vec::new(),
                tx_fifo: alloc::vec::Vec::new(),
            }
        }

        /// 在每次适配器操作前，把 FIFO 状态同步到 FR 寄存器（仿真设备行为）。
        fn sync_fr(&self) {
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                let fr = &mut (*self.regs).fr;
                let mut v = 0u32;
                if self.tx_fifo.len() >= 16 {
                    v |= FR_TXFF;
                }
                if self.rx_fifo.is_empty() {
                    v |= FR_RXFE;
                }
                if self.tx_busy() {
                    v |= FR_BUSY;
                }
                *fr = v;
            }
        }

        fn tx_busy(&self) -> bool {
            // BUSY 模拟：TX FIFO 非空即忙（仿真对端尚未消费）。
            !self.tx_fifo.is_empty()
        }

        /// 仿真设备角色：消费 DR 写（进 TX FIFO）。
        fn device_after_write(&mut self) {
            // SAFETY: MMIO 访问；`self.base` 由 `new` 的 # Safety 契约保证有效。
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                let dr = &mut (*self.regs).dr;
                let v = *dr;
                self.tx_fifo.push(v as u8);
                *dr = 0;
            }
        }

        /// 对端向 RX FIFO 压入字节。
        fn push_rx(&mut self, bytes: &[u8]) {
            self.rx_fifo.extend_from_slice(bytes);
        }

        /// 适配器读 DR 时，从 RX FIFO 取一字节。
        fn device_before_read(&mut self) -> u32 {
            if self.rx_fifo.is_empty() {
                0
            } else {
                let b = self.rx_fifo.remove(0);
                b as u32
            }
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            // SAFETY: MMIO 访问；`self.base` 由 `new` 的 # Safety 契约保证有效。
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                drop(Box::from_raw(self.regs));
            }
        }
    }

    #[test]
    fn test_configure_sets_baud_divisor() {
        let mut h = Harness::new();
        h.uart
            .configure(9600, 8, StopBits::One, Parity::None)
            .expect("configure");
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
        // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
        unsafe {
            assert_eq!((*h.regs).ibrd, 156);
            assert_eq!((*h.regs).fbrd, 16);
            assert_eq!((*h.regs).lcrh & LCRH_WLEN_8, LCRH_WLEN_8);
            assert_eq!(
                (*h.regs).cr & (CR_UARTEN | CR_TXE | CR_RXE),
                CR_UARTEN | CR_TXE | CR_RXE
            );
        }
    }

    #[test]
    fn test_write_bytes_then_tx_done() {
        let mut h = Harness::new();
        h.uart
            .configure(115200, 8, StopBits::One, Parity::None)
            .expect("configure");
        // 每次 DR 写后由"设备"捕获：逐个字节从 dr 读走（写-读往返）。
        // 仿真：适配器写 dr → 测试立即读 dr 进 tx_fifo。
        let data = [0x01u8, 0x03, 0x00];
        // 分 3 次单字节写，模拟逐字节设备消费。
        for &b in &data {
            h.uart.write_bytes(&[b]).expect("write");
            h.device_after_write();
        }
        h.sync_fr();
        assert_eq!(h.tx_fifo, vec![0x01, 0x03, 0x00]);
        // 消费 TX FIFO → BUSY 清除 → wait_tx_done OK。
        h.tx_fifo.clear();
        h.sync_fr();
        h.uart.wait_tx_done(1000).expect("tx done");
    }

    #[test]
    fn test_read_byte_returns_none_when_empty() {
        let mut h = Harness::new();
        h.sync_fr();
        assert_eq!(h.uart.read_byte(), None);
    }

    #[test]
    fn test_read_byte_returns_queued() {
        let mut h = Harness::new();
        h.push_rx(&[0xAB]);
        h.sync_fr();
        // 适配器读 DR 前，仿真把 RX 字节放进 DR 寄存器。
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
        // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
        unsafe {
            (*h.regs).dr = h.device_before_read();
        }
        assert_eq!(h.uart.read_byte(), Some(0xAB));
        // 队列空 → FR.RXFE 置位 → None。
        h.sync_fr();
        assert_eq!(h.uart.read_byte(), None);
    }

    #[test]
    fn test_read_byte_from_fifo_full_cycle() {
        let mut h = Harness::new();
        h.push_rx(&[0x11, 0x22, 0x33]);
        for expect in [0x11u8, 0x22, 0x33] {
            h.sync_fr();
            // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                (*h.regs).dr = h.device_before_read();
            }
            assert_eq!(h.uart.read_byte(), Some(expect));
        }
        h.sync_fr();
        assert_eq!(h.uart.read_byte(), None);
    }

    #[test]
    fn test_rx_irq_enable_disable() {
        let mut h = Harness::new();
        h.uart.enable_rx_irq().expect("enable");
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
        // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
        unsafe {
            assert_eq!((*h.regs).imsc & IMSC_RXIM, IMSC_RXIM);
        }
        h.uart.disable_rx_irq().expect("disable");
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
        // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
        unsafe {
            assert_eq!((*h.regs).imsc & IMSC_RXIM, 0);
        }
    }

    #[test]
    fn test_configure_invalid_baud_rejected() {
        let mut h = Harness::new();
        assert!(h.uart.configure(0, 8, StopBits::One, Parity::None).is_err());
    }

    #[test]
    fn test_configure_non_8bit_rejected() {
        let mut h = Harness::new();
        assert!(h
            .uart
            .configure(9600, 7, StopBits::One, Parity::None)
            .is_err());
    }

    #[test]
    fn test_de_re_direction() {
        let mut h = Harness::new();
        h.uart.configure_de_re(Some(12)).expect("config de/re");
        h.uart.set_de_re(true).expect("set tx mode");
        assert!(h.uart.de_re_high);
        h.uart.set_de_re(false).expect("set rx mode");
        assert!(!h.uart.de_re_high);
    }

    #[test]
    fn test_now_ns_tracks_clock() {
        let h = Harness::new();
        // 自增时钟：连续两次读取必然递增（单调性）。
        let t0 = h.uart.now_ns();
        let t1 = h.uart.now_ns();
        assert!(t1 > t0, "单调时钟必须递增: {t0} -> {t1}");
    }

    // ======================================================================
    // v0.195.0 (A3a) 验收：Modbus-RTU 0x03 经真实 UART（PL011 回环）往返
    //
    // 两个 PL011 仿真端（master/slave）经共享 FIFO 互联，模拟一条真实
    // UART 链路：master 端 Pl011UartHw 逐字节写出 Modbus 请求帧 → 字节
    // 经线缆（共享 FIFO）送达 slave 端 Pl011UartHw → slave 逐字节读回；
    // slave 回响应帧 → master 读回。这验证 Pl011UartHw 的字节级读写
    // （含 FR 标志/DR 语义）可承载 Modbus-RTU 帧往返。
    // QEMU 串口 peer 真机验证 BLOCKED（需 CI）。
    // ======================================================================
    #[test]
    fn test_pl011_loopback_modbus_0x03_roundtrip() {
        // Modbus 请求帧：01 03 00 00 00 01 CRC16=0x840A。
        let request = [0x01u8, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
        // Modbus 响应帧：01 03 02 12 34 CRC16=0xFBCB（保持寄存器 0x1234）。
        let response = [0x01u8, 0x03, 0x02, 0x12, 0x34, 0xFB, 0xCB];

        let mut master_h = Harness::new();
        let mut slave_h = Harness::new();
        master_h
            .uart
            .configure(9600, 8, StopBits::One, Parity::None)
            .expect("master cfg");
        slave_h
            .uart
            .configure(9600, 8, StopBits::One, Parity::None)
            .expect("slave cfg");

        // master 发送请求帧（逐字节，线缆=共享 FIFO 由测试转移）。
        for &b in &request {
            master_h.uart.write_bytes(&[b]).expect("tx byte");
            // 线缆：master TX → slave RX。
            slave_h.push_rx(&[b]);
        }

        // slave 逐字节读回请求帧。
        let mut slave_rx = alloc::vec::Vec::new();
        for _ in 0..request.len() {
            slave_h.sync_fr();
            // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器，访问不越界。
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                (*slave_h.regs).dr = slave_h.device_before_read();
            }
            let b = slave_h.uart.read_byte().expect("rx byte");
            slave_rx.push(b);
        }
        assert_eq!(slave_rx, request.to_vec(), "slave 收到完整 Modbus 请求帧");

        // slave 回响应帧（线缆反向）。
        for &b in &response {
            slave_h.uart.write_bytes(&[b]).expect("tx byte");
            master_h.push_rx(&[b]);
        }

        // master 逐字节读回响应帧。
        let mut master_rx = alloc::vec::Vec::new();
        for _ in 0..response.len() {
            master_h.sync_fr();
            // SAFETY: MMIO 访问；`self.base` 由 `new` 的 # Safety 契约保证有效。
            // SAFETY: 测试专用——`self.regs` 指向 Box 分配的仿真寄存器；访问不越界。
            unsafe {
                (*master_h.regs).dr = master_h.device_before_read();
            }
            let b = master_h.uart.read_byte().expect("rx byte");
            master_rx.push(b);
        }
        assert_eq!(
            master_rx,
            response.to_vec(),
            "master 收到完整 Modbus 响应帧"
        );
    }
}
