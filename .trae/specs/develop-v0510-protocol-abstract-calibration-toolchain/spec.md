# v0.51.0 协议抽象层 + v0.51.1 计量校准 + v0.51.2 调试工具链 Spec

> 本 spec 涵盖 v0.51.x 全部三个版本：v0.51.0（协议抽象层）、v0.51.1（计量校准 R5）、v0.51.2（调试与工厂测试工具链 R7）。按 EnerOS 规则，主版本及其子版本在同一任务中一起开发。

## Why

v0.50.0 定义了统一的 UPA 数据点模型（DataPoint），但各协议（Modbus RTU/TCP、IEC 104 主/从、CAN）仍各有独立接口。需要一个统一的 `PointAccess` trait 和协议适配器层，让 Agent 和上层应用通过统一接口访问不同协议的设备，无需关心底层协议细节（依赖倒置）。此外，储能参与电力市场结算需计量精度达标（CT/PT 变比校准、精度等级验证），工厂量产与现场运维需调试工具链（设备模拟器、协议分析器、批量配置工具）。这三个版本共同构成 P1-F 设备协议栈的完整收尾。

## What Changes

### v0.51.0 协议抽象层
- **新增 crate** `eneros-protocol-abstract`，置于 `crates/protocols/protocol-abstract/`
- **依赖** `eneros-upa-model`（path 依赖，复用 DataPoint/PointId/DeviceId/PointValue/PointQuality/PointType/DataSource）
- **新增** `PointAccess` trait（read_point/read_points/write_point/write_points/read_device_points/protocol_type）
- **新增** `ProtocolAdapter` trait（继承 PointAccess，init/start/stop/poll）
- **新增** `ProtocolType` 枚举（ModbusRtu/ModbusTcp/Iec104/Can/Internal）
- **新增** `ProtocolAddress` 枚举（Modbus/Iec104/Can 地址变体）
- **新增** `ProtocolPointMapping` 结构体（点 ID 到协议地址的映射）
- **新增** `AdapterConfig` 结构体（适配器配置）
- **新增** `ProtocolError` 错误枚举
- **新增** `ProtocolManager` 多协议管理器（注册适配器 + 统一访问入口 + poll_all）
- **新增** mock 适配器 `MockAdapter`（用于测试，不依赖具体协议 crate）
- **新增** 设计文档 `docs/protocols/protocol-abstract-design.md`

### v0.51.1 计量校准（R5）
- **新增 crate** `eneros-calibration`，置于 `crates/drivers/calibration/`
- **零外部依赖**（纯计算 + 数据结构，校准系数持久化由 trait 抽象）
- **新增** `CalibCoeffs` 结构体（ct_ratio/pt_ratio/phase_correction/offset_voltage/offset_current/calibrated_at）
- **新增** `AccuracyClass` 枚举（Class0_2S/Class0_5S/Class1_0/Class2_0）
- **新增** `CalibResult` 结构体（校准结果）
- **新增** `MeterCalibration` trait（apply_coefficients/measure_error/classify_accuracy）
- **新增** `calibrate_meter` / `verify_accuracy` 函数
- **新增** `CalibStore` trait（系数持久化抽象，load/save）
- **新增** `InMemoryCalibStore` 实现（测试用）
- **新增** 设计文档 `docs/drivers/meter-calibration-design.md`

### v0.51.2 调试与工厂测试工具链（R7）
- **新增** `tools/device_simulator/`（设备模拟器，主机侧 std 程序）
- **新增** `tools/protocol_analyzer/`（协议分析器，主机侧 std 程序）
- **新增** `tools/batch_config/`（批量配置工具，主机侧 std 程序）
- **新增** `TestSuite`/`TestItem`/`TestCategory`/`TestReport`/`TestFailure` 类型
- **新增** `FactoryTestRunner` trait + `DefaultTestRunner` 实现
- **新增** `SimConfig`/`SimHandle`/`SimError` 模拟器类型
- **新增** `Packet` 抓包类型
- **新增** 设计文档 `docs/runtime/factory-test-toolchain.md`

### 通用变更
- **更新** 根 `Cargo.toml`：版本号 0.50.0 → 0.51.0，members 增加 3 个新路径
- **更新** `Makefile` / `ci.yml` / `gate.rs` 版本号同步

## Impact

