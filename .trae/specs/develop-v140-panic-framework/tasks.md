# Tasks — EnerOS v0.14.0 Panic 处理框架

> **蓝图依据**：`蓝图/phase0.md` §v0.14.0（第 2988-3167 行）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动
> **依赖**：v0.11.0（已满足）、v0.12.0 `eneros-time`（已满足）

---

## Task 1: 创建 `panic` crate 骨架

- [x] SubTask 1.1: 创建 `panic/Cargo.toml`（name=eneros-panic, version=0.14.0, deps: eneros-time, spin, heapless）
- [x] SubTask 1.2: 创建 `panic/src/lib.rs`（`#![cfg_attr(not(test), no_std)]`，模块声明 isolation/logger，公共 API re-export）
- [x] SubTask 1.3: 创建 `panic/src/isolation.rs`、`panic/src/logger.rs` 最小存根
- [x] SubTask 1.4: 更新 workspace `Cargo.toml`（members 添加 "panic"，version 改 0.14.0）
- [x] 验证：`cargo build -p eneros-panic` 成功

## Task 2: 实现核心数据结构与策略（`panic/src/lib.rs`）

- [x] SubTask 2.1: 定义 `PanicLevel` 枚举（Kernel / Partition(u32)，derive Debug/Clone/Copy/PartialEq/Eq）
- [x] SubTask 2.2: 定义 `PanicContext` 结构体（level: PanicLevel, location: &'static str, message: &'static str, timestamp_ns: u64, core_id: u32）
- [x] SubTask 2.3: 实现 `PanicContext::new(level, location, message) -> Self`（timestamp_ns 调用 `eneros_time::get_monotonic_ns()`，core_id 调用 `read_core_id()` helper）
- [x] SubTask 2.4: 定义 `PanicStrategy` trait（`fn handle(&self, ctx: &PanicContext) -> !`）
- [x] SubTask 2.5: 定义 `KernelResetStrategy` 结构体，实现 `PanicStrategy`（日志输出→flush→硬件复位 aarch64 / 死循环 host）
- [x] SubTask 2.6: 定义 `PartitionIsolateStrategy { partition: u32 }`，实现 `PanicStrategy`（日志→标记 Dead→死循环）
- [x] SubTask 2.7: 定义 `ResetPolicy` 枚举（Immediate / Delayed(u64)），用于 KernelResetStrategy 的复位时机
- [x] SubTask 2.8: 实现 `handle_panic(info: &PanicInfo) -> !`（构造 ctx，调用注册的策略；crate 不定义 `#[panic_handler]`，由消费者委托）
- [x] SubTask 2.9: 定义全局静态 `STRATEGY: Mutex<Option<&'static dyn PanicStrategy>>` 和 `set_strategy()` / `set_partition_strategy(id)` API
- [x] SubTask 2.10: 实现 `read_core_id() -> u32`（aarch64 读 `mpidr_el1`；host 返回 0），用 `#[cfg(target_arch = "aarch64")]` gate
- [x] SubTask 2.11: 实现 `hard_reset() -> !`（aarch64 `asm!("b 0x0")`；host `loop { spin_loop() }`），cfg gate
- [x] SubTask 2.12: 编写单元测试（PanicContext 构造、PanicLevel 匹配、ResetPolicy 构造；策略 handle 因返回 `!` 不在 host 直接测，通过 logger 间接验证）
- [x] 验证：`cargo test -p eneros-panic` 通过（21 个测试）

## Task 3: 实现 panic 日志器（`panic/src/logger.rs`）

- [x] SubTask 3.1: 定义 `SerialSink` trait（`fn putc(&self, c: u8)` / `fn puts(&self, s: &str)`），供消费者注入串口实现
- [x] SubTask 3.2: 定义全局静态 `SERIAL_SINK: Mutex<Option<&'static dyn SerialSink>>` 和 `set_serial_sink()` 注册函数
- [x] SubTask 3.3: 定义 `NullSink` 默认实现（丢弃所有输出，用于未注册时 no-op）
- [x] SubTask 3.4: 实现 `panic_log(ctx: &PanicContext)`：用固定栈缓冲区 `[u8; 256]` + `core::fmt::Write` 格式化 `[PANIC] level=... loc=... msg=... core=... t=...ns\n`
- [x] SubTask 3.5: 实现 `panic_log_raw(msg: &str)`：直接输出原始字符串（蓝图接口 `pub fn panic_log(msg: &str)`）
- [x] SubTask 3.6: 实现 `flush()`：同步等待串口输出完成（no-op stub，预留接口；蓝图 §4.3"等待日志 flush"）
- [x] SubTask 3.7: 单元测试覆盖：NullSink 不 panic、panic_log 格式化含 level/loc/msg/core/t、未注册 sink 时 no-op、panic_log_raw 输出原样
- [x] 验证：`cargo test -p eneros-panic logger` 通过（6 个测试）

## Task 4: 实现分区隔离（`panic/src/isolation.rs`）

