# EnerOS 变更日志

本项目版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

---

## [Unreleased]

### 待发布内容

- 无

---

## [0.2.0] - 2026-06-17

### 核心架构修复（BUG3 全部9项）

#### 接入层：协议适配器真实化
- **IEC 104**：删除 `eneros-device` 中的 HashMap 假实现，替换为真实 TCP 协议栈（APCI 帧、STARTDT 握手、接收循环），`eneros-scada` crate 复用 `eneros-device` 的实现而非维护独立副本
- **IEC 61850**：替换 HashMap 假实现为完整 MMS 协议栈（COTP 连接、MMS 读/写服务），支持报告和 GOOSE 模型
- **TESTFR 应答**：IEC 104 客户端收到 TESTFR_ACT 时回复 TESTFR_CON，防止 RTU 断开连接
- 新增 98 个协议适配器测试（IEC104 TCP 传输 6 个、IEC61850 MMS 8 个等）

#### 执行层：命令执行落地
- 新增 `CommandExecutor` trait（`execute()` + `read_back()` 异步接口）
- 新增 `DeviceCommandExecutor`：桥接 `Command` → `DeviceManager::write()` → `ProtocolAdapter::write()`，写后读回 ACK 验证，失败自动重试
- 新增 `LoggingExecutor`：向后兼容的日志回退执行器
- `Command` 结构体新增 `device_id`、`device_address`、`device_value` 字段用于设备路由
- `SafetyGateway::execute_command` 改为 async，使用 `tokio::sync::Mutex` 串行化 validate→execute→record
- `RealtimeExecutor::execute_one` 移除假 ACK 等待，使用真实执行结果

#### 状态机联动
- `SystemStateMachine::on_state_changed` 真正调用 `ConstraintEngine::set_emergency_thresholds()`，不再只 push 字符串消息
- 状态转换时记录阈值乘数到 `triggered_actions`

#### 冲突解析
- 重构 `ActionConflictResolver` 为 authority→time→proximity→id 四级解析链
- `resolve_by_time` 不再返回 None，使用时间戳比较实现"谁先到谁赢"
- 新增 `ProximityProvider` trait 支持拓扑近邻性解析

#### 负荷预测
- `HoltWinters` 不再退化为二次指数平滑，调用真正的 `holt_winters_fit()` 实现
- 支持加性（Additive）和乘性（Multiplicative）季节分解
- 新增 `HoltWintersTyped` 变体支持显式季节性类型选择

#### 持久化
- `TimeSeriesEngine` 新增 `with_persistent_storage()` 和 `with_sqlite()` 构造函数
- 实现 write-through 缓存模式：`record()` 同时写内存和 SQLite，`query()`/`latest()` 优先读内存，缓存未命中时回退到 SQLite 并回填
- 重启后数据不丢失（`test_real_sqlite_survives_restart` 验证）

#### 分析层：数值算法生产级化
- **状态估计**：新增 `estimate_with_network()` 方法，使用 Y-bus 导纳矩阵推导真实雅可比矩阵；新增 `NetworkModel` 结构体；`Measurement` 新增 `to_element_id` 支持支路测量；Tikhonov 正则化保证增益矩阵非奇异；使用精确非线性 h(x) 替代 H·x 线性近似
- **短路分析**：新增 `SequenceNetworks` 结构体（独立正序/负序/零序 Z-bus 矩阵）；新增 `analyze_with_sequence_networks()` 生产级方法，SLG/LL/DLG 各序网络独立计算
- **OPF**：新增 `compute_lmp_rigorous()` 基于拉格朗日对偶的严格 LMP 计算（能量分量 + 拥塞分量），影子价格通过 KKT 条件计算
- **变压器分接头**：`TwoWindingTransformer` 新增 `tap_step_percent` 字段，步长从设备参数读取而非硬编码 1%

#### P16 端到端闭环
- 新增 `ObservationProvider` 类型：执行后从 SCADA/RTU 读回实际电网观测值
- `WhatIfResult::from_observation()`：从实际 `PowerObservation` 构建 WhatIfResult，直接检查电压/热力约束
- `ConstrainedDecisionPipeline` Stage 6 优先使用实测观测（`field_observation`），无 provider 时回退到模拟器预测（`simulator_prediction`/`simulator_fallback`）
- 审计日志记录 postcondition 数据来源

### 测试
- 全部 930+ 测试通过，0 失败，0 编译警告
- 新增测试：IEC104 TCP 传输 6 个、IEC61850 MMS 8 个、执行器 8 个、状态估计真实雅可比 8 个、短路序网络 8 个、postcondition 实测观测 4 个

---

## [0.1.0] - 2026-06-15

### 初始发布

#### 核心框架（19 个 crate）
- **eneros-core**：基础类型定义（StructuredAction、PowerObservation、AuthorityLevel 等）
- **eneros-topology**：电网拓扑建模
- **eneros-powerflow**：潮流计算（牛顿-拉夫逊、Y-bus 矩阵）
- **eneros-constraint**：约束引擎、可行性投影器、What-If 分析
- **eneros-equipment**：设备模型（变压器、线路、负荷、发电机）
- **eneros-timeseries**：时序数据引擎 + SQLite 存储
- **eneros-eventbus**：事件总线
- **eneros-gateway**：安全网关、命令队列、实时执行器、决策管线
- **eneros-device**：设备管理器、协议适配器（Modbus、MQTT、IEC104、IEC61850）
- **eneros-api**：REST API 服务
- **eneros-bridge**：设备桥接
- **eneros-network**：电力网络集成
- **eneros-memory**：Agent 记忆系统
- **eneros-tool**：工具链
- **eneros-reasoning**：推理引擎
- **eneros-agent**：Agent 运行时、领域 Agent、冲突解析、系统状态机
- **eneros-scada**：SCADA 数据采集
- **eneros-analysis**：分析模块（状态估计、OPF、短路计算）
- **eneros-dashboard**：Web 仪表盘

#### Phase 1-14 功能
- Phase 1：内核基础（类型系统、事件总线、时序存储）
- Phase 2：Agent 运行时（Agent trait、调度器、权威等级）
- Phase 3-5：设备模型、潮流计算、约束引擎
- Phase 6：领域 Agent（预测、规划、自愈、电力协同）
- Phase 7：实时集成（RT 执行器、看门狗、优先级队列）
- Phase 8：深度集成（Bridge、多 Agent 协同）
- Phase 9：Bug 修复轮
- Phase 10：LLM 集成（推理引擎、Agent-LLM 对接）
- Phase 11：RIG 工具统一
- Phase 12：实时执行域
- Phase 13：约束决策管线（6 步验证、预/后条件检查）
- Phase 14：闭环（执行→验证→回滚）

#### Phase 16-17 功能
- Phase 16：端到端管线验证（14 个集成测试）
- Phase 17：IEC 104 适配器（TCP 传输、心跳、半包/粘包处理）

### 测试
- 985 个测试全绿 / 0 编译警告 / clippy 零告警

---

## 版本号规则

| 版本号部分 | 变更触发 |
|-----------|---------|
| **主版本号** (X.0.0) | 不兼容的 API 修改 |
| **次版本号** (0.X.0) | 向下兼容的功能新增 |
| **修订号** (0.0.X) | 向下兼容的问题修复 |

## 链接

[Unreleased]: https://github.com/GAWG-AI/EnerOS/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/GAWG-AI/EnerOS/releases/tag/v0.2.0
[0.1.0]: https://github.com/GAWG-AI/EnerOS/releases/tag/v0.1.0
