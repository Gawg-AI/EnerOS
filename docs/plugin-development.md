# EnerOS 插件开发指南

本指南面向第三方开发者，说明如何为 EnerOS 开发协议适配器、Agent 策略与分析模块插件。EnerOS v0.27.0 引入插件框架，v0.28.0 增加 plugin-daemon 进程隔离模式。

## 1. 插件系统概述

EnerOS 插件系统支持三类插件，以动态库（.so/.dll/.dylib）形式接入系统：

| 插件类型 | 用途 | 实现 trait | 权限上限 |
|----------|------|-----------|----------|
| **Protocol** | 协议适配器（IEC 104/61850/Modbus 等扩展） | `ProtocolPlugin` | 设备访问 |
| **Agent** | Agent 策略（调度、预测、操作等扩展） | `AgentPlugin` | Operator |
| **Analysis** | 分析模块（潮流、状态估计等扩展） | `AnalysisPlugin` | 只读分析 |

插件通过 Ed25519 签名验证保障来源可信，通过 seccomp 沙箱与 cgroups 资源配额实现隔离。v0.28.0 起，插件默认在 plugin-daemon 独立进程中加载（Daemon 模式），崩溃不影响主进程；开发环境可使用 Inline 模式（同进程加载）。

## 2. 开发环境

### 2.1 依赖

插件开发依赖以下 crate：

- `eneros-sdk`：开发者 SDK，封装常用类型与辅助函数
- `eneros-plugin`：插件框架核心，提供 trait 定义与错误类型
- `eneros-plugin-macros`：`#[eneros_plugin]` 过程宏，自动生成 C ABI 入口函数

