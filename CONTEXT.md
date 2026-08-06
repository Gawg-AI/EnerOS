# EnerOS 领域上下文（CONTEXT.md）

> 单上下文布局：本文档 + `docs/adr/`（MASTER §0）。
> 本文是领域词汇的唯一事实源；与蓝图/代码冲突时，以本文 + 最新 ADR 为准。
> 词汇归属裁定见 ADR-0029；点模型见 ADR-0027；协议 seam 见 ADR-0026；驱动 seam 见 ADR-0028。

## 1. 核心实体

| 术语 | 定义 | 唯一来源 | 别名（禁止混用） |
|---|---|---|---|
| `DataPoint` | 统一数据点：point_id(u32) + device_id(u16) + value + quality + timestamp_ms + source + unit | `upa-model`（ADR-0027） | protocol-sdk 旧 `DataPoint`（已删除） |
| `PointValue` | 点值 6 变体：Float/Int/Bool/Enum/String/Null | `upa-model` | protocol-sdk 8 变体（已删除） |
| `PointQuality` | 数据品质 7 位标志：valid/invalid/questionable/substituted/overflow/blocked/outdated | `upa-model` | `Quality`（protocol-sdk 3 态）、`QualityFlag`（telemetry-model，保留为转换层） |
| `PointId` | 点唯一标识，u32 | `upa-model` | `HString<64>` 点 ID（已删除） |
| `DeviceId` | 设备标识，u16 | `upa-model` | — |
| `DataSource` | 采集来源：ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual | `upa-model` | — |
| `ProtocolAdapter` | 协议适配器 trait（生命周期 + 点访问），唯一协议 seam | `protocol-abstract`（ADR-0026） | driver-sdk/protocol-sdk 插件 trait（宿主内接口） |
| `PointAccess` | 统一点读写 trait（唯一协议 seam 底座） | `protocol-abstract` | device-agent 旧 `DeviceAdapter`（已删除） |
| `ProtocolPointMapping` | 点 ID ↔ 协议地址映射 + 工程量变换（scale/offset） | `protocol-abstract` | — |
| `DeviceDriver` | 用户态设备驱动 trait（生命周期/状态/健康），唯一驱动 seam | `drivers/framework`（ADR-0028） | `DriverPlugin`（driver-sdk，已删除） |
| `MockAdapter`/`MockDriver` | 契约测试用 mock（seam 层共享套件成员） | `protocol-abstract`/`framework` | — |

## 2. 市场域词汇

| 术语 | 定义 | 唯一来源 | 现状 |
|---|---|---|---|
| `TradeDir` | 交易方向：Buy/Sell | `market-types`（ADR-0029） | spot-market/realtime-market 旧定义收敛为 re-export |
| `TimeSlot` | 时间段（UTC 秒，start/end，end>start） | `market-types` | spot-market 旧定义收敛 |
| `Bid` | 报价单：bid_id + market + submitter + segments + submit_at | `market-types` | spot-market、energy-market-agent 旧定义收敛 |
| `YuanPerKwh` | 价格 newtype（元/kWh） | `market-types` | spot-market 裸 f64（已替换） |
| `YuanPerMwh` | 价格 newtype（元/MWh） | `market-types` | realtime-market 裸 f64（已替换） |
| `BidSegment` | 报价段：period + volume_kwh + price + direction | `market-types` | spot-market 旧定义收敛 |
| `BidAck` | 报价回执 | `market-types` | spot-market/ancillary 旧定义收敛 |
| `aggregator` | 仅用于"聚合计算"（见 §3 归属） | 各领域保留 | 见 ADR-0029 |

## 3. aggregator 归属边界（ADR-0029）

| 位置 | 语义 | 命名 |
|---|---|---|
| `coordinator/src/vpp_aggregator.rs` | VPP 资源聚合与调度（VppResource/AggregatedDispatch） | `VppAggregator` 语义 |
| `cloud-coordinator/src/aggregator.rs` | 跨域数据汇聚（DataSource→DataAggregator→DataSink） | `DataAggregator` |
| `dr/src/aggregator.rs` | 需求响应负荷聚合（aggregate_capacity） | `DrAggregator` |
| `twin-global` | 多域联合仿真/模型融合（MultiDomainSimulator/ModelFusion） | 不用 aggregator 字样（用"联合"） |

同义词纪律：`aggregator`=聚合计算；`coordinator`=编排/协调；`federation`=跨信任域协作；`fusion`=模型/数据融合。

## 4. 插件执行模型（ADR-0025）

| 术语 | 定义 |
|---|---|
| 插件 | 签名静态 ELF PIE → 受限 EL0 进程 → 版本化 IPC wire ABI（Agent/驱动/协议统一） |
| IPC wire | 二进制消息格式：固定头 + 有界长度 + CRC32（对齐 agent-sdk `wire.rs` EIPC） |
| cross-binary ABI | 禁止：`*mut dyn`、Rust trait object vtable、String/Vec/enum 直传 |

## 5. 已删除/已废弃词汇（不再使用）

- `DeviceAdapter`（device-agent 字符串点名旁路，ARCH-P02 删除）
- `DriverPlugin`/`ProtocolPlugin` C ABI 注册宏（ADR-0025 删除）
- `register_driver!`/`register_protocol!`（同上）
- protocol-sdk 旧点类型（`HString` 点 ID、8 变体 `PointValue`、3 态 `Quality`，ADR-0027）
- `eneros-driver-sdk` crate（ADR-0028）
