# EnerOS v0.135.0 ARINC 653 分区调度器设计（主时间框架 MAF + 时间触发分区轮转 + 两层调度）

> 版本：v0.135.0
> 适用范围：EnerOS 自研内核 ARINC 653 时间触发分区调度（主时间框架调度表 / slot 到期切换 / 分区层 + 线程层两层架构 / CNTP 1kHz tick 驱动；QEMU virt aarch64 目标机 `-M virt,gic-version=3` / host 测试）
> 蓝图依据：`蓝图/phase3.md` §v0.135.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> 配套文档：[scheduler_design.md](./scheduler_design.md)（v0.134.0 线程抽象与基本调度，inner 线程层来源）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 中断管理，CNTP 定时器与 IRQ 30 来源）、[partition_isolation.md](./partition_isolation.md)（v0.132.0 分区内存隔离，PartitionId/根页表/ASID 来源）、[vmm_design.md](./vmm_design.md)（v0.131.0 四级页表，switch_address_space 的 TTBR0/ASID 机制）、[../boot/qemu_run.md](../boot/qemu_run.md) §14（v0.135.0 QEMU 实测记录，含 ticks_per_ms 排障）

---

## 1. 概述

**核心一句话**：实现 ARINC 653 式时间触发分区调度，确保 RTOS 分区获得确定性 CPU 时间片（蓝图 §1 原样口径）。

- **Phase 定位**：P3-C 中断调度 SMP 线**第 3 版**（v0.133.0 ~ v0.138.0）；为 v0.139.0 IPC 提供分区隔离运行环境，SMP（v0.136.0）扩展此调度（蓝图 §1）。
- **业务价值**：混合关键性架构的核心——保证电力控制分区的 ms 级实时响应；RTOS 分区与 Agent/AI 分区在同一 CPU 上**时间隔离**（蓝图 §1）。
- **出口关联**：自研内核"分区调度"功能完整性证据，对应 v0.150.0 形式化证明目标（蓝图 §1）。
- **ADR-0001 口径**：依据 ADR-0001，Phase 3 主路径为 seL4 深度定制（方案 A），seL4 已具备调度域与时间片机制，方案 A 下复用 seL4 调度框架，仅自研 ARINC 653 分区调度策略（方案 A 保留的核心增量）；本实现属自研内核研究/fallback 线的功能完整性证据，非生产路径（蓝图 §1 裁剪口径原样）。
- **依赖关系**：前序版本 v0.134.0 线程调度可用（蓝图 §2）；假设单核分区时间片（SMP 在 v0.136.0，蓝图 §2）；阻塞口径：分区调度不正确阻塞 IPC（蓝图 §2）。

交付物落位（仓库规范 §2.3.1 子系统归口，偏差 D1）：

| 交付物 | 路径 |
|--------|------|
| ARINC 653 配置（TimeSlot/PartitionSchedule/PartitionState/default_schedule + MAX_SLOTS + ARINC1~ARINC3 单测） | [crates/kernel/microkernel/src/sched/arinc.rs](../../crates/kernel/microkernel/src/sched/arinc.rs) |
| 分区调度器（PartitionScheduler trait 五方法 + ArincScheduler 七字段 + tick/start_at/partition_time_remaining_at + PS1~PS6 单测） | [crates/kernel/microkernel/src/sched/partition_sched.rs](../../crates/kernel/microkernel/src/sched/partition_sched.rs) |
| 地址空间切换接口（switch_address_space 双形态 + SW1~SW2 单测；D4 主流程不激活） | [crates/kernel/microkernel/src/mem/partition.rs](../../crates/kernel/microkernel/src/mem/partition.rs) |
| 内核启动期接入（§3.9 分区调度冒烟段 psched_smoke/psched_timer_handler + ARINC BSS 静态 + PSCHED_* 观测原子量） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试（PSHT1~PSHT5） | [crates/kernel/microkernel/tests/partition_sched_test.rs](../../crates/kernel/microkernel/tests/partition_sched_test.rs) |
| 设计文档（蓝图 §3 文档类 / §7.4 验收） | [docs/kernel/partition_sched.md](./partition_sched.md)（本文档） |

> **配置类口径**：蓝图 §3 配置类「默认调度表：RTOS 5ms + Agent 20ms + RTOS 5ms + AI 30ms ...」落 [arinc.rs](../../crates/kernel/microkernel/src/sched/arinc.rs) `default_schedule()`——蓝图 §4.5 原样七槽（RTOS 5ms×4 + Agent 20ms×2 + AI 30ms），主时间框架周期 `period_ms = 90`（时间轴见 §4）。

**类型与 trait 一览**（[mod.rs](../../crates/kernel/microkernel/src/sched/mod.rs) 公开导出，蓝图 §3 接口类原样签名）：

| 项 | 定义 | 说明 |
|----|------|------|
| `TimeSlot` | struct 三字段 | `partition / duration_ms / offset_ms`——某分区在主时间框架中的一次确定性 CPU 占用窗口（蓝图 §3 原样） |
| `PartitionSchedule` | struct 三字段 | `slots / len / period_ms`——主时间框架调度表（`slots` 按 D2 定长化 `[Option<TimeSlot>; MAX_SLOTS]` + 填充前缀长度 `len`） |
| `PartitionState` | `enum { Idle, Running, ColdStart, WarmStart }` | 分区调度状态四变体（蓝图 §4.1 原样；见 §2.2） |
| `PartitionScheduler` | trait 五方法 | `configure_schedule / start / current_partition / partition_time_remaining / set_partition_affinity`（蓝图 §3 原样签名；`start`/`partition_time_remaining` 无参形态的时间源口径见偏差 D5） |
| `ArincScheduler` | struct 七字段 | `schedule / current_slot / slot_start_tick / ticks_per_ms / inner / affinity / cur_tick`（蓝图 §4.1 原样五字段 + affinity 亲和记录 + D5 `cur_tick` 内部时间态） |

