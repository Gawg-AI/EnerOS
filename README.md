<div align="center">

# EnerOS

**能枢 — 电力/能源原生的 AgentOS**

*聚能以枢，驱动万物智能*

</div>

---

## What is EnerOS?

EnerOS 是面向电力与能源领域的原生智能体操作系统（AgentOS）。它将电力系统的领域知识、工程约束与运行逻辑内建为系统基座，让 AI Agent 在能源场景中具备原生理解与决策能力。

传统通用 Agent 框架缺乏对电力拓扑、潮流约束、设备特性等领域的深度建模，导致在调度、运维、规划等核心场景中难以可靠落地。EnerOS 从电力原生出发，重新定义 Agent 与能源基础设施的交互范式。

## Core Ideas

- **Power-Native** — 电力拓扑、潮流计算、设备模型不是外挂插件，而是操作系统的原生抽象
- **Agent-as-Grid-Node** — 每个 Agent 对应电网中的一个功能节点，天然具备拓扑感知与约束遵守能力
- **Energy-Aware Scheduling** — 调度决策始终在物理可行域内，安全约束不可逾越
- **Open Protocol** — 标准化的 Agent 通信协议，支持异构能源设备的即插即用

## Architecture

```
┌─────────────────────────────────────────────┐
│              Application Layer              │
│   Dispatch · Operation · Planning · Trading │
├─────────────────────────────────────────────┤
│              Agent Runtime Layer            │
│  Agent Lifecycle · Memory · Tool · Reasoning│
├─────────────────────────────────────────────┤
│           Power-Native Kernel               │
│  Topology · Power Flow · Constraints · Model│
├─────────────────────────────────────────────┤
│           Infrastructure Layer              │
│  Data Bus · Device Protocol · Time-Series   │
└─────────────────────────────────────────────┘
```

## Status

EnerOS 目前处于早期设计阶段，核心架构与接口规范正在构建中。

## License

MIT
