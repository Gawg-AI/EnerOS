# ARINC 653 适配说明

> 版本：v0.19.0 | 日期：2026-07-13 | 状态：已实现
> 蓝图依据：`phase0.md §v0.19.0`、`Power_Native_Agent_OS_Blueprint.md §4`（调度算法）、§43.1（no_std 合规）、§43.2（非瓶颈版本）
> 实现位置：`crates/kernel/sched/src/partition_sched.rs`、`crates/kernel/sched/src/wcet.rs`
> 配套文档：`docs/smp/partition-scheduler-design.md`、`docs/smp/wcet-analysis.md`

## 1. ARINC 653 标准概述

ARINC 653（Avionics Application Standard Software Interface）是航空电子设备
软件领域的行业标准，由航空电子工程委员会（AEEC）制定。其核心目标是在
**综合模块化航电（IMA）** 平台上为不同关键等级的应用提供 **时间与空间隔离**，
保证一个应用的故障不会影响其他应用。

### 1.1 核心概念

| 概念 | 中文 | 说明 |
|------|------|------|
| Partition | 分区 | 时间与空间隔离的应用执行单元 |
| Major Frame | 主帧 | 一个完整调度周期，包含多个 Minor Frame |
| Minor Frame | 子帧 | 主帧内的一段时间窗口，分配给特定分区 |
| Partition Schedule | 分区调度表 | 定义各分区在主帧中的时间片分配 |
| Health Monitor | 健康监控 | 检测与处理分区故障的机制 |
| Partition Mode | 分区模式 | 冷启动 / 正常 / 停止等运行模式 |
| Inter-Partition Communication | 分区间通信 | 分区间的消息传递（IPC） |

### 1.2 时间触发分区调度原理

ARINC 653 调度的核心是 **时间触发**（time-triggered）而非 **事件触发**
（event-driven）：

1. **主帧周期循环**：一个 Major Frame 按预定义的时间片（slot）序列执行，
   主帧结束后从头开始循环。
2. **分区时间隔离**：每个分区在自己的时间片内独占 CPU，其他分区无法抢占。
3. **空间隔离**：每个分区有独立的地址空间（MMU 隔离），本版本不涉及
   （属于 Phase 3 seL4 范畴）。
4. **确定性**：由于调度表固定，每个分区在每个主帧周期内获得固定的 CPU
   时间，响应时间可预测。

```text
Major Frame (周期 30ms)
├── Slot 0: Partition A (5ms)   ← RTOS 控制大区
├── Slot 1: Partition B (20ms)  ← Agent Runtime
└── Slot 2: Partition A (5ms)   ← RTOS 控制大区
                                ↑ 主帧结束，回到 Slot 0 循环
```

### 1.3 为什么要借鉴 ARINC 653

EnerOS 面向电力行业，其场景具有 **混合关键性** 特征：

| 大区 | 关键性 | 周期 | 例子 |
|------|--------|------|------|
| RTOS 控制大区 | 安全关键 | 10ms | 继电保护、AGC 调频 |
| Agent Runtime（管理信息大区） | 业务关键 | 100ms | LLM 推理、Solver 求解 |
| 通信大区 | 业务关键 | 50ms | IEC 104 / Modbus |

ARINC 653 的时间触发调度天然适合这种混合关键性场景：安全关键分区获得
固定时间片，不受业务分区影响；业务分区在自己的时间片内运行，不会饿死
安全分区。这与电力行业「横向隔离、纵向加密」的安全要求（36 号文）契合。

## 2. EnerOS 适配范围

### 2.1 已实现（v0.19.0）

| ARINC 653 特性 | EnerOS 实现 | 位置 |
|---------------|-------------|------|
| 时间触发分区调度 | `MajorFrame` + `on_tick` | `partition_sched.rs` |
| 分区标识 | `PartitionId` 新类型 | `partition_sched.rs` |
| Major Frame 配置 | `MajorFrame::add` API | `partition_sched.rs` |
| 时间片（Slot） | `PartitionSlot { partition, duration_ms }` | `partition_sched.rs` |
| 抖动测量 | `JitterStats`（min/max/avg μs） | `partition_sched.rs` |
| WCET 估算 | `WCET_TABLE` + `check_partition_overrun` | `wcet.rs` |
| 时间源注入 | `set_time_source` / `set_timer_registrar` | `partition_sched.rs` |
| 启停控制 | `schedule_run` / `schedule_stop` | `partition_sched.rs` |

### 2.2 未实现（明确延后）

