# EnerOS 多核启动框架设计

> 版本：v0.15.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.15.0`（多核启动与 IPI）、`Power_Native_Agent_OS_Blueprint.md §6`（多核架构）、§43.1（no_std 合规）

## 1. 概述

EnerOS SMP 框架（crate 名 `eneros-smp`）是 Phase 0 P0-E 的核心交付物，为系统提供
多核启动与核间通信的基础能力。本文档描述其中的**多核启动**部分，对应的实现文件为
`smp/src/boot.rs`；核间中断（IPI）部分另见 `docs/ipi-mechanism.md`。

v0.15.0 多核启动框架的目标与范围：

- **唤醒 secondary core**：主核（primary core）通过 ARM PSCI `CPU_ON` 标准接口唤醒
  secondary core，避免依赖 SoC 特定的唤醒地址寄存器（设计决策 D1）。
- **核状态跟踪**：维护每个核的生命周期状态（`Offline / Booting / Online / Halted`），
  供调度器、IPI 子系统、panic 框架等查询。
- **secondary 入口**：提供 `secondary_entry()` 作为 secondary core 被唤醒后的统一入口，
  完成状态机迁移并在 `wfe` 循环中等待后续版本的调度器接管。
- **无 HAL 依赖**：所有 aarch64 专属指令（`mrs mpidr_el1` / `hvc #0` / `wfe`）在 crate
  内直接以内联汇编实现，不依赖 `eneros-hal`（设计决策 D2）。

本版本**不**包含的能力（明确标注为「未来扩展」，见 §9）：

- 多核调度与负载均衡（v0.16.0 调度器版本）
- GICv3 多核 Redistributor 真正的初始化（仅保留 stub）
- TLB Shootdown 在 MMU 虚拟化中的实际使用

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1 全项目 no_std 要求；
`Cargo.toml` 仅依赖 `spin` 与 `heapless`，无 `eneros-hal` 依赖。

## 2. 设计决策

### 2.1 D1：使用 PSCI CPU_ON 而非唤醒地址寄存器

| 维度 | 唤醒地址寄存器方案 | PSCI CPU_ON 方案（采纳） |
|------|---------------------|--------------------------|
| 标准化 | SoC 特定，需查阅芯片手册 | ARM 标准接口（PSCI 1.0+） |
| QEMU 支持 | 需针对 QEMU virt 模拟特定寄存器 | QEMU virt 原生支持 PSCI conduit |
| SoC 差异 | 每款 SoC 不同，移植成本高 | firmware（BL31/ATF）屏蔽 SoC 差异 |
| 启动顺序 | secondary 轮询唤醒寄存器 | firmware 直接让 secondary 跳转到入口 |
| 蓝图原方案 | 蓝图 §6 提到「唤醒地址寄存器」思路 | v0.15.0 实现改用 PSCI（见 §7 对比） |

**结论**：采用 PSCI `CPU_ON`，函数号 `0x8400_000E`，QEMU virt 的 PSCI conduit 为 HVC。

### 2.2 D2：smp crate 不依赖 eneros-hal

`smp` crate 直接在 `boot.rs` / `ipi.rs` 内用 `core::arch::asm!` 写 aarch64 内联汇编
（`mrs mpidr_el1`、`hvc #0`、`wfe`、`msr icc_sgi1r_el1`），不通过 `eneros-hal` 抽象。
理由：

1. **依赖图最小化**：`smp` 是底层子系统，反向依赖 HAL 会让依赖图绕回到底层 crate。
2. **crate 自包含**：测试与跨 crate 复用时无需拉起整套 HAL。
3. **指令面狭窄**：SMP 实际用到的 aarch64 指令仅 4 条，封装成 HAL trait 收益不大。

## 3. 核心数据结构

所有数据结构定义于 `smp/src/boot.rs`，全部 `Copy` 或无堆，满足 no_std 约束。

### 3.1 CoreState 枚举

CPU 核的生命周期状态。`#[repr(u8)]` 保证与 `CORE_STATES: [AtomicU8; 8]` 的存储
布局严格一致，可直接 `as u8` 写入原子数组。