---

## 2. ARINC 653 模型

### 2.1 主时间框架（MAF）与时间槽（slot）

- **主时间框架（Major Frame, MAF）**：`PartitionSchedule.period_ms`——调度表周期性重复的总周期（蓝图 §5.2「主时间框架（MAF）：周期性重复」）；默认表 90ms。
- **时间槽（slot）**：`TimeSlot { partition, duration_ms, offset_ms }`——某分区在 MAF 中的一次确定性 CPU 占用窗口；`offset_ms` 为槽起点相对主周期起点的偏移，`duration_ms` 为槽时长；默认表各槽 offset 与 duration 累计一致（Σduration == period，`validate()` 校验，见 §5 D3 口径）。
- **确定性保证**：调度决策仅由「当前 tick 与槽起点之差」驱动，不依赖分区行为——任一槽窗口内 CPU 归属唯一确定，RTOS 分区在每 90ms 主周期内获得 4×5ms 确定性时间片（蓝图 §9.1「ARINC 653 时间片完整」）。
- **定长化（D2）**：蓝图 `slots: Vec<TimeSlot>` → no_std 无堆改 `[Option<TimeSlot>; MAX_SLOTS]`（`MAX_SLOTS = 16`，与分区上限 `MAX_PARTITIONS` 同阶）+ `len` 填充前缀；不变式 `slots[..len]` 恒 `Some`、`slots[len..]` 恒 `None`；因 `Option<TimeSlot>` 与 `TimeSlot` 布局不同、禁 unsafe 不可 transmute 出切片视图，访问器为 `slot(i)` / `iter()`（按 push 顺序遍历已填充前缀）；push 满 16 → `OutOfMemory`（同 v0.134.0 find_free_tid 满表口径）。

### 2.2 分区状态机（四变体，蓝图 §4.1 原样）

```
                configure_schedule 合法表
   ┌─────────┐ ──────────────────► ┌─────────┐   start()/start_at()   ┌─────────┐
   │ColdStart│                     │  Idle   │ ─────────────────────► │ Running │
   │（首次上电│                     │（未启动/│                        │（占用当前│
   │ 初始化）│                     │ 未分到槽）│                        │ 时间槽） │
   └─────────┘                     └─────────┘                        └─────────┘
   ┌─────────┐                          ▲                                 │
   │WarmStart│ ─────────────────────────┴─────────────────────────────────┘
   │（重启恢复）│            主周期末回卷 slot 0 / 空表 idle 兜底（D6）
   └─────────┘
```

- 枚举按蓝图 §4.1 原样四变体落位（[arinc.rs](../../crates/kernel/microkernel/src/sched/arinc.rs) `PartitionState`，随 `mod.rs` 公开导出）；
- **本版本消费口径**：调度器不持有显式状态字段——「运行」由槽位轮转隐式表达（`current_partition()` 返回当前槽分区即 Running 语义），「空表/未启动」按 D6 兜底为 `PartitionId(0)` idle 口径；`ColdStart`/`WarmStart` 为分区启动语义预留，分区生命周期管理（含启动/重启）属后续版本（v0.139.0 IPC 前后分区实体化时集成）。

---

## 3. 两层调度架构

蓝图 §4.1 `ArincScheduler.inner: SchedulerImpl` 确立**分区层 + 线程层**两层嵌套（蓝图 §8.1 自认「两层嵌套复杂」）：

```
定时器 tick（CNTP 1kHz，IRQ 30）
        │
        ▼
┌─────────────────────────────────────────────┐
│ 分区层：ArincScheduler（时间触发，本版本激活） │
│   tick(current_tick) -> Option<PartitionId>  │
│   到期 → 前进下一槽（取模回卷）→ 返回新分区    │
│   ※ 分区切换为调度级：仅决定 inner 可选线程    │
│     的分区归属（D4：地址空间切换不激活）       │
└──────────────────┬──────────────────────────┘
                   │ 分区归属约束（schedule() 分区
                   │ 过滤属 v0.136.0 前补强，D7）
                   ▼
┌─────────────────────────────────────────────┐
│ 线程层：inner SchedulerImpl（v0.134.0，      │
│   32 桶位图 + 定长环形就绪队列 + 优先级抢占）  │
│   Tcb.partition 记录线程分区归属              │
│   ※ 本版本 IRQ 抢占切换不激活（见下）         │
└─────────────────────────────────────────────┘
```

