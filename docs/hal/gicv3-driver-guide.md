# GICv3 驱动说明

> 版本：v0.6.0
> 适用范围：EnerOS HAL ARM64 中断控制器驱动（GICv3）
> 蓝图依据：`蓝图/phase0.md` §v0.6.0、§4.5
> crate：eneros-hal（`hal/src/arm64/gicv3.rs`）
> 硬件参考：ARM GICv3 Architecture Specification（ARM IHI 0069）

---

## 1. 概述

ARM Generic Interrupt Controller version 3（GICv3）是 ARM 架构的中断控制器，为多核 SoC 提供中断分发、优先级仲裁与屏蔽能力。EnerOS 在 v0.6.0 选用 GICv3（而非 GICv2）作为 QEMU virt 与飞腾/鲲鹏平台的中断控制器基础。

### 1.1 三层架构设计

GICv3 采用三层分布式架构：

| 层级 | 名称 | 缩写 | 作用 | 寻址方式 |
|------|------|------|------|----------|
| 第一层 | Distributor | GICD | 全局中断分发：SPI（Shared Peripheral Interrupt）的使能/屏蔽/优先级/路由 | MMIO（全局唯一，所有核共享） |
| 第二层 | Redistributor | GICR | per-core 中断管理：SGI（Software Generated Interrupt）与 PPI（Private Peripheral Interrupt）的使能/优先级/配置；LPI 状态缓存 | MMIO（per-core，每个核一个实例） |
| 第三层 | CPU Interface | ICC | per-core 中断确认与结束：读取中断号（IAR）、结束中断（EOIR）、优先级屏蔽（PMR）、抢占配置（BPR） | ARM64 系统寄存器（`ICC_*_EL1`） |

### 1.2 中断类型

| 类型 | 缩写 | ID 范围 | 管理者 | 典型用途 |
|------|------|---------|--------|----------|
| Software Generated Interrupt | SGI | 0–15 | GICR | 核间通信（IPI） |
| Private Peripheral Interrupt | PPI | 16–31 | GICR | per-core 私有外设（Generic Timer、PMU、GIC 维护中断） |
| Shared Peripheral Interrupt | SPI | 32–1019 | GICD | 全局共享外设（UART、网卡、磁盘控制器） |
| Locality-specific Peripheral Interrupt | LPI | 8192+ | GICR + ITS | MSI/MSI-X 设备（v0.6.0 不涉及） |

> **特殊值**：IRQ ID 1020–1023 为保留值，其中 **1023** 表示"无中断"（spurious interrupt），读 `ICC_IAR1_EL1` 返回 1023 表示当前无待处理中断。

### 1.3 QEMU virt 默认配置

EnerOS QEMU virt 平台的 GICv3 基址由设备树 `board/qemu-virt/dts` 定义：

```
gic: interrupt-controller@8000000 {
    compatible = "arm,gic-v3";
    reg = <0x0 0x08000000 0x0 0x0100000>,  // GICD: 0x08000000, 1MB
          <0x0 0x080a0000 0x0 0xf60000>;   // GICR: 0x080A0000, ~15MB
    interrupts = <1 9 7>;                  // GIC 维护中断（PPI 9）
};
```

| 寄存器块 | 基址 | 大小 | 说明 |
|----------|------|------|------|
| GICD | `0x0800_0000` | 1 MB | Distributor，全局唯一 |
| GICR | `0x080A_0000` | ~15 MB | Redistributor，连续排列（每个核占 2 × 64KB） |

---

## 2. GICv3 vs GICv2 差异

### 2.1 为什么选 GICv3

| 原因 | 说明 |
|------|------|
| 蓝图合规性 | 蓝图 §43.2 合规性修复明确要求 GICv3 一致性，禁止用 GICv2 代码冒充 |
| 飞腾/鲲鹏兼容 | 国产化目标平台（飞腾 D2000、鲲鹏 920）原生支持 GICv3 |
| 多核扩展性 | GICv3 的 Redistributor + 亲和性路由支持 8+ 核场景，GICv2 受限于 8 核 target 位图 |
| 中断数量 | GICv3 最多 1020 个 SPI（GICv2 仅约 480 个），满足能源边缘外设需求 |
| 性能 | 系统寄存器模式（`ICC_*_EL1`）比内存映射（GICC）少一次总线访问 |
| 虚拟化基础 | GICv3 原生支持虚拟化（`ICH_*` 系统寄存器），为后续 seL4 虚拟化铺路 |

### 2.2 关键差异表

