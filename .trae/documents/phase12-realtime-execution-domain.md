# Phase 12 — 实时执行域实现

## 概述

实现 EnerOS 架构文档中描述的**双执行架构实时执行域**。当前系统所有任务（Agent 编排、AI 推理、SCADA 采集、命令下发）均在通用 tokio 异步任务中平等调度，无法为安全关键操作提供确定性时延保证。Phase 12 将构建优先级驱动的实时执行基础设施，使 Critical/High 命令和紧急事件获得优先处理。

## 当前状态分析

### 已有基础设施
| 组件 | 能力 | 缺陷 |
|------|------|------|
| `CommandPriority` (gateway) | 4 级优先级 (Low/Normal/High/Critical)，支持 Ord 排序 | 无调度逻辑使用 |
| `MessagePriority` (agent) | 4 级优先级 | 仅用于 AgentMessage，不影响路由 |
| `SafetyGateway` | 命令验证 + 历史记录 | FIFO 执行，无优先级队列，执行是占位符 |
| `EventBus` | broadcast 通道 + handler 分发 | 无优先级，紧急事件可能被 lag 丢弃 |
| `ScadaCollector` | 周期采集 + 死区过滤 | 所有点同一扫描率，无快/慢组 |
| `ProtocolType::is_realtime()` | 标识 GOOSE/SV 为实时协议 | 无实时传输路径 |
| `InterlockingRuleEngine` | 硬约束不可旁路 | 无实时执行保证 |
| `EmergencyResponsePipeline` | 自动触发紧急预案 | 执行是同步占位符 |

### 关键缺口
1. 无优先级命令队列 — Critical 命令无法插队
2. 无实时调度器 — 所有 tokio 任务平等调度
3. 无抢占机制 — 紧急事件无法打断普通操作
4. 无确定性时序 — SCADA/Agent tick 是 best-effort
5. 无快/慢扫描分组 — 所有 SCADA 点共享同一采集周期
6. 无命令确认闭环 — fire-and-forget
7. 无看门狗超时 — 关键操作无超时保护

## 实施方案

### Step 1: PriorityCommandQueue — 优先级命令队列

**文件**: `crates/eneros-gateway/src/priority_queue.rs` (新建)

实现基于 `CommandPriority` 的优先级队列，替代 SafetyGateway 中的 FIFO 执行：

```rust
pub struct PriorityCommandQueue {
    queues: [VecDeque<Command>; 4],  // [Low, Normal, High, Critical]
    pending_count: AtomicUsize,
    notify: tokio::sync::Notify,
}
```

核心方法：
- `enqueue(cmd: Command)` — 按 priority 推入对应队列，通知消费者
- `dequeue() -> Option<Command>` — 从最高优先级非空队列取出
- `dequeue_async() -> impl Future<Output = Command>` — 异步等待非空
- `peek() -> Option<&Command>` — 查看队首但不移除
- `len_by_priority(level: CommandPriority) -> usize` — 按级别统计
- `is_empty() -> bool`

**设计决策**：
- 使用 4 个 `VecDeque` 而非 `BinaryHeap`，因为同优先级内需保持 FIFO 顺序
- `Notify` 替代 `tokio::sync::mpsc`，因为需要优先级感知的出队逻辑
- 线程安全：内部使用 `parking_lot::Mutex`（命令队列操作是短临界区）

### Step 2: RealtimeExecutor — 实时命令执行器

**文件**: `crates/eneros-gateway/src/rt_executor.rs` (新建)

独立的命令执行引擎，从 PriorityCommandQueue 消费命令并执行：

```rust
pub struct RealtimeExecutor {
    queue: Arc<PriorityCommandQueue>,
    gateway: Arc<SafetyGateway>,
    device_manager: Arc<DeviceManager>,
    ack_timeout: Duration,         // 默认 500ms
    max_retries: u32,              // 默认 3
    stats: ExecutorStats,
}

pub struct ExecutorStats {
    total_executed: AtomicU64,
    by_priority: [AtomicU64; 4],
    total_timeouts: AtomicU64,
    total_retries: AtomicU64,
    avg_latency_us: AtomicU64,
}
```

核心方法：
- `new(queue, gateway, device_manager) -> Self`
- `start() -> JoinHandle` — 启动消费循环（独立 tokio 任务，高优先级）
- `execute_one(cmd: Command) -> CommandResult` — 单条命令执行
- `stats() -> ExecutorStats`

