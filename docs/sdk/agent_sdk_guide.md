# EnerOS Agent SDK 开发指南

> **版本**：v1.0.0 (v0.164.0)  
> **最后更新**：2026-07-30

## 1. 概述

EnerOS Agent SDK 是面向外部开发者的 Agent 开发框架。基于此 SDK，您可以开发运行在 EnerOS 上的自定义 Agent。

### 1.1 特性

- **no_std 兼容**：完全支持裸机环境
- **稳定 API**：v1.0 承诺向后兼容
- **完整生命周期**：init → start → tick → stop → destroy
- **IPC 通信**：Agent 间消息传递
- **键值存储**：本地持久化
- **结构化日志**：分级日志输出
- **配置管理**：运行时配置读取

## 2. 快速开始

### 2.1 添加依赖

```toml
[dependencies]
eneros-agent-sdk = "0.164"
```

### 2.2 Hello World Agent

```rust
use eneros_agent_sdk::{register_agent, Agent, AgentDescriptor, AgentError, Context};

struct HelloAgent;

impl HelloAgent {
    fn new() -> Self { Self }
}

impl Agent for HelloAgent {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor::minimal("hello", "1.0.0")
            .with_author("Your Name")
            .with_description("Hello World Agent")
    }

    fn on_init(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
        ctx.logger.info("HelloAgent initialized");
        Ok(())
    }

    fn on_start(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
        ctx.logger.info("HelloAgent started");
        Ok(())
    }

    fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
        // 周期执行逻辑
        Ok(())
    }

    fn on_stop(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
        ctx.logger.info("HelloAgent stopped");
        Ok(())
    }

    fn on_destroy(&mut self, _ctx: &mut Context) {
        // 最终清理
    }
}

register_agent!(HelloAgent, "hello-agent");
```

### 2.3 编译

```bash
cargo build --release
```

## 3. 核心概念

### 3.1 Agent Trait

`Agent` trait 是 SDK 的核心接口，包含 6 个方法：

| 方法 | 说明 | 可失败 |
|------|------|--------|
| `descriptor()` | 返回 Agent 元信息 | 否 |
| `on_init()` | 初始化（加载配置、分配资源） | 是 |
| `on_start()` | 启动（启动主循环） | 是 |
| `on_tick()` | 周期执行（由 Runtime 调用） | 是 |
| `on_stop()` | 停止（释放资源、保存状态） | 是 |
| `on_destroy()` | 销毁（最终清理） | 否 |

### 3.2 Context

`Context` 是 Runtime 提供的运行时环境，包含：

- `agent_id` — Agent 唯一标识
- `config` — 配置 API
- `ipc` — IPC 通信
- `logger` — 日志
- `storage` — 键值存储

### 3.3 生命周期

```
Created → Initialized → Running → Stopped → Destroyed
                ↓           ↓
              Error ←───────┘
                ↓
          Initialized (恢复)
```

## 4. API 详解

### 4.1 配置 API

```rust
// 读取配置
let interval = ctx.config.get_u32("interval_ms")?;
let name = ctx.config.get_str("name")?;
let enabled = ctx.config.get_bool("enabled")?;
```

### 4.2 IPC 通信

```rust
// 发送消息
ctx.ipc.send("target-agent", b"hello")?;

// 接收消息
let msg = ctx.ipc.recv()?;

// 请求-响应
let resp = ctx.ipc.request("target-agent", b"query")?;
```

### 4.3 存储 API

```rust
// 写入
ctx.storage.write("state", b"running")?;

// 读取
let state = ctx.storage.read("state")?;

// 删除
ctx.storage.delete("state")?;

// 检查存在
if ctx.storage.exists("state") {
    // ...
}
```

### 4.4 日志 API

```rust
ctx.logger.trace("detailed info");
ctx.logger.debug("debug info");
ctx.logger.info("normal info");
ctx.logger.warn("warning");
ctx.logger.error("error occurred");
```

## 5. 示例 Agents

### 5.1 数据采集 Agent

```rust
struct DataCollector {
    count: u32,
}

impl Agent for DataCollector {
    fn on_tick(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
        self.count += 1;
        // 采集数据
        let data = self.read_sensor();
        // 存储
        ctx.storage.write("latest", &data)?;
        // 发送
        ctx.ipc.send("data-server", &data)?;
        Ok(())
    }
}
```

### 5.2 告警 Agent

```rust
struct AlarmAgent;

impl Agent for AlarmAgent {
    fn on_tick(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
        // 检查告警条件
        if self.check_alarm_condition()? {
            ctx.logger.warn("Alarm triggered!");
            ctx.ipc.send("alarm-server", b"ALARM")?;
        }
        Ok(())
    }
}
```

## 6. 最佳实践

### 6.1 错误处理

- 使用 `?` 传播错误
- 在 `on_tick` 中处理可恢复错误
- 不可恢复错误返回 `Err` 让 Runtime 处理

### 6.2 资源管理

- 在 `on_init` 中分配资源
- 在 `on_stop` 中释放资源
- 在 `on_destroy` 中做最终清理

### 6.3 性能

- `on_tick` 应快速返回（< 10ms）
- 耗时操作使用异步或后台线程
- 避免在 `on_tick` 中分配内存

## 7. 部署

### 7.1 编译为动态库

```toml
[lib]
crate-type = ["cdylib"]
```

### 7.2 部署到 EnerOS

将编译产物 `.so` 文件部署到 EnerOS 的 Agent 目录，Runtime 会自动加载。

## 8. 故障排查

| 问题 | 解决方案 |
|------|---------|
| Agent 未加载 | 检查 `register_agent!` 宏是否调用 |
| IPC 失败 | 检查目标 Agent 名称是否正确 |
| 存储失败 | 检查键长度 ≤ 128，值长度 ≤ 64KB |
| 配置缺失 | 检查配置文件是否包含对应键 |
