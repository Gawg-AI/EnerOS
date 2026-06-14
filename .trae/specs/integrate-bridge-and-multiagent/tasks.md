# Tasks

## 层级 1: Bridge 架构重构（基础层）

- [x] Task 1: Python HTTP Bridge 服务
  - [x] 1.1: 创建 `bridge_http_server.py`，基于 aiohttp 实现 HTTP API，封装所有 cnpower/pandapower 命令
  - [x] 1.2: 实现 `/api/{command}` POST 路由，接收 JSON 参数，返回 JSON 结果
  - [x] 1.3: 添加 `/api/health` 健康检查端点
  - [x] 1.4: 添加启动时预加载 cnpower 设备库（避免首次调用延迟）

- [x] Task 2: Rust HTTP Bridge 客户端
  - [x] 2.1: 重构 `python_bridge.rs`，新增 `BridgeClient` 结构体（基于 reqwest）
  - [x] 2.2: 实现 `BridgeClient::start()` 启动 Python 子进程并等待 HTTP 就绪
  - [x] 2.3: 实现 `BridgeClient::call()` 通过 HTTP POST 调用 Python 端命令
  - [x] 2.4: 实现 `BridgeClient::stop()` 优雅关闭 Python 进程
  - [x] 2.5: 保留 `PythonBridge`（旧接口）作为 fallback，标记 deprecated
  - [x] 2.6: 添加 reqwest 依赖到 Cargo.toml

## 层级 2: pandapower 数据回传（依赖层级 1）

- [x] Task 3: pandapower 潮流结果序列化
  - [x] 3.1: Python 端新增 `run_powerflow` 命令，返回 bus 电压/相角、line 功率、损耗、收敛状态
  - [x] 3.2: Rust 端定义 `PandapowerResult` 结构体（voltage, angle, line_pf, losses, converged）
  - [x] 3.3: 实现 `CnpowerEquipmentLoader::run_powerflow()` 方法
  - [x] 3.4: 添加测试

- [x] Task 4: 拓扑信息回传
  - [x] 4.1: Python 端新增 `build_full_network` 命令，返回完整拓扑 JSON（buses + branches + types + gen/load）
  - [x] 4.2: Rust 端定义 `NetworkTopologyData` 结构体，可转换为 NetworkGraph
  - [x] 4.3: 实现 `CnpowerEquipmentLoader::build_full_network()` → `PowerNetwork::from_equipment()` 闭环
  - [x] 4.4: 添加测试

## 层级 3: 设备解析补全（依赖层级 1，可与层级 2 并行）

- [x] Task 5: Rust 端设备解析扩展
  - [x] 5.1: 修复 `load_all_loads()` 使用正确数据源（非 validation_rules）
  - [x] 5.2: 新增 `load_all_switchgear()` 解析方法
  - [x] 5.3: 新增 `load_all_reactive_compensation()` 解析方法（映射到 ShuntCompensator）
  - [x] 5.4: 新增 `load_all_new_energy()` 解析方法（光伏/风电/储能/充电桩 → StaticGenerator）
  - [x] 5.5: 添加测试

## 层级 4: 多智能体协作（依赖层级 1-3，Phase 4）

- [x] Task 6: Agent 间消息传递
  - [x] 6.1: 定义 `AgentMessage` 结构体（sender_id, target_id, content, timestamp, priority）
  - [x] 6.2: 在 AgentContext 中添加 `message_queue: Arc<RwLock<Vec<AgentMessage>>>`
  - [x] 6.3: 实现 `AgentContext::send_message()` 和 `receive_messages()` 方法
  - [x] 6.4: 添加测试

- [x] Task 7: 协作协议与角色分配
  - [x] 7.1: 定义 `CollaborationRole` 枚举（Coordinator, Executor, Advisor）
  - [x] 7.2: 定义 `TaskAssignment` 结构体（task_id, assignee, description, deadline, status）
  - [x] 7.3: 实现 `CollaborationProtocol` — 基于角色的任务分配与反馈流程
  - [x] 7.4: 在 AgentOrchestrator 中集成协作协议
  - [x] 7.5: 添加端到端协作测试：约束违规 → Dispatcher 分配 → Operator 执行 → 反馈确认

## 层级 5: 验证

- [x] Task 8: 全局验证
  - [x] 8.1: cargo test --workspace 全部通过
  - [x] 8.2: cargo clippy --workspace 无错误
  - [x] 8.3: 更新 DEVGUIDE.md Phase 4 完成度和路线图

# Task Dependencies
- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 2]
- [Task 4] depends on [Task 2]
- [Task 5] depends on [Task 2]
- [Task 3, Task 4, Task 5] 可并行执行
- [Task 6] depends on [Task 3] (需要 bridge 可用)
- [Task 7] depends on [Task 6]
- [Task 8] depends on [Task 7]
