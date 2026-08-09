//! 22 §2.W1.2 验收 2 探针：注入 panic → 看门狗武装 → 硬件复位 → 重启。
//!
//! aarch64 裸机（QEMU `-machine virt -kernel` 直跑，SP805 @ 0x09050000）：
//! `_start` 直接 `panic!("injected panic")` → panic handler 武装 SP805
//! 10 ms 短超时（与 root-task `panic::panic_reboot` 同序列：打印 marker →
//! `HwWatchdog::init(10)` → spin）→ 看门狗超时复位整机 → QEMU 重新加载
//! kernel → 再次 panic → 循环。
//!
//! 判定：QEMU 日志中 marker（`[panic-probe] arming watchdog reset`）出现
//! **≥2 次** = "注入 panic → 设备重启"证据链成立（单次出现无法区分
//! 挂起 vs 复位；两次及以上证明复位后重新启动并再次执行）。
//!
//! host 构建（cargo test/clippy --all-targets）为空壳（root-task 同模式）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(panic_info_message))]

#[cfg(target_os = "none")]
mod probe {
    use core::panic::PanicInfo;

    // ========================================================================
    // 入口：设置 SP → rust_main（同 oom-probe 模式）。
    // ========================================================================
    core::arch::global_asm!(
        ".pushsection .bss",
        ".balign 16",
        "stack_space: .skip 0x4000",
        "stack_top:",
        ".popsection",
        ".pushsection .text.start",
        ".global _start",
        "_start:",
        "    adrp x0, stack_top",
        "    add x0, x0, :lo12:stack_top",
        "    mov sp, x0",
        "    b rust_main",
        ".popsection",
    );

    #[no_mangle]
    pub extern "C" fn rust_main() -> ! {
        puts("[panic-probe] boot: injecting panic...\n");
        panic!("injected panic (W1.2)");
    }

    // ========================================================================
    // panic handler：武装 SP805 10 ms 超时 → 复位（root-task 同序列）。
    // ========================================================================
    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        puts("[panic-probe] PANIC: ");
        puts(info.message().as_str().unwrap_or("<msg>"));
        puts("\n[panic-probe] arming watchdog reset (SP805 @0x09050000, 10ms)\n");
        let wdt = HwWatchdog::new(0x0905_0000);
        wdt.init(10);
        loop {
            core::hint::spin_loop();
        }
    }

    // ========================================================================
    // SP805 最小驱动（同 eneros-watchdog::wdt，零依赖自包含）。
    // ========================================================================
    struct HwWatchdog {
        base: u64,
    }
    impl HwWatchdog {
        const fn new(base: u64) -> Self {
            Self { base }
        }
        fn init(&self, timeout_ms: u32) {
            if self.base == 0 {
                return;
            }
            // SAFETY: probe 专用裸机 MMIO；SP805 序列：解锁→装载→清中断→
            // 使能(中断+复位)→加锁。volatile 语义。
            unsafe {
                self.w(0xC00, 0x1ACCE551);
                self.w(0x00, timeout_ms * 1000);
                self.w(0x0c, 1);
                self.w(0x08, 0x3);
                self.w(0xC00, 0x1);
            }
        }
        #[inline]
        unsafe fn w(&self, off: u64, v: u32) {
            // SAFETY: base 为有效 SP805 MMIO 基址（调用方保证）。
            unsafe { core::ptr::write_volatile((self.base + off) as *mut u32, v) };
        }
    }

    // ========================================================================
    // UART 输出（同 oom-probe：PL011 @ 0x09000000，轮询 FR.TXFF）。
    // ========================================================================
    const UART_BASE: usize = 0x0900_0000;
    const PL011_FR: usize = 0x18;
    const PL011_FR_TXFF: u32 = 1 << 5;
    const PL011_DR: usize = 0x00;

    fn putc(c: u8) {
        // SAFETY: virt 平台 PL011 MMIO 固定地址；轮询-写。
        unsafe {
            let fr = (UART_BASE + PL011_FR) as *const u32;
            while core::ptr::read_volatile(fr) & PL011_FR_TXFF != 0 {}
            core::ptr::write_volatile((UART_BASE + PL011_DR) as *mut u8, c);
        }
    }

    fn puts(s: &str) {
        for &b in s.as_bytes() {
            if b == b'\n' {
                putc(b'\r');
            }
            putc(b);
        }
    }
}

/// host 构建空壳。
#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("panic-probe 仅用于 aarch64-unknown-none 目标构建（QEMU 裸机验证）");
}
