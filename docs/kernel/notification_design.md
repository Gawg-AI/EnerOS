# EnerOS v0.140.0 通知机制与异步信号设计文档（notification / 64 位信号 word / 中断绑定）

> 版本：v0.140.0
> 适用范围：EnerOS 自研内核 IPC 子系统通知层（notification 对象 + 64 位原子信号 word + 阻塞式 signal/wait/poll + IRQ 绑定注入；host 集成测试 + QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` 双线程 wait/signal 配对 CNTVCT 实测）
> 蓝图依据：`蓝图/phase3.md` §v0.140.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-D IPC 与能力线第 2 版**（v0.139.0 ~ v0.142.0 四版之二），在 v0.139.0 同步消息传递 IPC 之上提供非阻塞事件通知能力，为 **v0.141.0 共享内存 IPC 提供事件协调**；能力拦截接入属 v0.142.0
> 配套文档：[ipc_design.md](./ipc_design.md)（v0.139.0 同步 IPC 设计，主控 hub 仲裁切换 D-I1 与 1.5ms QEMU 放宽阈值先例来源）、[wcet_report.md](./wcet_report.md)（v0.138.0 调度确定性与 WCET 验证，CNTVCT/cycles_to_ns 测量设施复用来源）、[scheduler_design.md](./scheduler_design.md)（v0.134.0 线程调度，block/unblock 与显式 TCB 对切换设施来源）、[../boot/qemu_run.md](../boot/qemu_run.md) §19（v0.140.0 QEMU 实测记录：首轮一次 PASS，无阈值调整）

---

## 1. 版本目标

**核心一句话**：在 v0.139.0 同步消息传递 IPC 之上实现 notification 对象与异步信号机制（64 位信号 word + 中断绑定 + 事件通知），为设备驱动、定时器、Agent 事件提供非阻塞通知能力（蓝图 §v0.140.0 版本定位原样口径）。

- **业务价值**：同步 IPC（send/recv/call/reply）只能表达「请求-应答」一类交互；设备中断、定时器到期、Agent 事件广播等场景需要**异步、可累积、一对多**的事件通知原语，notification + 64 位信号 word 即该原语（蓝图 §1）。
- **下游衔接**：为 **v0.141.0 共享内存 IPC 提供事件协调**——共享内存负责大批量数据带外传递，notification 负责「数据就绪/已消费」事件握手（同 v0.139.0 MR 仅传描述符、长载荷走共享内存的分层口径）；能力拦截接入属 v0.142.0（owner 分区记录为本版本预留的接入点，蓝图 §7.3）。
- **ADR-0001 裁剪口径**（蓝图 §1 引用段原样口径）：本版本原为自研内核组件，已按方案 A 调整为 seL4 深度定制/信创认证路径。**notification 语义借鉴 seL4 notification 对象，本内核按同语义自研实现**（与 seL4 语义一致，蓝图 §8.4），保持语义同构以降低 v0.156.0 迁移成本；差异点（wait 按 mask 清位而非取走整个 word）属 D14 决策，见 §6.2。
- **依赖关系**：前序 v0.139.0 同步 IPC 可用（蓝图 §2：IPC 延迟不达标阻塞 v0.140.0，该阻塞项已按 v0.139.0 决策记录③口径解除）；v0.133.0 GICv3 中断子系统可用（IRQ 绑定注入的查表设施 `IRQ_TABLE_SIZE` 来源）；线程阻塞/唤醒机制可用（v0.134.0 `block_thread`/`unblock_thread`）。

## 2. 交付物清单

对照蓝图 §3 逐项标注偏差（D 编号与 [src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs) 头注 D8~D14 一一对应，全局表见 §11）：

| 蓝图 §3 交付物 | 实际落位 | 偏差 |
|---------------|---------|------|
| 代码：`kernel/src/ipc/notification.rs`（notification 对象） | [crates/kernel/microkernel/src/ipc/notification.rs](../../crates/kernel/microkernel/src/ipc/notification.rs)（NotificationId / WaiterRing / Notification / MAX_NOTIFICATIONS / WAITER_QUEUE_CAP） | D8 路径归口；D9 等待者环定长化 |
| 代码：`kernel/src/ipc/signal.rs`（信号与唤醒管理） | [crates/kernel/microkernel/src/ipc/signal.rs](../../crates/kernel/microkernel/src/ipc/signal.rs)（NotifyStats / NotificationManager / NotificationOps trait） | D8 路径归口；D10 注入调度器；D11 外挂等待结果槽；D12 IRQ 绑定查表 |
| 代码：模块挂载与公开导出 | [crates/kernel/microkernel/src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs)（`pub mod notification; pub mod signal;` + `pub use` 导出 + D8~D14 头注，surgical 追加） | D8（蓝图未单列 mod.rs） |
| 代码：BlockReason 追加 NotifyWait 变体 | [crates/kernel/microkernel/src/ipc/endpoint.rs](../../crates/kernel/microkernel/src/ipc/endpoint.rs) `BlockReason::NotifyWait(NotificationId)`（surgical 单变体追加，既有四变体零改动） | 无（蓝图 §4.1 原样语义） |
| 接口：`NotificationId` / `Notification` / `NotificationOps` trait | 蓝图 §3 原样导出（`NotificationId = u32`；Notification 五字段；trait 五方法 + take_wait_result/irq_signal 两补充） | D9 waiters 类型；D13 trait 含 `tid` 参数 |
| 文档：`docs/kernel/notification_design.md` | 本文档 | 无（路径同蓝图） |
| 测试：`kernel/tests/notification_test.rs` | [crates/kernel/microkernel/tests/notification_test.rs](../../crates/kernel/microkernel/tests/notification_test.rs)（NTI1~NTI8；NTI8 `#[ignore]` 故障注入） | D8 路径归口 |
| 测试：lib 内嵌单测 | NTF1~NTF3（notification.rs）/ SGN1~SGN7（signal.rs）（新增 10 个） | 蓝图 §6.1 单元测试落点 |
| 测试：QEMU 冒烟 | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `notify_smoke` 段（主控 hub 仲裁切换 D-I1 复用，100 轮 wait/signal 配对 CNTVCT 实测 + poll 累积路径 + bind_irq 注入路径） | 蓝图 §6.2 集成测试的目标机落点；D-N1 IRQ 注入口径 |
| 配置：MAX_NOTIFICATIONS=512 | [notification.rs](../../crates/kernel/microkernel/src/ipc/notification.rs) `MAX_NOTIFICATIONS = 512` | 无（蓝图原样） |
| 配置：等待者队列容量（蓝图未限定） | [notification.rs](../../crates/kernel/microkernel/src/ipc/notification.rs) `WAITER_QUEUE_CAP = 16` | D9 有界化取值 |

