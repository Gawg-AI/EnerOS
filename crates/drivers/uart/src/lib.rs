//! WP-27.3 D1（蓝图/27 §7）：PL011 UART 驱动——寄存器层 + `DeviceDriver`。
//!
//! # 五步模板进度
//! - ① host 纯逻辑测试：寄存器状态机（内存模拟后端，`tests`）
//! - ② PL011 寄存器常量表（来源：ARM PrimeCell UART (PL011) TRM DDI0183；
//!   qemu-virt.dts uart@9000000 节点；偏移/位定义下方常量区）
//! - ③ [`UartDriver`]（DeviceDriver seam）+ root-task BindingTable 条目
//!   （compatible `arm,pl011`）
//! - ④ QEMU 活体验证（pl011@9000000 绑定 + IRQ 收包接线）
//! - ⑤ 真机适配：基址/时钟频率来自平台 dts（reg/clocks 属性），无型号
//!   填空项（PL011 为 ARM 标准外设）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;

use eneros_driver_framework::{
    CapabilityKind, DeviceDriver, DriverError, DriverHealth, DriverId, DriverState, DriverType,
};

// ============================================================================
// PL011 寄存器偏移与位定义（DDI0183；五步模板 ② 寄存器常量表）
// ============================================================================

/// 数据寄存器（R/W）。
pub const PL011_DR: u64 = 0x000;
/// 接收状态/错误清除（RO/W）。
pub const PL011_RSRECR: u64 = 0x004;
/// 标志寄存器（RO）。
pub const PL011_FR: u64 = 0x018;
/// 整数波特率分频（R/W）。
pub const PL011_IBRD: u64 = 0x024;
/// 小数波特率分频（R/W）。
pub const PL011_FBRD: u64 = 0x028;
/// 线控寄存器（R/W）。
pub const PL011_LCRH: u64 = 0x02C;
/// 控制寄存器（R/W）。
pub const PL011_CR: u64 = 0x030;
/// 中断 FIFO 电平选择（R/W）。
pub const PL011_IFLS: u64 = 0x034;
/// 中断掩码置位（R/W）。
pub const PL011_IMSC: u64 = 0x038;
/// 原始中断状态（RO）。
pub const PL011_RIS: u64 = 0x03C;
/// 屏蔽中断状态（RO）。
pub const PL011_MIS: u64 = 0x040;
/// 中断清除（W1C）。
pub const PL011_ICR: u64 = 0x044;

/// FR：TX FIFO 满。
pub const PL011_FR_TXFF: u32 = 1 << 5;
/// FR：RX FIFO 空。
pub const PL011_FR_RXFE: u32 = 1 << 6;
/// CR：UART 使能。
pub const PL011_CR_UARTEN: u32 = 1 << 0;
/// CR：TX 使能。
pub const PL011_CR_TXE: u32 = 1 << 8;
/// CR：RX 使能。
pub const PL011_CR_RXE: u32 = 1 << 9;
/// IMSC/ICR：接收 FIFO 中断。
pub const PL011_IMSC_RXIM: u32 = 1 << 4;
/// IMSC/ICR：接收超时中断。
pub const PL011_IMSC_RTIM: u32 = 1 << 6;
/// LCRH：8 位数据长度（FEN=0 时）。
pub const PL011_LCRH_WLEN8: u32 = 0b11 << 5;

/// PL011 寄存器访问抽象（生产 = volatile MMIO；测试 = 内存模拟）。
///
/// `Send + Sync`：驱动经 `DeviceDriver`（Send + Sync）object 化，trait
/// object 自动继承两个 marker。
pub trait Pl011Io: Send + Sync {
    fn read(&self, offset: u64) -> u32;
    fn write(&self, offset: u64, value: u32);
}

/// 生产后端：volatile MMIO（基址由绑定表 `map_mmio` 提供）。
pub struct Pl011Mmio {
    base: *mut u8,
}

// SAFETY: `Pl011Mmio` 仅持有已映射 MMIO 基址（root-task 常驻装配，生命周期
// 覆盖驱动存续期）；volatile 访问不引入数据竞争。
unsafe impl Send for Pl011Mmio {}
unsafe impl Sync for Pl011Mmio {}

impl Pl011Mmio {
    /// # Safety
    /// `base` 必须指向已映射的 PL011 寄存器块（长度 ≥ 0x48），且在整个
    /// 驱动生命周期内保持有效（绑定表 `map_mmio` 保证）。
    pub unsafe fn new(base: *mut u8) -> Self {
        Self { base }
    }
}

