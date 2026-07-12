# Checklist — EnerOS v0.16.0 多核调度器

> **蓝图依据**：`蓝图/phase0.md` §v0.16.0（第 3353-3693 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（瓶颈版本★，骨架可用，无 stub）
> **验收标准**：蓝图 §7（第 3671-3676 行）

---

## 1. Crate 骨架

- [x] `sched/Cargo.toml` 存在，name = "eneros-sched"，version = "0.16.0"
- [x] `sched/Cargo.toml` 零外部依赖（D2，不依赖 spin/heapless/eneros-smp）
- [x] `sched/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] `sched/src/lib.rs` 声明 `pub mod percore` / `pub mod affinity` / `pub mod isolation` / `pub mod balance`
- [x] `sched/src/lib.rs` 含 `#![allow(dead_code)]` 标注示例代码
- [x] workspace `Cargo.toml` members 含 "sched"
- [x] workspace `Cargo.toml` version = "0.16.0"

## 2. per-core 运行队列（percore.rs）

- [x] `Spinlock` 结构体定义（`locked: AtomicBool`），`const fn new()`
- [x] `Spinlock::lock()` 实现（`compare_exchange_weak` + 双层 spin_loop backoff）
- [x] `Spinlock::unlock()` 实现（Release store）
- [x] `Tid(pub u32)` 定义，derive Clone/Copy/PartialEq/Eq/Debug/Default
- [x] `PerCoreRq` 结构体定义（core_id/runnable[Option<Tid>;64]/count/current/reserved/lock 六字段）
- [x] `PerCoreRq::new(core_id)` 实现（`const fn`，初始化全 None）
- [x] `PerCoreRq::enqueue(&mut self, tid)` 实现（找第一个 None slot 填入，count++）
- [x] `PerCoreRq::dequeue(&mut self) -> Option<Tid>` 实现（取第一个 Some slot，count--）
- [x] `PerCoreRq::load(&self) -> usize` 实现
- [x] `PerCoreRq::remove(&mut self, tid) -> bool` 实现（按 Tid 查找移除）
- [x] 单元测试覆盖（≥ 5 个测试：Spinlock、enqueue/dequeue、队列满、remove、load 查询）

## 3. 核亲和性（affinity.rs）

- [x] `CoreMask(pub u64)` 定义，derive Clone/Copy/Default/Debug/PartialEq/Eq
- [x] `CoreMask::single(core)` 实现（`1u64 << core`）
- [x] `CoreMask::all(count)` 实现（`(1u64 << count) - 1`，处理 count=64 边界）
- [x] `CoreMask::contains(core)` 实现
- [x] `CoreMask::add(core)` / `remove(core)` 实现
- [x] `CoreMask::count()` 实现（`count_ones()`）
- [x] `CoreMask::is_empty()` 实现
- [x] `CoreMask::intersects(other)` 实现（交集测试）
- [x] 单元测试覆盖（≥ 6 个测试：single/all/contains/add/remove/count/is_empty/intersects，边界值 count=0/count=64）

## 4. RTOS 绑核隔离（isolation.rs）

- [x] `SchedError` 枚举定义（InvalidCore/CoreReserved/NoRunnableTask），derive Debug/Clone/Copy/PartialEq/Eq
- [x] `CoreReservation` 结构体定义（`reserved: [bool; 8]`）
- [x] `CoreReservation::new()` 实现（`const fn`，全 false）
- [x] `CoreReservation::reserve(core)` 实现（已 reserved → Err(CoreReserved)；core≥8 → Err(InvalidCore)）
- [x] `CoreReservation::release(core)` 实现
- [x] `CoreReservation::is_reserved(core)` 实现
- [x] `CoreReservation::can_enqueue(core, is_rtos)` 实现（reserved 核仅允许 RTOS）
- [x] 单元测试覆盖（≥ 5 个测试：reserve/release/is_reserved/can_enqueue、重复 reserve、core≥8、RTOS vs 非 RTOS）

## 5. 负载均衡（balance.rs）

- [x] `Balancer` 结构体定义（threshold/interval_ms 两字段）
- [x] `Balancer::new(threshold, interval_ms)` 实现
- [x] `Balancer::default()` 实现（threshold=2, interval_ms=10）
- [x] `Balancer::balance(rqs, core_count)` 实现（找最忙/最闲核→差值超阈值→迁移）
- [x] `Balancer::find_busiest(rqs, core_count)` helper 实现
- [x] 迁移时正确加锁/解锁 per-core RQ（lock→dequeue→unlock→lock→enqueue→unlock）
- [x] 迁移失败不崩溃（空队列静默返回）
- [x] 单元测试覆盖（≥ 4 个测试：差>阈值触发迁移、差<阈值不迁移、空队列不 panic、reserved 核处理）

