# v0.54.0 RTOS 控制闭环引擎 Spec

## Why

储能系统需要在 RTOS 快平面以 10ms 固定周期执行功率控制、电压调节、频率响应等实时控制任务，
且周期抖动必须 < 1ms。v0.53.x 已交付 SOE/MQTT/告警等业务层，但缺少承载这些业务的实时控制骨架。
v0.54.0 在 P1-H RTOS 组件首层交付 `rtos-control` crate，提供 PID 算法 + 控制循环 trait + 引擎骨架，
是 v0.55.0 高频采样、v0.56.0 命令消费、v0.58.0 降级流程的前置依赖。

## What Changes

- **ADDED** 新 crate `eneros-rtos-control`，位于 `crates/kernel/rtos-control/`
  （子系统归属 `kernel`：控制闭环在内核态 RTOS 分区运行，时间触发调度，与 controlbus/sched 同层）
- **ADDED** `PidController` 结构体：P/I/D 三参数 + 积分限幅 + 输出限幅 + 抗饱和（Anti-Windup）
- **ADDED** `SetpointTracker` 设定值跟踪器：支持斜率限制（rate limit），防止设定值跳变引发超调
- **ADDED** `ControlLoop` trait：`name` / `period_us` / `init` / `execute` / `shutdown` 五个方法
- **ADDED** `ControlLoopEngine` 引擎：注册多循环、按最小周期驱动、统计周期抖动与执行时间
- **ADDED** `PowerControlLoop` 示例实现：从 Control Bus 取设定值 → 读反馈 → PID → 写下发
- **ADDED** `ControlError` 错误类型 + `EngineStats` 统计结构
- **ADDED** 设计文档 `docs/kernel/rtos-control-loop-design.md`（12 章节 + Mermaid 架构图 + 时序图）

## Impact

- **Affected specs**：
  - 依赖 `eneros-controlbus`（v0.22.0）：复用 `ControlCommand` / `command_consume` / `ControlAction` / `setpoint`
  - 依赖 `eneros-protocol-abstract`（v0.51.0）：复用 `PointAccess` trait 读写反馈与下发
  - 依赖 `eneros-upa-model`（v0.50.0）：复用 `PointId` / `PointValue` / `DataPoint`
  - 不依赖 `eneros-time` hrtimer 直接调用：时间触发由 v0.19.0 分区调度器负责，本 crate 仅提供
    "单次执行步" (`tick`)，由外部调度器循环调用，避免阻塞式 `run() -> !` 在 no_std 单线程下无法测试
- **Affected code**：
  - 新增 `crates/kernel/rtos-control/`（Cargo.toml + 8 源文件 + 1 mock + 测试模块）
  - 修改根 `Cargo.toml`：members 添加新 crate，workspace 版本 0.53.0 → 0.54.0
  - 修改 `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`：版本同步
- **Downstream unlocks**：v0.55.0 高频采样、v0.56.0 命令消费、v0.57.0 降级规则、v0.58.0 端到端降级流程

## ADDED Requirements

### Requirement: PidController 控制器

系统 SHALL 提供 PID 控制器，支持 P/I/D 三参数、积分限幅（anti-windup）、输出限幅、手动 reset。

#### Scenario: 阶跃响应

- **WHEN** 设定值 `setpoint=100.0`，过程变量 `process_variable=80.0`，`dt=0.01s`，`kp=1.0/ki=0.1/kd=0.01`
- **THEN** PID 输出 = `1.0 * 20.0 + 0.1 * (20.0 * 0.01) + 0.01 * (20.0 / 0.01) = 20.0 + 0.02 + 20.0 = 40.02`

#### Scenario: 积分限幅抗饱和

- **WHEN** `integral_limit=10.0`，连续 N 周期正向误差使积分累计超过 10.0
- **THEN** `integral` 被 clamp 到 10.0，避免积分饱和导致超调

#### Scenario: 输出限幅

- **WHEN** `output_limit=50.0`，PID 计算结果为 80.0
- **THEN** 返回值被 clamp 到 50.0

### Requirement: SetpointTracker 设定值跟踪

系统 SHALL 提供设定值跟踪器，支持斜率限制（rate limit），防止设定值跳变。

#### Scenario: 斜率限制

- **WHEN** 当前设定值 `current=50.0`，目标设定值 `target=100.0`，`max_rate_per_s=10.0`，`dt=0.01s`
- **THEN** 单步最大变化 `10.0 * 0.01 = 0.1`，`update()` 返回 `50.1` 而非 `100.0`
- **WHEN** 连续调用 500 次（5s 后）
- **THEN** 跟踪值收敛到 `100.0`

#### Scenario: 无斜率限制

- **WHEN** `max_rate_per_s = f64::MAX`（无限制）
- **THEN** `update(target)` 直接返回 `target`

### Requirement: ControlLoop 控制循环 trait

系统 SHALL 提供 `ControlLoop` trait，定义控制循环的统一接口。

#### Scenario: 注册与执行

