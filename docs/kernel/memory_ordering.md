# EnerOS v0.137.0 多核内存一致性设计（SpinLock 自旋锁 / 原子封装 / 内存屏障）

> 版本：v0.137.0
> 适用范围：EnerOS 自研内核多核内存一致性（ARM64 LDAXR/STLXR 排他指令自旋锁 + WFE/SEV 低功耗等待 + DAIF 关中断保护 / `AtomicPtr` SeqCst 薄封装 / dmb/dsb/isb 屏障接口；QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` / host 测试）
> 蓝图依据：`蓝图/phase3.md` §v0.137.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> 配套文档：[smp_design.md](./smp_design.md)（v0.136.0 SMP 多核启动/IPI，前序版本与本版本屏障点位来源）、[scheduler_design.md](./scheduler_design.md)（v0.134.0 线程调度）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 中断管理，SGI handler 注册通道）、[../boot/qemu_run.md](../boot/qemu_run.md) §16（v0.137.0 QEMU 实测记录，首轮一次 PASS）

---

## 1. 概述

**核心一句话**：实现多核缓存一致性管理、原子操作封装与内存屏障，确保多核并发数据正确（蓝图 §1 原样口径）。

- **Phase 定位**：P3-C 中断调度 SMP 线**第 5 版**（v0.133.0 ~ v0.138.0）；v3.0 新增基础服务，**v0.136.0 SMP 的必要补充**——v0.136.0 已交付多核启动、IPI 与绑核调度，但内核尚无任何多核同步原语，自旋锁、per-CPU 数据与 IPI 消息队列的正确性此前依赖手写原子量与易错的屏障点位；本版本提供统一的 SpinLock（LDAXR/STLXR）、原子操作封装与 dmb/dsb/isb 屏障接口，**为 v0.138.0 确定性调度提供正确并发基础**（蓝图 §1 / §5.5）。
- **业务价值**：多核环境下，自旋锁、per-CPU 数据、IPI 消息队列的正确性依赖内存一致性（蓝图 §1）。
- **出口关联**：自研内核"多核正确性"保障（蓝图 §1）。
- **ADR-0001 口径**：依据 ADR-0001，Phase 3 主路径为 seL4 深度定制（方案 A），seL4 已具备多核内存一致性保证，方案 A 下复用 seL4 已有实现；本实现属自研内核研究/fallback 线的功能完整性证据，非生产路径（蓝图 §1 裁剪口径原样）。
- **依赖关系**：前序版本 v0.136.0 SMP 多核可用（蓝图 §2）；外部依赖无；假设 ARM64 弱内存模型需显式屏障（蓝图 §2）；并发错误阻塞 v0.138.0（蓝图 §2）。

交付物落位（仓库规范 §2.3.1 子系统归口，偏差 D1）：

| 交付物 | 路径 |
|--------|------|
| sync 子系统根部（公开导出 + D1~D6 决策记录头注） | [crates/kernel/microkernel/src/sync/mod.rs](../../crates/kernel/microkernel/src/sync/mod.rs) |
| 内存屏障四函数（dmb/dsb/isb/dmb_ish + BAR1~BAR3 单测） | [crates/kernel/microkernel/src/sync/barrier.rs](../../crates/kernel/microkernel/src/sync/barrier.rs) |
| 多核自旋锁（SpinLock/SpinLockGuard + SPL1~SPL6 单测） | [crates/kernel/microkernel/src/sync/spinlock.rs](../../crates/kernel/microkernel/src/sync/spinlock.rs) |
| 原子操作封装（AtomicPtr 薄封装 + ATM1~ATM3 单测） | [crates/kernel/microkernel/src/sync/atomic.rs](../../crates/kernel/microkernel/src/sync/atomic.rs) |
| 内核启动期接入（§3.11 `sync_smoke` 段 + `sync_sgi_handler`，SGI1 触发四核竞争） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试（CONC1~CONC6） | [crates/kernel/microkernel/tests/concurrency_test.rs](../../crates/kernel/microkernel/tests/concurrency_test.rs) |
| QEMU 测试环境（第十条断言 `[sync] ok`，既有九条原样） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| 设计文档（蓝图 §3 文档类 / §7.4 验收） | [docs/kernel/memory_ordering.md](./memory_ordering.md)（本文档） |

**接口一览**（蓝图 §3 接口类原样签名，[sync/mod.rs](../../crates/kernel/microkernel/src/sync/mod.rs) 公开导出）：

| 项 | 定义 | 说明 |
|----|------|------|
| `SpinLock<T>` | struct 两字段 + debug owner 诊断字段 | `locked: AtomicU32` + `data: UnsafeCell<T>`（蓝图 §4.1 原样）；debug 构建追加 `owner: AtomicU32`（D7） |
| `SpinLockGuard<'a, T>` | struct 两字段 | `lock: &'a SpinLock<T>`（蓝图 §4.1 原样）+ `daif: usize` 快照字段（蓝图 §4.5 `_daif` 显式化） |
| `lock / try_lock` | `fn lock(&self) -> SpinLockGuard<'_, T>` / `fn try_lock(&self) -> Option<SpinLockGuard<'_, T>>` | 蓝图 §4.2 接口；try_lock 为非阻塞单次尝试变体 |
| `AtomicPtr<T>` | `core::sync::atomic::AtomicPtr<T>` 薄封装 | load/store/swap/compare_exchange 默认 SeqCst（蓝图「顺序一致性原子」口径，D5） |
| `dmb / dsb / isb / dmb_ish` | `pub fn dmb(); dsb(); isb(); dmb_ish();` | 蓝图 §3 原样四名；aarch64 对应同名指令，host 映射 fence/compiler_fence（D2） |

---

## 2. ARM64 弱内存模型速览

面向读者的最小必要知识（详细定义以 ARM ARM 为准）：

- **乱序执行与访存重排**：ARM64 为弱内存模型（weakly ordered）。CPU 为提高流水线效率，允许在**不改变单核程序语义**的前提下重排内存访问的实际发出/完成顺序；编译器同样会重排。单核下不可察觉，多核下另一核可能观察到与本核程序序不一致的中间状态。
- **缓存一致性 vs 内存序**：MESI 类一致性协议（多拷贝原子性）保证"一个地址的写最终被所有核看到同一值"，但**不保证不同地址的写被观察到的先后顺序**。例如"先写数据、再写标志"这一发布序列，另一核可能先看到新标志、再看到旧数据——必须靠屏障或 acquire/release 语义建立跨核的 happens-before。
- **观察者（observer）**：内存模型中每个能发起/观察内存访问的主体（CPU 核、DMA 设备等）都是观察者；屏障的**共享域**（shareability domain）圈定屏障生效的观察者集合——inner shareable（同一簇内各核）、outer shareable、全系统（sy）。域越窄开销越小。
- **显式屏障必要性**：ARM64 上跨核可见性不能依赖默认程序序，必须由软件在正确的位置插入 dmb/dsb/isb，或使用自带 acquire/release 语义的原子指令（LDAXR/STLXR/STLR 等）。蓝图 §8.1 风险「ARM64 内存模型细节多，屏障位置易错」即指此——屏障放少了丢正确性，放多了丢性能，放错位置两者皆失。
- **对照（蓝图 §8.4）**：x86 为强内存模型（TSO），多数重排不会发生（"x86 屏障可省但代码应保留"）——host 单测在弱序语义上的判别力有限，屏障接入点正确性的最终验证依赖 aarch64 目标机实测（见 §8 测试矩阵 CONC4 口径声明）。

---

## 3. LDAXR/STLXR acquire-release 语义

### 3.1 排他监视与指令语义

ARM64 提供两类原子原语：LSE 大系统扩展原子指令（LDADD/CAS 等）与**排他（exclusive）加载/存储对**。本项目 SpinLock 取后者：

| 指令 | 语义 | 内存序 |
|------|------|--------|
| `LDAXR Wt, [Xn]` | 排他加载：读字并打开本核对该地址的**排他监视器** | **Acquire**：本指令之后的内存访问不得被观察到先于本指令 |
| `STLXR Ws, Wt, [Xn]` | 排他存储：仅当监视器仍有效（期间无他核写同地址/无上下文切换清监视）才真正写入；Ws 回执 0=成功 | **Release**：本指令之前的内存访问必须先于本指令完成对外可见 |
| `STLR Wt, [Xn]` | 释放存储（非排他） | **Release**：单向释放屏障语义的单指令 |
| `CLREX` | 清除本核排他监视器 | —（try_lock 失败路径收尾用，D4） |

**与 CAS 的对应**：LDAXR/STLXR 重试环在语义上等价于 compare_exchange——LDAXR 读回旧值判定期望（locked==0），STLXR 在"期望仍成立（监视器未被破坏）"时提交新值。host（x86）无排他指令，D2 双实现直接以 `compare_exchange_weak(0, 1, Acquire, Relaxed)` 承载同一语义（成功 Acquire 序对应 LDAXR+dmb ish 的语义上界，失败 Relaxed 不引入额外序约束）。

**WFE/SEV 低功耗配对**：锁被持有时等待核执行 `WFE`（Wait For Event）停核等待事件寄存器置位，而非空转烧电；持锁核释放路径的 `SEV`（Send Event）向簇内全核送达事件唤醒等待者。检查-等待序列无丢失唤醒窗口：LDAXR 见锁占用到 WFE 之间到达的 SEV 已置位事件寄存器，WFE 立即返回重试。

### 3.2 蓝图 §4.3 核心算法（Markdown 化描述）

蓝图 §4.3 Mermaid 流程图的等效文字流程：

```
lock（获取）：
  A  lock 入口（先 save_daif 关 IRQ，见 §5）
  B  LDAXR 读 locked 锁字
  C  locked == 0 ?
       否 ──► D  WFE 低功耗等待事件 ──► 回 B 重试
       是 ──► E  STLXR 尝试写 1 -> locked
  F  STLXR 成功 ?
       否（竞争失败，监视器被破坏）──► 回 B 重试
       是 ──► G  DMB ISH（inner shareable 屏障，保证后续临界区读
                    不被重排到锁获取之前）
           ──► H  返回 SpinLockGuard

