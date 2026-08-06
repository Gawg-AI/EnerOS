# EnerOS v0.136.0 SMP 支持设计（PSCI 多核启动 + GICv3 SGI 核间中断 + per-CPU 数据 + 绑核调度）

> 版本：v0.136.0
> 适用范围：EnerOS 自研内核 SMP（ARM64 PSCI CPU_ON 多核启动 / GICv3 SGI0 核间中断 IPI / per-CPU 数据（TPIDR_EL1）/ 绑核调度过滤；QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` / host 测试）
> 蓝图依据：`蓝图/phase3.md` §v0.136.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> 配套文档：[scheduler_design.md](./scheduler_design.md)（v0.134.0 线程调度，ReadyQueue/SchedulerImpl/Tcb 来源）、[partition_sched.md](./partition_sched.md)（v0.135.0 分区调度，前序版本）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 中断管理，SGI handler 注册通道与向量表）、[vmm_design.md](./vmm_design.md)（v0.131.0 identity map，从核 MMU 两态同址前提）、[../boot/qemu_run.md](../boot/qemu_run.md) §15（v0.136.0 QEMU 实测记录，三轮排障）

---

## 1. 概述

**核心一句话**：实现多核启动（secondary CPU boot via IPI）、核间通信、多核调度与 RTOS 绑核支持（蓝图 §1 原样口径）。

- **Phase 定位**：P3-C 中断调度 SMP 线**第 4 版**（v0.133.0 ~ v0.138.0）；v3.0 新增基础服务，**v0.137.0 多核内存一致性的基础**（蓝图 §1 / §5.5）；为 v0.138.0 WCET 提供 RTOS 绑核（蓝图 §5.5）。
- **业务价值**：让自研内核利用多核 CPU（飞腾/鲲鹏均多核），RTOS 绑核独享确保实时性（蓝图 §1）。
- **出口关联**：自研内核"SMP"功能完整性证据（蓝图 §1）。
- **ADR-0001 口径**：依据 ADR-0001，Phase 3 主路径为 seL4 深度定制（方案 A），seL4 已具备 SMP 支持，方案 A 下只需在国产 CPU 上验证多核启动与绑核调度；本实现属自研内核研究/fallback 线的功能完整性证据，非生产路径（蓝图 §1 裁剪口径原样）。
- **依赖关系**：前序版本 v0.135.0 分区调度可用（蓝图 §2）；外部依赖多核硬件（QEMU `-smp 4`，蓝图 §2）；假设 ARM64 多核通过 PSCI 启动（蓝图 §2）；多核启动失败阻塞 v0.137.0（蓝图 §2）。
- **本版本激活范围（spec 裁剪口径）**：QEMU 验证「4 核全启动 + IPI 到达」两项蓝图 §6.2 集成判据；绑核过滤语义经 host 测试验证；**从核进完整调度 `schedule_loop` 本版本不激活**——共享 ReadyQueue 的多核加锁与线程迁移属 v0.137.0 内存一致性之后的完整议题，从核启动后安装 per-CPU、使能 SGI、进入 wfi idle（可收 IPI 并计数）（偏差 D3）。

交付物落位（仓库规范 §2.3.1 子系统归口，偏差 D1）：

| 交付物 | 路径 |
|--------|------|
| SMP 常量配置（MAX_CPUS=8 蓝图 §3 配置类原样 + IPI_RING_CAPACITY=16） | [crates/kernel/microkernel/src/smp/config.rs](../../crates/kernel/microkernel/src/smp/config.rs) |
| SMP 类型骨架（CpuId/CpuInfo/IpiMessage 四变体/PerCpu/IpiRing/SmpBootState + CFG1~CFG3 单测） | [crates/kernel/microkernel/src/smp/mod.rs](../../crates/kernel/microkernel/src/smp/mod.rs) |
| 多核启动（SMP_BOOT/boot_secondary/secondary_entry/secondary_rust/per-CPU 存取 + BOOT1~BOOT4 单测） | [crates/kernel/microkernel/src/smp/boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs) |
| 核间中断（IPI_RINGS/send_ipi/broadcast_ipi/ipi_handler/drain_ipi_ring/ipi_stats + IPI1~IPI4 单测） | [crates/kernel/microkernel/src/smp/ipi.rs](../../crates/kernel/microkernel/src/smp/ipi.rs) |
| 多核调度管理（SmpManager trait 六方法蓝图 §3 原样签名 + SmpManagerImpl + SMP1~SMP5 单测） | [crates/kernel/microkernel/src/sched/smp_sched.rs](../../crates/kernel/microkernel/src/sched/smp_sched.rs) |
| Tcb 追加 `pinned_cpu` 字段（**加法式 MODIFIED**，v0.134.0 交付物，默认 None 行为不变） | [crates/kernel/microkernel/src/sched/tcb.rs](../../crates/kernel/microkernel/src/sched/tcb.rs) |
| 绑核过滤调度（`SchedulerImpl::schedule_on`；`schedule()` 委托 `schedule_on(0)`） | [crates/kernel/microkernel/src/sched/scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs) |
| 内核启动期接入（§3.10 `smp_smoke` 段 + `smp_secondary_entry` 接入桩 + `smp_stack_alloc` PMM 适配器） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试（SMPT1~SMPT6） | [crates/kernel/microkernel/tests/smp_test.rs](../../crates/kernel/microkernel/tests/smp_test.rs) |
| QEMU 测试环境（`-smp 4` + 第九条断言 `[smp] ok`，既有八条原样） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| 设计文档（蓝图 §3 文档类 / §7.4 验收） | [docs/kernel/smp_design.md](./smp_design.md)（本文档） |

**类型与 trait 一览**（[smp/mod.rs](../../crates/kernel/microkernel/src/smp/mod.rs) 与 [smp_sched.rs](../../crates/kernel/microkernel/src/sched/smp_sched.rs) 公开导出，蓝图 §3 接口类原样签名）：

| 项 | 定义 | 说明 |
|----|------|------|
| `CpuId` | `pub type CpuId = u8` | 核标识符（取值 0..MAX_CPUS；QEMU virt 单核组与 MPIDR aff0 一一对应） |
| `CpuInfo` | struct 四字段 | `id / online / current_tid / current_partition`（蓝图 §3 原样顺序；后两字段改 `Option`，偏差 D4） |
| `IpiMessage` | `enum { Reschedule, TlbShootdown(usize), Wakeup(Tid), Shutdown }` | 核间中断消息四变体（蓝图 §3 原样） |
| `PerCpu` | struct 五字段 | `cpu_id / current_tid / kernel_stack / current_partition / ipi_queue`（蓝图 §4.1 原样；ipi_queue 为定长 IpiRing，偏差 D2） |
| `IpiRing` | 定长 SPSC 环 | `buf: [Cell<IpiMessage>; 16]` + head/tail/dropped 原子计数（偏差 D2，见 §5） |
| `SmpBootState` | struct 三字段 | `secondary_entry / secondary_stack[8] / booted[8]` 全原子量（蓝图 §4.1；统一原子量保证跨核可见性） |
| `SmpManager` | trait 六方法 | `boot_secondary / send_ipi / broadcast_ipi / cpu_online / online_count / pin_thread`（蓝图 §3 原样签名） |

> **配置类口径**：`MAX_CPUS = 8`（蓝图 §3 配置类原样，覆盖飞腾/鲲鹏 8 核平台与 QEMU `-smp 4` 验证环境）；`IPI_RING_CAPACITY = 16`（每核 IPI 环容量，取 2 的幂便于取模回绕）；从核独立栈 `SECONDARY_STACK_SIZE = 16KB`（4 个 4KB 页，与 `sched::config::STACK_SIZE` 同口径，线程栈/启动栈统一规格，蓝图 §8.5「从核栈必须独立分配」坑点）。

---

## 2. PSCI 启动时序

蓝图 §4.3 时序图的实现落位（aarch64 真实路径；host 桩路径见偏差 D10）：

```
CPU0（主核，smp_smoke 接入段，main.rs §3.10）        CPU<n>（从核）
─────────────────────────────────────────          ─────────────────────
① install_stack_allocator(smp_stack_alloc)
   （PMM 适配器注入，D6；4 页 = 16KB/核）
