# EnerOS 模拟器开发文档 (v0.167.0)

> **子系统**：crates/agents/simulator（eneros-simulator）  
> **Phase 定位**：P3-J 生态开放终版本

---

## 1. 架构

```text
┌─────────────────────────────────────────────────────────┐
│  eneros-simulator                                       │
│                                                         │
│  types.rs ─── 核心类型（no_std 兼容）                    │
│     │                                                   │
│  qemu.rs ──── QEMU 后端（Simulator trait + 进程管理）    │
│     │                                                   │
│  device_mock.rs ── 设备模拟（6 种设备 + Registry）       │
│     │                                                   │
│  debugger.rs ── GDB 调试器（断点/单步/寄存器）           │
│                                                         │
│  依赖: eneros-agent-sdk (v0.164.0)                      │
└─────────────────────────────────────────────────────────┘
```

## 2. 模块职责

| 模块 | 文件 | 职责 |
|------|------|------|
| types | `src/types.rs` | SimConfig, SimInstance, Arch, DeviceType 等核心类型 |
| qemu | `src/qemu.rs` | QemuSimulator, Simulator trait, QemuCommand 构建 |
| device_mock | `src/device_mock.rs` | DeviceMock trait + 6 种设备实现 + Registry |
| debugger | `src/debugger.rs` | Debugger, Breakpoint, DebugEvent, RegisterState |
| lib | `src/lib.rs` | SimError 错误枚举 + re-export |

## 3. std 策略

本 crate 采用分层 std 策略：

| 层级 | 内容 | std 需求 |
|------|------|---------|
| 核心类型 | types.rs 全部 | 无（no_std + alloc） |
| 状态机 | Debugger 状态、SimState | 无 |
| 设备模拟 | DeviceMock 实现 | 无 |
| 进程管理 | QEMU spawn/kill | 需 `std` feature |
| 网络连接 | TCP 调试器连接 | 需 `std` feature |

```rust
// 宿主机专属代码门控
#[cfg(feature = "std")]
{
    use std::process::Command;
    let child = Command::new("qemu-system-aarch64").spawn()?;
}
```

## 4. 扩展设备模拟

### 4.1 实现 DeviceMock trait

```rust
use eneros_simulator::device_mock::DeviceMock;
use eneros_simulator::types::DeviceType;
use eneros_simulator::SimError;

pub struct MyDeviceMock {
    initialized: bool,
    // ... 设备状态
}

impl DeviceMock for MyDeviceMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }
    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        // 实现寄存器读取
        todo!()
    }
    fn write_register(&mut self, addr: u16, value: u16) -> Result<(), SimError> {
        // 实现寄存器写入
        todo!()
    }
    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized)
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::ModbusRtu // 选择最接近的类型
    }
    fn name(&self) -> &str {
        "my-device"
    }
}
```

### 4.2 注册到 Registry

在 `DeviceMockRegistry` 中添加新字段和匹配分支。

## 5. 测试策略

- 所有测试使用 mock 模式（`mock_start`/`mock_stop`），不依赖 QEMU 安装
- 设备模拟测试验证寄存器读写边界
- 调试器测试验证状态机转换
- 33 个单元测试 + 1 个文档测试

## 6. 性能指标

| 指标 | 目标 |
|------|------|
| 模拟器启动 | < 5s |
| Agent 运行延迟 | 与真实硬件无显著差异 |
| 设备模拟开销 | < 1% CPU |

## 7. 文件清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/lib.rs` | ~120 | 入口 + 错误 |
| `src/types.rs` | ~300 | 核心类型 |
| `src/qemu.rs` | ~380 | QEMU 后端 |
| `src/device_mock.rs` | ~830 | 设备模拟 |
| `src/debugger.rs` | ~380 | 调试器 |
