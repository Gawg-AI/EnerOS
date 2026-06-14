# Tasks

## 层级 0: 基础设施打通（P0 — 阻塞所有领域Agent）

- [x] Task 1: 结构化电力系统观测模型 PowerObservation
  - [x] 1.1: 在 eneros-core/src/agentos_types.rs 定义 `PowerObservation` 结构体（bus_voltages, branch_flows, frequency_hz, gen_outputs, load_consumptions, timestamp）
  - [x] 1.2: 实现 `PowerObservation::from_network_state(state: &PowerSystemState) -> PowerObservation`
  - [x] 1.3: 实现 `PowerObservation::summary() -> String` 人类可读摘要
  - [x] 1.4: 添加测试

- [x] Task 2: 推理引擎结构化增强
  - [x] 2.1: 在 eneros-reasoning/src/engine.rs 的 `ReasoningInput` 新增 `power_observation: Option<PowerObservation>` 字段
  - [x] 2.2: 增强 `RuleBasedEngine` 支持数值判断规则（如 voltage < 0.95, frequency < 49.8）
  - [x] 2.3: 新增 `NumericRule` 结构体（field, operator, threshold, action_template）
  - [x] 2.4: 实现数值规则匹配逻辑
  - [x] 2.5: 添加测试

- [x] Task 3: 推理输出到 AgentAction 映射层
  - [x] 3.1: 在 eneros-agent 创建 `action_mapping.rs`
  - [x] 3.2: 定义 `EmergencyAction` 枚举（ExecuteDevice { device_id, operation, params }, NotifyAgent { agent_id, message }, ShedLoad { zone_id, amount_mw }, StartGenerator { gen_id, target_mw }）
  - [x] 3.3: 实现 `ActionMapper` 结构体，将 `EmergencyAction` 映射为 `AgentAction`
  - [x] 3.4: 实现 `map_reasoning_output(output: &ReasoningOutput) -> Vec<AgentAction>` 方法
  - [x] 3.5: 实现无法映射时的降级策略（降级为 PublishEvent）
  - [x] 3.6: 添加测试

- [x] Task 4: 紧急预案执行闭环
  - [x] 4.1: 在 eneros-core 修改 `EmergencyResponsePlan.actions` 类型从 `Vec<String>` 到 `Vec<StructuredAction>`
  - [x] 4.2: 更新 `EmergencyResponsePipeline` 的内置预案使用 `StructuredAction`
  - [x] 4.3: 实现 `EmergencyResponsePipeline.execute_with_mapper()` 方法，将 StructuredAction→EmergencyAction→AgentAction
  - [x] 4.4: 添加测试

- [x] Task 5: Agent 定时 tick 调度
  - [x] 5.1: Agent trait 新增 `tick_interval() -> std::time::Duration` 方法（默认1秒）
  - [x] 5.2: AgentOrchestrator 新增 `start_tick_loop()` 方法，按每个 Agent 的 tick_interval 定时调用 tick
  - [x] 5.3: 添加测试

## 层级 1: 领域 Agent 实现（P1）

- [x] Task 6: 调度领域 Agent — DispatchAgent
  - [x] 6.1: 在 eneros-agent 创建 `agents/dispatch_agent.rs`
  - [x] 6.2: 定义 `GeneratorCostCurve` 结构体（gen_id, a/b/c 系数, p_min, p_max）
  - [x] 6.3: 实现 `economic_dispatch(costs: &[GeneratorCostCurve], total_load_mw: f64) -> Vec<(String, f64)>` 经济调度算法
  - [x] 6.4: 实现 `calculate_ace(frequency_hz: f64, nominal_hz: f64, k_gov: f64) -> f64` ACE 计算
  - [x] 6.5: 实现 DispatchAgent 结构体，实现 Agent trait（authority_level: Supervisor, jurisdiction: 指定区域）
  - [x] 6.6: 实现 `handle_event()` — 响应负荷变化、频率偏差事件
  - [x] 6.7: 实现 `tick()` — 定期执行经济调度，生成调度指令
  - [x] 6.8: 实现 `handle_emergency()` — 紧急状态下快速调整出力
  - [x] 6.9: 添加测试

