# Checklist — v0.19.0 分区调度器

## 代码实现

- [x] C1: `crates/kernel/sched/src/jitter.rs` 已创建，含 `JitterStats` 结构体、`record_jitter`/`jitter_measure`/`jitter_reset` API，使用 `Spinlock` 保护（非 `static mut`）
- [x] C2: `crates/kernel/sched/src/wcet.rs` 已创建，含 `WCET_TABLE` 静态表、`wcet_set`/`wcet_estimate`/`check_partition_overrun` API
- [x] C3: `crates/kernel/sched/src/timeline.rs` 已创建，含 `PartitionSlot`/`MajorFrame` 结构体、`add_slot`/`advance_slot`/`current_partition`/`current_duration_ns` API，`MAX_SLOTS = 16`
- [x] C4: `crates/kernel/sched/src/partition_sched.rs` 已创建，含 `PartitionId` 新类型、时间源注入（`set_time_source`/`set_timer_registrar`/`now_ns`）、`schedule_add`/`schedule_run`/`schedule_stop`/`current_partition`/`on_tick`/`switch_partition` API
- [x] C5: 时间源通过函数指针注入（`Spinlock<Option<fn() -> u64>>`），保持 sched crate 零外部依赖（D1/D2）
- [x] C6: 全局状态使用 `Spinlock` 保护（`FRAME`/`JITTER`/`WCET_TABLE`/`CURRENT_PARTITION`/`SWITCH_COUNT`），非 `static mut`（D2）
- [x] C7: `lib.rs` 已添加 `pub mod jitter/partition_sched/timeline/wcet` 声明与 `pub use` 导出
- [x] C8: `SchedError` 枚举已添加 `NoTimerRegistrar` 与 `SlotFull` 变体

## API 完整性

- [x] C9: `schedule_add(partition, duration_ms) -> Result<(), SchedError>` 已实现，超 16 slot 返回 `Err(SlotFull)`
- [x] C10: `schedule_run() -> Result<(), SchedError>` 已实现，无定时器注册器返回 `Err(NoTimerRegistrar)`
- [x] C11: `schedule_stop()` 已实现
- [x] C12: `current_partition() -> Option<PartitionId>` 已实现
- [x] C13: `on_tick()` 已实现，计算抖动 + 推进 slot + 切换分区
- [x] C14: `record_jitter(j_us)` / `jitter_measure() -> JitterStats` / `jitter_reset()` 已实现
- [x] C15: `wcet_set(tid, ns)` / `wcet_estimate(tid) -> u64` / `check_partition_overrun(partition, ns) -> Option<Tid>` 已实现
- [x] C16: `set_time_source(f)` / `set_timer_registrar(f)` / `now_ns()` 已实现

## 数据结构正确性

- [x] C17: `PartitionId(pub u32)` 新类型，derive `Clone, Copy, Debug, PartialEq, Eq`
- [x] C18: `PartitionSlot { partition: PartitionId, duration_ms: u32 }`，derive `Clone, Copy, Debug`
- [x] C19: `MajorFrame` 含 `slots: [PartitionSlot; 16]`/`slot_count`/`period_ms`/`current_slot`/`frame_start_ns`，`new()` const 初始化
- [x] C20: `JitterStats` 含 `min_jitter_us`/`max_jitter_us`/`sum_jitter_us`/`samples`，初始 `min=i64::MAX, max=i64::MIN`
- [x] C21: `Tcb.partition` 字段（v0.18.0 已预留）被 `check_partition_overrun` 用于关联线程与分区

## 分区切换逻辑正确性

- [x] C22: `on_tick` 计算抖动 = `(now_ns - expected_ns) / 1000`（μs）
- [x] C23: `on_tick` 推进 `current_slot`，回绕到 0 时更新 `frame_start_ns`
- [x] C24: `switch_partition` 设置 `CURRENT_PARTITION` 并递增 `SWITCH_COUNT`
- [x] C25: `schedule_run` 初始化 `frame_start_ns = now_ns()`，`current_slot = 0`

## 测试覆盖

- [x] C26: `cargo test -p eneros-sched` 通过，新增 jitter/wcet/timeline/partition_sched 测试 ≥ 80% 覆盖（蓝图 §6.1）
- [x] C27: v0.18.0 原 72 个测试不退化（回归通过）
- [x] C28: 抖动测试覆盖：记录单次/多次、min/max 更新、reset 恢复、空统计查询
- [x] C29: WCET 测试覆盖：默认 0、设置后查询、超时检测、越界处理
- [x] C30: timeline 测试覆盖：添加 slot、超出上限、推进回绕、空帧查询、周期计算
- [x] C31: partition_sched 测试覆盖：schedule_add 正常/超限、schedule_run 无/有 mock 定时器、on_tick 推进与回绕、current_partition 查询、switch_partition 切换计数

## 构建与质量

- [x] C32: `crates/kernel/sched/Cargo.toml` 版本 `0.19.0`，零外部依赖（`[dependencies]` 为空）
- [x] C33: 根 `Cargo.toml` workspace 版本 `0.19.0`
- [x] C34: `Makefile` VERSION `0.19.0`
- [x] C35: `.github/workflows/ci.yml` 版本标识 v0.19.0
- [x] C36: `ci/src/gate.rs` 注释含 v0.19.0
- [x] C37: `cargo fmt --all -- --check` 通过
- [x] C38: `cargo clippy -p eneros-sched --all-targets -- -D warnings` 无 warning
- [x] C39: `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc` 通过
- [x] C40: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全通过（v0.18.0 线程抽象不退化）

## 文档与规范

- [x] C41: `docs/smp/partition-scheduler-design.md` 已创建（~400 行，分区调度器设计）
- [x] C42: `docs/smp/arinc653-adaptation.md` 已创建（~350 行，ARINC 653 适配说明）
- [x] C43: `docs/smp/wcet-analysis.md` 已创建（~300 行，WCET 分析）
- [x] C44: 文档放 `docs/smp/` 子目录，未平面化放 `docs/` 根（规则 §2.3.3）
- [x] C45: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C46: 新增 crate 源码在 `crates/kernel/sched/` 下（规则 §2.3.1）

## 验收标准（蓝图 §7）

- [x] C47: RTOS 分区按时间片执行（§7.1）— host 测试验证 on_tick 推进逻辑
- [ ] C48: 分区抖动 < 1ms（§7.2）— 延后至 QEMU 实机，host 仅测抖动聚合正确性
- [x] C49: 分区超时可检测并处理（§7.3）— `check_partition_overrun` 测试通过
- [x] C50: 文档齐全（§7.4）
- [x] C51: 出口判定：分区调度就绪，P0-F 完成（§7.5）
