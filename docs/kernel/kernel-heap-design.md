# EnerOS 内核堆分配器设计

> **版本**：v0.10.0
> **crate**：`eneros-heap`
> **蓝图依据**：`蓝图/phase0.md` §v0.10.0
> **最后更新**：2026-07-12

---

## 1. 架构概述

EnerOS 内核堆采用 **slab + buddy 混合算法**，为 no_std 内核态提供动态内存管理：

- **Slab 分配器**：处理小对象（8–1024 字节），O(1) 空闲链分配，避免 buddy 频繁分裂。
- **Buddy 分配器**：处理大块（> 1024 字节，页级），支持块分裂与合并，减少外部碎片。

分配路由策略：

```
alloc(layout)
  ├── size ≤ 1024 → SlabCache[size 向上取整到 bucket].alloc()
  └── size > 1024 → BuddyAllocator.alloc()
```

---

## 2. 数据结构

### 2.1 BuddyAllocator（`heap/src/buddy.rs`）

```rust
pub struct BuddyAllocator {
    pub base: *mut u8,                              // 堆池基址
    pub total_pages: usize,                         // 总页数
    pub free_lists: [Option<*mut u8>; MAX_ORDER + 1], // 12 阶空闲链
    pub free_count: [usize; MAX_ORDER + 1],         // 各阶空闲块计数
    bitmap: [u64; 128],                             // per-page 位图 (8192 bit)
}
```

| 常量 | 值 | 说明 |
|------|-----|------|
| `PAGE_SIZE` | 4096 | 页大小（4KB） |
| `MAX_ORDER` | 11 | 最大阶（块大小 = 4KB × 2^11 = 4MB） |
| `BITMAP_WORDS` | 128 | 位图字数（128 × 64 = 8192 bit，支持 32MB 堆） |

**侵入式空闲链**：每个空闲块的头部 8 字节存储 `Option<*mut u8>` 指向下一个空闲块，`None` 表示链尾。

**per-page 位图**：1 bit 对应 1 页（4KB），1 = 已分配，0 = 空闲。用于 `is_free()` 检测 buddy 块是否可合并。

### 2.2 SlabCache（`heap/src/slab.rs`）

```rust
pub struct SlabCache {
    pub obj_size: usize,            // 对象大小
    pub free_head: Option<*mut u8>, // 空闲链头
    pub total: usize,               // 总槽位数
    pub used: usize,                // 已用槽位数
}
```

**8 个 bucket**：

| 索引 | obj_size | 每页槽位数 (PAGE_SIZE / obj_size) |
|------|----------|----------------------------------|
| 0 | 8 | 512 |
| 1 | 16 | 256 |
| 2 | 32 | 128 |
| 3 | 64 | 64 |
| 4 | 128 | 32 |
| 5 | 256 | 16 |
| 6 | 512 | 8 |
| 7 | 1024 | 4 |

**侵入式空闲链**：与 buddy 类似，每个空闲槽头部 8 字节存储 next 指针。所有 bucket 大小 ≥ 8 字节，保证能容纳 64 位指针。

### 2.3 KernelHeap（`heap/src/lib.rs`）

```rust
pub struct KernelHeap;  // 零字段占位类型

static KERNEL_HEAP: Mutex<Option<KernelHeapInner>> = Mutex::new(None);

pub struct KernelHeapInner {
    pub buddy: BuddyAllocator,
    pub slabs: [SlabCache; 8],
    pub stats: HeapStats,
}
```

`KernelHeap` 是零字段类型，实现 `GlobalAlloc` trait。实际状态存储在 `KERNEL_HEAP` 全局静态变量中，用 `spin::Mutex` 保护。

### 2.4 HeapStats（`heap/src/stats.rs`）

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

碎片率公式：`(free_bytes - largest_free_block) × 1000 / free_bytes`

---

## 3. 分配/释放流程

### 3.1 alloc 流程

```
GlobalAlloc::alloc(&self, layout)
  │
  ├─ lock KERNEL_HEAP
  ├─ stats.alloc_count += 1
  ├─ stats.allocated_bytes += size
  │
  ├─ if size ≤ 1024:
  │    ├─ stats.slab_hits += 1
  │    ├─ bucket = slab_bucket_for(size)  // 最小能容纳的 bucket
  │    └─ slabs[bucket].alloc(&mut buddy)
  │         ├─ if free_head 非空: 取链头槽，used += 1
  │         └─ else: buddy.alloc(PAGE_SIZE) → 切分为 slots → 建链 → 取首槽
  │
  └─ else:
       ├─ stats.buddy_hits += 1
       └─ buddy.alloc(size)
            ├─ order = order_for(size)
            ├─ 从 order 到 MAX_ORDER 找空闲块
            ├─ 分裂到目标 order（剩余块加入低阶空闲链）
            └─ 位图标记已分配
```

