# EnerOS 双网冗余与切换设计文档 (v0.30.2)

> **范围**：双网络冗余管理——心跳监测、故障切换状态机、防抖回切、
> `RedundancyManager` 统一调度主备链路，保障储能 Edge Box 网络可用性 > 99.5%。
>
> **Crate**：`eneros-cellular` (`crates/drivers/cellular/src/redundancy.rs` 等)
> **版本**：v0.30.2（Phase 1 刚性子版本 R2 — 双网冗余与切换）
> **状态**：设计中 — 主机测试覆盖心跳与状态机；真实拔网线切换需硬件环境验证。

---

## 1. 概述

`eneros-cellular::redundancy` 模块基于 v0.30.0（有线以太网）与 v0.30.1（蜂窝）构建
网络高可用层。储能终端为关键基础设施，网络中断将导致市场数据丢失、远程运维失控、
告警无法上传。双网冗余保障通信高可用，满足电力系统可靠性要求（蓝图 Phase 1 出口
"autonomous 运行"要求网络可用性 > 99.5%）。本版本交付以下能力：

| 能力 | 模块 | 说明 |
|------|------|------|
| 心跳监测 | `heartbeat.rs` | `HeartbeatMonitor`（interval / timeout / max_missed） |
| 故障切换 | `failover.rs` | `FailoverManager`（4 状态机 + callback + 防抖） |
| 冗余管理 | `redundancy.rs` | `RedundancyManager`（统一管理 primary / backup 链路） |

### 设计原则

- **无 alloc 依赖**：心跳与故障切换模块为纯数值状态机，仅依赖 `core::`，可在 RTOS 控制大区（零堆分区）使用。
- **函数指针 callback**：状态切换通知通过 `fn(FailoverEvent)` 函数指针传递，避免 `Box<dyn Fn>` 的堆分配。
- **链路抽象**：`RedundancyManager` 不直接持有 `MacController` 或 `CellularModem`，通过 `LinkStatus` trait 查询链路状态，便于复用与测试。
- **防抖回切**：主链路恢复后不立即回切，需经过 `recovery_delay_ms`（默认 10 秒）稳定期，避免抖动导致频繁切换。

### v0.30.2 交付物

| 组件 | 文件 | 说明 |
|------|------|------|
| 心跳监测 | `heartbeat.rs` | HeartbeatMonitor + HeartbeatConfig |
| 故障切换 | `failover.rs` | FailoverManager + FailoverState + FailoverEvent + FailoverError |
| 冗余管理 | `redundancy.rs` | RedundancyManager + LinkType + LinkStatus trait |
| 模块入口 | `mod.rs` | 模块声明 + re-exports |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────────────┐
│  Caller (Agent Runtime / System Agent)               │
└─────────────────┬────────────────────────────────────┘
                  │  RedundancyManager API
┌─────────────────▼────────────────────────────────────┐
│  eneros_cellular::redundancy::RedundancyManager      │
│  ┌────────────────────────────────────────────────┐  │
│  │  primary_link:   LinkStatus (有线以太网)        │  │
│  │  backup_link:     LinkStatus (蜂窝)             │  │
│  │  heartbeat:       HeartbeatMonitor              │  │
│  │  failover:        FailoverManager               │  │
│  │  callback:        fn(FailoverEvent)             │  │
│  └──────────────────┬─────────────────────────────┘  │
└─────────────────────┼────────────────────────────────┘
                      │
        ┌─────────────┴─────────────┐
        ▼                           ▼
┌────────────────────┐    ┌──────────────────────────┐
│  HeartbeatMonitor  │    │  FailoverManager         │
│  interval_ms       │    │  state: FailoverState    │
│  timeout_ms        │    │  recovery_delay_ms       │
│  max_missed        │    │  last_event_ms           │
│  missed_count      │    │  callback                │
└────────────────────┘    └──────────────────────────┘
```

### 2.1 心跳监测

`HeartbeatMonitor` 在每个 `interval_ms` 周期内检查主链路心跳响应：

- 正常收到心跳响应 → `missed_count` 清零。
- 超过 `timeout_ms` 未收到响应 → `missed_count += 1`。
- `missed_count >= max_missed` → 触发主链路故障事件，进入故障切换流程。

### 2.2 故障切换

`FailoverManager` 维护 4 状态有限自动机，并通过函数指针 callback 在状态迁移时通知上层。

### 2.3 冗余管理

`RedundancyManager` 是统一调度入口：

- 持有 primary 与 backup 两条链路的 `LinkStatus` trait 对象引用。
- 周期性调用 `HeartbeatMonitor::check_timeout` 与 `FailoverManager::tick`。
- 暴露 `current_active_link() -> LinkType`、`force_switch()` 等接口供上层调用。

---

## 3. 故障切换状态机图

```text
                        ┌──────────────────┐
                        │  PrimaryActive   │  ◄── 初始状态
                        └────────┬─────────┘
                                 │
                  PrimaryDown (heartbeat missed >= max)
                                 │
                                 ▼
                        ┌──────────────────┐
       force_switch ──► │   Switching      │ ──► (任意状态均可强制进入)
                        └────────┬─────────┘
                                 │
                                 │ SwitchCompleted (backup link up)
                                 ▼
                        ┌──────────────────┐
                        │  BackupActive    │
                        └────────┬─────────┘
                                 │
                  PrimaryUp (primary heartbeat resumed)
                                 │
                                 ▼
                        ┌──────────────────┐
                        │   Recovering     │  ── 防抖期 (recovery_delay_ms)
                        └────────┬─────────┘
                                 │
                                 │ RecoveryCompleted (防抖期满，primary 稳定)
                                 ▼
                        ┌──────────────────┐
                        │  PrimaryActive   │  ── 回切完成
                        └──────────────────┘
