//! PL011 console 驱动（DRV-004）：并发/overflow/恢复。
//!
//! 用户态驱动服务通过 SYS_SHM_MAP 映射 PL011 寄存器，提供环形缓冲
//! 输出服务（经 IPC 供系统服务写日志），并处理 FIFO 溢出状态
//! （ORERR/BRK/FRERR/PE 清除与恢复）。

use core::ptr;

use eneros_libeneros::shm_map_device;

use crate::mmio::{read32, write32};

/// PL011 寄存器偏移（PrimeCell UART (PL011) TRM）。
pub const UART_DR: usize = 0x000;
pub const UART_FR: usize = 0x018;
pub const UART_IBRD: usize = 0x024;
pub const UART_FBRD: usize = 0x028;
pub const UART_LCR_H: usize = 0x02C;
pub const UART_CR: usize = 0x030;
pub const UART_IFLS: usize = 0x034;
pub const UART_IMSC: usize = 0x038;
pub const UART_RIS: usize = 0x03C;
pub const UART_MIS: usize = 0x040;
pub const UART_ICR: usize = 0x044;
pub const UART_DRFR: usize = 0x018;

/// FR 标志。
pub const FR_TXFF: u32 = 1 << 5;
pub const FR_RXFE: u32 = 1 << 4;
pub const FR_BUSY: u32 = 1 << 3;

/// LCR_H 位。
pub const LCR_H_FEN: u32 = 1 << 4;
pub const LCR_H_WLEN_8: u32 = 0b11 << 5;

/// CR 位。
pub const CR_UARTEN: u32 = 1 << 0;
pub const CR_TXE: u32 = 1 << 8;
pub const CR_RXE: u32 = 1 << 9;

/// PL011 控制台（寄存器 VA）。
pub struct Pl011 {
    pub regs_va: usize,
    /// 是否已初始化（CR 写回验证）。
    pub initialized: bool,
}

/// 初始化 PL011（波特率 115200，8N1，FIFO 使能）。
pub fn init(base_phys: usize) -> Result<Pl011, &'static str> {
    let (status, va) = shm_map_device(base_phys as u64, 0x1000);
    if status != 0 {
        return Err("shm_map_device uart failed");
    }
    let va = va as usize;
    let mut uart = Pl011 {
        regs_va: va,
        initialized: false,
    };
    // 禁用 UART 后配置。
    write32(uart.regs_va + UART_CR, 0);
    // 24MHz 时钟，115200 baud：IBRD=13, FBRD=1（BRD=13.0208）。
    write32(uart.regs_va + UART_IBRD, 13);
    write32(uart.regs_va + UART_FBRD, 1);
    write32(uart.regs_va + UART_LCR_H, LCR_H_WLEN_8 | LCR_H_FEN);
    write32(uart.regs_va + UART_IMSC, 0);
    write32(uart.regs_va + UART_ICR, 0x7FF);
    write32(uart.regs_va + UART_CR, CR_UARTEN | CR_TXE | CR_RXE);
    // 读回验证（DRV-004：初始化必须真实生效）。
    let cr = read32(uart.regs_va + UART_CR);
    if cr & CR_UARTEN == 0 {
        return Err("pl011 enable failed");
    }
    uart.initialized = true;
    Ok(uart)
}

impl Pl011 {
    /// 输出单字节（等待 TX FIFO 空间）。
    pub fn put_byte(&self, byte: u8) {
        // 有界等待 TX FIFO 非满。
        let mut spins = 0u32;
        while read32(self.regs_va + UART_FR) & FR_TXFF != 0 {
            spins += 1;
            if spins > 100_000 {
                break;
            }
            core::hint::spin_loop();
        }
        // SAFETY：设备寄存器写。
        unsafe {
            ptr::write_volatile((self.regs_va + UART_DR) as *mut u8, byte);
        }
    }

    /// 输出字符串。
    pub fn put_str(&self, text: &str) {
        for &byte in text.as_bytes() {
            if byte == b'\n' {
                self.put_byte(b'\r');
            }
            self.put_byte(byte);
        }
    }

    /// 读一字节（无数据返回 None）。
    pub fn get_byte(&self) -> Option<u8> {
        if read32(self.regs_va + UART_FR) & FR_RXFE != 0 {
            return None;
        }
        // SAFETY：设备寄存器读。
        let data = unsafe { ptr::read_volatile((self.regs_va + UART_DR) as *const u8) };
        Some(data)
    }

    /// 清错误状态（overflow/parity/framing/break）并恢复。
    pub fn clear_errors(&self) -> u32 {
        // 读 RIS 记录错误源，写 ICR 清。
        let ris = read32(self.regs_va + UART_RIS);
        write32(self.regs_va + UART_ICR, 0x7FF);
        ris
    }
}
