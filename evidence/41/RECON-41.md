# RECON-41 — 蓝图 41 侦察与 v1 能力冻结

> 生成：2026-08-16｜执行人：EnerOS 商业交付主开发 AI（蓝图 41）
> 基线：`HEAD=b96774e3a20a468ccfc4d6d81c0ee761a460860b` + 当前全部未提交/未跟踪工作树
> 侦察方式：四路并行只读扫描（configs 域 / 市场结算域 / OTA-联邦-MQTT-云域 / 管理-可观测域）+ 关键文件直读
> 本文件是 41 的 `evidence/41/RECON-41.md`（蓝图 §1.1 要求）

---

## 1. configs/** 使用图（蓝图 §1.1 第一项）

- `configs/` 顶层 83 项：76 个 `.toml` + 3 个 `.dts`（qemu-virt/phytium-d2000/kunpeng-920）+ 4 子目录（compliance/power/platform-contracts/time）。
- **80 个 .toml 全部仅含 `# consumer:` 占位注释**（“集成层占位（产线注入）——当前无 Rust 解析代码，仅交付模板”），唯一例外：
  - `control-write.toml` → 唯一真实加载方是 **测试路径** `crates/agents/device-agent/src/assembly_demo.rs`（手工字符串切分，非 typed parser）；
  - `dengbao.toml` → `compliance/src/dengbao.rs` 仅作证据指针字符串引用；
  - `tls.toml` → host 侧注入 PEM 的说明注释。
- **Rust 侧 typed parser 数量 = 0**。`ConfigId/ConfigConsumer/RuntimeConfigBundle/SignedConfigEntry/ConfigBundleVersion` 在任何 crate 中无定义。
- xtask Gate 现状：
  - `command_config_refs`（`xtask/src/gate.rs:313-424`）：GV-01 阻断“引用不存在的 toml”；**CF-01 只检查文件头 12 行内含 `# consumer:` 子串**——占位注释即可满足，正是蓝图 41 要废除的“注释自证”。
  - `command_config_syntax`（gate.rs:538-585）：仅 TOML 语法。
  - 绕过路径：注释行跳过、排除词表、扫描根不含 docs/evidence/蓝图/product。
- ProductManifest（`xtask/src/product_manifest.rs:248`）：已有 `ConfigSpec { id, path, digest, consumer }`、`ServiceSpec.configs: Vec<String>`、`config-no-consumer` 校验；`qemu-prod.json` 的 `configs: []`（空）。

## 2. 市场/成交/结算/账户现状（蓝图 §1.1 第二项）

- `market-types`：`Bid/BidAck/TradeDir/TimeSlot/Period/YuanPerKwh/YuanPerMwh/MilliYuan`（定点金额已存在，lib.rs:273）。**Bid/BidAck 无外部 ID、无 idempotency、无 deadline、无 tenant/product scope**。
- `energy-market-agent`：`MarketDataSource`、`BidIntentSource/BidOptimizer/BidPublisher` 链；`real_adapters.rs` 的 `MarketChannelBidPublisher<F>` 是 **FnMut 发送闭包**，lib.rs:184-188 自述“real_adapters 与 sdk_bridge 零生产消费者”；默认构造仍是 `MockBidPublisher`。
- `realtime-market`：`RealtimeLoop.submit_quote`（quote_loop.rs:87-118）**只做频率/会话检查 + `quotes_sent += 1` 计数，报价不落任何存储**。
- `p2p-match`：`MatchEngine` 纯内存 `Vec<P2pBid>/Vec<P2pAsk>`；`Deal.price` 裸 f64；无 ACK/外部 ID/幂等；Cargo 标 `[SPECULATIVE]`。
- `settlement`：`ClearingEngine { results: Vec<ClearingResult> }` 全内存；`account`：`AccountStore` 全内存 Vec；firmware-agent（`scenes/firmware-agent`）是 mock 骨架场景。
- 三个 crate Cargo.toml 均标 `[SPECULATIVE]`。

## 3. OTA/恢复/bootctrl 全路径（蓝图 §1.1 第三项）

- `ota-ab`：`OtaManager` trait（upgrade.rs:109）+ `AbPartitionOta`（ab_partition.rs:76，**真实 SM3 回读 + SM2 验签**，非 stub）；`PartitionState` 七态；`OtaError::DowngradeRejected`。
- **缺失**：`CheckPolicy` 无定义；`MarkTrialBoot/trial counter/boot success marker` 无定义（仅 `BootReason::UpdateTrial` 在 boot-protocol）；ota-ab 与 `boot_control.rs` 的 `BootControl/BootControlTransaction` **无实际接线**。
- `hot_migrate.rs:162` 有 `NullPlatform`（全 Ok 的 `PlatformAbstraction`）——P2-02，须移出 release 宣称。
- `downloader.rs` 的恒真闭包只存在于 `#[cfg(test)]`；生产 `verify` 真实。
- `model-ota`：`HotLoader`（Arc + mem::replace）纯 RAM 热切换；`OtaManager` 状态机 Idle→ReadyToSwap；**无 A/B 持久分区**；`transport_mqtt.rs` 自带一套手写 MQTT 3.1.1 编解码（与 protocols/mqtt 完全独立）。
- `recovery`：`RecoveryModeImpl` 已存在（flash 验签+写）；`diag.rs` 探针默认恒 ok。

