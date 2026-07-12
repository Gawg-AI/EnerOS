# HAL 设计白皮书

> 版本: v0.5.0
> 依据: 蓝图 phase0.md §v0.5.0 §4.5, §5, §8
> crate: eneros-hal

---

## 1. 背景与动机

EnerOS 面向能源边缘场景，目标硬件多样：ARM64（QEMU virt 验证平台）、飞腾（Phytium，国产化服务器/边缘）、鲲鹏（Kunpeng，华为 ARM64 服务器）、未来 RISC-V。不同平台的 CPU 核心控制、中断控制器（GICv3 vs. PLIC）、定时器、UART、GPIO 实现差异显著。

若内核直接调用硬件寄存器，每接入一款硬件需重写大量代码，且无法在 QEMU 上验证后再迁移到真实硬件。HAL（Hardware Abstraction Layer）的引入正是为了解决这一痛点：在硬件与内核之间建立稳定的契约层，让上层逻辑与硬件细节解耦。

蓝图 §4.5 明确要求 v0.5.0 先行定义完整的 HAL trait 接口规范集，让 v0.6.0/v0.7.0 的 ARM64 实现以及未来飞腾/鲲鹏/RISC-V 的 BSP 实现有统一契约可依，避免实现返工。本白皮书论证 HAL 的设计决策：为什么用 trait 抽象、为什么用 HalProvider 单例、dyn 安全性如何保证、no_std 下的取舍，以及与业界方案的对比。

---

## 2. 方案选型：为什么用 trait 抽象

### 2.1 候选方案对比

| 方案 | 类型安全 | 零成本抽象 | 可扩展性 | 文档化 | no_std 友好 | Rust 惯用 |
|------|----------|------------|----------|--------|--------------|-----------|
| Rust trait | 编译期检查 | 静态分发零开销，动态分发一次间接调用 | 新硬件 impl trait 即可 | 方法自带文档注释 | 原生支持 | 是 |
| 函数指针表 | 弱（无签名约束） | 一次间接调用 | 新硬件填充表 | 需外部文档 | 友好 | 否（C 风格） |
| 顶层 fn | 无（无多态） | 直接调用 | 差（需 cfg 切换） | 需外部文档 | 友好 | 否 |
| C 风格 ops 结构体 | 弱 | 一次间接调用 | 新硬件填充结构体 | 需外部文档 | 友好 | 否 |

trait 方案的优势：

- **零成本抽象**：静态分发（单态化）时编译器内联，零运行时开销；动态分发（`dyn`）仅一次间接调用，与函数指针表等价
- **类型安全**：编译期检查实现完整性，遗漏方法直接编译失败，无需运行时测试覆盖
- **可扩展**：新硬件只需 `impl HalCpu for MyBsp`，无需修改上层代码或全局表
- **文档化**：trait 方法的文档注释随签名传递，IDE 悬浮提示自动可用
- **Rust 惯用**：与 `embedded-hal`、`std::io::Read/Write` 等生态一致，降低学习成本

### 2.2 选定方案：trait + HalProvider 单例

在 trait 基础上，进一步引入 `HalProvider` 单例注册器。理由：

- **BSP 注入点统一**：所有 6 个子系统的实现通过一个 `HalProvider` 聚合，BSP 只需实现一个 trait
- **调用方无需传递 `&dyn`**：上层代码直接 `hal().cpu().enable_irq()`，无需在每个函数签名中穿透 HAL 引用
- **生命周期简化**：`&'static dyn` 避免生命周期标注污染上层 API
- **测试可替换**：mock 实现可通过 `init_hal(&MockProvider)` 注入，支持 host 单元测试

---

## 3. HalProvider 单例注入模式

### 3.1 设计

全局 HAL 引用存储于 `static mut`：

```rust
static mut HAL: Option<&'static dyn HalProvider> = None;

pub fn init_hal(provider: &'static dyn HalProvider) {
    unsafe { HAL = Some(provider); }
}

pub fn hal() -> &'static dyn HalProvider {
    unsafe { HAL.expect("HAL not initialized: call init_hal() during boot first") }
}
```

`init_hal` 在启动早期由 BSP 调用一次，注入实现；后续所有调用方通过 `hal()` 获取全局引用。

### 3.2 vs 全局可变状态