② 主核侧 SGI0 准备：GICR IGROUPR0/ISENABLER0 bit0
   （读改写保留 v0.133.0 PPI30 bit30）
   + register_irq_handler(0, ipi_handler)
   （必须先于任一从核开 IRQ，见下）
③ boot_secondary(cpu, smp_secondary_entry_addr)：
   参数校验（cpu==0 主核 / ≥MAX_CPUS 越界 → InvalidArgument）
   stack_top = 栈分配器(SECONDARY_STACK_PAGES)
   SECONDARY_STACK_BASE / secondary_entry /
   secondary_stack[cpu] / booted[cpu]=false —— Release 四连
   PSCI CPU_ON：hvc #0
     x0=0xC400_0003（SMC64 功能号，蓝图 §4.5 原样）
     x1=MPIDR affinity（QEMU virt 单核组 aff0 即 CpuId）
     x2=entry  x3=context_id=cpu_id（D7 修正蓝图 x3=0）
──────────────────────────────────────────────────► 固件复位从核、跳转 entry
                                                    smp_secondary_entry（main.rs 接入桩，D-S1）：
                                                      x19 = x0（cpu_id 跨 bl 保存，D-S2）
                                                      sp = secondary_stack[cpu]
                                                          （SECONDARY_STACK_BASE 指针槽间接寻址，D8；
                                                            MMU 关闭态 PA==identity 直访，D9）
                                                      bl smp_secondary_setup：
                                                        install_vbar（VBAR_EL1 per-core 装载）
                                                        daifclr #2（开 IRQ 掩码）
                                                      bl secondary_rust
                                                    secondary_rust（boot.rs，aarch64 门控）：
                                                      set_per_cpu：写本核槽位 → TPIDR_EL1=槽位基址
                                                                    → Release 置 installed（§4）
                                                      裸 PL011 打印 [smp] cpu<n> on
                                                      wake_redistributor_and_enable_sgi0：
                                                        GICR 唤醒（清 WAKER.ProcessorSleep +
                                                          轮询 ChildrenAsleep）
                                                        IGROUPR0/ISENABLER0 bit0（SGI0→Group1+使能）
                                                        ICC_IGRPEN1_EL1=1 / ICC_PMR_EL1=0xFF
                                                        （D11；PMR 为排障①补全，D19）
                                                      booted[cpu]=true —— Release
                                                        （置位后移，握手语义升级，D20）
                                                      wfi idle 循环（可收 SGI0 IPI；D3）
   Acquire 轮询 booted[cpu] ◄───────────────────────
   Ok(()) —— 返回即蕴含「从核 per-CPU 安装 + SGI 接收路径就绪」
   超时 Err(Timeout) —— 降级路径（见 §7）
```

时序要点：

| 环节 | 实现口径 | 锚点 |
|------|---------|------|
| 入口/栈发布 | 主核 Release 四连先于 `hvc #0`——CPU_ON 返回后从核随时可能取指入口读栈槽 | boot.rs `boot_secondary` |
| context_id 传递 | PSCI 架构约定 context_id 经 x0 送达新核入口；x3=cpu_id（非蓝图原样 0，否则新核 x0 恒 0 无法区分启动目标） | 偏差 D7 |
| 栈槽寻址 | 汇编经 `#[no_mangle]` `SECONDARY_STACK_BASE` 指针槽间接寻址栈表，不依赖 `SmpBootState` 字段布局（零布局假设） | 偏差 D8 |
| MMU 状态 | 从核 PSCI 复位启动时 MMU 关闭（PA==identity 直访）；主核已开 MMU 但为 identity map（v0.131.0 口径），栈/串口/GIC 地址两态同址有效，从核无需先行建页表 | 偏差 D9 |
| per-core 收尾 | VBAR_EL1/PSTATE 为 per-core 状态，主核无法代写——接入桩在从核取指流中完成 install_vbar + daifclr（D-S1），boot.rs `secondary_rust` 与 `interrupt/` 零改动 | 偏差 D-S1 |
| booted 握手 | 从核 Release 置位 / 主核 Acquire 轮询；**置位后移**至 GICR 唤醒 + SGI0 使能 + ICC_PMR 之后——握手语义 = 「per-CPU 已安装」+「本核 SGI 接收路径（GICR 唤醒 / IGROUPR0 / ISENABLER0 / ICC_IGRPEN1 / ICC_PMR）均就绪」，主核 `boot_secondary` 返回即从核可收 IPI | 偏差 D20（排障②） |
| handler 注册时机 | 全局 handler 表为全核共享（vector.rs `gic()` 单例），主核注册即覆盖从核分发路径；从核 `smp_secondary_setup` 开 IRQ 后即可承接 SGI0，注册必须先于任一从核启动 | main.rs `smp_smoke` 决策记录 |

---

## 3. GICv3 SGI 编码表

IPI 经 GICv3 SGI0（`IPI_SGI_ID = 0`；SGI 空间 0~15，取 0 为核间中断专用）触发，发送侧写 `ICC_SGI1R_EL1`。蓝图 §8.5 坑点「GICv3 SGI 编码格式复杂」在本模块收敛为 **QEMU virt 单集群口径**：

| 位域 | 位 | 本版本取值 | 说明 |
|------|----|-----------|------|
| TargetList | bits[15:0] | 单播 `1 << target`；广播 `0xFF & !self` | Aff0 核位图，bit t 置位即目标核 aff0=t |
| AFF1 | bits[23:16] | 0 | 单集群（QEMU virt 全部核同属 aff1=0） |
| INTID | bits[27:24] | 0（SGI0） | 本模块恒为 `IPI_SGI_ID` |
| AFF2 | bits[39:32] | 0 | 单集群 |
| AFF3 | bits[47:40] | 0 | 单集群 |
| IRM | bit 40 | 0 | TargetList 模式（非路由模式） |

故 **QEMU virt 单集群编码：单播 `sgir = 1 << target`，广播 `sgir = 0xFF & !self_bit`**（[ipi.rs](../../crates/kernel/microkernel/src/smp/ipi.rs) 模块头注编码表原样）。广播 TargetList 保留 8 核全集口径（`0xFF` 去自核位）——QEMU `-smp 4` 下高 4 位无对应核，GIC 对无核位忽略（偏差 D15）。多集群 aff1/aff2 编码与真机 DTB 拓扑解析属后续版本。

