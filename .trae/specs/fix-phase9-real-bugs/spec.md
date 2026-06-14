# Phase 9 — 修复真实 Bug 与消除空壳 Spec

## Why
深度审计发现 EnerOS 存在 5 个关键风险：
1. **`await_holding_lock` 定时炸弹** — `parking_lot::MutexGuard` 跨 `.await`，当前用独立线程绕过，一旦重构就死锁
2. **安全关键逻辑是空壳** — SelfHealingAgent 的联锁校验从未调用，故障定位接收 `Option<&()>` 占位
3. **Y-bus 计算逻辑 bug** — `if base_kv > 0.0 { 1.0 } else { 1.0 }` 两个分支完全一样
4. **消息系统设计缺陷** — 广播消息被第一个接收者消费，其他 Agent 永远收不到
5. **重复代码堆积** — SimulatedDataSource 重复 2 次，矩阵求逆重复 5 次

## What Changes

### 9A: 修复 await_holding_lock（P0）
- 将 `AgentEventHandler` 中的 `parking_lot::Mutex` 替换为 `tokio::sync::Mutex`
- 消除 `DataDrivenAgentLoop::start()` 中的 `std::thread::spawn` + 独立 runtime hack
- 改用 `tokio::spawn` 正常运行

### 9B: 修复安全关键空壳（P0）
- SelfHealingAgent 的 `locate_fault_section` 接收真实拓扑数据（`&NetworkGraph`）
- SelfHealingAgent 的操作序列经过 `InterlockingRuleEngine` 校验
- 消除 `Option<&()>` 占位

### 9C: 修复 Y-bus 计算 bug（P0）
- 修复 `if base_kv > 0.0 { 1.0 } else { 1.0 }` 逻辑错误
- 添加 ZIP 负荷模型正确性测试

### 9D: 修复消息系统（P1）
- 将 `AgentContext.message_queue` 从 `Vec<AgentMessage>` 改为真正的广播机制
- 确保所有 Agent 都能收到广播消息

### 9E: 消除重复代码（P1）
- 提取 `SimulatedDataSource` 到 eneros-scada 作为公共模块
- 提取矩阵求逆为 `eneros-core/src/linalg.rs` 公共工具
- 消除 main.rs 和 e2e_integration.rs 中的代码重复

### 9F: 修复 clippy 警告和死代码（P2）
- 修复所有 clippy 警告
- 清理未使用的字段和方法

## Impact
- Affected code: eneros-agent (event_adapter, context, self_healing_agent, data_driven_loop), eneros-equipment (models), eneros-api (main, handlers), eneros-core (新增 linalg), eneros-scada (公共 SimulatedDataSource)
- **BREAKING**: `locate_fault_section` 签名变更，`AgentContext.message_queue` 类型变更

## ADDED Requirements

### Requirement: 无 await_holding_lock
AgentEventHandler SHALL NOT 持有非 async 锁跨越 .await 点。

#### Scenario: Agent 在 tokio 任务中运行
- **WHEN** DataDrivenAgentLoop 使用 tokio::spawn 运行 Agent
- **THEN** 不会发生死锁

### Requirement: 自愈 Agent 联锁校验
SelfHealingAgent SHALL 对每个操作调用 InterlockingRuleEngine 校验。

#### Scenario: 故障隔离操作经过联锁校验
- **WHEN** SelfHealingAgent 生成隔离操作序列
- **THEN** 每个操作都经过 InterlockingRuleEngine.can_perform() 校验

### Requirement: ZIP 负荷 Y-bus 计算正确
ZIP 负荷模型中恒阻抗部分的导纳计算 SHALL 正确处理 base_kv == 0 的情况。

#### Scenario: base_kv 为零时的导纳计算
- **WHEN** base_kv == 0.0
- **THEN** v_pu_sq == 0.0（无电压则无导纳）

### Requirement: 消息广播正确
AgentContext 的消息队列 SHALL 支持广播，所有 Agent 都能收到广播消息。

#### Scenario: 多 Agent 接收广播
- **WHEN** 向 AgentContext 发送广播消息
- **THEN** 所有 Agent 的 receive_messages() 都能收到该消息
