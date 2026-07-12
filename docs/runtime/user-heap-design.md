# EnerOS 用户态堆分配器设计

> **版本**：v0.11.0
> **crate**：`eneros-user-heap`
> **蓝图依据**：`蓝图/phase0.md` §v0.11.0
> **前序依赖**：v0.10.0（`eneros-heap` 的 `BuddyAllocator`）
> **最后更新**：2026-07-12

---

## 1. 架构概述

EnerOS 用户态堆分配器为 seL4 用户态进程提供动态内存管理，复用 v0.10.0 内核堆的
`BuddyAllocator`，并在其之上增加两项关键能力：

- **配额（Quota）**：限制单分区可用内存上限，防止用户进程耗尽系统内存
- **OOM 处理器（OomHandler）**：可定制的内存耗尽策略，默认 `panic!`，可替换为
  日志、回滚、强制终止等策略

分配路由策略：

```
UserHeap::alloc(layout)
  ├── 堆未初始化       → 返回 null_mut（不 panic）
  ├── 配额超限          → 调用 OomHandler
  ├── BuddyAllocator.alloc() 失败 → 调用 OomHandler
  └── 成功              → 更新 quota.used，返回指针
```

与内核堆（v0.10.0）的关键差异：

| 特性 | 内核堆 (`eneros-heap`) | 用户态堆 (`eneros-user-heap`) |
|------|------------------------|-------------------------------|
| 算法 | slab + buddy 混合 | 仅 buddy |
| 配额 | 无 | 有（`Quota`） |
| OOM 策略 | 返回 null，调用方处理 | `OomHandler` 可定制 |
| `#[global_allocator]` 注册 | 库不注册，由二进制注册 | `#[cfg(not(test))]` 下注册 |
| 使用场景 | 内核态、调度器、驱动 | seL4 用户态进程 |

---

## 2. 与内核堆的关系

### 2.1 复用而非重复实现

`eneros-user-heap` 通过 workspace 路径依赖 `eneros-heap`，直接使用其
`BuddyAllocator`：

```rust
use eneros_heap::buddy::{BuddyAllocator, PAGE_SIZE};
```

设计决策（spec D2）：**只复用 buddy，不复用 slab**。原因：

1. 用户态进程的小对象分配模式与内核不同（无大量 8/16 字节对象）
2. 简化配额跟踪——只需在 buddy 层统计 `used`，避免 slab 缓存导致的配额偏差
3. 减少代码复杂度，符合蓝图 §43.2 非瓶颈版本"简洁优先"原则

### 2.2 `#[global_allocator]` 注册策略

v0.10.0 的 `eneros-heap` 作为库 crate **不注册** `#[global_allocator]`，
由消费方二进制 crate 注册。这是 Rust 的标准模式，避免 workspace 内多个 crate
同时注册导致的冲突。

`eneros-user-heap` 在 `#[cfg(not(test))]` 下注册自己的 `UserHeap`：

```rust
#[cfg(not(test))]
#[global_allocator]
static ALLOC: UserHeap = UserHeap::new();
```

测试构建下，`std` 提供自己的分配器，使 `Vec`、`String` 等在测试中正常工作。

---

## 3. 数据结构

### 3.1 UserHeapInner（`user/heap/src/lib.rs`）

```rust
pub struct UserHeapInner {
    pub buddy: BuddyAllocator,    // 复用 v0.10.0 的 buddy 分配器
    pub quota: Quota,             // 配额跟踪
    pub oom_handler: OomHandler,  // OOM 处理器
}
```

所有可变状态封装在 `USER_HEAP: Mutex<Option<UserHeapInner>>` 中。`Option` 表示
堆可能尚未初始化——此时 `alloc` 返回 `null_mut`，`dealloc` 静默返回。

### 3.2 UserHeap（零字段占位类型）

```rust
pub struct UserHeap;  // 仅作为 GlobalAlloc 的 handle
```

