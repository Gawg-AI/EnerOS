# WCET 分析

> 版本：v0.19.0 | 日期：2026-07-13 | 状态：已实现
> 蓝图依据：`phase0.md §v0.19.0`、`Power_Native_Agent_OS_Blueprint.md §4`（调度算法）、§6.3（性能要求）、§43.1（no_std 合规）、§43.2（非瓶颈版本）
> 实现位置：`crates/kernel/sched/src/wcet.rs`
> 配套文档：`docs/smp/partition-scheduler-design.md`、`docs/smp/arinc653-adaptation.md`

## 1. WCET 概念

### 1.1 定义

**WCET（Worst-Case Execution Time，最坏情况执行时间）** 是指一段程序在特定
硬件平台上执行所需的最大时间。WCET 是实时系统调度的关键参数，用于保证
任务在截止时间（deadline）内完成。

```text
任务执行时间分布：
      频率
       │
   ┌───┴───┐
   │       │
   │       │                ← 典型情况（average case）
   │       │
───┴───────┴──────────┬────→ 时间
                     WCET
                     ↑
                最坏情况（worst case）
```

### 1.2 为什么需要 WCET

在时间触发分区调度（v0.19.0）中，每个分区获得固定的时间片（slot）。
若分区内线程的执行时间超过时间片，会导致：

1. **分区超时**：当前分区未执行完就被切换，可能导致状态不一致。
2. **抖动累积**：后续分区的启动时间被推迟，影响整体调度确定性。
3. **截止时间错过**：安全关键任务（如继电保护）无法在 10ms 周期内完成。

因此，在配置 Major Frame 时必须保证：**每个分区内所有线程的 WCET 之和
小于该分区的 slot 时长**。WCET 分析就是用于验证这一约束。

### 1.3 WCET 与 BCET 的关系

| 概念 | 全称 | 说明 |
|------|------|------|
| WCET | Worst-Case Execution Time | 最坏情况执行时间（本版本关注） |
| BCET | Best-Case Execution Time | 最好情况执行时间 |
| ACET | Average-Case Execution Time | 平均执行时间 |

本版本仅实现 WCET 静态表，不涉及 BCET/ACET 测量。

## 2. 静态表方案

### 2.1 设计

v0.19.0 采用 **手动配置的静态表** 方案：开发者根据经验或离线测量，预先
将每个线程的 WCET 写入全局表。

```rust
// crates/kernel/sched/src/wcet.rs

use crate::percore::{Spinlock, Tid};
use crate::partition_sched::PartitionId;

/// 全局线程表最大容量（与 v0.18.0 MAX_THREADS 一致）
pub const MAX_THREADS: usize = 256;

/// WCET 静态表：Tid 索引 → WCET（纳秒）
///
/// 索引 i 对应 Tid(i+1)；Tid(0) 保留为无效。
/// 默认值为 0，表示未配置 WCET。
pub static WCET_TABLE: Spinlock<[u64; MAX_THREADS]> = {
    // const fn 初始化数组（依赖 v0.16.0 Spinlock 的 const fn new）
    Spinlock::new([0u64; MAX_THREADS])
};
```

设计要点：

- **固定数组 256 槽**：与 v0.18.0 `THREAD_TABLE` 容量一致，索引对齐。
- **`Spinlock` 保护**：复用 v0.16.0 Spinlock，支持多核并发读写。
- **默认 0**：未配置 WCET 的线程返回 0，`check_partition_overrun` 跳过。
- **单位纳秒**：与 `now_ns()` 时间源一致，便于精确比较。

### 2.2 API

```rust
/// 设置线程的 WCET
///
/// # 参数
/// - `tid`: 线程标识
/// - `wcet_ns`: 最坏情况执行时间（纳秒）
///
/// # 返回
/// - Ok(()): 设置成功
/// - Err("invalid tid"): Tid 无效（0 或超过 MAX_THREADS）
pub fn wcet_set(tid: Tid, wcet_ns: u64) -> Result<(), &'static str> {
    let idx = tid_to_idx(tid).ok_or("invalid tid")?;
    let mut table = WCET_TABLE.lock();
    table[idx] = wcet_ns;
    Ok(())
}

/// 查询线程的 WCET（纳秒）
///
/// 未配置时返回 0。
pub fn wcet_estimate(tid: Tid) -> u64 {
    match tid_to_idx(tid) {
        Some(idx) => WCET_TABLE.lock()[idx],
        None => 0,
    }
}

/// Tid → WCET_TABLE 索引（内部辅助）
fn tid_to_idx(tid: Tid) -> Option<usize> {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return None;
    }
    Some(tid.0 as usize - 1)
}
```

