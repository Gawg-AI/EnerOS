# Tasks — EnerOS v0.10.0 内核态堆分配器

> **蓝图依据**：`蓝图/phase0.md` §v0.10.0
> **合规性**：蓝图 §43.1（no_std）、§43.2（★瓶颈版本：可运行实现）
> **前序依赖**：v0.9.0（分区隔离完成）

---

## 实现任务

- [x] Task 1: 创建 `heap` crate 骨架（Cargo.toml + lib.rs 模块声明）
  - [x] SubTask 1.1: 创建 `heap/Cargo.toml`（crate name = `eneros-heap`，依赖 `spin = "0.9"`，`#![cfg_attr(not(test), no_std)]`）
  - [x] SubTask 1.2: 创建 `heap/src/lib.rs`（模块声明 `pub mod buddy; pub mod slab; pub mod stats;`，crate 文档注释）
  - [x] SubTask 1.3: workspace 根 `Cargo.toml` 添加 `"heap"` 到 members，version `0.9.0` → `0.10.0`

- [x] Task 2: 实现 `heap/src/stats.rs` — HeapStats 碎片统计
  - [x] SubTask 2.1: 定义 `HeapStats` 结构体（8 个字段，derive Clone/Copy/Debug/Default）
  - [x] SubTask 2.2: 实现 `HeapStats::compute_fragmentation()` 计算方法
  - [x] SubTask 2.3: 编写单元测试（default 值、字段赋值、碎片率计算）

- [x] Task 3: 实现 `heap/src/buddy.rs` — BuddyAllocator 页级分配器
  - [x] SubTask 3.1: 定义常量（PAGE_SIZE=4096, MAX_ORDER=11, BITMAP_WORDS=128）
  - [x] SubTask 3.2: 定义 `BuddyAllocator` 结构体（base, total_pages, free_lists[12], free_count[12], bitmap[128]）
  - [x] SubTask 3.3: 实现 `BuddyAllocator::new()` const 构造器
  - [x] SubTask 3.4: 实现 `init(base, pages)` — 初始化堆池，将所有页放入最大阶空闲链
  - [x] SubTask 3.5: 实现 `order_for(size)` — 计算所需阶数（向上取整）
  - [x] SubTask 3.6: 实现位图操作（`set_allocated`, `clear_allocated`, `is_range_free`）
  - [x] SubTask 3.7: 实现 `alloc(size)` — 分裂空闲块，返回指针或 null
  - [x] SubTask 3.8: 实现 `is_free(ptr, order)` — 通过位图检查块是否空闲
  - [x] SubTask 3.9: 实现 `remove_from_free(ptr, order)` — 从空闲链移除指定块
  - [x] SubTask 3.10: 实现 `dealloc(ptr, size)` — 合并 buddy 块，归还空闲链
  - [x] SubTask 3.11: 编写单元测试（init, alloc 单页, alloc 多页, dealloc 简单, dealloc 合并, OOM, order_for）

- [x] Task 4: 实现 `heap/src/slab.rs` — SlabCache 小对象池
  - [x] SubTask 4.1: 定义 `SlabSlot` 和 `SlabCache` 结构体（obj_size, free_head, total, used）
  - [x] SubTask 4.2: 实现 `SlabCache::new(obj_size)` const 构造器
  - [x] SubTask 4.3: 实现 `alloc(&mut buddy)` — 空闲链取槽，或向 buddy 申请新页并切分
  - [x] SubTask 4.4: 实现 `dealloc(ptr)` — 归还到空闲链头
  - [x] SubTask 4.5: 定义 `SLAB_SIZES: [usize; 8]` = [8, 16, 32, 64, 128, 256, 512, 1024]
  - [x] SubTask 4.6: 编写单元测试（new, alloc 首次触发页申请, alloc 空闲链命中, dealloc, 多 bucket）

- [x] Task 5: 实现 `heap/src/lib.rs` — KernelHeap + GlobalAlloc + 全局接口
  - [x] SubTask 5.1: 定义 `KernelHeapInner` 结构体（buddy, slabs[8], stats）
  - [x] SubTask 5.2: 定义 `KernelHeap` 零字段占位类型
  - [x] SubTask 5.3: 定义 `KERNEL_HEAP: spin::Mutex<Option<KernelHeapInner>>` 全局静态变量
  - [x] SubTask 5.4: 实现 `unsafe impl GlobalAlloc for KernelHeap`（alloc/dealloc，含 slab/buddy 路由和 stats 更新）
  - [x] SubTask 5.5: 实现 `heap_init(base, size)` 全局函数
  - [x] SubTask 5.6: 实现 `heap_stats()` 全局函数
  - [x] SubTask 5.7: 添加 `#[cfg(not(test))] #[global_allocator]` 注册（仅非测试构建）
  - [x] SubTask 5.8: 编写集成测试（heap_init + alloc/dealloc, OOM 返回 null, slab 命中率, stats 一致性）

- [x] Task 6: 更新构建系统与 CI 配置
  - [x] SubTask 6.1: 修改 `Makefile`（VERSION 0.9.0 → 0.10.0，添加 heap-build/heap-test 目标）
  - [x] SubTask 6.2: 修改 `.github/workflows/ci.yml`（版本标识，cross-build 添加 heap crate 步骤）
  - [x] SubTask 6.3: 修改 `ci/src/gate.rs`（注释 + v0.10.0 heap）

- [x] Task 7: 编写文档
  - [x] SubTask 7.1: 创建 `docs/kernel-heap-design.md`（架构概述、数据结构、分配/释放流程、初始化、GlobalAlloc 集成）
  - [x] SubTask 7.2: 创建 `docs/slab-buddy-algorithm.md`（buddy 分裂/合并原理、slab 空闲链、碎片分析、性能目标）

---

## 验证任务

- [x] Task 8: 运行全套验证检查
  - [x] SubTask 8.1: `cargo fmt --all -- --check` 格式检查通过
  - [x] SubTask 8.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
  - [x] SubTask 8.3: `cargo test -p eneros-heap` 所有单元测试通过
  - [x] SubTask 8.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全工作区测试通过
  - [x] SubTask 8.5: `cargo build -p eneros-heap --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] SubTask 8.6: `git status` 确认无垃圾文件（target/ 等）被追踪

---

# Task Dependencies

- Task 2（stats）无依赖，可与 Task 3 并行
- Task 3（buddy）无依赖，可与 Task 2 并行
- Task 4（slab）依赖 Task 3（slab.alloc 需调用 buddy.alloc）
- Task 5（lib.rs GlobalAlloc）依赖 Task 2/3/4（组合 buddy+slab+stats）
- Task 6（构建系统）依赖 Task 1（crate 骨架存在）
- Task 7（文档）依赖 Task 3/4/5（算法实现完成）
- Task 8（验证）依赖所有前序任务完成

**并行机会**：Task 2 与 Task 3 可并行；Task 6 可在 Task 1 完成后与 Task 3/4/5 并行；Task 7 可在 Task 5 完成后与 Task 8 的前半部分并行。
