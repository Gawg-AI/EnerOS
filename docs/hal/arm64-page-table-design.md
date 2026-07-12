# ARM64 页表设计

> 版本：v0.8.0
> 适用范围：EnerOS `mm` crate ARM64 四级页表实现
> 蓝图依据：`蓝图/phase1.md` §v0.8.0
> crate：eneros-mm（`mm/src/page_table.rs`）
> 硬件参考：ARMv8 Architecture Reference Manual（ARM DDI 0487）D5 章节

---

## 1. 概述

ARMv8-A 架构采用基于页表的虚拟内存系统，将虚拟地址（VA）翻译为物理地址（PA）。EnerOS `mm` crate 实现的是最常见、也是 QEMU virt 平台默认的配置：

- **四级页表**（4-level walk）：L0 → L1 → L2 → L3
- **48 位虚拟地址**（VA[47:0]），高 16 位（VA[63:48]）由 TCR_EL1 的 T0SZ/T1SZ 决定是否符号扩展或全 0
- **4KB granule**（页大小 4KB），TCR_EL1.TG0 = 0b00
- **每张页表 512 项**（4KB / 8 字节每项 = 512）
- **物理地址 48 位**（PA[47:0]）

一次完整的页表遍历（page table walk）依次读取 L0、L1、L2、L3 四张表，最终在 L3 得到一个指向 4KB 物理页的叶子表项（leaf PTE）。L0–L2 的表项为中间表项（table entry），指向下一级页表。

> **ARMv8 ARM 对应**：参见 ARMv8 ARM D5.2 "Virtual memory system architecture (VMSA)" 与 D5.3 "VMSAv8-64 translation table format"。

---

## 2. 地址分解

48 位虚拟地址按 9+9+9+9+12 分解为五段：四级索引各 9 位（512 项），加 12 位页内偏移（4KB）。

```
VA[47:0]
├── VA[47:39]  L0 索引（9 位，0–511）
├── VA[38:30]  L1 索引（9 位，0–511）
├── VA[29:21]  L2 索引（9 位，0–511）
├── VA[20:12]  L3 索引（9 位，0–511）
└── VA[11:0]   页内偏移（12 位，0–4095）
```

| 字段 | 位域 | 宽度 | 说明 |
|------|------|------|------|
| L0 索引 | VA[47:39] | 9 位 | 在 L0（PGD）表中的下标 |
| L1 索引 | VA[38:30] | 9 位 | 在 L1（PUD）表中的下标 |
| L2 索引 | VA[29:21] | 9 位 | 在 L2（PMD）表中的下标 |
| L3 索引 | VA[20:12] | 9 位 | 在 L3（PTE）表中的下标 |
| 页内偏移 | VA[11:0] | 12 位 | 4KB 页内的字节偏移 |

每级索引 9 位，恰好寻址 512 项；页内偏移 12 位，恰好寻址 4KB。这种规整的 9 位划分正是 4KB granule + 48 位 VA 下四级页表的必然结果。

---

## 3. 页表级别

ARMv8-A 四级页表自上而下依次为 L0、L1、L2、L3，每级对应 Linux 术语中的 PGD/PUD/PMD/PTE：

| 级别 | 别名 | 表项类型 | 单项覆盖范围 | 整表覆盖范围 |
|------|------|----------|--------------|--------------|
| L0 | PGD | 中间表项（table） | 512 GB | 256 TB（48 位 VA 全空间） |
| L1 | PUD | 中间表项（table） | 1 GB | 512 GB |
| L2 | PMD | 中间表项（table）* | 2 MB | 1 GB |
| L3 | PTE | 叶子表项（leaf） | 4 KB | 2 MB |

> *L2 表项也可以是叶子表项（block descriptor），映射 2MB 大页（block）。EnerOS v0.8.0 暂不使用大页，所有 L2 表项均为中间表项，最终映射粒度为 4KB。

**覆盖范围推导**：
- L3 叶子映射 1 个 4KB 页 → 4KB
- L2 表有 512 项，每项指向 1 张 L3 表（512 × 4KB）→ 2MB
- L1 表有 512 项，每项指向 1 张 L2 表（512 × 2MB）→ 1GB
- L0 表有 512 项，每项指向 1 张 L1 表（512 × 1GB）→ 512GB
- 故 48 位 VA 全空间 = 512 × 512GB = 256TB

---

## 4. PTE 位域

每个页表项（PTE）是 64 位。EnerOS `mm` crate 中定义的位域如下（`mm/src/page_table.rs`）：

