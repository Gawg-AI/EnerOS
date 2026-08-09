//! ARM PrimeCell PL011 UART HAL implementation.
//!
//! Implements [`crate::HalSerial`] for the ARM PrimeCell PL011 UART, the
//! default serial device on the QEMU `virt` platform (base `0x0900_0000`).
//!
//! # Register map
//! - `DR`: Data Register
//! - `FR`: Flag Register (TXFF / RXFE / BUSY)
//! - `IBRD` / `FBRD`: Integer / Fractional Baud Rate Divisors
//! - `LCRH`: Line Control (8N1, FIFO enable)
//! - `CR`: Control Register (UARTEN / TXE / RXE)
//! - `IMSC`: Interrupt Mask Set Clear

// ---------------------------------------------------------------------------
// PL011 register offsets
// ---------------------------------------------------------------------------
const PL011_DR: u64 = 0x00; // Data Register
const PL011_FR: u64 = 0x18; // Flag Register
const PL011_IBRD: u64 = 0x24; // Integer Baud Rate Divisor
const PL011_FBRD: u64 = 0x28; // Fractional Baud Rate Divisor
const PL011_LCRH: u64 = 0x2C; // Line Control
const PL011_CR: u64 = 0x30; // Control Register
const PL011_IMSC: u64 = 0x38; // Interrupt Mask Set Clear

// ---------------------------------------------------------------------------
// PL011 flag register bits
// ---------------------------------------------------------------------------
const FR_TXFF: u32 = 1 << 5; // Transmit FIFO Full
const FR_RXFE: u32 = 1 << 4; // Receive FIFO Empty
const FR_BUSY: u32 = 1 << 3; // UART Busy

// ---------------------------------------------------------------------------
// 有界忙等上限（HAL-005）
// ---------------------------------------------------------------------------
/// TX FIFO 空出轮询上限（HAL-005：有界忙等，防死循环）。
const UART_TX_MAX_ITERS: u32 = 10_000;
/// BUSY 清除轮询上限（HAL-005：有界忙等，防死循环）。
const UART_FLUSH_MAX_ITERS: u32 = 10_000;

/// ARM PrimeCell PL011 UART HAL implementation.
pub struct Pl011Uart {
    base: u64,
}

impl Pl011Uart {
    /// Create a new PL011 UART HAL instance at the given MMIO base address.
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    /// Write a 32-bit MMIO register.
    ///
    /// # Safety
    /// `base + off` 必须指向有效的 PL011 寄存器映射（MMIO），且当前执行
    /// 上下文对该地址具有写权限；写入为 volatile 语义，调用者保证不存在
    /// 与其它线程/中断的重叠访问竞争（本 crate 中 PL011 为单例串口）。
    #[inline]
    unsafe fn w32(base: u64, off: u64, v: u32) {
        // SAFETY: 由调用方保证 `base + off` 是有效 PL011 MMIO 地址且可写。
        unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) };
    }

    /// Read a 32-bit MMIO register.
    ///
    /// # Safety
    /// `base + off` 必须指向有效的 PL011 寄存器映射（MMIO），且当前执行
    /// 上下文对该地址具有读权限；读取为 volatile 语义，调用者保证无
    /// 与其它线程/中断的重叠访问竞争。
    #[inline]
    unsafe fn r32(base: u64, off: u64) -> u32 {
        // SAFETY: 由调用方保证 `base + off` 是有效 PL011 MMIO 地址且可读。
        unsafe { core::ptr::read_volatile((base + off) as *const u32) }
    }

    /// Initialize the UART for `baud` baud at `clock_hz` reference clock.
    ///
    /// Configures 8 data bits, no parity, 1 stop bit (8N1) with FIFOs enabled.
    ///
    /// # Safety
    /// Touches memory-mapped hardware registers; caller must ensure the base
    /// address is a valid PL011 register block.
    pub fn init(&self, baud: u32, clock_hz: u32) {
        // SAFETY: MMIO writes to configure the UART; caller guarantees the
        // base address is a valid PL011 register block.
        unsafe {
            // 1. Disable UART.
            Self::w32(self.base, PL011_CR, 0);
            // 2. Calculate baud divisor: divisor = (clock * 4) / baud.
            let divisor = (clock_hz * 4) / baud;
            let ibrd = divisor >> 6;
            let fbrd = divisor & 0x3f;
            Self::w32(self.base, PL011_IBRD, ibrd);
            Self::w32(self.base, PL011_FBRD, fbrd);
            // 3. Line control: 8N1 + FIFO enable (WLEN_8 | FEN).
            Self::w32(self.base, PL011_LCRH, 0x70);
            // 4. Disable all interrupts.
            Self::w32(self.base, PL011_IMSC, 0);
            // 5. Enable UART: UARTEN | TXE | RXE.
            Self::w32(self.base, PL011_CR, 0x301);
        }
    }
}

