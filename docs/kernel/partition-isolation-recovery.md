# 分区隔离恢复策略

> 版本：v0.14.0 | 日期：2026-07-12 | 状态：设计文档

## 1. 概述

分区隔离是 EnerOS 混合关键性架构的核心机制之一。当某一分区内发生 panic，
系统应仅隔离该故障分区，而非触发全局复位，从而保证其他分区（尤其是 RTOS 控制路径）
继续运行。本特性对应 `phase0.md §v0.14.0`，在 `eneros-panic` crate 的
`panic/src/isolation.rs` 模块中实现。

v0.14.0 实现的是「隔离标记」这一基础能力：分区状态表、`mark_partition_dead`、
handler 注册查询。真正的资源回收（线程 TCB 回收、调度器移除）由 v0.18.0 调度器版本完成。

## 2. 分区状态模型

### 2.1 PartitionState 枚举

```rust
// panic/src/isolation.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// Partition is running normally.
    Alive,
    /// Partition has panicked and been fenced off.
    Dead,
}
```

### 2.2 PARTITION_TABLE

固定 8 槽静态数组，每个槽独立 `spin::Mutex` 保护，全部初始化为 `Alive`。
采用静态数组而非 `Vec` 是为了满足 no_std 无堆约束（蓝图 §43.1）。

```rust
// panic/src/isolation.rs
pub const MAX_PARTITIONS: usize = 8;

static PARTITION_TABLE: [Mutex<PartitionState>; MAX_PARTITIONS] = [
    Mutex::new(PartitionState::Alive),
    // ... 8 个槽 ...
];
```

### 2.3 状态转换图

```
         mark_partition_dead(id)
Alive  ───────────────────────────►  Dead
  ▲                                   │
  │            reset_partition(id)     │
  └───────────────────────────────────┘
                  (仅调试/测试用)
```

`Alive → Dead` 是生产路径；`Dead → Alive` 由 `reset_partition` 提供，仅作为测试与调试
辅助接口，正常运行期不会自动迁移回 `Alive`（重启需经内核复位或后续 SystemAgent 重启流程）。

## 3. 隔离流程

### 3.1 触发条件

`PartitionIsolateStrategy::handle` 被 `PanicStrategy` 分发机制调用时触发。
调用方需先通过 `set_partition_strategy(id)` 把对应分区的策略注册进全局 `STRATEGY`。

### 3.2 隔离步骤

1. `logger::panic_log(ctx)` 输出结构化日志。
2. `isolation::mark_partition_dead(self.partition)` 把分区标记为 `Dead`。
3. 成功则死循环等待（`loop { spin_loop(); }`），让该核停在该处。
4. 失败（id 越界或分区已 `Dead`）则升级为内核 panic 复位（见 §4.2）。

### 3.3 代码示例

```rust
// panic/src/lib.rs
impl PanicStrategy for PartitionIsolateStrategy {
    fn handle(&self, ctx: &PanicContext) -> ! {
        logger::panic_log(ctx);
        match isolation::mark_partition_dead(self.partition) {
            Ok(()) => loop {
                spin_loop();
            },
            // Isolation failed → escalate to full kernel reset.
            Err(_) => KernelResetStrategy.handle(ctx),
        }
    }
}
```

`mark_partition_dead` 实现：

```rust
// panic/src/isolation.rs
pub fn mark_partition_dead(id: u32) -> Result<(), IsolationError> {
    let i = match usize::try_from(id) {
        Ok(v) if v < MAX_PARTITIONS => v,
        _ => return Err(IsolationError::InvalidId),
    };
    let mut slot = PARTITION_TABLE[i].lock();
    if *slot == PartitionState::Dead {
        return Err(IsolationError::AlreadyDead);
    }
    *slot = PartitionState::Dead;
    Ok(())
}
```

## 4. 错误处理与升级

### 4.1 IsolationError

```rust
// panic/src/isolation.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationError {
    /// Partition id is out of range (>= `MAX_PARTITIONS`).
    InvalidId,
    /// Partition is already `Dead`.
    AlreadyDead,
}
```

- `InvalidId`：`id >= 8` 或 `id` 无法用 `usize` 表示（如 `u32::MAX`）。
- `AlreadyDead`：分区已被标记过 `Dead`，重复隔离视为异常，触发升级。

### 4.2 升级内核 panic

按蓝图 §4.4「分区隔离失败：升级为内核 panic 复位」，`PartitionIsolateStrategy::handle`
在收到 `Err(_)` 时直接调用 `KernelResetStrategy.handle(ctx)`。这保证：
- 越界 id 不会静默放过；
- 重复隔离已死分区不会无意义死循环，而是把整个系统拉回复位向量，
  避免分区状态表与实际运行态失配。

## 5. 分区 panic handler 注册

### 5.1 register_partition_panic_handler

按蓝图接口，允许为每个分区单独注册一个 panic handler 函数指针：

```rust
// panic/src/isolation.rs
pub fn register_partition_panic_handler(
    partition: u32,
    handler: fn(&PanicContext) -> !,
)
```

handler 存入静态 `HANDLERS` 表：

