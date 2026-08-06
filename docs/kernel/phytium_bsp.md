# EnerOS v0.145.0 飞腾 D2000 BSP（★ 信创核心交付）设计文档（PhytiumHal 组合组装 / PhytiumSmmu cfg 门控 / PhytiumCrypto 占位 / dtb_find_node DTB 节点查找 / phytium_d2000 平台配置）

> 版本：v0.145.0
> 日期：2026-07-28
> 状态：已交付（host 单测 P1~P10 全绿 + PPI1~PPI7 集成测试全绿 + aarch64 release 交叉编译 cfg 门控真机路径包含；真机验证留待硬件——D1 三层口径第三层）
> 适用范围：EnerOS 自研内核飞腾 D2000 BSP 子系统（PhytiumHal 组合 Aarch64Hal + PhytiumSmmu + PhytiumCrypto + boot::dtb_find_node DTB 节点查找 + platform::phytium_d2000 平台常量组；★ 信创核心交付，真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径）
> 蓝图依据：`蓝图/phase3.md` §v0.145.0（§1 版本目标 ★ 信创核心 / §2 前置依赖 / §3 交付物清单 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-E HAL 多平台线第 3 版**（v0.143.0 HAL 接口与 ARM64 实现 → v0.144.0 x86_64 BSP 接口占位 → **v0.145.0 飞腾 D2000 BSP** → v0.146.0 鲲鹏 → v0.147.0 RISC-V/龙芯，五版之三），★ 信创认证核心交付（飞腾为 ADR-0001 方案 A 保留的生产平台之一）
> 配套文档：[hal_design.md](./hal_design.md)（v0.143.0 HAL 接口与 ARM64 实现设计文档，前序版本，Hal trait 十一方法接口冻结来源）、[x86_bsp.md](./x86_bsp.md)（v0.144.0 x86_64 BSP 接口占位设计文档，前序版本，BSP 实施模式参考）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 驱动，PhytiumHal 组装复用）

---

## 1. 版本目标

### 1.1 ★ 信创核心交付（蓝图 §1 逐字引用）

> ★ **飞腾为方案 A 保留的生产平台之一，本版本是信创认证核心交付，优先级最高，必须支持真机启动**。

蓝图 §1（`蓝图/phase3.md` L5534）原样。本信创核心口径为本版本**最高优先级决策**（仓库规范 §5.4 ADR-0001 已签署决策态），与 v0.144.0 x86_64「接口占位」根本不同——飞腾真机路径必须在源码层面保留作为信创认证代码化证据输入（即使本机无硬件可实测）。

### 1.2 核心一句话（蓝图 §1 原样）

**实现飞腾 D2000 BSP（CPU/中断 GICv3/SMMU/加密引擎 ARM 适配），支持自研内核在飞腾真机启动。**

（蓝图 `phase3.md` L5536 原样；按 §1.1 信创核心口径 + spec D1 三层口径，本版本交付"真机路径保留 + host 可测骨架 + 真机验证留待硬件"，完整飞腾真机启动链留待硬件就绪后扩展。）

### 1.3 Phase 定位（蓝图 §1 原样）

- **业务价值**：飞腾是国产 ARM 服务器主力芯片，信创认证核心平台（蓝图 §1 原样）。
- **Phase 定位**：**信创核心交付**（蓝图 §1 原样）。
- **出口关联**：自研内核"信创认证"能力（蓝图 §1 原样）。

### 1.4 三层口径落地（spec.md「Why」+ D1 决策）

v0.143.0 已冻结 `Hal` trait 十一方法接口规范，v0.144.0 已交付 `X86Hal` **接口占位**。蓝图 §v0.145.0 §1 明确飞腾为方案 A 保留的生产平台，本版本是信创认证核心交付。本机无飞腾 D2000 开发板且 QEMU 不支持飞腾模拟，但本版本仍需交付（三层口径）：

