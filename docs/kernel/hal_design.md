# EnerOS v0.143.0 HAL 接口与 ARM64 实现设计文档（Hal trait 十一方法 / Aarch64Hal 四元组装 / GenericTimer / Pl011 封装 / HalRegistry）

> 版本：v0.143.0
> 日期：2026-07-28
> 状态：已交付（host 全量测试零回归 + QEMU 冒烟首轮一次 PASS，无阈值调整）
> 适用范围：EnerOS 自研内核 HAL 硬件抽象子系统（Hal trait 十一方法接口规范 + HalRegistry 注册表 + platform::qemu_virt 平台配置常量组 + Aarch64Hal ARM64 实现；host 单测/集成测试 + QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` 冒烟 CNTVCT 实测）
> 蓝图依据：`蓝图/phase3.md` §v0.143.0（§1 版本目标 / §2 前置依赖 / §3 交付物清单 / §4 详细设计（§4.1 数据结构 / §4.2 接口定义 / §4.3 mermaid 初始化顺序 / §4.4 错误处理 / §4.5 关键代码）/ §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-E HAL 多平台线首版本**（v0.143.0 HAL 接口与 ARM64 实现 → v0.144.0 x86_64 BSP → v0.145.0 飞腾 → v0.146.0 鲲鹏 → v0.147.0 RISC-V/龙芯，五版之首），冻结 Hal trait 接口规范，为后续 x86/飞腾/鲲鹏/RISC-V/龙芯 BSP 提供接口规范（蓝图 §1 Phase 定位原样）
> 配套文档：[capability_design.md](./capability_design.md)（v0.142.0 能力管理器设计，前序版本）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 驱动与 install_vbar 设施来源）、[wcet_report.md](./wcet_report.md)（v0.138.0 wcet 时钟设施来源）、[../boot/qemu_run.md](../boot/qemu_run.md) §22（v0.143.0 QEMU 实测记录：首轮一次 PASS，无阈值调整）

---

## 1. 版本目标

**核心一句话**：复用 Phase 0 的 HAL 设计，适配自研内核，实现 ARM64 HAL 全功能（CPU/内存/中断/时钟/串口/GPIO）（蓝图 §v0.143.0 核心一句话原样）。

- **业务价值**：HAL 抽象让内核与硬件解耦，为多平台 BSP 奠定基础（蓝图 §1 原样）。
- **Phase 定位**：**P3-E 首版本**，为后续 x86/飞腾/鲲鹏/RISC-V/龙芯 BSP 提供接口规范（蓝图 §1 原样）；出口关联「自研内核 HAL 多平台能力基础」。
- **下游衔接（v0.144.0~v0.147.0 BSP）**：本版本冻结 `Hal` trait 十一方法签名 + `HalRegistry` 多态注册口径 + `platform::qemu_virt` 平台配置常量组三层结构；后续 BSP 版本（v0.144.0 x86_64 → v0.145.0 飞腾 → v0.146.0 鲲鹏 → v0.147.0 RISC-V/龙芯）**仅需新增同级 `platform` 模块（平台常量组）+ `Hal` trait 实现（如 `X86Hal`）**即可挂载新平台（蓝图 §9 可扩展「支持新平台」口径落地，见 §6.2）。
- **ADR-0001 裁剪口径**（蓝图 §1 引用段原样）：本版本原为自研内核组件，已按方案 A 调整为 seL4 深度定制/信创认证路径。原自研内核工作（语言重写、全量形式化验证、5平台BSP）已裁剪或合并。**HAL 接口设计保留（方案 A 仍需 HAL 抽象国产 CPU 差异），但实现从「适配自研内核」改为「适配 seL4 + 国产 CPU」**。
- **依赖关系**：前序 v0.142.0 能力管理可用（蓝图 §2 前序版本原样）；蓝图 §2 假设「ARM64 为基准平台」落地为 qemu_virt 平台常量组 + Aarch64Hal 实现；蓝图 §2 阻塞项「HAL 不完整阻塞 BSP」由本版本 trait 冻结 + ARM64 全功能落地解除。类型/设施复用既有 `interrupt::gicv3::Gicv3`（v0.133.0）/ `eneros_bootloader::{Pl011Uart, MmioRegIo, RegIo}`（v0.129.0）/ `sched::wcet::{read_counter_freq, read_cycle_counter, cycles_to_ns}`（v0.138.0）/ `interrupt::vector::install_vbar`（v0.133.0）。

## 2. 交付物清单

对照蓝图 §3 逐项标注偏差（D 编号与 [src/hal/mod.rs](../../crates/kernel/microkernel/src/hal/mod.rs) 头注 D1~D10 一一对应，全局表见 §12）：

| 蓝图 §3 交付物 | 实际落位 | 偏差 |
|---------------|---------|------|
| 代码：`kernel/src/hal/mod.rs`（HAL trait 定义） | [crates/kernel/microkernel/src/hal/mod.rs](../../crates/kernel/microkernel/src/hal/mod.rs)（494 行：Hal trait 十一方法 + HalRegistry 实例/全局双口径 + platform::qemu_virt 六常量 + D1~D10 头注 + 内存预算声明） | D1 路径归口；D2 init 六方法 Result 化；D10 全局单例语义 |
| 代码：`kernel/src/hal/aarch64.rs`（ARM64 实现） | [crates/kernel/microkernel/src/hal/aarch64.rs](../../crates/kernel/microkernel/src/hal/aarch64.rs)（562 行：Aarch64Hal\<B: RegIo = MmioRegIo\> 四元组装 + from_parts/new_system 双构造 + Hal 十一方法全量实现 + clock_disarm 固有方法 + A64H1~A64H10 单测） | D1 路径归口；D3~D9 |
| 代码：`kernel/src/hal/clock.rs` | [crates/kernel/microkernel/src/hal/clock.rs](../../crates/kernel/microkernel/src/hal/clock.rs)（292 行：GenericTimer 注入式时钟 + cycles_to_us 纯函数 + enable_masked/disarm + CLK1~CLK5 单测） | D1 路径归口；D4/D5/D9 |
| 代码：`kernel/src/hal/serial.rs`、`gpio.rs` | [crates/kernel/microkernel/src/hal/serial.rs](../../crates/kernel/microkernel/src/hal/serial.rs)（174 行：Pl011\<B: RegIo\> 泛型封装 + SER1~SER3 单测）、[crates/kernel/microkernel/src/hal/gpio.rs](../../crates/kernel/microkernel/src/hal/gpio.rs)（72 行：Gpio 占位 + GPIO1~GPIO2 单测） | D1 路径归口；D6/D9 |
| 接口类：`Hal` trait 十一方法 + `Aarch64Hal { gic, timer, uart }` | trait 十一方法签名原样（init 六方法按 D2 改返回 `Result<(), KernelError>`）；Aarch64Hal 三字段语义原样、具体类型全部复用既有设施（D6）+ gpio 第四字段（D9） | D2/D6/D9 |
| 接口类：`HalRegistry { hal: &'static dyn Hal }` | 蓝图 §4.1 原样 + 全局 `register_hal`/`hal()` 自由函数（D10 一次性注册语义） | D10 |
| 文档：`docs/kernel/hal_design.md` | 本文档 | 无（路径同蓝图） |
| 测试：`kernel/tests/hal_test.rs`（ARM64 HAL 全功能） | [crates/kernel/microkernel/tests/hal_test.rs](../../crates/kernel/microkernel/tests/hal_test.rs)（516 行：HPI1~HPI8，HPI8 `#[ignore]` 故障注入） | D1 路径归口 |
| 测试：lib 内嵌单测 | HAL1~HAL6（mod.rs）/ CLK1~CLK5（clock.rs）/ SER1~SER3（serial.rs）/ GPIO1~GPIO2（gpio.rs）/ A64H1~A64H10（aarch64.rs）（本版本新增 26 例） | 蓝图 §6 单元测试落点 |
| 测试：QEMU 冒烟 | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `hal_smoke` 段（§3.17：BSS 有效化 → 六 init 全链路 → HAL serial_putc 自证 → 时钟单调 → deadline 5s 编程/回读/disarm → SPI 36 enable/disable GICD 回读 → 100 轮 clock_now_us CNTVCT 夹取 → 判定打印） | 蓝图 §6 集成测试的目标机落点 |
| 配置类：QEMU virt 平台配置 | [mod.rs](../../crates/kernel/microkernel/src/hal/mod.rs) `platform::qemu_virt` 六常量（UART_PL011_BASE / GICD_BASE / GICR_BASE / GPIO_PL061_BASE / CNTP_IRQ / CNTV_IRQ） | 无（原样值，见 §6.1） |
| 回归断言：QEMU 关键串 | [tests/boot_test.sh](../../tests/boot_test.sh) 第十六条 `grep -q "\[hal\] ok"`（既有十五条原样） | 无（surgical 追加同 v0.140.0~v0.142.0 先例） |

**spec 分类口径对照**（spec.md「What Changes」）：接口类（Hal trait / HalRegistry）→ §3/§7；配置类（QEMU virt 平台配置）→ §6；实现类（Aarch64Hal / GenericTimer）→ §4/§5；文档类（hal_design.md）→ 本文档。

## 3. Hal trait 设计

### 3.1 十一方法逐项（蓝图 §3 接口类原样签名 + D2 修正）

| # | 方法签名（落地） | 蓝图 §3 原样签名 | 语义（蓝图锚点） | 落地决策 |
|---|----------------|----------------|----------------|---------|
| 1 | `fn cpu_init(&mut self) -> Result<(), KernelError>` | `fn cpu_init(&mut self)` | CPU 初始化：装载 VBAR_EL1 异常向量基址（§4.5 伪代码「设置 VBAR」） | D2 Result 化；D3 复用 `install_vbar` 幂等重装 |
| 2 | `fn mem_init(&mut self) -> Result<(), KernelError>` | `fn mem_init(&mut self)` | 内存初始化（§4.5 注释「MMU 在 v0.131.0 已完成」） | D2 Result 化；D9 空操作恒 Ok |
| 3 | `fn irq_init(&mut self) -> Result<(), KernelError>` | `fn irq_init(&mut self)` | 中断初始化：GIC 初始化（§4.5 `self.gic.init()`） | D2 Result 化；委托 `Gicv3::init`（v0.133.0 序列） |
| 4 | `fn clock_init(&mut self) -> Result<(), KernelError>` | `fn clock_init(&mut self)` | 时钟初始化：使能虚拟定时器（§4.5 伪代码） | D2 Result 化；D4 CNTV_CTL_EL0 = ENABLE\|IMASK |
| 5 | `fn serial_init(&mut self) -> Result<(), KernelError>` | `fn serial_init(&mut self)` | 串口初始化：PL011 UART 初始化（§4.2） | D2 Result 化；委托 `Pl011Uart::init`（幂等） |
| 6 | `fn gpio_init(&mut self) -> Result<(), KernelError>` | `fn gpio_init(&mut self)` | GPIO 初始化：平台相关（§4.5 注释） | D2 Result 化；D9 占位恒 Ok |
| 7 | `fn serial_putc(&self, ch: u8)` | 同左 | 串口输出单字节：PL011 输出（§4.2 原样） | 原样；`&self` 接收者（轮询写 DR 无内部可变状态） |
| 8 | `fn clock_now_us(&self) -> u64` | 同左 | 当前时钟（微秒）：读 CNTVCT_EL0 转微秒（§4.2/§4.5 公式） | 签名原样；D5 频率与计数源构造注入 |
| 9 | `fn clock_deadline(&mut self, us: u64)` | 同左 | 编程定时器 deadline（微秒）：换算 ticks 写 CNTV_TVAL_EL0（§4.5 公式） | 签名原样；配套 `clock_disarm` 为实现类型固有方法（D9，非 trait 项） |
| 10 | `fn enable_irq(&mut self, irq: u32, cpu: u8)` | 同左 | 使能中断：GIC 使能指定 IRQ 至目标 CPU（§4.5） | 签名原样；D7 内部固定 `IrqTrigger::Edge`；D8 `irq ≥ 1024` 防御性 no-op |
| 11 | `fn disable_irq(&mut self, irq: u32)` | 同左 | 禁能中断：GIC 禁能指定 IRQ（§4.5） | 签名原样；D8 `irq ≥ 1024` 防御性 no-op |

### 3.2 D2：init 六方法 Result 化（蓝图 §4.4「串口打印错误 + panic」修正）

- **蓝图口径**：§4.4 错误处理「初始化失败：串口打印错误 + panic」；trait init 六方法返回 `()`。
- **落地决策**：init 六方法（cpu/mem/irq/clock/serial/gpio）返回 `Result<(), KernelError>`。
- **理由**：仓库**禁 panic! 规则**（v0.127.0 D4 先例链：panic 语义改短路错误传播，ipc/cap/shm 各版本同口径）；蓝图 §9 可靠性「init 失败 panic」以**错误码短路 + 冒烟 `[hal] err code=<n>` 分支不挂死**等价落地——失败路径同样可观测（串口输出错误码与 KernelError 判别值 aux）、同样不继续推进，语义等价且满足 no_std 内核禁 panic 硬约束。
- **运行时五方法**（serial_putc/clock_now_us/clock_deadline/enable_irq/disable_irq）保持蓝图原样返回类型：均为无失败路径的直线操作（轮询写寄存器/读计数器换算/防御性 no-op），无需 Result。

### 3.3 trait object 开销处置（蓝图 §8 风险）

蓝图 §8 风险「技术：trait object 开销」。处置口径：**内核热点路径以静态分发为主**（hal_smoke 段经 `Aarch64Hal` 具体类型直接调用，编译期单态化零间接），**`HalRegistry` 全局单点为 dyn 分发**（`&'static dyn Hal` 胖指针，一次间接跳转）——dyn 分发仅出现在「按平台取 HAL 实例」的注册表路径，不进入中断/调度热点；HPI7 以 1000 轮 `clock_now_us` 记录分发延迟量级（见 §9.1），cap CPI8 先例同口径。

## 4. ARM64 实现（Aarch64Hal）

### 4.1 四元组装（蓝图 §3 三字段 + D9 gpio 字段）

```rust
pub struct Aarch64Hal<B: RegIo = MmioRegIo> {
    gic: Gicv3,            // v0.133.0 既有驱动原样复用（D6）
    timer: GenericTimer,   // D5 注入式时钟（aarch64 = CNTVCT/CNTFRQ）
    uart: Pl011<B>,        // D6 泛型：aarch64 = MmioRegIo，host = 缓冲假后端
    gpio: Gpio,            // D9 占位（引脚操作集留待 BSP 版本）
}
```

- **泛型口径**：`B: RegIo = MmioRegIo`——aarch64 真机经 `new_system()` 以 qemu_virt 平台常量组构造（GICD/GICR/UART/GPIO 基址 + CNTFRQ/CNTVCT 定时器设施）；host 单测经 `from_parts(gic, timer, uart, gpio)` 注入式构造（伪 MMIO GICD/GICR 对齐帧 + 缓冲假 UART + mock 计数源，构造期零硬件副作用）。
- **观测口**：`gic()` / `timer()` / `uart()` / `gpio()` 四个只读观测口供测试寄存器级回读与状态断言。
- **`clock_disarm(&mut self)`**：Aarch64Hal **固有方法**（非 trait 项，D9），委托 `GenericTimer::disarm`（CNTV_CTL 写 0，冒烟收尾不投递中断）。

### 4.2 D3~D9 逐项（ARM64 实现侧）

| 编号 | 落点 | 内容 |
|------|------|------|
| D3 | `cpu_init` | 蓝图伪代码 `exception_vector_base()`（该函数不存在且伪代码括号不编译）→ 复用既有 `interrupt::vector::install_vbar` 幂等重装 VBAR_EL1（v0.133.0 已交付并经 QEMU 实证，其 Safety 文档明示「重复调用无害」）；host 无异常向量设施为空操作 |
| D4 | `clock_init` | 蓝图伪代码 `msr cntvoff_el2, xzr`（EL2 寄存器）→ 删除该写；委托 `GenericTimer::enable_masked`——CNTV_CTL_EL0 = ENABLE(bit0)=1 \| IMASK(bit1)=1（0b11），使能且屏蔽防 stray IRQ（详见 §5.3） |
| D5 | `clock_now_us` / `clock_deadline` | 蓝图直接读 CNTVCT → GenericTimer 构造注入 `freq: u64` + 计数源 `now_cycles: fn() -> u64`；host 可测化（cap manager D5 时钟注入先例），CNTVCT 读为 aarch64 门控 |
| D6 | 三字段具体类型 | 零重复实现（Simplicity First）：gic = `interrupt::gicv3::Gicv3`（base 参数化，host 伪 MMIO 可注入）、uart = `hal::serial::Pl011<B: RegIo>` 泛型封装 `eneros_bootloader::Pl011Uart<B>`、timer = `hal::clock::GenericTimer`——Gicv3/Pl011Uart 均已经 QEMU + host 双测实证 |
| D7 | `enable_irq` | 签名无 trigger 参数原样，内部固定 `IrqTrigger::Edge`（蓝图 §4.5 伪代码原样）；触发方式可配置仍由 interrupt:: 层承担，BSP 版本按需扩展 HAL 签名属后续（v0.144~v0.147） |
| D8 | `enable_irq` / `disable_irq` | `irq ≥ 1024`（GICv3 INTID 上限）防御性 no-op——GICv3 层已有同口径防御（IRQ_LIMIT=1024），HAL 层守卫按 spec Scenario「寄存器帧零写入」显式落地（双保险口径）；不引入新错误变体 |
| D9 | `mem_init` / `gpio_init` / `clock_disarm` | mem_init = Ok(()) 空操作（蓝图 §4.5 原样「MMU 在 v0.131.0 已完成」）；gpio_init = 占位 Ok(())，PL061 基址 0x0903_0000 录入平台配置（GPIO 操作集留待 BSP 版本）；clock_deadline 编程后由 D4 IMASK 兜底不投递，冒烟末尾 `clock_disarm`（CNTV_CTL ENABLE=0，固有方法非 trait）收尾 |

### 4.3 复用关系（文字化依赖图）

```
Aarch64Hal（本版本，hal/aarch64.rs）
├─ gic: interrupt::gicv3::Gicv3 ............ v0.133.0 既有驱动原样复用
│   └─ 内联 IrqHandlerTable（interrupt::handler，v0.133.0）：
│      handlers [Option<IrqHandler>; 1024] 8KB + counts [u64; 1024] 8KB = 16KB
├─ timer: hal::clock::GenericTimer ......... 本版本新增（D5 注入式）
│   └─ aarch64 计数源/频率 = sched::wcet::{read_cycle_counter（CNTVCT_EL0）,
│      read_counter_freq（CNTFRQ_EL0）} .... v0.138.0 wcet 设施原样复用
├─ uart: hal::serial::Pl011<MmioRegIo> ..... 本版本新增泛型封装（D6）
│   └─ 封装 eneros_bootloader::Pl011Uart<B: RegIo> ... v0.129.0 bootloader 既有驱动
├─ gpio: hal::gpio::Gpio ................... 本版本新增占位（D9）
└─ cpu_init → interrupt::vector::install_vbar（VBAR_EL1 装载，幂等重装无害）
                                             v0.133.0 既有设施复用（D3）
```

零重复实现（Simplicity First）：全版本**零新外部依赖**；Gicv3/Pl011Uart/wcet/install_vbar 四处复用件均已经 QEMU + host 双测实证。

### 4.4 内存预算（仓库 §5.6 合规声明，spec 修正后口径）

| 组件 | 尺寸 | 说明 |
|------|------|------|
| `Aarch64Hal` | **≈16.4KB** | 内联 `Gicv3.table: IrqHandlerTable`（1024×Option\<fn\> 8KB + 1024×u64 计数 8KB，v0.133.0 既有结构原样复用）+ 基址字段 16B + `GenericTimer` 26B + `Pl011` 封装 + `Gpio` 8B |
| `HalRegistry` 全局槽 | ≈24B | `&'static dyn Hal` 胖指针 16B + AtomicBool + 对齐 |
| hal 子系统 BSS 总计 | **< 20KB** | 全零初始化落 BSS NOBITS 不占镜像（对照 IpcManager ≈1.6MB / CapabilityManagerImpl ≈10MB 先例，本版本无 BSS 压力） |

> 预算口径修正链：蓝图 §4.1 三结构（HalRegistry/GenericTimer\{freq\}/Pl011\{base\}）直观估算仅**数十字节** → Gicv3 内联 IrqHandlerTable 16KB 后修正为 **≈16.4KB**；spec/tasks 早期文本「BSS < 1KB」为过期口径，以 spec.md 内存预算节与本表为准（mod.rs 头注记录；GenericTimer 由 16B 修正为 26B，见 §12 落地修正④）。

## 5. 时钟子系统（GenericTimer）

### 5.1 CNTVCT_EL0 / CNTFRQ_EL0 口径

- **计数源**：CNTVCT_EL0（虚拟计数器，aarch64 真机经 `sched::wcet::read_cycle_counter` 读取，v0.138.0 设施复用）；**频率源**：CNTFRQ_EL0（计数频率 Hz，经 `sched::wcet::read_counter_freq` 读取）。QEMU virt 典型频率 **62_500_000 Hz（62.5MHz）**。
- **注入式构造（D5）**：`GenericTimer { freq: u64, now_cycles: fn() -> u64 }`——aarch64 = CNTVCT/CNTFRQ 真机设施（`new_system()`），host = mock 计数源（host 单测不触内联汇编；cap manager D5 时钟注入先例）。

### 5.2 换算公式（蓝图 §4.5 原样 + 饱和防溢出）

| 方向 | 公式 | 落地 |
|------|------|------|
| 计数 → 微秒 | `us = cycles × 1_000_000 / freq` | `cycles_to_us(cycles, freq)` 独立纯函数：`cycles.saturating_mul(1_000_000) / freq`（saturating 防 u64 溢出；`freq == 0` 除零防御返回 0 不 panic，参照 wcet.rs cycles_to_ns 口径） |
| 微秒 → ticks | `ticks = us × freq / 1_000_000` | `deadline_us(us)`：`us.saturating_mul(freq) / 1_000_000`，aarch64 写 CNTV_TVAL_EL0（相对偏移编程），host 记录 `last_deadline_ticks` 供断言 |

精度锚定：freq = 62_500_000、注入计数源返回 62_500_000 时 `now_us()` = 1_000_000（整秒无精度丢失）；`cycles_to_us(125_000_000, 62_500_000)` = 2_000_000；整除边界 62_499_999 → 999_999（向下取整）。

### 5.3 D4：EL2 矛盾化解与 IMASK-disarm 防护链

- **蓝图矛盾**：§4.5 伪代码 `msr cntvoff_el2, xzr`——内核运行于 **EL1**，写 EL2 寄存器为 **UNDEFINED 异常**（伪代码不可执行）。
- **化解**：删除该写；CNTVOFF 信任启动环境已清零（既有 wcet/cap CNTVCT 路径已在 QEMU 实证偏移为零——若偏移非零，v0.138.0~v0.142.0 的 CNTVCT 计时链路早已暴露）。
- **防护链（enable_masked → 不投递 → disarm 收尾）**：
  1. `clock_init` → `enable_masked`：CNTV_CTL_EL0 = **ENABLE(bit0)=1 | IMASK(bit1)=1（0b11）**——定时器使能但中断屏蔽，deadline 到期**不投递** IRQ（防 stray IRQ：本版本无定时器中断 handler 接入需求，冒烟仅验证编程/回读通路）；
  2. `clock_deadline(us)`：编程 CNTV_TVAL_EL0 相对偏移（IMASK=1 兜底不投递）；
  3. `clock_disarm()`（固有方法，D9）：CNTV_CTL_EL0 写 **0**（ENABLE=0 撤销定时器，同清 IMASK 位）——冒烟收尾不遗留未到期定时器。

### 5.4 CNTV_TVAL 有符号 32bit 上限与冒烟 5s 口径

- CNTV_TVAL_EL0 为**有符号 32bit 相对偏移寄存器**（bits[31:0] 有符号语义、bits[63:32] RES0）：62.5MHz 下编程上限 = 2^31 / 62.5e6 ≈ **34s**；若按 60s 量级编程将溢出回绕为负值。
- 冒烟 deadline 取 **5s = 5_000_000us → 312_500_000 ticks ≪ 2^31（2,147,483,648）**——裕量充足，编程后立即回读必为正（QEMU TCG 下 5s 裕量远超编程-回读间耗时）；回读非负判定取 bits[31:0] 有符号解释（bit31 清零即非负，main.rs `read_cntv_tval` 为本文件唯一读取点）。

## 6. 平台配置（platform::qemu_virt）

### 6.1 六常量表（蓝图 §3 配置类原样值）

| 常量 | 值 | 语义 | QEMU virt 地址图依据 |
|------|-----|------|---------------------|
| `UART_PL011_BASE` | `0x0900_0000` | PL011 UART0 基址 | QEMU virt machine UART0 映射地址 |
| `GICD_BASE` | `0x0800_0000` | GICv3 分发器（GICD）基址 | QEMU virt GICv3 Distributor 映射地址 |
| `GICR_BASE` | `0x080A_0000` | GICv3 再分发器（GICR）基址 | QEMU virt GICv3 Redistributor 映射地址 |
| `GPIO_PL061_BASE` | `0x0903_0000` | PL061 GPIO 基址 | QEMU virt PL061 映射地址（引脚操作集留待 BSP 版本，D9） |
| `CNTP_IRQ` | `30` | ARM 通用定时器 CNTP PPI 中断号 | 物理定时器 PPI，INTID 30，ARM 架构固定 |
| `CNTV_IRQ` | `27` | ARM 虚拟定时器 CNTV PPI 中断号 | 虚拟定时器 PPI，INTID 27，ARM 架构固定 |

锚定测试：HAL1 六常量逐一 `assert_eq`（蓝图 §3 配置类「QEMU virt 平台配置」原样值）。

### 6.2 BSP 演进路线（蓝图 §9 可扩展「支持新平台」）

**新平台 = 新 platform 模块 + Hal trait 实现**：

```
hal/mod.rs::platform::qemu_virt（本版本）  →  v0.144.0 起按平台新增同级模块：
    platform::x86_64 / platform::phytium / platform::kunpeng / ...
hal/aarch64.rs::Aarch64Hal（本版本）       →  v0.144.0 X86Hal / 后续飞腾/鲲鹏/RISC-V/龙芯 Hal
```

- trait 签名冻结（十一方法，§3.1）；Registry 多态注册口径不变（§7）；平台差异全部收敛于 platform 常量组与 Hal 实现两文件。
- 演进预留：D7 触发方式可配置（enable_irq 增 trigger 参数）、D9 PL061 引脚操作集（GPIODIR/GPIODATA 屏蔽寻址/GPIOIS/GPIOIBE/GPIOIEV/GPIOIE/GPIOIC）留待 v0.145.0/v0.146.0 BSP 版本按需扩展；GPIO 基址字段与构造签名保持不变（gpio.rs 头注演进路线原样）。

## 7. HalRegistry（D10 单例语义）

### 7.1 双口径：实例 + 全局

| 口径 | API | 语义 |
|------|-----|------|
| 实例（蓝图 §4.1 原样） | `HalRegistry::new(hal: &'static dyn Hal)`（const fn）/ `.hal()` | 持有平台 HAL 实例胖指针的注册表结构体，构造期可用（静态初始化期） |
| 全局（D10 扩展） | `register_hal(hal: &'static dyn Hal) -> Result<(), KernelError>` / `hal() -> Result<&'static dyn Hal, KernelError>` | 进程级全局单例：启动期单线程一次性注册；`hal()` 取用全局实例 |

### 7.2 D10 单例语义（spin::Once 等价手写，零新依赖）

- **存储**：`static HAL_CELL: HalCell(UnsafeCell<MaybeUninit<&'static dyn Hal>>)` 驻留 BSS（UnsafeCell 包装规避 edition 2024 `static_mut_refs` deny；同 main.rs PmmCell / interrupt::vector GicCell 口径）+ `static HAL_REGISTERED: AtomicBool` 注册旗标。
- **发布协议**：`register_hal` 先 Acquire 检查旗标（已注册 → `BadState` 且不覆盖）→ **先写槽**（`MaybeUninit::write`）→ **Release 置旗**发布；`hal()` **Acquire 读旗**（未注册 → `BadState`）→ 旗标为真 ⇒ 与 Release 写配对，槽位已初始化，拷贝出胖指针。Release/Acquire 配对保证槽位写入对读者可见。
- **不变式**：启动期单线程注册语义（v0.127.0 D3 先例）——HAL_REGISTERED 守卫保证写槽路径全程序仅到达一次；注册后槽位只读，无数据竞争；无堆无锁。
- **错误变体**：复用既有 `KernelError::BadState`（判别值 **7**，v0.127.0 起既有），**零新增错误变体**（HAL5 锚定 `KernelError::BadState as u16 == 7`）。
- **dyn 分发单点**：全局槽为全内核唯一 `&'static dyn Hal` 长期持有点（蓝图 §8 风险「trait object 开销」处置，见 §3.3）。

## 8. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`：**lib 202 passed / 0 failed**（含本版本新增 HAL1~6/CLK1~5/SER1~3/GPIO1~2/A64H1~10 共 26 例，基线 176 + 26）+ 集成 16 套件 **102 passed / 7 ignored** 零回归（lib + 集成合计 **304 passed / 7 ignored**）；hal_test 单套件 **7 passed / 1 ignored**，`-- --ignored` 独立运行 HPI8 **1 passed**（2026-07-28 本机实测复核）。

### 8.1 单元测试（src/hal 内嵌，新增 26 个）

| 编号 | 用例位置 | 验证点 | 蓝图锚点 |
|------|---------|--------|---------|
| HAL1 | mod.rs | qemu_virt 六常量逐一 assert_eq 锚定（0x0900_0000/0x0800_0000/0x080A_0000/0x0903_0000/30/27） | §3 配置类 |
| HAL2 | mod.rs | HalRegistry 实例 new/hal() 返回同一对象（辨识值 0x1430 + 指针同一性双重核验） | §4.1 |
| HAL3 | mod.rs | 经 `&mut dyn Hal` 调用全部十一方法，RecordingMock 副作用参数逐一核验（putc/deadline/enable/disable） | §6 集成/§7 出口 |
| HAL4 | mod.rs | 全局注册表全生命周期：未注册 BadState → 注册 Ok → 取用同一（111）→ 重复注册 BadState 不覆盖（**唯一触碰全局状态的 lib 测试**） | D10 |
| HAL5 | mod.rs | `KernelError::BadState as u16 == 7` 判别值锚定（复用既有变体、零新增错误变体的证据） | D10 |
| HAL6 | mod.rs | 注册前经 `&mut` 调用三可变方法（Box::leak 同一实例，记录落静态原子）+ 注册后经 registry 调用 serial_putc，四方法参数核验 | D10/落地修正① |
| CLK1 | clock.rs | cycles_to_us 换算精度：整除无失（2s⇔2_000_000us）/向下取整（999_999）/除零防御返 0/饱和防溢出（u64::MAX 不回绕） | §4.5 公式 |
| CLK2 | clock.rs | 注入计数源 now_us 换算：freq=62_500_000、mock 返回 62_500_000 ⇒ 1_000_000 | spec Scenario 原样 |
| CLK3 | clock.rs | 单调性：递增注入计数源双读第二次 ≥ 第一次 | §6 测试计划「时钟读取」 |
| CLK4 | clock.rs | deadline 公式：deadline_us(5_000_000) ⇒ last_deadline_ticks == 312_500_000 | §4.5 公式 |
| CLK5 | clock.rs | enable_masked/disarm host 语义：armed/masked 状态记录（使能且屏蔽 → 同真；disarm → 同假） | D4/D9 |
| SER1 | serial.rs | init 幂等：双次 init 帧回读 CR（0x30）== 0x301、LCR_H（0x2C）== 0x70（同值重写天然幂等） | §4.2 |
| SER2 | serial.rs | putc 落 DR：init 后 putc(b'A')，帧偏移 0x00（UARTDR）回读 == 0x41 | §4.2/spec Scenario |
| SER3 | serial.rs | 连续字节序：putc b'H'/b'A'/b'L'，发送日志 == b"HAL" | spec Scenario |
| GPIO1 | gpio.rs | 占位初始化：Gpio::new(GPIO_PL061_BASE).init() 恒 Ok | D9 |
| GPIO2 | gpio.rs | 基址锚定：base() == 0x0903_0000（QEMU virt 地址图原样值） | §3 配置类 |
| A64H1 | aarch64.rs | 六 init 蓝图 §4.3 mermaid 顺序全链路全 Ok（cpu→mem→irq→clock→serial→gpio） | §4.3/§7 功能 |
| A64H2 | aarch64.rs | irq_init GIC 寄存器级回读：GICD_CTLR==1（EnableGrp1）/ IGROUPR[1..32] 全 0xFFFF_FFFF / GICR_WAKER==0 | §6 集成 |
| A64H3 | aarch64.rs | serial_init 帧回读（CR=0x301/LCR_H=0x70）+ serial_putc tx_log==b"HAL" + DR 回读末字节 | §6 单元「串口输出」 |
| A64H4 | aarch64.rs | clock_now_us 注入换算 == 1_000_000 + 递增源双读单调 | §6 单元「时钟读取」 |
| A64H5 | aarch64.rs | deadline 公式 312_500_000 + clock_disarm 固有方法同清 armed/masked（D9 非 trait 项） | §4.5/D9 |
| A64H6 | aarch64.rs | clock_init ENABLE\|IMASK host 语义（初始双假 → init 后双真） | D4 |
| A64H7 | aarch64.rs | enable_irq(36,0)：ISENABLER1 bit4 置位 + IROUTER36==0（aff0=cpu0）+ ICFGR2 字段 == 0b10<<8（D7 Edge）；disable_irq(36)：ICENABLER1 写观测 + 使能态合成 bit4 清零 | D7/§6 集成 |
| A64H8 | aarch64.rs | IRQ 边界防御：enable(1024)/enable(u32::MAX)/disable(2000) 均 no-op，GICD/GICR 帧逐字节零改动 | D8/spec Scenario |
| A64H9 | aarch64.rs | cpu/mem/gpio init 幂等双调（D3 install_vbar 幂等无害 + D9 空操作 + gpio 占位） | D3/D9 |
| A64H10 | aarch64.rs | `&mut dyn Hal` 十一方法多态分发全核验（六 init 全 Ok + 运行时五方法副作用逐一回读） | §7 出口/§8 风险 |

### 8.2 集成测试（tests/hal_test.rs，HPI1~HPI8，仅以公开 API 端到端验证）

| 编号 | 用例 | 蓝图对照 | 验证点 | 结果 |
|------|------|---------|--------|------|
| HPI1 | `hpi1_trait_object_dispatch_all_eleven_methods` | §6 集成 + §7 出口 + §8 风险 | 经 `&mut dyn Hal` 调用全部十一方法；副作用核验（tx_log/deadline ticks/armed+masked/ISENABLER1+ICENABLER1 写观测） | PASS |
| HPI2 | `hpi2_six_inits_full_chain_in_blueprint_order` | §4.3 mermaid 顺序 + §7 功能 | cpu→mem→irq→clock→serial→gpio 全 Ok；GICD_CTLR==1 + UART CR=0x301/LCR_H=0x70 寄存器级回读 | PASS |
| HPI3 | `hpi3_serial_fake_backend_byte_order` | §6 单元「串口输出」+ §9 可观测 | serial_init 后连续 putc b'H'/b'A'/b'L'，tx_log == b"HAL"，DR 回读末字节 b'L' | PASS |
| HPI4 | `hpi4_clock_precision_and_monotonic` | §6 单元「时钟读取」 | 整秒注入 == 1_000_000us；cycles_to_us(125_000_000, 62_500_000) == 2_000_000；递增源双读单调 | PASS |
| HPI5 | `hpi5_enable_disable_irq_readback_and_boundary` | §6 集成 + D7/D8 | (a) enable_irq(36,0)：ISENABLER1 bit4 置位 + IROUTER36==0 + ICFGR[2]（0xC00+2×4）irq36 的 2bit 字段 == 0b10<<8（下标 36/16=2、shift=(36%16)×2=8）；(b) disable_irq(36)：使能态合成 bit4 清零；(c) enable(1024)/disable(2000) 帧零写入 | PASS |
| HPI6 | `hpi6_global_registry_register_get_duplicate_reject` | §7 出口 Registry 多态 + D10 | 未注册 BadState → 注册 Ok → 取用同一（0x143 + ptr::eq）→ 重复注册 BadState 不覆盖（**默认运行中唯一触碰全局注册表的用例**） | PASS |
| HPI7 | `hpi7_clock_now_us_performance_record` | §6 性能 + §8 风险 | std::time::Instant 夹取 1000 轮 clock_now_us，**eprintln 记录口径不设硬断言**（`-- --nocapture` 可见；cap CPI8 先例；真机判定由 QEMU 冒烟承载）；功能复核末读 ≥ 首读 | PASS（记录见 §9.1） |
| HPI8 | `hpi8_fault_injection`（`#[ignore]`） | §6 故障注入 | (a) 非法 IRQ 连发：enable(1024..1056)/disable(2000..2056) 全 no-op 帧零改动；(b) 未注册全局取用 BadState | ignore（口径内） |

> **HPI8 独立运行前提**：(b) 的「未注册」判定依赖本进程注册表从未被写入——`cargo test -p eneros-microkernel --test hal_test -- --ignored` 独立运行时仅本用例执行（HPI6 不运行），前提成立（实测 1 passed）；`--include-ignored` 混合运行时 HPI6 与本用例同进程并发、注册表可能已被写入，前提不成立，属不支持用法（hal_test.rs 头注「全局状态注记」节原样口径）。

### 8.3 QEMU 冒烟（main.rs §3.17 `hal_smoke` 段 + [qemu_run.md](../boot/qemu_run.md) §22，原样引用）

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **十六**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok` / `[sync] ok` / `[wcet] ok` / `[ipc] ok` / `[notify] ok` / `[shm] ok` / `[cap] ok` / **`[hal] ok`**；v0.143.0 起追加第十六条断言 `grep "[hal] ok"`，既有十五条原样） |
| 串口关键行 | **`[hal] ok avg=190 max=5808`**（PASS 轮原样收录，16/16 断言通过） |
| 轮次 | 首轮一次 PASS，无排障记录，无阈值调整（未触发放宽路径，`HAL_CLK_LAT_MAX_NS` 零调整） |
| 冒烟流程 | ① HAL_INSTANCE 整体 write 有效化（`new_system()` 以 qemu_virt 常量组构造）→ ② 六 init 全链路（cpu→mem→irq→clock→serial→gpio，蓝图 §4.3 mermaid 顺序原样，任一失败错误码 1~6 短路、aux=KernelError 判别值）→ ③ HAL 自身 serial_putc 逐字节输出 `"[hal] "` 行首标记（自证串口通路，蓝图 §9 可观测「串口+时钟」）→ ④ clock_now_us 双读单调性（t1 ≥ t0）→ ⑤ deadline 相对 5s 编程（312_500_000 ticks）+ CNTV_TVAL 回读非负 + clock_disarm 收尾（D9 不投递）→ ⑥ enable_irq(36,0) GICD_ISENABLER1（GICD_BASE+0x104）bit4 回读置位 + disable_irq(36) 回读清零 → ⑦ CNTVCT 夹取 100 轮 clock_now_us 计时（cycles→ns 复用 v0.138.0 wcet 设施；freq/10=100ms 全程兜底超时不挂死）→ ⑧ 功能全对（err_code==0）且 max < HAL_CLK_LAT_MAX_NS 打印 `[hal] ok avg=<ns> max=<ns>`，失败 `[hal] err code=/round=/aux=0x/max=` 不挂死（十类错误码语义见 main.rs hal_smoke 段文档注释） |
| 实测环境 | 2026-07-28，WSL2 Ubuntu-22.04 + QEMU 8.2.2 + nightly-2026-04-04，`-M virt,gic-version=3 -smp 4`；端到端 18s（构建 7.08s + QEMU timeout 10s 守护窗口） |
| 双镜像 ELF | bootloader **70,440 B** / kernel **152,200 B**（kernel 较 v0.142.0 148,104 B 增 4,096 B 恰一页 .text 增长；Aarch64Hal ≈16.4KB 经 BSS MaybeUninit 驻留 NOBITS 不占镜像；远低于 512KB 预算） |
| 回归 | host 全量 lib 202 passed + 集成 16 套件 102 passed / 7 ignored 零回归；与既有十五类输出正交（IRQ 36 使能即禁、deadline disarm 收尾、不触碰 SGI0=IPI/SGI1=sync 既有通道与 ARINC/IPC/Notification/SHM/Cap 设施，PMM 零消耗） |

## 9. 性能实测

### 9.1 实测数据汇总（三口径）

| 平台 | 口径 | 指标 | 实测值 | 来源 |
|------|------|------|--------|------|
| host（Windows，x86_64，debug） | std Instant + 单调注入计数源 | 1000 轮 clock_now_us 延迟 avg / max | **avg 35 ns/轮、max 100 ns/轮** | HPI7（2026-07-28 本轮 `-- --nocapture` eprintln 记录口径，不设硬断言；host 注入口径仅记录量级，逐轮数值随机器负载波动） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 100 轮 clock_now_us 延迟 avg | **190 ns** | `[hal] ok avg=190 max=5808`（qemu_run.md §22 PASS 轮原样收录） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 100 轮 clock_now_us 延迟 max | **5,808 ns ≈ 5.8µs** | 同上（单样本尖峰系 TCG 翻译块冷启动/仿真抖动，avg/max 相差 ≈31×，形态同 v0.139.0~v0.142.0 先例） |
| 真机（飞腾/鲲鹏） | CNTVCT 同口径 | clock < 100ns（蓝图 §6/§7） | —（未测） | **留待硬件验证**（见 §10 阈值对照） |

### 9.2 蓝图 §6/§7 clock<100ns 真机目标对照（QEMU TCG 1.5ms 放宽依据）

蓝图 §6/§7「clock_now < 100ns / clock < 100ns」为**真机目标**（硬件计数器直读口径）；QEMU 侧放宽至 `HAL_CLK_LAT_MAX_NS` = **1,500,000 ns（1.5ms）** 的依据（qemu_run.md §22 决策记录原样口径）：(a) WSL2 下 qemu-system-aarch64 为 **TCG 纯软件仿真**（无 aarch64 KVM 加速），指令翻译使每轮 clock_now_us 全路径墙钟代价远高于真机；(b) **CNTVCT 虚拟计数器粒度/递送开销**（本轮 wcet 上下文代理 ctx=100ns 即纯计数器读 + 稳态路径的 QEMU 下界，v0.138.0 已记录同类坑点；本轮 avg=190ns 已逼近「计数器读×2 + ×1e6/freq 换算」纯路径代价）；(c) 1.5ms 阈值沿袭 v0.138.0~v0.142.0 放宽先例链的统一量级，对真机回归仍有约束力。本轮实测 **max=5,808ns 距阈值余量 ≈258×**（1,500,000 / 5,808）、avg=190ns 距真机目标同量级——**首轮一次通过，未触发「实测接近/超阈值时放宽判定口径」路径，阈值零调整**；clock<100ns 真机目标不变，留待飞腾/鲲鹏硬件验证同口径重测。

## 10. 阈值对照（蓝图 §6/§7/§9 逐项判定）

| 蓝图锚点 | 要求 | 落点 | 判定 |
|---------|------|------|------|
| §6 单元：串口输出 | PL011 init/putc 通路验证 | SER1~SER3 + A64H3 + HPI3（假后端帧/日志回读）+ 冒烟 serial_putc 自证 | ✅ PASS |
| §6 单元：时钟读取 | CNTVCT 读取与换算验证 | CLK1~CLK5 + A64H4 + HPI4（换算精度/单调性）+ 冒烟双读单调 | ✅ PASS |
| §6 集成：ARM64 全功能回归 | 全方法链路回归 | A64H1~A64H10 + HPI1/HPI2/HPI5 + QEMU 冒烟全链路 + workspace 全量零回归 | ✅ PASS |
| §6 性能：clock_now < 100ns | **真机目标** | —（真机未测）；QEMU 判定口径 max=5,808ns < 1.5ms（**余量 ≈258×**，首轮一次通过阈值零调整）；host HPI7 记录 avg 35ns/轮 | ✅ PASS（QEMU 口径）；真机留待飞腾/鲲鹏同口径重测 |
| §6 GPU | 不适用 | 蓝图原样（本版本无模型训练/量化/仿真加速场景） | 不适用 |
| §7 功能：ARM64 全功能 | CPU/内存/中断/时钟/串口/GPIO 全功能 | 六 init + 五运行时方法全链路（A64H 单测 + HPI1/HPI2 + 冒烟 ②~⑥ 全步骤） | ✅ PASS |
| §7 性能：clock < 100ns | 同 §6 性能 | 同上 | ✅ PASS（QEMU 口径） |
| §7 出口：ARM64 全功能 → x86/飞腾/鲲鹏 BSP 可启动 | BSP 版本可基于本接口启动 | **Hal trait 十一方法冻结**（§3.1）+ **Registry 多态口径**（D10，HPI6）+ **平台配置分离**（platform 模块独立，§6.2 演进路线）三要素达成——BSP 版本（v0.144.0 起）仅需新增 platform 模块 + Hal 实现 | ✅ PASS（接口冻结判定达成） |
| §9 功能：7 类 HAL 接口完整 | 接口类全覆盖 | 十一方法按「六 init 功能域（cpu/mem/irq/clock/serial/gpio）+ 运行时方法集（putc/now_us/deadline/enable/disable）」计七类全量落地 | ✅ PASS |
| §9 性能：< 100ns | 同 §6 性能 | 同上 | ✅ PASS（QEMU 口径） |
| §9 安全：初始化顺序固定 | init 顺序确定性 | 六 init 蓝图 §4.3 mermaid 顺序固定（cpu→mem→irq→clock→serial→gpio），A64H1/HPI2 顺序锚定 + 冒烟错误码短路 | ✅ PASS |
| §9 可靠：init 失败 panic | 失败可观测不静默 | D2 Result 化等价落地：错误码短路 + 冒烟 `[hal] err code=<n>`（aux=KernelError 判别值）不挂死，同 cap/shm 先例 | ✅ PASS（等价口径） |
| §9 可维护：平台实现分离 | 平台代码隔离 | trait（mod.rs）/ 平台配置（platform 模块）/ 实现（aarch64.rs）三层分离；BSP 演进路线 §6.2 | ✅ PASS |
| §9 可观测：串口+时钟 | 两通路可观测 | 冒烟经 HAL 自身 serial_putc 输出行首标记自证串口通路 + clock_now_us 夹取 avg/max 随 ok/err 打印 | ✅ PASS |
| §9 可扩展：支持新平台 | 新平台接入成本 | 新 platform 模块 + Hal 实现两文件即可挂载（§6.2）；D7 trigger/D9 GPIO 操作集预留演进位 | ✅ PASS |
| 构建预算 | release ELF < 512KB | kernel 152,200B / bootloader 70,440B（≈16.4KB BSS NOBITS 不占镜像） | ✅ PASS |
| 内存预算（仓库 §5.6） | hal 子系统 BSS < 20KB | Aarch64Hal ≈16.4KB + Registry 全局槽 ≈24B（§4.4） | ✅ PASS |

## 11. 坑点与风险（蓝图 §8 + 本版本实测坑点）

| 坑点/风险 | 本版本处置 |
|----------|-----------|
| ① **VBAR 时序**（蓝图 §8 坑点「VBAR 必须在首次异常前设置」） | 既有启动序列（v0.133.0）已于首次异常前装载 VBAR_EL1，坑点由启动序列满足；HAL 侧 `cpu_init` 复用 `install_vbar` **幂等重装**（其 Safety 文档明示「重复调用无害（同一基址重写）」）——重装语义保险丝，不引入时序依赖；host 无异常向量设施为空操作（D3） |
| ② **CNTVCT QEMU 坑点（三口径不可互证）** | host std Instant（debug 注入源，avg 35ns/轮）仅记录量级；QEMU TCG + 虚拟计数器（avg=190ns）为功能验证记录，**不可作为真机性能证据**（蓝图 §8.5 坑点对照，qemu_run.md §22 决策记录）；真机 <100ns 目标留待飞腾/鲲鹏同口径重测（同 v0.138.0~v0.142.0 先例链） |
| ③ **stray IRQ 防护（IMASK-disarm 链）** | clock_init 编程 CNTV_CTL ENABLE\|IMASK=0b11（D4：定时器使能但中断屏蔽），deadline 到期不投递；deadline 编程后 `clock_disarm`（D9，CNTV_CTL 写 0 同清两位）收尾不遗留未到期定时器——本版本无定时器中断 handler 接入需求，防护链完整（§5.3） |
| ④ **SGI<16 no-op → SPI 36 选择** | 既有 `Gicv3::enable_irq` 对 SGI<16 为 no-op（v0.133.0 口径「SGI 本版本不使能」），且 SGI0=IPI/SGI1=sync 既有通道不触碰；IRQ 回读测试选 **SPI 36**（QEMU virt 未占用 INTID；GICD 已经 v0.133.0 identity map，MMU on 下物理直访有效）；使能即禁（无设备驱动源、边沿触发无挂起残留），与既有十五类输出正交零干扰 |
| ⑤ **trait object 开销**（蓝图 §8 风险） | 静态分发为主、Registry 单点 dyn（§3.3）；HPI7 1000 轮记录分发延迟量级（avg 35ns/轮 host 口径），cap CPI8 先例同口径 |
| ⑥ **CNTV_TVAL 有符号 32bit 上限** | 62.5MHz 下上限 ≈34s（2^31/62.5e6），60s 量级编程会溢出回绕为负；冒烟取 5s = 312_500_000 ticks ≪ 2^31（§5.4）；回读判定取 bits[31:0] 有符号解释（bit31 清零即非负），main.rs `read_cntv_tval` 唯一读取点 |
| ⑦ **IRQ 边界双保险** | `irq ≥ 1024`（GICv3 INTID 上限）HAL 层守卫显式落地 + GICv3 层既有同口径防御（IRQ_LIMIT=1024 双保险，D8）；A64H8/HPI5(c)/HPI8(a) 帧零写入锚定 |
| 蓝图 §5 难点「VBAR 设置需在早期」 | 同坑点①：install_vbar 复用 + 幂等重装无害 |
| 蓝图 §2 阻塞「HAL 不完整阻塞 BSP」 | 本版本 trait 冻结 + ARM64 全功能 + 三要素出口判定（§10）解除阻塞 |

## 12. D1~D10 偏差表（全局）

> 文档级编号与 [src/hal/mod.rs](../../crates/kernel/microkernel/src/hal/mod.rs) 头注 D1~D10 一一对应（自 spec.md「蓝图偏差决策」表原样整理）；文末附落地修正汇总五条。

| 编号 | 蓝图口径 | 决策 | 理由 |
|------|---------|------|------|
| D1 | 路径 `kernel/src/hal/` | `crates/kernel/microkernel/src/hal/` | 仓库规范 §2.3.1 强制子系统归口（同 ipc/cap D1 先例链） |
| D2 | §4.4「初始化失败：串口打印错误 + panic」，trait init 六方法返回 `()` | init 六方法（cpu/mem/irq/clock/serial/gpio）返回 `Result<(), KernelError>` | 仓库禁 panic! 规则（v0.127.0 D4 先例：panic 语义改短路错误传播）；§9 可靠性「init 失败 panic」以错误码短路 + 冒烟 `[hal] err` 分支等价落地 |
| D3 | 伪代码 `exception_vector_base()` + 多余括号（不编译） | cpu_init 复用既有 `interrupt::vector::install_vbar`（幂等重装 VBAR_EL1） | 该函数不存在；VBAR 装载设施 v0.133.0 已交付并经 QEMU 实证；§8 坑点「VBAR 必须在首次异常前设置」由既有启动序列满足，HAL 侧重装幂等无害 |
| D4 | 伪代码 `msr cntvoff_el2, xzr`（EL2 寄存器） | 删除该写；clock_init 仅使能 CNTV_CTL_EL0（ENABLE=1）且**置 IMASK=1**（防 stray IRQ）；CNTVOFF 信任启动环境已清零 | 内核运行于 EL1，写 EL2 寄存器为 UNDEFINED 异常；既有 wcet/cap CNTVCT 路径已在 QEMU 实证偏移为零；IMASK 口径见 D9 deadline 设计 |
| D5 | `clock_now_us()` 直接读 CNTVCT | GenericTimer 构造注入 `freq: u64` + 计数源 `now_cycles: fn() -> u64`（aarch64 = read_cycle_counter/CNTVCT，host = mock）；us 换算为独立纯函数 | host 可测性（cap manager D5 时钟注入先例）；CNTVCT 读为 aarch64 门控，host 单测不触内联汇编 |
| D6 | `Aarch64Hal { gic: Gicv3, timer: GenericTimer, uart: Pl011 }` | 三字段语义原样，具体类型全部复用既有设施：gic = `interrupt::gicv3::Gicv3`（base 参数化，host 伪 MMIO 可注入）、uart = `hal::serial::Pl011<B: RegIo>` 泛型封装 `eneros_bootloader::Pl011Uart<B>`（aarch64 = MmioRegIo，host = 缓冲假后端）、timer = `hal::clock::GenericTimer`（D5） | 零重复实现（Simplicity First）；Gicv3/Pl011Uart 均已经 QEMU + host 双测实证 |
| D7 | `enable_irq(&mut self, irq: u32, cpu: u8)` 无 trigger 参数 | 签名原样，内部固定 `IrqTrigger::Edge`（蓝图 §4.5 伪代码原样） | 触发方式可配置仍由 interrupt:: 层承担；BSP 版本按需扩展 HAL 签名属后续（v0.144~v0.147） |
| D8 | enable_irq/disable_irq 返回 `()` | 签名原样；内部 `irq ≥ 1024`（GICv3 INTID 上限）防御性 no-op | 防御性路径（同既有 IrqId 边界口径）；不引入新错误变体 |
| D9 | `mem_init` / `gpio_init` 空注释 | mem_init = Ok(()) 空操作（蓝图 §4.5 原样「MMU 在 v0.131.0 已完成」）；gpio_init = 占位 Ok(())，PL061 基址 0x0903_0000 录入平台配置（GPIO 操作集留待 BSP 版本）；clock_deadline 编程 CNTV_TVAL 后由 D4 IMASK 兜底不投递，冒烟末尾 disarm（CNTV_CTL ENABLE=0，Aarch64Hal 固有方法，非 trait） | 蓝图自身空实现口径；QEMU virt 无 GPIO 交互需求；stray IRQ 防护链完整 |
| D10 | `HalRegistry { hal: &'static dyn Hal }` | 原样 + `register` 一次性（重复注册 → `BadState`）+ `get()` 未注册 → `BadState`；全局单例经 `spin::Once` 等价手写（UnsafeCell + AtomicBool，零新依赖） | 启动期单线程注册语义（v0.127.0 D3 先例）；无堆无锁 |

**落地修正汇总**（tasks.md Task 1~4 记录 + 本版本实测，spec 内部矛盾的最小化解）：

| # | 修正 | 理由/出处 |
|---|------|----------|
| ① | HAL3/HAL6 的 `&mut self` 方法无法经共享 `&dyn Hal` 调用，改 `&mut dyn Hal`（HAL3/A64H10/HPI1）与 Box::leak 注册前调用可变方法（HAL6）完成多态分发核验 | Rust 借用规则：`&mut self` 接收者方法不可经共享 trait 对象调用；mod.rs HAL3/HAL6 用例头注记录 |
| ② | 内存预算由蓝图 §4.1 三结构直观估算的「数十字节」修正为 **Aarch64Hal ≈16.4KB**（内联 `Gicv3.table: IrqHandlerTable` 16KB：1024×Option\<fn\> 8KB + 1024×u64 计数 8KB，v0.133.0 既有结构原样复用） | spec.md 内存预算节 + mod.rs 头注；三字段语义原样但 Gicv3 为内联大结构 |
| ③ | spec/tasks 早期文本「BSS < 1KB」为过期口径，以 spec 内存预算节 **≈16.4KB / hal 子系统 BSS < 20KB** 为准 | mod.rs 头注「tasks.md Task 4 原文『BSS < 1KB』为过期口径，以本段为准」 |
| ④ | `GenericTimer` host 状态记录 Cell（last_deadline_ticks/armed/masked）使尺寸由 spec 预算 16B 增至 **26B**（freq 8 + fn 8 + ticks 8 + 2×bool） | clock.rs 头注「尺寸口径附记」；对照 BSS 总预算 < 20KB 无压力 |
| ⑤ | HPI5/A64H7 的 ICFGR 偏移计算注明：irq36 → ICFGR 下标 36/16=**2**（地址 0xC00+2×4）、2bit 字段 shift=(36%16)×2=**8**，期望值 0b10<<8 | hal_test.rs HPI5 / aarch64.rs A64H7 用例头注（D7 Edge 固定触发方式回读锚定） |

## 13. 源码链接

| 项 | 路径（含行号锚点） |
|----|-------------------|
| HAL 子系统挂载 + Hal trait 十一方法（L83~128）+ HalRegistry（L132~147）+ 全局 register_hal/hal()（L162~197）+ platform::qemu_virt 六常量（L204~222）+ D1~D10 头注与内存预算声明（L1~69）+ HAL1~HAL6 单测（L225~494） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) |
| GenericTimer 注入式时钟（L54~184：new/new_system/now_us/deadline_us/disarm/enable_masked）+ cycles_to_us 纯函数（L189~196）+ CLK1~CLK5 单测（L198~292） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs) |
| Pl011\<B: RegIo\> 泛型封装（L29~81：from_bus/new/init/putc/观测口）+ SER1~SER3 单测（L83~174） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/serial.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/serial.rs) |
| Gpio 占位（L22~44）+ GPIO1~GPIO2 单测（L46~72） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/gpio.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/gpio.rs) |
| Aarch64Hal 四元组装（L62~72）+ from_parts/new_system 双构造（L77~130）+ Hal 十一方法实现（L132~197）+ clock_disarm 固有方法（L109~111）+ A64H1~A64H10 单测（L199~562） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs) |
| host 集成测试 HPI1~HPI8（HPI1 L241 / HPI2 L287 / HPI3 L321 / HPI4 L345 / HPI5 L376 / HPI6 L432 / HPI7 L462 / HPI8 L498）+ 全局状态注记头注 | [file:///e:/EnerOS/crates/kernel/microkernel/tests/hal_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/hal_test.rs) |
| QEMU 冒烟段 `hal_smoke`（主体 L4411 起；判定常量 HAL_CLK_LAT_MAX_NS/HAL_ROUNDS/HAL_DEADLINE_US/HAL_TEST_IRQ L4306~4326；rust_main §3.17 调用点 L985；头注 v0.143.0 段 L173~192） | [file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| lib.rs 模块挂载（`pub mod hal;` L66） | [file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) |
| QEMU 测试脚本第十六条断言 `grep -q "\[hal\] ok"`（L182，既有十五条原样） | [file:///e:/EnerOS/tests/boot_test.sh](file:///e:/EnerOS/tests/boot_test.sh) |
| QEMU 实测记录（串口全文/十六断言/`[hal] ok avg=190 max=5808` 注解/延迟判定口径决策记录/坑点对照/首轮 PASS）§22 | [file:///e:/EnerOS/docs/boot/qemu_run.md](file:///e:/EnerOS/docs/boot/qemu_run.md) |
| 复用设施：Gicv3 驱动 + IrqHandlerTable（v0.133.0） | [file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/gicv3.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/gicv3.rs) + [file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/handler.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/handler.rs) |
| 复用设施：install_vbar（v0.133.0，「重复调用无害」Safety 文档 L197~198） | [file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/vector.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/interrupt/vector.rs) |
| 复用设施：wcet 时钟设施（read_cycle_counter/read_counter_freq/cycles_to_ns，v0.138.0） | [file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| 复用设施：Pl011Uart/MmioRegIo/RegIo（v0.129.0，eneros_bootloader） | [file:///e:/EnerOS/crates/kernel/bootloader/src/serial.rs](file:///e:/EnerOS/crates/kernel/bootloader/src/serial.rs) |
| KernelError::BadState = 7（复用变体，L30） | [file:///e:/EnerOS/crates/kernel/microkernel/src/error.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/error.rs) |
| 前序设计文档：v0.142.0 能力管理器（D 表与测试矩阵格式先例） | [./capability_design.md](./capability_design.md) |
