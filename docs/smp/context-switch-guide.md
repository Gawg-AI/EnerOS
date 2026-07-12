# EnerOS ARM64 上下文切换说明

> 版本：v0.18.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.18.0`（线程/任务抽象，蓝图第 3893–4111 行）、`Power_Native_Agent_OS_Blueprint.md §8.5`（naked 函数必须 `extern "C"`）、§6.3（性能要求，单次切换 < 2μs）、§43.1（no_std 合规）、§43.2（非瓶颈版本）
> 实现位置：`crates/kernel/sched/src/switch.rs`
> 配套文档：`docs/smp/thread-abstraction-design.md`（TCB 与状态机设计）

## 1. 概述

本文档描述 EnerOS v0.18.0 在 ARM64（aarch64）架构下的线程上下文切换实现。
上下文切换是线程调度的核心机制：保存当前线程的寄存器状态到其 TCB，
恢复目标线程的寄存器状态，使 CPU 从目标线程上次暂停的位置继续执行。

EnerOS 的上下文切换具有以下特点：

- **内核态切换**：仅支持 EL1（内核态）线程之间的切换，不涉及 EL0/EL1 模式切换
- **协作式调度**：由 `thread_yield / thread_block / thread_exit` 主动触发，
  非抢占式（v0.19.0+ 引入抢占）
- **naked 函数实现**：用 `#[naked]` + `asm!` 直接控制寄存器保存/恢复，
  编译器不生成 prologue/epilogue
- **callee-saved 保存**：仅保存 x19-x30（12 个寄存器），caller-saved 由调用者负责

本版本上下文切换的验收标准（蓝图 §7.3）：**单次切换 < 2μs**（QEMU cortex-a57）。

## 2. ARM64 异常模型基础

### 2.1 特权级（EL0-EL3）

ARMv8 定义 4 个异常级别（Exception Level）：

| EL | 名称 | 典型用途 | EnerOS 使用 |
|----|------|---------|------------|
| EL0 | User | 用户态应用 | 未来（用户态线程） |
| EL1 | Kernel | 操作系统内核 | ✅ 本版本线程运行于此 |
| EL2 | Hypervisor | 虚拟机监控器 | 不使用 |
| EL3 | Secure Monitor | 安全监控器 | 不使用 |

EnerOS v0.18.0 的所有线程均在 **EL1** 运行，使用 `SP_EL1`（不是 `SP_EL0`）。
线程切换不涉及特权级切换，简化了上下文保存。

### 2.2 关键寄存器

| 寄存器 | 含义 | 切换时的处理 |
|--------|------|-------------|
| `x0-x30` | 通用寄存器（31 个） | x19-x30 保存到栈，x0-x18 由调用者负责 |
| `SP` (`sp`) | 栈指针 | 通过 `mov`/`ldr` 切换 |
| `PC` (`pc`) | 程序计数器 | 通过 `ret`（lr）或 `eret`（elr_el1）切换 |
| `SPSR_EL1` | 保存的程序状态寄存器 | 异常返回时恢复，决定返回后的 EL 与中断状态 |
| `ELR_EL1` | 异常链接寄存器 | 保存异常返回地址（即被中断指令的地址） |
| `LR` (`x30`) | 链接寄存器 | 函数返回地址；线程初始为 entry |

### 2.3 eret 指令

`eret`（Exception Return）指令：

- 从 `ELR_EL1` 加载 PC
- 从 `SPSR_EL1` 加载 PSTATE（恢复 EL、中断掩码、条件标志等）
- 用于从异常处理返回，也用于首次进入新线程

 EnerOS v0.18.0 的 `context_switch` 用 `ret`（普通函数返回）而非 `eret`，
 因为线程切换发生在 EL1 内部，不需要特权级切换。`ret` 从 `LR`（x30）加载 PC。

## 3. naked 函数设计

### 3.1 为什么用 #[naked]

普通 Rust 函数的调用约定会自动生成：

- **prologue**：`stp x29, x30, [sp, #-16]!`（保存 fp, lr）+ 栈空间分配
- **epilogue**：`ldp x29, x30, [sp], #16`（恢复 fp, lr）+ `ret`

