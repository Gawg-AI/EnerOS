# Checklist — EnerOS v0.11.0 用户态堆分配器

> **蓝图依据**：`蓝图/phase0.md` §v0.11.0
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本）

---

## 1. Crate 结构与依赖

- [x] `user/heap/Cargo.toml` 存在，crate name = `eneros-user-heap`，edition = 2021
- [x] `user/heap/Cargo.toml` 依赖 `eneros-heap`（workspace 路径依赖）和 `spin = "0.9"`
- [x] `user/heap/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `user/heap/src/lib.rs` 声明 `pub mod quota;`
- [x] workspace 根 `Cargo.toml` members 含 `"user/heap"`，version = `0.11.0`

## 2. Quota 配额管理（quota.rs）

- [x] `OomHandler` 类型别名定义为 `Option<fn() -> !>`
- [x] `Quota` 结构体含 `limit: usize` 和 `used: usize` 字段
- [x] `Quota::new(limit)` 为 const fn，limit=0 表示无限制
- [x] `check(size)` 正确：limit=0 始终 true，否则 `used + size <= limit`
- [x] `add_used(size)` 正确递增 used
- [x] `sub_used(size)` 使用 saturating_sub 递减
- [x] 单元测试覆盖：new, check 通过/失败/无限制, add/sub_used

## 3. UserHeap + GlobalAlloc 实现（lib.rs）

- [x] `UserHeapInner` 结构体含 buddy, quota, oom_handler
- [x] `UserHeap` 为零字段占位类型，实现 `unsafe impl Sync`
- [x] `USER_HEAP: spin::Mutex<Option<UserHeapInner>>` 全局静态变量
- [x] `UserHeap::new()` 为 const fn
- [x] `GlobalAlloc::alloc` 实现：配额检查 → buddy 分配 → 更新 used
- [x] `GlobalAlloc::dealloc` 实现：buddy 归还 → 更新 used
- [x] 堆未初始化时 alloc 返回 null_mut，不 panic
- [x] 配额不足时调用 trigger_oom，返回 null_mut
- [x] `heap_init(base, size)` 初始化 buddy + quota，存入 USER_HEAP
- [x] `set_quota(limit)` 更新配额上限
- [x] `used()` 返回当前已用字节
- [x] `trigger_oom()` 调用 handler 或 panic
- [x] `#[cfg(not(test))] #[global_allocator] static ALLOC: UserHeap` 注册正确
- [x] 集成测试覆盖：heap_init + alloc/dealloc, 配额耗尽, used 查询

## 4. no_std 合规性

- [x] 正式构建（aarch64-unknown-none）为 no_std
- [x] 测试构建链接 std（`cfg_attr(not(test), no_std)` 模式）
- [x] 不使用 `std::*`，使用 `core::alloc::{GlobalAlloc, Layout}`、`core::ptr`、`spin::Mutex`、`eneros_heap::buddy::BuddyAllocator`
- [x] `#[global_allocator]` 仅在 `#[cfg(not(test))]` 下注册

## 5. 构建系统与 CI

- [x] `Makefile` VERSION = 0.11.0
- [x] `Makefile` 含 `user-heap-build` 和 `user-heap-test` 目标
- [x] `.github/workflows/ci.yml` 版本标识 v0.11.0
- [x] `.github/workflows/ci.yml` cross-build 含 "Build user-heap crate" 步骤
- [x] `ci/src/gate.rs` 注释含 "+ v0.11.0 user-heap"

## 6. 文档交付

- [x] `docs/user-heap-design.md` 存在，内容含：架构概述、与内核堆关系、配额机制、初始化、GlobalAlloc 集成
- [x] `docs/oom-policy.md` 存在，内容含：OOM 触发条件、handler 机制、默认行为、恢复策略

## 7. 质量门禁

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-user-heap --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-user-heap` 所有测试通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全工作区测试通过
- [x] `cargo build -p eneros-user-heap --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] `git status` 无垃圾文件（target/、*.elf、*.bin 等）被追踪
