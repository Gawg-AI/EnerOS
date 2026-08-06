# EnerOS v0.146.0 鲲鹏 920 BSP（★ 信创核心交付 — 双国产平台）设计文档（KunpengHal 组合组装 / KunpengSmmu cfg 门控 / KunpengNuma 占位 / dtb_count_nodes DTB 节点计数 / kunpeng_920 平台配置）

> 版本：v0.146.0
> 日期：2026-07-29
> 状态：已交付（host 单测 P1~P10 + DT5~DT7 全绿 + PPI1~PPI7 集成测试全绿 + aarch64 release 交叉编译 cfg 门控真机路径包含；真机验证留待硬件——D1 三层口径第三层）
> 适用范围：EnerOS 自研内核鲲鹏 920 BSP 子系统（KunpengHal 组合 Aarch64Hal + KunpengSmmu + KunpengNuma + boot::dtb_count_nodes DTB 节点计数 + platform::kunpeng_920 平台常量组；★ 信创核心交付 — 双国产平台，真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径）
> 蓝图依据：`蓝图/phase3.md` §v0.146.0（§1 版本目标 ★ 信创核心 / §2 前置依赖 / §3 交付物清单 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-E HAL 多平台线第 4 版**（v0.143.0 HAL 接口与 ARM64 实现 → v0.144.0 x86_64 BSP 接口占位 → v0.145.0 飞腾 D2000 BSP → **v0.146.0 鲲鹏 920 BSP** → v0.147.0 RISC-V/龙芯，五版之四），★ 信创认证核心交付（鲲鹏为 ADR-0001 方案 A 保留的生产平台之一，与飞腾 D2000 同属双国产平台）
> 配套文档：[hal_design.md](./hal_design.md)（v0.143.0 HAL 接口与 ARM64 实现设计文档，前序版本，Hal trait 十一方法接口冻结来源）、[x86_bsp.md](./x86_bsp.md)（v0.144.0 x86_64 BSP 接口占位设计文档，前序版本，BSP 实施模式参考）、[phytium_bsp.md](./phytium_bsp.md)（v0.145.0 飞腾 D2000 BSP 设计文档，前序版本，国产平台 BSP 实施模板）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 驱动，KunpengHal 组装复用）

---

## 1. 版本目标

### 1.1 ★ 信创核心交付（蓝图 §1 逐字引用）

> ★ **鲲鹏为方案 A 保留的生产平台之一，本版本是信创认证核心交付，优先级最高，必须支持真机启动**。

蓝图 §1（`蓝图/phase3.md` L5695）原样。本信创核心口径为本版本**最高优先级决策**（仓库规范 §5.4 ADR-0001 已签署决策态），与 v0.144.0 x86_64「接口占位」根本不同——鲲鹏真机路径必须在源码层面保留作为信创认证代码化证据输入（即使本机无硬件可实测）。**双国产平台定位**：与 v0.145.0 飞腾 D2000 BSP 同属信创核心交付，飞腾 + 鲲鹏共同构成 EnerOS 双国产生产平台证据。

### 1.2 核心一句话（蓝图 §1 原样）

**实现鲲鹏 920 BSP（CPU/中断 GICv3/SMMU/NUMA），支持自研内核在鲲鹏真机启动。**

（蓝图 `phase3.md` L5697 原样；按 §1.1 信创核心口径 + spec D1 三层口径，本版本交付"真机路径保留 + host 可测骨架 + 真机验证留待硬件"，完整鲲鹏真机启动链留待硬件就绪后扩展。）

### 1.3 Phase 定位（蓝图 §1 原样）

- **业务价值**：鲲鹏 920 是国产 ARM 服务器主力芯片，信创认证核心平台，与飞腾共同构成双国产生产平台（蓝图 §1 原样）。
- **Phase 定位**：**信创核心交付 — 双国产平台**（蓝图 §1 原样）。
- **出口关联**：自研内核"信创认证"能力 + 双国产平台证据（蓝图 §1 + §7 原样）。

### 1.4 三层口径落地（spec.md「Why」+ D1 决策）

v0.143.0 已冻结 `Hal` trait 十一方法接口规范，v0.144.0 已交付 `X86Hal` **接口占位**，v0.145.0 已交付 `PhytiumHal` 飞腾 BSP（cfg 门控 unsafe SMMUv3 CR0 写已编译进 aarch64 ELF）。蓝图 §v0.146.0 §1 明确鲲鹏为方案 A 保留的生产平台，本版本是信创认证核心交付。本机无鲲鹏 920 开发板且 QEMU 不支持鲲鹏模拟，但本版本仍需交付（三层口径，与 v0.145.0 D1 完全一致）：

1. **真机路径保留**：所有真机寄存器写（SMMUv3 CR0 / NUMA 探测 / DTB 解析）经 `#[cfg(target_arch = "aarch64")]` cfg 门控保留在源码中，附 SAFETY 注释 + 寄存器偏移引用（鲲鹏 920 数据手册），aarch64 编译包含该路径（双 ELF 体积可观测）。**这是与 v0.144.0 D1 接口占位的根本差异**——v0.144.0 x86_64 真机寄存器写全部留待真机版本（cfg 门控内为空注释），v0.146.0 鲲鹏真机寄存器写必须在 cfg 门控内实现（unsafe + write_volatile）。
2. **host 可测骨架**：`KunpengHal::from_parts(base, smmu, numa)` 注入式构造 + `KunpengSmmu`/`KunpengNuma` 用 `Cell<bool>`/`Cell<u8>` 状态记录（同 v0.145.0 PhytiumSmmu 口径），host 测试经观测口断言副作用。
3. **真机验证留待硬件**：`tests/kunpeng_boot.sh` 占位（cfg 门控早期 exit 0 + `[kunpeng] skip` 标注）+ `configs/kunpeng-920.dts` 占位最小骨架，硬件就绪后扩展真机启动链 + DTB 调试。

### 1.5 依赖关系（蓝图 §2 原样 + spec.md 落地）

