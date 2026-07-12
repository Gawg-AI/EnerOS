# Edge Box 电源管理设计

> 版本：v0.17.1
> 适用范围：EnerOS Edge Box 掉电检测、UPS ride-through、紧急 checkpoint、优雅关机序列
> 蓝图依据：`蓝图/phase0.md` §v0.17.1
> crate：eneros-power（`crates/drivers/power/`）
> 硬件参考：飞腾 D2000 / 鲲鹏 920 Edge Box，配 UPS 或超级电容

---

## 1. 概述

Edge Box 部署在工业现场，面临突发掉电风险。v0.17.1 引入电源管理模块，保证掉电时数据完整性：在主电消失后，利用 UPS/超级电容的 ride-through 时间窗口完成紧急 checkpoint 刷盘，并执行优雅关机序列。

### 1.1 关键指标

| 指标 | 目标值 | 说明 |
|------|--------|------|
| 掉电检测延迟 | < 10 ms | 从主电消失到回调触发 |
| Ride-through 预算 | 500 ms | UPS/超级电容可维持系统运行的时间 |
| 紧急 checkpoint 超时 | 200 ms | 刷盘最大允许时间 |
| 优雅关机超时 | 100 ms | 关机清理最大允许时间 |
| 数据完整性 | 零丢失 | checkpoint 完成后方可安全断电 |

### 1.2 在 EnerOS 中的位置

```
                    ┌──────────────────┐
   主电 ──► ADC ◄──►│                  │
   主电 ──► GPIO ◄─►│  detect.rs       │──► notify_power_loss()
                    │  (双路冗余检测)   │      │
                    └──────────────────┘      ▼
                                       ┌──────────────┐
                                       │ sequence.rs  │
                                       │ on_power_loss│
                                       │ advance_seq  │
                                       │ emergency_   │──► checkpoint callback
                                       │   checkpoint │    (FS flush, 注入)
                                       └──────────────┘
```

### 1.3 版本定位

v0.17.1 属于 Phase 0 阶段刚性子版本（`蓝图/Power_Native_Agent_OS_Version_Roadmap_v3.md` 标记），为 v0.18.0 文件系统 journal/crash-consistency 提供掉电保护基础。

---

## 2. 掉电检测原理

### 2.1 双路冗余设计

主电源状态由两条独立路径监测，必须同时确认才判定为掉电：

| 路径 | 原理 | 延迟 | 误报率 |
|------|------|------|--------|
| ADC 电压比较 | 读取主电源 ADC 值，与阈值（4.75 V）比较 | ~2 ms | 低（受噪声影响） |
| GPIO 中断 | PMIC power-fail 信号触发边沿中断 | ~1 ms | 极低（硬件信号） |

双路冗余的优势：
- **防误报**：ADC 噪声不会单独触发关机（需 GPIO 同时确认）
- **防漏报**：GPIO 故障时 ADC 仍可检测（周期轮询）
- **低延迟**：GPIO 中断提供 < 1 ms 响应，ADC 提供二次确认

### 2.2 aarch64 硬件实现

```rust
// detect.rs — aarch64 (cfg-gated)
const ADC_BASE: u64 = 0x0906_0000;
const GPIO_BASE: u64 = 0x0907_0000;
const POWER_OK_THRESHOLD_MV: u32 = 4750;

fn adc_check_voltage() -> bool {
    let adc_raw = unsafe { read_volatile((ADC_BASE + ADC_DATA_REG) as *const u32) };
    adc_raw >= POWER_OK_THRESHOLD_MV
}

fn gpio_check_signal() -> bool {
    let gpio_val = unsafe { read_volatile((GPIO_BASE + GPIO_DATA_REG) as *const u32) };
    (gpio_val & POWER_FAIL_BIT) == 0
}

pub fn is_main_power_ok() -> bool {
    adc_check_voltage() && gpio_check_signal()
}
```

