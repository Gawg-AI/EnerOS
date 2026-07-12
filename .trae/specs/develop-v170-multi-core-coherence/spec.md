# EnerOS v0.17.0 多核内存一致性 Spec

## Why

v0.16.0 完成了多核调度器，但多核并发数据的正确性依赖内存一致性。ARMv8 是弱内存模型（Weak Memory Model），多核间共享数据的可见性不自动保证，必须显式使用内存屏障（`dmb`/`dsb`/`isb`）和缓存维护操作（`dc civac`/`dc ivac`）。此外，非一致性 DMA 需要手动 clean/invalidate 缓存。v0.17.0 是 Phase 0 P0-E（多核 SMP）的终点版本，闭环多核能力，支撑"多核"出口标准中的"并发数据正确"验证项。

## What Changes

- 在现有 `crates/kernel/smp/` crate 新增 3 个源文件：
  - `crates/kernel/smp/src/coherence.rs`（~200 行）— 内存屏障（`dmb`/`dsb`/`isb`）与缓存操作（`cache_clean`/`cache_invalidate`）
  - `crates/kernel/smp/src/atomic_ops.rs`（~150 行）— `AtomicCounter` 原子计数器封装
  - `crates/kernel/smp/src/dma_coherent.rs`（~120 行）— `DmaBuffer` DMA 一致性管理
- 修改 `crates/kernel/smp/src/lib.rs` 添加 3 个模块声明与 `pub use` 导出
- 新增 2 个文档（放入 `docs/smp/`，遵循 §2.3.3 文档分类规则）：
  - `docs/smp/memory-coherence-design.md` — 内存一致性设计（屏障语义、缓存维护、与调度器/IPI 的关系）
  - `docs/smp/armv8-memory-model.md` — ARMv8 内存模型（弱序模型、Acquire/Release、Shareability 域）
- 更新构建系统：
  - 根 `Cargo.toml`：workspace version `0.16.0` → `0.17.0`
  - `crates/kernel/smp/Cargo.toml`：version `0.15.0` → `0.17.0`
  - `Makefile`：VERSION `0.16.0` → `0.17.0`（smp-build/smp-test 目标已存在，无需新增）
  - `.github/workflows/ci.yml`：版本标识 `v0.16.0` → `v0.17.0`（smp cross-build 步骤已存在，无需新增）
  - `ci/src/gate.rs`：注释追加 `eneros-smp (v0.17.0 coherence)`

## Impact

- **Affected specs**: v0.16.0（sched crate 依赖 smp 的原子语义，未来集成）、v0.20.0（IPC 依赖屏障）、v0.21.0（SPSC ring 依赖原子操作）
- **Affected code**: 仅修改 `crates/kernel/smp/`（新增 3 文件 + 修改 lib.rs + Cargo.toml），以及 4 个构建配置文件
- **不修改** 任何其他 crate 源码（sched/panic/time/watchdog/hal/kernel 等）
- **不涉及** §2.5 目录迁移（迁移已由用户完成）

## ADDED Requirements

### Requirement: 内存屏障

系统 SHALL 提供 ARMv8 内存屏障指令的封装函数，支持数据内存屏障（DMB）、数据同步屏障（DSB）和指令同步屏障（ISB）。

#### Scenario: 屏障使用
- **WHEN** 多核代码需要在 store 后保证可见性
- **THEN** 调用 `dmb()` 插入 Inner Shareable 屏障
- **AND** 在 host（非 aarch64）侧调用为 no-op（不 panic）

#### Scenario: 缓存维护
- **WHEN** DMA 发送前需要写回脏数据
- **THEN** 调用 `cache_clean(addr, size)` 对齐到 cacheline（64B）后逐行 `dc civac`
- **AND** 操作完成后自动执行 `dsb()` 保证完成

#### Scenario: 缓存失效
- **WHEN** DMA 接收后需要丢弃缓存行从内存重读
- **THEN** 调用 `cache_invalidate(addr, size)` 对齐到 cacheline 后逐行 `dc ivac`
- **AND** 操作完成后自动执行 `dsb()`

### Requirement: 原子计数器

系统 SHALL 提供基于 `core::sync::atomic::AtomicU64` 的原子计数器封装，支持 relaxed increment 和 acquire/release 语义的 load/store。

#### Scenario: 原子自增
- **WHEN** 调用 `AtomicCounter::inc()`
- **THEN** 使用 `fetch_add(1, Relaxed)` 自增
- **AND** 返回自增后的新值

#### Scenario: 原子读取
- **WHEN** 调用 `AtomicCounter::load()`
- **THEN** 使用 `Acquire` 语义加载，保证后续读操作看到最新值

### Requirement: DMA 一致性缓冲

系统 SHALL 提供 `DmaBuffer` 结构体管理 DMA 缓冲的一致性，根据 `coherent` 标志决定是否手动 clean/invalidate。

#### Scenario: 硬件一致性 DMA
- **WHEN** `DmaBuffer.coherent == true`
- **THEN** `sync_for_device()` 和 `sync_for_cpu()` 均为 no-op（硬件自动维护一致性）

