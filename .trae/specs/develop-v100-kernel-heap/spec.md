# EnerOS v0.10.0 — 内核态堆分配器 Spec

> **版本**：v0.10.0（Phase 0 / P0-C 第三步，★瓶颈版本）
> **类型**：基础服务实现版本（slab + buddy 混合堆分配器）
> **前序依赖**：v0.9.0（分区隔离，堆需在分区内运行）
> **后续版本**：v0.11.0（用户态堆分配器，复用本版本算法）
> **蓝图依据**：`蓝图/phase0.md` §v0.10.0（第 1855–2255 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（★瓶颈版本：代码必须可运行，不能 stub）

---

## Why

内核态堆是所有内核数据结构（页表页、TCB、IPC 缓冲区）的内存来源。v0.8.0/v0.9.0 的页表池使用静态数组，无法动态扩展。v0.10.0 实现 slab + buddy 混合堆分配器，使内核具备动态内存管理能力，是 Phase 0 的关键瓶颈之一。

---

## What Changes

- **新增** `heap/Cargo.toml`：新 crate `eneros-heap`，依赖 `spin`
- **新增** `heap/src/lib.rs`（~150 行）：`KernelHeap` 占位类型、`GlobalAlloc` 实现、`heap_init`/`heap_stats` 全局接口
- **新增** `heap/src/buddy.rs`（~280 行）：`BuddyAllocator` 页级分配器，含位图合并检测
- **新增** `heap/src/slab.rs`（~300 行）：`SlabCache`/`SlabAllocator` 小对象池（8 个 bucket：8/16/32/64/128/256/512/1024 字节）
- **新增** `heap/src/stats.rs`（~120 行）：`HeapStats` 碎片统计
- **修改** workspace 根 `Cargo.toml`：members 添加 `"heap"`，version `0.9.0` → `0.10.0`
- **修改** `.github/workflows/ci.yml`：版本标识 v0.9.0 → v0.10.0，cross-build 添加 heap crate
- **修改** `Makefile`：VERSION 0.9.0 → 0.10.0，添加 `heap-build`/`heap-test` 目标
- **修改** `ci/src/gate.rs`：注释更新（+ v0.10.0 heap）
- **新增** 文档：`docs/kernel-heap-design.md`、`docs/slab-buddy-algorithm.md`

---

## Impact

- **Affected specs**：v0.11.0（用户态堆）将复用本版本算法；v0.18.0（TCB）将依赖本堆分配
- **Affected code**：
  - `heap/` 新增 4 个源文件（~850 行代码）
  - workspace `Cargo.toml`、CI 配置、Makefile、ci/gate.rs
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello/hal/mm crate 的功能行为
- **不影响**：v0.8.0/v0.9.0 的页表/分区实现（回归兼容）
- **不影响**：mm crate 的静态页表池（未来版本才迁移到堆）

---

## ADDED Requirements

### Requirement: BuddyAllocator 页级分配器

系统 SHALL 提供 `BuddyAllocator` 结构体，实现基于 buddy 算法的页级内存分配，支持块分裂与合并。

#### Scenario: 初始化

- **WHEN** 调用 `buddy.init(base, pages)` 传入堆池基址和页数
- **THEN** 将所有页放入最大可用阶的空闲链
- **AND** 位图清零（所有页标记为空闲）
- **AND** `free_count[max_order]` = 1

#### Scenario: 分配单页

- **WHEN** 调用 `buddy.alloc(4096)`（1 页）
- **THEN** 从 order 0 空闲链取一块
- **AND** 若 order 0 无空闲，从更高阶分裂
- **AND** 位图中对应页标记为已分配
- **AND** 返回块基址指针

#### Scenario: 分配多页

- **WHEN** 调用 `buddy.alloc(8192)`（2 页）
- **THEN** 计算 order = 1
- **AND** 从 order 1 空闲链取一块
- **AND** 位图标记 2 页为已分配
- **AND** 返回块基址指针

#### Scenario: 释放并合并

- **WHEN** 调用 `buddy.dealloc(ptr, size)` 释放一块
- **THEN** 检查 buddy 块（XOR 地址）是否空闲
- **AND** 若空闲，从空闲链移除 buddy 并合并
- **AND** 递归向上合并直到 buddy 不空闲或达到 MAX_ORDER
- **AND** 位图清除对应页标记

