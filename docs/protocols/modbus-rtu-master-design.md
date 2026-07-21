# Modbus RTU 主站设计文档（v0.45.0）

> **版本**：v0.45.0
> **蓝图参考**：`蓝图/phase1.md` §7905-8201
> **前置版本**：v0.44.0（RS485 驱动）、v0.43.0（驱动框架）
> **后续版本**：v0.46.0（Modbus TCP，复用应用层）、v0.50.0（统一点表）
> **最后更新**：2026-07-15

---

## 1. 版本目标

基于 v0.44.0 RS485 串口驱动（`Rs485Driver::send()/recv()`）与 v0.43.0 驱动框架（`DeviceDriver` trait + `DriverError`）实现 Modbus RTU 主站协议栈，支持功能码 03/06/10 与点表映射，为 v0.50.0 统一点表提供数据源。

- **一句话目标**：实现 Modbus RTU 主站协议栈，支持功能码 03（读保持寄存器）/06（写单个寄存器）/10（写多个寄存器），提供 CRC16 校验、点表映射、轮询与超时重试。
- **架构定位**：P1-F 设备协议栈第三层——第一个工业协议实现，位于 RS485 物理链路层之上、应用层（Agent/业务）之下。
- **前置依赖**：
  - v0.44.0 RS485 驱动（`Rs485Driver` 提供 `send()`/`recv()`）
  - v0.43.0 驱动框架（`DeviceDriver` trait + `DriverError` 错误类型）
- **设计原则关联**：实时性（单次读写 <50ms@9600bps）、可靠性（CRC16 校验 + 超时重试 + 异常码处理）、可扩展性（应用层逻辑可被 v0.46.0 Modbus TCP 复用）。

## 2. 前置依赖

| 依赖版本 | 依赖产出 | 用途 |
|---------|---------|------|
| v0.44.0 | `Rs485Driver`（`send`/`recv` 方法） | 底层传输层，承载 Modbus RTU 帧的物理收发 |
| v0.43.0 | `DeviceDriver` trait + `DriverError` | 驱动框架，提供错误类型与驱动生命周期模型 |
| v0.7.0 | HAL Serial + GPIO | 硬件寄存器访问（经 `Rs485Driver` 间接使用） |

> **依赖关系说明**：`Rs485Driver`（v0.44.0）的方法签名 `send(&mut self, &[u8]) -> Result<(), DriverError>` 与 `recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError>` 与本版本定义的 `RtuTransport` trait（D1）方法签名一致，因此 `Rs485Driver` 自动满足 `RtuTransport` 接口，无需适配代码。

## 3. 交付物清单

| 类型 | 交付物 | 路径 |
|------|--------|------|
| 代码 crate | `eneros-modbus-rtu` | `crates/protocols/modbus-rtu/` |
| 接口 | `ModbusRtuMaster` 主站对象 | `crates/protocols/modbus-rtu/src/master.rs` |
| 接口 | `ModbusFrame` 帧结构 | `crates/protocols/modbus-rtu/src/frame.rs` |
| 接口 | `ModbusRequest` / `ModbusResponse` | `crates/protocols/modbus-rtu/src/request.rs` |
| 接口 | `PointMapping` / `RegToPoint` | `crates/protocols/modbus-rtu/src/point.rs` |
| 接口 | `RtuTransport` trait（D1） | `crates/protocols/modbus-rtu/src/master.rs` |
| 接口 | `FunctionCode` / `ExceptionCode` 枚举 | `crates/protocols/modbus-rtu/src/request.rs` |
| 接口 | `ModbusError` 错误枚举（D2） | `crates/protocols/modbus-rtu/src/error.rs` |
| 接口 | `ModbusStats` 统计结构（D3） | `crates/protocols/modbus-rtu/src/master.rs` |
| 接口 | `ModbusDataType` / `AccessMode` 枚举（D4） | `crates/protocols/modbus-rtu/src/point.rs` |
| 算法 | CRC-16/MODBUS 实现 | `crates/protocols/modbus-rtu/src/crc.rs` |
| 测试桩 | `MockRtuTransport` | `crates/protocols/modbus-rtu/src/mock.rs` |
| 测试 | CRC16 已知向量 + 帧编解码 + 主站收发 + 点表转换 | 各模块单元/集成测试 |
| 文档 | 本设计文档 | `docs/protocols/modbus-rtu-master-design.md` |

## 4. 详细设计

### 4.1 CRC-16/MODBUS 算法

CRC-16/MODBUS 是 Modbus RTU 帧的校验算法，参数如下：

| 参数 | 值 |
|------|-----|
| 多项式 | `0xA001`（即 `0x8005` 的位反转） |
| 初始值 | `0xFFFF` |
| 输入反射 | 是 |
| 输出反射 | 是 |
| 异或输出 | `0x0000` |
| 输出字节序 | 低字节在前（LE） |