- 前序 v0.143.0：HAL 接口（Hal trait 十一方法 + HalRegistry + platform::qemu_virt 已冻结，本版本消费）
- 前序 v0.144.0：BSP 实施模式参考（D8 cfg 门控 + D9 不改 trait + 注入式 host 可测口径，本版本沿用）
- 前序 v0.145.0：飞腾 BSP 实施模板（D1 三层口径 + D3 from_parts + D4 SMMUv3 CR0 cfg 门控 + D7 占位脚本 + D8 占位 DTB，本版本沿用并扩展至 NUMA + dtb_count_nodes）
- 前序 v0.133.0：GICv3 驱动（`interrupt::gicv3::Gicv3`，KunpengHal 组合 Aarch64Hal 复用）
- 前序 v0.129.0：Bootloader（`crates/kernel/bootloader/src/dtb.rs` FDT 解析模式，本版本 `boot/dtb.rs` 复用并扩展 `dtb_count_nodes`，零新依赖）
- 外部鲲鹏 920 硬件：蓝图 §2 假设，按 D1 三层口径第三层本版本不依赖（真机验证留待硬件）
- 阻塞项：无（D1 三层口径保证 host 可测 + 真机路径就绪待硬件）——蓝图 §2 原样

---

## 2. 交付物清单

对照蓝图 §3（`phase3.md` L5719~L5730）逐项标注 D1~D14 偏差（D 编号与 spec.md「蓝图偏差决策」表一一对应，全局表见 §10）：

