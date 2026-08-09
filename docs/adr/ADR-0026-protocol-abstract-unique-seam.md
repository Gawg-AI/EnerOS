# ADR-0026：protocol-abstract 为唯一协议 seam，interface 由真实适配器反推收缩

> 状态：Accepted（2026-08-04）
> 决策者：产品负责人 + 架构收敛实施（07 蓝图 ARCH-D02 / ARCH-P01 / ARCH-P03）
> 前置：ADR-0025（插件执行模型）；07 蓝图 §1.2
> 审计证据：`crates/protocols/protocol-abstract/` 全仓零消费者（无任何 crate `use protocol_abstract`）；
> `crates/agents/device-agent/src/registry.rs:13` 注释自述"避免 v0.51.0 `PointAccess` 的
> `PointId`/`DataPoint` 类型化 API 复杂性"而走私有 `DeviceAdapter` 旁路（C2）。

## 1. 背景

07 蓝图 C2 确认 `protocol-abstract` 自 v0.51.0 起定义的 `ProtocolAdapter`/`PointAccess`
seam **从未被任何真实适配器实现**：modbus-rtu、iec104-master 均未实现，唯一潜在消费者
device-agent 明确绕开。设计术语上这是 **hypothetical seam**——"一个适配器是假想，两个
才是真实"。同时，interface 在过去版本中按假想需求持续扩张（D1~D7 偏差声明），无真实
适配器验证方法是否必要、签名是否合理。

## 2. 决策

1. **唯一协议 seam**：`protocol-abstract` 保留为唯一协议抽象层；不新建第二套协议
   抽象，不允许协议 crate 私藏适配器接口。
2. **真实适配器反推**：`modbus-rtu` 实现 `ProtocolAdapter` 成为第一个真实适配器
   （mock 为第二个），接口必须由真实需求收缩而非扩张。
3. **收缩规则（ARCH-P03 落地）**：
   - 保留的方法必须同时被 ≥2 个适配器（mock + 至少一个真实）使用；
   - 未被真实适配器使用的方法移入 extension trait 或删除；
   - 收缩后 interface 方法数 ≤ 收缩前。
4. **device-agent 回归 seam**（ARCH-P02）：删除私有 `DeviceAdapter` 旁路，经
   `ProtocolPointMapping` 桥接 `PointAccess`；不允许把字符串点名包装成 `PointAccess`
   实现来"形式上接入"（07 §6 禁止捷径）。
5. **共享契约测试套件**：`protocol-abstract` 定义 seam 层契约测试套件；mock 与每个
   真实适配器跑同一套件（07 §4.1）。

## 3. 当前 interface 方法清单与收缩草案（以 modbus-rtu 真实需求为准）

### PointAccess（6 方法）

| 方法 | modbus 真实需求 | 裁决（草案，P03 复核） |
|---|---|---|
| `read_point(PointId) -> Result<DataPoint>` | 经 mapping 表寻址读寄存器 | **保留** |
| `read_points(&[PointId]) -> Vec<Result<DataPoint>>` | 批量轮询（poll_points） | **保留**（默认逐点实现可落默认方法） |
| `write_point(PointId, PointValue)` | 写单寄存器（write_single_register） | **保留** |
| `write_points(&[(PointId, PointValue)])` | 批量写 | **移 extension trait**（modbus 主路径用单写） |
| `read_device_points(DeviceId)` | 无直接对应（按设备过滤点表） | **移 extension trait**（可由 mapping 过滤实现） |
| `protocol_type() -> ProtocolType` | 路由索引必需 | **保留** |

### ProtocolAdapter（继承 PointAccess，另 5 方法）

| 方法 | modbus 真实需求 | 裁决（草案，P03 复核） |
|---|---|---|
| `init(&AdapterConfig)` | 加载串口/映射配置 | **保留** |
| `start()` | 打开串口连接 | **保留** |
| `stop()` | 关闭串口 | **保留** |
| `poll(now_ms)` | 周期轮询（ModbusRtuMaster 已内置） | **保留** |
| `state() -> AdapterState` | 状态查询 | **保留**（默认实现可落 default） |

收缩原则：**方法数不增加**；任何新增方法必须先有真实适配器需求与共享套件用例。

## 4. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 删除 protocol-abstract，让各协议自带接口 | 退回 C5 式"多套抽象并存"；统一 seam 是 07 蓝图 C2 的核心结论 |
| 新建第二套抽象层（如把 SDK 的 ProtocolPlugin 提升为 seam） | 重复 C1/C2 错误；seam 必须唯一且由真实适配器反推 |
| 继续在 protocol-abstract 加方法等待需求 | 假想需求扩张，07 §6 明令禁止 |
| 让 device-agent 保留旁路、仅新增适配器 | ARCH-P02 未闭环；旁路即第二套点模型（C3）的根源 |