`crc.rs` 模块提供两个函数：

```rust
/// 计算 CRC-16/MODBUS 校验值
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// 将 CRC16 以低字节在前（LE）追加到缓冲区末尾
pub fn append_crc(buf: &mut Vec<u8>) {
    let crc = crc16(buf);
    buf.push((crc & 0xFF) as u8);       // 低字节
    buf.push((crc >> 8) as u8);         // 高字节
}
```

**已知测试向量**：

| 输入 | 输出 CRC | 帧末尾字节（LE） |
|------|---------|-----------------|
| `[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]` | `0x840A` | `0x0A, 0x84` |
| `&[]`（空） | `0xFFFF`（初始值） | `0xFF, 0xFF` |
| `[0x01, 0x06, 0x00, 0x64, 0xAB, 0xCD]` | （由实现计算） | LE 输出 |

### 4.2 ModbusFrame 帧结构

`ModbusFrame` 是 Modbus RTU 帧的内存表示，结构为 `| SlaveAddr(1) | FuncCode(1) | Data(N) | CRC16(2 LE) |`：

```rust
pub struct ModbusFrame {
    pub slave_addr: u8,    // 从站地址（0=广播，1-247=有效从站）
    pub func_code: u8,     // 功能码（0x03/0x06/0x10 等）
    pub data: Vec<u8>,     // 数据域（含子功能码与参数）
    pub crc: u16,          // CRC16 校验值
}
```

**编码方法** `encode(&self) -> Vec<u8>`：将帧序列化为 `[SlaveAddr][FuncCode][Data(N)][CRC16(LE)]` 字节流，编码流程：
1. 构造 `buf = [slave_addr, func_code] ++ data`
2. 调用 `append_crc(&mut buf)` 追加 CRC16（LE）
3. 返回 `buf`

**解码方法** `decode(buf: &[u8]) -> Result<Self, ModbusError>`：从字节流解析帧，校验：
1. 长度校验：`buf.len() < 4` → `Err(FrameTooShort)`（最小帧 = 地址+功能码+CRC = 4 字节）
2. CRC 校验：分离末尾 2 字节为 CRC（LE），对前 `len-2` 字节重新计算 CRC，不匹配 → `Err(CrcMismatch)`
3. 解析：`slave_addr = buf[0]`、`func_code = buf[1]`、`data = buf[2..len-2]`、`crc = u16::from_le_bytes([buf[len-2], buf[len-1]])`

**场景示例**：

| 场景 | 输入 | 输出 |
|------|------|------|
| 编码帧 | `ModbusFrame { slave_addr: 1, func_code: 3, data: vec![0x00,0x00,0x00,0x01], crc: 0 }` | 8 字节，末尾 2 字节为 `0x0A, 0x84` |
| 解码帧 — CRC 失败 | `[0x01, 0x03, 0x02, 0x00, 0x00, 0xFF, 0xFF]` | `Err(CrcMismatch)` |
| 解码帧 — 帧过短 | `[0x01, 0x03]` | `Err(FrameTooShort)` |

### 4.3 功能码与请求/响应

#### 4.3.1 FunctionCode 枚举（D9）

蓝图列出 6 个功能码，本版本仅实现 03/06/10 的编解码，01/04/05 返回 `UnsupportedFunction`（保留枚举完整性以便 v0.46.0+ 扩展）：

```rust
#[repr(u8)]
pub enum FunctionCode {
    ReadCoils              = 0x01, // 保留（未实现）
    ReadHoldingRegisters   = 0x03, // ✅ 本版本实现
    ReadInputRegisters     = 0x04, // 保留（未实现）
    WriteSingleCoil        = 0x05, // 保留（未实现）
    WriteSingleRegister    = 0x06, // ✅ 本版本实现
    WriteMultipleRegisters = 0x10, // ✅ 本版本实现
}
```

#### 4.3.2 ModbusRequest 枚举（3 变体）

```rust
pub enum ModbusRequest {
    /// 功能码 03：读保持寄存器（quantity ≤ 125）
    ReadHoldingRegisters {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 功能码 06：写单个寄存器
    WriteSingleRegister {
        slave_addr: u8,
        reg_addr: u16,
        value: u16,
    },
    /// 功能码 10：写多个寄存器（values.len() ≤ 123）
    WriteMultipleRegisters {
        slave_addr: u8,
        start_addr: u16,
        values: Vec<u16>,
    },
}
```

**请求帧数据域编码**（BE = 大端）：

| 请求类型 | 数据域 |
|---------|--------|
| ReadHoldingRegisters | `[start_addr BE(2)][quantity BE(2)]` |
| WriteSingleRegister | `[reg_addr BE(2)][value BE(2)]` |
| WriteMultipleRegisters | `[start_addr BE(2)][quantity BE(2)][byte_count(1)][values BE(N*2)]` |

