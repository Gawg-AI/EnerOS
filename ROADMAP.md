# EnerOS 开发路线图

> 本文件基于代码审查结果规划未来版本，与 [CHANGELOG.md](./CHANGELOG.md) 配合使用。
> 每个规划项都标注了**代码现状依据**和**验收标准**。
>
> **架构基座**：EnerOS 定位为 **Power-Native Agent Operating System**（电力原生 Agent 操作系统）。
> 唯一架构基座蓝图见 [.trae/specs/agentos-native/spec.md](./.trae/specs/agentos-native/spec.md)，
> 任务分解见 [tasks.md](./.trae/specs/agentos-native/tasks.md)，
> 验收清单见 [checklist.md](./.trae/specs/agentos-native/checklist.md)。
> 所有版本规划围绕三条主线推进：
>
> 1. **电力原生**：协议覆盖、实时数据管道、电网模型、分析精度
> 2. **AgentOS 原生**：Agent 作为 OS 调度单元（独立进程）、AgentOS 内核、EventBus Broker、权限 OS 级强制
> 3. **生产就绪**：配置化、可观测性、安全加固、高可用部署、A/B 分区 OTA

***

## 当前状态（v0.30.0 发布基线）

| 指标         | 数值        |
| ---------- | --------- |
| 版本         | 0.30.0    |
| Crate 数    | 43        |
| 代码行数       | \~125,000 |
| 测试数        | 3500+     |
| 编译警告       | 0         |
| 测试失败       | 0         |
| TODO/FIXME | 0         |

### 架构分层

```
┌─────────────────────────────────────────────────────────────────┐
│ L3 应用层  eneros-api (HTTP/WS/CLI) · eneros-dashboard          │
├─────────────────────────────────────────────────────────────────┤
│ L3 编排层  eneros-agent (7种专业Agent+编排+调度+协作)            │
│            eneros-gateway (7阶段决策管线+安全网关+执行器)        │
├─────────────────────────────────────────────────────────────────┤
│ L2 能力层  eneros-reasoning · eneros-tool · eneros-memory       │
│            eneros-scada · eneros-device · eneros-constraint     │
│            eneros-analysis · eneros-network · eneros-bridge     │
│            eneros-timeseries · eneros-dashboard · eneros-equipment│
├─────────────────────────────────────────────────────────────────┤
│ L1 核心层  eneros-powerflow · eneros-topology · eneros-eventbus │
│            eneros-equipment · eneros-memory · eneros-timeseries │
├─────────────────────────────────────────────────────────────────┤
│ L0 基础层  eneros-core (类型/错误/配置/线性代数)                │
└─────────────────────────────────────────────────────────────────┘
```

### 系统级差距清单（v0.3.0 审查发现）

> 以下差距按"阻碍 Power-Native AgentOS 定位"的严重程度排序。

#### 致命差距（阻碍系统称为"接入电网的 AgentOS"）

| #  | 差距                        | 现状依据                                                                           | 影响                |
| -- | ------------------------- | ------------------------------------------------------------------------------ | ----------------- |
| F1 | **生产路径未接线设备层**            | `main.rs` 用 `SimulatedDataSource` + `LoggingExecutor`，所有命令只打印日志，所有 SCADA 数据是常数 | 系统无法接入真实电网        |
| F2 | **SCADA 采集循环与 IEC104 断裂** | `DataSource::read()` 同步，`start_dual_scan` 不调 `refresh_cache()`，即使接上 RTU 数据也进不来 | 实时数据管道断裂          |
| F3 | **网络模型硬编码 IEEE14**        | `main.rs` 三处 `from_ieee14()`，`eneros-bridge` 完整但未接线                            | 无法加载真实电网模型        |
| F4 | **Agent 是被动响应器非自主体**      | `Agent` trait 无 `spawn()`，无后台循环，只在 orchestrator 调用时被驱动                         | 不符合"原生 agentOS"定位 |
| F5 | **无 Agent 行为规划**          | `PlanningAgent` 是电网规划不是行为规划，`ReasoningStrategy::Deliberative` 只是标签             | Agent 无法分解复杂任务    |
| F6 | **无反思/学习闭环**              | `FeedbackLoop` 是被动重试，记忆系统与推理引擎单向只读连接                                           | Agent 无法从经验中改进    |

#### 严重差距（阻碍生产部署）

| #  | 差距                     | 现状依据                                                         | 影响               |
| -- | ---------------------- | ------------------------------------------------------------ | ---------------- |
| S1 | **API 认证完全缺失**         | 无 JWT/API Key，`POST /api/actions/structured` 可触发控制动作却完全开放    | 安全漏洞             |
| S2 | **配置系统未接线**            | `eneros.toml` 存在但 `main.rs` 从不加载，所有参数硬编码                     | 无法配置化部署          |
| S3 | **可观测性近乎为零**           | 无 metrics 端点，无 HTTP tracing，metrics 依赖声明但零使用                 | 无法运维监控           |
| S4 | **API 覆盖仅 6/17 crate** | 时序/事件/设备/工具/审计/记忆等核心能力无 API 暴露                               | 外部无法访问系统能力       |
| S5 | **工具层断裂**              | `ToolEngine` 未注册工具，`AgentAction` 无 CallTool，两套 Tool trait 分裂 | Agent 无法灵活调用系统能力 |
| S6 | **回滚只生成不执行**           | `RollbackPlan` 被填充但后条件失败时不自动触发回滚                             | 执行失败无法自动恢复       |
| S7 | **WebSocket 推送未触发**    | `broadcast_event()` 无调用方，实时推送形同虚设                            | Dashboard 无法实时更新 |
| S8 | **DelegateTask 不路由**   | `dispatcher.rs:96-99` 只打日志，不路由到目标 agent                      | 多 agent 协作断裂     |

#### 中等差距（阻碍工程化）

| #  | 差距                                      | 现状依据                          | 影响             |
| -- | --------------------------------------- | ----------------------------- | -------------- |
| M1 | `eneros-api` 依赖 17 个 crate，过度聚合         | `Cargo.toml`                  | 编译慢，职责不清       |
| M2 | dev-dependencies 循环（gateway ↔ agent）    | `gateway/Cargo.toml` dev-deps | 测试编译复杂         |
| M3 | `eneros-topology` 依赖 `eneros-powerflow` | `topology/Cargo.toml`         | 分层方向反了         |
| M4 | 记忆系统无语义检索                               | 仅 `contains(keyword)` 子串匹配    | 无法基于语义召回       |
| M5 | `tick_all()` 串行执行                       | `orchestrator.rs:147-161`     | 多 agent 无法并发   |
| M6 | 无 SOE（事件顺序记录）                           | SCADA 仅 HashMap 缓存            | 保护动作时标缺失       |
| M7 | 设备发现硬编码 Modbus                          | `discovery.rs:110`            | 无法识别协议类型       |
| M8 | Dashboard SVG 缺 data-\* 属性              | `topology_svg.rs:103-113`     | 热力图 overlay 失效 |

***

## 版本演进总览

> 版本规划遵循"先通后智、先稳后扩"原则，并在 OS 基础完成后插入 AgentOS 内核阶段：
>
> * **v0.4.0-v0.10.0 应用层完善**：电力算法+Agent 框架+协议+API+容器化（已完成）
>
> * **v0.11.0-v0.13.0 OS 基础（Phase 1）**：eneros-os crate + eneros-init PID 1 + kernel/rootfs/boot（已完成）
>
> * **v0.14.0 AgentOS 内核**：AgentRegistry/AgentSupervisor/AgentIPC/EventBusBroker/权限/配额（已完成）
>
> * **v0.15.0 Agent 进程化（激进迁移）**：7 种 Agent 拆为独立进程 + AgentContext 重构（已完成）
>
> * **v0.16.0 Gateway 进程化**：SafetyGateway/DecisionPipeline 独立进程 + IPC（已完成）
>
> * **v0.18.0 实时双执行域**：eneros-rt 接线 + 无锁 IPC + 看门狗（已完成）
>
> * **v0.19.0 网络配置服务**：netcfg/nftables/bonding/命名空间（已完成）
>
> * **v0.20.0 时间同步与日志**：PTP/NTP + 结构化日志 + 远程转发（已完成）
>
> * **v0.21.0 设备管理与 HAL**：devmgr + termios + USB/GPIO/I2C/SPI（已完成）
>
> * **v0.22.0 部署与 OTA 更新**：A/B 分区 + TUF 签名 + 安装器（已完成）
>
> * **v0.23.0 电力协议原生支持**：GOOSE/SV AF\_PACKET + 串口协议（规划中）
>
> * **v0.24.0 安全加固**：Secure Boot + seccomp + 审计 + 密钥管理（已完成）
>
> * **v0.25.0 高可用基础**：双节点心跳 + 状态同步 + 脑裂防护（已完成）
>
> * **v0.25.1 HA 基础加固修复**：修复 11 个 CRITICAL + 21 个 HIGH 缺陷（已完成）
>
> * **v0.26.0 高可用切换**：热备接管 + 服务降级 + 多节点集群（已完成）
>
> * **v0.27.0 插件系统**：动态库 + 签名验证 + 沙箱 + 协议/Agent/分析插件（已完成）
>
> * **v0.28.0 开发者工具**：Rust SDK + 模拟器 + CLI + 文档体系（已完成）
>
> * **v0.29.0 技术债务清偿与架构加固**：架构重构 + API 补全 + 性能优化 + 可观测性 + HA 高可用（已完成）
>
> * **v0.30.0 生态成熟**：认证 + 合规测试 + 端到端测试 + 混沌工程（已完成）

| 版本              | 主题                               | 目标日期      | 核心交付                                                                                                           | 状态      |
| --------------- | -------------------------------- | --------- | -------------------------------------------------------------------------------------------------------------- | ------- |
| v0.4.0          | 打通生产路径                           | 2026-07 ✅ | 设备层接线+SCADA实时管道+网络模型加载                                                                                         | 已完成     |
| v0.5.0          | Agent 自主化                        | 2026-08 ✅ | spawn生命周期+行为规划+反思学习+工具统一                                                                                       | 已完成     |
| v0.6.0          | 生产加固                             | 2026-09 ✅ | 配置化+可观测性+安全+API覆盖+回滚执行                                                                                         | 已完成     |
| v0.7.0          | 协议覆盖                             | 2026-10 ✅ | GOOSE/SV/OPC UA/DNP3+IEC104/61850增强+CIM+TLS运行时                                                                 | 已完成     |
| v0.8.0          | 分析精度                             | 2026-06 ✅ | AC-OPF+暂态稳定+状态估计增强+稀疏矩阵+不对称短路+开关物理建模                                                                           | 已完成     |
| v0.9.0          | 高可用部署                            | 2026-06 ✅ | 容器化+配置热重载+分布式追踪+DualScanGroup修复+CI/CD                                                                          | 已完成     |
| v0.10.0         | 生产深化                             | 2026-06 ✅ | 性能优化+时序增强+协议补全+API/可视化改进                                                                                       | 已完成     |
| v0.11.0-v0.13.0 | OS 基础（Phase 1）                   | 2026-06 ✅ | eneros-os crate + eneros-init PID 1 + kernel/rootfs/boot + 启动测试                                                | 已完成     |
| **v0.14.0**     | **AgentOS 内核 + EventBus Broker** | 2026-06 ✅ | AgentRegistry/AgentSupervisor/AgentIPC/EventBusBroker/AuthorityEnforcer/ResourceQuota/AgentScheduler/enerosctl | **已完成** |
| **v0.15.0**     | **Agent 进程化（激进迁移）**              | 2026-06 ✅ | 7 种 Agent 拆为独立进程 + AgentContext 重构 + ActionDispatcher IPC 化 + eneros-init 集成                                   | **已完成** |
| **v0.16.0**     | **Gateway 进程化**                  | 2026-06 ✅ | SafetyGateway/DecisionPipeline 独立进程 + GatewayClient + 端到端测试                                                    | **已完成** |
| v0.18.0         | 实时双执行域                           | 2026-06 ✅ | eneros-rt 接线 + 无锁 IPC + 看门狗 + 内核启动参数 + RT 基准测试                                                                 | 已完成     |
| v0.19.0         | 网络配置服务                           | 2026-06 ✅ | netcfg 静态IP/VLAN/网桥 + nftables 防火墙 + bonding + 命名空间隔离                                                          | 已完成     |
| v0.20.0         | 时间同步与日志                          | 2026-06 ✅ | PTP/NTP 时间同步 + 结构化日志 + 日志轮转 + 远程转发                                                                             | 已完成     |
| v0.21.0         | 设备管理与 HAL                        | 2026-06 ✅ | devmgr uevent + termios 串口 + USB/GPIO/I2C/SPI 设备接口                                                             | 已完成     |
| v0.22.0         | 部署与 OTA 更新                       | 2026-06 ✅ | A/B 分区 + TUF 签名 + eneros-imager v2 + 声明式配置 + 安装器                                                               | 已完成     |
| v0.23.0         | 电力协议原生支持                         | 规划中       | GOOSE/SV AF\_PACKET + IEC 104/Modbus 串口 + 协议时间戳 + 冗余路径                                                         | 规划中     |
| v0.24.0         | 安全加固                             | 2026-06 ✅ | Secure Boot + 内核加固 + seccomp + 审计系统 + 密钥管理                                                                     | 已完成     |
| v0.25.0         | 高可用基础                            | 2026-06 ✅ | 双节点心跳 + 状态同步 + 共享存储 + 脑裂防护                                                                                     | 已完成     |
| v0.25.1         | HA 基础加固修复                        | 2026-06 ✅ | 11 个 CRITICAL + 21 个 HIGH 缺陷修复，核心功能可用                                                                          | 已完成     |
| v0.26.0         | 高可用切换                            | 2026-06 ✅ | 热备接管 + 服务降级 + 自动恢复 + 多节点集群 + 灾备演练                                                                              | 已完成     |
| v0.27.0         | 插件系统                             | 2026-06 ✅ | 动态库加载 + 签名验证 + seccomp 沙箱 + 协议/Agent/分析插件                                                                      | 已完成     |
| v0.28.0         | 开发者工具                            | 2026-06 ✅ | Rust SDK + 模拟器 + enerosctl 全功能 + 文档体系                                                                          | 已完成     |
| v0.29.0         | 技术债务清偿与架构加固                      | 2026-06 ✅ | 架构重构 + API 补全 + 性能优化 + 可观测性 + HA 高可用 + AgentOS IPC                                                                  | 已完成     |
| v0.30.0         | 生态成熟                             | 2026-06 ✅ | 认证体系 + 合规测试 + 端到端测试 + 混沌工程 + 协议一致性 + 性能基准                                                                      | 已完成     |