| ARINC 653 特性 | 延后版本 | 理由 |
|---------------|---------|------|
| 分区模式（冷启动/正常/停止） | 未来版本 | 本版本仅支持正常运行模式 |
| 健康监控表（HM） | 未来版本 | 需定义故障分类与处理策略 |
| 分区间通信（IPC） | v0.20.0 | 需消息队列与端口机制 |
| 动态重配置 | 未来版本 | 需安全切换协议 |
| 分区内存隔离 | Phase 3（seL4） | 依赖 MMU 虚拟化 |
| 进程（Process）概念 | 未来版本 | ARINC 653 分区内可有多个进程 |
| 时钟服务（TIMED_WAIT） | 未来版本 | 依赖用户态系统调用 |
| 中断处理（INTERRUPT handling） | 未来版本 | 依赖中断虚拟化 |

### 2.3 简化项

本版本对 ARINC 653 做了以下简化：

1. **无分区模式状态机**：ARINC 653 定义了 `COLD_START / WARM_START / NORMAL /
   IDLE / STOP` 五种模式，本版本仅支持 `NORMAL`（启动即运行，停止即退出）。
2. **无优先级抢占**：ARINC 653 允许分区内优先级抢占调度，本版本分区内调度
   复用 v0.18.0 `select_next_by_priority`（非抢占式，FIFO + 优先级）。
3. **无 ARINC 653 API 符合性**：本版本不提供 `SET_PARTITION_MODE /
   CREATE_PROCESS / SEND_CHANNEL_MESSAGE` 等标准 API，仅提供 Rust 风格的
   `schedule_run / schedule_add` API。
4. **无形式化认证**：本版本不追求 DO-178C 等航空认证，仅借鉴调度模型。

## 3. 与完整 ARINC 653 的差异

### 3.1 差异对照表

| 维度 | 完整 ARINC 653 | EnerOS v0.19.0 |
|------|---------------|----------------|
| 应用领域 | 航空电子 | 电力系统 |
| 隔离强度 | 空间 + 时间 | 仅时间（空间隔离 Phase 3） |
| 分区模式 | 5 种 | 1 种（NORMAL） |
| 调度方式 | 时间触发 + 分区内优先级抢占 | 时间触发 + 分区内优先级非抢占 |
| IPC | 采样/队列端口 | 未实现（v0.20.0） |
| 健康监控 | HM 表 + 故障处理 | 未实现 |
| API 风格 | C 标准 APEX | Rust 原生 |
| 认证目标 | DO-178C | 无（电力等保 2.0） |
| 硬件平台 | IMA 专用 | 飞腾 / 鲲鹏 / QEMU |

### 3.2 电力场景适配

EnerOS 对 ARINC 653 的适配调整：

1. **时间单位**：ARINC 653 用纳秒，本版本主帧配置用毫秒（`duration_ms`），
   抖动统计用微秒（`JitterStats` 用 μs），WCET 用纳秒。这是因为电力场景
   控制周期为 10ms 量级，毫秒配置更直观。
2. **分区数量**：ARINC 653 典型 4~8 个分区，本版本上限 16 个 slot
   （`MAX_SLOTS = 16`），足够电力场景。
3. **分区定义**：本版本分区语义为「RTOS 控制大区 / 管理信息大区 / 通信大区」，
   而非航电的「显示 / 导航 / 飞控」分区。
4. **错误处理**：ARINC 653 用 HM 表处理故障，本版本用 `Result<(), &'static str>`
   返回错误（D7 决策），更符合 Rust 习惯。

### 3.3 无 ARINC 653 符合性认证目标

明确声明：**EnerOS v0.19.0 不追求 ARINC 653 符合性认证**。原因：

1. ARINC 653 认证面向航空电子场景，认证成本高、周期长。
2. 电力行业无强制 ARINC 653 要求，等保 2.0 与 36 号文不涉及。
3. 本版本仅借鉴 ARINC 653 的 **时间触发分区调度模型**，用于实现混合关键性
   隔离，而非完整复刻标准。

## 4. Major Frame 配置示例

### 4.1 典型配置：RTOS 5ms + Agent 20ms + RTOS 5ms

```rust
use eneros_sched::partition_sched::*;

/// 分区 ID 常量
const RTOS: PartitionId = PartitionId::new(0);
const AGENT: PartitionId = PartitionId::new(1);

fn build_typical_frame() -> MajorFrame {
    let mut frame = MajorFrame::new();
    // RTOS 控制大区先运行 5ms（保证 10ms 控制周期的前半段）
    frame.add(PartitionSlot::new(RTOS, 5)).unwrap();
    // Agent Runtime 运行 20ms（LLM 推理/Solver 求解）
    frame.add(PartitionSlot::new(AGENT, 20)).unwrap();
    // RTOS 再运行 5ms（保证 10ms 控制周期的后半段）
    frame.add(PartitionSlot::new(RTOS, 5)).unwrap();
    // 主帧总周期 = 30ms
    assert_eq!(frame.total_duration_ms(), 30);
    frame
}
```

时序图：