1. **真机路径保留**：所有真机寄存器写（SMMUv3 CR0 / crypto 探测 / DTB 解析）经 `#[cfg(target_arch = "aarch64")]` cfg 门控保留在源码中，附 SAFETY 注释 + 寄存器偏移引用（飞腾 D2000 数据手册），aarch64 编译包含该路径（双 ELF 体积可观测）。**这是与 v0.144.0 D1 接口占位的根本差异**——v0.144.0 x86_64 真机寄存器写全部留待真机版本（cfg 门控内为空注释），v0.145.0 飞腾真机寄存器写必须在 cfg 门控内实现（unsafe + write_volatile）。
2. **host 可测骨架**：`PhytiumHal::from_parts(base, smmu, crypto)` 注入式构造 + `PhytiumSmmu`/`PhytiumCrypto` 用 `Cell<bool>` 状态记录（同 v0.144.0 LocalApic 口径），host 测试经观测口断言副作用。
3. **真机验证留待硬件**：`tests/phytium_boot.sh` 占位（cfg 门控早期 exit 0 + `[phytium] skip` 标注）+ `configs/phytium-d2000.dts` 占位最小骨架，硬件就绪后扩展真机启动链 + DTB 调试。

### 1.5 依赖关系（蓝图 §2 原样 + spec.md 落地）

- 前序 v0.143.0：HAL 接口（Hal trait 十一方法 + HalRegistry + platform::qemu_virt 已冻结，本版本消费）
- 前序 v0.144.0：BSP 实施模式参考（D8 cfg 门控 + D9 不改 trait + 注入式 host 可测口径，本版本沿用）
- 前序 v0.133.0：GICv3 驱动（`interrupt::gicv3::Gicv3`，PhytiumHal 组合 Aarch64Hal 复用）
- 前序 v0.129.0：Bootloader（`crates/kernel/bootloader/src/dtb.rs` FDT 解析模式，本版本 `boot/dtb.rs` 复用，零新依赖）
- 外部飞腾 D2000 硬件：蓝图 §2 假设，按 D1 三层口径第三层本版本不依赖（真机验证留待硬件）
- 阻塞项：无（D1 三层口径保证 host 可测 + 真机路径就绪待硬件）——蓝图 §2 原样

---

## 2. 交付物清单

对照蓝图 §3（`phase3.md` L5561~L5570）逐项标注 D1~D11 偏差（D 编号与 spec.md「蓝图偏差决策」表一一对应，全局表见 §9）：