## 4. MQTT/AgentBus/联邦/云同步 production transport 与 durable state（蓝图 §1.1 第四项）

- `protocols/mqtt`：`MqttClient`（ConnectionState + 指数退避重连 + subscriptions 恢复）+ `QosStore`（InMemoryQosStore / FsQosStore@feature=fs-persist）+ `SecureMqttTransport`（`InsecureStream` 需 policy 放行，不冒充 TLS）+ `NetTcpTransport<D: NetDevice>`（feature=net，smoltcp）。
- **无 session cursor**（`clean_session: true` 硬编码）；**无 AgentBus 消费者**；`agent-bus-dds` 是旧 DDS 路线（grid_agent/twin-agent 引用，商业范围 out-of-v1）。
- `federation`：`real.rs` 的 `SeedPresenceBus`（broadcasts Vec 只记录不投递）、`TwoNodeConsensusBus<F>`（send_fn 闭包 + inbox Vec + inject 回调）；`tunnel.rs:1582` `VerticalEncryptDevice::poll` **永远 None**；PBFT/crypto/状态机组件（pbft.rs/consensus.rs/view_change.rs）真实存在，仅依赖 eneros-crypto。
- `cloud-sync`：`HttpSyncTransport<F>`（闭包）；`EventStore`（offset 游标语义）+ `SyncBatch` + `RetryQueue`；**无 UploadItem 类型**；`cloud-coordinator` 的 `TlsCloudChannel<S,R>` 双闭包。
- `SecureChannelFactory`（protocol-abstract/channel.rs:137）**无任何实现者**。
- QEMU 装配：`qemu-prod.json` 已有 `svc-mqtt`（protocol-services，UnassignedProtocolService 壳）；**无 svc-ota/svc-federation/svc-cloud-sync/svc-ledger/svc-market/svc-mgmt/svc-observability**。

## 5. mgmt/auth/RBAC/action handler（蓝图 §1.1 第五项）

- `mgmt-api/transport.rs`：`MqttRpcTransport`，`execute_remote_action`（:79-84）**忽略 action 恒返回 (Ok, payload)**；publish 错误全部 `let _ =` 吞掉；`parse_request` 默认 `Operator/StatusQuery`，未知 action → StatusQuery，“security”字符串映射 Operator。
- `auth.rs`：`SignedToken`（113B role‖times‖SM3‖SM2 sig）+ `TokenVerifier`——**只有 role，无 principal/tenant/channel binding**；`PrincipalId/RoleSet/TenantScope/AuthenticatedSession/ChannelBinding/TrustedTime` 全仓无定义。
- `ApiAction` 24 个 action，除 RBAC 拒绝外**全部空成功**；`ActionHandler` 不存在。

## 6. metrics/logging/tracing/dashboard/alert/aiops target consumer（蓝图 §1.1 第六项）

- 12 个 observability crate 均 no_std lib；**零 EL0 消费者**（无 crate 依赖它们）。
- 局部修复已存在（可复用）：telemetry-queue `OverflowPolicy::DropLowPriority`、`QueueSnapshot`、`MqttMetricsExporter`（闭包）、`PrivacyManager`；slo-alert `AlertStatus/TriggerEngine`（`MqttAlertSink` 闭包）；aiops `SelfHealEngine`（`RealHealExecutor` 闭包，无 executor 必须 Err）；logging/metrics 的 `FileStorage/FileMetricsStore` 整模块 `#[cfg(feature="std")]`（host/debug 专用）。
- `dashboard`：`MockTopology` 公开导出（lib.rs:35）、固定 3 节点 + `timestamp: 0`（topology.rs:75）；`WsServer` 真 RFC6455 但无 listener/auth/TLS/origin/rate/backpressure。
- **无 Evidence/Telemetry role partition 用法、无 target 持久化、无 svc-observability**。

## 7. Real/Default/Null/Mock/FnMut/Vec inbox/File* 生产导出清单（蓝图 §1.1 第七项）

