# Tasks — EnerOS v0.16.0 多核调度器

> **蓝图依据**：`蓝图/phase0.md` §v0.16.0（第 3353-3693 行）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动
> **依赖**：v0.15.0（多核启动与 IPI，已满足）
> **合规**：瓶颈版本（★），代码必须"骨架可用"（蓝图 §43.2）

---

## Task 1: 创建 `sched` crate 骨架

- [x] SubTask 1.1: 创建 `sched/Cargo.toml`（name=eneros-sched, version=0.16.0, 零依赖）
- [x] SubTask 1.2: 创建 `sched/src/lib.rs`（`#![cfg_attr(not(test), no_std)]`，模块声明 percore/affinity/isolation/balance，`#![allow(dead_code)]` 标注示例代码）
- [x] SubTask 1.3: 创建 `sched/src/percore.rs`、`sched/src/affinity.rs`、`sched/src/isolation.rs`、`sched/src/balance.rs` 最小存根
- [x] SubTask 1.4: 更新 workspace `Cargo.toml`（members 添加 "sched"，version 改 0.16.0）
- [x] 验证：`cargo build -p eneros-sched` 成功

## Task 2: 实现 per-core 运行队列（`sched/src/percore.rs`）

- [x] SubTask 2.1: 实现 `Spinlock` 结构体（`locked: AtomicBool`，`const fn new()`，`lock()` 用 `compare_exchange_weak` + 双层 spin_loop backoff，`unlock()` 用 Release store）。约 15 行
- [x] SubTask 2.2: 定义 `Tid(pub u32)`（derive Clone, Copy, PartialEq, Eq, Debug, Default）
- [x] SubTask 2.3: 定义 `PerCoreRq` 结构体（`core_id: u32`、`runnable: [Option<Tid>; 64]`、`count: usize`、`current: Option<Tid>`、`reserved: bool`、`lock: Spinlock`）
- [x] SubTask 2.4: 实现 `PerCoreRq::new(core_id: u32) -> Self`（`const fn`，初始化全 None）
- [x] SubTask 2.5: 实现 `PerCoreRq::enqueue(&mut self, tid: Tid)`（找第一个 None slot 填入，count++）
- [x] SubTask 2.6: 实现 `PerCoreRq::dequeue(&mut self) -> Option<Tid>`（取第一个 Some slot，count--）
- [x] SubTask 2.7: 实现 `PerCoreRq::load(&self) -> usize`（返回 count）
- [x] SubTask 2.8: 实现 `PerCoreRq::remove(&mut self, tid: Tid) -> bool`（按 Tid 查找并移除，用于 dequeue(tid)）
- [x] SubTask 2.9: 编写单元测试（Spinlock lock/unlock、PerCoreRq enqueue/dequeue/load、队列满不 panic、remove 操作）
- [x] 验证：`cargo test -p eneros-sched percore` 通过

## Task 3: 实现核亲和性（`sched/src/affinity.rs`）

- [x] SubTask 3.1: 定义 `CoreMask(pub u64)`（derive Clone, Copy, Default, Debug, PartialEq, Eq）
- [x] SubTask 3.2: 实现 `CoreMask::single(core: u32) -> Self`（`1u64 << core`）
- [x] SubTask 3.3: 实现 `CoreMask::all(count: u32) -> Self`（`(1u64 << count) - 1`，处理 count=64 的边界）
- [x] SubTask 3.4: 实现 `CoreMask::contains(&self, core: u32) -> bool`（位测试）
- [x] SubTask 3.5: 实现 `CoreMask::add(&mut self, core: u32)`（位设置）
- [x] SubTask 3.6: 实现 `CoreMask::remove(&mut self, core: u32)`（位清除）
- [x] SubTask 3.7: 实现 `CoreMask::count(&self) -> u32`（`count_ones()`）
- [x] SubTask 3.8: 实现 `CoreMask::is_empty(&self) -> bool`（`self.0 == 0`）
- [x] SubTask 3.9: 实现 `CoreMask::intersects(&self, other: CoreMask) -> bool`（交集测试，用于亲和性匹配）
- [x] SubTask 3.10: 编写单元测试（single/all/contains/add/remove/count/is_empty/intersects 各场景，边界值 count=0/count=64）
- [x] 验证：`cargo test -p eneros-sched affinity` 通过

## Task 4: 实现 RTOS 绑核隔离（`sched/src/isolation.rs`）

