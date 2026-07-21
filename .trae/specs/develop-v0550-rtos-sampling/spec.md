# v0.55.0 高频采样服务 Spec

## Why

v0.54.0 已交付 RTOS 控制闭环引擎，但控制循环需要高频反馈数据源。v0.55.0 在 P1-H RTOS 组件第二层
交付 `rtos-sampling` crate，以 100ms（或更高）周期从设备协议读取数据并写入双缓冲共享内存快照，
供 Agent 和控制循环无锁读取。快照机制避免高频采样对 Agent 的频繁唤醒，是 v0.56.0 命令消费、
v0.58.0 端到端降级流程的数据基础。

## What Changes

- **ADDED** 新 crate `eneros-rtos-sampling`，位于 `crates/kernel/rtos-sampling/`
  （子系统归属 `kernel`：采样在内核态 RTOS 分区运行，与 rtos-control/controlbus 同层）
- **ADDED** `SampledPoint` 结构体：point_id / value / quality（3 字段，Copy）
- **ADDED** `StateSnapshot` 状态快照：timestamp / seq / point_count / points 数组（MAX_POINTS=256）
- **ADDED** `SharedMemorySnapshot` 双缓冲无锁快照：AtomicU8 活跃指针 + AtomicU64 写入序列号
  - `write(timestamp_us, points)`：写非活跃缓冲区 → 原子切换
  - `read()`：读活跃缓冲区 + 序列号一致性验证 + 重试上限（D4：避免无限重试）
- **ADDED** `SamplingService<P: PointAccess>` 泛型采样服务：point_ids / period_us / snapshot / protocol
  - `sample(now_us) -> SampleReport`：批量读点 → 写快照 → 更新统计（单步驱动，D5）
- **ADDED** `SamplingStats` 统计：sample_count / read_failures / last_sample_time_us
- **ADDED** `SamplingError` 错误类型
- **ADDED** 设计文档 `docs/kernel/rtos-sampling-design.md`（12 章节 + Mermaid 双缓冲图 + 时序图）

## Impact

- **Affected specs**：
  - 依赖 `eneros-protocol-abstract`（v0.51.0）：复用 `PointAccess` trait 读取设备数据
  - 依赖 `eneros-upa-model`（v0.50.0）：复用 `PointId` / `PointValue` / `DataPoint` / `PointQuality`
  - 不依赖 `eneros-time` hrtimer：采样周期由 v0.19.0 分区调度器触发，本 crate 提供 `sample(now_us)` 单步接口
  - 不依赖 `eneros-rtos-control`（v0.54.0）：独立数据采集层，不参与控制计算
- **Affected code**：
  - 新增 `crates/kernel/rtos-sampling/`（Cargo.toml + 7 源文件 + 测试模块）
  - 修改根 `Cargo.toml`：members 添加新 crate，workspace 版本 0.54.0 → 0.55.0
  - 修改 `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`：版本同步
- **Downstream unlocks**：v0.56.0 命令消费（从快照读状态）、v0.58.0 端到端降级流程

## ADDED Requirements

### Requirement: StateSnapshot 状态快照

系统 SHALL 提供 `StateSnapshot` 结构体，包含时间戳、序列号、点数和最多 256 个采样点。

#### Scenario: 快照写入与读取

- **WHEN** `write(timestamp_us=1000, points=[(pid=1, v=50.0, q=1), (pid=2, v=60.0, q=1)])`
- **THEN** `read()` 返回 `Some(StateSnapshot{ timestamp: 1000, seq: 1, point_count: 2, points[0..2] = ... })`

#### Scenario: 序列号递增

- **WHEN** 连续调用 3 次 `write`
- **THEN** `read()` 返回的快照 `seq` 依次为 1, 2, 3

### Requirement: SharedMemorySnapshot 双缓冲无锁

系统 SHALL 提供基于双缓冲 + 原子指针切换的无锁快照，读取端通过序列号验证一致性。

#### Scenario: 读写并发不冲突

- **WHEN** 写入端调用 `write`，同时读取端调用 `read`
- **THEN** 读取端要么读到旧快照，要么读到新快照，不会读到半写状态

#### Scenario: 重试上限

- **WHEN** 读取端在 `MAX_READ_RETRIES=10` 次重试后仍无法读到一致状态
- **THEN** `read()` 返回 `None`（D4：避免无限重试）

### Requirement: SamplingService 采样服务

系统 SHALL 提供泛型 `SamplingService<P: PointAccess>`，单步 `sample(now_us)` 读取所有采样点并写入快照。

#### Scenario: 正常采样

- **WHEN** `point_ids=[1,2,3]`，protocol 返回 3 个有效点
- **THEN** 快照 point_count=3，stats.sample_count 递增

#### Scenario: 部分点读取失败

- **WHEN** 3 个采样点中 1 个 `read_point` 返回 Err
- **THEN** 快照仅含 2 个成功点，stats.read_failures 递增 1

#### Scenario: 空采样点列表

- **WHEN** `point_ids=[]`
- **THEN** 快照 point_count=0，不报错

## MODIFIED Requirements

### Requirement: 工作区版本同步

工作区版本号 SHALL 从 `0.54.0` 更新为 `0.55.0`，涉及根 `Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs`。

## 偏差声明（D1~D10）

| 偏差 | 说明 |
|------|------|
| **D1** | 时间戳用 `u64` 微秒参数注入（蓝图 `MonotonicTime::now()` 在 no_std 不存在；与 v0.50.0 D1、v0.54.0 D1 一致） |
| **D2** | crate 放入 `crates/kernel/rtos-sampling/`（P1-H RTOS 组件第二层，与 rtos-control/controlbus 同属 kernel 子系统） |
| **D3** | 不直接依赖 seL4 SharedMemory 对象（蓝图含此项，但 seL4 集成属 Phase 3；本版本用 in-memory 双缓冲实现相同接口，后续 Phase 3 替换为 seL4 SharedMemory — Loosely Coupled） |
| **D4** | `read()` 增加重试上限 `MAX_READ_RETRIES=10`（蓝图风险 8.3 提及；避免高并发切换时读取端无限重试） |
| **D5** | 不实现阻塞式 `run()` 循环（蓝图 `sample()` 是单次执行；采样周期由 v0.19.0 分区调度器触发，本 crate 仅提供 `sample(now_us) -> SampleReport` 单步接口 — 与 v0.54.0 D3 一致） |
| **D6** | 不使用 `Box<dyn PointAccess>` 字段（蓝图 `protocol: Box<dyn PointAccess>` 在 no_std 无 alloc 时复杂；改为泛型 `<P: PointAccess>` — 与 v0.54.0 D6 一致） |
| **D7** | `SamplingStats` 不使用 `AtomicU64`（no_std 单线程采样，无需原子；用普通 `u64` — 与 v0.54.0 D8 一致） |
| **D8** | `SharedMemorySnapshot` 使用 `core::sync::atomic::{AtomicU8, AtomicU64}`（no_std 可用；双缓冲无锁必须原子操作，与 SamplingStats 单线程不同） |
| **D9** | `StateSnapshot.points` 用固定数组 `[SampledPoint; MAX_POINTS]` 而非 `Vec`（蓝图含此项；`#[repr(C)]` 固定大小便于页对齐共享内存 — Simplicity First） |
| **D10** | `PointQuality.valid` 映射为 `SampledPoint.quality` 的 `u8`（蓝图 `point.quality.valid as u8`；保留 0/1 简化跨进程共享 — Simplicity First） |
