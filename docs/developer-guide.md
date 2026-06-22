# EnerOS 开发者指南

本指南面向 EnerOS 的核心开发者与贡献者，说明系统架构、workspace 组织方式，以及扩展系统能力（新增 Agent、协议适配器、插件）的标准流程。

## 1. 架构总览

EnerOS 定位为电力原生 Agent 操作系统（Power-Native AgentOS），采用双执行域 + 分层架构。

### 1.1 双执行架构

系统划分为两个执行域，通过实时安全网关通信：

- **通用执行域**：承载 Agent 编排、AI 推理、潮流计算、规划优化，响应时延秒级 ~ 分钟级，使用标准内核公平调度。
- **实时执行域**：承载继电保护、开关操作、故障隔离、频率调节，响应时延微秒级 ~ 毫秒级，使用优先级抢占调度。

核心原则：安全域不可被通用域阻塞；通用域→实时域的指令必须经过实时安全网关的约束校验与优先级仲裁；通用域异常时实时域自动降级到本地保护逻辑。

### 1.2 分层架构（L0-L3）

```
┌──────────────────────────────────────────────────────────────────┐
│ L3 应用层  eneros-api (HTTP/WS/CLI) · eneros-dashboard            │
├──────────────────────────────────────────────────────────────────┤
│ L3 编排层  eneros-agent (7种专业Agent+编排+调度+协作)              │
│            eneros-gateway (7阶段决策管线+安全网关+执行器)          │
├──────────────────────────────────────────────────────────────────┤
│ L2 能力层  eneros-reasoning · eneros-tool · eneros-memory         │
│            eneros-scada · eneros-device · eneros-constraint       │
│            eneros-analysis · eneros-network · eneros-bridge       │
│            eneros-plugin · eneros-simulator                       │
├──────────────────────────────────────────────────────────────────┤
│ L1 内核层  eneros-os (agentos/ha/init/security/syslog/timesync    │
│            /update/devmgr/hal/rt/netcfg) · eneros-eventbus        │
├──────────────────────────────────────────────────────────────────┤
│ L0 基础层  eneros-core · eneros-topology · eneros-linalg          │
│            eneros-powerflow · eneros-equipment · eneros-timeseries│
└──────────────────────────────────────────────────────────────────┘
```

| 层级 | 职责 | 代表 crate |
|------|------|-----------|
| **L0 基础层** | 领域类型、线性代数、潮流计算、拓扑、设备模型、时序存储 | eneros-core、eneros-linalg、eneros-powerflow、eneros-topology、eneros-equipment、eneros-timeseries |
| **L1 内核层** | OS 内核能力（Agent 调度、HA、安全、时间同步、OTA、设备管理、HAL） | eneros-os、eneros-eventbus |
| **L2 能力层** | 领域能力（推理、工具、记忆、SCADA、设备协议、约束、分析、网络、桥接、插件、仿真） | eneros-reasoning、eneros-tool、eneros-memory、eneros-scada、eneros-device、eneros-constraint、eneros-analysis、eneros-network、eneros-bridge、eneros-plugin、eneros-simulator |
| **L3 编排层** | Agent 编排与安全网关 | eneros-agent、eneros-gateway |
| **L3 应用层** | 对外接口（HTTP API、Dashboard） | eneros-api、eneros-dashboard |

### 1.3 crate 依赖方向

依赖严格自上而下，禁止反向依赖与循环依赖：

```
L0 基础层（无内部依赖或仅依赖 eneros-core）
  ↑
L1 内核层（依赖 L0）
  ↑
L2 能力层（依赖 L0、L1）
  ↑
L3 编排层（依赖 L0、L1、L2）
  ↑
L3 应用层（依赖全部下层）
```

`eneros-plugin` 独立于 eneros-device/eneros-agent/eneros-analysis，避免循环依赖。`eneros-sdk` 封装常用类型，供第三方开发者使用。

## 2. 开发环境

### 2.1 Rust 工具链

- **最低版本**：Rust 1.75+
- **edition**：2021
- **推荐组件**：rustfmt、clippy、rust-src（rust-analyzer 需要）

```bash
rustup component add rustfmt clippy rust-src
```

### 2.2 平台支持

| 平台 | 支持程度 | 说明 |
|------|----------|------|
| Linux x86_64 | 完整支持 | 生产部署目标平台 |
| Linux aarch64 | 完整支持 | 嵌入式/边缘部署 |
| Windows | 核心库开发 | eneros-os 的 HAL/seccomp/AF_PACKET 部分不编译，使用 `--exclude eneros-installer` |
| macOS | 核心库开发 | 同 Windows |

跨平台开发时编译核心库：

```bash
cargo build --workspace --exclude eneros-installer
```

### 2.3 IDE 与调试

推荐 VS Code + rust-analyzer。调试配置：