unlock（释放，Guard Drop 触发）：
  I  unlock 入口
  J  DMB ISH（保证临界区写入先于锁释放对其他核可见）
  K  STLR 写 0 -> locked（release 语义单指令）
  L  SEV 唤醒 WFE 等待者
  （随后 restore_daif 恢复 IRQ——Drop 顺序见 §5）
```

aarch64 实现落位（[spinlock.rs](../../crates/kernel/microkernel/src/sync/spinlock.rs)，D4 重写后的合法内联汇编）：

- `acquire()`：数字标号 2/3/4 局部循环，`ldaxr` → `cbnz`（已持有→`wfe`）→ `stlxr` → `cbnz`（失败重试）；输出寄存器 tmp/st 分绑（蓝图原稿重复 `out(reg)` 绑定同一变量的修正，D4）；
- `try_acquire()`：一轮 `ldaxr`/`stlxr`，失败路径 `clrex` 清理排他监视器（避免残留监视状态干扰后续排他操作，D4）；
- `release()`：`stlr 0` + `sev`（蓝图 §4.3 K/L 步原样）。

---

## 4. 屏障选择决策树

### 4.1 四屏障语义对比

| 屏障 | 全称 | 语义 | 等待完成? | 典型场景 |
|------|------|------|-----------|---------|
| `dmb sy` | Data Memory Barrier（全系统域） | 屏障前的内存访问**先于**屏障后的访问被全系统观察者看到（程序序约束） | 否（仅排序） | 跨核/跨设备共享数据的通用全屏障 |
| `dmb ish` | Data Memory Barrier（inner shareable 域） | 同上，但仅约束同一 inner shareable 域（同簇各核）内的观察者 | 否（仅排序） | 多核（同簇）共享数据：自旋锁获取/释放、消息传递发布 |
| `dsb sy` | Data Synchronization Barrier（全系统域） | 屏障前的内存访问必须**完成**（到达终点）后才执行后续指令 | 是 | 外设/GIC MMIO 寄存器写后等待生效、缓存/TLB 维护收口 |
| `isb` | Instruction Synchronization Barrier | 冲刷取指流水线，保证后续取指看到屏障前的上下文变更（系统寄存器写等） | 是（取指侧） | VBAR/系统寄存器写后、自修改代码后 |

### 4.2 选择决策树

```
需要约束的是什么？
├─ 取指/上下文变更（VBAR、系统寄存器写后新配置生效）
│    └─► isb
├─ 内存访问必须"完成"（MMIO 寄存器写到达设备后才继续，
│    如 GIC 使能写 → 触发 SGI 之间）
│    └─► dsb sy
└─ 仅需内存访问"排序"（多核间可见性顺序）
     ├─ 观察者在全系统域（DMA/外设参与） ──► dmb sy
     └─ 观察者仅同簇多核（自旋锁、消息环、done 标志）──► dmb ish（开销更小）
