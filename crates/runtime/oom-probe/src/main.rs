//! 22 §2.W1.1 验收 3 探针：构造堆耗尽 → `#[alloc_error_handler]` 触发而非
//! abort（aarch64-unknown-none 裸机，QEMU `-machine virt -kernel` 直跑）。
//!
//! 运行（workspace 根，见 `tests/oom-probe.sh`）：
//! ```text
//! cd crates/runtime/oom-probe && RUSTFLAGS="-C panic=abort -C link-arg=-T$(pwd)/linker.ld" \
//!     cargo build --target aarch64-unknown-none \
//!     -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
//! timeout 15 qemu-system-aarch64 -machine virt -cpu cortex-a57 -m 64M \
//!     -nographic -kernel <workspace>/target/aarch64-unknown-none/debug/oom-probe
//! ```
//! 期望 stdout：`[oom-probe] alloc_error_handler triggered ...`（handler 运行
//! 且挂起，进程未 abort；timeout 退出码 124 属预期——探针按设计不返回）。
//!
//! 判定语义：handler 触发标记出现 = 堆耗尽走 fail-safe（记录+打印+挂起）
//! 而非 abort。与生产实现 `eneros_runtime::oom::oom_fail_safe` 同语义
//! （记录 → 打印 → 挂起；无分配输出，UART 轮询写）。
//!
//! host 构建（cargo test/clippy --all-targets）：target 侧逻辑整体
//! `cfg(target_os = "none")` 隐藏（root-task 同模式），host 为空壳 main。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]

#[cfg(target_os = "none")]
mod probe {
    extern crate alloc;

    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::panic::PanicInfo;
    use core::sync::atomic::{AtomicUsize, Ordering};

    // ========================================================================
    // 入口：设置 SP → rust_main（QEMU virt -kernel 跳 ELF entry，MMU 关闭，
    // 物理地址直访；栈在 .bss 预留——汇编标签保证被 _start 引用而不被
    // --gc-sections 回收）。
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
        puts("[oom-probe] rust_main: exhausting heap...\n");
        let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        loop {
            v.push(0x5A); // 耗尽 4 KiB 堆 → alloc_error_handler（不返回）
        }
    }

    // ========================================================================
    // 全局分配器：4 KiB bump（并发安全模型同 services::allocator）。
    // ========================================================================
    const HEAP_SIZE: usize = 4 * 1024;

    struct TinyBump {
        heap: UnsafeCell<[u8; HEAP_SIZE]>,
        offset: AtomicUsize,
    }

    unsafe impl Sync for TinyBump {}
    impl TinyBump {
        const fn new() -> Self {
            Self {
                heap: UnsafeCell::new([0u8; HEAP_SIZE]),
                offset: AtomicUsize::new(0),
            }
        }
    }

    // SAFETY: UnsafeCell 封装堆存储，offset CAS 保证区间互不重叠（与
    // services::allocator::BumpAllocator 同并发模型）。
    unsafe impl GlobalAlloc for TinyBump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let heap_ptr = self.heap.get() as *mut u8;
            let heap_start = heap_ptr as usize;
            let align_mask = !(layout.align() - 1);
            let mut off = self.offset.load(Ordering::Relaxed);
            loop {
                let aligned_abs = (heap_start + off + layout.align() - 1) & align_mask;
                let aligned = aligned_abs - heap_start;
                let end = aligned.saturating_add(layout.size());
                if end > HEAP_SIZE {
                    return core::ptr::null_mut(); // OOM → null（GlobalAlloc 契约）
                }
                match self.offset.compare_exchange_weak(
                    off,
                    end,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return unsafe { heap_ptr.add(aligned) },
                    Err(cur) => off = cur,
                }
            }
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }

    #[global_allocator]
    static ALLOC: TinyBump = TinyBump::new();

    // ========================================================================
    // 22 §2.W1.1：alloc_error_handler —— 记录（计数+布局）→ UART 打印 →
    // 挂起。与 eneros_runtime::oom::oom_fail_safe 同语义；此处 UART 直写
    // 避免任何依赖。
    // ========================================================================
    static OOM_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[alloc_error_handler]
    fn oom(layout: Layout) -> ! {
        let n = OOM_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        puts("[oom-probe] alloc_error_handler triggered #");
        put_decimal(n);
        puts(" layout=size:");
        put_hex(layout.size());
        puts(",align:");
        put_hex(layout.align());
        puts(" — suspending (fail-closed, not abort)\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // ========================================================================
    // panic handler（fail-closed 兜底；panic=abort 下不可达则无碍）。
    // ========================================================================
    #[panic_handler]
    fn panic(_info: &PanicInfo) -> ! {
        puts("[oom-probe] panic — suspending\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // ========================================================================
    // UART 输出：QEMU virt PL011 @ 0x09000000（-nographic 接到 stdio）。
    // 无分配：轮询 FR.TXFF 后写 DR。
    // ========================================================================
    const UART_BASE: usize = 0x0900_0000;
    const PL011_FR: usize = 0x18; // 标志寄存器
    const PL011_FR_TXFF: u32 = 1 << 5; // 发送 FIFO 满
    const PL011_DR: usize = 0x00; // 数据寄存器

    fn putc(c: u8) {
        // SAFETY: virt 平台 PL011 MMIO 固定地址；轮询-写，无别名并发。
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

    fn put_decimal(mut v: usize) {
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putc(b'0');
            return;
        }
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putc(buf[i]);
        }
    }

    fn put_hex(mut v: usize) {
        let mut buf = [0u8; 16];
        let mut n = 0;
        if v == 0 {
            putc(b'0');
            return;
        }
        while v > 0 {
            buf[n] = match v & 0xF {
                0..=9 => b'0' + (v & 0xF) as u8,
                d => b'a' + (d - 10) as u8,
            };
            v >>= 4;
            n += 1;
        }
        for i in (0..n).rev() {
            putc(buf[i]);
        }
    }
}

/// host 构建空壳（无 seL4/裸机环境，探针仅 aarch64-unknown-none 有意义）。
#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("oom-probe 仅用于 aarch64-unknown-none 目标构建（QEMU 裸机验证）");
}
