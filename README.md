中文 | **[English](README_en.md)**

---

<div align="center">

# EnerOS（能枢OS）

### 能枢 — 电力/能源原生的 AgentOS

**聚能以枢，驱动万物智能**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

<table>
<tr><td>

**当前，人工智能 Agent 技术正以前所未有的速度重塑各行业的运作模式，然而电力与能源领域却面临着独特的挑战。**

通用 Agent 框架缺乏对电力系统物理规律的原生理解，安全约束被降级为提示词级别的建议，电网拓扑结构与电气耦合关系被忽视，设备异构性带来的协议与模型差异难以统一调度。这些根本性困境使得在通用框架上"外挂"电力知识的方案始终存在安全隐患与效率瓶颈。

**EnerOS** 是一个面向电力与能源领域的原生智能体操作系统（AgentOS）。它将电力系统的领域知识、物理约束与运行逻辑内建为操作系统内核，使 AI Agent 在能源场景中具备原生理解、安全决策与自主行动能力。正如传统操作系统为应用程序提供进程、文件、网络的统一抽象，EnerOS 为能源智能体提供拓扑、潮流、约束、设备的统一抽象——**让 Agent 天然"懂电"**。

</td></tr>
</table>

---

## 为什么需要 EnerOS？

通用 Agent 框架在电力能源领域面临根本性困境：

| 问题 | 表现 |
|------|------|
| **物理盲区** | Agent 不理解潮流、电压、频率等物理量，无法判断决策的物理可行性 |
| **约束缺失** | 安全约束（N-1、热稳定、电压越限）被当作"提示词"而非系统级保障 |
| **拓扑无感** | Agent 将电网视为扁平数据，无法感知拓扑结构与电气耦合关系 |
| **时序割裂** | 电力系统是强时序耦合系统，通用框架缺乏时间维度的一等公民支持 |
| **设备异构** | 变压器、断路器、逆变器各有独立模型与协议，难以统一调度 |

**EnerOS 的回答：不要在通用框架上"外挂"电力知识，而是从电力原生出发构建操作系统。**

---

## 设计哲学

### Power-Native First
电力拓扑、潮流计算、设备模型不是外挂插件，而是操作系统的原生抽象。Agent 从诞生起就运行在电网的物理世界模型之上。

### Agent-as-Grid-Node
每个 Agent 对应电网中的一个功能节点（厂站、馈线、设备），天然具备拓扑感知与约束遵守能力。Agent 之间的通信即电网节点之间的信息交换。

### Constraint as Kernel Law
安全约束（N-1 校验、热稳定、电压限值）由内核强制执行，任何 Agent 的决策不可逾越物理可行域。安全不是提示词，而是操作系统级的硬约束。

### Time-Series Native
电力系统是强时序耦合系统。EnerOS 将时间维度作为一等公民，支持实时数据流、历史回溯与预测推演的原生操作。

### Real-Time Determinism
电力系统对实时性有刚性需求。EnerOS 采用双执行架构：通用执行域承载 Agent 编排与 AI 推理，实时执行域保障保护逻辑与开关操作的确定性时延。安全域不可被通用域阻塞。

### Open & Interoperable
标准化的 Agent 通信协议与设备接入规范，支持异构能源设备与多厂商系统的即插即用。

---

## 系统架构

### 双执行架构：通用执行域 + 实时执行域

电力系统对实时性有刚性需求——继电保护动作必须在毫秒级完成，开关操作指令必须在确定时限内下发。通用操作系统内核无法提供硬实时保证，而纯实时系统又难以承载 AI 推理、Agent 编排等复杂计算。

EnerOS 采用**双执行架构**，将系统划分为两个执行域：