### 3.2 dealloc 流程

```
GlobalAlloc::dealloc(&self, ptr, layout)
  │
  ├─ lock KERNEL_HEAP
  ├─ stats.free_count += 1
  ├─ stats.allocated_bytes -= size
  │
  ├─ if size ≤ 1024:
  │    └─ slabs[bucket].dealloc(ptr)  // ptr 加入空闲链头
  │
  └─ else:
       └─ buddy.dealloc(ptr, size)
            ├─ order = order_for(size)
            ├─ 位图清除已分配
            ├─ 合并循环:
            │    ├─ buddy_addr = ptr XOR (PAGE_SIZE << order)
            │    ├─ if is_free(buddy, order):
            │    │    ├─ remove_from_free(buddy, order)
            │    │    ├─ block = min(block, buddy)
            │    │    └─ order += 1
            │    └─ else: break
            └─ push_free(block, order)
```

---

## 4. 初始化序列

```rust
// 1. 提供页对齐的堆池（真实内核中由引导加载器预留）
static mut HEAP_POOL: [u8; 4 * 1024 * 1024] = [0; 4 * 1024 * 1024];

// 2. 初始化堆
unsafe { heap_init(HEAP_POOL.as_mut_ptr(), 4 * 1024 * 1024); }

// 3. 此后 alloc::vec::Vec, alloc::boxed::Box 等可正常使用
let v: alloc::vec::Vec<u8> = alloc::vec![1, 2, 3];
```

`heap_init` 内部：
1. 计算 pages = size / PAGE_SIZE
2. 创建 `BuddyAllocator`，调用 `init(base, pages)` 将所有页放入最大阶空闲链
3. 创建 8 个 `SlabCache`（SLAB_SIZES = [8, 16, 32, 64, 128, 256, 512, 1024]）
4. 初始化 `HeapStats`（total_bytes = size, free_bytes = size）
5. 用 `Mutex::lock()` 存入 `KERNEL_HEAP`

---

## 5. GlobalAlloc 集成

### 5.1 &self 与可变性

`GlobalAlloc` trait 要求 `&self`，但分配器需要内部可变状态。解决方案：

- `KernelHeap` 为零字段占位类型
- 实际状态存于 `static KERNEL_HEAP: Mutex<Option<KernelHeapInner>>`
- `GlobalAlloc::alloc/dealloc` 通过 `KERNEL_HEAP.lock()` 获取可变引用

### 5.2 全局分配器注册

```rust
#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: KernelHeap = KernelHeap;
```

仅在非测试构建时注册。测试构建链接 std，使用 std 自带的分配器，避免冲突。

### 5.3 测试策略

- `#![cfg_attr(not(test), no_std)]`：正式构建 no_std，测试构建链接 std
- buddy/slab 单元测试使用 local 实例，不走 `KERNEL_HEAP`
- GlobalAlloc 集成测试通过 `GlobalAlloc::alloc(&heap, layout)` 直接调用
- 测试堆池使用 `#[repr(C, align(4096))]` 的静态数组保证页对齐

---

## 6. OOM 处理

- `BuddyAllocator::alloc` 在无可用块时返回 `core::ptr::null_mut()`
- `SlabCache::alloc` 在 buddy 返回 null 时传递 null
- `GlobalAlloc::alloc` 在 `KERNEL_HEAP` 未初始化或底层返回 null 时返回 null
- **不 panic**：上层调用者负责处理 null 返回值

---

## 7. 并发安全

- `KERNEL_HEAP` 使用 `spin::Mutex` 保护，单核自旋锁
- v0.10.0 为单核版本，自旋锁足够
- v0.16.0 多核版本可直接复用此锁（自旋锁天然多核安全，性能待优化）
- `KernelHeapInner` 实现 `unsafe impl Send + Sync`（裸指针在 Mutex 保护下安全）

---

## 8. 文件结构

```
heap/
├── Cargo.toml          # crate 配置，依赖 spin = "0.9"
└── src/
    ├── lib.rs          # KernelHeap, GlobalAlloc, heap_init, heap_stats
    ├── buddy.rs        # BuddyAllocator 页级分配器
    ├── slab.rs         # SlabCache 小对象池
    └── stats.rs        # HeapStats 碎片统计
```

---

## 9. 与其他模块的关系

| 方向 | 模块 | 关系 |
|------|------|------|
| 上游 | 引导加载器 | 提供页对齐堆池内存 |
| 下游 | v0.11.0 用户态堆 | 复用本版本算法 |
| 下游 | v0.18.0 TCB | 依赖本堆分配 TCB |
| 未来 | mm crate 页表池 | 当前用静态数组，未来迁移到堆 |
