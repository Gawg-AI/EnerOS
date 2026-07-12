# EnerOS Panic 处理框架设计

> 版本：v0.14.0 | 日期：2026-07-12 | 状态：设计文档

## 1. 概述

EnerOS Panic 处理框架（crate 名 `eneros-panic`）是 Phase 0 P0-D 的终点交付物，
为系统提供「可诊断、可隔离、可恢复」的 panic 处理能力：

- **可诊断**：panic 触发时通过串口输出结构化上下文（级别、位置、消息、时间戳、核号）。
- **可隔离**：分区级 panic 不会拖垮整个系统，仅标记故障分区为 `Dead`。
- **可恢复**：内核级 panic 走硬件复位路径；分区 panic 留待 v0.18.0 调度器配合做真正的资源回收。

本框架对应蓝图 `phase0.md §v0.14.0`，是「基础 OS 服务就绪」出口标准的关键一环。

## 2. 架构

`eneros-panic` crate 由三个模块构成：

| 模块 | 文件 | 职责 |
|------|------|------|
| 核心 | `panic/src/lib.rs` | `PanicLevel` / `PanicContext` / `PanicStrategy` trait、`KernelResetStrategy` / `PartitionIsolateStrategy`、全局策略表、`handle_panic` 入口、aarch64 专属 `read_core_id` / `hard_reset` |
| 日志器 | `panic/src/logger.rs` | `SerialSink` trait、固定栈缓冲格式化、`panic_log` / `panic_log_raw` / `flush` |
| 分区隔离 | `panic/src/isolation.rs` | `PartitionState` / `IsolationError`、`PARTITION_TABLE` / `HANDLERS`、`mark_partition_dead` / `reset_partition` / `partition_state` / handler 注册查询 |

依赖（`panic/Cargo.toml`）：

```toml
[dependencies]
eneros-time = { path = "../time" }   # 提供时间戳
spin = "0.9"                          # no_std 自旋锁
heapless = "0.8"                       # 测试用 Vec
```

crate 顶层属性为 `#![cfg_attr(not(test), no_std)]`，遵循蓝图 §43.1 全项目 no_std 要求。

## 3. 核心数据结构

### 3.1 PanicLevel

区分 panic 的作用范围，决定走复位还是隔离路径。

```rust
// panic/src/lib.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicLevel {
    /// Kernel-wide panic — requires full system reset.
    Kernel,
    /// Partition-scoped panic — isolate the offending partition.
    Partition(u32),
}
```

### 3.2 PanicContext

panic 事件的不可变快照，五字段全部为 `Copy` 或 `&'static str`，可在无堆环境下安全传递。

```rust
// panic/src/lib.rs
#[derive(Debug)]
pub struct PanicContext {
    pub level: PanicLevel,
    pub location: &'static str,
    pub message: &'static str,
    pub timestamp_ns: u64,
    pub core_id: u32,
}

impl PanicContext {
    pub fn new(level: PanicLevel, location: &'static str, message: &'static str) -> Self {
        Self {
            level,
            location,
            message,
            timestamp_ns: eneros_time::get_monotonic_ns(),
            core_id: read_core_id(),
        }
    }
}
```

`new` 在构造时自动盖上单调时间戳与当前核号。

### 3.3 PanicStrategy trait

所有策略必须实现该 trait，`handle` 不返回（`-> !`）。

```rust
// panic/src/lib.rs
pub trait PanicStrategy {
    fn handle(&self, ctx: &PanicContext) -> !;
}
```

## 4. 分级策略

### 4.1 KernelResetStrategy

内核 panic 的处置路径：日志 → flush → 硬复位。`ResetPolicy` 控制是立即复位还是延迟若干毫秒（留给看门狗触发或日志排空）。

```rust
// panic/src/lib.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetPolicy {
    Immediate,
    Delayed(u64),  // 延迟毫秒数
}

pub struct KernelResetStrategy;

impl PanicStrategy for KernelResetStrategy {
    fn handle(&self, ctx: &PanicContext) -> ! {
        logger::panic_log(ctx);
        logger::flush();
        let policy = *RESET_POLICY.lock();
        match policy {
            ResetPolicy::Immediate => hard_reset(),
            ResetPolicy::Delayed(ms) => {
                let start = eneros_time::get_monotonic_ns();
                let target = start.saturating_add(ms.saturating_mul(1_000_000));
                while eneros_time::get_monotonic_ns() < target {
                    spin_loop();
                }
                hard_reset();
            }
        }
    }
}
```