```
┌─────────────────────────────────────────────────────────────────┐
│                  通用执行域 (General Domain)                     │
│                                                                   │
│  Agent 运行时 · AI 推理引擎 · 规划与优化 · 人机交互              │
│  非确定性任务 · 响应时间：秒级 ~ 分钟级                           │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              实时安全网关 (RT Safety Gateway)                │ │
│  │    跨域通信 · 指令下发 · 状态同步 · 优先级仲裁              │ │
│  └────────────────────────┬────────────────────────────────────┘ │
│                           │ 域间通信                              │
├───────────────────────────┼─────────────────────────────────────┤
│                  实时执行域 (Real-Time Domain)                   │
│                                                                   │
│  继电保护逻辑 · 开关操作执行 · 故障隔离 · 频率调节               │
│  确定性任务 · 响应时间：微秒级 ~ 毫秒级                           │
│                                                                   │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────────┐ │
│  │  实时调度器  │ │  中断处理器  │ │  I/O 轮询引擎            │ │
│  │ (优先级抢占) │ │ (硬中断线程) │ │ (SCADA / IEC 104 / GOOSE)│ │
│  └──────────────┘ └──────────────┘ └──────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

| 执行域 | 内核模式 | 调度策略 | 典型任务 | 响应时延 |
|--------|----------|----------|----------|----------|
| **通用执行域** | 标准内核 | 公平调度 | Agent 编排、AI 推理、潮流计算、规划优化 | 秒级 ~ 分钟级 |
| **实时执行域** | 实时扩展内核 | 优先级抢占调度 | 继电保护、开关操作、故障隔离、频率调节 | 微秒级 ~ 毫秒级 |

**核心设计原则：**
- **安全域不可被通用域阻塞** — 实时执行域的任务拥有最高优先级，通用域的任何操作不得影响实时执行的确定性
- **单向信任** — 实时执行域可直接读取通用域的决策指令，但通用域不可直接干预实时执行域的调度
- **跨域通信通过实时安全网关** — 所有通用域→实时执行域的指令必须经过网关的约束校验与优先级仲裁
- **故障降级** — 当通用域异常时，实时执行域自动切换到本地保护逻辑，确保电网安全不依赖 AI

### 分层架构总览

```
┌──────────────────────────────────────────────────────────────────┐
│                           应用层                                │
│                                                                  │
│  调度 Agent · 运维 Agent · 规划 Agent · 交易 Agent               │
│  故障诊断 · 负荷预测 · 能效优化 · ...                            │
├──────────────────────────────────────────────────────────────────┤
│                        Agent 运行时层                            │
│                                                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────────┐  │
│  │  生命周期 │ │   记忆   │ │   工具   │ │   多智能体        │  │
│  │   管理   │ │   存储   │ │   引擎   │ │   协作            │  │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────────┘  │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────────────────┐    │
│  │   推理   │ │   安全   │ │  电网感知上下文注入           │    │
│  │   引擎   │ │   守卫   │ │ (拓扑 / 约束 / 时序)          │    │
│  └──────────┘ └──────────┘ └──────────────────────────────┘    │
├──────────────────────────────────────────────────────────────────┤
│                       电力原生内核                               │
│                                                                  │
│  ┌───────────────┐ ┌───────────────┐ ┌──────────────────────┐  │
│  │   拓扑引擎    │ │   潮流引擎    │ │    约束执行器        │  │
│  │  (图模型)     │ │  (PF / OPF)   │ │ (N-1 / 热稳定 / 电压)│  │
│  └───────────────┘ └───────────────┘ └──────────────────────┘  │
│  ┌───────────────┐ ┌───────────────┐ ┌──────────────────────┐  │
│  │  设备模型库   │ │  时序引擎     │ │    事件总线          │  │
│  │ (IEC / GB)    │ │(流式 / 历史)  │ │   (发布/订阅)        │  │
│  └───────────────┘ └───────────────┘ └──────────────────────┘  │
├──────────────────────────────────────────────────────────────────┤
│              实时安全网关 (RT Safety Gateway)                     │
│                                                                  │
│  跨域通信 · 指令下发 · 状态同步 · 优先级仲裁 · 约束校验         │
├──────────────────────────────────────────────────────────────────┤
│                         基础设施层                               │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              实时执行域 (Real-Time Domain)                  │ │
│  │  继电保护 · 开关操作 · 故障隔离 · 频率调节 · GOOSE        │ │
│  ├────────────────────────────────────────────────────────────┤ │
│  │              标准设备接入                                   │ │
│  │  SCADA · IEC 61850 · IEC 104 · MQTT · Modbus · OPC UA     │ │
│  └────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### 各层职责

| 层次 | 职责 | 关键抽象 | 执行域 |
|------|------|----------|--------|
| **应用层** | 面向业务场景的智能体应用 | 调度 / 运维 / 规划 / 交易 Agent | 通用域 |
| **Agent 运行时层** | Agent 生命周期管理与智能调度 | 生命周期 / 记忆 / 工具 / 推理 / 安全守卫 | 通用域 |
| **电力原生内核** | 电力系统物理世界建模与约束执行 | 拓扑 / 潮流 / 约束 / 设备 / 时序 / 事件 | 通用域 |
| **实时安全网关** | 跨域通信与指令安全校验 | 指令下发 / 状态同步 / 优先级仲裁 | 跨域 |
| **基础设施层** | 异构设备接入与实时控制执行 | SCADA / IEC 61850 / IEC 104 / MQTT / Modbus / OPC UA | 实时域 + 通用域 |