- **分区层职责**：持有主时间框架调度表与槽态（`current_slot`/`slot_start_tick`），由 tick 驱动做确定性槽切换；`current_partition()` 为当前 CPU 归属判定的唯一权威（PSHT3 锚定：RTOS 窗口内归属恒为 RTOS 分区，不被就绪队中更高优先级、属他分区的线程干扰）。
- **线程层职责**：分区内线程的优先级抢占调度（v0.134.0 全量能力：32 桶 O(1) 就绪队/yield/tick 时间片/上下文切换）；线程创建时以 `Tcb.partition` 显式绑定分区（PSHT3：线程 A → `PartitionId(1)`，线程 B → `PartitionId(2)`）。
- **本版本线程层 IRQ 抢占不激活口径**（main.rs `psched_timer_handler` 文档注释决策记录）：inner 线程调度在 IRQ 内的抢占切换**不激活**——v0.134.0 已证协作式切换可行；tick 驱动线程抢占的寄存器级集成（IRQ 现场与 TCB `saved_regs` 互转 + 关中断临界区，蓝图 §8.5 坑点）属后续版本口径；本版本 QEMU 冒烟仅做**分区层轮转 + 观测**（inner `SchedulerImpl` 仅作结构有效化驻留，IRQ 内不做线程级 `context_switch`）。
- **schedule() 分区过滤 gap（D7）**：v0.134.0 `SchedulerImpl::schedule()` 为全局 32 桶优先级队列，入参与选路均不感知分区（无分区过滤 API）；现状若直接调 `inner.schedule()` 会选中高优先级的他分区线程（越区），属**已知 gap** 而非本版本断言对象——分区层约束 inner 可选线程集的过滤逻辑属 v0.136.0 SMP 前需补强的口径（tests/partition_sched_test.rs 头注决策记录）。

---

## 4. 默认调度表与时间轴

### 4.1 七槽默认表（蓝图 §4.5 原样）

`default_schedule()`：RTOS = `PartitionId(1)`、Agent = `PartitionId(2)`、AI = `PartitionId(3)`，主周期 `period_ms = 90`ms：

| slot | 分区 | duration_ms | offset_ms | 窗口（周期 1） |
|-----:|------|------------:|----------:|----------------|
| 0 | RTOS(1) | 5 | 0 | [0, 5) |
| 1 | Agent(2) | 20 | 5 | [5, 25) |
| 2 | RTOS(1) | 5 | 25 | [25, 30) |
| 3 | AI(3) | 30 | 30 | [30, 60) |
| 4 | RTOS(1) | 5 | 60 | [60, 65) |
| 5 | Agent(2) | 20 | 65 | [65, 85) |
| 6 | RTOS(1) | 5 | 85 | [85, 90) |

Σduration = 5+20+5+30+5+20+5 = 90 == period（`validate()` 通过；ARINC1 逐槽锚定）。

### 4.2 蓝图 §4.3 时间轴（原样收录）

```
┌────┬────────┬────┬────────┬────┬────────┬────┐
│RTOS│ Agent  │RTOS│   AI   │RTOS│ Agent  │RTOS│
│ 5ms│ 20ms   │ 5ms│ 30ms   │ 5ms│ 20ms   │ 5ms│
└────┴────────┴────┴────────┴────┴────────┴────┘
```

蓝图 §4.3 核心算法 Mermaid 图（原样口径：定时器 tick → slot 时间到？→ 否继续当前分区 / 是切换下一 slot 分区 → 切换地址空间 ASID → 切换到该分区最高优先级线程 → eret；主周期结束回到 slot 0）——本版本实现口径：**地址空间 ASID 切换不激活**（D4），**线程级切换不激活**（§3）；调度级分区层轮转与回卷原样兑现。

### 4.3 QEMU 实测序列对照（[qemu_run.md](../boot/qemu_run.md) §14）

QEMU 冒烟（CNTP 1kHz 驱动，`ticks_per_ms=1`，`start_at(0)`，串口 `[psched] p<n>` 为每次槽切换打印）实测切换序列与蓝图时间轴**逐槽吻合**：

| 切换 tick | 切入槽 | 打印 | 蓝图期望 |
|----------:|-------|------|---------|
| 5 | slot1 Agent | `[psched] p2` | ✅ |
| 25 | slot2 RTOS | `[psched] p1` | ✅ |
| 30 | slot3 AI | `[psched] p3` | ✅ |
| 60 | slot4 RTOS | `[psched] p1` | ✅ |
| 65 | slot5 Agent | `[psched] p2` | ✅ |
| 85 | slot6 RTOS | `[psched] p1` | ✅ |
| 90 | 回卷 slot0 RTOS | `[psched] p1` | ✅（主周期末 `(0+1) % 7` 回卷） |

实测串口序列原样：`p2→p1→p3→p1→p2→p1→p1`（slot0 RTOS 为起始槽不打印，恰 7 次切换 = `PSCHED_PERIOD_SWITCHES` 满一主周期判定阈值）。host 侧 PSHT1 以半开区间 [0,180) 收集两主周期共 13 次切换（5,25,30,60,65,85,90,95,115,120,150,155,175）与手工推演表逐一相等，PS3 覆盖单周期七边界 + 回卷后槽起点 90。

---

## 5. tick 切换算法

`ArincScheduler::tick(&mut self, current_tick: u64) -> Option<PartitionId>`（[partition_sched.rs](../../crates/kernel/microkernel/src/sched/partition_sched.rs)，蓝图 §4.5 主体）：

```
① 同步内部时间态：cur_tick = current_tick（D5）
② 取当前槽时长：slot_ticks = schedule.slot(current_slot).duration_ms × ticks_per_ms
   —— 空表/槽下标越界 → 返回 None（D6 防御：不推进、无取模除零）
③ 到期判定：elapsed = current_tick.saturating_sub(slot_start_tick)
   —— saturating 防 tick 回拨
   elapsed < slot_ticks → 返回 None（继续当前分区）
④ 到期切换：current_slot = (current_slot + 1) % schedule.len   （主周期末回卷 slot 0）
            slot_start_tick = current_tick                      （槽起点以实际切换 tick 重置）
            返回 Some(slot(current_slot).partition)             （次槽分区）
```