| 维度 | HalProvider 单例 | 全局可变状态（如 `static mut COUNTER`） |
|------|------------------|------------------------------------------|
| 侵入性 | 低，调用方无需传参 | 高，每处需显式访问全局 |
| 测试性 | 高，mock 可注入 | 低，全局状态难替换 |
| 初始化顺序 | 明确，boot 早期一次 | 可能分散，时序复杂 |
| 可变性 | write-once 后只读 | 持续可变，需锁 |
| unsafe 范围 | 限于 init/hal 两处 | 散布各处 |

HalProvider 单例本质是"一次性写入的全局只读引用"，将可变性降到最低，是介于"完全全局可变"与"参数显式传递"之间的折中方案。

### 3.3 安全性分析

`static mut HAL` 的 `unsafe` 访问仅出现在两处：

1. **`init_hal`**：写入 `HAL`。契约要求在单线程 boot 上下文调用，调度器启动前。此时无并发，无数据竞争。
2. **`hal`**：读取 `HAL`。`init_hal` 之后 `HAL` 只读，多核并发读不会产生数据竞争。

关键不变量：

- **write-once 语义**：`init_hal` 应只调用一次，重复调用覆盖前值但不推荐（文档标注契约）
- **`&'static` 生命周期**：注入的 `provider` 必须有 `static` 生命周期，通常是 `static` 变量，避免悬垂引用
- **boot 上下文**：`init_hal` 必须在单线程阶段调用，避免写入期间的竞态

此设计在 seL4 微内核、Linux 内核等 OS 中均有先例（如 Linux 的 `early_param` 机制），是 OS HAL 注入的常见模式。

---

## 4. dyn 安全性分析

### 4.1 trait object 的限制

Rust 的 `dyn Trait`（trait object）有以下限制（蓝图 §8.4）：

- **无泛型方法**：泛型方法需单态化，与动态分发冲突（除非加 `where Self: Sized` 排除出 vtable）
- **无 `Self` 返回**：`Self` 大小未知，无法构造
- **无 `Self` 参数**（除 `&self`/`&mut self`）：同上
- **关联常量/类型需约束**：关联类型在 trait object 中需指定

违反上述限制的 trait 无法构造 `dyn Trait`。

### 4.2 本 crate 的合规性

逐个 trait 检查，所有方法均满足 dyn 安全：

| Trait | 方法 | 泛型 | Self 返回 | dyn 安全 |
|-------|------|------|-----------|----------|
| `HalCpu` | enable_irq/disable_irq/current_core/core_count/halt/wfi | 无 | 无 | 是 |
| `HalMem` | map/unmap/translate/set_domain | 无 | 无 | 是 |
| `HalIrq` | register/unregister/enable/disable/eoi | 无 | 无 | 是 |
| `HalClock` | now_ns/frequency_hz/set_deadline | 无 | 无 | 是 |
| `HalSerial` | write/read/flush | 无 | 无 | 是 |
| `HalGpio` | set_dir/set/get/toggle | 无 | 无 | 是 |

`HalProvider` 同样满足：6 个方法均返回 `&'static dyn HalXxx`，无泛型、无 `Self`。

注意：`HalSerial::write(&self, data: &[u8])` 中的 `&[u8]` 是切片引用，不是泛型参数，不影响 dyn 安全。

### 4.3 如果需要泛型方法怎么办

若未来某 trait 需要泛型方法（如 `fn read<T: Readable>(&self, dev: T)`），可选方案：

- **拆分 trait**：将泛型方法拆到单独的、非 dyn 的 trait，主 trait 保持 dyn 安全
- **用 `&dyn [T]` 替代泛型**：如 `fn read(&self, dev: &dyn Readable)`，通过 trait object 替代泛型参数
- **用枚举替代泛型参数**：如 `fn read(&self, dev_kind: DevKind)`，用枚举分发
- **`where Self: Sized` 排除**：在泛型方法上加约束，使其不进入 vtable，trait 仍可 dyn（但该方法只能静态调用）

本 crate v0.5.0 无泛型方法需求，所有方法签名已确认为 dyn 安全。

---

## 5. no_std 约束下的设计取舍

### 5.1 无 async fn

