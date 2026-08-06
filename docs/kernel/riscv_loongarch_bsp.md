# EnerOS v0.147.0 RISC-V BSP + 龙芯 BSP（接口占位 — P3-E 终版本）设计文档（RiscvHal standalone 三元组装 / LoongArchHal standalone 二元组装 / Plic/Clint/LoongsonPch Cell 占位 / Uart8250 跨平台复用 / riscv_virt + loongarch_qemu 平台配置）

> 版本：v0.147.0
> 日期：2026-07-29
> 状态：已交付（host 单测 R1~R10 / L1~L10 全绿 + PPI1~PPI7 集成测试全绿 + aarch64 release 交叉编译 cfg 门控零副作用——本版本接口占位口径无 cfg 门控真机路径，零副作用天然保证）
> 适用范围：EnerOS 自研内核 RISC-V + 龙芯 BSP 接口占位子系统（RiscvHal standalone Plic/Clint/UartSifive 三元组装 + LoongArchHal standalone LoongsonPch/Uart8250 二元组装 + D4 Uart8250 跨平台复用 + riscv_virt + loongarch_qemu 平台常量组；ADR-0001 BSP 收敛降级口径接口占位 + host 可测骨架，不保留 cfg 门控真机寄存器写路径 + 不做真机验证）
> 蓝图依据：`蓝图/phase3.md` §v0.147.0（§1 版本目标 ★ ADR-0001 BSP 收敛 / §2 前置依赖 / §3 交付物清单 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-E HAL 多平台线终版本（第 5 版）**（v0.143.0 HAL 接口与 ARM64 实现 → v0.144.0 x86_64 BSP 接口占位 → v0.145.0 飞腾 BSP → v0.146.0 鲲鹏 BSP → **v0.147.0 RISC-V + 龙芯 BSP 接口占位**，五版之五），六平台完整覆盖出口（双国产平台证据：飞腾 + 鲲鹏；开发平台证据：ARM64 QEMU；降级平台证据：x86_64 + RISC-V + 龙芯）
> 配套文档：[hal_design.md](./hal_design.md)（v0.143.0 HAL 接口与 ARM64 实现设计文档，前序版本，Hal trait 十一方法接口冻结来源）、[x86_bsp.md](./x86_bsp.md)（v0.144.0 x86_64 BSP 接口占位设计文档，前序版本，D1 接口占位模板来源——本版本 D1 同属降级口径）、[phytium_bsp.md](./phytium_bsp.md)（v0.145.0 飞腾 BSP 设计文档，前序版本，三层口径对比参考）、[kunpeng_bsp.md](./kunpeng_bsp.md)（v0.146.0 鲲鹏 BSP 设计文档，前序版本，PPI7 多态扩展先例——本版本 PPI7 六平台扩展基线）

---

## 1. 版本目标

### 1.1 ★ ADR-0001 BSP 收敛（蓝图 §1 逐字引用）

> ★ **ADR-0001 BSP 收敛**：原 5 平台（x86/飞腾/鲲鹏/RISC-V/龙芯）收敛为 2 生产平台（飞腾 + 鲲鹏）+ 1 开发平台（ARM64 QEMU）。信创认证是产品+供应链+材料+测试体系工程，非"做完内核就过"。**RISC-V 与龙芯不在信创认证范围，本版本工作降级为接口占位，不投入生产资源**。

蓝图 §1（`蓝图/phase3.md` L5835）原样。本降级口径为本版本**最高优先级决策**（仓库规范 §5.4 ADR-0001 已签署决策态），高于蓝图模板交付物（§3 交付物清单 / §6 测试计划 / §7 验收标准中的「四架构覆盖 / QEMU riscv64 启动 / 龙芯真机启动」项均按本口径降级，见 §2 偏差表与 §9 D1/D10/D11 决策）。

### 1.2 D1 接口占位口径（与 v0.144.0 X86Hal D1 一致，非三层口径）

v0.143.0 已冻结 `Hal` trait 十一方法接口规范。v0.144.0 已交付 `X86Hal` 接口占位（D1 降级口径）。v0.145.0 已交付 `PhytiumHal` 飞腾 BSP（★ 信创核心，三层口径）。v0.146.0 已交付 `KunpengHal` 鲲鹏 BSP（★ 信创核心，三层口径，PPI7 验证四平台多态）。蓝图 §v0.147.0 §1 明确 RISC-V 与龙芯不在信创认证范围，本版本降级为接口占位。本版本交付（**D1 接口占位口径，同 v0.144.0 X86Hal，根本不同于 v0.145.0/v0.146.0 三层口径**）：

1. **接口占位**：`RiscvHal` / `LoongArchHal` 结构体 + `impl Hal` 十一方法全量落地（占位 `Ok(())` / no-op），host 可测骨架（注入式构造 + Cell 状态记录）。
2. **不保留 cfg 门控真机寄存器写路径**——RISC-V/龙芯不在信创认证范围，不投入生产资源（与 v0.145.0/v0.146.0 三层口径的第一层「真机路径保留」根本不同；与 v0.144.0 X86Hal D8 cfg 门控编译验证口径一致——仅保留类型定义 + host 占位可测，不保留真机寄存器写路径）。
3. **不做真机验证**——不安装 riscv64/loongarch64 target，不做 QEMU riscv64 启动，不做龙芯真机启动。
4. **六平台多态验证**——PPI7 扩展为六平台（Aarch64Hal + X86Hal + PhytiumHal + KunpengHal + RiscvHal + LoongArchHal），证明 Hal trait 接口冻结后跨架构可扩展至第六平台。

### 1.3 P3-E HAL 多平台线终版本定位

- **业务价值**：完成 P3-E HAL 多平台线五版本（v0.143.0~v0.147.0）六平台覆盖——双国产生产平台（飞腾 + 鲲鹏）+ 1 开发平台（ARM64 QEMU）+ 3 降级平台（x86_64 + RISC-V + 龙芯）的完整 HAL 多态证据。
- **Phase 定位**：**P3-E 终版本出口**——证明 v0.143.0 接口冻结后跨架构+跨平台可扩展性达到完整覆盖（蓝图 §1「P3-E 终版本，完成多平台覆盖」原样）。
- **出口关联**：六平台 Hal trait 多态分发验证通过 → 信创认证代码化证据输入（飞腾 + 鲲鹏双国产平台）+ 跨架构可扩展性完整证据（六平台覆盖 ARM64/x86_64/RISC-V/LoongArch 全部架构）。

---

## 2. 交付物清单

对照蓝图 §3（`phase3.md` L5854~L5861）逐项标注 D1/D10/D11 降级偏差：