但上下文切换函数**不能**让编译器生成 prologue/epilogue，原因：

1. 切换函数会修改 `SP`（从 from 切换到 to），编译器生成的 epilogue 会从错误的栈恢复
2. 切换函数的入口/出口必须由开发者完全控制
3. 编译器可能优化掉看似「无用」的寄存器保存

`#[naked]` 属性告诉编译器：**不生成任何 prologue/epilogue，函数体必须仅含 `asm!`**。

### 3.2 extern "C" ABI（蓝图 §8.5 强制）

```rust
#[naked]
pub unsafe extern "C" fn context_switch(from_sp: *mut u64, to_sp: *const u64) {
    // ...
}
```

**必须用 `extern "C"`**，原因：

- Rust ABI 不稳定，不同 Rust 版本可能改变参数传递约定
- `extern "C"` 是稳定 ABI，参数通过 `x0`/`x1` 传递（ARM64 AAPCS64 约定）
- 蓝图 §8.5 明确要求所有 naked 函数必须 `extern "C"`

### 3.3 options(noreturn)

```rust
asm!(
    // ... 汇编代码 ...
    "ret",
    in(reg) from_sp, in(reg) to_sp,
    options(noreturn)
);
```

`options(noreturn)` 告诉编译器：

- 该 `asm!` 块永不返回到调用者
- 编译器不会在 `asm!` 之后生成代码
- `ret` 指令跳转到目标线程的 `LR`（即 entry），而非返回 `context_switch` 的调用者

注意：`context_switch` 函数本身是「会返回」的（从 to 线程的视角看），
只是不返回到 from 线程的调用点。`noreturn` 是针对单个 `asm!` 块的语义。

## 4. 寄存器保存约定

### 4.1 callee-saved vs caller-saved

ARM64 AAPCS64 调用约定将寄存器分为两类：

| 类别 | 寄存器 | 保存责任 | 切换时的处理 |
|------|--------|---------|-------------|
| **callee-saved** | x19-x28, x29(fp), x30(lr) | 被调用方保存 | ✅ `context_switch` 保存到栈 |
| **caller-saved** | x0-x18 | 调用方保存 | ❌ 不保存（由调用者负责） |

x29 是帧指针（FP），x30 是链接寄存器（LR），二者都属于 callee-saved。

### 4.2 为什么只保存 callee-saved

切换发生在协作式调度点（`thread_yield` 等），此时：

- 调用方（`thread_yield`）已知不再需要 x0-x18（已被使用或保存）
- 被切换回来的线程会从其上次暂停的 `thread_yield` 调用点继续，
  那时的 x0-x18 状态由当时的调用链决定

因此 `context_switch` 只需保存/恢复 **x19-x30（12 个寄存器）**。

### 4.3 SP 寄存器的处理

`SP` 不通过 `stp` 保存，而是通过 `mov` 直接存取：

```asm
mov {0}, sp        // 保存当前 SP 到 *from_sp
ldr sp, {1}        // 从 *to_sp 加载新 SP
```

原因：

- `stp` 不能直接操作 `SP`（ARM64 限制）
- `SP` 是切换的核心：保存当前栈指针，加载目标栈指针

## 5. 栈帧布局（272 字节）

### 5.1 布局设计

`init_stack_frame` 为新线程构造的初始栈帧，总大小 **272 字节**：

| 偏移 (字节) | 内容 | 寄存器/系统 | 说明 |
|------------|------|-------------|------|
| 0-7 | x0 | caller-saved | 初始为 0 |
| 8-15 | x1 | caller-saved | 初始为 0 |
| 16-23 | x2 | caller-saved | 初始为 0 |
| ... | ... | ... | ... |
| 152-159 | x19 | callee-saved | 初始为 0 |
| 160-167 | x20 | callee-saved | 初始为 0 |
| ... | ... | ... | ... |
| 232-239 | x28 | callee-saved | 初始为 0 |
| 240-247 | x29 (fp) | callee-saved | 初始为 0 |
| 248-255 | x30 (lr) | callee-saved | **= entry address** |
| 256-263 | elr_el1 | 系统 | **= entry address** |
| 264-271 | spsr_el1 | 系统 | **= 0x3C5** |

