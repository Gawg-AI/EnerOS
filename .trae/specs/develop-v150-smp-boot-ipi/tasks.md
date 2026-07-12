# Tasks — EnerOS v0.15.0 多核启动与 IPI

> **蓝图依据**：`蓝图/phase0.md` §v0.15.0（第 3181-3349 行）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动
> **依赖**：v0.6.0 HAL 核心（已满足，但不直接依赖 HAL crate，D2）

---

## Task 1: 创建 `smp` crate 骨架 ✅

- [x] SubTask 1.1: 创建 `smp/Cargo.toml`（name=eneros-smp, version=0.15.0, deps: spin, heapless）
- [x] SubTask 1.2: 创建 `smp/src/lib.rs`（`#![cfg_attr(not(test), no_std)]`，模块声明 boot/ipi/channel，公共 API re-export）
- [x] SubTask 1.3: 创建 `smp/src/boot.rs`、`smp/src/ipi.rs`、`smp/src/channel.rs` 最小存根
- [x] SubTask 1.4: 更新 workspace `Cargo.toml`（members 添加 "smp"，version 改 0.15.0）
- [x] 验证：`cargo build -p eneros-smp` 成功

## Task 2: 实现多核启动（`smp/src/boot.rs`） ✅

- [x] SubTask 2.1: 定义 `CoreState` 枚举（Offline=0, Booting=1, Online=2, Halted=3，derive Debug/Clone/Copy/PartialEq/Eq）。用 `repr(u8)` 以支持 AtomicU8 存储
- [x] SubTask 2.2: 定义 `CoreInfo` 结构体（id: u32, entry: u64, stack_base: u64, state: CoreState，derive Debug/Clone/Copy）
- [x] SubTask 2.3: 定义 `CORES: spin::Mutex<[CoreInfo; 8]>` 静态数组（初始全 Offline，id 0-7）。用 `const fn` 初始化
- [x] SubTask 2.4: 定义 `CORE_STATES: [core::sync::atomic::AtomicU8; 8]` 静态数组（无锁状态查询，初始 Offline）
- [x] SubTask 2.5: 定义 `CORE_COUNT: spin::Mutex<u32>` 全局核数（初始 1）
- [x] SubTask 2.6: 实现 `read_core_id() -> u32`（aarch64 读 `mpidr_el1` & 0xff；host 返回 0，cfg gate）
- [x] SubTask 2.7: 实现 `smp_init(core_count: u32)`（设置 CORE_COUNT，初始化 CORES 表 id 字段）
- [x] SubTask 2.8: 实现 `wake_secondary(core_id: u32, entry: u64)`（D1：通过 PSCI CPU_ON 调用。aarch64 用 `hvc #0`，参数：x0=0x8400_000E（CPU_ON），x1=target_mpidr（core_id 转 MPIDR），x2=entry；host no-op。cfg gate）
- [x] SubTask 2.9: 实现 `secondary_entry() -> !`（读取 core_id → 设状态 Booting → 初始化 GIC redistributor stub → 设状态 Online → `loop { wfe }`。aarch64 用 `asm!("wfe")`，host 死循环。cfg gate）
- [x] SubTask 2.10: 实现 `core_state(id: u32) -> Option<CoreState>`（从 AtomicU8 读取，id≥8 返回 None）
- [x] SubTask 2.11: 实现 `set_core_state(id: u32, state: CoreState)`（写 AtomicU8 + 同步 CORES 表）
- [x] SubTask 2.12: 实现 `core_count() -> u32`（返回 CORE_COUNT）
- [x] SubTask 2.13: 编写单元测试（CoreState 转换、CoreInfo 构造、core_state 查询、set_core_state、smp_init、read_core_id host 返回 0）
- [x] 验证：`cargo test -p eneros-smp boot` 通过

## Task 3: 实现 IPI 核间中断（`smp/src/ipi.rs`） ✅

- [x] SubTask 3.1: 定义 `IpiMsg` 枚举（Reschedule / Shutdown / TlbShootdown(u64) / Custom(u32)，derive Debug/Clone/Copy/PartialEq/Eq）
- [x] SubTask 3.2: 定义 `IPI_HANDLERS: spin::Mutex<[Option<fn(IpiMsg)>; 16]>` 静态表（按 msg_type 索引，16 种消息类型槽）
- [x] SubTask 3.3: 实现 `ipi_send(target: u32, msg: IpiMsg)`（1. channel::mailbox_push(target, msg)；2. 发 SGI：aarch64 写 `icc_sgi1r_el1`，target<<16 | SGI_NUM=0；host no-op。cfg gate）
- [x] SubTask 3.4: 实现 `ipi_broadcast(msg: IpiMsg)`（遍历所有 core 调 ipi_send，或 target=0xFFFF 一次性广播）
- [x] SubTask 3.5: 实现 `register_ipi_handler(msg_type: u32, handler: fn(IpiMsg))`（msg_type≥16 忽略；存入 IPI_HANDLERS）
- [x] SubTask 3.6: 实现 `ipi_dispatch()`（IPI 中断入口：从自身 core 邮箱取所有消息，按 msg_type 分发到 handler）
- [x] SubTask 3.7: 定义 SGI 中断号常量 `SGI_IRQ_NUM: u32 = 0`（GICv3 SGI 0）
- [x] SubTask 3.8: 实现 `send_sgi(target: u32, sgi_num: u32)` helper（aarch64 `icc_sgi1r_el1` 写入；host no-op。cfg gate）
- [x] SubTask 3.9: 编写单元测试（IpiMsg 构造与匹配、register_ipi_handler、ipi_dispatch 空邮箱不 panic、IPI_HANDLERS 表操作）
- [x] 验证：`cargo test -p eneros-smp ipi` 通过

