# 地址空间布局

> 版本：v0.8.0
> 适用范围：EnerOS `mm` crate 虚拟地址空间抽象
> 蓝图依据：`蓝图/phase1.md` §v0.8.0
> crate：eneros-mm（`mm/src/vspace.rs`、`mm/src/vregion.rs`）
> 关联文档：`docs/arm64-page-table-design.md`

---

## 1. 概述

EnerOS `mm` crate 提供两层抽象来管理虚拟地址空间：

- **`Vspace`**：一个完整的虚拟地址空间，对应一张 L0 根页表与一个 ASID，代表一个"进程"或"隔离分区"的地址空间。
- **`Vregion`**：地址空间内一段连续的虚拟内存区域描述符，记录起止地址、权限与物理后援类型。

在此之上定义了 **`AddressSpace`** trait，抽象出 `map` / `unmap` / `translate` / `set_flags` 四个核心操作，使上层（调度器、用户态加载器、分区管理器）不依赖具体硬件页表实现。

```
┌──────────────────────────────────────────────┐
│              AddressSpace trait              │
│   map()  unmap()  translate()  set_flags()   │
└─────────────────────┬────────────────────────┘
                      │ impl
┌─────────────────────▼────────────────────────┐
│                  Vspace                       │
│  root_paddr  asid  regions[16]                │
│  └─ L0 → L1 → L2 → L3 四级页表                │
└─────────────────────┬────────────────────────┘
                      │ 持有
┌─────────────────────▼────────────────────────┐
│                Vregion ×N                     │
│  start_va  size  flags  backing               │
└──────────────────────────────────────────────┘
```

---

## 2. Vspace 结构

`Vspace` 表示一个虚拟地址空间，定义于 `mm/src/vspace.rs`：