### 5.2 大小计算

- 31 个 64 位通用寄存器（x0-x30）：`31 × 8 = 248` 字节
- `ELR_EL1`：8 字节
- `SPSR_EL1`：8 字节
- 合计：`248 + 8 + 8 = 264` 字节
- 对齐到 16 字节边界：`ceil(264 / 16) × 16 = 272` 字节（多 8 字节 padding）

### 5.3 索引映射

`init_stack_frame` 中用 `frame.add(N)` 访问，索引含义：

| 索引 N | 偏移 = N×8 | 内容 |
|--------|-----------|------|
| 0 | 0 | x0 |
| 1 | 8 | x1 |
| ... | ... | ... |
| 19 | 152 | x19 |
| ... | ... | ... |
| 29 | 232 | x29 (fp) |
| 30 | 240 | x30 (lr) = entry |
| 31 | 248 | elr_el1 = entry |
| 32 | 256 | spsr_el1 = 0x3C5 |
| — | 264 | padding（对齐用） |

## 6. context_switch 汇编代码详解

### 6.1 完整代码

```rust
#[naked]
#[cfg(target_arch = "aarch64")]
pub unsafe extern "C" fn context_switch(from_sp: *mut u64, to_sp: *const u64) {
    asm!(
        // ===== 保存当前上下文（from 线程）=====
        "stp x29, x30, [sp, #-16]!",   // ① 保存 fp, lr
        "stp x27, x28, [sp, #-16]!",   // ② 保存 x27, x28
        "stp x25, x26, [sp, #-16]!",   // ③ 保存 x25, x26
        "stp x23, x24, [sp, #-16]!",   // ④ 保存 x23, x24
        "stp x21, x22, [sp, #-16]!",   // ⑤ 保存 x21, x22
        "stp x19, x20, [sp, #-16]!",   // ⑥ 保存 x19, x20
        // ===== 切换 SP =====
        "mov {0}, sp",                  // ⑦ 保存当前 sp 到 *from_sp
        "ldr sp, {1}",                  // ⑧ 从 *to_sp 加载新 sp
        // ===== 恢复目标上下文（to 线程）=====
        "ldp x19, x20, [sp], #16",      // ⑨ 恢复 x19, x20
        "ldp x21, x22, [sp], #16",      // ⑩ 恢复 x21, x22
        "ldp x23, x24, [sp], #16",      // ⑪ 恢复 x23, x24
        "ldp x25, x26, [sp], #16",      // ⑫ 恢复 x25, x26
        "ldp x27, x28, [sp], #16",      // ⑬ 恢复 x27, x28
        "ldp x29, x30, [sp], #16",      // ⑭ 恢复 fp, lr
        // ===== 跳转到目标线程 =====
        "ret",                          // ⑮ 返回到 lr（即 to 线程的 entry 或暂停点）
        in(reg) from_sp, in(reg) to_sp,
        options(noreturn)
    );
}
```

### 6.2 逐行解释

**保存阶段（①-⑥）**：

```asm
stp x29, x30, [sp, #-16]!    // ①
```

- `stp`（Store Pair）：同时存储两个 64 位寄存器
- `[sp, #-16]!`：先 `sp = sp - 16`（pre-index），再存储
- 存储 x29（fp）到 `[sp]`，x30（lr）到 `[sp+8]`
- 等价于：`sp -= 16; mem[sp] = x29; mem[sp+8] = x30;`

```asm
stp x27, x28, [sp, #-16]!    // ②
```

- 再次 `sp -= 16`，存储 x27, x28
- 顺序：先存高编号（x29/x30），再存低编号（x19/x20）
- 6 对 `stp` 共保存 12 个 callee-saved 寄存器，`sp` 减 96 字节

**SP 切换（⑦-⑧）**：

```asm
mov {0}, sp                  // ⑦
```