### 2.3 Host Mock 实现

非 aarch64 平台（host 测试）提供 mock：`set_main_power_ok(bool)` 模拟电源状态变化，`is_main_power_ok()` 返回缓存状态。所有 MMIO 代码通过 `#[cfg(target_arch = "aarch64")]` 门控。

### 2.4 中断回调

`register_power_irq(callback)` 注册掉电中断回调。当 `notify_power_loss()` 被调用时：
1. 更新全局状态：`main_power_ok = false`，`in_shutdown = true`
2. 调用注册的回调（回调中通常调用 `on_power_loss()` 启动关机序列）

---

## 3. Ride-through 预算

### 3.1 预算分配

500 ms ride-through 预算按以下方式分配：

```
|◄─────────────── ride_through_budget (500 ms) ──────────────►|

┌──────────┬──────────────────────┬─────────────────┬────────┐
│ 检测延迟  │   紧急 checkpoint    │  优雅关机        │ 余量   │
│  10 ms   │      200 ms          │    100 ms       │ 190 ms │
└──────────┴──────────────────────┴─────────────────┴────────┘
```

余量（190 ms）用于：
- 中断响应抖动
- FS flush 时间波动
- 关机清理额外开销

### 3.2 超时兜底

若 ride-through 预算耗尽（`RideThroughTimeout` 事件），序列强制跳转到 `HardOff`，无论 checkpoint 是否完成。这保证系统不会在 UPS 耗尽后仍然运行（导致不可控断电）。

---

## 4. 关机序列状态机

### 4.1 状态转换图

```
                    PowerLost
    Detect ───────────────────► RideThrough
      │                            │
      │ RideThroughTimeout         │ PowerLost
      │ (InvalidTransition)        ▼
      │                        Checkpoint
      │                            │
      │               ┌────────────┼────────────┐
      │               │ PowerLost  │            │ RideThroughTimeout
      │               │ (if done)  │            │ (未完成)
      │               ▼            │            ▼
      │        GracefulShutdown    │       HardOff
      │               │            │     (checkpoint_done=false)
      │               │ PowerLost  │
      │               ▼            │
      │           HardOff  ◄───────┘
      │        (checkpoint_done=true)
      │
      ▼  任何阶段 + PowerRestored
        → Err(NotAuthorized)
        （普通任务无权取消，需通过 detect 模块授权取消）
```

### 4.2 状态转换表

| 当前阶段 | 事件 | 下一阶段 | 说明 |
|---------|------|---------|------|
| Detect | PowerLost | RideThrough | 进入渡电 |
| RideThrough | PowerLost | Checkpoint | 开始刷盘 |
| Checkpoint | PowerLost | GracefulShutdown | 仅当 `checkpoint_done == true` |
| Checkpoint | PowerLost | (InvalidTransition) | `checkpoint_done == false` 时拒绝 |
| GracefulShutdown | PowerLost | HardOff | 安全断电 |
| RideThrough/Checkpoint/GracefulShutdown | RideThroughTimeout | HardOff | 渡电超时，强制断电 |
| Detect | RideThroughTimeout | (InvalidTransition) | 尚未开始渡电 |
| 任何 | PowerRestored | (NotAuthorized) | 普通任务无权取消 |
| HardOff | 任何 | (InvalidTransition) | 终态，不可转出 |

### 4.3 授权取消

普通任务通过 `advance_sequence(seq, PowerRestored)` 取消关机会被拒绝（`Err(NotAuthorized)`）。授权取消路径为：`detect::notify_power_restored()` 直接更新全局状态（`in_shutdown = false`），由掉电检测模块（特权上下文）调用。

### 4.4 PowerLost 语义

`PowerLost` 事件在状态机中表示"推进序列"。每次调用 `advance_sequence(seq, PowerLost)` 将序列推进到下一阶段。这是因为在实际场景中，掉电是一次性事件，但序列需要逐步推进，`PowerLost` 作为"继续/推进"信号。

