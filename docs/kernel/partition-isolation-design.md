# 分区隔离设计

> **版本**：EnerOS v0.9.0 — 分区内存隔离验证
> **模块**：`mm/src/partition.rs`
> **最后更新**：2026-07-12

---

## 1. 概述

### 1.1 目的

EnerOS 采用混合关键性架构（Mixed-Criticality System），在同一硬件平台上同时承载安全关键任务（如保护测控、协议栈）与应用层任务（如 Agent、LLM 推理）。不同关键性等级的任务对内存隔离有刚性需求：

- **安全分区**必须保证其物理内存不被其他分区越权读写，否则可能导致保护逻辑被篡改。
- **应用分区**运行较大但关键性较低的代码（AI 推理、协议解析），其内存访问错误不应波及安全分区。
- **故障隔离**：一个分区发生内存越界或泄漏，不应影响其他分区的正常运行。

### 1.2 原理

物理内存分区隔离（Physical Memory Partitioning）通过为每个分区划定一组**允许访问的物理地址区间**（`allowed_phys`），并在每次内存访问前进行两步检查：

1. **区间检查**：目标地址必须完全落在该分区授权的物理区间内。
2. **配额检查**：本次分配不得使分区已用内存超过其配额上限。

任何一步失败即拒绝访问，从而在物理地址层面实现分区间的强隔离。这与仅靠虚拟地址空间（`Vspace`）隔离不同：即使两个分区的虚拟地址相同，它们映射到的物理页也必须各自独立、互不重叠。

### 1.3 安全需求

| 需求 | 说明 |
|------|------|
| 物理隔离 | 不同分区的 `allowed_phys` 区间不得重叠 |
| 配额限制 | 单分区内存使用不得超过其 `quota` |
| 访问检查 | 所有可能跨分区的物理访问都须经过 `check_access` |
| 可验证性 | 隔离关系应可通过 `is_isolated_from` 静态验证 |

---

## 2. 核心数据结构

### 2.1 PaddrRange —— 物理地址区间