| 特性 | GICv2 | GICv3 |
|------|-------|-------|
| 最大中断数 | ~480（`ITLinesNumber × 32`） | 1020（SPI）+ LPI（最多 2^14） |
| CPU Interface | 内存映射（GICC 基址） | **系统寄存器**（`ICC_*_EL1`），需 `ICC_SRE_EL1` 使能 |
| Redistributor | 无（GICD 集中管理所有中断） | **有**（per-core GICR，管理 SGI/PPI/LPI） |
| 亲和性路由 | 无（`GICD_ITARGETSRn`，8-bit target 位图） | **有**（`GICD_IROUTERn`，64-bit `Aff3:Aff2:Aff1:Aff0`） |
| SGI/PPI 管理 | GICD 集中管理 | GICR per-core 管理（每核独立配置） |
| 中断路由模式 | target 位图（每核 1 bit） | 亲和性路由（64-bit 亲和性值，精确到核） |
| LPI / ITS | 不支持 | 支持（`GITS_*` 寄存器，MSI/MSI-X） |
| 优先级位数 | 5 bit（32 级） | 5–8 bit（可配置，GICD_TYPER.IDBits） |
| 虚拟化 | GICH（内存映射 Hypervisor Control） | `ICH_*` 系统寄存器 |
| 安全扩展 | 基础 Group 0/1 | 增强安全模型（Group 0/1 + Secure/Non-secure 独立） |
| 兼容模式 | — | 可配置为 GICv2 兼容模式（禁用 ARE 后），但失去 GICv3 优势 |

### 2.3 架构对比图

```
GICv2 架构（扁平式）:
  GICD ──┬── GICC (CPU0 Interface, MMIO)
         ├── GICC (CPU1 Interface, MMIO)
         └── GICC (CPUn Interface, MMIO)

GICv3 架构（分布式）:
  GICD ──┬── GICR0 ── ICC (CPU0, 系统寄存器)
         ├── GICR1 ── ICC (CPU1, 系统寄存器)
         └── GICRn ── ICC (CPUn, 系统寄存器)
```

> **设计决策 D2**（v0.6.0 spec）：CPU interface 使用 `ICC_*_EL1` 系统寄存器模式，而非 GICv2 内存映射兼容模式。GICv3 使能 ARE（Affinity Routing Enable）后必须用系统寄存器，GICv2 兼容模式无法使用 Redistributor 的亲和性路由。

---

## 3. 寄存器参考

### 3.1 GICD（Distributor）寄存器

GICD 寄存器基址为 `0x0800_0000`（QEMU virt），所有核共享。以下偏移相对 GICD 基址。

| 寄存器 | 偏移 | 访问 | 说明 |
|--------|------|------|------|
| `GICD_CTLR` | `0x000` | RW | Distributor 控制：使能 Group 0/1、ARE 亲和性路由 |
| `GICD_TYPER` | `0x004` | RO | Distributor 类型：中断数量、优先级位数、安全扩展支持 |
| `GICD_IIDR` | `0x008` | RO | 实现者 ID：厂商号与版本号 |
| `GICD_STATUSR` | `0x010` | RW | 状态寄存器（GICv3 新增） |
| `GICD_IGROUPRn` | `0x080` | RW | 中断分组：n=0 对应 SGI+PPI（SPI 由 GICR 管理），n≥1 对应 SPI；每 bit 控制一个中断的 Group（0=Group 0, 1=Group 1） |
| `GICD_ISENABLERn` | `0x100` | RW | 中断使能：写 1 使能对应中断；n=0 管理中断 0–31，n=1 管理 32–63，以此类推 |
| `GICD_ICENABLERn` | `0x180` | RW | 中断禁用：写 1 禁用对应中断 |
| `GICD_ISPENDRn` | `0x200` | RW | 中断挂起设置：写 1 将中断置为 pending |
| `GICD_ICPENDRn` | `0x280` | RW | 中断挂起清除：写 1 清除 pending 状态 |
| `GICD_ISACTIVERn` | `0x300` | RW | 中断激活设置：写 1 将中断置为 active |
| `GICD_ICACTIVERn` | `0x380` | RW | 中断激活清除：写 1 清除 active 状态 |
| `GICD_IPRIORITYRn` | `0x400` | RW | 中断优先级：每个中断 8 bit（实际有效位数由 `GICD_TYPER.IDBits` 决定，QEMU virt 为 5 bit），n 以 4 字节对齐，每 4 字节含 4 个中断的优先级 |
| `GICD_ICFGRn` | `0xC00` | RW | 中断配置：每 2 bit 控制一个中断的触发类型（bit[1]=edge, bit[0]=1）；n=0 对应 SGI 0–15，n=1 对应 PPI 16–31，n=2 对应 SPI 32–47 |
| `GICD_IROUTERn` | `0x6000` | RW | 中断路由（GICv3 新增）：64-bit 亲和性值，每中断一个；SPI 的路由目标核由 `Aff3:Aff2:Aff1:Aff0` 指定；`n = IRQ_ID` |