| 位 | 名称 | 常量 | 说明 |
|------|------|------|------|
| bit 0 | VALID | `PTE_VALID` | 有效位。0 表示该表项无效，访问将触发 Translation fault |
| bit 1 | TABLE | `PTE_TABLE` | 表类型。1 = 中间表项（指向下一级页表）；0 = 叶子表项（L3）或 block descriptor（L0–L2） |
| bits[3:2] | AttrIndex | `MT_NORMAL` / `MT_DEVICE` | MAIR_EL1 属性索引。0 = Normal（MT_NORMAL），1 = Device-nGnRE（MT_DEVICE） |
| bits[9:8] | SH | `PTE_SH_INNER` | Shareability。00 = Non-shareable，11 = Inner Shareable |
| bit 10 | AF | `PTE_AF` | Access Flag。必须置 1，否则首次访问触发 Access flag fault |
| bits[47:12] | PA | `PTE_ADDR_MASK` | 物理地址（页对齐，36 位 PA[47:12]） |
| bit 53 | PXN | `PTE_PXN` | Privileged Execute Never。EL1/EL3 不可执行 |
| bit 54 | XN | `PTE_XN` | Execute Never（所有异常等级不可执行） |

**位域示意图**：

```
63          55  54  53        48 47                              12 11  10  9   8   7    4  3   2  1   0
+-------------+----+----+-------+----------------------------------+-----+-------+------+------+-----+-----+
|  reserved   | XN | PXN| res   |     PA[47:12] (页对齐物理地址)    | res |  AF   |  SH  | res |Attr | V T |
+-------------+----+----+-------+----------------------------------+-----+-------+------+------+-----+-----+
                                                                                 | Idx |
                                                                                  3:2
```

**关键常量定义**（`mm/src/page_table.rs`）：

```rust
pub const PTE_VALID: u64    = 1 << 0;   // bit 0
pub const PTE_TABLE: u64    = 1 << 1;   // bit 1
pub const PTE_AF: u64       = 1 << 10;  // bit 10
pub const PTE_SH_INNER: u64 = 3 << 8;   // bits[9:8] = 0b11
pub const PTE_PXN: u64      = 1 << 53;  // bit 53
pub const PTE_XN: u64       = 1 << 54;  // bit 54
pub const MT_NORMAL: u64    = 0 << 2;   // AttrIndex = 0
pub const MT_DEVICE: u64    = 1 << 2;   // AttrIndex = 1
pub const PTE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000; // bits[47:12]
```

> **MAIR_EL1 配置**：AttrIndex 字段索引 MAIR_EL1 寄存器的 8 个 8 位属性域。EnerOS 约定 MAIR_EL1[0] = Normal（0xFF，Write-back Cacheable），MAIR_EL1[1] = Device-nGnRE（0x00）。

---

## 5. 索引计算

`PageTable::index(level, va)` 从虚拟地址 `va` 中提取指定级别的 9 位索引：

```rust
pub fn index(level: PageLevel, va: u64) -> usize {
    let shift = 39 - (level.as_u8() as u64) * 9;
    ((va >> shift) & 0x1FF) as usize
}
```

**公式**：

```
index(level, va) = (va >> (39 - level * 9)) & 0x1FF
```

其中 `level` ∈ {0, 1, 2, 3}，`0x1FF` = 9 位掩码。

**各级移位量**：

| 级别 | level | 移位量（39 - level×9） | 提取的 VA 位域 |
|------|-------|------------------------|----------------|
| L0   | 0     | 39                     | VA[47:39]      |
| L1   | 1     | 30                     | VA[38:30]      |
| L2   | 2     | 21                     | VA[29:21]      |
| L3   | 3     | 12                     | VA[20:12]      |

**示例**：

设 `va = 0x0000_0000_0000_1000`（即 0x1000，第二页）：

- L0 index = (0x1000 >> 39) & 0x1FF = 0
- L1 index = (0x1000 >> 30) & 0x1FF = 0
- L2 index = (0x1000 >> 21) & 0x1FF = 0
- L3 index = (0x1000 >> 12) & 0x1FF = **1**（第二项，对应页内偏移 0）

设 `va = 1 << 39`（0x80_0000_0000，L0 第二项起始）：

- L0 index = (0x80_0000_0000 >> 39) & 0x1FF = **1**

> 这正是 `mm/src/page_table.rs` 中 `test_index_l3` 与 `test_index_l0` 测试用例验证的值。

---

## 6. 叶子表项构造