```rust
// smp/src/boot.rs
const MAX_CORES: usize = 8;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Offline = 0,
    Booting  = 1,
    Online   = 2,
    Halted   = 3,
}
```

| 判别值 | 状态 | 含义 |
|--------|------|------|
| 0 | Offline | 核未启动或已被关停 |
| 1 | Booting | 已通过 PSCI `CPU_ON` 唤醒，正在执行 `secondary_entry` 早期路径 |
| 2 | Online | `secondary_entry` 完成状态迁移，已停在 `wfe` 等待调度器接管 |
| 3 | Halted | 显式停机（本版本不写入此状态，留作未来 `cpu_off` 路径） |

### 3.2 CoreInfo 结构体

每个核的静态描述。`entry` / `stack_base` 在 v0.15.0 暂未被 `wake_secondary` 使用
（PSCI `CPU_ON` 只接受 entry 参数），保留字段以便后续版本传递 stack。

```rust
// smp/src/boot.rs
#[derive(Debug, Clone, Copy)]
pub struct CoreInfo {
    pub id: u32,
    pub entry: u64,
    pub stack_base: u64,
    pub state: CoreState,
}
```

### 3.3 CORES 静态表

`spin::Mutex<[CoreInfo; 8]>`，编译期常量初始化（8 个槽位全部 `Offline`）。
锁保护整张表，写入路径主要是 `set_core_state` 与 `smp_init`。

```rust
// smp/src/boot.rs
static CORES: Mutex<[CoreInfo; MAX_CORES]> = Mutex::new([
    CoreInfo { id: 0, entry: 0, stack_base: 0, state: CoreState::Offline },
    // ... 共 8 个槽位 ...
]);
```

### 3.4 CORE_STATES 无锁查询数组

`[AtomicU8; 8]`，与 `CORES` 镜像但**无需锁**即可读。设计动机：

- 调度器、IPI 派发、panic 框架在热路径中频繁查询核状态，避免每次抢 `CORES` 锁。
- `core_state()` 走 `AtomicU8::load(Acquire)`，可由中断上下文安全调用。

```rust
// smp/src/boot.rs
static CORE_STATES: [AtomicU8; MAX_CORES] = [
    AtomicU8::new(0), /* × 8 */
];
```

`set_core_state` 同时更新 `CORE_STATES`（`Release` 序）与 `CORES`（互斥锁内），
保证两侧一致；读者走原子路径即可获得最终一致视图。

### 3.5 CORE_COUNT 全局核数

`spin::Mutex<u32>`，由 `smp_init(core_count)` 设置，默认值 1。
`ipi_broadcast` 与 `ipi_dispatch` 通过 `core_count()` 确定遍历范围。

```rust
// smp/src/boot.rs
static CORE_COUNT: Mutex<u32> = Mutex::new(1);
```

## 4. SMP 启动流程

### 4.1 Primary core 初始化

主核在系统启动早期调用 `smp_init(core_count)`：

```rust
// smp/src/boot.rs
pub fn smp_init(core_count: u32) {
    *CORE_COUNT.lock() = core_count;
    let mut cores = CORES.lock();
    let count = core::cmp::min(core_count as usize, MAX_CORES);
    for i in 0..count {
        cores[i].id = i as u32;
        cores[i].state = CoreState::Offline;
        CORE_STATES[i].store(CoreState::Offline as u8, Ordering::Release);
    }
}
```

- 设置 `CORE_COUNT`（即使 `core_count > 8`，写入原值，但 `CORES` 表只填前 8 槽）。
- 把 `0..count` 范围内的核状态显式置 `Offline`，覆盖任何先前残留状态。
- 主核自身通常也调用此函数，但 v0.15.0 不在 `smp_init` 内自动把 core 0 标为 `Online`
  —— 该操作由调用方（kernel 启动代码）显式完成。

### 4.2 唤醒 secondary core

主核对每个 secondary core 调用 `wake_secondary(core_id, entry)`：