| 蓝图 §3 交付物 | 蓝图要求 | 实际交付 | 偏差决策 |
|---------------|---------|---------|---------|
| 代码：`kernel/src/hal/phytium.rs` | 飞腾 HAL 实现 | [crates/kernel/microkernel/src/hal/phytium.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/phytium.rs)（约 850 行：`PhytiumHal<B: RegIo = MmioRegIo>` 三字段组合（base + smmu + crypto）+ `from_parts` 注入式 + cfg 门控 `new(dtb)` 真机路径 + Hal trait 十一方法委托 base + mem_init 联动 smmu.init + `PhytiumSmmu { base, initialized: Cell<bool> }` cfg 门控 CR0 写 + `PhytiumCrypto { base, sm2_enabled, sm4_enabled }` D5 占位 true + P1~P10 单测） | D1 三层口径；D3 from_parts 注入式 + cfg 门控；D4 SMMUv3 CR0 cfg 门控；D5 has_sm4/has_sm2 占位 true；D6 兼容性字符串原样；D11 单文件 |
| 代码：`kernel/src/boot/dtb.rs` | `crate::boot::dtb_find_node` | [crates/kernel/microkernel/src/boot/dtb.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs)（约 280 行：`dtb_find_node(dtb: usize, compat: &str) -> Option<usize>` aarch64 cfg 门控 FDT 解析 + host 占位返回 None + `dtb_find_node_or_err` 便捷封装 + DT1~DT3 单测） | D2 boot.rs → boot/ 目录化 + 新建 dtb.rs；零新依赖（复用 bootloader dtb.rs FDT 模式）；D6 兼容性字符串 |
| 接口：`PhytiumHal { base, smmu, crypto }` + Hal impl | 蓝图 §4.5 三字段 | `PhytiumHal<B: RegIo = MmioRegIo> { base: Aarch64Hal<B>, smmu: Option<PhytiumSmmu>, crypto: Option<PhytiumCrypto> }` 三字段组合（D3 组合关系——飞腾兼容 ARMv8）+ Hal trait 十一方法委托 base + mem_init 联动 smmu.init | D3 Aarch64Hal 复用；D9 不改 Hal trait |
| 接口：`PhytiumSmmu { base }` | 蓝图 §4.5 单字段 | `PhytiumSmmu { base: usize, initialized: Cell<bool> }` 双字段（D4 host Cell 状态记录）+ `new`/`base`/`is_initialized`/`init`/`probe` 签名 + cfg 门控 CR0 寄存器写 | D4 SMMUv3 CR0 cfg 门控 + Cell |
| 接口：`PhytiumCrypto` | 蓝图 §4.1 数据结构 | `PhytiumCrypto { base: usize, sm2_enabled: bool, sm4_enabled: bool }` 三字段（D5 占位 true）+ `new`/`base`/`has_sm2`/`has_sm4`/`probe` 签名 | D5 has_sm4/has_sm2 占位 true；真机 CPUID 检测扩展点 |
| 文档：`docs/kernel/phytium_bsp.md` | 飞腾 BSP 设计文档 | 本文档 | 无（路径同蓝图） |
| 测试：`tests/phytium_boot.sh` | 飞腾真机启动脚本 | [tests/phytium_boot.sh](file:///e:/EnerOS/tests/phytium_boot.sh)（占位脚本——cfg 门控早期 exit 0 + `[phytium] skip` 标注 + 硬件就绪后扩展点 TODO） | D7 占位脚本 + 真机扩展点 |
| 配置：飞腾 D2000 DTB | 蓝图 §3 要求 | [configs/phytium-d2000.dts](file:///e:/EnerOS/configs/phytium-d2000.dts)（占位最小骨架——CPU + GICv3 + UART + SMMUv3 + crypto + timer 节点占位，与 D10 平台常量对齐） | D8 占位 DTB + 真机调试按需补齐 |
| 模块挂载：`hal/mod.rs` | — | [hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs) 追加 `pub mod phytium;`（L77，字母序 serial 与 x86_64 之间）+ `platform::phytium_d2000`（L239~L268 八常量子模块） | D9 Surgical Changes：既有 Hal trait / HalRegistry / platform / aarch64 / clock / serial / gpio / qemu_virt / x86_qemu / x86_64 / apic 零改动 |
| 模块挂载：`boot/mod.rs` | — | [boot/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/mod.rs) `boot.rs → boot/` 目录化重构 + `pub mod dtb;` + `pub use dtb::{dtb_find_node, dtb_find_node_or_err};` 重导出（保持 `crate::boot::dtb_find_node` 路径不变） | D2 boot.rs → boot/ 目录化 + 重导出 |
| 头注：[lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs) | — | lib.rs 头注交付物追加 v0.145.0 phytium 行（L35：PhytiumHal + PhytiumSmmu + PhytiumCrypto + dtb_find_node + ★ 信创核心 + 三层口径说明） | D9 Surgical Changes：仅追加头注一行 |
| 集成测试：`tests/phytium_test.rs` | PPI1~PPI7 | [tests/phytium_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/phytium_test.rs)（host 注入式集成测试——PPI1 十一方法多态分发 + PPI2 SMMU/crypto 状态回读 + PPI3 serial_putc 字节序 + PPI4 clock 换算 + PPI5 enable/disable IRQ + PPI6 SMMU/crypto 探测降级 + PPI7 ★ 跨架构三平台多态） | D1 三层口径第二层 host 可测；PPI7 核心验收点 |

**spec 分类口径对照**（spec.md「What Changes」）：实现类（PhytiumHal / PhytiumSmmu / PhytiumCrypto）→ §3/§4；DTB 节点查找设施（dtb_find_node）→ §5；配置类（phytium_d2000 平台常量）→ §6；文档类（phytium_bsp.md）→ 本文档；测试类（P1~P10 单测 + PPI1~PPI7 集成测试）→ §7。

---

## 3. PhytiumHal 设计

### 3.1 三字段组合结构图（蓝图 §3 + spec D3 组合关系）

```rust
pub struct PhytiumHal<B: RegIo = MmioRegIo> {
    base: Aarch64Hal<B>,                // D3 复用 ARM64 既有驱动（飞腾兼容 ARMv8）
    smmu: Option<PhytiumSmmu>,          // D4 cfg 门控真机路径 + host Cell 状态记录
    crypto: Option<PhytiumCrypto>,      // D5 has_sm4/has_sm2 占位 true
}
```

**与 v0.144.0 X86Hal 的根本差异**：X86Hal 是「三元组装」（apic + timer + uart 平行组合），PhytiumHal 是「组合 + 扩展」（base Aarch64Hal 整体复用 + smmu/crypto 飞腾特有扩展）。理由：飞腾 D2000 兼容 ARMv8 + GICv3 + PL011 + ARMv8 GenericTimer，复用 Aarch64Hal 既有驱动是 Simplicity First；SMMUv3 + crypto 引擎是飞腾特有，需独立占位类型扩展。

### 3.2 双构造（注入式 + cfg 门控真机）

```rust
impl<B: RegIo> PhytiumHal<B> {
    pub fn from_parts(base: Aarch64Hal<B>, smmu: Option<PhytiumSmmu>,
                      crypto: Option<PhytiumCrypto>) -> Self { ... }
}

impl PhytiumHal<MmioRegIo> {
    #[cfg(target_arch = "aarch64")]
    pub fn new(dtb: usize) -> Self {
        // 飞腾 D2000 平台常量组锚定 GICD/GICR/UART/GPIO 基址
        // + DTB 探测 SMMU/crypto 节点（cfg 门控真机 FDT 解析）
        // + .ok() 将 probe Err(NotSupported) 转 None（D1 降级路径）
    }
}
```

`from_parts` 是 host 测试统一入口（D3 注入式 host 可测化口径，同 X86Hal::from_parts）；`new(dtb)` 是 aarch64 真机入口，cfg 门控 host 编译时排除。

### 3.3 Hal trait 十一方法委托 + mem_init 联动

```rust
impl<B: RegIo> Hal for PhytiumHal<B> {
    fn cpu_init(&mut self) -> Result<(), KernelError> { self.base.cpu_init() }
    fn mem_init(&mut self) -> Result<(), KernelError> {
        if let Some(smmu) = &mut self.smmu { smmu.init(); }  // 联动 SMMU 初始化
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

`mem_init` 联动 SMMU 初始化是 PhytiumHal 特有路径（蓝图 §4.5 原样），不引入 panic（smmu.init 永远返回 ()，无错误传播）；其余十方法纯委托 base Aarch64Hal。

---

## 4. PhytiumSmmu 占位

### 4.1 结构（蓝图 §4.5 `PhytiumSmmu { base }` + spec D4）

```rust
pub struct PhytiumSmmu {
    base: usize,                    // SMMUv3 寄存器帧基址（DTB "arm,smmu-v3" 节点 reg）
    initialized: Cell<bool>,         // D4 host Cell 状态记录（同 LocalApic 口径）
}
```

### 4.2 init() cfg 门控真机 CR0 写（spec D4 + 蓝图 §4.5）

```rust
pub fn init(&mut self) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // SAFETY：SMMUv3 CR0 寄存器位于 base + 0x20（SMMUv3 spec §6.3.1），
        // base 由 DTB "arm,smmu-v3" 节点 reg 属性提供（飞腾 D2000 数据手册
        // 锚定物理地址），写 0x1 使能 SMMU（ClientPD=0 + SMMUEN=1）。
        core::ptr::write_volatile((self.base + SMMU_CR0_OFFSET) as *mut u32, SMMU_CR0_ENABLE);
    }
    self.initialized.set(true);  // 真机路径同步记录便于 host 断言
}
```

**与 v0.144.0 D8 cfg 门控的根本差异**：v0.144.0 X86Hal PortIo 真机路径 cfg 门控内为空注释（接口占位），v0.145.0 PhytiumSmmu::init 真机路径 cfg 门控内**必须实现** `unsafe { write_volatile(...) }`（信创认证代码化证据输入）。

### 4.3 probe() DTB 节点查找（spec D2 + D6）

```rust
pub fn probe(dtb: usize) -> Result<Self, KernelError> {
    let base = dtb_find_node(dtb, SMMU_COMPAT).ok_or(KernelError::NotSupported)?;
    Ok(Self::new(base))
}
```

`dtb_find_node` host 占位返回 None → probe 返回 `Err(NotSupported)` → `.ok() = None` → 触发 D1 第三层降级路径（smmu=None 占位实例，mem_init 跳过联动仍 Ok）。

---

## 5. dtb_find_node DTB 节点查找设施

### 5.1 蓝图偏差 D2（重大）

蓝图 §4.5 `PhytiumSmmu::probe` / `PhytiumCrypto::probe` 引用 `crate::boot::dtb_find_node(dtb, "arm,smmu-v3")` / `crate::boot::dtb_find_node(dtb, "phytium,crypto")`，但 v0.144.0 前 `boot.rs` 仅有 `BootInfo` 结构体，无 `dtb_find_node` 函数。属重大蓝图偏差 D2。

### 5.2 落地：boot.rs → boot/ 目录化 + 新建 dtb.rs

```rust
// crates/kernel/microkernel/src/boot/mod.rs
pub mod dtb;
pub use dtb::{dtb_find_node, dtb_find_node_or_err};  // 保持 crate::boot::dtb_find_node 路径不变

