**[中文](README.md)** | English

---

<div align="center">

# EnerOS

### Power/Energy-Native AgentOS

**Converge Energy at the Hub, Drive Intelligence in All Things**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

<table>
<tr><td>

**AI Agent technology is reshaping industries at an unprecedented pace, yet the power and energy domain faces unique challenges.**

General-purpose Agent frameworks lack native understanding of power system physics; safety constraints are demoted to prompt-level suggestions; grid topology and electrical coupling are overlooked; protocol and model heterogeneity across devices makes unified dispatch intractable. These fundamental gaps render "bolt-on" approaches to power knowledge inherently unsafe and inefficient.

**EnerOS** is a native intelligent agent operating system (AgentOS) designed for the power and energy domain. It embeds domain knowledge, physical constraints, and operational logic of power systems into the OS kernel, enabling AI Agents with native understanding, safe decision-making, and autonomous action in energy scenarios. Just as a traditional OS provides unified abstractions of processes, files, and networking for applications, EnerOS provides unified abstractions of topology, power flow, constraints, and equipment for energy agents — **making Agents natively "understand electricity"**.

</td></tr>
</table>

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
Power systems have rigid real-time requirements. EnerOS adopts a dual-execution architecture: a General Domain for Agent orchestration and AI inference, and a Real-Time Domain for deterministic latency in protection logic and breaker operations. The safety domain cannot be blocked by the General Domain.

### Open & Interoperable
Standardized Agent communication protocols and device integration specifications enable plug-and-play for heterogeneous energy devices and multi-vendor systems.

---

## Architecture

### Dual-Execution Architecture: General Domain + Real-Time Domain

Power systems have rigid real-time requirements — relay protection must act within milliseconds, breaker commands must be issued within deterministic deadlines. General-purpose OS kernels cannot provide hard real-time guarantees, while pure real-time systems struggle to support complex workloads like AI inference and Agent orchestration.

EnerOS adopts a **dual-execution architecture**, dividing the system into two execution domains:

```
┌─────────────────────────────────────────────────────────────────┐
│                  General Domain                                  │
│                                                                   │
│  Agent Runtime · AI Inference · Planning & Optimization · HMI    │
│  Non-deterministic tasks · Latency: seconds ~ minutes            │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              RT Safety Gateway                               │ │
│  │    Cross-domain Comm · Command Dispatch · State Sync         │ │
│  │    Priority Arbitration · Constraint Verification            │ │
│  └────────────────────────┬────────────────────────────────────┘ │
│                           │ Inter-Domain Comm                    │
├───────────────────────────┼─────────────────────────────────────┤
│                  Real-Time Domain                                 │
│                                                                   │
│  Relay Protection · Breaker Operations · Fault Isolation          │
│  Frequency Regulation · Deterministic tasks                       │
│  Latency: microseconds ~ milliseconds                             │
│                                                                   │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────────┐ │
│  │  RT Scheduler │ │  Interrupt   │ │  I/O Polling Engine      │ │
│  │ (Priority     │ │  Handler     │ │ (SCADA / IEC 104 / GOOSE)│ │
│  │  Preemption)  │ │              │ │                           │ │
│  └──────────────┘ └──────────────┘ └──────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

| Execution Domain | Kernel Mode | Scheduling | Typical Tasks | Latency |
|------------------|-------------|------------|---------------|---------|
| **General Domain** | Standard Kernel | Fair Scheduling | Agent orchestration, AI inference, power flow, planning | Seconds ~ Minutes |
| **Real-Time Domain** | Real-Time Extended Kernel | Priority Preemption | Relay protection, breaker ops, fault isolation, frequency regulation | Microseconds ~ Milliseconds |

**Core Design Principles:**

- **Safety domain cannot be blocked by General Domain** — Real-Time Domain tasks have the highest priority; no General Domain operation may affect real-time execution determinism
- **Unidirectional trust** — The Real-Time Domain can directly read General Domain decisions, but the General Domain cannot directly intervene in Real-Time Domain scheduling
- **Cross-domain communication via RT Safety Gateway** — All General Domain → Real-Time Domain commands must pass through the gateway's constraint verification and priority arbitration
- **Graceful degradation** — When the General Domain fails, the Real-Time Domain automatically switches to local protection logic, ensuring grid safety does not depend on AI

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
│  │              Real-Time Domain                               │ │
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
| **Application Layer** | Business-scenario-oriented agent applications | Dispatch / Operation / Planning / Trading Agent | General Domain |
| **Agent Runtime Layer** | Agent lifecycle management and intelligent scheduling | Lifecycle / Memory / Tool / Reasoning / Security Guard | General Domain |
| **Power-Native Kernel** | Power system physical world modeling and constraint enforcement | Topology / PowerFlow / Constraint / Equipment / TimeSeries / Event | General Domain |
| **RT Safety Gateway** | Cross-domain communication and command safety verification | Command Dispatch / State Sync / Priority Arbitration | Cross-domain |
| **Infrastructure Layer** | Heterogeneous device integration and real-time control execution | SCADA / IEC 61850 / IEC 104 / MQTT / Modbus / OPC UA | RT Domain + General Domain |

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

## Crate Index

| Crate | Path | Responsibility | Key Types / Interfaces |
|-------|------|----------------|------------------------|
| **eneros-core** | `crates/eneros-core/` | Unified types, errors, and configuration | `EnerOSError`, `EnerOSConfig`, `ElementId`, `BusType`, `PowerSystemState` |
| **eneros-topology** | `crates/eneros-topology/` | Power grid topology graph modeling and analysis | `NetworkGraph`, `TopologyEngine`, `TopologySearcher`, `Bus`, `Branch`, `Switch` |
| **eneros-powerflow** | `crates/eneros-powerflow/` | Newton-Raphson power flow solving | `PowerFlowSolver`, `YBusMatrix`, `JacobianMatrix`, `PowerFlowResult` |
| **eneros-constraint** | `crates/eneros-constraint/` | Safety constraint verification and enforcement | `ConstraintEngine`, `Constraint`, `ConstraintType`, `Violation`, `ResponseStrategy` |
| **eneros-equipment** | `crates/eneros-equipment/` | Equipment parameter model library | `EquipmentModel` trait, `EquipmentLibrary`, `TransmissionLine`, `TwoWindingTransformer` |
| **eneros-timeseries** | `crates/eneros-timeseries/` | Time-series data storage and query | `TimeSeriesEngine`, `TimeSeriesStorage` trait, `TimeSeriesQuery`, `Aggregation` |
| **eneros-eventbus** | `crates/eneros-eventbus/` | Event-driven communication bus | `EventBus`, `Event`, `EventType`, `EventHandler` trait, `CallbackHandler` |
| **eneros-gateway** | `crates/eneros-gateway/` | Safety gateway and command control | `SafetyGateway`, `Command`, `SafetyCheck` trait, `CommandPriority` |
| **eneros-device** | `crates/eneros-device/` | Device communication and protocol adaptation | `ProtocolAdapter` trait, `DeviceManager`, `DeviceDiscovery`, `HealthMonitor` |
| **eneros-api** | `crates/eneros-api/` | CLI / HTTP API service | `ApiServer`, `ApiClient`, `ApiResponse` |
| **eneros-bridge** | `crates/eneros-bridge/` | Python bridge for cnpower / pandapower | `PythonBridge`, `CnpowerEquipmentLoader` |
| **eneros-network** | `crates/eneros-network/` | Unified topology-to-powerflow pipeline | `PowerNetwork`, `NetworkSimulatorAdapter` |

### Dependency Relationships

```
eneros-core <-- eneros-topology
             <-- eneros-powerflow
             <-- eneros-constraint
             <-- eneros-equipment
             <-- eneros-timeseries
             <-- eneros-eventbus
             <-- eneros-gateway
             <-- eneros-api

eneros-core + eneros-eventbus <-- eneros-device
eneros-core + eneros-equipment <-- eneros-bridge
eneros-core + eneros-topology + eneros-powerflow + eneros-equipment <-- eneros-network
```

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

## Quick Start

### Prerequisites

- Rust 1.70+ installed via [rustup](https://rustup.rs/)
- Cargo

### Build

```bash
# Clone the repository
git clone https://github.com/Gawg-AI/EnerOS.git
cd EnerOS

