# EnerOS seL4 API 绑定说明 v0.4.0

> 版本：v0.4.0
> 适用范围：EnerOS seL4 syscall Rust FFI 绑定（`eneros-sel4-sys` crate）
> 蓝图依据：`蓝图/phase0.md` §v0.4.0
> 关联 crate：`eneros-sel4-sys`、`eneros-runtime`、`eneros-hello`
> seL4 版本：14.0.0

---

## 概述

`eneros-sel4-sys` 是 EnerOS 自研的 seL4 最小 syscall Rust FFI 绑定库。它为 seL4 用户态 Rust 程序提供底层系统调用接口，封装 `svc #0` 指令的 inline assembly，向上层（如 `eneros-runtime`）暴露类型安全的 Rust 函数。

设计目标：

- **最小化**：仅绑定 v0.4.0 所需的 3 个 syscall（put_char / send / recv），不引入完整 seL4 API
- **no_std**：纯 `#![no_std]`，无 `alloc` 依赖，可在 `aarch64-unknown-none` 目标编译
- **可测试**：通过 `#[cfg(target_arch = "aarch64")]` 守卫提供 host stub，支持 host 单元测试
- **零外部依赖**：不依赖 `rust-sel4` 官方 crate，降低构建复杂度

---

## syscall 编号

`eneros-sel4-sys` 当前绑定以下 3 个 syscall，编号定义在 crate 根：

| 编号 | 常量 | 函数 | 说明 |
|------|------|------|------|
| 0 | `SYSCALL_PUT_CHAR` | `seL4_put_char` | debug putchar，输出单个字符到串口 |
| 1 | `SYSCALL_SEND` | `seL4_send` | 发送消息到指定 endpoint |
| 2 | `SYSCALL_RECV` | `seL4_recv` | 从指定 endpoint 接收消息 |

常量定义：

```rust
pub const SYSCALL_PUT_CHAR: u64 = 0;
pub const SYSCALL_SEND: u64 = 1;
pub const SYSCALL_RECV: u64 = 2;
```

> **注意**：上述编号为 EnerOS 自研绑定的内部约定，并非 seL4 官方 syscall 编号。未来若迁移至 `rust-sel4` 官方 crate，编号将遵循 seL4 ABI 规范。

---

## 调用约定（aarch64）

### 寄存器使用

`eneros-sel4-sys` 在 `aarch64` 目标上通过 `core::arch::asm!` 内联汇编触发 syscall，遵循以下调用约定：

| 寄存器 | 用途 | 说明 |
|--------|------|------|
| `x0` | 参数 1 / 返回值 | 第一个参数传入，syscall 返回值传出 |
| `x1` | 参数 2 | 第二个参数 |
| `x2` | 参数 3 | 第三个参数 |
| `x3` | 参数 4 | 第四个参数 |
| `x4` | 参数 5 | 第五个参数 |
| `x5` | 参数 6 | 第六个参数 |
| `x7` | syscall 号 | 标识调用的 syscall 类型 |
| — | 触发指令 | `svc #0`（Supervisor Call） |

### inline assembly 示例

以 `seL4_put_char` 为例：

```rust
#[cfg(target_arch = "aarch64")]
pub fn seL4_put_char(c: u8) -> isize {
    let mut ret: isize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x0") c as u64,
            in("x7") SYSCALL_PUT_CHAR,
            lateout("x0") ret,
            options(nostack, preserves_flags),
        );
    }
    ret
}
```

关键点：

- `in("x0")` 将字符传入 `x0` 寄存器
- `in("x7")` 将 syscall 号传入 `x7` 寄存器
- `lateout("x0")` 捕获 syscall 返回值
- `options(nostack, preserves_flags)` 声明 inline asm 不修改栈与标志位，便于编译器优化
- `svc #0` 触发同步异常，陷入 seL4 内核执行对应 syscall

---

## Endpoint 类型

`Endpoint` 表示 seL4 的能力句柄（capability），用于 IPC 操作的端点寻址。

### 定义

```rust
/// seL4 能力句柄
///
/// 封装 seL4 endpoint 的 capability slot 编号，
/// 用于 seL4_send / seL4_recv 等 IPC syscall。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// capability slot 编号
    pub cap: u64,
}
```

### derive 属性说明

| derive | 用途 |
|--------|------|
| `Clone` | 允许复制 Endpoint（值类型，复制开销极低） |
| `Copy` | 允许按值传递而非引用 |
| `Debug` | 支持 `{:?}` 格式化输出，便于调试 |
| `PartialEq` / `Eq` | 支持相等比较，便于测试断言 |

### 用法示例

```rust
use eneros_sel4_sys::{Endpoint, seL4_send, seL4_recv};

// 构造 endpoint
let ep = Endpoint { cap: 42 };

// 发送消息
let status = seL4_send(ep, 0xDEADBEEF);
assert_eq!(status, 0);

// 接收消息（使用 Copy 语义，无需 &ep）
let msg = seL4_recv(ep);
```

由于 `Endpoint` 实现 `Copy`，传递给 syscall 函数时按值复制，无需借用，调用简洁且无生命周期约束。

---

## 已绑定接口列表

### seL4_put_char

```rust
pub fn seL4_put_char(c: u8) -> isize
```

输出单个字符到 seL4 debug 串口。

| 参数 | 类型 | 说明 |
|------|------|------|
| `c` | `u8` | 待输出的 ASCII 字符 |

**返回值**：`isize` — seL4 syscall 返回值，0 表示成功，负值表示错误码。

