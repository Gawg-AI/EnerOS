//! Minimal PL011 UART serial driver (蓝图 §4.5)
//!
//! 提供 `SerialOut` trait 与 `Pl011Serial` 驱动实现，用于启动阶段的串口输出。
//! PL011 是 ARM 平台常见的串口 IP，QEMU virt 与多数 ARM64 开发板均兼容。

/// Serial output interface (蓝图 §4.2)
///
/// 启动阶段使用的串口输出抽象，支持单字符、字符串与十六进制输出。
pub trait SerialOut {
    /// 输出单个字节
    fn putc(&self, c: u8);
    /// 输出字符串（`\n` 自动补 `\r`）
    fn puts(&self, s: &str);
    /// 输出 64 位十六进制（`0x` 前缀，16 位定宽）
    fn hex(&self, v: u64);
}

// PL011 寄存器偏移
const UART_DR: u64 = 0x00; // 数据寄存器
const UART_FR: u64 = 0x18; // 标志寄存器
const FR_TXFF: u8 = 1 << 5; // 发送 FIFO 满

/// PL011 compatible serial driver
///
/// 通过内存映射寄存器访问 PL011 串口。`base` 为串口寄存器基址。
pub struct Pl011Serial {
    base: u64,
}

impl Pl011Serial {
    /// 创建一个 PL011 串口驱动实例
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    /// 读寄存器
    #[inline]
    unsafe fn read(&self, off: u64) -> u32 {
        core::ptr::read_volatile((self.base + off) as *const u32)
    }

    /// 写寄存器
    #[inline]
    unsafe fn write(&self, off: u64, v: u32) {
        core::ptr::write_volatile((self.base + off) as *mut u32, v);
    }
}

impl SerialOut for Pl011Serial {
    fn putc(&self, c: u8) {
        unsafe {
            // 等待发送 FIFO 不满
            while (self.read(UART_FR) & FR_TXFF as u32) != 0 {}
            self.write(UART_DR, c as u32);
        }
    }

    fn puts(&self, s: &str) {
        for &b in s.as_bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }

    fn hex(&self, v: u64) {
        let s = b"0123456789ABCDEF";
        self.puts("0x");
        for i in (0..16).rev() {
            let nib = ((v >> (i * 4)) & 0xF) as usize;
            self.putc(s[nib]);
        }
    }
}