- [x] SubTask 4.1: 定义 `SchedError` 枚举（InvalidCore / CoreReserved / NoRunnableTask，derive Debug, Clone, Copy, PartialEq, Eq）
- [x] SubTask 4.2: 定义 `CoreReservation` 结构体（`reserved: [bool; 8]`）
- [x] SubTask 4.3: 实现 `CoreReservation::new() -> Self`（`const fn`，全 false）
- [x] SubTask 4.4: 实现 `CoreReservation::reserve(&mut self, core: u32) -> Result<(), SchedError>`（已 reserved 返回 Err(CoreReserved)；core≥8 返回 Err(InvalidCore)）
- [x] SubTask 4.5: 实现 `CoreReservation::release(&mut self, core: u32)`（清除 reserved 标记）
- [x] SubTask 4.6: 实现 `CoreReservation::is_reserved(&self, core: u32) -> bool`（查询核是否被独占）
- [x] SubTask 4.7: 实现 `CoreReservation::can_enqueue(&self, core: u32, is_rtos: bool) -> bool`（reserved 核仅允许 RTOS 线程入队）
- [x] SubTask 4.8: 编写单元测试（reserve/release/is_reserved/can_enqueue 各场景、重复 reserve 返回 Err、core≥8 返回 Err、RTOS vs 非 RTOS can_enqueue）
- [x] 验证：`cargo test -p eneros-sched isolation` 通过

## Task 5: 实现负载均衡（`sched/src/balance.rs`）

- [x] SubTask 5.1: 定义 `Balancer` 结构体（`threshold: usize`（迁移阈值，默认 2）、`interval_ms: u32`（均衡周期，默认 10））
- [x] SubTask 5.2: 实现 `Balancer::new(threshold: usize, interval_ms: u32) -> Self`
- [x] SubTask 5.3: 实现 `Balancer::default() -> Self`（threshold=2, interval_ms=10）
- [x] SubTask 5.4: 实现 `Balancer::balance(&self, rqs: &mut [PerCoreRq; 8], core_count: u32)`：
  - 步骤 1：遍历 0..core_count 找最忙核（max_load/max_core）和最闲核（min_load/min_core）
  - 步骤 2：如果 `max_load - min_load > threshold` 且 `max_core != min_core`
  - 步骤 3：从 max_core 的 RQ 加锁→dequeue→解锁
  - 步骤 4：如果取到线程，加入 min_core 的 RQ 加锁→enqueue→解锁
  - 步骤 5：迁移失败（队列为空）不崩溃，静默返回
- [x] SubTask 5.5: 实现 `Balancer::find_busiest(&self, rqs: &[PerCoreRq; 8], core_count: u32) -> (usize, usize)` helper（返回 max_core, min_core）
- [x] SubTask 5.6: 编写单元测试（均衡前各核负载差>阈值→迁移后差减小、差<阈值不迁移、空队列不 panic、reserved 核不参与均衡迁移源但可作为目标）
- [x] 验证：`cargo test -p eneros-sched balance` 通过

## Task 6: 实现调度器主入口（`sched/src/lib.rs`）

- [x] SubTask 6.1: 定义 `Scheduler` 结构体（`rqs: [PerCoreRq; 8]`、`core_count: u32`、`reservation: CoreReservation`、`balancer: Balancer`、`affinity: [CoreMask; 256]`）
- [x] SubTask 6.2: 实现 `sched_init(core_count: u32) -> Scheduler`（初始化 8 个 PerCoreRq，设置 core_count，默认 balancer/reservation/affinity）
- [x] SubTask 6.3: 实现 `set_affinity(sched: &mut Scheduler, tid: Tid, cores: CoreMask) -> Result<(), SchedError>`（tid≥256 返回 Err(NoRunnableTask)）
- [x] SubTask 6.4: 实现 `pin_to_core(sched: &mut Scheduler, tid: Tid, core: u32) -> Result<(), SchedError>`（core≥core_count 返回 Err(InvalidCore)；调用 set_affinity with CoreMask::single(core)）
- [x] SubTask 6.5: 实现 `reserve_core(sched: &mut Scheduler, core: u32) -> Result<(), SchedError>`（委托 reservation.reserve）
- [x] SubTask 6.6: 实现 `release_core(sched: &mut Scheduler, core: u32)`（委托 reservation.release）
- [x] SubTask 6.7: 实现 `enqueue(sched: &mut Scheduler, tid: Tid, core: u32)`（检查 reservation.can_enqueue，加锁 RQ，调用 PerCoreRq::enqueue）
- [x] SubTask 6.8: 实现 `dequeue(sched: &mut Scheduler, tid: Tid)`（遍历所有 RQ 调用 remove）
- [x] SubTask 6.9: 实现 `pick_next(sched: &mut Scheduler, core: u32) -> Option<Tid>`（加锁 RQ→dequeue→解锁→设 current）
- [x] SubTask 6.10: 实现 `balance_load(sched: &mut Scheduler)`（委托 balancer.balance）
- [x] SubTask 6.11: 编写单元测试（sched_init 初始化、set_affinity/pin_to_core、reserve_core/release_core、enqueue/pick_next 流程、balance_load 触发迁移、RTOS 拒绝非 RTOS 线程）
- [x] 验证：`cargo test -p eneros-sched` 全部通过