**接口一览**（蓝图 §3 接口类签名 + D13/D10 修正，[ipc/signal.rs](../../crates/kernel/microkernel/src/ipc/signal.rs) 公开导出）：

| 项 | 定义 | 说明 |
|----|------|------|
| `NotificationId` | `pub type NotificationId = u32` | 蓝图 §3 原样；取值范围 0..512（= notification 表槽位下标） |
| `Notification` | struct 五字段 | `id / word: AtomicU64 / bound_irq: Option<IrqId> / waiters: WaiterRing / owner: PartitionId`（蓝图 §3/§4.1 字段 + §7.3 owner；waiters 定长化 D9） |
| `WaiterRing` | 定长环形队列 | 元素 `(Tid, u64)` = (tid, mask)；cap 16，下标取模回绕，len 区分空/满；满 push 返回 false（D9） |
| `BlockReason` | enum 五变体 | `None / IpcSend(EndpointId) / IpcRecv(EndpointId) / IpcReply(Tid) / NotifyWait(NotificationId)`（蓝图 §4.1 原样 + 本版本第五变体追加） |
| `NotificationOps` trait | 七方法 | `create_notification / signal / poll / wait / bind_irq / take_wait_result / irq_signal`（蓝图 §3 五方法 + 两补充；D13 含 `tid`、D10 注入 `&mut SchedulerImpl`） |
| `NotifyStats` | struct 五字段 | `signals / waits_immediate / waits_blocked / wakes / irq_signals`（蓝图 §9.6 可观测交付物；全部 `saturating_add(1)` 饱和累加） |

## 3. 64 位信号位模型

### 3.1 原子信号 word（蓝图 §5.2）

- **每 bit 代表一个事件**：`Notification.word: AtomicU64`（初值 0），单对象一次可表达 **64 个并行事件**；signal 经 `fetch_or(bits, Release)` 置位，wait 按 mask 命中消费（清除命中位），poll 非破坏读出。
- **多事件并行**：3 个等待者以互异 mask（0x1/0x2/0x8）挂同一 notification，一次 `signal(0x3)` 同时置两位 → w1/w2 命中唤醒、w3 未命中保留（NTI2 锚定）；单次 signal 的多位语义是信号量/单事件队列无法直接表达的（见 §3.2 选型对比）。
- **信号累积不丢失**（蓝图 §9.4）：无等待者时 signal 的事件位在 word 中累积保留，后续 wait 立即命中返回（SGN3：`signal(0x5)` 后 `wait(mask=0x1)` 立即 `Ok(0x5)`，命中位 0x1 清除、未命中位 0x4 保留）。
- **IRQ 事件位约定**：绑定 IRQ 触发的自动 signal 置位 `1 << (irq % 64)`（D12 查表路径，SGN6/NTI3 锚定）。

### 3.2 与信号量/事件队列选型对比（蓝图 §5.1 原样口径）

| 维度 | 信号量 | 事件队列 | 64 位信号 word（本实现） | 选择 |
|------|--------|---------|--------------------------|------|
| 多事件表达 | 单一计数，无法区分事件来源 | 每事件需包装为消息入队 | **一次表达 64 个并行事件（每 bit 一个）** | 信号 word |
| 内存开销 | 计数器 + 等待队列 | 消息缓冲 + 队列管理 | 单 AtomicU64 + 定长等待者环（零堆分配） | 信号 word |
| 等待语义 | 阻塞至计数非零 | 阻塞至队非空 | **按 mask 位匹配阻塞/唤醒（选择性消费）** | 信号 word |
| 中断绑定 | 需中断 handler 显式 up | 需 handler 投递消息 | **bind_irq 原生支持（查表自动 signal）** | 信号 word |

选型理由（蓝图 §5.1/§5.2）：notification 是 seL4 轻量事件原语，64 位并行事件 + 位掩码选择性等待 + 中断原生绑定三特性与设备驱动/定时器/Agent 事件场景精确匹配；信号量无法区分事件来源，事件队列需包装消息且有缓冲管理开销。实现路径（蓝图 §5.3）：notification 对象 → signal/wait/poll → bind_irq → 延迟优化。

## 4. 阻塞语义与状态机

### 4.1 BlockReason::NotifyWait 状态机

| 变体 | 语义 | 登记/复位 |
|------|------|----------|
| `NotifyWait(NotificationId)` | 阻塞于 notification **等待**（wait 未命中，等待对端 signal 置位命中） | wait 入队时登记；signal 扫描命中唤醒时复位 None |