#### GICD_CTLR 位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0 | EnableGrp0 | `0x1` | 使能 Group 0 中断（Secure） |
| bit 1 | EnableGrp1 | `0x2` | 使能 Group 1 中断（Non-secure） |
| bit 4 | ARE_NS | `0x10` | Non-secure 亲和性路由使能（GICv3 必须=1） |
| bit 5 | ARE_S | `0x20` | Secure 亲和性路由使能 |
| bit 31 | RWP | — | Register Write Pending（只读，1=有寄存器写入待完成） |

> **EnerOS 初始化值**：`GICD_CTLR = 0x12`（EnableGrp1=1, ARE_NS=1），使能 Group 1 中断并开启亲和性路由。

#### GICD_TYPER 关键位

| 位 | 名称 | 说明 |
|----|------|------|
| bits [4:0] | ITLinesNumber | SPI 数量 = `(ITLinesNumber + 1) × 32` |
| bit 10 | SecurityExtn | 安全扩展支持（1=支持 Group 0/1 分离） |
| bits [13:11] | IDBits | 优先级位数 = `IDBits + 1`（QEMU virt 通常为 4，即 5 bit） |

### 3.2 GICR（Redistributor）寄存器

每个 CPU 核对应一个 Redistributor 实例，由两个连续的 64KB 帧组成：

| 帧 | 偏移（相对 GICR 基址） | 名称 | 作用 |
|----|----------------------|------|------|
| RD_base | `0x0000` | Control frame | Redistributor 控制、唤醒、LPI 配置 |
| SGI_base | `0x10000` | SGI/PPI frame | per-core SGI(0–15) + PPI(16–31) 的使能/优先级/配置 |

> **多核排列**：QEMU virt 中 GICR 实例连续排列，每核占 `2 × 0x10000 = 0x20000` 字节。CPU0 的 GICR 在 `0x080A_0000`，CPU1 在 `0x080C_0000`，以此类推。

#### RD_base 寄存器（偏移相对 RD_base）

| 寄存器 | 偏移 | 访问 | 说明 |
|--------|------|------|------|
| `GICR_CTLR` | `0x000` | RW | Redistributor 控制：LPI 使能、Group 屏蔽 |
| `GICR_IIDR` | `0x004` | RO | 实现者 ID |
| `GICR_TYPER` | `0x008` | RO | Redistributor 类型：亲和性值、ProcessorNumber、Last 标志（定位下一个 GICR） |
| `GICR_STATUSR` | `0x010` | RW | 状态寄存器 |
| `GICR_WAKER` | `0x014` | RW | 唤醒控制：ProcessorSleep / ChildrenAsleep |
| `GICR_PROPBASER` | `0x070` | RW | LPI 配置表基址（v0.6.0 不使用） |
| `GICR_PENDBASER` | `0x078` | RW | LPI 挂起表基址（v0.6.0 不使用） |

#### GICR_WAKER 位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0 | ProcessorSleep | `0x1` | 写 1 使核心进入睡眠（清除=唤醒） |
| bit 1 | ChildrenAsleep | `0x2` | 只读，1=下游逻辑已睡眠（轮询此位清零确认唤醒完成） |

> **唤醒流程**：清除 `ProcessorSleep`（写 0）→ 轮询 `ChildrenAsleep` 直到读回 0，表示 Redistributor 已就绪。

#### GICR_TYPER 关键位

| 位 | 名称 | 说明 |
|----|------|------|
| bits [23:0] | ProcessorNumber | Redistributor 序号（用于多核遍历定位） |
| bits [39:32] | Affinity | 亲和性值 Aff3:Aff2:Aff1:Aff0（与 MPIDR_EL1 对应） |
| bit 12 | Last | 1=当前是最后一个 Redistributor（遍历终止条件） |
| bit 13 | DirectLPI | 1=支持 Direct LPI 注入 |

#### SGI_base 寄存器（偏移相对 SGI_base = RD_base + 0x10000）

以下寄存器仅管理当前核的 SGI(0–15) + PPI(16–31)，布局与 GICD 前 32 个中断的寄存器一致：