```rust
// smp/src/boot.rs (aarch64)
pub fn wake_secondary(core_id: u32, entry: u64) {
    set_core_state(core_id, CoreState::Booting);   // 先标记 Booting
    let psci_fn: u64 = 0x8400_000E;                 // PSCI CPU_ON
    let target_mpidr: u64 = core_id as u64;         // core_id → MPIDR Aff0
    let entry_addr: u64 = entry;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            in("x0") psci_fn,
            in("x1") target_mpidr,
            in("x2") entry_addr,
        );
    }
}
```

关键点：

1. **先置 Booting 再发 HVC**：确保 secondary 被唤醒后查询自身状态时已能读到 `Booting`。
2. **MPIDR 映射**：QEMU virt 单簇下 `core_id` 即 MPIDR Aff0，直接 `core_id as u64`。
3. **未传 stack**：v0.15.0 假设 secondary 复用 PSCI 给定的临时栈，待真机移植时补 `CoreInfo.stack_base`。
4. **失败处理**：本版本未检查 HVC 返回值（x0），唤醒失败的回退留待 §6 末与 §9 描述。

### 4.3 Secondary 入口

`secondary_entry()` 是 secondary core 被唤醒后跳转到的 Rust 函数，不返回（`-> !`）：

```rust
// smp/src/boot.rs (aarch64)
pub fn secondary_entry() -> ! {
    let id = read_core_id();
    set_core_state(id, CoreState::Booting);
    // GIC redistributor initialization stub.
    set_core_state(id, CoreState::Online);
    loop {
        core::arch::asm!("wfe", options(nostack, preserves_flags));
    }
}
```

执行步骤：

1. `read_core_id()` 通过 `mrs mpidr_el1` 取 Aff0。
2. `set_core_state(id, Booting)` —— 与主核侧的 `Booting` 标记保持幂等。
3. **GIC Redistributor 初始化 stub**：注释占位，不触碰 MMIO。真机移植时此处需做
   `GICR_WAKER` 清零、`GICR_ICENABLER0`、`GICR_IGROUPR0` 配置等（见 §9）。
4. `set_core_state(id, Online)` —— 至此 secondary 已就绪。
5. `loop { wfe }` —— 让出 CPU 等待事件，避免空转烧电。后续 v0.16.0 调度器会替换此循环
   为真正的 idle 线程切换。

## 5. PSCI CPU_ON 机制

### 5.1 PSCI 函数号

`0x8400_000E` 是 32-bit 调用约定的 PSCI `CPU_ON`。低 31 位为函数标识，
bit 31=0 表示走 32-bit 返回值约定（与之相对，`0xC400_000E` 是 64-bit 版本，本版本未采用）。

| PSCI 函数 | 函数号 | 用途 |
|-----------|--------|------|
| `PSCI_VERSION` | `0x8400_0000` | 探测 PSCI 实现（本版本未用） |
| `CPU_ON` | `0x8400_000E` | 唤醒 secondary core（本版本使用） |
| `CPU_OFF` | `0x8400_0008` | 关闭自身（未来扩展） |
| `SYSTEM_RESET` | `0x8400_0009` | 全系统复位（panic 框架未来可选路径） |

### 5.2 调用约定

PSCI 通过 `hvc #0`（Hypervisor Call）或 `smc #0`（Secure Monitor Call）陷入
firmware。QEMU virt 的 PSCI conduit 配置为 **HVC**，因此本 crate 使用 `hvc #0`。
真机移植时若 conduit 为 SMC（例如在 ATF 启动的真机环境），需把 `hvc` 改为 `smc`。

寄存器使用约定（SMC Calling Convention）：

| 寄存器 | 输入 | 含义 |
|--------|------|------|
| x0 | `0x8400_000E` | PSCI `CPU_ON` 函数号 |
| x1 | `core_id`（即 target MPIDR Aff0） | 目标核的 MPIDR |
| x2 | `entry` | secondary core 的跳转入口地址 |
| x0 | 返回值 | 0 = `PSCI_SUCCESS`，其余为错误码（本版本未检查） |

### 5.3 entry 地址要求

PSCI 要求 entry 地址是**物理地址**且 4 字节对齐。在 QEMU virt 上 entry 通常指向
内核镜像内的一段跳转 stub（汇编），由 stub 设置好栈与页表后再调用 Rust `secondary_entry`。
本 crate 只提供 `secondary_entry`，具体的汇编跳板由 kernel 顶层启动代码提供。