阻塞原因登记于 `NotificationManager.block_reasons: [BlockReason; MAX_THREADS]`（下标即 Tid），与调度器 `block_thread`/`unblock_thread` 联动（D10：蓝图 §4.5 自由函数 `crate::sched::block/unblock` 与 `get_thread_mut(...).unwrap()` → 注入 `&mut SchedulerImpl` 复用既有方法，unwrap 违仓库禁 panic 规则改 Result 短路；同 v0.139.0 D3 口径）。`Scheduler::block_thread` 仅置线程态不记录原因（签名单参 `tid`），故管理器自带 block_reasons 表（仿 `IpcManager::block_reasons` 先例）。

### 4.2 wait 双路径（立即命中 vs 阻塞登记）

| 路径 | 条件 | 行为 | 返回 |
|------|------|------|------|
| 立即命中 | `word & mask != 0` | 返回命中时刻 word（含其他未消费位，调用方可按位自取）+ **仅清除命中位**（matched = word & mask）+ `stats.waits_immediate +1` | `Ok(word)`，线程不阻塞 |
| 阻塞登记 | `word & mask == 0` | `(tid, mask)` 入 WaiterRing（满 16 → `OutOfMemory` 零副作用不阻塞）+ 登记 `NotifyWait(nid)` + `block_thread` + `stats.waits_blocked +1` | `Err(WouldBlock)`（「已阻塞登记」指示，蓝图 §4.5 原样语义；唤醒后经 `take_wait_result(tid)` 取回 word，取走后槽位清空 D11） |

### 4.3 signal 扫描唤醒与 D14 清位口径

- **置位**：`word.fetch_or(bits, Release)`——Release 序保证信号位对后续 wait/poll 的 Acquire 读可见（见 §9 坑点①）。
- **扫描唤醒**：WaiterRing 定长环无 retain，按快照长度逐个 pop_front——命中者（`word & mask != 0`）写 wait_results 等待结果槽（唤醒时刻 word，D11）+ block_reasons 归 None + `unblock_thread` + 出队；未命中者重新 push 回队尾（FIFO 保序：本批弹出次序即原队次序，回挂者相对次序不变，见 §9 坑点③）。
- **D14 唤醒清位口径**：命中唤醒后 `word.fetch_and(!matched, Release)`——**仅清被唤醒者命中的 mask 位，不清整个 word**。蓝图唤醒时 `word.fetch_and(!word)` 按整个 word 清除会丢失其他未消费事件位，违蓝图 §9.4 信号累积不丢失（蓝图 §4.1 注释「自动清除 word &= !mask（可选）」同向佐证）。SGN4 锚定：word 预累积 0x8，signal(0x1) 命中 A 后清 0x1 不波及 0x8（`poll == 0x8`）；A 取回结果 = 0x9（唤醒时刻完整 word）。
- **同 mask 多等待者 FIFO 消费口径**：同 mask 多等待者**共享同一事件位**——一次 signal 仅**队首**命中并消费该位（位被清除后后续等待者逐等待者重读的 word 已无该位，未命中回挂队尾且相对次序不变）。NTI6 锚定：3 等待者同 mask=0x1 依次入队，连续 3 次 signal(0x1) 各唤醒一人，**唤醒顺序 == 入队顺序**；SGN7 FIFO 核验：16 等待者互异位一次 signal(0xFFFF) 全唤醒，第 i 个等待者的结果恰为低 i 位已被前序消费后的 word，证明处理次序 == 入队次序。

## 5. 中断绑定

### 5.1 D12 irq_bindings 查表设计

- **偏差动机**：蓝图 `register_handler(irq, move |_irq,_ctx| ...)` 捕获闭包——本内核 IRQ handler 为 `extern "C"` fn 指针（v0.133.0 设施），**fn 指针无法捕获环境**，闭包中引用的 nid/manager 无从传递。
- **实际设计**：管理器内 `irq_bindings: [Option<NotificationId>; IRQ_TABLE_SIZE]` 查表（`IRQ_TABLE_SIZE = 1024`，下标即 IrqId）+ `irq_signal(irq, sched)` 方法：`bind_irq(nid, irq)` 记录 `bound_irq = Some(irq)` 并登记 `irq_bindings[irq] = Some(nid)`；中断触发时经 `irq_signal` 查表自动 `signal(nid, 1 << (irq % 64))`（`stats.irq_signals +1`，复用 signal 路径计数）。nid 不存在/越界或 irq ≥ IRQ_TABLE_SIZE → `InvalidArgument`。
- **trampoline 注册属集成层职责**：真中断 handler 到 `irq_signal` 的挂接属后续集成版本；host 测试经 `irq_signal` 直接注入验证全链路（SGN6/NTI3：wait → bind_irq → irq_signal 注入 → word 置位 → 唤醒取回 IRQ 事件位），语义等价。

### 5.2 中断上下文不可阻塞（蓝图 §8.1）与延迟调度（§8.5）

- 中断上下文仅**置位与唤醒登记，不做立即调度**（蓝图 §8.5：中断中 signal 后延迟调度，由 IRQ 返回后统一调度出口处理）——`irq_signal` 复用 signal 路径，仅完成 fetch_or 置位 + 扫描唤醒登记（写 wait_results + unblock），不触发上下文切换；实际调度由 IRQ 返回后的调度出口统一处理。
- 中断上下文不可阻塞（蓝图 §8.1）：signal/irq_signal 路径全程无阻塞点（置位、查表、唤醒登记均为 O(cap) 有限步操作），满足中断上下文约束；wait 为线程上下文原语，中断路径不得调用。

### 5.3 D-N1 决策记录（QEMU 冒烟为何采用 irq_signal(33) 注入口径而非挂真 timer IRQ）

引自 [main.rs notify_smoke 文档注释](../../crates/kernel/microkernel/src/main.rs) 决策记录原样口径：

