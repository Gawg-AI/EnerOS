<div align="center">

# EnerOS

### 能枢 — 电力/能源原生的 AgentOS

**聚能以枢，驱动万物智能**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

## Why EnerOS?

当前 AI Agent 技术蓬勃发展，但通用 Agent 框架在电力能源领域面临根本性困境：

| 问题 | 表现 |
|------|------|
| **物理盲区** | Agent 不理解潮流、电压、频率等物理量，无法判断决策的物理可行性 |
| **约束缺失** | 安全约束（N-1、热稳定、电压越限）被当作"提示词"而非系统级保障 |
| **拓扑无感** | Agent 将电网视为扁平数据，无法感知拓扑结构与电气耦合关系 |
| **时序割裂** | 电力系统是强时序耦合系统，通用框架缺乏时间维度的一等公民支持 |
| **设备异构** | 变压器、断路器、逆变器各有独立模型与协议，难以统一调度 |

**EnerOS 的回答：不要在通用框架上"外挂"电力知识，而是从电力原生出发构建操作系统。**

---

## What is EnerOS?

EnerOS 是面向电力与能源领域的原生智能体操作系统（AgentOS）。它将电力系统的领域知识、物理约束与运行逻辑内建为操作系统内核，使 AI Agent 在能源场景中具备原生理解、安全决策与自主行动能力。

正如传统操作系统为应用程序提供进程、文件、网络的统一抽象，EnerOS 为能源智能体提供拓扑、潮流、约束、设备的统一抽象——**让 Agent 天然"懂电"**。

---

## Design Philosophy

### Power-Native First
电力拓扑、潮流计算、设备模型不是外挂插件，而是操作系统的原生抽象。Agent 从诞生起就运行在电网的物理世界模型之上。

### Agent-as-Grid-Node
每个 Agent 对应电网中的一个功能节点（厂站、馈线、设备），天然具备拓扑感知与约束遵守能力。Agent 之间的通信即电网节点之间的信息交换。

### Constraint as Kernel Law
安全约束（N-1 校验、热稳定、电压限值）由内核强制执行，任何 Agent 的决策不可逾越物理可行域。安全不是提示词，而是操作系统级的硬约束。

### Time-Series Native
电力系统是强时序耦合系统。EnerOS 将时间维度作为一等公民，支持实时数据流、历史回溯与预测推演的原生操作。