- [x] Task 7: 运维领域 Agent — OperationAgent
  - [x] 7.1: 在 eneros-agent 创建 `agents/operation_agent.rs`
  - [x] 7.2: 定义 `DeviceHealth` 枚举（Healthy / Degraded / Warning / Critical）
  - [x] 7.3: 定义 `FaultDiagnosis` 结构体（fault_type, affected_devices, cause, severity, recommendation）
  - [x] 7.4: 实现因果推理链：告警事件 → 症状匹配 → 故障定位 → 原因推断 → 检修建议
  - [x] 7.5: 实现 OperationAgent 结构体，实现 Agent trait（authority_level: Operator, jurisdiction: 指定区域）
  - [x] 7.6: 实现 `handle_event()` — 响应设备告警、约束违规事件
  - [x] 7.7: 实现 `tick()` — 定期评估设备健康状态
  - [x] 7.8: 添加测试

- [x] Task 8: 自愈领域 Agent — SelfHealingAgent
  - [x] 8.1: 在 eneros-agent 创建 `agents/self_healing_agent.rs`
  - [x] 8.2: 定义 `FaultSection` 结构体（fault_bus_id, upstream_switch, downstream_switch, affected_loads）
  - [x] 8.3: 实现 `locate_fault_section(fault_bus: ElementId, topology: &NetworkGraph) -> FaultSection` 故障区段定位
  - [x] 8.4: 实现 `generate_isolation_sequence(section: &FaultSection) -> Vec<SwitchOperation>` 隔离操作序列生成
  - [x] 8.5: 实现 `find_restoration_path(de_energized_loads: &[ElementId], topology: &NetworkGraph) -> Vec<SwitchOperation>` 供电恢复路径搜索
  - [x] 8.6: 实现 SelfHealingAgent 结构体，实现 Agent trait（authority_level: Emergency, jurisdiction: 指定区域）
  - [x] 8.7: 实现 `handle_emergency()` — 自动执行故障隔离和恢复
  - [x] 8.8: 每步操作经过 InterlockingRuleEngine 校验
  - [x] 8.9: 添加测试

## 层级 2: 协作与集成（P2）

- [x] Task 9: 领域专用协作协议
  - [x] 9.1: 定义 `PowerCollaborationProtocol` trait（check_device_availability, coordinate_emergency, negotiate_cross_zone）
  - [x] 9.2: 实现 `DefaultPowerCollaboration` — 调度-运维协同（设备可用性确认、检修协调）
  - [x] 9.3: 实现 `EmergencyCoordination` — 紧急联动（自愈隔离→通知调度调整出力）
  - [x] 9.4: 集成到 AgentOrchestrator
  - [x] 9.5: 添加测试

- [x] Task 10: 端到端集成测试
  - [x] 10.1: 测试场景1 — 负荷增长 → DispatchAgent 经济调度 → 约束校验 → 指令下发
  - [x] 10.2: 测试场景2 — 设备告警 → OperationAgent 故障诊断 → 检修建议
  - [x] 10.3: 测试场景3 — 馈线故障 → SelfHealingAgent 隔离+恢复 → 通知 DispatchAgent
  - [x] 10.4: 测试场景4 — 频率崩溃 → 紧急响应自动执行 → 审计追踪验证

## 层级 3: 全局验证

- [x] Task 11: 全局验证
  - [x] 11.1: cargo test --workspace 全部通过
  - [x] 11.2: cargo clippy --workspace 无错误
  - [x] 11.3: 更新 README.md 路线图 Phase 6

# Task Dependencies
- [Task 2] depends on [Task 1]
- [Task 3] 可与 Task 1/2 并行
- [Task 4] depends on [Task 3]
- [Task 5] 可与 Task 1-4 并行
- [Task 6] depends on [Task 1, 2, 3, 5]
- [Task 7] depends on [Task 1, 2, 5]
- [Task 8] depends on [Task 1, 3, 5]
- [Task 9] depends on [Task 6, 7, 8]
- [Task 10] depends on [Task 9]
- [Task 11] depends on [Task 10]
