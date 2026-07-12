# Tasks — EnerOS v0.11.0 用户态堆分配器

> **蓝图依据**：`蓝图/phase0.md` §v0.11.0
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本）
> **前序依赖**：v0.10.0（内核堆 BuddyAllocator 可复用）

---

## 实现任务

- [x] Task 1: 创建 `user/heap` crate 骨架
  - [x] SubTask 1.1: 创建 `user/heap/Cargo.toml`（crate name = `eneros-user-heap`，依赖 `eneros-heap` + `spin = "0.9"`）
  - [x] SubTask 1.2: 创建 `user/heap/src/lib.rs`（含 `#![cfg_attr(not(test), no_std)]` 和模块声明 `pub mod quota;`）
  - [x] SubTask 1.3: workspace 根 `Cargo.toml` 添加 `"user/heap"` 到 members，version `0.10.0` → `0.11.0`

- [x] Task 2: 实现 `user/heap/src/quota.rs` — Quota 配额管理
  - [x] SubTask 2.1: 定义 `OomHandler` 类型别名 `Option<fn() -> !>`
  - [x] SubTask 2.2: 定义 `Quota` 结构体（limit, used），derive Debug
  - [x] SubTask 2.3: 实现 `Quota::new(limit)` — const fn，limit=0 表示无限制
  - [x] SubTask 2.4: 实现 `check(size) -> bool` — limit=0 始终 true，否则 used+size <= limit
  - [x] SubTask 2.5: 实现 `add_used(size)` / `sub_used(size)` — sub 用 saturating_sub
  - [x] SubTask 2.6: 编写单元测试（new, check 通过/失败/无限制, add/sub_used）

- [x] Task 3: 实现 `user/heap/src/lib.rs` — UserHeap + GlobalAlloc + 全局接口
  - [x] SubTask 3.1: 定义 `UserHeapInner` 结构体（buddy: BuddyAllocator, quota: Quota, oom_handler: OomHandler）
  - [x] SubTask 3.2: 定义 `UserHeap` 零字段占位类型，`unsafe impl Sync`
  - [x] SubTask 3.3: 定义 `USER_HEAP: spin::Mutex<Option<UserHeapInner>>` 全局静态变量
  - [x] SubTask 3.4: 实现 `UserHeap::new()` const 构造器
  - [x] SubTask 3.5: 实现 `unsafe impl GlobalAlloc for UserHeap`（alloc: 配额检查→buddy分配→更新used；dealloc: buddy归还→更新used）
  - [x] SubTask 3.6: 实现 `heap_init(base, size)` — 创建 BuddyAllocator + Quota，存入 USER_HEAP
  - [x] SubTask 3.7: 实现 `set_quota(limit)` / `used()` / `trigger_oom()` 全局函数
  - [x] SubTask 3.8: 添加 `#[cfg(not(test))] #[global_allocator] static ALLOC: UserHeap` 注册
  - [x] SubTask 3.9: 编写集成测试（heap_init + alloc/dealloc, 配额耗尽 OOM, buddy 分配失败, used 查询, stats 一致性）

- [x] Task 4: 更新构建系统与 CI 配置
  - [x] SubTask 4.1: 修改 `Makefile`（VERSION 0.10.0 → 0.11.0，添加 user-heap-build/user-heap-test 目标）
  - [x] SubTask 4.2: 修改 `.github/workflows/ci.yml`（版本标识，cross-build 添加 user-heap crate 步骤）
  - [x] SubTask 4.3: 修改 `ci/src/gate.rs`（注释 + v0.11.0 user-heap）

- [x] Task 5: 编写文档
  - [x] SubTask 5.1: 创建 `docs/user-heap-design.md`（架构概述、与内核堆关系、配额机制、初始化、GlobalAlloc 集成）
  - [x] SubTask 5.2: 创建 `docs/oom-policy.md`（OOM 触发条件、handler 机制、默认行为、恢复策略）

---

## 验证任务

- [x] Task 6: 运行全套验证检查
  - [x] SubTask 6.1: `cargo fmt --all -- --check` 格式检查通过
  - [x] SubTask 6.2: `cargo clippy -p eneros-user-heap --all-targets -- -D warnings` 通过
  - [x] SubTask 6.3: `cargo test -p eneros-user-heap` 所有单元测试通过
  - [x] SubTask 6.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全工作区测试通过
  - [x] SubTask 6.5: `cargo build -p eneros-user-heap --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] SubTask 6.6: `git status` 确认无垃圾文件被追踪

---

# Task Dependencies

- Task 2（quota）无依赖，可与 Task 3 骨架并行
- Task 3（lib.rs）依赖 Task 2（Quota 类型）
- Task 4（构建系统）依赖 Task 1（crate 骨架存在）
- Task 5（文档）依赖 Task 3（实现完成）
- Task 6（验证）依赖所有前序任务完成

**并行机会**：Task 2 可与 Task 1 并行（quota.rs 不依赖 crate 骨架）；Task 4 可在 Task 1 完成后与 Task 3 并行；Task 5 可在 Task 3 完成后与 Task 6 前半部分并行。
