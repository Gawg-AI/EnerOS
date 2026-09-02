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
| `AuthorizedWriter` | 协议侧写接缝：只接受 `AuthorizedCommand` 的写能力，与 `PointAccess`（读）信任分级（蓝图/24 D1 已落地：写从 PointAccess 分离，读写信任分级） | `protocol-abstract`（蓝图/24，ADR-0026 §7） | 裸 `write_point` 无授权写（已移除，删除闸门见蓝图/24 R2） |
| `AuthorizedCommand` | 控制链裁决产物：device_id(u16)/point_id(u32)/command/actor_kind/issuer/lease/fencing 等；唯一构造路径 `ControlGateway::authorize` | `telemetry-model::command`（蓝图/24 D4 下移，control-gateway re-export） | 信封 wire 上的裸 u32 字段（仅 wire 编解码内部） |
| `ProtocolPointMapping` | 点 ID ↔ 协议地址映射 + 工程量变换（scale/offset） | `protocol-abstract` | — |
| `DeviceDriver` | 用户态设备驱动 trait（生命周期/状态/健康），唯一驱动 seam | `drivers/framework`（ADR-0028） | `DriverPlugin`（driver-sdk，已删除） |
| `MockAdapter`/`MockDriver` | 契约测试用 mock（seam 层共享套件成员） | `protocol-abstract`/`framework` | — |
| `ObservationEvent` | 不可变观测事实（V1 wire 128B）：tenant/site/source/epoch/sequence/时间/资产/单位/schema/品质/完整性/映射世代 + event_id（SHA-256 前 16B，去重键） | `telemetry-model::event`（蓝图 61，ADR-0046） | `TelemetrySample`（旧 58B 兼容轨，.30 切换）、`DataPoint`（配置/状态模型，不替换） |
| `RawObservation` | adapter 内部原始观测（无 epoch 字段——身份由注册表分配；跨出 adapter 必须成为 ObservationEvent） | `ingestion-core`（蓝图 61，ADR-0046） | 裸 TelemetrySample 跨 binary（禁止） |
| `SourceRegistry` | 来源注册深模块：SourceDescriptor/register/verify/open_source；未注册事件只进 quarantine（控制 evaluator 零消费） | `ingestion-core`（蓝图 61，ADR-0046） | 评估器白名单数字（allowed_sources 硬编码，已废） |
| `UnitCatalog` | 封闭数字单位码表（0=未知；wire 层 unit_code 唯一事实） | `telemetry-model::unit`（蓝图 61） | 裸 f64 跨域传递（禁止） |
| `CandidatePlan` | 候选计划（全时域分配/状态/摘要；非成功 solve status 不可构造） | `decision-contract`（蓝图 61，ADR-0048） | `ProposedControlPlan`（迁移 facade，.32 删） |
| `CertifiedPlan` | 独立认证证书（seal 收口构造；外部只有 decode+verify） | `decision-contract`（蓝图 61，ADR-0048） | SafetyValidator bool（不得当证书） |
| `CommandIntent`/`CommandBatch` | 编译后命令意图/有序批（批构造必须持证书+资源视图） | `decision-contract`（蓝图 61，ADR-0048） | 直接设备写（禁止） |
| `SettleTrigger` | 结算触发源枚举（SafetyReject/BudgetRate/BudgetEnergy/…）：`settle(trigger, states)` 纯函数按表裁决唯一结算出口 | `control-dispatch::engine`（蓝图 62，settle 决策表 369 行纯函数+表级单测） | 结算逻辑散在调用点（已收表） |
| `SERVICE_WIRING` | 服务装配接线表：name/heartbeat/net_slot/stack_pages/driven（周期+相位）/driven_none_reason——装配属性单源，表序即驱动编排序 | `root-task::wiring`（蓝图 62，D-1） | spawn 期按名手抄表（已删）；完备性门：每个被 spawn 服务必须 ∈ 本表 ∪ EXPLICIT_UNWIRED（63.2） |
| `IngestionStage` | 采集交付半环单源模块：run_acq_round（tick→checkpoint→drain→ACQ 行→分批上行）+ TelemetryUplink（分批/chunked/epoch 内化）+ ACQ 词形单源 | `protocol-services::ingestion_stage`（蓝图 62 S2a） | bins 本地 collect_and_report/report_obs 双抄（已删） |
| `ChunkedEndpoint`/`ChunkedSession` | chunked Call 端点（epoch 私有，递增仅 call 内）；ChunkedSession=共享 epoch 会话（每次 call 后无条件写回共享计数器，成败无关——root 三步手工协议的结构化收拢） | `agent-sdk::endpoint`（蓝图 62 S4 / 63.2 WP-63.2.1） | 调用方 `*epoch = endpoint.epoch()` 手写回（已删，结构性不可漏） |
| `SignerKeyring` | SM2 密钥环 + 类型化验签原语（verify_signature→VerifiedSignature，Ok(false) 陷阱不外泄）+ 修订水位（check_revision 非变异判定 / commit_revision 推进——判据单源，消费方零手写比较） | `signer-keyring`（蓝图 62.3 / 63.3.1） | 消费方手写 `revision <= seen`（已删）；裸 sm2_verify 直调（逐处收口/豁免登记） |
| `WatermarkTracker` | **序列水位**（ingestion-core）：同源 (epoch, sequence) 两维，check 非变异 + advance 分离——sink 准入守卫判据唯一实现 | `ingestion-core`（蓝图 62.3 W3） | snapshot-service 旧同名 tracker（63.3.1 消歧改名 `SnapshotWatermark`）；手写水位比较（已删） |
| `SnapshotWatermark` | **快照构建水位**（snapshot-service）：epoch/seq + event_time/ingest_time 四维，observe 合体——快照域分层判据（⊃ 序列水位 + 两时间维），与 ingestion-core WatermarkTracker 不合并 | `snapshot-service::watermark`（蓝图 63.3.1 消歧定名） | 与 WatermarkTracker 同名混用（禁止——分层关系见 watermark.rs 模块 doc） |

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
| `Period` | 时段（峰/平/谷），进入 `PricePoint.period`/`ResourceBid.period` 跨 crate 契约 | `market-types`（ADR-0029，W5.1 蓝图23） | market_feed 旧定义已收敛为 re-export |
| `aggregator` | 仅用于"聚合计算"（见 §3 归属） | 各领域保留 | 见 ADR-0029 |