| 位置 | 形态 | 41 处置 |
|---|---|---|
| `energy-market-agent` 默认 MockBidPublisher + real_adapters 闭包 | Mock/FnMut | 替换为真实 MarketAdapter（durable outbox） |
| `realtime-market` submit_quote 计数 | 内存计数 | 接 outbox + 外部 peer receipt |
| `p2p-match/settlement/account` Vec 内存 | Vec | 迁移 durable ledger 模型 |
| `ota-ab/hot_migrate.rs` NullPlatform | Null 全成功 | 移入 test-only，release 熔断 |
| `federation` TwoNodeConsensusBus send_fn/inbox | 闭包/Vec | EIPC 真传输（服务间真实 IPC） |
| `tunnel.rs` VerticalEncryptDevice::poll None | 恒 None | 移除/显式 Unavailable |
| `cloud-sync` HttpSyncTransport 闭包 | FnMut | durable outbox + cursor |
| `mgmt-api` execute_remote_action 固定 Ok | 空成功 | 真实 action handler + session 认证 |
| `dashboard` MockTopology + timestamp=0 | Mock | 移 test-support + registry 真 provider |
| `aiops` RealHealExecutor 闭包 | 闭包 | 真 ActionExecutor（经 ControlDispatch/管理 RPC） |
| `slo-alert` MqttAlertSink 闭包 | 闭包 | AgentBus 真 adapter |
| logging/metrics FileStorage/FileMetricsStore | File sink | 保留 host/debug profile（std-gated，不冒充 target） |

## 8. 未提交 federation/dashboard/logging/slo/ops 差异保护清单（蓝图 §1.1 第八项）

工作树有大量未提交修改（git status 见 41 验证记录）；41 只触碰 41 owned paths（`configs/**`、业务 agents、mqtt AgentBus consumer、observability、manifest business 声明、ota-ab 产品 service 层）。**不得**：改 38 HAL/DMA、39 block/net/protocol core、40 ControlDispatch 语义、42 evidence schema/CI。

---

## 9. v1 能力冻结（蓝图 §1 裁决）

> 冻结依据：35-00/35-01 事实 + 上述侦察 + 36–40 已完成装配面。裁决人：41 主开发 AI（按蓝图授权）；42 前不得漂移。

| 能力 | 裁决 | 理由（现状 → 41 动作） |
|---|---|---|
| 市场报价/成交 | **RequiredProductCapability** | p2p-match 撮合算法真实；41 建 MarketAdapter + durable outbox + peer receipt |
| 结算/账户 | **RequiredProductCapability** | 双分录 ledger（金额已 MilliYuan 定点）；41 建 durable ledger 服务 |
| 系统 OTA | **RequiredProductCapability** | ota-ab 真验签/回读；41 建 CheckPolicy/Trial/Commit-Rollback 服务 |
| 模型 OTA | **OptionalProductCapability** | RAM swap primitive 保留；A/B 持久分区 v1.1（41 只做诚实降级与 A/B 接口）；release 不宣称持久模型槽 |
| MQTT AgentBus | **RequiredProductCapability** | MQTT client/QoS 组件真实；41 建 AgentBus 消费者 + session cursor |
| 联邦 | **OptionalProductCapability** | PBFT/crypto 真实；EIPC 真 transport 由 41 建成即入 v1，否则显式降级（release 不宣称网络联邦） |
| 云同步 | **OptionalProductCapability** | durable outbox + cursor 由 41 建成即入 v1，外部云端 endpoint 缺则显式 Degraded |
| 远程管理 | **RequiredProductCapability** | 24 action 必须真实化，空成功清零 |
| 可观测 | **RequiredProductCapability** | target queue + drop policy + redaction 由 41 建成；exporter 外部 peer 缺则 Degraded |
| Dashboard/HMI | **OptionalProductCapability** | 真 TopologyProvider + 认证 WsServer；MockTopology 移 test-support |
| 告警/AIOps | **OptionalProductCapability** | AgentBus 真 sink + 真 ActionExecutor（remediation 仅提案，执行走授权入口） |
| DDS 路线 | **OutOfV1** | agent-bus-dds 只留 library；`configs/dds.toml` 删除；文档不宣称 |

## 10. 关键 BLOCKED 事实（先声明，避免假 PASS）

1. **target 侧持久块存储能力未移交**：`svc-storage` 仍为 `UnassignedService`（39/40 存储句柄移交未完成）。41 的 durable 语义层全部 host 实测（含掉电模拟）；target 服务经 EIPC block client 接 svc-storage，未授予时显式 `ST_UNAVAILABLE`——不伪造持久化。该缺口登记 BLOCKED_EVIDENCE，属 39/40 遗留，41 不越界改 39 core。
2. **外部 peer（MQTT broker/云端/联邦对端）**：QEMU user-net 对 host 的 TCP 通道是否可用待实测；不可用时 exporter/同步证据标 BLOCKED。