```

### 状态迁移规则

| 当前状态 | 事件 | 下一状态 | callback 事件 | 备注 |
|---------|------|---------|--------------|------|
| PrimaryActive | PrimaryDown | Switching | `SwitchStarted` | 心跳 missed ≥ max_missed |
| Switching | SwitchCompleted | BackupActive | `SwitchCompleted` | 备份链路拨号成功 |
| BackupActive | PrimaryUp | Recovering | `RecoveryStarted` | 主链路心跳恢复 |
| Recovering | RecoveryCompleted | PrimaryActive | `RecoveryCompleted` | 防抖期满，主链路稳定 |
| Recovering | PrimaryDown | BackupActive | `SwitchAborted` | 防抖期内主链路再次故障 |
| 任意状态 | force_switch | Switching | `ForceSwitchTriggered` | 人工或上层强制切换 |

---

## 4. 心跳机制

`HeartbeatMonitor` 通过三个参数配置检测灵敏度：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `interval_ms` | 1000 | 心跳发送间隔（毫秒） |
| `timeout_ms` | 3000 | 单次心跳响应超时阈值 |
| `max_missed` | 3 | 连续丢失心跳次数上限 |

### 检测算法

```text
每个 tick (interval_ms 周期):
    if 收到心跳响应:
        missed_count = 0
        last_heartbeat_ms = now
    else if (now - last_heartbeat_ms) >= timeout_ms:
        missed_count += 1
        last_heartbeat_ms = now  // 重置以等待下一周期

    if missed_count >= max_missed:
        return HeartbeatStatus::Failed
    return HeartbeatStatus::Ok
```

- **`check_timeout(now_ms)`**：递增 `missed_count`，返回当前状态。
- **`on_heartbeat_received(now_ms)`**：清零 `missed_count`，更新 `last_heartbeat_ms`。
- 默认配置下，主链路故障后约 9 秒（3 × 3 秒）触发切换，满足蓝图"切换时间 < 5 秒"目标需调优为 `interval=1s / timeout=1.5s / max_missed=3`（≈4.5 秒）。

---

## 5. 防抖回切

主链路从故障恢复后不立即回切，需经过 `recovery_delay_ms`（默认 10 秒）稳定期：

- **目的**：避免主链路因物理抖动（如网线接触不良、交换机重启）导致频繁主备切换，引发会话中断与数据乱序。
- **实现**：进入 `Recovering` 状态后启动定时器，期间持续监测主链路心跳：
  - 防抖期内主链路持续稳定 → 定时器到期后迁移到 `PrimaryActive`，触发 `RecoveryCompleted`。
  - 防抖期内主链路再次故障 → 立即回到 `BackupActive`，触发 `SwitchAborted`，不重置防抖计数。
- **配置**：`recovery_delay_ms` 通过 `FailoverConfig` 配置，可根据现场环境调整（推荐 5~30 秒）。

---

## 6. 关键类型签名

```rust
// heartbeat.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatConfig {
    pub interval_ms: u32,
    pub timeout_ms: u32,
    pub max_missed: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    Ok,
    Missed(u8),   // 当前连续 missed 次数
    Failed,       // missed_count >= max_missed
}

pub struct HeartbeatMonitor {
    config: HeartbeatConfig,
    missed_count: u8,
    last_heartbeat_ms: u64,
}

// failover.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverState {
    PrimaryActive,
    Switching,
    BackupActive,
    Recovering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverEvent {
    SwitchStarted,
    SwitchCompleted,
    SwitchAborted,
    RecoveryStarted,
    RecoveryCompleted,
    ForceSwitchTriggered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverError {
    NoBackupAvailable,
    SwitchInProgress,
    HeartbeatTimeout,
    AlreadyInTargetState,
}

pub struct FailoverManager {
    state: FailoverState,
    recovery_delay_ms: u32,
    recovery_started_ms: u64,
    callback: Option<fn(FailoverEvent)>,
}

// redundancy.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    Primary,    // 有线以太网
    Backup,     // 蜂窝
    None,       // 双网均不可用
}

