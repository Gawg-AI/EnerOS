# EnerOS v0.12.0 RTC 驱动 + 系统时钟服务 Spec

## Why

EnerOS 当前（v0.11.0）已有堆分配能力，但缺乏时间服务——无单调时钟则调度器无法
定时、看门狗无法计时、TTL 无法过期。v0.12.0 是 P0-D（时钟看门狗 Panic）的起点，
也是 Phase 0 关键瓶颈版本（★），阻塞 v0.13.0（看门狗）、v0.19.0（分区调度器）、
v0.22.0（TTL）等 6+ 后续版本。

## What Changes

- 新建 `time` crate（no_std），实现 RTC 驱动、单调时钟、高精度定时器、时间 API
- RTC 驱动基于 PL031（QEMU virt 内置 RTC @ 0x09010000），提供墙钟时间
- 单调时钟复用 HAL 的 `HalClock` trait（v0.6.0 `Arm64Timer`），不重复读取 `cntpct_el0`
- 高精度定时器使用数组式 TimerWheel（64 槽），`spin::Mutex` 保证线程安全
- 时间 API：`get_time()` / `get_monotonic_ns()` / `sleep_until()` / `register_timer()` 等
- 更新 workspace 版本号至 0.12.0
- 新增 3 篇文档：《RTC 驱动设计》、《系统时钟服务》、《hrtimer 实现》

## Impact

- **Affected specs**: 无（新功能，不修改现有 spec）
- **Affected code**:
  - 新增 `time/` crate（4 个源文件 + Cargo.toml）
  - 修改 `Cargo.toml`（workspace members + 版本号）
  - 修改 `Makefile`（VERSION + 新目标）
  - 修改 `.github/workflows/ci.yml`（版本标识 + cross-build 步骤）
  - 修改 `ci/src/gate.rs`（注释更新）
- **Affected docs**: 新增 `docs/rtc-driver-design.md`、`docs/system-clock-service.md`、`docs/hrtimer-implementation.md`

## ADDED Requirements

### Requirement: RTC 驱动（PL031）

系统 SHALL 提供 PL031 RTC 驱动，支持读取和设置墙钟时间。

#### Scenario: 读取 RTC 时间
- **WHEN** 调用 `rtc_read()`
- **THEN** 返回 `RtcTime` 结构体（year/month/day/hour/minute/second/weekday）

#### Scenario: 设置 RTC 时间
- **WHEN** 调用 `rtc_write(time)`
- **THEN** PL031 的 `RTCLOAD` 寄存器被写入对应的 Unix 秒数

#### Scenario: RTC 电池失效
- **WHEN** RTC 读取返回 0 或无效值
- **THEN** 返回默认时间 1970-01-01，不 panic

### Requirement: 单调时钟

系统 SHALL 提供基于 ARM Generic Timer 的单调时钟，自 `time_init()` 起从 0 开始计数，永不回退。

#### Scenario: 获取单调时间
- **WHEN** 调用 `get_monotonic_ns()`
- **THEN** 返回自 `time_init()` 以来的纳秒数（u64）

#### Scenario: 单调性保证
- **WHEN** 连续两次调用 `get_monotonic_ns()`
- **THEN** 第二次返回值 ≥ 第一次返回值

### Requirement: 高精度定时器

系统 SHALL 提供一次性定时器和周期定时器，支持注册和取消。

#### Scenario: 注册一次性定时器
- **WHEN** 调用 `register_timer(deadline_ns, callback)`
- **THEN** 返回 `TimerId`，`TimerWheel::tick()` 在 deadline 到期时调用 callback

#### Scenario: 注册周期定时器
- **WHEN** 调用 `register_periodic(period_ns, callback)`
- **THEN** 返回 `TimerId`，每 period_ns 重复调用 callback

#### Scenario: 取消定时器
- **WHEN** 调用 `cancel_timer(id)`
- **THEN** 对应定时器从 TimerWheel 移除，不再触发 callback

### Requirement: 时间 API

系统 SHALL 提供统一的时间 API 接口。

#### Scenario: 获取墙钟时间
- **WHEN** 调用 `get_time()`
- **THEN** 返回 `TimeStamp`（Unix epoch 纳秒数），值为 RTC 基准 + 单调偏移

#### Scenario: 睡眠到指定时刻
- **WHEN** 调用 `sleep_until(deadline_ns)`
- **THEN** CPU 进入低功耗等待（`wfe`）直到单调时钟达到 deadline

#### Scenario: 未初始化
- **WHEN** 未调用 `time_init()` 就调用 `get_monotonic_ns()`
- **THEN** 返回 0，不 panic

### Requirement: no_std 合规

`time` crate SHALL 遵循蓝图 §43.1 no_std 要求，正式构建为 no_std，测试构建链接 std。

### Requirement: HAL 复用

`time` crate SHALL 复用 HAL 的 `HalClock` trait（v0.6.0），不直接使用内联汇编读取 `cntpct_el0`。

## MODIFIED Requirements

无。

## REMOVED Requirements

无。

---

## 设计决策

### D1: 新建顶层 `time/` crate

与 `heap/`、`mm/` 一致，`time` 作为顶层 workspace 成员。

### D2: 复用 HAL `HalClock` trait

`time` crate 依赖 `hal` crate，通过 `HalClock::now_ns()` 获取单调时间。不重复
`cntpct_el0` 内联汇编。`time_init()` 接受 `&'static dyn HalClock` 参数，支持
依赖注入（aarch64 用 `Arm64Timer`，测试用 `MockHal`）。

### D3: PL031 RTC 驱动（QEMU virt）

QEMU virt 机器内置 PL031 RTC @ 0x09010000，提供 Unix 秒数。驱动通过 MMIO 读写
`RTCDR`（数据寄存器）和 `RTCLOAD`（加载寄存器）。`RtcTime` 与 Unix 秒数之间
通过标准日历算法转换。

### D4: 数组式 TimerWheel（64 槽）

蓝图提到"简化：实际用最小堆"。采用 64 槽数组实现，足够 Phase 0 使用。`tick()`
遍历数组检查到期定时器，返回下一个最近 deadline。

### D5: `spin::Mutex` 线程安全

`TimerWheel` 和全局状态用 `spin::Mutex` 保护，支持中断上下文和线程上下文并发访问。

### D6: `cfg_attr(not(test), no_std)` 模式

正式构建 no_std，测试构建链接 std。与 `heap`、`user/heap` crate 一致。

### D7: 日历转换算法

实现 `secs_to_rtc(secs: u64) -> RtcTime` 和 `rtc_to_secs(t: &RtcTime) -> u64`，
使用标准 days-since-epoch 算法（Howard Hinnant 的 civil_from_days）。不依赖
任何外部日历库。