- **推进取模**：`(current_slot + 1) % len` 天然覆盖主周期末回卷（slot6 → slot0）；此处在 D6 保证下 `len > 0`，无除零；
- **复杂度**：O(1)——单次槽查表 + 整数比较/取模，无线性扫、无堆分配（PSHT5 实测 10,000 次 tick 均值 24ns）；
- **蓝图 §4.5 差异点**：蓝图的 `crate::mem::switch_address_space(next.partition)` 真实 TTBR0/ASID 切换**本版本不激活**（D4，见 §9）——分区切换为调度级，仅决定 inner 线程调度可选线程的分区归属；推进后槽下标恒在填充前缀内（`len > 0` 且取模），`slot()` 必 `Some`。

---

## 6. 超时强制切换与 idle 兜底

蓝图 §4.4 错误处理两规则的实现口径：

1. **slot 超时：强制切换（不允许分区超时占用）**——到期判定 `elapsed ≥ slot_ticks` 不依赖分区主动让出：即使分区 busy 未让出、tick 间隔被拖延越过槽长，下一次 tick 到达仍强制切换，槽起点以实际切换 tick 重置、后续窗口按新起点继续沿调度表推进（蓝图 §9.4「超时强制切换」/ §7.3「分区不能超时占用 CPU」）。PSHT2 故障注入实测：slot0（RTOS 5ms）期间 tick 从 t=4 直接跳到 t=7（越过槽长）→ `tick(7)` 仍返回 `Some(PartitionId(2))` 强制切入 slot1，`slot_start_tick` 重置为 7，新窗口 [7,27) 内未到期、t=27 沿时间轴正常推进回 RTOS。
2. **分区内无就绪线程：idle 直到 slot 结束**——线程层 idle 兜底由 v0.134.0 就位（idle 线程创建即入就绪队、prio 0 桶 0，无用户线程时经就绪队自然选中；见 [scheduler_design.md](./scheduler_design.md) §2.2）；分区层空表 idle 口径见 D6。

**空分区 idle 口径（D6）**：空表（`len=0`）行为蓝图未定义 → 防御口径：`tick` 恒返回 `None`（不推进、不取模除零）、`current_partition()` 返回 `PartitionId(0)`（idle 分区口径，与 `SchedulerImpl` idle 线程的 `PartitionId(0)` 一致）、`partition_time_remaining()` 返回 0；`start()` 无参形态在空表上同样无副作用（PS5 锚定：空表 `tick(10^6)` 不 panic 返回 None、槽态不动）。另附**除零防御**：`ticks_per_ms == 0` 时毫秒换算无定义，`partition_time_remaining_at` 返回 0（PS4 锚定）；**钳零防下溢**：`elapsed` 越过槽长时剩余返回 0 而不无符号下溢（蓝图 §4.5 直减口径在调度级仅作剩余提示；PSHT4：`partition_time_remaining_at(u64::MAX/2)` → 0 不 panic，蓝图 §9.6 可观测）。

---

## 7. 与 v0.134.0 衔接

v0.134.0 调度冒烟段的「timer 已停不重启 + 协作式 yield」驱动口径（[scheduler_design.md](./scheduler_design.md) §6 / 偏差 D8）在本版本**变更为 tick 驱动**——main.rs `psched_smoke` 文档注释记录四项决策：

1. **timer 重启**：v0.134.0「timer 已停不重启」决策变更为**重启 CNTP 1kHz**（蓝图 §8.5 坑点「tick 频率需足够高（≥ 1kHz）」：1kHz 即 1ms 周期，默认表最薄槽 5ms 亦有 5 个 tick 粒度，`PSCHED_TICK_HZ = 1000`）；读 `cntfrq_el0` 算 1ms 装载值 `tval = freq / 1000`（freq < 1kHz 不可达防御 `[psched] err freq`）；GIC 侧 IRQ 30 使能/PMR/Group 配置自 v0.133.0 段持续有效，本段仅重新 arm CNTP（`cntp_tval_el0` 装载 + `cntp_ctl_el0` 写 1）。
2. **handler 覆盖注册**：v0.133.0 `timer_handler` 完成 `[irq] ok` 一次性冒烟后已停定时器、预期不再触发（语义已完结）；本段经 `register_irq_handler(CNTP_IRQ, psched_timer_handler)` **同号覆盖注册**（IrqHandlerTable 同号覆盖生效口径），IRQ 30 后续中断由 `psched_timer_handler` 接管——每次触发：重装 tval（下一 1ms）→ `PSCHED_TICKS` +1 → `ArincScheduler::tick(count)` 驱动分区层轮转；返回 `Some(pid)` 时切换计数 +1、分区位图置位并经 raw_uart 打印 `[psched] p<n>`。
3. **线程层不激活**：inner `SchedulerImpl` 仅作结构有效化驻留，IRQ 内不做线程级 `context_switch`（§3 口径）；与 v0.134.0 调度冒烟段**正交**——本段不触碰 SCHED/线程设施，调度冒烟成败与否均执行（协作式线程与分区层互无依赖）。
4. **DONE 时序**：观测满一主周期（位图同时含分区 1&2 且切换 ≥ 7）或超时（tick ≥ 10,000 = 10 秒兜底，余量两个数量级）后，主流程先停定时器（CTL=0）再置位 `PSCHED_DONE`——时序契约：**DONE 置位蕴含 CTL 已停**，handler 见 DONE 直接认领返回、不再计数/打印/重装 tval（不可能电平风暴）；在途的一次尾部 IRQ 由 handler DONE 分支认领无害，防尾部在途 IRQ 干扰后续 idle。

