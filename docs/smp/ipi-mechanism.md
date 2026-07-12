# EnerOS 核间中断（IPI）机制设计

> 版本：v0.15.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.15.0`（多核启动与 IPI）、`Power_Native_Agent_OS_Blueprint.md §6.3`（IPI 延迟 < 5μs）、§43.1（no_std 合规）

## 1. 概述

EnerOS IPI（Inter-Processor Interrupt）机制为多核系统提供核间事件通知能力，
是 v0.15.0 多核框架的另一半交付物（多核启动另见 `docs/smp-boot-design.md`）。
本机制在 `eneros-smp` crate 的 `smp/src/ipi.rs` 与 `smp/src/channel.rs` 中实现。

v0.15.0 IPI 机制的目标与范围：

- **基于 GICv3 SGI**：通过系统寄存器 `icc_sgi1r_el1` 触发 Software Generated Interrupt。
- **per-core 邮箱通道**：每个核拥有独立的有界邮箱，缓存待处理 IPI 消息。
- **handler 注册与分发**：按消息类型（`msg_type`）注册函数指针，接收侧从邮箱
  批量取出消息并派发给已注册 handler。
- **死锁预防**：分发时先把 handler 表拷贝出锁外，再调用 handler，避免 handler
  内部再次 `ipi_send` 导致锁重入死锁。

本版本**不**包含的能力（明确标注为「未来扩展」，见 §11）：

- IPI 延迟真机测量（蓝图 §6.3 要求 < 5μs，host 无法测，留待 QEMU 阶段）
- `Reschedule` / `TlbShootdown` 在调度器与 MMU 中的真正使用
- `IpiMsg::Shutdown` 触发的全核协调关机流程

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1；
`Cargo.toml` 仅依赖 `spin` 与 `heapless`。

## 2. GICv3 SGI 机制

### 2.1 SGI 概述

SGI（Software Generated Interrupt）是 GICv3 提供的核间中断机制，编号 0–15 共 16 个。
任意核可通过写系统寄存器 `icc_sgi1r_el1` 向一个或多个目标核触发 SGI，目标核在
其 IRQ 入口收到对应中断后处理。

| 属性 | 值 |
|------|-----|
| SGI 编号范围 | 0 – 15 |
| 本 crate 使用编号 | 0（`SGI_IRQ_NUM = 0`） |
| 触发方式 | 写 `icc_sgi1r_el1` 系统寄存器 |
| 中断类型 | 边沿触发（GICv3 默认） |
| 目标选择 | Target List（位图）或 IRM 广播 |

本 crate 固定使用 SGI 0 作为 IPI 通道：所有 IPI 消息共用同一个 SGI，消息**类型**
通过 per-core 邮箱中的 `IpiMsg` 区分，而非分配多个 SGI 编号。这样设计的好处是：

- 单一 IRQ handler 入口，简化 GIC 配置。
- 16 个 SGI 编号留给未来其它用途（如性能采样中断、debug IPI）。
- 消息分多类复用同一条 IRQ，扩展性由 `IpiMsg` 枚举承担。

### 2.2 SGI_IRQ_NUM

```rust
// smp/src/ipi.rs
pub const SGI_IRQ_NUM: u32 = 0;
```

`SGI_IRQ_NUM` 通过 `lib.rs` re-export，供 kernel 顶层 IRQ 路由表注册 GICv3 IRQ handler
时引用。

## 3. icc_sgi1r_el1 寄存器格式

`ICC_SGI1R_EL1` 是 64-bit 系统寄存器，写入即触发一次 SGI。其字段布局如下
（仅列出与本 crate 相关的字段，完整字段见 ARM ARM D19.3）：

| 位段 | 字段 | 含义 |
|------|------|------|
| [3:0] | SGI ID | SGI 编号（0–15） |
| [15:8] | Target Aff0 | 目标 CPU 的 Aff0 字段（用于 Cluster 路由） |
| [40:16] | Target List | 最多 16 个 CPU 的位图（每 bit 对应一个 Aff0 CPU） |
| [41] | IRM | Interrupt Routing Mode：0=指定 target，1=广播除自己外所有核 |
| [47:44] | RS | Range Selector，选择 16 位 Target List 对应的 Aff0 子范围 |
| [55:48] | Target Aff3 | 目标 Affinity 3 字段 |
| [63:56] | Reserved | 保留 |

### 3.1 本 crate 实际编码

```rust
// smp/src/ipi.rs (aarch64)
fn send_sgi(target: u32, sgi_num: u32) {
    let val: u64 = ((target as u64) << 16) | (sgi_num as u64 & 0xf);
    unsafe {
        core::arch::asm!(
            "msr icc_sgi1r_el1, {}",
            in(reg) val,
            options(nostack, preserves_flags),
        );
    }
}
```

实际写入值为 `target << 16 | sgi_num`：

- **`sgi_num & 0xf`** 写入位 [3:0]（SGI ID 字段）。
- **`target << 16`** 把 `target` 当作 Target List 中的 bit 位置：bit `16 + target` 置 1，
  即只选中 Target List 中第 `target` 个 CPU（Aff0 = target）。
- **IRM=0**：指定目标，不广播。
- 其它字段（Aff3、RS）保持 0，适用于 QEMU virt 单簇 8 核场景。

### 3.2 单簇假设

QEMU virt 默认配置为单簇（Aff2/Aff3 均为 0），因此 `target << 16` 的简化编码可用。
真机若跨簇（Aff2 不同），需扩展为：写 Aff3 字段 + 选择 RS 子范围 + 计算 Target List
位图。此扩展留待真机移植阶段。

## 4. IPI 消息类型

```rust
// smp/src/ipi.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiMsg {
    /// 请求目标核重新调度。
    Reschedule,
    /// 请求目标核关机。
    Shutdown,
    /// 请求 TLB 失效，参数为需要失效的地址范围标识。
    TlbShootdown(u64),
    /// 用户自定义消息，携带 32-bit 参数。
    Custom(u32),
}
```

| 变体 | 参数 | 用途 | 实现状态 |
|------|------|------|----------|
| `Reschedule` | 无 | 触发调度器重新调度 | ⏳ v0.16.0 调度器接入 |
| `Shutdown` | 无 | 系统关机协调 | ⏳ 未来关机流程 |
| `TlbShootdown(u64)` | 地址范围标识 | MMU 页表更新后失效其它核 TLB | ⏳ v0.16.0+ MMU 接入 |
| `Custom(u32)` | 32-bit 用户参数 | 自定义消息（最多 13 种） | ✅ 通用通道可用 |

`Custom` 的参数仅 `u32`，足以携带小枚举或短 token；更大的 payload 应通过共享内存传递，
IPI 仅作通知。

### 4.1 msg_type 索引

```rust
// smp/src/ipi.rs
impl IpiMsg {
    pub fn msg_type(&self) -> u32 {
        match self {
            IpiMsg::Reschedule          => 0,
            IpiMsg::Shutdown            => 1,
            IpiMsg::TlbShootdown(_)     => 2,
            IpiMsg::Custom(t)           => 3 + (*t % 13),   // 3..=15
        }
    }
}
```

`msg_type` 是 handler 表的索引（见 §5）。设计要点：

- 固定变体（`Reschedule` / `Shutdown` / `TlbShootdown`）占用索引 0/1/2。
- `Custom(t)` 通过 `t % 13` 映射到 3..=15，共 13 个槽位。
- `Custom(0)` 与 `Custom(13)` 都映射到索引 3（`13 % 13 == 0`），调用方需自行区分
  payload 内容，或避免在同类型下复用 0 与 13。

| msg_type | 占用者 |
|----------|--------|
| 0 | `Reschedule` |
| 1 | `Shutdown` |
| 2 | `TlbShootdown(_)` |
| 3..=15 | `Custom(t)`（其中 `t % 13 == msg_type - 3`） |

## 5. handler 注册与分发

### 5.1 IPI_HANDLERS 静态表

```rust
// smp/src/ipi.rs
const MAX_IPI_TYPES: usize = 16;

