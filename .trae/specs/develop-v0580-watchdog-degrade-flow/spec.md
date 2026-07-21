# v0.58.0 看门狗与端到端降级流程 Spec

## Why

v0.57.0 实现了降级规则引擎，但缺少端到端编排：Agent 崩溃 → 心跳超时 → 降级切换 → RTOS 接管 → Agent 恢复 → 过渡回切。本版本整合 v0.13.0（分层看门狗）、v0.37.0（心跳）、v0.56.0（命令执行）、v0.57.0（降级引擎），实现 P1-H RTOS 组件收官层、Phase 1 关键瓶颈版本的完整降级流程。

## What Changes

- 新增 crate `eneros-rtos-watchdog-degrade`（位置：`crates/kernel/rtos-watchdog-degrade/`）
- 新增 `WatchdogDegradeFlow<P, S>` 端到端降级流程管理器
- 新增 `DegradeState` 5 态状态机（Normal / Degrading / Degraded / Recovering / Emergency）
- 新增 `HeartbeatWatcher` 单 Agent 心跳监控器
- 新增 `RecoveryManager` 恢复过渡管理器（纯状态，I/O 在 flow 中执行）
- 新增 `DegradeConfig` 降级配置
- 新增 `FlowStats` / `FlowReport` 统计与报告
- 复用 v0.13.0 `Watchdog`（分层喂狗）+ v0.56.0 `CommandExecutor` + v0.57.0 `DegradeEngine`
- 设计文档 `docs/kernel/watchdog-degrade-flow-design.md`

## Impact

- Affected specs: v0.13.0（看门狗复用）/ v0.37.0（心跳模式参考）/ v0.56.0（CommandExecutor.tick 复用）/ v0.57.0（DegradeEngine.evaluate 复用）
- Affected code: 新增 `crates/kernel/rtos-watchdog-degrade/`；根 `Cargo.toml` members 新增条目
- **无破坏性改动**：本版本仅新增 crate，不修改既有 crate

## 偏差声明（D1~D12）

> 依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 API 的偏差。

### D1：复用 v0.13.0 分层 `Watchdog`，不新建 `WatchdogFeeder`

**蓝图**：新建 `WatchdogFeeder { levels: [bool; 3], watchdog: HardwareWatchdog }`，手写 3 层喂狗逻辑。

**实际**：v0.13.0 `eneros-watchdog` crate 的 `Watchdog` 已提供分层喂狗：
```rust
wd.register_layer("kernel", 100)?;     // 注册层级
wd.feed_layer(kernel_id, now_ns);      // 喂指定层
wd.check(now_ns) -> WatchdogStatus     // 检查所有层，超时则 hw.stop() 触发硬件复位
```

**决策**：直接复用 `Watchdog`，在 `WatchdogDegradeFlow::new` 中注册 3 层（kernel/runtime/agent）。不创建并行的 `WatchdogFeeder` 结构。

### D2：本地轻量 `HeartbeatWatcher`，不复用 v0.37.0 `HeartbeatMonitor`

**蓝图**：新建 `HeartbeatWatcher { heartbeat_period, last_heartbeat, timeout_count, max_timeout, agent_alive }`，单 Agent。

**实际**：v0.37.0 `HeartbeatMonitor` 是多 Agent 的（`BTreeMap<AgentId, HeartbeatState>`），且位于 `eneros-agent` crate，引入会造成重依赖。

**决策**：在 v0.58.0 本地实现单 Agent 的 `HeartbeatWatcher`（与蓝图一致），避免引入 `eneros-agent`。逻辑参考 v0.37.0 的 `check()` 算法（elapsed > period → missed_count++ → 达阈值判定 Dead）。

### D3：注入 `now_ns: u64`，拒绝 `MonotonicTime::now()`

**蓝图**：`HeartbeatWatcher::on_heartbeat()` 与 `check()` 内部调用 `MonotonicTime::now()`。

**实际**：no_std 无系统时钟（与 v0.56.0 D12 / v0.57.0 D5 一致）。

**决策**：所有时间相关方法追加 `now_ns: u64` 参数：`on_heartbeat(&mut self, now_ns)` / `check(&mut self, now_ns) -> HeartbeatStatus`。

### D4：统计计数器，拒绝 `log_info!` / `log_warn!` / `log_error!`

**蓝图**：`log_info!("Agent heartbeat received")` / `log_warn!("Degrade state: {:?} → {:?}", from, to)` / `log_error!("Emergency state")`。

**实际**：no_std 无日志框架（与 v0.56.0 D7 / v0.57.0 D7 一致）。

**决策**：`FlowStats` 累计 `state_transitions: u64` / `emergency_count: u64` / `recovery_count: u64` / `heartbeat_timeouts: u64` 等计数器。

### D5：`u64` 毫秒/纳秒，拒绝 `Duration` 类型

**蓝图**：`DegradeConfig { cmd_ttl: Duration, degrade_delay: Duration, recovery_transition: Duration, watchdog_timeout: Duration }`。

**实际**：与 v0.57.0 D5 一致，使用 `u64` 纳秒/毫秒。

