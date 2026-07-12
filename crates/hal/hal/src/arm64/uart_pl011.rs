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
    #[inline]
    unsafe fn w32(base: u64, off: u64, v: u32) {
        core::ptr::write_volatile((base + off) as *mut u32, v);
    }

    /// Read a 32-bit MMIO register.
    #[inline]
    unsafe fn r32(base: u64, off: u64) -> u32 {
        core::ptr::read_volatile((base + off) as *const u32)
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
        for &byte in data {
            while unsafe { Self::r32(self.base, PL011_FR) } & FR_TXFF != 0 {
                core::hint::spin_loop();
            }
            unsafe { Self::w32(self.base, PL011_DR, byte as u32) };
        }
        Ok(data.len())
    }

    fn read(&self, buf: &mut [u8]) -> Result<usize, crate::HalError> {
        let mut count = 0;
        for slot in buf.iter_mut() {
            if unsafe { Self::r32(self.base, PL011_FR) } & FR_RXFE != 0 {
                break;
            }
            *slot = unsafe { Self::r32(self.base, PL011_DR) } as u8;
            count += 1;
        }
        Ok(count)
    }

    fn flush(&self) -> Result<(), crate::HalError> {
        while unsafe { Self::r32(self.base, PL011_FR) } & FR_BUSY != 0 {
            core::hint::spin_loop();
        }
        Ok(())
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