`PageTable::make_leaf(pa, flags)` 构造一个 L3 叶子表项，将物理地址 `pa` 按 `flags` 描述的属性映射：

```rust
pub fn make_leaf(pa: u64, flags: MemFlags) -> u64 {
    let mut pte = (pa & !0xFFF) | PTE_VALID | PTE_AF | PTE_SH_INNER;
    let mt = if flags.device { MT_DEVICE } else { MT_NORMAL };
    pte |= mt;
    if !flags.executable {
        pte |= PTE_XN;
    }
    if !flags.writable {
        pte |= PTE_PXN;
    }
    pte
}
```

**构造逻辑**：

1. **基础值** = `pa`（页对齐，屏蔽低 12 位）| `PTE_VALID` | `PTE_AF` | `PTE_SH_INNER`
   - VALID = 1：表项有效
   - AF = 1：访问标志置位，避免 Access flag fault（EnerOS 不依赖 AF fault 做按需换页）
   - SH_INNER = 0b11：Inner Shareable，多核间缓存一致
2. **内存属性**：`flags.device` 为真 → `MT_DEVICE`（AttrIndex=1）；否则 `MT_NORMAL`（AttrIndex=0）
3. **可执行性**：`flags.executable` 为假 → 置 `PTE_XN`（bit 54），禁止任何异常等级执行
4. **可写性**：`flags.writable` 为假 → 置 `PTE_PXN`（bit 53），禁止特权写
   > 注：EnerOS v0.8.0 复用 PXN 位表达"不可写"语义，因为 ARMv8 叶子 PTE 没有独立的"Writable"位——写权限通过 AP（bits[7:6]）控制，此处为简化实现。

**三种预设 MemFlags**（`hal/src/types.rs`）：

| 预设 | readable | writable | executable | device | cacheable | 生成的 PTE 特征 |
|------|----------|----------|------------|--------|-----------|-----------------|
| `device()` | ✓ | ✓ | ✗ | ✓ | ✗ | MT_DEVICE \| XN |
| `normal()` | ✓ | ✓ | ✗ | ✗ | ✓ | MT_NORMAL \| XN |
| `code()`   | ✓ | ✗ | ✓ | ✗ | ✓ | MT_NORMAL \| PXN |

---

## 7. 中间表项构造

`PageTable::make_table(child_pa)` 构造一个 L0–L2 的中间表项，指向下一级页表：

```rust
pub fn make_table(child_pa: u64) -> u64 {
    (child_pa & !0xFFF) | PTE_VALID | PTE_TABLE
}
```

**构造逻辑**：

1. `child_pa & !0xFFF`：取下一级页表的物理地址（4KB 对齐，低 12 位为 0）
2. `| PTE_VALID`：置有效位
3. `| PTE_TABLE`：置 TABLE 位（bit 1 = 1），表示该项是中间表项而非叶子

**中间表项与叶子表项的区别**：

| 属性 | 中间表项（table） | 叶子表项（leaf） |
|------|-------------------|------------------|
| TABLE 位（bit 1） | 1 | 0 |
| 出现级别 | L0、L1、L2 | L3（EnerOS 不用 L0–L2 block） |
| 指向 | 下一级页表（4KB） | 4KB 物理页 |
| 包含属性位 | 无（属性在 L3 叶子上） | AttrIndex / SH / AF / XN / PXN |

> **ARMv8 ARM 对应**：参见 D5.3.1 "VMSAv8-64 translation table entry formats"。bit[1:0] = 0b11 为 table descriptor，0b01 为 L3 block（leaf）descriptor。

---

## 8. TLB 管理

页表修改后，旧的 TLB（Translation Lookaside Buffer）缓存项可能与新页表不一致，必须刷新。ARMv8 提供多种 TLB 刷新指令：

| 指令 | 范围 | 用途 |
|------|------|------|
| `tlbi alle1` | 当前 EL 全部 ASID | 刷新所有地址空间（开销大） |
| `tlbi asid, <Xt>` | 指定 ASID | 仅刷新某一地址空间（推荐） |
| `tlbi vaae1, <Xt>` | 指定 VA，所有 ASID | 刷新某 VA 在所有空间的项 |
| `tlbi vae1, <Xt>` | 指定 VA + ASID | 精确刷新某 VA 某空间 |

### 8.1 ASID 机制