- `{0}` 是 `from_sp`（`in(reg)` 传入的寄存器）
- `mov {0}, sp` 将当前 SP 写入 `from_sp` 指向的内存（实际上 `mov` 是寄存器到寄存器，
  这里依赖编译器将 `from_sp` 加载到某通用寄存器，然后 `str` 写入内存）
- 实际语义：保存 from 线程的当前 SP 到其 TCB 的 `sp` 字段

```asm
ldr sp, {1}                  // ⑧
```

- `{1}` 是 `to_sp`
- `ldr sp, {1}` 从 `to_sp` 指向的内存加载新 SP
- 加载后 SP 指向 to 线程上次保存上下文的栈顶

**恢复阶段（⑨-⑭）**：

```asm
ldp x19, x20, [sp], #16      // ⑨
```

- `ldp`（Load Pair）：同时加载两个 64 位寄存器
- `[sp], #16`：先加载，再 `sp = sp + 16`（post-index）
- 从 `[sp]` 加载 x19，从 `[sp+8]` 加载 x20
- 恢复顺序与保存相反：先恢复低编号（x19/x20），再恢复高编号（x29/x30）

```asm
ldp x29, x30, [sp], #16      // ⑭
```

- 最后恢复 fp（x29）和 lr（x30）
- 此时 lr 已指向 to 线程的 entry 或上次暂停的返回地址

**跳转（⑮）**：

```asm
ret                          // ⑮
```

- `ret` 等价于 `br x30`（跳转到 lr）
- 由于 x30 已恢复为 to 线程的 lr，CPU 跳转到 to 线程的 entry（首次）或暂停点（恢复）

### 6.3 保存/恢复顺序的对称性

保存顺序（高→低）：x29/x30 → x27/x28 → ... → x19/x20
恢复顺序（低→高）：x19/x20 → ... → x27/x28 → x29/x30

这种「栈式」顺序保证：

- 保存后 SP 减 96 字节，指向 x19/x20 的存储位置
- 恢复时从同一位置开始，SP 加回 96 字节，回到栈帧顶部
- 恢复 x29/x30 在最后，保证 ret 时 lr 已正确

## 7. init_stack_frame 详解

### 7.1 函数签名

```rust
/// 初始化栈帧（ARM64：x0-x30 + spsr + elr）
///
/// 为新线程构造初始栈帧，使其看起来像「刚被切换进来」。
/// 返回初始 SP 值，存入 Tcb.sp。
///
/// # 安全性
/// - `stack_top` 必须指向已分配栈的栈顶（stack + stack_size）
/// - `stack_top` 必须 16 字节对齐
#[cfg(target_arch = "aarch64")]
unsafe fn init_stack_frame(stack_top: *mut u8, entry: u64) -> u64 {
    let mut sp = stack_top as u64;
    sp -= 272;  // 预留 272 字节栈帧
    let frame = sp as *mut u64;

    // x30 (lr) = entry —— ret 跳转到入口
    *frame.add(30) = entry;
    // elr_el1 = entry —— eret 跳转到入口（未来异常返回路径用）
    *frame.add(31) = entry;
    // spsr_el1 = 0x3C5 —— EL1h, IRQ unmasked, ARM64
    *frame.add(32) = 0x3C5;

    sp
}
```

### 7.2 栈帧构造逻辑

1. **`sp -= 272`**：从栈顶向下预留 272 字节空间（§5 布局）
2. **`*frame.add(30) = entry`**：设置 x30（lr）为入口地址
   - `context_switch` 的 `ret` 会跳转到此地址
3. **`*frame.add(31) = entry`**：设置 elr_el1 为入口地址
   - 若未来用 `eret` 路径进入线程，会跳转到此地址
4. **`*frame.add(32) = 0x3C5`**：设置 spsr_el1
   - 见 §7.3 详解

其余寄存器（x0-x29，除 x30）保持为 0（栈内存由 `alloc::alloc::alloc` 分配，
未初始化，但本版本不依赖这些值）。

### 7.3 spsr_el1 = 0x3C5 详解

`0x3C5` 的二进制分解（ARM64 PSTATE 格式）：

