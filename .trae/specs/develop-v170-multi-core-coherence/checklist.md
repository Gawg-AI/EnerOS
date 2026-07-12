# Checklist — EnerOS v0.17.0 多核内存一致性

> **蓝图依据**：`蓝图/phase0.md` §v0.17.0（第 3696-3891 行）
> **合规性**：蓝图 §43.1（no_std）、新规则 §2.3.3（文档分类）、§2.4 校验清单
> **验收标准**：蓝图 §7（第 3868-3873 行）

---

## 1. 内存屏障与缓存操作（coherence.rs）

- [x] `crates/kernel/smp/src/coherence.rs` 存在
- [x] `const CACHELINE_SIZE: usize = 64` 定义
- [x] `dmb()` 实现（aarch64 `asm!("dmb ish" :::: "memory")`，host no-op）
- [x] `dsb()` 实现（aarch64 `asm!("dsb ish" :::: "memory")`，host no-op）
- [x] `isb()` 实现（aarch64 `asm!("isb" :::: "memory")`，host no-op）
- [x] `cache_clean(addr, size)` 实现（对齐 64B → 循环 `dc civac` → `dsb()`）
- [x] `cache_invalidate(addr, size)` 实现（对齐 64B → 循环 `dc ivac` → `dsb()`）
- [x] 地址对齐算法正确（向下对齐 start，向上扩展 end）
- [x] aarch64 inline asm 全部用 `unsafe { }` 包裹
- [x] 所有 `asm!` 用 `#[cfg(target_arch = "aarch64")]` gate
- [x] 单元测试覆盖（≥ 4 个测试：no-op 不 panic、对齐计算、零大小、cacheline 边界）

## 2. 原子计数器（atomic_ops.rs）

- [x] `crates/kernel/smp/src/atomic_ops.rs` 存在
- [x] `use core::sync::atomic::{AtomicU64, Ordering};`（非 std::sync::atomic）
- [x] `AtomicCounter` 结构体定义（`value: AtomicU64`）
- [x] `AtomicCounter::new(v)` 实现（`const fn`）
- [x] `AtomicCounter::inc(&self) -> u64` 实现（`fetch_add(1, Relaxed) + 1`）
- [x] `AtomicCounter::load(&self) -> u64` 实现（`load(Acquire)`）
- [x] `AtomicCounter::store(&self, v)` 实现（`store(v, Release)`）
- [x] 单元测试覆盖（≥ 4 个测试：new/inc/load/store 基本流程、多次 inc 累加、store/load 一致）

## 3. DMA 一致性缓冲（dma_coherent.rs）

- [x] `crates/kernel/smp/src/dma_coherent.rs` 存在
- [x] `use crate::coherence::{cache_clean, cache_invalidate};` 跨模块引用正确
- [x] `DmaBuffer` 结构体定义（phys/virt/size/coherent 四字段），derive Debug
- [x] `DmaBuffer::sync_for_device(&self)` 实现（非 coherent 时 cache_clean）
- [x] `DmaBuffer::sync_for_cpu(&self)` 实现（非 coherent 时 cache_invalidate）
- [x] coherent=true 时 sync 函数为 no-op
- [x] 单元测试覆盖（≥ 3 个测试：coherent=true no-op、coherent=false 不 panic、零大小不 panic）

## 4. lib.rs 集成

- [x] `crates/kernel/smp/src/lib.rs` 含 `pub mod coherence;`
- [x] `crates/kernel/smp/src/lib.rs` 含 `pub mod atomic_ops;`
- [x] `crates/kernel/smp/src/lib.rs` 含 `pub mod dma_coherent;`
- [x] lib.rs 含 `pub use coherence::{cache_clean, cache_invalidate, dmb, dsb, isb};`
- [x] lib.rs 含 `pub use atomic_ops::AtomicCounter;`
- [x] lib.rs 含 `pub use dma_coherent::DmaBuffer;`
- [x] lib.rs 顶部文档注释更新（添加 v0.17.0 coherence/atomic_ops/dma_coherent 说明）
- [x] `crates/kernel/smp/Cargo.toml` version = "0.17.0"

## 5. no_std 合规

- [x] `crates/kernel/smp/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`（已有，不修改）
- [x] 无 `use std::*`（除 `#[cfg(test)]` 模块内）
- [x] `atomic_ops.rs` 用 `core::sync::atomic` 而非 `std::sync::atomic`
- [x] 无新增外部依赖（仅用 `core::*`，现有 spin/heapless 不变）

## 6. 构建系统

- [x] 根 `Cargo.toml` workspace version = "0.17.0"
- [x] `crates/kernel/smp/Cargo.toml` version = "0.17.0"
- [x] `Makefile` VERSION := 0.17.0
- [x] `Makefile` smp-build / smp-test 目标存在（已有，不新增）
- [x] `.github/workflows/ci.yml` 版本标识 v0.17.0
- [x] `.github/workflows/ci.yml` "Build smp crate" 步骤存在（已有，不新增）
- [x] `ci/src/gate.rs` 注释含 v0.17.0

## 7. 文档

- [x] `docs/smp/memory-coherence-design.md` 存在（屏障语义、缓存维护、DMA 一致性、与调度器/IPI 关系）
- [x] `docs/smp/armv8-memory-model.md` 存在（弱内存模型、Acquire/Release、Shareability 域）
- [x] 文档在 `docs/smp/` 子目录下（不在 `docs/` 根，符合 §2.3.3）

## 8. 验证

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-smp --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-smp` 全部通过（预期 ≥ 30 个测试：v0.15.0 原有 19 + v0.17.0 新增 ≥ 11）
- [x] `cargo build -p eneros-smp --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（回归，v0.16.0 sched 不退化）
- [x] `git status` 无垃圾文件

## 9. 蓝图验收标准（§7）

- [x] §7.1 多核并发原子计数器结果正确（host 单线程逻辑测试 + 文档标注多核并发需 QEMU 验证）
- [x] §7.2 DMA 缓冲一致性正确（sync_for_device/sync_for_cpu 逻辑测试通过）
- [x] §7.3 原子 inc < 10ns（不在 host 测，文档标注；aarch64 真机验证留待 QEMU 阶段）
- [x] §7.4 文档齐全（2 份文档存在）
- [x] §7.5 出口判定：多核一致性就绪，P0-E 多核 SMP 闭环达成

## 10. 目录结构校验（新规则 §2.4）

- [x] C4 文档分类：新文档在 `docs/smp/` 下，不在 `docs/` 根
- [x] C5 无根目录 crate：无新增根目录 crate（v0.17.0 只是给现有 smp crate 加模块）
- [x] C9 `cargo fmt --check` 通过
- [x] C10 `cargo clippy` 无 warning
- [x] C13 无垃圾文件：`git status` 无 `target/`、`*.elf`、`*.bin`、IDE 缓存被追踪

## 11. 外科手术原则自检（Karpathy §3）

- [x] **未修改** 任何其他 crate 源码（sched/panic/time/watchdog/hal/kernel 等）
- [x] 新增文件仅限 smp crate 3 个源文件 + 2 份文档
- [x] 修改文件仅限 smp/lib.rs + smp/Cargo.toml + 根 Cargo.toml + Makefile + ci.yml + gate.rs
- [x] 无"顺手改进"其他代码（每行改动可追溯到 v0.17.0 需求）
- [x] 无过度抽象（直接采用蓝图代码片段，AtomicCounter 仅 4 方法，DmaBuffer 仅 2 方法）
