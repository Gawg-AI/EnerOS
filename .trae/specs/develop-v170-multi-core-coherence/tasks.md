# Tasks — EnerOS v0.17.0 多核内存一致性

> **蓝图依据**：`蓝图/phase0.md` §v0.17.0（第 3696-3891 行）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动
> **依赖**：v0.16.0（多核调度器，已满足）
> **合规**：非瓶颈版本，但蓝图提供完整代码片段，直接采用

---

## Task 1: 实现内存屏障与缓存操作（`crates/kernel/smp/src/coherence.rs`）

- [x] SubTask 1.1: 创建 `coherence.rs`，定义 cacheline 常量 `const CACHELINE_SIZE: usize = 64;`
- [x] SubTask 1.2: 实现 `dmb()` — aarch64 用 `asm!("dmb ish" :::: "memory")`，host 用 no-op stub
- [x] SubTask 1.3: 实现 `dsb()` — aarch64 用 `asm!("dsb ish" :::: "memory")`，host 用 no-op stub
- [x] SubTask 1.4: 实现 `isb()` — aarch64 用 `asm!("isb" :::: "memory")`，host 用 no-op stub
- [x] SubTask 1.5: 实现 `cache_clean(addr: u64, size: usize)` — 地址向下对齐到 64B，大小向上扩展，循环 `dc civac`，末尾 `dsb()`
- [x] SubTask 1.6: 实现 `cache_invalidate(addr: u64, size: usize)` — 地址向下对齐到 64B，大小向上扩展，循环 `dc ivac`，末尾 `dsb()`
- [x] SubTask 1.7: 编写单元测试（host 侧 no-op 不 panic、对齐计算验证、零大小不 panic、cacheline 边界对齐）
- [x] 验证：`cargo test -p eneros-smp coherence` 通过

## Task 2: 实现原子计数器（`crates/kernel/smp/src/atomic_ops.rs`）

- [x] SubTask 2.1: 创建 `atomic_ops.rs`，`use core::sync::atomic::{AtomicU64, Ordering};`
- [x] SubTask 2.2: 定义 `AtomicCounter` 结构体（`value: AtomicU64`）
- [x] SubTask 2.3: 实现 `AtomicCounter::new(v: u64) -> Self`（`const fn`）
- [x] SubTask 2.4: 实现 `AtomicCounter::inc(&self) -> u64`（`fetch_add(1, Relaxed) + 1`）
- [x] SubTask 2.5: 实现 `AtomicCounter::load(&self) -> u64`（`load(Acquire)`）
- [x] SubTask 2.6: 实现 `AtomicCounter::store(&self, v: u64)`（`store(v, Release)`）
- [x] SubTask 2.7: 编写单元测试（new/inc/load/store 基本流程、多次 inc 累加正确、store 后 load 一致）
- [x] 验证：`cargo test -p eneros-smp atomic_ops` 通过

## Task 3: 实现 DMA 一致性缓冲（`crates/kernel/smp/src/dma_coherent.rs`）

- [x] SubTask 3.1: 创建 `dma_coherent.rs`，`use crate::coherence::{cache_clean, cache_invalidate};`
- [x] SubTask 3.2: 定义 `DmaBuffer` 结构体（`phys: u64`、`virt: *mut u8`、`size: usize`、`coherent: bool`），derive Debug
- [x] SubTask 3.3: 实现 `DmaBuffer::sync_for_device(&self)` — 非 coherent 时调用 `cache_clean(self.virt as u64, self.size)`
- [x] SubTask 3.4: 实现 `DmaBuffer::sync_for_cpu(&self)` — 非 coherent 时调用 `cache_invalidate(self.virt as u64, self.size)`
- [x] SubTask 3.5: 编写单元测试（coherent=true 时 sync 为 no-op、coherent=false 时调用 clean/invalidate 不 panic、零大小缓冲不 panic）
- [x] 验证：`cargo test -p eneros-smp dma_coherent` 通过

## Task 4: 更新 smp crate lib.rs 与 Cargo.toml