| 寄存器 | 偏移（相对 SGI_base） | 访问 | 说明 |
|--------|----------------------|------|------|
| `GICR_IGROUPR0` | `0x080` | RW | SGI/PPI 分组：32 bit 对应中断 0–31 |
| `GICR_ISENABLER0` | `0x100` | RW | SGI/PPI 使能：写 1 使能 |
| `GICR_ICENABLER0` | `0x180` | RW | SGI/PPI 禁用：写 1 禁用 |
| `GICR_ISPENDR0` | `0x200` | RW | SGI/PPI 挂起设置 |
| `GICR_ICPENDR0` | `0x280` | RW | SGI/PPI 挂起清除 |
| `GICR_ISACTIVER0` | `0x300` | RW | SGI/PPI 激活设置 |
| `GICR_ICACTIVER0` | `0x380` | RW | SGI/PPI 激活清除 |
| `GICR_IPRIORITYR0` | `0x400` | RW | SGI/PPI 优先级（每中断 8 bit，共 32 字节） |
| `GICR_ICFGR0` | `0xC00` | RW | SGI(0–15) 触发配置 |
| `GICR_ICFGR1` | `0xC04` | RW | PPI(16–31) 触发配置 |

### 3.3 ICC（CPU Interface）系统寄存器

GICv3 的 CPU interface 通过 ARM64 系统寄存器访问（非内存映射），需先使能 `ICC_SRE_EL1`。

| 系统寄存器 | 编码 | 访问 | 说明 |
|-----------|------|------|------|
| `ICC_SRE_EL1` | `S3_0_C12_C12_5` | RW | System Register Enable：bit0=1 使能系统寄存器模式（GICv3 必须使能） |
| `ICC_IGRPEN1_EL1` | `S3_0_C12_C12_7` | RW | Group 1 中断使能：bit0=1 使能 Group 1 中断信号传递到 CPU |
| `ICC_IGRPEN0_EL1` | `S3_0_C12_C12_6` | RW | Group 0 中断使能（Secure，EnerOS 不使用） |
| `ICC_PMR_EL1` | `S3_0_C4_C6_0` | RW | 优先级屏蔽：8-bit，值越小屏蔽越严格；`0xFF`=允许所有优先级 |
| `ICC_BPR1_EL1` | `S3_0_C12_C12_3` | RW | 二进制点（抢占优先级分组）：值越小允许越多抢占层级 |
| `ICC_IAR1_EL1` | `S3_0_C12_C12_0` | RO | 中断确认：读取返回当前最高优先级 pending 中断的 IRQ ID（读操作有副作用：将中断置为 active） |
| `ICC_EOIR1_EL1` | `S3_0_C12_C12_1` | WO | 结束中断：写入 IRQ ID 表示处理完成（使中断可再次触发）；必须与 IAR 配对 |
| `ICC_CTLR_EL1` | `S3_0_C12_C12_4` | RW | CPU interface 控制：优先级位数、EOI 模式 |
| `ICC_RPR_EL1` | `S3_0_C12_C11_3` | RO | Running Priority：当前 active 中断的优先级 |

#### ICC_IAR1_EL1 读取语义

- **原子操作**：读 `ICC_IAR1_EL1` 是 GICv3 的中断确认原子操作，同时完成两件事：
  1. 返回当前最高优先级 pending 中断的 IRQ ID
  2. 将该中断从 pending 转为 active 状态（防止重复分发）
- **伪中断（Spurious）**：若无 pending 中断，返回 `1023`（`0x3FF`），调用方应忽略
- **必须配对**：每次 IAR 读取后，必须对同一 IRQ ID 执行 EOIR 写入，否则该中断永远无法再次触发

#### ICC_PMR_EL1 优先级屏蔽

| PMR 值 | 含义 |
|--------|------|
| `0x00` | 屏蔽所有中断（无中断能通过） |
| `0x80` | 仅允许优先级高于 `0x80` 的中断（数值越小优先级越高） |
| `0xFF` | 允许所有优先级的中断（EnerOS 默认值） |

> **优先级语义**：ARM GIC 中优先级数值越小表示优先级越高（0=最高优先级）。

---

## 4. 初始化序列

EnerOS v0.6.0 的 GICv3 初始化分四步，在 `Arm64Gic::init()` 中实现。必须在调度器启动前、单核 boot 上下文完成。

### 4.1 步骤1：GICD 使能（Distributor）

使能 Distributor 并开启亲和性路由：

```rust
// 使能 Group 1 中断 + Non-secure 亲和性路由
// GICD_CTLR = EnableGrp1(0x2) | ARE_NS(0x10) = 0x12
unsafe {
    w32(GICD_BASE, GICD_CTLR, 0x12);
}
```

