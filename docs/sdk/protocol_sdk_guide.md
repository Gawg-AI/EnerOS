# EnerOS Protocol SDK 开发指南

> **版本**：v1.0.0 (v0.165.0)

## 1. 概述

Protocol SDK 用于开发设备协议插件，支持 Modbus、IEC 104、自定义协议等。

## 2. 快速开始

```rust
use eneros_protocol_sdk::{
    register_protocol, DataPoint, PointValue, ProtocolConfig, ProtocolError, ProtocolPlugin,
};

struct MyProtocol;
impl MyProtocol { fn new() -> Self { Self } }

impl ProtocolPlugin for MyProtocol {
    fn name(&self) -> &str { "my-protocol" }
    fn version(&self) -> &str { "1.0.0" }
    fn connect(&mut self, config: &ProtocolConfig) -> Result<(), ProtocolError> { Ok(()) }
    fn read_point(&mut self, id: &str) -> Result<DataPoint, ProtocolError> {
        Ok(DataPoint::new(id, PointValue::F32(0.0)))
    }
    fn write_point(&mut self, _id: &str, _v: PointValue) -> Result<(), ProtocolError> { Ok(()) }
    fn disconnect(&mut self) -> Result<(), ProtocolError> { Ok(()) }
}

register_protocol!(MyProtocol);
```

## 3. 核心 API

### ProtocolPlugin trait

| 方法 | 说明 |
|------|------|
| `name()` | 协议名称 |
| `version()` | 协议版本 |
| `connect(config)` | 连接设备 |
| `read_point(id)` | 读取测点 |
| `write_point(id, value)` | 写入测点 |
| `disconnect()` | 断开连接 |
| `read_batch(ids)` | 批量读取（可选） |
| `is_connected()` | 连接状态（可选） |

### 传输方式

- `Transport::Serial` — RS485/RS232
- `Transport::Tcp` — Modbus TCP
- `Transport::Can` — CAN 总线

### 测点值类型

`Bool` / `I16` / `U16` / `I32` / `U32` / `F32` / `F64` / `Bytes`

## 4. 配置示例

```rust
// TCP
let config = ProtocolConfig::tcp("192.168.1.100", 502)
    .with_timeout(2000)
    .with_slave_id(1);

// 串口
let config = ProtocolConfig::serial("/dev/ttyS0", 9600)
    .with_poll_interval(50);
```