`UserHeap` 本身无字段，所有状态在 `USER_HEAP` 静态变量中。这使 `UserHeap` 可以
`const` 构造，满足 `#[global_allocator]` 对 `static` 的要求。

### 3.3 Quota（`user/heap/src/quota.rs`）

```rust
pub struct Quota {
    pub limit: usize,  // 上限，0 = 无限制
    pub used: usize,   // 当前已用
}
```

`limit = 0` 表示无限制——`check()` 始终返回 `true`。所有算术使用 `saturating_*`
避免溢出/下溢。

### 3.4 OomHandler

```rust
pub type OomHandler = Option<fn() -> !>;
```

`Option<fn() -> !>`：返回类型 `!` 表示处理器必须发散（`panic!`、`loop {}`、
`process::abort()` 等）。`None` 表示使用默认 `panic!("user heap OOM")`。

---

## 4. 配额机制

### 4.1 配额检查流程

```
alloc(size):
  1. quota.check(size) → false → trigger OOM
  2. buddy.alloc(size) → null → trigger OOM（buddy 池耗尽但配额未满）
  3. quota.add_used(size)
  4. 返回指针

dealloc(ptr, size):
  1. buddy.dealloc(ptr, size)
  2. quota.sub_used(size)  // saturating_sub，不会下溢
```

### 4.2 配额设置

`heap_init(base, size)` 默认将配额设为 `size`（即整个池大小）。通过 `set_quota(limit)`
可进一步收紧：

```rust
unsafe { heap_init(pool, 2 * 1024 * 1024); }  // 2MB 池
set_quota(1024 * 1024);                         // 限制为 1MB
// 超过 1MB 的分配会触发 OOM，尽管池本身有 2MB
```

### 4.3 配额语义

- 配额基于**请求字节数**（`layout.size()`），不含 buddy 对齐开销
- 配额检查在锁内进行，线程安全
- `dealloc` 后 `used` 立即扣减，配额空间可被后续分配复用

---

## 5. 初始化

### 5.1 heap_init

```rust
pub unsafe fn heap_init(base: *mut u8, size: usize);
```

**前置条件**：
- `base` 必须 4KB 页对齐
- `[base, base + size)` 必须可写，且在程序生命周期内有效
- `size` 应为 `PAGE_SIZE`（4096）的整数倍

**行为**：
1. 计算 `pages = size / PAGE_SIZE`
2. 创建 `BuddyAllocator::new()` 并 `init(base, pages)`
3. 创建 `Quota::new(size)`（默认配额 = 池大小）
4. 将 `UserHeapInner { buddy, quota, oom_handler: None }` 存入 `USER_HEAP`

**可重复调用**：再次调用 `heap_init` 会替换现有堆状态。测试中用于重置堆。

### 5.2 典型使用模式

```rust
// 在 seL4 用户态进程的 main 入口
static mut HEAP_POOL: [u8; 2 * 1024 * 1024] = [0; 2 * 1024 * 1024];

#[no_mangle]
pub extern "C" fn main() {
    unsafe { eneros_user_heap::heap_init(HEAP_POOL.as_mut_ptr(), 2 * 1024 * 1024); }
    eneros_user_heap::set_quota(1024 * 1024);  // 限制 1MB

    // 此后 alloc::vec::Vec、alloc::string::String 等可用
    let v: alloc::vec::Vec<u8> = alloc::vec![1, 2, 3];
}
```

---

## 6. GlobalAlloc 集成

### 6.1 trait 实现

```rust
unsafe impl GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
}
```

**alloc 行为**：
1. 锁定 `USER_HEAP`，获取 `inner`
2. 若 `None`（未初始化）→ 返回 `null_mut`（不 panic）
3. `quota.check(size)` 失败 → 释放锁，调用 `trigger_oom_handler`
4. `buddy.alloc(size)` 返回 null → 释放锁，调用 `trigger_oom_handler`
5. `quota.add_used(size)`，返回指针