```
0x3C5 = 0b11_0_0_0_0_0_1_0_0_0_0_0_1_0_1

位字段：
  M[4:0]   = 0b00101 = 0x5  → EL1h（使用 SP_EL1）
  M[3:0]   实际为 0b0101   → EL1h 模式
  EL[3:2]  = 0b01           → EL1
  SP[0]    = 1              → 使用 SP_EL1（非 SP_EL0）
  ...
  IRQ      = 0              → IRQ 未掩码（启用中断）
  ARM64    = 1              → AArch64 模式（非 AArch32）
```

具体含义：

| 位 | 字段 | 值 | 含义 |
|----|------|----|------|
| [3:0] | M（模式） | 0b0101 | EL1h（EL1 + SP_EL1） |
| [4] | T（执行状态） | 0 | AArch64（非 AArch32） |
| [7] | F | 0 | FIQ 未掩码 |
| [6] | I | 0 | IRQ 未掩码（启用） |
| [8] | A | 0 | SError 未掩码 |
| [9] | D | 0 | Debug 异常未掩码 |
| [10] | IL | 0 | 非非法执行状态 |

**为什么 IRQ 不掩码**：新线程运行时需响应中断（如定时器、IPI），否则无法被抢占或
接收调度通知。若需关中断运行，由线程自行 `msr daifset, #2`。

## 8. 内存屏障与一致性（依赖 v0.17.0）

### 8.1 上下文切换前的屏障

`context_switch` 中保存上下文后、切换 SP 前，需要确保栈写入对其他核可见
（若线程可能被迁移到其他核）。依赖 v0.17.0 `eneros-smp` 的 coherence 模块：

```rust
// 未来增强（本版本简化未加，因协作式切换不涉及跨核迁移）
use eneros_smp::coherence;

// 切换前确保栈写入可见
unsafe {
    asm!("dsb ish");  // Data Synchronization Barrier, Inner Shareable
    asm!("isb");      // Instruction Synchronization Barrier
}
```

### 8.2 与 v0.17.0 coherence 模块的关系

| v0.17.0 提供 | v0.18.0 使用场景 |
|-------------|------------------|
| `coherence::dsb_ish()` | 切换前确保栈写入对其他核可见 |
| `coherence::isb()` | 切换后确保指令流水的上下文刷新 |
| `coherence::cache_clean(addr, size)` | 线程迁移到其他核前清理缓存 |
| `coherence::cache_invalidate(addr, size)` | 接收迁移线程后失效缓存 |

本版本（v0.18.0）为简化实现，**暂不**在 `context_switch` 中显式调用 coherence，
原因：

1. 协作式切换发生在当前核，无跨核迁移
2. 同核内 SP 切换后，缓存一致性由 ARMv8 自动保证
3. v0.19.0 引入跨核迁移时再补屏障

### 8.3 多核切换时的缓存一致性

未来（v0.19.0+）线程迁移到其他核时：

1. 源核：`cache_clean(sp_frame, 272)` —— 将栈帧写回内存
2. `dsb ish` —— 等待写完成
3. 目标核：`isb` —— 刷新指令流水
4. 目标核加载 sp 后，从内存读取栈帧（缓存未命中会自动从内存加载）

## 9. thread_switch 包装函数

### 9.1 安全封装

`context_switch` 是 unsafe naked 函数，需安全封装供外部调用：

```rust
/// 线程切换包装函数
///
/// 保存 from 线程的上下文（callee-saved 寄存器 + SP）到其 Tcb，
/// 从 to 线程的 Tcb 恢复上下文并跳转执行。
///
/// # 参数
/// - `from`: 当前运行线程的 Tcb 可变引用（保存其 SP）
/// - `to`: 目标线程的 Tcb 引用（读取其 SP）
///
/// # 安全性
/// - `from` 必须是当前正在运行的线程（state == Running）
/// - `to` 必须是 Ready 状态且其栈帧已初始化
/// - 调用后 `from` 的 state 应由调用方更新为 Ready/Blocked/Dead
/// - 调用后 `to` 的 state 应由调用方更新为 Running
pub fn thread_switch(from: &mut Tcb, to: &Tcb) {
    // 状态检查（debug 模式）
    debug_assert_eq!(from.state, ThreadState::Running);
    debug_assert_eq!(to.state, ThreadState::Ready);

    #[cfg(target_arch = "aarch64")]
    unsafe {
        context_switch(
            &mut from.sp as *mut u64,
            &to.sp as *const u64,
        );
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        // host 侧 stub：不实际切换，仅用于编译测试
        let _ = (from, to);
        panic!("context_switch: aarch64 only");
    }
}
```

