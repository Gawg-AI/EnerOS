中文 | **[English](README_en.md)**

---

<div align="center">

# EnerOS（能枢OS）

### 能枢 — 电力/能源原生的 AgentOS

**聚能以枢，驱动万物智能**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![codecov](https://codecov.io/gh/Gawg-AI/EnerOS/branch/main/graph/badge.svg)](https://codecov.io/gh/Gawg-AI/EnerOS)
[![Security](https://img.shields.io/badge/Security-OWASP%20Top%2010-green.svg)](tests/security/)
[![IEC 62443](https://img.shields.io/badge/IEC%2062434-SL1%20Ready-blue.svg)](docs/compliance/)
[![Conformance](https://img.shields.io/badge/Protocol-Conformance%20IEC%2061850%2FModbus%2F104-orange.svg)](tests/protocol_conformance/)
[![Benchmarks](https://img.shields.io/badge/Performance-criterion%20tracked-success.svg)](benches/)

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

### 实时双执行域
通用执行域承载 Agent 编排与 AI 推理，实时执行域保障保护逻辑与开关操作的确定性时延。采用 SCHED_FIFO 优先级抢占调度 + CPU 隔离（isolcpus）+ mlockall 内存锁定 + 无锁 SPSC IPC + 硬件看门狗（/dev/watchdog），命令时延 P99 < 1ms。安全域不可被通用域阻塞。

---

## 项目结构

EnerOS 采用 Cargo workspace 组织，所有可编译的 Rust crate 统一放在 `crates/` 下；OS 镜像构建基础设施（非 Rust）放在 `os/` 下。两者职责不同，不可合并。

```
eneros/
├── crates/                    # Rust workspace（所有可编译 crate）
│   ├── eneros-core/           #   统一类型、错误、配置
│   ├── eneros-os/             #   OS 服务 crate（源代码）：init/rt/agentos/hal/netcfg/firewall/devmgr
│   ├── eneros-gateway/        #   安全网关、决策管线、实时执行器
│   ├── eneros-agent/          #   Agent 运行时与 7 种领域 Agent
│   ├── eneros-powerflow/      #   潮流求解
│   ├── eneros-topology/       #   电网拓扑图建模
│   ├── ...                    #   其余 14 个 crate（见下方 Crate 索引）
│   └── eneros-dashboard/      #   Web 仪表盘
├── os/                        # OS 镜像构建基础设施（脚本 + 配置，非 Rust）
│   ├── boot/                  #   启动配置（grub.cfg、initramfs、启动参数验证）
│   ├── kernel/                #   内核配置（config-x86_64、config-aarch64、构建脚本）
│   ├── rootfs/                #   根文件系统（build.sh + /etc/eneros/*.toml 配置文件）
│   ├── image-builder/         #   镜像构建（分区、引导装载程序安装）
│   └── tests/                 #   OS 集成测试 crate（boot_test、boot_params_test）
├── docs/                      # 项目文档
├── .trae/specs/agentos-native/  # 架构蓝图（spec.md、tasks.md、checklist.md）
├── Cargo.toml                 # workspace 根配置
├── CHANGELOG.md               # 变更日志
└── ROADMAP.md                 # 开发路线图
```

### `crates/eneros-os/` 与 `os/` 的区别

| | `crates/eneros-os/` | `os/` |
|---|---|---|
| **本质** | Rust crate（编译为库 + 二进制） | 镜像构建基础设施（shell 脚本 + 配置文件） |
| **产物** | `eneros-init`（PID 1）、`enerosctl`（CLI）、`libeneros_os.rlib` | 可启动的 OS 镜像（rootfs tarball、initramfs、磁盘镜像） |
| **内容** | `src/init/`、`src/rt/`、`src/agentos/`、`src/hal/`、`src/netcfg/` 等 | `boot/`、`kernel/`、`rootfs/`、`image-builder/` |
| **关系** | 被 `os/rootfs/build.sh` 编译后拷入 rootfs | 调用 `cargo build -p eneros-init` 编译 crate 产物 |

简言之：`crates/eneros-os/` 是 **OS 服务的源代码**，`os/` 是 **把源代码打包成可启动镜像的构建系统**。

---

## Crate 索引

| Crate | 路径 | 职责 | 关键类型/接口 |
|-------|------|------|---------------|
| **eneros-core** | `crates/eneros-core/` | 统一类型、错误、配置 | `EnerOSError`, `EnerOSConfig`, `ElementId`, `BusType`, `PowerSystemState` |
| **eneros-linalg** | `crates/eneros-linalg/` | 稀疏线性代数（CSR 矩阵、LU/Cholesky 分解） | `SparseMatrix`, `SparseLuFactorization`, `SymbolicFactorization` |
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
| **eneros-os** | `crates/eneros-os/` | OS 服务层（init/rt/agentos/hal/netcfg/firewall/devmgr） | `eneros-init`（PID 1）, `enerosctl`（CLI）, `NetworkConfig`, `FirewallManager`, `RtRuntime`, `HardwareWatchdog`, `AgentRegistry` |

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
             ◄── eneros-os

eneros-core + eneros-eventbus ◄── eneros-device
eneros-core + eneros-equipment ◄── eneros-bridge
eneros-core + eneros-topology + eneros-powerflow + eneros-equipment ◄── eneros-network
eneros-core + eneros-device ◄── eneros-scada
eneros-powerflow + eneros-equipment ◄── eneros-analysis
eneros-core + eneros-os ◄── eneros-gateway
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
cargo run --bin eneros -- run --host 0.0.0.0 --port 8080

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

### 已完成

- [x] **v0.3.0 — 生产就绪基线** — 持久化全面接入、配置体系、可观测性（Prometheus/结构化日志）、安全加固（JWT/mTLS）
- [x] **v0.4.0 — 打通生产路径** — 设备层接线、SCADA 实时管道、网络模型配置化加载
- [x] **v0.5.0 — Agent 自主化** — spawn 生命周期、行为规划、反思学习、工具统一
- [x] **v0.6.0 — 生产加固** — 配置化、可观测性、安全、API 覆盖、回滚执行
- [x] **v0.7.0 — 协议覆盖** — GOOSE/SV/OPC UA/DNP3、IEC104/61850 增强、CIM 导入、TLS 运行时
- [x] **v0.8.0 — 分析精度进阶** — 稀疏线性代数、AC-OPF、暂态稳定、状态估计增强、不对称短路、开关物理建模、5 个新 API 端点
- [x] **v0.9.0 — 交付级运维** — 容器化、配置热重载、分布式追踪、DualScanGroup 修复、CI/CD
- [x] **v0.10.0 — 生产深化** — PipelineStatistics 原子化、per-device 锁池、SOE 事件记录、存储级降采样、CIM 转换器、OpenAPI 文档、SVG data-* 修复
- [x] **v0.14.0 — AgentOS 内核 + EventBus Broker** — AgentRegistry/AgentSupervisor/AgentIPC/EventBusBroker/AuthorityEnforcer/ResourceQuota/AgentScheduler/enerosctl
- [x] **v0.15.0 — Agent 进程化（激进迁移）** — 7 种 Agent 拆为独立进程 + AgentContext 重构 + ActionDispatcher IPC 化
- [x] **v0.16.0 — Gateway 进程化** — SafetyGateway/DecisionPipeline 独立进程 + GatewayClient + 端到端测试
- [x] **v0.18.0 — 实时双执行域** — eneros-rt 接线（SCHED_FIFO + CPU 隔离 + mlockall + huge pages）+ 无锁 SPSC IPC + 硬件看门狗 + 内核启动参数验证 + RT 基准测试
- [x] **v0.19.0 — 网络配置服务** — netcfg 静态IP/VLAN/网桥 + nftables 防火墙 + bonding（active-backup/LACP）+ 网络命名空间隔离 + DNS + uevent 热插拔 + enerosctl network 子命令
- [x] **v0.20.0 — 时间同步与日志** — PTP IEEE 1588 + NTP 回退 + PHC 管理 + syslog 结构化 JSON 日志 + 轮转/压缩/远程转发 + 审计日志 HMAC 签名 + enerosctl log 子命令
- [x] **v0.20.1 — v0.20.0 安全与正确性修复** — 审计签名绕过/空密钥/seq持久化 + PTP孤儿进程/ptp4l参数/NTP校验/Duration panic + 日志轮转/TLS明文/RFC5424转义 + CLI路径遍历/grep注入防护
- [x] **v0.20.2 — v0.20.0 功能完整性修复** — timesync 新增 eneros-timesync 守护进程（后台循环/PTP pmc 轮询/phc2sys -w/Drop trait/NTP 重试）+ syslog 线程安全（Mutex/BufWriter/fsync/TLS fail-fast/轮转修复）+ audit 链式哈希（prev_hash/轮转/fsync/查询过滤/签名修复）+ enerosctl audit/time 子命令 + log level 真正生效
- [x] **v0.21.0 — 设备管理与 HAL** — devmgr uevent 热插拔 + termios 串口 + USB/GPIO/I2C/SPI 设备接口 + enerosctl device 子命令
- [x] **v0.22.0 — 部署与 OTA 更新** — A/B 分区原子更新 + Ed25519 签名验证 + eneros-imager v2 五分区布局 + eneros-installer 交互式安装器 + enerosctl update 子命令 + 启动成功检测与自动回滚
- [x] **v0.23.0 — 电力协议原生支持** — AF_PACKET 原始套接字 transport（GOOSE/SV Layer 2 直采）+ IEC 104 FT 1.2 串口模式 + Modbus RTU 串口模式 + 协议时间戳（SO_TIMESTAMPNS + PTP 对齐）+ PRP/HSR 冗余框架 + enerosctl protocol 子命令
- [x] **v0.23.0 交付级修复（Delivery-Grade Hardening）** — AF_PACKET 改用内核 EtherType 过滤（`htons(ethertype)` 替代 `ETH_P_ALL`）+ cmsg_len/MSG_TRUNC 安全校验 + FT 1.2 帧区分算法重写（先试变长帧再回退固定帧，去除字符间超时依赖）+ Modbus Float32/Int32 IEEE 754 双寄存器写入（功能码 0x10）+ GOOSE BIT STRING 越界防护 + GooseTransport trait 改 `&self` 消除锁阻塞 + SV 多 ASDU 回调遍历 + timestamp 负值/溢出防护 + PTP 偏移过期检查（`is_stale`）+ PRP/HSR 序列号窗口回绕算法 + RCT 标准兼容（LSDU_size 字段）+ HSR Tag path 高 2 位编码 + enerosctl `receive()` 编译修复 + `for_goose`/`for_sv` 构造器 + 超时非零退出码 + IPv6 解析
- [x] **v0.24.0 — 安全加固** — UEFI Secure Boot（PK/KEK/db/dbx 变量管理 + Ed25519 签名验证 + `secure-boot.sh` 5 命令脚本）+ 内核加固（CONFIG_SECURITY_DMESG_RESTRICT + page_alloc.shuffle/slab_nomerge/init_on_alloc/init_on_free 命令行参数）+ seccomp 4 级 profile（Observer/Operator/Supervisor/Emergency，libseccomp BPF）+ 审计系统增强（HMAC-SHA256 签名 + 链式哈希 + 远程转发 + 365 天保留）+ 密钥管理服务（AES-256-GCM + Argon2id + Ed25519/Aes256/HmacSha256 + 访问控制 + 备份恢复）+ enerosctl security 子命令（status/keys/audit）
- [x] **v0.25.0 — 高可用基础** — 双节点心跳（UDP 多播 100ms 间隔 + 300ms 故障检测 + Alive/Suspect/Dead 状态机 + 主备角色优先级 + 双网卡冗余）+ HA 配置管理（HaConfig/SyncScope）+ 共享状态存储（应用级复制引擎 + 冲突检测/解决 PrimaryWins/TimestampWins/VersionWins + 配额管理）+ 状态同步/Fencing 框架占位 + enerosctl ha 子命令（status/nodes/sync-status/failover）
- [x] **v0.25.1 — HA 基础加固修复** — 修复 v0.25.0 HA 模块的 11 个 CRITICAL + 21 个 HIGH 缺陷：HaConfig 配置语义校验 + 心跳包 HMAC-SHA256 认证 + 双网卡冗余实现 + SyncManager 长连接/读取缓冲区/SharedStore 集成 + SharedStore role 可变/replicate 配额/delete 复制/O(1) 配额 + Fencing 自 fencing 防护/速率限制/多节点校验/历史持久化 + RwLock 中毒安全 + CLI 桩实现标注/failover 确认提示
- [x] **v0.26.0 — 高可用切换** — eneros-ha 守护进程（TCP IPC 127.0.0.1:5402）+ FailoverEngine 5 状态状态机 + VIP 漂移（ip addr + arping）+ 服务降级（is_readonly）+ 自动故障恢复（增量同步）+ 多节点集群（ClusterManager + Quorum 多数派 + witness 仲裁）+ 灾备演练（DrillScheduler 3 场景）+ SharedStore 持久化（JSON 快照 + WAL）+ enerosctl failover 子命令
- [x] **v0.27.0 — 插件系统** — eneros-plugin crate（独立框架核心）+ libloading 动态库加载 + C ABI 入口函数（eneros_plugin_create/destroy/metadata）+ Ed25519 签名验证 + seccomp 沙箱 + cgroups v2 资源配额 + catch_unwind 崩溃隔离 + ProtocolPlugin/AgentPlugin/AnalysisPlugin 三类插件接口 + ProtocolType::Custom 扩展 + Agent 权限上限 Operator + Kahn 拓扑排序依赖解析 + enerosctl plugin 子命令（9 个）+ 3 个示例插件（IEC 103/负荷均衡/可靠性分析）
- [x] **v0.28.0 — 开发者工具** — eneros-sdk crate（Agent/协议/插件开发 SDK，feature 门控）+ eneros-simulator crate（场景脚本引擎 + 电网/设备/故障/负荷四类模拟器，34 测试）+ eneros-plugin-macros crate（`#[eneros_plugin]` 过程宏自动生成 C ABI 入口）+ plugin-daemon 独立守护进程（IPC JSON 行协议 + 崩溃隔离 + Unix socket/TCP 双传输）+ PluginDaemonClient IPC 客户端 + LoadMode 双模式加载（Daemon 默认/Inline 向后兼容）+ PluginMarketClient 插件市场基础（搜索/下载/LRU 缓存）+ enerosctl 交互式 shell（rustyline REPL + Tab 补全 + 命令历史）+ enerosctl config/service/doctor/simulator 子命令 + plugin 命令 IPC 化 + clap_complete 补全脚本生成 + 完整文档体系（CONTRIBUTING + developer-guide + ADR 0001-0004 + user-manual + plugin-development + deployment 增强）+ API 文档完善（simulator validate 端点 + plugin_market tags）
- [x] **v0.28.1 — 开发者工具加固修复** — 7 CRITICAL + 23 HIGH + 60 MEDIUM + 44 LOW 修复：FFI `OnceLock<CString>` 零泄漏 + JSON RFC 8259 控制字符转义 + 潮流 `base_mva` per-unit 转换 + 分支功率真实 ID 映射 + Slack 母线校验 + NaN `is_finite()` 拒绝 + `plugin_op_lock` 串行化 + IPC `connect_timeout` + 路径遍历 `validate_config_file_name()` + `env!("CARGO_PKG_VERSION")` 编译期版本嵌入 + serde `snake_case` 场景动作 + vtable 死代码消除

- [x] **v0.29.0 — 技术债务清偿与架构加固** — 25 项任务全部完成：`eneros-runtime` 架构重构 + dev-deps 循环消除 + 拓扑依赖反转 + TraceLayer/JSON日志/trace_id贯穿/TLS/OTLP 可观测性 + Agent控制API/校验合规规划WhatIf审计API/SSE Dashboard + WatchdogTimer + 热点路径p99<10ms + Gorilla压缩 + 连接池 + 决策缓存 + CI/CD + TDengine/InfluxDB后端 + FencingManager Quorum + 成员变更回调 + bincode批量同步 + SharedMemoryChannel(mmap+eventfd)

- [x] **v0.30.0 — 生态成熟与质量保障** — 8 项任务全部完成：IEC 62443-4-1/4-2 安全认证文档（SL1 91% / SL2 66% 符合性矩阵）+ OWASP Top 10 安全合规测试套件（97 测试，cargo audit + SAST 自定义规则）+ 端到端测试框架（TestCluster 本地进程组模式，6 场景 12 测试）+ 混沌工程（5 类注入器：网络/磁盘/CPU/内存/进程，4 场景）+ 电力协议一致性测试（IEC 61850 MMS/GOOSE/SV + Modbus TCP/RTU + IEC 104，174 测试通过）+ 性能基准体系（criterion 5 基准：SCADA/Agent/HA/API/PowerFlow，p50/p95/p99，回归 > 10% CI 失败）+ 测试覆盖率补充（114 新测试覆盖 powerflow/agent/ha）+ 集成验证发布（0 错误/0 警告/3500+ 测试）

### 规划中（v0.31.0-v0.50.0）

> 完整 22 版本蓝图见 [ROADMAP.md](ROADMAP.md)，任务分解见 [.trae/specs/roadmap-v029-to-v050/tasks.md](.trae/specs/roadmap-v029-to-v050/tasks.md)。

| 版本 | 主题 | 核心交付 |
|------|------|----------|
| v0.29.0 | 技术债务清偿与架构加固 | 42 项推迟项修复 + `eneros-runtime` 架构重构 + 性能优化（p99 < 10ms）+ TDengine/InfluxDB + OTLP + FencingManager Quorum + SharedMemoryChannel + Gorilla 压缩 + 连接池 + 决策缓存 + SSE Dashboard（25/25 任务完成，3115 测试通过）|
| v0.30.0 | 生态成熟与质量保障 | IEC 62443 认证 + 安全合规测试 + e2e 测试 + 混沌工程 + 协议一致性 + 性能基准 + 覆盖率 > 80%（8/8 任务完成，3500+ 测试通过）|
| v0.31.0 | 数字孪生引擎 | TwinModel 实时镜像 + What-If 推演 + 历史回放 + 虚拟传感器 |
| v0.32.0 | 高级 API 与数据平台 | GraphQL + API 版本管理 + 限流配额 + 时序查询 + 数据导出 |
| v0.33.0 | AI/ML 集成基础 | LLM 集成 + 异常检测 + 预测性维护 + 嵌入模型 + Agent LLM 增强 |
| v0.34.0 | 边缘计算与云边协同 | 边缘 Agent + 云边 gRPC + 模型分发 + 边缘推理 + 边缘自治 |
| v0.35.0 | 电力协议扩展 | IEC 60870-5-103 + CDT + R-GOOSE + PMU/PDC + ICCP + 协议网关 |
| v0.36.0 | 高级电网分析 | 预想事故分析 + 动态安全评估 + 新能源 + 储能 + 微电网 |
| v0.37.0 | 插件生态与市场 | 插件市场 + 进程隔离 + 第三方 SDK + 插件 CI/CD + 依赖管理 |
| v0.38.0 | 多租户与隔离 | 租户模型 + 命名空间隔离 + 租户配额 + 租户感知 API |
| v0.39.0 | 安全增强与零信任 | 零信任 mTLS + 威胁检测 + 安全自动化 + NERC CIP + 审计取证 |
| v0.40.0 | 生产级运维自动化 | SLO/SLI + 自动扩缩容 + 事件响应 + 容量规划 + AIOps |
| v0.41.0 | 高可用增强 | 多区域 HA + 灾难恢复 + 零停机升级 + 地理冗余多活 |
| v0.42.0 | 性能极致优化 | 亚毫秒延迟 + 100 万点/秒 SCADA + 内存优化 + 共享内存 IPC + 内核旁路 |
| v0.43.0 | Agent 智能进阶 | 多 Agent 协作 + 强化学习 + 任务分解 + 策略市场 + 可解释性 |
| v0.44.0 | 电网分析进阶 | 动态状态估计 + 实时稳定 + EMTP 电磁暂态 + 谐波分析 + 电网等值 |
| v0.45.0 | 物联网与泛在接入 | MQTT + CoAP + LwM2M + 传感器网络 + 边缘设备管理 |
| v0.46.0 | 可视化与交互增强 | 高级仪表盘 + 3D 可视化 + 自然语言接口 + 移动端 + 报表 |
| v0.47.0 | 国际化与合规 | 多语言 i18n + GDPR/PIPL/CCPA + 区域协议 + 本地化 |
| v0.48.0 | 数据治理与隐私 | 数据治理 + 差分隐私 + 数据血缘 + 数据脱敏 + 字段级加密 |
| v0.49.0 | 1.0 候选准备 | 功能冻结 + 全面测试 + 文档完善 + 性能验证 + 安全审计 |
| v0.50.0 | 1.0 Release Candidate | 最终加固 + 生产就绪 + IEC 62443 SL2 认证 + v1.0.0-rc.1 发布 |

### HA 高可用（v0.26.0）

EnerOS 提供双节点主备冗余和多节点集群高可用能力，故障切换 < 3s。

**核心组件**：
- `eneros-ha` 守护进程：独立二进制，运行 HeartbeatManager + SyncManager + SharedStore + FencingManager + FailoverEngine
- `FailoverEngine` 状态机：Standby → TakingOver → Active → FailingBack → Failed
- VIP 漂移：Linux 下通过 `ip addr add/del` + `arping -U` 实现 IP 接管
- 服务降级模式：备节点 `is_readonly = true` 防止双主冲突
- 自动故障恢复：原主节点恢复后增量同步，按策略（AutoPreferPrimary/Manual）回切
- 多节点集群：ClusterManager + Quorum 多数派仲裁 + witness 仲裁节点
- 灾备演练：DrillScheduler 支持 PrimaryDown/NetworkPartition/DiskFailure 场景
- 持久化：JSON 快照 + WAL 追加日志，ha-daemon 重启后状态恢复

**CLI 控制**（`enerosctl ha` 子命令）：
- `enerosctl ha status` — 查询 HA 整体状态
- `enerosctl ha nodes` — 查询集群成员列表
- `enerosctl ha sync-status` — 查询同步统计
- `enerosctl ha failover-status` — 查询 failover 状态机
- `enerosctl ha failover-trigger --force` — 手动触发切换
- `enerosctl ha failover-history` — 查询切换历史
- `enerosctl ha failover-drill --scenario primary_down` — 触发灾备演练

**配置**：`/etc/eneros/ha.toml` 包含 [heartbeat]/[sync]/[fencing]/[failover]/[cluster]/[drill] 配置段

**已知限制**：
- IP 接管/释放为 Linux only
- FencingManager::fence 的 Quorum 校验推迟到 v0.27.0
- 集群成员变更通知回调推迟到 v0.27.0
- 二进制序列化、批量同步性能优化推迟到 v0.27.0

### 插件系统（v0.27.0）

EnerOS v0.27.0 引入完整的插件框架，支持第三方协议适配器、Agent 策略、分析模块以动态库形式接入系统，通过 Ed25519 签名验证与 seccomp 沙箱保障安全隔离。

**核心组件**：
- `eneros-plugin` crate：插件框架核心，独立于 eneros-device/eneros-agent/eneros-analysis，通过镜像类型避免循环依赖
- `PluginLoader`：基于 libloading 0.8 的动态库加载器，支持 .so/.dll/.dylib 跨平台
- `PluginSignatureVerifier`：Ed25519 签名验证（复用 v0.22.0 OTA 签名基础设施）
- `PluginSandbox`：seccomp BPF 沙箱 + cgroups v2 资源配额 + catch_unwind 崩溃隔离
- `PluginRegistry`：线程安全注册表（RwLock<HashMap>）+ Kahn 拓扑排序依赖解析
- `PluginState` 状态机：Loaded → Initialized → Starting → Running → Stopping → Stopped / Crashed / Failed

**三类插件接口**：
- `ProtocolPlugin`：协议适配器插件，`ProtocolType::Custom(String)` 支持第三方协议接入
- `AgentPlugin`：Agent 策略插件，权限上限 Operator（Emergency/Supervisor 强制降级）+ StrategyPriority 冲突解决
- `AnalysisPlugin`：分析模块插件，输入/输出使用 serde_json::Value 避免 ABI 不安全类型

**CLI 控制**（`enerosctl plugin` 子命令）：
- `enerosctl plugin list` — 列出插件目录中的已安装插件
- `enerosctl plugin load <path>` — 加载插件（验证签名 → 加载库 → 显示入口符号）
- `enerosctl plugin verify <path>` — 验证插件签名（不加载）
- `enerosctl plugin info <name>` — 显示插件详情（manifest + 元数据）
- `enerosctl plugin gen-keys --output <dir>` — 生成 Ed25519 签名密钥对
- `enerosctl plugin sign --plugin <path> --key <key>` — 对插件文件签名

**示例插件**（`crates/eneros-plugin/examples/`）：
- `iec103-plugin`：IEC 103 协议适配器示例
- `custom-strategy-agent`：基于规则的负荷均衡策略示例
- `reliability-analysis`：SAIFI/SAIDI/CAIDI 可靠性指标计算示例

**配置**：`/etc/eneros/plugin.toml` 包含 [plugin]/[quota]/[sandbox] 三段配置

**已知限制**：
- 插件进程隔离（独立进程 + IPC 通信）推迟到 v0.28.0，v0.27.0 采用同进程加载
- `#[eneros_plugin]` 过程宏推迟到 v0.28.0，v0.27.0 用 C ABI 入口函数替代
- plugin-daemon 独立守护进程推迟到 v0.28.0，v0.27.0 CLI 直接调用库
- seccomp/cgroups 仅 Linux 生效，非 Linux 平台返回 Unsupported

### 开发者工具（v0.28.0）

EnerOS v0.28.0 引入完整开发者工具链，支持快速构建、测试和部署 EnerOS 应用。

**Rust SDK**（`crates/eneros-sdk/`）：
- `AgentBuilder` 链式构造器 + `AgentSdk` 封装 IPC 客户端句柄
- `ProtocolAdapterBuilder` 协议适配器开发辅助
- `PluginBuilder` 生成 PluginManifest TOML + `#[eneros_plugin]` 宏 re-export
- feature 门控：`full`（默认）/`agent`/`protocol`/`plugin`

**统一模拟器框架**（`crates/eneros-simulator/`）：
- 场景脚本引擎：TOML 格式场景定义，7 种动作（InjectFault/ClearFault/LoadChange/GeneratorTrip/LineTrip/LoadShed/Observe）
- 电网模拟器：稳态潮流重求解，支持支路开断/发电机调整/负荷调节
- 设备模拟器：RTU/IED/保护装置行为仿真，IEC 104 + Modbus 协议响应
- 故障注入：5 种故障类型 + 5 个预置场景（N-1/N-2/级联/保护拒动/保护误动）
- 负荷曲线：4 季 × 3 区域类型典型曲线 + 光伏/风电新能源模型

**plugin-daemon 进程隔离**：
- 独立守护进程，插件在 daemon 进程内加载，IPC 通信实现崩溃隔离
- `LoadMode::Daemon`（v0.28.0 默认）/ `LoadMode::Inline`（v0.27.0 同进程，向后兼容）
- IPC 协议：JSON 行协议，Unix socket（Linux）/ TCP 127.0.0.1:5410（跨平台回退）

**`#[eneros_plugin]` 过程宏**（`crates/eneros-plugin-macros/`）：
- 标注 `impl Plugin` 的结构体，自动生成 `eneros_plugin_create`/`destroy`/`metadata`/`vtable` C ABI 入口
- 使用具体类型进行 FFI 指针转换，避免 fat pointer vtable 丢失问题

**插件市场基础**（`crates/eneros-plugin/src/market.rs`）：
- `PluginMarketClient` 远程仓库索引（TOML 清单）、搜索、下载、LRU 缓存淘汰

**enerosctl 全功能 CLI**：
- 交互式 shell：`enerosctl shell` 启动 REPL（`eneros> ` 提示符，Tab 补全，命令历史）
- 补全脚本：`enerosctl completions bash/zsh/fish/powershell`
- 配置管理：`enerosctl config get/set/edit/list`
- 服务管理：`enerosctl service start/stop/restart/status/list`
- 系统诊断：`enerosctl doctor`（内核/控制通道/状态文件/权限/依赖服务检查）
- 模拟器：`enerosctl simulator run/validate/list`
- plugin 命令 IPC 化：通过 PluginDaemonClient 调用 plugin-daemon

**完整文档体系**：
- [CONTRIBUTING.md](CONTRIBUTING.md) — 贡献指南
- [docs/developer-guide.md](docs/developer-guide.md) — 开发者指南
- [docs/adr/](docs/adr/) — 架构决策记录（0001-0004）
- [docs/user-manual.md](docs/user-manual.md) — 用户手册
- [docs/plugin-development.md](docs/plugin-development.md) — 插件开发指南
- [docs/deployment.md](docs/deployment.md) — 部署运维手册

### 规划中

- [ ] **v1.0.0 — 生态扩展** — GraphQL、数字孪生、文档体系

---

## 参与贡献

EnerOS 当前版本 v0.30.0（已发布，8/8 任务完成，3500+ 测试通过），43 个 crate。欢迎对电力系统与 AI 交叉领域感兴趣的贡献者参与。请阅读 [ROADMAP.md](ROADMAP.md) 了解规划方向，[CONTRIBUTING.md](CONTRIBUTING.md) 了解贡献流程。

---

## 许可证

[MIT](LICENSE)

---

## 致谢

- [pandapower](https://github.com/e2nIEE/pandapower) - 电力系统分析
- [PowerGridModel](https://github.com/PowerGridModel/power-grid-model) - 高性能潮流计算
- [libIEC61850](https://github.com/mz-automation/libiec61850) - IEC 61850 协议实现