**场景示例**：`ReadHoldingRegisters { slave_addr: 1, start_addr: 0, quantity: 10 }` → `build_frame()` 数据域 = `[0x00, 0x00, 0x00, 0x0A]`。

#### 4.3.3 ModbusResponse 枚举（5 变体含 Broadcast）

```rust
pub enum ModbusResponse {
    /// 功能码 03 响应：返回读取的寄存器值
    ReadHoldingRegisters(Vec<u16>),
    /// 功能码 06 响应：回显地址与值
    WriteSingleRegister { addr: u16, value: u16 },
    /// 功能码 10 响应：回显起始地址与数量
    WriteMultipleRegisters { start_addr: u16, quantity: u16 },
    /// 异常响应（功能码最高位置 1）
    Error { exception_code: ExceptionCode },
    /// 广播响应（D10：广播地址 0 写操作无响应）
    Broadcast,
}
```

#### 4.3.4 ExceptionCode 枚举（6 异常码）

```rust
#[repr(u8)]
pub enum ExceptionCode {
    IllegalFunction      = 0x01,
    IllegalDataAddress   = 0x02,
    IllegalDataValue     = 0x03,
    SlaveDeviceFailure   = 0x04,
    Acknowledge          = 0x05,
    SlaveDeviceBusy      = 0x06,
}
```

异常响应帧：`func_code = 原功能码 | 0x80`，`data = [exception_code]`。

### 4.4 RtuTransport trait（D1）

蓝图假设 `ModbusRtuMaster` 持有 `DriverHandle`（v0.43.0 能力令牌），但 `DriverHandle` 无 `send()`/`recv()` 方法。因此定义 `RtuTransport` trait 解耦主站与 RS485 驱动：

```rust
/// RTU 传输层抽象（D1）
/// Rs485Driver 自动满足此接口（方法签名一致），便于 mock 测试
pub trait RtuTransport {
    /// 发送字节流到总线
    fn send(&mut self, data: &[u8]) -> Result<(), DriverError>;
    /// 接收字节流（阻塞至超时或收到帧间隔）
    fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError>;
}
```

**设计要点**：
- `Rs485Driver`（v0.44.0）的 `send`/`recv` 方法签名与此 trait 完全一致，自动满足，无需适配代码。
- 主站持 `&mut dyn RtuTransport`，可在测试中替换为 `MockRtuTransport`。
- 该 trait 为 v0.46.0 Modbus TCP 复用应用层逻辑预留扩展点（TCP 版本将提供不同的 `RtuTransport` 实现）。

### 4.5 ModbusRtuMaster 主站

```rust
pub struct ModbusRtuMaster<'a> {
    transport: &'a mut dyn RtuTransport,  // D1: 传输层抽象
    timeout_ms: u32,                       // 响应超时
    retry_count: u8,                       // 重试次数
    stats: ModbusStats,                    // D3: 收发统计
}
```

**核心方法**：

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(transport, timeout_ms, retry_count) -> Self` | 构造主站 |
| `read_holding_registers` | `(slave_addr, start_addr, quantity) -> Result<Vec<u16>, ModbusError>` | 功能码 03 |
| `write_single_register` | `(slave_addr, reg_addr, value) -> Result<(), ModbusError>` | 功能码 06 |
| `write_multiple_registers` | `(slave_addr, start_addr, values) -> Result<(), ModbusError>` | 功能码 10 |
| `build_frame` | `(slave_addr, &request) -> Vec<u8>`（D7） | 编码请求帧 + CRC |
| `parse_response` | `(&request, &frame) -> Result<ModbusResponse, ModbusError>`（D7） | 解码响应帧 + 校验 |
| `send_request_with_retry` | `(slave_addr, &request) -> Result<ModbusResponse, ModbusError>` | 带超时重试的收发（含广播处理 D10） |
| `poll_points` | `(&PointMapping) -> Vec<(u32, Result<f64, ModbusError>)>` | 按点表轮询读取 |

**`build_frame()` 实现（D7）**：
1. 根据 `ModbusRequest` 变体编码数据域（功能码 + 参数，BE 编码）
2. 构造 `ModbusFrame { slave_addr, func_code, data, crc: 0 }`
3. 调用 `frame.encode()` 返回带 CRC 的字节流

**`parse_response()` 实现（D7）**：
1. 调用 `ModbusFrame::decode(buf)` 校验长度与 CRC
2. 校验从站地址匹配 → 否则 `Err(AddrMismatch)`
3. 校验功能码匹配（异常响应功能码 = 原功能码 | 0x80）
4. 若为异常响应 → `Ok(ModbusResponse::Error { exception_code })`
5. 按 `ModbusRequest` 变体解析数据域为对应 `ModbusResponse`

**`send_request_with_retry()` 流程**（含广播处理 D10）：
1. 若 `slave_addr == 0`（广播）：`build_frame()` → `transport.send()` → 不等待响应 → 返回 `Ok(Broadcast)`；`stats.request_count++`
2. 否则：循环 `0..=retry_count` 次：
   - `build_frame()` → `transport.send()` → `stats.request_count++`
   - `transport.recv(timeout_ms)`：
     - `Ok(frame)` → `parse_response()` → 成功返回，`stats.response_count++`
     - 超时 → `stats.timeout_count++`，继续重试
   - 重试耗尽 → `Err(MaxRetryExceeded)`

### 4.6 点表映射

#### 4.6.1 ModbusDataType 枚举（5 变体）

```rust
pub enum ModbusDataType {
    U16,        // 16 位无符号（1 寄存器）
    I16,        // 16 位有符号（1 寄存器）
    U32,        // 32 位无符号（2 寄存器）
    F32,        // 32 位浮点（2 寄存器，IEEE 754）
    Bit(u8),    // 位（寄存器内的位索引 0-15，1 寄存器）
}
```

#### 4.6.2 AccessMode 枚举（D4）

```rust
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}
```

#### 4.6.3 RegToPoint 结构

```rust
pub struct RegToPoint {
    pub point_id: u32,          // 点位 ID（全局唯一）
    pub point_name: String,     // 点位名称
    pub slave_addr: u8,         // 从站地址
    pub reg_addr: u16,          // 寄存器起始地址
    pub data_type: ModbusDataType,
    pub scale: f64,             // 缩放系数
    pub offset: f64,            // 偏移量
    pub access: AccessMode,
}