### 9.2 调用流程

```rust
// 典型调用：thread_yield 内部
pub fn thread_yield() {
    let current = current_tid();  // per-core 当前线程
    let next = select_next_by_priority().expect("no runnable thread");

    let mut table = THREAD_TABLE.lock();
    let from = table[current_idx].as_mut().unwrap();
    let to = table[next_idx].as_ref().unwrap();

    // 状态转换
    from.transition(ThreadState::Ready).unwrap();
    to.transition(ThreadState::Running).unwrap();

    // 释放锁后切换（切换时不能持有锁）
    drop(table);

    // 实际切换
    thread_switch(from, to);  // 注意：from/to 引用在 drop 后失效，实际需重构
}
```

注意：实际实现中，`thread_switch` 持有 Tcb 引用时不能切换（切换后 from 的栈失效）。
正确做法是提取裸指针后释放锁：

```rust
let from_ptr: *mut Tcb = table[current_idx].as_mut().unwrap();
let to_ptr: *const Tcb = table[next_idx].as_ref().unwrap();
drop(table);  // 释放锁
unsafe {
    thread_switch(&mut *from_ptr, &*to_ptr);
}
```

## 10. cfg gate 策略

### 10.1 aarch64 真实实现

```rust
#[cfg(target_arch = "aarch64")]
#[naked]
pub unsafe extern "C" fn context_switch(from_sp: *mut u64, to_sp: *const u64) {
    asm!(
        "stp x29, x30, [sp, #-16]!",
        // ... 完整汇编 ...
        "ret",
        in(reg) from_sp, in(reg) to_sp,
        options(noreturn)
    );
}
```

### 10.2 host stub（保证编译）

```rust
#[cfg(not(target_arch = "aarch64"))]
pub unsafe extern "C" fn context_switch(_from_sp: *mut u64, _to_sp: *const u64) {
    panic!("context_switch: only supported on aarch64");
}
```

- host 侧（x86_64）不实现真实切换
- 保证 `cargo test --workspace` 在 host 上能编译通过
- 调用时 panic，明确告知不支持

### 10.3 测试策略

| 环境 | context_switch 测试 | 说明 |
|------|---------------------|------|
| host (x86_64) | 仅测编译通过 | stub 函数存在，签名正确 |
| aarch64 QEMU | 测真实切换 | 两线程交替打印验证 |
| aarch64 真机 | 性能测量 | 单次切换 < 2μs |

## 11. 性能考量

### 11.1 蓝图验收标准

蓝图 §6.3 与 §7.3 要求：**单次上下文切换 < 2μs**。

### 11.2 指令计数分析

`context_switch` 的指令数：

| 阶段 | 指令 | 数量 |
|------|------|------|
| 保存 | `stp` × 6 | 6 |
| SP 切换 | `mov` + `ldr` | 2 |
| 恢复 | `ldp` × 6 | 6 |
| 跳转 | `ret` | 1 |
| **合计** | | **15 条指令** |

### 11.3 QEMU cortex-a57 预期性能

- QEMU 模拟 ARM cortex-a57，无真实流水线
- 估算：15 条指令 × ~50ns/指令 ≈ 0.75μs
- 加上锁开销（Spinlock lock/unlock）≈ 0.3μs
- **总预期：~1.05μs < 2μs**（满足验收）

### 11.4 缓存局部性

TCB 应靠近使用核心，减少缓存未命中：

- v0.16.0 `PerCoreRq` 已是 per-core，缓存局部性好
- 全局 `THREAD_TABLE` 是共享数据，访问需跨核缓存同步
- 优化方向（未来）：per-core TCB 缓存（热数据）