| 蓝图 §3 交付物 | 蓝图要求 | 实际交付 | 偏差决策 |
|---------------|---------|---------|---------|
| 代码：`kernel/src/hal/riscv.rs` | RISC-V HAL 完整实现 | [riscv.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs)（508 行：`RiscvHal<B: RegIo = MmioRegIo>` 三字段 standalone 组合（D3——不组合 Aarch64Hal）+ `Plic { base, initialized: Cell<bool> }` + `Clint { base, armed: Cell<bool> }` + `UartSifive<B: RegIo>` 复用 `eneros_bootloader::RegIo` + Hal trait 十一方法占位 + from_parts 注入式 + 观测口 + R1~R10 单测） | D1 接口占位；D3 standalone；D5 Cell 占位；D12 单文件 |
| 代码：`kernel/src/hal/loongarch.rs` | 龙芯 HAL 完整实现 | [loongarch.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs)（500 行：`LoongArchHal<B: RegIo = MmioRegIo>` 两字段 standalone 组合（D3）+ `LoongsonPch { base, initialized: Cell<bool> }` + `Uart8250<B>` **复用 `x86_64::Uart8250`**（D4 Simplicity First，零重复定义）+ Hal trait 十一方法占位 + from_parts 注入式 + 观测口 + L1~L10 单测） | D1 接口占位；D3 standalone；D4 Uart8250 复用；D5 Cell 占位；D12 单文件 |
| 文档：`docs/kernel/riscv_loongarch_bsp.md` | RISC-V + 龙芯 BSP 设计文档 | 本文档 | 无（路径同蓝图 D8 修正） |
| 测试：`tests/riscv_boot.sh` + 龙芯真机启动 | QEMU riscv64 启动 + 龙芯真机启动 | **不交付**（D1/D10/D11 降级口径） | D1 ★ ADR-0001：RISC-V/龙芯不在信创认证范围，不投入生产资源；D10 不做 QEMU riscv64 启动；D11 不做龙芯真机启动 |
| 配置：`configs/riscv-virt.dts` + `configs/loongarch-qemu.dts` | DTB 占位 | **不交付**（D1 降级口径——不保留真机路径，无 DTB 占位需求） | D1：与 v0.145.0/v0.146.0 D7/D8 三层口径根本不同——接口占位不投入生产资源 |
| 模块挂载：`hal/mod.rs` | — | [hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) L78 追加 `pub mod loongarch;` + L80 追加 `pub mod riscv;`（字母序）+ L312 `platform::riscv_virt` + L339 `platform::loongarch_qemu` 子模块 + L1 头注更新为「v0.143.0 + v0.144.0 + v0.145.0 + v0.146.0 + v0.147.0」 | D9 Surgical Changes：既有 Hal trait / HalRegistry / aarch64 / clock / serial / gpio / x86_64 / apic / phytium / kunpeng / qemu_virt / x86_qemu / phytium_d2000 / kunpeng_920 子模块零改动 |
| 头注：[lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) | — | lib.rs L36 头注交付物追加 v0.147.0 riscv + loongarch 行（RiscvHal + LoongArchHal + Plic/Clint/UartSifive/LoongsonPch 占位 + D1 接口占位口径 + PPI7 六平台多态说明） | D9 Surgical Changes：仅追加头注一行 |
| 集成测试：`tests/riscv_loongarch_test.rs` | PPI1~PPI7 | [riscv_loongarch_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs)（985 行：host 注入式集成测试 PPI1~PPI7，含 PPI7 ★ 跨架构六平台多态——Aarch64Hal + X86Hal + PhytiumHal + KunpengHal + RiscvHal + LoongArchHal 同经 `&mut dyn Hal` 分发六 init 全 Ok） | D1 host 可测骨架；D7 PPI7 核心验收点 |

**spec 分类口径对照**（spec.md「What Changes」）：实现类（RiscvHal / LoongArchHal）→ §3/§4；占位类（Plic / Clint / LoongsonPch / UartSifive）→ §5；配置类（riscv_virt + loongarch_qemu 平台常量）→ §6；文档类（本文档）→ 本文档；测试类（R1~R10 + L1~L10 单测 + PPI1~PPI7 集成测试）→ §7。

---

## 3. RiscvHal 设计

### 3.1 三元组装结构图（蓝图 §3 + spec D3 standalone）

```rust
pub struct RiscvHal<B: RegIo = MmioRegIo> {
    plic: Plic,                // D5 占位——PLIC 中断控制器（init/enable/disable 签名落地，Cell 状态记录）
    clint: Clint,              // D5 占位——CLINT 定时器（init/arm_deadline/disarm 签名落地，Cell 状态记录）
    uart: UartSifive<B>,       // D5 泛型封装——SiFive UART 16550 兼容（host = FakeRegIo 缓冲帧）
}
```

源码锚点：[riscv.rs L383~390](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L383-L390)。

**D3 standalone**（与 v0.144.0 X86Hal 口径一致，根本不同于 v0.145.0/v0.146.0 组合模式）：RISC-V 是完全不同的指令集架构（RV64GCSU），不能复用 Aarch64Hal 的 ARMv8 驱动（GICv3/PL011/GenericTimer），故 `RiscvHal` 自有三元组（Plic + Clint + UartSifive），十一方法直接实现（占位），不委托 `self.base.*()`（与 PhytiumHal/KunpengHal 委托 Aarch64Hal 模式根本不同）。

### 3.2 from_parts 注入式构造 + 观测口

```rust
impl<B: RegIo> RiscvHal<B> {
    pub fn from_parts(plic: Plic, clint: Clint, uart: UartSifive<B>) -> Self { ... }
    pub fn plic(&self) -> &Plic;        // 观测口：base/is_initialized 回读
    pub fn clint(&self) -> &Clint;      // 观测口：base/is_armed 回读
    pub fn uart(&self) -> &UartSifive<B>; // 观测口：bus/tx_log 回读
    pub fn clock_disarm(&mut self);     // 固有方法（非 trait 项）：委托 clint.disarm
}
```

`from_parts` 是 host 测试统一入口（D5 注入式 host 可测化口径，同 X86Hal::from_parts）；接口占位口径不保留真机路径（spec D1），故无 `new_system()` 真机构造方法（与 v0.144.0 X86Hal 不同——x86_64 保留 cfg 门控真机路径，RISC-V 接口占位口径不保留）。

### 3.3 Hal trait 十一方法占位表（v0.143.0 接口冻结，本版本消费实现）

