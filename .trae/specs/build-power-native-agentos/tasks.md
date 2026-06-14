# Tasks

## 层级 1: 核心类型与权限体系

- [x] Task 1: 在 eneros-core 中定义电力原生 AgentOS 基础类型
  - [x] 1.1: 定义 `AuthorityLevel` 枚举（Observer / Operator / Supervisor / Emergency）及权限规则
  - [x] 1.2: 定义 `SystemOperatingState` 枚举及状态转换规则（Normal / Alert / Emergency / Blackout / Restoration）
  - [x] 1.3: 定义 `InterlockingRule` 结构体（rule_id, condition, blocked_action, message）
  - [x] 1.4: 定义 `AuditEntry` 结构体（agent_id, authority, action, constraint_result, approval_chain, timestamp, reasoning）
  - [x] 1.5: 定义 `ActionVerdict` 枚举（Approved / Rejected / PendingApproval / EmergencyBypassed）
  - [x] 1.6: 定义 `Jurisdiction` 结构体（zone_ids, voltage_levels, device_ids）
  - [x] 1.7: 在 EnerOSConfig 中新增 AgentConfig（max_agents, tick_interval_ms, execution_timeout_ms）和 EmergencyConfig（触发条件、权限提升规则、阈值调整策略）
  - [x] 1.8: 添加测试

## 层级 2: Agent trait 与 AgentContext 增强

- [ ] Task 2: 增强 Agent trait 和 AgentContext
  - [ ] 2.1: Agent trait 新增 `authority_level() -> AuthorityLevel` 方法
  - [ ] 2.2: Agent trait 新增 `jurisdiction() -> Jurisdiction` 方法
  - [ ] 2.3: Agent trait 新增 `handle_emergency(&mut self, event: &Event, ctx: &AgentContext) -> Result<Vec<AgentAction>>` 异步方法，提供默认实现（调用 handle_event）
  - [ ] 2.4: AgentAction 新增变体：RequestApproval / DelegateTask / EmergencyOverride / RollbackAction
  - [ ] 2.5: AgentContext 新增 `constraint_engine: Arc<ConstraintEngine>` 引用
  - [ ] 2.6: AgentContext 新增 `system_state: Arc<RwLock<SystemOperatingState>>` 引用
  - [ ] 2.7: AgentContext 新增 `authority: AuthorityLevel` 和 `jurisdiction: Jurisdiction` 字段
  - [ ] 2.8: AgentContext 新增 `audit_trail: Arc<RwLock<Vec<AuditEntry>>>` 引用
  - [ ] 2.9: 更新 MockAgent 实现新增方法
  - [ ] 2.10: 添加测试

## 层级 3: 约束感知动作验证器

- [x] Task 3: 实现 ConstraintAwareValidator
  - [ ] 3.1: 在 eneros-gateway 中创建 `constraint_validator.rs`
  - [ ] 3.2: 实现 `ConstraintAwareValidator` 结构体，持有 ConstraintEngine + SafetyGateway 引用
  - [ ] 3.3: 实现 `validate_action(action, authority, jurisdiction, system_state) -> ActionVerdict` 核心方法
  - [ ] 3.4: 实现权限校验逻辑：根据 AuthorityLevel 判断动作是否允许
  - [ ] 3.5: 实现管辖区域校验：检查动作目标设备是否在 Agent 管辖范围内
  - [ ] 3.6: 实现约束预校核：调用 ConstraintEngine.check_action_feasibility() 预测动作影响
  - [ ] 3.7: 实现紧急旁路：Emergency 状态下跳过非关键检查，保留硬约束
  - [ ] 3.8: 实现审批流：高危动作标记为 PendingApproval，记录审批链
  - [ ] 3.9: 添加测试

## 层级 4: ConstraintEngine 增强

- [x] Task 4: 增强 ConstraintEngine 支持动作预校核和动态阈值
  - [ ] 4.1: 新增 `check_action_feasibility(action: &Command) -> Result<ActionFeasibility>` 方法
  - [ ] 4.2: 实现 ActionFeasibility 结构体（feasible, new_violations, worsened_violations, risk_level）
  - [ ] 4.3: 新增 `set_emergency_thresholds(state: SystemOperatingState)` 动态阈值调整方法
  - [ ] 4.4: 新增约束违规主动通知：违规时自动发布 ConstraintViolation 事件到 EventBus
  - [ ] 4.5: 新增 `get_current_violations() -> Vec<Violation>` 查询方法
  - [ ] 4.6: 添加测试

## 层级 5: 操作闭锁规则引擎

- [x] Task 5: 实现 InterlockingRuleEngine
  - [ ] 5.1: 在 eneros-gateway 中创建 `interlocking.rs`
  - [ ] 5.2: 定义 `InterlockingRule` trait（check(device_states, action) -> InterlockingResult）
  - [ ] 5.3: 实现内置规则：BreakerOpenBeforeDisconnector（断路器断开后才能拉隔离开关）
  - [ ] 5.4: 实现内置规则：GroundRemovedBeforeClose（接地线拆除后才能合闸）
  - [ ] 5.5: 实现内置规则：SyncCheckBeforeClose（合环前同期检查）
  - [ ] 5.6: 实现 `InterlockingRuleEngine`：注册规则、批量检查、规则优先级
  - [ ] 5.7: 添加测试