**决策**：`DegradeConfig { heartbeat_period_ms: u64, heartbeat_timeout_count: u8, recovery_transition_ms: u64, watchdog_hard_timeout_ms: u32 }`。

### D6：泛型 `<P: PointAccess, S: DeviceStateProvider>`，拒绝 `Box<dyn PointAccess>`

**蓝图**：`RecoveryManager { protocol: Box<dyn PointAccess> }`。

**实际**：与 v0.57.0 D6 一致，no_std 友好用泛型。

**决策**：`WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider>` 持有 `DegradeEngine<P>` + `CommandExecutor<P, S>`。`RecoveryManager` 不持有 protocol（见 D10）。

### D7：`cmd_executor.tick(now_ns)`，拒绝 `process_commands()`

**蓝图**：`self.cmd_executor.process_commands()`。

**实际**：v0.56.0 `CommandExecutor::tick(&mut self, now_ns: u64) -> ExecutorReport`。

**决策**：在 Normal 状态调用 `self.cmd_executor.tick(now_ns)`。

### D8：`degrade_engine.evaluate(ctx, ctx.now_ns)`，拒绝 `evaluate(context)`

**蓝图**：`self.degrade_engine.evaluate(context)`。

**实际**：v0.57.0 `DegradeEngine::evaluate(&mut self, ctx: &DegradeContext, now_ns: u64) -> DegradeReport`。

**决策**：在 Degrading/Degraded 状态调用 `self.degrade_engine.evaluate(ctx, ctx.now_ns)`。

### D9：复用 v0.56.0 `DevicePointMap`，拒绝 `POWER_SETPOINT_ID` / `POWER_CMD_ID`

**蓝图**：`RecoveryManager::save_last_setpoint()` 调用 `self.protocol.read_point(POWER_SETPOINT_ID)`，`transition_step()` 调用 `self.protocol.write_point(POWER_CMD_ID, ...)`。

**实际**：`POWER_SETPOINT_ID` / `POWER_CMD_ID` 在代码库中不存在（与 v0.57.0 D9 一致）。

**决策**：`WatchdogDegradeFlow` 通过 `degrade_engine.protocol()` 读写点表。`DegradeConfig` 新增 `power_setpoint_point: PointId` 与 `power_cmd_point: PointId` 两个字段，由调用方配置。

### D10：`RecoveryManager` 为纯状态结构，I/O 在 `WatchdogDegradeFlow` 中执行

**蓝图**：`RecoveryManager` 持有 `protocol: Box<dyn PointAccess>`，内部直接读写点表。

**实际**：D6 已拒绝 `Box<dyn PointAccess>`。且 `WatchdogDegradeFlow` 已通过 `degrade_engine.protocol()` 拥有协议访问，三处持有 P 会冗余。

**决策**：`RecoveryManager` 只维护过渡状态（`saved_setpoint` / `transition_start_ns` / `progress` / `degraded_setpoint` / `agent_setpoint`），不持有 protocol。`WatchdogDegradeFlow` 在 Recovering 状态调用 `recovery.transition_step(now_ns)` 获取插值结果，再通过 `degrade_engine.protocol_mut().write_point()` 下发。

**注**：v0.57.0 `DegradeEngine` 有 `pub fn protocol(&self) -> &P` 但无 `protocol_mut()`。需要新增 `protocol_mut(&mut self) -> &mut P` 访问器（外科手术式扩展 v0.57.0）。

### D11：`tick(&mut self, ctx: &DegradeContext) -> FlowReport`

**蓝图**：`tick(&mut self, context: &DegradeContext)`。

**实际**：与 v0.56.0/v0.57.0 单步驱动模式一致。`DegradeContext` 已含 `now_ns` 字段，无需额外参数。

**决策**：`WatchdogDegradeFlow::tick(&mut self, ctx: &DegradeContext) -> FlowReport`。内部从 `ctx.now_ns` 取时间戳。

### D12：EmergencyStop 锁定 + `force_mode()` 恢复

**蓝图**：Emergency 状态"停止喂狗，触发硬件复位"。

**实际**：v0.57.0 D11 已实现 EmergencyStop 锁定（`evaluate` 不自动回切，需 `force_mode()`）。

**决策**：v0.58.0 的 `DegradeState::Emergency` 对应看门狗硬复位场景。当 `watchdog.check(now_ns) == WatchdogStatus::HardReset` 时，状态转为 Emergency 并停止喂狗。**不自动恢复**（对应蓝图风险 8.4 / 8.6：硬件复位后由启动流程恢复，不在本版本范围）。

## ADDED Requirements

### Requirement: 端到端降级流程管理器

系统 SHALL 提供 `WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider>` 结构体，整合心跳监控、降级引擎、命令执行器与分层看门狗，通过 5 态状态机（Normal / Degrading / Degraded / Recovering / Emergency）编排端到端降级流程。

#### Scenario: Agent 正常运行（Normal）
- **WHEN** `tick(ctx)` 被调用且 `ctx.agent_alive == true`
- **THEN** 状态保持 `Normal`，调用 `cmd_executor.tick(ctx.now_ns)` 消费 Agent 命令，喂狗（kernel + runtime + agent 三层）