### 4.2 PartitionIsolateStrategy

分区 panic 的处置路径：日志 → 标记分区 `Dead` → 死循环等待。若 `mark_partition_dead`
失败（id 越界或分区已死），按蓝图 §4.4 升级为内核 panic 复位。

```rust
// panic/src/lib.rs
pub struct PartitionIsolateStrategy {
    pub partition: u32,
}

impl PanicStrategy for PartitionIsolateStrategy {
    fn handle(&self, ctx: &PanicContext) -> ! {
        logger::panic_log(ctx);
        match isolation::mark_partition_dead(self.partition) {
            Ok(()) => loop { spin_loop(); },
            // Isolation failed → escalate to full kernel reset.
            Err(_) => KernelResetStrategy.handle(ctx),
        }
    }
}
```

为避免运行时分配，crate 预分配了 8 个 `PartitionIsolateStrategy` 静态实例，`set_partition_strategy(id)`
直接把对应槽位注册进全局策略表。

## 5. 日志器设计（无 alloc）

### 5.1 SerialSink trait

消费者注入自己的串口实现，框架不绑定具体 UART 驱动。

```rust
// panic/src/logger.rs
pub trait SerialSink {
    fn putc(&self, c: u8);
    fn puts(&self, s: &str);
}
```

### 5.2 固定栈缓冲区

为解决蓝图 §5.4 难点（panic 时堆可能损坏，日志不能用 alloc），格式化使用栈上 `[u8; 256]`
缓冲，通过 `core::fmt::Write` 适配器 `StackBufWriter` 写入。超长时截断而非报错，
保证总能发出一行带换行符的日志。

```rust
// panic/src/logger.rs
struct StackBufWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> fmt::Write for StackBufWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // 截断写入，避免溢出
        ...
    }
}

pub fn panic_log(ctx: &PanicContext) {
    let mut buf = [0u8; 256];
    let mut w = StackBufWriter::new(&mut buf);
    // ... 格式化 ...
    if let Some(sink) = *SERIAL_SINK.lock() {
        sink.puts(s);
    }
}
```

### 5.3 日志格式

```
[PANIC] level=KERNEL loc=<file> msg=<msg> core=<id> t=<ns>ns
[PANIC] level=Partition(3) loc=<file> msg=<msg> core=<id> t=<ns>ns
```

### 5.4 NullSink 与未注册 no-op

未调用 `set_serial_sink` 时 `SERIAL_SINK` 为 `None`，`panic_log` 与 `panic_log_raw`
静默 no-op，不会因 sink 未注册而二次 panic。`NullSink` 提供给测试或显式禁用日志的场景。

另有 `CaptureSink`（基于 `heapless::Vec<u8, 512>`），仅供单元测试断言日志内容。

## 6. aarch64 专属代码 cfg gate

### 6.1 read_core_id()

```rust
// panic/src/lib.rs
#[cfg(target_arch = "aarch64")]
pub fn read_core_id() -> u32 {
    let id: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, mpidr_el1",
            out(reg) id,
            options(nostack, preserves_flags),
        );
    }
    (id & 0xff) as u32
}

#[cfg(not(target_arch = "aarch64"))]
pub fn read_core_id() -> u32 { 0 }
```

aarch64 读 `MPIDR_EL1` 并取 Aff0；host 测试构建返回 0。

### 6.2 hard_reset()

```rust
// panic/src/lib.rs
#[cfg(target_arch = "aarch64")]
pub fn hard_reset() -> ! {
    unsafe {
        core::arch::asm!("b 0x0", options(noreturn));
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn hard_reset() -> ! {
    loop { spin_loop(); }
}
```

aarch64 跳转到复位向量 `0x0`；host 死循环以免测试返回。

## 7. 与现有 panic_handler 的关系

### 7.1 设计决策 D1