| 操作 | 寄存器 | 写入值 | 说明 |
|------|--------|--------|------|
| 使能 Group 1 | `GICD_CTLR` bit 1 | `0x2` | 允许 Non-secure Group 1 中断分发 |
| 使能 ARE_NS | `GICD_CTLR` bit 4 | `0x10` | 开启 Non-secure 亲和性路由（GICv3 必须） |

> **ARE 使能后不可逆**：一旦 `ARE_NS=1`，`GICD_ITARGETSRn`（GICv2 target 寄存器）失效，必须改用 `GICD_IROUTERn`。GICv2 兼容模式无法回退。

### 4.2 步骤2：GICR per-core 唤醒（Redistributor）

每个核心启动时必须唤醒自己的 Redistributor：

```rust
// 1. 清除 ProcessorSleep，请求唤醒
unsafe {
    w32(GICR_BASE, GICR_WAKER, 0);
}
// 2. 轮询 ChildrenAsleep 直到清零（唤醒完成）
unsafe {
    while r32(GICR_BASE, GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0 {
        core::hint::spin_loop();
    }
}
```

| 步骤 | 寄存器 | 操作 | 说明 |
|------|--------|------|------|
| 请求唤醒 | `GICR_WAKER` bit 0 | 写 0（清除 ProcessorSleep） | 通知 Redistributor 核心已唤醒 |
| 等待就绪 | `GICR_WAKER` bit 1 | 轮询直到读回 0（ChildrenAsleep 清零） | Redistributor 内部逻辑完成初始化 |

> **多核注意**：v0.6.0 简化为单核（仅唤醒 CPU0 的 GICR）。多核场景需遍历 `GICR_TYPER` 定位每个核的 Redistributor 基址（见 §7）。

### 4.3 步骤3：CPU Interface 使能

通过系统寄存器使能 CPU interface：

```rust
// 1. 使能系统寄存器访问（GICv3 必须）
unsafe {
    asm!("msr icc_sre_el1, {0}", in(reg) 0x1u64);
}
// 2. 设置优先级屏蔽为 0xFF（允许所有优先级）
unsafe {
    asm!("msr icc_pmr_el1, {0}", in(reg) 0xFFu64);
}
// 3. 使能 Group 1 中断信号传递
unsafe {
    asm!("msr icc_igrpen1_el1, {0}", in(reg) 0x1u64);
}
```

| 步骤 | 系统寄存器 | 写入值 | 说明 |
|------|-----------|--------|------|
| 系统寄存器使能 | `ICC_SRE_EL1` bit 0 | `0x1` | 使能 `ICC_*_EL1` 系统寄存器访问（替代 GICC 内存映射） |
| 优先级屏蔽 | `ICC_PMR_EL1` | `0xFF` | 允许所有优先级中断通过（不屏蔽） |
| Group 1 使能 | `ICC_IGRPEN1_EL1` bit 0 | `0x1` | 使能 Group 1 中断信号传递到 CPU 核心 |

### 4.4 步骤4：默认优先级设置

为所有中断设置默认优先级（可选但推荐）：

```rust
// 将 SPI 32–255 的优先级设为 0xA0（默认中等优先级）
for irq in 32..MAX_IRQ {
    let reg_offset = GICD_IPRIORITYR + (irq as u32);
    unsafe {
        w32(GICD_BASE, reg_offset, 0xA0);
    }
}
```

> **优先级约定**：EnerOS 默认优先级 `0xA0`，数值越小优先级越高。中断优先级 `0x00` 为最高（紧急中断），`0xFF` 为最低。

### 4.5 完整初始化序列

```
boot 早期（单核、中断关闭）
  │
  ├─ Step 1: GICD_CTLR = 0x12（EnableGrp1 + ARE_NS）
  │
  ├─ Step 2: GICR_WAKER.ProcessorSleep = 0
  │          轮询 GICR_WAKER.ChildrenAsleep == 0
  │
  ├─ Step 3: ICC_SRE_EL1 = 1（系统寄存器模式）
  │          ICC_PMR_EL1 = 0xFF（全优先级放行）
  │          ICC_IGRPEN1_EL1 = 1（Group 1 使能）
  │
  ├─ Step 4: GICD_IPRIORITYRn 默认优先级（0xA0）
  │
  └─ 初始化完成，可注册并使能中断
```

---

## 5. 中断分发流程

当 CPU 接收到中断异常后，EnerOS 的中断分发流程在 `Arm64Gic::dispatch_irq()` 中实现。

### 5.1 分发步骤

