# v0.53.x SOE 事件引擎 + MQTT 上报 + 告警管理 Spec

> **覆盖版本**：v0.53.0（SOE 事件顺序记录引擎）+ v0.53.1（MQTT 物联网上报）+ v0.53.2（告警管理体系）
> **依据**：蓝图 `phase1.md` 第 10166~10715 行；附录 `appendix.md` 第 804~806 行
> **规则**：项目规则要求"一个任务中完成 X.x 下所有子版本"

## Why

v0.52.0 四遥标准数据模型已就绪，但缺少：

1. **事件顺序记录（SOE）**：故障分析需要 ms 级时标的事件回放能力。当前四遥模型只能"上报当前值"，无法保留"事件发生顺序"用于事后追溯。
2. **远程上报通道**：SOE 事件与四遥数据无法上传到云端运维平台/SCADA 主站。MQTT 是物联网标准轻量协议，适合储能终端弱网场景。
3. **告警全生命周期管理**：原始事件量大且无优先级，运维人员无法快速识别关键故障。需要告警分级、抑制（抖动过滤）、确认（ACK）、升级（Escalation）能力。

三者构成"采集 → 记录 → 上报 → 告警"的完整数据链。

## What Changes

### v0.53.0 SOE 事件顺序记录引擎
- **新增 crate** `eneros-soe-engine` 位于 `crates/protocols/soe-engine/`
- 依赖：`eneros-upa-model`（PointId/DeviceId/PointValue/DataPoint/PointQuality）+ `eneros-telemetry-model`（QualityFlag）
- 类型：`SoeEvent`/`SoeEventType`（11 变体）/`EventPriority`（4 级）/`SoeConfig`/`SoeStats`/`SoeError`
- 引擎：`SoeEngine`（事件队列 + record/query/upload/cleanup）
- 抽象：`SoeStorage` trait + `InMemorySoeStorage` 实现；`UploadChannel` trait + `MockUploadChannel` 实现
- 触发器：`EventTrigger` trait + `DigitalChangeTrigger` + `OverLimitTrigger`

### v0.53.1 MQTT 物联网上报
- **新增 crate** `eneros-mqtt` 位于 `crates/protocols/mqtt/`
- 依赖：仅 `eneros-upa-model`（基础类型）
- 协议：MQTT v3.1.1 报文编解码（CONNECT/CONNACK/PUBLISH/PUBACK/PUBREC/PUBREL/PUBCOMP/SUBSCRIBE/SUBACK/UNSUBSCRIBE/UNSUBACK/PINGREQ/PINGRESP/DISCONNECT）
- 客户端：`MqttClient` 状态机（Disconnected/Connecting/Connected）
- QoS：0（AtMostOnce）/1（AtLeastOnce）/2（ExactlyOnce）
- 抽象：`MqttTransport` trait + `MockTransport` 实现（解耦 smoltcp TCP，与 v0.46.0/v0.49.0 网络栈解耦模式一致）
- 重连：指数退避（初始 1s，最大 30s，重连后恢复订阅）
- 遗嘱：`LastWill` 支持

### v0.53.2 告警管理体系
- **新增 crate** `eneros-alarm` 位于 `crates/agents/alarm/`（蓝图明确指定 `crates/agents/alarm/`）
- 依赖：`eneros-upa-model`（基础类型）+ `eneros-soe-engine`（事件源，可选）
- 类型：`AlarmId`/`AlarmLevel`（Info/Warning/Critical/Emergency）/`AlarmRecord`/`AlarmError`
- 管理器：`AlarmManager`（raise/acknowledge/clear/escalate/query_active/query_history）
- 抑制：`SuppressionRule`（滑动窗口 N 秒内同源合并）
- 升级：`EscalationPolicy`（Critical 未 ACK 超时升级 Emergency）

## Impact

- **新增依赖**：3 个新 crate 加入 workspace members
- **受影响 spec**：v0.52.0 四遥数据模型（被 SOE 引擎复用 QualityFlag）
- **不影响现有代码**：仅新增 crate，不修改已有 crate 行为
- **后续解锁**：v0.54.0 RTOS 控制闭环（SOE 提供事件回放）、v0.109.0 故障录波（SOE 提供事件源）

## ADDED Requirements

### Requirement: SOE 事件顺序记录

系统 SHALL 提供 SOE 引擎，支持 ms 级时标事件记录、按时间戳排序（不乱序）、持久化（通过 trait 抽象）、查询（按时间/设备/最新）、上传（通过 trait 抽象）、过期清理。

#### Scenario: 事件按时间戳排序不乱序
- **WHEN** 三个事件分别以时间戳 t=300、t=100、t=200 入队（采集乱序）
- **THEN** 出队顺序按时间戳升序（t=100 → t=200 → t=300）

#### Scenario: 触发器检测遥信变位
- **WHEN** DigitalChangeTrigger.check(old=On, new=Off)
- **THEN** 返回 Some(SoeEvent{ event_type: DigitalChange, priority: Medium })

#### Scenario: 越限触发器检测从正常到越限
- **WHEN** OverLimitTrigger.check(old=10.0 在限内, new=15.0 越上限 12.0)
- **THEN** 返回 Some(SoeEvent{ event_type: AnalogOverLimit, priority: High })

### Requirement: MQTT 物联网上报