// crates/kernel/microkernel/src/boot/dtb.rs
pub fn dtb_find_node(dtb: usize, compat: &str) -> Option<usize> {
    #[cfg(target_arch = "aarch64")] { /* FDT 解析 */ }
    #[cfg(not(target_arch = "aarch64"))] { None }  // host 占位返回 None
}
```

### 5.3 零新依赖（spec D2 + D8 口径）

复用 `crates/kernel/bootloader/src/dtb.rs` 的 FDT 解析模式（FDT_MAGIC / FDT_BEGIN_NODE / FDT_PROP / FDT_END token + strings block 查找 + align4 helper），不引入外部 `fdt` crate（与 v0.144.0 D8 口径一致，避免 SBOM 风险）。

### 5.4 D6 兼容性字符串

蓝图 §4.5 原样字符串 `"arm,smmu-v3"` 与 `"phytium,crypto"`，子串匹配兼容多值 compatible 属性。真机调试扩展点：飞腾 D2000 实际 DTB 兼容性字符串可能为 `"phytium,smmu-v3"` 或 `"arm,smmu-v3"` 等，按 `/proc/device-tree` 实际值调整即可，不影响类型签名。

---

## 6. PhytiumCrypto 占位

### 6.1 结构（蓝图 §4.1 数据结构 + spec D5）

```rust
pub struct PhytiumCrypto {
    base: usize,                 // crypto 引擎寄存器帧基址（DTB "phytium,crypto" 节点 reg）
    sm2_enabled: bool,           // D5 占位 true（飞腾 D2000 数据手册明确支持 SM2）
    sm4_enabled: bool,           // D5 占位 true（飞腾 D2000 数据手册明确支持 SM4/SM3 指令）
}
```

### 6.2 has_sm4/has_sm2 占位 true（spec D5）

蓝图 §4.5 原样 `has_sm4(&self) -> bool { true }`，飞腾 D2000 数据手册明确支持 SM4/SM3 指令，硬编码 true 是合理占位。真机扩展点：应读 `ID_AA64ISAR0_EL1.SM4` 字段（bits[63:60] != 0）+ `ID_AA64ISAR0_EL1.SM3` 字段（bits[59:56] != 0）覆盖占位 true。

---

## 7. platform::phytium_d2000 平台配置（spec D10）

```rust
pub mod phytium_d2000 {
    pub const UART_PL011_BASE: usize = 0x2800_0000;  // PL011 UART0 基址占位
    pub const GICD_BASE: usize = 0x2f00_0000;          // GICv3 分发器基址占位
    pub const GICR_BASE: usize = 0x2f10_0000;          // GICv3 再分发器基址占位
    pub const GPIO_PL061_BASE: usize = 0x2801_0000;   // GPIO 基址占位
    pub const SMMU_BASE: usize = 0x2b00_0000;          // SMMUv3 基址占位
    pub const CRYPTO_BASE: usize = 0x2c00_0000;       // crypto 引擎基址占位
    pub const CNTP_IRQ: u32 = 30;                       // ARM 架构固定
    pub const CNTV_IRQ: u32 = 27;                       // ARM 架构固定
}
```

与既有 `qemu_virt`（ARM64）/ `x86_qemu`（x86_64）并列。占位地址依据飞腾 D2000 数据手册，实际值待真机调试确认；与 `configs/phytium-d2000.dts` 占位地址对齐（spec D8）。

---

## 8. 测试矩阵

### 8.1 host 单测（hal/phytium.rs 内 #[cfg(test)]，P1~P10）

| 编号 | 测试点 | 验收 |
|------|--------|------|
| P1 | from_parts 注入式构造 + base()/smmu()/crypto() 观测口回读 | 三观测口回读正确 |
| P2 | 六 init 蓝图顺序全 Ok + mem_init 联动 smmu.init | is_initialized 由 false 转 true |
| P3 | serial_putc tx_log == b"PHY" 字节序 | tx_log 内容序一致 |
| P4 | clock_now_us 注入源换算 + 单调性双读 | 62.5MHz ⇔ 1_000_000us；单调性 |
| P5 | clock_deadline 编程记录 last_deadline_ticks + disarm | deadline(5_000_000us) ⇔ 312_500_000 ticks |
| P6 | enable_irq/disable_irq 委托 base Aarch64Hal + 边界防御 | ISENABLER1 bit4 置位；irq ≥ 1024 no-op |
| P7 | PhytiumSmmu init + probe 降级 | init 后 is_initialized == true；probe host 返回 Err(NotSupported) |
| P8 | PhytiumCrypto 占位 + probe 降级 | has_sm4/has_sm2 == true；probe host 返回 Err(NotSupported) |
| P9 | mem_init 降级路径（smmu=None 时仍 Ok） | smmu=None / crypto=None 时 mem_init 仍 Ok |
| P10 | trait 对象多态分发（十一方法） | `&mut dyn Hal` 全方法可调 + 副作用核验 |

### 8.2 host 集成测试（tests/phytium_test.rs，PPI1~PPI7）

| 编号 | 测试点 | 验收 |
|------|--------|------|
| PPI1 | `&mut dyn Hal` 十一方法多态分发（PhytiumHal 注入式） | 六 init 全 Ok + 运行时五方法不崩溃 |
| PPI2 | 六 init 全链路 Ok + SMMU/crypto 状态回读 | smmu.is_initialized() == true；has_sm4/has_sm2 == true |
| PPI3 | serial_putc tx_log == b"PHY" 字节序 | tx_log 内容序一致；DR 偏移 0x00 末字节 b'Y' |
| PPI4 | clock_now_us 注入源换算 + 单调性 + deadline 编程 | 62.5MHz ⇔ 1_000_000us；deadline(5_000_000us) ⇔ 312_500_000 ticks |
| PPI5 | enable/disable IRQ 委托 base GICv3 寄存器帧回读 + 边界 1024/2000 no-op | ISENABLER1 bit4 置位；边界 no-op |
| PPI6 | SMMU/crypto 探测降级（host dtb_find_node 返回 None） | smmu=None/crypto=None → mem_init 仍 Ok；probe 返回 Err(NotSupported) |
| PPI7 | ★ 跨架构 Hal trait 三平台多态——Aarch64Hal + X86Hal + PhytiumHal 同经 `&mut dyn Hal` 分发六 init 全 Ok | **核心验收点**：证明 v0.143.0 接口冻结后 BSP 可扩展至飞腾信创生产平台 |

### 8.3 不做的测试（D1 第三层——真机验证留待硬件）

- 不做飞腾 D2000 真机启动（`tests/phytium_boot.sh` 占位早期 exit 0）
- 不做飞腾 DTB 实机解析（host 占位返回 None，aarch64 真机解析路径 cfg 门控留待真机调试）
- 不做 SM4 指令 CPUID 检测（`has_sm4()` 占位 true，真机 CPUID 检测留待真机）
- 不做 SMMUv3 完整驱动验证（仅 CR0 使能占位，完整命令队列/页表/上下文管理留待真机）
- 不做 aarch64 交叉编译飞腾真机二进制（cfg 门控下 aarch64 编译包含真机寄存器写路径，但 main.rs 仍走 Aarch64Hal QEMU 路径，飞腾真机入口留待硬件）

---

## 9. Hal trait 跨架构三平台多态（PPI7 核心验收点）

### 9.1 三平台分发验证（spec PPI7）

PPI7 集成测试验证 `Hal` trait 可同时承载 `Aarch64Hal`、`X86Hal`、`PhytiumHal` 三个实现：

```rust
// Aarch64Hal 注入式构造（hal_test.rs HPI1 口径）
let mut aarch64_rig = Aarch64Rig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut aarch64_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... 其余五 init + 五运行时 */ }