## 6. CoreState 状态机

### 6.1 状态转换图

```
                        smp_init()
                  ┌──────────────────────────┐
                  │                          ▼
              ┌────────┐  wake_secondary()  ┌────────┐
              │Offline │ ─────────────────► │Booting │
              └────────┘                    └────────┘
                   ▲                            │
                   │                            │ secondary_entry()
                   │                            │ 完成初始化
                   │                            ▼
                   │                        ┌────────┐
                   │      cpu_off (未来)    │ Online │
                   └────────────────────────┤        │
                                            └────────┘
                                                 │
                                                 │ 显式停机 (未来)
                                                 ▼
                                            ┌────────┐
                                            │ Halted │
                                            └────────┘
```

### 6.2 各转换的触发点

| 转换 | 触发函数 | 文件位置 |
|------|----------|----------|
| (init) → Offline | `smp_init()` | `smp/src/boot.rs` |
| Offline → Booting | `wake_secondary()`（主核侧） | `smp/src/boot.rs` |
| (Booting) → Booting | `secondary_entry()`（从核侧，幂等覆盖） | `smp/src/boot.rs` |
| Booting → Online | `secondary_entry()` | `smp/src/boot.rs` |
| Online → Halted | （未来 `cpu_off` 路径） | 未实现 |
| Halted/Online → Offline | （未来 reset 路径） | 未实现 |

### 6.3 唤醒失败处理（当前限制）

v0.15.0 **未检查** `hvc #0` 的返回值，唤醒失败时 `CORE_STATES[id]` 会停留在 `Booting`
状态。当前调用方可通过轮询 `core_state(id) == Online` 与超时来判定唤醒是否成功。
超时后回退到 `Offline` 的逻辑由调用方实现：

```rust
// 未来调用方示例（未实现，仅示意）
let deadline = now_ms + 1000;
while core_state(1) != Some(CoreState::Online) {
    if now_ms > deadline {
        set_core_state(1, CoreState::Offline);   // 超时回退
        break;
    }
}
```

完整的 PSCI 错误码解析与超时回退将在真机移植阶段补入 `wake_secondary`。

## 7. 与蓝图唤醒地址寄存器对比

蓝图 `phase0.md §6` 与 `Power_Native_Agent_OS_Blueprint.md §6` 描述的多核启动
原方案为「SoC 特定的唤醒地址寄存器」——secondary core 上电后轮询某个 SoC 内部
寄存器，主核写入跳转地址后 secondary 跳出循环。本版本实际改用 PSCI `CPU_ON`，
对比见下表：

| 维度 | 蓝图原方案（唤醒地址寄存器） | 实际方案（PSCI CPU_ON） |
|------|-------------------------------|--------------------------|
| 抽象层级 | SoC 寄存器级 | firmware 抽象层 |
| SoC 依赖 | 强（每款 SoC 寄存器地址不同） | 弱（PSCI 标准化） |
| QEMU 支持 | 需模拟特定 SoC | QEMU virt 原生 PSCI 支持 |
| 启动延迟 | secondary 轮询周期 | firmware 直接跳转，延迟低 |
| 文档需求 | 需 SoC 数据手册 | 仅需 ARM PSCI 规范 |
| firmware 依赖 | 不需要 firmware | 依赖 BL31/ATF 实现 PSCI |
| 多 SoC 移植成本 | 高（每个 SoC 重写唤醒代码） | 低（仅 conduit 类型差异） |

### 7.1 优势

- **标准化**：PSCI 是 ARM 推荐的多核管理标准接口，长期可维护。
- **跨 SoC**：从 QEMU virt 迁移到真机时仅需确认 conduit（HVC vs SMC）。
- **无需 SoC 文档**：不必为每款芯片查找唤醒寄存器地址。
- **QEMU 一致性**：QEMU virt 即用 PSCI，调试体验与真机一致。

### 7.2 劣势

- **依赖 firmware**：BL31/ATF 必须正确实现 PSCI `CPU_ON`，否则唤醒失败。
  QEMU 自带的 PSCI 实现已验证可用；真机移植需确认 ATF 版本与配置。
