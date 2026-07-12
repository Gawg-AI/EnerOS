# v0.19.0 分区调度器 Spec

## Why

v0.18.0 完成线程/任务抽象（TCB、上下文切换、优先级调度）后，调度器已具备"选下一个线程"的能力，但仍缺少**时间触发分区调度**——混合关键性系统的核心机制。ARINC 653 式分区调度保证 RTOS 分区获得确定性时间片，是"实时性"出口标准（分区抖动 < 1ms）的最终验证项。

v0.19.0 在 `eneros-sched` crate 内引入 MajorFrame 时间片表、周期定时器驱动的分区切换、WCET 估算与抖动测量，完成 P0-F（调度器）阶段，为 v0.20.0 IPC 跨分区通信提供基础。

> **注意**：v0.18.0（线程抽象）已于上一阶段完成，所有交付物已就绪（workspace 版本 0.18.0，`tcb.rs`/`switch.rs`/`priority.rs` 已存在，72 个测试通过）。本 spec 仅覆盖 v0.19.0 新增内容。

## What Changes

- **新增** `crates/kernel/sched/src/partition_sched.rs`（~280 行）— `PartitionId`、`PartitionSlot`、`MajorFrame` 数据结构，`schedule_add`/`schedule_run`/`schedule_stop`/`current_partition` API，`on_tick` 分区切换回调
- **新增** `crates/kernel/sched/src/timeline.rs`（~180 行）— MajorFrame 时间片配置、周期循环、slot 推进逻辑
- **新增** `crates/kernel/sched/src/wcet.rs`（~150 行）— WCET 静态估算表、`wcet_estimate(tid)`/`wcet_set(tid, ns)` API
- **新增** `crates/kernel/sched/src/jitter.rs`（~120 行）— `JitterStats` 聚合、`record_jitter`/`jitter_measure`/`jitter_reset` API
- **新增** 时间源注入机制（在 `partition_sched.rs` 中）— `set_time_source(fn() -> u64)` / `set_timer_registrar(fn(u64, fn()) -> bool)`，保持 sched crate 零外部依赖（D2）
- **修改** `crates/kernel/sched/src/lib.rs` — 添加 `pub mod partition_sched/timeline/wcet/jitter` 声明与 `pub use` 导出
- **修改** `crates/kernel/sched/Cargo.toml` — 版本 `0.18.0` → `0.19.0`
- **修改** 根 `Cargo.toml` — workspace 版本 `0.18.0` → `0.19.0`
- **修改** `Makefile` — `VERSION` `0.18.0` → `0.19.0`
- **修改** `.github/workflows/ci.yml` — 版本标识更新为 v0.19.0
- **修改** `ci/src/gate.rs` — 注释追加 v0.19.0
- **新增** `docs/smp/partition-scheduler-design.md` — 分区调度器设计文档
- **新增** `docs/smp/arinc653-adaptation.md` — ARINC 653 适配说明
- **新增** `docs/smp/wcet-analysis.md` — WCET 分析文档

## Impact

- **Affected specs**: v0.18.0（线程抽象，复用其 `Tcb.partition` 字段与 `Spinlock`）、v0.20.0（IPC，需跨分区）、v0.22.0（双平面联调，依赖分区调度）
- **Affected code**:
  - `crates/kernel/sched/` — 主要变更点（4 个新文件 + lib.rs 修改）
  - 根 `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs` — 版本同步
- **依赖关系**: v0.19.0 依赖 v0.18.0（TCB 的 `partition` 字段）与 v0.12.0（时钟，运行时通过函数指针注入时基，非编译期依赖）
- **回归风险**: sched crate 原零外部依赖，v0.19.0 保持此特性（时间源通过函数指针注入，非 crate 依赖）

## ADDED Requirements

### Requirement: 分区数据结构

系统 SHALL 提供 `PartitionId(pub u32)` 新类型、`PartitionSlot { partition, duration_ms }` 与 `MajorFrame { slots, slot_count, period_ms, current_slot, frame_start_ns }` 结构体。MajorFrame 最大 16 个 slot。

#### Scenario: 添加分区时间片
- **WHEN** `schedule_add(PartitionId(1), 5)` 被调用且当前 slot_count < 16
- **THEN** slot 被追加到 `MajorFrame.slots`，`period_ms` 累加 5，`slot_count` 递增

#### Scenario: 超出最大 slot 数
- **WHEN** `schedule_add` 在 slot_count == 16 时被调用
- **THEN** 返回错误或静默丢弃（不越界写入）

