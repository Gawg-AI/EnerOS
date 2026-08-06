# EnerOS v0.134.0 线程抽象与基本调度设计（TCB + 优先级抢占式调度器 + 上下文切换）

> 版本：v0.134.0
> 适用范围：EnerOS 自研内核线程调度（TCB 线程抽象 / 32 桶位图 + 定长环形就绪队列 / 优先级抢占 + tick 时间片轮转 / aarch64 上下文切换汇编与首启动帧；QEMU virt aarch64 目标机 `-M virt,gic-version=3` / host 测试）
> 蓝图依据：`蓝图/phase3.md` §v0.134.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> 配套文档：[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 中断管理，时钟中断来源）、[partition_isolation.md](./partition_isolation.md)（v0.132.0 分区内存隔离，PartitionId 类型来源）、[vmm_design.md](./vmm_design.md)（v0.131.0 四级页表与 VMM）、[../boot/qemu_run.md](../boot/qemu_run.md) §13（v0.134.0 QEMU 实测记录，首轮一次通过）

---

## 1. 概述

**核心一句话**：实现线程控制块（TCB）与优先级抢占式调度器，支持上下文切换，让内核能运行多个线程（蓝图 §1 原样口径）。

- **Phase 定位**：P3-C 中断调度 SMP 线**第 2 版**（v0.133.0 ~ v0.138.0）；v0.135.0 分区调度器的前提（蓝图 §1 / §5.5）。
- **业务价值**：多线程是 Agent Runtime 与 RTOS 组件运行的基础（蓝图 §1）。
- **出口关联**：自研内核"调度"功能完整性证据（蓝图 §1）。
- **ADR-0001 口径**：依据 ADR-0001，Phase 3 主路径为 seL4 深度定制（方案 A），seL4 已具备线程（TCB）与调度器，方案 A 下复用 seL4 调度，仅做分区调度策略适配；本实现属自研内核研究/fallback 线的功能完整性证据，非生产路径（蓝图 §1 裁剪口径原样）。
- **依赖关系**：前序版本 v0.133.0 时钟中断可用（蓝图 §2）；假设单核调度（SMP 在 v0.136.0，蓝图 §2）；阻塞口径：无法切换则阻塞 v0.135.0（蓝图 §2）。

交付物落位（仓库规范 §2.3.1 子系统归口，偏差 D1）：

| 交付物 | 路径 |
|--------|------|
| 调度常量配置（MAX_THREADS=256 / TIME_QUANTUM_MS=10 / PRIO_LEVELS=32 / STACK_SIZE=16KB，蓝图 §3 配置类） | [crates/kernel/microkernel/src/sched/config.rs](../../crates/kernel/microkernel/src/sched/config.rs) |
| TCB 抽象（Tid/Priority/ThreadState/SavedRegs/Tcb + Tcb::new + TCB1~TCB4 单测） | [crates/kernel/microkernel/src/sched/tcb.rs](../../crates/kernel/microkernel/src/sched/tcb.rs) |
| 调度器主体（ReadyRing/ReadyQueue + Scheduler trait 七方法 + SchedulerImpl 五字段 + tick + SCH1~SCH6 单测） | [crates/kernel/microkernel/src/sched/scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs) |
| 上下文切换（context_switch/init_stack_frame/thread_trampoline + 偏移锚定 const 断言；aarch64 文件级门控） | [crates/kernel/microkernel/src/sched/context.rs](../../crates/kernel/microkernel/src/sched/context.rs) |
| 模块骨架与公开导出（pub mod config/scheduler/tcb + cfg 门控 pub mod context） | [crates/kernel/microkernel/src/sched/mod.rs](../../crates/kernel/microkernel/src/sched/mod.rs) |
| 内核启动期接入（§3.8 调度冒烟段 sched_smoke/sched_yield/back_to_main/t1_entry/t2_entry/sched_idle/bind_stack + SCHED/MAIN_TCB BSS 静态） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试（scht1~scht7） | [crates/kernel/microkernel/tests/sched_test.rs](../../crates/kernel/microkernel/tests/sched_test.rs) |
| 设计文档（蓝图 §3 文档类 / §7.4 验收） | [docs/kernel/scheduler_design.md](./scheduler_design.md)（本文档） |

> **配置类口径**：蓝图 §3 配置类「MAX_THREADS=256, TIME_QUANTUM=10ms」——[config.rs](../../crates/kernel/microkernel/src/sched/config.rs) 四常量全部落位；`TIME_QUANTUM_MS` 单位为 tick，本内核 **1 tick = 1ms 口径**，即默认 10ms 运行配额（耗尽触发抢占重调度，见 §5）；`STACK_SIZE = 16KB`（4 个 4KB 页，PMM order 2 分配，见 §4.1）。

**类型与 trait 一览**（[mod.rs](../../crates/kernel/microkernel/src/sched/mod.rs) 公开导出，蓝图 §3 接口类原样签名）：

| 项 | 定义 | 说明 |
|----|------|------|
| `Tid` | `pub type Tid = u32` | 线程标识符（取值 0..MAX_THREADS，下标即线程表槽位） |
| `Priority` | `pub type Priority = u8` | 优先级（0~255，数值越大越优先；32 桶映射见 §2.1） |
| `ThreadState` | `enum { Ready, Running, Blocked, Suspended, Dead }` | 线程状态机五变体（蓝图 §3 原样；迁移图见 §3） |
| `SavedRegs` | `struct { x: [usize; 31], sp, pc, pstate }` | 调度上下文切换完整现场（272B；与 interrupt::SavedRegs 异型并存，偏差 D3） |
| `Tcb` | struct 十字段 | `tid / state / priority / partition / stack_ptr / entry / arg / saved_regs / quantum / time_used`（蓝图 §3 原样顺序；无 Default，Tcb::new 十字段显式构造，偏差 D9） |
| `Scheduler` | trait 七方法 | `create_thread` / `delete_thread` / `yield_now` / `schedule` / `set_priority` / `block_thread` / `unblock_thread`（蓝图 §3 原样签名；`schedule` 返回 `Option<Tid>`，偏差 D7） |

---

## 2. 调度算法

### 2.1 32 桶位图 + 定长环形队列（O(1) 就绪队列）

蓝图 §4.1 `ReadyQueue { queues: [VecDeque<Tid>; 32], bitmap: u32 }` 的 no_std 定长化（偏差 D2）：`ReadyQueue` 持 `slots: [ReadyRing; 32]` + `bitmap: u32`；`ReadyRing` 为定长环形（`buf: [Tid; MAX_THREADS]` + `head/tail/len`，下标取模回绕、len 区分空/满），每桶容量 256 与线程表上限一致（单线程同时在就绪队至多一次，正常路径不会满；满时 push 返回 false，调用方按注释口径忽略）。

- **入队 O(1)**：`push(tid, prio)` → 队尾写入、tail 回绕、len+1、位图置位 `bitmap |= 1 << bucket`；
- **找最高 O(1)**：`pop_highest()` → 位图 `leading_zeros` 定位最高非空桶（CLZ 口径，蓝图 §5.2）：`bucket = u32::BITS - 1 - bitmap.leading_zeros()`，弹该桶队首；桶弹空则清位 `bitmap &= !(1 << bucket)`；
- **同优先级 FIFO**：同桶内环形队首弹出，先创建/先重入队者先运行（SCH2 FIFO 全量回绕校验、SCH6/scht6 同优先级交替轮转）；
- **摘除 O(n)**：`remove(tid)` 跨桶线性扫描 + 桶内后继前移保持 FIFO 相对顺序，仅 set_priority/delete/block 等管理操作用，**不在调度热路径**。

**优先级 → 桶映射**：`bucket = prio / 8`（封顶 31；每桶覆盖连续 8 个优先级，prio 255 → 桶 31）：

| 优先级区间 | 桶号 | 位图位 |
|-----------|------|--------|
| 0 ~ 7 | 0 | bit 0 |
| 8 ~ 15 | 1 | bit 1 |
| … | … | … |
| 200 ~ 207 | 25 | bit 25 |
| 248 ~ 255 | 31 | bit 31 |

### 2.2 SchedulerImpl 五字段与 schedule 流程

`SchedulerImpl` 蓝图 §4.1 原样五字段（单核口径；SMP 在 v0.136.0）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `threads` | `[Option<Tcb>; MAX_THREADS]` | 线程表（下标即 Tid；None = 空槽可复用，find_free_tid 线性扫小 tid 优先） |
| `ready` | `ReadyQueue` | 就绪队列（§2.1） |
| `current` | `Tid` | 当前运行线程 |
| `idle` | `Tid` | idle 线程（无就绪线程时运行；蓝图 §8.5：不可阻塞/删除） |
| `tick` | `u64` | 累计时钟 tick 数 |

> **尺寸口径**：SchedulerImpl 内联 `[Option<Tcb>; 256]` + ReadyQueue ≈ **117KB**，严禁栈上整体构造（内核栈仅 64KB）——接入层经 `UnsafeCell<MaybeUninit<SchedulerImpl>>` 驻留 BSS 逐字段就地有效化（main.rs SchedCell，同 PmmCell 惯例）。

`schedule()` 流程（偏差 D5：host 仅更新内部态与返回 Tid，寄存器保存/恢复属 context.rs aarch64 职责）：

1. `ready.pop_highest()`——空队 → 返回 `None`（偏差 D7：None = 无就绪线程，调用方以 idle 兜底）；
2. 弹出 `next` 且 `current != next`：当前线程仍 Running → Running→Ready 并按原优先级**重入队尾**（蓝图 §4.5 save_current 口径的队列侧主体）；
3. `current = next`，`next` 置 Running，返回 `Some(next)`。

**idle 兜底语义**：idle 创建即入就绪队（prio 0 桶 0），无用户线程时经就绪队**自然选中**（SCH4/scht4：阻塞唯一工作线程后 schedule → `Some(idle)`）；仅当 idle 也在 Running（就绪队真空）时返回 `None`，current 保持 idle Running，由调用方继续以 idle 兜底（scht4 锚定该语义）。

### 2.3 复杂度与选型

| 操作 | 复杂度 | 说明 |
|------|--------|------|
| push / pop_highest | O(1) | 环形队尾/队首 + 位图 CLZ |
| schedule / yield_now / tick | O(1) | 热路径全 O(1)，无线性扫 |
| remove（管理操作） | O(桶数×桶长) | 仅 set_priority/delete/block 用，非热路径 |

蓝图 §5.1 选型对比（原样口径）：优先级抢占实时性高、确定性高 → 选优先级抢占（vs 时间片轮转实时性低 / CFS 确定性低）；同优先级 FIFO 轮转由 tick 时间片补足公平性（§5）。

---

## 3. TCB 状态机

### 3.1 五态迁移图

```
                    create_thread（占空槽 + 置 Ready + 入就绪队尾）
                         │
                         ▼
   unblock_thread   ┌─────────┐   schedule() 弹最高桶队首
  ┌─────────────────│  Ready  │──────────────────┐
  │ （Blocked→Ready  └────▲────┘                  │
  │   重入就绪队）        │                      ▼
  │                     │                ┌──────────┐
  │   yield_now() /     │                │ Running  │
  │   tick() 耗尽        │                └────┬─────┘
  │   （Running→Ready    │                     │ block_thread（阻塞当前线程，
  │    重入队尾）        │                     │  调用方随后 schedule 切换）
  │                     │                     ▼
  │                 ┌──────────┐        ┌──────────┐
  └─────────────────┤（重入队）│        │ Blocked  │
                    └──────────┘        └──────────┘

  delete_thread：Ready/Running/Blocked → Dead（槽位置 None 可复用；
    idle 拒绝 Err(PermissionDenied)；Ready 态先摘除出队）
  Suspended：蓝图 §3 原样五变体保留，本版本无 API 产生迁移边（预留）
```

### 3.2 触发边表

| 触发 | 迁移 | 实现点（[scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs)） |
|------|------|------|
| `create_thread(entry, arg, prio, pid)` | （空槽）→ Ready，入对应桶队尾 | `find_free_tid` → 满表 `Err(OutOfMemory)`（蓝图 §4.4）；`Tcb::new(...)` 十字段 + `ready.push`；stack_ptr=0 未绑栈（偏差 D6） |
| `schedule()` | 弹出桶首 Ready→Running；原 Running→Ready 重入队尾 | 见 §2.2 流程 |
| `yield_now()` | Running→Ready 重入队尾 + schedule | 当前线程非 Running（如已被 tick 重排）则只 schedule |
| `tick()` 耗尽 | Running→Ready + quantum 重置 + 重入队 + schedule | 见 §5（idle 豁免） |
| `block_thread(tid)` | Ready→Blocked（摘除出队）/ Running→Blocked | idle 拒绝（蓝图 §8.5）；空槽忽略 |
| `unblock_thread(tid)` | Blocked→Ready 重入就绪队 | 其他状态无可唤醒，按注释口径忽略 |
| `delete_thread(tid)` | → Dead，槽位置 None 复用 | idle 拒绝 `Err(PermissionDenied)`（蓝图 §8.5）；空槽 `Err(InvalidArgument)`；栈不回收（偏差 D6） |
| `set_priority(tid, prio)` | 状态不变；Ready 态先摘除再以新优先级重入对应桶 | 空槽 `Err(InvalidArgument)`；桶号随优先级变化 |
| —（无 API） | Suspended 无迁移边 | 蓝图 §3 原样变体预留，本版本最小实现 |

---

## 4. 上下文切换

> [context.rs](../../crates/kernel/microkernel/src/sched/context.rs) 全模块文件级 `#![cfg(target_arch = "aarch64")]` 门控（偏差 D10）：host 编译为空模块（host 行为不变，同 interrupt/vector.rs 门控惯例）；aarch64 真机承担切换汇编与首启动帧构造。

### 4.1 初始帧布局（init_stack_frame 写入，SavedRegs 镜像 272B = 17×16B）

`init_stack_frame(stack_top, entry, arg)` 在新线程内核栈顶（16KB = STACK_SIZE，PMM order 2 分配，页分配天然 16B 对齐）向下落一帧，帧底 = `(stack_top - 272) & !0xF`，返回帧底 sp：

| 帧内偏移 | 槽位 | 初值 | 消费方 |
|---------:|------|------|--------|
| 0 | x[0] | arg | trampoline 弹入 x0（entry 首参） |
| 8..=232 | x[1]..=x[29] | 0 | trampoline 弹入 x1~x29 |
| 240 | x[30] | 0 | trampoline 弹入 x30（entry 不可返回，偏差 D14） |
| 248 | sp | 0（不读） | ——（弹帧后 sp = 帧底 + 272 ≈ 栈顶） |
| 256 | pc | entry | trampoline → ELR_EL1 |
| 264 | pstate | 0x345 | trampoline → SPSR_EL1（逐位表见 §4.3） |

### 4.2 context_switch 保存区偏移对照表（TCB 基址偏移，.equ 硬编码）

`context_switch(old: *mut Tcb, new: *const Tcb)`（x0 = old，x1 = new）：保存 old 的 AAPCS64 callee-saved 现场（x19~x30 + sp，偏差 D11），从 new 对称恢复后 `ret`（x30 = 新线程上次让出点；首启动线程 = thread_trampoline 地址）：

| TCB 偏移 | .equ 常量 | 槽位 | 汇编指令 |
|---------:|-----------|------|---------|
| 152 | `TCB_X19_20` | saved_regs.x[19]/x[20] | `stp/ldp x19, x20` |
| 168 | `TCB_X21_22` | saved_regs.x[21]/x[22] | `stp/ldp x21, x22` |
| 184 | `TCB_X23_24` | saved_regs.x[23]/x[24] | `stp/ldp x23, x24` |
| 200 | `TCB_X25_26` | saved_regs.x[25]/x[26] | `stp/ldp x25, x26` |
| 216 | `TCB_X27_28` | saved_regs.x[27]/x[28] | `stp/ldp x27, x28` |
| 232 | `TCB_X29_30` | saved_regs.x[29]/x[30] | `stp/ldp x29, x30` |
| 248 | `TCB_SP` | saved_regs.sp | `mov x9, sp; str x9` / `ldr x9; mov sp, x9` |
| — | — | — | `ret`（跳新线程 x30 槽位） |

**offset_of 锚定机制（偏差 D13，关键发现）**：Tcb 非 repr(C)，rustc repr(Rust) 按 (align, size) 降序重排字段——**272B 的 `saved_regs` 被提为 `Tcb` 首字段，`offset_of!(Tcb, saved_regs) == 0`**，故 TCB 基址偏移 = SavedRegs 帧内偏移（x[i] 槽位 = i*8，sp = 248，pc = 256，pstate = 264）。global_asm 不插值 Rust 常量，asm 内 `.equ` 硬编码偏移，两侧锚定：context.rs 编译期 const 断言（aarch64 核对 `offset_of!` 实测值与尺寸 272）+ host 单测 TCB4（host 与 aarch64 usize 均 8 字节、同一 rustc 布局算法，断言等价有效）。数值漂移 = 汇编读错槽位，必须同步改 .equ 与两侧断言。

**保存集口径（偏差 D11）**：x0~x18 为 AAPCS64 调用者保存，跨 extern "C" 调用已被编译器视为可破坏，无需入帧；让出点 pc 以 x30 槽位承载（LR 即返回地址）；协作式切换前后 pstate 不变不入保存集（pstate 槽位仅供首启动帧经 trampoline eret 消费）；x18 平台保留寄存器不入帧；FP/SIMD 不保存（内核线程无浮点，同 vector.rs D11 口径）。

### 4.3 pstate = 0x345 逐位表（SPSR_EL1 口径，0x3C5 清零 I 位所得）

| 位 | 值 | 含义 |
|----|----|------|
| M[3:0]（bits 0~3） | 0b0101 | EL1h（EL1 + SP_EL1，与本内核 SPSel=1 恒用 SP_EL1 口径一致） |
| M[4]（bit 4） | 0 | AArch64 执行态 |
| F（bit 6） | 1 | FIQ 屏蔽（本内核未用 FIQ，GIC 仅配 Group 1 IRQ——同 vector.rs D12 口径） |
| I（bit 7） | 0 | **IRQ 打开**——新线程出生即可被定时器抢占，与主流程 daifclr 开中断口径、vector.rs irq_entry eret 返回路径（硬件恢复的 SPSR 同为 I=0 现场）兼容 |
| A（bit 8） | 1 | SError 屏蔽 |
| D（bit 9） | 1 | Debug 屏蔽 |
| 其余 | 0 | N/Z/C/V=0、PAN/UAO/SSBS/BTYPE/DIT/TCO=0 |

### 4.4 thread_trampoline 首入口链路（偏差 D12）

选「独立 trampoline + 弹帧 eret」（蓝图 §5.4「像中断返回一样启动」口径）：init 帧为 SavedRegs 镜像落新栈，trampoline 与 vector.rs `irq_entry` 返回尾段同构（ldp 弹帧 + eret），两套帧布局同一结构体、偏移互相锚定。完整链路（接入层契约，main.rs `bind_stack` 实现）：

```
接入层（main.rs bind_stack）             新线程内核栈（16KB，PMM order 2）
──────────────────────────────          ─────────────────────────────
init_stack_frame(stack_top, entry, arg) ──► 栈顶向下落 SavedRegs 镜像帧（272B）：
                                            [x0=arg][x1~x30=0][sp=0][pc=entry][pstate=0x345]
tcb.stack_ptr        = frame_sp（帧底）
tcb.saved_regs.sp    = frame_sp
tcb.saved_regs.x[30] = &thread_trampoline（ret 着陆点，仅取址禁调用）
        │
        ▼ schedule() 选中（prio 10 桶 FIFO）
context_switch(old → new)
  保存 old：stp x19~x30（6 对）→ old+152..232；str sp → old+248
  恢复 new：ldp x19~x30（6 对）← new+152..232；ldr sp ← new+248（= 帧底）
  ret ──► x30 = thread_trampoline
        ▼
thread_trampoline（进入时 sp = 帧底）
  ldr x9, [sp,#256] → msr elr_el1   （pc = entry）
  ldr x9, [sp,#264] → msr spsr_el1  （pstate = 0x345）
  ldp x0~x29（15 对）+ ldr x30      ← 对称弹帧
  add sp, #272                      （sp ≈ 栈顶）
  eret ──► entry(arg)：EL1h、IRQ 开（0x345）
```

**entry 不可返回（偏差 D14）**：帧 x[30] 槽位 = 0，entry 若 ret 将跳转到 0 触发 Sync 异常，经 vector.rs `sync_handler` 诊断打印后挂起；线程退出语义（Dead 态回收栈与槽位）属后续版本范畴，本版最小实现不引入 exit 桩（冒烟线程主体均为无限循环）。

---

## 5. tick 时间片轮转

蓝图 §4.5 原样口径（[scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs) `tick()`）：

```
tick+1 → 当前线程 quantum 递减（>0 才减）
       → quantum == 0 且 current != idle（idle 豁免，蓝图 §8.5）
       → quantum 重置 TIME_QUANTUM_MS(10) + Running→Ready + 按原优先级重入队尾
       → schedule() 重调度（同优先级轮转：对方先出队运行，本线程排至队尾）
```

- **配额口径**：`TIME_QUANTUM_MS = 10`，单位 tick，**1 tick = 1ms 口径** → 默认 10ms 运行配额（config.rs 头注）；耗尽触发抢占重调度；
- **idle 豁免**：`current != idle` 才做耗尽重排（idle 永不因 tick 被换出/重置）；
- **time_used 统计**：字段就位（蓝图 §9.6 可观测，create 时初值 0），累计逻辑随 tick 驱动接入版本落地（本版本 tick 内未累计）；
- **本版本驱动口径声明（偏差 D8）**：QEMU 冒烟用**协作式 yield**（线程内显式 `yield_now` + `context_switch` 链式轮转，见 §6），`tick()` 逻辑经 host SCH6/scht6 完整覆盖（10 tick 耗尽轮转 + quantum 重置 + 交替）；**tick 驱动抢占 + 关中断临界区接入留待 v0.135.0**（蓝图 §8.5 坑点：调度器临界区必须关中断，否则数据竞争）。

---

## 6. 与 v0.133.0 中断衔接

- **未来路径（时钟中断为 tick 源）**：v0.133.0 已验证 CNTP 定时器中断（PPI 30）经完整硬件路径到达（[interrupt_design.md](./interrupt_design.md) §6 时序图）。tick 驱动抢占的接入路径为：重启 CNTP（周期编程）→ timer handler 内 `sched.tick()` → 耗尽重排队 + schedule → 中断返回路径完成切换；接入前提是调度器临界区关中断（蓝图 §8.5 坑点）与 GIC/向量表（v0.133.0 已就绪）。**pstate=0x345 I=0 预留**：新线程出生即可被定时器抢占，与 `irq_entry` eret 返回（硬件恢复 SPSR I=0）兼容，tick 驱动接入无需改首启动帧。
- **当前冒烟决策（偏差 D8，main.rs `sched_smoke` 文档注释记录）**：timer 中断段已停 CNTP（`timer_handler` CTL=0，电平触发须撤销断言），调度冒烟段**不重启定时器**；线程内显式 yield_now + context_switch 链式轮转（主→t1→t2→…→主），不依赖中断重入调度器、无临界区竞争（蓝图 §8.5 坑点正面规避）；单核启动期任一时刻仅一个执行流运行，无并发（SchedCell Sync SAFETY 论证）；tick 驱动抢占 + 关中断临界区接入留待 v0.135.0。

冒烟链路（main.rs §3.8，`[irq] ok` 之后进入）：

```
① 调度器 BSS 句柄就地有效化（threads 256 槽逐位写 None；ready 全零即初值；≈117KB 严禁栈构造）
② create_thread × 3：sched_idle（tid 0，prio 0）/ t1 / t2（prio 10）
③ PMM alloc_page(2) 各分 16KB 栈 → bind_stack：init_stack_frame 落 272B 首启动帧
   + saved_regs 回填（sp=帧底、x[30]=thread_trampoline）
④ schedule() 选出 t1（prio 10 桶 FIFO 先于 t2；idle 桶 0 恒不被选中）
   → 首次 context_switch(MAIN_TCB → t1)：主流程 callee-saved 现场存入 MAIN_TCB
t1 经 trampoline 弹帧 eret → t1_entry：T1_HITS+1，首圈打印 [sched] t1
   → sched_yield()：yield_now 重入队尾 + context_switch(t1 → t2)
t2 对称：T2_HITS+1，首圈打印 [sched] t2 → 让出轮转回 t1
双计数均达 SCHED_ROUNDS(3)（或单边超 SCHED_GUARD(64) 兜底）→ back_to_main()：
   context_switch(当前 → MAIN_TCB)，主流程自首次切换点续跑
主流程检出 T1_HITS>0 && T2_HITS>0 → 打印 [sched] ok
```

失败分支（本次实测均未触发）：`[sched] err create/stack/bind/dispatch/hits 0x<err>`。

---

## 7. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`（lib 内嵌单测 + tests/ 集成测试；host 上可使用 std）。

### 7.1 单元测试（tcb.rs 内嵌，TCB1~TCB4）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| TCB1 | `tcb1_thread_state_five_states_semantics` | 五态同态相等、异态不等、五变体两两互异；Debug 含变体名 | PASS |
| TCB2 | `tcb2_saved_regs_default_all_zero` | SavedRegs::default() 全零（x 全数组/sp/pc/pstate），两次 default 相等 | PASS |
| TCB3 | `tcb3_tcb_new_ten_fields_in_place` | Tcb::new 十字段构造逐字段断言（含 partition/quantum/time_used） | PASS |
| TCB4 | `tcb4_layout_offsets_anchor_context_asm` | 布局偏移锚点：size 272、x@0/sp@248/pc@256/pstate@264、**Tcb.saved_regs@0**（repr(Rust) 重排首字段）、stp 对址 152~248——与 context.rs .equ 硬编码一致 | PASS |

### 7.2 单元测试（scheduler.rs 内嵌，SCH1~SCH6）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| SCH1 | `sch1_ready_queue_bitmap_highest_first` | 位图查找：prio 8（桶 1）/200（桶 25）→ 高桶先出；桶弹空清位；空队 None；prio 255 封顶桶 31 | PASS |
| SCH2 | `sch2_ready_ring_boundaries_and_fifo` | ReadyRing 边界：空 pop None；满 256 拒第 257；FIFO 全量回绕顺序；remove 中间/队首/队尾后顺序保持 | PASS |
| SCH3 | `sch3_create_thread_table_full_out_of_memory` | create 占满 256 槽（new 已占 idle 首槽）→ 第 257 线程 Err(OutOfMemory) 且状态不破坏 | PASS |
| SCH4 | `sch4_preemption_highest_priority_wins` | 抢占：prio10 A + prio20 B → 选 B；block B → 选 A；无用户线程时 idle 兜底 | PASS |
| SCH5 | `sch5_block_unblock_and_set_priority` | block 摘除出队/unblock 重入可被选；set_priority 5→30 提升后下一轮选中；空槽 tid Err(InvalidArgument) | PASS |
| SCH6 | `sch6_tick_quantum_exhaustion_round_robin` | tick 耗尽重排：10 tick 后轮转、quantum 重置 10、原线程 Ready 重入队；再 10 tick 交替回 | PASS |

### 7.3 集成测试（tests/sched_test.rs，scht1~scht7，仅以公开 API 端到端验证）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| scht1 | `scht1_priority_preemption_order` | 三线程创建 + 抢占顺序：A(10)/B(30)/C(20) → 选 B；新建 D(25) 并 block B → D(25)>C(20)>A(10) 选中 | PASS |
| scht2 | `scht2_yield_now_same_priority_round_robin` | yield_now 同优先级轮转：T1 让出重入队尾 → T2 运行 → 再让出回 T1 | PASS |
| scht3 | `scht3_set_priority_dynamic_boost_preempts` | set_priority 动态提升：A 从 5 提到 40 → 抢占 B(10) | PASS |
| scht4 | `scht4_block_last_worker_idle_fallback_then_none` | block 后 idle 兜底 + 空队 None 语义：阻塞唯一工作线程 → 选 idle；再 schedule 真空 → None，current 保持 idle Running | PASS |
| scht5 | `scht5_delete_frees_slot_for_reuse` | delete 槽位复用：删 tid 1 → 再 create 复用同一 tid（小 tid 优先）；delete idle → Err(PermissionDenied) | PASS |
| scht6 | `scht6_tick_quantum_exhaustion_rotates` | tick 耗尽轮转：TIME_QUANTUM_MS(10) 次 tick 后重排（重置 + 重入队 + 切换）；再 10 tick 轮转回 | PASS |
| scht7 | `scht7_context_switch_perf_smoke` | 切换性能 smoke：10_000 次 yield_now（含内部 schedule 重调度）计时，eprintln 实测均值，断言 < 5μs 宽松上限防算法退化 | PASS |

> **scht7 口径声明**：实测均值 **≈5 ns/op**（10_000 次 yield+schedule 平均，eprintln 输出）——host 侧（Windows/Linux 用户态）smoke 口径，**不代表目标机（ARM64 裸机）性能**（同 v0.131.0 T07 map < 5μs / v0.133.0 偏差 D6 口径惯例）；蓝图 §6.3/§7.2「切换 < 5μs」按本口径声明，目标机性能测定属后续版本。

### 7.4 QEMU 冒烟（main.rs §3.8 段 + [qemu_run.md](../boot/qemu_run.md) §13）

冒烟流程见 §6 链路图。串口输出关键行（原样收录，WSL2 QEMU 8.2.2，`-M virt,gic-version=3`）：

```text
[part] iso ok
[irq] timer
[irq] ok
[sched] t1
[sched] t2
[sched] ok
```

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **七**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok`；v0.134.0 起追加第七条断言 `grep "[sched] ok"`，既有六条原样） |
| 端到端总耗时 | 13s（构建 3.21s + QEMU timeout 10s 守护窗口） |
| 排障 | 无——**首轮一次通过**，七断言全过。四项高风险点（context_switch 首入口帧布局契约 / 线程栈 PMM 帧映射 / pstate 0x345 eret / yield 链路死循环）均未触发；TCB4 + context.rs const 断言双侧锚定已提前排除布局类风险 |
| 回归 | host **116** 测试全过零回归（99 基线 + TCB4 + SCH6 + scht7）；`cargo fmt` / `cargo clippy -D warnings` 干净 |

蓝图 §6 其余条目对照：

| 蓝图条目 | 覆盖情况 |
|---------|---------|
| §6.1 单元测试「就绪队列 push/pop、优先级抢占逻辑」 | SCH1/SCH2（队列边界）+ SCH4（抢占）+ scht1 |
| §6.2 集成测试「创建 3 个线程验证轮流执行、高优先级抢占低优先级」 | scht1（四线程抢占顺序）/ scht2（轮转）+ QEMU 冒烟 idle+t1+t2 三线程协作式轮转（`[sched] t1/t2/ok`） |
| §6.3 性能基准「上下文切换 < 5μs」 | scht7 host smoke 实测 ≈5 ns/op（口径声明见 §7.3 注；目标机测定属后续版本） |
| §6.4 回归「中断功能不受影响」 | QEMU 冒烟 `[irq] ok` 先于 `[sched] ok` 通过；host 116 测试零回归 |
| §6.5 故障注入「线程栈溢出测试」 | 本版本未做专项注入；防御口径：SCHED_GUARD=64 兜底防静默死循环、`[sched] err` 五类分支逐类报错（实测均未触发）、线程栈由 PMM order 2 独占分配 |
| §6.6 GPU 规则 | 不适用（线程调度，蓝图原样口径） |

---

## 8. 蓝图偏差表（D1~D14）

> 文档级编号与源码头注模块内编号的对应关系（各模块头注编号模块内独立）：**scheduler.rs 头注 D2/D4/D5/D6 与本文档同号**；scheduler.rs 头注「返回类型」未编号记录 → 本文档 D7；**tcb.rs 头注 D1 → 本文档 D3**、tcb.rs 头注 D2 → 本文档 D9（tcb.rs D2 与 scheduler.rs D2 为不同条目）；**context.rs 头注 D1~D5 → 本文档 D10~D14**；D1/D8 为文档级记录（D1 路径归口同 v0.133.0 惯例；D8 见 main.rs `sched_smoke` 文档注释决策记录）。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D1 | 蓝图交付物 `kernel/src/sched/{tcb,scheduler,context,config}.rs`、`kernel/tests/sched_test.rs` → 落位 `crates/kernel/microkernel/src/sched/` 与 `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/scheduler_design.md` 同蓝图 | 仓库规范 §2.3.1 强制子系统归口（`crates/<subsystem>/`），禁止根目录 crate | 仅路径差异，交付物一一对应 |
| D2 | 蓝图 §4.1 `ReadyQueue.queues: [VecDeque<Tid>; 32]` → no_std 无堆改定长环形 `ReadyRing`（`[Tid; MAX_THREADS]` + head/tail/len）+ u32 位图；每桶容量 256 与线程表上限一致（单线程同时在队至多一次），正常路径不会满，满时 push 返回 false（调用方按注释口径忽略）（= scheduler.rs 头注 D2） | no_std 无堆（仓库规范 §4.3 硬性要求），VecDeque 不可用；容量编译期确定、无分配失败路径 | 入队 O(1)/找最高 O(1)（CLZ）保持蓝图 §5.2 口径；SCH2 覆盖满/空/回绕边界；remove O(n) 仅管理操作非热路径 |
| D3 | `sched::SavedRegs` 与 `interrupt::SavedRegs` 异型并存——interrupt 版为异常现场占位（31 通用寄存器 + sp，u64 数组，由异常入口汇编填充）；本版为调度上下文切换完整现场（31 通用寄存器 + sp + pc + pstate，usize 标量字段，由 context 切换汇编保存/恢复）（= tcb.rs 头注 D1） | 两者语义不同、填充路径不同，不合并且互不引用；后续版本若收敛以集成层决策为准 | interrupt::SavedRegs 保持 v0.133.0 原样零改动；sched 版独立演进（272B 帧布局由 TCB4 + context.rs const 断言锚定） |
| D4 | 蓝图 §4.5 `new()` 内 `create_thread(...).unwrap()` → 本实现 let-else 短路而不 panic（仓库禁 unwrap/panic 规则）：threads 表刚清零时 tid=0 槽位必然空闲，idle 创建不可能失败；else 分支返回仅 idle=0 占位的空表（= scheduler.rs 头注 D4） | 仓库禁 panic/unwrap 硬性规则；空表首槽必空闲，else 分支不可达 | 语义与蓝图等价；全部 host 用例均经 `new()` 构造验证 |
| D5 | host `schedule()` 仅更新内部态与返回 Tid——寄存器保存/恢复（蓝图 §4.5 save_current/restore_current 主体）属 context.rs aarch64 汇编职责，host 无可保存现场（= scheduler.rs 头注 D5；sched_test.rs 头注口径背书） | host 无 aarch64 寄存器现场可切换；context.rs 文件级门控 host 不可见（D10） | host 测试验证调度决策逻辑（选谁）；切换机制（怎么切）由 QEMU 冒烟实测（t1/t2 三圈轮转）验证 |
| D6 | `create_thread` 时 `stack_ptr=0` 表示尚未绑定栈——栈分配与 init_stack_frame 属接入层/context.rs 职责（栈由 main.rs 接入时经 PMM 分配并回填 stack_ptr + saved_regs）；`delete_thread` 不回收 PMM（栈回收留待接入层/v0.135.0，256 槽位内复用栈）（= scheduler.rs 头注 D6） | lib 不依赖 PMM 保持 host 可测性；接入层独占持有 pmm &mut | 蓝图 §4.5 create 内 alloc_stack 拆分为 lib 占位 + 接入层绑定两步（main.rs `bind_stack`：PMM alloc_page(2)=16KB → init_stack_frame → 回填）；scht5 验证槽位复用不含栈语义 |
| D7 | 蓝图 §4.5 `schedule() -> Tid`（空队返 idle）与 §3 trait `schedule() -> Option<Tid>` 不一致 → **以 trait 签名为准**（None = 无就绪线程），idle 兜底由调用方处理（= scheduler.rs 头注「返回类型」记录） | trait 为公开接口契约；idle 创建即入就绪队（prio 0 桶 0），无用户线程时经就绪队自然选中；None 仅在 idle 也 Running（就绪队真空）时出现 | scht4 锚定语义：阻塞唯一工作线程 → `Some(idle)` 兜底；再 schedule 真空 → None，current 保持 idle Running |
| D8 | 冒烟驱动口径选**协作式 yield**（备选：重启 CNTP + timer handler 内 `sched.tick()`）——timer 中断段已停 CNTP（timer_handler CTL=0），调度冒烟段不重启定时器；线程内显式 yield_now + context_switch 链式轮转，不依赖中断重入调度器、无临界区竞争（蓝图 §8.5 坑点规避）；**tick 驱动抢占 + 关中断临界区接入留待 v0.135.0** | main.rs `sched_smoke` 文档注释决策记录；单核启动期任一时刻仅一个执行流运行，无并发（SchedCell Sync SAFETY 论证） | `tick()` 逻辑经 SCH6/scht6 host 完整覆盖；pstate=0x345 I=0 已为 tick 驱动接入预留（§6）；v0.135.0 前置工作明确 |
| D9 | `Tcb` 不提供 `Default`——`PartitionId` 无 Default 实现（其取值即分区槽位下标，默认槽位无语义），构造统一走 `Tcb::new` 十字段显式入参（= tcb.rs 头注 D2） | 默认分区槽位无语义，隐式默认掩盖归属错误 | 蓝图 §3 未要求 Tcb: Default；`SavedRegs` 保留 Default（全零现场有语义，TCB2 覆盖） |
| D10 | context.rs 全模块文件级 `#![cfg(target_arch = "aarch64")]` 门控——host 编译时本文件仅含注释/文档（空模块，host 行为不变），与 interrupt/vector.rs 文件级门控同口径（= context.rs 头注 D1） | 汇编仅 aarch64 可汇编；host 无可保存现场（D5） | host 测试零感知；aarch64 交叉编译承担汇编正确性；mod.rs 以 `#[cfg(target_arch = "aarch64")] pub mod context` 挂载 |
| D11 | context_switch 只保存 callee-saved x19~x30 + sp——x0~x18 为 AAPCS64 调用者保存，跨 extern "C" 调用已被编译器视为可破坏，无需入帧；让出点 pc 以 x30 槽位承载（LR 即返回地址）；协作式切换前后 pstate 不变不入保存集（pstate 槽位仅供首启动帧经 trampoline eret 消费）；x18 平台保留不入帧；FP/SIMD 不保存（内核线程无浮点，同 vector.rs D11）（= context.rs 头注 D2） | AAPCS64 调用约定 + 最小正确实现 | 保存/恢复各 6 对 stp/ldp + sp 单 str/ldr + ret；272B 完整帧仅首启动经 trampoline 消费（§4.1/§4.2 对照表） |
| D12 | 首启动选「独立 trampoline + 弹帧 eret」（蓝图 §5.4「像中断返回一样启动」口径）——init 帧为 SavedRegs 镜像落新栈，trampoline 与 vector.rs `irq_entry` 返回尾段同构（ldp 弹帧 + eret），两套帧布局同一结构体、偏移互相锚定（= context.rs 头注 D3） | 与既有中断返回路径同构降低心智负担；单一 SavedRegs 布局贯穿切换与首启动 | 首次进入链路 6 步契约（§4.4）；`thread_trampoline` 仅取址写入 x[30]，禁止调用/解引用（同 vector.rs exception_vectors 取址口径） |
| D13 | global_asm 不插值 Rust 常量，asm 内 `.equ` 硬编码偏移，两侧锚定——context.rs 编译期 const 断言（aarch64 核对 `offset_of!` 实测值与尺寸 272）+ host 单测 TCB4（host 与 aarch64 usize 均 8 字节、同一 rustc 布局算法，断言等价有效）；**关键发现**：repr(Rust) 按 (align, size) 降序重排字段，272B 的 `saved_regs` 提为 Tcb 首字段，`offset_of!(Tcb, saved_regs) == 0`，故 TCB 基址偏移 = SavedRegs 帧内偏移（= context.rs 头注 D4） | 布局漂移 = 汇编读错槽位，必须编译期/测试期强制一致 | 数值漂移触发编译错误或 TCB4 失败；改 .equ 须同步两侧断言（§4.2 对照表即锚定清单） |
| D14 | entry 不可返回——帧 x[30] 槽位 = 0，entry 若 ret 将跳转到 0 触发 Sync 异常，经 vector.rs `sync_handler` 诊断打印后挂起；线程退出语义（Dead 态回收栈与槽位）属后续 Task/版本范畴，本版最小实现不引入 exit 桩（= context.rs 头注 D5） | 蓝图未要求本版本线程退出路径；最小实现 | 冒烟线程主体均为无限循环（t1/t2/sched_idle）；`back_to_main` 切出后防御性 wfe 停转；回收属 v0.135.0 |

---

## 9. 蓝图 §7 验收对照

| 蓝图 §7 验收标准 | 实现证据 | 结论 |
|-----------------|---------|------|
| §7.1 功能：多线程切换正确 | QEMU 冒烟 idle+t1+t2 三线程协作式轮转实测：t1/t2 双计数各 3 圈（SCHED_ROUNDS）交替执行，`[sched] t1`/`[sched] t2`/`[sched] ok` 首轮一次通过；host scht2（yield 轮转）/ scht6（tick 耗尽轮转）/ SCH4（抢占选优）全绿 | ✅ |
| §7.2 性能量化：切换 < 5μs | scht7 性能 smoke：10_000 次 yield+schedule 实测均值 ≈5 ns/op << 5μs 上限；**host 用户态口径声明**（不代表目标机 ARM64 裸机性能，同 v0.131.0 T07 / v0.133.0 D6 口径惯例），目标机性能测定属后续版本 | ✅（host smoke + 口径声明） |
| §7.3 安全：线程不能访问其他分区内存 | `Tcb.partition: PartitionId` 字段就位（蓝图 §3 原样），`create_thread` 显式入参绑定分区（scht 系列均以 `PartitionId(0)` 构造验证）；v0.132.0 分区隔离机制（独立根页表 + ASID + check_access）已就位——**按分区切换页表的隔离执行属 v0.135.0 分区调度器范畴**（本版本单内核地址空间，字段就位口径） | ✅（字段就位；隔离执行 v0.135.0） |
| §7.4 文档：scheduler_design.md | 本文档（[docs/kernel/scheduler_design.md](./scheduler_design.md)） | ✅ |
| §7.5 出口判定：多线程切换成功 → v0.135.0 可启动 | 七关键串断言 PASS（含 `[sched] ok`）+ host 116 测试全绿零回归；线程基础（TCB/就绪队列/切换帧/yield 链路）全部就绪，tick 驱动抢占接入路径与前提明确（§5/§6、偏差 D8） | ✅ |

---

## 10. 源码链接

| 模块 | 路径 |
|------|------|
| 调度模块骨架与公开导出（ReadyQueue/Scheduler/SchedulerImpl + Priority/SavedRegs/Tcb/ThreadState/Tid） | [crates/kernel/microkernel/src/sched/mod.rs](../../crates/kernel/microkernel/src/sched/mod.rs) |
| 调度常量配置（MAX_THREADS/TIME_QUANTUM_MS/PRIO_LEVELS/STACK_SIZE） | [crates/kernel/microkernel/src/sched/config.rs](../../crates/kernel/microkernel/src/sched/config.rs) |
| TCB 抽象（Tid/Priority/ThreadState/SavedRegs/Tcb + Tcb::new + TCB1~TCB4 单测） | [crates/kernel/microkernel/src/sched/tcb.rs](../../crates/kernel/microkernel/src/sched/tcb.rs) |
| 调度器主体（ReadyRing/ReadyQueue/Scheduler trait/SchedulerImpl/tick/idle_loop + SCH1~SCH6 单测） | [crates/kernel/microkernel/src/sched/scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs) |
| 上下文切换（context_switch/init_stack_frame/thread_trampoline + 偏移锚定 const 断言；aarch64 门控） | [crates/kernel/microkernel/src/sched/context.rs](../../crates/kernel/microkernel/src/sched/context.rs) |
| 内核启动期接入（§3.8 调度冒烟段 + SCHED/MAIN_TCB BSS 静态 + bind_stack/sched_yield/back_to_main/t1/t2/sched_idle） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试 scht1~scht7 | [crates/kernel/microkernel/tests/sched_test.rs](../../crates/kernel/microkernel/tests/sched_test.rs) |
| 内核统一错误码 KernelError（OutOfMemory/PermissionDenied/InvalidArgument） | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs) |
| QEMU 实测记录（v0.134.0 串口全文/七断言/首轮 PASS） | [docs/boot/qemu_run.md](../boot/qemu_run.md) §13 |
| 中断管理设计文档（v0.133.0，时钟中断与向量表来源） | [docs/kernel/interrupt_design.md](./interrupt_design.md) |
| 分区隔离设计文档（v0.132.0，PartitionId 与分区机制来源） | [docs/kernel/partition_isolation.md](./partition_isolation.md) |
| VMM 设计文档（v0.131.0，页表机制设计） | [docs/kernel/vmm_design.md](./vmm_design.md) |