- `rust-analyzer.checkOnSave.command`: `clippy`
- `rust-analyzer.cargo.features`: `all`
- 断点调试：使用 `lldb` 扩展或 `CodeLLDB`

VS Code launch.json 示例（调试 eneros-api）：

```json
{
  "type": "lldb",
  "request": "launch",
  "name": "debug eneros-api",
  "cargo": {
    "args": ["build", "--package", "eneros-api"],
    "filter": { "kind": "bin" }
  },
  "args": ["run", "--config", "eneros.toml"],
  "cwd": "${workspaceFolder}"
}
```

## 3. Workspace 组织

EnerOS 使用 Cargo workspace 组织 36 个 crate，分为核心库、二进制、SDK/宏三类。

### 3.1 核心库（library crate）

| crate | 层级 | 职责 |
|-------|------|------|
| eneros-core | L0 | 领域类型、配置、错误、事件、命令 |
| eneros-linalg | L0 | 线性代数运算（矩阵分解、求解器） |
| eneros-powerflow | L0 | 潮流计算（牛顿-拉夫逊、前推回代） |
| eneros-topology | L0 | 电网拓扑图、搜索、连接模式分析 |
| eneros-equipment | L0 | 设备模型库（变压器、线路、开关等） |
| eneros-timeseries | L0 | 时序数据存储、聚合、降采样、异常检测 |
| eneros-os | L1 | OS 内核（agentos/ha/init/security/syslog/timesync/update/devmgr/hal/rt） |
| eneros-eventbus | L1 | 事件总线（broker/bus/client/priority_bus） |
| eneros-reasoning | L2 | 推理引擎（LLM 集成、策略、反馈） |
| eneros-tool | L2 | 工具注册与调用 |
| eneros-memory | L2 | Agent 记忆（文件存储、向量检索） |
| eneros-scada | L2 | SCADA 数据采集（IEC 104、双扫描、快照） |
| eneros-device | L2 | 设备协议适配器（IEC 104/61850/Modbus/DNP3/GOOSE/SV/OPC UA/MQTT） |
| eneros-constraint | L2 | 安全约束引擎（N-1、热稳定、电压限值） |
| eneros-analysis | L2 | 电力分析（OPF、状态估计、短路、暂稳） |
| eneros-network | L2 | 电网网络模型与仿真 pipeline |
| eneros-bridge | L2 | Python 桥接（pandapower/cnpower） |
| eneros-plugin | L2 | 插件框架（清单/生命周期/加载器/签名/沙箱/IPC） |
| eneros-simulator | L2 | 电网仿真器（场景脚本、故障注入） |
| eneros-agent | L3 | Agent 编排（7 种 Agent + 调度 + 协作 + 冲突解决） |
| eneros-gateway | L3 | 实时安全网关（7 阶段决策管线 + 执行器） |
| eneros-api | L3 | HTTP/WS API 服务 |
| eneros-dashboard | L3 | Web Dashboard 资产生成 |

### 3.2 二进制（binary crate）

| 二进制 | 所属 crate | 职责 |
|--------|-----------|------|
| enerosctl | eneros-os/bins/enerosctl | 管理 CLI |
| eneros-init | eneros-os/bins/eneros-init | 系统初始化守护进程 |
| eneros-ha | eneros-os/bins/eneros-ha | 高可用守护进程 |
| eneros-timesync | eneros-os/bins/eneros-timesync | 时间同步守护进程 |
| eneros-installer | eneros-os/bins/eneros-installer | 安装器 |
| broker | eneros-eventbus/bins/broker | 事件总线 broker |
| gateway | eneros-gateway/bins/gateway | 实时安全网关 |
| plugin-daemon | eneros-plugin/bins/plugin-daemon | 插件守护进程 |
| dispatch-agent | eneros-agent/bins/dispatch-agent | 调度 Agent |
| forecast-agent | eneros-agent/bins/forecast-agent | 预测 Agent |
| operation-agent | eneros-agent/bins/operation-agent | 操作 Agent |
| planning-agent | eneros-agent/bins/planning-agent | 规划 Agent |
| trading-agent | eneros-agent/bins/trading-agent | 交易 Agent |
| self-healing-agent | eneros-agent/bins/self-healing-agent | 自愈 Agent |

### 3.3 SDK 与宏

| crate | 职责 |
|-------|------|
| eneros-sdk | 第三方开发者 SDK，封装 Agent/协议/插件开发的常用类型 |
| eneros-plugin-macros | `#[eneros_plugin]` 过程宏，自动生成 C ABI 入口函数 |

## 4. 添加新 Agent

EnerOS 的 Agent 作为 OS 调度单元运行，每个 Agent 是一个独立进程。

### 4.1 实现步骤