- [x] SubTask 4.1: 定义 `PartitionState` 枚举（Alive / Dead，derive Debug/Clone/Copy/PartialEq/Eq）
- [x] SubTask 4.2: 定义 `PARTITION_TABLE: [Mutex<PartitionState>; 8]` 静态数组（8 槽，初始 Alive）
- [x] SubTask 4.3: 实现 `mark_partition_dead(id: u32) -> Result<(), IsolationError>`（id≥8 返回 Err；成功置 Dead）
- [x] SubTask 4.4: 实现 `partition_state(id: u32) -> Option<PartitionState>`（id≥8 返回 None）
- [x] SubTask 4.5: 实现 `reset_partition(id: u32)`（恢复为 Alive，测试/调试用）
- [x] SubTask 4.6: 实现 `register_partition_panic_handler(partition: u32, handler: fn(&PanicContext) -> !)`（蓝图接口；存入静态 HANDLERS 表，槽满忽略）
- [x] SubTask 4.7: 定义 `IsolationError` 枚举（InvalidId / AlreadyDead）
- [x] SubTask 4.8: 单元测试覆盖：mark_dead 成功/超界、partition_state 查询、reset_partition、状态转换、register handler
- [x] 验证：`cargo test -p eneros-panic isolation` 通过（8 个测试）

## Task 5: 更新构建系统

- [x] SubTask 5.1: 更新 `Makefile`（VERSION := 0.14.0，添加 panic-build / panic-test 目标）
- [x] SubTask 5.2: 更新 `.github/workflows/ci.yml`（版本标识 v0.14.0，添加 panic crate cross-build 步骤）
- [x] SubTask 5.3: 更新 `ci/src/gate.rs`（注释含 v0.14.0）
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 6: 编写文档

- [x] SubTask 6.1: 创建 `docs/panic-framework-design.md`（PanicContext/PanicStrategy/分级策略/日志器无 alloc 设计/core_id 与复位 cfg gate/与现有 panic_handler 关系）
- [x] SubTask 6.2: 创建 `docs/partition-isolation-recovery.md`（PartitionState 表/隔离流程/升级内核 panic/v0.18.0 调度器集成计划/当前限制）

## Task 7: 验证

- [x] SubTask 7.1: `cargo fmt --all -- --check` 通过
- [x] SubTask 7.2: `cargo clippy -p eneros-panic --all-targets -- -D warnings` 通过
- [x] SubTask 7.3: `cargo test -p eneros-panic` 全部通过（21 个测试）
- [x] SubTask 7.4: `cargo build -p eneros-panic --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] SubTask 7.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（184 个测试，回归，v0.13.0 看门狗不退化）
- [x] SubTask 7.6: `git status` 无垃圾文件（无 target/、*.elf、*.bin 入仓）

---

# Task Dependencies

- Task 1（crate 骨架）→ Task 2-4 依赖
- Task 2（lib.rs 核心）和 Task 3（logger.rs）有弱依赖：Task 2 的 KernelResetStrategy 调用 Task 3 的 panic_log。建议 Task 3 先行或并行定义 trait
- Task 3（logger.rs）和 Task 4（isolation.rs）无互相依赖，可并行
- Task 4 的 PartitionIsolateStrategy（在 lib.rs，Task 2）调用 Task 4 的 mark_partition_dead
- Task 5（构建系统）独立，可与 Task 2-4 并行
- Task 6（文档）依赖 Task 2-4 完成
- Task 7（验证）依赖全部完成

**并行机会**：Task 3 + Task 4 可并行；Task 5 可与 Task 2-4 并行。

---

# 蓝图符合性自检

| 蓝图条目 | 任务覆盖 |
|---------|---------|
| §3 交付物 lib.rs(~200行)/isolation.rs(~150行)/logger.rs(~120行) | Task 2 / Task 4 / Task 3 |
| §3 接口 `#[panic_handler]` / `register_partition_panic_handler` / `panic_log` | D1（不直接定义，提供 handle_panic）+ SubTask 4.6 + SubTask 3.5 |
| §4.1 PanicLevel / PanicContext | SubTask 2.1-2.3 |
| §4.2 PanicStrategy / KernelResetStrategy / PartitionIsolateStrategy | SubTask 2.4-2.6 |
| §4.4 错误处理（隔离失败→升级内核 panic） | SubTask 4.3-4.7 |
| §5.4 难点（panic 时不用 alloc） | SubTask 3.4 固定缓冲区 |
| §6.1 单元测试 PanicContext 构造 ≥80% | SubTask 2.12 |
| §6.2 集成测试（触发 panic 验证日志与复位/隔离） | SubTask 3.7 + 4.8（host 侧间接验证，因 `!` 不可在 host 测） |
| §6.3 性能 panic 到复位 <50ms | 不在 host 测，文档标注（aarch64 真机验证留待 QEMU/硬件阶段） |
| §6.4 回归 v0.13.0 不退化 | SubTask 7.5 |
| §6.5 故障注入（堆损坏时 panic 仍可输出） | SubTask 3.4 注释 + 文档说明（因不依赖 alloc 天然满足） |
| §7 验收标准 | checklist.md 覆盖 |