***

## v0.3.0 — pandapower/cnpower 融入升级（已完成 2026-06-17）

### 目标：吸收 pandapower 电网计算优点 + cnpower 配网规划优点，升级 EnerOS 自身能力

> **设计原则**：不删除 EnerOS 独有层（agent/SCADA/协议栈/API），而是把 pandapower/cnpower 的算法和数据优点融入 EnerOS 的 Rust 原生实现，保持"电力原生 Agent 操作系统"定位。

### 改进 1：BFSW 配电网潮流算法（融入 pandapower 优点）✅

* 新增 `eneros-powerflow::bfsw_solver::BfswSolver`，实现前推回代算法

* 参考 pandapower `run_bfswpf.py` 实现 BIBC/BCBV/DLF 矩阵构造

* 新增 `PowerFlowAlgorithm` 枚举（NewtonRaphson / BackwardForwardSweep / DC）

* 3 个单元测试验证 2-bus、3-bus 辐射网和孤岛检测

### 改进 2：合规规则引擎（融入 cnpower 优点）✅

* 新增 `eneros-constraint::compliance` 模块，5 条国标合规检查规则

* 三态评估：Passed / Failed / Inconclusive

* 7 个单元测试

### 改进 3：Q 限值强制 + Recycle 机制（融入 pandapower 优点）✅

* 新增 `QLimits` 结构，PV 节点 Q 越限自动转 PQ

* 新增 `RecycleCache` 结构，时序计算复用电压初值

* 最大 10 次外层迭代

### 改进 4：配网规划参数库 + 典型接线模式（融入 cnpower 优点）✅

* 新增 `eneros-analysis::planning` 模块（SupplyAreaClass/VoltageLimits/LoadingLimits/SupplyRadius/LoadModel/RenewableHosting/CandidatePlan/PlanningEvaluator）

* 新增 `eneros-topology::connection_modes` 模块（7 种接线模式 + SAIFI/SAIDI + match\_network）

* 14 个单元测试

### 改进 5：系统级校验规则引擎（融入 pandapower/cnpower 优点）✅

* 新增 `eneros-constraint::validation_rules` 模块

* 电压质量（GB/T 12325/15945/14549/12326）+ N-1 安全（GB/T 38306）+ 短路（GB/T 15544）

* 18 个单元测试

### 验证结果

* 编译：0 error, 0 warning

* 测试：**1119 passed, 0 failed**

* Clippy：0 warning, 0 error

***

## v0.4.0 — 打通生产路径（已完成 2026-06-17）

### 目标：让 EnerOS 真正接入电网

> **核心问题**：当前 `main.rs` 使用 `SimulatedDataSource` + `LoggingExecutor` + `from_ieee14()`，
> 所有"命令下发"只打印日志，所有"SCADA 数据"是常数，所有"电网模型"是 IEEE14。
> 本版本目标是**接线设备层、SCADA 实时管道、网络模型加载**，让系统真正可接入电网。
>
> **对应差距**：F1（生产路径未接线）、F2（SCADA 断裂）、F3（网络硬编码）

### 任务 1：接线 DeviceManager 到生产路径（F1） ✅

**现状**：`main.rs` 无 `DeviceManager` 引用，`SafetyGateway` 默认用 `LoggingExecutor`。

**方案**：

* [x] `main.rs` 创建 `DeviceManager`，注册配置的协议适配器（IEC104/IEC61850/Modbus/MQTT）

* [x] `SafetyGateway::with_queue_and_executor()` 切换为 `DeviceCommandExecutor::new(device_manager.clone())`

* [x] `DeviceCommandExecutor` 持有 `DeviceManager` 引用，write 后 read\_back 验证

* [x] 保留 `LoggingExecutor` 作为 `--dry-run` 模式选项（无设备配置时自动降级）

* **验收**：`POST /api/actions/structured` 下发的命令通过 `DeviceManager` 到达协议适配器

* **影响文件**：`api/src/main.rs`、`gateway/src/gateway.rs`、`gateway/src/executor.rs`

### 任务 2：修复 SCADA 实时数据管道（F2） ✅

**现状**：`DataSource::read()` 同步，`start_dual_scan` 不调 `refresh_cache()`，IEC104 数据进不来。

**方案**：

* [x] `DataSource` trait 新增 `async fn refresh(&self)` 方法（默认空实现，向后兼容）

* [x] `Iec104DataSource::refresh()` 调用 `Iec104Client::refresh_cache()` 拉取最新数据

* [x] `DataPipeline::start()` 循环中先调 `refresh()` 再 `collect_once()`

* [x] `DataPipeline::run_once()` 同样先 refresh 再 collect（测试可验证）

* [ ] `DualScanGroup` 的 Fast/Normal 两组各自调 `refresh()`（已通过 `start_dual_scan` 间接支持，v0.5.0 补强）

* [ ] 新增 `deadband` 变化检测（推迟到 v0.5.0）

* **验收**：接上 IEC104 模拟器，SCADA latest API 返回实时变化值

* **影响文件**：`scada/src/collector.rs`、`scada/src/dual_scan.rs`、`scada/src/pipeline.rs`、`scada/src/iec104/datasource.rs`

### 任务 3：接线 cnpower 网络模型加载（F3） ✅

**现状**：`main.rs` 三处 `from_ieee14()`，`eneros-bridge` 完整但未接线。

**方案**：

* [x] `main.rs` 新增 `build_network_from_config()` 函数，根据配置选择网络来源：

  * `ieee14`：内置标准测试系统（默认）

  * `cnpower`：通过 `CnpowerEquipmentLoader::build_full_network()` 加载（桥接不可用时降级 IEEE 14）

  * `cim`：CIM/XML 文件解析（预留接口，v0.7.0 实现）

* [x] `eneros.toml` 新增 `[network]` 配置节：`source = "ieee14"` / `"cnpower"` / `"cim"`

* [x] `CnpowerEquipmentLoader` 启动时自动启动 `bridge_server.py`（如未运行）

* **验收**：配置 `source = "cnpower"` 启动后，`GET /api/topology` 返回 cnpower 网络数据

* **影响文件**：`api/src/main.rs`、`bridge/src/equipment_bridge.rs`、`eneros.toml`

### 任务 4：ObservationProvider 接线（F1 延伸） ✅

**现状**：`ConstrainedDecisionPipeline` 支持 `ObservationProvider` 但 `main.rs` 未注入。

**方案**：

* [x] `main.rs` 创建 `ObservationProvider` 闭包，从 `ScadaCollector::latest_all()` 读取实测值

* [x] 注入到 `ConstrainedDecisionPipeline::with_observation_provider()`

* [x] 后条件验证优先用实测值，回退到模拟器预测

* **验收**：命令执行后后条件验证读取 SCADA 实测值而非模拟器预测

* **影响文件**：`api/src/main.rs`、`gateway/src/decision_pipeline.rs`

### 任务 5：端到端集成测试 ✅

**方案**：

* [x] 新增 `tests/e2e_v04_wiring.rs`：18 个测试覆盖 F1/F2/F3 全链路接线

* [x] 测试流程：配置加载 → 网络构建 → SCADA refresh+collect → ObservationProvider → Gateway 执行器选择

* [x] 验证全链路：Config → PowerNetwork → DataSource::refresh → ScadaCollector → DataPipeline → ObservationProvider → SafetyGateway

* [ ] tokio mock IEC104 server 完整端到端测试（推迟到 v0.5.0，当前用 `set_state_for_testing` + 数据注入替代）

* **验收**：端到端测试通过，18 个测试覆盖三大缺陷修复

* **影响文件**：新增 `api/tests/e2e_v04_wiring.rs`

### 任务 6：配置系统接线（S2） ✅

**方案**：

* [x] `EnerOSConfig` 扩展 `NetworkConfig` / `ScadaSourceConfig` / `DeviceConnectionConfig`

* [x] `eneros.toml` 新增 `[network]` / `[scada]` / `[[devices]]` 段

* [x] `run_server()` 18 步初始化全部从配置读取

* [x] 向后兼容：v0.3.0 配置文件无需修改即可加载（`#[serde(default)]`）

* **影响文件**：`core/src/config.rs`、`api/src/main.rs`、`eneros.toml`

### 任务 7：编译+测试+clippy 验证 ✅

* [x] `cargo build --workspace` 通过，0 error

* [x] `cargo test --workspace` **1137 passed; 0 failed**

* [x] `cargo clippy --workspace --all-targets` **0 warning**

### 任务 8：更新 CHANGELOG.md ✅

* [x] v0.4.0 变更已记录到 CHANGELOG.md

***

## v0.5.0 — Agent 自主化（已完成 2026-06-17）

### 目标：从被动响应器到自主体

> **核心问题**：当前 Agent 只在 orchestrator 调用时被驱动，没有后台循环，没有规划/反思/学习。
> 本版本目标是实现 **spawn 生命周期 + 行为规划 + 反思学习 + 工具统一**，让 Agent 成为真正的自主体。
>
> **对应差距**：F4（被动响应器）、F5（无行为规划）、F6（无反思学习）、S5（工具断裂）、S8（DelegateTask 不路由）、M4（关键词匹配）

### 任务 1：Agent spawn 生命周期（F4）✅

**现状**：`Agent` trait 只有 `start()/stop()`，无 `spawn()`，无后台循环。

**方案**：

* [x] `SpawnedAgent` 后台 tokio task 包装 `Arc<Mutex<Box<dyn Agent>>>`

* [x] 感知-行动循环：接收消息 → `handle_event` → `tick` → 分发动作 → sleep

* [x] watch channel 控制 Run/Pause/Stop 信号

* [x] `AgentLifecycle` 状态机：Created → Initializing → Running ⇄ Paused → Stopping → Stopped

* **验收**：Agent 在后台持续运行，无需外部 tick 驱动 ✅

* **影响文件**：新增 `agent/src/spawn.rs`

### 任务 2：行为规划引擎（F5）✅

**现状**：`PlanningAgent` 是电网规划，`ReasoningStrategy::Deliberative` 只是标签。

**方案**：

* [x] 新增 `eneros-agent::planning` 模块（行为规划，非电网规划）

* [x] `Plan` 结构：目标 + 步骤 DAG（有向无环图）+ 依赖关系 + 前置条件 + 预期结果

* [x] `Planner` trait：`async fn plan(goal: &Goal) -> Result<Plan>`

* [x] `RuleBasedPlanner`：4 个规则模板（voltage\_violation / overload / frequency\_deviation / restore\_supply）

* [x] `PlanExecutor`：按 DAG 拓扑序执行，首步失败即中止

* [x] DAG 验证使用 Kahn 拓扑排序算法

* **验收**：Agent 收到目标后，生成多步骤计划并按序执行 ✅

* **影响文件**：新增 `agent/src/planning.rs`

* **备注**：`LlmPlanner`（feature = "rig"）推迟到 v0.6.0+，当前规则模板已覆盖核心场景

### 任务 3：反思与学习闭环（F6）✅

**现状**：`FeedbackLoop` 是被动重试，记忆系统与推理引擎单向只读。

**方案**：

* [x] 新增 `eneros-agent::reflection` 模块

* [x] `ReflectionEngine`：执行后评估，对比 Plan 预期结果与实际执行结果

* [x] `Lesson` 结构：scenario + failure\_reason + improvement + importance

* [x] Lesson 自动存入 `AgentMemory`（procedural memory 类型）

* [x] `LearningPolicy`：控制学习频率（每 N 次执行学习一次）+ 最小重要性阈值

* [x] `generate_improvement_suggestion()` 按 goal\_type 生成改进建议

* [x] `calculate_importance()` 对约束拒绝和安全失败提升重要性

* **验收**：Agent 执行失败后，记忆中新增 Lesson ✅

* **影响文件**：新增 `agent/src/reflection.rs`

* **备注**：`RuleBasedEngine::reason()` 查询记忆时优先匹配 Lesson 的集成推迟到 v0.6.0

### 任务 4：统一工具调用协议（S5）✅

**现状**：`ToolEngine` 未注册工具，`AgentAction` 无 CallTool，两套 Tool trait 分裂。