## 3. aggregator 归属边界（ADR-0029）

| 位置 | 语义 | 命名 |
|---|---|---|
| `coordinator/src/vpp_aggregator.rs` | VPP 资源聚合与调度（VppResource/AggregatedDispatch） | `VppAggregator` 语义 |
| `cloud-coordinator/src/aggregator.rs` | 跨域数据汇聚（DataSource→DataAggregator→DataSink） | `DataAggregator` |
| `dr/src/aggregator.rs` | 需求响应负荷聚合（aggregate_capacity） | `DrAggregator` |
| `twin-global` | 多域联合仿真/模型融合（MultiDomainSimulator/ModelFusion） | 不用 aggregator 字样（用"联合"） |

同义词纪律：`aggregator`=聚合计算；`coordinator`=编排/协调；`federation`=跨信任域协作；`fusion`=模型/数据融合。
水位消歧：**watermark 序列水位（ingestion-core WatermarkTracker，(epoch,seq) 准入判据）≠ event-store `WatermarkReport` 容量水位（分区容量观测报告）**——两词不混用（蓝图 63.3 WP-63.3.4）。

## 4. 插件执行模型（ADR-0025）

| 术语 | 定义 |
|---|---|
| 插件 | 签名静态 ELF PIE → 受限 EL0 进程 → 版本化 IPC wire ABI（Agent/驱动/协议统一） |
| IPC wire | 二进制消息格式：固定头 + 有界长度 + CRC32（对齐 agent-sdk `wire.rs` EIPC） |
| cross-binary ABI | 禁止：`*mut dyn`、Rust trait object vtable、String/Vec/enum 直传 |
| `InteropLevel::SpecBlocked` | 互操作诚实分级最低档：规约文本缺失——codec 字节层未实现（蓝图 48；owner 提供规约并落地权威 KAT 后方可升 CodecReady） | `protocol-abstract::plugin::InteropLevel` |

## 5. 已删除/已废弃词汇（不再使用）

- `DeviceAdapter`（device-agent 字符串点名旁路，ARCH-P02 删除）
- `DriverPlugin`/`ProtocolPlugin` C ABI 注册宏（ADR-0025 删除）
- `register_driver!`/`register_protocol!`（同上）
- protocol-sdk 旧点类型（`HString` 点 ID、8 变体 `PointValue`、3 态 `Quality`，ADR-0027）
- `eneros-driver-sdk` crate（ADR-0028）
