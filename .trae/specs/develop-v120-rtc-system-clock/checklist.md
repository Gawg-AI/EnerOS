# Checklist — EnerOS v0.12.0 RTC 驱动 + 系统时钟服务

> **蓝图依据**：`蓝图/phase0.md` §v0.12.0
> **版本类型**：★瓶颈版本（代码必须骨架可用）
> **合规性**：蓝图 §43.1（no_std）、§43.2（瓶颈版本代码可用性）

---

## 1. Crate 骨架

- [x] `time/Cargo.toml` 存在，name = "eneros-time"，version = "0.12.0"
- [x] `time/Cargo.toml` 依赖 `eneros-heap`（无）、`hal`（path）、`spin`
- [x] `time/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `time/src/lib.rs` 声明 `pub mod rtc/monotonic/hrtimer/api`
- [x] workspace `Cargo.toml` members 含 "time"
- [x] workspace `Cargo.toml` version = "0.12.0"

## 2. RTC 驱动（rtc.rs）

- [x] `RtcTime` 结构体含 year(u16)/month(u8)/day(u8)/hour(u8)/minute(u8)/second(u8)/weekday(u8)
- [x] `TimeStamp(pub u64)` 类型定义（derive Clone/Copy/Debug/PartialEq/Eq/PartialOrd/Ord）
- [x] `Pl031Rtc` 结构体含 base: u64
- [x] `Pl031Rtc::read_secs(&self) -> u64` 通过 MMIO 读取 RTCDR
- [x] `Pl031Rtc::write_secs(&self, secs: u64)` 通过 MMIO 写 RTCLOAD
- [x] `Pl031Rtc::read(&self) -> RtcTime` 读取并转换为 RtcTime
- [x] `Pl031Rtc::write(&self, t: RtcTime)` 转换为秒并写入
- [x] `secs_to_rtc(secs: u64) -> RtcTime` 正确实现日历转换
- [x] `rtc_to_secs(t: &RtcTime) -> u64` 正确实现反向转换
- [x] `weekday_from_secs(secs: u64) -> u8` 正确计算星期（0=周日）
- [x] RTC 读取失败（返回 0）时返回 1970-01-01，不 panic
- [x] 单元测试覆盖：secs↔rtc 往返、1970-01-01 边界、闰年 2024-02-29、月末、weekday

## 3. 单调时钟（monotonic.rs）

- [x] `MonotonicClock` 结构体含 boot_ns: u64
- [x] `MonotonicClock::init(clock: &dyn HalClock) -> Self` 记录启动时间
- [x] `MonotonicClock::now_ns(&self, clock: &dyn HalClock) -> u64` 返回自启动的纳秒数
- [x] 使用 `saturating_sub` 防止下溢
- [x] 单元测试验证单调性（第二次 ≥ 第一次）
- [x] 单元测试验证 boot 偏移（初始为 0）

## 4. 高精度定时器（hrtimer.rs）

- [x] `TimerId(pub u64)` 定义（derive Clone/Copy/Debug/PartialEq/Eq）
- [x] `HrTimer` 结构体含 id/deadline_ns/callback/periodic/period_ns
- [x] `TimerWheel` 结构体含 timers: [Option<HrTimer>; 64]/count/next_id
- [x] `TimerWheel::new()` 是 const fn
- [x] `TimerWheel::add()` 返回 `Option<TimerId>`（槽位满返回 None）
- [x] `TimerWheel::cancel()` 按 id 移除定时器
- [x] `TimerWheel::tick(now_ns)` 处理到期定时器，返回下一个 deadline（u64::MAX 表示无）
- [x] `tick()` 正确处理周期定时器（更新 deadline，不移除）
- [x] `tick()` 正确处理一次性定时器（调用后移除）
- [x] 单元测试覆盖：add/tick/cancel、周期定时器、槽位满、空 wheel tick

## 5. 时间 API（api.rs + lib.rs）

- [x] 全局静态：CLOCK（Mutex<Option<&dyn HalClock>>）、MONO（Mutex<Option<MonotonicClock>>）、WHEEL（Mutex<TimerWheel>）、RTC_OFFSET_NS（Mutex<u64>）
- [x] `time_init(clock: &'static dyn HalClock, rtc_base: u64)` 初始化所有全局状态
- [x] `get_monotonic_ns() -> u64` 未初始化返回 0
- [x] `get_time() -> TimeStamp` 返回 RTC offset + monotonic
- [x] `sleep_until(deadline_ns: u64)` aarch64 用 wfe，host 用 busy-wait
- [x] `register_timer(deadline_ns, cb) -> Option<TimerId>`
- [x] `register_periodic(period_ns, cb) -> Option<TimerId>`
- [x] `cancel_timer(id: TimerId)`
- [x] `rtc_read() -> RtcTime`
- [x] `rtc_write(t: RtcTime)`
- [x] `timer_expired_count() -> u64` 可观测性
- [x] 集成测试覆盖：未初始化、time_init、get_time、get_monotonic_ns、register/cancel timer

## 6. no_std 合规

- [x] `time/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`（除 `#[cfg(test)]` 模块内）
- [x] 使用 `spin::Mutex` 而非 `std::sync::Mutex`
- [x] 使用 `core::*` 而非 `std::*`

## 7. HAL 复用

- [x] `time/Cargo.toml` 依赖 `hal`（path = "../hal"）
- [x] `monotonic.rs` 使用 `hal::HalClock` trait
- [x] 不直接使用 `core::arch::asm!("mrs ..., cntpct_el0")`（由 HAL 提供）
- [x] `time_init()` 接受 `&'static dyn HalClock` 参数（依赖注入）

## 8. 构建系统

- [x] `Makefile` VERSION := 0.12.0
- [x] `Makefile` 含 time-build / time-test 目标
- [x] `ci.yml` 版本标识 v0.12.0
- [x] `ci.yml` 含 "Build time crate" cross-build 步骤
- [x] `ci/src/gate.rs` 注释含 v0.12.0

## 9. 文档

- [x] `docs/rtc-driver-design.md` 存在（PL031 寄存器、RtcTime、日历转换）
- [x] `docs/system-clock-service.md` 存在（单调时钟、HalClock 复用、API）
- [x] `docs/hrtimer-implementation.md` 存在（TimerWheel、并发安全）

## 10. 验证

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-time --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-time` 全部通过（≥ 15 个测试）
- [x] `cargo build -p eneros-time --target aarch64-unknown-none` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部通过（回归）
- [x] `git status` 无垃圾文件