// X86Hal 注入式构造（x86_test.rs XPI6 口径）
let mut xhal = make_x86hal(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut xhal;
  assert!(h.cpu_init().is_ok()); /* ... */ }

// PhytiumHal 注入式构造（本版本核心交付）
let mut phytium_rig = PhytiumRig::new(FREQ_QEMU, mock_cycles_fixed);
{ let h: &mut dyn Hal = &mut phytium_rig.hal;
  assert!(h.cpu_init().is_ok()); /* ... */ }
assert!(phytium_rig.hal.smmu().unwrap().is_initialized());  // PhytiumHal 特有路径
```

### 9.2 接口冻结后 BSP 可扩展性证据

v0.143.0 → v0.144.0 两平台（Aarch64Hal + X86Hal）→ v0.145.0 三平台（+ PhytiumHal），证明 v0.143.0 Hal trait 接口冻结后跨架构 + 跨平台可扩展性。飞腾作为信创生产平台 BSP 出口验证通过 → 信创认证代码化证据输入。

---

## 10. D1~D11 偏差表 + 落地修正

| D 编号 | 蓝图偏差 | 决策 | 落地 |
|--------|---------|------|------|
| D1 | ★ 信创核心 vs 本机无飞腾硬件 | 真机路径保留 + host 可测骨架 + 真机验证留待硬件（三层口径） | PhytiumSmmu::init cfg 门控 unsafe CR0 写 + PhytiumHal::from_parts 注入式 + tests/phytium_boot.sh 占位 |
| D2 | `crate::boot::dtb_find_node` 不存在 | boot.rs → boot/ 目录化 + 新建 boot/dtb.rs + 重导出 dtb_find_node | boot/mod.rs `pub use dtb::{dtb_find_node, dtb_find_node_or_err};` |
| D3 | `Aarch64Hal::new(dtb)` 不存在 | PhytiumHal::new 内部用 from_parts + cfg 门控（不新增 Aarch64Hal::new_from_dtb） | PhytiumHal::new(dtb) cfg 门控 aarch64 真机路径 + from_parts host 注入 |
| D4 | SMMUv3 寄存器操作 unsafe | cfg 门控真机路径 + host Cell 状态记录 + SAFETY 注释 | PhytiumSmmu::init() `#[cfg(target_arch = "aarch64")] unsafe { write_volatile(...) }` + `initialized.set(true)` |
| D5 | `crypto has_sm4()` 恒 true | 占位 true + 真机 SM4 指令 CPUID 检测留待真机 | PhytiumCrypto::has_sm4/has_sm2 占位 true + 注释扩展点 |
| D6 | DTB 兼容性字符串 | 蓝图原样 + 真机调试时按需调整 | SMMU_COMPAT = "arm,smmu-v3"；CRYPTO_COMPAT = "phytium,crypto"；子串匹配 |
| D7 | `tests/phytium_boot.sh` 真机脚本 | 脚本占位（cfg 门控早期 exit 0 + [phytium] skip 标注）+ 硬件就绪后扩展 | tests/phytium_boot.sh 头部硬件探测占位 + TODO 扩展点 |
| D8 | 飞腾 DTB 文件 | configs/phytium-d2000.dts 占位最小骨架（CPU + GICv3 + UART + SMMUv3 + crypto + timer 节点占位） | 与 D10 platform::phytium_d2000 常量对齐 |
| D9 | 不改 Hal trait（接口已冻结） | v0.143.0 Hal trait 十一方法已冻结，本版本仅消费 trait 实现 | hal/mod.rs 仅追加 `pub mod phytium;` + `platform::phytium_d2000` |
| D10 | `platform::phytium_d2000` 子模块 | 与 qemu_virt / x86_qemu 并列，占位地址依据飞腾 D2000 数据手册 | 八常量子模块（UART/GICD/GICR/GPIO/SMMU/CRYPTO/CNTP_IRQ/CNTV_IRQ） |
| D11 | PhytiumHal 文件位置 | 单文件 hal/phytium.rs（参考 v0.144.0 X86Hal 单文件先例） | PhytiumHal + PhytiumSmmu + PhytiumCrypto 全部在单文件 |

