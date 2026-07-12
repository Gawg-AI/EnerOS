# Checklist — EnerOS v0.10.0 内核态堆分配器

> **蓝图依据**：`蓝图/phase0.md` §v0.10.0
> **合规性**：蓝图 §43.1（no_std）、§43.2（★瓶颈版本：可运行实现）

---

## 1. Crate 结构与依赖

- [x] `heap/Cargo.toml` 存在，crate name = `eneros-heap`，edition = 2021
- [x] `heap/Cargo.toml` 依赖 `spin = "0.9"`
- [x] `heap/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `heap/src/lib.rs` 声明 `pub mod buddy; pub mod slab; pub mod stats;`
- [x] workspace 根 `Cargo.toml` members 含 `"heap"`，version = `0.10.0`

## 2. HeapStats 实现（stats.rs）

- [x] `HeapStats` 结构体含 8 个字段（total_bytes, allocated_bytes, free_bytes, fragmentation_ratio, alloc_count, free_count, slab_hits, buddy_hits）
- [x] `HeapStats` derive `Clone, Copy, Debug, Default`
- [x] `HeapStats::default()` 所有字段为 0
- [x] 碎片率计算公式正确：`(free_bytes - largest_free_block) / free_bytes * 1000`
- [x] 单元测试覆盖 default 值、字段赋值、碎片率计算

## 3. BuddyAllocator 实现（buddy.rs）

- [x] 常量定义正确：PAGE_SIZE=4096, MAX_ORDER=11, BITMAP_WORDS=128
- [x] `BuddyAllocator` 结构体含 base, total_pages, free_lists[12], free_count[12], bitmap[128]
- [x] `new()` 为 const fn，初始化所有字段为零/None
- [x] `init(base, pages)` 将所有页放入最大阶空闲链，位图清零
- [x] `order_for(size)` 正确计算阶数（4096→0, 4097→1, 8192→1, 4MB→10）
- [x] 位图操作正确：`set_allocated`/`clear_allocated`/`is_range_free`
- [x] `alloc(size)` 正确分裂空闲块，位图标记已分配
- [x] `is_free(ptr, order)` 通过位图检查块范围所有页是否空闲
- [x] `remove_from_free(ptr, order)` 正确从空闲链移除指定块
- [x] `dealloc(ptr, size)` 正确合并 buddy 块（XOR 地址），递归向上合并
- [x] OOM 时 `alloc` 返回 `core::ptr::null_mut()`，不 panic
- [x] 单元测试覆盖：init, alloc 单页, alloc 多页, dealloc 简单, dealloc 合并, OOM, order_for

## 4. SlabCache 实现（slab.rs）

- [x] `SlabCache` 结构体含 obj_size, free_head, total, used
- [x] `new(obj_size)` 为 const fn
- [x] `alloc(&mut buddy)` 空闲链空时向 buddy 申请新页并切分为槽
- [x] `alloc(&mut buddy)` 空闲链非空时从链头取槽
- [x] `dealloc(ptr)` 将指针加入空闲链头
- [x] `SLAB_SIZES` = [8, 16, 32, 64, 128, 256, 512, 1024]
- [x] 单元测试覆盖：new, alloc 首次触发页申请, alloc 空闲链命中, dealloc, 多 bucket

## 5. KernelHeap + GlobalAlloc 实现（lib.rs）

- [x] `KernelHeapInner` 结构体含 buddy, slabs[8], stats
- [x] `KernelHeap` 为零字段占位类型
- [x] `KERNEL_HEAP: spin::Mutex<Option<KernelHeapInner>>` 全局静态变量
- [x] `unsafe impl GlobalAlloc for KernelHeap` 实现完整
- [x] `alloc` 方法：size ≤ 1024 走 slab，size > 1024 走 buddy
- [x] `alloc` 方法：更新 stats（slab_hits/buddy_hits/alloc_count）
- [x] `dealloc` 方法：根据 size 路由到 slab 或 buddy
- [x] `dealloc` 方法：更新 stats（free_count）
- [x] `heap_init(base, size)` 初始化 buddy + slabs + stats，存入 KERNEL_HEAP
- [x] `heap_stats()` 返回 HeapStats（未初始化返回 default）
- [x] `#[cfg(not(test))] #[global_allocator] static ALLOCATOR: KernelHeap` 注册正确
- [x] 集成测试：heap_init + alloc/dealloc 正常工作
- [x] 集成测试：OOM 返回 null
- [x] 集成测试：slab 命中率统计正确
- [x] 集成测试：stats 字段一致性

## 6. no_std 合规性

- [x] 正式构建（aarch64-unknown-none）为 no_std
- [x] 测试构建链接 std（`cfg_attr(not(test), no_std)` 模式）
- [x] 不使用 `std::*`，使用 `core::alloc::{GlobalAlloc, Layout}`、`core::ptr`、`spin::Mutex`
- [x] `#[global_allocator]` 仅在 `#[cfg(not(test))]` 下注册

## 7. 构建系统与 CI

- [x] `Makefile` VERSION = 0.10.0
- [x] `Makefile` 含 `heap-build` 和 `heap-test` 目标
- [x] `.github/workflows/ci.yml` 版本标识 v0.10.0
- [x] `.github/workflows/ci.yml` cross-build 含 "Build heap crate" 步骤
- [x] `ci/src/gate.rs` 注释含 "+ v0.10.0 heap"

## 8. 文档交付

- [x] `docs/kernel-heap-design.md` 存在，内容含：架构概述、数据结构、分配/释放流程、初始化序列、GlobalAlloc 集成
- [x] `docs/slab-buddy-algorithm.md` 存在，内容含：buddy 分裂/合并原理、slab 空闲链机制、碎片分析、性能目标

## 9. 质量门禁

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-heap` 所有测试通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全工作区测试通过
- [x] `cargo build -p eneros-heap --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] `git status` 无垃圾文件（target/、*.elf、*.bin 等）被追踪