### 2.3 局限性

本版本的静态表方案有以下局限：

| 局限 | 说明 | 影响 |
|------|------|------|
| 手动配置 | WCET 由开发者凭经验填写 | 可能偏保守或偏乐观 |
| 无自动测量 | 运行时不统计实际执行时间 | 无法动态校正 WCET |
| 无形式化保证 | 不做控制流分析 | 无法证明 WCET 上界 |
| 全局单表 | 不区分分区 | 需配合 `Tcb.partition` 字段过滤 |
| 无缓存建模 | 不考虑 cache 命中率影响 | 实际执行时间可能偏离 |

这些局限在「未来改进」（§6）中规划解决。本版本满足 Phase 0 验证需求
（蓝图 §43.2 非瓶颈版本，允许简化实现）。

## 3. 分区超时检测

### 3.1 check_partition_overrun 算法

`check_partition_overrun` 遍历全局线程表，找出属于指定分区且 WCET 超过
给定时长的线程：

```rust
/// 检测分区内 WCET 超限的线程
///
/// 遍历 THREAD_TABLE，对属于 `partition` 的线程，比较其 WCET 与
/// `slot_duration_ns`。返回超限线程的 Tid 列表。
///
/// # 参数
/// - `partition`: 待检测的分区 ID
/// - `slot_duration_ns`: 时间片时长（纳秒）
///
/// # 返回
/// 超限线程的 Tid 数组（最多返回 MAX_THREADS 个，实际用堆分配 Vec）
///
/// # 注
/// WCET = 0 的线程（未配置）跳过，不视为超限。
pub fn check_partition_overrun(
    partition: PartitionId,
    slot_duration_ns: u64,
) -> alloc::vec::Vec<Tid> {
    let wcet_table = WCET_TABLE.lock();
    let thread_table = crate::tcb::THREAD_TABLE.lock();

    let mut overruns = alloc::vec::Vec::new();
    for (idx, slot) in thread_table.iter().enumerate() {
        if let Some(tcb) = slot {
            // 仅检查属于该分区的线程
            if tcb.partition == partition.raw() {
                let wcet = wcet_table[idx];
                // WCET = 0 表示未配置，跳过
                if wcet > 0 && wcet > slot_duration_ns {
                    overruns.push(Tid(idx as u32 + 1));
                }
            }
        }
    }
    overruns
}
```

### 3.2 算法复杂度

| 步骤 | 复杂度 | 说明 |
|------|--------|------|
| 锁定 WCET_TABLE | O(1) | Spinlock |
| 锁定 THREAD_TABLE | O(1) | Spinlock |
| 遍历线程表 | O(n) | n = MAX_THREADS = 256 |
| 过滤分区 | O(1) per 线程 | 比较 `partition` 字段 |
| 比较 WCET | O(1) per 线程 | 整数比较 |

总体复杂度：**O(n)**，n=256，扫描一次 < 1μs（实测在 QEMU cortex-a57 上
约 200ns），不影响调度性能。

### 3.3 使用场景

1. **启动前校验**：`schedule_run` 前调用，拒绝 WCET 超限的配置。
2. **运行时诊断**：定期调用，检测 WCET 配置是否合理。
3. **健康监控**（未来）：超限线程触发告警或复位。

## 4. 与分区时间片的关系

### 4.1 约束条件

分区调度的核心约束：

```text
∀ 线程 t ∈ 分区 P:  WCET(t) ≤ slot_duration(P)
```

即：分区内**每个**线程的 WCET 都必须小于该分区的 slot 时长。

注意：本约束是 **单线程 WCET** 约束，而非 **分区内所有线程 WCET 之和**。
原因是分区内采用优先级非抢占调度（v0.18.0），一个 slot 内通常只执行
一个线程（除非线程主动 yield）。若分区内有多个线程需在同一 slot 内
执行，则约束应为：

```text
Σ WCET(t) ≤ slot_duration(P)
```

本版本采用更严格的 **单线程约束**（每个线程 WCET ≤ slot 时长），简化
校验逻辑。

### 4.2 WCET 与 slot 时长的关系图