**命令执行闭环**：
1. 从 PriorityCommandQueue 取出命令
2. SafetyGateway.validate_command() 安全校验
3. DeviceManager.write() 下发到设备
4. 等待 ACK（超时则重试）
5. 记录执行结果到历史

**CommandResult**：
```rust
pub enum CommandResult {
    Executed { latency: Duration },
    Rejected { reason: String },
    Timeout { retries: u32 },
    PartialFailure { detail: String },
}
```

### Step 3: SafetyGateway 集成 PriorityCommandQueue

**文件**: `crates/eneros-gateway/src/gateway.rs` (修改)

改造 SafetyGateway：
- 新增 `submit_command(cmd: Command) -> Result<()>` — 推入 PriorityCommandQueue
- 保留 `execute_command()` 作为同步直接执行路径（向后兼容）
- 新增 `queue() -> Arc<PriorityCommandQueue>` — 暴露队列引用
- 新增 `executor() -> Option<Arc<RealtimeExecutor>>` — 暴露执行器引用
- 新增 `start_executor(device_manager) -> Arc<RealtimeExecutor>` — 启动执行器

### Step 4: PriorityEventBus — 优先级事件总线

**文件**: `crates/eneros-eventbus/src/priority_bus.rs` (新建)

为紧急事件提供独立的高优先级通道：

```rust
pub struct PriorityEventBus {
    normal_bus: EventBus,                    // 现有 EventBus，处理 Normal/Low 事件
    urgent_sender: broadcast::Sender<Event>, // 紧急通道，处理 High/Critical 事件
    urgent_receiver: Mutex<broadcast::Receiver<Event>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventPriority {
    Low,
    Normal,
    High,
    Critical,
}
```

核心方法：
- `publish(event: Event, priority: EventPriority)` — 按优先级分发到不同通道
- `subscribe() -> PriorityEventReceiver` — 订阅者同时接收两个通道，紧急优先
- `subscribe_urgent_only() -> broadcast::Receiver<Event>` — 仅接收紧急事件

**Event 扩展**：
在 `crates/eneros-eventbus/src/event.rs` 中为 `Event` 添加 `priority: EventPriority` 字段（默认 Normal）。

**PriorityEventReceiver**：
```rust
pub struct PriorityEventReceiver {
    urgent_rx: broadcast::Receiver<Event>,
    normal_rx: broadcast::Receiver<Event>,
}

impl PriorityEventReceiver {
    /// 优先接收紧急事件，无紧急时接收普通事件
    pub async fn recv(&mut self) -> Result<Event> { ... }
}
```

### Step 5: DualScanGroup — SCADA 快/慢扫描分组

**文件**: `crates/eneros-scada/src/dual_scan.rs` (新建)

将 SCADA 采集点分为快速组和普通组：

```rust
pub struct DualScanGroup {
    fast_points: Vec<ScadaPoint>,    // 快速扫描组：保护信号、频率、电压
    normal_points: Vec<ScadaPoint>,  // 普通扫描组：功率、温度、状态
    fast_interval: Duration,         // 默认 100ms
    normal_interval: Duration,       // 默认 1000ms
}

pub struct ScanGroupBuilder {
    // 构建器，按 protocol_type / point_type 自动分组
}
```

分组规则：
- **快速组** (< 100ms)：GOOSE/SV 协议点、频率测量、电压测量、断路器位置
- **普通组** (1s)：功率测量、温度、设备状态、非关键遥测

**DataPipeline 改造**：
- `crates/eneros-scada/src/pipeline.rs` 新增 `start_dual_scan(dual_group: DualScanGroup)` 方法
- 启动两个独立的 tokio 任务，分别以 fast_interval 和 normal_interval 采集
- 快速组采集结果直接发送到 PriorityEventBus 的 High 通道

### Step 6: WatchdogTimer — 看门狗超时保护

**文件**: `crates/eneros-gateway/src/watchdog.rs` (新建)

为关键操作提供超时监控：

```rust
pub struct WatchdogTimer {
    operations: DashMap<String, PendingOp>,
    check_interval: Duration,    // 默认 50ms
    default_timeout: Duration,   // 默认 500ms
    on_timeout: Box<dyn Fn(String) + Send + Sync>,
}

struct PendingOp {
    deadline: Instant,
    on_timeout: Option<Box<dyn FnOnce() + Send>>,
}
```

