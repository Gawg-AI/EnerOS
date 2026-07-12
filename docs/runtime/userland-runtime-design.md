# EnerOS 用户态运行时设计 v0.4.0

> 版本：v0.4.0
> 适用范围：EnerOS seL4 用户态 Rust 程序运行时（`eneros-runtime` crate）
> 蓝图依据：`蓝图/phase0.md` §v0.4.0、`蓝图/Power_Native_Agent_OS_Blueprint.md` §43.1
> 关联 crate：`eneros-runtime`、`eneros-sel4-sys`、`eneros-hello`、`eneros-board`

---

## 概述

`eneros-runtime` 是 EnerOS 在 seL4 用户态运行的 Rust 程序所依赖的最小 no_std 运行时库。它为 seL4 用户态 Rust 程序提供以下基础能力：

- `print!` / `println!` 格式化输出宏
- 控制台初始化接口 `init()`
- 基于 `core::fmt::Write` 的格式化输出抽象
- 串口输出的 seL4 syscall 封装

运行时库本身不定义 `panic_handler`、不定义 `_start` 入口、不依赖 `alloc`，是一个纯粹的 no_std 库 crate。panic 处理与程序入口由使用该库的二进制 crate（如 `eneros-hello`）自行定义。

---

## 架构设计

### 三层架构

seL4 用户态 Rust 程序的调用栈分为三层，自顶向下依次为：

```
+---------------------------------------------------+
|  hello 二进制（eneros-hello）                     |
|  - _start 入口                                    |
|  - #[panic_handler]                               |
|  - 调用 runtime::init() 与 println!               |
+-------------------------+-------------------------+
                          |
                          v
+---------------------------------------------------+
|  runtime 库（eneros-runtime）                     |
|  - console: ConsoleWriter + print!/println! 宏    |
|  - serial:  SeL4Serial（实现 SerialOut trait）     |
+-------------------------+-------------------------+
                          |
                          v
+---------------------------------------------------+
|  sel4-sys FFI（eneros-sel4-sys）                  |
|  - seL4_put_char / seL4_send / seL4_recv          |
|  - aarch64: svc #0 inline asm                     |
|  - host: stub 返回 0                              |
+---------------------------------------------------+
```

上层只依赖直接下层，避免跨层耦合。hello 二进制不直接调用 `eneros-sel4-sys`，所有 syscall 都经由 runtime 库封装。

### 模块划分

`eneros-runtime` 内部划分为两个模块：

| 模块 | 职责 | 关键类型/宏 |
|------|------|-------------|
| `serial` | seL4 用户态串口抽象，实现 `SerialOut` trait | `SeL4Serial` |
| `console` | 控制台写入器与格式化宏，基于 `core::fmt::Write` | `ConsoleWriter`、`print!`、`println!`、`init()` |

`lib.rs` 通过 `pub use console::{print, println, init};` 将常用 API 重新导出至 crate 根，使用者可直接 `use eneros_runtime::{println, init};`。

### 与 board crate 的关系

`eneros-board` 提供硬件层的 PL011 串口驱动（`Pl011Serial`，直接访问内存映射寄存器），用于内核/启动阶段在物理串口上输出。`eneros-runtime` 中的 `SeL4Serial` 则是 seL4 用户态的串口抽象，通过 seL4 syscall（`seL4_put_char`）输出字符——用户态程序无权直接访问硬件寄存器，必须经 seL4 内核代理。

两者都实现 `SerialOut` trait，但底层机制不同：

| 维度 | `board::Pl011Serial` | `runtime::SeL4Serial` |
|------|----------------------|----------------------|
| 运行态 | 内核态 / 启动阶段 | seL4 用户态 |
| 输出方式 | 内存映射寄存器（MMIO） | seL4 syscall（`svc #0`） |
| 依赖 crate | 无 | `eneros-sel4-sys` |
| 适用版本 | v0.3.0 起 | v0.4.0 起 |

---

## print!/println! 宏机制

### 实现原理

`print!` / `println!` 宏基于 `core::fmt::Write` trait 实现，无需 `alloc`，纯 no_std。核心流程：

1. 宏展开为对 `ConsoleWriter` 的 `write_fmt` 调用
2. `ConsoleWriter` 实现 `core::fmt::Write` trait 的 `write_str` 方法
3. `write_str` 将字符串逐字节通过 `SeL4Serial::putc` 输出
4. `SeL4Serial::putc` 调用 `eneros_sel4_sys::seL4_put_char` 触发 syscall

### ConsoleWriter 实现

```rust
use core::fmt::{self, Write};
use crate::serial::SeL4Serial;

/// 控制台写入器，实现 core::fmt::Write
pub struct ConsoleWriter;

impl Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let serial = SeL4Serial;
        for &b in s.as_bytes() {
            // \n 前自动补 \r，适配串口终端
            if b == b'\n' {
                serial.putc(b'\r');
            }
            serial.putc(b);
        }
        Ok(())
    }
}
```

### 宏定义

```rust
/// 无换行的格式化输出
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        let _ = core::fmt::Write::write_fmt(
            &mut $crate::console::ConsoleWriter,
            core::format_args!($($arg)*),
        );
    };
}

/// 带换行的格式化输出
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", core::format_args!($($arg)*))
    };
}
```