- 本冒烟可复用的 timer IRQ 设施为 v0.135.0 CNTP 1kHz 分区调度 tick——其 handler **已由 v0.138.0 `wcet_timer_handler` 同号覆盖注册**（INTID 30）且 wcet_smoke 收尾已停定时器，无可挂接的空闲 timer IRQ；
- 挂真 IRQ 需在 handler **中断上下文取 `NTFY_MANAGER` 锁 + 触碰 SCHED**——主流程持锁期间 IRQ 到达即**自旋死锁**，与本段单核协作式模型冲突（锁口径：临界区严格不含切换点，持锁跨切换必死锁）；
- 故取 **D12 等价语义**：主流程经 `irq_signal(NTFY_TEST_IRQ=33, sched)` 注入（SGN6 同款查表路径：`irq_bindings[33]` → signal 置位 `1 << (33 % 64)`）；**INTID 33 为 SPI 区间安全值**（不冲突 SGI0/SGI1（INTID 0/1，v0.136.0/v0.137.0 占用）与 CNTP PPI 30），**GIC 不触碰、该 INTID 永不使能**，与既有十二类输出正交。

## 6. seL4 notification 语义对照（蓝图 §8.4）

### 6.1 逐项对照表

| 本实现 | seL4 对应 | 语义一致性 |
|--------|-----------|-----------|
| `signal(nid, bits)` | `seL4_Signal(notification)` | 一致：置位 + 唤醒匹配等待者；seL4 以 badge 位 OR 入 word，本实现以显式 bits 参数（badge 语义由调用方编码进 bits） |
| `poll(nid)` | `seL4_Poll(notification)` | 一致：非阻塞读 word；差异——seL4 Poll 取走（清除）word，本实现 poll **非破坏读出不清除**（SGN5 连读两次一致锚定），取走语义由 wait 承担 |
| `wait(tid, nid, mask)` | `seL4_Wait(notification)` | 一致：命中即返/未命中阻塞登记 + 唤醒后取回 word；**差异——本实现 wait 按 mask 清位（D14），seL4 Wait 取走整个 word**（见 §6.2） |
| `bind_irq(nid, irq)` + `irq_signal(irq, sched)` | `seL4_IRQHandler_SetNotification(handler, notification)` | 一致：IRQ 触发自动 signal 关联 notification；实现形态差异——seL4 内核对象绑定，本实现管理器查表注入（D12），trampoline 注册属集成层职责 |
| 等待结果取回（`take_wait_result`） | seL4 Wait 出参 badge | 一致：唤醒后取回唤醒时刻 word；形态差异——本实现外挂每线程等待结果槽（D11，TCB 零改动） |

### 6.2 语义一致性声明与差异点

- **声明**：notification 对象模型（64 位信号 word + 等待者队列 + IRQ 绑定）与 signal/poll/wait/bind_irq 四原语语义与 seL4 notification **同构**（蓝图 §8.4 兼容口径），降低 v0.156.0 迁移成本；与 ADR-0001 的衔接：方案 A 下不重写 seL4 notification，自研内核侧同构实现即「复用 seL4 notification 模型」的落地形态。
- **差异点①（D14）**：seL4 Wait 取走整个 word（等待方独占消费），本实现 wait/signal 唤醒**仅按命中 mask 位清除**——保障蓝图 §9.4 信号累积不丢失（未命中事件位保留给其他等待者/后续 wait），是蓝图 §4.1 注释「自动清除 word &= !mask（可选）」的采纳项。
- **差异点②（D11）**：seL4 经 TCB 内嵌 badge 出参返回 word，本实现外挂 `wait_results: [Option<u64>; MAX_THREADS]` 等待结果槽（TCB 零改动同 v0.139.0 D4；wait 返回 u64 而非 Message，与 IPC recv_slots 解耦）。
- **差异点③（mask 参数）**：seL4 Wait 无 mask（等待全部事件位），本实现 wait 含 mask 参数（蓝图 §4.5 原样签名），支持按事件位选择性等待——seL4 等价语义需多 notification 拆分，本实现单对象 64 位即可表达。

## 7. 性能实测

### 7.1 host 口径（NTI7，口径声明）

- host 无法读 ARM CNTVCT_EL0，NTI7 以 `std::time::Instant` 夹取 **512 次 create_notification + 1,000 轮 signal/wait 配对**（每轮 client wait 阻塞登记 → signal 唤醒 → take_wait_result 逐位校验，事件位逐轮轮换 `1 << (round % 64)`）总耗时并 eprintln 输出（`-- --nocapture` 可见）。
- 实测：总耗时**约 45.5 微秒**（1,000 轮配对 + 512 次 create，host 注入时钟口径）。
- **口径声明**：host 数值**仅记录耗时量级供人工对照，不设硬断言**——性能目标 signal<1μs / wait 唤醒<5μs 为真机 CNTVCT 口径（蓝图 §6.3/§7.2），判定路径由 QEMU CNTVCT 实测承载（见 §7.2）；功能计数复核 signals/waits_blocked/wakes == 1,000。

### 7.2 QEMU CNTVCT 实测（[qemu_run.md](../boot/qemu_run.md) §19 原样数字）