## 层级 6: 动作冲突检测与仲裁

- [ ] Task 6: 实现 ActionConflictResolver
  - [ ] 6.1: 在 eneros-agent 中创建 `conflict_resolver.rs`
  - [ ] 6.2: 定义 `ActionConflict` 结构体（conflicting_actions, conflict_type, resolution）
  - [ ] 6.3: 实现 `detect_conflicts(actions: &[AgentAction]) -> Vec<ActionConflict>` 冲突检测
  - [ ] 6.4: 实现基于权限等级的仲裁（高权限优先）
  - [ ] 6.5: 实现基于拓扑优先级的仲裁（管辖区域更近的 Agent 优先）
  - [ ] 6.6: 实现 `resolve(conflicts: Vec<ActionConflict>) -> Vec<AgentAction>` 仲裁结果
  - [ ] 6.7: 添加测试

## 层级 7: 系统运行状态机与紧急响应

- [x] Task 7: 实现 SystemOperatingState 状态机与紧急响应管线
  - [ ] 7.1: 在 eneros-agent 中创建 `system_state.rs`，实现 SystemOperatingState 状态机
  - [ ] 7.2: 实现状态转换规则及守卫条件（Normal→Alert: Critical违规; Alert→Emergency: 连锁违规; Emergency→Blackout: 全局失稳）
  - [ ] 7.3: 实现状态转换时的自动动作（权限提升、阈值调整、通知发布）
  - [ ] 7.4: 在 eneros-agent 中创建 `emergency.rs`，实现 EmergencyResponsePipeline
  - [ ] 7.5: 定义 `EmergencyResponsePlan` 结构体（trigger_condition, actions, bypass_checks）
  - [ ] 7.6: 实现内置紧急响应方案：FrequencyCollapse（频率崩溃）、CascadingFailure（级联故障）、VoltageCollapse（电压崩溃）
  - [ ] 7.7: 实现紧急动作执行：绕过审批流，保留硬约束检查
  - [ ] 7.8: 添加测试

## 层级 8: 拓扑感知调度器

- [x] Task 8: 实现 TopologyAwareScheduler
  - [ ] 8.1: 在 eneros-agent 中创建 `topology_scheduler.rs`
  - [ ] 8.2: 实现 Agent 管辖区域注册与查询
  - [ ] 8.3: 实现基于拓扑区域的事件路由（事件只分发给管辖区域内的 Agent）
  - [ ] 8.4: 实现优先级调度：紧急事件优先处理，普通事件按拓扑就近分发
  - [ ] 8.5: 实现拓扑变化后的管辖区域自动更新
  - [ ] 8.6: 添加测试

## 层级 9: 审计追踪

- [x] Task 9: 实现 AuditTrail
  - [ ] 9.1: 在 eneros-agent 中创建 `audit.rs`
  - [ ] 9.2: 实现 `AuditTrail` 结构体（append-only 日志，不可篡改）
  - [ ] 9.3: 实现 `record(entry: AuditEntry)` 追加方法
  - [ ] 9.4: 实现 `query(filters) -> Vec<AuditEntry>` 查询方法（按 agent_id / 时间范围 / 动作类型）
  - [ ] 9.5: 实现 `verify_integrity() -> bool` 完整性校验
  - [ ] 9.6: 添加测试

## 层级 10: ActionDispatcher 与 Orchestrator 集成

- [ ] Task 10: 重构 ActionDispatcher 和 AgentOrchestrator 集成所有新组件
  - [ ] 10.1: ActionDispatcher 新增 ConstraintAwareValidator 调用步骤
  - [ ] 10.2: ActionDispatcher 新增 InterlockingRuleEngine 调用步骤
  - [ ] 10.3: ActionDispatcher 新增 ActionConflictResolver 调用步骤
  - [ ] 10.4: ActionDispatcher 新增 AuditTrail 记录步骤
  - [ ] 10.5: ActionDispatcher 新增紧急旁路通道
  - [ ] 10.6: AgentOrchestrator 集成 SystemOperatingState 状态机
  - [ ] 10.7: AgentOrchestrator 集成 TopologyAwareScheduler
  - [ ] 10.8: AgentOrchestrator 集成 EmergencyResponsePipeline
  - [ ] 10.9: AgentOrchestrator 在紧急状态下优先调用 handle_emergency
  - [ ] 10.10: 更新 DispatchResult 新增变体：Rejected / ConstraintRejected / PendingApproval / ConflictDetected / EmergencyBypassed
  - [ ] 10.11: 添加集成测试

## 层级 11: 全局验证

- [x] Task 11: 全局验证
  - [x] 11.1: cargo test --workspace 全部通过
  - [x] 11.2: cargo clippy --workspace 无错误
  - [x] 11.3: 更新 README.md 路线图

# Task Dependencies
- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 1, Task 4]
- [Task 5] 可与 Task 3/4 并行
- [Task 6] depends on [Task 1]
- [Task 7] depends on [Task 1, Task 4]
- [Task 8] depends on [Task 1, Task 2]
- [Task 9] depends on [Task 1]
- [Task 10] depends on [Task 2, 3, 4, 5, 6, 7, 8, 9]
- [Task 11] depends on [Task 10]