#### Scenario: 查询当前分区
- **WHEN** `current_partition()` 在 slot 2 执行期间被调用
- **THEN** 返回 `PartitionId(slots[2].partition)`

### Requirement: 时间源注入

系统 SHALL 提供函数指针注入机制以获取单调时间与注册周期定时器，保持 sched crate 零外部依赖。

#### Scenario: 默认无时间源
- **WHEN** 未调用 `set_time_source` 时 `now_ns()` 被调用
- **THEN** 返回 `0`

#### Scenario: 注入时间源后
- **WHEN** `set_time_source(|| 12345)` 被调用后 `now_ns()` 被调用
- **THEN** 返回 `12345`

#### Scenario: 注入定时器注册器后
- **WHEN** `set_timer_registrar(|ns, cb| { ... true })` 被调用后 `schedule_run()` 被调用
- **THEN** 定时器注册器被调用，返回 `Ok(())`

#### Scenario: 未注入定时器注册器
- **WHEN** 未调用 `set_timer_registrar` 时 `schedule_run()` 被调用
- **THEN** 返回 `Err(SchedError::NoTimerRegistrar)`，frame 仍初始化但不启动周期切换

### Requirement: 分区切换回调

系统 SHALL 提供 `on_tick()` 回调，由周期定时器触发，计算抖动、推进 slot、切换分区。

#### Scenario: 正常切换
- **GIVEN** MajorFrame 含 3 个 slot：`[(P0, 5ms), (P1, 20ms), (P0, 5ms)]`，当前 slot=0
- **WHEN** `on_tick()` 被调用
- **THEN** `current_slot` 推进到 1，`switch_partition(P1)` 被调用，抖动被记录

#### Scenario: 帧循环
- **GIVEN** 当前 slot 是最后一个（slot_count-1）
- **WHEN** `on_tick()` 被调用
- **THEN** `current_slot` 回到 0，`frame_start_ns` 更新为当前时间

### Requirement: 抖动测量

系统 SHALL 提供 `JitterStats { min_jitter_us, max_jitter_us, sum_jitter_us, samples }` 与 `record_jitter(j_us)`/`jitter_measure() -> JitterStats`/`jitter_reset()` API。

#### Scenario: 记录抖动
- **GIVEN** `JITTER` 初始为 `{min=MAX, max=MIN, sum=0, samples=0}`
- **WHEN** `record_jitter(100)` 被调用
- **THEN** `min=100, max=100, sum=100, samples=1`

#### Scenario: 聚合统计
- **GIVEN** 已记录 `[100, 200, 50]` 三次抖动
- **WHEN** `jitter_measure()` 被调用
- **THEN** 返回 `{min=50, max=200, sum=350, samples=3}`，`avg = 350/3 = 116`

#### Scenario: 重置统计
- **WHEN** `jitter_reset()` 被调用
- **THEN** `JITTER` 恢复为初始值 `{min=MAX, max=MIN, sum=0, samples=0}`

### Requirement: WCET 估算

系统 SHALL 提供基于静态表的 WCET 估算：`wcet_set(tid, ns)` 配置、`wcet_estimate(tid) -> u64` 查询。

#### Scenario: 默认 WCET
- **WHEN** `wcet_estimate(Tid(5))` 被调用且未设置过
- **THEN** 返回 `0`

#### Scenario: 设置后查询
- **WHEN** `wcet_set(Tid(5), 500_000)` 后 `wcet_estimate(Tid(5))` 被调用
- **THEN** 返回 `500_000`

#### Scenario: 分区超时检测
- **GIVEN** 分区 P0 时间片 5ms，P0 内线程 Tid(3) 的 WCET = 6ms
- **WHEN** `check_partition_overrun(P0)` 被调用
- **THEN** 返回 `Some(Tid(3))`（检测到超时）

## MODIFIED Requirements

### Requirement: eneros-sched crate

v0.18.0 提供 `Scheduler`/`PerCoreRq`/`CoreMask`/`Tid`/`Tcb`/`ThreadState`/`context_switch`/`thread_create` 等线程抽象 API。

v0.19.0 新增 `partition_sched`/`timeline`/`wcet`/`jitter` 模块，引入 ARINC 653 式时间触发分区调度。`Tcb.partition` 字段（v0.18.0 已预留，`u32` 类型）用于关联线程与分区。不重定义现有类型，不修改现有 API 签名。

## Design Decisions