pub struct PointMapping {
    pub mappings: Vec<RegToPoint>,
}
```

#### 4.6.4 RegToPoint 方法（D5）

```rust
impl RegToPoint {
    /// 返回所需寄存器数（D5）
    pub fn word_count(&self) -> u16 {
        match self.data_type {
            ModbusDataType::U16 | ModbusDataType::I16 | ModbusDataType::Bit(_) => 1,
            ModbusDataType::U32 | ModbusDataType::F32 => 2,
        }
    }

    /// 将原始寄存器值转换为工程值（D5）
    /// 转换公式：value = raw * scale + offset
    pub fn convert(&self, regs: &[u16]) -> Result<f64, ModbusError> {
        let raw = match self.data_type {
            ModbusDataType::U16 => regs.first().copied().unwrap_or(0) as f64,
            ModbusDataType::I16 => (regs.first().copied().unwrap_or(0) as i16) as f64,
            ModbusDataType::U32 => {
                // 大端拼接：高字在前
                ((regs[0] as u32) << 16) | (regs[1] as u32)
            }.max(0) as f64, // 注意：U32 转 f64
            ModbusDataType::F32 => {
                let bits = ((regs[0] as u32) << 16) | (regs[1] as u32);
                f32::from_bits(bits) as f64
            }
            ModbusDataType::Bit(idx) => {
                let reg = regs.first().copied().unwrap_or(0);
                ((reg >> idx) & 0x0001) as f64
            }
        };
        Ok(raw * self.scale + self.offset)
    }
}
```

**转换场景**：

| 数据类型 | 输入寄存器 | scale | offset | 输出 |
|---------|----------|-------|--------|------|
| U16 | `[0x0064]` | 0.1 | 0.0 | `10.0`（100×0.1） |
| F32（大端） | `[0x41A0, 0x0000]` | 1.0 | 0.0 | `20.0`（0x41A00000 = 20.0） |
| I16 | `[0xFF9C]` | 1.0 | 0.0 | `-100.0`（0xFF9C = -100） |

#### 4.6.5 group_by_slave() 辅助函数（D6）

```rust
/// 按从站地址分组点位（D6）
/// 用于 poll_points() 优化：同一从站的点位合并为一次 ReadHoldingRegisters 请求
pub fn group_by_slave(mappings: &[RegToPoint]) -> Vec<(u8, Vec<&RegToPoint>)> {
    // 按 slave_addr 排序后分组
    // 返回 Vec<(slave_addr, 该从站的点位引用列表)>
}
```

**`poll_points()` 流程**：
1. 调用 `group_by_slave()` 按从站分组
2. 对每个从站：按 `reg_addr` 排序，合并连续寄存器区间为单次读请求
3. 调用 `read_holding_registers()` 读取
4. 对每个点位调用 `RegToPoint::convert()` 转换为工程值
5. 返回 `Vec<(point_id, Result<f64, ModbusError>)>`

### 4.7 偏差声明表（D1~D10）

> 以下偏差与 `.trae/specs/develop-v0450-modbus-rtu-master/spec.md` §偏差声明一致。

| 偏差 | 蓝图假设 | 实际情况 | 处理方案 |
|------|---------|---------|---------|
| **D1** | `ModbusRtuMaster` 持有 `rs485: DriverHandle` | `DriverHandle`（v0.43.0）是能力令牌，无 `send()`/`recv()` 方法 | 定义 `RtuTransport` trait（`send`/`recv` 两方法），`Rs485Driver` 自动满足；主站持 `&mut dyn RtuTransport`，便于 mock 测试 |
| **D2** | `ModbusError` 枚举被引用但未定义 | 蓝图未给出完整定义 | 定义 `ModbusError` 枚举：`FrameTooShort`/`CrcMismatch`/`AddrMismatch`/`UnexpectedResponse`/`Exception(ExceptionCode)`/`Driver(DriverError)`/`MaxRetryExceeded`/`InvalidSlaveAddr`/`InvalidQuantity`/`InvalidRegisterAddr`/`UnsupportedFunction` |
| **D3** | `ModbusStats` 被引用但未定义 | 蓝图未给出定义 | 定义 `ModbusStats`：`request_count`/`response_count`/`error_count`/`timeout_count`/`crc_error_count` + `Default` |
| **D4** | `AccessMode` 被引用但未定义 | 蓝图未给出定义 | 定义 `AccessMode` 枚举：`ReadOnly`/`WriteOnly`/`ReadWrite` |
| **D5** | `RegToPoint::word_count()` / `convert()` 被引用但未定义 | 蓝图未给出实现 | `word_count()` 按 `data_type` 返回寄存器数（U16/I16/Bit=1, U32/F32=2）；`convert(regs)` 按 `data_type`+`scale`+`offset` 转换为 `f64` |
| **D6** | `group_by_slave()` 被引用但未定义 | 蓝图未给出实现 | 在 `point.rs` 内实现 `group_by_slave()` 辅助函数，返回 `Vec<(u8, Vec<&RegToPoint>)>` |
| **D7** | `build_frame()` / `parse_response()` 被引用但未定义 | 蓝图未给出实现 | 在 `master.rs` 内实现：`build_frame(slave_addr, &request) -> Vec<u8>`（编码+CRC）；`parse_response(&request, &frame) -> Result<ModbusResponse, ModbusError>`（解码+校验） |
| **D8** | crate 名 `modbus-rtu-master` 放于不确定位置 | §2.3.1 要求所有 crate 放 `crates/<subsystem>/` | 放入 `crates/protocols/modbus-rtu/`（crate 名 `eneros-modbus-rtu`）；`crates/protocols/` 为设备协议栈子系统 |
| **D9** | `FunctionCode` 枚举列 6 个功能码（01/03/04/05/06/10），但版本目标仅要求 03/06/10 | 部分功能码非本版本必需 | 枚举包含全部 6 个变体（9.7 可扩展），但 `build_frame()`/`parse_response()` 仅实现 03/06/10 编解码；01/04/05 返回 `ModbusError::UnsupportedFunction` |
| **D10** | 蓝图未明确广播地址 0 的处理 | 风险 §8.4 提及"广播地址 0 写操作无响应" | `send_request_with_retry()` 中：若 `slave_addr == 0`，发送后不等待响应，直接返回 `Ok(ModbusResponse::Broadcast)` |

## 5. 收发流程

### 5.1 发送流程（单播读/写）

```
应用调用 read_holding_registers(slave_addr, start_addr, quantity) →
  1. 构造 ModbusRequest::ReadHoldingRegisters
  2. build_frame(slave_addr, &request)
     ├─ 编码数据域 [start_addr BE][quantity BE]
     ├─ 构造 ModbusFrame { slave_addr, func_code: 0x03, data, crc: 0 }
     └─ frame.encode() → 追加 CRC16(LE)
  3. transport.send(frame_bytes)
     └─ stats.request_count++
  4. transport.recv(timeout_ms)
     ├─ Ok(buf) → 进入步骤 5
     └─ Err(Timeout) → 进入超时重试流程（§5.3）
  5. parse_response(&request, &buf)
     ├─ ModbusFrame::decode(buf) → 校验长度(≥4) + CRC
     ├─ 校验 slave_addr 匹配 → 否则 Err(AddrMismatch)
     ├─ 校验 func_code 匹配（含异常位 0x80）
     ├─ 若异常响应 → Err(Exception(exception_code))
     └─ 解析数据域 → Ok(ModbusResponse::ReadHoldingRegisters(vec))
  6. stats.response_count++
  7. 返回 Ok(vec)