| Hal trait 方法 | RiscvHal 实现 | 联动 |
|---------------|--------------|------|
| `cpu_init` | 占位 `Ok(())`——不设置 `stvec`（D2 不引入 `asm!` 宏，真机路径留待真机版本） | — |
| `mem_init` | 占位 `Ok(())`——RISC-V Sv39/Sv48 页表留待真机版本 | — |
| `irq_init` | 委托 `self.plic.init()` 后 `Ok(())` | ★ Plic 联动 |
| `clock_init` | 委托 `self.clint.init()` 后 `Ok(())`（init 记录 `armed=false` 初始状态） | ★ Clint 联动 |
| `serial_init` | 委托 `self.uart.init()` 后 `Ok(())`（占位空操作） | — |
| `gpio_init` | 占位 `Ok(())`——RISC-V 平台无通用 GPIO 控制器（同 x86_64.rs 占位口径） | — |
| `serial_putc` | 委托 `self.uart.putc(ch)`——轮询 LSR THRE + 写 THR；host FakeRegIo 记录至 tx_log | — |
| `clock_now_us` | 占位返回 `0`——D2 不引入 `asm!("rdtime")` 宏（避免架构相关编译失败） | — |
| `clock_deadline` | 委托 `self.clint.arm_deadline(us)`——Cell 记录 `armed=true`（D2 补全，蓝图 §4.5 缺） | ★ Clint 联动 |
| `enable_irq` | `if irq < IRQ_LIMIT { self.plic.enable(irq) }`——`_cpu` 占位未使用（PLIC 内部路由） | D8 边界防御：irq ≥ 1024 no-op |
| `disable_irq` | `if irq < IRQ_LIMIT { self.plic.disable(irq) }` | D8 边界防御：同 enable_irq |

源码锚点：[riscv.rs L422~503](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L422-L503)（Hal trait impl 块）。`IRQ_LIMIT = 1024`（[riscv.rs L128](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L128)）。

### 3.4 内存预算（蓝图 §5.6 + spec.md 口径）

| 组件 | 尺寸 | 说明 |
|------|------|------|
| `RiscvHal` | **≈48B** | `Plic` 16B + `Clint` 16B + `UartSifive` 16B |
| `Plic` | **≈16B** | `base: usize` 8B + `initialized: Cell<bool>` 1B + 对齐填充 7B |
| `Clint` | **≈16B** | `base: usize` 8B + `armed: Cell<bool>` 1B + 对齐填充 7B |
| `UartSifive` | **≈16B** | `bus: RefCell<B>` + `base: usize`（同 Uart8250 口径） |
| hal 子系统 BSS 总计 | **< 20KB** | RiscvHal 不驻留 BSS——接口占位无全局注册（host 测试用 `Box` 承载） |

---

## 4. LoongArchHal 设计

### 4.1 二元组装结构图（蓝图 §3 + spec D3 standalone + D4 Uart8250 复用）

```rust
use super::x86_64::Uart8250;  // D4 复用——8250 UART 标准跨平台复用，零重复定义

pub struct LoongArchHal<B: RegIo = MmioRegIo> {
    pch: LoongsonPch,          // D5 占位——龙芯平台控制器（init 签名落地，Cell 状态记录）
    uart: Uart8250<B>,         // D4 复用 x86_64::Uart8250（Simplicity First——8250 UART 标准跨平台复用）
}
```

源码锚点：[loongarch.rs L78](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L78)（`use super::x86_64::Uart8250;` 导入）+ [loongarch.rs L162~168](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L162-L168)（结构体定义）。

**D3 standalone**（同 RiscvHal 口径）：LoongArch 与 ARMv8 指令集不兼容，不能组合 Aarch64Hal，自有二元组（LoongsonPch + Uart8250），十一方法直接实现（占位）。

**D4 Uart8250 跨平台复用**（Simplicity First）：8250 UART 是标准 IP，x86_64/龙芯/RISC-V 均可使用，复用避免 DRY 违规；`Uart8250` 已是泛型 `<B: RegIo>`，跨平台天然兼容。L8 测试经编译时类型断言 `let uart: &super::super::x86_64::Uart8250<FakeRegIo> = loong.uart();` 验证同类型（[loongarch.rs L456~465](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L456-L465)）。

### 4.2 from_parts 注入式构造 + 观测口

```rust
impl<B: RegIo> LoongArchHal<B> {
    pub fn from_parts(pch: LoongsonPch, uart: Uart8250<B>) -> Self { ... }
    pub fn pch(&self) -> &LoongsonPch;     // 观测口：base/is_initialized 回读
    pub fn uart(&self) -> &Uart8250<B>;    // 观测口：bus/tx_log 回读（类型为 x86_64::Uart8250<B>——D4 复用证据）
    pub fn clock_disarm(&mut self);        // 固有方法（非 trait 项）：占位空操作（无 timer 字段）
}
```

### 4.3 Hal trait 十一方法占位表

| Hal trait 方法 | LoongArchHal 实现 | 联动 |
|---------------|-------------------|------|
| `cpu_init` | 占位 `Ok(())`——LoongArch CPU 初始化（异常向量 + TLB）留待真机版本（D1 不引入 `asm!` 宏） | — |
| `mem_init` | 占位 `Ok(())`——LoongArch 页表留待真机版本 | — |
| `irq_init` | 委托 `self.pch.init()` 后 `Ok(())` | ★ LoongsonPch 联动 |
| `clock_init` | 占位 `Ok(())`——LoongArch 定时器使能留待真机版本 | — |
| `serial_init` | 委托 `self.uart.init()` 后 `Ok(())`（D4 复用 Uart8250::init，占位空操作） | — |
| `gpio_init` | 占位 `Ok(())`——LoongArch GPIO 留待真机版本 | — |
| `serial_putc` | 委托 `self.uart.putc(ch)`——D4 复用 Uart8250::putc（轮询 LSR THRE + 写 THR） | — |
| `clock_now_us` | 占位返回 `0`——D2 不引入 `asm!("rdtime.d")` 宏 | — |
| `clock_deadline` | 占位 no-op——D2 不引入 `asm!` 宏 | — |
| `enable_irq` | `if irq < IRQ_LIMIT { /* 占位 */ }`——PCH 中断使能寄存器写留待真机版本 | D8 边界防御：irq ≥ 1024 no-op |
| `disable_irq` | `if irq < IRQ_LIMIT { /* 占位 */ }` | D8 边界防御：同 enable_irq |

源码锚点：[loongarch.rs L201~285](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L201-L285)（Hal trait impl 块）。`IRQ_LIMIT = 1024`（[loongarch.rs L84](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L84)）。

### 4.4 内存预算