接收侧（每核各做一次，per-core）：GICR 唤醒 → `GICR_IGROUPR0` bit0（SGI0 → Group 1）→ `GICR_ISENABLER0` bit0（使能）→ `ICC_IGRPEN1_EL1`=1 → `ICC_PMR_EL1`=0xFF（优先级门限放开；复位值 0 屏蔽全部优先级，缺则 SGI0 在 CPU interface 被过滤——排障①，偏差 D19）。SGI 属 banked 中断（INTID 0~15），使能走本核 GICR SGI 帧而非 GICD。

---

## 4. per-CPU 数据与 TPIDR_EL1

`PerCpu` 蓝图 §4.1 原样五字段（[smp/mod.rs](../../crates/kernel/microkernel/src/smp/mod.rs)）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `cpu_id` | `CpuId` | 本核标识 |
| `current_tid` | `Tid` | 当前运行线程（初态 0 = 启动桩线程，裸值口径同蓝图） |
| `kernel_stack` | `usize` | 本核内核栈顶地址（16KB 独立栈，D6） |
| `current_partition` | `PartitionId` | 当前运行分区（初态 `PartitionId(0)`，裸值口径同蓝图） |
| `ipi_queue` | `IpiRing` | 本核 IPI 消息环（定长 SPSC 环，偏差 D2；本版本 handler 实际排水走 IPI_RINGS 全局数组，偏差 D12） |

**存储与安装**（[boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs)）：

- 槽位：`static PER_CPU: [PerCpuSlot; MAX_CPUS]`（`UnsafeCell<MaybeUninit<PerCpu>>` 驻留静态区，规避 edition 2024 `static_mut_refs` deny，同 vector.rs GicCell 口径）+ `PER_CPU_INSTALLED: [AtomicBool; 8]` 安装标志；
- `set_per_cpu(cpu, per_cpu)`：写本核槽位 → **aarch64 经 `msr tpidr_el1` 记录槽位基址**（per-CPU 快速访问通道，蓝图 §5.2 关键技术）→ Release 置 installed；每核槽位仅在本核启动路径写入一次（installed 置位前无观察者，无数据竞争）；
- `per_cpu(cpu) -> Option<&'static PerCpu>`：越界或未安装返回 `None`；Acquire 检查 installed（配对 set_per_cpu 的 Release，构成 happens-before）——蓝图 §9.6「per-CPU 状态可查」口径的承载；
- `current_cpu() -> CpuId`：aarch64 读 `MPIDR_EL1 & 0xFF`（aff0；QEMU virt MPIDR = `0x8000_0000 | cpu`，单核组 aff0 与 CpuId 一一对应，`aff0_of` 单点解析）；host 恒 0（host 测试无双核上下文，单核桩口径）。

`SmpBootState`（蓝图 §4.1 多核启动状态）：`secondary_entry: AtomicUsize` / `secondary_stack: [AtomicUsize; 8]` / `booted: [AtomicBool; 8]` 全字段原子量 + const fn new，整体静态初始化（`SMP_BOOT` 全局实例承载）；`secondary_stack` 蓝图原样 `[usize; 8]` 与 entry/booted 统一为原子量，保证跨核可见性口径。

---

## 5. IpiRing SPSC 设计

蓝图 §4.1 `PerCpu.ipi_queue: SpinLock<VecDeque<IpiMessage>>` 的 no_std 定长化（偏差 D2；同 v0.134.0 ReadyRing 定长环口径）：`IpiRing` 持 `buf: [Cell<IpiMessage>; 16]` + `head/tail: AtomicUsize`（单调递增计数，槽位下标取模回绕）+ `dropped: AtomicUsize`（满环丢弃累计）；空/满以 `tail - head` 距离判定（usize 回绕减法在计数不溢出 2^64 前恒正确）。

**免锁论证（SPSC 口径）**：IPI 场景为**单生产者**（发送核 `send_ipi` 路径 push；本版本仅主核发送，多生产者并发 send 属 v0.137.0 内存一致性议题）**单消费者**（目标核自身 IRQ handler 路径 pop），无需锁，原子位序即保证可见性：

| 侧 | 操作序列 | 位序 |
|----|---------|------|
| 生产者 push | 读 head（**Acquire**，配对消费者 Release 推进）→ 距离开满则丢弃计数 +1 返回 false → 写槽位 → tail+1（**Release**：槽位写入先于 tail 推进对消费者可见） | 满环不阻塞发送路径 |
| 消费者 pop | 读 tail（**Acquire**：观察到生产者 Release 的 tail 即观察到槽位写入）→ 空则返回 None → 读槽位 → head+1（**Release**：head 推进先于槽位复用对生产者可见） | FIFO 逐条排水 |

- **满环丢弃计数**：环满（16 条）时 push 返回 false 并累计 `dropped_count`（观测口径），消息丢弃不阻塞发送路径；`send_ipi` 满环仍返回 `Ok(())` 且不再触发 SGI（蓝图 §6.5「IPI 丢包处理」为可观测降级而非发送错误，偏差 D13）；
- **Cell 槽位安全**：SPSC 口径下不存在跨核同时读写同一槽的别名冲突（生产者写下标 tail%16 槽、消费者读 head%16 槽，满/空判定保证两侧不越界）；`SyncRings` 静态共享的 Sync 论证同此；
- **容量口径**：16 槽对冒烟与诊断负载充足；host CFG3 覆盖满/空/回绕边界 + 丢弃计数，IPI3 覆盖「满环丢弃 → 排水 → 重新入环」全流程。

**分类计数与排水**（蓝图 §9.6 可观测）：四变体各一 AtomicUsize 计数器（`CNT_RESCHEDULE/CNT_TLB_SHOOTDOWN/CNT_WAKEUP/CNT_SHUTDOWN`），`ipi_stats()` 取 Relaxed 快照；`drain_ipi_ring(cpu)` 逐条 pop + 分类计数，返回排水条数。主核 IPI 到达确认即经分类计数增量轮询（见 §7）。

---

## 6. 绑核调度过滤语义

**Tcb 加字段**（**加法式 MODIFIED**，偏差 D5）：`Tcb` 追加第十一字段 `pinned_cpu: Option<CpuId>`——`None` = 任意核可调度（默认，与 v0.134.0 单核行为全等）；`Some(cpu)` = 仅在该核调度。`Tcb::new` 保持十字段签名不变（既有调用与 TCB1~TCB4 零改动），`pinned_cpu` 恒初始化为 `None`，绑核写入统一经 `SmpManager::pin_thread`。

**pin_thread 校验**（蓝图 §4.4）：tid 不存在（空槽/超表）→ `Err(InvalidArgument)`；cpu ≥ MAX_CPUS → `Err(InvalidArgument)`（拒绝后既有绑核状态不被破坏，SMP2 锚定）；重复绑核覆写生效（SMP1）。

**schedule_on 过滤算法**（[scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs)，偏差 D16/D17）：自最高非空桶向低桶（位图 CLZ 口径同 v0.134.0）、桶内自队首向队尾扫描首个 `pinned_cpu ∈ {None, Some(cpu)}` 的就绪线程并摘除切换；无匹配返回 `None`（空队语义同 v0.134.0 `schedule()`，idle 兜底由调用方处理）：