- **返回值检查缺失**：v0.15.0 未解析 PSCI 错误码（见 §6.3），需后续补全。
- **conduit 切换**：从 HVC 改 SMC 需改源码（单行汇编差异），无运行时切换。

### 7.3 与蓝图一致性的处理

蓝图 §6 的唤醒地址寄存器思路在实现层面被 PSCI 替代，但**架构语义未变**：
secondary core 仍由主核显式唤醒，唤醒后仍进入统一入口。本文档作为对蓝图的实现偏差说明，
后续版本路线图中将把 PSCI 作为标准方案。

## 8. aarch64 cfg gate 策略

所有 aarch64 专属指令均用 `#[cfg(target_arch = "aarch64")]` 门控，
host（x86_64）测试构建走 stub 路径，保证 `cargo test` 在开发机直接运行。

### 8.1 read_core_id

```rust
// smp/src/boot.rs (aarch64)
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
    (id & 0xff) as u32   // Aff0
}

// smp/src/boot.rs (host)
#[cfg(not(target_arch = "aarch64"))]
pub fn read_core_id() -> u32 { 0 }
```

host 返回 0，使所有测试中 `read_core_id()` 表现为「主核」。

### 8.2 wake_secondary

```rust
// smp/src/boot.rs (host)
#[cfg(not(target_arch = "aarch64"))]
pub fn wake_secondary(core_id: u32, _entry: u64) {
    set_core_state(core_id, CoreState::Booting);   // 状态更新仍执行
    // 不发 hvc，无副作用
}
```

host 上 `wake_secondary` 仍把目标核标为 `Booting`，但不发出 `hvc`。
**不**自动推进到 `Online`，以保留「唤醒失败」的语义供测试断言。

### 8.3 secondary_entry

```rust
// smp/src/boot.rs (host)
#[cfg(not(target_arch = "aarch64"))]
pub fn secondary_entry() -> ! {
    let id = read_core_id();
    set_core_state(id, CoreState::Booting);
    set_core_state(id, CoreState::Online);
    loop {
        core::hint::spin_loop();   // host 用 spin_loop 而非 wfe
    }
}
```

host 路径用 `core::hint::spin_loop()` 替代 `wfe`，因 `wfe` 是 aarch64 独有指令。
**注意**：host 测试从不真正调用 `secondary_entry`（它死循环），cfg gate 仅保证编译通过。

### 8.4 cfg gate 一览

| 函数 | aarch64 行为 | host 行为 |
|------|--------------|-----------|
| `read_core_id()` | `mrs mpidr_el1` 取 Aff0 | 返回 0 |
| `wake_secondary()` | `set_core_state` + `hvc #0` | 仅 `set_core_state` |
| `secondary_entry()` | `wfe` 死循环 | `spin_loop` 死循环 |

## 9. 未来扩展

### 9.1 v0.16.0：多核调度与绑核

- 把 `secondary_entry` 末尾的 `wfe` 死循环替换为调度器 idle 线程切换。
- 实现 `cpu_affinity_set(tid, core_mask)` 与调度器运行队列按核分组。
- 通过 `IpiMsg::Reschedule` 在核间触发重新调度（见 `docs/ipi-mechanism.md` §7）。

### 9.2 v0.16.0+：GICv3 多核 Redistributor 发现

v0.15.0 `secondary_entry` 中 GIC Redistributor 初始化仅留 stub。真机移植需：

1. 遍历 `GICR_TYPER` 寄存器发现每个 Redistributor 基址。
2. 对当前核的 Redistributor：
   - `GICR_WAKER` 清 `ProcessorSleep` 位、置 `ChildrenAwake`。
   - `GICR_ICENABLER0` 禁用所有 SGI/PPI。
   - `GICR_IGROUPR0` 配置分组（Group 1 用于内核 IRQ）。
   - `GICR_ISENABLER0` 启用 SGI 0（IPI 通道）。

### 9.3 真机验证：QEMU virt 启动 4 核