```

### 4.3 本项目选择表

| 点位 | 屏障 | 理由 |
|------|------|------|
| SpinLock 获取后（蓝图 §4.3 G 步） | `dmb ish` | 临界区读不得重排到锁获取前；观察者仅同簇核 |
| SpinLock 释放前（蓝图 §4.3 J 步） | `dmb ish` | 临界区写必须先于锁释放对同簇核可见 |
| sync_smoke done 标志发布/收口 | `dmb ish` | 消息传递序：计数器写 → done 发布；done 收齐 → 计数器读回 |
| sync_smoke GICR 使能写 → SGI 触发之间 | `dsb sy` | MMIO 写必须到达 GIC 寄存器后才产生 SGI，关闭使能-触发竞态（D-S3a） |
| VBAR_EL1 装载后 | `isb` | 保证后续取指看到新向量表基址（vector.rs） |
| GIC init 寄存器序列收口 | `dsb sy` | 外设寄存器写完成语义（gicv3.rs） |
| smp boot 握手 / IpiRing 位序 | `Ordering::Release/Acquire` 原子位序 | 单变量发布-订阅场景，acquire/release 语义足够且开销低于显式屏障（本版本不迁移既有代码，见 §6） |

**host 映射口径（D2）**：host（x86）为强内存模型（蓝图 §8.4），`dmb`/`dsb`/`dmb_ish` 映射 `fence(Ordering::SeqCst)`（最强序，语义上界），`isb` 映射 `compiler_fence(Ordering::SeqCst)`（host 无取指重排概念，仅需编译器序）——保证 host 单测可编译可运行，两路径函数签名一致、调用方无感。

---

## 5. SpinLock 设计

### 5.1 字段布局

蓝图 §4.1 原样两字段 + debug 诊断字段（D7）与 Guard 的 daif 快照字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `SpinLock.locked` | `AtomicU32` | 锁字（0=空闲，1=持有；4B 对齐天然满足排他指令对齐要求） |
| `SpinLock.owner` | `AtomicU32`（`#[cfg(debug_assertions)]`） | 持有者标识（D7 诊断；0=空闲；release 构建整段剔除、零开销） |
| `SpinLock.data` | `UnsafeCell<T>` | 受保护数据（仅 Guard 经 Deref/DerefMut 可访问） |
| `SpinLockGuard.lock` | `&'a SpinLock<T>` | 所属锁（蓝图 §4.1 原样字段） |
| `SpinLockGuard.daif` | `usize` | 获取锁时的 DAIF 快照（aarch64 真值；host 恒 0，D3 桩） |

`unsafe impl<T: Send> Send/Sync for SpinLock<T>`：所有共享访问均经 `locked` 原子字排他保护，仅持锁上下文能触及 `data`（字段私有、无绕过路径），`T: Send` 保证数据可跨核/跨线程转移所有权。

### 5.2 lock / try_lock / unlock 流程

- **`lock()`**：`save_daif`（读 DAIF 快照 + `msr daifset, #2` 关本核 IRQ）→ debug owner 重复 lock 断言（D7，见 §5.4）→ `acquire()` 排他获取（aarch64 LDAXR/STLXR+WFE 循环 / host Acquire CAS 自旋 + `spin_loop()` 提示）→ `dmb ish`（蓝图 §4.3 G 步）→ 写 owner → 返回 Guard。**先关中断再竞争锁**：持锁期间本核不被 IRQ 抢占（蓝图 §4.4「中断中不可持有自旋锁（关中断保护）」）。
- **`try_lock()`**：`save_daif` → `try_acquire()` 单次尝试 → 成功走同 lock 的 G 步屏障返回 `Some(Guard)`；失败 `restore_daif` 恢复进入时中断状态（对称口径）返回 `None`——不自旋不死锁（spec「try_lock 语义」Scenario）。
- **`unlock()`（私有，仅 Guard Drop 调用）**：debug 清 owner → `dmb ish`（J 步，临界区写入先于锁释放可见）→ `release()`（aarch64 `stlr 0`+`sev` / host Release store）。

### 5.3 DAIF 关中断保护与 Drop 顺序