冒烟链路（main.rs §3.9，`[sched] ok` 段之后进入；≈118KB `ArincScheduler` 内联 inner，严禁栈上构造，内核栈仅 64KB）：

```
① ARINC BSS 句柄就地有效化：schedule 以 const 空表整体覆写；current_slot/
   slot_start_tick 锚零；ticks_per_ms=1（D8：1kHz 下 1 tick 即 1ms，满一主
   周期 90 tick，同 host 测试口径）；inner threads 256 槽逐槽写 None（ready
   纯整数零值天然有效免写）；affinity/cur_tick 私有整数 BSS 全零即初值
② configure_schedule(default_schedule())（默认表 Σ=90=period 恒合法，
   err 分支不可达防御）→ start_at(0)
③ register_irq_handler(CNTP_IRQ, psched_timer_handler) 同号覆盖注册
④ 读 cntfrq_el0 → tval=freq/1000 → TIMER_TVAL 存储 → arm CNTP 1kHz
⑤ wfi 轮询观测：1ms 中断逐次唤醒；位图含分区 1&2 且切换 ≥7 → 成功
⑥ 停定时器 CTL=0 → 置位 DONE → 打印 [psched] ok
   （失败：[psched] err ticks=0x.. sw=0x.. seen=0x..，实测未触发）
```

---

## 8. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`（lib 内嵌单测 + tests/ 集成测试；host 上可使用 std）。

### 8.1 单元测试（arinc.rs 内嵌，ARINC1~ARINC3）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| ARINC1 | `arinc1_default_schedule_matches_blueprint_timeline` | default_schedule() 七槽逐槽与蓝图 §4.3 时间轴一致：len=7、period=90、Σduration==90、validate() Ok；slot(i)/iter() 口径一致、越界 None | PASS |
| ARINC2 | `arinc2_validate_rejects_bad_schedule` | Σ=15≠period=90 → Err(InvalidArgument)；空表（len=0，即使 period=0 形式成立）同样拒绝（D3 增强）；Σ==period 且非空 → Ok 对照 | PASS |
| ARINC3 | `arinc3_push_full_and_iteration_order` | push 满 16 后第 17 次 Err(OutOfMemory) 且状态不破坏；迭代顺序即 push 顺序；iter() 恰 len 项；slot(MAX_SLOTS) 越界 None | PASS |

### 8.2 单元测试（partition_sched.rs 内嵌，PS1~PS6）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| PS1 | `ps1_configure_rejects_invalid_and_keeps_old_table` | 合法表换表成功并复位 current_slot=0；非法表（Σ≠period）→ Err(InvalidArgument) 且**不覆盖**原默认表（先校验后换表，拒绝零副作用） | PASS |
| PS2 | `ps2_slot_boundary_advance` | slot0（RTOS 5ms）边界：tick(4) → None 不切换；边界 tick(5) → Some(PartitionId(2)) 切 Agent；current_slot=1、槽起点同步 5 | PASS |
| PS3 | `ps3_full_period_wraps_to_slot0` | 连续 tick 沿蓝图 §4.3 时间轴推进七边界（5/25/30/60/65/85/90），中途 tick 恒 None；t=90 回卷 slot 0 返回 Some(PartitionId(1))、槽起点 90 | PASS |
| PS4 | `ps4_time_remaining_monotonic_and_clamped` | 剩余随 tick 单调递减（5→1）；elapsed 越界钳零不溢出（t=999 → 0）；切换后新槽满量 20；ticks_per_ms=0 除零防御 → 0 | PASS |
| PS5 | `ps5_empty_table_defense` | 空表 new(1000)：tick(10^6) 不 panic 返回 None、槽态不动；current_partition()==PartitionId(0) idle 口径（D6）；剩余 0；start() 后空表仍不推进 | PASS |
| PS6 | `ps6_affinity_record_and_out_of_range_ignored` | set_partition_affinity(P2,3) → affinity_of==3 生效、他槽不受影响；越界 pid（≥MAX_PARTITIONS / usize::MAX）静默忽略无副作用、越界读取 0 | PASS |

### 8.3 单元测试（partition.rs 内嵌，SW1~SW2，D4 接口占位）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| SW1 | `sw1_host_switch_recorded` | host 记录桩：switch_address_space(0x1234_5000, 7) 后读回 (root_frame_phys, asid) 原值；aarch64 真实寄存器路径由 v0.139.0 前 QEMU 集成验证（D4 主流程不激活） | PASS |
| SW2 | `sw2_host_switch_overwrites` | 覆盖语义：第二次调用覆写记录读回新值 (0x2000, 0) | PASS |

> SW1/SW2 共用全局记录桩 `LAST_SWITCH`，经 `SWITCH_LOCK` 互斥防测试并行交叉覆写导致断言抖动。