```

### 5.2 广播流程（D10）

```
应用调用 write_multiple_registers(0, start_addr, &values) →  // slave_addr=0
  1. 构造 ModbusRequest::WriteMultipleRegisters
  2. build_frame(0, &request) → 带广播地址 0 的帧
  3. transport.send(frame_bytes)
     └─ stats.request_count++
  4. 检测 slave_addr == 0 → 不等待响应（D10）
     └─ 不调用 transport.recv()
  5. 返回 Ok(ModbusResponse::Broadcast)
     └─ stats.response_count 不递增
```

> **注意**：广播仅支持写操作（功能码 06/10）。读操作（功能码 03）使用广播地址无意义，应在 `build_frame()` 前校验并返回 `Err(InvalidSlaveAddr)`。

### 5.3 超时重试流程

```
send_request_with_retry(slave_addr, &request) →  // retry_count = N
  for attempt in 0..=N:
    1. build_frame(slave_addr, &request)
    2. transport.send(frame_bytes)
       └─ stats.request_count++
    3. match transport.recv(timeout_ms):
       ├─ Ok(buf):
       │   ├─ parse_response 成功 → stats.response_count++; return Ok(response)
       │   ├─ Err(CrcMismatch) → stats.crc_error_count++; 继续重试
       │   └─ Err(Exception) → stats.error_count++; return Err(Exception)
       └─ Err(Timeout) → stats.timeout_count++; 继续重试
  重试耗尽 → return Err(MaxRetryExceeded)