| 组件 | 尺寸 | 说明 |
|------|------|------|
| `LoongArchHal` | **≈32B** | `LoongsonPch` 16B + `Uart8250` 16B（复用 x86_64::Uart8250） |
| `LoongsonPch` | **≈16B** | `base: usize` 8B + `initialized: Cell<bool>` 1B + 对齐填充 7B |
| `Uart8250` | **≈16B** | `bus: RefCell<B>` + `base: usize`（复用 x86_64::Uart8250） |
| hal 子系统 BSS 总计 | **< 20KB** | LoongArchHal 不驻留 BSS——接口占位无全局注册 |

---

## 5. 占位类型设计（D5 Plic/Clint/LoongsonPch/UartSifive Cell 状态记录 + 观测口）

| 类型 | 字段 | 方法 | Cell 状态记录 |
|------|------|------|--------------|
| `Plic` | `base: usize` + `initialized: Cell<bool>` | `new(base)` const fn + `base()` + `is_initialized()` + `init()`（`initialized.set(true)`）+ `enable(_irq)` no-op + `disable(_irq)` no-op | init 后 `is_initialized()` 由 false 转 true（参照 v0.144.0 LocalApic 口径） |
| `Clint` | `base: usize` + `armed: Cell<bool>` | `new(base)` const fn + `base()` + `is_armed()` + `init()`（`armed.set(false)` 初始）+ `arm_deadline(_ticks)`（`armed.set(true)`）+ `disarm()`（`armed.set(false)`） | arm_deadline 后 `is_armed()` 转 true；disarm 后转 false（参照 GenericTimer armed 口径） |
| `LoongsonPch` | `base: usize` + `initialized: Cell<bool>` | `new(base)` const fn + `base()` + `is_initialized()` + `init()`（`initialized.set(true)`，无 unsafe write_volatile） | init 后 `is_initialized()` 由 false 转 true |
| `UartSifive<B: RegIo>` | `bus: RefCell<B>` + `base: usize` | `from_bus(bus)` + `base()` + `bus()` 观测口 + `init()` 空操作 + `putc(ch)` 轮询 LSR THRE + 写 THR | 复用 `eneros_bootloader::RegIo` trait 抽象（同 v0.144.0 Uart8250 口径），host = FakeRegIo 缓冲帧 |

源码锚点：[riscv.rs L145~203](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L145-L203)（UartSifive）+ [riscv.rs L216~285](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L216-L285)（Plic）+ [riscv.rs L297~370](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L297-L370)（Clint）+ [loongarch.rs L98~142](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L98-L142)（LoongsonPch）。

**安全口径**：全文件零 unsafe（host 路径纯 Cell 记录 + RegIo 抽象，无真机寄存器写——接口占位口径不保留真机路径，spec D1 + D2 + D6）；无 `#[cfg(target_arch = "riscv64")]` / `#[cfg(target_arch = "loongarch64")]` 真机路径；无 `asm!` 宏；无 `unsafe write_volatile`。`bus` 以 `RefCell` 持有——`putc(&self)` 为共享引用语义（匹配 `Hal::serial_putc` 签名），而 `RegIo::write32(&mut self)` 需独占引用，`RefCell` 提供零 unsafe 的内可变性（同 Uart8250 D1 口径）。

---

## 6. 平台配置（riscv_virt + loongarch_qemu 常量 + 六平台并列）

### 6.1 riscv_virt 四常量（spec D3/D6）

源码锚点：[hal/mod.rs L312~328](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L312-L328)（`platform::riscv_virt` 子模块）。

| 常量 | 值 | 语义 | 地址图依据 |
|------|-----|------|-----------|
| `UART_SIFIVE_BASE` | `0x1000_0000` | SiFive UART 基址占位（16550 兼容，复用 `UartSifive` 封装） | QEMU riscv virt 地址图 |
| `PLIC_BASE` | `0x0C00_0000` | PLIC 寄存器帧基址占位 | QEMU riscv virt 地址图 |
| `CLINT_BASE` | `0x0200_0000` | CLINT 寄存器帧基址占位 | QEMU riscv virt 地址图 |
| `CNTP_IRQ_RISCV` | `5`（u32） | RISC-V 通用定时器中断号（supervisor timer IRQ = 5） | QEMU riscv virt machine |

### 6.2 loongarch_qemu 三常量（spec D3/D4/D6）

源码锚点：[hal/mod.rs L339~350](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L339-L350)（`platform::loongarch_qemu` 子模块）。

| 常量 | 值 | 语义 | 地址图依据 |
|------|-----|------|-----------|
| `UART_8250_BASE` | `0x1FE0_01E0` | 8250 UART 基址占位（D4 跨平台复用 `x86_64::Uart8250`） | QEMU loongarch virt 地址图 |
| `PCH_BASE` | `0x1FE0_0000` | LoongsonPch 寄存器帧基址占位 | QEMU loongarch virt 地址图 |
| `IRQ_LIMIT` | `1024`（u32） | IRQ 号上限（与 aarch64.rs/x86_64.rs/riscv.rs `IRQ_LIMIT` 同值——HAL 层守卫双保险） | HAL 层守卫常量 |

占位地址依据 QEMU riscv/loongarch virt 机器地址图（待真机调试确认）。

### 6.3 六平台常量并列对照

| 平台 | UART 基址 | 中断控制器 | 时钟 | 备注 |
|------|----------|-----------|------|------|
| `qemu_virt` | 0x0900_0000（PL011） | GICD 0x0800_0000 / GICR 0x080A_0000 | CNTP=30 / CNTV=27 | ARM64 QEMU 开发平台（v0.143.0） |
| `x86_qemu` | 0x3F8（COM1） | APIC 0xFEE0_0000 | — | x86_64 降级平台（v0.144.0 接口占位） |
| `phytium_d2000` | 0x2800_0000（PL011） | GICD 0x2f00_0000 / GICR 0x2f10_0000 | CNTP=30 / CNTV=27 | 飞腾生产平台（v0.145.0 三层口径） |
| `kunpeng_920` | 0x9FFF_F000（PL011） | GICD 0x4_0080_0000 / GICR 0x4_00A0_0000 | CNTP=30 / CNTV=27 | 鲲鹏生产平台（v0.146.0 三层口径） |
| `riscv_virt` | 0x1000_0000（SiFive） | PLIC 0x0C00_0000 | CLINT 0x0200_0000 / CNTP_IRQ_RISCV=5 | RISC-V 降级平台（v0.147.0 接口占位） |
| `loongarch_qemu` | 0x1FE0_01E0（8250） | PCH 0x1FE0_0000 | IRQ_LIMIT=1024 | 龙芯降级平台（v0.147.0 接口占位） |

---

## 7. 测试矩阵

### 7.1 host 单测（hal/riscv.rs 内 #[cfg(test)]，R1~R10）