```rust
pub struct Vspace {
    /// Physical address of the L0 (root) page table.
    pub root_paddr: u64,
    /// Address Space ID for TLB management.
    pub asid: u16,
    /// Tracked memory regions.
    pub regions: [Option<Vregion>; MAX_REGIONS],
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `root_paddr` | `u64` | L0 根页表的物理地址。MMU 遍历页表的起点，写入 `TTBR0_EL1` |
| `asid` | `u16` | 地址空间 ID（16 位）。TLB 刷新时使用 `tlbi asid` 精确刷新 |
| `regions` | `[Option<Vregion>; 16]` | 跟踪的内存区域数组，最多 16 个（`MAX_REGIONS = 16`） |

**构造**：

```rust
impl Vspace {
    pub const fn new(root_paddr: u64, asid: u16) -> Self {
        Self {
            root_paddr,
            asid,
            regions: [None; MAX_REGIONS],
        }
    }
}
```

`new` 是 `const fn`，可在编译期构造静态 `Vspace`（如内核主地址空间）。

---

## 3. Vregion 结构

`Vregion` 描述地址空间内一段连续虚拟内存，定义于 `mm/src/vregion.rs`：

```rust
#[derive(Clone, Copy)]
pub struct Vregion {
    pub start_va: u64,      // 起始虚拟地址
    pub size: u64,          // 字节数
    pub flags: MemFlags,    // 内存保护标志
    pub backing: Backing,   // 物理后援类型
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `start_va` | `u64` | 区域起始虚拟地址（应页对齐） |
| `size` | `u64` | 区域大小（字节，应页对齐） |
| `flags` | `MemFlags` | 内存保护标志（读/写/执行/设备/缓存） |
| `backing` | `Backing` | 物理后援类型，决定如何分配物理内存 |

**辅助方法**：

```rust
impl Vregion {
    /// 结束虚拟地址（exclusive，不含）
    pub const fn end_va(&self) -> u64 { self.start_va + self.size }

    /// 判断 va 是否落在本区域内
    pub const fn contains(&self, va: u64) -> bool {
        va >= self.start_va && va < self.end_va()
    }
}
```

---

## 4. Backing 类型

`Backing` 枚举描述虚拟区域与物理内存的对应关系，定义于 `mm/src/vregion.rs`：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backing {
    /// Identity mapping (va == pa).
    Identity,
    /// Mapping to a specific physical address.
    Phys(u64),
    /// Demand paging (allocated on fault).
    Demand,
}
```

| 变体 | 含义 | 典型用途 |
|------|------|----------|
| `Identity` | 等同映射（VA == PA） | 内核早期启动、MMU 尚未完全启用时的线性映射；设备树查看 |
| `Phys(u64)` | 映射到指定物理地址 | MMIO 设备寄存器映射（UART、GIC、GPIO 等） |
| `Demand` | 按需分配（访问时触发 fault 再分配） | 用户进程的匿名内存（堆、栈）；v0.8.0 占位，依赖 v0.10.0 堆分配 |

**各 Backing 的使用场景**：

- **Identity**：内核自身代码/数据在启动早期常用等同映射，避免 VA→PA 转换的复杂性
- **Phys(pa)**：设备 MMIO 区域必须映射到设备真实的物理地址，例如 PL011 UART 固定在 `0x0900_0000`
- **Demand**：用户进程的堆/栈，物理页在首次访问时由 page fault handler 分配（v0.8.0 仅占位，完整实现见后续版本）

---

## 5. AddressSpace trait

`AddressSpace` trait 定义于 `mm/src/vspace.rs`，抽象虚拟内存管理的四个核心操作：

```rust
pub trait AddressSpace {
    /// Map `size` bytes from `pa` to `va` with `flags`.
    fn map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError>;
    /// Unmap `size` bytes starting at `va`.
    fn unmap(&mut self, va: u64, size: u64) -> Result<(), MmError>;
    /// Translate `va` to its physical address, or None if not mapped.
    fn translate(&self, va: u64) -> Option<u64>;
    /// Update protection flags for the page at `va`.
    fn set_flags(&mut self, va: u64, flags: MemFlags) -> Result<(), MmError>;
}
```

| 方法 | 签名要点 | 语义 |
|------|----------|------|
| `map` | `&mut self` | 将 `[pa, pa+size)` 映射到 `[va, va+size)`，按 `flags` 设置权限。要求 `va`、`pa` 页对齐；若已映射返回 `AlreadyMapped` |
| `unmap` | `&mut self` | 解除 `[va, va+size)` 的映射，将 L3 表项清零。未映射返回 `NotMapped` |
| `translate` | `&self` | 查询 `va` 对应的物理地址（只读，不修改页表）。未映射返回 `None` |
| `set_flags` | `&mut self` | 修改 `va` 所在页的保护标志（保留原 PA）。未映射返回 `NotMapped` |

> **`&self` vs `&mut self`**：`translate` 是只读查询用 `&self`；`map`/`unmap`/`set_flags` 都会修改页表，用 `&mut self`。这一签名设计正是 HAL `HalMem` trait（`&self`）与 `AddressSpace`（`&mut self`）不兼容、v0.8.0 暂缓适配的原因（见 `hal/src/arm64/provider.rs` 注释）。

---

## 6. MmError 错误类型

`MmError` 定义于 `mm/src/vspace.rs`，涵盖所有地址空间操作的错误：

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum MmError {
    InvalidAddr,    // 无效地址
    NotMapped,      // 虚拟地址未映射
    AlreadyMapped,  // 虚拟地址已映射
    OutOfMemory,    // 页表页池耗尽
    Misaligned,     // 地址未页对齐
}
```

| 变体 | 触发场景 | 触发方法 |
|------|----------|----------|
| `InvalidAddr` | 地址超出合法范围（如 ≥48 位空间） | map / unmap / translate |
| `NotMapped` | unmap/set_flags 时该 VA 未映射 | unmap / set_flags |
| `AlreadyMapped` | map 时该 VA 已存在有效映射 | map |
| `OutOfMemory` | 静态页表页池（64 张）耗尽，无法分配中间页表 | map |
| `Misaligned` | `va` 或 `pa` 低 12 位非 0（未 4KB 对齐） | map |

`MmError` 实现了 `Display`，便于错误上报：

```rust
impl fmt::Display for MmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MmError::InvalidAddr   => write!(f, "invalid address"),
            MmError::NotMapped     => write!(f, "not mapped"),
            MmError::AlreadyMapped => write!(f, "already mapped"),
            MmError::OutOfMemory   => write!(f, "out of memory (page table pool exhausted)"),
            MmError::Misaligned    => write!(f, "address not page-aligned"),
        }
    }
}
```

---

## 7. ASID 分配策略

每个 `Vspace` 拥有独立的 ASID（Address Space ID），用于 TLB 标签与精确刷新。

### 7.1 ASID 的作用

- **TLB 标签**：MMU 在填充 TLB 时为每一项打上当前 ASID 标签。切换地址空间（改写 `TTBR0_EL1`）时，其他 ASID 的 TLB 项仍然有效，避免全刷。
- **精确刷新**：修改页表后只需 `tlbi asid, <Xt>` 刷新本空间，不影响其他空间，降低性能开销。

### 7.2 分配策略

- ASID 为 16 位（`u16`），取值范围 0–65535
- v0.8.0 中 ASID 由调用方在 `Vspace::new(root_paddr, asid)` 时显式指定（如内核主空间用 ASID 0，首个用户空间用 ASID 1）
- 后续版本将提供 ASID 分配器（位图管理），自动回收已销毁 Vspace 的 ASID

### 7.3 TLB 刷新实现

```rust
#[cfg(target_arch = "aarch64")]
fn flush_tlb(&self) {
    unsafe {
        core::arch::asm!(
            "tlbi asid, {0}",
            in(reg) (self.asid as u64) << 48,
        );
    }
}
```

操作数高 16 位填 ASID，`tlbi asid` 仅刷新该 ASID 对应的所有 TLB 项。`map`/`unmap`/`set_flags` 修改页表后均调用此方法。

---

## 8. 映射流程

`Vspace::map(va, pa, size, flags)` 的完整执行步骤：

```
1. 对齐校验
   ├── va & 0xFFF != 0  →  Err(Misaligned)
   └── pa & 0xFFF != 0  →  Err(Misaligned)

2. 按 4KB 步长遍历 [va, va+size)
   对每一页 cur_va = va + off：
   │
   ├── 2a. walk_or_alloc(cur_va)  // 遍历 L0→L3，按需分配中间表
   │    ├── L0: 读 PTE
   │    │    ├── VALID=0 → alloc_page_table() → make_table() → 写入 → 进入新表
   │    │    └── VALID=1 → 取 PA → 进入下一级
   │    ├── L1: 同上
   │    ├── L2: 同上
   │    └── L3: 返回 (table_addr, idx, existing_pte)
   │         └── alloc 失败 → Err(OutOfMemory)
   │
   ├── 2b. 检测 AlreadyMapped
   │    └── existing & PTE_VALID != 0  →  Err(AlreadyMapped)
   │
   └── 2c. 写 L3 叶子
        └── make_leaf(cur_pa, flags) → write_pte(table_addr, idx, leaf)

3. flush_tlb()  // tlbi asid，刷新本地址空间
4. return Ok(())
```

**关键实现**（`mm/src/vspace.rs`）：

```rust
fn map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError> {
    if va & 0xFFF != 0 || pa & 0xFFF != 0 {
        return Err(MmError::Misaligned);
    }
    let mut off = 0u64;
    while off < size {
        let cur_va = va + off;
        let cur_pa = pa + off;
        unsafe {
            let (table_addr, idx, existing) = self.walk_or_alloc(cur_va)?;
            if existing & PTE_VALID != 0 {
                return Err(MmError::AlreadyMapped);
            }
            let leaf = PageTable::make_leaf(cur_pa, flags);
            Self::write_pte(table_addr, idx, leaf);
        }
        off += PAGE_SIZE;
    }
    self.flush_tlb();
    Ok(())
}
```

**unmap / translate / set_flags 流程**类似，区别在于：
- `unmap`：`walk_to_l3`（不分配），找到后写 0
- `translate`：`walk_to_l3`（只读 `&self`），返回 `pte & PTE_ADDR_MASK`
- `set_flags`：`walk_or_alloc`，保留 PA，用新 flags 重写叶子

---

## 9. QEMU virt 地址空间布局

EnerOS 在 QEMU virt 平台（`-machine virt`）上运行，其物理地址空间布局如下：

| 起始地址 | 结束地址 | 大小 | 说明 |
|----------|----------|------|------|
| `0x0000_0000` | `0x3FFF_FFFF` | 1 GB | RAM（DDR），内核镜像与用户进程驻留 |
| `0x0800_0000` | `0x0800_0FFF` | 4 KB | GICv3 Distributor（GICD） |
| `0x080A_0000` | `0x080A_0FFF` | 4 KB | GICv3 Redistributor（GICR，per-core） |
| `0x0900_0000` | `0x0900_0FFF` | 4 KB | PL011 UART（串口） |
| `0x0901_0000` | `0x0901_0FFF` | 4 KB | 网口 MAC（virtio-net MMIO） |
| `0x0902_0000` | `0x0902_0FFF` | 4 KB | GPIO（PL061） |

### 9.1 地址空间划分说明

- **RAM（0x0000_0000 ~ 0x3FFF_FFFF）**：QEMU virt 默认 1GB DRAM，内核镜像加载在 `0x4000_0000` 附近（由 QEMU `-kernel` 参数决定）。EnerOS 内核运行于此区域。
- **GICv3（0x0800_0000 / 0x080A_0000）**：中断控制器。GICD 全局唯一，GICR 每核一个实例（连续排列，每核占 2×64KB）。
- **PL011 UART（0x0900_0000）**：串口，EnerOS 早期调试输出通道（`hal/src/arm64/uart_pl011.rs`）。
- **virtio-net MMIO（0x0901_0000）**：QEMU 虚拟网卡，后续网络栈（v0.28.0）使用。
- **GPIO PL061（0x0902_0000）**：QEMU 虚拟 GPIO 控制器（`hal/src/arm64/gpio.rs`）。

### 9.2 典型映射方案

| 区域 | 虚拟地址 | 物理地址 | Backing | MemFlags | 说明 |
|------|----------|----------|---------|----------|------|
| 内核代码 | `0xFFFF_0000_4000_0000` | `0x4000_0000` | `Phys` | `code()` | 高地址区，只读可执行 |
| 内核数据 | `0xFFFF_0000_4020_0000` | `0x4020_0000` | `Phys` | `normal()` | 读写，不可执行 |
| UART | `0xFFFF_0000_0900_0000` | `0x0900_0000` | `Phys(0x0900_0000)` | `device()` | MMIO，Device-nGnRE |
| GICD | `0xFFFF_0000_0800_0000` | `0x0800_0000` | `Phys(0x0800_0000)` | `device()` | MMIO |
| GICR | `0xFFFF_0000_080A_0000` | `0x080A_0000` | `Phys(0x080A_0000)` | `device()` | MMIO |

> v0.8.0 阶段地址空间布局仍在演进，上表为典型方案，实际映射由内核启动代码配置。

---

## 10. 使用示例

以下 Rust 代码展示如何创建 `Vspace`、映射地址、翻译地址：

```rust
use eneros_hal::MemFlags;
use eneros_mm::vspace::{AddressSpace, Vspace, MmError};
use eneros_mm::page_table::{PageTable, PAGE_SIZE};

// 假设已有一张 4KB 对齐的 L0 根页表（物理地址 root_pa）
// 实际系统中由启动代码分配
extern "C" {
    static ROOT_PT: PageTable;  // 4KB 对齐的静态根页表
}

fn example() -> Result<(), MmError> {
    let root_pa = unsafe { &ROOT_PT as *const PageTable as u64 };

    // 1. 创建地址空间，ASID = 1
    let mut vspace = Vspace::new(root_pa, 1);

    // 2. 映射：VA 0x1000 → PA 0x9000，1 页，普通可读写内存
    vspace.map(0x1000, 0x9000, PAGE_SIZE, MemFlags::normal())?;

    // 3. 翻译：VA 0x1000 应返回 PA 0x9000
    let pa = vspace.translate(0x1000);
    assert_eq!(pa, Some(0x9000));

    // 4. 修改权限：改为只读可执行代码页
    vspace.set_flags(0x1000, MemFlags::code())?;

    // 5. 映射设备 MMIO：VA 0x0900_0000 → PA 0x0900_0000（UART）
    vspace.map(0x0900_0000, 0x0900_0000, PAGE_SIZE, MemFlags::device())?;

    // 6. 解除映射
    vspace.unmap(0x1000, PAGE_SIZE)?;
    assert_eq!(vspace.translate(0x1000), None);

    Ok(())
}
```

### 10.1 配合 Vregion 使用

```rust
use eneros_mm::vregion::{Vregion, Backing};
use eneros_hal::MemFlags;

// 描述内核代码段区域：等同映射，只读可执行
let code_region = Vregion::new(
    0x4000_0000,
    0x0020_0000,            // 2MB
    MemFlags::code(),
    Backing::Identity,
);

// 描述 UART 设备区域：指定物理地址
let uart_region = Vregion::new(
    0x0900_0000,
    PAGE_SIZE,
    MemFlags::device(),
    Backing::Phys(0x0900_0000),
);

// 描述用户堆：按需分配
let heap_region = Vregion::new(
    0x0000_1000_0000,
    0x0001_0000,            // 64KB
    MemFlags::normal(),
    Backing::Demand,
);

assert!(code_region.contains(0x4000_0000));
assert!(!code_region.contains(0x4020_0000));  // end_va exclusive
assert_eq!(uart_region.end_va(), 0x0900_1000);
```

> 注：`Vregion` 是描述性结构，v0.8.0 中不直接驱动映射。后续版本将提供 `Vspace::map_region(&Vregion)` 等便捷方法，根据 `backing` 自动计算 PA 并调用 `map`。

---

## 参考

- `mm/src/vspace.rs` — Vspace 结构与 AddressSpace trait 实现
- `mm/src/vregion.rs` — Vregion 与 Backing 定义
- `mm/src/page_table.rs` — 页表与 PTE 构造
- `hal/src/types.rs` — MemFlags 定义
- `docs/arm64-page-table-design.md` — 页表设计细节
- `蓝图/phase1.md` §v0.8.0 — 版本蓝图
