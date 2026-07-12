# EnerOS v0.8.0 — 页表管理与地址空间 Spec

> **版本**：v0.8.0（Phase 0 / P0-C 起点）
> **类型**：实现版本（ARM64 四级页表 + 虚拟地址空间抽象）
> **前序依赖**：v0.6.0（HAL 核心，提供 MemFlags 类型）、v0.7.0（HAL 外设）
> **后续版本**：v0.9.0（分区内存隔离验证）
> **蓝图依据**：`蓝图/phase0.md` §v0.8.0（第 1443–1661 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：签名必须可编译）

---

## Why

虚拟内存是隔离的物理基础。v0.8.0 实现 ARM64 四级页表（48 位 VA，4KB granule）、`Vspace`/`Vregion` 抽象与 `AddressSpace` trait，提供 map/unmap/translate/set_flags 能力。这是 P0-C（内存与堆）的起点，直接支撑 v0.9.0 分区隔离验证与"双分区隔离"出口标准。

---

## What Changes

- **新增** `mm/` crate（no_std 库），包含三个源文件：
  - `mm/src/page_table.rs`：`Pte`/`PageLevel`/`PageTable`，PTE 位标志常量，`index`/`make_leaf`/`make_table` 方法（~280 行）
  - `mm/src/vregion.rs`：`Vregion`/`Backing`，虚拟内存区域描述（~120 行）
  - `mm/src/vspace.rs`：`Vspace`/`AddressSpace` trait/`MmError`，四级页表遍历与映射（~300 行）
- **新增** `mm/Cargo.toml`、`mm/src/lib.rs`
- **修改** workspace 根 `Cargo.toml`：members 添加 `"mm"`，version `0.7.0` → `0.8.0`
- **修改** `hal/src/arm64/provider.rs`：`mem()` panic 消息从 "v0.8.0" 更新为 "v0.9.0"（HalMem trait 签名 `&self` 与 AddressSpace `&mut self` 不兼容，trait 适配推迟）
- **修改** `.github/workflows/ci.yml`：版本标识 v0.7.0 → v0.8.0，新增 mm crate 交叉编译步骤
- **修改** `Makefile`：VERSION 0.7.0 → 0.8.0，新增 `mm-build`/`mm-test` 目标
- **修改** `ci/src/gate.rs`：注释更新说明 v0.8.0 mm crate
- **新增** 文档：`docs/arm64-page-table-design.md`《ARM64 页表设计》、`docs/address-space-layout.md`《地址空间布局》

---

## Impact

- **Affected specs**：v0.9.0（分区隔离）依赖本版本页表；v0.10.0（内核堆）依赖本版本地址空间
- **Affected code**：
  - 新建 `mm/` crate（~700 行代码）
  - workspace `Cargo.toml`、CI 配置、Makefile、provider.rs 注释
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello/hal crate 的功能行为
- **不影响**：v0.5.0/v0.6.0/v0.7.0 的 HAL 实现（回归兼容）
- **依赖关系**：mm crate 依赖 hal crate（使用 `MemFlags` 类型，避免重复定义）

---

## ADDED Requirements

### Requirement: PageTable 页表实现

系统 SHALL 提供 `mm/src/page_table.rs`，实现 ARM64 四级页表的核心数据结构与操作。

#### Scenario: 页表项结构

- **GIVEN** `Pte(pub u64)` 包装类型
- **THEN** 每个页表项为 64 位，通过位域编码物理地址与属性标志

#### Scenario: 页表级别

- **GIVEN** `PageLevel` 枚举：`L0`（PGD）、`L1`（PUD）、`L2`（PMD）、`L3`（PTE 叶子）
- **THEN** L0→L1→L2→L3 逐级索引，L3 为叶子层级（4KB 页）

#### Scenario: PTE 位标志常量

- **PTE_VALID** = 1 << 0（有效位）
- **PTE_TABLE** = 1 << 1（非叶子，指向下一级表）
- **PTE_AF** = 1 << 10（Access Flag）
- **PTE_SH_INNER** = 3 << 8（Inner Shareable）
- **PTE_PXN** = 1 << 53（Privileged Execute Never）
- **PTE_XN** = 1 << 54（Execute Never）
- **MT_NORMAL** = 0 << 2（Normal 内存属性索引）
- **MT_DEVICE** = 1 << 2（Device 内存属性索引）

#### Scenario: 页表索引计算

- **WHEN** 调用 `PageTable::index(level, va)`
- **THEN** 根据 `level` 从 VA 中提取对应 9 位索引
- **AND** L0 取 bit[47:39]，L1 取 bit[38:30]，L2 取 bit[29:21]，L3 取 bit[20:12]

#### Scenario: 构造叶子表项