蓝图 §8.5 明确指出 `async fn` 在 no_std trait 中不稳定（依赖 `async-trait` 宏或 nightly 特性），本版本所有 trait 方法为同步签名。

替代方案：

- **轮询接口**：`HalSerial::read` 返回已读字节数，调用方循环轮询
- **显式状态机**：复杂异步操作由调用方维护状态枚举，HAL 提供 `step()` 推进
- **中断驱动**：`HalIrq::register` 注册回调，中断触发时由 BSP 调用，天然异步

未来若 Rust 稳定 no_std async trait，可评估引入，但当前同步 + 中断回调已满足需求。

### 5.2 无 alloc 依赖

本 crate 不依赖 `alloc`，所有类型与 trait 方法避免堆分配：

- **`IrqHandler` 用函数指针**：`fn(irq: u32) -> IrqAction`，而非 `Box<dyn Fn(u32) -> IrqAction>`。函数指针是 `Copy` 的 `usize` 大小，无需堆
- **错误用枚举**：`HalError` 是固定大小枚举，非 `Box<dyn Error>`
- **无 `Vec`/`String`**：所有方法签名用切片 `&[u8]`/`&mut [u8]`，不返回集合

这使 crate 可在无堆环境（如内核早期启动、seL4 root task 初期）直接使用。

### 5.3 cfg_attr(not(test), no_std) 模式

```rust
#![cfg_attr(not(test), no_std)]
```

- **正式构建**（`cargo build --target aarch64-unknown-none`）：`no_std`，仅依赖 `core`
- **host 测试**（`cargo test`）：启用 `std`，用于 `format!`/`println!`/`assert_eq!` 的错误消息格式化
- **mock feature**：`#[cfg(feature = "mock")]` 的 mock 实现参与 host 测试，验证 trait 实现完整性

此模式与 v0.4.0 的 board/sel4-sys/runtime 一致，是 EnerOS 库 crate 的标准实践。

---

## 6. 与业界 HAL 设计对比

### 6.1 seL4 libplatsupport

seL4 的 libplatsupport 是 C 语言实现的 HAL 库：

- **语言**：C
- **抽象方式**：函数指针表 + 头文件声明
- **类型安全**：弱，无编译期实现完整性检查
- **文档化**：依赖外部注释，IDE 无法悬浮提示
- **no_std**：原生（C 无运行时）

对比：本设计用 Rust trait 获得编译期检查与文档化，代价是绑定 Rust 语言。

### 6.2 Linux HAL

Linux 内核的 HAL 是 C 语言的 `ops` 结构体模式：

- **语言**：C
- **抽象方式**：`struct file_operations`/`struct clk_ops` 等 ops 结构体，运行时填充
- **类型安全**：弱，函数签名靠头文件约束，易出错
- **可扩展**：新增硬件需修改 ops 表
- **绑定时机**：运行时（driver probe 时填充）

对比：本设计在编译期确定实现，无运行时绑定开销；trait 方法的文档注释随签名传递。

### 6.3 Rust embedded-hal

`embedded-hal` 是 Rust 嵌入式社区的 HAL trait 集合：

- **语言**：Rust
- **抽象方式**：trait（与本设计一致）
- **目标**：MCU（单片机，no OS）
- **async**：新版本支持 `async-trait`
- **单例**：通常由调用方持有 trait object，无全局单例

对比：本设计借鉴 embedded-hal 的 trait 思路，但面向 OS（EnerOS），增加 `HalProvider` 单例以简化上层调用。

### 6.4 本设计的优势

| 维度 | 本设计 | seL4 libplatsupport | Linux HAL | embedded-hal |
|------|--------|---------------------|-----------|--------------|
| 类型安全 | 编译期 | 弱 | 弱 | 编译期 |
| no_std | 是 | 原生 | 否（依赖内核） | 是 |
| BSP 可插拔 | 是 | 是 | 是 | 是 |
| dyn 安全 | 是（显式保证） | N/A | N/A | 是 |
| 单例注入 | 是 | 否 | 否 | 否 |
| 文档化 | trait 注释 | 外部 | 外部 | trait 注释 |
| OS 定位 | 是 | 是 | 是 | 否（MCU） |

本设计结合了 Rust trait 的类型安全与 OS 级 HAL 的单例注入，适合 EnerOS 的多硬件、no_std、OS 级定位。

---