- **桶内遍历选首个匹配 + 原地保序摘除**：未命中线程位置不动，同优先级 FIFO 相对顺序保持；队首命中 `pop_front` O(1)，队中命中 `ReadyRing::remove` 前移保序 O(桶长)；弃用「弹出暂存回灌」（扰动 FIFO 顺序且同为 O(n)）（偏差 D17）；
- **过滤优先于优先级**：绑核到他核的线程即使优先级最高也不被选中（SMP4：A 绑 cpu3 prio50 在 cpu0 上下文 idle 兜底；SMPT2：cpu0 第 3 选时 C(30)/D(20) 均高于 idle(0) 仍被过滤）；
- **`schedule()` 委托 `schedule_on(0)`**：全 `None` 绑核下首桶首项命中，与 `ReadyQueue::pop_highest` 同阶 O(1)，行为与 v0.134.0 全等（SCH1~SCH6 于 scheduler.rs 原样复跑零回归；SMP5 经 SmpManagerImpl 聚合路径复验 SCH4 抢占主口径）（偏差 D16）。

**本版本语义边界（偏差 D3）**：绑核过滤语义经 host 单测/集成测试完整验证（SMP4/SMPT2 两核过滤矩阵）；真机从核 wfi idle 不运行调度循环，共享 ReadyQueue 的多核加锁、per-CPU 调度表与线程迁移属 v0.137.0 内存一致性之后的完整议题（蓝图 §4.5 `secondary_entry` 末尾 `schedule_loop` 本版本不激活）。

---

## 7. 超时降级

蓝图 §4.4 原样口径「从核超时未启动：警告但继续（降级单核）」：

- **轮询超时**：`boot_secondary` Acquire 轮询 booted 上限 `BOOT_TIMEOUT_ITERS = 1_000_000` 次迭代（蓝图 §4.5 原样数值；aarch64 真机对应毫秒级，满足蓝图 §6.3「从核启动 < 10ms」观测口径）；超时返回 `Err(KernelError::Timeout)`——**不 panic 不挂死**，调用方以单核或部分核降级继续运行；
- **冒烟段降级打印**（main.rs `smp_smoke`）：超时核主核打印 `[smp] cpu<n> timeout` 并继续启动下一核；PSCI 拒绝（BadState）等非超时错误打印 `[smp] cpu<n> err 0x<code>` 同口径降级；
- **判定口径**：`≥2 核在线（主核 + 任一从核）且 IPI 到达` → 打印 `[smp] ok`；否则 `[smp] err online=<n> ipi=<0|1>`。蓝图 §6.2 集成判据为「4 核全启动 + IPI 到达」，故障注入/部分核超时场景按蓝图 §4.4 降级语义放宽——IPI 到达本身蕴含至少一个从核在线，两条件并列显式判定；
- **IPI 到达确认机制**：`[smp] ipi ok` 由**从核** handler 打印，主核无法直接观测从核串口写——经 ipi.rs 分类原子计数（从核 handler 上下文 `fetch_add`）轮询增量确认；主核 spin 轮询而非 wfi（从核计数自增不会唤醒主核）；Relaxed 计数在本场景仅作「到达与否」诊断旗标（ARM64 原子量多拷贝原子性保证最终可见，无依赖序数据消费）；超时上限 `SMP_IPI_WAIT_ITERS = 10_000_000` 防死循环（同 v0.133.0 §3.7 首中断超时兜底口径）；
- **host 失败注入**（偏差 D10）：`inject_boot_failure(cpu, true)` 模拟「固件接受但从核永不上报」，主核走真实轮询超时路径——BOOT2/BOOT3/SMPT3 覆盖「Err(Timeout) 返回 / 失败核保持离线 / 其他核仍可正常启动 / 解除注入可逆」全语义；
- **栈泄漏口径**：成功启动从核的 16KB 栈随核常驻（从核 wfi idle 复用为 IPI 承接栈），不释放；超时核的栈帧归还属后续版本（本版本冒烟路径不归还，泄漏 16KB/超时核，PMM zone 余量充足，观测口径注释）。

---

## 8. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`（lib 内嵌单测 + tests/ 集成测试；host 上可使用 std）。全局态（IPI 环数组/分类计数/host SGI 桩/SMP_BOOT/失败注入表/栈分配器槽）为进程级单例，相关用例经测试锁串行化 +「进入先清场 + 增量断言」口径保证 cargo test 默认并行下结果确定。

### 8.1 单元测试（smp/mod.rs 内嵌，CFG1~CFG3）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| CFG1 | `cfg1_constants_and_cpuid_width` | MAX_CPUS==8、IPI_RING_CAPACITY==16、CpuId 为 u8（1 字节）、最大 id 7 | PASS |
| CFG2 | `cfg2_ipi_message_variants_and_semantics` | IpiMessage 四变体构造/相等/Copy/Clone；TlbShootdown 带 ASID、Wakeup 带 Tid 载荷传递 | PASS |
| CFG3 | `cfg3_ipi_ring_push_pop_full_empty_wraparound` | IpiRing 边界：空 pop None；满 16 拒第 17 且丢弃计数 +1；FIFO 全量回绕顺序；回绕后再填弹顺序仍正确；丢弃计数不受正常收发影响 | PASS |

### 8.2 单元测试（smp/boot.rs 内嵌，BOOT1~BOOT4）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| BOOT1 | `boot1_stub_success_path_and_arg_validation` | host 桩成功路径：Ok + booted 置位 + entry/stack Release 可见 + per-CPU 安装（cpu_id/kernel_stack/初态字段）；cpu=0 与 ≥MAX_CPUS 返回 InvalidArgument；未启动核保持离线 | PASS |
| BOOT2 | `boot2_failure_injection_returns_timeout_and_not_booted` | 失败注入：Err(Timeout)、booted 未置位、per-CPU 未安装；entry/stack 存储不受注入影响；解除注入可逆 | PASS |
| BOOT3 | `boot3_timeout_degrades_without_panic_and_system_continues` | 超时降级语义：Err(Timeout) 而非 panic/挂死（用例存活即证）；其他核仍可正常启动；失败核保持离线不污染在线计数 | PASS |
| BOOT4 | `boot4_mpidr_aff0_parsing_and_host_current_cpu` | aff0_of 提取 bits[7:0]（0x8000_0000\|cpu → cpu）；高位 aff1~3 不干扰；`current_cpu()` host 恒 0 | PASS |

### 8.3 单元测试（smp/ipi.rs 内嵌，IPI1~IPI4）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| IPI1 | `ipi1_send_then_handler_drains_fifo_and_counts` | 入环三条 → host 桩三次 SGI 触发、TargetList=1<<0、消息原样；handler 排水后环空、分类计数各 +1、返回 Handled；同排水路径 FIFO 顺序校验 | PASS |
| IPI2 | `ipi2_broadcast_reaches_all_other_cpus` | 本核 cpu0 广播：除自核外全部核环各入一条（cpu1/2/3 逐核验证恰好一条），自核环空；桩记录 broadcast 计数 +1、TargetList=0xFE | PASS |
| IPI3 | `ipi3_full_ring_drops_and_counts` | 满环丢弃：填满 16 条逐条触发 SGI，第 17/18 条丢弃计数递增、仍 Ok、不再触发 SGI；排水后环空可重新入环；越界目标 InvalidArgument 不入环不触发 | PASS |
| IPI4 | `ipi4_handler_classifies_all_four_variants` | 四变体各入一条到本核环，ipi_handler 排水后四计数器各恰好 +1（增量口径），返回 Handled，环空 | PASS |

