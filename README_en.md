**[中文](README.md)** | English

---

<div align="center">

# EnerOS

### Power/Energy-Native AgentOS

**Converge Energy at the Hub, Drive Intelligence in All Things**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

> **AI Agent technology is reshaping industries at an unprecedented pace, yet the power and energy domain faces unique challenges.** General-purpose Agent frameworks lack native understanding of power system physics; safety constraints are demoted to prompt-level suggestions; grid topology and electrical coupling are overlooked; protocol and model heterogeneity across devices makes unified dispatch intractable. These fundamental gaps render "bolt-on" approaches to power knowledge inherently unsafe and inefficient.
>
> **EnerOS** is a native intelligent agent operating system (AgentOS) designed for the power and energy domain. It embeds domain knowledge, physical constraints, and operational logic of power systems into the OS kernel, enabling AI Agents with native understanding, safe decision-making, and autonomous action in energy scenarios. Just as a traditional OS provides unified abstractions of processes, files, and networking for applications, EnerOS provides unified abstractions of topology, power flow, constraints, and equipment for energy agents — **making Agents natively "understand electricity"**.

---

## Why EnerOS?

General-purpose Agent frameworks face fundamental challenges in the power and energy domain:

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

### Real-Time Determinism
Power systems have rigid real-time requirements. EnerOS adopts a dual-execution architecture: a standard Linux soft base for Agent orchestration and AI inference, and a PREEMPT_RT hard execution domain for deterministic latency in protection logic and breaker operations. The safety domain cannot be blocked by the soft base.

### Open & Interoperable
Standardized Agent communication protocols and device integration specifications enable plug-and-play for heterogeneous energy devices and multi-vendor systems.

---

## Architecture

### Dual-Execution Architecture: Soft Base + PREEMPT_RT Hard Execution

Power systems have rigid real-time requirements — relay protection must act within milliseconds, breaker commands must be issued within deterministic deadlines. Standard Linux kernels cannot provide hard real-time guarantees, while pure real-time systems struggle to support complex workloads like AI inference and Agent orchestration.

EnerOS adopts a **dual-execution architecture**, dividing the system into two execution domains:

```
┌─────────────────────────────────────────────────────────────────┐
│                  Soft Base (Standard Linux)                      │
│                                                                   │
│  Agent Runtime · AI Inference · Planning & Optimization · HMI    │
│  Non-deterministic tasks · Latency: seconds ~ minutes            │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              RT Safety Gateway                               │ │
│  │    Cross-domain Comm · Command Dispatch · State Sync         │ │
│  │    Priority Arbitration · Constraint Verification            │ │
│  └────────────────────────┬────────────────────────────────────┘ │
│                           │ IPC / Shared Memory                   │
├───────────────────────────┼─────────────────────────────────────┤
│                  PREEMPT_RT Hard Execution Domain                │
│                                                                   │
│  Relay Protection · Breaker Operations · Fault Isolation          │
│  Frequency Regulation · Deterministic tasks                       │
│  Latency: microseconds ~ milliseconds                             │
│                                                                   │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────────┐ │
│  │  RT Scheduler │ │  IRQ Thread  │ │  I/O Polling Engine      │ │
│  │ (SCHED_FIFO)  │ │  Handler     │ │ (SCADA / IEC 104 / GOOSE)│ │
│  └──────────────┘ └──────────────┘ └──────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

| Execution Domain | Kernel | Scheduling | Typical Tasks | Latency |
|------------------|--------|------------|---------------|---------|
| **Soft Base** | Standard Linux | CFS (Completely Fair Scheduler) | Agent orchestration, AI inference, power flow, planning | Seconds ~ Minutes |
| **PREEMPT_RT Hard Execution** | Linux + PREEMPT_RT patch | SCHED_FIFO / SCHED_RR | Relay protection, breaker ops, fault isolation, frequency regulation | Microseconds ~ Milliseconds |

**Core Design Principles:**

- **Safety domain cannot be blocked by soft base** — PREEMPT_RT real-time tasks have the highest priority; no soft base operation may affect hard execution determinism
- **Unidirectional trust** — The hard execution domain can directly read soft base decisions, but the soft base cannot directly intervene in hard execution scheduling
- **Cross-domain communication via RT Safety Gateway** — All soft base → hard execution commands must pass through the gateway's constraint verification and priority arbitration
- **Graceful degradation** — When the soft base fails, the hard execution domain automatically switches to local protection logic, ensuring grid safety does not depend on AI

### Layered Architecture Overview

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
│              RT Safety Gateway                                    │
│                                                                  │
│  Cross-domain Comm · Command Dispatch · State Sync               │
│  Priority Arbitration · Constraint Verification                  │
├──────────────────────────────────────────────────────────────────┤
│                      Infrastructure Layer                        │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              PREEMPT_RT Hard Execution Domain               │ │
│  │  Relay Protection · Breaker Ops · Fault Isolation           │ │
│  │  Frequency Regulation · GOOSE                               │ │
│  ├────────────────────────────────────────────────────────────┤ │
│  │              Standard Device Integration                     │ │
│  │  SCADA · IEC 61850 · IEC 104 · MQTT · Modbus · OPC UA      │ │
│  └────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

| Layer | Responsibility | Key Abstractions | Execution Domain |
|-------|---------------|------------------|------------------|
| **Application Layer** | Business-scenario-oriented agent applications | Dispatch / Operation / Planning / Trading Agent | Soft Base |
| **Agent Runtime Layer** | Agent lifecycle management and intelligent scheduling | Lifecycle / Memory / Tool / Reasoning / Security Guard | Soft Base |
| **Power-Native Kernel** | Power system physical world modeling and constraint enforcement | Topology / PowerFlow / Constraint / Equipment / TimeSeries / Event | Soft Base |
| **RT Safety Gateway** | Cross-domain communication and command safety verification | Command Dispatch / State Sync / Priority Arbitration | Cross-domain |
| **Infrastructure Layer** | Heterogeneous device integration and real-time control execution | SCADA / IEC 61850 / IEC 104 / MQTT / Modbus / OPC UA | Hard Execution + Soft Base |

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