**方案**：

* [x] `AgentAction` 新增 `CallTool { tool_name: String, params: Value }` 变体

* [x] `ActionDispatcher` 持有 `Option<Arc<tokio::sync::RwLock<ToolEngine>>>`

* [x] `CallTool` 动作路由到 `ToolEngine::execute()`

* [x] `DispatchResult` 新增 `ToolExecuted(String)` 变体

* [x] 使用 `tokio::sync::RwLock` 而非 `parking_lot::RwLock`（读锁需跨 `.await` 点持有）

* **验收**：Agent 可通过 `CallTool` 动作调用任意注册工具 ✅

* **影响文件**：`agent/src/agent.rs`、`agent/src/dispatcher.rs`

* **备注**：`main.rs` 启动时注册内置工具、`to_rig_tools()` 导出推迟到 v0.6.0

### 任务 5：DelegateTask 路由 + 并发 tick（S8）✅

**现状**：`DelegateTask` 只打日志，`tick_all()` 串行。

**方案**：

* [x] `ActionDispatcher` 持有 `Option<Arc<AgentContext>>` 引用

* [x] `DelegateTask` 当 context 可用时通过 `MessageStore` 投递 `AgentMessage::direct()`

* [x] `tick_all()` 改为 `futures::future::join_all()` 并发执行所有 agent 的 tick

* **验收**：Agent A 可委托 Agent B 执行任务；多 agent 并发 tick 无锁死 ✅

* **影响文件**：`agent/src/dispatcher.rs`、`agent/src/orchestrator.rs`

* **备注**：`ConflictResolver` 仲裁推迟到 v0.6.0

### 任务 6：记忆系统语义检索（M4）✅

**现状**：仅 `contains(keyword)` 子串匹配，无语义检索。

**方案**：

* [x] 新增 `eneros-memory::vector` 模块

* [x] `SemanticMemory` 实现 `AgentMemory` trait

* [x] TF-IDF（词频-逆文档频率）+ 余弦相似度，纯 Rust 实现零外部依赖

* [x] `recall_semantic()` 方法支持自然语言查询

* [x] `recall()` 当指定 keyword 时自动走语义搜索路径

* [x] 支持所有原有过滤器（memory\_type / min\_importance / tags / time\_range）

* **验收**：`recall("电压异常处理经验")` 能召回相关历史事件，即使无关键词匹配 ✅

* **影响文件**：新增 `memory/src/vector.rs`、`memory/src/lib.rs`

* **备注**：嵌入模型（fastembed/ONNX）和 SQLite 向量扩展推迟到 v0.8.0，当前 TF-IDF 已满足语义检索需求

### 验证结果

* 编译：`cargo build --workspace` 通过，0 error

* 测试：`cargo test --workspace` **1175 passed; 0 failed**（v0.4.0: 1137 + v0.5.0 新增 38）

* 静态检查：`cargo clippy --workspace --all-targets` **0 warning**

***

## v0.6.0 — 生产加固（已完成 2026-06-17）

### 目标：从"能跑"到"能部署能运维"（已达成）

> **核心问题**：配置系统未接线、可观测性近乎为零、API 无认证、API 覆盖不足、回滚不执行。
> 本版本目标是**配置化 + 可观测性 + 安全 + API 覆盖 + 回滚执行**，让系统可部署可运维。
>
> **对应差距**：S1（无认证）、S2（配置未接线）、S3（无可观测性）、S4（API 覆盖不足）、S6（回滚不执行）、S7（WS 未触发）
>
> **架构重构任务（M1/M2/M3）经评估后推迟到 v0.7.0**，以保持当前版本的稳定性。

### 任务 1：配置系统接线（S2）✅

**现状**：`eneros.toml` 存在但 `main.rs` 从不加载。

**方案**：

* [x] `main.rs` 启动时调用 `EnerOSConfig::load_from_file("eneros.toml")`

* [x] 所有组件从配置初始化：network source / scada points / agent list / scan rates / watchdog timeout

* [x] 环境变量覆盖支持（`ENEROS_NETWORK__SOURCE=cnpower`）

* [x] 配置校验：启动时校验必填字段，缺失时友好报错

* [ ] 配置热加载（SIGHUP 信号触发重新加载非破坏性配置）— 推迟到 v0.7.0

* **验收**：修改 `eneros.toml` 重启后，系统按新配置运行 ✅

* **影响文件**：`api/src/main.rs`、`core/src/config.rs`、`eneros.toml`

### 任务 2：可观测性体系（S3）✅

**现状**：无 metrics 端点，无 HTTP tracing，metrics 依赖零使用。

**方案**：

* [ ] `api/src/app.rs` 添加 `TraceLayer::new_for_http()` 记录所有 HTTP 请求 — 推迟到 v0.7.0

* [x] 新增 `GET /metrics` 端点，导出 Prometheus 格式 metrics：

  * `eneros_commands_total{result="success|failed"}` 计数器

  * `eneros_command_duration_seconds` 直方图

  * `eneros_command_queue_depth` 仪表

  * `eneros_constraint_violations_total{type="voltage|thermal|frequency"}` 计数器

  * `eneros_agent_decisions_total{agent="...",result="..."}` 计数器

  * `eneros_agent_decision_duration_seconds` 直方图

  * `eneros_device_connections{protocol="...",status="connected|disconnected"}` 仪表

  * `eneros_powerflow_iterations` 直方图

  * `eneros_pipeline_stage_duration_seconds{stage="..."}` 直方图

* [ ] 结构化日志（JSON 格式），`tracing_subscriber::fmt().json()` — 推迟到 v0.7.0

* [ ] 日志级别动态调整（`POST /api/log-level` 端点）— 推迟到 v0.7.0

* [ ] 分布式追踪：`trace_id` 贯穿 API → Pipeline → Gateway → Device — 推迟到 v0.7.0

* **验收**：Prometheus 可抓取 `/metrics`，Grafana 可可视化所有指标 ✅

* **影响文件**：`api/src/app.rs`、`api/src/handlers/metrics.rs`、`gateway/src/decision_pipeline.rs`

### 任务 3：API 安全加固（S1）✅

**现状**：无认证，`POST /api/actions/structured` 完全开放。

**方案**：

* [x] 新增 `eneros-api::auth` 模块

* [x] JWT 认证中间件：`Authorization: Bearer <token>`

* [x] API Key 认证（备用）：`X-API-Key: <key>`

* [x] RBAC 权限模型：Observer（只读）/ Operator（读写）/ Supervisor（控制动作）/ Emergency（紧急操作）

* [x] `POST /api/auth/login` 签发 JWT，`POST /api/auth/refresh` 刷新

* [x] 审计日志：所有写操作记录 who/what/when/result/IP

* [ ] TLS 加密支持（`--tls-cert` / `--tls-key` 参数）— 推迟到 v0.7.0（配置已支持，运行时未接线）

* **验收**：未认证请求返回 401，权限不足返回 403，审计日志可追溯 ✅

* **影响文件**：新增 `api/src/auth.rs`、`api/src/handlers/auth.rs`、`api/src/app.rs`

### 任务 4：API 覆盖完善（S4）✅

**现状**：仅 6/17 crate 有 API 暴露。

**方案**：

* [x] `GET /api/timeseries/query` — 时序数据查询（element\_id + parameter + time\_range + aggregation）

* [x] `GET /api/timeseries/latest` — 最新值批量查询

* [x] `GET /api/events/stats` — 事件总线统计

* [x] `POST /api/events/publish` — 事件发布

* [x] `GET /api/devices` — 设备列表（支持按协议/状态过滤）

* [x] `GET /api/devices/{id}/health` — 设备健康状态

* [x] `POST /api/devices/{id}/connect` / `POST /api/devices/{id}/disconnect` — 设备连接控制

* [x] `GET /api/tools` — 工具列表

* [x] `POST /api/tools/{name}/execute` — 工具调用

* [x] `POST /api/memory/{agent_id}/store` — Agent 记忆存储

* [x] `POST /api/memory/{agent_id}/recall` — Agent 记忆召回

* [x] `GET /api/memory/{agent_id}/count` — Agent 记忆计数

* [x] `DELETE /api/memory/{agent_id}/{entry_id}` — 删除记忆条目

* [x] `DELETE /api/memory/{agent_id}` — 清空 Agent 记忆

* [ ] `GET /api/agents/{id}/control` — Agent 控制（start/stop/pause/resume）— 推迟到 v0.7.0

* [ ] `POST /api/validation/check` — 系统级校验（v0.3.0 validation\_rules）— 推迟到 v0.7.0

* [ ] `POST /api/compliance/check` — 设备合规检查（v0.3.0 compliance）— 推迟到 v0.7.0

* [ ] `POST /api/planning/evaluate` — 配网规划评估（v0.3.0 planning）— 推迟到 v0.7.0

* [ ] `POST /api/whatif` — WhatIf 假设计算（FeasibilityProjector）— 推迟到 v0.7.0

* [ ] `GET /api/audit` — 审计日志查询 — 推迟到 v0.7.0

* **验收**：核心 crate（timeseries/events/devices/tools/memory）都有 API 暴露 ✅

* **影响文件**：新增 `api/src/handlers/timeseries.rs`、`events.rs`、`devices.rs`、`tools.rs`、`memory.rs`

### 任务 5：自动回滚执行（S6）✅

**现状**：`RollbackPlan` 被生成但后条件失败时不自动触发。

**方案**：

* [x] `ConstrainedDecisionPipeline` 后条件失败时，检查 `rollback_plan`

* [x] 若 `RollbackPlan::full_rollback(steps)`，按逆序执行回滚步骤

* [x] 每步回滚走完整执行路径（`gateway.execute_command()`）

* [x] 回滚结果记录到审计日志（`stage: "rollback"`）

* [x] 若 `RollbackPlan::manual_only()`，仅告警不自动回滚

* [ ] `WatchdogTimer` 集成到管线：每步执行注册 watchdog，超时触发回滚 — 推迟到 v0.7.0

* **验收**：后条件失败时自动执行回滚步骤，回滚结果记录在审计日志 ✅

* **影响文件**：`gateway/src/decision_pipeline.rs`、`gateway/src/pipeline_types.rs`

### 任务 6：WebSocket 实时推送（S7）✅

**现状**：`broadcast_event()` 无调用方。

**方案**：

* [x] `EventBus::publish()` 时通过 `start_event_bus_ws_bridge()` 同步推送到 WS 客户端

* [x] 推送事件类型：DataReceived / ConstraintViolation / AgentDecision / DeviceConnected / Alarm（全部 EventType）

* [ ] Dashboard 前端 JS 接收事件并实时刷新对应面板 — 推迟到 v0.7.0

* [ ] 修复 Dashboard SVG `data-bus-id` / `data-branch-id` 属性缺失（M8）— 推迟到 v0.7.0

* **验收**：设备数据变化时 WS 客户端实时收到事件 ✅

* **影响文件**：`api/src/app.rs`、`api/src/main.rs`

### 任务 7：架构重构（M1, M2, M3）— 推迟到 v0.7.0

**现状**：`eneros-api` 依赖 17 crate，dev-deps 循环，topology 依赖 powerflow。

**方案**：

* [ ] 新增 `eneros-runtime` crate 作为中间聚合层，持有 `AppState` 所有组件

* [ ] `eneros-api` 仅依赖 `eneros-runtime`，不再直接依赖 17 个 crate

* [ ] `eneros-gateway` 的 dev-dependencies 循环：提取集成测试到 `eneros-runtime::tests`

* [ ] `eneros-topology` 反转依赖：`eneros-powerflow` 依赖 `eneros-topology`（而非反过来）

  * 将 `YBusMatrix` 移到 `eneros-powerflow`，`eneros-topology` 仅保留图论算法

* [ ] 补充 `eneros-scada` / `eneros-analysis` / `eneros-dashboard` 的 `description` 字段

* **验收**：`eneros-api` 内部依赖 < 5 个，无 dev-deps 循环，topology 不依赖 powerflow

* **影响文件**：新增 `crates/eneros-runtime/`、`api/Cargo.toml`、`gateway/Cargo.toml`、`topology/Cargo.toml`、`powerflow/Cargo.toml`

***

## v0.7.0 — 协议覆盖完善（目标：2026-10）

### 目标：覆盖主流工业协议，达到实际部署能力

> **核心问题**：GOOSE/SV/OPC UA/DNP3 未实现，IEC104/61850 功能不完整，设备发现硬编码 Modbus。
> 本版本目标是**补全协议覆盖 + 增强已有协议 + 设备发现智能化**。

### 任务 1：GOOSE / SV 协议（变电站自动化必需）

**现状**：仅枚举，无实现。

**方案**：

* [ ] GOOSE 适配器：Layer 2 原始套接字（`socket2` crate），以太网多播

* [ ] SV 适配器：采样值传输，IEC 61850-9-2 LE

* [ ] GOOSE 订阅：解析数据集，映射到 `DataValue`

* [ ] SV 订阅：解析采样值，映射到电压/电流瞬时值

* **验收**：可订阅模拟 GOOSE/SV 报文并解析数据

* **影响文件**：新增 `device/src/adapters/goose.rs`、`device/src/adapters/sv.rs`

### 任务 2：OPC UA 客户端（新能源场站主流）

**现状**：仅枚举。

**方案**：

* [ ] 基于 `opcua` crate 实现 OPC UA 客户端适配器