### 格式说明符支持

由于底层使用 `core::fmt::write`，宏支持所有标准格式说明符：

```rust
println!("Hello from Rust on seL4!");           // 字符串字面量
println!("Target: {}", "aarch64-unknown-none");  // Display
println!("Count: {:?}", 42u64);                  // Debug
println!("Hex: {:#x}", 0xDEADBEEF);              // 十六进制
println!("{} + {} = {}", 1, 2, 3);               // 多参数
```

### 宏展开后的调用链

以 `println!("Hello {}", x)` 为例，宏展开后的调用链如下：

```
println!("Hello {}", x)
  -> print!("Hello {}\n", format_args!("Hello {}", x))
     -> ConsoleWriter.write_fmt(format_args!("Hello {}\n", x))
        -> core::fmt::write(&mut ConsoleWriter, args)
           -> ConsoleWriter.write_str("Hello ")
              -> SeL4Serial.putc('H'), putc('e'), ...
           -> ConsoleWriter.write_str(<x 的字符串表示>)
           -> ConsoleWriter.write_str("\n")
              -> putc('\r'), putc('\n')    // \n 前补 \r
```

---

## 与 sel4-sys 的关系

`eneros-runtime` 通过路径依赖 `eneros-sel4-sys`（`eneros-sel4-sys = { path = "../sel4-sys" }`）获取 seL4 syscall 能力。runtime 不直接内联汇编 `svc #0`，而是经由 sel4-sys 提供的 Rust 函数接口调用。

### 关键调用路径

`SeL4Serial::putc` 的实现：

```rust
use eneros_sel4_sys::seL4_put_char;

pub struct SeL4Serial;

impl crate::board_compat::SerialOut for SeL4Serial {
    fn putc(&self, c: u8) {
        // 调用 seL4 syscall 输出单字符
        let _ = seL4_put_char(c);
    }
    // ...
}
```

### 双实现机制

`eneros-sel4-sys` 提供双实现以支持 host 单元测试：

| 目标架构 | 实现方式 | 行为 |
|----------|---------|------|
| `aarch64` | `core::arch::asm!` 内联 `svc #0` | 触发 seL4 syscall，返回内核结果 |
| 非 aarch64（host） | stub 函数返回 0 | 不触发任何 syscall，便于 host 测试 |

通过 `#[cfg(target_arch = "aarch64")]` 守卫，runtime 库在 host 上编译时链接到 stub，可在 `cargo test` 中验证宏格式化、`\n` → `\r\n` 转换等逻辑，无需真实 seL4 环境。

---

## 初始化流程

### 启动序列

```
seL4 加载 hello 二进制
  -> 跳转到 _start（hello/src/main.rs）
     -> eneros_runtime::init()       // 初始化运行时（当前为 no-op）
     -> println!("Hello from Rust on seL4!")
     -> ... 其他输出 ...
     -> loop { core::hint::spin_loop() }   // 不返回
```

### init() 函数

```rust
/// 初始化用户态运行时
///
/// 当前版本为 no-op：seL4 内核在加载用户态程序时已完成串口初始化，
/// runtime 无需重复配置 PL011 寄存器。保留此函数用于未来扩展
/// （如分配用户堆、初始化日志级别等）。
pub fn init() {}
```

保留 `init()` 作为扩展点，未来版本将在其中加入堆分配初始化、日志系统配置等逻辑，使用方无需修改调用代码。

---

## panic 处理

panic 处理职责位于使用 runtime 的二进制 crate 中（如 `eneros-hello`），而非 runtime 库本身。这是 no_std 库 crate 的标准做法——一个 Rust 程序只能有一个 `#[panic_handler]`，由最终的二进制提供。

`eneros-hello` 中的 panic handler 实现：

```rust
use eneros_runtime::println;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // 通过 runtime 的 println! 宏输出 panic 信息
    println!("[PANIC] {}", info);
    // 死循环，等待内核干预
    loop {
        core::hint::spin_loop();
    }
}
```

panic 输出以 `[PANIC]` 前缀标识，便于在串口日志中快速定位。`PanicInfo` 通过 `{}` 格式说明符输出，包含 panic 发生的文件、行号与消息。

---

## 未来扩展

| 版本 | 计划内容 | 说明 |
|------|---------|------|
| v0.5.0+ | HAL 接口集成 | 与 `eneros-board` 的 HAL trait 对齐，统一内核态/用户态的设备抽象 |
| v0.11.0 | alloc 支持 | seL4 用户堆建立后，runtime 提供 `Vec` / `String` 所需的 `#[global_allocator]` |
| v0.12.0+ | 多组件支持 | IPC（基于 `seL4_send` / `seL4_recv`）、线程抽象 |
| 后续 | 日志系统 | 基于 `log` crate 的 no_std 日志 facade，支持日志级别过滤 |
| 后续 | 可能迁移至 rust-sel4 | 评估 `seL4/rust-sel4` 官方 crate 替代自研 `eneros-sel4-sys` |

`init()` 函数将随版本演进而扩展，但 API 签名保持稳定，确保使用方代码向前兼容。