---

## 核心能力

### 电网拓扑一等公民
电网拓扑图是 EnerOS 的核心数据结构。Agent 通过拓扑感知上下文自动获取其所在节点的电气关系、上下游设备与运行状态，无需显式查询。

### 物理约束决策
所有 Agent 的决策输出经过电力原生内核的约束校验——潮流是否收敛、电压是否越限、线路是否过载。不满足物理约束的决策在内核层即被拒绝。

### 设备模型库
内置符合中国国标（GB）与国际电工委员会标准（IEC）的设备参数库，涵盖变压器、线路、开关、逆变器等核心设备类型，支持 pandapower 兼容格式。

### 多智能体协作
基于电网拓扑的 Agent 组织模型：同一厂站内的 Agent 自动形成协作组，跨厂站 Agent 通过拓扑路径进行结构化通信，避免全局广播的混乱。

### 时序原生操作
实时数据流、历史数据回溯、预测数据推演——三种时间模式在内核层统一抽象，Agent 可无缝切换"回顾-感知-预判"的时间视角。

### 安全守卫
内核级安全守卫：N-1 安全校验、热稳定校验、电压越限检测。安全约束不可被 Agent 绕过或降级，是操作系统的"硬法律"。

---

## Crate 索引

| Crate | 路径 | 职责 | 关键类型/接口 |
|-------|------|------|---------------|
| **eneros-core** | `crates/eneros-core/` | 统一类型、错误、配置 | `EnerOSError`, `EnerOSConfig`, `ElementId`, `BusType`, `PowerSystemState` |
| **eneros-topology** | `crates/eneros-topology/` | 电网拓扑图建模与分析 | `NetworkGraph`, `TopologyEngine`, `TopologySearcher`, `Bus`, `Branch`, `Switch` |
| **eneros-powerflow** | `crates/eneros-powerflow/` | Newton-Raphson 潮流求解 | `PowerFlowSolver`, `YBusMatrix`, `JacobianMatrix`, `PowerFlowResult` |
| **eneros-constraint** | `crates/eneros-constraint/` | 安全约束校验与可行性投影 | `ConstraintEngine`, `Constraint`, `Violation`, `FeasibilityProjector`, `WhatIfResult` |
| **eneros-equipment** | `crates/eneros-equipment/` | 设备参数模型库 | `EquipmentModel` trait, `EquipmentLibrary`, `TransmissionLine`, `TwoWindingTransformer` |
| **eneros-timeseries** | `crates/eneros-timeseries/` | 时序数据存储与查询（SQLite 持久化） | `TimeSeriesEngine`, `TimeSeriesStorage` trait, `TimeSeriesQuery`, `Aggregation` |
| **eneros-eventbus** | `crates/eneros-eventbus/` | 事件驱动通信总线 | `EventBus`, `Event`, `EventType`, `EventHandler` trait, `PriorityEventBus` |
| **eneros-gateway** | `crates/eneros-gateway/` | 安全网关、命令执行、决策管线 | `SafetyGateway`, `Command`, `CommandExecutor`, `ConstrainedDecisionPipeline`, `RealtimeExecutor` |
| **eneros-device** | `crates/eneros-device/` | 设备通信与协议适配（IEC104/IEC61850/Modbus/MQTT） | `ProtocolAdapter` trait, `DeviceManager`, `DeviceDiscovery`, `HealthMonitor` |
| **eneros-api** | `crates/eneros-api/` | CLI / HTTP API 服务 | `ApiServer`, `ApiClient`, `ApiResponse` |
| **eneros-bridge** | `crates/eneros-bridge/` | Python 桥接 (cnpower/pandapower) | `PythonBridge`, `CnpowerEquipmentLoader` |
| **eneros-network** | `crates/eneros-network/` | 拓扑-潮流统一管线与端到端测试 | `PowerNetwork`, `NetworkSimulatorAdapter` |
| **eneros-memory** | `crates/eneros-memory/` | Agent 记忆系统 | `MemoryStore` trait, `FileMemoryStore`, `MemoryEntry` |
| **eneros-tool** | `crates/eneros-tool/` | Agent 工具引擎 | `Tool` trait, `ToolRegistry`, `ToolResult` |
| **eneros-reasoning** | `crates/eneros-reasoning/` | 推理引擎（LLM + rig 集成） | `ReasoningEngine` trait, `RigReasoningEngine`, `FeedbackLoop` |
| **eneros-agent** | `crates/eneros-agent/` | Agent 运行时与领域 Agent | `Agent` trait, `DispatchAgent`, `SelfHealingAgent`, `Orchestrator`, `SystemStateMachine` |
| **eneros-scada** | `crates/eneros-scada/` | SCADA 数据采集（IEC 104 集成） | `ScadaEngine`, `DataSource` trait, `Iec104DataSource` |
| **eneros-analysis** | `crates/eneros-analysis/` | 电力系统分析（状态估计/OPF/短路） | `StateEstimator`, `OpfSolver`, `ShortCircuitAnalyzer`, `SequenceNetworks` |
| **eneros-dashboard** | `crates/eneros-dashboard/` | Web 仪表盘 | `DashboardServer`, `TopologySvg`, `FlowHeatmap`, `AgentPanel` |