#### Scenario: OOM 处理

- **WHEN** 堆内存耗尽，无可用块满足请求
- **THEN** 返回 `core::ptr::null_mut()`
- **AND** 不 panic

#### Scenario: 阶数计算

- **WHEN** `order_for(4096)` → 返回 0
- **WHEN** `order_for(4097)` → 返回 1（向上取整到 2 页）
- **WHEN** `order_for(8192)` → 返回 1
- **WHEN** `order_for(4 * 1024 * 1024)` → 返回 10（MAX_ORDER=11，最大 4MB）

#### Scenario: 位图常量

- **MAX_ORDER** = 11（最大 4MB 块 = 1024 页）
- **PAGE_SIZE** = 4096
- **BITMAP_WORDS** = 128（128 × 64 = 8192 bit，支持最多 8192 页 = 32MB 堆）

### Requirement: SlabCache 小对象池

系统 SHALL 提供 `SlabCache` 结构体，为固定大小对象提供 O(1) 分配的空闲链池。

#### Scenario: 初始化

- **WHEN** 调用 `SlabCache::new(64)` 创建 64 字节对象的 slab
- **THEN** `obj_size` = 64，`free_head` = None，`total` = 0，`used` = 0

#### Scenario: 首次分配触发页申请

- **WHEN** slab 空闲链为空时调用 `slab.alloc(&mut buddy)`
- **THEN** 向 buddy 申请 1 页（4096 字节）
- **AND** 将页切分为 `4096 / obj_size` 个槽位
- **AND** 所有槽加入空闲链
- **AND** 返回第一个槽的指针

#### Scenario: 后续分配从空闲链取

- **WHEN** slab 空闲链非空时调用 `slab.alloc(&mut buddy)`
- **THEN** 从空闲链头取一个槽
- **AND** `used` += 1
- **AND** 不向 buddy 申请新页
- **AND** 返回槽指针

#### Scenario: 释放归还空闲链

- **WHEN** 调用 `slab.dealloc(ptr)`
- **THEN** 将 ptr 加入空闲链头
- **AND** `used` -= 1

#### Scenario: Slab bucket 配置

- **8 个 bucket**：obj_size = [8, 16, 32, 64, 128, 256, 512, 1024] 字节
- **每页槽位数**：PAGE_SIZE / obj_size（如 64 字节 → 64 槽/页）

### Requirement: KernelHeap GlobalAlloc 实现

系统 SHALL 提供 `KernelHeap` 零字段占位类型，实现 `core::alloc::GlobalAlloc` trait，通过 `spin::Mutex<KernelHeapInner>` 访问内部可变状态。

#### Scenario: alloc 小对象走 slab

- **WHEN** `GlobalAlloc::alloc(&self, layout)` 且 `layout.size() <= 1024`
- **THEN** 找到最小能容纳的 slab bucket
- **AND** 调用 `slab.alloc(&mut buddy)`
- **AND** `stats.slab_hits` += 1
- **AND** `stats.alloc_count` += 1
- **AND** 返回槽指针

#### Scenario: alloc 大对象走 buddy

- **WHEN** `GlobalAlloc::alloc(&self, layout)` 且 `layout.size() > 1024`
- **THEN** 调用 `buddy.alloc(layout.size())`
- **AND** `stats.buddy_hits` += 1
- **AND** `stats.alloc_count` += 1
- **AND** 返回块指针

#### Scenario: dealloc 归还

- **WHEN** `GlobalAlloc::dealloc(&self, ptr, layout)`
- **THEN** 若 `layout.size() <= 1024`，归还到对应 slab bucket
- **AND** 若 `layout.size() > 1024`，归还到 buddy
- **AND** `stats.free_count` += 1

#### Scenario: OOM 返回 null

- **WHEN** alloc 时堆耗尽
- **THEN** 返回 `core::ptr::null_mut()`
- **AND** 不 panic

### Requirement: heap_init / heap_stats 全局接口

系统 SHALL 提供两个全局函数管理堆生命周期。

#### Scenario: heap_init 初始化

