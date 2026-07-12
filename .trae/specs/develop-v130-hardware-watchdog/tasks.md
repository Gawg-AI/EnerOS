# Tasks — EnerOS v0.13.0 硬件看门狗

> **蓝图依据**：`蓝图/phase0.md` §v0.13.0
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动

## Task 1: 创建 `watchdog` crate 骨架

- [x] SubTask 1.1: 创建 `watchdog/Cargo.toml`（name=eneros-watchdog, version=0.13.0, deps: eneros-time, spin）
- [x] SubTask 1.2: 创建 `watchdog/src/lib.rs`（`#![cfg_attr(not(test), no_std)]`，模块声明，公共 API re-export）
- [x] SubTask 1.3: 创建 `watchdog/src/wdt.rs`、`layered.rs`、`api.rs` 最小存根
- [x] SubTask 1.4: 更新 workspace `Cargo.toml`（members 添加 "watchdog"，version 改 0.13.0）
- [x] 验证：`cargo build -p eneros-watchdog` 成功

## Task 2: 实现 SP805 WDT 驱动（`watchdog/src/wdt.rs`）

- [x] SubTask 2.1: 定义 SP805 寄存器常量（WDT_LOAD=0x00/WDT_VALUE=0x04/WDT_CTRL=0x08/WDT_INTCLR=0x0c/WDT_LOCK=0xC00）
- [x] SubTask 2.2: 定义 `HwWatchdog { base: u64 }` 结构体
- [x] SubTask 2.3: 实现 `HwWatchdog::new(base: u64)`（const fn）
- [x] SubTask 2.4: 实现 `HwWatchdog::init(&self, timeout_ms: u32)`（解锁→加载→清中断→使能→锁定，base=0 时 no-op）
- [x] SubTask 2.5: 实现 `HwWatchdog::kick(&self)`（解锁→写 INTCLR→锁定，base=0 时 no-op）
- [x] SubTask 2.6: 实现 `HwWatchdog::stop(&self)`（解锁→CTRL 清零→锁定，base=0 时 no-op）
- [x] SubTask 2.7: 实现 `HwWatchdog::is_enabled(&self) -> bool`（base != 0）
- [x] SubTask 2.8: 编写单元测试（construction、is_enabled、base=0 不 panic）
- [x] 验证：`cargo test -p eneros-watchdog wdt` 通过

## Task 3: 实现分层喂狗（`watchdog/src/layered.rs`）

- [x] SubTask 3.1: 定义 `LayerId(pub u32)`（derive Clone/Copy/Debug/PartialEq/Eq）
- [x] SubTask 3.2: 定义 `FeedLayer` 结构体（id/name/period_ms/last_feed_ns/enabled）
- [x] SubTask 3.3: 定义 `WatchdogStatus` 枚举（AllFed/LayerTimeout(LayerId)/HardReset）
- [x] SubTask 3.4: 定义 `Watchdog` 结构体（hw/layers[8]/hard_timeout_ms/next_id）
- [x] SubTask 3.5: 实现 `Watchdog::new()`（const fn）
- [x] SubTask 3.6: 实现 `Watchdog::register_layer()`（返回 Option<LayerId>，槽位满返回 None）
- [x] SubTask 3.7: 实现 `Watchdog::feed_layer()`（更新 last_feed_ns）
- [x] SubTask 3.8: 实现 `Watchdog::check(now_ns)`（两级超时检测，修复蓝图 LayerTimeout(0) bug）
- [x] SubTask 3.9: 编写单元测试（register/feed/check AllFed/LayerTimeout/HardReset/槽位满/禁用层）
- [x] 验证：`cargo test -p eneros-watchdog layered` 通过

## Task 4: 实现全局 API（`watchdog/src/api.rs` + `lib.rs`）

- [x] SubTask 4.1: 定义全局静态状态（WATCHDOG: Mutex<Watchdog>，INITIALIZED: Mutex<bool>）
- [x] SubTask 4.2: 实现 `wdt_init(timeout_ms: u32, wdt_base: u64)`（初始化硬件 + 全局状态）
- [x] SubTask 4.3: 实现 `wdt_kick()`（未初始化时 no-op）
- [x] SubTask 4.4: 实现 `wdt_register_layer(name: &'static str, period_ms: u32) -> Option<LayerId>`
- [x] SubTask 4.5: 实现 `wdt_feed_layer(id: LayerId)`
- [x] SubTask 4.6: 实现 `wdt_check() -> WatchdogStatus`（调用 `eneros_time::get_monotonic_ns()`）
- [x] SubTask 4.7: 实现 `wdt_stop()`（调试用）
- [x] SubTask 4.8: 实现 `wdt_layer_count() -> usize`（可观测性）
- [x] SubTask 4.9: 编写集成测试（init + register + feed + check + 未初始化 no-op）
- [x] 验证：`cargo test -p eneros-watchdog` 全部通过（22 个测试）

## Task 5: 更新构建系统

- [x] SubTask 5.1: 更新 `Makefile`（VERSION := 0.13.0，添加 watchdog-build/watchdog-test 目标）
- [x] SubTask 5.2: 更新 `.github/workflows/ci.yml`（版本标识 v0.13.0，添加 watchdog crate cross-build 步骤）
- [x] SubTask 5.3: 更新 `ci/src/gate.rs`（注释含 v0.13.0）
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 6: 编写文档

- [x] SubTask 6.1: 创建 `docs/watchdog-design.md`（SP805 寄存器、HwWatchdog 驱动、软件模式、MMIO 操作）
- [x] SubTask 6.2: 创建 `docs/layered-feeding-protocol.md`（FeedLayer、Watchdog、两级超时检测、分层协议）

## Task 7: 验证

- [x] SubTask 7.1: `cargo fmt --all -- --check` 通过
- [x] SubTask 7.2: `cargo clippy -p eneros-watchdog --all-targets -- -D warnings` 通过
- [x] SubTask 7.3: `cargo test -p eneros-watchdog` 全部通过（22 个测试）
- [x] SubTask 7.4: `cargo build -p eneros-watchdog --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] SubTask 7.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（163 个测试，回归）
- [x] SubTask 7.6: `git status` 无垃圾文件

---

# Task Dependencies

- Task 1（crate 骨架）→ Task 2-4 依赖
- Task 2（wdt.rs）和 Task 3（layered.rs）无互相依赖，可并行
- Task 4（api.rs）依赖 Task 2-3
- Task 5（构建系统）独立，可与 Task 2-4 并行
- Task 6（文档）依赖 Task 2-4 完成
- Task 7（验证）依赖全部完成

**并行机会**：Task 2 + Task 3 可并行；Task 5 可与 Task 2-4 并行。