---

## 11. 后续真机路线图（D1 第三层——硬件就绪后扩展）

硬件就绪后的扩展点清单（按优先级排序）：

1. **`tests/phytium_boot.sh` 真机启动链**：交叉编译 bootloader + kernel → dtc 加载 `configs/phytium-d2000.dts` → 飞腾开发板串口启动 → 断言 `[kernel] EnerOS kernel booting` + `[phytium] smmu ok` + `[phytium] crypto ok`（cfg 门控早期 exit 0 → 真机扩展点 TODO）。
2. **`configs/phytium-d2000.dts` 实际 DTB**：按飞腾 D2000 实际 `/proc/device-tree` 值补齐 reg 地址 + compatible 字符串 + 节点完整属性（D6 + D8 占位 → 真机值）。
3. **`dtb_find_node` aarch64 真机解析路径调试**：cfg 门控 FDT 解析在真机 DTB 上验证节点匹配 + reg 属性返回（host 占位返回 None → 真机实际解析）。
4. **SMMUv3 完整驱动**：CR0 使能占位 → 完整命令队列 / 页表 / 上下文管理（Stream Table Entry / Context Descriptor / Translation Table Base 等）。
5. **SM4 指令 CPUID 检测**：`has_sm4()` 占位 true → 真机读 `ID_AA64ISAR0_EL1.SM4` 字段（bits[63:60] != 0）+ `ID_AA64ISAR0_EL1.SM3` 字段（bits[59:56] != 0）覆盖占位 true。
6. **main.rs 飞腾真机入口冒烟段**：cfg 门控下 aarch64 编译 main.rs 仍走 Aarch64Hal QEMU 路径；硬件就绪后扩展飞腾真机入口（DTB 探测 + PhytiumHal::new + 冒烟断言 `[phytium] smmu ok` + `[phytium] crypto ok`）。
7. **真机 SMMUv3 上下文表 / SM4 加密缓冲区**：属 v0.158.0 国密硬件加速版本范畴，本版本不涉及。