* [ ] 支持节点浏览、属性读取、订阅、方法调用

* [ ] 支持用户名/密码和证书认证

* **验收**：可连接 OPC UA 服务器并读取节点数据

* **影响文件**：新增 `device/src/adapters/opcua.rs`

### 任务 3：DNP3 适配器（北美市场）

**现状**：仅枚举。

**方案**：

* [ ] 基于 `dnp3` crate 或自研实现 DNP3 客户端

* [ ] 支持 Class 0/1/2/3 事件扫描

* [ ] 支持控制输出（CROB）

* **验收**：可连接 DNP3 RTU 并读取数据

* **影响文件**：新增 `device/src/adapters/dnp3.rs`

### 任务 4：IEC 104 增强

**现状**：基础 ASDU 解析，缺高级功能。

**方案**：

* [ ] 支持双点信息（M\_DP\_NA\_1）、步位置信息（M\_ST\_NA\_1）、BCR 总计

* [ ] 支持事件触发传输（Spontaneous）和周期扫描两种模式

* [ ] 支持时钟同步命令（C\_CS\_NA\_1）—— 补全 P8

* [ ] 支持参数下装（Para\_Para\_Set）

* [ ] 支持双机冗余（双连接监视）

* [ ] 支持 IEC 104/T 62351 安全扩展（TLS）

* **验收**：与真实 RTU 互通测试通过

* **影响文件**：`device/src/adapters/iec104/`

### 任务 5：IEC 61850 增强

**现状**：基础 MMS 读写，缺报告/控制服务。

**方案**：

* [ ] 实现报告控制块（RCB）使能和报告接收 —— 补全 P9

* [ ] 支持 SCL（变电站配置语言）文件解析

* [ ] 支持数据集操作（创建/删除/读取）

* [ ] 支持控制服务（Select/Operate/Cancel）

* **验收**：与真实 IED 互通测试通过

* **影响文件**：`device/src/adapters/iec61850/`

### 任务 6：设备发现智能化（M7）

**现状**：`discovery.rs:110` 硬编码返回 `ProtocolType::Modbus`。

**方案**：

* [ ] 多协议端口探测：2404→IEC104、102→IEC61850、502→Modbus、1883→MQTT、4840→OPC UA、20000→DNP3

* [ ] 协议握手识别：连接后发送协议特定帧，根据响应判断

* [ ] OPC UA Discovery Service 支持

* **验收**：扫描网络能自动识别设备协议类型

* **影响文件**：`device/src/discovery.rs`

### 任务 7：CIM 模型导入（IEC 61968/61970）

**现状**：无 CIM 支持。

**方案**：

* [ ] 新增 `eneros-network::cim` 模块

* [ ] 解析 CIM RDF/XML 文件（IEC 61970-301）

* [ ] 映射 CIM 类到 EnerOS 类型：Substation→Bus、Breaker→Switch、EnergyConsumer→Load

* [ ] 构建拓扑图 + 设备库

* **验收**：可从 CIM 文件加载电网模型

* **影响文件**：新增 `network/src/cim.rs`

***

## v0.8.0 — 分析精度进阶 ✅ 已完成（2026-06-18）

### 目标：工程级分析精度

> **核心问题**：仅 DC-OPF，无暂态稳定，状态估计功能有限，Y-Bus 密集矩阵。
> 本版本目标是**AC-OPF + 暂态稳定 + 状态估计增强 + 稀疏矩阵**。
>
> **交付结果**：1564 个测试通过，IEEE-118 潮流 17.15ms < 100ms，IEEE-14 AC-OPF 168.2μs < 500ms。新增 `eneros-linalg` crate（Crate 总数 19→20）。

### 任务 1：稀疏线性代数层（H5, P15）✅

**现状**：Y-Bus 密集矩阵，大网络性能差。

**方案**：

* [x] 新建 `eneros-linalg` crate，提供 CSR/CSC 稀疏矩阵

* [x] 实现稀疏 LU/Cholesky 分解 + 符号分解缓存

* [x] `YBusMatrix` 重构为 CSR 稀疏存储

* [x] state\_estimation、powerflow、analysis 迁移到稀疏接口

* **验收**：✅ IEEE-118 潮流求解 17.15ms < 100ms，1000 节点 < 500ms

* **影响文件**：新增 `crates/eneros-linalg/`、`powerflow/src/matrix.rs`、`analysis/src/state_estimation.rs`

### 任务 2：AC-OPF 完整实现 ✅

**现状**：仅 DC-OPF。

**方案**：

* [x] 交流 OPF（AC-OPF）完整实现：牛顿法 + 内点法

* [x] 安全约束 OPF（SCOPF）：考虑 N-1 约束

* [x] 机组组合（Unit Commitment）优化

* [x] 实时电价计算和发布（LMP）

* [x] OPF Handler 使用真实网络参数 —— 补全 P5

* **验收**：✅ AC-OPF IEEE-14 求解 168.2μs < 500ms，与 MATPOWER 结果趋势一致

* **影响文件**：`analysis/src/ac_opf.rs`

### 任务 3：暂态稳定分析 ✅

**现状**：无暂态稳定。

**方案**：

* [x] 时域仿真（龙格-库塔法 + 隐式梯形法）

* [x] 发电机暂态模型（经典二阶模型、四阶模型）

* [x] 临界故障清除时间计算

* [x] 等面积法则快速评估

* [x] 电压稳定分析（CPF 连续潮流 / 模态分析）—— 补全 P14

* **验收**：✅ 暂态稳定仿真收敛，等面积法则解析解正确

* **影响文件**：新增 `analysis/src/transient_stability.rs`

### 任务 4：状态估计增强 ✅

**现状**：基础 WLS 状态估计。

**方案**：

* [x] 不良数据检测和辨识（最大标准残差法、χ² 检验）

* [x] 可观测性分析（数值法 + 拓扑法）

* [x] 变压器分接头估计

* [x] 相量测量单元（PMU）接入和线性状态估计

* [x] 拓扑错误辨识

* **验收**：✅ 可检测并剔除坏数据，PMU 线性 SE 可用

* **影响文件**：`analysis/src/state_estimation.rs`、新增 `bad_data.rs`、`observability.rs`

### 任务 5：短路分析增强 ✅

**现状**：基础三相短路。

**方案**：

* [x] 动态短路分析（考虑发电机暂态电抗）

* [x] 不对称故障（单相接地、两相短路、两相接地）

* [x] 继电保护配合校验（故障清除时间 vs CCT）

* [x] 故障穿越能力评估

* **验收**：✅ SLG/LL/DLG 三种不对称故障分析可用

* **影响文件**：`analysis/src/short_circuit.rs`

### 任务 6：开关动作物理建模（P13）✅

**现状**：`simulator.rs` 保守拒绝开关动作。

**方案**：

* [x] `NetworkSimulatorAdapter` 实现开关动作后的 Y-Bus 重构

* [x] 支持拓扑变更：开关开合 → 修改邻接矩阵 → 重建 Y-Bus

* [x] 支持故障隔离模拟：断开故障支路 → 重新潮流计算

* **验收**：✅ 开关动作 WhatIf 投影返回真实物理结果

* **影响文件**：`network/src/simulator.rs`

***

## v0.9.0 — 交付级运维与可观测性补全 ✅ 已完成（2026-06-18）

### 目标：生产级部署能力

### 任务 1：容器化 ✅

* [x] Dockerfile（多阶段构建，优化镜像大小）

* [x] docker-compose.yml（含 API + Jaeger + Prometheus + Grafana）

* [x] 健康检查和就绪探针

* [x] 配置文件外部化（环境变量 + volume 挂载）

* **验收**：`docker compose up` 一键启动完整系统 ✅

### 任务 2：集群部署（推迟到 v1.0）

* [ ] 主备冗余（心跳 + 自动切换）

* [ ] 数据库主从复制

* [ ] 配置中心化（etcd/Consul）

* [ ] 负载均衡（多 API 实例）

* **验收**：主节点故障后备节点 < 3s 接管

### 任务 3：灾备（推迟到 v1.0）

* [ ] 数据库定期备份和恢复脚本

* [ ] 配置导出/导入

* [ ] 故障恢复 Runbook

* **验收**：数据恢复 RTO < 5min，RPO < 1min

### 任务 4：性能优化（推迟到 v1.0）

* [ ] 热点路径性能分析（命令执行 < 10ms 目标）

* [ ] 内存使用优化（时序数据压缩存储）

* [ ] 连接池复用（设备连接、数据库连接）

* [x] SafetyGateway 锁粒度重构（H3：per-device 锁池）— v0.10.0 完成

* [x] PipelineStatistics 原子化（M5）— v0.10.0 完成

* [ ] 决策管道结果复用（M6）

* **验收**：1000 TPS 命令处理，P99 延迟 < 50ms

### 任务 5：CI/CD ✅

* [x] GitHub Actions: push 时自动运行 cargo test + clippy + fmt

* [x] 自动构建 Docker 镜像（build-push-action + GHA 缓存）

* [ ] 自动发布 Release（tag 触发）— 推迟到 v1.0

* [ ] 代码覆盖率报告（tarpaulin）— 推迟到 v1.0

* **验收**：PR 合并前 CI 全绿 ✅

### 任务 5b：配置热重载 ✅（v0.9.0 新增）

* [x] `SharedConfig = Arc<RwLock<EnerOSConfig>>` 共享配置句柄

* [x] `ConfigWatcher` 轮询文件 mtime 变化（2 秒间隔）

* [x] 安全字段热重载（log\_level、enable\_metrics、scada intervals、emergency 阈值、powerflow 参数）

* [x] `POST /api/config/reload` 手动重载端点

* [x] `GET /api/config` 运行时配置查看（敏感字段脱敏）

* **验收**：修改 eneros.toml 后 2 秒内 log\_level 生效 ✅

### 任务 5c：分布式追踪基础 ✅（v0.9.0 新增）

* [x] `enable_tracing=true` 时启用 span 事件记录（FmtSpan::NEW | CLOSE）

* [x] `otel_endpoint` 和 `otel_service_name` 配置字段

* [x] `#[tracing::instrument]` 注解添加到关键 handler（auth、powerflow、analysis）

* [ ] 完整 OpenTelemetry OTLP 导出（opentelemetry-otlp crate）— 推迟到 v1.0

* **验收**：JSON 日志包含 span 创建/关闭事件 ✅

### 任务 5d：DualScanGroup 生命周期修复 ✅（v0.9.0 新增）

* [x] `DualScanHandles::shutdown()` 优雅关停（watch signal + join）

* [x] `Drop` trait 防止任务泄漏

* [x] `DualScanOptions` 消除硬编码（timeout\_ms、enable\_quality\_check、event\_bus）

* [x] 共享 `data_source` 避免重复 TCP 连接

* [x] 从 config 读取 fast/normal 间隔

* [x] `DataPipeline::start_with_shutdown()` 支持 graceful shutdown

* **验收**：Ctrl+C 时 SCADA pipeline 完成当前周期后退出 ✅

### 任务 6：时序数据库集成（推迟到 v1.0）

* [ ] 新增 `eneros-timeseries::tdengine` 后端（feature = "tdengine"）

* [ ] 新增 `eneros-timeseries::influxdb` 后端（feature = "influxdb"）

* [x] 数据降采样策略（短期 1s、中期 1min、长期 1h）— v0.10.0 Task 5 完成内存级降采样缓存

* [x] SOE（事件顺序记录）时标精度 1ms — v0.10.0 Task 4 完成

* **验收**：百万级点数查询 < 100ms

* **影响文件**：新增 `timeseries/src/tdengine.rs`、`timeseries/src/influxdb.rs`

***

## v0.10.0 — 生产深化 ✅ 已完成（2026-06-18）

### 目标：性能优化 + 时序增强 + 协议补全 + API/可视化改进

> **核心问题**：PipelineStatistics 锁争用、SafetyGateway 全局锁串行化、无 SOE 事件记录、无降采样、CIM 转换器未接线、无 API 文档、Dashboard SVG 缺 data-\* 属性。
> 本版本采用"综合推进（混合）"策略，覆盖性能、时序、协议、API 四大方向的关键短板。

### 任务 1：PipelineStatistics 原子化（M5）✅

* [x] 所有 14 个 `u64` 计数字段改为 `AtomicU64`，`fetch_add(1, Relaxed)` 无锁更新

* [x] 新增 `PipelineStatisticsSnapshot` 快照结构体，`snapshot()` 一次性读取所有字段

* [x] `record_decision(&mut self)` → `record_decision(&self)`，消除写锁

* [x] `statistics: RwLock<PipelineStatistics>` → `statistics: PipelineStatistics`（直接持有）

* [x] 5 个并发测试验证 8 线程 × 1000 次计数正确性

* **验收**：`cargo test -p eneros-gateway` 130 项通过 ✅

### 任务 2：SafetyGateway per-device 锁池（H3）✅

* [x] 移除全局 `execution_lock`，新增 `device_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>`

* [x] 新增 `history_lock` 短持有锁保护 `command_history` push

* [x] `get_device_lock()` 懒创建 per-device 锁

* [x] 不同设备命令并发执行，同设备命令串行

* [x] 3 个并发测试验证（不同设备 < 200ms、同设备 >= 200ms、无 device\_id 兜底）

* **验收**：`cargo test -p eneros-gateway` 133 项通过 ✅

### 任务 3：时序配置接线 ✅

