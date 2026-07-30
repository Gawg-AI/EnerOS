# EnerOS Agent SDK API 参考

> **版本**：v1.0.0 (v0.164.0)

## 模块

| 模块 | 说明 |
|------|------|
| `agent` | Agent trait、AgentDescriptor、AgentError |
| `context` | Context（运行时环境） |
| `lifecycle` | 生命周期状态机 |
| `ipc` | IPC 通信 |
| `storage` | 键值存储 |
| `logger` | 日志 |
| `config` | 配置 |

## 核心类型

### Agent trait

```rust
pub trait Agent: Send + Sync {
    fn descriptor(&self) -> AgentDescriptor;
    fn on_init(&mut self, ctx: &mut Context) -> Result<(), AgentError>;
    fn on_start(&mut self, ctx: &mut Context) -> Result<(), AgentError>;
    fn on_tick(&mut self, ctx: &mut Context) -> Result<(), AgentError>;
    fn on_stop(&mut self, ctx: &mut Context) -> Result<(), AgentError>;
    fn on_destroy(&mut self, ctx: &mut Context);
}
```

### AgentDescriptor

```rust
pub struct AgentDescriptor {
    pub name: HString<64>,
    pub version: HString<32>,
    pub author: HString<64>,
    pub description: HString<256>,
    pub capabilities: [Option<HString<32>>; 8],
    pub config_schema: HString<512>,
}
```

**方法**：
- `minimal(name, version)` — 创建最小描述符
- `with_author(author)` — 设置作者
- `with_description(desc)` — 设置描述
- `with_capability(index, cap)` — 添加能力
- `with_config_schema(schema)` — 设置配置 schema

### AgentError

```rust
pub enum AgentError {
    InitFailed(HString<128>),
    IpcError(HString<128>),
    StorageError(HString<128>),
    ConfigError(HString<128>),
    NotSupported,
    InvalidState,
    Timeout,
}
```

### Context

```rust
pub struct Context {
    pub agent_id: AgentId,
    pub config: Config,
    pub ipc: IpcClient,
    pub logger: Logger,
    pub storage: Storage,
}
```

### IpcClient

```rust
impl IpcClient {
    pub fn new() -> Self;
    pub fn with_endpoint(self, name: &str) -> Self;
    pub fn send(&mut self, target: &str, msg: &[u8]) -> Result<(), AgentError>;
    pub fn recv(&mut self) -> Result<Vec<u8>, AgentError>;
    pub fn request(&mut self, target: &str, req: &[u8]) -> Result<Vec<u8>, AgentError>;
    pub fn send_count(&self) -> u64;
    pub fn recv_count(&self) -> u64;
    pub fn endpoint(&self) -> &str;
}
```

**常量**：`MAX_MSG_SIZE = 4096`

### Storage

```rust
impl Storage {
    pub fn new() -> Self;
    pub fn read(&self, key: &str) -> Result<Vec<u8>, AgentError>;
    pub fn write(&mut self, key: &str, data: &[u8]) -> Result<(), AgentError>;
    pub fn delete(&mut self, key: &str) -> Result<(), AgentError>;
    pub fn exists(&self, key: &str) -> bool;
    pub fn count(&self) -> usize;
}
```

**常量**：
- `MAX_KEY_LEN = 128`
- `MAX_VALUE_LEN = 65536`
- `MAX_ENTRIES = 64`

### Logger

```rust
impl Logger {
    pub fn new(agent_id: AgentId) -> Self;
    pub fn with_min_level(self, level: LogLevel) -> Self;
    pub fn log(&mut self, level: LogLevel, msg: &str);
    pub fn trace(&mut self, msg: &str);
    pub fn debug(&mut self, msg: &str);
    pub fn info(&mut self, msg: &str);
    pub fn warn(&mut self, msg: &str);
    pub fn error(&mut self, msg: &str);
    pub fn log_count(&self) -> u64;
    pub fn last_message(&self) -> &str;
}
```

### LogLevel

```rust
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}
```

### Config

```rust
impl Config {
    pub fn new() -> Self;
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), AgentError>;
    pub fn get_str(&self, key: &str) -> Result<&str, AgentError>;
    pub fn get_u32(&self, key: &str) -> Result<u32, AgentError>;
    pub fn get_u64(&self, key: &str) -> Result<u64, AgentError>;
    pub fn get_bool(&self, key: &str) -> Result<bool, AgentError>;
    pub fn count(&self) -> usize;
    pub fn contains(&self, key: &str) -> bool;
}
```

**常量**：`MAX_CONFIG_ENTRIES = 32`

### LifecycleState

```rust
pub enum LifecycleState {
    Created,
    Initialized,
    Running,
    Stopped,
    Destroyed,
    Error,
}
```

### LifecycleManager

```rust
impl LifecycleManager {
    pub fn new() -> Self;
    pub fn state(&self) -> LifecycleState;
    pub fn transition_to(&mut self, target: LifecycleState) -> bool;
    pub fn transition_count(&self) -> u64;
    pub fn is_running(&self) -> bool;
    pub fn is_destroyed(&self) -> bool;
    pub fn is_error(&self) -> bool;
}
```

## 宏

### register_agent!

```rust
register_agent!(AgentType, "agent-name");
```

导出 C ABI 函数：
- `eneros_agent_create()` — 创建 Agent 实例
- `eneros_agent_descriptor()` — 获取描述符
- `eneros_agent_destroy(ptr)` — 销毁实例

## 常量

| 常量 | 值 | 说明 |
|------|-----|------|
| `SDK_VERSION` | "1.0.0" | SDK 版本 |
| `MAX_MSG_SIZE` | 4096 | IPC 消息最大长度 |
| `MAX_KEY_LEN` | 128 | 存储键最大长度 |
| `MAX_VALUE_LEN` | 65536 | 存储值最大长度 |
| `MAX_ENTRIES` | 64 | 存储最大条目数 |
| `MAX_CONFIG_ENTRIES` | 32 | 配置最大条目数 |
