# EnerOS v0.144.0 x86_64 BSP（接口占位）设计文档（X86Hal 三元组装 / LocalApic 占位 / X86PageTable 四级类型 / x86_qemu 平台配置）

> 版本：v0.144.0
> 日期：2026-07-28
> 状态：已交付（host 单测 X1~X8 / AP1~AP3 / XPT1~XPT3 全绿 + aarch64 release 交叉编译 cfg 门控零副作用；XPI1~XPI6 集成测试为 Task 5 交付物，本文档按 spec 测试矩阵落点记录设计验证点）
> 适用范围：EnerOS 自研内核 x86_64 BSP 接口占位子系统（X86Hal 三元组装 + LocalApic 占位 + Uart8250 泛型封装 + X86PageTable 四级类型 + platform::x86_qemu 平台常量组；host 单测注入式可测 + aarch64 交叉编译 cfg 门控零副作用；x86_64 真机路径留待 ADR-0001 修订或信创范围扩展时补齐）
> 蓝图依据：`蓝图/phase3.md` §v0.144.0（§1 版本目标 ★ ADR-0001 降级口径 / §2 前置依赖 / §3 交付物清单 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-E HAL 多平台线第 2 版**（v0.143.0 HAL 接口与 ARM64 实现 → **v0.144.0 x86_64 BSP** → v0.145.0 飞腾 → v0.146.0 鲲鹏 → v0.147.0 RISC-V/龙芯，五版之二），跨架构验证第一步，证明 v0.143.0 Hal trait 接口冻结后 BSP 可扩展
> 配套文档：[hal_design.md](./hal_design.md)（v0.143.0 HAL 接口与 ARM64 实现设计文档，前序版本，Hal trait 十一方法接口冻结来源）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 驱动，LocalApic 接口对齐参考）

---

## 1. 版本目标

### 1.1 ★ ADR-0001 降级口径（蓝图 §1 逐字引用）

> ★ **ADR-0001 BSP 收敛**：原 5 平台（x86/飞腾/鲲鹏/RISC-V/龙芯）收敛为 2 生产平台（飞腾 + 鲲鹏）+ 1 开发平台（ARM64 QEMU）。信创认证是产品+供应链+材料+测试体系工程，非"做完内核就过"。**x86_64 不在信创认证范围**，本版本工作降级为接口占位，不投入生产资源。

蓝图 §1（`蓝图/phase3.md` L5403）原样。本降级口径为本版本**最高优先级决策**（仓库规范 §5.4 ADR-0001 已签署决策态），高于蓝图模板交付物（§3 交付物清单 / §6 测试计划 / §7 验收标准中的"完整 x86 启动链"项均按本口径降级，见 §2 偏差表与 §9 D1 决策）。

### 1.2 核心一句话（蓝图 §1 原样）

**实现 x86_64 BSP（CPU/中断 APIC/页表 x86 适配），支持自研内核在 x86 启动。**

（蓝图 `phase3.md` L5405 原样；按 §1.1 ADR-0001 降级口径，本版本交付"接口占位 + host 可测骨架"，完整 x86 启动链留待真机版本。）

### 1.3 Phase 定位（蓝图 §1 原样）

- **业务价值**：证明内核跨架构能力，x86 是主流服务器平台（蓝图 §1 原样）。
- **Phase 定位**：**跨架构验证第一步**（蓝图 §1 原样）。
- **出口关联**：自研内核"多架构"能力（蓝图 §1 原样）。

### 1.4 降级落地口径（spec.md「Why」）

v0.143.0 已冻结 `Hal` trait 十一方法接口规范，为 x86/飞腾/鲲鹏/RISC-V/龙芯 BSP 提供出口。蓝图 §v0.144.0 要求实现 x86_64 BSP，但 ★ ADR-0001 BSP 收敛明确"x86_64 不在信创认证范围，本版本工作降级为接口占位，不投入生产资源"。本版本交付 `X86Hal` 接口占位与 host 可测骨架，证明内核跨架构能力（Hal trait 多态分发至 x86 实现），为后续真正投入 x86 生产资源时提供可编译可测试的起点。

### 1.5 依赖关系（蓝图 §2 原样 + spec.md 落地）

- 前序 v0.143.0：HAL 接口（Hal trait 十一方法 + HalRegistry + platform::qemu_virt 已冻结，本版本消费）
- 外部 QEMU x86_64：蓝图 §2 假设，按 ADR-0001 降级口径本版本不依赖（不交叉编译 x86_64，不跑 QEMU x86 启动）
- 假设 x86 长模式（64bit）：蓝图 §2 假设，本版本仅类型定义与签名落地，长模式切换留待真机
- 阻塞项：无（非主架构）——蓝图 §2 原样

---

## 2. 交付物清单

对照蓝图 §3（`phase3.md` L5421~5432）逐项标注 D1 降级偏差（D 编号与 spec.md「蓝图偏差决策」表一一对应，全局表见 §9）：