#### Scenario: Agent 心跳超时触发降级（Normal → Degrading → Degraded）
- **WHEN** `heartbeat.check(now_ns)` 返回 `HeartbeatStatus::Dead`（连续 3 次超时）
- **THEN** 状态转为 `Degrading`（保存当前设定值），下一 tick 转为 `Degraded`，调用 `degrade_engine.evaluate(ctx, ctx.now_ns)` 由规则引擎接管，喂狗（kernel + runtime 两层，跳过 agent 层）

#### Scenario: Agent 恢复触发回切（Degraded → Recovering → Normal）
- **WHEN** `heartbeat.check(now_ns)` 返回 `HeartbeatStatus::Alive` 且当前状态为 `Degraded`
- **THEN** 状态转为 `Recovering`（启动过渡计时器），后续 tick 线性插值从降级值过渡到 Agent 设定值，过渡完成（`progress >= 1.0`）后转为 `Normal`

#### Scenario: 恢复过程中再次崩溃（Recovering → Degraded）
- **WHEN** 恢复过程中 `heartbeat.check(now_ns)` 返回 `HeartbeatStatus::Dead`
- **THEN** 状态立即转为 `Degraded`，由规则引擎重新接管（对应蓝图风险 8.4）

#### Scenario: 看门狗硬复位（任意状态 → Emergency）
- **WHEN** `watchdog.check(now_ns)` 返回 `WatchdogStatus::HardReset`
- **THEN** 状态转为 `Emergency`，停止喂狗，等待硬件复位

### Requirement: 单 Agent 心跳监控器

系统 SHALL 提供 `HeartbeatWatcher` 单 Agent 心跳监控器，追踪最后心跳时间与连续超时次数。

#### Scenario: 收到心跳
- **WHEN** `on_heartbeat(now_ns)` 被调用
- **THEN** 更新 `last_heartbeat_ns = now_ns`，重置 `timeout_count = 0`，`agent_alive = true`

#### Scenario: 心跳超时
- **WHEN** `check(now_ns)` 发现 `now_ns - last_heartbeat_ns > heartbeat_period_ns`
- **THEN** `timeout_count += 1`；若 `timeout_count >= max_timeout`（默认 3），返回 `HeartbeatStatus::Dead` 并设 `agent_alive = false`；否则返回 `HeartbeatStatus::Timeout(count)`

#### Scenario: 心跳正常
- **WHEN** `check(now_ns)` 发现 `now_ns - last_heartbeat_ns <= heartbeat_period_ns`
- **THEN** 返回 `HeartbeatStatus::Alive`，重置 `timeout_count = 0`

### Requirement: 恢复过渡管理器

系统 SHALL 提供 `RecoveryManager` 管理从降级值到 Agent 设定值的线性插值过渡。

#### Scenario: 保存降级前设定值
- **WHEN** Normal → Degrading 转换时调用 `save_setpoint(current_value, now_ns)`
- **THEN** 保存 `saved_setpoint = Some(current_value)`

#### Scenario: 启动过渡
- **WHEN** Degraded → Recovering 转换时调用 `start_transition(degraded_value, agent_value, now_ns)`
- **THEN** 记录 `transition_start_ns`、`degraded_setpoint`、`agent_setpoint`，`progress = 0.0`

#### Scenario: 过渡步进
- **WHEN** Recovering 状态调用 `transition_step(now_ns) -> Option<f64>`
- **THEN** 计算 `progress = min(1.0, elapsed / transition_duration)`，返回线性插值 `degraded + (agent - degraded) * progress`；若 `progress >= 1.0` 返回 `None` 表示完成

#### Scenario: 过渡完成
- **WHEN** `is_complete()` 返回 `true`
- **THEN** 调用 `complete()` 清理过渡状态

### Requirement: 降级流程配置

系统 SHALL 提供 `DegradeConfig` 配置降级流程参数。

#### Scenario: 默认配置
- **WHEN** `DegradeConfig::default()` 被调用
- **THEN** 返回：`heartbeat_period_ms = 1000`（1s）、`heartbeat_timeout_count = 3`（3 次超时 = 3s）、`recovery_transition_ms = 30000`（30s 过渡）、`watchdog_hard_timeout_ms = 10000`（10s 硬复位）、`power_setpoint_point = PointId(0)`、`power_cmd_point = PointId(0)`

## MODIFIED Requirements

### Requirement: DegradeEngine 协议访问器扩展

v0.57.0 `DegradeEngine` 新增 `protocol_mut(&mut self) -> &mut P` 访问器，供 v0.58.0 `WatchdogDegradeFlow` 在 Recovering 状态写入插值结果。原 `protocol(&self) -> &P` 只读访问器保留不变。

**理由**：`RecoveryManager` 为纯状态结构（D10），I/O 由 `WatchdogDegradeFlow` 执行，需要可变协议访问。

**回归**：v0.57.0 的 16 个测试全部保持通过（新增方法非破坏性）。