## 6. 调度器主入口（lib.rs）

- [x] `Scheduler` 结构体定义（rqs/core_count/reservation/balancer/affinity 五字段）
- [x] `sched_init(core_count)` 实现（初始化 8 个 PerCoreRq + 默认 balancer/reservation/affinity）
- [x] `set_affinity(sched, tid, cores)` 实现（tid≥256 返回 Err(NoRunnableTask)）
- [x] `pin_to_core(sched, tid, core)` 实现（core≥core_count 返回 Err(InvalidCore)）
- [x] `reserve_core(sched, core)` 实现（委托 reservation.reserve）
- [x] `release_core(sched, core)` 实现（委托 reservation.release）
- [x] `enqueue(sched, tid, core)` 实现（检查 can_enqueue→加锁→PerCoreRq::enqueue→解锁）
- [x] `dequeue(sched, tid)` 实现（遍历所有 RQ 调用 remove）
- [x] `pick_next(sched, core)` 实现（加锁→dequeue→解锁→设 current）
- [x] `balance_load(sched)` 实现（委托 balancer.balance）
- [x] 单元测试覆盖（≥ 6 个测试：sched_init、set_affinity/pin_to_core、reserve/release、enqueue/pick_next、balance_load、RTOS 拒绝非 RTOS）

## 7. no_std 合规

- [x] `sched/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`（除 `#[cfg(test)]` 模块内）
- [x] 使用 `core::sync::atomic::AtomicBool` 而非 `std::sync::atomic`
- [x] 使用 `core::*` 而非 `std::*`
- [x] 零外部依赖（无 spin/heapless）

## 8. 瓶颈版本合规（蓝图 §43.2）

- [x] 无 `todo!()` / `unimplemented!()` / 返回 null 的 stub
- [x] 关键算法路径完整（Spinlock CAS+backoff、负载均衡扫描+迁移）
- [x] 所有接口签名与蓝图 §4.2 一致
- [x] 代码标注"示例代码，非生产就绪"（`#![allow(dead_code)]` + 文档注释）

## 9. 构建系统

- [x] `Makefile` VERSION := 0.16.0
- [x] `Makefile` 含 sched-build / sched-test 目标
- [x] `ci.yml` 版本标识 v0.16.0
- [x] `ci.yml` 含 "Build sched crate" cross-build 步骤
- [x] `ci/src/gate.rs` 注释含 v0.16.0

## 10. 文档

- [x] `docs/multi-core-scheduler-design.md` 存在（调度器整体架构、per-core RQ、Scheduler 结构、sched_init 流程）
- [x] `docs/cpu-affinity-policy.md` 存在（CoreMask 位运算、set_affinity/pin_to_core、亲和性匹配规则）
- [x] `docs/rtos-core-pinning.md` 存在（CoreReservation、reserve/release、RTOS 绑核独占、混合关键性架构）

## 11. 验证

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy -p eneros-sched --all-targets -- -D warnings` 通过
- [x] `cargo test -p eneros-sched` 全部通过（预期 ≥ 25 个测试）
- [x] `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello --exclude eneros-user-heap` 全部通过（回归，v0.15.0 smp 不退化）
- [x] `git status` 无垃圾文件

## 12. 蓝图验收标准（§7）

- [x] §7.1 RTOS 独占 Core 0，不被 Agent 抢占（CoreReservation.can_enqueue 规则）
- [x] §7.2 Agent 分布在 Core 1+ 运行（enqueue 检查 reserved，非 RTOS 不进 reserved 核）
- [x] §7.3 负载均衡生效（负载差 < 阈值）（Balancer.balance 迁移逻辑）
- [x] §7.4 pick_next < 1μs（不在 host 测，文档标注；aarch64 真机验证留待 QEMU 阶段）
- [x] §7.5 出口判定：多核调度就绪，RTOS 绑核达成

## 13. 外科手术原则自检（Karpathy §3）

- [x] **未修改** 任何现有 crate 源码（smp/panic/time/watchdog/hal/kernel 等）
- [x] 新增文件仅限 sched/ crate 五个源文件 + 三份文档
- [x] 修改文件仅限 Cargo.toml / Makefile / ci.yml / gate.rs 四个构建配置
- [x] 无"顺手改进"其他代码（每行改动可追溯到 v0.16.0 需求）
- [x] 无过度抽象（零外部依赖、自定义 Spinlock 仅 15 行、固定大小数组无堆分配）