冒烟设置：`notify_smoke` 段（main.rs §3.14，`[ipc] ok` 后追加）——NTFY_MANAGER（NotificationManager ≈230KB）经 BSS `MaybeUninit` 逐字段就地有效化（驻留 NOBITS 不胀镜像）→ `create_notification(PartitionId(0))` 得 nid → PMM 新分 2×16KB 栈 → 创建 server/client 双线程（prio 20）并绑栈 → **主控 hub 严格交替直切**（D-I1 口径复用 v0.139.0 `ipc_yield_to_main`/`ipc_switch_to` 设施，首切 client 使其先入 wait 阻塞登记，不经 `schedule()` 避免复活 v0.134.0 遗留 t1/t2）→ **NTFY_ROUNDS=100 轮**配对：client `wait(nid, mask=0x1)` 走 NotifyWait 阻塞路径 → server `signal(nid, 0x1)`（先存 CNTVCT t0 再置位唤醒）→ client 唤醒校验 wait_result==0x1 并读 t1 夹取 **signal→唤醒延迟**（含主控两次直切的端到端口径，cycles→ns 复用 v0.138.0 wcet 设施）。收齐完成标志后主流程补两路径冒烟：poll 累积（无等待者 signal(0x2) 后 poll 读出 0x2）+ bind_irq 注入（`bind_irq(nid, 33)` → `irq_signal(33)` 验证置位，D-N1）。判定：唤醒完整性全对（ok==100 且 code==0）且 max < `NTFY_LAT_MAX_NS` 打印 `[notify] ok avg=<ns> max=<ns>`。

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（首轮一次通过，无排障记录，无阈值调整；2026-07-26 实测，WSL2 Ubuntu-22.04 + QEMU 8.2.2 + nightly-2026-04-04；端到端 15s，release ELF 131,720 B < 512 KB 预算） |
| 串口关键行 | **`[notify] ok avg=2914 max=250496`**（PASS 轮原样收录） |
| 延迟 avg | **2,914 ns ≈ 2.9μs**（100 轮稳态均值；蓝图 §7.2 真机目标 signal<1μs / wait 唤醒<5μs，avg 已落在真机目标量级附近——QEMU TCG 口径，含主控两次直切） |
| 延迟 max | **250,496 ns ≈ 250.5μs**（单样本尖峰系 TCG 翻译块冷启动/仿真抖动；**avg/max 相差 86×**，形态同 v0.139.0 `avg=3264 max=231232` 先例，非通知机制退化） |
| 判定阈值 | `NTFY_LAT_MAX_NS` = **1,500,000 ns**（QEMU 放宽口径，同 v0.139.0 1.5ms 先例） |
| 余量 | max=250,496 ns < 1,500,000 ns，**余量 ≈6.0×** |
| 激活上限兜底 | `NTFY_ACTIVATION_CAP`=2000（正常全程 202 次激活，近 10× 余量，本轮未触发） |

## 8. 阈值对照（蓝图 §6.3/§7.2 验收标准）

| 口径 | 阈值 | 实测 | 判定 |
|------|------|------|------|
| 蓝图 §6.3/§7.2 真机目标 | **signal < 1μs、wait 唤醒 < 5μs** | —（真机未测） | **留待飞腾/鲲鹏硬件验证**（QEMU 数值不可作为真机性能证据，见 §9 坑点⑤） |
| QEMU 判定口径（Task 4 决策） | `NTFY_LAT_MAX_NS` = **1,500,000 ns**（依据 v0.139.0 放宽先例：主控 hub 每轮 2 次上下文直切 + TCG 指令翻译 + CNTVCT 虚拟计数器粒度主导） | max=250,496 ns | ✅ PASS（≈6.0× 余量；**首轮一次通过，未触发「实测接近/超阈值时放宽判定口径」路径，NTFY_LAT_MAX_NS/NTFY_ACTIVATION_CAP 零调整**） |
| 冒烟硬判定（功能完整性） | code==0 ∧ ok==100 ∧ 无超时（激活上限 2,000） | 全部满足 | ✅ PASS |
| host 判定口径 | 无硬断言（仅记录，见 §7.1） | 1,000 轮配对 + 512 次 create ≈45.5μs | 口径内记录 |

**QEMU 放宽口径声明**：CNTVCT 虚拟计数器粒度/递送开销与 TCG 纯软件仿真的上下文直切代价为已记录坑点（同 v0.138.0 wcet 段、v0.139.0 IPC 段先例），故 QEMU 冒烟判定 = 「功能完整性硬判定 + 延迟实测值软记录 + 1.5ms 量级约束阈值」；蓝图真机目标不变，留待硬件验证同口径重测。

## 9. 坑点与风险（蓝图 §8.5）

| 坑点/风险 | 本版本处置 |
|----------|-----------|
| ① **signal 原子操作与唤醒顺序** | 置位 `fetch_or(bits, Release)`——Release 序保证信号位对后续 wait/poll 的 Acquire 读可见；扫描期间**逐等待者重读 word（Acquire）**——前一命中者可能已清位（D14 消费口径的正确性依赖此重读，NTI2/SGN7 FIFO 核验锚定）；清位 `fetch_and(!matched, Release)` 与后续等待者读建立 happens-before |
| ② **中断中 signal 后不立即调度** | 中断上下文仅置位与唤醒登记，不做立即调度（蓝图 §8.5 延迟调度：IRQ 返回后统一调度出口处理）；`irq_signal` 复用 signal 路径全程无阻塞点，满足蓝图 §8.1 中断上下文不可阻塞约束（见 §5.2） |
| ③ **WaiterRing 无 retain 需遍历重排保持 FIFO** | signal 扫描按快照长度逐个 pop_front，命中者出队唤醒、未命中者重新 push 回队尾——本批弹出次序即原队次序，回挂者相对次序不变，FIFO 保序（signal.rs 实现注释原样口径；SGN7/NTI6 锚定） |
| ④ **同 mask 多等待者共享事件位仅队首消费** | D14 语义推论：同 mask 等待者共享同一事件位，一次 signal 仅队首命中消费（位清除后后续等待者重读已无该位，未命中回挂）——唤醒顺序 == 入队顺序（NTI6 锚定，见 §4.3）；多等待者需各自消费时应使用互异事件位（NTI2/SGN7 口径） |
| ⑤ QEMU TCG 仿真延迟不可作为真机性能证据 | QEMU 数值为功能验证记录（§7.2）；真机验收（飞腾/鲲鹏）以 §7.2 signal<1μs / wait 唤醒<5μs 同口径重测为准（同 v0.138.0/v0.139.0 先例） |
| ⑥ 等待者队满 OutOfMemory 路径 | WAITER_QUEUE_CAP=16 有界：第 17 个 wait → `Err(OutOfMemory)` **零副作用**（不阻塞、阻塞原因不登记、队列长度不变、stats 不计，NTI8 `#[ignore]` 故障注入锚定） |
| ⑦ 大对象驻留（≈230KB） | NotificationManager 全体 ≈230KB 静态零初始化段（NOBITS 不胀 ELF）；QEMU 侧经 BSS `MaybeUninit` 逐字段就地有效化（内核栈 64KB 严禁栈上构造，v0.138.0 D-W1 惯例）；host 测试经 `run_big_stack` 8MB 大栈线程 + `Box` 持有（同 sync.rs/mem partition.rs 夹具口径） |
| 风险 §8.2 依赖：依赖调度器 block/unblock | D10 注入 `&mut SchedulerImpl` 复用 `block_thread`/`unblock_thread`（v0.134.0 设施），无全局单例；阻塞原因表外挂（仿 IpcManager 先例） |