### 8.4 单元测试（sched/smp_sched.rs 内嵌，SMP1~SMP5）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| SMP1 | `smp1_pin_thread_records_pinned_cpu` | 新建线程 pinned_cpu 默认 None（D5）；pin_thread 写入就位；重复绑核覆写生效 | PASS |
| SMP2 | `smp2_pin_thread_out_of_range_cpu_rejected` | cpu ≥ MAX_CPUS（含 255）InvalidArgument；拒绝后既有绑核状态不被破坏 | PASS |
| SMP3 | `smp3_pin_thread_invalid_tid_rejected` | 空槽 tid 与超表 tid（≥256）均 InvalidArgument | PASS |
| SMP4 | `smp4_schedule_on_filters_pinned_threads` | 绑核过滤：A 绑 cpu3（最高优先级）在 cpu0 不被选中（idle 兜底）；A 不被误摘（换 cpu3 即选中）；未绑核线程任意核可选；过滤不扰动就绪队 FIFO 位置 | PASS |
| SMP5 | `smp5_schedule_backward_compatible_with_v134` | 兼容：schedule() 委托 schedule_on(0)，全 None 绑核下与 v0.134.0 全等（高优先级抢占/阻塞让位/idle 兜底 SCH4 口径复验） | PASS |

### 8.5 集成测试（tests/smp_test.rs，SMPT1~SMPT6，仅以公开 API 端到端验证）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| SMPT1 | `smpt1_ipi_end_to_end_ring_and_classified_stats` | IPI 端到端：cpu0 向 cpu1 连发四变体各一（桩四次触发、TargetList=1<<1）；切 cpu1 上下文 handler 排水：Handled + 四计数各 +1 + 环排空；同型环补全 FIFO 保序证据 | PASS |
| SMPT2 | `smpt2_pinned_schedule_filter_matrix` | 绑核两核过滤矩阵：A 不绑核(50)/B 绑 cpu0(40)/C 绑 cpu1(30)/D 绑 cpu3(20)；cpu0 选 A→B→idle、cpu1 选 C→idle、cpu3 选 D→idle→None；过滤优先于优先级；未命中线程就绪位置不扰动；空队 None 语义同 v0.134.0 | PASS |
| SMPT3 | `smpt3_secondary_boot_timeout_degrades_and_continues` | 超时降级端到端：cpu1 上线（online 1→2）→ 注入 cpu2 失败 → Err(Timeout)、计数不变 → cpu3 正常上线 → 解除注入 cpu2 补上线（online 4）；参数校验委托口径 | PASS |
| SMPT4 | `smpt4_broadcast_reaches_three_cpus_one_each` | broadcast 三核各收一条：cpu0 广播 Reschedule，cpu1/2/3 各恰好一条逐核验证，自核环空 | PASS |
| SMPT5 | `smpt5_tlb_shootdown_asid_payload_passing` | TlbShootdown(asid=0x1234) 载荷传递：host SGI 桩重建消息载荷原样 | PASS |
| SMPT6 | `smpt6_ipi_push_drain_perf_smoke` | IPI 入环+排水性能 smoke：1000 次「send → drain」计时，eprintln 实测均值，断言 < 5μs 宽松上限防算法退化；1000 条全量收发无丢失 | PASS |

> **SMPT6 口径声明**：实测均值为 **ns 级/op**（1000 次入环+排水平均，eprintln 输出）——host 侧（Windows/Linux 用户态）smoke 口径，**不代表目标机（ARM64 裸机）性能**（同 v0.134.0 scht7 / v0.131.0 T07 口径惯例）；蓝图 §6.3/§7.2「IPI 延迟 < 1μs」为目标机口径，目标机性能测定属后续版本。

### 8.6 QEMU 冒烟（main.rs §3.10 段 + [qemu_run.md](../boot/qemu_run.md) §15）

冒烟链路见 §2 时序图。串口输出关键行（原样收录，WSL2 QEMU 8.2.2，`-M virt,gic-version=3 -smp 4`）：

