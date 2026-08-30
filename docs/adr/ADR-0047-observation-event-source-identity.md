# ADR-0047：ObservationEvent 观测事件与来源身份窄腰（蓝图 61 v0.314.27）

- 状态：已接受（2026-08-30）
- 关联：蓝图 61 [02] §3.1/§4、[06] §2、[16] §3.1/§3.2、[17] .27 行
- 影响面：telemetry-model（新增 event 模块）、protocol-abstract（映射身份绑定）、
  ingestion-core（新 crate）、config-bundle（FIELD_SOURCES/FIELD_POINTS_V2）、
  control-services（obs_shadow）、protocol-services（adapter bridge）

## 背景

蓝图 61 Phase 0 完成控制正确性止血后，观测侧仍是"裸样本"世界：
`TelemetrySample`（58B wire）的 `source/source_epoch/mapping_version` 由
binary 硬编码（生产 Modbus 适配器 source=1/epoch=0/mapping=1）——
身份可伪造、epoch 无持久语义、点映射无单位/资产绑定，任何 binary 都能
冒充合法采集链；评估器只按白名单数字判断，无法回答"谁、在哪个租户/站点、
哪个 epoch/sequence、何时、以什么 schema/单位/映射产生"。

## 裁决

1. **`ObservationEvent` 落 `telemetry-model::event`**（canonical 唯一定义）：
   不可变观测事实，V1 wire 固定 128B 小端（[02] §3.1 表逐字段冻结）；
   `event_id = first_16(SHA-256("eneros.observation.v1" || LE(身份+时间+值)))`
   （ingest_time/reserved 不参与——重放同 ID）；NaN/Inf、epoch=0 非 TEST、
   未知单位码在门上拒绝。`DataPoint`/`Telemetry` 保留配置/状态职责，
   不被替换。
2. **`RawObservation` 只存在于 adapter 内部**：跨出 adapter 必须经
   `SourceRegistry::verify` 成为 `ObservationEvent`——伪造入口在类型上
   不存在（RawObservation 无 epoch 字段，epoch 由 registry 分配）。
3. **新深模块 `crates/protocols/ingestion-core`**：SourceDescriptor/
   SourceRegistry（register/descriptor/verify/open_source）/ObservationSink/
   EpochStore/QuarantineStore。依赖只允许 upa-model/telemetry-model/
   protocol-abstract（[16] §3.1 依赖方向）；scheduling/限流/去重/quarantine
   全私有。未注册事件只能进 quarantine（有界、可观测），控制 evaluator
   零消费（类型隔离）。
4. **身份三事实全部来自签名配置**：source_id = points 配置 `[[source]]`
   表（FIELD_SOURCES，config-bundle 编译/解码封闭 schema）；source_epoch =
   registry `open_source`（EpochStore 持久化上界 + 启动时刻下界，严格递增）；
   mapping_version = bundle generation（GET_GENERATION，既有服务端动词 +
   新客户端封装）。生产代码零硬编码。
5. **`ProtocolPointMapping` 增身份绑定 `PointIdentityBinding`**（unit_code/
   schema_id/schema_version/asset_id/mapping_generation）——[06] §2.2
   "映射关联 unit、schema、asset binding 和 mapping generation"；全零
   UNKNOWN 不满足控制快照资格。config-bundle v2 点表记录尾随 24B 身份块
   （FIELD_POINTS_V2），apply 期以 bundle generation 覆盖注入映射世代。
6. **影子轨切换纪律（[17] .27 行）**：svc-modbus-tcp 每轮先发 OBS 批次
   （TEL_INGEST_OBS，V1 128B 帧）后发 legacy 58B INGEST（兼容读不变）；
   svc-telemetry 影子模块（obs_shadow）独立评估 V1 事件并与生产 verdict
   对照（计数+串口行）——**零控制消费**：影子 TrustedTelemetry 不进任何
   下游；生产 TrustedTelemetry 仍只从 legacy 轨产生（.30 才切换）。
   20 点参考批新旧轨 provenance 摘要逐字节一致（E3 判据）。

## 事实差异与缺口登记（如实）

- **target 侧 epoch 持久化**：[06] §2.4 允许"持久化启动计数或远端 boot id
  的认证映射"。svc-telemetry/svc-modbus-tcp 均无持久存储 capability
  （E-DEF-01 磁盘 TSDB 停用；boot_id 为随机熵非计数器）。target 装配使用
  `MemEpochStore` + 启动时刻下界（epoch 单次开机内单调、开源即递增；
  跨重启持久递增待 .32 event-store 卷授予 durable 槽后切 `FileEpochStore`
  语义）。host 侧重启递增/水位恢复已 E3 实证。**owner 行动项**：.32 卷
  持久 epoch 槽授予。
- **qemu-test 合成设施**：root 合成遥测（source=1）保持 feature 门控
  （qemu-test-synth；qemu-prod 闭包不含），svc-telemetry 初始
  allowed_sources=[1] 为兼容轨初值，配置装载后覆盖为注册来源集合。

## 结果

- 伪造/重放红线关闭：身份由类型与注册表分配，篡改冲突（同身份不同
  payload）/epoch 回退/sequence 回退结构化拒绝。
- 单位/资产/schema 进入观测身份：unit_code=0/asset_id=0/schema_id=0 的
  观测不满足控制资格（[02] §3.1 硬门）。
- 新 wire 全字段 golden/变异/负例测试在库；workspace 7905 测试全绿。