type Handler = Option<fn(IpiMsg)>;
type HandlerTable = [Handler; MAX_IPI_TYPES];

static IPI_HANDLERS: Mutex<HandlerTable> = Mutex::new({
    const NONE: Handler = None;
    [NONE; MAX_IPI_TYPES]
});
```

- 表长 16，与 `msg_type` 取值范围 0..=15 严格对齐。
- 元素类型 `Option<fn(IpiMsg)>` 是函数指针（非闭包、非 trait 对象），无堆分配。
- `Mutex<HandlerTable>` 整表加锁，注册与拷贝均一次性完成。

### 5.2 register_ipi_handler

```rust
// smp/src/ipi.rs
pub fn register_ipi_handler(msg_type: u32, handler: fn(IpiMsg)) {
    if msg_type as usize >= MAX_IPI_TYPES {
        return;   // 越界静默忽略
    }
    IPI_HANDLERS.lock()[msg_type as usize] = Some(handler);
}
```

- `msg_type >= 16` 静默忽略（按蓝图惯例，不 panic）。
- 注册是覆盖式：同一 `msg_type` 重复注册会替换前一个 handler。
- handler 必须是 `fn(IpiMsg)`（非闭包），保证可 `Copy` 与 `const` 上下文构造。

### 5.3 ipi_dispatch

```rust
// smp/src/ipi.rs
pub fn ipi_dispatch() {
    let msgs = channel::mailbox_drain(read_core_id());
    // 先拷贝 handler 表出锁外，避免 handler 内发 IPI 导致重入死锁
    let handlers: HandlerTable = *IPI_HANDLERS.lock();
    for msg in msgs.as_slice() {
        let idx = msg.msg_type() as usize;
        if idx < MAX_IPI_TYPES {
            if let Some(handler) = handlers[idx] {
                handler(*msg);
            }
        }
    }
}
```

执行步骤：

1. `mailbox_drain(read_core_id())` 一次性取走本核邮箱所有消息（FIFO 序）。
2. **拷贝** handler 表出锁外（`*IPI_HANDLERS.lock()`），立即释放锁。
3. 遍历消息，按 `msg_type()` 索引找到 handler 并调用。
4. 未注册 handler 的消息静默丢弃（蓝图惯例）。

### 5.4 死锁预防

**关键设计**：调用 handler **之前**释放 `IPI_HANDLERS` 锁。

考虑场景：核 A 的 handler 内部调用 `ipi_send(B, ...)`。若不先释放锁：

1. 核 A 持有 `IPI_HANDLERS` 锁 → 调用 handler → handler 内 `ipi_send` →
   `ipi_send` 内 `mailbox_push`（不同锁 `MAILBOXES`，不死锁） → `send_sgi` 触发 SGI。
2. 此时若 handler 又再次 `ipi_dispatch`（罕见但可能），会再次 `IPI_HANDLERS.lock()`，
   因 `spin::Mutex` 非可重入，**立即死锁**。

通过先拷贝表出锁外，handler 内部的任何 IPI 操作都不再阻塞 `IPI_HANDLERS` 锁。

代价：拷贝 16 个函数指针（128 字节）到栈上，开销可忽略。

## 6. 邮箱通道设计

### 6.1 数据结构

```rust
// smp/src/channel.rs
pub const MAILBOX_CAPACITY: usize = 16;
const MAX_CORES: usize = 8;