1. **实现 Agent 逻辑**：在 `crates/eneros-agent/src/agents/` 下新建模块，实现 Agent 的决策循环与消息处理。

2. **创建二进制入口**：在 `crates/eneros-agent/bins/` 下新建目录，包含 `Cargo.toml` 和 `src/main.rs`：

```toml
# crates/eneros-agent/bins/my-agent/Cargo.toml
[package]
name = "my-agent"
version.workspace = true
edition.workspace = true

[dependencies]
eneros-agent = { path = "../.." }
eneros-core = { path = "../../../eneros-core" }
tokio = { workspace = true }
```

```rust
// crates/eneros-agent/bins/my-agent/src/main.rs
use eneros_agent::agents::my_agent::MyAgent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = MyAgent::new();
    agent.run().await
}
```

3. **注册到 workspace**：在根 `Cargo.toml` 的 `members` 中添加 `"crates/eneros-agent/bins/my-agent"`。

4. **配置 init.toml**：在 `os/rootfs/files/etc/eneros/init.toml` 中添加 Agent 启动配置，指定二进制路径、启动参数、资源配额。

5. **编译验证**：

```bash
cargo build -p my-agent
cargo test -p eneros-agent
```

### 4.2 Agent 类型

EnerOS 内置 7 种专业 Agent，新增 Agent 时应明确其类型与职责边界：

| Agent | 职责 |
|-------|------|
| dispatch-agent | 调度决策 |
| forecast-agent | 负荷/发电预测 |
| operation-agent | 操作执行 |
| planning-agent | 运行规划 |
| trading-agent | 电力交易 |
| self-healing-agent | 故障自愈 |

## 5. 添加新协议适配器

协议适配器位于 `crates/eneros-device/src/adapters/`，实现设备协议的编解码与通信。

### 5.1 实现步骤

1. **创建适配器模块**：在 `crates/eneros-device/src/adapters/` 下新建目录（如 `my_protocol/`），实现协议编解码。

2. **实现 DeviceAdapter trait**：参考 `crates/eneros-device/src/adapter.rs` 中的 trait 定义，实现连接、读写、断开等方法。

3. **注册到 DeviceManager**：在 `crates/eneros-device/src/adapters/mod.rs` 中声明模块，并在 `DeviceManager` 中注册适配器工厂。

4. **添加测试**：在适配器模块内添加单元测试，在 `crates/eneros-device/tests/` 下添加集成测试。

5. **配置设备**：在 `eneros.toml` 或 `init.toml` 中配置使用新协议的设备连接。

### 5.2 已支持协议

| 协议 | 模块 | 传输层 |
|------|------|--------|
| IEC 60870-5-104 | `adapters/iec104/` | TCP |
| IEC 61850 (MMS) | `adapters/iec61850/` | TCP |
| IEC 61850 (GOOSE) | `adapters/goose/` | AF_PACKET (L2) |
| IEC 61850 (SV) | `adapters/sv/` | AF_PACKET (L2) |
| Modbus TCP | `adapters/modbus/` | TCP |
| Modbus RTU | `adapters/modbus_rtu/` | 串口 |
| DNP3 | `adapters/dnp3/` | TCP/串口 |
| OPC UA | `adapters/opcua/` | TCP |
| MQTT | `adapters/mqtt/` | TCP |

GOOSE/SV 等二层协议通过 AF_PACKET 原始套接字实现，为 OS 原生能力。

## 6. 添加新插件

插件系统支持三类插件（Protocol/Agent/Analysis），通过 `#[eneros_plugin]` 宏简化开发。详细流程见 [插件开发指南](./plugin-development.md)。

### 6.1 快速流程

1. 在 `eneros-sdk` 依赖基础上创建插件 crate
2. 使用 `#[eneros_plugin(...)]` 宏标注 `impl Plugin for MyPlugin`
3. 编写 `manifest.toml` 描述插件元数据
4. 使用 `enerosctl plugin gen-keys` 生成签名密钥
5. 使用 `enerosctl plugin sign` 签名插件
6. 使用 `enerosctl plugin load` 加载插件

## 7. 测试指南

### 7.1 单元测试

位于各 crate 的 `src/*.rs` 文件内 `#[cfg(test)] mod tests` 块中，测试单个函数或模块逻辑。

```bash
# 运行单个 crate 的单元测试
cargo test -p eneros-plugin

# 运行匹配名称的测试
cargo test -p eneros-powerflow -- solver

# 显示 println 输出
cargo test -p eneros-core -- --nocapture
```

### 7.2 集成测试

位于各 crate 的 `tests/` 目录，测试 crate 间集成与端到端场景。命名约定 `e2e_*.rs` 表示端到端测试。

```bash
# 运行指定集成测试
cargo test -p eneros-api --test e2e_integration

# 运行全部 E2E 测试
cargo test --workspace --test e2e_*
```

主要 E2E 测试：

