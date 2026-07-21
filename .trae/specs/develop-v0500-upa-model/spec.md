# v0.50.0 统一点表模型 UPA Spec

## Why

v0.45.0~v0.49.0 实现了 Modbus RTU/TCP、IEC 104 主/从、CAN 等多种工业协议，但各协议有各自的数据表示方式（Modbus 寄存器/线圈、IEC 104 ASDU、CAN 帧）。需要一个统一的数据点模型 UPA（Unified Point Abstraction），将所有协议数据归一化为统一的 `DataPoint` 结构，为 v0.51.0 协议抽象层和 v0.52.0 四遥模型提供基础。这是 P1-F 设备协议栈第八层——数据归一化层。

## What Changes

- **新增 crate** `eneros-upa-model`，置于 `crates/protocols/upa-model/`
- **零外部依赖**（纯数据模型，不耦合具体协议 crate，D6）
- **新增** `DataPoint` 统一数据点结构（point_id/device_id/name/description/point_type/value/quality/timestamp/source/unit）
- **新增** 类型别名 `PointId = u32`、`DeviceId = u16`
- **新增** `PointType` 枚举（Analog/Digital/Control/Setpoint/Counter）
- **新增** `PointValue` 枚举（Float(f64)/Int(i64)/Bool/Enum(u16)/String/Null）
- **新增** `PointQuality` 品质标志结构（valid/invalid/questionable/substituted/overflow/blocked/outdated）
- **新增** `DataSource` 枚举（ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual）
- **新增** `PointDatabase` 点表数据库（BTreeMap 主存储 + device_index + type_index + name_index + next_id 自增器）
- **实现** register() / update() / get_by_id() / get_by_device() / get_by_type() / get_by_name() / remove() / count() / list_all() 方法
- **新增** mock 测试模块 + 集成测试（点表 CRUD 全覆盖）
- **新增** 设计文档 `docs/protocols/upa-model-design.md`
- **更新** 根 `Cargo.toml`：版本号 0.49.0 → 0.50.0，members 增加 `"crates/protocols/upa-model"`
- **更新** `Makefile` / `ci.yml` / `gate.rs` 版本号同步

## Impact

- **Affected specs**: 无（UPA 是全新数据模型，不修改已有 crate）
- **Affected code**:
  - `e:\eneros\Cargo.toml` — workspace 版本号 + members 列表
  - `e:\eneros\crates\protocols\upa-model\` — 新 crate 全部源码
  - `e:\eneros\docs\protocols\upa-model-design.md` — 设计文档
  - `e:\eneros\Makefile` / `e:\eneros\.github\workflows\ci.yml` / `e:\eneros\ci\src\gate.rs` — 版本号
- **依赖关系**: v0.50.0 完成后解锁 v0.51.0（协议抽象层 PointAccess trait）和 v0.52.0（四遥模型）

## ADDED Requirements

### Requirement: 统一数据点 DataPoint

系统 SHALL 提供统一的 `DataPoint` 结构，将不同协议（Modbus/IEC 104/CAN）的数据归一化为统一格式。

#### Scenario: 创建数据点

- **WHEN** 调用 `PointDatabase::register(device_id, name, point_type)` 注册新点
- **THEN** 返回全局唯一的 `PointId`（u32 自增）
- **AND** 点初始值为 `PointValue::Null`，品质为 `PointQuality::invalid()`

#### Scenario: 更新点值

- **WHEN** 调用 `PointDatabase::update(point_id, value, quality, now_ms)`
- **THEN** 点的 value/quality/timestamp 更新
- **AND** 若点不存在则静默忽略（返回 bool 表示是否成功）

#### Scenario: 按 ID 查询

- **WHEN** 调用 `get_by_id(point_id)`
- **THEN** 返回 `Option<&DataPoint>`

#### Scenario: 按设备查询

- **WHEN** 调用 `get_by_device(device_id)`
- **THEN** 返回该设备下所有点的 `Vec<&DataPoint>`

#### Scenario: 按类型查询

- **WHEN** 调用 `get_by_type(point_type)`
- **THEN** 返回该类型所有点的 `Vec<&DataPoint>`

#### Scenario: 按名称查询

- **WHEN** 调用 `get_by_name(name)`
- **THEN** 返回 `Option<&DataPoint>`

#### Scenario: 删除点

- **WHEN** 调用 `remove(point_id)`
- **THEN** 从主存储和所有索引中移除该点
- **AND** 返回 `bool` 表示是否删除成功

### Requirement: PointValue 六种值类型

系统 SHALL 支持六种统一值类型：Float(f64)、Int(i64)、Bool、Enum(u16)、String、Null。

#### Scenario: 浮点值

- **WHEN** 存储 Modbus 保持寄存器或 IEC 104 遥测值
- **THEN** 使用 `PointValue::Float(f64)`

#### Scenario: 布尔值

- **WHEN** 存储 Modbus 线圈或 IEC 104 单点遥信
- **THEN** 使用 `PointValue::Bool(bool)`

#### Scenario: 空值

- **WHEN** 点未初始化或数据丢失
- **THEN** 使用 `PointValue::Null`

### Requirement: PointQuality 品质标志

系统 SHALL 提供统一的品质标志，包含 valid/invalid/questionable/substituted/overflow/blocked/outdated 七个标志位。

#### Scenario: 好品质

- **WHEN** 数据有效
- **THEN** `PointQuality::good()` 返回 `{ valid: true, 其余 false }`

#### Scenario: 无效品质

- **WHEN** 数据无效
- **THEN** `PointQuality::invalid()` 返回 `{ invalid: true, 其余 false }`

### Requirement: PointDatabase 多索引查询

系统 SHALL 提供 PointDatabase 支持按 ID/设备/类型/名称四种查询方式，通过 BTreeMap 索引实现高效查询。

#### Scenario: 多维索引

- **WHEN** 注册新点时
- **THEN** 自动更新 device_index、type_index、name_index
- **AND** 查询时无需遍历全表

## 偏差声明（D1~D9）

| 偏差 | 说明 |
|------|------|
| **D1** | 时间戳用 `u64` 毫秒参数注入（无 `MonotonicTime` 类型，与 v0.48.0 D3 一致；蓝图 `timestamp: MonotonicTime` 改为 `timestamp_ms: u64`） |
| **D2** | `PointDatabase` 不内置 `RwLock`（no_std 单线程使用；蓝图 `RwLock` 为 std 类型；多线程场景由调用方用 `spin::RwLock` 包装）—— Karpathy Simplicity First |
| **D3** | `next_id` 用普通 `u32` 自增字段（非 `AtomicU32`；所有方法 `&mut self`，无并发需求）—— Karpathy Simplicity First |
| **D4** | 使用 `alloc::collections::BTreeMap` 替代 `std::collections::HashMap`（有序、no_std 友好、key 可推导） |
| **D5** | crate 放入 `crates/protocols/upa-model/`（P1-F 协议栈第八层，与 modbus/iec104 同级） |
| **D6** | 零外部依赖（纯数据模型，不耦合 eneros-modbus-*/eneros-iec104-*/eneros-can；协议适配在 v0.51.0 实现） |
| **D7** | `PointValue::Float` 用 `f64`（蓝图原样；`PointValue` 仅派生 `PartialEq` 不派生 `Eq`，因 f64 不实现 Eq） |
| **D8** | 不实现 `DeviceDriver` trait（数据模型非设备驱动，与 v0.48.0/v0.49.0 一致） |
| **D9** | `update()` 接受 `now_ms: u64` 参数用于设置时间戳（D1 时间注入） |