### 8.4 集成测试（tests/partition_sched_test.rs，PSHT1~PSHT5，仅以公开 API 端到端验证）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| PSHT1 | `psht1_two_major_periods_slot_sequence` | 两主周期 slot 切换序列：[0,180) 收集 13 次切换（5,25,30,60,65,85,90,95,115,120,150,155,175）与手工推演表逐一相等；非切换时刻 tick 恒 None；t=175 已切 slot6、槽起点同步 | PASS |
| PSHT2 | `psht2_busy_partition_timeout_forces_switch` | 超时强制切换（故障注入）：busy 未让出 tick 4→7 越过 5ms 槽长仍 Some(P2) 强制切换；槽起点重置 7；新窗口 [7,27) 沿时间轴继续推进 | PASS |
| PSHT3 | `psht3_two_layer_scheduling_no_cross_partition` | 两层调度不越区：A(P1,prio10)/B(P2,prio30) 共存就绪队；slot0 RTOS 窗口分区层归属恒 P1 不被高优先级 B 干扰；边界 t=5 归属精确交接 P2；t=25 回 P1；Tcb.partition 归属记录正确 | PASS |
| PSHT4 | `psht4_time_remaining_monotonic_no_underflow` | slot0 剩余 5,4,3,2,1,0 单调递减；partition_time_remaining_at 纯查询不驱动槽态；u64::MAX/2 越界 saturating 钳零不 panic；切换后新槽满量 20、槽末 0 | PASS |
| PSHT5 | `psht5_switch_perf_and_jitter_smoke` | 切换性能与抖动 smoke：10,000 次 tick 计均值/峰值，相邻切换间隔 max-min 为抖动；断言均值 <10μs、抖动 <100μs 防算法退化；实测经 eprintln 输出 | PASS |

> **PSHT3 口径决策**（测试头注记录）：v0.134.0 `SchedulerImpl::schedule()` 全局 32 桶不感知分区（无分区过滤 API）→ 采用 fallback 口径仅断言**分区层**归属语义（current_partition() 恒为窗口分区，tick 返回值随时间轴精确交接）+ 线程层 `Tcb.partition` 归属记录；`schedule()` 分区过滤 gap（D7）属 v0.136.0 前补强口径，本测试不对 inner 调度行为断言。
>
> **PSHT5 口径声明**：实测均值 **24 ns/op**、相邻切换间隔抖动 **2.2 μs**——host 侧（用户态）smoke 口径，**不代表目标机（ARM64 裸机）性能**（同 v0.134.0 scht7 / v0.131.0 T07 口径惯例）；蓝图 §6.3/§7.2「切换 <10μs、抖动 <100μs」按本口径声明，目标机性能测定属后续版本。

### 8.5 QEMU 冒烟（main.rs §3.9 段 + [qemu_run.md](../boot/qemu_run.md) §14）

冒烟流程见 §7 链路图。串口输出关键行（原样收录，WSL2 QEMU 8.2.2，`-M virt,gic-version=3`）：