### 依赖关系

```
eneros-core ◄── eneros-topology
             ◄── eneros-powerflow
             ◄── eneros-constraint
             ◄── eneros-equipment
             ◄── eneros-timeseries
             ◄── eneros-eventbus
             ◄── eneros-memory
             ◄── eneros-tool
             ◄── eneros-reasoning

eneros-core + eneros-eventbus ◄── eneros-device
eneros-core + eneros-equipment ◄── eneros-bridge
eneros-core + eneros-topology + eneros-powerflow + eneros-equipment ◄── eneros-network
eneros-core + eneros-device ◄── eneros-scada
eneros-powerflow + eneros-equipment ◄── eneros-analysis
eneros-gateway + eneros-agent + eneros-reasoning + eneros-tool ◄── eneros-api
eneros-gateway + eneros-agent + eneros-constraint ◄── eneros-dashboard
```

---

## 应用场景

| 场景 | 描述 | 核心 Agent |
|------|------|-----------|
| **智能调度** | 基于负荷预测与新能源出力的日前/日内/实时调度 | Dispatch Agent |
| **智能运维** | 设备状态监测、故障诊断与检修决策 | Operation Agent |
| **配网规划** | 负荷增长预测下的网架扩展与设备选型 | Planning Agent |
| **电力交易** | 现货市场报价策略与结算分析 | Trading Agent |
| **故障自愈** | 故障定位、隔离与非故障区域恢复供电 | Self-Healing Agent |
| **能效优化** | 工商业用户的用能优化与需求响应 | Energy Agent |

---

## 技术设计原则

- **内核-用户分离** — 物理约束执行在内核层，Agent 逻辑在用户层，安全边界清晰
- **图为中心** — 电网拓扑图是系统的核心索引，一切操作围绕图结构展开
- **事件驱动** — 基于事件总线的异步架构，适配电力系统的实时响应需求
- **插件架构** — 设备协议、求解器、Agent 能力均以插件形式接入，可扩展
- **标准合规** — 设备模型与通信协议遵循 IEC 61850 / IEC 60870-5-104 / GB 系列标准

---

## 对比

| 维度 | 通用 Agent 框架 | SCADA / EMS | **EnerOS** |
|------|----------------|-------------|------------|
| 电力物理建模 | 无 / 外挂 | 深度但封闭 | **原生内核** |
| AI Agent 支持 | 原生 | 无 | **原生** |
| 安全约束保障 | 提示词级 | 硬编码 | **内核级强制** |
| 拓扑感知 | 无 | 有 | **Agent 原生感知** |
| 多智能体协作 | 通用协议 | 无 | **拓扑结构化协作** |
| 开放性 | 高 | 低 | **高（插件架构）** |
| 设备模型标准 | 无 | 厂商私有 | **IEC / GB 标准** |

---

## 快速开始

### 前置条件