type Mailbox = heapless::Vec<IpiMsg, MAILBOX_CAPACITY>;

static MAILBOXES: Mutex<[Mailbox; MAX_CORES]> = Mutex::new([
    heapless::Vec::new(),
    heapless::Vec::new(),
    /* × 8 个 */
    heapless::Vec::new(),
]);
```

- 每核独立邮箱，容量 16 条 `IpiMsg`。
- `heapless::Vec<IpiMsg, 16>` 在编译期固定容量，无堆分配，满足 no_std。
- 整张 `MAILBOXES` 表用单把 `Mutex` 保护，所有核共享。

### 6.2 实现说明：为何不用 spsc::Queue

原计划使用 `heapless::spsc::Queue`（无锁单生产单消费队列，更适合 IPI 场景），但
`spin::Mutex<heapless::spsc::Queue<...>>` **不实现 `Copy`** trait，因此无法用
`[expr; N]` 数组初始化语法生成 `[Mutex<Queue>; 8]`（Rust 的 `[expr; N]` 要求
`expr: Copy`）。

权衡后改用 `heapless::Vec<IpiMsg, 16>`：

| 方案 | Copy | 初始化 | 锁开销 |
|------|------|--------|--------|
| `Mutex<spsc::Queue>` | ❌ | 需手写 8 个 `Mutex::new(...)` | 同 |
| `Mutex<Vec<IpiMsg, 16>>` | ❌ | 需手写 8 个 | 同 |
| 直接 `[Mutex<Vec>; 8]` | ❌（Mutex 不 Copy） | **必须手列** 8 个元素 | 同 |

最终方案是**手动列出 8 个 `heapless::Vec::new()` 初始化**`MAILBOXES`，代码冗长但
正确无歧义。未来若 `const` 上下文支持 `Mutex::new` 内联初始化更易写，可重构。

### 6.3 邮箱操作 API

```rust
// smp/src/channel.rs
pub fn mailbox_push(core_id: u32, msg: IpiMsg) -> Result<(), IpiMsg>;
pub fn mailbox_pop(core_id: u32) -> Option<IpiMsg>;
pub fn mailbox_drain(core_id: u32) -> Mailbox;
pub fn mailbox_clear(core_id: u32);
```

| API | 行为 | 失败处理 |
|-----|------|----------|
| `mailbox_push` | 写入目标核邮箱末尾 | 越界或满 → `Err(msg)` |
| `mailbox_pop` | 取出邮箱首条消息 | 越界或空 → `None` |
| `mailbox_drain` | 取出全部消息并清空邮箱 | 越界 → 空 `Vec` |
| `mailbox_clear` | 清空邮箱 | 越界静默忽略 |

**注意**：`mailbox_pop` 内部用 `swap_remove(0)` 删除首元素，是 O(1) 但**不保持
原顺序**（被删位置由末尾元素回填）。`mailbox_drain` 通过遍历 `as_slice()` 再
`clear()` 实现，**保持 FIFO 顺序**。IPI 派发路径使用 `mailbox_drain`，因此实际
handler 调用顺序与 push 顺序一致。

### 6.4 容量与溢出策略

`MAILBOX_CAPACITY = 16`。溢出时 `mailbox_push` 返回 `Err(msg)`，调用方
（`ipi_send`）**静默丢弃**消息：

```rust
// smp/src/ipi.rs
pub fn ipi_send(target: u32, msg: IpiMsg) {
    let _ = channel::mailbox_push(target, msg);   // 溢出丢弃
    send_sgi(target, SGI_IRQ_NUM);
}
```

设计权衡：

- 容量 16 足以吸收典型 IPI 突发（`Reschedule` 短时间内多次重发可合并）。
- 溢出时**不重试**，避免发送侧阻塞。
- 若 handler 需要可靠投递，应在应用层使用更高层协议（未来 Control Bus）。

## 7. ipi_send 流程

### 7.1 步骤分解

```rust
// smp/src/ipi.rs
pub fn ipi_send(target: u32, msg: IpiMsg) {
    let _ = channel::mailbox_push(target, msg);   // 步骤 1
    send_sgi(target, SGI_IRQ_NUM);                // 步骤 2
}
```

| 步骤 | 操作 | 文件 |
|------|------|------|
| 1 | `mailbox_push(target, msg)`：把消息写入**目标核**邮箱 | `smp/src/channel.rs` |
| 2 | `send_sgi(target, 0)`：向目标核触发 SGI 0 | `smp/src/ipi.rs` |

### 7.2 接收侧流程

目标核收到 SGI 0 → 进入 IRQ handler → 调用 `ipi_dispatch()`：

1. `mailbox_drain(read_core_id())` 一次性取走本核邮箱所有消息。
2. 拷贝 `IPI_HANDLERS` 表出锁外。
3. 遍历消息按 `msg_type` 派发到 handler。

### 7.3 时序图

```
核 A (发送)                  核 B (接收)
   │                              │
   │ mailbox_push(B, msg)         │
   │─────────────────────────────►│ 邮箱写入
   │                              │
   │ send_sgi(B, 0)               │
   │  msr icc_sgi1r_el1           │
   │─────────────────────────────►│ SGI 0 中断
   │                              │
   │                              │ IRQ handler
   │                              │ → ipi_dispatch()
   │                              │   → mailbox_drain(B)
   │                              │   → handler(msg)
   │                              │