蓝图 §8.5 坑点「Drop 顺序：先释放锁再开中断」的落位（`SpinLockGuard::drop`）：

```
① self.lock.unlock()     —— dmb ish → STLR 0 → SEV（锁释放完成）
② restore_daif(self.daif) —— 恢复进入时的 PSTATE 中断屏蔽位
```

锁释放完成前中断保持关闭，避免「锁已释放但快照未恢复」窗口被 IRQ 重入破坏。IRQ 上下文持锁为**重入安全**：异常进入时硬件已置 PSTATE.I，`save_daif` 快照含 I=1、Drop 恢复原态，快照-屏蔽对可嵌套（sync_smoke 从核 handler 持锁即依赖此语义，见 §7）。

### 5.4 debug owner 诊断生效形态（D7）

蓝图 §4.4「自旋锁不可重入，重复 lock 死锁（开发期断言检测）」的实现：`owner: AtomicU32` 记录持有者标识，`lock()` 进入时若 `owner == 自身标识` 则 `debug_assert!` 触发重复 lock 诊断。**诊断生效范围仅两种形态**：

| 构建形态 | 标识来源 | 诊断 |
|---------|---------|------|
| aarch64 目标机 | `MPIDR_EL1 & 0xFF + 1`（+1 避开 0 空闲值） | 生效 |
| host + cfg(test)（crate 内单测） | `std::thread::current().id()` 哈希 `\| 1`（保证非零） | 生效 |
| host + 非 test debug 构建（cargo 集成测试 tests/*.rs 链接的 lib 形态） | 无 TLS 可用（no_std lib 不链 std），`current_owner_id()` 返回 0 表「诊断不可用」，`lock()` 跳过断言 | **关闭**（D7 修复；此前返回常量 1 致多线程并发 `lock()` 误报重复 lock，即 D8 根因——误报大于收益，故关闭） |

release 构建整段 cfg 剔除、零开销，按蓝图口径重复 lock 即死锁（调用方契约）。

---

## 6. 本项目屏障点位表

本项目实际使用的屏障/内存序点位（既有点位本版本只引用不改动；sync 子系统为后续统一点位提供原语）：

| 点位 | 文件 | 机制 | 语义 |
|------|------|------|------|
| 从核启动握手（entry/stack/booted 四连发布；booted 轮询） | [src/smp/boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs) | `Ordering::Release` 存储 / `Ordering::Acquire` 轮询 | 主核 Release 发布先于 `hvc #0`；观察到从核 Release 置位 ⇒ per-CPU 安装 + SGI 接收路径就绪完整可见（v0.136.0 D20 握手语义） |
| per-CPU 安装标志 | [src/smp/boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs) | Release 置 installed / Acquire 检查 | 槽位内容先于 installed 标志对观察者可见（happens-before 配对） |
| IpiRing 入环/出环位序 | [src/smp/ipi.rs](../../crates/kernel/microkernel/src/smp/ipi.rs) | push：读 head Acquire → 写槽 → tail+1 Release；pop：读 tail Acquire → 读槽 → head+1 Release | SPSC 免锁：槽位写入先于 tail 推进对消费者可见；head 推进先于槽位复用对生产者可见 |
| SpinLock 获取/释放 | [src/sync/spinlock.rs](../../crates/kernel/microkernel/src/sync/spinlock.rs) | LDAXR/STLXR 自带 acquire/release + 获取后/释放前 `dmb ish` | 蓝图 §4.3 G/J 步原样（本版本交付） |
| GIC init / GICR 寄存器序列收口 | [src/interrupt/gicv3.rs](../../crates/kernel/microkernel/src/interrupt/gicv3.rs) | 模块私有 `dsb sy` | 外设 MMIO 写完成语义（配置写到达 GIC 后后续操作才生效） |
| VBAR_EL1 装载 | [src/interrupt/vector.rs](../../crates/kernel/microkernel/src/interrupt/vector.rs) | `isb` | 保证后续取指看到新向量表基址 |
| sync_smoke GICR 代写 → SGI1 触发 | [src/main.rs](../../crates/kernel/microkernel/src/main.rs) | `sync::barrier::dsb()` | 使能写到达 GIC 后才触发 SGI，关闭使能-触发竞态（D-S3a；本版本 sync 屏障**首个真机自用点**） |
| sync_smoke done 标志可见性 | [src/main.rs](../../crates/kernel/microkernel/src/main.rs) | handler 侧 `dmb ish` → Relaxed 存 done；主核收齐后 `dmb ish` → 经 lock 读计数器 | 消息传递序：本核全部临界区写先于 done 对主核可见；计数器读回经锁 acquire 语义双保险（本版本内存一致性主题自用点） |

---

## 7. sync_smoke 多核竞争冒烟设计

main.rs §3.11（v0.137.0 Task 5，surgical 追加；既有九类流程与打印零改动）的真机验证链路：

```
主核（rust_main / sync_smoke）                    从核（IRQ 上下文）
─────────────────────────────────────         ─────────────────────────
① register_irq_handler(1, sync_sgi_handler)
   （D-S3 分工：SGI0 已被 IPI 占用，SGI 为
     banked 中断全核共享分发表，取 SGI1 专用）
② 主核 GICR 使能 SGI1（IGROUPR0 |= bit1
   读改写保留 PPI30/SGI0；ISENABLER0 bit1）
③ 主核代写每个在线从核 GICR 使能 SGI1
   （D-S3a；GICR 为 MMIO，0x080A_0000 +
     cpu×0x20000 + 0x10000 SGI 帧）
④ dsb sy 收口（使能写到达 GIC）
⑤ icc_sgi1r_el1（INTID=1，TargetList=在线
   从核位图，smp_smoke 返回值回传）
──────────────────────────────────────► ⑥ 从核 sync_sgi_handler：
                                          对 BSS SpinLock<u64> 计数器
                                          执行 10,000 次 lock/+=1/unlock
                                          → dmb ish → 置本核 done
                                          （不打印串口，防交错）
⑦ 主核自身同次数递增（D-S3b，4 核真实并发竞争）
   → dmb ish → 置 done[0]
⑧ 轮询各从核 done（超时兜底，缺失计入
   missing 位图）→ dmb ish
⑨ 经 lock 读回计数器，校验总数 ==
   在线核数 × 10,000 → 打印 [sync] ok
   （不符/超时打印 [sync] err count=<n>
     expect=<n> missing=0x<m>，不挂死）
```

设计要点：

| 决策 | 内容 | 锚点 |
|------|------|------|
| SGI1 分工（D-S3） | SGI0 已被 v0.136.0 IPI 通道占用（`IPI_SGI_ID`=0），SGI 空间 0~15 取 INTID=1 为 sync 专用，两通道互不干扰；从核参与机制 = IPI 触发式（复用已验证中断链路，对比备选「从核驻留参与」改动面最小） | main.rs `sync_smoke` 文档注释 |
| IRQ 上下文持锁语义 | handler 运行于异常上下文（硬件已置 PSTATE.I），`SpinLock::lock` 的 save_daif/daifset 为重入安全快照对；自旋 WFE 由他核 unlock 的 STLR+SEV 唤醒（SEV 跨核送达事件寄存器，检查-等待无丢失唤醒窗口）；持有者必然推进至 unlock，不会永久挂起 | main.rs `sync_sgi_handler` 文档注释 |
| GICR 主核代写（D-S3a） | boot.rs 唤醒序列仅置 SGI0（其 IGROUPR0 全写 1 覆盖语义 = 仅 bit0 归 Group1），SGI1 须补 IGROUPR0 |= bit1（读改写）+ ISENABLER0 bit1；GICR 为内存映射寄存器主核可代写，写后 `dsb` 收口再触发，无需改 boot.rs | 偏差表 D-S3a |
| 主核参与竞争（D-S3b） | 主核不经 SGI1（TargetList 仅含从核），在主流程上下文执行同次数递增；主核关本核 IRQ 期间从核 handler 的持锁/等待不受影响（unlock 的 SEV 跨核送达），无死锁 | 偏差表 D-S3b |
| done 可见性 | handler 侧 `dmb ish` → Relaxed 存 done；主核收齐后 `dmb ish` → 经 lock 读计数器（acquire 双保险）——本版本内存一致性主题自用点 | §6 点位表 |
| handler 不打印 | 多核无锁并发字节写同一 UART 会交错（v0.136.0 `[smp] ipi ok` 交错教训），完成观测经 done 原子标志 + 主核校验收口 | qemu_run.md §16 |
| 降级口径 | 无在线从核（targets=0，对应 `[smp] err` 路径）时不发 SGI1，期望值退化为 1×每核次数，单核锁路径自验仍打印 `[sync] ok`（与 smp_smoke 蓝图 §4.4「警告但继续」降级哲学一致） | main.rs `sync_smoke` 文档注释 |

---

## 8. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`（lib 内嵌单测 + tests/ 集成测试）。各用例仅使用自身私有的局部锁/函数内静态量，互不共享全局态，cargo test 默认并行下结果确定（不同于 smp_test.rs 的 TEST_LOCK 口径——本文件无跨用例共享态）。

### 8.1 单元测试（sync/barrier.rs 内嵌，BAR1~BAR3）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| BAR1 | `bar1_four_barriers_callable_on_host` | 四屏障函数 host 可调用、不 panic（cfg 门控映射 fence/compiler_fence） | PASS |
| BAR2 | `bar2_fence_semantics_smoke` | message passing 最小骨架：写数据→屏障→发布标志 / 见标志→屏障→读数据不得见旧值（host SeqCst fence 口径下恒成立，验证屏障接入点位置正确） | PASS |
| BAR3 | `bar3_repeated_calls_idempotent` | 连续调用幂等：同一屏障多次调用不产生额外效果、屏障间任意交错合法 | PASS |

### 8.2 单元测试（sync/spinlock.rs 内嵌，SPL1~SPL6）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| SPL1 | `spl1_lock_unlock_roundtrip` | lock/unlock 往返：获取、写入、Drop 后再次获取读到新值 | PASS |
| SPL2 | `spl2_try_lock_contended_and_after_release` | try_lock 语义：持锁期间返回 None；释放后可获取 | PASS |
| SPL3 | `spl3_guard_deref_mut_visible_across_critical_sections` | Guard DerefMut 写入跨临界区可见 | PASS |
| SPL4 | `spl4_drop_releases_lock` | Drop 自动释放：Guard 离开作用域后 try_lock 立即成功 | PASS |
| SPL5 | `spl5_debug_reentrant_lock_diagnosed` | debug 重复 lock 诊断（catch_unwind 捕获 debug_assert panic；诊断未破坏锁状态，panic 发生于获取前） | PASS |
| SPL6 | `spl6_eight_threads_contended_counter` | 8 线程 × 10,000 次竞争计数，终值恒等于 80,000（无丢失更新） | PASS |

### 8.3 单元测试（sync/atomic.rs 内嵌，ATM1~ATM3）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| ATM1 | `atm1_new_load_store_roundtrip` | new/load/store 往返 + 显式序变体（默认序 SeqCst 按实现固定传参的编译期语义声明） | PASS |
| ATM2 | `atm2_swap_returns_old` | swap 返回旧值并写入新值（双向往返） | PASS |
| ATM3 | `atm3_compare_exchange_ok_and_err` | compare_exchange 成功路径（Ok 旧值、内存更新）与失败路径（Err 当前值、内存不变） | PASS |

### 8.4 集成测试（tests/concurrency_test.rs，CONC1~CONC6，仅以公开 API 端到端验证）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| CONC1 | `conc1_spinlock_contended_counter` | 有锁路径竞争计数（蓝图 §6.2）：4 std 线程 × 250,000 次 `lock()` 递增，终值恒等于 1,000,000（D8 已修复后回退纯 lock 获取） | PASS |
| CONC2 | `conc2_lockfree_atomic_fetch_add_control` | 无锁对照：`AtomicU64::fetch_add` 同规模递增终值同为 1,000,000——互斥临界区 vs 原子 RMW 两种同步机制并列对照 | PASS |
| CONC3 | `conc3_try_lock_conservation` | try_lock 竞争守恒：4 线程 × 100,000 次尝试，成功次数 × 每次增量(3) == 受保护计数终值，且成功次数 > 0 | PASS |
| CONC4 | `conc4_barrier_message_passing` | 屏障 message passing：100,000 轮「写 data→`dmb_ish`→Release 发 flag / Acquire 读 flag→`dmb_ish`→读 data」，不得见重排旧值（host x86 强序下恒通过属预期，判别力在 aarch64；host 侧验证接入点位置与编译链接） | PASS |
| CONC5 | `conc5_uncontended_lock_unlock_perf_smoke` | 无竞争性能 smoke：单线程 lock/unlock 往返 100,000 次计时，实测均值 **13 ns/op**（host x86 用户态口径，**仅记录非验收**——蓝图 §6.3「无竞争 lock < 50ns」为目标机 aarch64 裸机验收口径，同 smp_test.rs SMPT6、sched_test.rs SCHT7 口径惯例）；断言 < 1μs 极宽松上限防算法退化 | PASS |
| CONC6 | `conc6_fault_injection_no_barrier_observation`（`#[ignore]`） | 故障注入对照（spec D6 / 蓝图 §6.5）：故意省略屏障的 message passing，统计「见 flag 第 i 轮但 data 滞后」的重排观测次数；仅 eprintln 观测不断言（host x86 强序预期 0 作对照基线，弱序目标机可能非零——屏障必要性的实验证据）；默认不跑，`cargo test -- --ignored` 才执行 | ignore（口径内） |

### 8.5 QEMU 冒烟（main.rs §3.11 段 + [qemu_run.md](../boot/qemu_run.md) §16）

串口输出关键行（原样收录，WSL2 QEMU 8.2.2，`-M virt,gic-version=3 -smp 4`）：

```text
[smp] cpu1 on
[smp] cpu2 on
[smp] cpu3 on
[smp] ipi ok
[smp] ok
[sync] ok
```

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **十**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok` / `[sync] ok`；v0.137.0 起追加第十条断言 `grep "[sync] ok"`，既有九条原样），**首轮一次通过，无排障记录** |
| 端到端总耗时 | 14s（构建 3.60s + QEMU timeout 10s 守护窗口） |
| 竞争规模 | 4 核并发 40,000 次 lock/unlock 往返（主核主流程 + 三从核 IRQ handler 各 10,000 次），计数终值精确等于 4×10,000 |
| 回归 | host 测试全过零回归（Task 2~5 交付时）；`cargo fmt` / `cargo clippy -D warnings` 干净 |

**四项高风险点均未触发**（qemu_run.md §16 分析）：① LDAXR/STLXR 排他监视地址（`locked` 为 `AtomicU32`，4B 对齐天然满足）；② WFE/SEV 配对（未出现挂死）；③ DAIF 嵌套（handler 上下文重入安全快照对，未出现中断状态破坏）；④ 从核 GICR bit1 使能时序（主核代写 + `dsb` 收口后才发 SGI1，三从核 handler 全部到达，`[sync] err` 分支未触发）。

蓝图 §6 其余条目对照：

| 蓝图条目 | 覆盖情况 |
|---------|---------|
| §6.1 单元测试「自旋锁 lock/unlock 往返、try_lock 逻辑」 | SPL1/SPL4（往返）+ SPL2（try_lock）+ CONC1/CONC3 集成复验 |
| §6.2 集成测试「多核并发计数器（无锁 vs 有锁）；4 核 × 100 万次压力」 | CONC1（有锁）/CONC2（无锁）对照 + QEMU sync_smoke 4 核真机竞争（40,000 次排他获取）；host 百万次级由 CONC1 4×250,000=1,000,000 次承载 |
| §6.3 性能基准「无竞争 lock < 50ns；有竞争 < 500ns」 | CONC5 host smoke 实测 13 ns/op（host 用户态口径声明，非验收；目标机 aarch64 裸机性能测定属后续版本） |
| §6.4 回归「SMP 功能不受影响」 | QEMU 冒烟既有九断言先于 `[sync] ok` 全部通过；sync/ 新建、mem//interrupt//smp//sched/ 既有文件零改动（spec 零改动承诺） |
| §6.5 故障注入「故意省略屏障验证数据错乱（对照实验）」 | CONC6（#[ignore] + 文档形式保留，spec D6 防 CI 偶发） |
| §6.6 GPU 规则 | 不适用（内存一致性，蓝图原样口径） |

---

## 9. 蓝图偏差表（D1~D8 + D-S3/D-S3a/D-S3b）

> 文档级编号与源码头注编号一一对应：sync/mod.rs 头注 D1~D6 → 本文档同号；spinlock.rs 头注 D7 → 本文档同号；concurrency_test.rs 头注 D8 → 本文档同号；main.rs 头注/sync_smoke 文档注释 D-S3/D-S3a/D-S3b → 本文档同号（v0.136.0 smp_design.md 的 D-S1/D-S2 为前一版本接入桩决策，不在本表）。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D1 | 蓝图交付物 `kernel/src/sync/*` → 落位 `crates/kernel/microkernel/src/sync/*`（mod/barrier/spinlock/atomic 四文件）；测试 `kernel/tests/concurrency_test.rs` → `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/memory_ordering.md` 同蓝图 | 仓库规范 §2.3.1 强制子系统归口（`crates/<subsystem>/`），禁止根目录 crate | 仅路径差异，交付物一一对应 |
| D2 | 双实现：aarch64 真实 LDAXR/STLXR/WFE/SEV/STLR 与屏障指令汇编；host（x86 单测）SpinLock 用 `AtomicU32::compare_exchange` 等价路径、屏障映射 `fence`/`compiler_fence`（x86 无 LDAXR；蓝图 §8.4「x86 屏障可省但代码应保留」口径） | host 无排他指令且为强内存模型；host 单测需可编译可运行 | 两路径函数签名一致、对外语义一致、调用方无感；aarch64 真实路径由 QEMU 冒烟验证 |
| D3 | DAIF host 桩：`save_daif`/`restore_daif` 仅 aarch64 有真实语义（`mrs daif` + `msr daifset, #2` 关 IRQ / `msr daif` 恢复）；host 为 no-op（返回 0），Guard 的 daif 字段 host 恒 0 | host 单测无 PSTATE/DAIF 概念，无需关中断 | host 路径无中断保护语义（单测无 IRQ 上下文）；aarch64 语义完整 |
| D4 | 蓝图 §4.5 lock 汇编修正：原稿存在重复 `out(reg) tmp`（tmp/tmp2 同名绑定同一变量）与 label 复用问题；实现重写为合法内联汇编——独立数字标号（lock 用 2/3/4，try_lock 用 5/6）+ 独立输出寄存器分绑（tmp/st/acq），try_lock 失败路径补 `clrex` 清理排他监视器 | 蓝图原稿无法通过编译；STLXR 状态寄存器与数据寄存器架构要求不同名 | 汇编合法可编译；try_lock 失败不残留排他监视状态干扰后续排他操作 |
| D5 | AtomicPtr 薄封装：不重新实现 core 原子类型，`AtomicPtr<T>` 内部直接持有 `core::sync::atomic::AtomicPtr<T>`（零额外状态、零开销、无 unsafe），仅按蓝图接口类提供默认 SeqCst 的便捷层；`load_ordering`/`store_ordering` 显式变体供弱序场景，`as_core()` 逃生舱取回 core 全量 API | 仓库规范 §5.5 防重复造轮子；蓝图「顺序一致性原子」原样口径 | 文档化「优先直接使用 core::sync::atomic 基础类型」口径；swap/CAS 弱序罕见场景经 as_core 直达 |
| D6 | 故障注入（蓝图 §6.5）「故意省略屏障」对照实验仅以文档 + `#[ignore]` 测试（CONC6）形式保留，默认不跑 | 防 CI 偶发（spec 既定偏差原样） | `cargo test -- --ignored` 才执行；仅观测不断言，host 强序预期 0 作对照基线 |
| D7 | owner 诊断（debug_assertions 专属，蓝图 §4.4「开发期断言检测」）：`owner: AtomicU32` 记录持有者标识（0=空闲）；aarch64 标识 = `MPIDR_EL1 & 0xFF + 1`，host cfg(test) 标识 = 线程 id 哈希；**诊断生效形态收窄**——host 非 test debug 构建（集成测试链接的 lib 形态）无 TLS 可用，`current_owner_id()` 返回 0 跳过断言（修复：此前返回常量 1 致多线程误报，即 D8 根因）；release 构建整段剔除零开销，重复 lock 按蓝图口径死锁（调用方契约） | 该形态无法获得真实线程标识，误报大于收益；aarch64 真机与 crate 内单测诊断可靠保留 | SPL5 锚定诊断有效性；CONC1 多线程路径不被误报阻塞（见 D8） |
| D8 | CONC1 获取方式【已修复，绕行已回退】：D7 修复前集成测试形态 owner 诊断误报重复 lock（dev profile debug_assertions 生效，实测 spinlock.rs:174 left:1 right:1），CONC1 一度改用 `try_lock()` + 应用层自旋绕行；D7 修复（该形态返回 0 跳过断言）后回退为纯 `lock()` 获取，互斥正确性断言（终值 == 1,000,000）不变 | 根因修复优于测试绕行；互斥正确性须以真实 lock 路径验证 | CONC1 按蓝图 §6.2 有锁路径原样口径端到端验证 |
| D-S3 | SGI1 分工（main.rs 本地接入，sync/、smp/、interrupt/ 零改动）：SGI0 已被 v0.136.0 IPI 通道占用（`IPI_SGI_ID`=0；SGI 为 banked 中断全核共享同一分发表，无法复用承载 sync 语义），取 SGI1（INTID=1）为 sync 测试专用；从核参与机制 = 主核发 SGI1 → 从核 IRQ handler 上下文执行递增（对比备选「从核驻留参与」改动面最小） | 复用 v0.136.0 已验证的中断链路；banked SGI 分发表不可复用 | SGI0/SGI1 两通道互不干扰；IRQ 上下文持锁语义（重入安全 DAIF 快照对 + WFE/SEV 跨核唤醒）论证记录于 handler 文档注释 |
| D-S3a | 从核 GICR 使能主核代写：boot.rs 唤醒序列仅置 SGI0（IGROUPR0 全写 1 = 仅 bit0 归 Group1），SGI1 须补 IGROUPR0 |= bit1（读改写在 Group1 呈递）+ ISENABLER0 bit1；GICR 为内存映射寄存器（0x080A_0000 + cpu×0x20000 + 0x10000 SGI 帧），主核直接代写 MMIO，写后 `dsb` 收口再触发 SGI，无需改 boot.rs | per-core 使能时序与「GIC 配置写完成」语义；使能-触发竞态关闭 | 三从核 handler 全部到达（qemu_run.md §16 高风险点④未触发）；interrupt/ 零改动 |
| D-S3b | 主核参与竞争：主核不经 SGI1（TargetList 仅含从核），在主流程上下文执行同次数递增；主核 `lock` 的 save_daif 关本核 IRQ 期间从核 handler 的持锁/等待不受影响（unlock 的 SEV 跨核送达），无死锁；降级口径：无在线从核时不发 SGI1、期望值退化为 1×每核次数，单核自验仍打印 `[sync] ok` | 4 核真实并发竞争为蓝图 §6.2 真机版本的直接承载；与 smp_smoke 降级哲学一致 | QEMU 实测 40,000 次排他获取计数终值精确（qemu_run.md §16） |

---

## 10. 蓝图 §7 验收对照

| 蓝图 §7 验收标准 | 实现证据 | 结论 |
|-----------------|---------|------|
| §7.1 功能：多核并发数据正确 | host：SPL6（8×10,000=80,000）/CONC1（4×250,000=1,000,000）/CONC3（try_lock 守恒）全过；QEMU：sync_smoke 4 核 40,000 次竞争计数终值精确等于期望值，`[sync] ok` 十断言首轮一次 PASS（qemu_run.md §16） | ✅ |
| §7.2 性能量化：无竞争 < 50ns | CONC5 host smoke 实测均值 13 ns/op << 1μs 极宽松上限（**host x86 用户态口径声明，仅记录非验收**；蓝图 <50ns 为目标机 aarch64 裸机口径，目标机性能测定属后续版本，同 v0.136.0 SMPT6 口径惯例） | ✅（host smoke + 口径声明） |
| §7.3 安全：临界区关中断保护 | `lock()` 先 `save_daif` 关 IRQ 再竞争锁（蓝图 §4.4）；Guard Drop 先释放锁（STLR+DMB ISH+SEV）再 `restore_daif`（蓝图 §8.5 Drop 顺序）；IRQ 上下文持锁为重入安全快照对（sync_sgi_handler 真机验证）；debug owner 重复 lock 诊断（D7，SPL5） | ✅ |
| §7.4 文档：memory_ordering.md | 本文档（[docs/kernel/memory_ordering.md](./memory_ordering.md)）：ARM64 弱内存模型（§2）/ LDAXR-STLXR 语义（§3）/ 屏障选择决策树（§4）/ 使用点位表（§6）/ D1~Dn 偏差表（§9）/ 验收对照（本节） | ✅ |
| §7.5 出口判定：多核并发数据正确 → v0.138.0 可启动 | 十关键串断言 PASS（含 `[sync] ok`）+ host 测试全绿零回归；统一 SpinLock/原子封装/屏障原语就绪并经真机多核竞争验证，为 v0.138.0 确定性调度（共享 ReadyQueue 多核加锁等）提供正确并发基础 | ✅ |

---

## 11. 源码链接

| 模块 | 路径 |
|------|------|
| sync 子系统根部（公开导出 SpinLock/SpinLockGuard/dmb/dsb/isb/dmb_ish + D1~D6 头注） | [crates/kernel/microkernel/src/sync/mod.rs](../../crates/kernel/microkernel/src/sync/mod.rs) |
| 内存屏障（aarch64 同名指令 / host fence 映射 + BAR1~BAR3 单测） | [crates/kernel/microkernel/src/sync/barrier.rs](../../crates/kernel/microkernel/src/sync/barrier.rs) |
| 多核自旋锁（LDAXR/STLXR + WFE/SEV + DAIF 保护 + owner 诊断 + SPL1~SPL6 单测） | [crates/kernel/microkernel/src/sync/spinlock.rs](../../crates/kernel/microkernel/src/sync/spinlock.rs) |
| 原子操作封装（AtomicPtr 薄封装 + ATM1~ATM3 单测） | [crates/kernel/microkernel/src/sync/atomic.rs](../../crates/kernel/microkernel/src/sync/atomic.rs) |
| 内核启动期接入（§3.11 sync_smoke 段 + sync_sgi_handler + D-S3 系列决策） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试 CONC1~CONC6 | [crates/kernel/microkernel/tests/concurrency_test.rs](../../crates/kernel/microkernel/tests/concurrency_test.rs) |
| 既有屏障点位：smp boot 握手（Release/Acquire） | [crates/kernel/microkernel/src/smp/boot.rs](../../crates/kernel/microkernel/src/smp/boot.rs) |
| 既有屏障点位：IpiRing 位序 | [crates/kernel/microkernel/src/smp/ipi.rs](../../crates/kernel/microkernel/src/smp/ipi.rs) |
| 既有屏障点位：VBAR isb | [crates/kernel/microkernel/src/interrupt/vector.rs](../../crates/kernel/microkernel/src/interrupt/vector.rs) |
| 既有屏障点位：GIC 寄存器序列 dsb | [crates/kernel/microkernel/src/interrupt/gicv3.rs](../../crates/kernel/microkernel/src/interrupt/gicv3.rs) |
| QEMU 测试脚本（第十条断言 `[sync] ok`） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| QEMU 实测记录（v0.137.0 串口全文/十断言/四项高风险点分析） | [docs/boot/qemu_run.md](../boot/qemu_run.md) §16 |
| SMP 设计文档（v0.136.0，前序版本与屏障点位来源） | [docs/kernel/smp_design.md](./smp_design.md) |
| 线程调度设计文档（v0.134.0） | [docs/kernel/scheduler_design.md](./scheduler_design.md) |
| 中断管理设计文档（v0.133.0，SGI handler 注册通道） | [docs/kernel/interrupt_design.md](./interrupt_design.md) |