impl Pl011Io for Pl011Mmio {
    fn read(&self, offset: u64) -> u32 {
        // SAFETY: `base + offset` 在已映射寄存器块内（offset ≤ 0x44）。
        unsafe { core::ptr::read_volatile(self.base.add(offset as usize) as *const u32) }
    }
    fn write(&self, offset: u64, value: u32) {
        // SAFETY: 同上，写 volatile。
        unsafe { core::ptr::write_volatile(self.base.add(offset as usize) as *mut u32, value) }
    }
}

/// UART 驱动（DeviceDriver seam）：PL011 状态机 + RX 环形缓冲 + IRQ 收包。
pub struct UartDriver {
    id: DriverId,
    name: String,
    state: DriverState,
    io: Box<dyn Pl011Io>,
    clock_hz: u32,
    baud: u32,
    rx: [u8; 256],
    rx_head: usize,
    rx_tail: usize,
    rx_overflow: u64,
    /// IRQ 触发计数（WP-27.4 中断计数证据的驱动侧分量）。
    pub irq_count: u64,
    /// 绑定表分配的 IRQ badge（0 = 未绑定，轮询模式）。
    irq_badge: u32,
}

impl UartDriver {
    pub fn new(id: DriverId, name: &str, io: Box<dyn Pl011Io>, clock_hz: u32, baud: u32) -> Self {
        Self {
            id,
            name: String::from(name),
            state: DriverState::Uninitialized,
            io,
            clock_hz,
            baud,
            rx: [0u8; 256],
            rx_head: 0,
            rx_tail: 0,
            rx_overflow: 0,
            irq_count: 0,
            irq_badge: 0,
        }
    }

    // ---- PL011 寄存器层状态机（五步模板 ②；host 可测）----

    /// 初始化序列：波特率分频 → 8N1 → 使能 UART/TX/RX → RX 中断掩码。
    fn pl011_init(&self) {
        // BRD = UARTCLK / (16 × baud)（DDI0183 3.3.3）。
        let brd = self.clock_hz / (16 * self.baud);
        self.io.write(PL011_IBRD, brd);
        self.io.write(PL011_FBRD, 0);
        self.io.write(PL011_LCRH, PL011_LCRH_WLEN8);
        self.io
            .write(PL011_CR, PL011_CR_UARTEN | PL011_CR_TXE | PL011_CR_RXE);
        self.io.write(PL011_IMSC, PL011_IMSC_RXIM | PL011_IMSC_RTIM);
    }

    /// TX FIFO 未满。
    fn can_tx(&self) -> bool {
        self.io.read(PL011_FR) & PL011_FR_TXFF == 0
    }

    /// RX FIFO 非空。
    fn rx_ready(&self) -> bool {
        self.io.read(PL011_FR) & PL011_FR_RXFE == 0
    }

    fn read_byte(&self) -> u8 {
        (self.io.read(PL011_DR) & 0xFF) as u8
    }

    fn write_byte(&self, byte: u8) {
        self.io.write(PL011_DR, u32::from(byte));
    }

    /// 中断服务：清接收中断，返回是否有待收数据。
    fn irq_service(&self) -> bool {
        let mis = self.io.read(PL011_MIS);
        if mis & (PL011_IMSC_RXIM | PL011_IMSC_RTIM) == 0 {
            return false;
        }
        self.io.write(PL011_ICR, PL011_IMSC_RXIM | PL011_IMSC_RTIM);
        true
    }

    // ---- 协议面 ----

    /// 读已收字节（IRQ 收包后由协议层消费）；空 → None。
    pub fn recv(&mut self) -> Option<u8> {
        if self.rx_head == self.rx_tail {
            return None;
        }
        let byte = self.rx[self.rx_tail];
        self.rx_tail = (self.rx_tail + 1) % self.rx.len();
        Some(byte)
    }