```

### 7.4 消息丢失场景

以下情况会导致消息丢失（设计上接受）：

1. **邮箱溢出**：16 条已满，后续 `mailbox_push` 返回 `Err`，`ipi_send` 静默丢弃。
2. **handler 未注册**：消息取出但 `handlers[idx] == None`，静默丢弃。
3. **SGI 丢失**：GICv3 SGI 是边沿触发，理论上不会丢；但若目标核 IRQ 屏蔽期间
   多次触发相同 SGI，会合并为一次。因此**handler 必须从邮箱 drain 全部消息**，
   而非只处理一条。

## 8. ipi_broadcast 流程

```rust
// smp/src/ipi.rs
pub fn ipi_broadcast(msg: IpiMsg) {
    let self_id = read_core_id();
    let count = core_count();
    for i in 0..count {
        if i != self_id {
            ipi_send(i, msg);
        }
    }
}
```

- 遍历 `0..core_count()` 调用 `ipi_send`。
- **跳过自身核**（`i != self_id`），避免自中断：
  - 自中断会立即在当前核触发 SGI 0，打断当前执行流。
  - 若需要自我通知，调用方应显式调用 `ipi_send(read_core_id(), msg)` 或直接调用
    handler。

### 8.1 广播的开销

- 时间复杂度 `O(core_count)`：8 核下 7 次 `mailbox_push` + 7 次 `send_sgi`。
- 锁竞争：每次 `mailbox_push` 抢 `MAILBOXES` 锁，串行化执行。
- 真广播（IRM=1）的优化留待未来：通过设置 `icc_sgi1r_el1` 的 IRM 位一次性
  广播除自己外所有核，但 QEMU virt 对 IRM 的支持需验证。

## 9. aarch64 cfg gate 策略

### 9.1 send_sgi

```rust
// smp/src/ipi.rs (aarch64)
#[cfg(target_arch = "aarch64")]
fn send_sgi(target: u32, sgi_num: u32) {
    let val: u64 = ((target as u64) << 16) | (sgi_num as u64 & 0xf);
    unsafe {
        core::arch::asm!(
            "msr icc_sgi1r_el1, {}",
            in(reg) val,
            options(nostack, preserves_flags),
        );
    }
}