- **WHEN** 调用 `PageTable::make_leaf(pa, flags)`
- **THEN** 返回 L3 叶子 PTE，包含物理地址（页对齐）、PTE_VALID、PTE_AF、PTE_SH_INNER
- **AND** 根据 `flags.device` 选择 MT_DEVICE 或 MT_NORMAL
- **AND** `flags.executable` 为 false 时置 PTE_XN
- **AND** `flags.writable` 为 false 时置 PTE_PXN（简化：用 PXN 兼职只读）

#### Scenario: 构造中间表项

- **WHEN** 调用 `PageTable::make_table(child_pa)`
- **THEN** 返回 L0-L2 非叶子 PTE，包含子表物理地址（页对齐）、PTE_VALID、PTE_TABLE

#### Scenario: 页表常量

- **PAGE_SIZE** = 4096
- **TABLE_ENTRIES** = 512（4KB / 8B = 512 项）

### Requirement: Vregion 虚拟内存区域

系统 SHALL 提供 `mm/src/vregion.rs`，描述虚拟内存区域。

#### Scenario: Vregion 结构

- **GIVEN** `Vregion` 结构体，含字段：`start_va: u64`、`size: u64`、`flags: MemFlags`、`backing: Backing`
- **THEN** 描述一段连续虚拟地址范围及其属性

#### Scenario: Backing 后端类型

- **GIVEN** `Backing` 枚举：`Identity`（等同映射）、`Phys(u64)`（指定物理地址）、`Demand`（按需分配）
- **THEN** Identity 表示 va == pa；Phys 表示映射到指定物理地址；Demand 表示缺页时分配

### Requirement: Vspace 与 AddressSpace trait

系统 SHALL 提供 `mm/src/vspace.rs`，实现虚拟地址空间管理与 `AddressSpace` trait。

#### Scenario: Vspace 结构

- **GIVEN** `Vspace` 结构体，含字段：`root_paddr: u64`（L0 页表物理地址）、`asid: u16`（地址空间 ID）、`regions: [Option<Vregion>; 16]`
- **THEN** 每个地址空间有独立的 L0 根页表和 ASID

#### Scenario: AddressSpace trait

- **GIVEN** `AddressSpace` trait，含方法：
  - `map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError>`
  - `unmap(&mut self, va: u64, size: u64) -> Result<(), MmError>`
  - `translate(&self, va: u64) -> Option<u64>`
  - `set_flags(&mut self, va: u64, flags: MemFlags) -> Result<(), MmError>`
- **THEN** 提供完整的虚拟内存映射/取消映射/地址翻译/属性修改能力

#### Scenario: MmError 错误类型

- **GIVEN** `MmError` 枚举：`InvalidAddr`、`NotMapped`、`AlreadyMapped`、`OutOfMemory`、`Misaligned`
- **THEN** 覆盖所有内存管理错误场景

#### Scenario: 映射地址

- **WHEN** 调用 `vspace.map(va, pa, size, flags)` 且 va/pa 4KB 对齐
- **THEN** 遍历 L0→L1→L2→L3 四级页表
- **AND** 中间表项缺失时分配新页表页（本版本用静态预留）
- **AND** L3 写入叶子表项
- **AND** 完成 size 范围内所有 4KB 页映射
- **AND** TLB 刷新（使用 ASID）

#### Scenario: 映射未对齐地址

- **WHEN** 调用 `vspace.map(va, pa, size, flags)` 且 va 或 pa 非 4KB 对齐
- **THEN** 返回 `Err(MmError::Misaligned)`

#### Scenario: 重复映射拒绝

- **WHEN** 调用 `vspace.map(va, pa, size, flags)` 且 va 已有映射
- **THEN** 返回 `Err(MmError::AlreadyMapped)`

#### Scenario: 地址翻译

- **WHEN** 调用 `vspace.translate(va)` 且 va 已映射
- **THEN** 遍历四级页表，返回对应物理地址 `Some(pa)`
- **AND** va 未映射时返回 `None`

### Requirement: 静态页表页预留

系统 SHALL 使用静态预留的页表页内存，而非动态堆分配。v0.10.0 内核堆就绪后切换为动态分配。

#### Scenario: 页表页池

- **GIVEN** 一个静态 `[PageTable; N]` 数组作为页表页池
- **THEN** map 遍历时从此池分配中间页表页
- **AND** 池耗尽时返回 `Err(MmError::OutOfMemory)`

### Requirement: no_std 合规

mm crate MUST 遵循蓝图 §43.1：`#![no_std]`（`#![cfg_attr(not(test), no_std)]` 模式支持 host 测试），不使用 `std::*`。MMIO 使用 `core::ptr::read_volatile`/`write_volatile`。

### Requirement: 文档交付

