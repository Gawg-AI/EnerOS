# Checklist — EnerOS v0.14.0 Panic 处理框架

> **蓝图依据**：`蓝图/phase0.md` §v0.14.0（第 2988-3167 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本，签名可编译）
> **验收标准**：蓝图 §7（第 3156-3161 行）

---

## 1. Crate 骨架

- [x] `panic/Cargo.toml` 存在，name = "eneros-panic"，version = "0.14.0"
- [x] `panic/Cargo.toml` 依赖 `eneros-time`（path）、`spin`、`heapless`
- [x] `panic/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `panic/src/lib.rs` 声明 `pub mod isolation` / `pub mod logger`
- [x] `panic/src/lib.rs` **不定义** `#[panic_handler]`（D1 设计决策，避免与 kernel/hello 冲突）
- [x] workspace `Cargo.toml` members 含 "panic"
- [x] workspace `Cargo.toml` version = "0.14.0"

## 2. 核心数据结构（lib.rs）

- [x] `PanicLevel` 枚举定义（Kernel / Partition(u32)），derive Debug/Clone/Copy/PartialEq/Eq
- [x] `PanicContext` 结构体定义（level/location/message/timestamp_ns/core_id 五字段）
- [x] `PanicContext::new(level, location, message)` 实现，timestamp_ns 取自 `eneros_time::get_monotonic_ns()`
- [x] `core_id` 通过 `read_core_id()` helper 获取（aarch64 读 mpidr_el1，host 返回 0，cfg gate）
- [x] `PanicStrategy` trait 定义（`fn handle(&self, ctx: &PanicContext) -> !`）
- [x] `KernelResetStrategy` 实现 PanicStrategy（日志→flush→hard_reset）
- [x] `PartitionIsolateStrategy { partition: u32 }` 实现 PanicStrategy（日志→mark_dead→死循环）
- [x] `ResetPolicy` 枚举定义（Immediate / Delayed(u64)）
- [x] `handle_panic(info: &PanicInfo) -> !` 函数实现（构造 ctx + 调用注册策略）
- [x] 全局 `STRATEGY: Mutex<Option<&'static dyn PanicStrategy>>` + `set_strategy()` API
- [x] `hard_reset() -> !` 实现（aarch64 `asm!("b 0x0")`，host 死循环，cfg gate）
- [x] 单元测试覆盖 PanicContext 构造、PanicLevel 匹配、ResetPolicy（≥ 5 个测试）

## 3. Panic 日志器（logger.rs）

- [x] `SerialSink` trait 定义（putc / puts）
- [x] 全局 `SERIAL_SINK: Mutex<Option<&'static dyn SerialSink>>` + `set_serial_sink()` 注册
- [x] `NullSink` 默认实现（丢弃输出，未注册时 no-op）
- [x] `panic_log(ctx: &PanicContext)` 实现，使用固定栈缓冲区 `[u8; 256]`（不依赖 alloc）
- [x] 日志格式含：`[PANIC] level=KERNEL/Partition(N) loc=file:line msg=... core=N t=Ns`
- [x] `panic_log(msg: &str)` 实现蓝图接口（原始字符串输出）
- [x] `flush()` 实现（no-op stub，预留接口）
- [x] 单元测试覆盖：NullSink 不 panic、格式化含全部字段、未注册 sink no-op、panic_log 原样输出（≥ 5 个测试）

## 4. 分区隔离（isolation.rs）

- [x] `PartitionState` 枚举定义（Alive / Dead），derive Debug/Clone/Copy/PartialEq/Eq
- [x] `PARTITION_TABLE: [Mutex<PartitionState>; 8]` 静态数组（初始 Alive）
- [x] `mark_partition_dead(id: u32) -> Result<(), IsolationError>` 实现（id≥8 返回 Err）
- [x] `partition_state(id: u32) -> Option<PartitionState>` 实现
- [x] `reset_partition(id: u32)` 实现（调试用，恢复 Alive）
- [x] `register_partition_panic_handler(partition: u32, handler: fn(&PanicContext) -> !)` 实现蓝图接口
- [x] `IsolationError` 枚举定义（InvalidId / AlreadyDead）
- [x] 隔离失败时升级为内核 panic 的逻辑（蓝图 §4.4）
- [x] 单元测试覆盖：mark_dead 成功/超界、partition_state 查询、reset、状态转换、register（≥ 6 个测试）

## 5. no_std 合规

- [x] `panic/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`（除 `#[cfg(test)]` 模块内）
- [x] 使用 `spin::Mutex` 而非 `std::sync::Mutex`
- [x] 使用 `core::*` 而非 `std::*`
- [x] panic_log 使用固定缓冲区，不依赖 `alloc::string::String`（D2，蓝图 §5.4）

## 6. aarch64 cfg gate

- [x] `read_core_id()` 用 `#[cfg(target_arch = "aarch64")]` gate，host 有 stub
- [x] `hard_reset()` 用 `#[cfg(target_arch = "aarch64")]` gate，host 有 stub
- [x] aarch64 内联汇编正确（`asm!("b 0x0")` / `mpidr_el1` 读取）
- [x] host 测试不触发 aarch64 专属代码

## 7. 构建系统

- [x] `Makefile` VERSION := 0.14.0
- [x] `Makefile` 含 panic-build / panic-test 目标
- [x] `ci.yml` 版本标识 v0.14.0
- [x] `ci.yml` 含 "Build panic crate" cross-build 步骤
- [x] `ci/src/gate.rs` 注释含 v0.14.0

## 8. 文档

- [x] `docs/panic-framework-design.md` 存在（PanicContext/PanicStrategy/分级策略/日志器无 alloc/core_id 与复位 cfg gate/与现有 panic_handler 关系）
- [x] `docs/partition-isolation-recovery.md` 存在（PartitionState 表/隔离流程/升级内核 panic/v0.18.0 集成计划/当前限制）

## 9. 验证

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-panic --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-panic` 全部通过（预期 ≥ 20 个测试）
- [x] `cargo build -p eneros-panic --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（回归，v0.13.0 不退化）
- [x] `git status` 无垃圾文件

## 10. 蓝图验收标准（§7）

- [x] §7.1 panic 后日志输出到串口（通过 SerialSink 注入，host 用 NullSink/capture sink 验证）
- [x] §7.2 分区 panic 隔离该分区，不影响其他分区（mark_partition_dead 仅影响指定槽位，其他槽位状态不变）
- [x] §7.3 内核 panic 触发硬件复位（aarch64 路径 `asm!("b 0x0")`，host 路径死循环）
- [x] §7.4 文档齐全（两份文档）
- [x] §7.5 出口判定：Panic 框架就绪（P0-D 终点达成）

## 11. 外科手术原则自检（Karpathy §3）

- [x] **未修改** `kernel/src/lib.rs` 的 `#[panic_handler]`（D1，现有保持不变）
- [x] **未修改** `hello/src/main.rs` 的 `#[panic_handler]`（D1）
- [x] **未修改** v0.12.0 time crate、v0.13.0 watchdog crate 的任何源码（仅作为依赖引用）
- [x] 新增文件仅限 panic/ crate 三个源文件 + 两份文档
- [x] 修改文件仅限 Cargo.toml / Makefile / ci.yml / gate.rs 四个构建配置
- [x] 无"顺手改进"其他代码（Karpathy 原则：每行改动可追溯到 v0.14.0 需求）