- **Affected specs**: v0.50.0（upa-model，作为依赖被复用）
- **Affected code**:
  - `e:\eneros\Cargo.toml` — workspace 版本号 + members 列表
  - `e:\eneros\crates\protocols\protocol-abstract\` — v0.51.0 新 crate
  - `e:\eneros\crates\drivers\calibration\` — v0.51.1 新 crate
  - `e:\eneros\tools\device_simulator\` / `tools\protocol_analyzer\` / `tools\batch_config\` — v0.51.2 工具链
  - `e:\eneros\docs\protocols\protocol-abstract-design.md` / `docs\drivers\meter-calibration-design.md` / `docs\runtime\factory-test-toolchain.md`
  - `e:\eneros\Makefile` / `e:\eneros\.github\workflows\ci.yml` / `e:\eneros\ci\src\gate.rs` — 版本号
- **依赖关系**: v0.51.0 完成后解锁 v0.52.0（四遥模型）、v0.55.0+（Agent 设备数据访问）

## ADDED Requirements

### Requirement: PointAccess 统一点访问接口（v0.51.0）

系统 SHALL 提供统一的 `PointAccess` trait，所有协议适配器必须实现此 trait。

#### Scenario: 读取单个点

- **WHEN** 调用 `adapter.read_point(point_id)`
- **THEN** 返回 `Result<DataPoint, ProtocolError>`
- **AND** 若点不存在返回 `ProtocolError::PointNotFound`

#### Scenario: 写入单个点（遥控/遥调）

- **WHEN** 调用 `adapter.write_point(point_id, value)`
- **THEN** 返回 `Result<(), ProtocolError>`

#### Scenario: 批量读取

- **WHEN** 调用 `adapter.read_points(&[point_ids])`
- **THEN** 返回 `Vec<Result<DataPoint, ProtocolError>>`

#### Scenario: 按设备读取

- **WHEN** 调用 `adapter.read_device_points(device_id)`
- **THEN** 返回 `Result<Vec<DataPoint>, ProtocolError>`

### Requirement: ProtocolAdapter 适配器 trait（v0.51.0）

系统 SHALL 提供 `ProtocolAdapter` trait（继承 PointAccess），定义适配器生命周期。

#### Scenario: 适配器生命周期

- **WHEN** 创建适配器后调用 `init(config)` → `start()` → `poll()` → `stop()`
- **THEN** 适配器正确初始化、启动、轮询、停止

### Requirement: ProtocolManager 多协议管理（v0.51.0）

系统 SHALL 提供 `ProtocolManager` 支持多协议适配器注册和统一访问。

#### Scenario: 多协议注册

- **WHEN** 调用 `manager.register_adapter(box<dyn ProtocolAdapter>)`
- **THEN** 适配器按 `protocol_type()` 注册到内部 BTreeMap

#### Scenario: 统一访问路由

- **WHEN** 调用 `manager.read_point(point_id)`
- **THEN** 自动路由到对应协议适配器

#### Scenario: 全协议轮询

- **WHEN** 调用 `manager.poll_all()`
- **THEN** 所有已注册适配器依次执行 poll()

### Requirement: 计量校准（v0.51.1）

系统 SHALL 提供计量校准功能，包括 CT/PT 变比校准、精度等级验证、系数持久化。

#### Scenario: CT/PT 校准

- **WHEN** 调用 `calibrate_meter(ct_ratio, pt_ratio)`
- **THEN** 返回 `CalibResult`（含校准前后误差、精度等级、是否通过）

#### Scenario: 精度等级验证

- **WHEN** 调用 `verify_accuracy(class)`
- **THEN** 返回校准结果，验证误差是否满足目标精度等级

#### Scenario: 系数持久化

- **WHEN** 调用 `store.save(coeffs)` 后 `store.load()`
- **THEN** 系数正确持久化并恢复

### Requirement: 工厂测试工具链（v0.51.2）

系统 SHALL 提供设备模拟器、协议分析器、批量配置工具三件套。

#### Scenario: 设备模拟

- **WHEN** 调用 `simulate_device(config)`
- **THEN** 返回 `SimHandle`，模拟从站响应

#### Scenario: 工厂测试套件

- **WHEN** 调用 `run_factory_test(suite)`
- **THEN** 返回 `TestReport`（含通过/失败统计、失败明细）

#### Scenario: 批量配置

- **WHEN** 批量配置工具读取配置模板并下发到多台设备
- **THEN** 校验回读一致性

## 偏差声明（D1~D14）

### v0.51.0 偏差
| 偏差 | 说明 |
|------|------|
| **D1** | 不直接依赖 eneros-modbus-*/eneros-iec104-*/eneros-can（适配器实现为后续版本任务；本版本仅定义 trait + mock 适配器 + ProtocolManager）—— Karpathy Simplicity First |
| **D2** | 不实现 `Send + Sync` 约束（蓝图 `PointAccess: Send + Sync` 为 std 约束；no_std 单线程场景不需要） |
| **D3** | 不实现 `subscribe`/`unsubscribe` 订阅回调（蓝图有此方法，但 `Box<dyn Fn>` 在 no_std + 无 std::sync 下复杂；变化上报通过 poll() 主动查询实现，订阅机制后置）—— Karpathy Simplicity First |
| **D4** | 不使用 `Arc<RwLock<PointDatabase>>`（蓝图有此字段；no_std 无 Arc/RwLock；ProtocolManager 持有 `&mut PointDatabase` 引用或由调用方管理） |
| **D5** | 时间戳用 `u64` 毫秒参数注入（与 v0.50.0 D1 一致） |
| **D6** | crate 放入 `crates/protocols/protocol-abstract/`（P1-F 协议栈最后一层） |
| **D7** | 不实现 `DeviceDriver` trait（协议抽象层非设备驱动，与 v0.48.0~v0.50.0 一致） |

### v0.51.1 偏差
| 偏差 | 说明 |
|------|------|
| **D8** | crate 放入 `crates/drivers/calibration/`（计量校准属驱动层补强） |
| **D9** | `CalibStore` trait 抽象持久化（不直接依赖文件系统；InMemoryCalibStore 用于测试，文件系统实现后置） |
| **D10** | 零外部依赖（纯计算 + 数据结构） |

### v0.51.2 偏差
| 偏差 | 说明 |
|------|------|
| **D11** | 工具链为主机侧 **std** 程序（蓝图明确"工具非产品代码"，不受 no_std 约束） |
| **D12** | 工具放入 `tools/` 目录（非 `crates/`，因非产品代码） |
| **D13** | 设备模拟器/协议分析器/批量配置为独立可执行程序（各有 `main.rs`） |
| **D14** | 不入 workspace members（工具非 Rust crate 成员，独立编译；若需统一构建可用 Makefile target） |