```rust
// panic/src/isolation.rs
static HANDLERS: [Mutex<Option<fn(&PanicContext) -> !>>; MAX_PARTITIONS] = [
    Mutex::new(None),  // × 8
];
```

越界注册静默忽略（按蓝图），不触发 panic。

### 5.2 get_partition_handler

```rust
// panic/src/isolation.rs
pub fn get_partition_handler(id: u32) -> Option<fn(&PanicContext) -> !>
```

返回已注册的 handler；未注册或 `id >= 8` 返回 `None`。
当前 `PartitionIsolateStrategy` 并未自动调用该 handler —— 这为后续版本
（如 v0.18.0 调度器接管分区生命周期）保留了扩展点。

## 6. 当前限制（v0.14.0）

### 6.1 不做真正资源回收

分区标记 `Dead` 后只是把当前核死循环在 panic 路径上，**不做**：
- 线程 TCB 回收
- 调度器运行队列移除
- 内存/MPU 隔离配置更新

这意味着 v0.14.0 的隔离是「逻辑标记」级别的，物理资源仍占用。

### 6.2 不通知 SystemAgent 重启

蓝图 §4.3 流程图中的「通知 SystemAgent 重启」步骤**未实现**。死循环后没有 IPC 通知，
SystemAgent 不会自动拉起该分区的新实例。该流程依赖：
- v0.18.0 调度器集成
- 后续版本的 IPC / Control Bus 通道就绪

### 6.3 固定 8 槽

`MAX_PARTITIONS = 8` 是编译期常量，不支持运行时动态分区数。超出 8 的 id 会被
`IsolationError::InvalidId` 拒绝。蓝图未来版本若需扩展，需改静态数组容量并重新编译。

## 7. v0.18.0 调度器集成计划

### 7.1 调度器查询分区状态

v0.18.0 调度器在调度决策点调用 `partition_state(id)`：

```rust
// panic/src/isolation.rs
pub fn partition_state(id: u32) -> Option<PartitionState>
```

返回 `Some(PartitionState::Dead)` 时跳过该分区的所有线程，避免把 CPU 时间片
分给已隔离分区。

### 7.2 线程回收

v0.18.0 实现真正的线程 TCB 回收：调度器遍历分区所属线程，释放其栈与控制块，
然后从运行队列移除。这才能完成蓝图 §4.3 中「回收分区资源」步骤。

### 7.3 v0.22.0 降级依赖

蓝图 §5.5 指出 v0.22.0 的 Control Bus 降级机制将依赖 panic 隔离的 `Dead` 标记：
当某分区被标记 `Dead`，Control Bus 据此触发依赖该分区的下游服务的降级路径。
本版本提供的 `partition_state` 查询接口已为该集成预留。

## 8. 测试覆盖

`isolation.rs` 内 8 个单元测试：

| 测试 | 验证点 |
|------|--------|
| `test_partition_state_initial_alive` | 启动时 0/7 槽均为 `Alive` |
| `test_mark_partition_dead_success` | 标记 0 为 `Dead` 成功，1 仍 `Alive` |
| `test_mark_partition_dead_invalid_id` | id=8 与 `u32::MAX` 返回 `InvalidId` |
| `test_mark_partition_dead_already_dead` | 重复标记同一分区返回 `AlreadyDead` |
| `test_partition_state_query` | 查询返回正确状态；越界 id 返回 `None` |
| `test_reset_partition` | `reset_partition` 把 `Dead` 还原为 `Alive`；越界返回 `InvalidId` |
| `test_register_partition_panic_handler` | 注册后 `get_partition_handler` 返回 `Some`；越界注册静默忽略 |
| `test_get_partition_handler` | 未注册槽返回 `None`；注册后返回 `Some`；越界查询返回 `None` |

所有测试通过 `std::sync::Mutex` 串行化以避免共享全局表数据竞争，
`reset_all()` helper 在每个测试开头重置 `PARTITION_TABLE` 与 `HANDLERS`。

## 9. 蓝图符合性

对照 `phase0.md §v0.14.0`：

| 蓝图条目 | 实现状态 |
|----------|----------|
| §4.3 流程图「标记分区 Dead」 | ✅ `mark_partition_dead` |
| §4.3 流程图「回收分区资源」 | ⏳ v0.14.0 仅标记，回收留待 v0.18.0（见 §6.1） |
| §4.3 流程图「通知 SystemAgent 重启」 | ⏳ 未实现，留待 v0.18.0 + IPC 就绪（见 §6.2） |
| §4.4 错误处理：分区隔离失败升级内核 panic | ✅ `Err(_)` 分支委托 `KernelResetStrategy` |
| §5.5 交互：v0.18.0 调度器配合、v0.22.0 降级依赖 | ⏳ 接口 `partition_state` / `get_partition_handler` 已就位 |
| §7.2 分区 panic 隔离该分区，不影响其他分区 | ✅ 仅目标槽位改 `Dead`，其他槽位不受影响 |
| §8.5 `#[panic_handler]` 只能有一个 | ✅ 隔离逻辑放在普通 trait 实现里，由消费者 panic_handler 委托 |