impl crate::HalSerial for Pl011Uart {
    fn write(&self, data: &[u8]) -> Result<usize, crate::HalError> {
        for (i, &byte) in data.iter().enumerate() {
            // HAL-005：有界忙等 TX FIFO 空出（旧代码无限忙等）。
            // SAFETY: `self.base` 由构造时保证为有效 PL011 基址；FR 寄存器
            // 为只读标志位，读取无副作用。
            crate::wait_until(
                || unsafe { Self::r32(self.base, PL011_FR) } & FR_TXFF == 0,
                UART_TX_MAX_ITERS,
            )
            .map_err(|_| crate::HalError::Timeout)?;
            // SAFETY: `self.base` 为有效 PL011 基址；DR 写入为 MMIO 语义。
            unsafe { Self::w32(self.base, PL011_DR, byte as u32) };
            let _ = i;
        }
        Ok(data.len())
    }

    fn read(&self, buf: &mut [u8]) -> Result<usize, crate::HalError> {
        let mut count = 0;
        for slot in buf.iter_mut() {
            // SAFETY: `self.base` 由构造时保证为有效 PL011 基址；FR 为只读标志。
            if unsafe { Self::r32(self.base, PL011_FR) } & FR_RXFE != 0 {
                break;
            }
            // SAFETY: `self.base` 为有效 PL011 基址；DR 读取为 MMIO 语义。
            *slot = unsafe { Self::r32(self.base, PL011_DR) } as u8;
            count += 1;
        }
        Ok(count)
    }

    fn flush(&self) -> Result<(), crate::HalError> {
        // HAL-005：有界忙等 BUSY 清除（旧代码无限忙等）。
        // SAFETY: `self.base` 由构造时保证为有效 PL011 基址；FR 为只读标志。
        crate::wait_until(
            || unsafe { Self::r32(self.base, PL011_FR) } & FR_BUSY == 0,
            UART_FLUSH_MAX_ITERS,
        )
        .map_err(|_| crate::HalError::Timeout)
    }
}

static ARM64_UART: Pl011Uart = Pl011Uart::new(0x09000000);

/// Returns the ARM64 PL011 UART serial HAL singleton.
pub fn serial() -> &'static dyn crate::HalSerial {
    &ARM64_UART
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pl011_register_offsets() {
        assert_eq!(PL011_DR, 0x00);
        assert_eq!(PL011_FR, 0x18);
        assert_eq!(PL011_IBRD, 0x24);
        assert_eq!(PL011_FBRD, 0x28);
        assert_eq!(PL011_LCRH, 0x2C);
        assert_eq!(PL011_CR, 0x30);
        assert_eq!(PL011_IMSC, 0x38);
    }

    #[test]
    fn pl011_flag_bits() {
        assert_eq!(FR_TXFF, 32);
        assert_eq!(FR_RXFE, 16);
        assert_eq!(FR_BUSY, 8);
    }
}
