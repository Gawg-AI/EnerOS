# EnerOS v0.9.0 — 分区内存隔离验证 Spec

> **版本**：v0.9.0（Phase 0 / P0-C 第二步）
> **类型**：验证版本（物理内存分区 + 跨分区防护 + DMA 保护）
> **前序依赖**：v0.8.0（页表与地址空间，提供 Vspace/MmError）
> **后续版本**：v0.10.0（内核态堆分配器，需在分区内运行）
> **蓝图依据**：`蓝图/phase0.md` §v0.9.0（第 1664–1852 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：签名必须可编译）

---

## Why

隔离是混合关键性架构的安全根基。v0.9.0 实现物理内存分区、跨分区访问检查与 DMA 保护，验证分区 A 无法读写分区 B 内存。这是"双分区隔离"出口标准的核心验证。

---

## What Changes

- **新增** `mm/src/partition.rs`：`PaddrRange`/`Partition`/`PartitionError`，物理内存分区与隔离检查（~250 行）
- **新增** `mm/src/dma_guard.rs`：`DmaGuard` trait/`SmmuGuard`/`DmaDomain`/`DeviceId`，DMA 保护域（~150 行）
- **修改** `mm/src/vspace.rs`：`MmError` 新增 `PermissionDenied` 变体 + Display 分支
- **修改** `mm/src/lib.rs`：新增 `pub mod partition;` `pub mod dma_guard;`
- **修改** workspace 根 `Cargo.toml`：version `0.8.0` → `0.9.0`
- **修改** `hal/src/arm64/provider.rs`：`mem()` panic 消息 `v0.9.0` → `v0.10.0`
- **修改** `.github/workflows/ci.yml`：版本标识 v0.8.0 → v0.9.0
- **修改** `Makefile`：VERSION 0.8.0 → 0.9.0
- **修改** `ci/src/gate.rs`：注释更新
- **新增** 文档：`docs/partition-isolation-design.md`、`docs/dma-protection-guide.md`

---

## Impact

- **Affected specs**：v0.10.0（堆分配需在分区内运行）、v0.21.0（共享内存需跨分区授权）
- **Affected code**：
  - 新增 `mm/src/partition.rs`、`mm/src/dma_guard.rs`（~400 行）
  - 修改 `mm/src/vspace.rs`（+1 enum 变体）、`mm/src/lib.rs`（+2 mod 声明）
  - workspace 版本号、CI/Makefile/gate.rs、provider.rs 注释
- **不影响**：v0.5.0~v0.8.0 的现有功能（回归兼容）

---

## ADDED Requirements

### Requirement: PaddrRange 物理地址范围

系统 SHALL 提供 `PaddrRange` 结构体描述一段物理地址区间。

#### Scenario: 结构定义

- **GIVEN** `PaddrRange { pub start: u64, pub end: u64 }`（derive Clone, Copy, Debug）
- **THEN** 描述 `[start, end)` 半开区间

#### Scenario: 包含检查

- **WHEN** 调用 `contains(pa)` 且 `start <= pa < end`
- **THEN** 返回 `true`
- **AND** `pa >= end` 或 `pa < start` 时返回 `false`

#### Scenario: 重叠检查

- **WHEN** 调用 `overlaps(other)` 且两区间有交集
- **THEN** 返回 `true`
- **AND** 无交集时返回 `false`

### Requirement: Partition 内存分区

系统 SHALL 提供 `Partition` 结构体，描述一个内存分区及其物理地址授权范围。

#### Scenario: 结构定义

- **GIVEN** `Partition` 含字段：`id: u32`、`name: &'static str`、`vspace: Vspace`、`allowed_phys: [PaddrRange; 8]`、`quota: u64`、`used: u64`
- **THEN** 每个分区有独立 ID、名称、地址空间、授权物理区间、配额上限与已用量

#### Scenario: 访问检查 — 授权范围内

- **WHEN** 调用 `check_access(pa, size)` 且 `[pa, pa+size)` 完全在某个 `allowed_phys` 区间内
- **AND** `used + size <= quota`
- **THEN** 返回 `Ok(())`

#### Scenario: 访问检查 — 超出授权范围

- **WHEN** 调用 `check_access(pa, size)` 且 `pa` 不在任何 `allowed_phys` 区间内
- **THEN** 返回 `Err(MmError::PermissionDenied)`

#### Scenario: 访问检查 — 超出配额

- **WHEN** 调用 `check_access(pa, size)` 且在授权范围内
- **AND** `used + size > quota`
- **THEN** 返回 `Err(MmError::OutOfMemory)`

#### Scenario: 隔离判断

- **WHEN** 调用 `is_isolated_from(other)` 且两分区的 `allowed_phys` 无重叠
- **THEN** 返回 `true`
- **AND** 任何区间重叠时返回 `false`

#### Scenario: 物理地址分配

- **WHEN** 调用 `alloc_phys(size)` 且配额充足
- **THEN** 从第一个 `allowed_phys` 区间 bump 分配，返回起始物理地址
- **AND** 递增 `used`
- **AND** 配额不足时返回 `Err(MmError::OutOfMemory)`

#### Scenario: 物理地址释放

- **WHEN** 调用 `free_phys(pa, size)`
- **THEN** 递减 `used`（本版本不做实际回收，仅记账）