```

**重试策略说明**：
- CRC 错误视为可重试错误（总线噪声）
- 异常码响应（`ExceptionCode`）不重试（从站明确拒绝，重试无意义）
- 超时按 `retry_count` 次重试，耗尽后返回 `MaxRetryExceeded`

## 6. 测试计划

### 6.1 单元测试

| 模块 | 测试内容 | 测试向量 |
|------|---------|---------|
| `crc.rs` | CRC16 已知向量 | `[0x01,0x03,0x00,0x00,0x00,0x01]` → `0x840A`；空切片 → `0xFFFF` |
| `crc.rs` | `append_crc` 字节序 | 追加后末尾 2 字节为 LE（低字节在前） |
| `frame.rs` | `encode()` 编码帧 | 8 字节输出，末尾 2 字节为 CRC16(LE) |
| `frame.rs` | `decode()` CRC 失败 | `[0x01,0x03,0x02,0x00,0x00,0xFF,0xFF]` → `Err(CrcMismatch)` |
| `frame.rs` | `decode()` 帧过短 | `[0x01,0x03]` → `Err(FrameTooShort)` |
| `frame.rs` | `decode()` 正常帧 | 完整帧解码字段正确 |
| `request.rs` | `FunctionCode` 枚举值 | 6 变体 `repr(u8)` 值正确 |
| `request.rs` | `ExceptionCode` 枚举值 | 6 异常码值正确 |
| `point.rs` | U16 转换 | `[0x0064]`, scale=0.1 → `10.0` |
| `point.rs` | F32 转换（大端） | `[0x41A0,0x0000]` → `20.0` |
| `point.rs` | I16 转换 | `[0xFF9C]` → `-100.0` |
| `point.rs` | `word_count()` | U16=1, U32=2, F32=2, Bit=1 |
| `point.rs` | `group_by_slave()` | 多点位按从站分组正确 |

### 6.2 集成测试

| 测试 | 描述 | 预期 |
|------|------|------|
| 读保持寄存器 | `MockRtuTransport` 预填充合法响应 | `Ok(vec![u16; 5])`；`request_count`/`response_count` 各+1 |
| 写单个寄存器 | `MockRtuTransport` 预填充回显响应 | `Ok(())`；统计递增 |
| 写多个寄存器 | `MockRtuTransport` 预填充回显响应 | `Ok(())`；统计递增 |
| 超时重试 | `set_recv_timeout(true)`，`retry_count=2` | `Err(MaxRetryExceeded)`；`timeout_count` +3 |
| 广播写（D10） | `slave_addr=0` 写操作 | `Ok(Broadcast)`；`response_count` 不递增 |
| 异常码响应 | 预填充异常帧（`0x83, 0x02`） | `Err(Exception(IllegalDataAddress))` |
| CRC 错误 | 预填充 CRC 错误帧 | 触发重试或 `Err(CrcMismatch)` |
| 地址不匹配 | 预填充其他从站响应 | `Err(AddrMismatch)` |
| 轮询点表 | `PointMapping` 含 2 点位（同从站） | 返回 `Vec` 长度 2 |

### 6.3 性能基准

| 基准 | 目标 | 测试方法 |
|------|------|---------|
| 单次读保持寄存器 | <50ms@9600bps | `MockRtuTransport` 模拟时序 + 帧长计算 |
| 单次写单个寄存器 | <50ms@9600bps | 同上 |
| 单次写多个寄存器（10 个） | <80ms@9600bps | 帧长增加，传输时间线性增长 |
| CRC16 计算（8 字节） | <100μs | 主机侧基准测试 |

**9600bps 时序估算**：
- 读请求帧 8 字节 = 8×10bit/9600 ≈ 8.3ms
- 从站处理 + 响应帧（5+2N 字节）≈ 10-20ms
- 总单次读写时间 < 30ms（含往返），满足 <50ms 目标

### 6.4 边界测试

| 边界场景 | 输入 | 预期 |
|---------|------|------|
| 异常码 0x01 | 从站返回 `IllegalFunction` | `Err(Exception(IllegalFunction))` |
| 异常码 0x02 | 从站返回 `IllegalDataAddress` | `Err(Exception(IllegalDataAddress))` |
| 异常码 0x06 | 从站返回 `SlaveDeviceBusy` | `Err(Exception(SlaveDeviceBusy))` |
| 超时重试耗尽 | `retry_count=0` + 超时 | `Err(MaxRetryExceeded)`，`timeout_count` +1 |
| 广播无响应 | `slave_addr=0` 写操作 | `Ok(Broadcast)`，不调用 `recv()` |
| CRC 错误重试 | 第 1 次 CRC 错，第 2 次正确 | 第 2 次成功，`crc_error_count` +1 |
| 无效从站地址 | `slave_addr=248` | `Err(InvalidSlaveAddr)`（>247） |
| 无效数量 | `quantity=126`（>125） | `Err(InvalidQuantity)` |
| 写多寄存器超限 | `values.len() > 123` | `Err(InvalidQuantity)` |
| 功能码 01（未实现） | `FunctionCode::ReadCoils` | `Err(UnsupportedFunction)` |

## 7. 验收标准

- [ ] 功能码 03（读保持寄存器）实现完整：请求编码 + 响应解码 + 数量校验（≤125）
- [ ] 功能码 06（写单个寄存器）实现完整：请求编码 + 响应解码
- [ ] 功能码 10（写多个寄存器）实现完整：请求编码 + 响应解码 + 数量校验（≤123）
- [ ] CRC16 校验正确：国标测试向量通过（`0x840A` 等已知向量）
- [ ] 点表映射支持 `U16`/`I16`/`U32`/`F32`/`Bit` 数据类型转换
- [ ] `RegToPoint::convert()` 正确应用 `raw * scale + offset` 公式
- [ ] 超时重试机制工作正常：`retry_count` 次后返回 `MaxRetryExceeded`
- [ ] 广播地址 0 写操作不等待响应，返回 `Broadcast`（D10）
- [ ] 异常码响应正确映射为 `ModbusError::Exception(ExceptionCode)`
- [ ] `ModbusStats` 统计字段正确递增
- [ ] `RtuTransport` trait（D1）解耦主站与 `Rs485Driver`，`MockRtuTransport` 可注入测试
- [ ] 单次读写延迟 <50ms@9600bps
- [ ] crate 位于 `crates/protocols/modbus-rtu/`（D8 目录规范）
- [ ] no_std 合规：仅使用 `alloc`/`core`，无 `std::*`

## 8. 风险与注意事项

| # | 风险 | 缓解措施 |
|---|------|---------|
| 8.1 | 总线冲突：Modbus RTU 只支持单主站 | 协议层不处理冲突；上层调度保证单主站轮询；多主站需切换到 Modbus TCP |
| 8.2 | 从站响应慢：需配置足够超时 | `timeout_ms` 参数化（默认 1000ms）；从站密集场景调大超时；`retry_count` 应对偶发超时 |
| 8.3 | 字节序问题：32 位数据（U32/F32）字节序因设备而异 | 当前 `convert()` 默认大端（高字在前）；后续版本需增加 `byte_order` 字段支持逐设备配置（大端/小端/字交换） |
| 8.4 | 广播地址 0：写操作无响应，不应等待 | D10：`send_request_with_retry()` 中 `slave_addr==0` 直接返回 `Broadcast`，不调用 `recv()` |
| 8.5 | 功能码 03 读取数量 ≤125 个寄存器 | `build_frame()` 前校验 `quantity`，超限返回 `Err(InvalidQuantity)` |
| 8.6 | 功能码 10 写数量 ≤123 个寄存器 | 同上，`values.len() ≤ 123` 校验 |
| 8.7 | 从站地址范围 1-247 | `slave_addr` 校验：0=广播，1-247=有效，>247 返回 `Err(InvalidSlaveAddr)` |
| 8.8 | RS485 半双工冲突 | 由 v0.44.0 `Rs485Driver` 的 DE/RE 方向控制保证；主站协议层串行轮询 |

## 9. 多角度要求

| 维度 | 要求 | 实现 |
|------|------|------|
| 9.1 功能 | 03/06/10 功能码、点表映射、轮询 | ✅ `ModbusRequest` 3 变体 + `PointMapping` + `poll_points()` |
| 9.2 性能 | 单次读写 <50ms@9600bps | ✅ 9600bps 时序估算 <30ms；CRC16 <100μs |
| 9.3 安全 | 从站地址过滤、CRC 校验 | ✅ `InvalidSlaveAddr` 校验 + CRC16 强制校验 |
| 9.4 可靠 | 超时重试、异常码处理 | ✅ `retry_count` + `MaxRetryExceeded` + `ExceptionCode` 6 异常码 |
| 9.5 可维护 | 点表 JSON 配置、易扩展 | ✅ `RegToPoint` 字段化；后续版本支持 JSON 加载 |
| 9.6 可观测 | 读写统计、错误率 | ✅ `ModbusStats` 5 字段（request/response/error/timeout/crc_error） |
| 9.7 可扩展 | 支持后续 Modbus TCP 复用 | ✅ `RtuTransport` trait（D1）解耦传输层；`FunctionCode` 含 6 变体（D9）预留 01/04/05 |

## 10. 架构图

```
┌──────────────────────────────────────────────────┐
│              应用层（Agent / 业务）                │
│         poll_points() / read / write             │
└──────────────────┬───────────────────────────────┘
                   │ ModbusRequest / ModbusResponse
                   ▼