源码锚点：[riscv.rs L505~756](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L505-L756)。

| 编号 | 测试点 | 结果 |
|------|--------|------|
| R1 | from_parts 注入式构造 + plic()/clint()/uart() 观测口回读 | PASS |
| R2 | 六 init 蓝图顺序全 Ok（cpu → mem → irq → clock → serial → gpio；irq_init 联动 plic.init + clock_init 联动 clint.init） | PASS |
| R3 | serial_putc tx_log == b"RIS" 字节序 | PASS |
| R4 | clock_now_us 占位返回 0 + 单调性（0 >= 0 成立） | PASS |
| R5 | clock_deadline 占位记录 + clock_disarm 清 armed（Clint Cell 状态记录） | PASS |
| R6 | enable_irq/disable_irq 委托 plic.enable/disable + 边界 1024/u32::MAX/2000 no-op | PASS |
| R7 | Plic 占位——init() 后 is_initialized()==true | PASS |
| R8 | Clint 占位——arm_deadline 后 is_armed()==true；disarm 后 is_armed()==false | PASS |
| R9 | 降级路径——Plic/Clint 占位时 init 仍 Ok | PASS |
| R10 | trait 多态——RiscvHal 经 `&mut dyn Hal` 分发六 init 全 Ok | PASS |

### 7.2 host 单测（hal/loongarch.rs 内 #[cfg(test)]，L1~L10）

源码锚点：[loongarch.rs L287~500](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L287-L500)。

| 编号 | 测试点 | 结果 |
|------|--------|------|
| L1 | from_parts 注入式构造 + pch()/uart() 观测口回读 | PASS |
| L2 | 六 init 蓝图顺序全 Ok（irq_init 联动 pch.init） | PASS |
| L3 | serial_putc tx_log == b"LOO" 字节序 | PASS |
| L4 | clock_now_us 占位返回 0 + 单调性 | PASS |
| L5 | clock_deadline 占位 + clock_disarm（不 panic） | PASS |
| L6 | enable_irq/disable_irq 占位 + 边界 1024/u32::MAX/2000 no-op | PASS |
| L7 | LoongsonPch 占位——init() 后 is_initialized()==true | PASS |
| L8 | Uart8250 跨平台复用验证——LoongArchHal.uart 与 X86Hal.uart 同类型（编译时类型断言） | PASS |
| L9 | 降级路径——Pch 占位时 init 仍 Ok | PASS |
| L10 | trait 多态——LoongArchHal 经 `&mut dyn Hal` 分发六 init 全 Ok | PASS |

### 7.3 host 集成测试（tests/riscv_loongarch_test.rs，PPI1~PPI7）

源码锚点：[riscv_loongarch_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs)（985 行）。

| 编号 | 测试点 | 结果 |
|------|--------|------|
| PPI1 | `&mut dyn Hal` 十一方法多态分发（RiscvHal + LoongArchHal 注入式） | PASS |
| PPI2 | 六 init 全链路 Ok + Plic/Clint/LoongsonPch 状态回读（irq_init 后 plic.is_initialized()==true / clock_init 后 clint init / irq_init 后 pch.is_initialized()==true） | PASS |
| PPI3 | serial_putc tx_log 字节序（RiscvHal == b"RIS" + LoongArchHal == b"LOO"） | PASS |
| PPI4 | clock_now_us 占位 + deadline 编程（RiscvHal 联动 clint.arm_deadline；LoongArchHal 占位 no-op） | PASS |
| PPI5 | enable/disable IRQ 占位 + 边界 1024/2000 no-op | PASS |
| PPI6 | 占位降级路径（Plic/Clint/LoongsonPch 占位时 init 仍 Ok） | PASS |
| PPI7 | ★ 跨架构 Hal trait 六平台多态——Aarch64Hal + X86Hal + PhytiumHal + KunpengHal + RiscvHal + LoongArchHal 同经 `&mut dyn Hal` 分发六 init 全 Ok（P3-E 终版本核心验收点） | PASS |

### 7.4 不做的测试（D1 接口占位口径 — D10/D11 降级）

| 项 | 蓝图 §6/§7 要求 | 不做原因 |
|----|----------------|---------|
| QEMU riscv64 启动 | §6「QEMU riscv64 启动」 | D10 ★ ADR-0001：RISC-V 不在信创认证范围，不投入生产资源；不安装 riscv64-unknown-none target |
| 龙芯真机启动 | §6「龙芯真机启动」 | D11 ★ ADR-0001：龙芯不在信创认证范围；§8 风险「龙芯开发板稀缺」 |
| RISC-V Sv48 页表 | §4.5 mem_init 注释 | 真机版本范畴——接口占位口径不保留真机路径 |
| PLIC/CLINT 完整驱动 | §3 交付物 | 仅 Cell 占位，寄存器操作留待真机 |
| LoongsonPch 完整驱动 | §3 交付物 | 仅 Cell 占位，寄存器操作留待真机 |
| riscv64/loongarch64 交叉编译 | —（隐含） | D6：CI 仍仅 aarch64-unknown-none + host（同 v0.144.0 D8 口径） |

---

## 8. Hal trait 跨架构六平台多态（PPI7 核心验收点 + P3-E 终版本出口）

### 8.1 六平台分发验证（spec D7 + PPI7）