- **WHEN** 调用 `heap_init(base, size)` 传入堆池基址和大小
- **THEN** 创建 `BuddyAllocator` 并 init(base, size / PAGE_SIZE)
- **THEN** 创建 8 个 `SlabCache`（SLAB_SIZES 数组）
- **THEN** 用 `spin::Mutex` 包裹存入 `KERNEL_HEAP` 全局静态变量
- **AND** 后续 alloc/dealloc 可正常工作

#### Scenario: heap_stats 查询

- **WHEN** 调用 `heap_stats()`
- **THEN** 返回 `HeapStats` 结构体
- **AND** 若堆未初始化，返回全零 `HeapStats::default()`

### Requirement: HeapStats 碎片统计

系统 SHALL 提供 `HeapStats` 结构体，实时反映堆状态。

#### Scenario: HeapStats 字段

```rust
pub struct HeapStats {
    pub total_bytes: u64,           // 堆总大小
    pub allocated_bytes: u64,       // 已分配字节
    pub free_bytes: u64,            // 空闲字节
    pub fragmentation_ratio: u32,   // 碎片率 0-1000（千分比）
    pub alloc_count: u64,           // 累计分配次数
    pub free_count: u64,            // 累计释放次数
    pub slab_hits: u64,             // slab 命中次数
    pub buddy_hits: u64,            // buddy 命中次数
}
```

#### Scenario: 碎片率计算

- **WHEN** `allocated_bytes` < `total_bytes` 且存在外部碎片
- **THEN** `fragmentation_ratio` = `(free_bytes - largest_free_block) / free_bytes * 1000`
- **AND** 范围为 0–1000（千分比）

#### Scenario: Default 实现

- **WHEN** 调用 `HeapStats::default()`
- **THEN** 所有字段为 0
- **AND** derive `Clone, Copy, Debug, Default`

### Requirement: no_std 合规

所有代码 MUST 遵循蓝图 §43.1：`#![cfg_attr(not(test), no_std)]`，正式构建 no_std，测试构建链接 std。使用 `core::alloc::{GlobalAlloc, Layout}`、`core::ptr`、`spin::Mutex`，不使用 `std::*`。

### Requirement: 文档交付

系统 SHALL 交付两份文档：
1. `docs/kernel-heap-design.md`：《内核堆分配器设计》——架构概述、数据结构、分配/释放流程、初始化序列、与 GlobalAlloc 的集成
2. `docs/slab-buddy-algorithm.md`：《slab/buddy 算法说明》——buddy 分裂/合并原理、slab 空闲链机制、碎片分析、性能目标

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.9.0` 升级到 `0.10.0`，members 列表添加 `"heap"`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 的版本标识从 v0.9.0 升级到 v0.10.0，cross-build job 添加 "Build heap crate" 步骤。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.9.0 升级到 0.10.0，添加 `heap-build`/`heap-test` 目标。

### Requirement: CI 门禁注释

`ci/src/gate.rs` 的 clippy/test 注释从 "+ v0.9.0 partition" 更新为 "+ v0.10.0 heap"。

---

## 设计决策（Design Decisions）

### D1: 顶层 `heap` crate 而非 `mm/heap/` 子目录

蓝图路径写作 `mm/heap/src/`，但 Cargo workspace 中新建顶层 `heap/` crate。理由：
- 与现有 workspace 模式一致（kernel/runtime/hal/mm 均为顶层 crate）
- 避免嵌套 Cargo.toml 的复杂性
- `mm` crate 专注页表/地址空间，`heap` crate 专注分配器，职责分离更清晰
- 蓝图的 `mm/heap/` 是概念分组（内存管理 / 堆子系统），非物理路径

### D2: `spin::Mutex` 包裹内部状态（蓝图 v1.1 修正）

`GlobalAlloc` trait 要求 `&self`，但分配器需内部可变。使用 `spin::Mutex<KernelHeapInner>` 而非 `UnsafeCell`。理由：
- 蓝图 v1.1 明确修正此点（原 v1.0 的 `DummyAlloc` 返回 null 不可接受）
- `spin::Mutex` 在 no_std 下提供自旋锁，单核安全
- v0.16.0 多核仍可用此锁（自旋锁天然多核安全，只是性能待优化）
- `KernelHeap` 为零字段占位类型，通过 `KERNEL_HEAP: Mutex<Option<KernelHeapInner>>` 访问状态

### D3: buddy 位图用 per-page bitmap

使用 `[u64; 128]`（8192 bit）的 per-page 位图：1 bit 对应 1 页（4KB），1=已分配，0=空闲。理由：
- 蓝图 §4.1 的 `BuddyAllocator` 结构体已含 `bitmap: [u64; 128]` 字段
- per-page bitmap 简单直观，`is_free(ptr, order)` 检查 `2^order` 个页位即可
- 8192 页 = 32MB 堆，满足 v0.10.0 需求（蓝图 §8.3 要求 ≥ 4MB）
- `remove_from_free` 遍历对应 order 的空闲链（链通常很短）

### D4: 测试用 `cfg_attr(not(test), no_std)` 模式

与 mm/hal crate 一致，使用 `#![cfg_attr(not(test), no_std)]`。理由：
- 测试构建链接 std，可使用 `#[test]` 宏和 std 测试框架
- `#[global_allocator]` 仅在 `#[cfg(not(test))]` 下注册（避免与 std 的分配器冲突）
- 单元测试直接调用 buddy/slab 方法，不经过 GlobalAlloc
- GlobalAlloc 实现仍被编译（类型检查），但不注册为全局分配器