┌──────────────────────────────────────────────────┐
│           v0.45.0 Modbus RTU 主站                 │
│           (eneros-modbus-rtu)                     │
│  ┌─────────────────────────────────────────────┐ │
│  │ ModbusRtuMaster                             │ │
│  │  ├── build_frame() / parse_response() (D7)  │ │
│  │  ├── send_request_with_retry() (含 D10)     │ │
│  │  ├── poll_points()                          │ │
│  │  ├── ModbusStats (D3)                       │ │
│  │  └── PointMapping / RegToPoint (D5/D6)      │ │
│  └─────────────┬───────────────────────────────┘ │
│                │ RtuTransport trait (D1)           │
│  ┌─────────────┴───────────────────────────────┐ │
│  │ ModbusFrame | CRC16 | ModbusRequest/Response│ │
│  │ FunctionCode(6) (D9) | ExceptionCode(6)     │ │
│  │ ModbusError (D2) | ModbusDataType | AccessMode(D4)│ │
│  └─────────────────────────────────────────────┘ │
└──────────────────┬───────────────────────────────┘
                   │ send() / recv()
                   ▼
┌──────────────────────────────────────────────────┐
│           v0.44.0 RS485 驱动                      │
│           (eneros-rs485)                          │
│  ┌─────────────────────────────────────────────┐ │
│  │ Rs485Driver (自动满足 RtuTransport, D1)      │ │
│  │  ├── send(&[u8]) -> Result<(), DriverError> │ │
│  │  ├── recv(timeout_ms) -> Result<Vec<u8>>    │ │
│  │  └── DE/RE 方向控制 + 帧间隔检测             │ │
│  └─────────────┬───────────────────────────────┘ │
│                │ UartHw trait (v0.44.0 D1)        │
└────────────────┼─────────────────────────────────┘
                 ▼
