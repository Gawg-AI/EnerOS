# Tasks — EnerOS v0.12.0 RTC 驱动 + 系统时钟服务

> **蓝图依据**：`蓝图/phase0.md` §v0.12.0
> **版本类型**：★瓶颈版本（代码必须骨架可用，不能 stub）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动

--- [x] Task 1: 创建 `time` crate 骨架

- [x] SubTask 1.1: 创建 `time/Cargo.toml`（name=eneros-time, version=0.12.0, deps: eneros-hal, spin）
- [x] SubTask 1.2: 创建 `time/src/lib.rs`（`#![cfg_attr(not(test), no_std)]`，模块声明，公共 API re-export）
- [x] SubTask 1.3: 更新 workspace `Cargo.toml`（members 添加 "time"，version 改 0.12.0）
- [x] 验证：`cargo build -p eneros-time` 成功

## Task 2: 实现 RTC 驱动（`time/src/rtc.rs`）

- [x] SubTask 2.1: 定义 `RtcTime` 结构体（year/month/day/hour/minute/second/weekday）
- [x] SubTask 2.2: 定义 `TimeStamp(pub u64)` 类型（Unix epoch 纳秒数）
- [x] SubTask 2.3: 实现 `Pl031Rtc` 驱动（read_secs/write_secs/read/write，MMIO 寄存器操作）
- [x] SubTask 2.4: 实现日历转换 `secs_to_rtc(secs: u64) -> RtcTime`（Howard Hinnant 算法）
- [x] SubTask 2.5: 实现日历转换 `rtc_to_secs(t: &RtcTime) -> u64`
- [x] SubTask 2.6: 实现 `weekday_from_secs(secs: u64) -> u8`（0=周日）
- [x] SubTask 2.7: 编写单元测试（secs↔rtc 往返、边界值 1970-01-01、闰年、月末）
- [x] 验证：`cargo test -p eneros-time rtc` 通过

## Task 3: 实现单调时钟（`time/src/monotonic.rs`）

- [x] SubTask 3.1: 定义 `MonotonicClock` 结构体（boot_ns: u64）
- [x] SubTask 3.2: 实现 `MonotonicClock::init(clock: &dyn HalClock) -> Self`
- [x] SubTask 3.3: 实现 `MonotonicClock::now_ns(&self, clock: &dyn HalClock) -> u64`（saturating_sub）
- [x] SubTask 3.4: 编写单元测试（使用 TestClock + AtomicU64，验证单调性、boot 偏移）
- [x] 验证：`cargo test -p eneros-time monotonic` 通过

## Task 4: 实现高精度定时器（`time/src/hrtimer.rs`）

- [x] SubTask 4.1: 定义 `TimerId(pub u64)`、`HrTimer` 结构体
- [x] SubTask 4.2: 定义 `TimerWheel` 结构体（timers: [Option<HrTimer>; 64], count, next_id）
- [x] SubTask 4.3: 实现 `TimerWheel::new()`（const fn）
- [x] SubTask 4.4: 实现 `TimerWheel::add()`（返回 TimerId，槽位已满返回 None）
- [x] SubTask 4.5: 实现 `TimerWheel::cancel()`
- [x] SubTask 4.6: 实现 `TimerWheel::tick(now_ns)`（处理到期定时器，返回下一个 deadline）
- [x] SubTask 4.7: 编写单元测试（add/cancel/tick、周期定时器、槽位满、空 wheel）
- [x] 验证：`cargo test -p eneros-time hrtimer` 通过

## Task 5: 实现时间 API（`time/src/api.rs` + `lib.rs`）

- [x] SubTask 5.1: 定义全局静态状态（CLOCK/MONO/WHEEL/RTC_OFFSET_NS/RTC_BASE，均用 spin::Mutex；ClockRef 包装处理 Send+Sync）
- [x] SubTask 5.2: 实现 `time_init(clock: &'static dyn HalClock, rtc_base: u64)`（初始化所有全局状态）
- [x] SubTask 5.3: 实现 `get_monotonic_ns() -> u64`（未初始化返回 0）
- [x] SubTask 5.4: 实现 `get_time() -> TimeStamp`（RTC offset + monotonic）
- [x] SubTask 5.5: 实现 `sleep_until(deadline_ns: u64)`（aarch64 用 wfe，host 用 busy-wait）
- [x] SubTask 5.6: 实现 `register_timer()` / `register_periodic()` / `cancel_timer()`
- [x] SubTask 5.7: 实现 `rtc_read() -> RtcTime` / `rtc_write(t: RtcTime)`
- [x] SubTask 5.8: 实现定时器到期计数 `timer_expired_count() -> u64`（可观测性）
- [x] SubTask 5.9: 编写集成测试（time_init + get_time + get_monotonic_ns + register_timer + cancel）
- [x] 验证：`cargo test -p eneros-time` 全部通过（30 个测试）

## Task 6: 更新构建系统

- [x] SubTask 6.1: 更新 `Makefile`（VERSION := 0.12.0，添加 time-build/time-test 目标）
- [x] SubTask 6.2: 更新 `.github/workflows/ci.yml`（版本标识 v0.12.0，添加 time crate cross-build 步骤）
- [x] SubTask 6.3: 更新 `ci/src/gate.rs`（注释含 v0.12.0）
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 7: 编写文档

- [x] SubTask 7.1: 创建 `docs/rtc-driver-design.md`（PL031 寄存器、RtcTime、日历转换算法）
- [x] SubTask 7.2: 创建 `docs/system-clock-service.md`（单调时钟、HalClock 复用、时间 API）
- [x] SubTask 7.3: 创建 `docs/hrtimer-implementation.md`（TimerWheel、定时器生命周期、并发安全）

## Task 8: 验证

- [x] SubTask 8.1: `cargo fmt --all -- --check` 通过
- [x] SubTask 8.2: `cargo clippy -p eneros-time --all-targets -- -D warnings` 通过
- [x] SubTask 8.3: `cargo test -p eneros-time` 全部通过（30 个测试）
- [x] SubTask 8.4: `cargo build -p eneros-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] SubTask 8.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部通过（回归；eneros-user-heap 预存在并行测试隔离问题，单线程下 9/9 通过，非 v0.12.0 回归）
- [x] SubTask 8.6: `git status` 无垃圾文件

---

# Task Dependencies

- Task 1（crate 骨架）→ Task 2-5 依赖
- Task 2-5（源码实现）可部分并行：Task 2（rtc）和 Task 4（hrtimer）无依赖
- Task 3（monotonic）依赖 Task 2 的 TimeStamp 类型
- Task 5（api）依赖 Task 2-4
- Task 6（构建系统）独立，可与 Task 2-5 并行
- Task 7（文档）依赖 Task 2-5 完成
- Task 8（验证）依赖全部完成

**并行机会**：Task 2 + Task 4 可并行；Task 6 可与 Task 2-5 并行。