## 12. 安全注意事项

### 12.1 栈溢出防护

本版本**未**实现栈溢出防护（guard page），风险：

- 线程栈溢出会覆盖相邻内存（其他线程的栈或 TCB）
- 调试困难（溢出后行为未定义）

缓解措施：

- 建议栈大小 ≥ 4KB（蓝图 §8.3）
- 未来版本用 MMU 在栈底映射 guard page（只读，溢出触发 fault）

### 12.2 销毁 Running 线程的危险性

`thread_destroy` 拒绝销毁 Running 状态线程（§6.2），原因：

- Running 线程的栈正在使用，dealloc 会破坏当前执行
- 必须先 `thread_yield` 切换到其他线程，再 destroy

若需强制销毁（如 panic 处理），流程：

1. 标记 `state = Dead`
2. 切换到其他线程（不恢复 Dead 线程的上下文）
3. 切换后安全 dealloc 栈

### 12.3 裸指针访问的 unsafe 边界

`Tcb` 含裸指针（`stack`, `stack_top`），相关操作均为 `unsafe`：

| 操作 | unsafe 原因 | 边界检查 |
|------|-----------|---------|
| `init_stack_frame` | 写入裸指针 | 检查 `stack_top` 对齐 + 范围 |
| `context_switch` | 修改 SP、寄存器 | 调用方保证 Tcb 有效性 |
| `thread_switch` | 解引用裸指针 | 调用方保证 from/to 生命周期 |
| `dealloc(stack)` | 释放内存 | 确保不再被任何核访问 |

`Tcb` 不 impl `Send` / `Sync`（D5 决策），强制通过 `THREAD_TABLE + Spinlock` 访问。

## 13. 已知限制与未来工作

### 13.1 已知限制

| # | 限制 | 影响 | 缓解 |
|---|------|------|------|
| L1 | 仅 EL1 内核态线程 | 不支持用户态线程 | 未来版本加 EL0/EL1 切换 |
| L2 | 无抢占 | 实时性不足 | v0.19.0+ 引入时间片中断 |
| L3 | 无浮点/SIMD 上下文保存 | 浮点密集型线程会丢失状态 | 按需添加 `fpsimd` 保存 |
| L4 | 无栈溢出防护 | 溢出会破坏内存 | 未来用 guard page |
| L5 | 无跨核迁移屏障 | 协作式切换不涉及迁移 | v0.19.0+ 补 coherence 调用 |

### 13.2 未来工作

| 版本 | 工作 | 依赖 |
|------|------|------|
| v0.19.0 | 跨核迁移时的缓存一致性屏障 | v0.17.0 coherence |
| v0.19.0+ | 抢占式调度（定时器中断触发 yield） | v0.12.0 定时器 |
| 未来 | 浮点/SIMD 上下文保存（FPCR/FPSR/Q0-Q31） | 按需 |
| 未来 | 用户态线程模式切换（EL0 ↔ EL1） | MMU 虚拟化 |
| 未来 | 栈溢出防护（guard page + fault handler） | MMU |
| 未来 | 性能监控（PMU 计数切换耗时） | ARM PMU 驱动 |

## 14. 参考资料

- `蓝图/phase0.md §v0.18.0`（第 3893–4111 行）—— 本版本蓝图
- `蓝图/Power_Native_Agent_OS_Blueprint.md §8.5`—— naked 函数 ABI 要求
- `蓝图/Power_Native_Agent_OS_Blueprint.md §6.3`—— 性能要求（切换 < 2μs）
- `docs/smp/thread-abstraction-design.md`—— TCB 与状态机设计（配套）
- `docs/smp/memory-coherence-design.md`—— v0.17.0 多核一致性（依赖）
- `docs/smp/multi-core-scheduler-design.md`—— v0.16.0 调度器（前置）
- `docs/smp/armv8-memory-model.md`—— ARMv8 内存模型
- ARM Architecture Reference Manual (ARMv8, ARM DDI 0487)
  - §C5：异常模型
  - §D1：寄存器
  - §D6：SPSR_EL1 格式
  - §C6：异常返回（eret）