| 蓝图 §3 交付物 | 蓝图要求 | 实际交付 | 偏差决策 |
|---------------|---------|---------|---------|
| 代码：`kernel/src/hal/kunpeng.rs` | 鲲鹏 HAL 实现 | [crates/kernel/microkernel/src/hal/kunpeng.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs)（约 870 行：`KunpengHal<B: RegIo = MmioRegIo>` 三字段组合（base + smmu + numa）+ `from_parts` 注入式 + cfg 门控 `new(dtb)` 真机路径 + Hal trait 十一方法委托 base + mem_init 联动 smmu.init + numa.configure + `KunpengSmmu { base, initialized: Cell<bool> }` cfg 门控 CR0 写 + `KunpengNuma { nodes: Cell<u8>, configured: Cell<bool> }` D12 占位 + P1~P10 单测） | D1 三层口径；D3 from_parts 注入式 + cfg 门控；D4 SMMUv3 CR0 cfg 门控；D5 configure Cell 记录不打印；D12 NUMA 占位 nodes:u8；D11 单文件 |
| 代码：`kernel/src/boot/dtb.rs` | `crate::boot::dtb_count_nodes` | [crates/kernel/microkernel/src/boot/dtb.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs)（约 520 行：既有 `dtb_find_node`/`dtb_find_node_or_err` 零改动 + 新增 `dtb_count_nodes(dtb: usize, compat: &str) -> u8` aarch64 cfg 门控 FDT 节点计数 + host 占位返回 0 + DT5~DT7 单测） | D2 既有 dtb.rs 内追加新函数；零新依赖（复用 dtb_find_node FDT 解析模式）；D6 兼容性字符串 |
| 接口：`KunpengHal { base, smmu, numa }` + Hal impl | 蓝图 §4.5 三字段 | `KunpengHal<B: RegIo = MmioRegIo> { base: Aarch64Hal<B>, smmu: Option<KunpengSmmu>, numa: Option<KunpengNuma> }` 三字段组合（D3 组合关系——鲲鹏 920 兼容 ARMv8.2）+ Hal trait 十一方法委托 base + mem_init 联动 smmu.init + numa.configure | D3 Aarch64Hal 复用；D9 不改 Hal trait |
| 接口：`KunpengSmmu { base }` | 蓝图 §4.5 单字段 | `KunpengSmmu { base: usize, initialized: Cell<bool> }` 双字段（D4 host Cell 状态记录）+ `new`/`base`/`is_initialized`/`init`/`probe` 签名 + cfg 门控 CR0 寄存器写 | D4 SMMUv3 CR0 cfg 门控 + Cell |
| 接口：`KunpengNuma` | 蓝图 §4.5 `nodes: u8` | `KunpengNuma { nodes: Cell<u8>, configured: Cell<bool> }` 双字段（D12 占位——§4.5 简化结构，§4.1 distances 矩阵留待真机）+ `from_parts`/`nodes`/`is_configured`/`configure`/`probe` 签名 + D5 configure Cell 记录不打印 | D12 nodes:u8 占位；D5 configure 不打印；distances 矩阵留待真机 |
| 文档：`docs/kernel/kunpeng_bsp.md` | 鲲鹏 BSP 设计文档 | 本文档 | 无（路径同蓝图） |
| 测试：`tests/kunpeng_boot.sh` | 鲲鹏真机启动脚本 | [tests/kunpeng_boot.sh](file:///e:/EnerOS/tests/kunpeng_boot.sh)（占位脚本——cfg 门控早期 exit 0 + `[kunpeng] skip` 标注 + 硬件就绪后扩展点 TODO） | D7 占位脚本 + 真机扩展点 |
| 配置：鲲鹏 920 DTB | 蓝图 §3 要求 | [configs/kunpeng-920.dts](file:///e:/EnerOS/configs/kunpeng-920.dts)（占位最小骨架——CPU + GICv3 + UART + SMMUv3 + numa-node + timer 节点占位，与 D10 平台常量对齐） | D8 占位 DTB + 真机调试按需补齐 |
| 模块挂载：`hal/mod.rs` | — | [hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) 追加 `pub mod kunpeng;`（L77，字母序 gpio 与 phytium 之间）+ `platform::kunpeng_920`（L278~L300 八常量子模块） | D9 Surgical Changes：既有 Hal trait / HalRegistry / platform / aarch64 / clock / serial / gpio / qemu_virt / x86_qemu / x86_64 / apic / phytium 零改动 |
| 模块挂载：`boot/mod.rs` | — | [boot/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/mod.rs) `pub use dtb::{dtb_find_node, dtb_find_node_or_err, dtb_count_nodes};` 重导出追加 `dtb_count_nodes`（保持 `crate::boot::dtb_count_nodes` 路径不变） | D2 重导出追加新函数 |
| 头注：[lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) | — | lib.rs 头注交付物追加 v0.146.0 kunpeng 行（KunpengHal + KunpengSmmu + KunpengNuma + dtb_count_nodes + ★ 信创核心 — 双国产平台 + 三层口径说明） | D9 Surgical Changes：仅追加头注一行 |
| 集成测试：`tests/kunpeng_test.rs` | PPI1~PPI7 | [tests/kunpeng_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/kunpeng_test.rs)（host 注入式集成测试——PPI1 十一方法多态分发 + PPI2 SMMU/NUMA 状态回读 + PPI3 serial_putc 字节序 + PPI4 clock 换算 + PPI5 enable/disable IRQ + PPI6 SMMU/NUMA 探测降级 + PPI7 ★ 跨架构四平台多态） | D1 三层口径第二层 host 可测；D14 PPI7 核心验收点 |

**spec 分类口径对照**（spec.md「What Changes」）：实现类（KunpengHal / KunpengSmmu / KunpengNuma）→ §3/§4/§5；DTB 节点计数设施（dtb_count_nodes）→ §6；配置类（kunpeng_920 平台常量）→ §7；文档类（kunpeng_bsp.md）→ 本文档；测试类（P1~P10 单测 + DT5~DT7 单测 + PPI1~PPI7 集成测试）→ §8。

---

## 3. KunpengHal 设计

### 3.1 三字段组合结构图（蓝图 §3 + spec D3 组合关系）

```rust
pub struct KunpengHal<B: RegIo = MmioRegIo> {
    base: Aarch64Hal<B>,                // D3 复用 ARM64 既有驱动（鲲鹏 920 兼容 ARMv8.2）
    smmu: Option<KunpengSmmu>,          // D4 cfg 门控真机路径 + host Cell 状态记录
    numa: Option<KunpengNuma>,          // D12 占位 nodes:u8 + D5 configure Cell 记录不打印
}
```

**与 v0.144.0 X86Hal 的根本差异**：X86Hal 是「三元组装」（apic + timer + uart 平行组合），KunpengHal 是「组合 + 扩展」（base Aarch64Hal 整体复用 + smmu/numa 鲲鹏特有扩展）。**与 v0.145.0 PhytiumHal 的差异**：PhytiumHal 扩展 smmu + crypto（飞腾特有加密引擎），KunpengHal 扩展 smmu + numa（鲲鹏 920 特有 NUMA 拓扑）。理由：鲲鹏 920 兼容 ARMv8.2 + GICv3 + PL011 + ARMv8 GenericTimer，复用 Aarch64Hal 既有驱动是 Simplicity First；SMMUv3 + NUMA 拓扑是鲲鹏 920 服务器场景特有，需独立占位类型扩展。

### 3.2 双构造（注入式 + cfg 门控真机）

```rust
impl<B: RegIo> KunpengHal<B> {
    pub fn from_parts(base: Aarch64Hal<B>, smmu: Option<KunpengSmmu>,
                      numa: Option<KunpengNuma>) -> Self { ... }
}

impl KunpengHal<MmioRegIo> {
    #[cfg(target_arch = "aarch64")]
    pub fn new(dtb: usize) -> Self {
        // 鲲鹏 920 平台常量组锚定 GICD/GICR/UART/GPIO 基址构造 Aarch64Hal
        // + DTB 探测 SMMU/NUMA 节点（cfg 门控真机 FDT 解析）
        // + .ok() 将 probe Err(NotSupported) 转 None（D1 降级路径）
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn new(_dtb: usize) -> Self {
        // host 降级实例（smmu=None/numa=None），仅供 cfg 门控编译验证
    }
}
```

`from_parts` 是 host 测试统一入口（D3 注入式 host 可测化口径，同 X86Hal::from_parts / PhytiumHal::from_parts）；`new(dtb)` 是 aarch64 真机入口，cfg 门控 host 编译时排除。

### 3.3 Hal trait 十一方法委托 + mem_init 联动

```rust
impl<B: RegIo> Hal for KunpengHal<B> {
    fn cpu_init(&mut self) -> Result<(), KernelError> { self.base.cpu_init() }
    fn mem_init(&mut self) -> Result<(), KernelError> {
        if let Some(smmu) = &mut self.smmu { smmu.init(); }       // 联动 SMMU 初始化
        if let Some(numa) = &self.numa { numa.configure(); }      // 联动 NUMA 配置
        self.base.mem_init()
    }
    fn irq_init(&mut self) -> Result<(), KernelError> { self.base.irq_init() }
    fn clock_init(&mut self) -> Result<(), KernelError> { self.base.clock_init() }
    fn serial_init(&mut self) -> Result<(), KernelError> { self.base.serial_init() }
    fn gpio_init(&mut self) -> Result<(), KernelError> { self.base.gpio_init() }
    fn serial_putc(&self, ch: u8) { self.base.serial_putc(ch) }
    fn clock_now_us(&self) -> u64 { self.base.clock_now_us() }
    fn clock_deadline(&mut self, us: u64) { self.base.clock_deadline(us) }
    fn enable_irq(&mut self, irq: u32, cpu: u8) { self.base.enable_irq(irq, cpu) }
    fn disable_irq(&mut self, irq: u32) { self.base.disable_irq(irq) }
}
```

`mem_init` 联动 SMMU 初始化 + NUMA 配置是 KunpengHal 特有路径（蓝图 §4.5 原样），不引入 panic（smmu.init / numa.configure 永远返回 ()，无错误传播）；其余十方法纯委托 base Aarch64Hal。

### 3.4 Hal trait 十一方法实现表

| Hal trait 方法 | KunpengHal 实现 | 联动 |
|---------------|----------------|------|
| `cpu_init` | 委托 `self.base.cpu_init()` | — |
| `mem_init` | 委托 `self.base.mem_init()` + 联动 `smmu.init()` + `numa.configure()` | ★ SMMU + NUMA |
| `irq_init` | 委托 `self.base.irq_init()` | — |
| `clock_init` | 委托 `self.base.clock_init()` | — |
| `serial_init` | 委托 `self.base.serial_init()` | — |
| `gpio_init` | 委托 `self.base.gpio_init()` | — |
| `serial_putc` | 委托 `self.base.serial_putc(ch)` | — |
| `clock_now_us` | 委托 `self.base.clock_now_us()` | — |
| `clock_deadline` | 委托 `self.base.clock_deadline(us)` | — |
| `enable_irq` | 委托 `self.base.enable_irq(irq, cpu)` | — |
| `disable_irq` | 委托 `self.base.disable_irq(irq)` | — |

### 3.5 内存预算（蓝图 §5.6 + spec.md 内存预算口径）

- `KunpengHal` ≈ **16.6KB**——内联 `Aarch64Hal` 16.4KB（v0.143.0 既有结构原样复用）+ `Option<KunpengSmmu>` 16B（base + Cell<bool> + 对齐填充）+ `Option<KunpengNuma>` 16B（Cell<u8> + Cell<bool> + 对齐填充，D12 占位无 distances 矩阵）。
- `KunpengSmmu` ≈ **16B**——`base: usize` 8B + `initialized: Cell<bool>` 1B + 对齐填充 7B（host Cell 口径同 apic.rs LocalApic / phytium.rs PhytiumSmmu）。
- `KunpengNuma` ≈ **16B**——`nodes: Cell<u8>` 1B + `configured: Cell<bool>` 1B + 对齐填充 14B（D12 占位，distances 矩阵 64B 留待真机版本）。
- hal 子系统 BSS 总计仍 < 20KB（v0.145.0 口径，KunpengHal 不驻留 BSS——鲲鹏启动链未实现，不全局注册；host 测试用 `Box` 承载）。

---

## 4. KunpengSmmu 占位

### 4.1 结构（蓝图 §4.5 `KunpengSmmu { base }` + spec D4）

```rust
pub struct KunpengSmmu {
    base: usize,                    // SMMUv3 寄存器帧基址（DTB "arm,smmu-v3" 节点 reg）
    initialized: Cell<bool>,         // D4 host Cell 状态记录（同 PhytiumSmmu 口径）
}
```

### 4.2 init() cfg 门控真机 CR0 写（spec D4 + 蓝图 §4.5）

```rust
pub fn init(&mut self) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // SAFETY：SMMUv3 CR0 寄存器位于 base + 0x20（SMMUv3 spec §6.3.1），
        // base 由 DTB "arm,smmu-v3" 节点 reg 属性提供（鲲鹏 920 数据手册
        // 锚定物理地址），写 0x1 使能 SMMU（ClientPD=0 + SMMUEN=1）。
        core::ptr::write_volatile((self.base + SMMU_CR0_OFFSET) as *mut u32, SMMU_CR0_ENABLE);
    }
    self.initialized.set(true);  // 真机路径同步记录便于 host 断言
}
```

**与 v0.144.0 D8 cfg 门控的根本差异**：v0.144.0 X86Hal PortIo 真机路径 cfg 门控内为空注释（接口占位），v0.146.0 KunpengSmmu::init 真机路径 cfg 门控内**必须实现** `unsafe { write_volatile(...) }`（信创认证代码化证据输入）。**与 v0.145.0 PhytiumSmmu 的一致性**：完全沿用 PhytiumSmmu::init 的 cfg 门控 + Cell + SAFETY 注释口径（D4 决策原样复用）。

### 4.3 probe() DTB 节点查找（spec D2 + D6）

```rust
pub fn probe(dtb: usize) -> Result<Self, KernelError> {
    let base = dtb_find_node(dtb, SMMU_COMPAT).ok_or(KernelError::NotSupported)?;
    Ok(Self::new(base))
}
```

`dtb_find_node` host 占位返回 None → probe 返回 `Err(NotSupported)` → `.ok() = None` → 触发 D1 第三层降级路径（smmu=None 占位实例，mem_init 跳过联动仍 Ok）。

### 4.4 SMMUv3 完整驱动真机路线图

仅 CR0 使能占位，完整 SMMUv3 命令队列 / 页表 / 上下文管理（Stream Table Entry / Context Descriptor / Translation Table Base 等）留待真机版本扩展（D1 第三层）。本版本 cfg 门控 CR0 写已编译进 aarch64 ELF，作为信创认证代码化证据输入。

---

## 5. KunpengNuma 占位

### 5.1 结构（蓝图 §4.5 `KunpengNuma { nodes: u8 }` + spec D12）

```rust
pub struct KunpengNuma {
    nodes: Cell<u8>,           // D12 占位 NUMA 节点数（probe 时由 dtb_count_nodes 返回值赋值）
    configured: Cell<bool>,    // D5 configure 调用状态记录（host Cell 口径，不打印）
}
```

**蓝图内部不一致处理**（spec D12）：蓝图 §4.1 `KunpengNuma { nodes: u8, distances: [[u8; 8]; 8] }` 含 NUMA 距离矩阵（8×8 = 64 字节），但 §4.5 `pub struct KunpengNuma { nodes: u8 }` 仅 nodes 字段。决策：按 §4.5 简化结构落地（D12 占位 nodes:u8），distances 矩阵留待真机扩展点（§4.1 distances 矩阵需真机 DTB `distance-map` 节点解析，属真机版本范畴）。

### 5.2 configure() Cell 记录不打印（spec D5）

```rust
pub fn configure(&self) {
    // D5 决策：改用 Cell 状态记录，不新增 serial_println! 宏
    // 真机版本应经 Hal trait serial_putc 输出 NUMA 节点数日志，
    // 本版本仅 Cell 记录避免引入打印抽象（Simplicity First + Surgical Changes）。
    self.configured.set(true);
}
```

**D5 决策理由**：蓝图 §4.5 `KunpengNuma::configure` 用 `crate::serial_println!("[hal] Kunpeng NUMA: {} nodes", self.nodes)` 打印——但 microkernel lib.rs 无 `serial_println!` 宏（no_std 内核无 println 抽象）。新增 `serial_println!` 宏需触碰 lib.rs + 引入格式化缓冲区抽象（alloc String 或 heapless），违反 Surgical Changes + Simplicity First。v0.145.0 PhytiumSmmu 已建立 Cell 状态记录先例，本版本沿用。

### 5.3 probe() dtb_count_nodes 节点计数（spec D2 + D12）

```rust
pub fn probe(dtb: usize) -> Result<Self, KernelError> {
    let count = dtb_count_nodes(dtb, NUMA_COMPAT);  // 蓝图 §4.5 调用路径
    if count == 0 { return Err(KernelError::NotSupported); }
    Ok(Self::from_parts(count))
}
```

`dtb_count_nodes` host 占位返回 0 → probe 返回 `Err(NotSupported)` → `.ok() = None` → 触发 D1 第三层降级路径（numa=None 占位实例，mem_init 跳过联动仍 Ok）。

### 5.4 distances 矩阵扩展点

```rust
// TODO 真机扩展点（D12）：蓝图 §4.1 distances: [[u8; 8]; 8] NUMA 距离矩阵
// 鲲鹏 920 数据手册 §x.y 提供节点间距离表，真机版本应：
// 1. 解析 DTB "distance-map" 节点填充 distances 矩阵
// 2. 提供 distance(from, to) -> u8 观测口
// 3. 调度器/分配器消费 distances 优化跨节点访问
// 本版本仅占位 nodes:u8，distances 矩阵留待真机版本。
```

### 5.5 观测口（host 测试断言入口）

```rust
pub fn nodes(&self) -> u8 { self.nodes.get() }
pub fn is_configured(&self) -> bool { self.configured.get() }
```

host 测试断言 `nodes()` 返回值 + `is_configured()` 状态（P8 验收点）。

---

## 6. dtb_count_nodes DTB 节点计数设施

### 6.1 蓝图偏差 D2（重大）

蓝图 §4.5 `KunpengNuma::probe` 调用 `crate::boot::dtb_count_nodes(dtb, "numa-node")` 统计 NUMA 节点数，但 v0.145.0 前 `boot/dtb.rs` 仅有 `dtb_find_node` + `dtb_find_node_or_err`（v0.145.0 新增），无 `dtb_count_nodes` 函数。属重大蓝图偏差 D2。

### 6.2 落地：既有 boot/dtb.rs 内追加 dtb_count_nodes

```rust
// crates/kernel/microkernel/src/boot/dtb.rs（既有 dtb_find_node / dtb_find_node_or_err 零改动）

pub fn dtb_count_nodes(dtb: usize, compat: &str) -> u8 {
    #[cfg(target_arch = "aarch64")] {
        // 复用 dtb_find_node 的 FDT 解析模式：
        // 遍历 structure block，统计 compatible 属性含 compat 子串的节点数
        // FDT_MAGIC / FDT_BEGIN_NODE / FDT_PROP / FDT_END token + strings block 查找
    }
    #[cfg(not(target_arch = "aarch64"))] {
        0  // host 占位返回 0，触发 D1 降级路径
    }
}
```

### 6.3 零新依赖（spec D2 + D8 口径）

复用 `crates/kernel/bootloader/src/dtb.rs` 的 FDT 解析模式（FDT_MAGIC / FDT_BEGIN_NODE / FDT_PROP / FDT_END token + strings block 查找 + align4 helper），不引入外部 `fdt` crate（与 v0.145.0 D8 口径一致，避免 SBOM 风险）。`dtb_count_nodes` 与 `dtb_find_node` 共享 FDT 解析模式，区别在：`dtb_find_node` 返回首个匹配节点的 reg 地址（`Option<usize>`），`dtb_count_nodes` 遍历全部节点统计匹配数（`u8`）。

### 6.4 D6 兼容性字符串

蓝图 §4.5 原样字符串 `"numa-node"` 与 `"arm,smmu-v3"`，子串匹配兼容多值 compatible 属性。真机调试扩展点：鲲鹏 920 实际 DTB 兼容性字符串可能为 `"hisilicon,numa-node"` 或 `"numa-node"` 等，按 `/proc/device-tree` 实际值调整即可，不影响类型签名。

---

## 7. platform::kunpeng_920 平台配置（spec D10）

```rust
pub mod kunpeng_920 {
    pub const UART_PL011_BASE: usize = 0x9FFF_F000;  // PL011 UART0 基址占位
    pub const GICD_BASE: usize = 0x4_0080_0000;       // GICv3 分发器基址占位
    pub const GICR_BASE: usize = 0x4_00A0_0000;       // GICv3 再分发器基址占位
    pub const GPIO_PL061_BASE: usize = 0x9400_0000;   // GPIO 基址占位
    pub const SMMU_BASE: usize = 0x4_0040_0000;       // SMMUv3 基址占位
    pub const NUMA_MAX_NODES: u8 = 8;                  // 鲲鹏 920 最大 NUMA 节点数占位
    pub const CNTP_IRQ: u32 = 30;                      // ARM 架构固定
    pub const CNTV_IRQ: u32 = 27;                      // ARM 架构固定
}
```

与既有 `qemu_virt`（ARM64）/ `x86_qemu`（x86_64）/ `phytium_d2000`（飞腾）并列。占位地址依据鲲鹏 920 数据手册，实际值待真机调试确认；与 `configs/kunpeng-920.dts` 占位地址对齐（spec D8）。

**四平台常量并列对照**：

| 平台 | UART_PL011_BASE | GICD_BASE | GICR_BASE | GPIO_PL061_BASE | SMMU_BASE | 备注 |
|------|----------------|-----------|-----------|-----------------|-----------|------|
| `qemu_virt` | 0x0900_0000 | 0x0800_0000 | 0x080A_0000 | 0x0903_0000 | — | QEMU virt（v0.143.0） |
| `x86_qemu` | — | — | — | — | — | x86_64 占位（v0.144.0，无 ARM 寄存器） |
| `phytium_d2000` | 0x2800_0000 | 0x2f00_0000 | 0x2f10_0000 | 0x2801_0000 | 0x2b00_0000 | 飞腾 D2000（v0.145.0） |
| `kunpeng_920` | 0x9FFF_F000 | 0x4_0080_0000 | 0x4_00A0_0000 | 0x9400_0000 | 0x4_0040_0000 | 鲲鹏 920（v0.146.0） |

---

## 8. 测试矩阵

### 8.1 host 单测（boot/dtb.rs 内 #[cfg(test)] 追加 DT5~DT7）

| 编号 | 测试点 | 验收 |
|------|--------|------|
| DT5 | dtb_count_nodes host 占位返回 0 | `dtb_count_nodes(0x4000_0000, "numa-node")` 返回 0 |
| DT6 | dtb_count_nodes 兼容性字符串边界 | 空字符串 / 超长字符串均返回 0 |
| DT7 | dtb_count_nodes 多次调用幂等 | 连续 3 次调用均返回 0 |

### 8.2 host 单测（hal/kunpeng.rs 内 #[cfg(test)]，P1~P10）

| 编号 | 测试点 | 验收 |
|------|--------|------|
| P1 | from_parts 注入式构造 + base()/smmu()/numa() 观测口回读 | 三观测口回读正确 |
| P2 | 六 init 蓝图顺序全 Ok + mem_init 联动 smmu.init + numa.configure | is_initialized 由 false 转 true；is_configured 由 false 转 true |
| P3 | serial_putc tx_log == b"KUN" 字节序 | tx_log 内容序一致 |
| P4 | clock_now_us 注入源换算 + 单调性双读 | 62.5MHz ⇔ 1_000_000us；单调性 |
| P5 | clock_deadline 编程记录 last_deadline_ticks + disarm | deadline(5_000_000us) ⇔ 312_500_000 ticks |
| P6 | enable_irq/disable_irq 委托 base Aarch64Hal + 边界防御 | ISENABLER1 bit4 置位；irq ≥ 1024 no-op |
| P7 | KunpengSmmu init + probe 降级 | init 后 is_initialized == true；probe host 返回 Err(NotSupported) |
| P8 | KunpengNuma 占位 + probe 降级 | from_parts(2) 后 nodes()==2 / is_configured()==false；configure() 后 is_configured()==true；probe host 返回 Err(NotSupported) |
| P9 | mem_init 降级路径（smmu=None/numa=None 时仍 Ok） | smmu=None / numa=None 时 mem_init 仍 Ok |
| P10 | trait 对象多态分发（十一方法） | `&mut dyn Hal` 全方法可调 + 副作用核验 |

### 8.3 host 集成测试（tests/kunpeng_test.rs，PPI1~PPI7）

| 编号 | 测试点 | 验收 |
|------|--------|------|
| PPI1 | `&mut dyn Hal` 十一方法多态分发（KunpengHal 注入式） | 六 init 全 Ok + 运行时五方法不崩溃 |
| PPI2 | 六 init 全链路 Ok + SMMU/NUMA 状态回读 | smmu.is_initialized() == true；numa.is_configured() == true；numa.nodes() == 2 |
| PPI3 | serial_putc tx_log == b"KUN" 字节序 | tx_log 内容序一致；DR 偏移 0x00 末字节 b'N' |
| PPI4 | clock_now_us 注入源换算 + 单调性 + deadline 编程 | 62.5MHz ⇔ 1_000_000us；deadline(5_000_000us) ⇔ 312_500_000 ticks |
| PPI5 | enable/disable IRQ 委托 base GICv3 寄存器帧回读 + 边界 1024/2000 no-op | ISENABLER1 bit4 置位；边界 no-op |
| PPI6 | SMMU/NUMA 探测降级（host dtb_count_nodes 返回 0 / dtb_find_node 返回 None） | smmu=None/numa=None → mem_init 仍 Ok；probe 返回 Err(NotSupported) |
| PPI7 | ★ 跨架构 Hal trait 四平台多态——Aarch64Hal + X86Hal + PhytiumHal + KunpengHal 同经 `&mut dyn Hal` 分发六 init 全 Ok | **核心验收点**：证明 v0.143.0 接口冻结后 BSP 可扩展至鲲鹏信创生产平台（双国产平台证据：飞腾 + 鲲鹏） |

### 8.4 不做的测试（D1 第三层——真机验证留待硬件）

- 不做鲲鹏 920 真机启动（`tests/kunpeng_boot.sh` 占位早期 exit 0）
- 不做鲲鹏 DTB 实机解析（host 占位返回 0/None，aarch64 真机解析路径 cfg 门控留待真机调试）
- 不做 NUMA distances 矩阵验证（D12 占位 nodes:u8，distances 矩阵留待真机）
- 不做 64 核 SMP 启动验证（D13 SMP 属 v0.136.0 域，本版本 BSP 仅单核 HAL）
- 不做 SMMUv3 完整驱动验证（仅 CR0 使能占位，完整命令队列/页表/上下文管理留待真机）
- 不做 aarch64 交叉编译鲲鹏真机二进制（cfg 门控下 aarch64 编译包含真机寄存器写路径，但 main.rs 仍走 Aarch64Hal QEMU 路径，鲲鹏真机入口留待硬件）

---

## 9. Hal trait 跨架构四平台多态（PPI7 核心验收点）

### 9.1 四平台分发验证（spec D14 + PPI7）

PPI7 集成测试验证 `Hal` trait 可同时承载 `Aarch64Hal`、`X86Hal`、`PhytiumHal`、`KunpengHal` 四个实现：

```rust
// Aarch64Hal 注入式构造（hal_test.rs HPI1 口径）
let mut aarch64_rig = Aarch64Rig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut aarch64_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... 其余五 init + 五运行时 */ }

// X86Hal 注入式构造（x86_test.rs XPI6 口径）
let mut xhal = make_x86hal(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut xhal;
  assert!(h.cpu_init().is_ok()); /* ... */ }

// PhytiumHal 注入式构造（v0.145.0 飞腾信创生产平台）
let mut phytium_rig = PhytiumRig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut phytium_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }

// KunpengHal 注入式构造（本版本核心交付——鲲鹏信创生产平台）
let mut kunpeng_rig = KunpengRig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut kunpeng_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(kunpeng_rig.hal.smmu().unwrap().is_initialized());  // KunpengHal 特有路径
assert!(kunpeng_rig.hal.numa().unwrap().is_configured());    // KunpengHal 特有路径
```

### 9.2 接口冻结后 BSP 可扩展性证据（v0.143.0 → v0.146.0 四平台递进）

| 版本 | 平台数 | 平台 | 验证点 |
|------|--------|------|--------|
| v0.143.0 | 1 | Aarch64Hal | Hal trait 十一方法接口冻结 |
| v0.144.0 | 2 | + X86Hal | 跨架构（ARM64 → x86_64）扩展证明 |
| v0.145.0 | 3 | + PhytiumHal | 国产平台扩展证明（飞腾信创生产平台） |
| **v0.146.0** | **4** | **+ KunpengHal** | **双国产平台扩展证明（飞腾 + 鲲鹏，信创认证核心交付）** |

v0.143.0 → v0.144.0 两平台（Aarch64Hal + X86Hal）→ v0.145.0 三平台（+ PhytiumHal）→ **v0.146.0 四平台（+ KunpengHal）**，证明 v0.143.0 Hal trait 接口冻结后跨架构 + 跨平台可扩展性。**双国产平台证据**：飞腾（v0.145.0）+ 鲲鹏（v0.146.0）共同构成 EnerOS 信创认证核心交付的生产平台 BSP 出口验证通过 → 信创认证代码化证据输入。

---

## 10. D1~D14 偏差表 + 后续真机路线图

### 10.1 D1~D14 偏差表

| D 编号 | 蓝图偏差 | 决策 | 落地 |
|--------|---------|------|------|
| D1 | ★ 信创核心 vs 本机无鲲鹏硬件 | 真机路径保留 + host 可测骨架 + 真机验证留待硬件（三层口径，与 v0.145.0 D1 完全一致） | KunpengSmmu::init cfg 门控 unsafe CR0 写 + KunpengHal::from_parts 注入式 + tests/kunpeng_boot.sh 占位 |
| D2 | `crate::boot::dtb_count_nodes` 不存在 | 既有 boot/dtb.rs 内追加 dtb_count_nodes 新函数（不改既有 dtb_find_node / dtb_find_node_or_err） | boot/dtb.rs 追加 dtb_count_nodes + boot/mod.rs pub use 追加重导出 |
| D3 | `Aarch64Hal::new(dtb)` 不存在 | KunpengHal::new 内部用 from_parts + cfg 门控（不新增 Aarch64Hal::new_from_dtb） | KunpengHal::new(dtb) cfg 门控 aarch64 真机路径 + from_parts host 注入 |
| D4 | SMMUv3 寄存器操作 unsafe | cfg 门控真机路径 + host Cell 状态记录 + SAFETY 注释（同 v0.145.0 PhytiumSmmu） | KunpengSmmu::init() `#[cfg(target_arch = "aarch64")] unsafe { write_volatile(...) }` + `initialized.set(true)` |
| D5 | `crate::serial_println!` 不存在 | 改用 Cell 状态记录，不新增 serial_println! 宏 | KunpengNuma::configure() `self.configured.set(true)` + 观测口 is_configured() |
| D6 | DTB 兼容性字符串 | 蓝图原样 + 真机调试时按需调整 | NUMA_COMPAT = "numa-node"；SMMU_COMPAT = "arm,smmu-v3"；子串匹配 |
| D7 | `tests/kunpeng_boot.sh` 真机脚本 | 脚本占位（cfg 门控早期 exit 0 + [kunpeng] skip 标注）+ 硬件就绪后扩展 | tests/kunpeng_boot.sh 头部硬件探测占位 + TODO 扩展点 |
| D8 | 鲲鹏 DTB 文件 | configs/kunpeng-920.dts 占位最小骨架（CPU + GICv3 + UART + SMMUv3 + numa-node + timer 节点占位） | 与 D10 platform::kunpeng_920 常量对齐 |
| D9 | 不改 Hal trait（接口已冻结） | v0.143.0 Hal trait 十一方法已冻结，本版本仅消费 trait 实现 | hal/mod.rs 仅追加 `pub mod kunpeng;` + `platform::kunpeng_920` |
| D10 | `platform::kunpeng_920` 子模块 | 与 qemu_virt / x86_qemu / phytium_d2000 并列，占位地址依据鲲鹏 920 数据手册 | 八常量子模块（UART/GICD/GICR/GPIO/SMMU/NUMA_MAX_NODES/CNTP_IRQ/CNTV_IRQ） |
| D11 | KunpengHal 文件位置 | 单文件 hal/kunpeng.rs（参考 v0.145.0 phytium.rs 单文件先例） | KunpengHal + KunpengSmmu + KunpengNuma 全部在单文件 |
| D12 | `KunpengNuma` 占位 vs 完整实现 | 占位 `nodes: u8` + `configured: bool` Cell 记录，distances 矩阵留待真机（§4.5 简化结构） | KunpengNuma { nodes: Cell<u8>, configured: Cell<bool> } + distances 扩展点注释 |
| D13 | 64 核 SMP 启动 | BSP 不实现 64 核 SMP，留待真机扩展（SMP 调度器属 v0.136.0 域） | KunpengHal 仅单核 HAL + configs/kunpeng-920.dts CPU 节点占位 8 核简化 |
| D14 | 跨架构 Hal trait 四平台多态验证 | v0.145.0 三平台扩展至 v0.146.0 四平台（+ KunpengHal） | tests/kunpeng_test.rs PPI7 四平台多态测试 |

### 10.2 后续真机路线图（D1 第三层——硬件就绪后扩展）

硬件就绪后的扩展点清单（按优先级排序，触发条件汇总）：

1. **`tests/kunpeng_boot.sh` 真机启动链**：交叉编译 bootloader + kernel → dtc 加载 `configs/kunpeng-920.dts` → 鲲鹏开发板串口启动 → 断言 `[kernel] EnerOS kernel booting` + `[kunpeng] smmu ok` + `[kunpeng] numa ok`（cfg 门控早期 exit 0 → 真机扩展点 TODO）。**触发条件**：鲲鹏 920 开发板硬件就绪。
2. **`configs/kunpeng-920.dts` 实际 DTB**：按鲲鹏 920 实际 `/proc/device-tree` 值补齐 reg 地址 + compatible 字符串 + 节点完整属性（D6 + D8 占位 → 真机值）。**触发条件**：真机 DTB 调试。
3. **`dtb_count_nodes` aarch64 真机解析路径调试**：cfg 门控 FDT 解析在真机 DTB 上验证节点计数 + compatible 匹配（host 占位返回 0 → 真机实际计数）。**触发条件**：真机 DTB 调试。
4. **SMMUv3 完整驱动**：CR0 使能占位 → 完整命令队列 / 页表 / 上下文管理（Stream Table Entry / Context Descriptor / Translation Table Base 等）。**触发条件**：真机 IOMMU 调试。
5. **NUMA distances 矩阵实现**：D12 占位 nodes:u8 → §4.1 distances: [[u8; 8]; 8] 矩阵，解析 DTB `distance-map` 节点填充。**触发条件**：真机 NUMA 拓扑调试。
6. **64 核 SMP 启动序列**：D13 BSP 仅单核 HAL → PSCI secondary CPU boot + GICv3 SGI/IPI 路由 + 64 核启动时序（蓝图 §5/§8）。**触发条件**：真机 SMP 调试（v0.136.0 SMP 域扩展）。
7. **main.rs 鲲鹏真机入口冒烟段**：cfg 门控下 aarch64 编译 main.rs 仍走 Aarch64Hal QEMU 路径；硬件就绪后扩展鲲鹏真机入口（DTB 探测 + KunpengHal::new + 冒烟断言 `[kunpeng] smmu ok` + `[kunpeng] numa ok`）。**触发条件**：鲲鹏真机入口集成。
8. **真机 SMMUv3 上下文表 / NUMA distances 矩阵消费**：属后续版本范畴，本版本不涉及。**触发条件**：调度器/分配器消费 NUMA 拓扑优化跨节点访问。

---

## 11. 双国产平台对比（飞腾 D2000 vs 鲲鹏 920 技术交底）

### 11.1 双国产平台技术交底表

| 维度 | 飞腾 D2000（v0.145.0） | 鲲鹏 920（v0.146.0） | 备注 |
|------|----------------------|---------------------|------|
| 厂商 | 飞腾信息技术有限公司 | 华为海思 | 双国产平台 |
| 指令集 | ARMv8（兼容 ARMv8.0） | ARMv8.2（兼容 ARMv8.0） | 均兼容 ARMv8，复用 Aarch64Hal |
| 核心数 | 4-8 核（占位 4 核） | 64 核（占位 8 核简化，D13） | 鲲鹏 920 服务器场景多核 |
| NUMA | 无（嵌入式 SoC） | 有（多 NUMA 节点，D12 占位 nodes:u8） | 鲲鹏特有 NUMA 拓扑 |
| SMMU | SMMUv3（D4 cfg 门控 CR0） | SMMUv3（D4 cfg 门控 CR0） | 均集成 ARM SMMUv3 |
| 中断控制器 | GICv3 | GICv3 | 复用 Gicv3 既有驱动 |
| UART | PL011 | PL011 | 复用 Pl011 既有驱动 |
| GPIO | PL061 | PL061 | 复用 Gpio 既有驱动 |
| 加密引擎 | SM2/SM3/SM4 加速引擎（PhytiumCrypto 占位 has_sm4/has_sm2=true） | 无独立 crypto 引擎（鲲鹏 920 依赖 ARMv8 crypto 扩展指令） | 飞腾特有 crypto，鲲鹏用指令集扩展 |
| NUMA 拓扑 | 无 | 有（KunpengNuma 占位 nodes:u8，distances 矩阵留待真机） | 鲲鹏特有 NUMA |
| 启动方式 | PSCI（cfg 门控） | PSCI（cfg 门控，64 核 SMP 启动留待真机 D13） | 均兼容 ARM PSCI |
| 平台常量基址 | UART=0x2800_0000 / GICD=0x2f00_0000 / GICR=0x2f10_0000 / SMMU=0x2b00_0000 | UART=0x9FFF_F000 / GICD=0x4_0080_0000 / GICR=0x4_00A0_0000 / SMMU=0x4_0040_0000 | 占位地址依据各自数据手册 |
| 真机验证 | 留待硬件（D1 第三层） | 留待硬件（D1 第三层） | 双国产平台真机路径均保留 |
| Hal trait 多态 | PPI7 三平台（v0.145.0） | PPI7 四平台（v0.146.0） | 双国产平台共同验证 |

### 11.2 双国产平台 BSP 实施差异

**飞腾 D2000 BSP（v0.145.0）**：扩展 `PhytiumSmmu`（SMMUv3 CR0）+ `PhytiumCrypto`（SM2/SM3/SM4 加速引擎 has_sm4/has_sm2 占位 true）——飞腾特有加密引擎是嵌入式 SoC 场景的核心差异化能力。

**鲲鹏 920 BSP（v0.146.0）**：扩展 `KunpengSmmu`（SMMUv3 CR0，与 PhytiumSmmu 口径一致）+ `KunpengNuma`（NUMA 节点数 nodes:u8 占位，distances 矩阵留待真机）——鲲鹏 920 服务器场景的 NUMA 拓扑是核心差异化能力。**新增 `dtb_count_nodes` DTB 节点计数设施**（D2 重大蓝图偏差）支持 NUMA 节点数探测。

---

## 12. 源码链接

- [crates/kernel/microkernel/src/hal/kunpeng.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs)（KunpengHal + KunpengSmmu + KunpengNuma + impl Hal + P1~P10 单测）
  - [`pub struct KunpengHal`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs#L186)（三字段组合结构体定义）
  - [`impl KunpengHal<MmioRegIo>`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs#L245)（cfg 门控 new(dtb) + from_parts 注入式）
  - [`pub struct KunpengSmmu`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs#L395)（SMMUv3 占位 + Cell 状态记录）
  - [`pub struct KunpengNuma`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs#L490)（NUMA 占位 nodes:u8 + configure Cell 记录）
  - [`mod tests`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/kunpeng.rs#L558)（P1~P10 单测）
- [crates/kernel/microkernel/src/boot/dtb.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs)（dtb_find_node + dtb_find_node_or_err + dtb_count_nodes + DT1~DT7 单测）
  - [`pub fn dtb_count_nodes` (aarch64]](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs#L314)（cfg 门控真机 FDT 节点计数）
  - [`pub fn dtb_count_nodes` [host]](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs#L468)（host 占位返回 0）
  - [`mod tests`](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs#L473)（DT1~DT7 单测，含 DT5~DT7 新增 dtb_count_nodes 测试）
- [crates/kernel/microkernel/src/boot/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/mod.rs)（BootInfo + pub mod dtb + 重导出 dtb_count_nodes）
- [crates/kernel/microkernel/src/hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（Hal trait + HalRegistry + pub mod kunpeng + platform::kunpeng_920）
  - [`pub mod kunpeng;`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L77)（L77 模块声明追加）
  - [`pub mod kunpeng_920`](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs#L278)（L278~L300 八常量子模块）
- [crates/kernel/microkernel/src/hal/aarch64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs)（Aarch64Hal——KunpengHal 组合复用，v0.143.0 冻结）
- [crates/kernel/microkernel/src/hal/phytium.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/phytium.rs)（PhytiumHal——KunpengHal 实施模板，v0.145.0）
- [crates/kernel/microkernel/src/hal/apic.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（LocalApic——KunpengSmmu Cell 口径参考，v0.144.0）
- [crates/kernel/microkernel/src/lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs)（lib.rs 头注 v0.146.0 kunpeng 行）
- [crates/kernel/microkernel/tests/kunpeng_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/kunpeng_test.rs)（PPI1~PPI7 集成测试，含 PPI7 四平台多态）
- [tests/kunpeng_boot.sh](file:///e:/EnerOS/tests/kunpeng_boot.sh)（鲲鹏真机启动脚本占位）
- [configs/kunpeng-920.dts](file:///e:/EnerOS/configs/kunpeng-920.dts)（鲲鹏 920 DTB 占位最小骨架）
- [docs/kernel/kunpeng_bsp.md](file:///e:/EnerOS/docs/kernel/kunpeng_bsp.md)（本文档）
- [.trae/specs/develop-v14600-kunpeng-bsp/spec.md](file:///e:/EnerOS/.trae/specs/develop-v14600-kunpeng-bsp/spec.md)（v0.146.0 spec，D1~D14 决策来源）