- Rust 1.70+ (通过 [rustup](https://rustup.rs/) 安装)
- Cargo

### 构建

```bash
# 克隆仓库
git clone https://github.com/Gawg-AI/EnerOS.git
cd EnerOS

# 构建项目
cargo build --release

# 运行测试
cargo test
```

### 运行

```bash
# 启动 API 服务器
cargo run --bin eneros -- serve --host 0.0.0.0 --port 8080

# 执行潮流计算
cargo run --bin eneros -- power-flow --case ieee14
```

---

## 路线图

> 详细版本规划见 [ROADMAP.md](ROADMAP.md)，历史变更见 [CHANGELOG.md](CHANGELOG.md)。

### 已完成

- [x] **Phase 1 — 内核基座** — 拓扑引擎、潮流计算内核、设备模型库
- [x] **Phase 2 — Agent 运行时** — Agent 生命周期管理、记忆系统、工具引擎
- [x] **Phase 3 — 电网感知上下文** — 拓扑感知注入、约束校验守卫、事件总线
- [x] **Phase 4 — 多智能体协作** — 多智能体协作协议、拓扑结构化通信
- [x] **Phase 5 — 基础设施适配器** — SCADA / IEC 61850 / IEC 104 / MQTT / Modbus 协议适配器
- [x] **Phase 6 — 领域应用** — 调度Agent(经济调度/AGC)、运维Agent(故障诊断/设备健康)、自愈Agent(故障隔离/网络重构)、领域协作协议
- [x] **Phase 7 — 实时闭环与系统集成** — SCADA数据管线、DC-OPF/状态估计/短路分析、负荷预测/规划/交易Agent、axum API+WebSocket+Web仪表盘
- [x] **Phase 8 — 深度集成与生产化** — 组件端到端连通、TOML配置加载、E2E集成测试、Dashboard集成、ApiClient真实HTTP、SQLite持久化
- [x] **Phase 9 — 修复真实Bug与消除空壳** — await_holding_lock死锁修复、SelfHealingAgent联锁校验、Y-bus计算bug修复、消息广播修复、重复代码消除、clippy零警告
- [x] **Phase 10 — 精度验证与LLM推理集成** — IEEE 14-bus标准答案精度验证、LlmReasoningEngine(OpenAI/Ollama/vLLM兼容)、Agent LLM推理增强、降级回退机制
- [x] **Phase 11 — rig Tool实化与统一推理引擎** — rig框架集成(rig-core 0.38)、4个电力系统Tool实化、RigReasoningEngine统一推理引擎、Feature flag隔离
- [x] **Phase 12 — 实时执行域** — PriorityCommandQueue优先级命令队列、RealtimeExecutor实时命令执行器、PriorityEventBus双通道事件总线、DualScanGroup快/慢扫描分组、WatchdogTimer看门狗超时保护
- [x] **Phase 13 — 约束驱动的确定性决策管道** — StructuredActionOutput、FeasibilityProjector可行性投影、ConstrainedDecisionPipeline三阶段管道、FeedbackLoop LLM反馈重推理
- [x] **Phase 14 — 接通确定性决策闭环** — 修复"幽灵闭环"、FeedbackLoop接入orchestrator、5个端到端闭环集成测试
- [x] **Phase 15 — 仿真器真实性增强** — 物理校正（变压器分接头、并联补偿器导纳、ZIP负荷约定、三绕组变压器模型）、数据完整性修复
- [x] **Phase 16 — 端到端管线验证** — 14个集成测试覆盖自愈场景、回滚计划、约束验证全链路
- [x] **Phase 17 — IEC 104 适配器** — 真实TCP协议栈、TESTFR心跳应答、半包/粘包处理、6个TCP传输测试
- [x] **v0.2.0 — 生产级架构修复（BUG3 全部9项）** — 接入层协议真实化、执行层命令落地（DeviceCommandExecutor + ACK验证）、状态机联动、冲突解析四级链、Holt-Winters真实实现、SQLite持久化write-through、分析层生产级化（真实雅可比/序网络/严格对偶LMP/参数化分接头）、P16闭环实测观测验证

### 进行中 / 规划中

- [ ] **v0.3.0 — 生产就绪** — 持久化全面接入、配置体系、可观测性（Prometheus/结构化日志）、安全加固（JWT/mTLS）
- [ ] **v0.4.0 — 协议覆盖完善** — Modbus RTU、IEC 104/61850增强、DNP3、OPC UA
- [ ] **v0.5.0 — Agent 智能化** — LLM多后端、多Agent协同、自愈策略库、预测精度提升
- [ ] **v0.6.0 — 分析层进阶** — 不良数据检测、AC-OPF、SCOPF、暂态稳定
- [ ] **v0.7.0 — 高可用部署** — 容器化、集群冗余、灾备、性能优化
- [ ] **v0.8.0 — 生态扩展** — 插件系统、GraphQL、可视化增强、文档体系

---

## 参与贡献

EnerOS 当前版本 v0.2.0，已完成核心架构的生产级修复，930+ 测试通过。欢迎对电力系统与 AI 交叉领域感兴趣的贡献者参与。请阅读 [ROADMAP.md](ROADMAP.md) 了解规划方向。

---

## 许可证

[MIT](LICENSE)

---

## 致谢

- [pandapower](https://github.com/e2nIEE/pandapower) - 电力系统分析
- [PowerGridModel](https://github.com/PowerGridModel/power-grid-model) - 高性能潮流计算
- [libIEC61850](https://github.com/mz-automation/libiec61850) - IEC 61850 协议实现