```text
时间(ms):  0    5         25   30   35        55   60
           │    │          │    │    │          │    │
分区:      ├───RTOS──┤───AGENT──┤─RTOS├───AGENT──┤─RTOS┤
           │   5ms   │   20ms   │ 5ms │   20ms   │ 5ms │
           │                                   │
           └────── 主帧周期 30ms ──────────────┘
```

RTOS 分区每 10ms 获得一次 CPU（5ms + 5ms），满足 10ms 控制周期；Agent 分区
每 30ms 获得 20ms CPU 时间，足够 LLM 推理与 Solver 求解。

### 4.2 三分区配置：RTOS + 通信 + Agent

```rust
const RTOS: PartitionId = PartitionId::new(0);
const COMM: PartitionId = PartitionId::new(1);   // 通信大区
const AGENT: PartitionId = PartitionId::new(2);  // Agent Runtime

fn build_three_partition_frame() -> MajorFrame {
    let mut frame = MajorFrame::new();
    frame.add(PartitionSlot::new(RTOS, 3)).unwrap();   // RTOS 3ms
    frame.add(PartitionSlot::new(COMM, 2)).unwrap();   // 通信 2ms（IEC 104）
    frame.add(PartitionSlot::new(AGENT, 15)).unwrap(); // Agent 15ms
    frame.add(PartitionSlot::new(RTOS, 3)).unwrap();   // RTOS 3ms
    frame.add(PartitionSlot::new(COMM, 2)).unwrap();   // 通信 2ms
    frame.add(PartitionSlot::new(AGENT, 15)).unwrap(); // Agent 15ms
    // 主帧周期 = 40ms
    assert_eq!(frame.total_duration_ms(), 40);
    frame
}
```

### 4.3 配置注意事项

1. **slot 数量上限**：最多 16 个 slot（`MAX_SLOTS = 16`）。
2. **总周期与控制周期对齐**：主帧总周期应能被控制周期整除。例如 10ms 控制
   周期，主帧总周期应为 10/20/30/40ms，保证每个主帧内 RTOS 都能按时运行。
3. **WCET 检查**：每个 slot 的 `duration_ms` 必须大于该分区内所有线程的
   WCET（见 `docs/smp/wcet-analysis.md`），否则 `check_partition_overrun`
   会报错。
4. **抖动预算**：`duration_ms` 应预留抖动余量。例如实际需要 4.5ms，配置
   5ms，预留 0.5ms 抖动余量。

## 5. 分区超时处理

### 5.1 超时检测机制

ARINC 653 要求分区不能超过其时间片执行，否则视为故障。EnerOS v0.19.0
通过 WCET 表实现超时检测：

```rust
use eneros_sched::wcet::*;

/// 检测分区内是否有线程 WCET 超过 slot 时长
fn verify_partition_timings(frame: &MajorFrame) -> Result<(), &'static str> {
    for i in 0..frame.count() {
        let slot = frame.slot(i).unwrap();
        let overrun = check_partition_overrun(slot.partition, slot.duration_ms * 1_000_000);
        if !overrun.is_empty() {
            return Err("partition WCET overrun detected");
        }
    }
    Ok(())
}
```

### 5.2 超时处理策略

| 场景 | 处理方式 | 说明 |
|------|---------|------|
| WCET 配置超限 | 启动前拒绝 `schedule_run` | 静态检查 |
| 运行时超限（本版本） | 记录到 `JitterStats` | 不强制切换 |
| 运行时超限（未来） | 触发看门狗复位 | 与 eneros-watchdog 集成 |
| 分区无响应 | 健康监控标记故障 | 未来版本 |

### 5.3 强制切换（本版本不实现）

完整 ARINC 653 要求时间片到期时强制切换分区（即使当前分区未执行完）。
本版本 `on_tick` 仅推进 slot 索引，**不强制中断当前分区**（D11 决策）。
真正的强制切换依赖硬件定时器中断 + 上下文切换，将在 QEMU 验证阶段
完善（D8 决策）。

### 5.4 违规记录

本版本通过 `JitterStats` 间接记录超时：若实际执行时间超过 `duration_ms`，
`on_tick` 计算的抖动会为正值，`max_us` 会反映最大超时量。

```rust
// 读取抖动统计
let state = SCHED_STATE.lock();
if state.jitter.max_us > 1000 {
    // 超过 1ms 抖动，可能存在超时
    log_warn!("partition jitter exceeds 1ms: max={}us", state.jitter.max_us);
}
```

## 6. 电力调度可借鉴点

### 6.1 时间触发确定性在电力 SCADA/EMS 中的应用

电力 SCADA（数据采集与监控）与 EMS（能量管理系统）传统上采用 **周期扫描**
机制：每隔固定周期（如 1~3 秒）扫描一次现场数据。这与 ARINC 653 的时间
触发调度高度契合：