### Requirement: DmaGuard trait 与 SmmuGuard

系统 SHALL 提供 `DmaGuard` trait 和 `SmmuGuard` 实现，用于 DMA 保护域管理。

#### Scenario: DmaGuard trait

- **GIVEN** `DmaGuard` trait 含方法：
  - `authorize(&self, dev: DeviceId, range: PaddrRange) -> Result<(), MmError>`
  - `check(&self, dev: DeviceId, pa: u64) -> Result<(), MmError>`
- **THEN** 提供设备 DMA 授权与检查能力

#### Scenario: SmmuGuard 结构

- **GIVEN** `SmmuGuard` 含 `domains: [DmaDomain; 16]`
- **THEN** 最多管理 16 个 DMA 保护域

#### Scenario: DMA 授权检查 — 已授权

- **WHEN** 调用 `check(dev, pa)` 且 `dev` 对应域的 `allowed_phys` 包含 `pa`
- **THEN** 返回 `Ok(())`

#### Scenario: DMA 授权检查 — 未授权

- **WHEN** 调用 `check(dev, pa)` 且 `dev` 无对应域或 `pa` 不在授权范围
- **THEN** 返回 `Err(MmError::PermissionDenied)`

#### Scenario: DMA 授权配置

- **WHEN** 调用 `authorize(dev, range)`
- **THEN** 本版本为 stub（返回 `Ok(())`），SMMU 硬件配置推迟到硬件集成阶段

### Requirement: DeviceId 与 DmaDomain

- `DeviceId(pub u32)` — 设备标识符（newtype，derive Clone, Copy, Debug, PartialEq, Eq）
- `DmaDomain { owner_partition: u32, allowed_phys: PaddrRange }` — DMA 保护域

### Requirement: MmError 新增 PermissionDenied

系统 SHALL 在 `MmError` 枚举中新增 `PermissionDenied` 变体，表示跨分区访问被拒绝。

### Requirement: 文档交付

1. `docs/partition-isolation-design.md`《分区隔离设计》
2. `docs/dma-protection-guide.md`《DMA 保护方案》

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.8.0` 升级到 `0.9.0`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 版本标识从 v0.8.0 升级到 v0.9.0。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.8.0 升级到 0.9.0。

### Requirement: ARM64 HAL Provider

`hal/src/arm64/provider.rs` 的 `mem()` panic 消息从 "v0.9.0" 更新为 "v0.10.0"。

---

## 设计决策（Design Decisions）

### D1: Partition 包含 vspace 字段

蓝图 §4.1 接口定义包含 `vspace: Vspace`，§4.5 关键代码省略。本版本遵循 §4.1 包含该字段。理由：
- 架构正确性：分区拥有独立地址空间
- 未来版本（v0.10.0+）将使用 vspace 进行分区内映射
- 本版本方法（check_access/is_isolated_from）不使用 vspace，但结构体完整定义

### D2: alloc_phys 为 bump 分配器

本版本无堆分配器（v0.10.0 才实现），`alloc_phys` 实现为从第一个 `allowed_phys` 区间的 bump 分配。理由：
- 最小可工作方案：无需复杂分配器即可测试配额逻辑
- `free_phys` 仅递减 `used`，不做实际回收（记账式）
- v0.10.0 buddy 分配器就绪后替换

### D3: DmaGuard::authorize 为 stub

`authorize` 方法返回 `Ok(())` 但不配置 SMMU 硬件。理由：
- SMMUv3 硬件配置需要 seL4 capability 深度集成（蓝图 §5.4）
- 本版本聚焦隔离逻辑验证，非硬件配置
- `check` 方法仍可测试（基于 domains 数组的软件检查）

### D4: PermissionDenied 复用 MmError

不新建 `PartitionError`，而是给 `MmError` 新增 `PermissionDenied` 变体。理由：
- 蓝图 §4.5 代码直接使用 `MmError::PermissionDenied`
- 避免错误类型膨胀
- 保持 mm crate 错误类型统一

### D5: allowed_phys 固定 8 槽

`allowed_phys: [PaddrRange; 8]` 使用固定数组而非动态集合。理由：
- no_std 无堆，不能用 Vec
- 8 个物理区间足够描述典型分区（RAM 段 + 设备 MMIO 段）
- 空槽用 `{start: 0, end: 0}` 表示

---

## 非目标（Non-Goals）

- **不实现** SMMU 硬件配置（推迟到硬件集成）
- **不实现** 实际物理页回收（free_phys 仅记账）
- **不做** QEMU 运行时 fault 注入（延后到 kernel 集成）
- **不做** 动态分区创建/销毁（蓝图 §8.4 未来扩展）
- **不实现** HalMem trait（仍推迟，provider.rs panic 指向 v0.10.0）
- **不修改** v0.8.0 的 PageTable/Vspace/AddressSpace 实现

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| SMMU 不可用 | 中/高 | 软件检查兜底（DmaGuard::check） |
| 物理区间碎片化 | 低/中 | v0.10.0 buddy 分配器解决 |
| 共享内存误判越权 | 中/中 | 蓝图 §8.5：共享区需显式授权 |
| bump 分配器耗尽 | 低/低 | 返回 OutOfMemory |