- 在 QEMU virt `-smp 4` 下验证 4 核全部进入 `Online`。
- 通过 `core_count()` 与 `core_state()` 在不同核上交叉验证。
- 后续在真机（RK3568 / 飞腾 D2000 等 ATF 平台）验证 conduit 切换为 SMC。

### 9.4 PSCI 错误码解析

补全 `wake_secondary` 对 `hvc #0` 返回值的解析：

| 返回值 | 含义 | 处理 |
|--------|------|------|
| 0 | `PSCI_SUCCESS` | 等待 `Online` |
| -2 | `INVALID_PARAMETERS` | MPIDR 不存在 |
| -6 | `ALREADY_ON` | 该核已在运行 |
| -7 | `INTERNAL_FAILURE` | firmware 内部错误 |

### 9.5 CPU_OFF / Hotplug

未来支持 `cpu_off` 把自身标记 `Halted` 并通过 PSCI `CPU_OFF` 通知 firmware 关停。
配合 `Online → Halted → Offline` 完成完整生命周期。

## 10. 全局 API

| API | 作用 | 文件位置 |
|-----|------|----------|
| `read_core_id() -> u32` | 读 `MPIDR_EL1` Aff0（host 返回 0） | `smp/src/boot.rs` |
| `smp_init(core_count: u32)` | 初始化 `CORES` / `CORE_STATES` / `CORE_COUNT` | `smp/src/boot.rs` |
| `wake_secondary(core_id, entry)` | 通过 PSCI `CPU_ON` 唤醒 secondary | `smp/src/boot.rs` |
| `secondary_entry() -> !` | secondary core 入口，完成后停在 `wfe` | `smp/src/boot.rs` |
| `core_state(id) -> Option<CoreState>` | 无锁查询核状态 | `smp/src/boot.rs` |
| `set_core_state(id, state)` | 同时更新 `CORE_STATES` 与 `CORES` | `smp/src/boot.rs` |
| `core_count() -> u32` | 返回配置的核数 | `smp/src/boot.rs` |

`lib.rs` 通过 `pub use` 把以上 API 全部 re-export 到 crate 根。

## 11. 测试覆盖

`smp/src/boot.rs` 内 7 个单元测试：

| 测试 | 验证点 |
|------|--------|
| `test_core_state_variants` | 4 个 `CoreState` 判别值互不相等 |
| `test_core_state_repr_u8` | `Offline=0 / Booting=1 / Online=2 / Halted=3` |
| `test_core_info_construction` | `CoreInfo` 字段正确填充 |
| `test_core_state_query` | `set_core_state(0, Offline)` 后查询返回 `Offline` |
| `test_set_core_state` | `Offline → Booting → Online` 序列推进 |
| `test_smp_init` | `smp_init(4)` 后 `core_count()==4`，可回滚到 1 |
| `test_read_core_id_host_returns_zero` | host 构建下 `read_core_id()==0` |

测试用 `std::sync::Mutex` 串行化以避免共享全局 `CORES` / `CORE_STATES` 数据竞争。

## 12. 蓝图符合性

对照 `phase0.md §v0.15.0`：

| 蓝图条目 | 实现状态 |
|----------|----------|
| 多核启动：唤醒 secondary core | ✅ `wake_secondary` 通过 PSCI `CPU_ON` |
| secondary 入口：状态机迁移 | ✅ `secondary_entry` 完成 Booting → Online |
| 核状态跟踪：`Offline/Booting/Online/Halted` | ✅ `CoreState` enum + `CORE_STATES` 原子数组 |
| no_std 合规（蓝图 §43.1） | ✅ `#![cfg_attr(not(test), no_std)]`，仅依赖 `spin` / `heapless` |
| 不依赖 HAL（D2） | ✅ 内联汇编直写，无 `eneros-hal` 依赖 |
| 真机唤醒地址寄存器方案（蓝图 §6） | ⚠️ 改用 PSCI（见 §7 对比说明） |
| GICv3 Redistributor 初始化 | ⏳ stub 占位，真机移植补全（见 §9.2） |
| PSCI 错误码解析 | ⏳ v0.15.0 未检查返回值，留待 §9.4 |
| 多核调度集成（蓝图 v0.16.0） | ⏳ 接口已就位，待 v0.16.0 调度器版本接入 |