```text
[psched] ok
[smp] cpu1 on
[smp] cpu2 on
[smp] cpu3 on
[smp] ipi ok
[smp] ok
```

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **九**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok`；v0.136.0 起追加第九条断言 `grep "[smp] ok"` 与 `-smp 4`，既有八条原样） |
| 端到端总耗时 | 14s（构建 3.64s + QEMU timeout 10s 守护窗口） |
| 在线情况 | 4 核全在线（未触发蓝图 §4.4 降级路径，timeout/err 分支未触发） |
| 回归 | host **154** 测试全过零回归；`cargo fmt` / `cargo clippy -D warnings` 干净 |

**排障记录（三轮失败 → 三处多核启动 bug 修复，详见 [qemu_run.md](../boot/qemu_run.md) §15）**：

| 轮次 | 症状 | 根因 | 修复（偏差锚点） |
|------|------|------|-----------------|
| ① 首轮 FAIL | 三核 `[smp] cpu<n> on` 全上线但 `[smp] err online=4 ipi=0`（IPI 永不到达）；`-d int` 诊断从核上无任何 IRQ 异常 | 从核 ICC_PMR_EL1 复位值 0 屏蔽全部优先级（v0.133.0 §3.7 主核修正②同根因的 per-core 版本），SGI0 在从核 CPU interface 被 PMR 过滤 | `wake_redistributor_and_enable_sgi0` 追加 `msr icc_pmr_el1, 0xFF`（D19） |
| ② 次轮 FAIL | 症状不变；`-trace` 取证主核 SGI 生成**先于** cpu1 的 IGRPEN/PMR 写 | booted 握手竞态：`secondary_rust` 原在 GIC 使能前置位 booted，主核见到后即启动后续核并发送 IPI，SGI 于从核 ISENABLER0=0/PMR=0 复位态到达被丢弃 | booted Release 置位后移至 GIC 使能之后，握手语义升级为「per-CPU 安装 + SGI 接收路径就绪」（D20） |
| ③ 三轮 FAIL | 输出尾部 `[[smsp]m ippi ]ok` 交错，两个串均不可 grep | 双核无锁打印同一 UART：ipi_handler 原先排水计数（主核到达判据）再打印，主核观测到计数立即打印 `[smp] ok`，与 cpu1 打印并发交错 | ipi_handler 打印前移至排水计数之前——计数是主核等待的唯一同步点，观测到计数时对侧打印在程序序上必然已结束，串口输出经既有计数同步通道天然串行化（D21） |
| ④ 修复后 | 一次通过，九断言全过 | — | 三处均确属多核启动 bug 的最小修复（host 测试零影响，aarch64 门控） |

蓝图 §6 其余条目对照：

| 蓝图条目 | 覆盖情况 |
|---------|---------|
| §6.1 单元测试「IPI 消息队列逻辑、绑核配置」 | CFG3/IPI1~4（环逻辑）+ SMP1~3（绑核配置）+ SMPT1/SMPT5 |
| §6.2 集成测试「QEMU -smp 4，4 核全部启动；RTOS 线程绑核到 CPU3 独占」 | QEMU 冒烟 4 核全在线（`[smp] cpu1~3 on` + `[smp] ok`）；绑核语义由 SMP4/SMPT2 host 矩阵验证（真机从核调度循环本版本不激活，D3 口径） |
| §6.3 性能基准「IPI 延迟 < 1μs；从核启动 < 10ms」 | SMPT6 host smoke ns 级/op（口径声明见 §8.5 注）；BOOT_TIMEOUT_ITERS 毫秒级轮询上限满足 < 10ms 观测口径；目标机性能测定属后续版本 |
| §6.4 回归「单核功能不受影响」 | QEMU 冒烟既有八断言（`[boot]`~`[psched] ok`）先于 `[smp] ok` 全部通过；SMP5 委托全等复验；host 154 测试零回归 |
| §6.5 故障注入「从核启动超时降级；IPI 丢包处理」 | BOOT2/BOOT3/SMPT3（超时降级，host 失败注入走真实轮询路径）；IPI3（满环丢弃计数、不触发 SGI、仍 Ok） |
| §6.6 GPU 规则 | 不适用（SMP 启动，蓝图原样口径） |

---

## 9. 蓝图偏差表（D1~D21 + D-S1/D-S2）

> 文档级编号与源码头注模块内编号的对应关系（各模块头注编号模块内独立）：**smp/mod.rs 头注 D1~D3 → 本文档 D1/D2/D4**；**sched/tcb.rs 头注 D4 → 本文档 D5**；**smp/boot.rs 头注 D4~D9 → 本文档 D6~D11**；**smp/ipi.rs 头注 D4~D8 → 本文档 D12~D15、头注 D9 → 本文档 D21**；**sched/smp_sched.rs 头注 D5~D7 → 本文档 D16~D18**；D3 为 spec 级偏差（从核 wfi idle 降级）；D19/D20 为 QEMU 排障①/②修复（qemu_run.md §15 记录，boot.rs 函数内注释同步）；**main.rs 头注 D-S1/D-S2 → 本文档同号**。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D1 | 蓝图交付物 `kernel/src/smp/{boot,ipi,config}.rs`、`kernel/src/sched/smp_sched.rs`、`kernel/tests/smp_test.rs` → 落位 `crates/kernel/microkernel/src/smp/`、`crates/kernel/microkernel/src/sched/` 与 `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/smp_design.md` 同蓝图（= smp/mod.rs 头注 D1） | 仓库规范 §2.3.1 强制子系统归口（`crates/<subsystem>/`），禁止根目录 crate | 仅路径差异，交付物一一对应 |
| D2 | 蓝图 §4.1 `PerCpu.ipi_queue: SpinLock<VecDeque<IpiMessage>>` → 定长 `IpiRing`（`[Cell<IpiMessage>; 16]` + head/tail/dropped 原子计数）：IPI 场景为单生产者（发送核 send_ipi）单消费者（目标核 IRQ handler）SPSC 口径，head/tail 原子位序替代锁（= smp/mod.rs 头注 D2；spec 偏差①） | no_std 无堆（仓库规范 §4.3 硬性要求），VecDeque/spin 外部依赖不可用；SPSC 口径原子位序即可保证可见性（§5 位序表） | 容量编译期确定、无分配失败路径；满环丢弃计数口径（D13）；CFG3/IPI3 覆盖边界 |
| D3 | 蓝图 §4.5 `secondary_entry` 末尾 `crate::sched::schedule_loop()` 从核进入完整调度**本版本不激活**——从核启动后安装 per-CPU、使能 SGI、进入 wfi idle（可收 IPI 并计数）（= spec 偏差②；boot.rs `secondary_rust` / main.rs `smp_smoke` 文档注释记录） | 共享 ReadyQueue 的多核加锁与线程迁移属 v0.137.0 内存一致性之后的完整议题；本版本最小激活面 = 4 核启动 + IPI 到达 + 绑核过滤语义 | QEMU 验证蓝图 §6.2「4 核全启动 + IPI 到达」两项集成判据；绑核过滤经 host SMP4/SMPT2 验证；v0.137.0 前置工作明确 |
| D4 | `CpuInfo.current_tid`/`current_partition` 蓝图原样 `Tid`/`PartitionId` 裸值 → `Option`（= smp/mod.rs 头注 D3） | 核在线但尚无运行线程/分区的初态无可表述裸值，查询快照以 `None` 表达 | spec 接口节同口径；蓝图 §3 四字段顺序保持 |
| D5 | `Tcb` 追加第十一字段 `pinned_cpu: Option<CpuId>`（默认 None = 任意核可调度，与 v0.134.0 全等）；`Tcb::new` 保持十字段签名不变，`pinned_cpu` 恒初始化为 `None`，绑核写入统一经 `pin_thread`（= sched/tcb.rs 头注 D4） | 加法式 MODIFIED，行为兼容（spec MODIFIED Requirements 口径） | v0.134.0 既有十字段与 TCB1~TCB4 零改动；SMP1 锚定默认 None |
| D6 | 从核栈分配经 `install_stack_allocator` 回调注入——库 crate 不持有 PMM 实例（全局 BuddyAllocator 归 main.rs 二进制所有，同 v0.130.0 PmmCell 口径）；栈 16KB（4 页）与线程栈同规格（= smp/boot.rs 头注 D4） | 蓝图 §8.5 坑点「从核栈必须独立分配」；lib 不依赖 PMM 保持 host 可测性 | Task 6 真机注入 PMM 适配器 `smp_stack_alloc`、host 测试注入伪分配器；未安装分配器返回 `NotSupported`（配置错误非运行时故障） |
| D7 | PSCI CPU_ON 的 x3 context_id 传 `cpu_id` 而非蓝图 §4.5 原样 0——蓝图注释「x0=cpu_id 由 PSCI ctx 传入」与「x3=0」自相矛盾（ctx=0 则新核 x0 恒 0，无法区分启动目标）；PSCI 架构约定 context_id 经 x0 送达新核入口（= smp/boot.rs 头注 D5，蓝图偏差） | 架构约定 + 蓝图自相矛盾修正 | x1=MPIDR affinity（QEMU virt 单核组 aff0 即 CpuId）；从核入口 x0=cpu_id 全链路（asm → secondary_rust 参数）一致 |
| D8 | `secondary_entry` 汇编经 `#[no_mangle]` `SECONDARY_STACK_BASE` 指针槽间接寻址栈表——`boot_secondary` 在 CPU_ON 前将栈表基址 Release 写入指针槽，asm 读指针再按 CpuId 索引；不改 `SmpBootState` 定义（不加 repr(C)），零布局假设（= smp/boot.rs 头注 D6） | 初版 `offset_of!` 断言实测 rustc 默认 repr 字段重排，间接寻址根除布局依赖（v0.134.0 D13 同类教训的前置规避） | `SmpBootState` 保持蓝图 §4.1 原样定义；main.rs 接入桩复用同一指针槽（D-S1） |
| D9 | 从核经 PSCI 复位启动时 MMU 关闭（PA==identity 直访）；主核已开 MMU 但为 identity map（v0.131.0 口径），栈/串口/GIC 地址两态同址有效，从核无需先行建页表（= smp/boot.rs 头注 D7） | QEMU virt identity map 既有口径复用 | 从核入口装 sp、裸 PL011 打印、GICR 访问均在 MMU 关闭态完成；主核 GIC 2MB identity map 覆盖 `-smp 4` 全部 GICR 帧 |
| D10 | host 桩 `psci_cpu_on` 无固件可调用——成功路径直接安装 per-CPU + 置 booted（时序与真机 `secondary_rust` 前半段一致，略过串口/GICR/wfi 硬件段）；失败注入 `inject_boot_failure` 跳过 booted 置位，主核走真实轮询超时路径（= smp/boot.rs 头注 D8） | host 无 PSCI 固件；超时降级语义需真实轮询路径验证 | BOOT1~BOOT3/SMPT3 覆盖成功/超时/降级继续/注入可逆；aarch64 真实 `hvc #0` 由 QEMU 冒烟验证 |
| D11 | 本核 GICR 帧基址 = 0x080A_0000 + cpu×0x20000（QEMU virt 布局，RD 64KB + SGI 64KB）；唤醒序列（清 WAKER.ProcessorSleep + 轮询 ChildrenAsleep）复刻 gicv3.rs D4 的 per-core 版本（`Gicv3::init` 仅覆盖 CPU0）；SGI0 经 GICR IGROUPR0 + ISENABLER0 使能 + ICC_IGRPEN1/ICC_PMR per-core 各写（= smp/boot.rs 头注 D9） | gicv3.rs 未导出多核 GICR 公开接口与 IGROUPR0 偏移，按 GICv3 架构手册私有补全，interrupt/ 零改动 | 从核 SGI 接收路径就绪（含 PMR 补全 D19）；真机 DTB 解析 GICR 基址属后续版本 |
| D12 | per-CPU IPI 环不经 `PerCpu::ipi_queue` 存取，而由 ipi.rs 自持 `static IPI_RINGS: [IpiRing; MAX_CPUS]`（下标即 CpuId）——boot.rs 并行开发避免模块间耦合；`PerCpu::ipi_queue` 字段保留（结构不变），后续版本如需统一可在 boot 安装路径将 handler 引流至同一环（= smp/ipi.rs 头注 D4） | 模块解耦并行开发；SPSC 口径承 IpiRing 头注 | 本版本 handler 排水走 IPI_RINGS；`PerCpu::ipi_queue` 为结构占位（BOOT1 断言其空环初态） |
| D13 | 满环口径——push 失败累计 `dropped_count`，不再触发 SGI，`send_ipi` 仍返回 `Ok(())`；目标核越界（≥ MAX_CPUS）返回 `Err(InvalidArgument)`（= smp/ipi.rs 头注 D5） | 蓝图 §6.5「IPI 丢包处理」为可观测降级而非发送错误；不阻塞发送路径 | IPI3 锚定（丢两条计数 +2、SGI 触发停在 16）；`SmpManager::send_ipi` 蓝图 trait 签名无返回值，错误口径由被委托方内部承载 |
| D14 | host 发送桩——非 aarch64 无 CPU interface，`raise_sgi0` 记录 (target_list, msg 标签+载荷) 到静态原子槽 + 发送/广播计数，经 `host_sgi_sent_count`/`host_last_sgi_target_list`/`host_last_sgi_msg`/`host_broadcast_count` 供 host 测试观测；aarch64 真实 `msr icc_sgi1r_el1`（= smp/ipi.rs 头注 D6） | host 无可写 SGI 系统寄存器 | IPI1~IPI4/SMPT1/SMPT4/SMPT5 经桩观测编码正确性；SGI 真实到达由 QEMU 冒烟验证 |
| D15 | broadcast 自核除外——入环 0..MAX_CPUS 中除本核外全部核（在线过滤属 SmpManagerImpl 聚合层职责；未启动核的环消息驻留无害，启动后首次排水即清），SGIR TargetList = `0xFF & !self_bit`（= smp/ipi.rs 头注 D7） | 自核无需经 IPI 通知自身；QEMU `-smp 4` 下高 4 位无对应核 GIC 忽略，编码保留 8 核全集口径 | IPI2/SMPT4 锚定（自核环空、他核各一条、TargetList=0xFE） |
| D16 | `SchedulerImpl::schedule_on` 返回 `Option<Tid>`（任务书述及 `-> Tid`）——保持 v0.134.0「None = 无就绪线程」空队语义，`schedule()` 得以整函数委托 `schedule_on(0)` 且行为全等；idle 兜底由调用方处理（= sched/smp_sched.rs 头注 D5；同 v0.134.0 D7 返回类型决策口径） | 与 v0.134.0 trait 签名契约一致 | SMP5/SMPT2（cpu3 第 3 选 None）锚定空队语义；SCH1~SCH6 原样复跑零回归 |
| D17 | 绑核过滤实现口径——「桶内遍历选首个匹配 + 原地保序摘除」：未命中线程位置不动、同优先级 FIFO 相对顺序保持、队首命中 O(1)；弃用「弹出暂存回灌」（扰动 FIFO 顺序且同为 O(n)）（= sched/smp_sched.rs 头注 D6） | 定长环形 + 位图结构下保序与复杂度兼顾 | 扫描 O(非空桶数 + 扫过线程数)，摘除队首 O(1)/队中 O(桶长)；SMP4/SMPT2 锚定「过滤不扰动 FIFO 位置」 |
| D18 | `SmpManagerImpl` 的 boot/ipi/online 四方法委托 `crate::smp::{boot, ipi}` 真实实现与 SMP 启动状态（初版 host 桩字段随接线删除）：`boot_secondary` → boot.rs（PSCI + 轮询超时降级）、`send_ipi`/`broadcast_ipi` → ipi.rs、`cpu_online`/`online_count` → `SMP_BOOT.booted`（CPU0 主核恒在线 + 各从核 Acquire 计数）（= sched/smp_sched.rs 头注 D7） | 聚合层单一事实来源；host 下被委托方本身为桩/状态查询，语义与接线前一致 | SMPT3 经 SmpManager 公开接口端到端验证在线计数与降级；`pin_thread` 写 Tcb 字段（D5） |
| D19 | **（QEMU 排障①）**从核 `wake_redistributor_and_enable_sgi0` 追加 `msr icc_pmr_el1, 0xFF`——ICC_PMR_EL1 为 per-core 系统寄存器、复位值 0 屏蔽全部优先级，主核在 v0.133.0 §3.7 已放开但从核路径从未写过，SGI0（复位优先级 0，不满足 0 < PMR=0）在从核 CPU interface 被过滤，handler 永不运行（qemu_run.md §15 排障①；boot.rs 函数内注释同步） | v0.133.0 §3.7 主核接入层修正②同根因的 per-core 版本 | 修复后 SGI0 可呈递到从核 CPU interface；host 测试零影响（aarch64 门控） |
| D20 | **（QEMU 排障②）**`secondary_rust` 的 booted Release 置位后移至 `wake_redistributor_and_enable_sgi0` 之后——原在 GIC 使能前置位，主核见到 booted 即启动后续核并发送 IPI，SGI 于从核 ISENABLER0=0/IGROUPR0=Group0/PMR=0 复位态到达被丢弃（trace 证据 generate_sgi 先于本核 igrpen/pmr 写）；握手语义升级为「per-CPU 安装 + 本核 SGI 接收路径（GICR 唤醒/IGROUPR0/ISENABLER0/ICC_IGRPEN1/ICC_PMR）均就绪」（qemu_run.md §15 排障②；boot.rs 函数内注释同步） | 主核 `boot_secondary` 返回 ⇒ 从核可收 IPI，竞态关闭 | host 桩路径不受影响（aarch64 门控）；三核 `[smp] cpu<n> on` 打印交错佐证修复前并发重叠 |
| D21 | **（QEMU 排障③）**`ipi_handler` 打印前移至排水计数之前——主核以分类计数增量为 IPI 到达判据并随即打印 `[smp] ok`；若计数在先，主核打印与从核 `[smp] ipi ok` 打印并发交错（双核无锁字节写同一 UART，实测输出 `[[smsp]m ippi ]ok` 致两个串均不可 grep）；打印先完成后计数：主核观测到计数时本核打印在程序序上必然已结束，串口输出经既有计数同步通道天然串行化（= smp/ipi.rs 头注 D9 的 handler 部分；qemu_run.md §15 排障③） | 计数是主核等待的唯一同步点；最小修复不引入锁 | host 不打印（aarch64 门控），host 测试零影响；修复后一次通过九断言全过 |
| D-S1 | 从核接入入口桩 `smp_secondary_entry`（main.rs 本地适配，smp/、interrupt/ 零改动）：boot.rs `secondary_rust` 未装载从核 VBAR 且不开 IRQ 掩码（其 D9 明确「VBAR 装载与 daifclr 属接入层」）；PSCI 复位启动的从核 DAIF 全屏蔽、VBAR_EL1 复位值未定，缺失二者则 SGI0 永不作为 IRQ 被承接（wfi 可被屏蔽中断唤醒但 handler 不运行）；VBAR/PSTATE 为 per-core 状态主核无法代写，唯一可行路径是在从核取指流中注入——本桩复刻 boot.rs `secondary_entry` 独立栈装载序列（同一 `SECONDARY_STACK_BASE` 指针槽），随后 `bl smp_secondary_setup` 完成 install_vbar + daifclr，再 `bl secondary_rust` 回到既有从核流程（= main.rs 头注 D-S1） | per-core 系统寄存器只能本核写；复用既有设施不改 boot.rs | `boot_secondary` 的 entry 参数以本桩地址落地；从核 SGI0 经共享向量表 `irq_entry` → 全局 handler 表 → `ipi_handler` 正常承接 |
| D-S2 | x19 承载 cpu_id 跨 `bl smp_secondary_setup`（AAPCS64 callee-saved，Rust 函数自保；本桩为核复位后首条指令流，无调用方现场需要保持）；x9~x11 仅为临时寄存器（= main.rs 头注 D-S2） | 调用约定合规的最小寄存器分配 | 与 boot.rs 原入口（x9 暂存）同构；无功能差异 |