## Task 4: 实现核间通信通道（`smp/src/channel.rs`） ✅

- [x] SubTask 4.1: 定义 `MAILBOX_CAPACITY: usize = 16` 常量
- [x] SubTask 4.2: 定义 `Mailbox` 结构体（`queue: heapless::spsc::Queue<IpiMsg, 16>`，用 spin::Mutex 包裹）— **实现调整**：改用 `heapless::Vec<IpiMsg, 16>` 替代 `spsc::Queue`（因 `spin::Mutex` 不实现 Copy，无法用 `[expr; N]` 初始化数组）
- [x] SubTask 4.3: 定义 `MAILBOXES` 静态数组（per-core 邮箱，8 槽手动初始化）
- [x] SubTask 4.4: 实现 `mailbox_push(core_id: u32, msg: IpiMsg) -> Result<(), IpiMsg>`（core_id≥8 返回 Err(msg)；队列满返回 Err(msg)；否则入队 Ok）
- [x] SubTask 4.5: 实现 `mailbox_pop(core_id: u32) -> Option<IpiMsg>`（从指定 core 邮箱取一条消息）
- [x] SubTask 4.6: 实现 `mailbox_drain(core_id: u32) -> heapless::Vec<IpiMsg, 16>`（取所有待处理消息，用于 ipi_dispatch）
- [x] SubTask 4.7: 实现 `mailbox_clear(core_id: u32)`（清空邮箱，调试用）
- [x] SubTask 4.8: 编写单元测试（push/pop 基本操作、队列满返回 Err、drain 取所有、clear 清空、跨 core 操作）
- [x] 验证：`cargo test -p eneros-smp channel` 通过

## Task 5: 更新构建系统 ✅

- [x] SubTask 5.1: 更新 `Makefile`（VERSION := 0.15.0，添加 smp-build / smp-test 目标）
- [x] SubTask 5.2: 更新 `.github/workflows/ci.yml`（版本标识 v0.15.0，添加 smp crate cross-build 步骤）
- [x] SubTask 5.3: 更新 `ci/src/gate.rs`（注释含 v0.15.0）
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 6: 编写文档 ✅

- [x] SubTask 6.1: 创建 `docs/smp-boot-design.md`（506 行，12 节：概述/设计决策/数据结构/启动流程/PSCI/CoreState 状态机/蓝图对比/cfg gate/未来扩展/API/测试/蓝图符合性）
- [x] SubTask 6.2: 创建 `docs/ipi-mechanism.md`（587 行，14 节：概述/GICv3 SGI/icc_sgi1r_el1 格式/IpiMsg 类型/handler 注册分发/邮箱设计/ipi_send/ipi_broadcast/cfg gate/性能/未来扩展/API/测试/蓝图符合性）

## Task 7: 验证 ✅

- [x] SubTask 7.1: `cargo fmt --all -- --check` 通过（exit 0）
- [x] SubTask 7.2: `cargo clippy -p eneros-smp --all-targets -- -D warnings` 通过
- [x] SubTask 7.3: `cargo test -p eneros-smp` 全部通过（19/19 测试：boot 7 + channel 7 + ipi 5）
- [x] SubTask 7.4: `cargo build -p eneros-smp --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（修复 boot.rs:190 wfe 缺 unsafe 块）
- [x] SubTask 7.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（203 测试：board 7 + ci 5 + hal 11 + heap 27 + mm 44 + panic 21 + runtime 11 + sel4-sys 6 + smp 19 + time 30 + watchdog 22；v0.14.0 panic 不退化 ✅）
- [x] SubTask 7.6: `git status` 无垃圾文件（smp/ 目录正常追踪，无 target/ 或 *.elf 等产物）

---

# Task Dependencies

- Task 1（crate 骨架）→ Task 2-4 依赖
- Task 2（boot.rs）和 Task 4（channel.rs）无互相依赖，可并行
- Task 3（ipi.rs）依赖 Task 4（channel.rs）—— ipi_send 调用 mailbox_push；也依赖 Task 2 的 read_core_id
- Task 5（构建系统）独立，可与 Task 2-4 并行
- Task 6（文档）依赖 Task 2-4 完成
- Task 7（验证）依赖全部完成

**并行机会**：Task 2 + Task 4 可并行；Task 5 可与 Task 2-4 并行。

---

# 蓝图符合性自检

| 蓝图条目 | 任务覆盖 |
|---------|---------|
| §3 交付物 boot.rs(~200行)/ipi.rs(~180行)/channel.rs(~150行) | Task 2 / Task 3 / Task 4 |
| §3 接口 wake_secondary/ipi_send/ipi_broadcast/smp_init/register_ipi_handler | SubTask 2.8 / 3.3 / 3.4 / 2.7 / 3.5 |
| §4.1 CoreInfo/CoreState/IpiMsg | SubTask 2.1-2.2 / 3.1 |
| §4.4 错误处理（唤醒超时→Offline；IPI 丢失→SGI 可靠） | SubTask 2.9-2.11 + Task 3 |
| §5.4 难点（唤醒地址 SoC 特定） | D1：PSCI 替代 |
| §6.1 单元 CoreState 状态机 ≥80% | SubTask 2.13 |
| §6.2 集成所有核打印 ID | host 间接验证（read_core_id）；真机留待 QEMU 阶段 |
| §6.3 性能 IPI <5μs | 不在 host 测，文档标注 |
| §6.4 回归 v0.14.0 不退化 | SubTask 7.5 |
| §6.5 故障注入 Secondary 唤醒失败 | SubTask 2.9（唤醒超时→Offline）+ 文档 |
| §7 验收标准 | checklist.md 覆盖 |
| §8.5 Secondary 必须先初始化 GIC redistributor | SubTask 2.9 secondary_entry 调用 redistributor 初始化 stub |