- **WHEN** 引擎注册一个 `period_us=10000`（10ms）的 `ControlLoop` 实现
- **THEN** 引擎的 `tick(now_us, elapsed_us)` 在 `last_execute + 10000 <= now` 时调用 `execute(elapsed_us)`

#### Scenario: 错误隔离

- **WHEN** 某控制循环 `execute()` 返回 `Err`
- **THEN** 引擎记录错误统计但不影响其他循环执行

### Requirement: ControlLoopEngine 引擎

系统 SHALL 提供 `ControlLoopEngine`，支持注册多循环、按最小周期驱动、统计周期抖动。

#### Scenario: 多循环调度

- **WHEN** 引擎注册循环 A（10ms）和循环 B（20ms），调用 `tick(now=30000, elapsed=10000)`
- **THEN** 循环 A 在 30ms 触发（满足 `30000 - 20000 >= 10000`），循环 B 在 40ms 触发

#### Scenario: 抖动统计

- **WHEN** 循环目标周期 10000μs，实际 `elapsed_us=10500`
- **THEN** `JitterRecord` 记录 `jitter_us=500`，并更新 `max_jitter_us` / `avg_jitter_us`

### Requirement: PowerControlLoop 功率控制循环示例

系统 SHALL 提供一个 `PowerControlLoop` 示例，演示完整的"设定值→反馈→PID→下发"链路。

#### Scenario: 完整控制链路

- **WHEN** Agent 通过 Control Bus 下发 `setpoint=80.0`，反馈点读取 `process_variable=70.0`
- **THEN** PID 计算输出值，通过 `PointAccess::write_point` 写入下发点
- **WHEN** Control Bus 无新命令
- **THEN** 继续使用上一次设定值（保持）

## MODIFIED Requirements

### Requirement: 工作区版本同步

工作区版本号 SHALL 从 `0.53.0` 更新为 `0.54.0`，涉及：
- 根 `Cargo.toml` 的 `[workspace.package].version`
- `Makefile` 的 `VERSION` 变量
- `.github/workflows/ci.yml` 的版本字段
- `ci/src/gate.rs` 的 clippy/test 注释段补充 `eneros-rtos-control` 说明

## 偏差声明（D1~D12）

| 偏差 | 说明 |
|------|------|
| **D1** | 时间戳用 `u64` 微秒参数注入（蓝图 `MonotonicTime` / `Duration` 在 no_std 不存在；与 v0.50.0 D1、v0.53.0 D1 一致） |
| **D2** | crate 放入 `crates/kernel/rtos-control/`（P1-H RTOS 组件，控制循环在内核态 RTOS 分区运行，与 controlbus/sched 同属 kernel 子系统） |
| **D3** | 不实现阻塞式 `run() -> !`（蓝图含此项，但 `run() -> !` 在 no_std 单线程下无法测试；改为 `tick(now_us, elapsed_us) -> EngineTickStats` 单步驱动，由外部调度器循环调用 — Surgical Changes + Simplicity First） |
| **D4** | 不直接依赖 `eneros-time` 的 `Hrtimer` / `MonotonicClock`（时间触发由 v0.19.0 分区调度器负责；本 crate 通过 `tick(now_us)` 接收当前时间，避免跨子系统循环依赖 — Loosely Coupled） |
| **D5** | 不要求 `ControlLoop: Send + Sync`（蓝图含此项；no_std 单线程无需该约束，与 v0.51.0 D2、v0.53.0 D7 一致） |
| **D6** | 不使用 `Box<dyn PointAccess>` 字段（蓝图 `protocol: Box<dyn PointAccess>` 在 no_std 无 alloc 时复杂；改为泛型 `<P: PointAccess>` — Simplicity First） |
| **D7** | 不使用 `ControlBusReader`（蓝图含此项，但 controlbus crate 提供的是全局函数 `command_consume() -> Option<ControlCommand>`，非 reader 对象；`PowerControlLoop` 直接调用 `command_consume()`） |
| **D8** | `EngineStats` 不使用 `AtomicU64`（no_std 单线程无需原子操作；用普通 `u64` — 与 v0.53.0 D8 一致） |
| **D9** | `JitterRecord` 不使用 `BTreeMap<&str, u64>`（no_std 无 `&str` 作为 BTreeMap key 的生命周期问题；用 `[(name, stats); N]` 固定数组或 `Vec<(String, JitterStats)>` — Simplicity First） |
| **D10** | `PidController` 的 `clamp` 用 `core::cmp::min/max` 手写实现（no_std 无 `f64::clamp` 在某些 target 不可用） |
| **D11** | `SetpointTracker::update` 接受 `target` + `dt` 两个参数（蓝图 `set_setpoint(sp)` 仅设置目标，未考虑斜率限制的 dt；改为 `update(target, dt) -> f64` 一次完成 — Surgical Changes） |
| **D12** | 不实现 `PowerControlLoop::shutdown` 的复杂清理逻辑（示例实现，仅重置 PID；蓝图 `shutdown` 由具体设备实现） |
