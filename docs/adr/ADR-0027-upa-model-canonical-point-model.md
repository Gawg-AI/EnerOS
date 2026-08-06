# ADR-0027：upa-model 为 canonical 点模型，其余点类型为转换层

> 状态：Accepted（2026-08-04）
> 决策者：产品负责人 + 架构收敛实施（07 蓝图 ARCH-D03 / ARCH-S02 / ARCH-P02）
> 前置：ADR-0026（唯一协议 seam）
> 审计证据：全仓 `pub struct DataPoint` 2 处（upa-model/point.rs、protocol-sdk/lib.rs）、
> `QualityFlag`/`Quality`/`PointQuality` 3 套品质语义、`PointValue` 3 套值类型
> （upa-model 6 变体 / protocol-sdk 8 变体 / device-agent `String→f64` 隐式）。

## 1. 背景

07 蓝图 C3 确认点模型存在"四宇宙"：UPA 统一模型（`DataPoint`/`PointValue`/
`PointQuality`）、IEC 四遥模型（`Telemetry`/`Telesignaling`/…/`QualityFlag`）、
protocol-sdk 自带模型（`HString<64>` 点 ID + 8 变体 `PointValue` + 3 态 `Quality`）、
device-agent 隐式模型（`String → f64` 字符串点名）。四套语义并存导致：质量位无法跨
协议传递、点 ID 类型不可互操作、`PointValue` 分叉（Float vs F32/F64）。

## 2. 决策

1. **`upa-model` 是 canonical 点模型**：`PointId`(u32)、`DeviceId`(u16)、
   `DataPoint`、`PointValue`（6 变体）、`PointQuality`（7 位标志）、`DataSource`、
   `PointType` 的唯一演化地。新代码不得再定义等价点类型。
2. **其余点类型为转换层**：
   - `telemetry-model`：IEC 四遥语义类型（Telemetry/…/QualityFlag）**保留**为领域
     模型（四遥专有字段：deadband、limits、control 方向），但**必须复用**
     upa-model 的 `PointId`/`DeviceId`（已复用），并在 `upa-model::bridge` 提供
     `Telemetry ↔ DataPoint` 转换函数；不允许新增第 N 套基础点类型。
   - `protocol-sdk`：删除自建 `DataPoint`/`PointValue`/`Quality`，改为 upa-model
     re-export + 薄转换层（ARCH-S02）。
   - `device-agent`：删除 `String → f64` 隐式模型，经 `ProtocolPointMapping`
     桥接 `PointAccess`（ARCH-P02），读点返回带质量位与时间戳的 `DataPoint`。
3. **转换函数集中**：跨模型转换集中在 `upa-model::bridge` 模块，不允许散落在各
   消费者；转换层必须有 **round-trip 属性测试**（07 §4.2，不接受样例断言）。
4. **`PointQuality` 与 `QualityFlag`**：二者语义一一对应（7 位标志 ↔ 7 态单选），
   `bridge` 提供双向转换；转换不丢失信息（单选取唯一真位，多真位取优先级）。

## 3. 四套点类型映射表

| # | 模型（crate） | 点 ID | 值类型 | 品质 | 处置 |
|---|---|---|---|---|---|
| 1 | upa-model（canonical） | `PointId` = u32 | `PointValue` 6 变体（Float/Int/Bool/Enum/String/Null） | `PointQuality` 7 位标志 | **唯一演化地** |
| 2 | telemetry-model（IEC 四遥） | `PointId`（复用 upa-model） | `value: f64`（Telemetry 等专有字段） | `QualityFlag` 7 态单选 | 领域模型保留；`bridge` 双向转换 |
| 3 | protocol-sdk（v1.0 自带） | `HString<64>` | `PointValue` 8 变体（I16/U16/I32/U32/F32/F64/Bytes…） | `Quality` 3 态 | **删除自建**，re-export upa-model + 转换层（ARCH-S02） |
| 4 | device-agent（隐式） | `String` 点名 | `f64` | 无 | **删除旁路**，经 `ProtocolPointMapping` 接 `PointAccess`（ARCH-P02） |

### 值类型映射（protocol-sdk → upa-model）

| protocol-sdk `PointValue` | upa-model `PointValue` |
|---|---|
| `Bool(b)` | `Bool(b)` |
| `I16(i)` / `I32(i)` | `Int(i as i64)` |
| `U16(u)` / `U32(u)` | `Int(u as i64)` |
| `F32(f)` | `Float(f as f64)` |
| `F64(f)` | `Float(f)` |
| `Bytes(v)` | 无直接变体 → `String`（按 UTF-8 损失转换）或扩展 upa-model（新增前须 ADR） |

### 品质映射（protocol-sdk `Quality` → upa-model `PointQuality`）

| `Quality` | `PointQuality` |
|---|---|
| `Good` | `{ valid: true, .. }` |
| `Uncertain` | `{ questionable: true, .. }` |
| `Bad` | `{ invalid: true, .. }` |

### 品质映射（telemetry-model `QualityFlag` ↔ upa-model `PointQuality`）

| `QualityFlag` | `PointQuality` 位 |
|---|---|
| `Good` | `valid` |
| `Invalid` | `invalid` |
| `Questionable` | `questionable` |
| `Substituted` | `substituted` |
| `Blocked` | `blocked` |
| `Overflow` | `overflow` |
| `Outdated` | `outdated` |

## 4. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 以 protocol-sdk 点模型为 canonical | 8 变体含 `Bytes` 无工程量语义、HString 点 ID 无设备归属；upa-model 已 4 crate 消费且带 DeviceId/quality/source/unit |
| 四套模型各自独立、由消费者适配 | 正是 C3 现状（转换散落、语义漂移），07 蓝图结论是归一点模型 |
| 删除 telemetry-model 并入 upa-model | IEC 四遥专有字段（deadband/limits/control 方向）是领域语义，不是基础点模型；保留为领域层是正确分层 |

## 5. 影响面

- `crates/protocols/upa-model/`：新增 `bridge` 模块（QualityFlag ↔ PointQuality、
  protocol-sdk 旧类型 → upa-model 转换；后者仅在 ARCH-S02 需要时保留）。
- `crates/protocols/protocol-sdk/`：点类型切换（ARCH-S02）。
- `crates/agents/device-agent/`：点访问桥接（ARCH-P02）。
- `crates/protocols/telemetry-model/`：补 `bridge` 转换 + round-trip 属性测试。
- 出口：07 §5「全仓 `DataPoint`/`PointValue`/`Quality` 的 canonical 定义唯一
  （upa-model），其余为转换层」。

## 6. 追踪

- 蓝图：07 §1.3、§2.3；MASTER §G3 SVC-008。
- 实施：ARCH-S02、ARCH-P02；出口验证以全仓 `pub struct DataPoint` 唯一性为准。
