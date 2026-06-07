**[中文](README.md)** | English

---

<div align="center">

# EnerOS

### Power/Energy-Native AgentOS

**Converge Energy at the Hub, Drive Intelligence in All Things**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

## Why EnerOS?

AI Agent technology is booming, yet general-purpose Agent frameworks face fundamental challenges in the power and energy domain:

| Problem | Manifestation |
|---------|---------------|
| **Physics Blindness** | Agents cannot understand power flow, voltage, frequency — unable to judge physical feasibility of decisions |
| **Missing Constraints** | Safety constraints (N-1, thermal stability, voltage limits) are treated as "prompts" rather than system-level guarantees |
| **Topology Unawareness** | Agents view the grid as flat data, unable to perceive topology structure and electrical coupling |
| **Temporal Disconnection** | Power systems are strongly time-coupled; general frameworks lack first-class time-dimension support |
| **Device Heterogeneity** | Transformers, breakers, inverters each have distinct models and protocols, making unified dispatch difficult |

**EnerOS's answer: Don't "bolt on" power knowledge to generic frameworks — build the OS power-native from the ground up.**

---

## What is EnerOS?

EnerOS is a native intelligent agent operating system (AgentOS) designed for the power and energy domain. It embeds domain knowledge, physical constraints, and operational logic of power systems into the OS kernel, enabling AI Agents with native understanding, safe decision-making, and autonomous action in energy scenarios.

Just as a traditional OS provides unified abstractions of processes, files, and networking for applications, EnerOS provides unified abstractions of topology, power flow, constraints, and equipment for energy agents — **making Agents natively "understand electricity"**.

---

## Design Philosophy

### Power-Native First
Power topology, power flow computation, and equipment models are not plug-ins — they are native OS abstractions. Agents are born running on top of the grid's physical world model.

### Agent-as-Grid-Node
Each Agent corresponds to a functional node in the grid (substation, feeder, device), inherently possessing topology awareness and constraint compliance. Inter-Agent communication mirrors information exchange between grid nodes.

### Constraint as Kernel Law
Safety constraints (N-1 verification, thermal stability, voltage limits) are enforced by the kernel — no Agent decision may exceed the physically feasible domain. Safety is not a prompt; it is a hard OS-level constraint.

### Time-Series Native
Power systems are strongly time-coupled. EnerOS treats the time dimension as a first-class citizen, supporting native operations for real-time data streams, historical lookback, and predictive forecasting.

### Open & Interoperable
Standardized Agent communication protocols and device integration specifications enable plug-and-play for heterogeneous energy devices and multi-vendor systems.

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

| Layer | Responsibility | Key Abstractions |
|-------|---------------|------------------|
| **Application Layer** | Business-scenario-oriented agent applications | Dispatch / Operation / Planning / Trading Agent |
| **Agent Runtime Layer** | Agent lifecycle management and intelligent scheduling | Lifecycle / Memory / Tool / Reasoning / Security Guard |
| **Power-Native Kernel** | Power system physical world modeling and constraint enforcement | Topology / PowerFlow / Constraint / Equipment / TimeSeries / Event |
| **Infrastructure Layer** | Heterogeneous device integration and data acquisition | SCADA / IEC 61850 / IEC 104 / MQTT / Modbus / OPC UA |

---

## Core Capabilities

### Grid Topology as First-Class Citizen
The grid topology graph is EnerOS's core data structure. Agents automatically acquire their node's electrical relationships, upstream/downstream devices, and operational status through topology-aware context — no explicit queries needed.

### Physics-Constrained Decision Making
All Agent decision outputs pass through the Power-Native Kernel's constraint verification — whether power flow converges, voltage exceeds limits, or lines are overloaded. Decisions failing physical constraints are rejected at the kernel level.

### Equipment Model Store
Built-in equipment parameter library compliant with Chinese national standards (GB) and IEC standards, covering transformers, lines, switches, inverters, and other core equipment types, with pandapower-compatible format support.

### Multi-Agent Coordination
A grid-topology-based Agent organization model: Agents within the same substation automatically form collaboration groups; cross-substation Agents communicate structurally along topology paths, avoiding the chaos of global broadcasting.

### Time-Series Native Operations
Real-time data streams, historical lookback, and predictive forecasting — three temporal modes are unified at the kernel level, allowing Agents to seamlessly switch between "review — perceive — predict" time perspectives.

### Security Guard
Kernel-level security guard: N-1 safety verification, thermal stability check, voltage limit detection. Safety constraints cannot be bypassed or downgraded by any Agent — they are the "hard law" of the operating system.

---

## Application Scenarios

| Scenario | Description | Core Agent |
|----------|-------------|------------|
| **Intelligent Dispatch** | Day-ahead / intra-day / real-time dispatch based on load forecasting and renewable output | Dispatch Agent |
| **Smart Operation & Maintenance** | Equipment condition monitoring, fault diagnosis, and maintenance decisions | Operation Agent |
| **Distribution Planning** | Network expansion and equipment selection under load growth projections | Planning Agent |
| **Power Trading** | Spot market bidding strategies and settlement analysis | Trading Agent |
| **Self-Healing** | Fault location, isolation, and service restoration for non-faulted areas | Self-Healing Agent |
| **Energy Optimization** | Energy consumption optimization and demand response for commercial/industrial users | Energy Agent |

---

## Technical Design Principles

- **Kernel-User Separation** — Physical constraint enforcement in the kernel layer, Agent logic in the user layer; clear security boundary
- **Graph-Centric** — Grid topology graph is the system's core index; all operations revolve around the graph structure
- **Event-Driven** — Asynchronous architecture based on event bus, adapted to real-time response requirements of power systems
- **Plugin Architecture** — Device protocols, solvers, and Agent capabilities are all plugged in, ensuring extensibility
- **Standard-Compliant** — Equipment models and communication protocols follow IEC 61850 / IEC 60870-5-104 / GB series standards

---

## Comparison

| Dimension | General Agent Framework | SCADA / EMS | **EnerOS** |
|-----------|------------------------|-------------|------------|
| Power Physics Modeling | None / Bolt-on | Deep but closed | **Native Kernel** |
| AI Agent Support | Native | None | **Native** |
| Safety Constraint Guarantee | Prompt-level | Hardcoded | **Kernel-level Enforcement** |
| Topology Awareness | None | Yes | **Agent-native Awareness** |
| Multi-Agent Coordination | Generic protocol | None | **Topology-structured Collaboration** |
| Openness | High | Low | **High (Plugin Architecture)** |
| Equipment Model Standards | None | Vendor-proprietary | **IEC / GB Standards** |

---

## Roadmap

- [ ] **Phase 1 — Kernel Foundation** — Topology engine, power flow computation kernel, equipment model store
- [ ] **Phase 2 — Agent Runtime** — Agent lifecycle management, memory system, tool engine
- [ ] **Phase 3 — Grid-Aware Context** — Topology-aware injection, constraint verification guard, event bus
- [ ] **Phase 4 — Multi-Agent Coordination** — Multi-agent collaboration protocol, topology-structured communication
- [ ] **Phase 5 — Infrastructure Adapters** — SCADA / IEC 61850 / MQTT protocol adapters
- [ ] **Phase 6 — Domain Applications** — Scenario-specific agent applications for dispatch, O&M, planning, etc.

---

## Contributing

EnerOS is in its early design stage. Contributors interested in the intersection of power systems and AI are welcome to join the discussion and co-build.

---

## License

[MIT](LICENSE)