# Build the project
cargo build --release

# Run tests
cargo test
```

### Run

```bash
# Start the API server
cargo run --bin eneros -- serve --host 0.0.0.0 --port 8080

# Run power flow calculation
cargo run --bin eneros -- power-flow --case ieee14
```

---

## Roadmap

- [x] **Phase 1 — Kernel Foundation** — Topology engine, power flow computation kernel, equipment model store
- [x] **Phase 2 — Agent Runtime** — Agent lifecycle management, memory system, tool engine
- [x] **Phase 3 — Grid-Aware Context** — Topology-aware injection, constraint verification guard, event bus
- [x] **Phase 4 — Multi-Agent Coordination** — Multi-agent collaboration protocol and topology-structured communication
- [x] **Phase 5 — Infrastructure Adapters** — SCADA / IEC 61850 / IEC 104 / MQTT protocol adapters
- [x] **Phase 6 — Domain Applications** — Dispatch Agent (economic dispatch / AGC), Operation Agent (fault diagnosis / equipment health), Self-Healing Agent (fault isolation / network reconfiguration), and domain collaboration protocol
- [x] **Phase 7 — Real-Time Closed Loop and System Integration** — SCADA data pipeline, DC-OPF / state estimation / short-circuit analysis, load forecasting / planning / trading Agents, axum API + WebSocket + web dashboard
- [x] **Phase 8 — Deep Integration and Productionization** — End-to-end component connectivity, TOML configuration loading, E2E integration tests, dashboard integration, real HTTP ApiClient, SQLite persistence
- [x] **Phase 9 — Real Bug Fixes and Shell Removal** — `await_holding_lock` deadlock fix, SelfHealingAgent interlocking validation, Y-bus calculation bug fix, message broadcast fix, duplicate-code removal, zero clippy warnings
- [x] **Phase 10 — Accuracy Verification and LLM Reasoning Integration** — IEEE 14-bus benchmark accuracy verification, LlmReasoningEngine (OpenAI / Ollama / vLLM compatible), Agent LLM reasoning enhancements (OperationAgent fault diagnosis + DispatchAgent dispatch review), fallback mechanism
- [x] **Phase 11 — Real rig Tools and Unified Reasoning Engine** — rig framework integration (`rig-core` 0.38), four real power-system tools (PowerFlow / ConstraintCheck / N1Analysis / VoltageStability), unified RigReasoningEngine, deprecated LlmReasoningEngine marker, feature-flag isolation
- [x] **Phase 12 — Real-Time Execution Domain** — PriorityCommandQueue, RealtimeExecutor, SafetyGateway priority-queue integration, PriorityEventBus dual-channel event bus, DualScanGroup fast / slow scan groups (100ms / 1s), WatchdogTimer timeout protection
- [x] **Phase 13 — Constraint-Driven Deterministic Decision Pipeline** — StructuredActionOutput, FeasibilityProjector (What-If analysis + boundary clipping), three-stage ConstrainedDecisionPipeline (projection -> validation -> execution), ActionDispatcher `dispatch_structured()` integration, ConstraintAwareValidator projector support, FeedbackLoop LLM re-reasoning, NetworkSimulatorAdapter
- [x] **Phase 14 — Deterministic Decision Closed Loop Wiring** — Fixed the Phase 13 "ghost loop" (structured action parsing + ActionMapper priority consumption + Orchestrator routing to `dispatch_structured`), changed FeedbackLoop to `Arc<dyn>` and wired it into the orchestrator (rejection -> re-reasoning), added `AgentAction::ExecuteStructured`, injected FeedbackLoop into the API, added five end-to-end closed-loop integration tests, fixed flaky eneros-bridge tests

---

## Contributing

EnerOS is in its early design stage. Contributors interested in the intersection of power systems and AI are welcome to join the discussion and co-build.

---

## License

[MIT](LICENSE)

---

## Acknowledgements

- [pandapower](https://github.com/e2nIEE/pandapower) - power system analysis
- [PowerGridModel](https://github.com/PowerGridModel/power-grid-model) - high-performance power flow calculation
- [libIEC61850](https://github.com/mz-automation/libiec61850) - IEC 61850 protocol implementation