### Open & Interoperable
标准化的 Agent 通信协议与设备接入规范，支持异构能源设备与多厂商系统的即插即用。

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                        Application Layer                        │
│                                                                  │
│  Dispatch Agent · Operation Agent · Planning Agent · Trading Agent│
│  Fault Diagnosis · Load Forecasting · Energy Optimization · ...  │
├──────────────────────────────────────────────────────────────────┤
│                       Agent Runtime Layer                        │
│                                                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────────┐  │
│  │ Lifecycle │ │  Memory  │ │  Tool    │ │ Multi-Agent       │  │
│  │ Manager   │ │  Store   │ │  Engine  │ │ Coordination      │  │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────────┘  │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────────────────┐    │
│  │Reasoning │ │ Security │ │ Grid-Aware Context Injection  │    │
│  │ Engine   │ │ Guard    │ │ (Topology / Constraint / Time)│    │
│  └──────────┘ └──────────┘ └──────────────────────────────┘    │
├──────────────────────────────────────────────────────────────────┤
│                     Power-Native Kernel                          │
│                                                                  │
│  ┌───────────────┐ ┌───────────────┐ ┌──────────────────────┐  │
│  │ Topology      │ │ Power Flow    │ │ Constraint           │  │
│  │ Engine        │ │ Engine        │ │ Enforcer             │  │
│  │ (Graph Model) │ │ (PF / OPF)   │ │ (N-1 / Thermal / V)  │  │
│  └───────────────┘ └───────────────┘ └──────────────────────┘  │
│  ┌───────────────┐ ┌───────────────┐ ┌──────────────────────┐  │
│  │ Equipment     │ │ Time-Series   │ │ Event                │  │
│  │ Model Store   │ │ Engine        │ │ Bus                  │  │
│  │ (IEC / GB)    │ │ (Stream / Hist│ │ (Pub/Sub)            │  │
│  └───────────────┘ └───────────────┘ └──────────────────────┘  │
├──────────────────────────────────────────────────────────────────┤
│                      Infrastructure Layer                        │
│                                                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────────┐  │
│  │ SCADA    │ │ IEC 61850│ │ MQTT     │ │ OPC UA           │  │
│  │ Connector│ │ Mapping  │ │ Broker   │ │ Client           │  │
│  └──────────┘ └──────────┘ └──────────┘ └──────────────────┘  │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────────────────┐   │
│  │ IEC 104  │ │ Modbus   │ │ Custom Protocol Adapter      │   │
│  │ Client   │ │ Gateway  │ │ (Plug-in Architecture)       │   │
│  └──────────┘ └──────────┘ └──────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

| 层次 | 职责 | 关键抽象 |
|------|------|----------|
| **Application Layer** | 面向业务场景的智能体应用 | Dispatch / Operation / Planning / Trading Agent |
| **Agent Runtime Layer** | Agent 生命周期管理与智能调度 | Lifecycle / Memory / Tool / Reasoning / Security Guard |
| **Power-Native Kernel** | 电力系统物理世界建模与约束执行 | Topology / PowerFlow / Constraint / Equipment / TimeSeries / Event |
| **Infrastructure Layer** | 异构设备接入与数据采集 | SCADA / IEC 61850 / IEC 104 / MQTT / Modbus / OPC UA |

---

## Core Capabilities

### Grid Topology as First-Class Citizen
电网拓扑图是 EnerOS 的核心数据结构。Agent 通过拓扑感知上下文自动获取其所在节点的电气关系、上下游设备与运行状态，无需显式查询。

### Physics-Constrained Decision Making
所有 Agent 的决策输出经过 Power-Native Kernel 的约束校验——潮流是否收敛、电压是否越限、线路是否过载。不满足物理约束的决策在内核层即被拒绝。

### Equipment Model Store
内置符合中国国标（GB）与国际电工委员会标准（IEC）的设备参数库，涵盖变压器、线路、开关、逆变器等核心设备类型，支持 pandapower 兼容格式。

### Multi-Agent Coordination
基于电网拓扑的 Agent 组织模型：同一厂站内的 Agent 自动形成协作组，跨厂站 Agent 通过拓扑路径进行结构化通信，避免全局广播的混乱。

### Time-Series Native Operations
实时数据流、历史数据回溯、预测数据推演——三种时间模式在内核层统一抽象，Agent 可无缝切换"回顾-感知-预判"的时间视角。

### Security Guard
内核级安全守卫：N-1 安全校验、热稳定校验、电压越限检测。安全约束不可被 Agent 绕过或降级，是操作系统的"硬法律"。

---

## Application Scenarios

| 场景 | 描述 | 核心 Agent |
|------|------|-----------|
| **智能调度** | 基于负荷预测与新能源出力的日前/日内/实时调度 | Dispatch Agent |
| **智能运维** | 设备状态监测、故障诊断与检修决策 | Operation Agent |
| **配网规划** | 负荷增长预测下的网架扩展与设备选型 | Planning Agent |
| **电力交易** | 现货市场报价策略与结算分析 | Trading Agent |
| **故障自愈** | 故障定位、隔离与非故障区域恢复供电 | Self-Healing Agent |
| **能效优化** | 工商业用户的用能优化与需求响应 | Energy Agent |

---

## Technical Design Principles

- **Kernel-User Separation** — 物理约束执行在内核层，Agent 逻辑在用户层，安全边界清晰
- **Graph-Centric** — 电网拓扑图是系统的核心索引，一切操作围绕图结构展开
- **Event-Driven** — 基于事件总线的异步架构，适配电力系统的实时响应需求
- **Plugin Architecture** — 设备协议、求解器、Agent 能力均以插件形式接入，可扩展
- **Standard-Compliant** — 设备模型与通信协议遵循 IEC 61850 / IEC 60870-5-104 / GB 系列标准

---

## Comparison

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

## Roadmap

- [ ] **Phase 1 — Kernel Foundation** — 拓扑引擎、潮流计算内核、设备模型库
- [ ] **Phase 2 — Agent Runtime** — Agent 生命周期管理、记忆系统、工具引擎
- [ ] **Phase 3 — Grid-Aware Context** — 拓扑感知注入、约束校验守卫、事件总线
- [ ] **Phase 4 — Multi-Agent Coordination** — 多智能体协作协议、拓扑结构化通信
- [ ] **Phase 5 — Infrastructure Adapters** — SCADA / IEC 61850 / MQTT 协议适配器
- [ ] **Phase 6 — Domain Applications** — 调度、运维、规划等场景化智能体应用

---

## Contributing

EnerOS 处于早期设计阶段，欢迎对电力系统与 AI 交叉领域感兴趣的贡献者参与讨论与共建。

---

## License

[MIT](LICENSE)