系统 SHALL 提供 MQTT v3.1.1 客户端，支持 QoS 0/1/2 发布与订阅、遗嘱消息、断线指数退避重连、订阅自动恢复。

#### Scenario: QoS 0 发布成功
- **WHEN** MqttClient.publish(topic="a/b", payload=[1,2,3], qos=QoS::AtMostOnce) 在已连接状态
- **THEN** 返回 Ok(())，无需等待 ACK

#### Scenario: QoS 1 发布需等待 PUBACK
- **WHEN** MqttClient.publish(topic="a/b", payload=[1,2,3], qos=QoS::AtLeastOnce)
- **THEN** 发送 PUBLISH 后等待 PUBACK，收到后返回 Ok(())

#### Scenario: 断线重连恢复订阅
- **WHEN** 连接断开后触发重连
- **THEN** 指数退避后重连成功，已订阅 topic 自动重新订阅

### Requirement: 告警全生命周期管理

系统 SHALL 提供告警管理器，支持告警生成（含级别判定）、抖动抑制（同源 N 秒内合并）、运维 ACK、Critical 未 ACK 超时升级 Emergency、故障恢复自动 Clear。

#### Scenario: 抖动抑制
- **WHEN** 同一源 5 秒内连续 raise 3 次相同级别告警
- **THEN** 实际只生成 1 条告警，后续 2 次被抑制

#### Scenario: Critical 未 ACK 升级
- **WHEN** Critical 告警在配置的 timeout（如 300s）内未 ACK
- **THEN** 调用 escalate() 后级别升级为 Emergency

#### Scenario: 故障恢复自动清除
- **WHEN** 故障源发送恢复事件
- **THEN** 对应告警状态从 Active 变为 Cleared，记录清除时间戳

## MODIFIED Requirements

无（仅新增功能，不修改现有需求）。

## REMOVED Requirements

无。

## 偏差声明（D1~Dn）

### v0.53.0 SOE 引擎
- **D1** 时间戳用 `u64` 毫秒参数注入（蓝图 `MonotonicTime`/`SystemTime` 在 no_std 不存在；与 v0.50.0/v0.51.0/v0.52.0 D1 一致）
- **D2** crate 放入 `crates/protocols/soe-engine/`（P1-G 四遥与 SOE，与 upa-model/telemetry-model 同级）
- **D3** 仅依赖 `eneros-upa-model` + `eneros-telemetry-model`（复用 PointId/DeviceId/PointValue/QualityFlag）
- **D4** 持久化抽象为 `SoeStorage` trait + `InMemorySoeStorage` mock 实现（不直接依赖 v0.25.0 TSDB；与 v0.49.0 transport trait 模式一致）
- **D5** 上传抽象为 `UploadChannel` trait + `MockUploadChannel` mock 实现（不直接依赖网络栈）
- **D6** 优先队列使用 `alloc::collections::BinaryHeap`（no_std 友好；蓝图 `PriorityQueue` 不存在标准实现）
- **D7** 不要求 `Send + Sync`（no_std 单线程；与 v0.51.0 D2 一致）
- **D8** 不使用 `AtomicU64`（no_std 单线程，用 `&mut self` 内的 `u64` 自增；蓝图 `next_event_id: AtomicU64` 改为 `next_event_id: u64`）
- **D9** `SystemTime::now()` 改为 `now_ms: u64` 参数注入（与 D1 一致）
- **D10** 不实现 `EventTrigger: Send + Sync`（D7 一致）

### v0.53.1 MQTT 客户端
- **D11** crate 放入 `crates/protocols/mqtt/`（P1-G 物联网协议层）
- **D12** TCP 传输抽象为 `MqttTransport` trait + `MockTransport` 实现（不直接依赖 smoltcp；与 v0.46.0/v0.49.0 transport trait 模式一致）
- **D13** 仅支持 MQTT v3.1.1（不支持 MQTT 5；蓝图 §5 技术交底明确"MVP 不需要复杂特性"）
- **D14** 不实现 TLS（MVP；蓝图 §8 注明"凭证安全：与 v0.31.0 国密联动"留待后续集成）
- **D15** QoS 1/2 未确认消息仅在内存（不持久化；蓝图 §5 提及"需持久化"但 MVP 简化）
- **D16** 时间戳/超时使用 `u64` 毫秒参数注入（与 D1 一致）
- **D17** 不要求 `Send + Sync`
- **D18** 凭证使用 `String` 明文（不加密；加密留待与 v0.31.0 集成）

### v0.53.2 告警管理
- **D19** crate 放入 `crates/agents/alarm/`（蓝图 §3 明确指定路径）
- **D20** 仅依赖 `eneros-upa-model`（不直接依赖 soe-engine，告警源通过 `AlarmSource` trait 抽象，避免循环依赖）
- **D21** 抑制策略使用滑动窗口计数（`VecDeque<u64>` 时间戳队列），不实现依赖抑制（蓝图 §4.3 提及但 MVP 简化）
- **D22** 升级策略简化为"超时升级一级"（Critical → Emergency），不实现多级升级阶梯
- **D23** 配置以结构体注入（不解析 TOML；蓝图 `configs/alarm_rules.toml` 留待 v0.26.0 配置管理集成）
- **D24** 不要求 `Send + Sync`
- **D25** 时间戳使用 `u64` 毫秒参数注入