PPI7 集成测试验证 `Hal` trait 可同时承载六个实现（[riscv_loongarch_test.rs L876~1053](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs#L876-L1053)）：

```rust
// Aarch64Hal 注入式构造（参照 hal_test.rs HPI1 口径）
let mut aarch64_rig = Aarch64Rig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut aarch64_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... 其余五 init + 五运行时 */ }

// X86Hal 注入式构造（参照 x86_test.rs XPI6 口径）
let mut x86_hal = make_x86hal(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut x86_hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }

// PhytiumHal 注入式构造（v0.145.0 飞腾信创生产平台之一）
let mut phytium_rig = PhytiumRig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut phytium_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(phytium_rig.hal.smmu().unwrap().is_initialized());  // 飞腾特有路径

// KunpengHal 注入式构造（v0.146.0 鲲鹏信创生产平台之二）
let mut kunpeng_rig = KunpengRig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut kunpeng_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(kunpeng_rig.hal.smmu().unwrap().is_initialized());  // 鲲鹏特有路径
assert!(kunpeng_rig.hal.numa().unwrap().is_configured());    // 鲲鹏 NUMA 特有路径

// RiscvHal 注入式构造（本版本核心扩展——降级平台之一，standalone Plic + Clint + UartSifive）
let mut riscv_hal = make_riscv_hal();
{ let h: &mut dyn Hal = &mut riscv_hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(riscv_hal.plic().is_initialized());  // RISC-V 特有路径
assert!(riscv_hal.clint().is_armed());        // RISC-V 特有路径

// LoongArchHal 注入式构造（本版本核心扩展——降级平台之二，standalone LoongsonPch + Uart8250）
let mut loongarch_hal = make_loongarch_hal();
{ let h: &mut dyn Hal = &mut loongarch_hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(loongarch_hal.pch().is_initialized());  // 龙芯特有路径
```

### 8.2 接口冻结后 BSP 可扩展性证据（v0.143.0 → v0.147.0 五版本六平台递进）

| 版本 | 平台数 | 平台 | 验证点 |
|------|--------|------|--------|
| v0.143.0 | 1 | Aarch64Hal | Hal trait 十一方法接口冻结（[hal/mod.rs L89~134](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L89-L134)） |
| v0.144.0 | 2 | + X86Hal | 跨架构（ARM64 → x86_64）扩展证明（D1 接口占位降级口径） |
| v0.145.0 | 3 | + PhytiumHal | 国产平台扩展证明（飞腾信创生产平台，三层口径） |
| v0.146.0 | 4 | + KunpengHal | 双国产平台扩展证明（鲲鹏信创生产平台，三层口径，PPI7 四平台验证） |
| **v0.147.0** | **6** | **+ RiscvHal + LoongArchHal** | **六平台完整覆盖扩展证明（RISC-V + 龙芯降级平台，PPI7 六平台验证——P3-E 终版本出口）** |

v0.143.0 接口冻结 → v0.144.0 两平台（Aarch64Hal + X86Hal）→ v0.145.0 三平台（+ PhytiumHal）→ v0.146.0 四平台（+ KunpengHal）→ **v0.147.0 六平台（+ RiscvHal + LoongArchHal）**，证明 v0.143.0 Hal trait 接口冻结后跨架构+跨平台可扩展性达到完整覆盖（P3-E 终版本出口）。**六平台多态证据**：双国产生产平台（飞腾 + 鲲鹏）+ 1 开发平台（ARM64 QEMU）+ 3 降级平台（x86_64 + RISC-V + 龙芯）。

---

## 9. D1~D12 偏差表 + 后续真机路线图

### 9.1 D1~D12 偏差表

| D 编号 | 蓝图偏差 | 决策 | 落地 |
|--------|---------|------|------|
| **D1 ★** | 蓝图 §1 ★ ADR-0001 BSP 收敛（RISC-V/龙芯不在信创认证范围，降级为接口占位）vs §3/§6/§7 完整交付物矛盾 | 接口占位口径（同 v0.144.0 X86Hal D1，**非** v0.145.0/v0.146.0 三层口径）：结构体 + impl Hal 十一方法占位 + host 可测骨架；不保留 cfg 门控真机寄存器写路径；不做真机验证；六平台多态验证（PPI7） | riscv.rs + loongarch.rs 无 cfg 门控真机路径 + 无真机脚本 + 无 DTB 占位 |
| D2 | 蓝图 §4.5 cpu_init 返回 () + 缺 clock_deadline + trap_vector_base 不存在 + asm! 宏 host 编译失败 | 全部修正为符合 Hal trait 签名 + 不引入 asm! 宏（同 v0.144.0 D3/D4 口径）：六 init 返回 `Result<(), KernelError>`（占位 Ok）；补全 clock_deadline；clock_now_us 占位返回 0；不引入 asm! 宏 | riscv.rs + loongarch.rs 所有 init 返回 Ok(())，clock_now_us 返回 0，无 asm! 宏 |
| D3 | RiscvHal/LoongArchHal 不能组合 Aarch64Hal（不同架构） | standalone Hal 实现（同 v0.144.0 X86Hal 口径）：自有三元组/二元组，十一方法直接实现（占位），不委托 self.base.*() | RiscvHal 三字段 + LoongArchHal 两字段，无 Aarch64Hal 组合 |
| D4 | LoongArchHal 用 Uart8250——是否复用还是重复定义？ | 复用 `x86_64::Uart8250`（Simplicity First——8250 UART 标准跨平台复用，零重复定义） | loongarch.rs L78 `use super::x86_64::Uart8250;` 导入 |
| D5 | Plic/Clint/LoongsonPch 占位类型——蓝图 §4.5 仅给结构体无方法签名 | 占位类型 + Cell 状态记录（同 v0.144.0 LocalApic + v0.146.0 KunpengSmmu/KunpengNuma 口径） | riscv.rs Plic/Clint/UartSifive + loongarch.rs LoongsonPch |
| D6 | 架构特定 target 不在 CI（riscv64/loongarch64 不交叉编译） | 不安装 riscv64/loongarch64 target，CI 仍仅 aarch64-unknown-none + host（同 v0.144.0 D8 口径） | riscv.rs + loongarch.rs 全部 host 可编译代码，无 cfg 门控真机路径 |
| D7 | 跨架构 Hal trait 六平台多态验证（P3-E 终版本出口） | v0.146.0 四平台扩展至六平台（+ RiscvHal + LoongArchHal），PPI7 六平台同经 `&mut dyn Hal` 分发六 init 全 Ok | riscv_loongarch_test.rs PPI7 六平台多态测试 |
| D8 | 蓝图 §3 文件路径偏差（同 v0.143.0 D1 口径） | 按实际路径 `crates/kernel/microkernel/src/hal/` | riscv.rs + loongarch.rs 落位 crates/kernel/microkernel/src/hal/ |
| D9 | 不改 Hal trait（接口已冻结，同 v0.145.0 D9 / v0.146.0 D9） | v0.143.0 Hal trait 十一方法已冻结，本版本仅消费 trait 实现 | hal/mod.rs 仅追加 `pub mod riscv;` + `pub mod loongarch;` + platform::riscv_virt + platform::loongarch_qemu + 头注 |
| D10 | 不做 QEMU riscv64 启动验证（ADR-0001 降级） | 不安装 riscv64-unknown-none target + 不做 QEMU riscv64 启动脚本 + host 测试覆盖接口正确性 | 无 tests/riscv_boot.sh 脚本 |
| D11 | 不做龙芯真机启动验证（ADR-0001 降级） | 不做龙芯真机启动 + 无 configs/loongarch-qemu.dts 占位 | 无 tests/loongarch_boot.sh 脚本 + 无 DTB 占位 |
| D12 | RiscvHal/LoongArchHal 文件位置 | 各自单文件（参考 v0.144.0 x86_64.rs + v0.145.0 phytium.rs + v0.146.0 kunpeng.rs 单文件先例） | riscv.rs + loongarch.rs 各自单文件 |

### 9.2 后续真机路线图（触发条件汇总）

RISC-V / 龙芯真机路径留待 ADR-0001 修订或信创范围扩展时补齐（蓝图 §1 ★ ADR-0001 降级口径）。真机路线图（按优先级排序）：

1. **RISC-V Sv48 页表实现**：替换 [riscv.rs mem_init](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L430) 占位 Ok——Sv39/Sv48 三级/四级页表映射 + `sfence.vma` TLB 刷新。**触发条件**：RISC-V 硬件/QEMU 投入生产资源。
2. **PLIC 完整驱动**：替换 [Plic::init](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L255) Cell 占位——PLIC MMIO 寄存器写（使能位 / 优先级 / 目标 CPU 阈值）+ `#[cfg(target_arch = "riscv64")]` 门控 + SAFETY 注释。**触发条件**：真机 PLIC 调试。
3. **CLINT 完整驱动**：替换 [Clint::arm_deadline](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L351) Cell 占位——CLINT mtimecmp 寄存器写 + `asm!("rdtime")` 读 time 寄存器 + `asm!("csrw stimecmp")` 编程。**触发条件**：真机 CLINT 调试。
4. **RISC-V `stvec` 异常向量加载**：替换 [riscv.rs cpu_init](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L423) 占位 Ok——`asm!("csrw stvec, {addr}")` 加载异常向量基址。**触发条件**：真机 RISC-V 启动链。
5. **LoongsonPch 完整驱动**：替换 [LoongsonPch::init](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L135) Cell 占位——PCH MMIO 寄存器写（中断使能 + 时钟配置）+ `#[cfg(target_arch = "loongarch64")]` 门控。**触发条件**：龙芯硬件投入生产资源。
6. **LoongArch 定时器 + 异常向量**：替换 [loongarch.rs cpu_init / clock_init / clock_now_us](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L202) 占位——`asm!("rdtime.d")` 读稳定计数器 + 定时器比较寄存器编程 + 异常向量基址加载。**触发条件**：龙芯真机调试。
7. **riscv64-unknown-none / loongarch64-unknown-none target 安装**：`rustup target add riscv64-unknown-none` + `rustup target add loongarch64-unknown-none` + `.cargo/config.toml` 追加 target 配置。**触发条件**：真机交叉编译。
8. **QEMU riscv64 / 龙芯真机启动脚本**：`tests/riscv_boot.sh` + `tests/loongarch_boot.sh` + `configs/riscv-virt.dts` + `configs/loongarch-qemu.dts` 占位补齐。**触发条件**：真机启动验证。

**触发条件汇总**：当 RISC-V / LoongArch 硬件 / QEMU 投入生产资源时（ADR-0001 修订或信创范围扩展），按上述清单逐项扩展真机路径。

---

## 10. 双架构对比（RISC-V RV64 vs LoongArch64 技术交底表）

| 维度 | RISC-V RV64 | LoongArch64 | 备注 |
|------|------------|-------------|------|
| 指令集 | RV64GCSU（通用 + 压缩 + 监督模式 + 用户模式） | LoongArch64 | 均为开源/自主 64bit RISC 指令集 |
| 中断控制器 | PLIC（Platform-Level Interrupt Controller） | LoongsonPch（龙芯平台控制器，类比 x86 PCH / ARM GIC） | RISC-V 平台级中断控制器；LoongArch 集中式 PCH |
| 时钟 | CLINT（Core Local Interruptor）—— `rdtime` 读 time 寄存器 | LoongsonPch timer—— `rdtime.d` 读稳定计数器 | RISC-V CLINT 含 mtime/mtimecmp；LoongArch 集成于 PCH |
| 页表 | Sv39（三级 39bit）/ Sv48（四级 48bit） | LA64 page table（四级页表） | 本版本均占位 Ok，真机版本补页表实现 |
| 启动方式 | OpenSBI + S-mode（Supervisor Mode） | LoongArch bootrom | RISC-V 经 SBI 进入 S-mode；LoongArch 经 bootrom 进入内核 |
| UART | SiFive UART（16550 兼容，THR/LSR 偏移同 8250） | 8250 UART（标准 IP，D4 复用 `x86_64::Uart8250`） | SiFive UART 16550 兼容；LoongArch 直接复用 8250 |
| 异常向量 | `stvec` 寄存器（CSR）+ `asm!("csrw stvec, {addr}")` | LoongArch 异常向量基址寄存器 + 专用指令 | 均不引入 `asm!` 宏（D2 接口占位口径） |
| HAL 组合 | standalone 三元组（Plic + Clint + UartSifive） | standalone 二元组（LoongsonPch + Uart8250） | 均 D3 standalone，不组合 Aarch64Hal |
| 跨平台复用 | UartSifive 独立封装（复用 `eneros_bootloader::RegIo`） | Uart8250 复用 `x86_64::Uart8250`（D4 Simplicity First） | LoongArch 8250 跨平台复用零重复定义 |
| 平台常量 | riscv_virt（UART_SIFIVE_BASE=0x1000_0000 / PLIC_BASE=0x0C00_0000 / CLINT_BASE=0x0200_0000 / CNTP_IRQ_RISCV=5） | loongarch_qemu（UART_8250_BASE=0x1FE0_01E0 / PCH_BASE=0x1FE0_0000 / IRQ_LIMIT=1024） | 占位地址依据 QEMU 地址图，待真机调试确认 |
| 真机验证 | 留待真机（D1 接口占位降级口径） | 留待真机（D1 接口占位降级口径） | 均不投入生产资源（ADR-0001） |

---

## 11. P3-E HAL 多平台线总结（v0.143.0~v0.147.0 五版本六平台完整覆盖）

### 11.1 五版本递进总结

| 版本 | 交付物 | 口径 | 平台数 |
|------|--------|------|--------|
| v0.143.0 | Hal trait 十一方法 + HalRegistry + Aarch64Hal + qemu_virt | 接口冻结（D2 Result 化） | 1（ARM64 QEMU） |
| v0.144.0 | X86Hal + LocalApic + X86PageTable + x86_qemu | 接口占位（D1 ADR-0001 降级） | 2（+ x86_64） |
| v0.145.0 | PhytiumHal + PhytiumSmmu + PhytiumCrypto + phytium_d2000 + dtb_find_node | 三层口径（★ 信创核心，飞腾生产平台） | 3（+ 飞腾） |
| v0.146.0 | KunpengHal + KunpengSmmu + KunpengNuma + kunpeng_920 + dtb_count_nodes | 三层口径（★ 信创核心，鲲鹏生产平台，双国产平台证据） | 4（+ 鲲鹏） |
| **v0.147.0** | RiscvHal + LoongArchHal + Plic + Clint + UartSifive + LoongsonPch + riscv_virt + loongarch_qemu | 接口占位（D1 ADR-0001 收敛降级，P3-E 终版本出口） | **6（+ RISC-V + 龙芯）** |

### 11.2 ADR-0001 BSP 收敛口径（六平台分类）

| 分类 | 平台 | 信创认证范围 | 口径 |
|------|------|------------|------|
| 2 生产平台 | 飞腾 D2000 + 鲲鹏 920 | ✅ 是 | 三层口径（真机路径保留 + host 可测骨架 + 真机验证留待硬件） |
| 1 开发平台 | ARM64 QEMU | —（开发用） | QEMU 真机路径（v0.143.0 Aarch64Hal） |
| 3 降级平台 | x86_64 + RISC-V + 龙芯 | ❌ 否 | 接口占位（D1 不保留 cfg 门控真机寄存器写路径 + 不做真机验证） |

### 11.3 P3-E 终版本出口

六平台完整覆盖证明 **v0.143.0 Hal trait 接口冻结后跨架构+跨平台可扩展性达到完整覆盖**：

- **双国产平台证据**：飞腾（v0.145.0）+ 鲲鹏（v0.146.0）——信创认证核心交付的生产平台 BSP 出口验证通过 → 信创认证代码化证据输入。
- **开发平台证据**：ARM64 QEMU（v0.143.0）——开发与冒烟验证基线。
- **降级平台证据**：x86_64（v0.144.0）+ RISC-V + 龙芯（v0.147.0）——接口占位证明 Hal trait 跨架构可扩展至 x86/RISC-V/LoongArch 全部架构。

**P3-E 终版本出口**：六平台完整覆盖证明 Hal trait 接口冻结后跨架构可扩展性达到完整覆盖——P3-E HAL 多平台线五版本（v0.143.0~v0.147.0）圆满收口。

---

## 12. 源码链接

> 所有源码引用用 `file:///` 绝对路径含行号锚点（参照 [hal_design.md](./hal_design.md) §13 + [kunpeng_bsp.md](./kunpeng_bsp.md) §12 风格）。

- [riscv.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs)（RiscvHal + Plic + Clint + UartSifive + impl Hal + R1~R10 单测）
  - [UartSifive 泛型封装 L145~203](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L145-L203)
  - [Plic 占位类型 L216~285](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L216-L285)
  - [Clint 占位类型 L297~370](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L297-L370)
  - [RiscvHal 三元组装 L383~390](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L383-L390)
  - [from_parts 注入式 + 观测口 + clock_disarm L392~420](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L392-L420)
  - [impl Hal for RiscvHal 十一方法 L422~503](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L422-L503)
  - [R1~R10 单测 L505~756](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/riscv.rs#L505-L756)
- [loongarch.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs)（LoongArchHal + LoongsonPch + impl Hal + L1~L10 单测）
  - [use super::x86_64::Uart8250 导入 L78](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L78)（D4 跨平台复用证据）
  - [LoongsonPch 占位类型 L98~142](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L98-L142)
  - [LoongArchHal 二元组装 L162~168](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L162-L168)
  - [from_parts 注入式 + 观测口 + clock_disarm L170~199](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L170-L199)
  - [impl Hal for LoongArchHal 十一方法 L201~285](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L201-L285)
  - [L1~L10 单测 L287~500](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/loongarch.rs#L287-L500)
- [hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（Hal trait + HalRegistry + 模块挂载 + 六平台常量组）
  - [L1 头注 v0.143.0~v0.147.0](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L1)
  - [L78 `pub mod loongarch;`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L78) + [L80 `pub mod riscv;`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L80)（模块声明追加）
  - [Hal trait 十一方法 L89~134](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L89-L134)（v0.143.0 冻结）
  - [HalRegistry L138~153](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L138-L153)
  - [platform::riscv_virt 四常量 L312~328](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L312-L328)
  - [platform::loongarch_qemu 三常量 L339~350](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L339-L350)
- [lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs)（lib.rs 头注 v0.147.0 riscv + loongarch 行）
  - [L36 v0.147.0 头注交付物](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs#L36)
- [riscv_loongarch_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs)（PPI1~PPI7 集成测试，含 PPI7 六平台多态）
  - [PPI1 L412~468](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs#L412-L468)
  - [PPI2 L487~582](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs#L487-L582)
  - [PPI7 六平台多态 L876~1053](file:///e:/EnerOS/crates/kernel/microkernel/tests/riscv_loongarch_test.rs#L876-L1053)
- 复用设施参考：
  - [x86_64.rs Uart8250](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/x86_64.rs)（D4 跨平台复用来源，v0.144.0）
  - [apic.rs LocalApic](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（Plic/LoongsonPch Cell 占位口径参考，v0.144.0）
  - [clock.rs GenericTimer](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/clock.rs)（Clint armed Cell 口径参考，v0.143.0）
  - [aarch64.rs Aarch64Hal](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs)（standalone 对比参考，v0.143.0）
  - [phytium.rs PhytiumHal](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/phytium.rs)（三层口径对比参考，v0.145.0）
  - [kunpeng.rs KunpengHal](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs)（三层口径对比参考 + PPI7 多态扩展先例，v0.146.0）
- 前序设计文档：
  - [hal_design.md](./hal_design.md)（v0.143.0 Hal trait 十一方法接口冻结来源）
  - [x86_bsp.md](./x86_bsp.md)（v0.144.0 D1 接口占位模板来源）
  - [phytium_bsp.md](./phytium_bsp.md)（v0.145.0 三层口径对比参考）
  - [kunpeng_bsp.md](./kunpeng_bsp.md)（v0.146.0 PPI7 多态扩展先例）
- spec 文档：
  - [spec.md](file:///e:/EnerOS/.trae/specs/develop-v14700-riscv-loongarch-bsp/spec.md)（D1~D12 决策来源）
  - [tasks.md](file:///e:/EnerOS/.trae/specs/develop-v14700-riscv-loongarch-bsp/tasks.md)（Task 1~6 任务分解）
  - [checklist.md](file:///e:/EnerOS/.trae/specs/develop-v14700-riscv-loongarch-bsp/checklist.md)（C5.1~C5.14 验证点）
- 蓝图依据：[phase3.md §v0.147.0](file:///e:/EnerOS/蓝图/phase3.md)（§1 版本目标 ★ ADR-0001 BSP 收敛 + §3 交付物清单 + §4.5 关键代码 + §6 测试计划 + §7 验收标准）
- 本文档：[riscv_loongarch_bsp.md](file:///e:/EnerOS/docs/kernel/riscv_loongarch_bsp.md)