┌──────────────────────────────────────────────────┐
│           UART 硬件（HAL）                         │
│  ├── HalSerial (write/read/flush)                │
│  └── GPIO (DE/RE 方向控制)                        │
└──────────────────────────────────────────────────┘
```

**数据流说明**：

1. 应用层调用 `ModbusRtuMaster::poll_points(&mapping)` 或具体读写方法
2. 主站构造 `ModbusRequest`，经 `build_frame()` 编码为带 CRC16 的字节流
3. 字节流经 `RtuTransport` trait（D1）传递给 `Rs485Driver`
4. `Rs485Driver` 通过 `UartHw` trait 驱动 UART 硬件发送
5. 从站响应经 UART → `Rs485Driver::recv()` → 主站 `parse_response()` 解码
6. 主站返回 `ModbusResponse` 给应用层

**关键解耦点**：
- **D1 `RtuTransport` trait**：主站不直接依赖 `Rs485Driver`，便于 mock 测试与 v0.46.0 Modbus TCP 复用应用层
- **D7 `build_frame()`/`parse_response()`**：帧编解码与应用层逻辑分离，可被 TCP 版本复用
- **D9 `FunctionCode` 6 变体**：枚举完整定义，仅实现 03/06/10，预留扩展

---

> **后续演进**：
> - **v0.46.0 Modbus TCP**：复用本版本的 `ModbusRequest`/`ModbusResponse`/`FunctionCode`/`PointMapping`，仅替换 `RtuTransport` 实现为 TCP 套接字，移除 CRC16（TCP 模式无 CRC）。
> - **v0.50.0 统一点表**：使用本版本的 `PointMapping` 作为数据源，整合 Modbus RTU/TCP、IEC 104、CAN 等多协议点表为统一抽象。