```rust
pub fn dispatch_irq(&self) {
    // 1. 读取中断确认寄存器，获取 IRQ ID
    let irq_id: u64;
    unsafe { asm!("mrs {0}, icc_iar1_el1", out(reg) irq_id); }
    let irq = irq_id as u32;

    // 2. 检查伪中断（1023 = 无中断）
    if irq == 1023 {
        return; // spurious interrupt，忽略
    }

    // 3. 查 handler 表
    // SAFETY: 单核 boot 阶段或中断上下文，无并发
    let handler = unsafe { IRQ_HANDLERS[irq as usize] };

    match handler {
        Some(h) => {
            // 4a. 调用注册的 handler
            let action = h(irq);
            if action == IrqAction::Disabled {
                self.disable(irq);
            }
        }
        None => {
            // 4b. 未知中断，打印告警
            // TODO: 通过 serial 打印 "unhandled IRQ {irq}"
        }
    }

    // 5. 结束中断（EOI），无论是否处理都必须执行
    unsafe { asm!("msr icc_eoir1_el1, {0}", in(reg) irq_id); }
}
```

### 5.2 流程图

```
CPU 收到中断异常
  │
  ▼
读 ICC_IAR1_EL1 ──→ IRQ ID
  │
  ├─ IRQ ID == 1023? ──→ 是: 伪中断，直接返回（无 EOI）
  │
  ├─ 否: 查 IRQ_HANDLERS[irq]
  │     │
  │     ├─ Some(handler) ──→ 调用 handler(irq)
  │     │                      │
  │     │                      ├─ IrqAction::Handled   → 继续
  │     │                      ├─ IrqAction::WakeThread → 唤醒线程
  │     │                      └─ IrqAction::Disabled   → disable(irq)
  │     │
  │     └─ None ──→ 打印告警（未注册的中断）
  │
  ▼
写 ICC_EOIR1_EL1 = IRQ ID（结束中断，允许再次触发）
  │
  ▼
中断返回
```

### 5.3 伪中断（IRQ 1023）处理

读 `ICC_IAR1_EL1` 返回 1023 表示当前无待处理中断，可能原因：

| 原因 | 说明 |
|------|------|
| 中断已被其他核取走 | 多核场景下 Distributor 可能同时通知多核，先取走者得 |
| 中断被禁用 | 在 IAR 读取前中断被 `disable()` 禁用 |
| 优先级不足 | 中断优先级低于 `ICC_PMR_EL1` 屏蔽值 |
| 硬件毛刺 | 总线干扰导致的虚假触发 |

> **处理策略**：IRQ 1023 直接返回，**不执行 EOI**（GICv3 规范要求伪中断不配对 EOI）。

### 5.4 EOI 顺序约束

| 规则 | 说明 |
|------|------|
| IAR 与 EOIR 必须配对 | 每次读 IAR 获取的 IRQ ID，必须用同一 IRQ ID 写 EOIR |
| EOI 必须在中断返回前 | 若 EOI 延迟到下一次中断，会导致该中断无法再次触发 |
| EOI 后中断可再次 pending | EOI 将 active 状态清除，允许同一中断再次进入 pending |
| 不允许嵌套 EOI | IAR/EOIR 是栈式配对，不可交叉 |

---

## 6. EnerOS 实现说明

### 6.1 Arm64Gic 结构体设计

```rust
/// GICv3 中断控制器驱动。
///
/// 管理 GICD（Distributor）与 GICR（Redistributor）的 MMIO 访问，
/// CPU interface 通过 ICC_*_EL1 系统寄存器直接访问（不在此结构体中存储）。
pub struct Arm64Gic {
    /// GICD 基址（Distributor，全局共享）
    gicd_base: usize,
    /// GICR 基址（Redistributor，当前核的实例）
    gicr_base: usize,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `gicd_base` | `usize` | GICD MMIO 基址（QEMU virt: `0x0800_0000`） |
| `gicr_base` | `usize` | 当前核的 GICR MMIO 基址（QEMU virt CPU0: `0x080A_0000`） |

> CPU interface 的 `ICC_*_EL1` 系统寄存器不需要存储基址，通过 `msr`/`mrs` 指令直接访问。

### 6.2 静态 handler 表

由于 `HalIrq::register(&self, ...)` 签名不允许 `&mut self`（trait object 限制），handler 表必须使用全局静态存储：

```rust
/// 最大支持的中断号（覆盖 SGI 0–15 + PPI 16–31 + SPI 32–255）。
const MAX_IRQ: usize = 256;