| 测试文件 | 验证内容 |
|----------|----------|
| eneros-api/tests/e2e_integration.rs | API 端到端集成 |
| eneros-api/tests/e2e_v04_wiring.rs | v0.4 接线验证 |
| eneros-api/tests/e2e_v08_analysis.rs | v0.8 分析功能 |
| eneros-gateway/tests/e2e_agentos.rs | 网关 AgentOS 集成 |
| eneros-agent/tests/e2e_domain.rs | Agent 领域逻辑 |
| eneros-bridge/tests/bridge_e2e.rs | Python 桥接 |
| eneros-network/tests/e2e_pipeline.rs | 网络 pipeline |
| os/tests/boot_test.rs | 系统启动测试 |

### 7.3 OS 级测试

位于 `os/tests/`，测试系统启动、引导参数、分区等 OS 级行为：

```bash
cargo test -p eneros-os-tests
```

### 7.4 测试规范

- 新增功能必须附带单元测试
- Bug 修复必须附带回归测试
- 异步测试使用 `#[tokio::test]`
- 测试函数命名表达被测行为：`test_signature_verify_rejects_tampered_plugin`

## 8. 性能基准测试

### 8.1 基准测试方法

使用 `criterion` 进行基准测试。实时安全网关的基准测试位于 `crates/eneros-gateway/tests/rt_benchmark.rs`。

```bash
# 运行基准测试
cargo bench -p eneros-gateway

# 运行指定基准
cargo bench -p eneros-gateway -- rt_benchmark
```

### 8.2 关键性能指标

| 指标 | 目标 | 测试位置 |
|------|------|----------|
| 网关决策管线延迟 | < 1ms（实时域） | eneros-gateway/tests/rt_benchmark.rs |
| 潮流计算（IEEE 14） | < 10ms | eneros-powerflow |
| IEC 104 扫描周期 | 100ms（快速组） | eneros-scada |
| 事件总线吞吐 | > 100k events/s | eneros-eventbus |

### 8.3 性能分析

```bash
# 使用 perf（Linux）
cargo build --release -p eneros-gateway
perf record -g ./target/release/gateway
perf report

# 使用 flamegraph
cargo install flamegraph
cargo flamegraph -p eneros-powerflow -- bench_solver
```

## 9. 调试技巧

### 9.1 tracing 日志

EnerOS 使用 `tracing` 进行结构化日志，通过 `RUST_LOG` 环境变量控制级别：

```bash
# 全局 debug 级别
RUST_LOG=debug cargo run -p eneros-api -- run --config eneros.toml

# 指定 crate 级别
RUST_LOG=eneros_device=trace,eneros_scada=debug,info cargo run -p eneros-api

# 仅警告和错误
RUST_LOG=warn cargo run -p eneros-api
```

日志级别优先级：`error` > `warn` > `info` > `debug` > `trace`。

### 9.2 JSON 结构化日志

生产环境使用 JSON 格式日志，便于日志聚合系统采集：

```bash
# 输出 JSON 日志
ENEROS_OBSERVABILITY__LOG_FORMAT=json cargo run -p eneros-api
```

### 9.3 enerosctl doctor 诊断

使用 `enerosctl doctor` 进行系统级诊断，检查内核、Agent、EventBus、设备、安全等子系统状态：

```bash
enerosctl doctor
```

该命令会输出各子系统的健康状态与异常提示。

### 9.4 事件总线订阅

实时查看事件总线消息：

```bash
# 订阅所有事件
enerosctl eventbus subscribe

# 订阅指定主题
enerosctl eventbus subscribe agent.command
```

### 9.5 常见问题排查

**编译失败：找不到 libseccomp**

Linux 平台需要安装 libseccomp 开发库：

```bash
sudo apt install libseccomp-dev   # Debian/Ubuntu
sudo dnf install libseccomp-devel # Fedora
```

跨平台开发时排除依赖该库的 crate：

```bash
cargo build --workspace --exclude eneros-installer
```

**Agent 无法启动**

1. 检查 `enerosctl agent list` 是否显示该 Agent
2. 查看 `enerosctl log tail agent` 日志
3. 确认 `init.toml` 中 Agent 配置正确
4. 运行 `enerosctl doctor` 检查系统状态

**IEC 104 连接失败**

1. 确认 RTU 地址和端口：`telnet <addr> 2404`
2. 检查 ASDU 地址匹配
3. 查看日志中 TESTFR 心跳状态
4. 使用 `enerosctl protocol test iec104 <addr>` 测试连通性

## 10. 相关文档

- [贡献指南](../CONTRIBUTING.md)
- [部署运维指南](./deployment.md)
- [用户手册](./user-manual.md)
- [插件开发指南](./plugin-development.md)
- [架构决策记录](./adr/0001-record-architecture-decisions.md)
