# Checklist — EnerOS v0.13.0 硬件看门狗

> **蓝图依据**：`蓝图/phase0.md` §v0.13.0
> **合规性**：蓝图 §43.1（no_std）

---

## 1. Crate 骨架

- [x] `watchdog/Cargo.toml` 存在，name = "eneros-watchdog"，version = "0.13.0"
- [x] `watchdog/Cargo.toml` 依赖 `eneros-time`（path）、`spin`
- [x] `watchdog/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `watchdog/src/lib.rs` 声明 `pub mod wdt/layered/api`
- [x] workspace `Cargo.toml` members 含 "watchdog"
- [x] workspace `Cargo.toml` version = "0.13.0"

## 2. SP805 WDT 驱动（wdt.rs）

- [x] SP805 寄存器常量定义（WDT_LOAD=0x00/WDT_VALUE=0x04/WDT_CTRL=0x08/WDT_INTCLR=0x0c/WDT_LOCK=0xC00）
- [x] WDT_UNLOCK=0x1ACCE551、WDT_LOCK_V=0x1 常量
- [x] `HwWatchdog { base: u64 }` 结构体
- [x] `HwWatchdog::new(base: u64)` 是 const fn
- [x] `HwWatchdog::init(&self, timeout_ms: u32)` 正确操作 MMIO（解锁→加载→清中断→使能→锁定）
- [x] `HwWatchdog::kick(&self)` 正确操作 MMIO（解锁→写 INTCLR→锁定）
- [x] `HwWatchdog::stop(&self)` 正确操作 MMIO（解锁→CTRL 清零→锁定）
- [x] `HwWatchdog::is_enabled(&self) -> bool`（base != 0）
- [x] base=0 时所有 MMIO 操作为 no-op，不 panic
- [x] MMIO 用 `core::ptr::read_volatile`/`write_volatile`
- [x] 单元测试覆盖：construction、is_enabled、base=0 不 panic

## 3. 分层喂狗（layered.rs）

- [x] `LayerId(pub u32)` 定义（derive Clone/Copy/Debug/PartialEq/Eq）
- [x] `FeedLayer` 结构体含 id/name/period_ms/last_feed_ns/enabled
- [x] `WatchdogStatus` 枚举：AllFed/LayerTimeout(LayerId)/HardReset
- [x] `Watchdog` 结构体含 hw/layers[8]/hard_timeout_ms/next_id
- [x] `Watchdog::new()` 是 const fn
- [x] `Watchdog::register_layer()` 返回 `Option<LayerId>`（槽位满返回 None）
- [x] `Watchdog::feed_layer()` 更新 last_feed_ns
- [x] `Watchdog::check(now_ns)` 两级超时检测（period_ms 警告 / hard_timeout_ms 硬复位）
- [x] `check()` 返回正确的 LayerId（修复蓝图 LayerTimeout(0) bug）
- [x] `check()` 使用 saturating_sub 防止时间差下溢
- [x] 单元测试覆盖：register/feed/check AllFed/LayerTimeout/HardReset/槽位满/禁用层

## 4. 全局 API（api.rs + lib.rs）

- [x] 全局静态：WATCHDOG（Mutex<Watchdog>）、INITIALIZED（Mutex<bool>）
- [x] `wdt_init(timeout_ms: u32, wdt_base: u64)` 初始化硬件和全局状态
- [x] `wdt_kick()` 未初始化时 no-op
- [x] `wdt_register_layer(name: &'static str, period_ms: u32) -> Option<LayerId>`
- [x] `wdt_feed_layer(id: LayerId)`
- [x] `wdt_check() -> WatchdogStatus` 调用 `eneros_time::get_monotonic_ns()`
- [x] `wdt_stop()` 调试用
- [x] `wdt_layer_count() -> usize` 可观测性
- [x] 集成测试覆盖：未初始化 no-op、init + register + feed + check

## 5. no_std 合规

- [x] `watchdog/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`（除 `#[cfg(test)]` 模块内）
- [x] 使用 `spin::Mutex` 而非 `std::sync::Mutex`
- [x] 使用 `core::*` 而非 `std::*`

## 6. 时间戳复用

- [x] `watchdog/Cargo.toml` 依赖 `eneros-time`（path = "../time"）
- [x] `api.rs` 使用 `eneros_time::get_monotonic_ns()` 获取时间戳
- [x] 不直接使用内联汇编读取 `cntpct_el0`

## 7. 构建系统

- [x] `Makefile` VERSION := 0.13.0
- [x] `Makefile` 含 watchdog-build / watchdog-test 目标
- [x] `ci.yml` 版本标识 v0.13.0
- [x] `ci.yml` 含 "Build watchdog crate" cross-build 步骤
- [x] `ci/src/gate.rs` 注释含 v0.13.0

## 8. 文档

- [x] `docs/watchdog-design.md` 存在（SP805 寄存器、HwWatchdog 驱动、软件模式、MMIO）
- [x] `docs/layered-feeding-protocol.md` 存在（FeedLayer、Watchdog、两级超时、分层协议）

## 9. 验证

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-watchdog --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-watchdog` 全部通过（22 个测试）
- [x] `cargo build -p eneros-watchdog --target aarch64-unknown-none` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（163 个测试，回归）
- [x] `git status` 无垃圾文件