/// 全局中断 handler 表。
///
/// # Safety
/// `static mut` 访问仅出现在 `register()`（写入）与 `dispatch_irq()`（读取）两处。
/// - `register()` 在 boot 早期单线程上下文调用，无并发
/// - `dispatch_irq()` 在中断上下文调用，此时中断已关闭（DAIF 屏蔽），无重入
static mut IRQ_HANDLERS: [Option<IrqHandler>; MAX_IRQ] = [None; MAX_IRQ];
```

**设计决策 D3**（v0.6.0 spec）：使用 `static mut` 而非 `spin::Mutex`，理由：
- v0.6.0 的 eneros-hal crate 无外部依赖（蓝图要求仅依赖 `core`）
- OS 内核场景 `static mut` + `unsafe` 是标准模式
- handler 表是全局唯一的，不需要 per-instance 存储

### 6.3 MMIO 辅助函数

```rust
/// 读 32-bit MMIO 寄存器。
///
/// # Safety
/// 调用方需确保 `base + off` 是有效的 GIC 寄存器地址且已映射。
unsafe fn r32(base: usize, off: u32) -> u32 {
    core::ptr::read_volatile((base + off as usize) as *const u32)
}

/// 写 32-bit MMIO 寄存器。
///
/// # Safety
/// 同 `r32`。
unsafe fn w32(base: usize, off: u32, val: u32) {
    core::ptr::write_volatile((base + off as usize) as *mut u32, val);
}
```

> 使用 `core::ptr::read_volatile`/`write_volatile` 确保编译器不优化掉 MMIO 访问。不使用 `core::ptr::read`/`write`（可能被优化为寄存器访问或合并）。

### 6.4 寄存器偏移常量

```rust
// GICD 寄存器偏移
const GICD_CTLR: u32 = 0x000;
const GICD_TYPER: u32 = 0x004;
const GICD_ISENABLER: u32 = 0x100;
const GICD_ICENABLER: u32 = 0x180;
const GICD_ICPENDR: u32 = 0x280;
const GICD_IPRIORITYR: u32 = 0x400;
const GICD_ICFGR: u32 = 0xC00;
const GICD_IROUTER: u32 = 0x6000;

// GICR RD_base 寄存器偏移
const GICR_CTLR: u32 = 0x000;
const GICR_TYPER: u32 = 0x008;
const GICR_WAKER: u32 = 0x014;

// GICR SGI_base 偏移（相对 RD_base）
const GICR_SGI_BASE: u32 = 0x10000;
const GICR_IGROUPR0: u32 = 0x080;
const GICR_ISENABLER0: u32 = 0x100;
const GICR_ICENABLER0: u32 = 0x180;
const GICR_IPRIORITYR0: u32 = 0x400;

// GICR_WAKER 位常量
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 0x1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 0x2;

// GICD_CTLR 使能值
const GICD_CTLR_ENABLE_GRP1: u32 = 0x2;
const GICD_CTLR_ARE_NS: u32 = 0x10;
```

### 6.5 GICv3 基址配置

```rust
// QEMU virt 默认 GICv3 基址（来自 board/qemu-virt/dts）
const QEMU_VIRT_GICD_BASE: usize = 0x0800_0000;
const QEMU_VIRT_GICR_BASE: usize = 0x080A_0000;

// 全局单例
static ARM64_GIC: Arm64Gic = Arm64Gic {
    gicd_base: QEMU_VIRT_GICD_BASE,
    gicr_base: QEMU_VIRT_GICR_BASE,
};

pub fn irq() -> &'static dyn HalIrq {
    &ARM64_GIC
}
```

### 6.6 enable/disable 实现

```rust
impl HalIrq for Arm64Gic {
    fn enable(&self, irq: u32) {
        // SPI(32+) 由 GICD 管理，SGI/PPI(0-31) 由 GICR 管理
        let reg_base = if irq < 32 {
            self.gicr_base + GICR_SGI_BASE as usize
        } else {
            self.gicd_base
        };
        let bank = (irq / 32) as u32;
        let bit = 1u32 << (irq % 32);
        // SGI/PPI 用 ISENABLER0（bank 恒为 0）
        let offset = if irq < 32 {
            GICR_ISENABLER0
        } else {
            GICD_ISENABLER + bank * 4
        };
        unsafe { w32(reg_base, offset, bit); }
    }

    fn disable(&self, irq: u32) {
        // 逻辑同 enable，改写 ICENABLER
        // ...
    }