    /// 发送一字节（busy-wait TX 就绪；固定上限防挂死）。
    pub fn send_byte(&mut self, byte: u8) -> Result<(), DriverError> {
        for _ in 0..10_000 {
            if self.can_tx() {
                self.write_byte(byte);
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(DriverError::Timeout)
    }

    /// 记录绑定表分配的中断 badge（装配期由 factory 写入；事件循环据此
    /// 路由 IRQ → `handle_irq`）。0 = 未绑定（轮询模式）。
    pub fn set_irq_badge(&mut self, badge: u32) {
        self.irq_badge = badge;
    }

    /// 中断 badge（0 = 未绑定）。
    pub fn irq_badge(&self) -> u32 {
        self.irq_badge
    }

    /// IRQ 服务：清中断 + 收 FIFO 全部字节入环。
    fn service_rx(&mut self) {
        if !self.irq_service() {
            return;
        }
        self.irq_count += 1;
        while self.rx_ready() {
            let byte = self.read_byte();
            let next = (self.rx_head + 1) % self.rx.len();
            if next == self.rx_tail {
                self.rx_overflow += 1; // 环满丢字节（如实计数）
            } else {
                self.rx[self.rx_head] = byte;
                self.rx_head = next;
            }
        }
    }
}

impl DeviceDriver for UartDriver {
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
        self.pl011_init();
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
        self.state = DriverState::Dead;
        Ok(())
    }
    fn handle_irq(&mut self, _irq_id: u32) {
        self.service_rx();
    }
    fn health_check(&self) -> DriverHealth {
        DriverHealth::Healthy
    }
    fn capabilities(&self) -> &'static [CapabilityKind] {
        // 38（蓝图 §7）：UART 发布 Console + SerialBus。
        &[CapabilityKind::Console, CapabilityKind::SerialBus]
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;

    use spin::Mutex;

    use super::*;

    /// 内存模拟后端（共享状态：测试侧可直接注入字节，host 纯逻辑测试）。
    #[derive(Default)]
    struct MockIo {
        regs: Mutex<[u32; 0x48 / 4]>,
        rx_fifo: Arc<Mutex<Vec<u8>>>,
    }

    impl MockIo {
        fn push_rx(&self, bytes: &[u8]) {
            self.rx_fifo.lock().extend_from_slice(bytes);
        }
        fn rx_empty(&self) -> bool {
            self.rx_fifo.lock().is_empty()
        }
    }

    impl Pl011Io for MockIo {
        fn read(&self, offset: u64) -> u32 {
            match offset {
                PL011_FR => {
                    let rxfe = self.rx_empty();
                    (if rxfe { PL011_FR_RXFE } else { 0 })
                }
                PL011_DR => {
                    let mut fifo = self.rx_fifo.lock();
                    if fifo.is_empty() {
                        0
                    } else {
                        u32::from(fifo.remove(0))
                    }
                }
                PL011_MIS => {
                    if self.rx_empty() {
                        0
                    } else {
                        PL011_IMSC_RXIM
                    }
                }
                other => self.regs.lock()[(other / 4) as usize],
            }
        }
        fn write(&self, offset: u64, value: u32) {
            match offset {
                PL011_DR => self.rx_fifo.lock().push((value & 0xFF) as u8),
                PL011_ICR => {}
                other => {
                    self.regs.lock()[(other / 4) as usize] = value;
                }
            }
        }
    }

    fn mock_driver() -> (UartDriver, Arc<Mutex<Vec<u8>>>) {
        let io = MockIo::default();
        let fifo = io.rx_fifo.clone();
        let driver = UartDriver::new(DriverId(1), "uart0", Box::new(io), 24_000_000, 115_200);
        (driver, fifo)
    }

    /// 五步模板 ①：init 寄存器序列。
    #[test]
    fn init_writes_expected_register_sequence() {
        let (mut driver, _) = mock_driver();
        driver.init().unwrap();
        let io = driver.io.as_ref();
        // BRD = 24 MHz / (16 × 115200) = 13
        assert_eq!(io.read(PL011_IBRD), 13);
        assert_eq!(io.read(PL011_FBRD), 0);
        assert_eq!(io.read(PL011_LCRH), PL011_LCRH_WLEN8);
        assert_eq!(
            io.read(PL011_CR),
            PL011_CR_UARTEN | PL011_CR_TXE | PL011_CR_RXE
        );
        assert_eq!(io.read(PL011_IMSC), PL011_IMSC_RXIM | PL011_IMSC_RTIM);
        assert_eq!(driver.state(), DriverState::Ready);
    }

    /// 五步模板 ①：IRQ 收包入环。
    #[test]
    fn irq_service_receives_bytes_into_ring() {
        let (mut driver, fifo) = mock_driver();
        driver.init().unwrap();
        driver.start().unwrap();
        fifo.lock().extend_from_slice(&[b'A', b'B']);
        driver.handle_irq(33);
        assert_eq!(driver.irq_count, 1);
        assert_eq!(driver.recv(), Some(b'A'));
        assert_eq!(driver.recv(), Some(b'B'));
        assert_eq!(driver.recv(), None);
    }

    /// 五步模板 ①：发送回环。
    #[test]
    fn send_byte_loopback() {
        let (mut driver, fifo) = mock_driver();
        driver.init().unwrap();
        driver.send_byte(b'X').unwrap();
        // 回环：发送字节进入 FIFO → IRQ 收包读回。
        driver.handle_irq(33);
        assert_eq!(driver.recv(), Some(b'X'));
    }

    /// 环溢出：计数不 panic。
    #[test]
    fn ring_overflow_counts_but_does_not_panic() {
        let (mut driver, fifo) = mock_driver();
        driver.init().unwrap();
        fifo.lock().extend(vec![0x55u8; 300]);
        driver.handle_irq(33);
        assert!(driver.rx_overflow > 0);
        let mut count = 0;
        while driver.recv().is_some() {
            count += 1;
        }
        assert!(count <= 256);
    }
}