* [x] `compute_retention_capacity()` 函数从配置计算 max\_retention（上限 1000 万点）

* [x] `TimeSeriesEngine::with_sqlite()` 从硬编码 10000 改为配置驱动

* [x] 6 个测试覆盖 retention 计算

* **验收**：配置 `retention_days=30` 启动后容量正确计算 ✅

### 任务 4：SOE 事件顺序记录 ✅

* [x] 新增 `soe.rs` 模块：`SoeRecord`、`SoeEventType`（5 种）、`SoeRecorder`、`SoeStorage`（Memory/Sqlite）

* [x] SQLite `soe_events` 表 + 时间索引 + 设备索引

* [x] `AtomicU64` 全局序号，1ms 精度时间戳

* [x] `GET /api/soe` 和 `GET /api/soe/latest` 端点

* [x] SCADA pipeline 自动检测 breaker/switch/position/relay 参数 0↔1 翻转

* [x] 16 个测试（11 单元 + 5 handler）

* **验收**：开关变位 → SOE 记录 → API 查询完整链路 ✅

### 任务 5：存储级降采样基础 ✅

* [x] 新增 `downsample.rs`：`DownsampleLevel`（Second/Minute/Hour）、`AggregatedPoint`、`DownsampledCache`

* [x] `rollup()` 按时间窗口聚合（avg/min/max/count/sum）

* [x] `start_rollup_task()` 后台任务（60s→1min，60min→1h），支持 graceful shutdown

* [x] `query_downsampled()` 自动粒度选择（<1h 原始，1h-7d 1min，>7d 1h）

* [x] 16 个测试验证多粒度查询

* **验收**：`cargo test -p eneros-timeseries` 59 项通过 ✅

### 任务 6：CIM→PowerNetwork 转换器 ✅

* [x] `cim_to_power_network()` 函数（约 270 行），支持 IEC 61968/61970 CIM 类型映射

* [x] BusbarSection→Bus、AcLineSegment/PowerTransformer→Branch、SynchronousMachine→Generator、EnergyConsumer→Load、LinearShuntCompensator→Shunt

* [x] Breaker/Disconnector 作为零阻抗支路

* [x] Terminal→ConnectivityNode→BusbarSection 拓扑解析

* [x] `main.rs` `build_cim_network()` 接线（复用 `NetworkConfig.path`）

* [x] 11 个单元测试（含潮流收敛测试）

* **验收**：`source = "cim"` 配置加载 CIM 文件并转换为 PowerNetwork ✅

### 任务 7：Dashboard SVG data-\* 修复（M8）✅

* [x] branch `<line>` 添加 `data-branch-id`

* [x] bus `<circle>` 和 `<text>` 添加 `data-bus-id`

* [x] 2 个测试验证 data-\* 属性存在

* **验收**：前端热力图 overlay 可定位元素 ✅

### 任务 8：OpenAPI 自动文档 ✅

* [x] `utoipa = "5"` 依赖，`#[derive(ToSchema)]` 16 个类型

* [x] `#[utoipa::path]` 注解 6 个端点

* [x] `GET /api/openapi.json` 返回 OpenAPI 3.0 JSON

* [x] `GET /docs` 返回 CDN Swagger UI

* [x] 2 个测试验证端点

* **验收**：访问 `/docs` 可交互测试 API ✅

***

## v0.14.0 — AgentOS 内核 + EventBus Broker（已完成）