    fn eoi(&self, irq: u32) {
        // GICv3 系统寄存器模式：写 ICC_EOIR1_EL1
        unsafe {
            asm!("msr icc_eoir1_el1, {0}", in(reg) irq as u64);
        }
    }
}
```

---

## 7. 多核支持（TODO）

v0.6.0 简化为单核（仅初始化 CPU0 的 GICR）。多核支持计划在后续版本（v0.16.0 多核调度）补全。

### 7.1 GICR 遍历定位

多核场景下，需遍历 GICR 实例定位每个核的 Redistributor 基址：

```rust
/// 定位指定亲和性值对应的 GICR 基址。
///
/// GICR 实例在 MMIO 空间连续排列，每个实例占 2 × 64KB（RD_base + SGI_base）。
/// 通过读取 GICR_TYPER 的 Affinity 字段匹配目标核。
fn locate_gicr(gicr_base: usize, target_aff: u64) -> Option<usize> {
    let mut offset = 0;
    loop {
        let typer = unsafe { r64(gicr_base + offset, GICR_TYPER) };
        let aff = (typer >> 32) & 0xFF_FFFF; // Aff3:Aff2:Aff1:Aff0
        if aff == target_aff {
            return Some(gicr_base + offset);
        }
        // 检查 Last 位（bit 12）
        if (typer >> 12) & 1 == 1 {
            return None; // 遍历到末尾未找到
        }
        offset += 0x20000; // 下一个 GICR 实例
    }
}
```

### 7.2 亲和性路由

通过 `MPIDR_EL1` 读取当前核的亲和性值，用于定位 GICR：

```rust
/// 读取当前核的亲和性值（Aff3:Aff2:Aff1:Aff0）。
fn current_affinity() -> u64 {
    let mpidr: u64;
    unsafe { asm!("mrs {0}, mpidr_el1", out(reg) mpidr); }
    // MPIDR_EL1 格式：
    //   bits [7:0]   = Aff0 (CPU ID within cluster)
    //   bits [15:8]  = Aff1 (cluster ID)
    //   bits [23:16] = Aff2
    //   bits [39:32] = Aff3
    let aff0 = mpidr & 0xFF;
    let aff1 = (mpidr >> 8) & 0xFF;
    let aff2 = (mpidr >> 16) & 0xFF;
    let aff3 = (mpidr >> 32) & 0xFF;
    (aff3 << 24) | (aff2 << 16) | (aff1 << 8) | aff0
}
```

### 7.3 SPI 路由配置

多核场景下，SPI 需通过 `GICD_IROUTERn` 配置目标核：

```rust
/// 设置 SPI 的目标核（亲和性路由）。
fn route_spi(irq: u32, target_aff: u64) {
    let offset = GICD_IROUTER + irq as u32 * 8;
    unsafe { w64(self.gicd_base, offset, target_aff); }
}
```

### 7.4 多核 TODO 清单

| 项目 | 说明 | 目标版本 |
|------|------|----------|
| GICR 遍历 | 遍历 GICR_TYPER 定位每核 Redistributor | v0.16.0 |
| 亲和性路由 | MPIDR_EL1 读取 + GICD_IROUTER 配置 | v0.16.0 |
| 核间中断（SGI） | `GICR_SGIR` 或 `ICC_SGI1R_EL1` 发送 IPI | v0.16.0 |
| per-core handler 表 | 多核独立 handler 表或共享表加锁 | v0.16.0 |

---

## 8. 参考

### 8.1 规范文档

| 文档 | 编号 | 说明 |
|------|------|------|
| ARM GICv3 Architecture Specification | ARM IHI 0069 | GICv3/GICv4 架构规范（权威参考） |
| ARM Architecture Reference Manual (ARMv8) | ARM DDI 0487 | ARMv8 架构手册，含 ICC_*_EL1 系统寄存器定义 |
| ARM Generic Interrupt Controller v3 FAQ | — | GICv3 常见问题与设计说明 |

### 8.2 QEMU 参考

| 资源 | 说明 |
|------|------|
| QEMU virt machine 文档 | 默认 GICv3 基址配置（`-machine virt,gic-version=3`） |
| `board/qemu-virt/dts` | EnerOS QEMU virt 设备树源文件（GIC 节点定义基址） |

### 8.3 EnerOS 内部参考

| 文档 | 说明 |
|------|------|
| `docs/hal-interface-spec.md` | HAL trait 接口规范（v0.5.0，`HalIrq` trait 定义） |
| `docs/hal-design-whitepaper.md` | HAL 设计白皮书（trait 抽象与 HalProvider 模式） |
| `.trae/specs/develop-v060-hal-arm64-core/spec.md` | v0.6.0 实现规格（GICv3 设计决策 D2/D3） |
| `hal/src/arm64/gicv3.rs` | GICv3 驱动源码（`Arm64Gic` 实现） |

---

> 本文档是 EnerOS GICv3 中断控制器驱动的权威参考。寄存器定义变更需同步更新本文档并升级版本号。