| ARINC 653 概念 | 电力 SCADA 对应 |
|---------------|----------------|
| Major Frame | 扫描周期（如 2 秒） |
| Partition | 功能模块（遥测/遥信/遥控/AGC） |
| Slot | 各功能模块的执行时间片 |
| WCET | 各功能模块的最坏执行时间 |
| Jitter | 扫描周期的抖动 |

借鉴 ARINC 653 的好处：

1. **确定性**：每个功能模块在固定时间窗口执行，避免低优先级模块饿死。
2. **可验证性**：每个分区独立验证 WCET，无需全局调度分析。
3. **混合关键性**：安全关键功能（继电保护）与业务功能（报表生成）隔离。

### 6.2 混合关键性系统设计

EnerOS 的混合关键性分区设计：

```text
┌─────────────────────────────────────────┐
│           硬件平台（飞腾/鲲鹏）            │
├─────────────────────────────────────────┤
│  Partition 0: RTOS 控制大区 (安全关键)    │
│  - 继电保护逻辑                          │
│  - AGC 自动发电控制                      │
│  - 10ms 控制周期                         │
├─────────────────────────────────────────┤
│  Partition 1: Agent Runtime (业务关键)   │
│  - LLM 推理（离线规划）                  │
│  - Solver 求解（LP/MILP）                │
│  - 100ms 业务周期                        │
├─────────────────────────────────────────┤
│  Partition 2: 通信大区 (业务关键)         │
│  - IEC 104 / Modbus 协议栈               │
│  - 50ms 通信周期                         │
└─────────────────────────────────────────┘
         时间触发分区调度（v0.19.0）
```

### 6.3 与 36 号文横向隔离的关系

36 号文要求电力安全区（控制大区）与管理信息大区 **横向隔离**。本版本
通过时间隔离实现了 CPU 资源的横向隔离：

| 隔离维度 | 实现方式 | 版本 |
|---------|---------|------|
| CPU 时间隔离 | 分区调度（v0.19.0） | ✅ 本版本 |
| 内存空间隔离 | seL4 MMU（Phase 3） | 未来 |
| 网络隔离 | 纵向加密认证（v0.98.1） | Phase 2 |
| 设备隔离 | 设备树 + 驱动权限 | 未来 |

本版本的时间隔离是横向隔离的 **第一道防线**，保证即使 Agent Runtime
发生故障（如 LLM 推理死循环），RTOS 控制大区仍能在自己的时间片内正常
执行控制逻辑。

## 7. 参考资料

### 7.1 标准文档

- **ARINC Specification 653P1-3**: Avionics Application Software Standard
  Interface, Part 1, Required Services. Aeronautical Radio, Inc.
- **ARINC Specification 653P2-3**: Avionics Application Software Standard
  Interface, Part 2, Optional Services.
- **DO-178C**: Software Considerations in Airborne Systems and Equipment
  Certification（参考，EnerOS 不追求此认证）。

### 7.2 EnerOS 内部文档

- `蓝图/phase0.md §v0.19.0`—— 本版本蓝图
- `蓝图/Power_Native_Agent_OS_Blueprint.md §4`—— 调度算法
- `蓝图/Power_Native_Agent_OS_Blueprint.md §43.7`—— 合规矩阵（横向隔离）
- `docs/smp/partition-scheduler-design.md`—— 分区调度器设计（配套）
- `docs/smp/wcet-analysis.md`—— WCET 分析（配套）
- `docs/smp/multi-core-scheduler-design.md`—— v0.16.0 多核调度器
- `docs/smp/thread-abstraction-design.md`—— v0.18.0 线程抽象

### 7.3 学术参考

- Rushby, J. "Partitioning for Avionics Safety: An Introduction to the
  ARINC 653 Standard." SRI International, 2008.
- Crum, E. "ARINC 653 Partitioning: Concepts and Benefits." Wind River
  Systems, 2010.

## 8. 术语表

| 术语 | 英文 | 说明 |
|------|------|------|
| 主帧 | Major Frame | 一个完整调度周期 |
| 子帧 | Minor Frame | 主帧内的时间窗口 |
| 时间片 | Slot | 分区获得 CPU 的时间段 |
| 分区 | Partition | 时间隔离的执行单元 |
| 分区调度 | Partition Scheduling | 时间触发的调度方式 |
| 健康监控 | Health Monitor (HM) | 故障检测与处理机制 |
| 分区间通信 | Inter-Partition Communication (IPC) | 分区间消息传递 |
| WCET | Worst-Case Execution Time | 最坏情况执行时间 |
| 抖动 | Jitter | 实际时间与期望时间的偏差 |
| 横向隔离 | Lateral Isolation | 安全区与管理信息区的隔离 |