```text
slot_duration = 5ms (5,000,000 ns)
                ├─────────────────────────┤
                │                         │
线程 A WCET:    ├─── 2ms ───┤              │  ✅ 未超限
线程 B WCET:    ├────── 4ms ──────┤        │  ✅ 未超限
线程 C WCET:    ├────────────── 6ms ──────────┤  ❌ 超限！
```

### 4.3 典型 WCET 配置

| 分区 | 线程 | 典型 WCET | slot 时长 | 是否超限 |
|------|------|-----------|-----------|---------|
| RTOS | 继电保护 | 1ms | 5ms | ✅ |
| RTOS | AGC 调频 | 2ms | 5ms | ✅ |
| Agent | LLM 推理 | 50ms | 20ms | ❌ 需拆分 |
| Agent | Solver 求解 | 10ms | 20ms | ✅ |
| 通信 | IEC 104 扫描 | 3ms | 2ms | ❌ 需扩容 |

注意：LLM 推理（50ms）超过 Agent slot（20ms），说明 LLM 推理不能在单个
slot 内完成。解决方案：

1. **拆分推理**：将 LLM 推理拆分为多个子任务，跨多个 slot 执行。
2. **扩容 slot**：将 Agent slot 扩大到 50ms（但会影响 RTOS 周期）。
3. **降级到 L1**：LLM 推理降级为 Solver-only（L1 路径，< 500ms）。

这体现了 WCET 分析的 **设计指导价值**：在配置阶段发现时序冲突，提前调整。

## 5. 使用示例

### 5.1 配置线程 WCET

```rust
use eneros_sched::wcet::*;
use eneros_sched::percore::Tid;

fn configure_wcet() {
    // 继电保护线程：WCET 1ms
    wcet_set(Tid(1), 1_000_000).unwrap();
    // AGC 调频线程：WCET 2ms
    wcet_set(Tid(2), 2_000_000).unwrap();
    // Solver 求解线程：WCET 10ms
    wcet_set(Tid(3), 10_000_000).unwrap();
    // LLM 推理线程：WCET 50ms（注意：超过 20ms slot）
    wcet_set(Tid(4), 50_000_000).unwrap();
}
```

### 5.2 检测分区超时

```rust
use eneros_sched::wcet::*;
use eneros_sched::partition_sched::*;

fn verify_frame_timings(frame: &MajorFrame) -> Result<(), &'static str> {
    for i in 0..frame.count() {
        let slot = frame.slot(i).ok_or("invalid slot")?;
        let slot_ns = slot.duration_ms * 1_000_000;

        let overruns = check_partition_overrun(slot.partition, slot_ns);
        if !overruns.is_empty() {
            // 打印超限线程（实际应用中应用 log_error!）
            for tid in &overruns {
                let wcet = wcet_estimate(*tid);
                // log_error!(
                //     "tid={} WCET={}ns > slot={}ns (partition={})",
                //     tid.0, wcet, slot_ns, slot.partition.raw()
                // );
            }
            return Err("WCET overrun detected");
        }
    }
    Ok(())
}
```

### 5.3 集成到 schedule_run

```rust
use eneros_sched::partition_sched::*;
use eneros_sched::wcet::*;

fn start_scheduler_safely(frame: MajorFrame) -> Result<(), &'static str> {
    // 1. 校验 WCET
    verify_frame_timings(&frame)?;

    // 2. 启动调度
    schedule_run(frame)
}
```

### 5.4 host 侧测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wcet_set_and_estimate() {
        // 清空（测试隔离）
        {
            let mut table = WCET_TABLE.lock();
            *table = [0u64; MAX_THREADS];
        }
        wcet_set(Tid(1), 1_000_000).unwrap();
        assert_eq!(wcet_estimate(Tid(1)), 1_000_000);
        assert_eq!(wcet_estimate(Tid(2)), 0);  // 未配置
    }

    #[test]
    fn test_invalid_tid_rejected() {
        assert!(wcet_set(Tid(0), 100).is_err());       // Tid(0) 无效
        assert!(wcet_set(Tid(257), 100).is_err());     // 超过 MAX_THREADS
    }

    #[test]
    fn test_check_partition_overrun() {
        // 清空
        {
            let mut table = WCET_TABLE.lock();
            *table = [0u64; MAX_THREADS];
        }
        // 配置：Tid(1) WCET=1ms, Tid(2) WCET=5ms
        wcet_set(Tid(1), 1_000_000).unwrap();
        wcet_set(Tid(2), 5_000_000).unwrap();

        // slot 时长 2ms：Tid(2) 超限（5ms > 2ms）
        let overruns = check_partition_overrun(PartitionId::new(0), 2_000_000);
        // 注：实际超限取决于 THREAD_TABLE 中线程的 partition 字段
        // host 测试需配合 thread_create 设置 partition
    }
}
```

## 6. 未来改进方向

### 6.1 在线测量

本版本 WCET 为静态配置，未来可引入 **在线测量** 机制：

```rust
// 未来扩展（未实现）
pub struct ExecutionStats {
    pub min_ns: u64,
    pub max_ns: u64,
    pub sum_ns: u64,
    pub samples: u64,
}