本 crate **不定义** `#[panic_handler]`。原因是 `#[panic_handler]` 是 lang item，
每个二进制只能有一个。如果 `eneros-panic` 自带，则 `kernel`、`hello` 等下游 crate
自己的 `#[panic_handler]` 会与之符号冲突，无法编译。

### 7.2 消费者委托模式

消费者在自己 crate 里定义 `#[panic_handler]`，内部委托给 `eneros_panic::handle_panic`：

```rust
// panic/src/lib.rs
pub fn handle_panic(_info: &core::panic::PanicInfo) -> ! {
    let ctx = PanicContext::new(PanicLevel::Kernel, "?", "?");
    let strategy = *STRATEGY.lock();
    match strategy {
        Some(s) => s.handle(&ctx),
        None => KernelResetStrategy.handle(&ctx),
    }
}
```

`location` / `message` 用 `"?"`，因为 `PanicInfo` 在不分配的情况下无法给出 `&'static str`。
需要结构化位置/消息的调用点应直接构造 `PanicContext::new(...)` 并调用已注册策略。

### 7.3 代码示例（未来 kernel/hello 迁移）

```rust
// kernel/src/panic.rs (未来)
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eneros_panic::handle_panic(info)
}
```

或携带更精确的位置/消息：

```rust
static KS: eneros_panic::KernelResetStrategy = eneros_panic::KernelResetStrategy;
eneros_panic::set_strategy(&KS);

let ctx = eneros_panic::PanicContext::new(
    eneros_panic::PanicLevel::Kernel,
    file!(),
    "init failed",
);
// 由 #[panic_handler] 内部调用策略
```

## 8. 全局 API

| API | 作用 |
|-----|------|
| `set_strategy(&'static dyn PanicStrategy + Sync)` | 注册全局 panic 策略 |
| `set_partition_strategy(id: u32)` | 注册预分配的 `PartitionIsolateStrategy`（0..8） |
| `set_reset_policy(ResetPolicy)` | 配置 `KernelResetStrategy` 的复位时机 |
| `handle_panic(&PanicInfo) -> !` | panic_handler 委托入口 |
| `logger::set_serial_sink(&'static dyn SerialSink + Sync)` | 注册串口 sink |

## 9. 测试策略

共 21 个单元测试，分布在三模块：

- `lib.rs`：7 个 — `PanicLevel` 相等性、`PanicContext::new`、`ResetPolicy`、`set_strategy`、host 下 `read_core_id()==0`。
- `logger.rs`：6 个 — `NullSink` 不 panic、kernel/partition 日志格式、`panic_log_raw`、未注册 sink no-op、`set_serial_sink`。
- `isolation.rs`：8 个 — 见分区隔离文档 §8。

host 构建下间接验证策略与日志路径（`hard_reset` 走死循环、串口走 `CaptureSink`）；
aarch64 真机下的硬件复位与 `MPIDR_EL1` 读取留待 QEMU 阶段验证。

## 10. 蓝图符合性

对照 `phase0.md §v0.14.0`：

| 蓝图条目 | 实现状态 |
|----------|----------|
| §4.1 `PanicLevel` / `PanicContext` 数据结构 | ✅ 完全一致 |
| §4.2 `PanicStrategy` trait / `KernelResetStrategy` / `PartitionIsolateStrategy` | ✅ 完全一致 |
| §4.3 流程图「日志落盘+串口 → flush → 复位」 | ✅ 内核路径已实现；分区路径标记 Dead 后死循环，资源回收与通知 SystemAgent 留待后续版本（见分区文档 §6） |
| §4.4 错误处理：分区隔离失败升级内核 panic | ✅ `PartitionIsolateStrategy::handle` 的 `Err(_)` 分支 |
| §5.4 难点：panic 时日志不能用 alloc | ✅ 栈上 `[u8; 256]` + `core::fmt::Write` |
| §5.5 交互：v0.18.0 调度器配合、v0.22.0 降级依赖 | ⏳ 接口已就位（`partition_state`），待下游版本 |
| §8.5 坑点：`#[panic_handler]` 只能有一个 | ✅ D1 决策，crate 不定义 lang item |
| §7.1 panic 后日志输出到串口 | ✅ `panic_log` + `SerialSink` |
| §7.3 内核 panic 触发硬件复位 | ✅ `hard_reset` aarch64 分支 |
