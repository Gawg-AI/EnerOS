# EnerOS v0.16.0 多核调度器 Spec

## Why

v0.15.0 完成了多核启动与 IPI，但尚无调度框架——无法实现 RTOS 绑核独占 Core 0、Agent 分布 Core 1+ 的混合关键性架构。v0.16.0 是 Phase 0 关键瓶颈版本（★），需实现核亲和性、负载均衡、RTOS 绑核独占，支撑"多核"出口标准。

## What Changes

- 新增 `sched/` crate（no_std，零外部依赖），实现多核调度器
- 4 个源文件：
  - `sched/src/percore.rs`（~200 行）— 自旋锁 Spinlock、Tid、PerCoreRq per-core 运行队列
  - `sched/src/affinity.rs`（~250 行）— CoreMask 核掩码、set_affinity/pin_to_core
  - `sched/src/isolation.rs`（~150 行）— CoreReservation 核独占、RTOS 绑核
  - `sched/src/balance.rs`（~280 行）— Balancer 负载均衡器
- `sched/src/lib.rs` — Scheduler 主结构、sched_init、全局 API
- 3 份文档：多核调度器设计、核亲和性策略、RTOS 绑核方案
- 更新构建系统：Cargo.toml / Makefile / ci.yml / gate.rs

## Impact

- Affected specs: v0.15.0（smp crate 提供多核启动基础）、v0.17.0（一致性依赖调度迁移）、v0.18.0（线程抽象依赖）
- Affected code: 新增 `sched/` crate（6 个文件），修改 4 个构建配置文件
- **不修改** 任何现有 crate 源码（smp/panic/time/watchdog/hal 等）

## ADDED Requirements

### Requirement: 多核调度器框架

系统 SHALL 提供 per-core 运行队列调度器，支持核亲和性、负载均衡、RTOS 绑核独占。

#### Scenario: RTOS 独占 Core 0
- **WHEN** 调用 `reserve_core(0)` 标记 Core 0 为独占
- **AND** 非 RTOS 线程尝试加入 Core 0 的运行队列
- **THEN** 返回 `SchedError::CoreReserved`，线程被拒绝

#### Scenario: Agent 分布在 Core 1+
- **WHEN** Agent 线程就绪且无亲和性约束
- **AND** Core 0 已被 reserved
- **THEN** 线程被分配到 Core 1+ 的运行队列

#### Scenario: 负载均衡触发
- **WHEN** 各核负载差超过阈值（默认 2）
- **AND** 均衡器被触发（`balance_load()`）
- **THEN** 从最忙核迁移一个线程到最闲核

#### Scenario: 核亲和性设置
- **WHEN** 调用 `set_affinity(tid, cores)` 设置线程亲和性
- **THEN** 线程只能被调度到 `cores` 指定的核上
- **AND** 调用 `pin_to_core(tid, 3)` 等价于 `set_affinity(tid, CoreMask::single(3))`

#### Scenario: pick_next 选取下一个线程
- **WHEN** 核调度器调用 `pick_next(core)`
- **THEN** 从该核的运行队列取出一个可运行线程
- **AND** 返回 `None` 如果队列为空

### Requirement: 自旋锁（per-core RQ 锁）

系统 SHALL 提供轻量级自旋锁用于 per-core 运行队列保护，使用 `compare_exchange` + backoff。

#### Scenario: 锁竞争
- **WHEN** 多核同时尝试锁定同一个 per-core RQ
- **THEN** 使用 TAS（test-and-set）自旋，失败时 backoff（spin_loop）

### Requirement: 核掩码（CoreMask）

系统 SHALL 提供 64 位核掩码类型，支持位运算操作。

#### Scenario: 核掩码操作
- **WHEN** 创建 `CoreMask::single(3)` 
- **THEN** 仅第 3 位为 1
- **AND** `contains(3)` 返回 true，`contains(2)` 返回 false
- **AND** `count()` 返回 1

## MODIFIED Requirements

### Requirement: 构建系统
workspace `Cargo.toml` 的 members 列表 SHALL 添加 "sched"，workspace version SHALL 升级至 0.16.0。

### Requirement: CI 流水线
CI SHALL 包含 `sched` crate 的交叉编译步骤（`cargo build -p eneros-sched --target aarch64-unknown-none`）。

## Design Decisions

### D1: 自旋锁实现选择
- **决策**：使用蓝图提供的自定义 Spinlock（`compare_exchange` + backoff），不使用 `spin::Mutex`
- **理由**：
  1. v0.16.0 是瓶颈版本（★），蓝图明确提供了 Spinlock 代码
  2. 自定义 Spinlock 是 `const fn` newable，支持 `[PerCoreRq; 8]` 数组 const 初始化
  3. 蓝图设计中锁在 PerCoreRq 内部，允许细粒度锁定
  4. 零外部依赖（仅用 `core::sync::atomic::AtomicBool`）

### D2: sched crate 零外部依赖
- **决策**：sched crate 不依赖任何外部 crate（无 spin/heapless）
- **理由**：
  1. 自定义 Spinlock 替代 spin::Mutex
  2. 所有数据结构用固定大小数组（`[Option<Tid>; 64]`），无 heap 分配
  3. 符合"最简依赖图"原则
  4. 仅依赖 `core::*`

### D3: 不依赖 eneros-smp
- **决策**：sched crate 不依赖 eneros-smp crate
- **理由**：
  1. 蓝图设计中 `sched_init(core_count)` 接收核数作为参数
  2. IPI 集成由调用方（kernel/runtime）在更高层完成
  3. 保持 sched crate 自包含、可独立测试

### D4: 瓶颈版本合规（蓝图 §43.2）
- **决策**：所有代码必须"骨架可用"
- **要求**：
  1. 无 `todo!()` / `unimplemented!()` / 返回 null 的 stub
  2. 关键算法路径完整（负载均衡的扫描+迁移、自旋锁的 CAS+backoff）
  3. 所有接口签名与蓝图 §4.2 一致
  4. 标注"示例代码，非生产就绪"

## Constraints

- **no_std**：`#![cfg_attr(not(test), no_std)]`，生产代码仅用 `core::*`
- **aarch64 cfg gate**：调度器逻辑无 aarch64 专属代码（纯算法），不需要 cfg gate
- **测试串行化**：全局静态变量测试用 `TEST_LOCK: std::sync::Mutex<()>` 模式
- **代码量**：~880 行（4 个源文件），符合蓝图估算