---

## 5. Checkpoint 完整性

### 5.1 回调注入机制

power crate 不依赖文件系统。`emergency_checkpoint()` 通过注入的函数指针调用 FS 层的刷盘逻辑：

```rust
// 初始化时注册回调（由 FS/Runtime 层调用）
register_checkpoint_callback(|| -> Result<(), CheckpointError> {
    // 执行 FS journal flush
    // 返回 Ok(()) 或 Err(...)
});

// 掉电时调用
let result = emergency_checkpoint();
if result.is_ok() {
    seq.checkpoint_done = true;
}
```

### 5.2 错误处理

| 错误 | 含义 | 处理 |
|------|------|------|
| `IoError` | 刷盘 I/O 错误或无回调注册 | 等待 RideThroughTimeout → HardOff |
| `Timeout` | 刷盘超时 | 等待 RideThroughTimeout → HardOff |
| `AlreadyInProgress` | 重复调用 | 返回错误，不重复刷盘 |

### 5.3 重入保护

`CHECKPOINT_IN_PROGRESS` 标志防止重入。若回调内递归调用 `emergency_checkpoint()`，返回 `AlreadyInProgress`。

### 5.4 Checkpoint 完成与序列推进

`advance_sequence` 从 `Checkpoint` 阶段推进到 `GracefulShutdown` 时，检查 `checkpoint_done` 标志：
- `true` → 正常推进到 `GracefulShutdown`
- `false` → 返回 `Err(InvalidTransition)`，序列停留在 `Checkpoint` 阶段
- 若 ride-through 超时，`RideThroughTimeout` 事件强制跳转到 `HardOff`（`checkpoint_done` 保持 `false`）

---

## 6. 多核协调

### 6.1 自旋锁保护

全局状态（`POWER_STATE`、`CHECKPOINT_CALLBACK`、`POWER_IRQ_CALLBACK`）使用 `spin::Mutex` 保护，aarch64 上自旋锁基于 `LDXR/STXR` 原子指令，多核安全。

### 6.2 中断上下文安全

`notify_power_loss()` 在更新状态后释放锁，再调用回调。这避免回调中获取同一锁导致死锁：

```rust
pub fn notify_power_loss() {
    {
        let mut state = POWER_STATE.lock();
        state.main_power_ok = false;
        state.in_shutdown = true;
    } // 锁在此释放
    if let Some(cb) = *POWER_IRQ_CALLBACK.lock() {
        cb(); // 回调可安全获取 POWER_STATE
    }
}
```

### 6.3 关机序列的核亲和性

关机序列应在掉电检测核（通常绑定到 CPU0）上执行，避免跨核 IPI 延迟。`advance_sequence` 操作的是调用者的栈局部 `PowerDownSequence`，无跨核共享，天然安全。

全局 `POWER_STATE` 的 `in_shutdown` 标志由所有核读取，决定是否阻止新任务调度。该读取是原子可见的（spin lock 保证内存屏障）。

---

## 7. API 速查

| 函数 | 模块 | 说明 |
|------|------|------|
| `register_power_irq(cb)` | detect | 注册掉电中断回调 |
| `notify_power_loss()` | detect | 通知掉电（更新状态 + 调回调） |
| `notify_power_restored()` | detect | 通知主电恢复（取消关机） |
| `is_main_power_ok()` | detect | 查询主电状态 |
| `set_main_power_ok(ok)` | detect | host mock：模拟电源状态 |
| `on_power_loss()` | sequence | 创建关机序列（Detect 阶段） |
| `advance_sequence(seq, ev)` | sequence | 推进关机序列状态机 |
| `emergency_checkpoint()` | sequence | 紧急刷盘（调用注入回调） |
| `register_checkpoint_callback(cb)` | sequence | 注册刷盘回调 |
| `current_state()` | lib | 查询当前电源状态快照 |