**调用路径**：`seL4_put_char` → `svc #0`（x0=字符, x7=0）→ seL4 kernel debug putchar → PL011 UART 输出。

**典型用途**：被 `eneros-runtime` 的 `SeL4Serial::putc` 调用，作为 `println!` 宏的底层输出通道。

### seL4_send

```rust
pub fn seL4_send(ep: Endpoint, msg: u64) -> isize
```

向指定 endpoint 发送消息。

| 参数 | 类型 | 说明 |
|------|------|------|
| `ep` | `Endpoint` | 目标 endpoint 的 capability |
| `msg` | `u64` | 待发送的消息（64 位无符号整数） |

**返回值**：`isize` — 0 表示成功，负值表示错误码。

**调用路径**：`seL4_send` → `svc #0`（x0=cap, x1=msg, x7=1）→ seL4 kernel IPC send。

**注意**：当前为阻塞发送，消息被对端接收后才返回。非阻塞发送 `seL4_NBSend` 计划在未来版本支持。

### seL4_recv

```rust
pub fn seL4_recv(ep: Endpoint) -> u64
```

从指定 endpoint 接收消息。

| 参数 | 类型 | 说明 |
|------|------|------|
| `ep` | `Endpoint` | 源 endpoint 的 capability |

**返回值**：`u64` — 接收到的 64 位消息。

**调用路径**：`seL4_recv` → `svc #0`（x0=cap, x7=2）→ seL4 kernel IPC recv → 返回消息至 x0。

**注意**：阻塞接收，直到有对端发送消息才返回。调用方需确保对端会发送消息，否则永久阻塞。

---

## 双实现机制

为支持 host 单元测试，`eneros-sel4-sys` 对每个 syscall 函数提供两套实现，通过 `#[cfg(target_arch = "aarch64")]` 守卫切换。

### aarch64 实现（目标侧）

```rust
#[cfg(target_arch = "aarch64")]
pub fn seL4_put_char(c: u8) -> isize {
    let mut ret: isize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x0") c as u64,
            in("x7") SYSCALL_PUT_CHAR,
            lateout("x0") ret,
            options(nostack, preserves_flags),
        );
    }
    ret
}
```

### host 实现（测试 stub）

```rust
#[cfg(not(target_arch = "aarch64"))]
pub fn seL4_put_char(_c: u8) -> isize {
    // host stub：不触发 syscall，直接返回 0
    0
}
```

### 双实现对照表

| 函数 | aarch64 实现 | host stub 返回值 |
|------|-------------|-----------------|
| `seL4_put_char` | `svc #0` inline asm | `0`（isize） |
| `seL4_send` | `svc #0` inline asm | `0`（isize） |
| `seL4_recv` | `svc #0` inline asm | `0`（u64） |

### 设计目的

| 目的 | 说明 |
|------|------|
| host 单元测试 | `eneros-sel4-sys` 与 `eneros-runtime` 可在 x86_64 host 上 `cargo test`，验证 `Endpoint` 构造、宏格式化、`\n` → `\r\n` 转换等逻辑 |
| 编译隔离 | stub 不引用任何 `aarch64` 专有指令，host 编译不报错 |
| 零运行时开销 | `#[cfg]` 守卫在编译期决定链接哪套实现，无运行时分支判断 |
| CI 友好 | CI 流水线可在标准 runner 上运行单元测试，无需 ARM64 硬件或 QEMU |

> **重要**：host stub 不模拟 seL4 行为，仅返回固定值。涉及真实 syscall 语义的测试必须通过 QEMU + seL4 交叉验证（v0.4.0 范围之外）。

---

## 未来扩展计划

| 版本 | 计划 syscall | 用途 |
|------|-------------|------|
| v0.5.0+ | `seL4_Call` | 同步 IPC（send + recv 原子组合） |
| v0.5.0+ | `seL4_Reply` | 回复消息给等待的 sender |
| v0.6.0+ | `seL4_NBSend` | 非阻塞发送（消息队列满时立即返回错误） |
| v0.8.0+ | 内存映射 syscall | `seL4_Map` / `seL4_Unmap`，支持用户态地址空间管理 |
| v0.11.0+ | 用户堆 syscall | `seL4_Badge` / `seL4_Mint`，支持 capability 衍生与权限管理 |
| 后续 | rust-sel4 迁移 | 评估替换为 `seL4/rust-sel4` 官方 crate，获取完整 API 覆盖与官方维护 |

### 迁移至 rust-sel4 的考量

自研 `eneros-sel4-sys` 在 v0.4.0 满足最小验证需求，但长期存在以下限制：

- 仅覆盖极少数 syscall，完整 seL4 API 需大量重复绑定
- 缺乏 capability 类型系统（rust-sel4 提供类型安全的 cap 包装）
- 需自行维护与 seL4 版本的兼容性（当前锁定 seL4 14.0.0）

当 EnerOS 进入 Phase 1（v0.23+，需要完整 IPC 与内存管理）时，将评估迁移至 `rust-sel4` v3.0.0（`蓝图/Power_Native_Agent_OS_Version_Roadmap_v3.md` 已锁定版本）。迁移期间 `eneros-sel4-sys` 可作为渐进式过渡的兼容层保留。

---

## 参考

- seL4 官方手册：`https://sel4.systems/Info/Docs/`
- seL4 v14.0.0 源码：`https://github.com/seL4/seL4/tree/14.0.0`
- rust-sel4 官方 crate：`https://github.com/seL4/rust-sel4`
- ARM Architecture Reference Manual（AArch64）：`svc` 指令与异常处理