### D5: 测试堆池用静态字节数组

测试中使用 `static mut HEAP_POOL: [u8; 1024 * 1024]`（1MB）作为堆池。理由：
- 匹配 no_std 场景（真实内核也用静态预留内存）
- 不依赖 std 的堆分配（避免递归）
- 1MB 足够覆盖分裂/合并/OOM 测试

### D6: Slab dealloc 需定位所属 bucket

`GlobalAlloc::dealloc` 接收 `Layout`（含 size），根据 size 选择 slab bucket。理由：
- `GlobalAlloc` 的 API 设计保证了 dealloc 时有 size 信息
- 无需元数据页记录 ptr 所属 slab（简化实现）
- 若 size 不匹配任何 bucket（>1024），走 buddy dealloc

### D7: 不实现 guard page / 多堆实例 / NUMA

蓝图 §9.3/9.7/8.4 提及 guard page、per-partition 多堆、NUMA 支持，但均不在 v0.10.0 交付物清单。理由：
- Karpathy 原则：不做未要求的复杂功能
- guard page 需页表支持（v0.8.0 mm crate），集成推迟
- 多堆实例需分区管理器（v0.9.0 Partition），集成推迟
- NUMA 需多核（v0.16.0），推迟

---

## 非目标（Non-Goals）

- **不实现** guard page（堆越界检测，需页表集成，推迟）
- **不实现** per-partition 多堆实例（推迟到后续版本）
- **不实现** NUMA per-node 堆（推迟到多核版本）
- **不实现**多核锁优化（v0.16.0 后补强，当前单核自旋锁足够）
- **不做** QEMU 运行时验证（延后到 kernel 集成）
- **不集成**到 mm crate 的页表池分配（未来版本迁移）
- **不做**纳秒级性能基准测试（设计目标 alloc < 200ns，但 CI 中不强制测量）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| buddy 合并错误导致内存泄漏 | 中/高 | 位图严格测试，覆盖分裂/合并/多重合并场景 |
| `GlobalAlloc` 的 `&self` 与可变性冲突 | 中/高 | `spin::Mutex<KernelHeapInner>`（蓝图 v1.1 修正方案） |
| slab bucket 满时 fallback 到 buddy | 低/低 | `SlabCache::alloc` 内部自动向 buddy 申请新页 |
| OOM 时 panic | 中/高 | `alloc` 返回 `null_mut()`，上层处理（蓝图 §4.4） |
| 测试中 `#[global_allocator]` 冲突 | 中/中 | `#[cfg(not(test))]` 门控注册 |
| `spin` crate 版本兼容 | 低/低 | 锁定 `spin = "0.9"`（稳定版本） |

---

## 性能目标（设计目标，非 CI 强制）

- 单次 alloc < 200ns（蓝图 §7.3）
- slab 命中率 ≥ 80%（蓝图 §7.3）
- 碎片率 < 15%（压测后，蓝图 §7.4）

**验证策略**：slab 命中率通过 `HeapStats.slab_hits / (slab_hits + buddy_hits)` 计算；碎片率通过 `HeapStats.fragmentation_ratio` 读取。纳秒级时序不在 CI 中测量（环境差异大），作为设计目标文档化。