`PaddrRange` 表示一个**半开区间** `[start, end)`，即 `start` 包含、`end` 不包含。

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaddrRange {
    pub start: u64,
    pub end: u64,
}
```

| 方法 | 语义 |
|------|------|
| `new(start, end)` | 构造区间 `[start, end)` |
| `contains(pa)` | `pa >= start && pa < end`，判断点是否落在区间内 |
| `overlaps(other)` | `self.start < other.end && other.start < self.end`，判断两区间是否重叠 |
| `is_empty()` | `start >= end`，空区间（包括 `{0, 0}`） |

**半开区间的关键性质**：相邻区间 `[0x1000, 0x2000)` 与 `[0x2000, 0x3000)` **不重叠**，这正是物理内存连续分区的期望行为。

### 2.2 Partition —— 内存分区

```rust
pub struct Partition {
    pub id: u32,                              // 分区 ID
    pub name: &'static str,                  // 可读名称
    pub vspace: Vspace,                       // 分区虚拟地址空间
    pub allowed_phys: [PaddrRange; 8],        // 允许访问的物理区间（固定 8 槽）
    pub quota: u64,                           // 内存配额上限（字节）
    pub used: u64,                            // 当前已用内存（字节）
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `u32` | 分区唯一标识，用于 DMA 域归属与日志 |
| `name` | `&'static str` | 人类可读名称，如 `"safety"`、`"app"` |
| `vspace` | `Vspace` | 该分区独立的虚拟地址空间（独立页表根） |
| `allowed_phys` | `[PaddrRange; 8]` | 最多 8 个授权物理区间，空槽用 `{0, 0}` 表示 |
| `quota` | `u64` | 该分区物理内存使用上限 |
| `used` | `u64` | 该分区当前已分配字节数 |

`MAX_PHYS_RANGES` 常量固定为 8，满足 RTOS 场景下分区物理区间数量需求，同时避免动态分配。

---

## 3. 物理区间授权

### 3.1 授权数组

`allowed_phys` 是固定大小的 8 槽数组，**不使用动态分配**（符合 no_std 约束）。空槽以 `PaddrRange { start: 0, end: 0 }` 表示，因为 `is_empty()` 判定为 `start >= end`，`{0, 0}` 满足该条件。

```rust
const MAX_PHYS_RANGES: usize = 8;
// 初始化：所有槽均为空
allowed_phys: [PaddrRange::new(0, 0); MAX_PHYS_RANGES],
```

### 3.2 add_phys_range 流程

`add_phys_range(range)` 线性扫描 `allowed_phys`，将新区间写入**首个空槽**：

```rust
pub fn add_phys_range(&mut self, range: PaddrRange) -> Result<(), MmError> {
    for slot in self.allowed_phys.iter_mut() {
        if slot.is_empty() {
            *slot = range;
            return Ok(());
        }
    }
    Err(MmError::OutOfMemory)
}
```

- **成功**：找到空槽，写入区间，返回 `Ok(())`。
- **失败**：8 个槽均已使用，返回 `Err(MmError::OutOfMemory)`。

> **注意**：本版本不做新区间与已有区间的重叠检查，调用方须保证授权区间互不重叠（可通过 `is_isolated_from` 验证）。

---

## 4. 访问检查流程

### 4.1 check_access 两步检查

`check_access(pa, size)` 验证区间 `[pa, pa+size)` 是否可被本分区访问：

```rust
pub fn check_access(&self, pa: u64, size: u64) -> Result<(), MmError> {
    let end = pa.checked_add(size).ok_or(MmError::InvalidAddr)?;

    // 步骤 1：区间检查
    let mut found = false;
    for r in self.allowed_phys.iter() {
        if r.is_empty() { continue; }
        if pa >= r.start && end <= r.end {
            found = true;
            break;
        }
    }
    if !found {
        return Err(MmError::PermissionDenied);
    }

    // 步骤 2：配额检查
    if self.used + size > self.quota {
        return Err(MmError::OutOfMemory);
    }

    Ok(())
}
```

### 4.2 检查步骤说明

| 步骤 | 检查内容 | 失败返回 |
|------|---------|---------|
| 前置 | `pa + size` 是否溢出（`checked_add`） | `MmError::InvalidAddr` |
| 1 | `[pa, pa+size)` 是否完全包含在某个 `allowed_phys` 区间内 | `MmError::PermissionDenied` |
| 2 | `used + size <= quota` 是否成立 | `MmError::OutOfMemory` |

**完全包含**的含义：`pa >= r.start && end <= r.end`，即请求区间不能跨越授权区间边界，也不能部分落在授权区间外。

### 4.3 流程图

```
            ┌──────────────────────────┐
            │ check_access(pa, size)   │
            └─────────────┬────────────┘
                          │
                          ▼
            ┌──────────────────────────┐
            │ end = pa + size          │
            │ (checked_add 防溢出)      │
            └─────────────┬────────────┘
                          │ 溢出?
                 ┌────────┴────────┐
                 │ 是              │ 否
                 ▼                 ▼
        ┌────────────────┐  ┌─────────────────────┐
        │ InvalidAddr    │  │ 遍历 allowed_phys   │
        └────────────────┘  │ 跳过空槽            │
                            └──────────┬──────────┘
                                       │
                                       ▼
                            ┌─────────────────────┐
                            │ 存在 r 使得         │
                            │ pa>=r.start 且      │
                            │ end<=r.end ?        │
                            └──────────┬──────────┘
                       ┌───────────────┴───────────────┐
                       │ 否                            │ 是
                       ▼                               ▼
              ┌────────────────┐          ┌─────────────────────┐
              │ PermissionDenied│         │ used + size <= quota?│
              └────────────────┘          └──────────┬──────────┘
                                     ┌───────────────┴───────────────┐
                                     │ 否                            │ 是
                                     ▼                               ▼
                            ┌────────────────┐          ┌────────────┐
                            │ OutOfMemory    │          │ Ok(())     │
                            └────────────────┘          └────────────┘
```

---

## 5. 隔离判断逻辑

### 5.1 is_isolated_from

`is_isolated_from(other)` 判断本分区与 `other` 分区的物理区间是否完全不重叠：

```rust
pub fn is_isolated_from(&self, other: &Partition) -> bool {
    for a in self.allowed_phys.iter() {
        if a.is_empty() { continue; }
        for b in other.allowed_phys.iter() {
            if b.is_empty() { continue; }
            if a.overlaps(b) {
                return false;   // 任意一对重叠 → 不隔离
            }
        }
    }
    true
}
```

### 5.2 区间重叠判断公式

两个半开区间 `[a.start, a.end)` 与 `[b.start, b.end)` 重叠的充要条件：

```
a.start < b.end  &&  b.start < a.end
```

**正确性分析**：
- 若 `a.start >= b.end`：a 完全在 b 右侧（或恰好相邻），不重叠。
- 若 `b.start >= a.end`：b 完全在 a 右侧（或恰好相邻），不重叠。
- 否则两区间在数轴上有公共部分，重叠。

**相邻不重叠**：`[0x1000, 0x2000)` 与 `[0x2000, 0x3000)` 满足 `a.start(0x1000) < b.end(0x3000)` 但 `b.start(0x2000) < a.end(0x2000)` 为假（`0x2000 < 0x2000` 为假），故不重叠。

### 5.3 示例

**示例 1：重叠 → 不隔离**

```
分区 A: allowed_phys = [ [0x1000, 0x3000) ]
分区 B: allowed_phys = [ [0x2000, 0x4000) ]

数轴：
A: [0x1000 ████████ 0x3000)
B:           [0x2000 ████████ 0x4000)
             └── 重叠区域 [0x2000, 0x3000) ──┘

判断：a.start(0x1000) < b.end(0x4000) && b.start(0x2000) < a.end(0x3000)
     → true && true → 重叠 → is_isolated_from 返回 false
```

**示例 2：不重叠 → 隔离**

```
分区 A: allowed_phys = [ [0x1000, 0x2000) ]
分区 B: allowed_phys = [ [0x3000, 0x4000) ]

数轴：
A: [0x1000 ████ 0x2000)
B:                   [0x3000 ████ 0x4000)
                    └── 无公共部分 ──┘

判断：a.start(0x1000) < b.end(0x4000) && b.start(0x3000) < a.end(0x2000)
     → true && false → 不重叠 → is_isolated_from 返回 true
```

---

## 6. 配额管理

### 6.1 quota / used 机制

每个分区维护两个字段：

- `quota`：该分区物理内存使用上限（字节），在 `Partition::new` 时设定。
- `used`：该分区当前已分配字节数，初始为 0。

所有分配/访问检查都基于 `used + size <= quota` 这一不变式。

### 6.2 alloc_phys —— Bump 分配器

`alloc_phys(size)` 从**首个非空 allowed_phys 区间**进行 bump 分配：

```rust
pub fn alloc_phys(&mut self, size: u64) -> Result<u64, MmError> {
    if self.used + size > self.quota {
        return Err(MmError::OutOfMemory);
    }
    for r in self.allowed_phys.iter() {
        if r.is_empty() { continue; }
        let pa = r.start + self.used;   // 从区间起始 + 已用偏移
        self.used += size;
        return Ok(pa);
    }
    Err(MmError::OutOfMemory)
}
```

**特点**：
- 只从第一个非空区间分配，不跨区间。
- 分配地址 = `r.start + 旧 used`，然后 `used += size`。
- 不回收：bump 分配器只前进不回退。
- 配额耗尽返回 `OutOfMemory`。

### 6.3 free_phys —— 记账式释放

`free_phys(pa, size)` **仅递减 `used`**，不真正回收物理页：

```rust
pub fn free_phys(&mut self, _pa: u64, size: u64) {
    if size <= self.used {
        self.used -= size;
    }
}
```

- `_pa` 参数被忽略（下划线前缀），仅用于未来接口兼容。
- `size > used` 时静默不操作，防止下溢。
- **这是 v0.9.0 的临时实现**，v0.10.0 将由 buddy 分配器替换，实现真正的物理页回收。

### 6.4 设计取舍

| 项 | 当前 (v0.9.0) | 未来 (v0.10.0+) buddy |
|----|--------------|----------------------|
| 分配算法 | bump（线性前进） | buddy（按幂次分裂/合并） |
| 释放 | 仅记账，不回收 | 真正回收并合并 |
| 碎片 | 无外部碎片，但无法重用 | 可重用，需管理碎片 |
| 复杂度 | O(1) | O(log n) |

---

## 7. QEMU virt 双分区示例配置

### 7.1 配置表

QEMU `virt` 平台（aarch64）物理内存基址 `0x40000000`。以下为安全分区 A 与应用分区 B 的双分区配置：

| 分区 | ID | 名称 | 物理区间 | 配额 |
|------|----|----|---------|------|
| A | 1 | `safety` | `[0x40000000, 0x40100000)` | 1 MB |
| B | 2 | `app` | `[0x40200000, 0x40400000)` | 2 MB |

### 7.2 隔离说明

- 两分区物理区间不重叠：`[0x40000000, 0x40100000)` 与 `[0x40200000, 0x40400000)` 之间有 `0x40100000~0x40200000` 共 1MB 间隙。
- `A.is_isolated_from(&B)` 返回 `true`。
- 分区 A 尝试访问 `0x40200000`（B 的内存）时：
  - `check_access(0x40200000, 0x1000)` 遍历 A 的 `allowed_phys`，`[0x40000000, 0x40100000)` 不包含 `0x40200000`，返回 `MmError::PermissionDenied`。

### 7.3 配置代码示意

```rust
use mm::partition::{Partition, PaddrRange};
use mm::vspace::Vspace;

// 分区 A：安全关键
let mut part_a = Partition::new(
    1, "safety",
    Vspace::new(0x40000000, 1),
    0x100000,   // quota: 1MB
);
part_a.add_phys_range(PaddrRange::new(0x40000000, 0x40100000)).unwrap();

// 分区 B：应用层
let mut part_b = Partition::new(
    2, "app",
    Vspace::new(0x40200000, 2),
    0x200000,   // quota: 2MB
);
part_b.add_phys_range(PaddrRange::new(0x40200000, 0x40400000)).unwrap();

// 验证隔离
assert!(part_a.is_isolated_from(&part_b));
assert!(part_b.is_isolated_from(&part_a));

// 验证跨分区访问被拒
assert_eq!(
    part_a.check_access(0x40200000, 0x1000),
    Err(MmError::PermissionDenied)
);
```

---

## 8. 错误处理

### 8.1 MmError 相关变体

`MmError` 定义于 `mm/src/vspace.rs`，v0.9.0 新增 `PermissionDenied` 变体：

| 变体 | 触发场景 | Display |
|------|---------|---------|
| `PermissionDenied` | 跨分区访问：目标地址不在本分区 `allowed_phys` 内 | `permission denied (cross-partition access)` |
| `OutOfMemory` | 配额耗尽：`used + size > quota`；或 `allowed_phys` 8 槽已满 | `out of memory (page table pool exhausted)` |
| `InvalidAddr` | 地址溢出：`pa + size` 超过 `u64` 范围（`checked_add` 返回 `None`） | `invalid address` |

### 8.2 错误处理策略

| 场景 | 返回错误 | 调用方处理建议 |
|------|---------|--------------|
| 分区 A 访问分区 B 的物理页 | `PermissionDenied` | 终止访问，记录安全日志 |
| 分区内分配超过 `quota` | `OutOfMemory` | 触发分区级 OOM 策略（回收或终止任务） |
| `add_phys_range` 时 8 槽已满 | `OutOfMemory` | 扩展为动态数组（未来版本）或合并区间 |
| `pa + size` 溢出 | `InvalidAddr` | 拒绝请求，视为非法参数 |

### 8.3 Display 实现

```rust
impl fmt::Display for MmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MmError::PermissionDenied => write!(f, "permission denied (cross-partition access)"),
            MmError::OutOfMemory => write!(f, "out of memory (page table pool exhausted)"),
            MmError::InvalidAddr => write!(f, "invalid address"),
            // ... 其他变体
        }
    }
}
```

---

## 9. 未来扩展

| 版本 | 扩展内容 | 说明 |
|------|---------|------|
| v0.10.0 | buddy 分配器 | 替换 `alloc_phys`/`free_phys` 的 bump+记账实现，支持物理页真正回收与合并，引入碎片统计接口 |
| v0.21.0 | 共享内存跨分区授权 | 允许两个分区显式共享某段物理内存（用于零拷贝通信），通过 `SharedRegion` 抽象管理共享授权与引用计数 |
| v0.22.0+ | 动态分区创建/销毁 | 运行时创建/销毁分区，回收其物理区间并重新分配；配合 SMMU 硬件实现 DMA 域动态绑定 |
| 后续 | 分区级 OOM 回调 | 每个分区注册 OOM 回调函数，配额耗尽时触发分区自定义策略（而非全局终止） |
| 后续 | 物理区间重叠检测 | `add_phys_range` 时自动检测与已有区间重叠，拒绝非法授权 |

---

## 10. 参考资料

- 源码：`mm/src/partition.rs`
- 错误码：`mm/src/vspace.rs` (`MmError`)
- 蓝图：`蓝图/phase0.md`（Phase 0 内存管理）
- 路线图：`蓝图/Power_Native_Agent_OS_Version_Roadmap_v3.md`（v0.9.0、v0.10.0、v0.21.0）