## 10. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`：**全量 228 passed / 0 failed / 4 ignored**（16 套件；4 个 ignored 均为既有/新增 `#[ignore]` 故障注入用例按设计跳过）。构造 NotificationManager 的用例统一经 `run_big_stack` 在 8 MB 大栈线程中运行（内联 ≈230KB，防 host 默认栈溢出，同 v0.139.0 夹具口径）。

### 10.1 单元测试（src/ipc 内嵌，新增 10 个）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| NTF1 | `ntf1_notification_construction_five_fields`（notification.rs） | Notification 五字段就位（id/bound_irq/waiters/owner 原值读回）+ word 原子初值 0；字段可写可用（置位读回/绑 IRQ/waiters 操作） | PASS |
| NTF2 | `ntf2_waiter_ring_wraparound_fifo_full`（notification.rs） | WaiterRing push/pop 回绕保序 + 满 16 返 false（拒绝后长度不变）+ 空 pop 返 None（口径同 endpoint.rs EP1） | PASS |
| NTF3 | `ntf3_block_reason_five_variants_distinct`（notification.rs） | BlockReason 五变体互异（含新增 NotifyWait）+ 载荷参与判等 + Copy | PASS |
| SGN1 | `sgn1_create_full_out_of_memory_and_owner`（signal.rs） | create 占槽 0 + owner 原值读回（§7.3）；顺序填满 512 槽后第 513 次 `OutOfMemory` 不 panic、满表状态不破坏 | PASS |
| SGN2 | `sgn2_direct_wake_of_blocked_waiter`（signal.rs） | 直接唤醒路径：A wait 阻塞（WouldBlock + 入 waiters + NotifyWait + Blocked）→ signal(0x1) → wait_results 有值、block_reasons 归 None、回 Ready、命中位清除；take_wait_result 取后清空 | PASS |
| SGN3 | `sgn3_accumulation_then_immediate_wait`（signal.rs） | 累积路径：无等待者 signal(0x5) 位累积（§9.4 不丢失）→ 后 wait(0x1) 立即 Ok(0x5)、命中位清除、未命中位 0x4 保留、不阻塞 | PASS |
| SGN4 | `sgn4_multi_event_bits_selective_wake_and_clear`（signal.rs） | 多事件位：A(0x1)/B(0x4) 双等待者 + 预累积 0x8 → signal(0x1) 仅命中 A（结果=0x9）、B 保留阻塞、清 0x1 不波及 0x8（D14）；再 signal(0x4) 命中 B，0x8 全程保留 | PASS |
| SGN5 | `sgn5_poll_non_destructive_read`（signal.rs） | poll 非阻塞不清除（连读两次一致）、不阻塞、stats 不计 wait 路径 | PASS |
| SGN6 | `sgn6_bind_irq_and_irq_signal_injection`（signal.rs） | bind_irq(irq=33) + irq_signal 注入：word 置位 1<<(33%64)、等待者唤醒取回该位、irq_signals==1 复用 signal 计数；未绑定 IRQ 注入 → InvalidArgument | PASS |
| SGN7 | `sgn7_fault_injection_and_stats_accounting`（signal.rs） | 故障注入：不存在 nid（空槽/越界）signal/poll/wait/bind → InvalidArgument 零副作用；越界 irq bind 拒绝；waiters 满 16 第 17 个 → OutOfMemory 不阻塞；16 等待者互异位一次 signal(0xFFFF) 全唤醒 + stats 计数核验 + FIFO 保序核验 | PASS |

### 10.2 集成测试（tests/notification_test.rs，NTI1~NTI8，仅以公开 API 端到端验证）