## 7. 扩展路径

### 7.1 新硬件接入流程

接入新硬件（如飞腾 2000）的步骤：

1. **实现 6 个 trait**：为飞腾的 CPU/GICv3/Timer/UART/GPIO 分别 impl `HalCpu`/`HalIrq`/`HalClock`/`HalMem`/`HalSerial`/`HalGpio`
2. **实现 `HalProvider`**：聚合上述实现，返回 `&'static dyn` 引用
3. **编译期选择**：通过 `cfg` 标志或 Cargo feature 在 `Cargo.toml` 选择 BSP
4. **`init_hal` 注入**：在飞腾的 boot 早期调用 `init_hal(&PROVIDER)`
5. **验证**：`cargo build -p eneros-hal --target aarch64-unknown-none` 确认编译通过，QEMU 或真机验证运行时行为

整个过程无需修改 `eneros-hal` crate 本身，符合开闭原则。

### 7.2 未来 RISC-V BSP

trait 设计不绑定 ARM64 语义：

- `HalCpu::enable_irq`：ARM64 用 `MSR DAIFSet`，RISC-V 用 `csrs mstatus, MIE`
- `HalIrq`：ARM64 用 GICv3，RISC-V 用 PLIC
- `HalClock`：ARM64 用 Generic Timer，RISC-V 用 `rdtime` 指令
- `HalMem`：ARM64 用 EL2/EL1 页表，RISC-V 用 Sv39/Sv48 页表

RISC-V BSP 需实现等价语义，trait 接口不变。`set_domain` 在 RISC-V 无等价物，可返回 `NotSupported`。

### 7.3 飞腾/鲲鹏国产化支持

飞腾与鲲鹏均为 ARM64 架构，BSP 层处理差异：

- **GICv3 兼容**：飞腾 2000+/鲲鹏 920 均支持 GICv3，`HalIrq` 实现可复用
- **定时器**：均兼容 ARM64 Generic Timer，`HalClock` 实现可复用
- **UART**：飞腾用 PL011 兼容，鲲鹏用 PL011，`HalSerial` 可复用
- **GPIO**：各平台 GPIO 控制器不同，需独立实现 `HalGpio`
- **设备树解析**：BSP 层解析设备树（`.dtb`）确定外设基址与中断号，trait 接口不感知

国产化支持的难点在 BSP 层（设备树、SoC 特定寄存器），HAL trait 接口本身无需改动。

---

## 8. 风险与缓解

| 风险 | 等级 | 缓解措施 |
|------|------|----------|
| 接口过度设计 | 中 | 仅定义蓝图明确要求的方法，避免 speculative generality；v0.5.0 为纯设计版本，可在 v0.6.0 实现前调整 |
| trait object 限制 | 低 | 所有方法已确认 dyn 安全；若未来需泛型，按 §4.3 方案处理 |
| `static mut` 安全性 | 低 | unsafe 限于 init/hal 两处；write-once 语义；单线程 boot 上下文；文档标注契约 |
| async 需求 | 低 | 当前同步 + 中断回调满足需求；未来可评估 no_std async trait 稳定后引入 |
| 跨架构兼容 | 低 | trait 不绑定 ARM64 语义；RISC-V BSP 返回 `NotSupported` 处理无等价物的方法 |
| BSP 实现质量 | 中 | v0.6.0/v0.7.0 的 ARM64 实现需通过 QEMU 验证；mock 实现保证接口可编译 |

---

## 9. 结论

trait 抽象 + `HalProvider` 单例注入是 EnerOS HAL 的最优方案。它在类型安全、零成本抽象、BSP 可插拔、no_std 兼容、dyn 安全之间取得平衡，优于 C 风格的函数指针表/ops 结构体，也契合 OS 级 HAL 的单例需求。

v0.5.0 定义的 6 个 trait + `HalProvider` 注册器模式，为 v0.6.0/v0.7.0 的 ARM64 实现以及未来飞腾/鲲鹏/RISC-V 的 BSP 实现提供了稳定契约。本设计决策已通过蓝图 §4.5/§8 的合规性审查，并在 mock 实现中完成编译期接口验证。

---

> 本白皮书论证 v0.5.0 HAL 的设计决策。若架构演进导致 trait 变更，需同步更新本文档并升级版本号。
