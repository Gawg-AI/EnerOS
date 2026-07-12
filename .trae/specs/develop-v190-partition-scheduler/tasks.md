# Tasks — v0.19.0 分区调度器

- [x] Task 1: 升级 sched crate 版本号与构建配置
  - [x] SubTask 1.1: 修改 `crates/kernel/sched/Cargo.toml`，版本 `0.18.0` → `0.19.0`
  - [x] SubTask 1.2: 修改根 `Cargo.toml`，workspace `version` `0.18.0` → `0.19.0`
  - [x] SubTask 1.3: 修改 `Makefile`，`VERSION` `0.18.0` → `0.19.0`
  - [x] SubTask 1.4: 修改 `.github/workflows/ci.yml`，版本标识更新为 v0.19.0
  - [x] SubTask 1.5: 修改 `ci/src/gate.rs`，注释追加 v0.19.0 说明

- [x] Task 2: 实现 `crates/kernel/sched/src/jitter.rs`（抖动测量）
  - [x] SubTask 2.1: 定义 `JitterStats { min_jitter_us: i64, max_jitter_us: i64, sum_jitter_us: i64, samples: u64 }` 结构体，derive `Clone, Copy, Debug, PartialEq, Eq`
  - [x] SubTask 2.2: 定义 `static JITTER: Spinlock<JitterStats>`，const 初始化 `{min=i64::MAX, max=i64::MIN, sum=0, samples=0}`
  - [x] SubTask 2.3: 实现 `record_jitter(j_us: i64)`：更新 min/max/sum/samples
  - [x] SubTask 2.4: 实现 `jitter_measure() -> JitterStats`：返回当前统计快照（sum 为总和，调用方自行计算 avg）
  - [x] SubTask 2.5: 实现 `jitter_reset()`：恢复初始值
  - [x] SubTask 2.6: 编写单元测试：记录单次/多次、min/max 更新、reset 恢复、空统计查询

- [x] Task 3: 实现 `crates/kernel/sched/src/wcet.rs`（WCET 估算）
  - [x] SubTask 3.1: 定义 `static WCET_TABLE: Spinlock<[u64; MAX_THREADS]>`，const 初始化全 0
  - [x] SubTask 3.2: 实现 `wcet_set(tid: Tid, ns: u64)`：写入 `WCET_TABLE[tid.0]`，越界静默丢弃
  - [x] SubTask 3.3: 实现 `wcet_estimate(tid: Tid) -> u64`：读取 `WCET_TABLE[tid.0]`，越界返回 0
  - [x] SubTask 3.4: 实现 `check_partition_overrun(partition: u32, slot_duration_ns: u64) -> Option<Tid>`：遍历 THREAD_TABLE 找该分区线程，若 `wcet_estimate(tid) > slot_duration_ns` 返回首个超时 Tid（需 `unsafe` 访问 THREAD_TABLE，或通过 `thread_state` API 间接查询）
  - [x] SubTask 3.5: 编写单元测试：默认 WCET 为 0、设置后查询、超时检测、越界处理

- [x] Task 4: 实现 `crates/kernel/sched/src/timeline.rs`（时间片配置）
  - [x] SubTask 4.1: 定义 `const MAX_SLOTS: usize = 16`
  - [x] SubTask 4.2: 定义 `PartitionSlot { partition: PartitionId, duration_ms: u32 }`，derive `Clone, Copy, Debug`
  - [x] SubTask 4.3: 定义 `MajorFrame { slots: [PartitionSlot; MAX_SLOTS], slot_count: usize, period_ms: u32, current_slot: usize, frame_start_ns: u64 }`，实现 `MajorFrame::new()` const 初始化
  - [x] SubTask 4.4: 实现 `MajorFrame::add_slot(&mut self, partition, duration_ms) -> Result<(), SchedError>`：追加 slot，更新 period_ms，满时返回 `Err(SchedError::SlotFull)`
  - [x] SubTask 4.5: 实现 `MajorFrame::advance_slot(&mut self) -> usize`：推进 current_slot，回绕到 0 时返回新起始
  - [x] SubTask 4.6: 实现 `MajorFrame::current_partition(&self) -> Option<PartitionId>`：返回当前 slot 的分区
  - [x] SubTask 4.7: 实现 `MajorFrame::current_duration_ns(&self) -> u64`：返回当前 slot 的 duration（ns）
  - [x] SubTask 4.8: 编写单元测试：添加 slot、超出上限、推进回绕、空帧查询、周期计算

