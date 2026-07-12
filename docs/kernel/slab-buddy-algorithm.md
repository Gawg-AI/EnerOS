# slab/buddy 算法说明

> **版本**：v0.10.0
> **crate**：`eneros-heap`
> **蓝图依据**：`蓝图/phase0.md` §v0.10.0
> **最后更新**：2026-07-12

---

## 1. Buddy 算法

### 1.1 基本原理

Buddy 系统将内存按 2 的幂次划分为块。每个阶（order）的块大小为 `PAGE_SIZE × 2^order`：

| Order | 块大小 | 页数 |
|-------|--------|------|
| 0 | 4 KB | 1 |
| 1 | 8 KB | 2 |
| 2 | 16 KB | 4 |
| 3 | 32 KB | 8 |
| 4 | 64 KB | 16 |
| 5 | 128 KB | 32 |
| 6 | 256 KB | 64 |
| 7 | 512 KB | 128 |
| 8 | 1 MB | 256 |
| 9 | 2 MB | 512 |
| 10 | 4 MB | 1024 |
| 11 | 8 MB | 2048（MAX_ORDER=11，但实际最大分配 4MB = order 10） |

### 1.2 分裂（Split）

当请求 order `n` 的块但只有 order `m`（m > n）的空闲块时，递归分裂：

```
分裂 order 3 块为 order 0:
  order 3 (32KB) → 拆为 2 个 order 2 (16KB)
    ├─ order 2 前半 → 保留，继续拆
    │   ├─ order 1 前半 → 保留，继续拆
    │   │   ├─ order 0 前半 → 返回给调用者
    │   │   └─ order 0 后半 → 加入 free_lists[0]
    │   └─ order 1 后半 → 加入 free_lists[1]
    └─ order 2 后半 → 加入 free_lists[2]
```

**实现**（`buddy.rs` alloc 方法）：

```rust
// 从 found_order 分裂到 order
let mut cur_order = found_order;
while cur_order > order {
    cur_order -= 1;
    let buddy = block.add(PAGE_SIZE << cur_order);
    self.push_free(buddy, cur_order);  // 后半加入空闲链
}
// block 指向前半，作为目标阶的块返回
```

### 1.3 合并（Merge）

释放块时，检查其 buddy（XOR 地址）是否空闲，若空闲则合并并递归向上：

**Buddy 地址计算**：给定块偏移 `offset` 和阶 `order`，buddy 偏移 = `offset XOR (PAGE_SIZE << order)`。

```
释放 page 0 (order 0):
  buddy = 0 XOR 4096 = 4096 (page 1)
  若 page 1 空闲 → 合并为 order 1 块 [page 0-1]
    buddy = 0 XOR 8192 = 8192 (page 2-3)
    若 page 2-3 空闲 → 合并为 order 2 块 [page 0-3]
      buddy = 0 XOR 16384 = 16384 (page 4-7)
      ...递归直到 buddy 不空闲或达到 MAX_ORDER
```

**实现**（`buddy.rs` dealloc 方法）：

```rust
while order < MAX_ORDER {
    let buddy_offset = page_offset ^ (PAGE_SIZE << order);
    let buddy = self.base.add(buddy_offset);
    if !self.is_free(buddy, order) { break; }
    self.remove_from_free(buddy, order);
    if (buddy as usize) < (block as usize) {
        block = buddy;
        page_offset = buddy_offset;
    }
    order += 1;
}
self.push_free(block, order);
```

### 1.4 位图检测

`is_free(ptr, order)` 通过 per-page 位图检查块范围内所有页是否空闲：

```rust
unsafe fn is_free(&self, ptr: *mut u8, order: usize) -> bool {
    let page_idx = ((ptr as usize) - (self.base as usize)) / PAGE_SIZE;
    self.is_range_free(page_idx, 1usize << order)
}
```

位图操作：
- `set_allocated(page_idx)`: `bitmap[page_idx / 64] |= 1 << (page_idx % 64)`
- `clear_allocated(page_idx)`: `bitmap[page_idx / 64] &= !(1 << (page_idx % 64))`
- `is_page_free(page_idx)`: `(bitmap[page_idx / 64] & (1 << (page_idx % 64))) == 0`

---

## 2. Slab 算法

### 2.1 基本原理

Slab 分配器为固定大小对象维护空闲链池，分配/释放均为 O(1)：

```
SlabCache (obj_size=64)
  ┌─────────────────────────────────────────┐
  │ Page (4KB) 切分为 64 个槽                │
  │  slot[0] → slot[1] → ... → slot[63] → None │
  │  ↑ free_head                              │
  └─────────────────────────────────────────┘
```

### 2.2 分配流程

1. **快速路径**：若 `free_head` 非空，取链头槽，`free_head` 更新为 next 指针
2. **慢速路径**：若 `free_head` 为空，向 buddy 申请 1 页，切分为 `PAGE_SIZE / obj_size` 个槽，建立空闲链，再走快速路径