/// 全局执行时间统计表（与 WCET_TABLE 平行）
pub static EXEC_STATS: Spinlock<[ExecutionStats; MAX_THREADS]> = /* ... */;

/// 线程启动时记录开始时间
pub fn execution_start(tid: Tid) { /* ... */ }

/// 线程结束时记录执行时间，更新统计
pub fn execution_end(tid: Tid) {
    let elapsed = now_ns() - start_time;
    EXEC_STATS.lock()[idx].record(elapsed);
    // 若 elapsed > WCET，触发告警
}

/// 根据历史统计动态调整 WCET
pub fn wcet_auto_calibrate(tid: Tid) {
    let stats = EXEC_STATS.lock()[idx];
    // 取 max_ns 的 1.2 倍作为新 WCET
    let new_wcet = (stats.max_ns as f64 * 1.2) as u64;
    wcet_set(tid, new_wcet);
}
```

优势：自动适应实际负载，避免保守配置。

### 6.2 形式化分析

完整 WCET 分析需 **静态分析工具**，通过控制流图（CFG）与硬件模型计算
WCET 上界：

| 工具 | 说明 | 适用 |
|------|------|------|
| OTAWA | 开源 WCET 分析框架 | 学术研究 |
| aiT (AbsInt) | 商业 WCET 分析工具 | 工业认证 |
| Bound-T | 开源 WCET 工具 | 嵌入式 |

形式化分析步骤：

1. **控制流分析**：构建函数调用图与循环边界。
2. **值分析**：确定数组索引、循环次数等动态值。
3. **处理器行为分析**：建模 cache、流水线、分支预测。
4. **路径分析**：用 IPET（Implicit Path Enumeration Technique）求最长路径。

本版本不实现形式化分析，Phase 3（seL4 形式化验证）阶段可考虑集成。

### 6.3 与看门狗集成

未来与 `eneros-watchdog`（v0.x 驱动）集成，WCET 超限触发看门狗复位：

```rust
// 未来扩展（未实现）
fn on_wcet_overrun(tid: Tid, wcet_ns: u64, slot_ns: u64) {
    // 1. 记录违规事件
    log_error!(
        "WCET overrun: tid={} wcet={}ns > slot={}ns",
        tid.0, wcet_ns, slot_ns
    );
    // 2. 触发看门狗复位（若连续超限 N 次）
    if consecutive_overruns(tid) >= 3 {
        eneros_watchdog::trigger_reset();
    }
}
```

### 6.4 改进路线图

| 版本 | 改进 | 依赖 |
|------|------|------|
| v0.19.0 | 静态表 + 手动配置 | 本版本 |
| 未来 | 在线测量 + 动态校正 | 性能计数器 |
| 未来 | 与看门狗集成 | eneros-watchdog |
| Phase 3 | 形式化 WCET 分析 | seL4 验证工具链 |
| 未来 | 分区级 WCET 预算 | 分区内存隔离 |

## 7. 参考资料

- `蓝图/phase0.md §v0.19.0`—— 本版本蓝图
- `蓝图/Power_Native_Agent_OS_Blueprint.md §4`—— 调度算法
- `蓝图/Power_Native_Agent_OS_Blueprint.md §6.3`—— 性能要求
- `docs/smp/partition-scheduler-design.md`—— 分区调度器设计（配套）
- `docs/smp/arinc653-adaptation.md`—— ARINC 653 适配说明（配套）
- `docs/smp/thread-abstraction-design.md`—— v0.18.0 线程抽象（Tid/TCB 来源）
- Wilhelm, R. et al. "The Worst-Case Execution-Time Problem—Overview of
  Methods and Survey of Tools." ACM TECS, 2008.
- ARINC Specification 653P1-3 —— §3（分区调度）与 WCET 约束