---

## 10. 蓝图 §7 验收对照

| 蓝图 §7 验收标准 | 实现证据 | 结论 |
|-----------------|---------|------|
| §7.1 功能：多核全部启动 + 绑核 | QEMU `-smp 4` 冒烟 4 核全在线：`[smp] cpu1 on`/`cpu2 on`/`cpu3 on` + `[smp] ipi ok` + `[smp] ok` 九断言首轮修复后一次通过；绑核过滤语义 host 全绿（SMP4 单核过滤 + SMPT2 两核过滤矩阵，过滤优先于优先级） | ✅ |
| §7.2 性能量化：IPI < 1μs，启动 < 10ms | SMPT6 host smoke：1000 次入环+排水实测均值 ns 级/op << 5μs 宽松上限（**host 用户态口径声明**，不代表目标机 ARM64 裸机性能，同 v0.134.0 scht7 / v0.131.0 T07 口径惯例）；从核启动 BOOT_TIMEOUT_ITERS 毫秒级轮询上限满足 < 10ms 观测口径（QEMU 实测三核启动在 10s 守护窗口内瞬时完成）；目标机性能测定属后续版本 | ✅（host smoke + 口径声明） |
| §7.3 安全：绑核 RTOS 不被其他核干扰 | `Tcb.pinned_cpu` 字段就位（D5），`schedule_on` 过滤语义保证绑核线程仅在本核可选、他核上下文即使优先级最高也被过滤（SMP4/SMPT2）；**真机执行隔离（per-CPU 调度表 + 多核加锁）属 v0.137.0 内存一致性之后议题**（D3 口径，本版本从核 wfi idle 不运行调度循环） | ✅（语义层；执行隔离后续版本） |
| §7.4 文档：smp_design.md | 本文档（[docs/kernel/smp_design.md](./smp_design.md)） | ✅ |
| §7.5 出口判定：多核全部启动 → v0.137.0 可启动 | 九关键串断言 PASS（含 `[smp] ok`）+ host 154 测试全绿零回归；多核环境（4 核在线/per-CPU 安装/IPI 环与分类计数/Acquire-Release 位序）全部就绪，为 v0.137.0 多核内存一致性（自旋锁、per-CPU、IPI 队列正确性依赖）提供验证基座 | ✅ |