```text
[irq] ok
[sched] t1
[sched] t2
[sched] ok
[psched] p2
[psched] p1
[psched] p3
[psched] p1
[psched] p2
[psched] p1
[psched] p1
[psched] ok
```

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **八**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok`；v0.135.0 起追加第八条断言 `grep "[psched] ok"`，既有七条原样） |
| 切换序列 | `p2→p1→p3→p1→p2→p1→p1` 与蓝图 §4.3 时间轴逐槽吻合（对照表见 §4.3；实测于首次 arm 后 ~90ms 满一主周期，距 10,000 tick 超时兜底余量两个数量级） |
| kernel release ELF | 98,952 B（修复仅改常量立即数，指令数不变） |
| 端到端总耗时 | 13s（构建 2.99s + QEMU timeout 10s 守护窗口） |
| 排障 | 首轮 FAIL：`ticks_per_ms` 误写 `PSCHED_TICK_HZ`(1000) → slot0 占 5,000 tick（@1kHz = 5 墙钟秒）、全主周期 90,000 tick ≫ 10,000 tick 超时，仅见一次 `[psched] p2` 后静默；修复 `ticks_per_ms=1`（D8，见 §9） |
| 回归 | host **132** 测试全过零回归；`cargo fmt --check` 干净；release 交叉编译通过 |

蓝图 §6 测试计划映射：

| 蓝图条目 | 覆盖情况 |
|---------|---------|
| §6.1 单元测试「调度表验证 / slot 切换逻辑」 | ARINC1~ARINC3（表验证）+ PS2/PS3（slot 切换）+ PSHT4（剩余查询） |
| §6.2 集成测试「运行默认调度表，验证分区按时间片切换」 | PSHT1（两主周期 13 次切换序列）+ QEMU 冒烟实测 `p2→p1→p3→p1→p2→p1→p1` |
| §6.3 性能基准「分区切换 <10μs / 抖动 <100μs」 | PSHT5 host 实测均值 24ns、抖动 2.2μs（口径声明见 §8.4 注；目标机测定属后续版本） |
| §6.4 回归「线程调度不受影响」 | PSHT3（两层共存不越区）+ QEMU 冒烟 `[sched] ok` 先于 `[psched] ok` 通过 + host 132 测试零回归 |
| §6.5 故障注入「分区超时（busy loop），验证强制切换」 | PSHT2（tick 间隔拖延越过槽长仍强制切换，槽起点重置后沿表推进） |
| §6.6 GPU 规则 | 不适用（分区调度，蓝图原样口径） |

---

## 9. 蓝图偏差表（D1~D8）

> 文档级编号与源码头注模块内编号的对应关系：**arinc.rs 头注 D2 与本文档同号**；arinc.rs 头注「validate 增强空表拒绝」未编号记录 → 本文档 D3；**partition_sched.rs 头注 D5/D6 与本文档同号**；**partition.rs 头注 D4（v0.135.0 条目）与本文档同号**；D7 记录于 tests/partition_sched_test.rs 头注「偏差记录」段；D8 记录于 main.rs `psched_smoke` 有效化段 SAFETY 注释与 [qemu_run.md](../boot/qemu_run.md) §14 排障记录。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D1 | 蓝图交付物 `kernel/src/sched/{partition_sched,arinc}.rs`、`kernel/tests/partition_sched_test.rs` → 落位 `crates/kernel/microkernel/src/sched/` 与 `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/partition_sched.md` 同蓝图 | 仓库规范 §2.3.1 强制子系统归口（`crates/<subsystem>/`），禁止根目录 crate | 仅路径差异，交付物一一对应 |
| D2 | 蓝图 §3 `PartitionSchedule.slots: Vec<TimeSlot>` → no_std 无堆改定长数组 `[Option<TimeSlot>; MAX_SLOTS]`（16 槽）+ `len` 填充前缀（同 v0.134.0 ReadyRing 口径）；因 `Option<TimeSlot>` 与 `TimeSlot` 布局不同、禁 unsafe 不可 transmute 出切片视图，访问器为 `slot(i)` / `iter()`（= arinc.rs 头注 D2） | no_std 无堆（仓库规范 §4.3 硬性要求），Vec 不可用；容量编译期确定、无分配失败路径 | 槽位上限 16（push 满 → `OutOfMemory`，ARINC3 覆盖）；语义与蓝图一致；不变式 `slots[..len]` 恒 Some 由全部构造路径维持 |
| D3 | 蓝图 §4.5 configure_schedule 仅校验 Σduration == period（空表 + period=0 形式上可通过）→ `validate()` 追加 **len > 0** 前置条件：空表无语义属非法配置，Err(InvalidArgument)（= arinc.rs 头注「validate 增强」记录） | 空表 + period=0 使 Σ==period 形式成立却无语义；配合 D6 空表运行期防御构成「配置期拒绝 + 运行期兜底」双保险 | 较蓝图语义增强；ARINC2 锚定空表拒绝；`configure_schedule` 先校验后换表，非法表不覆盖原表（PS1） |
| D4 | 蓝图 §4.5 调度循环 `switch_address_space(next.partition)` 真实 TTBR0/ASID 切换**不在主流程激活**——v0.132.0 `create_partition` 创建的根页表为零填充空表，真实切换必 translation fault；`switch_address_space` 提供正确实现（aarch64：TTBR0_EL1 装载 BADDR\|ASID + DSB/TLBI ASID/DSB/ISB 序列；host：原子记录桩零 unsafe），留待 v0.139.0 IPC 前分区拥有独立映射时激活（= partition.rs 头注 D4；partition_sched.rs 头注同口径背书） | 分区根页表为空表，切换必 fault；接口先行交付正确实现与可观测桩 | 本版本分区切换为**调度级**——仅决定 inner 线程调度可选线程的分区归属；SW1/SW2 覆盖 host 桩记录/覆写语义；aarch64 寄存器路径由 v0.139.0 前 QEMU 集成验证 |
| D5 | 蓝图 §3 trait `start()` / `partition_time_remaining()` 无参，而 §4.5 实现 `start(current_tick)` / `partition_time_remaining(current_tick)` 带参（蓝图自身不一致）→ 采用「**调度器持有当前 tick**」口径：trait 签名保持 §3 原样无参，内部 `cur_tick` 由 `tick()`/`start_at()` 驱动同步，`start()` 以其为槽起点；另提供带参 `start_at` / `partition_time_remaining_at`（即蓝图 §4.5 带参原样形态）（= partition_sched.rs 头注 D5） | no_std 内核无全局时间源，无参 trait 若无内部时间态则不可实现，且蓝图自身实现即带参 | trait 与蓝图 §3 签名逐字一致；PS4/PSHT4 以带参形态做纯查询（不驱动槽态）；PS5 锚定无参形态空表无副作用 |
| D6 | 空表（len=0）行为蓝图未定义 → 防御口径：`tick` 恒返回 None（不推进、不取模除零）、`current_partition()` 返回 `PartitionId(0)`（idle 分区口径，与 `SchedulerImpl` idle 线程的 `PartitionId(0)` 一致）、`partition_time_remaining()` 返回 0（= partition_sched.rs 头注 D6） | 蓝图未定义，实现须自洽且禁 panic；`PartitionId(0)` 与 v0.134.0 idle 线程分区归属天然一致 | PS5 锚定：空表 tick(10^6) 不 panic、槽态不动；配合 D3「配置期拒绝空表」形成双保险（非法表 configure 进不来，直接构造的空表运行期安全） |
| D7 | `SchedulerImpl::schedule()` 分区过滤 gap——v0.134.0 就绪队为全局 32 桶优先级队列，入参与选路均不感知分区（无分区过滤 API）；现状直接调 `inner.schedule()` 会选中高优先级他分区线程（越区），属**已知 gap**，分区层约束 inner 可选线程集的过滤逻辑属 **v0.136.0 SMP 前需补强**的口径（= tests/partition_sched_test.rs 头注「偏差记录」段） | 两层嵌套的分区归属约束需线程层 API 支持（按分区过滤就绪集），属后续增量；本版本最小交付分区层时间触发语义 | PSHT3 采用 fallback 口径仅断言分区层归属语义 + `Tcb.partition` 记录，不对 inner 调度行为断言；QEMU 冒烟线程层不激活（§3），gap 不在本版本验收路径上暴露 |
| D8 | `ticks_per_ms = 1`（1kHz tick 下 1 tick 即 1ms，满一主周期恰 90 tick）——首轮 FAIL 排障：`psched_smoke` 有效化段误将 `ticks_per_ms` 写为 `PSCHED_TICK_HZ`(1000)，`tick()` 以 `duration_ms × ticks_per_ms` 算槽长，slot0（5ms）遂占 5,000 tick（@1kHz = 5 墙钟秒），全主周期 90,000 tick ≫ 10,000 tick 超时兜底；首次切换于 tick 5000 打出 p2 后次切换永不到达，err 行又与 QEMU 10s 墙钟超时赛跑落败故未观测到；修复改写 1（与 host 测试口径及 `PSCHED_TIMEOUT_TICKS` 注释「满一主周期仅需 90 tick」自洽）（= main.rs 有效化段 SAFETY 注释 + qemu_run.md §14 排障记录） | ms→tick 换算系数的语义口径错位（频率值 ≠ 每毫秒 tick 数）；surgical 单点修复不动既有段 | host 132 测试复跑全过零回归、`cargo fmt --check` 干净、release 重交叉编译通过（ELF 98,952B 不变，仅改常量立即数指令数不变）；修复后八断言 PASS |

---

## 10. 蓝图 §7 验收对照

| 蓝图 §7 验收标准 | 实现证据 | 结论 |
|-----------------|---------|------|
| §7.1 功能：分区时间片调度正确 | QEMU 冒烟实测切换序列 `p2→p1→p3→p1→p2→p1→p1` 与蓝图 §4.3 时间轴逐槽吻合（恰 7 次切换满一主周期，`[psched] ok`）；host PSHT1 两主周期 13 次切换序列与手工推演逐一相等、PS2 边界推进 / PS3 回卷 / ARINC1 默认表逐槽锚定全绿 | ✅ |
| §7.2 性能量化：切换 < 10μs，抖动 < 100μs | PSHT5 性能 smoke：10,000 次 tick 实测均值 **24 ns/op** << 10μs 上限、相邻切换间隔抖动 **2.2 μs** << 100μs 上限；**host 用户态口径声明**（不代表目标机 ARM64 裸机性能，同 v0.134.0 scht7 / v0.131.0 T07 口径惯例），目标机性能测定属后续版本 | ✅（host smoke + 口径声明） |
| §7.3 安全：分区不能超时占用 CPU | 到期判定 `elapsed ≥ slot_ticks` 不依赖分区主动让出（§6）；PSHT2 故障注入：busy 未让出 tick 间隔拖延越过槽长仍强制切换、槽起点重置后沿时间轴推进（蓝图 §4.4/§9.4 超时强制切换原样兑现） | ✅ |
| §7.4 文档：partition_sched.md | 本文档（[docs/kernel/partition_sched.md](./partition_sched.md)） | ✅ |
| §7.5 出口判定：分区时间片调度 → v0.136.0 SMP 可启动 | 八关键串断言 PASS（含 `[psched] ok`）+ host 132 测试全绿零回归；分区层时间触发轮转、默认表、亲和记录（`set_partition_affinity`/`affinity_of` 就位，SMP 生效属 v0.136.0）与两层架构基础全部就绪；v0.136.0 前补强项明确（D7 schedule() 分区过滤） | ✅ |

---

## 11. 源码链接

| 模块 | 路径 |
|------|------|
| ARINC 653 配置（TimeSlot/PartitionSchedule/PartitionState/default_schedule + MAX_SLOTS + ARINC1~ARINC3 单测） | [crates/kernel/microkernel/src/sched/arinc.rs](file:///E:/EnerOS/crates/kernel/microkernel/src/sched/arinc.rs) |
| 分区调度器（PartitionScheduler trait/ArincScheduler/tick/start_at/partition_time_remaining_at + PS1~PS6 单测） | [crates/kernel/microkernel/src/sched/partition_sched.rs](file:///E:/EnerOS/crates/kernel/microkernel/src/sched/partition_sched.rs) |
| 分区内存隔离与地址空间切换接口（PartitionId/switch_address_space 双形态 + SW1~SW2 单测；D4 头注） | [crates/kernel/microkernel/src/mem/partition.rs](file:///E:/EnerOS/crates/kernel/microkernel/src/mem/partition.rs) |
| 内核启动期接入（§3.9 psched_smoke/psched_timer_handler + ARINC BSS 静态 + PSCHED_* 观测原子量 + 四决策记录） | [crates/kernel/microkernel/src/main.rs](file:///E:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| host 集成测试 PSHT1~PSHT5（含 PSHT3 口径决策与 D7 gap 记录头注） | [crates/kernel/microkernel/tests/partition_sched_test.rs](file:///E:/EnerOS/crates/kernel/microkernel/tests/partition_sched_test.rs) |
| 一键冒烟脚本（八关键串断言，v0.135.0 起追加 `[psched] ok`） | [tests/boot_test.sh](file:///E:/EnerOS/tests/boot_test.sh) |
| QEMU 实测记录（v0.135.0 串口全文/八断言/切换序列对照/ticks_per_ms 排障/ELF 98,952B） | [docs/boot/qemu_run.md §14](file:///E:/EnerOS/docs/boot/qemu_run.md) |
| 调度模块骨架与公开导出（arinc/partition_sched 挂载 + re-export 口径） | [crates/kernel/microkernel/src/sched/mod.rs](file:///E:/EnerOS/crates/kernel/microkernel/src/sched/mod.rs) |
| 线程调度设计文档（v0.134.0，inner 线程层与衔接决策来源） | [docs/kernel/scheduler_design.md](./scheduler_design.md) |
| 中断管理设计文档（v0.133.0，CNTP 定时器与 IRQ 30 handler 覆盖来源） | [docs/kernel/interrupt_design.md](./interrupt_design.md) |
| 分区隔离设计文档（v0.132.0，分区根页表/ASID 机制来源） | [docs/kernel/partition_isolation.md](./partition_isolation.md) |