| 蓝图 §3 交付物 | 蓝图要求 | 实际交付 | 偏差决策 |
|---------------|---------|---------|---------|
| 代码：`kernel/src/hal/x86_64.rs` | x86_64 HAL 实现 | [crates/kernel/microkernel/src/hal/x86_64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（495 行：`Uart8250<B: RegIo>` 泛型封装 + `X86Hal<B: RegIo = MmioRegIo>` 三元组装 + `from_parts`/`new_system` 双构造 + Hal trait 十一方法全量实现 + `clock_disarm` 固有方法 + `read_tsc` cfg 门控 + X1~X8 单测） | D1 路径归口；D3 补全 clock_deadline；D4 GenericTimer 复用；D5 Uart8250 泛型；D6 LocalApic 占位；D8 cfg 门控 |
| 代码：`kernel/src/hal/apic.rs` | Local APIC 实现 | [crates/kernel/microkernel/src/hal/apic.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（176 行：`LocalApic { base, initialized, last_irq, last_op_enable }` 占位 + `new`/`base`/`init`/`enable`/`disable` 签名 + 观测口 + `APIC_DEFAULT_BASE` 常量 + AP1~AP3 单测） | D1 路径归口；D6 占位签名落地；D8 cfg 门控寄存器写留待真机 |
| 代码：`kernel/src/mem/x86_page_table.rs` | x86 四级页表 | [crates/kernel/microkernel/src/mem/x86_page_table.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（272 行：`Pml4`/`Pdpt`/`Pd`/`Pt` 四级类型定义各 `[u64; 512]` + `X86PageTable { pml4 }` + `new`/`map`/`activate` 签名 + 观测口 + XPT1~XPT3 单测） | D1 路径归口；D2 Result 化禁 panic；D7 四级类型定义；D8 CR3 cfg 门控留待真机 |
| 接口：`X86Hal { apic, uart }` + Hal impl | 蓝图 §3 两字段 | `X86Hal<B: RegIo = MmioRegIo> { apic: LocalApic, timer: GenericTimer, uart: Uart8250<B> }` 三元组装（D4 复用 GenericTimer）+ Hal trait 十一方法全量实现 | D4 增 timer 字段（复用 v0.143.0 注入式时钟）；D6 三字段语义对齐 Aarch64Hal |
| 接口：`X86PageTable { pml4: PhysFrameNum }` | 蓝图 §3 单字段 | `X86PageTable { pml4: Pml4 }` + 四级类型 `Pml4`/`Pdpt`/`Pd`/`Pt` 各 `[u64; 512]` + `new`/`map`/`activate` Result 签名 | D2 Result 化；D7 四级类型定义 + map/activate 签名落地 |
| 文档：`docs/kernel/x86_bsp.md` | x86 BSP 设计文档 | 本文档 | 无（路径同蓝图） |
| 测试：`tests/x86_boot_test.sh` | QEMU x86 启动脚本 | **不交付**（D1 降级口径） | D1 ★ ADR-0001：x86 启动链未实现，QEMU x86 启动脚本无意义；留待真机版本 |
| 配置：`x86_64-unknown-none` target | target 安装 | **不交付**（D1 降级口径） | D1 ★ ADR-0001：不交叉编译 x86_64；`rust-toolchain.toml` 与 `.cargo/config.toml` 零改动 |
| 模块挂载：`hal/mod.rs` | — | [hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) 追加 `pub mod apic;`（L74）+ `pub mod x86_64;`（L78）+ `platform::x86_qemu`（L229~236）两常量子模块 | D9 Surgical Changes：既有 Hal trait / HalRegistry / platform / aarch64 / clock / serial / gpio / qemu_virt 零改动 |
| 模块挂载：`mem/mod.rs` | — | [mem/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/mod.rs) 追加 `pub mod x86_page_table;`（字母序） | D9 Surgical Changes：既有 PMM/VMM/buddy/zone 零改动 |
| 头注：[lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) | — | [lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) 头注交付物追加 v0.144.0 x86 行（L34：X86Hal + LocalApic + X86PageTable + ADR-0001 降级口径说明） | D9 Surgical Changes：仅追加头注一行 |
| 集成测试：`tests/x86_test.rs` | XPI1~XPI6 | **Task 5 待交付**（spec 测试矩阵已定义，集成测试文件为 Task 5 交付物） | D1 降级口径：host 注入式集成测试（非 QEMU x86 启动）；XPI1~XPI6 验证点见 §7.2 |

**spec 分类口径对照**（spec.md「What Changes」）：实现类（X86Hal / LocalApic / Uart8250）→ §3/§4；占位类（X86PageTable）→ §5；配置类（x86_qemu 平台常量）→ §6；文档类（x86_bsp.md）→ 本文档；测试类（X1~X8 单测已交付，XPI1~XPI6 集成 Task 5 待交付）→ §7。

---

## 3. X86Hal 设计

### 3.1 三元组装结构图（蓝图 §3 + spec D4/D5/D6）

```rust
pub struct X86Hal<B: RegIo = MmioRegIo> {
    apic: LocalApic,            // D6 占位（Task 1，与 Gicv3 接口对齐）
    timer: GenericTimer,        // D4 复用 v0.143.0 注入式时钟（x86_64 = rdtsc）
    uart: Uart8250<B>,          // D5 泛型（x86_64 = MmioRegIo，host = FakeRegIo）
}
```

源码锚点：[hal/x86_64.rs L185~192](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)。

- **泛型口径**：`<B: RegIo = MmioRegIo>`——x86_64 真机经 `new_system()` 以 x86_qemu 平台常量组构造（COM1/APIC_BASE 基址 + rdtsc 封装定时器）；host 单测经 `from_parts(apic, timer, uart)` 注入式构造（伪 MMIO 假后端 + mock 计数源，构造期零硬件副作用）。
- **三字段语义对齐 Aarch64Hal**：apic ↔ gic（中断控制器）、timer ↔ timer（复用 GenericTimer）、uart ↔ uart（泛型封装）；x86 无 GPIO，故无 gpio 字段（gpio_init 占位恒 Ok，见 §3.3）。
- **观测口**：`apic()` / `timer()` / `uart()` 三个只读观测口供测试寄存器级回读与状态断言（[hal/x86_64.rs L201~214](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）。
- **`clock_disarm(&mut self)`**：X86Hal **固有方法**（非 trait 项，同 Aarch64Hal::clock_disarm 口径），委托 `GenericTimer::disarm`（[hal/x86_64.rs L219~221](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）。

### 3.2 D4 GenericTimer 复用（v0.143.0 注入式时钟，零重复定义 cycles_to_us）

- **矛盾**：蓝图 §4.5（`phase3.md` L5473~5476）`clock_now_us` 用 `rdtsc` 读 TSC 后 `/ 1000` 得微秒，但 TSC 频率并非固定 1GHz（x86 不同 CPU TSC 频率差异大），硬编码 `/ 1000` 不准确。
- **决策**：与 v0.143.0 D5 注入式时钟口径一致——`X86Hal` 构造注入 `freq: u64` + `now_cycles: fn() -> u64`（x86_64 = rdtsc 封装，host = mock）；`cycles_to_us` 纯函数复用 [hal::clock::cycles_to_us](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs)（零重复定义）；x86_64 真机路径读 TSC 频率留待真机（本版本 host 注入源可测）。
- **落地**：`X86Hal::new_system()` 经 `GenericTimer::new(TSC_FREQ_HZ, read_tsc)` 构造（[hal/x86_64.rs L234](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)），`TSC_FREQ_HZ = 1_000_000_000`（1GHz 占位，真机校准留待 CPUID 0x15 读取或启动期标定）；`clock_now_us` 委托 `timer.now_us()`（[hal/x86_64.rs L306~310](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）。

### 3.3 D5 Uart8250 泛型封装（RefCell<B> + base + from_bus/new/init/putc）

- **矛盾**：蓝图 §4.5（`phase3.md` L5484~5503）Uart8250 用 `core::ptr::write_volatile` 直接读写端口地址（0x3F8~0x3FD），但 x86 端口 IO 需 `out`/`in` 指令（`asm!("out", ...)`）而非内存映射；`write_volatile` 仅适用于 MMIO。
- **决策**：Uart8250 封装泛型化 `<B: RegIo>`（复用 v0.143.0 Pl011 口径）——x86_64 真机路径用 PortIo 假后端（`asm!("out")` 封装留待真机，本版本仅类型定义 + host 用 FakeRegIo 缓冲帧可测）；COM1 0x3F8 基址录入平台配置常量。
- **落地**：[hal/x86_64.rs L96~178](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs) 定义 `Uart8250<B: RegIo> { bus: RefCell<B>, base: usize }`：
  - `from_bus(bus: B) -> Self`（host 注入路径，base=0；L107~112）
  - `new(base: usize) -> Self`（x86_64 真机路径，`MmioRegIo::new(base)`；L172~178）
  - `base(&self) -> usize`（L115~117）
  - `bus(&self) -> Ref<'_, B>`（观测口；L120~122）
  - `init(&mut self)`（cfg 门控 8250 寄存器序列 DLAB/8N1/FIFO/IRQ，L131~147）
  - `putc(&self, ch: u8)`（轮询 LSR THRE bit5=0x20 + 写 THR offset 0x00；L154~165）
- **`bus` 以 `RefCell` 持有**——`putc(&self)` 为共享引用语义（匹配 `Hal::serial_putc` 签名），而 `RegIo::write32(&mut self)` 需独占引用，`RefCell` 提供零 unsafe 的内可变性（同 Pl011Uart D1 口径）。
- **常量**：`THR_OFFSET = 0x00` / `LSR_OFFSET = 0x05` / `LSR_THRE = 0x20` / `IRQ_LIMIT = 1024`（[hal/x86_64.rs L70~78](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）。

### 3.4 D6 LocalApic 占位（与 Gicv3 接口对齐）

- **矛盾**：蓝图 §4.5（`phase3.md` L5436）仅给出 `LocalApic { base: usize }` 结构体，无方法签名；§4.2 接口说"APIC 中断（替代 GICv3）"但未给 enable/disable 签名。
- **决策**：LocalApic 占位——`new(base)` / `init()` / `enable(irq)` / `disable(irq)` 签名落地（与 Gicv3 接口对齐，便于 X86Hal enable_irq/disable_irq 委托）；寄存器操作（APIC MMIO 基址 0xFEE00000，LVT 寄存器等）留待真机，host 用 Cell 状态记录可测（参照 v0.143.0 GenericTimer host 口径）。
- **落地**：详见 §4。

### 3.5 D8 cfg 门控（所有 x86_64 真机路径门控，host 编译零副作用）

- **决策**：所有 x86_64 真机路径（rdtsc / PortIo / APIC MMIO / CR3）均 `#[cfg(target_arch = "x86_64")]` 门控；host 测试用注入式 mock（D4/D5 口径）；aarch64 编译不包含 x86_64 代码（cfg 门控零副作用）；不安装 x86_64-unknown-none target（本版本不交叉编译 x86_64）。
- **落地门控点**：
  - `X86Hal::new_system()`（[hal/x86_64.rs L230](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）
  - `read_tsc()` rdtsc 内联汇编（[hal/x86_64.rs L244](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）
  - `TSC_FREQ_HZ` 常量（[hal/x86_64.rs L82~83](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）
  - `Uart8250::init()` 内 8250 寄存器序列占位（[hal/x86_64.rs L135~145](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）
  - `X86PageTable::activate()` 内 CR3 加载占位（[mem/x86_page_table.rs L201~208](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)）
- **host/aarch64 编译零副作用**：cfg 门控排除上述路径，host 单测经 `from_parts` 注入式构造，aarch64 release 交叉编译通过（Task 3 实测：双 ELF < 512KB）。

### 3.6 Hal trait 十一方法实现表（v0.143.0 接口冻结，本版本消费实现）

| # | 方法名 | 蓝图 §4.5 落点 | 委托目标 | 偏差决策 |
|---|--------|---------------|---------|---------|
| 1 | `cpu_init` | 设置 IDT | 占位 Ok（IDT 加载留待真机） | D2 Result 化；D8 cfg 门控 IDT 加载留待真机 |
| 2 | `mem_init` | x86 页表 | 占位 Ok（CR3 加载留待真机） | D2 Result 化；D7 X86PageTable 占位类型已交付，CR3 加载属 x86 启动链非本版本范围 |
| 3 | `irq_init` | `self.apic.init()` | 委托 `LocalApic::init` | D6 委托 apic.init；host 记录 initialized 标志 |
| 4 | `clock_init` | APIC timer 配置 | 委托 `GenericTimer::enable_masked` | D4 复用 v0.143.0 注入式时钟；host 记录 armed/masked 状态 |
| 5 | `serial_init` | `self.uart.init()` | 委托 `Uart8250::init` | D5 委托 uart.init；host 空操作，x86_64 真机 8250 寄存器序列 cfg 门控 |
| 6 | `gpio_init` | x86 无 GPIO | 占位 Ok | D9 同 aarch64.rs gpio 占位口径，x86 平台无 PL061 等通用 GPIO 控制器 |
| 7 | `serial_putc` | `self.uart.putc(ch)` | 委托 `Uart8250::putc` | D5 委托 uart.putc；轮询 LSR THRE + 写 THR；host FakeRegIo 记录至 tx_log |
| 8 | `clock_now_us` | `rdtsc / 1000` | 委托 `GenericTimer::now_us` | D4 删除 `/ 1000` 硬编码，复用注入式 freq + now_cycles 换算 |
| 9 | `clock_deadline` | （蓝图 §4.5 缺） | 委托 `GenericTimer::deadline_us` | D3 补全（蓝图 §4.5 仅列 9 方法，缺 clock_deadline），十一方法全覆盖 |
| 10 | `enable_irq` | `self.apic.enable(irq)` | 委托 `LocalApic::enable`（irq<1024） | D8 边界防御：irq ≥ 1024 no-op（同 aarch64.rs IRQ_LIMIT=1024 口径） |
| 11 | `disable_irq` | `self.apic.disable(irq)` | 委托 `LocalApic::disable`（irq<1024） | D8 边界防御：同 enable_irq 口径 |

源码锚点：[hal/x86_64.rs L258~333](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（Hal trait impl 块）。

### 3.7 复用关系（文字化依赖图）

```
X86Hal（本版本，hal/x86_64.rs）
├─ apic: hal::apic::LocalApic .............. 本版本新增占位（D6）
│   └─ host 状态记录 Cell（initialized/last_irq/last_op_enable）
│      参照 v0.143.0 GenericTimer host 口径
├─ timer: hal::clock::GenericTimer ......... v0.143.0 既有注入式时钟原样复用（D4）
│   └─ x86_64 计数源 = read_tsc（rdtsc 封装，cfg 门控）
│      host = mock 计数源（同 v0.143.0 host 口径）
├─ uart: hal::x86_64::Uart8250<B: RegIo> ... 本版本新增泛型封装（D5）
│   └─ 复用 eneros_bootloader::RegIo trait 抽象（同 v0.143.0 Pl011 口径）
│      x86_64 = MmioRegIo 占位（真机 PortIo asm!("out") 留待真机）
│      host = FakeRegIo 缓冲帧
└─ clock_disarm 固有方法 → GenericTimer::disarm（v0.143.0 既有）
```

零重复实现（Simplicity First）：本版本**零新外部依赖**；GenericTimer/RegIo/MmioRegIo 三处复用件均已经 v0.143.0 host + QEMU 双测实证。

### 3.8 内存预算（仓库 §5.6 合规声明，spec.md 口径）

| 组件 | 尺寸 | 说明 |
|------|------|------|
| `X86Hal` | **≈48B** | `LocalApic` 16B（base 8 + 3×Cell 8 对齐）+ `GenericTimer` 26B（复用 v0.143.0 注入式，D4）+ `Uart8250<B>` 6B（RefCell<B> + base） |
| `X86PageTable` | **4KB + 16B** | 四级页表各 `[u64; 512]` = 4KB×1（Pml4 根表）+ X86PageTable 包装 16B（Pdpt/Pd/Pt 留待真机按需分配，本版本仅 Pml4 类型定义 + 空表构造） |
| `LocalApic` host 伪 MMIO 帧 | ≈4KB | 测试用 4KB 对齐帧（同 Gicv3 测试口径） |
| hal 子系统 BSS 总计 | **< 20KB** | X86Hal 不驻留 BSS——x86 启动链未实现，不全局注册（v0.143.0 口径延续） |

> 预算口径：spec.md 内存预算节原样；X86Hal 不驻留 BSS（与 Aarch64Hal 不同——Aarch64Hal 经 `new_system()` 构造后驻留 BSS，X86Hal 的 `new_system()` cfg 门控仅 x86_64 编译，本版本不交叉编译 x86_64）。

---

## 4. LocalApic 占位

### 4.1 D6 签名（new/base/init/enable/disable + 观测口）

源码锚点：[hal/apic.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（176 行）。

```rust
pub const APIC_DEFAULT_BASE: usize = 0xFEE0_0000;  // L43

pub struct LocalApic {                              // L55~66
    base: usize,
    initialized: Cell<bool>,
    last_irq: Cell<u32>,
    last_op_enable: Cell<bool>,
}

impl LocalApic {
    pub const fn new(base: usize) -> Self;          // L73~80
    pub fn base(&self) -> usize;                    // L83~85
    pub fn init(&mut self);                         // L95~101
    pub fn enable(&mut self, irq: u32);             // L110~116
    pub fn disable(&mut self, irq: u32);            // L124~130
    // 观测口：
    pub fn is_initialized(&self) -> bool;           // L133~135
    pub fn last_irq(&self) -> u32;                  // L138~140
    pub fn last_op_is_enable(&self) -> bool;        // L143~146
}
```

- **`new(base)` const fn**：可驻留 BSS；纯赋值不触碰 MMIO——host 可构造，init 前无硬件副作用，同 `Gicv3::new` 口径。
- **`init()` / `enable(irq)` / `disable(irq)` 签名与 Gicv3 接口对齐**：便于 X86Hal enable_irq/disable_irq 委托（[hal/x86_64.rs L318~332](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)）。

### 4.2 host 状态记录口径（Cell<bool>/Cell<u32>，同 v0.143.0 GenericTimer）

参照 v0.143.0 GenericTimer host 口径（[hal/clock.rs L54~67](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs)）：host 路径不触内联汇编与真机 MMIO 寄存器写，init/enable/disable 副作用记录至 `core::cell::Cell` 供断言：

| 方法 | host 副作用 | 观测口 |
|------|------------|--------|
| `init()` | `initialized.set(true)` | `is_initialized()` 由 false 转 true |
| `enable(irq)` | `last_irq.set(irq)` + `last_op_enable.set(true)` | `last_irq()` == irq + `last_op_is_enable()` == true |
| `disable(irq)` | `last_irq.set(irq)` + `last_op_enable.set(false)` | `last_irq()` == irq + `last_op_is_enable()` == false |

aarch64 编译时真机路径 cfg 门控排除，Cell 记录零副作用（spec D8）。

### 4.3 真机寄存器路线图（留待真机实现）

x86_64 真机路径的 LocalApic 寄存器操作（`#[cfg(target_arch = "x86_64")]` 门控）留待真机版本补实现，路线图：

| 寄存器 | 偏移 | 语义 | 真机操作 |
|--------|------|------|---------|
| IA32_APIC_BASE | MSR 0x1B | APIC 全局使能基址寄存器 | 使能 bit11（APIC global enable）+ 写基址 0xFEE0_0000（bit12 x2APIC 留待真机） |
| SVR（Spurious Interrupt Vector Register） | MMIO 0xF0 | 伪中断向量寄存器 | 写 vector（如 0xFF）+ bit8 使能 APIC |
| LVT Timer | MMIO 0x320 | Local Vector Table 定时器项 | vector + mode（one-shot/periodic/TSC-deadline）+ mask |
| LVT LINT0/LINT1 | MMIO 0x350/0x360 | Local Vector Table LINT 项 | 留待真机按需配置 |
| ICR（Interrupt Command Register） | MMIO 0x300~0x310 | 中断命令寄存器 | IPI 发送（替代 GICv3 SGIR） |
| EOI（End of Interrupt） | MMIO 0xB0 | 中断结束寄存器 | 中断处理完成写 0 |

- **基址锚定**：`APIC_DEFAULT_BASE = 0xFEE0_0000`（[hal/apic.rs L43](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)，x86_64 架构固定值，与 [platform::x86_qemu::APIC_BASE](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) 同值）。
- **触发条件**：ADR-0001 修订或信创范围扩展时补齐（见 §10）。

---

## 5. X86PageTable 占位

### 5.1 D7 四级类型定义（Pml4/Pdpt/Pd/Pt 各 [u64;512] + new/entries）

源码锚点：[mem/x86_page_table.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（272 行）。

```rust
pub struct Pml4 { entries: [u64; 512] }   // L49~52  第一级（bits[47:39] 索引）
pub struct Pdpt { entries: [u64; 512] }   // L77~80  第二级（bits[38:30] 索引，或 1GB huge page）
pub struct Pd   { entries: [u64; 512] }   // L104~107 第三级（bits[29:21] 索引，或 2MB large page）
pub struct Pt   { entries: [u64; 512] }   // L131~134 第四级（bits[20:12] 索引，4KB 页叶子）

impl Pml4 { pub const fn new() -> Self; pub fn entries(&self) -> &[u64; 512]; }  // L54~64
impl Pdpt { pub const fn new() -> Self; pub fn entries(&self) -> &[u64; 512]; }  // L82~92
impl Pd   { pub const fn new() -> Self; pub fn entries(&self) -> &[u64; 512]; }  // L109~119
impl Pt   { pub const fn new() -> Self; pub fn entries(&self) -> &[u64; 512]; }  // L136~146
// 各级 impl Default 委托 new() 规避 array Default bound（D7）

pub struct X86PageTable { pml4: Pml4 }    // L160~163

impl X86PageTable {
    pub fn new() -> Self;                                          // L167~169
    pub fn map(&mut self, _vaddr: u64, _paddr: u64,
               _flags: u64) -> Result<(), KernelError>;            // L179~182  D2 占位恒 Ok
    pub fn activate(&self) -> Result<(), KernelError>;             // L195~210  D8 CR3 cfg 门控
    pub fn pml4(&self) -> &Pml4;                                   // L213~215
}
impl Default for X86PageTable { fn default() -> Self { Self::new() } }  // L218~222
```

- **四级类型各 `[u64; 512]`**：512 项 × 8B = 4KB/级，下标即 VA 对应 bit 字段的索引（spec D7）。
- **`new()` const fn 全零初始化**：可驻留 BSS；`entries()` 观测口回读 512 项供断言（测试全零 / 真机项写入回读）。
- **`impl Default` 委托 `new()`**：规避 array `[u64; 512]` 的 `Default` trait bound 限制（D7）。

### 5.2 D2 Result 化（map/activate 返回 Result，占位恒 Ok，不 panic）

- **矛盾**：蓝图 §4.4（`phase3.md` L5454）"页表设置失败 panic"与仓库规则 §8.1 禁忌 #3"内核代码用 panic!"冲突（v0.143.0 D2 先例：init 返回 Result 短路错误传播）。
- **决策**：`map` / `activate` 返回 `Result<(), KernelError>`（与 v0.143.0 Hal trait D2 口径一致），占位空操作恒 Ok，不引入 `panic!` / `todo!` / `unimplemented!` / `.unwrap()` / `.expect()`（仓库规则 §8.1 禁忌 #3 + §4.3 no_std 合规）。
- **落地**：[mem/x86_page_table.rs L179~182](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（map 占位 `Ok(())`）、L195~210（activate 占位 `Ok(())` + cfg 门控 CR3 加载留待真机）。

### 5.3 D8 CR3 加载 cfg 门控（asm!("mov cr3,...") 留待真机）

- **决策**：`activate()` 内 x86_64 真机路径的 CR3 加载 `asm!("mov cr3, {pml4_addr}")` `#[cfg(target_arch = "x86_64")]` 门控，本版本占位空操作恒 Ok；host（含 x86_64 host）编译此块为空操作，aarch64 交叉编译此块 cfg 排除零副作用。
- **落地**：[mem/x86_page_table.rs L201~208](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（cfg 门控块占位注释，真机实现时替换为 `unsafe { core::arch::asm!("mov cr3, {}", in(reg) ...) }`，需物理地址转换，属启动链范围）。

### 5.4 真机路线图（四级页表映射 / CR3 加载 / 页表项标志位）

x86_64 长模式四级页表真机实现路线图（留待真机版本）：

| 项 | 真机操作 | 备注 |
|----|---------|------|
| 四级页表映射 | 按 VA 的 PML4/PDPT/PD/PT 索引逐级查表/建表，写入叶子 PTE（`paddr` 帧号 + `flags` 权限位） | 替换 [map()](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs) 占位 Ok |
| CR3 加载 | `mov cr3, {pml4 物理基址}` 加载 PML4 根表物理地址至 CR3 寄存器 | 替换 [activate()](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs) cfg 门控占位 |
| 页表项标志位 | P（Present, bit0）=1 / R/W（Read/Write, bit1）/ U/S（User/Supervisor, bit2）/ PWT/PCD/PAT/G 等 | 真机按 `flags` 参数解释 |
| Huge Page | PDPT 项 bit7=1 → 1GB huge page；PD 项 bit7=1 → 2MB large page | 真机按需启用 |
| PAT（Page Attribute Table） | bit12/PAT/bit7/PSE 组合编码内存类型 | 真机按需配置 |

- **触发条件**：ADR-0001 修订或信创范围扩展时补齐（见 §10）。

---

## 6. 平台配置

### 6.1 x86_qemu 两常量表（spec D10）

源码锚点：[hal/mod.rs L229~236](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（`platform::x86_qemu` 子模块）。

| 常量 | 值 | 语义 | x86_64 地址图依据 |
|------|-----|------|-------------------|
| `COM1_BASE` | `0x3F8` | 8250 UART COM1 基址 | x86_64 端口 IO 0x3F8（IBM PC 兼容机 COM1 固定地址；蓝图 §4.5 + spec D5 平台常量） |
| `APIC_BASE` | `0xFEE0_0000` | Local APIC MMIO 寄存器帧基址 | x86_64 架构固定值（与 [hal/apic.rs APIC_DEFAULT_BASE](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs) 同值；spec D6 APIC MMIO 基址） |

- **与 `apic::APIC_DEFAULT_BASE` 同值**：`platform::x86_qemu::APIC_BASE == apic::APIC_DEFAULT_BASE == 0xFEE0_0000`（双处锚定便于 `X86Hal::new_system()` 经平台常量组构造，host 单测经 `APIC_DEFAULT_BASE` 构造，两路径同值）。
- **真机 PortIo `asm!("out")` 封装留待真机**：本版本 COM1_BASE 仅录入常量，Uart8250 真机路径用 `MmioRegIo` 占位（D5 口径）。

### 6.2 与 qemu_virt（ARM64）并列说明

[hal/mod.rs L203~237](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) `platform` 模块下两同级子模块并列：

| 子模块 | 平台 | 常量数 | 用途 |
|--------|------|--------|------|
| `platform::qemu_virt` | ARM64 QEMU virt | 6（UART_PL011_BASE / GICD_BASE / GICR_BASE / GPIO_PL061_BASE / CNTP_IRQ / CNTV_IRQ） | v0.143.0 Aarch64Hal 真机路径构造 |
| `platform::x86_qemu` | x86_64 QEMU | 2（COM1_BASE / APIC_BASE） | v0.144.0 X86Hal 真机路径构造（cfg 门控，本版本不交叉编译 x86_64） |

- **BSP 演进路线**（v0.143.0 §6.2 口径延续）：新平台 = 新 `platform` 子模块 + Hal trait 实现；本版本新增 `x86_qemu`，后续飞腾/鲲鹏/RISC-V/龙芯按平台新增同级模块。

### 6.3 BSP 演进路线（蓝图 §9 可扩展「支持新平台」）

```
hal/mod.rs::platform
├─ qemu_virt（v0.143.0，ARM64）         ← 6 常量
├─ x86_qemu（v0.144.0，x86_64）          ← 2 常量（本版本新增）
├─ phytium（v0.145.0，飞腾）             ← 待新增
├─ kunpeng（v0.146.0，鲲鹏）             ← 待新增
└─ riscv_loongson（v0.147.0，RISC-V/龙芯） ← 待新增

hal/
├─ aarch64.rs（v0.143.0，Aarch64Hal）    ← ARM64 实现
├─ x86_64.rs（v0.144.0，X86Hal）         ← x86_64 实现（本版本新增）
├─ phytium.rs（v0.145.0）                ← 待新增
├─ kunpeng.rs（v0.146.0）                ← 待新增
└─ riscv.rs / loongson.rs（v0.147.0）    ← 待新增
```

- **trait 签名冻结**（十一方法，v0.143.0 §3.1）：BSP 版本仅需消费 trait 实现，不改动 Hal trait / HalRegistry（D9 Surgical Changes）。
- **平台差异全部收敛于 platform 常量组与 Hal 实现两文件**：新平台接入成本 = 新 platform 模块 + Hal 实现两文件（v0.143.0 §6.2 演进路线落地）。

---

## 7. 测试矩阵

### 7.1 Host 单测（X1~X8 / AP1~AP3 / XPT1~XPT3，已交付）

host 侧执行 `cargo test -p eneros-microkernel`：X1~X8（hal/x86_64.rs）+ AP1~AP3（hal/apic.rs）+ XPT1~XPT3（mem/x86_page_table.rs）共 14 例单测全绿（Task 1~3 实测：lib 全量 216 passed 零回归）。

#### 7.1.1 X1~X8（hal/x86_64.rs 内嵌单测）

源码锚点：[hal/x86_64.rs L423~548](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)。

| 编号 | 测试名 | 验证点 | 结果 |
|------|--------|--------|------|
| X1 | `x1_from_parts_construction_and_observation` | `X86Hal::from_parts` 注入式构造 + `apic()`/`timer()`/`uart()` 观测口回读（apic.base == APIC_DEFAULT_BASE / timer.freq == FREQ_QEMU / uart.base == 0） | PASS |
| X2 | `x2_six_inits_blueprint_order_all_ok` | 六 init 蓝图 §4.3 mermaid 顺序全 Ok（cpu → mem → irq → clock → serial → gpio） | PASS |
| X3 | `x3_serial_putc_tx_log_byte_order` | serial_putc 写入 Uart8250 tx_log 字节序——连续 putc b'X'/b'8'/b'6'，tx_log == b"X86" | PASS |
| X4 | `x4_clock_now_us_injected_and_monotonic` | clock_now_us 注入源换算精度（freq=62_500_000、mock 返回 62_500_000 ⇒ 1_000_000）+ 单调性双读（递增源第二次 ≥ 第一次） | PASS |
| X5 | `x5_clock_deadline_and_disarm` | clock_deadline(5_000_000) 编程记录 last_deadline_ticks == 312_500_000 + clock_init 使 armed/masked 置真后 clock_disarm() 清 armed | PASS |
| X6 | `x6_enable_disable_irq_delegation_and_boundary` | enable_irq(36,0) 委托 apic.enable(36) + disable_irq(36) 委托 apic.disable(36) + 边界 1024/u32::MAX/2000 no-op（D8） | PASS |
| X7 | `x7_local_apic_state_readback` | LocalApic 伪 MMIO 帧回读——init/enable/disable 状态记录经 apic.is_initialized()/last_irq()/last_op_is_enable() 观测口断言 | PASS |
| X8 | `x8_x86_page_table_construction_and_placeholder_ops` | X86PageTable::new 四级空表构造（PML4 512 项全零）+ map 占位 Ok + activate 占位 Ok | PASS |

#### 7.1.2 AP1~AP3（hal/apic.rs 内嵌单测）

源码锚点：[hal/apic.rs L161~192](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)。

| 编号 | 测试名 | 验证点 | 结果 |
|------|--------|--------|------|
| AP1 | `ap1_new_and_base_readback` | `new(base)` 构造 + `base()` 回读 == 输入基址（APIC_DEFAULT_BASE） | PASS |
| AP2 | `ap2_init_records_state` | `init()` 调用后 `is_initialized()` 由 false 转 true（host Cell 口径） | PASS |
| AP3 | `ap3_enable_disable_records_state` | `enable(36)`/`disable(36)` 调用后 `last_irq()` 与 `last_op_is_enable()` 反映最后操作 | PASS |

#### 7.1.3 XPT1~XPT3（mem/x86_page_table.rs 内嵌单测）

源码锚点：[mem/x86_page_table.rs L236~271](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)。

| 编号 | 测试名 | 验证点 | 结果 |
|------|--------|--------|------|
| XPT1 | `xpt1_new_yields_four_level_empty_table` | `new()` 构造 + `pml4().entries()` 全零 + 四级空表占位类型（Pml4/Pdpt/Pd/Pt）均可构造 + 观测口回读全零 | PASS |
| XPT2 | `xpt2_map_placeholder_returns_ok` | `map(0x1000, 0x2000, 0x3)` 返回 Ok（占位恒 Ok，D2 Result 化） | PASS |
| XPT3 | `xpt3_activate_placeholder_returns_ok` | `activate()` 返回 Ok（占位恒 Ok，D7/D8） | PASS |

### 7.2 Host 集成测试（XPI1~XPI6，Task 5 待交付）

> **状态说明**：`tests/x86_test.rs` 为 Task 5 交付物，本文档按 spec.md 测试矩阵落点记录设计验证点；Task 5 完成后将补充实测结果列。

| 编号 | 测试名（设计） | 验证点 | 结果 |
|------|---------------|--------|------|
| XPI1 | `&mut dyn Hal` 十一方法多态分发（X86Hal 注入式） | X86Hal 经 `&mut dyn Hal` 多态分发调用十一方法，六 init 返回 Ok，serial_putc 写入假后端 tx_log，clock_now_us 返回注入源换算值 | Task 5 待交付 |
| XPI2 | 六 init 全链路 Ok + LocalApic 帧回读 + Uart8250 帧回读 | 六 init 全 Ok + apic.is_initialized()/last_irq()/last_op_is_enable() 状态回读 + uart.bus().tx_log 回读 | Task 5 待交付 |
| XPI3 | serial_putc tx_log == b"X86" 字节序 | serial_init 后连续 putc b'X'/b'8'/b'6'，tx_log == b"X86"，THR 回读末字节 b'6' | Task 5 待交付 |
| XPI4 | clock_now_us 注入源换算 + 单调性 + deadline 编程 | 整秒注入 == 1_000_000us + cycles_to_us(125_000_000, 62_500_000) == 2_000_000 + 递增源双读单调 + deadline(5_000_000) ⇒ 312_500_000 ticks | Task 5 待交付 |
| XPI5 | enable/disable IRQ LocalApic 帧回读 + 边界 1024/2000 no-op | enable_irq(36,0) → apic.last_irq()==36 + last_op_is_enable()==true + 边界 1024/2000 no-op（D8） | Task 5 待交付 |
| XPI6 | 跨架构 Hal trait 多态——Aarch64Hal 与 X86Hal 同经 `&mut dyn Hal` 分发 | 同一 `&mut dyn Hal` 槽位分别承载 Aarch64Hal 与 X86Hal 两架构实现，验证 Hal trait 接口冻结后的跨架构可扩展性 | Task 5 待交付 |

### 7.3 不做的测试（D1 降级口径）

依 spec.md「不做的测试」节（D1 ★ ADR-0001 降级口径）：

| 项 | 蓝图 §6/§7 要求 | 不做原因 |
|----|----------------|---------|
| QEMU x86 启动（`tests/x86_boot_test.sh`） | §6 集成「QEMU x86 启动」/ §7 功能「x86 启动」 | D1 ★ ADR-0001：x86 启动链未实现（长模式切换/IDT/CR3），QEMU x86 启动脚本无意义；留待真机版本 |
| aarch64 交叉编译 x86_64 代码 | —（隐含） | D8 cfg 门控：aarch64 编译不含 x86_64 真机路径（rdtsc/PortIo/APIC MMIO/CR3 均门控），零副作用；不安装 x86_64-unknown-none target |
| main.rs x86 冒烟段 | §6 集成（隐含） | D1：x86 启动链未实现，冒烟无意义；host 测试覆盖接口正确性（X1~X8 + XPI1~XPI6） |

---

## 8. Hal trait 跨架构多态

### 8.1 v0.143.0 接口冻结（Hal trait 十一方法 + HalRegistry）

v0.143.0 已冻结 Hal trait 十一方法接口规范（[hal/mod.rs L85~130](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)）：

| 类别 | 方法 | 签名 |
|------|------|------|
| 六 init（返回 `Result<(), KernelError>`，D2） | cpu_init / mem_init / irq_init / clock_init / serial_init / gpio_init | `fn xxx_init(&mut self) -> Result<(), KernelError>` |
| 五运行时 | serial_putc / clock_now_us / clock_deadline / enable_irq / disable_irq | `fn serial_putc(&self, ch: u8)` / `fn clock_now_us(&self) -> u64` / `fn clock_deadline(&mut self, us: u64)` / `fn enable_irq(&mut self, irq: u32, cpu: u8)` / `fn disable_irq(&mut self, irq: u32)` |

配套设施（v0.143.0 冻结）：
- **HalRegistry**（[hal/mod.rs L132~149](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)）：实例 + 全局双口径（`new`/`hal()` + `register_hal`/`hal()` 自由函数）
- **platform::qemu_virt**（[hal/mod.rs L206~224](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)）：ARM64 平台 6 常量
- **Aarch64Hal**（[hal/aarch64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs)）：ARM64 实现（四元组装 gic/timer/uart/gpio）

### 8.2 v0.144.0 BSP 扩展证明（XPI6：Aarch64Hal + X86Hal 同经 `&mut dyn Hal` 分发）

本版本通过新增 `X86Hal` 实现 Hal trait 十一方法，证明 v0.143.0 接口冻结后的 BSP 可扩展性：

- **XPI6 验证场景**（spec.md「跨架构 Hal trait 多态验证」Requirement）：host 测试先以 Aarch64Hal 注入构造的实例调用十一方法，再以 X86Hal 注入构造的实例调用十一方法，两者均通过 trait 对象多态分发完成调用。
- **多态分发机制**：`&mut dyn Hal` 胖指针（vtable + data），X86Hal 与 Aarch64Hal 同为 `Hal` trait 的实现类型，经同一 `&mut dyn Hal` 槽位分发——证明 trait 签名冻结后新平台实现可无缝接入。
- **架构意义**：v0.143.0 接口冻结 → v0.144.0 BSP 扩展证明 → 后续飞腾/鲲鹏/RISC-V/龙芯 BSP 版本仅需新增 platform 模块 + Hal 实现两文件即可挂载新平台（§6.3 演进路线）。

### 8.3 接口冻结→BSP 扩展的架构意义

| 维度 | v0.143.0 接口冻结 | v0.144.0 BSP 扩展证明 |
|------|------------------|---------------------|
| Hal trait | 十一方法签名冻结（D2 Result 化） | X86Hal 消费 trait 实现十一方法（D9 不改 trait） |
| HalRegistry | 实例 + 全局双口径（D10） | X86Hal 可经 `register_hal` 注册（host 测试未触全局，XPI6 用实例分发） |
| platform | qemu_virt 6 常量 | x86_qemu 2 常量同级并列（D10） |
| Hal 实现 | Aarch64Hal 四元组装 | X86Hal 三元组装（无 gpio，x86 无 GPIO） |
| 跨架构多态 | （未验证，仅 ARM64 单实现） | XPI6 双架构同槽分发证明 |

- **新平台接入成本**：新 `platform` 模块 + `Hal` 实现两文件（零改动 Hal trait / HalRegistry / 既有平台实现）。
- **演进预留**：D7 trigger 参数可配置（enable_irq 增 trigger 参数）、D9 GPIO 操作集等留待 v0.145.0/v0.146.0 BSP 版本按需扩展（v0.143.0 §6.2 口径延续）。

---

## 9. D1~D10 偏差表 + 落地修正

> 文档级编号与 spec.md「蓝图偏差决策」表逐字一致；D1 ★ 为本版本核心决策（ADR-0001 降级口径）。

| 编号 | 矛盾 | 决策 | 落地 |
|------|------|------|------|
| **D1 ★** | 蓝图 §1 ★ ADR-0001 降级口径（"x86_64 不在信创认证范围，本版本工作降级为接口占位，不投入生产资源"）vs §3/§6/§7 完整交付物矛盾（§3 列 `x86_boot_test.sh` + `x86_64-unknown-none target` + 完整 Hal 实现，§6「集成：QEMU x86 启动」，§7「功能：x86 启动」） | 以 §1 ★ ADR-0001 口径为准——本版本为**接口占位 + host 可测**，不做完整 x86 启动链。理由：① ADR-0001 为已签署决策态（仓库规范 §5.4），优先级高于蓝图模板交付物；② x86_64 非主架构，投入生产资源做完整长模式切换/IDT/CR3 启动链违背 ADR-0001 收敛原则；③ Karpathy「Simplicity First」——最小代码证明 Hal trait 跨架构多态分发即可 | 交付 X86Hal + LocalApic + X86PageTable 骨架（签名可编译，host 注入可测），不交付 x86_boot_test.sh / x86_64 target 安装 / x86 bootloader；XPI1~XPI6 host 集成测试为 Task 5 交付物 |
| D2 | 蓝图 §4.4「页表设置失败 panic」vs 仓库规则 §8.1 禁忌 #3「内核代码用 panic!」 | `mem_init` 返回 `Result<(), KernelError>`（与 v0.143.0 Hal trait D2 口径一致），页表占位空操作恒 Ok；`X86PageTable::map`/`activate` 同口径返回 `Result`，占位恒 Ok | [mem/x86_page_table.rs L179~182](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（map Ok）、L195~210（activate Ok） |
| D3 | 蓝图 §4.5 X86Hal 的 Hal impl 仅列 9 方法，缺 `clock_deadline`（Hal trait 有十一方法，v0.143.0 冻结） | 补全 `clock_deadline`（委托 `GenericTimer::deadline_us`，占位空操作记 last_deadline_ticks 供断言），十一方法全覆盖 | [hal/x86_64.rs L312~316](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（clock_deadline 委托 timer.deadline_us） |
| D4 | 蓝图 §4.5 `clock_now_us` 用 `rdtsc / 1000` 硬编码（TSC 频率并非固定 1GHz） | 与 v0.143.0 D5 注入式时钟口径一致——`X86Hal` 构造注入 `freq: u64` + `now_cycles: fn() -> u64`（x86_64 = rdtsc 封装，host = mock）；`cycles_to_us` 纯函数复用 `hal::clock::cycles_to_us`（零重复定义）；x86_64 真机路径读 TSC 频率留待真机 | [hal/x86_64.rs L185~192](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（timer 字段）+ L234（new_system 注入 TSC_FREQ_HZ + read_tsc）+ L306~310（clock_now_us 委托） |
| D5 | 蓝图 §4.5 Uart8250 用 `write_volatile` 直接读写端口地址，但 x86 端口 IO 需 `out`/`in` 指令而非 MMIO | Uart8250 封装泛型化 `<B: RegIo>`（复用 v0.143.0 Pl011 口径）——x86_64 真机路径用 PortIo 假后端（`asm!("out")` 封装留待真机，本版本仅类型定义 + host 用 FakeRegIo 缓冲帧可测）；COM1 0x3F8 基址录入平台配置常量 | [hal/x86_64.rs L96~178](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（Uart8250 泛型封装）+ [hal/mod.rs L232](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（COM1_BASE 常量） |
| D6 | 蓝图 §4.5 仅给出 `LocalApic { base: usize }` 结构体，无方法签名；§4.2 说"APIC 中断（替代 GICv3）"但未给 enable/disable 签名 | LocalApic 占位——`new(base)` / `init()` / `enable(irq)` / `disable(irq)` 签名落地（与 Gicv3 接口对齐）；寄存器操作（APIC MMIO 基址 0xFEE00000，LVT 寄存器等）留待真机，host 用 Cell 状态记录可测 | [hal/apic.rs L55~146](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（LocalApic 占位类型 + 签名 + 观测口） |
| D7 | 蓝图 §3 仅给 `X86PageTable { pml4: PhysFrameNum }` 结构体，无方法签名；§4.2 说"x86 四级页表"但未给接口 | X86PageTable 占位——`new()` / `map(vaddr, paddr, flags)` / `activate()` 签名落地；四级页表类型定义（Pml4/Pdpt/Pd/Pt 各为 `[u64; 512]`）；CR3 加载留待真机 | [mem/x86_page_table.rs L49~222](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（四级类型 + X86PageTable + 签名） |
| D8 | x86_64 交叉编译门控 | 所有 x86_64 真机路径（rdtsc / PortIo / APIC MMIO / CR3）均 `#[cfg(target_arch = "x86_64")]` 门控；host 测试用注入式 mock；aarch64 编译不包含 x86_64 代码（cfg 门控零副作用）；不安装 x86_64-unknown-none target | [hal/x86_64.rs L82~83](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（TSC_FREQ_HZ 门控）+ L135~145（Uart8250 init 门控）+ L230（new_system 门控）+ L244（read_tsc 门控）+ [mem/x86_page_table.rs L201~208](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs)（CR3 门控） |
| D9 | 不改 Hal trait（接口已冻结） | v0.143.0 Hal trait 十一方法已冻结锚定 BSP 出口，本版本**仅消费 trait 实现**，不改动 `hal/mod.rs` 的 Hal trait / HalRegistry / platform（Surgical Changes）；仅追加 `pub mod x86_64;` + `pub mod apic;` 两行模块声明 + 头注 | [hal/mod.rs L74](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（`pub mod apic;`）+ L78（`pub mod x86_64;`）；既有 Hal trait / HalRegistry / aarch64 / clock / serial / gpio / qemu_virt 零改动 |
| D10 | x86 platform 配置模块 | `hal/platform` 新增 `x86_qemu` 子模块（COM1 0x3F8 / APIC_BASE 0xFEE0_0000 常量），与既有 `qemu_virt`（ARM64）并列；BSP 版本按平台新增同级模块 | [hal/mod.rs L229~236](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（platform::x86_qemu 两常量） |

---

## 10. 后续真机路线图

x86_64 真机路径留待 ADR-0001 修订或信创范围扩展时补齐（蓝图 §1 ★ ADR-0001 降级口径）。真机路线图：

### 10.1 长模式切换（实模式 → 保护模式 → 长模式）

- **实模式 → 保护模式**：加载 GDT（Global Descriptor Table）+ 设置 CR0.PE=1 + 远跳转至保护模式代码段
- **保护模式 → 长模式**：加载临时 PML4 页表 + 设置 CR4.PAE=1 + 设置 IA32_EFER.LME=1 + 设置 CR0.PG=1（启用分页）→ 进入长模式
- **参考**：蓝图 §4.3 mermaid 图（`phase3.md` L5445~5452）「BIOS/UEFI 加载 → 实模式 → 保护模式 → 长模式 → 内核入口 _start → APIC init → 页表启用 → idle」

### 10.2 IDT 加载（中断描述符表，与 VBAR 不同）

- **IDT 结构**：256 项 × 16B 门描述符（中断门 / 陷阱门 / 任务门），每项包含段选择子 + 偏移 + 类型
- **加载指令**：`lidt [idt_ptr]`（加载 IDTR 寄存器，x86_64 专属，与 ARM VBAR_EL1 机制不同）
- **坑点**（蓝图 §8）：IDT 格式与 VBAR 不同——ARM VBAR 是异常向量表基地址（每项 128B 跳转指令序列），x86 IDT 是门描述符数组（每项 16B 结构）
- **落地**：替换 [X86Hal::cpu_init](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs) 占位 Ok（L259~264），补 `load_idt` 函数 + IDT 表构建

### 10.3 CR3 页表启用

- **CR3 加载**：`mov cr3, {pml4 物理基址}` 加载 PML4 根表物理地址至 CR3 寄存器
- **页表项标志位**：P（Present, bit0）=1 / R/W（Read/Write, bit1）/ U/S（User/Supervisor, bit2）/ PWT/PCD/PAT/G 等
- **Huge Page**：PDPT 项 bit7=1 → 1GB huge page；PD 项 bit7=1 → 2MB large page
- **落地**：替换 [X86PageTable::activate](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs) cfg 门控占位（L201~208），补 `unsafe { core::arch::asm!("mov cr3, {}", in(reg) ...) }`（需物理地址转换，属启动链范围）

### 10.4 x86 bootloader（v0.129.0 等价 x86 版本）

- **蓝图 §4.3 mermaid**：BIOS/UEFI 加载 → 实模式 → 保护模式 → 长模式 → 内核入口 _start
- **等价版本**：v0.129.0 为 ARM64 bootloader（EL2 → EL1 + 装载 VBAR + 跳转内核），x86 bootloader 需实现等价的长模式切换 + IDT 加载 + CR3 启用
- **触发条件**：ADR-0001 修订或信创范围扩展时补齐

### 10.5 x86_64-unknown-none target 安装 + tests/x86_boot_test.sh

- **target 安装**：`rustup target add x86_64-unknown-none` + `.cargo/config.toml` 追加 x86 target 配置
- **QEMU x86 启动脚本**：`tests/x86_boot_test.sh`（蓝图 §3 交付物，本版本不交付，D1 降级口径）
- **冒烟段**：[main.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) x86 冒烟段（蓝图 §6 集成，本版本不交付）
- **触发条件**：ADR-0001 修订或信创范围扩展时补齐

### 10.6 触发条件汇总

| 项 | 触发条件 | 落地版本 |
|----|---------|---------|
| 长模式切换 / IDT 加载 / CR3 启用 / x86 bootloader | ADR-0001 修订或信创范围扩展 | 后续真机版本 |
| x86_64-unknown-none target 安装 + tests/x86_boot_test.sh + main.rs x86 冒烟段 | 同上 | 后续真机版本 |
| LocalApic 寄存器操作（APIC MMIO 写） | 同上 | 后续真机版本 |
| X86PageTable 四级页表项写入 + CR3 加载 | 同上 | 后续真机版本 |
| Uart8250 PortIo `asm!("out")` 封装 | 同上 | 后续真机版本 |
| TSC 频率校准（CPUID 0x15 读取或启动期标定） | 同上 | 后续真机版本 |

---

## 11. 源码链接

> 所有源码引用用 `file:///` 绝对路径含行号锚点（参照 [hal_design.md](./hal_design.md) §13 风格）。

| 项 | 路径（含行号锚点） |
|----|-------------------|
| LocalApic 占位类型（`APIC_DEFAULT_BASE` L43 + `LocalApic` 结构体 L55~66 + `new`/`base`/`init`/`enable`/`disable` L73~130 + 观测口 L133~146 + AP1~AP3 单测 L161~192） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs) |
| X86Hal + Hal trait 实现（常量 L70~83 + `Uart8250<B>` 泛型封装 L96~178 + `X86Hal<B>` 三元组装 L185~192 + `from_parts`/观测口/`clock_disarm` L194~222 + `new_system` cfg 门控 L224~238 + `read_tsc` cfg 门控 L244~256 + Hal trait 十一方法实现 L258~333 + X1~X8 单测 L423~548） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs) |
| X86PageTable 占位类型（`Pml4` L49~70 + `Pdpt` L77~98 + `Pd` L104~125 + `Pt` L131~152 + `X86PageTable` L160~222 + `new`/`map`/`activate`/`pml4` L165~215 + CR3 cfg 门控 L201~208 + XPT1~XPT3 单测 L236~271） | [file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/x86_page_table.rs) |
| Hal trait 十一方法（L85~130）+ HalRegistry（L132~149）+ 全局 register_hal/hal()（L174~199）+ platform::qemu_virt 六常量（L206~224）+ platform::x86_qemu 两常量（L229~236）+ D1~D10 头注（L1~69）+ 模块挂载 `pub mod apic;`（L74）/ `pub mod x86_64;`（L78） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) |
| 集成测试 XPI1~XPI6（Task 5 待交付，spec 测试矩阵落点见 §7.2） | [file:///e:/EnerOS/crates/kernel/microkernel/tests/x86_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/x86_test.rs) |
| GenericTimer 复用（v0.143.0 注入式时钟，D4 复用来源：`GenericTimer` 结构体 L54~67 + `new`/`new_system`/`now_us`/`deadline_us`/`disarm`/`enable_masked` L69~184 + `cycles_to_us` 纯函数 L189~196） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs) |
| Aarch64Hal 对比参考（v0.143.0 ARM64 实现，四元组装 gic/timer/uart/gpio + Hal trait 十一方法实现 + `clock_disarm` 固有方法） | [file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs) |
| lib.rs 模块挂载（`pub mod hal;` L67 + `pub mod mem;` L71 + v0.144.0 头注交付物 L34） | [file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) |
| mem/mod.rs 模块挂载（`pub mod x86_page_table;` 字母序追加，Task 2 已挂载） | [file:///e:/EnerOS/crates/kernel/microkernel/src/mem/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/mem/mod.rs) |
| 复用设施：RegIo / MmioRegIo（v0.129.0 eneros_bootloader，Uart8250 泛型封装 trait 抽象来源） | [file:///e:/EnerOS/crates/kernel/bootloader/src/serial.rs](file:///e:/EnerOS/crates/kernel/bootloader/src/serial.rs) |
| 复用设施：KernelError（`mem_init`/`map`/`activate` 返回 Result 错误类型来源） | [file:///e:/EnerOS/crates/kernel/microkernel/src/error.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/error.rs) |
| 前序设计文档：v0.143.0 HAL 接口与 ARM64 实现（Hal trait 十一方法接口冻结来源 + D 表与测试矩阵格式先例） | [./hal_design.md](./hal_design.md) |
| 蓝图依据：phase3.md §v0.144.0（§1 版本目标 ★ ADR-0001 降级口径 L5403 + §3 交付物清单 L5421~5432 + §4.5 关键代码 L5457~5505） | [file:///e:/EnerOS/蓝图/phase3.md](file:///e:/EnerOS/蓝图/phase3.md) |
| Spec 文档：v0.144.0 x86_64 BSP（接口占位）Spec（D1~D10 决策表 + 内存预算 + 测试矩阵 + 验收标准） | [file:///e:/EnerOS/.trae/specs/develop-v14400-x86-bsp/spec.md](file:///e:/EnerOS/.trae/specs/develop-v14400-x86-bsp/spec.md) |
| Tasks 文档：v0.144.0 Task 1~7（Task 1 apic.rs / Task 2 x86_page_table.rs / Task 3 x86_64.rs / Task 4 模块挂载 / Task 5 x86_test.rs / Task 6 本文档 / Task 7 版本同步） | [file:///e:/EnerOS/.trae/specs/develop-v14400-x86-bsp/tasks.md](file:///e:/EnerOS/.trae/specs/develop-v14400-x86-bsp/tasks.md) |