核心方法：
- `register(id: String, timeout: Duration) -> WatchdogGuard` — 注册操作，返回 RAII guard
- `register_with_action(id: String, timeout: Duration, action: impl FnOnce()) -> WatchdogGuard`
- `start() -> JoinHandle` — 启动检查循环
- `cancel(id: &str)` — 手动取消

**WatchdogGuard**：RAII 模式，Drop 时自动从 pending 列表移除。

**与 RealtimeExecutor 集成**：每条命令执行前注册 Watchdog，超时自动触发重试或安全动作。

### Step 7: 模块导出与集成

**文件修改列表**：

1. `crates/eneros-gateway/src/lib.rs` — 导出新模块
2. `crates/eneros-eventbus/src/lib.rs` — 导出 PriorityEventBus
3. `crates/eneros-scada/src/lib.rs` — 导出 DualScanGroup
4. `crates/eneros-api/src/main.rs` — 集成实时执行域：
   - 创建 PriorityCommandQueue
   - 启动 RealtimeExecutor
   - 替换 EventBus 为 PriorityEventBus
   - 启动 DualScanGroup 替代单一 DataPipeline
   - 启动 WatchdogTimer
5. `crates/eneros-gateway/Cargo.toml` — 添加 `dashmap` 依赖
6. `crates/eneros-scada/Cargo.toml` — 添加 `eneros-device` 依赖（如需）

### Step 8: 集成测试

**文件**: `crates/eneros-gateway/tests/rt_executor.rs` (新建)

测试用例：
1. PriorityCommandQueue — 入队/出队优先级正确性
2. PriorityCommandQueue — 同优先级 FIFO 顺序
3. PriorityCommandQueue — 异步等待非空
4. RealtimeExecutor — Critical 命令优先执行
5. RealtimeExecutor — 超时重试
6. PriorityEventBus — 紧急事件优先接收
7. DualScanGroup — 快/慢组独立采集
8. WatchdogTimer — 超时触发回调
9. WatchdogTimer — RAII guard 自动取消
10. 端到端 — 紧急事件 → 优先命令 → 快速执行

## 依赖关系

```
Step 1 (PriorityCommandQueue) ──┐
Step 4 (PriorityEventBus) ──────┤
Step 5 (DualScanGroup) ─────────┤── Step 7 (集成) ── Step 8 (测试)
Step 6 (WatchdogTimer) ─────────┤
Step 2 (RealtimeExecutor) ──────┘  (依赖 Step 1)
Step 3 (SafetyGateway 集成) ──────── (依赖 Step 1, 2)
```

Steps 1, 4, 5, 6 可并行开发；Step 2 依赖 Step 1；Step 3 依赖 Step 1+2；Step 7 依赖全部。

## 设计决策

1. **软实时而非硬实时**：Rust + tokio 无法提供微秒级硬实时保证。本 Phase 实现的是"优先级驱动的软实时"，通过优先级队列、独立通道、专用任务来保证 Critical 操作的响应时延远优于普通操作。硬实时需要 OS 级支持（SCHED_FIFO/IRQ），超出当前范围。

2. **不修改 EventBus 原有 API**：新增 PriorityEventBus 作为独立类型，原有 EventBus 保持不变，避免破坏现有代码。需要优先级的地方使用 PriorityEventBus。

3. **PriorityCommandQueue 使用 Mutex 而非 async channel**：命令队列操作是短临界区（push/pop），Mutex 比 async channel 更高效且无 allocation 开销。

4. **WatchdogTimer 使用 DashMap**：并发注册/取消/检查操作，DashMap 比 RwLock<HashMap> 性能更好。

5. **RealtimeExecutor 是独立 tokio 任务**：而非独立 OS 线程。tokio 的 cooperative scheduling 足以满足软实时需求，且避免了跨线程同步的复杂性。

## 验证步骤

1. `cargo test -p eneros-gateway` — gateway 测试通过
2. `cargo test -p eneros-eventbus` — eventbus 测试通过
3. `cargo test -p eneros-scada` — scada 测试通过
4. `cargo test --workspace` — 全局测试通过
5. `cargo clippy --workspace` — 零错误
6. 集成测试验证优先级调度正确性
