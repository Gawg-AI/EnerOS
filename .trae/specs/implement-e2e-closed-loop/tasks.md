# Tasks

## 层级 1: 基础层（无前置依赖，可并行）

- [x] Task 1: NetworkGraph 添加潮流字段和导出方法
  - [x] 1.1: Bus 结构添加 bus_type_pf, p_gen, q_gen, p_load, q_load, v_pu 字段（均有 Default）
  - [x] 1.2: Branch 结构添加 tap_ratio 字段（默认 1.0）
  - [x] 1.3: NetworkGraph::to_solver_input() 方法导出潮流计算所需数据
  - [x] 1.4: 更新现有测试适配新字段

- [x] Task 2: EquipmentLibrary 添加批量导出接口
  - [x] 2.1: collect_admittances(base_mva, base_kv) 方法
  - [x] 2.2: get_injections_at_bus(bus_id) 方法
  - [x] 2.3: bus_ids() 方法
  - [x] 2.4: 添加测试

- [x] Task 3: Command 添加 TopologyChange 关联
  - [x] 3.1: CommandType 枚举添加 SwitchToggle, BranchToggle 变体
  - [x] 3.2: Command::to_topology_change() 转换方法
  - [x] 3.3: 添加测试

## 层级 2: 核心桥梁（依赖层级 1）

- [x] Task 4: PowerNetwork::from_equipment()
  - [x] 4.1: 实现 from_equipment(library, graph, base_mva) 构造方法
  - [x] 4.2: 从 EquipmentLibrary 收集导纳贡献，组装 YBusMatrix
  - [x] 4.3: 从 NetworkGraph + EquipmentLibrary 推导 p_spec, q_spec, bus_types
  - [x] 4.4: 添加测试：从设备库构建网络并求解潮流

- [x] Task 5: AgentContext 定义
  - [x] 5.1: 定义 AgentContext 结构体，持有 Arc<EventBus>, Arc<SafetyGateway>, Arc<ToolEngine>, Arc<RwLock<PowerNetwork>>, Arc<dyn AgentMemory>, Arc<dyn ReasoningEngine>
  - [x] 5.2: 修改 Agent trait 签名：handle_event 和 tick 新增 ctx 参数
  - [x] 5.3: 更新 MockAgent 适配新签名
  - [x] 5.4: 添加测试

## 层级 3: 集成层（依赖层级 2，可并行）

- [x] Task 6: TopologyQueryTool
  - [x] 6.1: 实现 TopologyQueryTool（is_connected, find_path, zone_count, has_cycle 查询）
  - [x] 6.2: 注册到 ToolEngine
  - [x] 6.3: 添加测试

- [x] Task 7: EventBus <-> Agent 集成
  - [x] 7.1: 实现 AgentEventHandler 适配器（Agent → EventHandler）
  - [x] 7.2: Agent 注册到 EventBus 的流程
  - [x] 7.3: 添加测试

- [x] Task 8: ActionDispatcher
  - [x] 8.1: 实现 ActionDispatcher（路由 PublishEvent/ExecuteCommand/LogMessage/NoOp）
  - [x] 8.2: ExecuteCommand 路由到 SafetyGateway
  - [x] 8.3: 命令执行后发布反馈事件
  - [x] 8.4: 添加测试

## 层级 4: 系统层（依赖层级 3）

- [x] Task 9: AgentOrchestrator 主循环
  - [x] 9.1: 定义 AgentOrchestrator 结构体
  - [x] 9.2: 实现 run() 主循环：事件分发→Agent推理→Action路由
  - [x] 9.3: 实现 tick 调度：定期调用 Running 状态 Agent 的 tick()
  - [x] 9.4: 端到端集成测试：约束违规事件→Agent推理→命令生成→安全校验→执行反馈

## 层级 5: 验证

- [x] Task 10: 全局验证
  - [x] 10.1: cargo test --workspace 全部通过
  - [x] 10.2: cargo clippy --workspace 无错误
  - [x] 10.3: 更新 DEVGUIDE.md 完成度和路线图

# Task Dependencies
- [Task 4] depends on [Task 1, Task 2]
- [Task 5] depends on [Task 3]
- [Task 6] depends on [Task 4, Task 5]
- [Task 7] depends on [Task 5]
- [Task 8] depends on [Task 5, Task 7]
- [Task 9] depends on [Task 6, Task 7, Task 8]
- [Task 10] depends on [Task 9]
- [Task 1, Task 2, Task 3] 可并行执行
- [Task 6, Task 7, Task 8] 可并行执行（Task 5 完成后）