| 编号 | 用例 | 蓝图对照 | 验证点 | 结果 |
|------|------|---------|--------|------|
| NTI1 | `nti1_signal_wait_pairing_with_manual_scheduling` | §6.1 signal/wait 配对 | client wait(mask=0x5) 阻塞（NotifyWait）→ 手动步进调度切 server → signal(0x5) → client 唤醒 → 步进切回 → take_wait_result 64 位逐位校验 == 0x5，取后槽空；stats 一轮配对各计一次 | PASS |
| NTI2 | `nti2_multi_event_bits_parallel_waiters` | §5.2 一次表达 64 个事件 | 3 等待者互异 mask（0x1/0x2/0x8）挂同一 nid → 一次 signal(0x3) → w1/w2 命中唤醒（逐等待者读各自唤醒时刻 word）、w3 未命中保留；再 signal(0x8) 全员唤醒（D14） | PASS |
| NTI3 | `nti3_irq_trigger_to_thread_wakeup_full_chain` | §6.2 中断触发全链路 | wait(1<<(33%64)) 阻塞 → bind_irq(nid, 33) 登记 → irq_signal(33) 注入模拟 IRQ → word 置位 → client 唤醒取回 IRQ 事件位（D12 查表注入口径） | PASS |
| NTI4 | `nti4_notification_exhaustion_out_of_memory` | §3 MAX_NOTIFICATIONS=512 | 512 次 create 槽位顺序分配 → 第 513 次 `OutOfMemory` 不 panic，满表状态不破坏 | PASS |
| NTI5 | `nti5_notification_owner_partition` | §7.3 绑定 owner 分区 | create_notification(PartitionId(7)) → owner == PartitionId(7)，id 即槽位下标、word 初值 0、bound_irq None、waiters 空 | PASS |
| NTI6 | `nti6_multi_waiter_fifo_ordering` | §4.5 唤醒扫描 FIFO | 3 等待者同 mask=0x1 依次入队 → 连续 3 次 signal(0x1) → 唤醒顺序 == 入队顺序（D14 口径：同 mask 共享事件位仅队首消费） | PASS |
| NTI7 | `nti7_performance_smoke` | §6.3/§7.2 性能基准 smoke | std Instant 夹取 512 次 create + 1,000 轮 signal/wait 配对总耗时 eprintln（host 口径仅记录不设硬断言；功能计数 signals/waits_blocked/wakes==1,000 复核） | PASS |
| NTI8 | `nti8_waiter_queue_full_out_of_memory`（`#[ignore]`） | §6.5 故障注入 | waiters 满（cap 16 连 wait 17 无 signal）→ 第 17 个 `OutOfMemory` 不 panic、不阻塞、零副作用（线程保持 Ready、阻塞原因不归零、队列长度不变、stats 不计）（`cargo test -- --ignored` 触发，同 IPC7/CONC6/WCT6 惯例） | ignore（口径内） |