- [x] SubTask 4.1: 在 `crates/kernel/smp/src/lib.rs` 添加 `pub mod coherence;` / `pub mod atomic_ops;` / `pub mod dma_coherent;`
- [x] SubTask 4.2: 在 lib.rs 添加 `pub use coherence::{cache_clean, cache_invalidate, dmb, dsb, isb};`
- [x] SubTask 4.3: 在 lib.rs 添加 `pub use atomic_ops::AtomicCounter;` 和 `pub use dma_coherent::DmaBuffer;`
- [x] SubTask 4.4: 更新 lib.rs 顶部文档注释，添加 v0.17.0 相关注明（coherence/atomic_ops/dma_coherent 模块说明）
- [x] SubTask 4.5: 更新 `crates/kernel/smp/Cargo.toml` version `0.15.0` → `0.17.0`
- [x] 验证：`cargo build -p eneros-smp` 成功

## Task 5: 更新构建系统

- [x] SubTask 5.1: 更新根 `Cargo.toml` workspace version `0.16.0` → `0.17.0`
- [x] SubTask 5.2: 更新 `Makefile` VERSION `0.16.0` → `0.17.0`（smp-build/smp-test 目标已存在，无需新增）
- [x] SubTask 5.3: 更新 `.github/workflows/ci.yml` 版本标识 `v0.16.0` → `v0.17.0`（smp cross-build 步骤已存在，无需新增）
- [x] SubTask 5.4: 更新 `ci/src/gate.rs` 注释追加 `eneros-smp (v0.17.0 coherence)`
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 6: 编写文档（放入 `docs/smp/`）

- [x] SubTask 6.1: 创建 `docs/smp/memory-coherence-design.md`（内存屏障语义、缓存维护操作、DMA 一致性流程、与调度器/IPI 的关系、蓝图 §4 对齐）
- [x] SubTask 6.2: 创建 `docs/smp/armv8-memory-model.md`（ARMv8 弱内存模型、Acquire/Release 语义、Shareability 域、与 x86 强序模型对比、蓝图 §5 技术交底对齐）

## Task 7: 验证

- [x] SubTask 7.1: `cargo fmt --all -- --check` 通过
- [x] SubTask 7.2: `cargo clippy -p eneros-smp --all-targets -- -D warnings` 通过
- [x] SubTask 7.3: `cargo test -p eneros-smp` 全部通过（v0.15.0 原有 19 测试 + v0.17.0 新增测试，预期 ≥ 30 个）
- [x] SubTask 7.4: `cargo build -p eneros-smp --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] SubTask 7.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（回归，v0.16.0 sched 不退化）
- [x] SubTask 7.6: `git status` 无垃圾文件

---

# Task Dependencies

- Task 1（coherence.rs）→ Task 3（dma_coherent.rs 依赖 cache_clean/cache_invalidate）
- Task 2（atomic_ops.rs）独立，可并行
- Task 3（dma_coherent.rs）依赖 Task 1
- Task 4（lib.rs）依赖 Task 1-3 全部完成
- Task 5（构建系统）独立，可与 Task 1-3 并行
- Task 6（文档）依赖 Task 1-3 完成（需引用实现细节）
- Task 7（验证）依赖全部完成

**并行机会**：Task 1 + Task 2 + Task 5 可并行；Task 6 可与 Task 4 并行。

---

# 蓝图符合性自检

| 蓝图条目 | 任务覆盖 |
|---------|---------|
| §3 交付物 coherence.rs(~200行)/atomic_ops.rs(~150行)/dma_coherent.rs(~120行) | Task 1 / Task 2 / Task 3 |
| §3 接口 dmb/dsb/cache_flush/cache_invalidate | SubTask 1.2-1.6 |
| §4.1 数据结构 AtomicCounter/DmaBuffer | SubTask 2.2 / 3.2 |
| §4.2 接口定义（dmb/dsb/isb/cache_flush/cache_invalidate/dma_sync） | Task 1 + Task 3 |
| §4.5 关键代码片段（完整采用） | Task 1 + Task 2 + Task 3 |
| §5.1 显式屏障而非依赖硬件强序 | D1 + Task 1 |
| §5.2 Acquire/Release 对应 ldar/stlr | Task 2（load Acquire / store Release） |
| §6.1 单元 AtomicCounter ≥80% | SubTask 2.7 |
| §6.4 回归 v0.16.0 不退化 | SubTask 7.5 |
| §6.5 故障注入移除屏障验证损坏 | 文档标注（host 无法真测） |
| §7 验收标准 | checklist.md 覆盖 |
| §43.1 no_std 合规 | SubTask 4.1（lib.rs 已有） |
| 新规则 §2.3.3 文档分类 | SubTask 6.1/6.2（docs/smp/） |