- [x] Task 5: 实现 `crates/kernel/sched/src/partition_sched.rs`（分区调度器核心）
  - [x] SubTask 5.1: 定义 `PartitionId(pub u32)` 新类型，derive `Clone, Copy, Debug, PartialEq, Eq`
  - [x] SubTask 5.2: 定义时间源注入静态变量：`static TIME_SOURCE: Spinlock<Option<fn() -> u64>>` + `static TIMER_REGISTRAR: Spinlock<Option<fn(u64, fn()) -> bool>>`，const 初始化 None
  - [x] SubTask 5.3: 实现 `set_time_source(f: fn() -> u64)` / `set_timer_registrar(f: fn(u64, fn()) -> bool)` / `now_ns() -> u64`（默认返回 0）
  - [x] SubTask 5.4: 定义 `static FRAME: Spinlock<MajorFrame>` + `static CURRENT_PARTITION: Spinlock<Option<PartitionId>>` + `static SWITCH_COUNT: Spinlock<u64>`，const 初始化
  - [x] SubTask 5.5: 实现 `schedule_add(partition, duration_ms) -> Result<(), SchedError>`：委托 `MajorFrame::add_slot`
  - [x] SubTask 5.6: 实现 `schedule_run() -> Result<(), SchedError>`：初始化 frame_start_ns，设置 current_slot=0，若有 TIMER_REGISTRAR 则注册首个周期定时器（`on_tick` 回调），无则返回 `Err(NoTimerRegistrar)`
  - [x] SubTask 5.7: 实现 `schedule_stop()`：取消调度（设置运行标志为 false，定时器由调用方取消）
  - [x] SubTask 5.8: 实现 `current_partition() -> Option<PartitionId>`：查询 CURRENT_PARTITION
  - [x] SubTask 5.9: 实现 `on_tick()`：计算抖动（`now_ns - expected`），调用 `record_jitter`，推进 slot，更新 frame_start_ns（回绕时），调用 `switch_partition`
  - [x] SubTask 5.10: 实现 `switch_partition(partition)`：设置 CURRENT_PARTITION，递增 SWITCH_COUNT
  - [x] SubTask 5.11: 编写单元测试：schedule_add 正常/超限、schedule_run 无/有 mock 定时器、on_tick 推进与回绕、current_partition 查询、switch_partition 切换计数

- [x] Task 6: 修改 `crates/kernel/sched/src/lib.rs` 导出新模块
  - [x] SubTask 6.1: 添加 `pub mod jitter;` `pub mod partition_sched;` `pub mod timeline;` `pub mod wcet;`
  - [x] SubTask 6.2: 添加 `pub use` 导出：`PartitionId`/`PartitionSlot`/`MajorFrame`/`MAX_SLOTS`/`schedule_add`/`schedule_run`/`schedule_stop`/`current_partition`/`on_tick`/`set_time_source`/`set_timer_registrar`/`JitterStats`/`record_jitter`/`jitter_measure`/`jitter_reset`/`wcet_set`/`wcet_estimate`/`check_partition_overrun`
  - [x] SubTask 6.3: 在 `SchedError` 枚举中添加 `NoTimerRegistrar` 与 `SlotFull` 变体（若 SchedError 在 isolation.rs 中定义，则修改该文件）
  - [x] SubTask 6.4: 更新 lib.rs 顶部文档注释，追加 v0.19.0 分区调度器说明

- [x] Task 7: 创建文档（由并行 sub-agent 完成）
  - [x] SubTask 7.1: 创建 `docs/smp/partition-scheduler-design.md`（~400 行）：ARINC 653 分区调度原理、MajorFrame 结构、时间片配置、on_tick 切换流程、与 v0.16.0/v0.18.0 调度器的关系
  - [x] SubTask 7.2: 创建 `docs/smp/arinc653-adaptation.md`（~350 行）：ARINC 653 标准摘要、EnerOS 适配范围（时间触发分区、major frame）、与完整 ARINC 653 的差异（未实现分区模式、健康监控等）、电力调度可借鉴点
  - [x] SubTask 7.3: 创建 `docs/smp/wcet-analysis.md`（~300 行）：WCET 概念、静态表方案、超时检测机制、与分区时间片的关系、未来改进方向（形式化分析）

- [x] Task 8: 验证与回归
  - [x] SubTask 8.1: `cargo fmt --all -- --check`
  - [x] SubTask 8.2: `cargo clippy -p eneros-sched --all-targets -- -D warnings`
  - [x] SubTask 8.3: `cargo test -p eneros-sched`（含新增 jitter/wcet/timeline/partition_sched 测试 + v0.16.0/v0.18.0 回归）
  - [x] SubTask 8.4: `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - [x] SubTask 8.5: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（v0.18.0 线程抽象不退化）
  - [x] SubTask 8.6: `git status` 确认无垃圾文件（无 `target/`、`*.elf`、`*.bin`）

# Task Dependencies

- Task 2（jitter）/ Task 3（wcet）/ Task 4（timeline）可与 Task 1 并行（版本号独立）
- Task 5（partition_sched）依赖 Task 2（jitter 的 record_jitter）、Task 4（timeline 的 MajorFrame）
- Task 6（lib.rs 导出）依赖 Task 2/3/4/5 完成
- Task 7（文档）依赖 Task 2/3/4/5 完成（文档描述实现）
- Task 8（验证）依赖所有前序任务完成

# Notes

- v0.19.0 非瓶颈版本（蓝图未标 ★），代码"骨架可用"——算法完整无 stub，但抖动 < 1ms 性能验证延后至 QEMU 实机
- sched crate 保持零外部依赖（D2）：时间源通过函数指针注入，非 crate 依赖
- `SchedError` 枚举当前在 `isolation.rs` 中定义，需添加 `NoTimerRegistrar` 与 `SlotFull` 变体
- `check_partition_overrun` 需访问 `THREAD_TABLE`（在 `tcb.rs` 中），通过 `thread_state` API 间接查询分区归属，或直接 `unsafe` 访问——选择间接查询以减少 unsafe
- 测试预估：jitter ~6 + wcet ~5 + timeline ~7 + partition_sched ~8 = ~26 个新测试，加 v0.18.0 原 72 个 = ~98 总测试