## Task 7: 更新构建系统

- [x] SubTask 7.1: 更新 `Makefile`（VERSION := 0.16.0，添加 sched-build / sched-test 目标）
- [x] SubTask 7.2: 更新 `.github/workflows/ci.yml`（版本标识 v0.16.0，添加 sched crate cross-build 步骤）
- [x] SubTask 7.3: 更新 `ci/src/gate.rs`（注释含 v0.16.0）
- [x] 验证：`cargo fmt --all -- --check` 通过

## Task 8: 编写文档

- [x] SubTask 8.1: 创建 `docs/multi-core-scheduler-design.md`（调度器整体架构、per-core RQ 设计、Scheduler 结构、sched_init 初始化流程、与其他 crate 关系、蓝图 §4.1-4.3 对齐）
- [x] SubTask 8.2: 创建 `docs/cpu-affinity-policy.md`（CoreMask 位运算设计、set_affinity/pin_to_core 接口、亲和性匹配规则、256 线程亲和性表、边界条件处理）
- [x] SubTask 8.3: 创建 `docs/rtos-core-pinning.md`（CoreReservation 机制、reserve_core/release_core、RTOS 绑核独占策略、can_enqueue 规则、与蓝图混合关键性架构关系）

## Task 9: 验证

- [x] SubTask 9.1: `cargo fmt --all -- --check` 通过
- [x] SubTask 9.2: `cargo clippy -p eneros-sched --all-targets -- -D warnings` 通过
- [x] SubTask 9.3: `cargo test -p eneros-sched` 全部通过（预期 ≥ 25 个测试）
- [x] SubTask 9.4: `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] SubTask 9.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（回归，v0.15.0 smp 不退化）
- [x] SubTask 9.6: `git status` 无垃圾文件

---

# Task Dependencies

- Task 1（crate 骨架）→ Task 2-6 依赖
- Task 2（percore.rs）和 Task 3（affinity.rs）和 Task 4（isolation.rs）无互相依赖，可并行
- Task 5（balance.rs）依赖 Task 2（PerCoreRq）和 Task 4（SchedError）
- Task 6（lib.rs）依赖 Task 2-5 全部完成
- Task 7（构建系统）独立，可与 Task 2-6 并行
- Task 8（文档）依赖 Task 2-6 完成
- Task 9（验证）依赖全部完成

**并行机会**：Task 2 + Task 3 + Task 4 可并行；Task 7 可与 Task 2-6 并行。

---

# 蓝图符合性自检

| 蓝图条目 | 任务覆盖 |
|---------|---------|
| §3 交付物 percore.rs(~200行)/affinity.rs(~250行)/balance.rs(~280行)/isolation.rs(~150行) | Task 2 / Task 3 / Task 5 / Task 4 |
| §3 接口 set_affinity/pin_to_core/balance_load/reserve_core | SubTask 6.3 / 6.4 / 6.10 / 6.5 |
| §4.1 数据结构 CoreMask/PerCoreRq/Scheduler | SubTask 3.1 / 2.3 / 6.1 |
| §4.4 错误处理（CoreReserved/InvalidCore/NoRunnableTask） | SubTask 4.1 + Task 6 各接口 |
| §5.2 per-core RQ 减少锁竞争 | Task 2 Spinlock + PerCoreRq |
| §5.4 难点（锁竞争/迁移一致性/实时性） | Task 2 CAS backoff + Task 5 迁移 + Task 4 reserved |
| §6.1 单元 CoreMask/PerCoreRq/CoreReservation ≥80% | SubTask 2.9 / 3.10 / 4.8 |
| §6.3 性能 pick_next <1μs / 均衡 10ms | 不在 host 测，文档标注 |
| §6.4 回归 v0.15.0 不退化 | SubTask 9.5 |
| §6.5 故障注入 非 RTOS 线程入 reserved 核 | SubTask 4.8 + 6.11 |
| §7 验收标准 | checklist.md 覆盖 |
| §43.2 瓶颈版本骨架可用 | 全部代码无 stub，关键算法完整 |