// smp/src/ipi.rs (host)
#[cfg(not(target_arch = "aarch64"))]
fn send_sgi(_target: u32, _sgi_num: u32) {}
```

host 路径 `send_sgi` 是完全 no-op，但 `ipi_send` 中的 `mailbox_push` 仍执行，
因此 host 测试可通过 `mailbox_pop` / `mailbox_drain` 直接验证消息流转。

### 9.2 cfg gate 一览

| 函数 | aarch64 行为 | host 行为 |
|------|--------------|-----------|
| `send_sgi(target, sgi_num)` | `msr icc_sgi1r_el1` | no-op |
| `ipi_send(target, msg)` | `mailbox_push` + `send_sgi` | 仅 `mailbox_push` |
| `ipi_broadcast(msg)` | 循环 `ipi_send` | 同（host `read_core_id()==0`） |
| `ipi_dispatch()` | 邮箱 drain + handler 派发 | 同 |
| `register_ipi_handler` | 写 `IPI_HANDLERS` | 同 |

`ipi_dispatch` / `register_ipi_handler` 不需要 cfg gate，因为它们不触碰 aarch64
专属指令，纯 Rust 逻辑。

### 9.3 host 测试策略

host 上无法触发真实 SGI，但可通过直接调用 `ipi_dispatch` 模拟接收路径：

```rust
// smp/src/ipi.rs (test 示意)
mailbox_clear(0);
ipi_send(0, IpiMsg::Reschedule);   // 写入邮箱（host 不发 SGI）
ipi_dispatch();                     // 手动派发
```

这覆盖了 mailbox → drain → handler 路径，仅 `send_sgi` 的硬件行为留待真机验证。

## 10. 性能考量

### 10.1 蓝图要求

蓝图 §6.3 要求 IPI 延迟 < 5μs（从 `ipi_send` 到目标核 handler 入口的端到端时间）。

### 10.2 影响因素

| 因素 | 影响 | 缓解 |
|------|------|------|
| `mailbox_push` 锁竞争 | 多核同时发 IPI 时串行化 | 容量 16 足够，临界区极短 |
| `msr icc_sgi1r_el1` 路径 | GICv3 SGI 路由延迟 | 硬件固有，无法软件优化 |
| IRQ 屏蔽时间 | 目标核 IRQ 屏蔽期间 SGI 挂起 | 关键路径尽快开 IRQ |
| handler 执行时间 | handler 同步执行会延长下一次派发 | handler 应短小，重活延后 |
| `mailbox_drain` 拷贝 | 16 条消息拷贝到栈 | `IpiMsg` 是 `Copy`，开销小 |

### 10.3 host 无法测量

host 构建下 `send_sgi` 是 no-op，无法测真实 IPI 延迟。真机测量留待 §11.3。

### 10.4 优化空间

- **IRM 广播**：`ipi_broadcast` 可改为单次 `msr` 设置 IRM=1，消除 7 次 `send_sgi`。
- **per-core 锁**：把 `MAILBOXES` 拆为 8 个独立 `Mutex`，减少跨核锁竞争。
- **无锁 spsc 队列**：若 `const` 初始化问题解决，改用 `heapless::spsc::Queue`
  可消除发送侧锁。

以上优化均非 v0.15.0 范围，列为未来工作。

## 11. 未来扩展

### 11.1 v0.16.0：多核调度使用 Reschedule IPI

- 调度器在唤醒新线程时，若目标线程绑定的核不是当前核，调用
  `ipi_send(target_core, IpiMsg::Reschedule)`。
- 目标核 handler 触发调度器 `yield`，切换到新线程。
- 这是 IPI 机制最核心的使用场景。

### 11.2 v0.16.0+：TLB Shootdown 在 MMU 虚拟化中使用

- 内核修改页表后，需失效其它核 TLB 中对应条目。
- 调用 `ipi_broadcast(IpiMsg::TlbShootdown(addr_range_id))`。
- 目标核 handler 执行 `tlbi vaae1is, <xaddr>` 失效 TLB。
- `TlbShootdown` 的 `u64` 参数设计为地址范围标识（如 ASID + 页范围 token），
  具体编码由 MMU 子系统定义。

### 11.3 性能测量：QEMU 启动后用 ARM Generic Timer 测 IPI 往返延迟

- 利用 `cntvct_el0`（虚拟计数器）或 `cntpct_el0`（物理计数器）读取时间戳。
- 测量方法：核 A `ipi_send(B, Ping)`，B 的 handler 内 `ipi_send(A, Pong)`，
  A 测量往返时间。
- 目标：QEMU virt 下 IPI 往返 < 10μs（QEMU 模拟有额外开销，真机应更优）。
- 真机验证蓝图 §6.3 的 < 5μs 单向延迟要求。

### 11.4 Shutdown 协调关机

- `ipi_broadcast(IpiMsg::Shutdown)` 通知所有核进入关机流程。
- 各核 handler 设置本地「shutdown pending」标志，调度器检测后停止调度。
- 最后由主核调用 PSCI `SYSTEM_RESET` 或 `SYSTEM_OFF` 完成关机。

### 11.5 IPI 多优先级

未来若需要区分 IPI 优先级（如 `TlbShootdown` 高于 `Reschedule`），可分配额外
SGI 编号（1–15），每个 SGI 对应一个优先级层。当前 v0.15.0 仅用 SGI 0，
所有消息同优先级。

## 12. 全局 API

| API | 作用 | 文件位置 |
|-----|------|----------|
| `register_ipi_handler(msg_type, handler)` | 注册 handler（`msg_type >= 16` 忽略） | `smp/src/ipi.rs` |
| `ipi_send(target, msg)` | 向 `target` 发送 IPI | `smp/src/ipi.rs` |
| `ipi_broadcast(msg)` | 向除自身外所有核广播 IPI | `smp/src/ipi.rs` |
| `ipi_dispatch()` | 派发本核邮箱所有消息 | `smp/src/ipi.rs` |
| `mailbox_push(core_id, msg)` | 写入目标核邮箱 | `smp/src/channel.rs` |
| `mailbox_pop(core_id)` | 取出目标核邮箱首条 | `smp/src/channel.rs` |
| `mailbox_drain(core_id)` | 取出目标核全部消息 | `smp/src/channel.rs` |
| `mailbox_clear(core_id)` | 清空目标核邮箱 | `smp/src/channel.rs` |
| `SGI_IRQ_NUM` | IPI 使用的 SGI 编号（=0） | `smp/src/ipi.rs` |
| `MAILBOX_CAPACITY` | 单核邮箱容量（=16） | `smp/src/channel.rs` |
| `IpiMsg` | IPI 消息枚举 | `smp/src/ipi.rs` |

`lib.rs` 通过 `pub use` 把 `ipi_send` / `ipi_broadcast` / `ipi_dispatch` /
`register_ipi_handler` / `IpiMsg` / `SGI_IRQ_NUM` 与邮箱 API re-export 到 crate 根。

## 13. 测试覆盖

### 13.1 ipi.rs 测试（5 个）

| 测试 | 验证点 |
|------|--------|
| `test_ipi_msg_variants` | 4 个 `IpiMsg` 变体构造与相等性 |
| `test_ipi_msg_msg_type` | `Reschedule=0 / Shutdown=1 / TlbShootdown=2 / Custom(t)=3+(t%13)`；`Custom(13)` 回卷到 3 |
| `test_register_ipi_handler` | 注册后 `IPI_HANDLERS[0].is_some()`；可清空还原 |
| `test_register_ipi_handler_ignored` | `msg_type=16/100` 静默忽略，全表保持 `None` |
| `test_ipi_dispatch_empty_mailbox_no_panic` | 空邮箱下 `ipi_dispatch` 不 panic |

### 13.2 channel.rs 测试（7 个）

| 测试 | 验证点 |
|------|--------|
| `test_mailbox_push_pop` | push 后 pop 取出同一消息；再 pop 返回 `None` |
| `test_mailbox_push_full_returns_err` | 满 16 条后第 17 条返回 `Err(msg)` |
| `test_mailbox_pop_empty_returns_none` | 空邮箱 pop 返回 `None` |
| `test_mailbox_drain` | drain 取出全部 2 条 FIFO 序；邮箱变空 |
| `test_mailbox_clear` | clear 后 pop 返回 `None` |
| `test_mailbox_invalid_core_id` | `core_id=8` push/pop/drain/clear 均安全 |
| `test_mailbox_cross_core` | core 0 push 到 core 1 邮箱，core 1 pop 取出 |

测试用 `std::sync::Mutex` 串行化以避免共享 `MAILBOXES` 数据竞争。

## 14. 蓝图符合性

对照 `phase0.md §v0.15.0`：

| 蓝图条目 | 实现状态 |
|----------|----------|
| IPI 机制：GICv3 SGI | ✅ `send_sgi` 通过 `icc_sgi1r_el1` |
| SGI 编号 0 | ✅ `SGI_IRQ_NUM = 0` |
| per-core 邮箱 | ✅ `MAILBOXES: Mutex<[Vec<IpiMsg,16>; 8]>` |
| handler 注册与分发 | ✅ `register_ipi_handler` + `ipi_dispatch` |
| 死锁预防：handler 拷贝出锁外 | ✅ `ipi_dispatch` 先 `*IPI_HANDLERS.lock()` 拷贝再调用 |
| `Reschedule` / `Shutdown` / `TlbShootdown` / `Custom` | ✅ `IpiMsg` 4 变体 |
| 广播跳过自身 | ✅ `ipi_broadcast` 中 `if i != self_id` |
| no_std 合规（蓝图 §43.1） | ✅ `#![cfg_attr(not(test), no_std)]`，仅依赖 `spin` / `heapless` |
| IPI 延迟 < 5μs（蓝图 §6.3） | ⏳ host 无法测，留待 QEMU 阶段（见 §11.3） |
| `Reschedule` 实际使用 | ⏳ v0.16.0 调度器接入（见 §11.1） |
| `TlbShootdown` 实际使用 | ⏳ v0.16.0+ MMU 接入（见 §11.2） |
| IRM 真广播优化 | ⏳ v0.15.0 用循环 `ipi_send`，留待 §10.4 |