---

## 11. 源码链接

| 模块 | 路径 |
|------|------|
| SMP 类型骨架与公开导出（CpuId/CpuInfo/IpiMessage/PerCpu/IpiRing/SmpBootState + CFG1~CFG3 单测） | [crates/kernel/microkernel/src/smp/mod.rs](../../crates/kernel/microkernel/src/smp/mod.rs) |
| SMP 常量配置（MAX_CPUS/IPI_RING_CAPACITY） | [crates/kernel/microkernel/src/smp/config.rs](../../crates/kernel/microkernel/src/smp/config.rs) |
| 多核启动（SMP_BOOT/boot_secondary/secondary_entry/secondary_rust/set_per_cpu/per_cpu/current_cpu + BOOT1~BOOT4 单测） | [crates/kernel/microkernel/src/smp/boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs) |
| 核间中断（IPI_RINGS/send_ipi/broadcast_ipi/ipi_handler/drain_ipi_ring/ipi_stats + IPI1~IPI4 单测） | [crates/kernel/microkernel/src/smp/ipi.rs](../../crates/kernel/microkernel/src/smp/ipi.rs) |
| 多核调度管理（SmpManager trait/SmpManagerImpl/pin_thread + SMP1~SMP5 单测） | [crates/kernel/microkernel/src/sched/smp_sched.rs](../../crates/kernel/microkernel/src/sched/smp_sched.rs) |
| Tcb 追加 pinned_cpu 字段（v0.134.0 TCB 抽象） | [crates/kernel/microkernel/src/sched/tcb.rs](../../crates/kernel/microkernel/src/sched/tcb.rs) |
| 绑核过滤调度（SchedulerImpl::schedule_on；schedule() 委托） | [crates/kernel/microkernel/src/sched/scheduler.rs](../../crates/kernel/microkernel/src/sched/scheduler.rs) |
| 内核启动期接入（§3.10 smp_smoke 段 + smp_secondary_entry 接入桩 + smp_secondary_setup + smp_stack_alloc） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试 SMPT1~SMPT6 | [crates/kernel/microkernel/tests/smp_test.rs](../../crates/kernel/microkernel/tests/smp_test.rs) |
| QEMU 测试脚本（-smp 4 + 第九条断言） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| 内核统一错误码 KernelError（InvalidArgument/Timeout/BadState/NotSupported） | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs) |
| QEMU 实测记录（v0.136.0 串口全文/九断言/三轮排障） | [docs/boot/qemu_run.md](../boot/qemu_run.md) §15 |
| 线程调度设计文档（v0.134.0，ReadyQueue/SchedulerImpl/Tcb 来源） | [docs/kernel/scheduler_design.md](./scheduler_design.md) |
| 分区调度设计文档（v0.135.0，前序版本） | [docs/kernel/partition_sched.md](./partition_sched.md) |
| 中断管理设计文档（v0.133.0，GICv3/向量表/handler 注册通道） | [docs/kernel/interrupt_design.md](./interrupt_design.md) |
| VMM 设计文档（v0.131.0，identity map 口径来源） | [docs/kernel/vmm_design.md](./vmm_design.md) |