**dealloc 行为**：
1. 锁定 `USER_HEAP`，获取 `inner`
2. 若 `None` → 静默返回
3. `buddy.dealloc(ptr, size)`
4. `quota.sub_used(size)`

### 6.2 锁释放时机

`trigger_oom_handler` 在调用前**显式 `drop(guard)`** 释放 `USER_HEAP` 锁。原因：
- OOM handler 可能访问其他受 `USER_HEAP` 保护的状态（如 `used()` 查询）
- handler 通常 `panic!`，若持锁 panic 会导致 `Mutex` 中毒

---

## 7. 全局接口

| 函数 | 签名 | 说明 |
|------|------|------|
| `heap_init` | `unsafe fn(base: *mut u8, size: usize)` | 初始化堆（可重复调用） |
| `set_quota` | `fn(limit: usize)` | 设置配额上限（0 = 无限制） |
| `used` | `fn() -> usize` | 返回当前已用字节 |
| `set_oom_handler` | `fn(handler: fn() -> !)` | 设置自定义 OOM 处理器 |
| `trigger_oom` | `fn()` | 手动触发 OOM（测试用） |

所有函数对未初始化的堆安全：`set_quota`、`set_oom_handler` 是 no-op，`used()` 返回 0。

---

## 8. no_std 合规性

遵循蓝图 §43.1：

```rust
#![cfg_attr(not(test), no_std)]
```

| 构建 | `no_std` | `alloc` | 说明 |
|------|----------|---------|------|
| 正式（aarch64-unknown-none） | ✅ | ✅ | 用户态进程链接 `alloc` crate |
| 测试（host） | ❌ | ✅ | 链接 `std`，`Vec`/`String` 可用 |

**禁止使用的 API**：
- `std::collections::HashMap` → 用 `alloc::collections::BTreeMap`
- `std::sync::Mutex` → 用 `spin::Mutex`
- `std::time::Duration` → 用 `core::time::Duration`

---

## 9. 测试策略

### 9.1 单元测试（quota.rs，7 个）

| 测试 | 覆盖点 |
|------|--------|
| `test_quota_new` | 构造器 |
| `test_quota_check_pass` | 配额内通过 |
| `test_quota_check_fail` | 超配额拒绝 |
| `test_quota_unlimited` | limit=0 无限制 |
| `test_quota_add_used` | 递增 |
| `test_quota_sub_used` | 递减 |
| `test_quota_sub_overflow` | saturating_sub 不下溢 |

### 9.2 集成测试（lib.rs，2 个）

| 测试 | 覆盖点 |
|------|--------|
| `test_user_heap_integration` | 未初始化 alloc、heap_init、alloc/dealloc、used 跟踪、配额超限 OOM、buddy 耗尽 OOM、自定义 handler |
| `test_trigger_oom_default` | 默认 OOM handler 触发 panic |

### 9.3 测试隔离

所有集成测试共享 `USER_HEAP` 全局静态变量，因此必须在单个 `#[test]` 函数内顺序执行。
每次子测试前调用 `heap_init` 重置堆状态，确保隔离。

---

## 10. 局限性与未来工作

| 局限 | 说明 | 计划版本 |
|------|------|----------|
| 仅 buddy，无 slab | 小对象分配效率低于内核堆 | 视用户态 workload 决定是否添加 |
| 单分区 | 不支持多用户进程独立配额 | v0.12+（与 seL4 capability 集成） |
| 无线程局部堆 | 所有线程共享 `USER_HEAP` 锁 | 多核版本评估 |
| 无碎片整理 | buddy 碎片无法主动整理 | 暂无计划 |

---

## 11. 参考

- [内核堆设计](../kernel/kernel-heap-design.md) — v0.10.0 slab + buddy 混合算法
- [slab/buddy 算法说明](../kernel/slab-buddy-algorithm.md) — 算法细节
- [OOM 策略](oom-policy.md) — OOM handler 机制详解
- 蓝图 `phase0.md` §v0.11.0 — 版本规格