ASID（Address Space ID）是 8 位或 16 位标识符（由 TCR_EL1.AS 决定，EnerOS 使用 16 位），用于区分不同地址空间。每个 `Vspace` 拥有独立 ASID，MMU 在填充 TLB 时自动为缓存项打上当前 ASID 标签。切换地址空间时（写 TTBR0_EL1）只需设置新 ASID，无需全刷 TLB——其他 ASID 的缓存项仍有效。

### 8.2 EnerOS 实现

`Vspace::flush_tlb()` 使用 `tlbi asid` 指令，仅刷新当前地址空间：

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

**操作数格式**：`tlbi asid` 的操作数高 16 位为 ASID，低 48 位保留。故将 `self.asid` 左移 48 位填入。这样在 `map`/`unmap`/`set_flags` 后只刷新受影响的地址空间，避免全局 TLB 失效带来的性能损失。

> **非 aarch64 目标**：`flush_tlb` 在 host 测试环境下为空操作（no-op），因为测试不触及真实硬件。

---

## 9. EnerOS 实现

### 9.1 PageTable 结构体

`mm/src/page_table.rs` 定义的页表结构体：

```rust
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; TABLE_ENTRIES],  // 512 个 64 位表项
}
```

- `#[repr(C, align(4096))]`：保证 4KB 对齐，与硬件要求一致（页表基址必须 4KB 对齐）
- `entries`：512 个 `u64` 表项，共 4096 字节，恰好一页

### 9.2 PageLevel 枚举

```rust
pub enum PageLevel {
    L0,  // PGD
    L1,  // PUD
    L2,  // PMD
    L3,  // PTE (leaf, 4KB page)
}
```

### 9.3 静态页表页池

v0.8.0 使用静态数组作为中间页表页的分配源（`mm/src/vspace.rs`）：

```rust
const PT_POOL_SIZE: usize = 64;
static mut PAGE_TABLE_POOL: [PageTable; PT_POOL_SIZE] = [const { PageTable::new() }; PT_POOL_SIZE];
static mut PT_POOL_NEXT: usize = 0;
```

- 池大小 **64 张页表**，每张 4KB，共占用 256KB
- `alloc_page_table()` 线性分配，返回页表的内核虚拟地址作为"物理地址"占位
- 池耗尽返回 `None`，调用方转为 `MmError::OutOfMemory`

> **v0.10.0 演进**：蓝图 v0.10.0 将实现内核态 buddy 堆分配器，届时 `alloc_page_table()` 将改为从堆分配真实物理页，本静态池将被移除。当前静态池是 v0.8.0 在堆可用前的过渡方案。

### 9.4 Pte 包装类型

```rust
#[derive(Clone, Copy)]
pub struct Pte(pub u64);
```

提供对 64 位 PTE 的类型安全包装（v0.8.0 主要使用裸 `u64` 操作，`Pte` 为后续扩展预留）。

---

## 10. 与 ARMv8 ARM 对应

本文档中的设计严格对应 ARMv8 Architecture Reference Manual（ARM DDI 0487）的以下章节：

| ARMv8 ARM 章节 | 内容 | 本文档对应节 |
|----------------|------|--------------|
| D5.2 | VMSA 概述（Virtual Memory System Architecture） | §1 概述 |
| D5.3.1 | VMSAv8-64 表项格式（table descriptor / block descriptor） | §4 PTE 位域、§7 中间表项 |
| D5.3.2 | Table descriptor 的 PA 字段与 TABLE 位 | §7 中间表项构造 |
| D5.4   | TTBR0_EL1 / TTBR1_EL1 与 ASID | §8 TLB 管理 |
| D5.10  | TLB 维护指令（`tlbi`） | §8.2 EnerOS 实现 |
| D5.5.5 | MAIR_EL1 与 AttrIndex | §4 PTE 位域 |
| D8     | ARMv8 内存模型与 Shareability | §4 SH 字段、§6 SH_INNER |

**关键约束**（来自 ARMv8 ARM）：
- 页表基址（TTBRn_EL1）必须 4KB 对齐
- 页表项必须 8 字节对齐访问
- AF 位在硬件支持下可由 MMU 自动置位（AF fault 模式），EnerOS 选择软件预置为 1
- AttrIndex 索引 MAIR_EL1 的 8 个属性域，EnerOS 仅使用前 2 个（Normal / Device）

---

## 参考

- `mm/src/page_table.rs` — 页表结构体与 PTE 构造
- `mm/src/vspace.rs` — Vspace 与页表遍历
- `蓝图/phase1.md` §v0.8.0 — 版本蓝图
- ARMv8 Architecture Reference Manual（ARM DDI 0487）D5 章