### 10.3 QEMU 冒烟（main.rs §3.14 `notify_smoke` 段 + [qemu_run.md](../boot/qemu_run.md) §19）

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **十三**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok` / `[sync] ok` / `[wcet] ok` / `[ipc] ok` / **`[notify] ok`**；v0.140.0 起追加第十三条断言 `grep "[notify] ok"`，既有十二条原样） |
| 串口关键行 | `[notify] ok avg=2914 max=250496`（PASS 轮原样收录） |
| 轮次 | 首轮一次 PASS，无排障记录，无阈值调整（未触发放宽路径） |
| 冒烟规模 | 100 轮 wait/signal 配对（NTFY_ROUNDS=100）+ poll 累积路径 + bind_irq 注入路径（NTFY_TEST_IRQ=33，D-N1）；主控 hub 激活上限 2,000 兜底（正常 202 次） |
| 回归 | host 全量 228 passed / 0 failed / 4 ignored 零回归；release ELF 131,720 B < 512 KB 预算（NotificationManager ≈230KB 驻留 BSS NOBITS 不胀镜像）；与既有十二类输出正交（不触碰 timer/GIC/SGI/ARINC/IPC 设施） |

## 11. 蓝图偏差表（D8~D14）

> 文档级编号与 [src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs) 头注 D8~D14 一一对应（续 v0.139.0 D1~D7；D-N1 为 main.rs `notify_smoke` 本地接入决策，见 §5.3）。照 spec.md 末尾「蓝图偏差声明」全文原样摘录成表。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D8 | 蓝图路径 `kernel/src/ipc/` → `crates/kernel/microkernel/src/ipc/`（notification.rs / signal.rs，mod.rs 挂载导出）；测试 `kernel/tests/notification_test.rs` → `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/notification_design.md` 同蓝图 | 仓库规范 §2.3.1 强制子系统归口（同 D1） | 仅路径差异，交付物一一对应 |
| D9 | 蓝图 `waiters: VecDeque<(Tid, u64)>` → 定长环 `WaiterRing`（cap 16，`[Option<(Tid, u64)>; 16]` + head/tail/len，下标取模回绕，len 区分空/满） | 仿 D2 TidRing/SendRing 先例；no_std 无 alloc 无 `VecDeque`，有界防 BSS 爆炸 | 满 push 返回 false，调用方按 `OutOfMemory` 错误传播；容量上限即单 notification 最大挂起等待数（NTI8 锚定） |
| D10 | 蓝图 §4.5 自由函数 `crate::sched::block/unblock` 与 `get_thread_mut(...).unwrap()` → 注入 `&mut SchedulerImpl` 复用 `block_thread`/`unblock_thread` | 同 D3；unwrap 违仓库禁 panic 规则，改 Result 短路 | trait 方法签名含 `sched` 参数；host 可测且与 SMP 多实例演进兼容 |
| D11 | 蓝图唤醒写 `tcb.recv_message = Some(Message{...})` → `wait_results: [Option<u64>; MAX_THREADS]` 外挂每线程等待结果槽 | TCB 零改动同 D4；wait 返回 u64 而非 Message，与 IPC recv_slots 解耦 | 唤醒后经 `take_wait_result(tid)` 取回 word（取走后槽位清空）；下标即 Tid |
| D12 | 蓝图 `register_handler(irq, move |_irq,_ctx| ...)` 捕获闭包 → fn 指针无法捕获环境，改为管理器内 `irq_bindings: [Option<NotificationId>; IRQ_TABLE_SIZE]` 查表 + `irq_signal(irq, sched)` 方法 | fn 指针无法捕获环境；trampoline 注册属集成层职责（host 经 `irq_signal` 注入验证，语义等价） | IRQ 触发自动 `signal(nid, 1 << (irq % 64))`，`stats.irq_signals +1`；真中断挂接属后续集成版本（QEMU 冒烟 D-N1 注入口径，见 §5.3） |
| D13 | 蓝图 §3 trait `wait(nid, mask)` 无 `tid` 参数 → 采用 §4.5 口径含 `tid` | 同 D6：§3 与 §4.5 实现矛盾；阻塞/唤醒登记必须知道调用线程 | trait wait 签名含 `tid`；越界 tid ≥ MAX_THREADS → `InvalidArgument` 防御（check_tid） |
| D14 | 蓝图唤醒时 `word.fetch_and(!word)` 按整个 word 清除 → 仅按被唤醒者命中的 mask 位清除（matched = word & mask） | 蓝图 §4.1 注释「自动清除 word &= !mask（可选）」；按 word 清会丢失其他未消费事件位，违 §9.4 信号累积不丢失 | 唤醒/立即命中均按命中位清除；未命中事件位保留累积（SGN3/SGN4 锚定）；与 seL4 Wait 取走整个 word 的语义差异属本决策（见 §6.2） |

## 12. 源码链接

| 模块 | 路径（含行号区间） |
|------|-------------------|
| IPC 子系统挂载（头注 D1~D14 决策记录 + pub use 导出） | [crates/kernel/microkernel/src/ipc/mod.rs L1~L81](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/mod.rs) |
| notification 对象（NotificationId / MAX_NOTIFICATIONS / WAITER_QUEUE_CAP / WaiterRing / Notification + NTF1~NTF3 单测） | [crates/kernel/microkernel/src/ipc/notification.rs L1~L236](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/notification.rs) |
| — WaiterRing 定长等待者环（D9） | [notification.rs L33~L104](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/notification.rs) |
| — Notification 五字段 | [notification.rs L106~L126](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/notification.rs) |
| — NTF1~NTF3 内嵌单测 | [notification.rs L128~L236](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/notification.rs) |
| 通知信号管理器（NotifyStats / NotificationManager / NotificationOps trait + SGN1~SGN7 单测） | [crates/kernel/microkernel/src/ipc/signal.rs L1~L680](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/signal.rs) |
| — NotificationOps trait 七方法（D13 含 tid、D10 注入调度器） | [signal.rs L88~L131](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/signal.rs) |
| — signal（fetch_or Release 置位 + 扫描唤醒 + D14 清位 + FIFO 重排） | [signal.rs L190~L238](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/signal.rs) |
| — wait（立即命中 vs 阻塞登记 WouldBlock 语义）/ poll / bind_irq / take_wait_result / irq_signal | [signal.rs L240~L315](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/signal.rs) |
| — SGN1~SGN7 内嵌单测（run_big_stack 8MB 夹具） | [signal.rs L317~L680](file:///e:/EnerOS/crates/kernel/microkernel/src/ipc/signal.rs) |
| 内核启动期接入（§3.14 notify_smoke 段：主控 hub 仲裁切换 D-I1 复用 + NTFY_ROUNDS=100 + NTFY_LAT_MAX_NS=1_500_000 + NTFY_TEST_IRQ=33 及 D-N1 决策注释） | [crates/kernel/microkernel/src/main.rs L2832~L3368](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — 通知支撑静态量与判定常量（NTFY_MANAGER BSS 驻留 / NTFY_NID / 完成标志 / 延迟统计 / 首错记录 / NTFY_ROUNDS / NTFY_ACTIVATION_CAP / NTFY_LAT_MAX_NS / NTFY_TEST_IRQ） | [main.rs L2832~L2908](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — ntfy_server_entry / ntfy_client_entry（100 轮 wait/signal 配对线程体 + CNTVCT 夹取） | [main.rs L2910~L3046](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — notify_smoke 主体（决策记录含 D-N1 全文 + ①有效化 → ②create → ③双线程 → ④主控交替直切 → ⑤poll/bind_irq 两路径 → ⑥统计判定） | [main.rs L3048~L3368](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — rust_main 调用点（`[ipc] ok` 后追加 §3.14） | [main.rs L838~L850](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| host 集成测试 NTI1~NTI8 | [crates/kernel/microkernel/tests/notification_test.rs L1~L477](file:///e:/EnerOS/crates/kernel/microkernel/tests/notification_test.rs) |
| QEMU 测试脚本（第十三条断言 `[notify] ok`） | [tests/boot_test.sh](file:///e:/EnerOS/tests/boot_test.sh) |
| QEMU 实测记录（串口全文 / 十三断言 / 延迟判定口径说明 / 首轮一次 PASS） | [docs/boot/qemu_run.md](file:///e:/EnerOS/docs/boot/qemu_run.md) §19（L678~L747） |
| 同步 IPC 设计文档（v0.139.0，D-I1 主控 hub 切换与 1.5ms 放宽阈值先例来源） | [docs/kernel/ipc_design.md](file:///e:/EnerOS/docs/kernel/ipc_design.md) |
| WCET 验证报告（v0.138.0，CNTVCT/cycles_to_ns 测量设施复用来源） | [docs/kernel/wcet_report.md](file:///e:/EnerOS/docs/kernel/wcet_report.md) |
| 线程调度设计文档（v0.134.0，block/unblock 与显式 TCB 对切换设施来源） | [docs/kernel/scheduler_design.md](file:///e:/EnerOS/docs/kernel/scheduler_design.md) |