```rust
pub unsafe fn alloc(&mut self, buddy: &mut BuddyAllocator) -> *mut u8 {
    if let Some(slot) = self.free_head {
        self.free_head = *(slot.cast::<Option<*mut u8>>());
        self.used += 1;
        return slot;
    }
    let page = buddy.alloc(PAGE_SIZE);
    if page.is_null() { return ptr::null_mut(); }
    let slots = PAGE_SIZE / self.obj_size;
    for i in 0..slots {
        let slot = page.add(i * self.obj_size);
        let next = if i + 1 < slots { Some(page.add((i + 1) * self.obj_size)) } else { None };
        *(slot.cast::<Option<*mut u8>>()) = next;
    }
    self.free_head = Some(page);
    self.total += slots;
    self.alloc(buddy)  // 递归走快速路径
}
```

### 2.3 释放流程

将槽归还到空闲链头：

```rust
pub unsafe fn dealloc(&mut self, ptr: *mut u8) {
    *(ptr.cast::<Option<*mut u8>>()) = self.free_head;
    self.free_head = Some(ptr);
    self.used = self.used.saturating_sub(1);
}
```

### 2.4 Bucket 路由

`GlobalAlloc` 根据 `layout.size()` 选择最小能容纳的 bucket：

```rust
fn slab_bucket_for(size: usize) -> Option<usize> {
    SLAB_SIZES.iter().position(|&s| size <= s)
}
```

| 请求大小 | Bucket | obj_size |
|----------|--------|----------|
| 1–8 | 0 | 8 |
| 9–16 | 1 | 16 |
| 17–32 | 2 | 32 |
| 33–64 | 3 | 64 |
| 65–128 | 4 | 128 |
| 129–256 | 5 | 256 |
| 257–512 | 6 | 512 |
| 513–1024 | 7 | 1024 |
| > 1024 | None | 走 buddy |

---

## 3. 碎片分析

### 3.1 内部碎片

由向上取整产生：
- Slab：请求 65 字节 → 分配 128 字节槽，内部碎片 = 63 字节（49%）
- Buddy：请求 4097 字节 → 分配 8192 字节块，内部碎片 = 4095 字节（50%）

**缓解**：8 个 slab bucket 覆盖 8–1024 字节范围，减少小对象的内部碎片。

### 3.2 外部碎片

由空闲块不连续产生：
- Buddy 合并机制最大化大块可用性
- Slab 槽位在页内紧凑排列，无页内外部碎片

**碎片率公式**：`(free_bytes - largest_free_block) × 1000 / free_bytes`

- 0 = 无碎片（所有空闲内存在一个连续块中）
- 1000 = 完全碎片化（无大块可用）

### 3.3 性能目标

| 指标 | 目标 | 验证方式 |
|------|------|----------|
| 单次 alloc | < 200ns | 设计目标，CI 不强制测量 |
| slab 命中率 | ≥ 80% | `HeapStats.slab_hits / (slab_hits + buddy_hits)` |
| 碎片率 | < 15% | `HeapStats.fragmentation_ratio < 150` |

---

## 4. 侵入式空闲链

### 4.1 设计

Buddy 和 Slab 均使用侵入式空闲链：利用空闲块自身的内存存储 next 指针，无需额外元数据。

```
空闲块布局:
  ┌──────────────────┬─────────────────────┐
  │ next: *mut u8    │ unused space        │
  │ (8 bytes)        │                     │
  └──────────────────┴─────────────────────┘
```

### 4.2 约束

- 最小块大小 ≥ 8 字节（64 位指针大小）
- Buddy 最小块 = PAGE_SIZE (4096 字节) ✓
- Slab 最小 obj_size = 8 字节 ✓

### 4.3 优势

- 零额外内存开销
- 无需元数据页
- 分配/释放只需指针操作

---

## 5. 实现细节

### 5.1 pop_free 防御性编程

`pop_free` 方法包含防御性 null 检查，防止编译器优化导致的 UB：

```rust
unsafe fn pop_free(&mut self, order: usize) -> Option<*mut u8> {
    let head = self.free_lists[order]?;
    if head.is_null() {  // 防御性检查
        self.free_lists[order] = None;
        return None;
    }
    let next = *(head.cast::<Option<*mut u8>>());
    self.free_lists[order] = next;
    self.free_count[order] -= 1;
    Some(head)
}
```

### 5.2 alloc 中的 pop 循环

`alloc` 方法直接在循环中调用 `pop_free`，避免 `is_some()` + `pop()` 分离导致的 read-check-then-pop race：

```rust
// 正确：直接循环 pop
for o in order..=MAX_ORDER {
    if let Some(b) = self.pop_free(o) {
        block = b;
        found_order = o;
        break;
    }
}
```

### 5.3 Send/Sync 实现

`KernelHeapInner` 含裸指针，需手动实现 `Send + Sync`：

```rust
unsafe impl Send for KernelHeapInner {}
unsafe impl Sync for KernelHeapInner {}
```

安全性保证：裸指针仅在 `KERNEL_HEAP` 的 `Mutex` 保护下访问，保证独占访问。

---

## 6. 与纯 buddy / 纯 slab 的对比

| 维度 | slab+buddy | 纯 buddy | 纯 slab |
|------|-----------|----------|---------|
| 小对象速度 | 快（O(1)） | 慢（分裂） | 快 |
| 大块支持 | 好（buddy 兜底） | 好 | 差 |
| 碎片控制 | 好（slab 定长） | 中 | 差 |
| 复杂度 | 中 | 低 | 低 |
| 内存利用率 | 高 | 中 | 低（大对象浪费） |

选择 slab+buddy 混合：兼顾小对象速度和大块支持，碎片可控。