- **D1（时间源函数指针注入）**：sched crate 保持零外部依赖（v0.16.0 D2）。分区调度需要单调时间与周期定时器，但直接依赖 `eneros-time` 会引入 `eneros-hal` 间接依赖，破坏独立性并增加 host 测试复杂度。改用 `static TIME_SOURCE: Spinlock<Option<fn() -> u64>>` + `static TIMER_REGISTRAR: Spinlock<Option<fn(u64, fn()) -> bool>>` 函数指针注入，由调用方（有 `eneros-time` 访问权）在初始化时设置。host 测试用 mock 函数或直接调用 `on_tick()` 跳过定时器。
- **D2（Spinlock 替代 static mut）**：蓝图代码用 `static mut FRAME`/`static mut JITTER`，这在 Rust 2024 edition 下不安全且触发 clippy 警告。改用 `Spinlock<MajorFrame>`/`Spinlock<JitterStats>`（复用 `percore::Spinlock`），与 v0.18.0 `THREAD_TABLE` 设计一致。const 初始化。
- **D3（PartitionId 新类型）**：`pub struct PartitionId(pub u32)`，与 `Tid(pub u32)` 风格一致。`Tcb.partition: u32` 字段（v0.18.0 已预留）存储 `PartitionId.0`，不引入类型转换开销。
- **D4（MajorFrame 容量）**：蓝图 `[PartitionSlot; 16]`，定义 `const MAX_SLOTS: usize = 16`。`slot_count` 跟踪实际使用数。超出的 `schedule_add` 返回 `Err(SchedError::SlotFull)`。
- **D5（JitterStats 字段修正）**：蓝图 `JitterStats` 有 `sum_jitter_us` 但 `jitter_measure()` 返回时将其作为 `avg`——命名混淆。修正：保留 `sum_jitter_us` 字段，`jitter_measure()` 返回的 `JitterStats` 中 `sum_jitter_us` 仍为总和，调用方自行计算 `avg = sum / samples`。或新增 `avg_jitter_us` 计算字段。选择后者：返回时计算 `avg = if samples > 0 { sum / samples as i64 } else { 0 }` 放入 `sum_jitter_us`（保持与蓝图字段名一致，但语义为 avg）。**最终决策**：字段名保持 `sum_jitter_us`（总和），`jitter_measure()` 返回原始统计，调用方按需计算 avg。避免字段语义混淆。
- **D6（WCET 静态表）**：`static WCET_TABLE: Spinlock<[u64; MAX_THREADS]>`，默认全 0。`wcet_set`/`wcet_estimate` 通过 `Tid.0` 索引。`check_partition_overrun(partition)` 遍历该分区所有线程，若有 `wcet_estimate(tid) > slot.duration_ms * 1_000_000` 则返回该 Tid。简单但满足蓝图 §4.4 "分区超时检测"需求。
- **D7（schedule_run 返回 Result）**：蓝图 `schedule_run()` 无返回值。但若未注入定时器注册器则无法启动周期切换。改为 `schedule_run() -> Result<(), SchedError>`，`NoTimerRegistrar` 错误时 frame 仍初始化（允许手动调用 `on_tick` 测试）但不启动定时器。
- **D8（非瓶颈版本）**：v0.19.0 非蓝图标记的瓶颈版本（★），代码"骨架可用"——分区切换算法、抖动聚合、WCET 检测必须完整实现（无 `todo!()`/`unimplemented!()`），但抖动 < 1ms 性能验证延后至 QEMU 实机（与 v0.18.0 < 2μs 同策略）。
- **D9（文档位置）**：遵循规则 §2.3.3，三份文档放 `docs/smp/`（与 v0.16.0/v0.18.0 调度器文档保持一致）。
- **D10（测试策略）**：
  - MajorFrame 配置与 slot 推进：host 可测（无需定时器）
  - 抖动聚合：host 可测
  - WCET 表：host 可测
  - `on_tick` 逻辑：host 可测（直接调用，跳过定时器）
  - `schedule_run` 无定时器注册器：host 可测（验证返回错误）
  - `schedule_run` 有 mock 定时器注册器：host 可测（验证注册器被调用）
  - 分区抖动 < 1ms：延后至 QEMU 实机
  - 单元测试覆盖率 ≥ 80%（蓝图 §6.1）
- **D11（switch_partition 实现）**：蓝图 `switch_partition` 是空 stub。v0.19.0 实现：设置 `CURRENT_PARTITION` 静态变量，记录分区切换次数。实际的线程集切换（阻塞非当前分区线程、唤醒当前分区线程）依赖 v0.18.0 `thread_block`/`thread_resume`，但在无实际线程运行的 host 测试中仅记录切换。aarch64 实机时由调用方集成。