---

## 12. 源码链接

- [crates/kernel/microkernel/src/hal/phytium.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/phytium.rs)（PhytiumHal + PhytiumSmmu + PhytiumCrypto + impl Hal + P1~P10 单测）
- [crates/kernel/microkernel/src/boot/dtb.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/dtb.rs)（dtb_find_node + dtb_find_node_or_err + DT1~DT3 单测）
- [crates/kernel/microkernel/src/boot/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/boot/mod.rs)（BootInfo + pub mod dtb + 重导出）
- [crates/kernel/microkernel/src/hal/mod.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/mod.rs)（Hal trait + HalRegistry + pub mod phytium + platform::phytium_d2000）
- [crates/kernel/microkernel/src/hal/aarch64.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/aarch64.rs)（Aarch64Hal——PhytiumHal 组合复用，v0.143.0 冻结）
- [crates/kernel/microkernel/src/hal/apic.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/hal/apic.rs)（LocalApic——PhytiumSmmu Cell 口径参考，v0.144.0）
- [crates/kernel/microkernel/src/lib.rs](file:///e:/EnerOS/crates/kernel/microkernel/src/lib.rs)（lib.rs 头注 v0.145.0 phytium 行）
- [crates/kernel/microkernel/tests/phytium_test.rs](file:///e:/EnerOS/crates/kernel/microkernel/tests/phytium_test.rs)（PPI1~PPI7 集成测试）
- [tests/phytium_boot.sh](file:///e:/EnerOS/tests/phytium_boot.sh)（飞腾真机启动脚本占位）
- [configs/phytium-d2000.dts](file:///e:/EnerOS/configs/phytium-d2000.dts)（飞腾 D2000 DTB 占位最小骨架）
- [docs/kernel/phytium_bsp.md](file:///e:/EnerOS/docs/kernel/phytium_bsp.md)（本文档）
- [.trae/specs/develop-v14500-phytium-bsp/spec.md](file:///e:/EnerOS/.trae/specs/develop-v14500-phytium-bsp/spec.md)（v0.145.0 spec）
- [.trae/specs/develop-v14500-phytium-bsp/tasks.md](file:///e:/EnerOS/.trae/specs/develop-v14500-phytium-bsp/tasks.md)（v0.145.0 任务分解）
