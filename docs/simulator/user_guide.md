# EnerOS 模拟器使用指南 (v0.167.0)

> **适用对象**：Agent 开发者  
> **前置要求**：Rust nightly、QEMU >= 7.0（ARM64 模拟）

---

## 1. 概述

EnerOS 模拟器让开发者在 PC 上无需 Edge Box 硬件即可开发、调试、测试完整 Agent。

核心能力：
- QEMU ARM64 虚拟机管理
- 6 种工业设备模拟（Modbus RTU/TCP、IEC 104、CAN、存储、网络）
- GDB 调试器集成（断点/单步/寄存器/内存查看）

## 2. 环境准备

### 2.1 安装 QEMU

```bash
# Ubuntu/Debian
sudo apt install qemu-system-aarch64

# 验证
qemu-system-aarch64 --version
```

### 2.2 构建 EnerOS 镜像

```bash
cd eneros/
make build
# 产出: build/eneros.bin
```

## 3. 快速开始

### 3.1 创建模拟器实例

```rust
use eneros_simulator::{QemuSimulator, Simulator, SimConfig, Arch};

let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
let config = SimConfig::new()
    .with_arch(Arch::Arm64)
    .with_memory(256)
    .with_kernel("build/eneros.bin".into());

let id = sim.create_instance(&config).unwrap();
```

### 3.2 启动/停止

```rust
sim.start(id).unwrap();   // 派生 QEMU 进程
sim.stop(id).unwrap();    // 终止 QEMU 进程
```

### 3.3 附加调试器

```rust
let mut dbg = sim.attach_debugger(id).unwrap();
dbg.connect().unwrap();

// 添加断点
let bp = dbg.add_breakpoint(0x8000_0000).unwrap();

// 继续执行
dbg.continue_exec().unwrap();

// 单步
dbg.step().unwrap();

// 查看寄存器
let regs = dbg.read_registers().unwrap();
println!("PC = {:#x}", regs.pc);
```

## 4. 设备模拟

### 4.1 Modbus RTU

```rust
use eneros_simulator::device_mock::{DeviceMock, ModbusRtuMock};
use eneros_simulator::types::{DeviceMockConfig, DeviceType};

let config = DeviceMockConfig::new(DeviceType::ModbusRtu, 1, 100, "meter-1".into());
let mut dev = ModbusRtuMock::new(&config);
dev.init().unwrap();

dev.write_register(10, 220).unwrap();  // 写入电压值
let v = dev.read_register(10).unwrap(); // 读取
```

### 4.2 IEC 104

```rust
use eneros_simulator::device_mock::Iec104Mock;

let config = DeviceMockConfig::new(DeviceType::Iec104, 1, 20, "rtu-1".into());
let mut dev = Iec104Mock::new(&config);
dev.init().unwrap();

dev.set_telemetry(0, 500).unwrap();    // 设置遥测值
dev.set_teleindication(0, true).unwrap(); // 设置遥信
```

### 4.3 CAN

```rust
use eneros_simulator::device_mock::{CanMock, CanFrame};

let config = DeviceMockConfig::new(DeviceType::Can, 0x10, 0, "bms".into());
let mut dev = CanMock::new(&config);
dev.init().unwrap();

let frame = CanFrame { id: 0x123, dlc: 4, data: [1,2,3,4,0,0,0,0] };
dev.send_frame(frame).unwrap();
```

### 4.4 批量设备注册

```rust
use eneros_simulator::device_mock::DeviceMockRegistry;

let configs = vec![
    DeviceMockConfig::new(DeviceType::ModbusRtu, 1, 100, "meter".into()),
    DeviceMockConfig::new(DeviceType::Network, 1, 0, "eth0".into()),
];
let registry = DeviceMockRegistry::from_configs(&configs).unwrap();
assert_eq!(registry.total_count(), 2);
```

## 5. 配置参考

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `arch` | `Arm64` | 目标架构 |
| `memory_mb` | `256` | 虚拟机内存 |
| `kernel_image` | `build/eneros.bin` | 内核镜像路径 |
| `gdb_base_port` | `1234` | GDB 端口（实例端口 = base + id） |

## 6. 故障排查

| 问题 | 原因 | 解决 |
|------|------|------|
| `QemuNotFound` | QEMU 未安装 | `apt install qemu-system-aarch64` |
| `ImageNotFound` | 镜像路径错误 | 先执行 `make build` |
| `ProcessSpawnFailed` | 权限/路径问题 | 检查 QEMU 可执行权限 |
| 调试器连接失败 | 实例未运行 | 先 `start(id)` |