系统 SHALL 交付两份文档：
1. `docs/arm64-page-table-design.md`《ARM64 页表设计》——四级页表架构、PTE 位域、索引计算、TLB 管理、与 ARMv8 ARM 对应
2. `docs/address-space-layout.md`《地址空间布局》——Vspace/Vregion 模型、ASID 机制、Backing 类型、QEMU virt 地址空间布局

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.7.0` 升级到 `0.8.0`，members 新增 `"mm"`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 的版本标识从 v0.7.0 升级到 v0.8.0，cross-build 新增 mm crate 编译步骤。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.7.0 升级到 0.8.0，新增 `mm-build`/`mm-test` 目标。

### Requirement: ARM64 HAL Provider

`hal/src/arm64/provider.rs` 的 `mem()` panic 消息从 "v0.8.0" 更新为 "v0.9.0"（HalMem trait `&self` 签名与 AddressSpace `&mut self` 不兼容，trait 适配推迟到隔离验证阶段）。

---

## 设计决策（Design Decisions）

### D1: 独立 mm crate 而非 hal 子模块

页表管理放在独立 `mm/` crate，而非 `hal/src/arm64/` 下。理由：
- 蓝图 §3 交付物明确为 `mm/src/` 目录
- mm crate 是更高层抽象（AddressSpace），hal crate 是硬件抽象（HalMem）
- mm crate 依赖 hal crate（使用 MemFlags），依赖方向清晰
- 后续 v0.10.0 内核堆、v0.11.0 用户堆都在 mm crate 内扩展

### D2: AddressSpace trait 用 &mut self

蓝图 §4.2 定义的 `AddressSpace` trait 方法用 `&mut self`（map/unmap/set_flags 需要修改页表结构）。这与 `HalMem` trait 的 `&self` 不兼容。理由：
- 页表映射本质上是修改内部结构，`&mut self` 语义正确
- `HalMem` trait 的 `&self` 设计适用于单例场景（通过内部可变性），但本版本优先遵循蓝图
- HalMem 的适配包装推迟到后续版本

### D3: 复用 hal::MemFlags

mm crate 依赖 hal crate，直接使用 `hal::MemFlags` 类型，不重复定义。理由：
- 蓝图 §4.5 代码使用 `flags.device`/`flags.executable`/`flags.writable`，正是 MemFlags 字段
- 避免类型重复定义与转换开销
- 保持类型一致性

### D4: 静态页表页池

本版本使用静态 `[PageTable; N]` 数组作为页表页池，而非动态堆分配。理由：
- v0.10.0 才实现内核堆，本版本无堆可用
- 蓝图 §5.4 明确"本版本用静态预留，v0.10.0 后用堆"
- 蓝图 §4.4"缺页表页：从内核堆分配（v0.10.0 后），本版本预留接口"
- 池大小 N 根据典型地址空间需求设定（如 64 个页表页 = 256KB）

### D5: 48 位 VA + 4KB granule

采用 ARMv8 标准配置：48 位虚拟地址、4KB granule、四级页表。理由：
- 蓝图 §5.1 明确假设前提
- ARMv8 ARM 标准配置，飞腾/鲲鹏均支持
- QEMU virt 默认 48 位 VA
- 后续可通过配置支持 52 位 VA 或 64KB granule

### D6: TLB 刷新使用 ASID

TLB 刷新使用 `tlbi asid` 而非 `tlbi alle1`（全刷）。理由：
- 蓝图 §5.2"ASID 避免频繁 TLB 全刷"
- ASID 允许不同地址空间共存于 TLB
- 减少上下文切换时的 TLB 失效

### D7: cfg_attr(not(test), no_std) 模式

mm crate 使用 `#![cfg_attr(not(test), no_std)]` 模式。理由：
- 正式构建保持 no_std
- host 测试时链接 std，便于运行单元测试
- 与 hal crate（v0.5.0）保持一致的模式

---

## 非目标（Non-Goals）

- **不实现** `HalMem` trait（签名 `&self` 与 AddressSpace `&mut self` 不兼容，推迟到后续版本）
- **不实现**动态堆分配页表页（属于 v0.10.0 内核堆）
- **不做** QEMU 运行时映射验证（延后到 kernel 集成）
- **不做**大页支持（2MB/1GB block，蓝图 §8.4 为未来扩展）
- **不做**分区隔离验证（属于 v0.9.0）
- **不集成**到 kernel/runtime 调用链（属于后续版本）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| 页表页内存来源受限 | 中/中 | 静态预留池，v0.10.0 后切换堆分配 |
| TLB 一致性 | 中/高 | ASID + tlbi asid 刷新 |
| AF 位未置位导致 fault | 中/高 | make_leaf 强制置 PTE_AF |
| 交叉编译 aarch64 内联汇编 | 低/中 | asm! 用 cfg 门控，host 测试不涉及 |
| MemFlags 跨 crate 依赖 | 低/低 | mm crate 依赖 hal crate，版本同步 |