#### Scenario: 非一致性 DMA 发送
- **WHEN** `DmaBuffer.coherent == false` 且 CPU 写完数据准备 DMA 发送
- **THEN** 调用 `sync_for_device()` 执行 `cache_clean` 写回脏数据

#### Scenario: 非一致性 DMA 接收
- **WHEN** `DmaBuffer.coherent == false` 且 DMA 写完数据 CPU 准备读取
- **THEN** 调用 `sync_for_cpu()` 执行 `cache_invalidate` 丢弃缓存从内存重读

## MODIFIED Requirements

### Requirement: smp crate 模块结构

`crates/kernel/smp/src/lib.rs` SHALL 新增 `pub mod coherence` / `pub mod atomic_ops` / `pub mod dma_coherent` 三个模块声明，并 re-export 关键 API：`dmb`/`dsb`/`isb`/`cache_clean`/`cache_invalidate`/`AtomicCounter`/`DmaBuffer`。

### Requirement: 构建系统版本

workspace `Cargo.toml` 的 version SHALL 升级至 `0.17.0`；`crates/kernel/smp/Cargo.toml` 的 version SHALL 升级至 `0.17.0`；`Makefile` VERSION SHALL 升级至 `0.17.0`；CI 版本标识 SHALL 升级至 `v0.17.0`。

## Design Decisions

### D1: aarch64 inline asm cfg gate

- **决策**：所有 `asm!` 调用（`dmb`/`dsb`/`isb`/`dc civac`/`dc ivac`）用 `#[cfg(target_arch = "aarch64")]` gate，host 侧（x86_64）提供 no-op stub
- **理由**：
  1. 与 v0.15.0 smp crate 现有模式一致（`boot.rs`/`ipi.rs` 均如此）
  2. host 侧单元测试可运行（验证对齐计算、逻辑流程不 panic）
  3. aarch64 交叉编译验证真实指令生成

### D2: AtomicCounter 用 core::sync::atomic::AtomicU64

- **决策**：不引入外部并发 crate，直接用 `core::sync::atomic::AtomicU64`
- **理由**：
  1. `core::sync::atomic` 是 no_std 可用的标准原子类型
  2. 无需新增外部依赖
  3. `AtomicU64` 在 aarch64 上有原生支持（`ldxr`/`stxr`/`ldar`/`stlr`）

### D3: DmaBuffer 含裸指针，不 impl Send/Sync

- **决策**：`DmaBuffer` 含 `virt: *mut u8`，不为它实现 `Send`/`Sync`
- **理由**：
  1. 遵循蓝图设计（§4.1 数据结构）
  2. DMA 缓冲由驱动层管理生命周期，跨核传递需调用方自行确保安全
  3. 保持简单（Karpathy 原则 2），不为假设场景过度设计

### D4: cacheline 64 字节，对齐处理

- **决策**：cacheline 大小固定 64 字节（ARMv8 标准），`cache_clean`/`cache_invalidate` 内部对地址向下对齐到 64B 边界，大小向上扩展到覆盖完整 cacheline
- **理由**：
  1. ARMv8 AArch64 cacheline 通常是 64 字节
  2. 蓝图代码片段（§4.5）已给出对齐算法
  3. 避免部分 cacheline 操作导致的数据不一致

### D5: cache_clean 用 `dc civac`（clean + invalidate）

- **决策**：`cache_clean` 函数使用 `dc civac`（clean and invalidate by VA to PoC），而非 `dc cvac`（仅 clean）
- **理由**：遵循蓝图代码片段（§4.5），蓝图设计如此

### D6: 文档放 docs/smp/

- **决策**：两份新文档放入 `docs/smp/` 子目录，不放在 `docs/` 根
- **理由**：
  1. 遵循新规则 §2.3.3（多核/调度相关文档放 `docs/smp/`）
  2. `docs/smp/` 已存在（含 v0.15.0/v0.16.0 的 smp/sched 文档）
  3. 规则 §8.1 禁忌 #10 明确禁止文档平面化放 `docs/` 根

### D7: 不涉及目录迁移

- **决策**：v0.17.0 不执行 §2.5 目录迁移
- **理由**：目录迁移已由用户独立完成（所有 crate 已在 `crates/<subsystem>/` 下），v0.17.0 只做蓝图交付物

### D8: 非瓶颈版本合规

- **决策**：v0.17.0 非瓶颈版本（无 ★ 标记），但蓝图提供了完整代码片段，直接采用蓝图代码
- **理由**：蓝图 §4.5 代码片段完整可编译，无需 stub

## Constraints

- **no_std**：`#![cfg_attr(not(test), no_std)]`（smp crate 已有，无需修改）
- **零新增外部依赖**：仅用 `core::sync::atomic`（标准库 no_std 可用）
- **aarch64 cfg gate**：所有 `asm!` 用 `#[cfg(target_arch = "aarch64")]` gate
- **测试串行化**：全局静态变量测试用 `TEST_LOCK: std::sync::Mutex<()>` 模式（与 v0.15.0 一致）
- **代码量**：~470 行（3 个源文件），符合蓝图估算