在插件的 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
eneros-plugin = { path = "../eneros-plugin" }
eneros-plugin-macros = { path = "../eneros-plugin-macros" }
eneros-sdk = { path = "../eneros-sdk", features = ["plugin"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### 2.2 工具链

- Rust 1.75+，edition 2021
- 编译为动态库（`crate-type = ["cdylib"]`）

## 3. 快速开始

使用 `#[eneros_plugin]` 宏创建第一个插件。以下示例实现一个最小的分析插件：

```rust
// src/lib.rs
use std::sync::OnceLock;

use eneros_plugin::{Plugin, PluginMetadata, PluginResult, PluginType};
use eneros_plugin_macros::eneros_plugin;

struct MyAnalysisPlugin;

/// 静态元数据存储（OnceLock 保证线程安全初始化）
static METADATA: OnceLock<PluginMetadata> = OnceLock::new();

#[eneros_plugin(
    name = "my-analysis",
    version = "1.0.0",
    api_version = "0.28.0",
    plugin_type = "analysis",
    author = "Your Name",
    description = "My first analysis plugin"
)]
#[async_trait::async_trait]
impl Plugin for MyAnalysisPlugin {
    fn metadata(&self) -> &PluginMetadata {
        METADATA.get_or_init(|| PluginMetadata {
            name: "my-analysis".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Analysis,
            description: "My first analysis plugin".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Analysis
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}
```

`Cargo.toml`：

```toml
[package]
name = "my-analysis-plugin"
version = "1.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
eneros-plugin = { path = "../../crates/eneros-plugin" }
eneros-plugin-macros = { path = "../../crates/eneros-plugin-macros" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
```

`#[eneros_plugin]` 宏会自动生成 C ABI 入口函数（`eneros_plugin_create` / `eneros_plugin_destroy` / `eneros_plugin_metadata`）及 vtable 全局变量，无需手动编写。

编译：

```bash
cargo build --release
# 产物：target/release/libmy_analysis_plugin.so（Linux）
#       target/release/my_analysis_plugin.dll（Windows）
#       target/release/libmy_analysis_plugin.dylib（macOS）
```

## 4. 三类插件开发流程

### 4.1 协议插件（Protocol）

协议插件实现 `ProtocolPlugin` trait，用于扩展 EnerOS 支持的设备协议。

```rust
use std::sync::OnceLock;

use async_trait::async_trait;
use eneros_plugin::protocol::{
    PluginDataPoint, PluginDataQuality, PluginDataValue, ProtocolAdapterInstance, ProtocolPlugin,
    ProtocolPluginConfig,
};
use eneros_plugin::{Plugin, PluginError, PluginMetadata, PluginResult, PluginType};
use eneros_plugin_macros::eneros_plugin;

struct Iec103Plugin;

static METADATA: OnceLock<PluginMetadata> = OnceLock::new();

#[eneros_plugin(
    name = "iec103-driver",
    version = "1.2.0",
    api_version = "0.28.0",
    plugin_type = "protocol",
    author = "EnerOS",
    description = "IEC 60870-5-103 protocol driver"
)]
#[async_trait::async_trait]
impl Plugin for Iec103Plugin {
    fn metadata(&self) -> &PluginMetadata {
        METADATA.get_or_init(|| PluginMetadata {
            name: "iec103-driver".to_string(),
            version: "1.2.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Protocol,
            description: "IEC 60870-5-103 protocol driver".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Protocol
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}

#[async_trait]
impl ProtocolPlugin for Iec103Plugin {
    fn protocol_name(&self) -> &str {
        "iec103"
    }

    fn protocol_type_str(&self) -> String {
        // 默认实现返回 "custom:<protocol_name>"
        // 与 eneros_device::ProtocolType::Custom(name) 的 serde 表示一致
        format!("custom:{}", self.protocol_name())
    }

    fn description(&self) -> &str {
        "IEC 60870-5-103 protocol driver"
    }

    async fn create_adapter(
        &self,
        _config: &ProtocolPluginConfig,
    ) -> Result<Box<dyn ProtocolAdapterInstance>, PluginError> {
        // 每次调用返回独立的适配器实例（对应一次设备连接）
        Ok(Box::new(Iec103Adapter::new()))
    }
}

struct Iec103Adapter {
    connected: bool,
}

impl Iec103Adapter {
    fn new() -> Self {
        Self { connected: false }
    }
}

#[async_trait]
impl ProtocolAdapterInstance for Iec103Adapter {
    async fn connect(&mut self) -> Result<(), PluginError> {
        // 建立连接
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), PluginError> {
        // 断开连接
        self.connected = false;
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<PluginDataPoint, PluginError> {
        // 读取数据点
        Ok(PluginDataPoint {
            address: address.to_string(),
            value: PluginDataValue::Float32(0.0),
            timestamp: 0,
            quality: if self.connected {
                PluginDataQuality::Good
            } else {
                PluginDataQuality::Offline
            },
        })
    }

    async fn write(
        &mut self,
        _address: &str,
        _value: &PluginDataValue,
    ) -> Result<(), PluginError> {
        // 写入数据
        Ok(())
    }

    fn name(&self) -> &str {
        "iec103-adapter"
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
```

设备层通过协议类型字符串（`custom:iec103`）查找对应插件。

### 4.2 Agent 插件（Agent）

Agent 插件实现 `AgentPlugin` trait，用于扩展 Agent 的策略逻辑。Agent 插件的权限上限为 `Operator`，即使声明更高权限也会被系统强制降级。

```rust
use std::sync::OnceLock;

use async_trait::async_trait;
use eneros_core::AuthorityLevel;
use eneros_plugin::agent::{
    AgentPlugin, AgentPluginAction, AgentPluginConfig, AgentPluginEvent, AgentStrategyInstance,
    StrategyPriority,
};
use eneros_plugin::{Plugin, PluginError, PluginMetadata, PluginResult, PluginType};
use eneros_plugin_macros::eneros_plugin;

struct CustomDispatchStrategy;

static METADATA: OnceLock<PluginMetadata> = OnceLock::new();

#[eneros_plugin(
    name = "custom-dispatch",
    version = "0.1.0",
    api_version = "0.28.0",
    plugin_type = "agent",
    author = "Your Name",
    description = "Custom dispatch strategy"
)]
#[async_trait::async_trait]
impl Plugin for CustomDispatchStrategy {
    fn metadata(&self) -> &PluginMetadata {
        METADATA.get_or_init(|| PluginMetadata {
            name: "custom-dispatch".to_string(),
            version: "0.1.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Agent,
            description: "Custom dispatch strategy".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Agent
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}

#[async_trait]
impl AgentPlugin for CustomDispatchStrategy {
    fn strategy_name(&self) -> &str {
        "custom-dispatch"
    }

    fn description(&self) -> &str {
        "Custom dispatch strategy"
    }

    fn authority_level(&self) -> AuthorityLevel {
        // 系统会通过 enforce_authority_limit 强制降级到 Operator 上限
        // 插件即使声明 Emergency 或 Supervisor 也无法获得高于 Operator 的权限
        AuthorityLevel::Operator
    }

    fn priority(&self) -> StrategyPriority {
        // 用于多插件冲突解决，高优先级插件的动作优先执行
        StrategyPriority::Normal
    }

    async fn create_agent(
        &self,
        config: &AgentPluginConfig,
    ) -> Result<Box<dyn AgentStrategyInstance>, PluginError> {
        Ok(Box::new(CustomDispatchAgent {
            agent_id: config.agent_id.clone(),
            agent_type: config.agent_type.clone(),
        }))
    }
}

struct CustomDispatchAgent {
    agent_id: String,
    agent_type: String,
}

#[async_trait]
impl AgentStrategyInstance for CustomDispatchAgent {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn agent_type(&self) -> &str {
        &self.agent_type
    }

    async fn handle_event(
        &mut self,
        _event: &AgentPluginEvent,
    ) -> Result<Vec<AgentPluginAction>, PluginError> {
        Ok(vec![AgentPluginAction::NoOp])
    }

    async fn tick(&mut self) -> Result<Vec<AgentPluginAction>, PluginError> {
        Ok(vec![])
    }
}
```

### 4.3 分析插件（Analysis）

分析插件实现 `AnalysisPlugin` trait，输入输出为 `serde_json::Value`，用于扩展分析能力。

```rust
use std::sync::OnceLock;

use async_trait::async_trait;
use eneros_plugin::analysis::{AnalysisPlugin, AnalysisResult};
use eneros_plugin::{Plugin, PluginError, PluginMetadata, PluginResult, PluginType};
use eneros_plugin_macros::eneros_plugin;

struct ReliabilityAnalysis;

static METADATA: OnceLock<PluginMetadata> = OnceLock::new();

#[eneros_plugin(
    name = "reliability-analysis",
    version = "1.0.0",
    api_version = "0.28.0",
    plugin_type = "analysis",
    author = "Your Name",
    description = "Power grid reliability analysis"
)]
#[async_trait::async_trait]
impl Plugin for ReliabilityAnalysis {
    fn metadata(&self) -> &PluginMetadata {
        METADATA.get_or_init(|| PluginMetadata {
            name: "reliability-analysis".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Analysis,
            description: "Power grid reliability analysis".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Analysis
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}

impl AnalysisPlugin for ReliabilityAnalysis {
    fn analyze_type(&self) -> &str {
        "reliability"
    }

    fn description(&self) -> &str {
        "Power grid reliability analysis"
    }

    fn analyze(
        &self,
        input: &serde_json::Value,
    ) -> Result<AnalysisResult<serde_json::Value>, PluginError> {
        // 解析输入字段
        let load_level = input["load_level"]
            .as_f64()
            .ok_or_else(|| PluginError::InvalidManifest("missing load_level".into()))?;

        // 执行分析逻辑
        let sai = 1.0 - load_level * 0.01;
        let caidi = 2.5;

        // 构造输出 JSON
        let output = serde_json::json!({
            "sai": sai,
            "caidi": caidi,
            "assessment": if sai > 0.999 { "good" } else { "needs_attention" }
        });

        // AnalysisResult::new 创建收敛结果（converged=true, iterations=1, warnings=[]）
        Ok(AnalysisResult::new(output))
    }
}
```

## 5. 插件清单（manifest.toml）

每个插件需附带 `manifest.toml` 清单文件，描述元数据、依赖与安全信息：

```toml
[plugin]
name = "iec104-driver"
version = "1.2.0"
api_version = "0.28.0"
plugin_type = "Protocol"
description = "IEC 104 protocol driver"
author = "EnerOS"

[dependencies]
plugins = ["core-mbus"]       # 依赖的其他插件名列表

[security]
signer = "eneros-trusted"      # 签名者标识
```

### 字段说明

| 段 | 字段 | 类型 | 说明 |
|----|------|------|------|
| `[plugin]` | `name` | String | 插件名称（唯一标识） |
| | `version` | String | 插件版本（语义化版本） |
| | `api_version` | String | 插件 API 版本（与 EnerOS API 版本兼容性检查） |
| | `plugin_type` | String | 插件类型（Protocol / Agent / Analysis） |
| | `description` | String | 插件描述 |
| | `author` | String | 插件作者 |
| `[dependencies]` | `plugins` | Vec\<String\> | 依赖的其他插件名列表 |
| `[security]` | `signer` | String | 签名者标识 |

### 版本兼容性规则

- 0.x 版本：比较次版本号（MINOR），次版本号相同即兼容
- 1.x+ 版本：比较主版本号（MAJOR），主版本号相同即兼容

### 依赖解析

插件依赖通过 Kahn 算法进行拓扑排序，确定加载顺序。若检测到循环依赖，加载将被拒绝。

## 6. 签名流程

生产环境中插件必须经过 Ed25519 签名验证。签名流程复用 v0.22.0 OTA 签名基础设施（ed25519-dalek）。

### 6.1 生成密钥对

```bash
enerosctl plugin gen-keys --output /etc/eneros/keys/
```

生成两个文件：

- `private.key`：私钥（base64 编码，妥善保管）
- `public.pub`：公钥（base64 编码，部署到设备）

### 6.2 签名插件

```bash
enerosctl plugin sign ./libiec104_driver.so /etc/eneros/keys/private.key
```

生成签名文件 `./libiec104_driver.so.sig`。

### 6.3 验证签名

```bash
# 验证签名（不加载插件）
enerosctl plugin verify ./libiec104_driver.so

# 指定签名文件路径
enerosctl plugin verify ./libiec104_driver.so --sig ./custom.sig
```

验证结果：

- `Valid`：签名有效，签名者匹配可信公钥
- `Invalid`：签名无效（文件被篡改）
- `Missing`：未找到签名文件
- `UntrustedSigner`：签名有效但签名者不在可信公钥列表中

### 6.4 可信公钥管理

可信公钥部署在 `/etc/eneros/keys/` 目录。`plugin.toml` 中 `require_signature = true` 时，未签名或签名不可信的插件将被拒绝加载。

开发环境可使用 `--skip-signature` 跳过验证：

```bash
enerosctl plugin load ./my_plugin.so --skip-signature
```

## 7. 沙箱限制

插件在 plugin-daemon 进程中运行，受 seccomp 与 cgroups 双重约束。

### 7.1 seccomp 限制

以下 syscall 被 seccomp BPF 规则禁止：

| syscall | 禁止原因 |
|---------|----------|
| `mount` | 禁止挂载文件系统 |
| `reboot` | 禁止重启系统 |
| `kexec_load` | 禁止加载新内核 |
| `init_module` / `finit_module` | 禁止加载内核模块 |
| `ptrace` | 禁止进程追踪 |
| `setuid` / `setgid` | 禁止权限提升 |

### 7.2 cgroups 资源配额

通过 `plugin.toml` 配置资源上限：

```toml
[plugin.sandbox]
enable_seccomp = true
enable_quota = true
default_cpu_percent = 50      # CPU 上限（百分比）
default_memory_mb = 256       # 内存上限（MB）
allowed_paths = ["/var/lib/eneros/data"]   # 允许访问的路径
denied_paths = ["/etc/shadow"]             # 禁止访问的路径
allowed_network = ["tcp:2404"]             # 允许的网络访问
```

### 7.3 崩溃隔离

插件 panic 或段错误时，plugin-daemon 进程崩溃，主进程不受影响。主进程检测到 plugin-daemon 退出后可自动重启并重新加载插件。

## 8. 插件部署

### 8.1 加载模式

| 模式 | 说明 | 适用场景 |
|------|------|----------|
| **Daemon**（默认） | 插件在 plugin-daemon 独立进程中加载，通过 IPC 通信 | 生产环境 |
| **Inline** | 插件在同进程加载，直接函数调用 | 开发/测试，低延迟场景 |

通过 `plugin.toml` 配置默认模式：

```toml
[plugin]
default_mode = "daemon"
```

单个插件可在 manifest 中指定模式。

### 8.2 IPC 通信

Daemon 模式下，主进程通过 `PluginDaemonClient`（eneros-plugin/src/ipc.rs）与 plugin-daemon 通信：

- 主进程发送 `DaemonRequest`（JSON 行协议 over Unix socket / TCP）
- plugin-daemon 返回 `DaemonResponse`

### 8.3 加载与卸载

```bash
# 加载插件（验证签名 → 加载库 → 初始化 → 启动）
enerosctl plugin load ./libiec104_driver.so

# 卸载插件（停止 → 卸载库）
enerosctl plugin unload iec104-driver

# 查看插件详情
enerosctl plugin info iec104-driver

# 启用/禁用插件
enerosctl plugin enable iec104-driver
enerosctl plugin disable iec104-driver
```

### 8.4 插件生命周期

插件状态机遵循以下转换：

```
Loaded → Initialized → Starting → Running → Stopping → Stopped
                                              ↘ Crashed
                                              ↘ Failed
```

非法状态转换会返回 `InvalidStateTransition` 错误。

## 9. 插件市场发布流程

### 9.1 打包

将插件动态库、manifest.toml、签名文件打包为 tar.gz：

```bash
tar czf iec104-driver-1.2.0.tar.gz \
    libiec104_driver.so \
    manifest.toml \
    libiec104_driver.so.sig
```

### 9.2 上传

将打包文件上传至插件市场索引服务器，附带公钥信息以便消费者验证。

### 9.3 索引

插件市场维护插件索引（名称、版本、签名者、下载地址），消费者通过 `enerosctl plugin list` 查看可用插件。

## 10. 完整示例：从零开发一个协议插件

以下示例从零开发一个简单的自定义协议插件，包含完整代码、清单、签名与部署流程。

### 10.1 创建项目

```bash
mkdir -p my-protocol-plugin/src
cd my-protocol-plugin
```

### 10.2 Cargo.toml

```toml
[package]
name = "my-protocol-plugin"
version = "1.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
eneros-plugin = { path = "../eneros/crates/eneros-plugin" }
eneros-plugin-macros = { path = "../eneros/crates/eneros-plugin-macros" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
```

### 10.3 src/lib.rs

```rust
use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use eneros_plugin::protocol::{
    PluginDataPoint, PluginDataQuality, PluginDataValue, ProtocolAdapterInstance, ProtocolPlugin,
    ProtocolPluginConfig,
};
use eneros_plugin::{Plugin, PluginError, PluginMetadata, PluginResult, PluginType};
use eneros_plugin_macros::eneros_plugin;

struct MyProtocolPlugin;

static METADATA: OnceLock<PluginMetadata> = OnceLock::new();

#[eneros_plugin(
    name = "my-protocol",
    version = "1.0.0",
    api_version = "0.28.0",
    plugin_type = "protocol",
    author = "Example Author",
    description = "Example custom protocol adapter"
)]
#[async_trait::async_trait]
impl Plugin for MyProtocolPlugin {
    fn metadata(&self) -> &PluginMetadata {
        METADATA.get_or_init(|| PluginMetadata {
            name: "my-protocol".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.28.0".to_string(),
            plugin_type: PluginType::Protocol,
            description: "Example custom protocol adapter".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Protocol
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}

#[async_trait]
impl ProtocolPlugin for MyProtocolPlugin {
    fn protocol_name(&self) -> &str {
        "my-protocol"
    }

    fn description(&self) -> &str {
        "Example custom protocol adapter"
    }

    async fn create_adapter(
        &self,
        _config: &ProtocolPluginConfig,
    ) -> Result<Box<dyn ProtocolAdapterInstance>, PluginError> {
        Ok(Box::new(MyAdapter::new()))
    }
}

struct MyAdapter {
    connected: bool,
    data: HashMap<String, PluginDataValue>,
}

impl MyAdapter {
    fn new() -> Self {
        Self {
            connected: false,
            data: HashMap::new(),
        }
    }
}

#[async_trait]
impl ProtocolAdapterInstance for MyAdapter {
    async fn connect(&mut self) -> Result<(), PluginError> {
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), PluginError> {
        self.connected = false;
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<PluginDataPoint, PluginError> {
        if !self.connected {
            return Err(PluginError::InitFailed("not connected".into()));
        }
        let value = self
            .data
            .get(address)
            .cloned()
            .unwrap_or(PluginDataValue::Float32(0.0));
        Ok(PluginDataPoint {
            address: address.to_string(),
            value,
            timestamp: 0,
            quality: PluginDataQuality::Good,
        })
    }

    async fn write(
        &mut self,
        address: &str,
        value: &PluginDataValue,
    ) -> Result<(), PluginError> {
        if !self.connected {
            return Err(PluginError::InitFailed("not connected".into()));
        }
        self.data.insert(address.to_string(), value.clone());
        Ok(())
    }

    fn name(&self) -> &str {
        "my-adapter"
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
```

### 10.4 manifest.toml

```toml
[plugin]
name = "my-protocol"
version = "1.0.0"
api_version = "0.28.0"
plugin_type = "Protocol"
description = "Example custom protocol adapter"
author = "Example Author"

[dependencies]
plugins = []

[security]
signer = "example-author"
```

### 10.5 编译

```bash
cargo build --release
```

产物：`target/release/libmy_protocol_plugin.so`

### 10.6 签名

```bash
# 生成密钥对（首次）
enerosctl plugin gen-keys --output /etc/eneros/keys/

# 签名
enerosctl plugin sign ./target/release/libmy_protocol_plugin.so /etc/eneros/keys/private.key

# 验证
enerosctl plugin verify ./target/release/libmy_protocol_plugin.so
```

### 10.7 部署

```bash
# 复制到插件目录
cp ./target/release/libmy_protocol_plugin.so /var/lib/eneros/plugins/
cp ./target/release/libmy_protocol_plugin.so.sig /var/lib/eneros/plugins/
cp manifest.toml /var/lib/eneros/plugins/my-protocol.toml

# 加载
enerosctl plugin load /var/lib/eneros/plugins/libmy_protocol_plugin.so

# 验证加载状态
enerosctl plugin list
enerosctl plugin info my-protocol
```

### 10.8 测试

编写单元测试验证插件逻辑：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_adapter_connect_read_write() {
        let mut adapter = MyAdapter::new();
        assert!(!adapter.connected);

        adapter.connect().await.unwrap();
        assert!(adapter.connected);

        adapter.write("reg.1", &PluginDataValue::Int32(42)).await.unwrap();
        let dp = adapter.read("reg.1").await.unwrap();
        assert_eq!(dp.value, PluginDataValue::Int32(42));

        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[test]
    fn test_protocol_type_str() {
        let plugin = MyProtocolPlugin;
        assert_eq!(plugin.protocol_name(), "my-protocol");
        assert_eq!(plugin.protocol_type_str(), "custom:my-protocol");
    }
}
```

## 11. 相关文档

- [用户手册 — plugin 命令](./user-manual.md#313-plugin--插件管理)
- [开发者指南 — 添加新插件](./developer-guide.md#6-添加新插件)
- [ADR-0003：plugin-daemon 进程隔离](./adr/0003-plugin-process-isolation.md)
- [部署运维指南 — plugin-daemon 部署](./deployment.md)
- [eneros-plugin crate 源码](../crates/eneros-plugin/src/lib.rs)
