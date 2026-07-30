# EnerOS Driver SDK 开发指南

> **版本**：v1.0.0 (v0.165.0)

## 1. 概述

Driver SDK 用于开发硬件驱动插件，支持 UART、SPI、I2C、GPIO、CAN 等设备。

## 2. 快速开始

```rust
use eneros_driver_sdk::{
    register_driver, DeviceClass, DeviceInfo, DriverConfig, DriverError, DriverPlugin,
};

struct MyDriver;
impl MyDriver { fn new() -> Self { Self } }

impl DriverPlugin for MyDriver {
    fn name(&self) -> &str { "my-driver" }
    fn version(&self) -> &str { "1.0.0" }
    fn probe(&mut self, info: &DeviceInfo) -> Result<bool, DriverError> {
        Ok(info.vendor_id == 0x1234)
    }
    fn init(&mut self, _config: &DriverConfig) -> Result<(), DriverError> { Ok(()) }
    fn read(&mut self, _reg: u32, buf: &mut [u8]) -> Result<(), DriverError> {
        buf.fill(0); Ok(())
    }
    fn write(&mut self, _reg: u32, _data: &[u8]) -> Result<(), DriverError> { Ok(()) }
    fn deinit(&mut self) -> Result<(), DriverError> { Ok(()) }
}

register_driver!(MyDriver);
```

## 3. 核心 API

### DriverPlugin trait

| 方法 | 说明 |
|------|------|
| `name()` | 驱动名称 |
| `version()` | 驱动版本 |
| `probe(info)` | 探测设备（返回是否匹配） |
| `init(config)` | 初始化 |
| `read(reg, buf)` | 读寄存器 |
| `write(reg, data)` | 写寄存器 |
| `deinit()` | 去初始化 |
| `handle_irq()` | 中断处理（可选） |
| `status()` | 驱动状态（可选） |

### 设备类别

`Network` / `Storage` / `Serial` / `Gpio` / `I2c` / `Spi` / `Can` / `Timer` / `Analog` / `Power` / `Sensor` / `Custom`

### 驱动状态

`Unknown` → `Probed` → `Initialized` → `Running` → `Deinitialized`

## 4. 配置示例

```rust
let config = DriverConfig::new("uart0", 0x0900_0000)
    .with_irq(33)
    .with_clock(24_000_000);
```