pub trait LinkStatus {
    fn is_up(&self) -> bool;
    fn link_type(&self) -> LinkType;
}

pub struct RedundancyManager {
    heartbeat: HeartbeatMonitor,
    failover: FailoverManager,
    primary_up: bool,
    backup_up: bool,
}

impl RedundancyManager {
    pub fn new(
        heartbeat_config: HeartbeatConfig,
        recovery_delay_ms: u32,
        callback: Option<fn(FailoverEvent)>,
    ) -> Self;
    pub fn tick(&mut self, now_ms: u64, primary_up: bool, backup_up: bool);
    pub fn current_active_link(&self) -> LinkType;
    pub fn force_switch(&mut self, now_ms: u64) -> Result<(), FailoverError>;
    pub fn current_state(&self) -> FailoverState;
}
```

---

## 7. no_std 合规（蓝图 §43.1）

| 标准库用法 | 本模块替代 | 出现位置 |
|-----------|-----------|---------|
| `std::sync::Mutex` | `spin::Mutex`（如需跨核共享） | RedundancyManager 外层包装（可选） |
| `Box<dyn Fn>` | `fn(FailoverEvent)` 函数指针 | FailoverManager callback |
| `std::time::Instant` | `u64` 时间戳（ms）由 `HalClock::now_ms()` 提供 | HeartbeatMonitor / FailoverManager |
| `std::collections::*` | 无 — 纯数值状态机 | HeartbeatMonitor / FailoverManager |

**关键特性**：心跳与故障切换模块**完全不依赖 `alloc`**，仅使用 `core::` 原语类型
（`u32` / `u64` / `u8` / `Option` / `fn` 指针）。这使其可在 RTOS 控制大区（零堆分区，
蓝图 §5.6 内存预算 ≤ 32 MB）直接使用，无需借用 Agent Runtime 用户堆。

---

## 8. 内存预算声明（蓝图 §5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| HeartbeatMonitor | ~32 B | config (12 B) + missed_count (1 B) + last_heartbeat_ms (8 B) + padding |
| FailoverManager | ~32 B | state (1 B) + recovery_delay_ms (4 B) + recovery_started_ms (8 B) + fn 指针 (8 B) + padding |
| RedundancyManager | ~80 B | 上述两者 + primary_up / backup_up 标志 + padding |
| **运行时总计** | **≤ 1 KB** | 含状态机 + 配置 + 链路状态缓存 |

> 双网冗余模块本身内存占用极小（< 100 B），主要内存消耗来自底层链路（v0.27.0 以太网驱动 + v0.30.1 蜂窝驱动）的缓冲，复用各自模块的预算。

---

## 9. OOM 策略

双网冗余模块为纯数值状态机，**自身不分配堆内存**，理论上不会触发 OOM。但当底层链路因 OOM 不可用时，按以下策略降级：

1. **主链路 OOM**：`MacController` 报告链路 down → `HeartbeatMonitor` 检测到超时 → 触发 `PrimaryDown` → 切换到备份蜂窝链路。
2. **备份链路 OOM**：`CellularModem` 拨号失败 → `FailoverManager` 返回 `NoBackupAvailable` → 上层收到 `FailoverError`。
3. **双网均故障**：`current_active_link() == LinkType::None` → Agent Runtime 切换到 L1 Solver-only 路径（蓝图 L1 主路径），暂停远程通信与 LLM 增强路径，仅保留本地实时控制。
4. **冻结非关键 Agent**：触发 OOM handler（蓝图 §43.6），冻结依赖网络的非关键 Agent 释放其堆配额。

---

## 10. 偏差声明

| 偏差项 | 蓝图原计划 | 实际实现 | 原因 |
|--------|-----------|---------|------|
| 集成测试 | 真实拔网线切换测试 | 主机 mock 状态机测试 | 真实拔网线切换需硬件环境（双网口 Edge Box + 蜂窝模组 + 物理网络），主机测试仅覆盖状态机迁移逻辑与心跳超时判定 |
| 心跳协议 | 应用层心跳报文 | 接口预留，依赖链路层状态 | 应用层心跳协议需与对端（云端 / 邻居 Edge Box）约定报文格式，本版本仅交付监测框架，具体报文格式留作 v0.30.x 后续子版本 |
| 切换时间 < 5 秒 | 硬性目标 | 配置可达，需实机验证 | 默认配置（1s/3s/3）约 9 秒触发切换；调优为（1s/1.5s/3）约 4.5 秒可达目标，但需在实机上验证误报率 |
| callback 类型 | `Box<dyn Fn>` | `fn(FailoverEvent)` 函数指针 | no_std 合规 + 零堆分配——`Box<dyn Fn>` 需 `alloc` 且在 RTOS 控制大区不可用；函数指针满足静态注册场景，动态回调留作后续评估 |
| `LinkStatus` trait | 持有具体链路实例 | 仅查询接口 | 解耦 RedundancyManager 与底层驱动，便于在 mock 链路上测试状态机 |