## 5. 影响面

- `crates/protocols/protocol-abstract/`：interface 收缩（P03）、共享契约测试套件（P01）；
  新增 modbus-rtu/iec104-master 依赖（实现 trait 需要依赖 seam）。
- `crates/protocols/modbus-rtu/`：新增 `protocol-abstract` + `upa-model` 依赖，实现
  `ProtocolAdapter`（P01）。
- `crates/agents/device-agent/`：删 `DeviceAdapter` 旁路，桥接 `PointAccess`（P02）。
- 依赖方向：协议 crate 依赖抽象层（上层依赖抽象），抽象层不依赖具体协议——保持。

## 6. 追踪

- 蓝图：07 §1.2、§2.3、§4.1；MASTER §G4 NET-009～011。
- 实施：ARCH-P01（modbus 适配器 + 共享套件）、ARCH-P02（device-agent 去旁路）、
  ARCH-P03（seam 收缩）、ARCH-P04（iec104 第三适配器）。

## 7. 补充说明（2026-08-09，蓝图/24 D10）：控制写接缝 AuthorizedWriter

> 蓝图/24《控制写接缝（AuthorizedWriter）开发规格与计划》实施补充。本 ADR 的
> "唯一协议 seam"结论不推翻，seam 内部分级（读写信任分级）按蓝图/24 D1 扩展。

### 7.1 读写信任分级（蓝图/24 §1.1 推论 A）

读操作幂等、无副作用（错了可以重读）；**写操作改变物理世界状态**（功率设定值
下发到 PCS 就是真实的充放电），错误代价以设备损伤与安全事故计。因此整个系统中
唯一需要"裁决接缝"的操作类是写——把读写放进同一个信任等级的接口（原
`PointAccess` 含 `write_point`）是范畴错误。

**落地**：`PointAccess` 收窄为纯读（`read_point`/`read_points`/`protocol_type`），
`write_point`/`write_points` 移除；新增 [`AuthorizedWriter`]（`write_authorized(cmd,
value) -> Result<WriteOutcome, WriteError>`），只接受 [`AuthorizedCommand`]
（控制链裁决产物）。

### 7.2 AuthorizedWriter 与 PointAccess 的关系

- **分离而非继承**：`AuthorizedWriter` 不继承 `PointAccess`（读消费方不应被迫
  携带写知识，写接缝不应暴露读能力）；设备注册表持组合形态
  `PointAccessWriter: PointAccess + AuthorizedWriter`（`dyn` 双 trait 不合法，
  以单一 principal trait 表达，E0225 规避）。
- **编译期强制（蓝图/24 §1.2 推论 C）**：`PointAccess` 无写方法——读接缝消费方
  在编译期无法写；唯一写入口要求 `AuthorizedCommand` 参数（类型形状强制）。
  命令"真实性"由 device-agent 收口复验保证（运行时强制，纵深防御）。
- **类型化（蓝图/24 D4）**：`AuthorizedCommand` 的 `device_id` 为 upa-model
  `DeviceId(u16)`、`point_id` 为 `PointId(u32)`；信封 wire 布局不变（wire 上
  device_id 仍 4 字节，`ControlGateway::authorize` 在 HMAC 验证后收窄，
  越界 → `GatewayError::BadDeviceId` default-deny）。
- **落位**：`AuthorizedCommand`/`ActorKind` 随 `ControlCommand` 下移
  telemetry-model（共享模型层），control-gateway re-export 保持导入路径；
  protocol-abstract 不反向依赖 control-gateway（依赖方向约束）。

### 7.3 急停与逃生门（蓝图/24 §1.4 推论 F、D8）

"安全默认拒绝"的反面不是"默认放行"，而是设计过的最小逃生门：

- 急停/就地操作走**正常信封路径**（预置租约 + `ActorKind::Operator`/`Emergency`）——
  急停也进审计链（合规卖点）；
- `EmergencyBypass`（CTL-009）保留为**租约机制故障时**的逃生门：只绕策略 PDP，
  不绕安全校验（CTL-008），全程留审计——三条逃生门原则（显式设计/最小化/
  全程留审计）满足；
- 断网本地自治写完整可用（本地租约本地签发，§6.3）；fencing epoch 以平台
  重启计数为种子（单调性，§1.5 推论 G）。