> **目标**：建立 AgentOS 内核层（L3），实现 Agent 进程管理、IPC、权限强制、资源配额，以及独立 EventBus Broker 进程。
> **前置条件**：v0.13.0 OS 基础已完成（eneros-os crate + eneros-init PID 1）
> **详细任务分解**：[.trae/specs/agentos-native/tasks.md](./.trae/specs/agentos-native/tasks.md#v0140--agentos-内核--eventbus-broker)
> **验收清单**：[.trae/specs/agentos-native/checklist.md](./.trae/specs/agentos-native/checklist.md#v0140--agentos-内核--eventbus-broker)

### 任务 1：共享 Schema 迁移到 eneros-core

* [x] 将 `Command/CommandType/CommandPriority` 从 `eneros-gateway` 迁移到 `eneros-core`

* [x] 将 `Event/EventType/EventPayload` 从 `eneros-eventbus` 迁移到 `eneros-core`

* [x] 将 `DecisionContext/EnhancedPipelineDecision/PipelineAuditEntry` 迁移到 `eneros-core`

* [x] 将 `AgentMessage` 从 `eneros-agent` 迁移到 `eneros-core`

* [x] 为 `ExecutionResult` 补 `Serialize/Deserialize`，迁移到 eneros-core

* [x] gateway/eventbus/agent crate 通过 `pub use` 重导出，向后兼容

* **验收**：`cargo build --workspace` 通过，所有现有测试不受影响

### 任务 2：AgentRegistry 进程注册表

* [x] 新增 `eneros-os/src/agentos/registry.rs`

* [x] `AgentInfo`：`agent_id`、`pid`、`agent_type`、`authority`、`status`、`started_at`

* [x] `register/lookup/list/unregister/update_status` 接口

* **验收**：注册后可查询，注销后不可查询 ✅

### 任务 3：AgentSupervisor 生命周期监督

* [x] 新增 `eneros-os/src/agentos/supervisor.rs`

* [x] `spawn/stop/restart/health_check` 接口

* [x] 复用 `RestartPolicy::OnFailure` + 5 次/分钟降级

* [ ] 集成到 `eneros-init/main.rs` 主循环（推迟至 v0.15.0 Agent 进程化时集成）

* **验收**：Agent 崩溃后 1s 内自动重启（单元测试验证，端到端推迟至 v0.15.0）

### 任务 4：AgentIPC 消息传递

* [x] 新增 `eneros-os/src/agentos/ipc.rs`

* [x] `AgentIpcClient/AgentIpcServer`（Unix socket + TCP 双传输）

* [ ] RT 域 `SharedMemoryChannel`（共享内存 + eventfd，推迟至 v1.0.0 实时双执行域）

* [x] `publish(topic, event)` 广播到 EventBusBroker

* **验收**：IPC 延迟 < 100μs（Unix socket，推迟至 Linux 环境验证）

### 任务 5：EventBusBroker 独立进程

* [x] 新增 `eneros-eventbus/src/broker.rs` + `eneros-eventbus/bins/broker/`

* [x] `EventBusBroker` TCP/Unix socket 服务端

* [x] `EventBusClient` IPC 客户端 stub

* [x] 保留 `PriorityEventBus` 语义（urgent/normal 双 topic）

* [ ] 集成到 `eneros-init` 作为系统服务（推迟至 v0.15.0 Agent 进程化时集成）

* **验收**：独立进程运行，1000 events/s 吞吐，延迟 < 1ms（单元测试验证 pub/sub 全链路）

### 任务 6：AuthorityEnforcer 权限强制

* [x] 新增 `eneros-os/src/agentos/authority.rs`

* [x] 基于 Linux capabilities（seccomp 推迟至 v1.4.0 安全加固）

* [x] AuthorityLevel → capabilities 映射（Observer/Operator/Supervisor/Emergency）

* **验收**：Observer Agent 进程无法 open(/dev/ttyS0, O\_WRONLY)（Linux 环境验证，单元测试验证映射逻辑）

### 任务 7：ResourceQuota 资源配额

* [x] 新增 `eneros-os/src/agentos/quota.rs`

* [x] 基于 cgroups v2

* [x] `set_quota/usage` 接口

* [ ] 集成到 AgentSupervisor：spawn 时创建 cgroup，stop 时删除（推迟至 v0.15.0 Agent 进程化时集成）

* **验收**：Agent 内存超限被 OOM kill，不影响其他（Linux 环境验证，单元测试验证 cgroup 路径生成）

### 任务 8：AgentScheduler 调度策略

* [x] 新增 `eneros-os/src/agentos/scheduler.rs`

* [x] `SchedulingPolicy::Normal`（SCHED\_OTHER）/ `Realtime`（SCHED\_FIFO）

* [x] RT Agent：SCHED\_FIFO + CPU 隔离 + mlockall

* [x] `preempt(agent_id)` 提升优先级

* **验收**：RT Agent 命令时延 P99 < 1ms（Linux 环境验证，单元测试验证策略设置）

### 任务 9：enerosctl 管理 CLI

* [x] 新增 `eneros-os/bins/enerosctl/`

* [x] `agent list/start/stop/status` 子命令

* [x] `eventbus status/subscribe` 子命令

* [x] 通过 Unix socket 与 eneros-init 通信（TCP 回退到本地 state 文件）

* **验收**：`enerosctl agent list` 可查询所有 Agent 状态 ✅

### 任务 10：编译+测试+clippy 验证

* [x] `cargo build --workspace` 通过，0 error

* [x] `cargo test --workspace -- --test-threads=1` 全部通过（1769 通过，0 失败）

* [x] `cargo clippy -p eneros-os -p eneros-eventbus -p eneros-eventbus-broker -p enerosctl --all-targets` 0 错误

* [x] 更新 CHANGELOG.md v0.14.0

***

## v0.15.0 — Agent 进程化（激进迁移）（已完成 2026-06-18）

> **目标**：将 7 种专业 Agent 从库级 tokio task 迁移为独立进程，重构 AgentContext 为本地缓存 + 远程句柄。
> **前置条件**：v0.14.0 AgentOS 内核完成
> **破坏性变更**：eneros-agent crate 重构，现有 SpawnedAgent API 不兼容
> **关键保护**：7 种专业 Agent 的领域算法（经济调度、故障诊断、负荷预测、自愈、规划、交易）完整保留
> **详细任务分解**：[.trae/specs/agentos-native/tasks.md](./.trae/specs/agentos-native/tasks.md#v0150--agent-进程化激进迁移)
> **验收清单**：[.trae/specs/agentos-native/checklist.md](./.trae/specs/agentos-native/checklist.md#v0150--agent-进程化激进迁移)

### 任务 1：Agent trait 重构为进程入口点

* [x] `Agent` trait 保留领域方法，移除 `start/stop`

* [x] 新增 `AgentProcess` trait：`fn main(config: AgentConfig) -> Result<()>`

* [x] `AgentConfig`：`agent_id`、`agent_type`、`authority`、`jurisdiction`、`ipc_config`、`tick_interval`

* **验收**：Agent 可作为独立二进制运行 ✅

### 任务 2：7 种专业 Agent 拆为独立进程

* [x] 新增 `eneros-agent/bins/{dispatch,forecast,operation,self-healing,trading,planning}-agent/`

* [x] 每个 main.rs：加载配置 → 连接 EventBusBroker → 连接 Gateway IPC → 运行 tick 循环

* [x] self-healing-agent 为 RT 进程（SCHED\_FIFO）

* [x] **领域算法完整保留**

* **验收**：每个 Agent 独立进程运行，崩溃不影响其他 ✅

### 任务 3：AgentContext 重构为本地缓存 + 远程句柄

* [x] `AgentContext` 拆分为 `LocalContext` + `RemoteHandles`

* [x] `RemoteHandles`：`event_bus_client`、`gateway_client`、`memory_client`、`reasoning_client`、`network_snapshot`

* [x] `MessageStore` 替换为 `EventBusClient::subscribe()` + 本地游标

* **验收**：Agent 通过 RemoteHandles 访问远程服务 ✅

### 任务 4：ActionDispatcher 重构为 IPC 客户端

* [x] `ActionDispatcher` 持有 `GatewayClient`（IPC stub），替代 `Arc<SafetyGateway>`

* [x] `ExecuteCommand/ExecuteStructured/PublishEvent/DelegateTask` 路由到 IPC

* [x] `CallTool` 保留本地调用（ToolEngine 可在 Agent 进程内）

* **验收**：Agent 通过 IPC 下发命令到 Gateway 进程 ✅

### 任务 5：AgentOrchestrator 重构为远程编排

* [x] `AgentOrchestrator` 持有 `AgentRegistry`（远程查询）+ `EventBusClient`

* [x] `tick_all()` 改为广播 tick 事件，各 Agent 进程自行响应

* [x] `route_action()` 通过 EventBusClient 发送

* [x] 保留 `ConflictResolver` 仲裁逻辑

* **验收**：编排器可协调多个独立 Agent 进程 ✅

### 任务 6：eneros-init 集成 Agent 启动

* [x] `InitConfig` 新增 `agents: Vec<AgentServiceConfig>` 字段

* [x] `eneros-init/main.rs` 启动时：先启动系统服务 → 再启动 Agent 进程

* [x] `init.toml` 新增 `[agents]` 段

* **验收**：eneros-init 启动后自动 spawn 所有配置的 Agent ✅

### 任务 7：编译+测试+clippy 验证

* [x] `cargo build --workspace` 通过，0 error

* [x] `cargo test --workspace -- --test-threads=1` 全部通过（领域算法测试保留）

* [x] `cargo clippy --workspace --all-targets` 0 警告

* [x] 更新 CHANGELOG.md v0.15.0，标注 BREAKING CHANGES

***

## v0.16.0 — Gateway 进程化（已完成 2026-06-18）

> **目标**：将 SafetyGateway/DecisionPipeline 从库迁移为独立进程，通过 IPC 提供服务。
> **前置条件**：v0.15.0 Agent 进程化完成
> **详细任务分解**：[.trae/specs/agentos-native/tasks.md](./.trae/specs/agentos-native/tasks.md#v0160--gateway-进程化)
> **验收清单**：[.trae/specs/agentos-native/checklist.md](./.trae/specs/agentos-native/checklist.md#v0160--gateway-进程化)

### 任务 1：SafetyGateway 独立进程

* [x] 新增 `eneros-gateway/bins/gateway/src/main.rs`

* [x] `GatewayServer` TCP 服务端，接收 Command 请求

* [x] `execute_command/validate_command/submit_command` IPC 接口

* [x] 保留 per-device 锁池、safety\_checks、command\_history

* **验收**：Gateway 独立进程运行，Agent 通过 IPC 调用 ✅

### 任务 2：GatewayClient IPC 客户端

* [x] 新增 `eneros-gateway/src/client.rs`（v0.15.0 已完成）

* [x] `RemoteGatewayClient` TCP 客户端

* [x] `execute_command/validate_command/submit_command/decide` 接口

* **验收**：Agent 通过 GatewayClient 调用远程 Gateway ✅

### 任务 3：ConstrainedDecisionPipeline 进程化

* [x] 决策管线作为 Gateway 进程的子服务

* [x] `GatewayServer` 持有 `Arc<ConstrainedDecisionPipeline>`（通过 `LocalGatewayClient::with_pipeline()`）

* [x] `ObservationProvider` 可选注入（默认不配置）

* [x] **关键保护**：7 阶段管线逻辑不变（precondition→...→rollback）

* **验收**：Agent 通过 IPC 提交 StructuredAction，接收 DecisionResultCore ✅

### 任务 4：DeviceCommandExecutor IPC 化

* [x] DeviceManager 保留在 Gateway 进程内（方案 A）

* [x] 保留 write-then-readback 验证逻辑

* **验收**：命令通过 Gateway 进程下发到设备 ✅

### 任务 5：SharedPriorityCommandQueue 跨进程

* [x] `SharedPriorityCommandQueue` 保留在 Gateway 进程内

* [x] Agent 通过 `GatewayClient::submit_command()` IPC 投递

* [x] `RealtimeExecutor` 在 Gateway 进程内运行，消费队列

* **验收**：Agent 投递命令到 Gateway 队列，RealtimeExecutor 消费执行 ✅

### 任务 6：端到端集成测试

* [x] 新增 `tests/e2e_agentos.rs`

* [x] 测试流程：GatewayServer → RemoteGatewayClient → validate/execute/submit/decide → 验证响应

* [x] 测试 IPC 错误路径：连接拒绝、无管线 decide 返回错误

* **验收**：端到端测试通过（6 个测试全部通过）✅

### 任务 7：编译+测试+clippy 验证

* [x] `cargo build --workspace` 通过，0 error

* [x] `cargo test --workspace -- --test-threads=1` 全部通过

* [x] `cargo clippy --workspace --all-targets` v0.16.0 新增代码 0 警告

* [x] 更新 CHANGELOG.md v0.16.0

***

## v0.18.0 — 实时双执行域（已完成 2026-06-19）

### 核心交付

* eneros-rt 接线到 RealtimeExecutor 线程

* SCHED\_FIFO 优先级 80，CPU 隔离（isolcpus=2,3）

* mlockall 锁定内存

* rt/ipc.rs 真正无锁 SPSC（原子索引 + MaybeUninit）

* 硬件看门狗集成（/dev/watchdog，500ms 超时）

* 内核启动参数（isolcpus, nohz\_full, rcu\_nocbs）

* 实时性基准测试（P99 < 1ms）

### 验收标准

* RealtimeExecutor 线程运行在 SCHED\_FIFO + 隔离 CPU

* IPC 延迟 < 10μs，无 Mutex 争用

* 系统卡死 500ms 后硬件复位

* P99 < 1ms，P999 < 5ms

***

## v0.19.0 — 网络配置服务（已完成）

### 核心交付

* netcfg 静态 IP/VLAN/网桥配置，无 NetworkManager 依赖

* nftables 防火墙规则管理

* 网络 bonding（802.3ad LACP）

* 网络命名空间隔离

* DNS 配置与解析

* 网络热插拔支持

* enerosctl network 子命令

### 验收标准

* 配置后 ip addr 显示正确，重启后持久

* 未授权端口被拒绝

* 拔掉一根网线，通信不中断

* Agent 进程只能访问授权的网络资源

***

## v0.20.0 — 时间同步与日志（已完成）

### 核心交付

* timesync PTP IEEE 1588 优先，NTP 回退，精度 < 100μs

* PTP 硬件时钟管理（PHC）

* syslog 结构化 JSON 日志 + 轮转 + 持久化（7 天）

* 日志级别动态调整

* 日志远程转发（RFC 5424）

* 审计日志增强（防篡改 + 签名）

* enerosctl log 子命令

### 验收标准

* PTP 同步精度 < 100μs，NTP < 10ms

* 日志文件自动轮转，7 天后自动清理

* 审计日志不可篡改，可远程查询

***

## v0.21.0 — 设备管理与 HAL（已完成）

### 核心交付

* devmgr uevent 监听 + 设备枚举 + 热插拔

* HAL 完整 termios 配置（9600-115200 波特率）

* 串口设备管理（/dev/ttyS\* / /dev/ttyUSB\*）

* USB 设备管理（白名单授权）

* GPIO 设备接口（sysfs + libgpiod）

* I2C/SPI 设备接口

* enerosctl device 子命令

### 验收标准

* 插入设备后 1s 内识别并加载驱动

* 串口通信稳定，支持所有标准波特率

* GPIO 状态可读写，中断可触发

* I2C/SPI 传感器数据可读取

***

## v0.22.0 — 部署与 OTA 更新（已完成）

### 核心交付

* A/B 分区方案（boot A/B + rootfs A/B + data + config）

* OTA 完整流程（下载→校验→写入→切换→重启→回滚）

* TUF 签名更新包（Ed25519）

* eneros-imager v2（aarch64 交叉编译 + 配置注入）

* 声明式配置 eneros-machine.yaml

* 安装器（交互式 TUI + PXE 自动化）

* enerosctl update 子命令

### 验收标准

* 完整 OTA 流程通过，失败自动回滚

* 未签名更新包被拒绝

* 可构建 aarch64 镜像并在 ARM 设备上运行

* 全新设备可通过安装器完成部署

***

## v0.23.0 — 电力协议原生支持（已完成 2026-06-19）

### 核心交付

* GOOSE 协议 AF\_PACKET 原始套接字（Layer 2 直采）

* SV 协议 AF\_PACKET（IEC 61850-9-2 LE 直采）

* IEC 104 串口模式（FT 1.2 帧格式）

* Modbus RTU 串口模式

* 协议时间戳精确同步（PTP 对齐）

* 协议冗余路径管理（PRP/HSR）

* enerosctl protocol 子命令

### 验收标准

* GOOSE 报文延迟 < 1ms，无丢包

* SV 采样率精确，时间戳对齐

* 串口 IEC 104/Modbus RTU 通信稳定

* 单链路故障时通信不中断

***

## v0.24.0 — 安全加固（已完成）

### 核心交付

* Secure Boot（签名内核 + UEFI 变量 + OTA 包签名）

* 内核安全加固（HARDENED\_USERCOPY + FORTIFY\_SOURCE + 模块签名）

* seccomp 完整接线（Observer Agent 禁止 write() 到设备文件）

* 审计系统增强（系统级审计 + 防篡改 + 远程转发）

* 密钥管理服务（TPM 优先 + 软件回退）

* enerosctl security 子命令

### 验收标准

* 未签名内核无法启动

* 内核配置检查脚本通过

* Observer Agent 无法写入设备文件

* 所有安全事件被审计，日志不可篡改

### 实现详情

* `init/security.rs`：SecureBootManager（UEFI 变量读取 + Ed25519 签名验证 + 内核加固参数检查）

* `init/kms.rs`：KeyStore（AES-256-GCM 加密 + Argon2id 派生 + 访问控制 + 备份恢复）

* `os/boot/secure-boot.sh`：5 命令脚本（status/init-keys/sign-kernel/verify/enroll）

* `os/boot/grub.cfg`：内核命令行加固（page\_alloc.shuffle/slab\_nomerge/init\_on\_alloc/init\_on\_free）

* `os/kernel/config-*`：CONFIG\_SECURITY\_DMESG\_RESTRICT=y

* `agentos/seccomp.rs`：4 级 profile（Observer/Operator/Supervisor/Emergency）

* `init/audit.rs`：HMAC-SHA256 签名 + 链式哈希 + 远程转发 + 365 天保留

* `enerosctl security`：status/keys list/info/rotate/audit list/search/verify

### 验证结果

* `cargo build -p eneros-os`：0 编译错误，新增代码 0 警告

* `cargo test -p eneros-os --lib`：369 passed, 0 failed

* `cargo clippy -p eneros-os --all-targets`：新增代码 0 clippy 警告

***

## v0.25.0 — 高可用基础（已完成）

### 核心交付

* ✅ 双节点心跳（UDP 多播 100ms 间隔 + 300ms 故障检测 + 双网卡冗余 + 主/备角色优先级）

* ✅ 状态同步（SCADA/Agent/命令历史/配置 + 增量同步 + 延迟统计 < 100ms）

* ✅ 共享状态存储（应用级复制引擎 + 冲突检测/解决 + 配额管理）

* ✅ 脑裂防护（Fencing 框架 + 脑裂检测算法 + SCSI/IPMI/Network stub）

* ✅ HA 配置管理（HaConfig load/load\_from\_str + ha.toml 模板 + HaConfigError）

* ✅ enerosctl ha 子命令（status/nodes/sync-status/failover）

### 验收标准

* 节点故障 300ms 内检测到

* 备节点数据与主节点一致，延迟 < 100ms

* 脑裂发生时备节点被 fencing

### 验证结果

* `cargo build --workspace`：0 编译错误，新增代码 0 警告

* `cargo test --workspace --exclude eneros-installer`：全部通过（69 个 HA 测试）

* `cargo clippy --workspace --all-targets`：新增代码 0 clippy 警告

***

## v0.25.1 — HA 基础加固修复（已完成）

**验证日期**：2026-06-20
**验证结果**：11 个 CRITICAL + 21 个 HIGH 缺陷全部修复，核心功能可用

### 修复内容

* HaConfig 配置语义校验

* 心跳包 HMAC-SHA256 认证

* 双网卡冗余实现

* SyncManager 长连接 + 读取缓冲区 + pending drain + SharedStore 集成

* SharedStore role 可变 + replicate 配额 + delete 复制 + O(1) 配额

* Fencing 自 fencing 防护 + 速率限制 + 多节点校验 + 历史持久化

* RwLock 中毒安全处理

* CLI 桩实现标注 + failover 确认提示

### 留待 v0.26.0

* CLI IPC 控制通道（查询守护进程真实状态）

* 真实 failover 执行

* 持久化存储（WAL/快照）

* 二进制序列化、批量同步性能优化

***

## v0.26.0 — 高可用切换（已完成 2026-06-20）

### 核心交付

* 热备切换引擎（IP 接管 + ARP 广播，< 3s）

* 服务降级模式（备节点只读运行）

* 自动故障恢复（主节点恢复后重新同步）

* 多节点集群支持（>2 节点 + 仲裁节点）

* 灾备演练自动化

* enerosctl failover 子命令

### 验收标准

* 主节点故障后 3s 内备节点接管

* 备节点只读运行，不产生控制命令

* 3 节点集群稳定运行

* 灾备演练自动执行，结果可追溯

### 实现详情

* **HA 守护进程**（`eneros-ha`）：独立二进制，TCP IPC（127.0.0.1:5402），7 个控制命令

* **FailoverEngine**：5 状态状态机，VIP 漂移，切换 < 3s，JSON Lines 日志

* **服务降级**：is\_readonly 原子标志，HaEvent 事件发布

* **自动恢复**：增量同步（request\_incremental\_sync），RecoveryPolicy 策略

* **多节点集群**：ClusterManager + Quorum 多数派 + witness 仲裁

* **灾备演练**：DrillScheduler + 3 种场景 + 调度策略

* **持久化**：JSON 快照 + WAL，重启后状态恢复

* **CLI**：enerosctl failover 子命令通过 IPC 查询真实状态

* **集成**：eneros-init 启动 eneros-ha 系统服务

### 推迟到 v0.27.0 的项

* FencingManager::fence 的 Quorum 校验（仅 Leader 有权 fence）

* 集群成员变更通知回调（on\_member\_change）

* 二进制序列化、批量同步性能优化

***

## v0.27.0 — 插件系统（已完成 2026-06-20）

### 核心交付

* 插件框架核心（Plugin trait + 生命周期管理 + 注册表）

* 动态库加载（libloading + ABI 稳定性）

* 插件签名验证（Ed25519）

* 插件沙箱（seccomp + 资源配额 + 崩溃隔离）

* 协议适配器插件接口

* Agent 策略插件接口

* 分析模块插件接口

* enerosctl plugin 子命令

### 验收标准

* 插件可动态加载和卸载 ✓

* 未签名插件无法加载（require\_signature=true 时）✓

* 插件崩溃不影响主系统（catch\_unwind 隔离）✓

* 第三方协议/策略/分析可通过插件接入 ✓

### 实现详情

* **eneros-plugin crate**（`crates/eneros-plugin/`）：独立 crate，不依赖 eneros-device/eneros-agent/eneros-analysis，通过镜像类型避免循环依赖

* **Plugin 框架核心**：PluginManifest（TOML 三段式）/ PluginState（8 状态状态机）/ PluginRegistry（RwLock 线程安全）/ check\_dependencies + resolve\_load\_order（Kahn 拓扑排序）/ check\_compatibility（语义化版本兼容性）

* **动态库加载**：libloading 0.8 + C ABI 入口函数（eneros\_plugin\_create/destroy/metadata）+ PluginVTable 函数指针表，跨平台 .so/.dll/.dylib

* **签名验证**：Ed25519（复用 ed25519-dalek）+ generate\_keypair/sign\_plugin/verify\_plugin + 可信公钥管理 + require\_signature 配置

* **沙箱隔离**：PluginSeccompProfile（禁止 8 类危险 syscall）+ cgroups v2 资源配额 + catch\_unwind 崩溃隔离 + SandboxGuard RAII，Linux 完整功能 / 非 Linux stub

* **协议插件**：ProtocolPlugin trait + ProtocolType::Custom(String) 扩展 + IEC 103 示例插件

* **Agent 插件**：AgentPlugin trait + 权限上限 Operator（Emergency/Supervisor 强制降级）+ StrategyPriority 冲突解决 + 负荷均衡示例插件

* **分析插件**：AnalysisPlugin trait（serde\_json::Value 输入/输出）+ AnalysisScheduler 批量调度 + SAIFI/SAIDI/CAIDI 可靠性分析示例插件

* **CLI**：enerosctl plugin 子命令（list/load/unload/info/verify/enable/disable/gen-keys/sign 共 9 个），v0.27.0 直接调用库，IPC 推迟到 v0.28.0

* **配置**：/etc/eneros/plugin.toml（\[plugin]/\[quota]/\[sandbox] 三段）+ PluginConfig 结构体

* **错误扩展**：EnerOSError 增加 Plugin(String) 变体

### 推迟到 v0.28.0 的项

* 插件进程隔离（独立进程 + IPC 通信）

* \#\[eneros\_plugin] 过程宏（v0.27.0 用 C ABI 入口函数替代）

* plugin-daemon 独立守护进程（v0.27.0 CLI 直接调用库）

* 插件市场/远程仓库支持

***

## v0.28.0 — 开发者工具（已完成）

### 核心交付

* Rust SDK（Agent/协议/插件开发 SDK）

* 模拟器（电网/设备/故障注入/负荷曲线）

* enerosctl 全功能 CLI（所有子命令 + 交互式 shell + 自动补全）

* 开发者指南（ADR + 贡献指南 + 代码规范）

* 部署运维手册

* 用户手册

* API 文档完善（OpenAPI/Swagger）

### 验收标准

* 开发者可基于 SDK 快速开发 Agent ✅

* 模拟器可模拟真实电网场景 ✅

* CLI 覆盖所有管理功能 ✅

* 文档体系完整可查 ✅

***

## v0.29.0 — 技术债务清偿与架构加固（已完成）

> 详细任务分解见 [.trae/specs/roadmap-v029-to-v050/tasks.md](./.trae/specs/roadmap-v029-to-v050/tasks.md)

### 核心交付

**架构重构**：

* 新增 `eneros-runtime` 中间聚合层，`eneros-api` 直接依赖 < 5 个 crate

* 消除 `eneros-gateway` ↔ `eneros-agent` dev-dependencies 循环

* 反转 `eneros-topology` → `eneros-powerflow` 依赖方向

**API 补全（v0.6.0 推迟项）**：

* TraceLayer HTTP 请求追踪 + `X-Trace-Id` 响应头

* 结构化 JSON 日志 + 动态日志级别 API

* 分布式追踪 trace\_id 贯穿 Agent 管线

* TLS 加密运行时接线（`--tls-cert` / `--tls-key`）

* Agent 控制 API（start/stop/pause/resume/status）

* 校验/合规/规划/WhatIf/审计 5 个 API 端点

* WatchdogTimer 管线集成

* Dashboard 前端 SSE 实时刷新

**性能优化（v0.9.0 推迟项）**：

* 热点路径 p99 < 10ms（SCADA 采集、Agent 决策、命令下发）

* 时序数据 Gorilla 压缩存储（压缩比 > 5x）

* SCADA/Modbus/IEC 61850 连接池复用

* 决策管线结果 LRU 缓存（命中率 > 60%）

**基础设施（v0.9.0 推迟项）**：

* GitHub Actions 自动发布（tag 触发）

* tarpaulin 代码覆盖率报告 + Codecov 徽章

* OpenTelemetry OTLP gRPC 导出

* TDengine / InfluxDB 时序后端集成

**HA 增强（v0.26.0 推迟项）**：

* FencingManager Quorum 校验（已完成，T029-21）

* 集群成员变更通知回调（已完成，T029-22）

* bincode 二进制序列化 + 批量同步（已完成，T029-23：延迟下降 51.9%，带宽下降 72.1%）

**AgentOS（v0.14.0 推迟项）**：

* RT 域 SharedMemoryChannel（共享内存 + eventfd，延迟 < 1μs）

### 验收标准

* `cargo tree -p eneros-api` 直接依赖 < 5 个 crate

* 无 dev-deps 循环告警

* `eneros-topology` 不依赖 `eneros-powerflow`

* TraceLayer/JSON 日志/TLS/Agent 控制 API/校验合规等 API 全部可用

* 热点路径 p99 < 10ms

* 覆盖率徽章显示

* Jaeger UI 可查看 traces

* TDengine/InfluxDB 10 万点查询 < 100ms

* 3 节点 HA 集群，2 节点故障时 fencing 被拒绝

* SharedMemoryChannel 同进程延迟 < 1μs

***

## v0.30.0 — 生态成熟与质量保障（已完成）

> 细化为 7 个质量保障子领域，建立生产级质量准入标准。

### 核心交付

* **IEC 62443 安全认证**：SL1/SL2 安全等级文档 + 第三方预评估（SL1 符合率 91%，SL2 符合率 66%）

* **安全合规测试套件**：OWASP Top 10（A01-A10 全覆盖）+ `cargo audit` + SAST 静态扫描（硬编码密钥/不安全反序列化/SQL 注入/路径遍历）

* **端到端测试框架**：`TestCluster` 集群启动器（本地进程组模式），6 个测试场景，12 个集成测试

* **混沌工程测试**：自研轻量级混沌注入器（network/disk/cpu/memory/process 5 类），4 个混沌测试场景，50% 混沌下核心功能正常

* **电力协议一致性测试**：IEC 61850（MMS/GOOSE/SV）+ Modbus（TCP/RTU）+ IEC 60870-5-104，174 passed / 27 ignored

* **性能基准体系**：`criterion` 覆盖 SCADA/Agent/HA/API/PowerFlow，p50/p95/p99 + 回归检测（> 10% 失败）

* **测试覆盖率 > 80%**：补充 `eneros-powerflow`(+42)/`eneros-agent`(+54)/`eneros-ha`(+18) 共 114 个单元测试

### 验收标准

* IEC 62443 SL1 预评估就绪（符合率 91%）

* CI 安全扫描 0 高危

* e2e 测试框架就绪（CI 中运行）

* 50% 混沌注入下核心功能正常

* 协议一致性测试 174 passed / 27 ignored

* 性能回归 > 10% 时 CI 失败

* workspace 覆盖率报告就绪（tarpaulin + Codecov）

### 实现详情

* **新增 4 个测试 crate**：`tests/security/`（97 测试）、`tests/e2e/`（12 测试）、`tests/protocol_conformance/`（174 测试）、`benches/`（5 基准）

* **新增 4 个 CI 工作流**：`security.yml`、`e2e.yml`、`conformance.yml`、`benchmark.yml`

* **新增混沌注入器**：`eneros-test-utils::chaos` 模块，5 类注入器，跨平台兼容

* **新增 IEC 62443 文档**：`docs/compliance/iec-62443-4-1-sdlc.md` + `docs/compliance/iec-62443-4-2-sl-matrix.md`

***

## v0.31.0 — 数字孪生引擎（规划中）

### 核心交付

* 数字孪生核心引擎（`crates/eneros-twin/`，TwinModel 实时电网镜像，延迟 < 100ms）

* What-If 推演引擎（TwinModel 克隆 + 场景注入 + 潮流计算，10 并行推演 < 5s）

* 历史回放（时序数据库快照加载 + 倍速/暂停/继续）

* 虚拟传感器（衍生量测计算 + 公式定义 + 阈值告警，误差 < 1%）

### 验收标准

* 镜像状态与实时数据延迟 < 100ms

* 10 个并行推演 < 5s 完成

* 24 小时历史回放状态准确

* 虚拟传感器与物理传感器对比误差 < 1%

***

## v0.32.0 — 高级 API 与数据平台（规划中）

### 核心交付

* GraphQL API（`crates/eneros-graphql/`，基于 `async-graphql`，支持 Subscription）

* API 版本管理（URL `/api/v1/` `/api/v2/` + Header `Accept` + `Sunset` 弃用头）

* 限流与配额（IP/API Key 维度，`X-RateLimit-Remaining` 头）

* 时序数据查询 API（time range + downsampling + aggregation，支持 Flux/PromQL）

* 数据导出（CSV/Parquet/Arrow + 异步导出）

### 验收标准

* GraphQL Playground 可查询 + 订阅

* v1/v2 端点并存，弃用头正确返回

* 超限返回 429，头信息正确

* 100 万点查询 < 500ms

* 100 万行 CSV 导出 < 10s

***

## v0.33.0 — AI/ML 集成基础（规划中）

### 核心交付

* LLM 集成框架（`crates/eneros-ai/`，支持 OpenAI/Anthropic/Ollama，统一 `LlmClient` trait）

* 异常检测（统计 Z-score/IQR + ML Isolation Forest，流式实时检测，准确率 > 90%）

* 预测性维护（设备健康度评分 + RUL 预测，回测 MAE < 10%）

* 嵌入模型与向量检索（`candle`/`ort` 本地嵌入 + Qdrant/`usearch`，Top-10 准确率 > 80%）

* Agent LLM 增强（Prompt 模板 + 上下文注入 + LLM 决策步骤）

### 验收标准

* 3 个 LLM 后端可调用

* 异常检测准确率 > 90%

* RUL 预测 MAE < 10%

* 相似事件检索 Top-10 准确率 > 80%

* Agent 可基于 LLM 输出执行动作

***

## v0.34.0 — 边缘计算与云边协同（规划中）

### 核心交付

* 边缘 Agent 运行时（轻量模式，内存 < 50MB）

* 云边通信协议（gRPC 双向流，边缘注册/心跳/配置下发/数据上报）

* 模型分发（ONNX 打包 + 增量 diff + 自动加载，< 30s 全网生效）

* 边缘推理（`ort` ONNX Runtime + 版本管理 + A/B 测试，延迟 < 50ms）

* 边缘自治（断连自治 + 本地缓存 + 恢复同步，断网 1 小时无数据丢失）

### 验收标准

* ARM64 边缘设备启动成功，内存 < 50MB

* 1 云 + 10 边节点通信正常

* 模型更新 < 30s 全网生效

* 边缘推理延迟 < 50ms

* 断网 1 小时后恢复，数据无丢失

***

## v0.35.0 — 电力协议扩展（规划中）

### 核心交付

* IEC 60870-5-103 协议（`crates/eneros-protocol-iec103/`，主站/子站 + 故障录波 + 定值读写）

* CDT 循环式远动协议（`crates/eneros-protocol-cdt/`，部颁 CDT 92 + DL/T 634.5101）

* R-GOOSE 路由 GOOSE（跨子网 GOOSE 传输，延迟 < 10ms）

* PMU/PDC 同步相量（`crates/eneros-protocol-pmu/`，IEEE C37.118，30/60/120 Hz）

* ICCP（IEC 60870-6 TASE.2，控制中心间数据交换）

* 协议网关（`eneros-gateway` 任意 ↔ 任意转换，配置式映射）

### 验收标准

* IEC 103/CDT 与模拟器互通

* R-GOOSE 跨子网传输 < 10ms

* PMU 30/60/120 Hz 数据率正确

* ICCP 与测试服务器互通

* Modbus → IEC 61850 转换正确

***

## v0.36.0 — 高级电网分析（规划中）

### 核心交付

* 实时预想事故分析（N-1/N-2/自定义故障集，1000 故障 < 30s）

* 动态安全评估（暂态稳定 BCU + 时域仿真 + 电压稳定 CPF + 小干扰特征值，IEEE 39 节点 < 60s）

* 新能源建模（风电/光伏 + 出力波动 + 低电压穿越 + 预测接入）

* 储能系统建模（电池 SOC + 充放电效率 + 寿命 + 策略）

* 微电网分析（并网/孤岛切换 + 三相不平衡潮流）

### 验收标准

* 1000 故障扫描 < 30s

* IEEE 39 节点 DSA < 60s

* 新能源出力波动模拟正确

* 24 小时储能充放电仿真正确

* 并网/孤岛切换仿真稳定

***

## v0.37.0 — 插件生态与市场（规划中）

### 核心交付

* 插件市场（`crates/eneros-market/` + Web UI，上传/审核/发布/下载/评分 + 版本管理）

* 进程隔离插件（out-of-process + 沙箱 CPU/内存/文件系统，崩溃不影响主进程）

* 第三方 SDK（`eneros-sdk` 发布 crates.io + C/Python/Go 绑定）

* 插件 CI/CD 模板（GitHub Actions + 自动签名 + 自动发布）

* 插件依赖管理（依赖声明 + 自动解析 + 冲突检测）

### 验收标准

* 插件上传 + 下载 + 安装全流程通过

* 插件崩溃不影响主进程

* 第三方使用 SDK 开发插件并加载成功

* 使用模板的插件可自动发布到市场

* 依赖链正确解析，冲突检测有效

***

## v0.38.0 — 多租户与隔离（规划中）

### 核心交付

* 租户模型（`crates/eneros-tenant/`，CRUD + 成员管理 + 角色权限）

* 命名空间隔离（资源按 `tenant_id:resource_id` 隔离，跨租户访问拒绝）

* 租户配额（设备数/Agent 数/API 调用数/存储量，超额 429）

* 租户感知 API（`X-Tenant-ID` 头 + API Key 绑定租户）

### 验收标准

* 租户隔离测试通过

* 跨租户访问被拒绝

* 超额请求返回 429

* 不同租户 API Key 数据隔离

***

## v0.39.0 — 安全增强与零信任（规划中）

### 核心交付

* 零信任架构（服务间 mTLS + 基于身份/上下文的动态授权）

* 威胁检测（异常行为检测 + 威胁情报源集成）

* 安全自动化（漏洞扫描 + 自动修复 PR + 证书自动轮转）

* NERC CIP 合规（CIP-005 电子安全边界 + CIP-007 系统安全管理 + CIP-010 配置变更管理）

* 审计与取证（全量审计日志 + WORM 防篡改 + 签名链）

### 验收标准

* 未授权请求被拒绝

* 模拟攻击被检测并告警

* 证书到期前自动续期

* NERC CIP 合规审计通过

* 日志完整性校验通过

***

## v0.40.0 — 生产级运维自动化（规划中）

### 核心交付

* 高级监控（业务指标看板 + SLO/SLI 体系 + 错误预算）

* 自动运维（自动扩缩容 + 自动备份恢复）

* 事件响应（P0-P3 分级 + 自动分派 + On-call 排班 + Runbook 自愈）

* 容量规划（历史数据预测 + 提前 7 天预警，误差 < 15%）

* AIOps（告警降噪聚类 + 根因分析，噪声下降 > 70%）

### 验收标准

* SLO 看板实时更新

* 负载增加时自动扩容

* P0 事件 5 分钟内通知

* 预测误差 < 15%

* 告警噪声下降 > 70%

***

## v0.41.0 — 高可用增强（规划中）

### 核心交付

* 多区域 HA（跨数据中心 3 区域 × 3 节点 + 区域感知路由）

* 灾难恢复（异地连续复制 + RPO < 1s + RTO < 60s + DR 演练自动化）

* 零停机升级（滚动升级 + 健康检查 + 版本兼容性矩阵 + 回滚）

* 地理冗余（Active-Active 多活 + CRDT/最后写胜出冲突解决）

### 验收标准

* 单区域故障，服务 < 10s 恢复

* RPO < 1s，RTO < 60s，DR 演练通过

* 升级过程无请求失败

* 多活写入数据一致

***

## v0.42.0 — 性能极致优化（规划中）

### 核心交付

* 亚毫秒延迟（热点路径 lock-free + 批处理 + 异步化，p99 < 1ms）

* 高吞吐 SCADA（单节点 100 万点/秒 + 零拷贝）

* 内存优化（对象池 + arena 分配 + 流式处理，占用下降 > 50%）

* 共享内存 IPC（跨进程无锁环形缓冲，延迟 < 5μs）

* 内核旁路网络（DPDK/AF\_XDP 可选，延迟下降 > 50%）

### 验收标准

* 热点路径 p99 < 1ms

* 100 万点/秒采集稳定

* 内存占用下降 > 50%

* IPC 延迟 < 5μs

* 网络延迟下降 > 50%

***

## v0.43.0 — Agent 智能进阶（规划中）

### 核心交付

* 多 Agent 协作（消息总线 + 主从/对等/竞标模式）

* Agent 学习（强化学习 PPO + 模仿学习 + 在线学习 + 经验回放）

* 复杂任务分解（LLM 驱动 Task Decomposition + 子任务调度 + 结果聚合）

* 策略市场（策略打包/发布/订阅 + 版本管理 + A/B 测试）

* Agent 可解释性（决策路径可视化 + SHAP/LIME 解释）

### 验收标准

* 3 个 Agent 协作完成复杂任务

* Agent 决策准确率随时间提升

* 复杂任务自动分解为 5+ 子任务

* 策略订阅 + 加载成功

* 决策可追溯 + 可解释

***

## v0.44.0 — 电网分析进阶（规划中）

### 核心交付

* 动态状态估计（卡尔曼滤波 + 不良数据辨识 + 拓扑错误辨识）

* 实时稳定分析（在线暂态稳定基于 PMU + 稳定裕度评估，< 1s）

* 电磁暂态仿真 EMTP（`crates/eneros-emtp/` + 电力电子精细建模）

* 谐波分析（FFT 谐波检测 + 谐波潮流，精度 < 1%）

* 电网等值（Ward/REI 等值 + 外网等值自动更新，误差 < 2%）

### 验收标准

* 估计精度优于静态状态估计

* 实时稳定评估 < 1s

* IEEE 标准测试系统 EMTP 仿真正确

* 谐波检测精度 < 1%

* 等值后潮流误差 < 2%

***

## v0.45.0 — 物联网与泛在接入（规划中）

### 核心交付

* MQTT 协议（`crates/eneros-protocol-mqtt/`，MQTT 5.0 broker + client）

* CoAP 协议（`crates/eneros-protocol-coap/`，RFC 7252）

* LwM2M 设备管理（`crates/eneros-protocol-lwm2m/`，注册/固件升级/资源订阅）

* 传感器网络（低功耗传感器接入 + 数据聚合 + 异常过滤）

* 边缘设备管理（注册/配置/监控 + 批量 OTA）

### 验收标准

* 与 Mosquitto/libcoap 互通

* LwM2M 设备接入 + 管理

* 1000 传感器接入稳定

* 100 设备批量 OTA 升级成功

***

## v0.46.0 — 可视化与交互增强（规划中）

### 核心交付

* 高级仪表盘（可拖拽编辑器 + 自定义组件/主题/布局）

* 3D 可视化（WebGPU + 变电站/线路/设备 3D 模型，> 30 FPS）

* 自然语言接口（LLM 驱动查询，准确率 > 85%）

* 移动端（响应式 Web + PWA + WebView App）

* 报表与导出（日报/周报/月报 + PDF/Excel/Word + 邮件发送）

### 验收标准

* 自定义仪表盘保存 + 加载

* 3D 场景 > 30 FPS

* 自然语言查询准确率 > 85%

* 移动端可查看实时数据 + 告警

* 报表自动生成 + 邮件发送

***

## v0.47.0 — 国际化与合规（规划中）

### 核心交付

* 多语言 i18n（`fluent`/`rust-i18n` + `i18next`，中/英/日/西 4 种语言）

* 国际合规（GDPR/PIPL/CCPA + 数据本地化）

* 区域协议（北美 NERC/IEEE + 欧洲 ENTSO-E/IEC 62325 + 中国 DL/T 698/645）

* 本地化（时区/日期/货币/单位公制英制）

### 验收标准

* 4 种语言切换正确

* GDPR/PIPL/CCPA 合规检查通过

* 区域协议适配测试通过

* 不同区域配置显示正确

***

## v0.48.0 — 数据治理与隐私（规划中）

### 核心交付

* 数据治理（分类分级 + 生命周期管理）

* 隐私保护（差分隐私 DP + K-匿名/L-多样性）

* 数据血缘（字段级追踪 + DAG 可视化）

* 数据脱敏（动态基于角色 + 静态导出时，不可逆）

* 数据加密（字段级加密 + KMS 集成）

### 验收标准

* 数据分类标签正确

* 隐私攻击测试通过

* 血缘图完整 + 准确

* 脱敏后数据不可逆

* 加密字段查询正确

***

## v0.49.0 — 1.0 候选准备（规划中）

### 核心交付

* 功能冻结（除 P0 bug 修复外禁止新功能 + 未完成功能标记 1.0 后续）

* 全面测试（单元覆盖率 > 85% + e2e + 混沌 + 7×24 稳定性）

* 文档完善（用户/管理员/开发者/API/教程/FAQ）

* 性能验证（所有基准达标 + 与 v0.28.0 对比报告）

* 安全审计（第三方审计 + 渗透测试，无高危漏洞）

### 验收标准

* 功能冻结清单确认

* 覆盖率 > 85%，所有测试通过，无内存泄漏

* 文档评审通过

* 性能无回归，关键指标提升

* 无高危漏洞

***

## v0.50.0 — 1.0 Release Candidate（规划中）

### 核心交付

* 最终加固（修复所有 P0/P1 bug + 代码审查全覆盖）

* 生产就绪（3 个真实场景部署 + 30 天稳定运行 + 运维工具链完善）

* 认证完成（IEC 62443 SL2 + NERC CIP 合规认证）

* 发布候选（v1.0.0-rc.1 + 公开反馈征集）

* 1.0 路线图（GA 发布计划 + 1.x 长期维护计划）

### 验收标准

* 0 P0/P1 bug

* 3 个生产场景 30 天稳定运行

* IEC 62443 SL2 + NERC CIP 认证证书获取

* v1.0.0-rc.1 发布 + 反馈渠道建立

* 1.0 GA 计划文档完成

***

## 版本依赖与并行规划

### 版本间依赖关系

* v0.30.0 依赖 v0.29.0（质量保障需要架构稳定）

* v0.31.0 依赖 v0.29.0（数字孪生需要架构加固）

* v0.32.0 依赖 v0.29.0（高级 API 需要 API 框架完善）

* v0.33.0 依赖 v0.31.0（AI/ML 需要数字孪生训练数据）

* v0.34.0 依赖 v0.33.0（边缘计算需要 AI 模型）

* v0.35.0 独立（协议扩展可并行）

* v0.36.0 依赖 v0.31.0（高级分析需要数字孪生）

* v0.37.0 依赖 v0.29.0（插件生态需要架构稳定）

* v0.38.0 依赖 v0.32.0（多租户需要 API 平台）

* v0.39.0 依赖 v0.30.0（安全增强需要合规基础）

* v0.40.0 依赖 v0.30.0（运维自动化需要监控基础）

* v0.41.0 依赖 v0.29.0（HA 增强需要架构加固）

* v0.42.0 依赖 v0.29.0（性能优化需要架构稳定）

* v0.43.0 依赖 v0.33.0（Agent 智能需要 AI 基础）

* v0.44.0 依赖 v0.36.0（电网分析进阶需要基础分析）

* v0.45.0 独立（IoT 接入可并行）

* v0.46.0 依赖 v0.32.0（可视化需要 API 平台）

* v0.47.0 独立（国际化可并行）

* v0.48.0 依赖 v0.38.0（数据治理需要多租户）

* v0.49.0 依赖 v0.29-v0.48 所有版本

* v0.50.0 依赖 v0.49.0

### 可并行版本组

* 组 1：v0.35.0（协议扩展）、v0.37.0（插件生态）、v0.45.0（IoT）、v0.47.0（国际化）

* 组 2：v0.40.0（运维自动化）、v0.41.0（HA 增强）、v0.42.0（性能优化）

***

## 版本发布节奏

| 节奏               | 说明                                     |
| ---------------- | -------------------------------------- |
| **修订版本** (0.0.X) | 按需发布，仅包含 Bug 修复                        |
| **次版本** (0.X.0)  | 每 4-6 周一个次版本，包含新功能                     |
| **小版本** (0.x.y)  | 所有版本均为小版本更新，不升级到 1.0.0，持续在 0.x.y 范围内迭代 |

## 优先级原则

1. **P0 — 安全/数据完整性**：数据丢失、安全漏洞优先修复
2. **P1 — 核心功能**：影响主流程的 Bug 和功能缺失
3. **P2 — 体验优化**：性能、可观测性、文档
4. **P3 — 新功能扩展**：新协议、新分析模块、新 Agent 策略

## 架构设计原则

> 以下原则与 [.trae/specs/agentos-native/spec.md](./.trae/specs/agentos-native/spec.md) 第二章"关键设计原则"对齐。

1. **电力原生**：所有抽象以电力系统领域模型为核心，不套用通用 IT 模式
2. **AgentOS 原生**：Agent 是 OS 调度单元（独立进程），不是 tokio task，有 PID、权限、配额
3. **电力协议是 OS 能力**：GOOSE/SV 走 AF\_PACKET，IEC 104 串口走 /dev/ttyS
4. **安全约束是 OS 强制**：AuthorityLevel 映射为 Linux capabilities + seccomp
5. **共享 schema 单一来源**：eneros-core 作为所有进程的共享类型库
6. **领域算法保留**：7 种专业 Agent 的核心算法在进程化重构中完整保留
7. **分层解耦**：L0-L5 严格分层，上层依赖下层，禁止反向依赖
8. **可观测性内建**：每个组件自带 metrics/tracing，非事后补丁
9. **配置化**：所有参数可通过配置文件调整，无硬编码
10. **渐进式智能**：规则 → LLM → 强化学习，每层都有生产级实现

