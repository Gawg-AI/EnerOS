# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.53.0`
- [x] C2 members 列表已添加 `crates/protocols/soe-engine`、`crates/protocols/mqtt`、`crates/agents/alarm`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## v0.53.0 SOE 引擎 — Crate 骨架
- [x] C4 `crates/protocols/soe-engine/Cargo.toml` 存在，package name 为 `eneros-soe-engine`
- [x] C5 dependencies 包含 `eneros-upa-model` + `eneros-telemetry-model`（D3）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / event / config / storage / upload / trigger / engine
- [x] C8 D1~D10 偏差声明表存在于 lib.rs
- [x] C9 `SoeError` 枚举包含 QueueFull/StorageError/UploadError/NotFound/InvalidArgument
- [x] C10 `SoeConfig` 包含 max_queue_size/persist_enabled/persist_batch_size/upload_interval_ms/retention_days
- [x] C11 `SoeStats` 包含 total_events/persisted_events/uploaded_events/dropped_events

## v0.53.0 SOE 引擎 — SoeEvent + 类型
- [x] C12 `SoeEventType` 枚举包含 11 变体（DigitalChange/AnalogOverLimit/AnalogRecovery/QualityChange/ControlExecute/ControlDone/ControlFailed/ManualSet/CommLost/CommRestore/Custom(u16)）
- [x] C13 `EventPriority` 枚举（Critical=0/High=1/Medium=2/Low=3）派生 PartialOrd/Ord
- [x] C14 `SoeEvent` 结构体包含 11 字段（event_id/timestamp_ms/system_time_ms/point_id/device_id/event_type/old_value/new_value/quality/priority/description）
- [x] C15 `timestamp_ms` 为 `u64` 类型（D1/D9）
- [x] C16 `SoeEvent::new()` / `is_critical()` 方法实现

## v0.53.0 SOE 引擎 — SoeStorage + UploadChannel 抽象
- [x] C17 `SoeStorage` trait 定义 7 个方法（append/query_by_time/query_by_device/get_latest/get_unuploaded/mark_uploaded/delete_before）
- [x] C18 `InMemorySoeStorage` 实现所有方法
- [x] C19 `UploadChannel` trait 定义 upload/is_connected
- [x] C20 `MockUploadChannel` 实现并记录上传调用统计

## v0.53.0 SOE 引擎 — EventTrigger
- [x] C21 `EventTrigger` trait 不要求 Send+Sync（D7/D10）
- [x] C22 `DigitalChangeTrigger` 检测遥信变位（Digital 类型 + 值变化）
- [x] C23 `OverLimitTrigger` 使用 BTreeMap 存储越限配置
- [x] C24 `OverLimitTrigger` 正常→越限生成 AnalogOverLimit/High
- [x] C25 `OverLimitTrigger` 越限→正常生成 AnalogRecovery/Medium

## v0.53.0 SOE 引擎 — SoeEngine
- [x] C26 `SoeEngine` 使用 `BinaryHeap` 按时间戳排序（D6）
- [x] C27 `next_event_id` 为 `u64` 而非 AtomicU64（D8）
- [x] C28 `record_event()` 分配 event_id + 入队 + 统计
- [x] C29 `process_point_change()` 遍历触发器记录事件
- [x] C30 `persist_events()` 排空队列调用 storage.append
- [x] C31 `query_by_time()` / `query_by_device()` / `get_latest()` 查询方法
- [x] C32 `upload_events()` 上传未上传事件
- [x] C33 `cleanup_expired()` 按 retention_days 清理

## v0.53.0 SOE 引擎 — 集成测试
- [x] C34 T1 SoeEvent 构造与 is_critical
- [x] C35 T2 SoeEventType 11 变体覆盖
- [x] C36 T3 EventPriority 排序
- [x] C37 T4 SoeConfig 默认值
- [x] C38 T5 InMemorySoeStorage append + query_by_time
- [x] C39 T6 InMemorySoeStorage query_by_device
- [x] C40 T7 InMemorySoeStorage get_latest
- [x] C41 T8 InMemorySoeStorage mark_uploaded + get_unuploaded
- [x] C42 T9 InMemorySoeStorage delete_before
- [x] C43 T10 MockUploadChannel 上传统计
- [x] C44 T11 DigitalChangeTrigger 检测变位
- [x] C45 T12 DigitalChangeTrigger 同值不触发
- [x] C46 T13 OverLimitTrigger 越上限触发
- [x] C47 T14 OverLimitTrigger 越下限触发
- [x] C48 T15 OverLimitTrigger 恢复事件触发
- [x] C49 T16 SoeEngine record_event 分配 event_id 递增
- [x] C50 T17 SoeEngine 事件不乱序（乱序入队，按时间戳出队）
- [x] C51 T18 SoeEngine process_point_change 触发多事件
- [x] C52 T19 SoeEngine upload_events 流程
- [x] C53 T20 SoeEngine cleanup_expired 清理过期事件

## v0.53.0 SOE 引擎 — 设计文档
- [x] C54 `docs/protocols/soe-engine-design.md` 存在
- [x] C55 文档包含 12 章节 + Mermaid 架构图 + 流程图
- [x] C56 文档位置在 `docs/protocols/` 下

## v0.53.1 MQTT 客户端 — Crate 骨架 + 类型
- [x] C57 `crates/protocols/mqtt/Cargo.toml` 存在，package name 为 `eneros-mqtt`
- [x] C58 dependencies 仅 `eneros-upa-model`（D11~D18）
- [x] C59 `src/lib.rs` 包含 no_std + alloc
- [x] C60 模块声明完整：error / qos / packet / transport / client / will / topic
- [x] C61 D11~D18 偏差声明表存在
- [x] C62 `MqttError` 枚举包含 8+ 错误变体
- [x] C63 `QoS` 枚举（AtMostOnce=0/AtLeastOnce=1/ExactlyOnce=2）
- [x] C64 `LastWill` 结构体（topic/payload/qos/retain）
- [x] C65 `TopicFilter::matches()` 支持 + 和 # 通配符

## v0.53.1 MQTT 客户端 — 报文编解码
- [x] C66 `MqttPacket` 枚举覆盖 14 种控制报文
- [x] C67 `encode()` 实现变长剩余长度编码
- [x] C68 `decode()` 实现报文解析
- [x] C69 CONNECT 报文包含 ClientID/Will/Credentials
- [x] C70 PUBLISH 报文包含 QoS/Retain/DUP 标志
- [x] C71 SUBSCRIBE 报文包含 Topic 过滤器列表 + QoS

## v0.53.1 MQTT 客户端 — MqttTransport + MqttClient
- [x] C72 `MqttTransport` trait 定义 connect/send/recv/close/is_connected
- [x] C73 `MockTransport` 实现内存缓冲
- [x] C74 `MqttClient` 状态机包含 Disconnected/Connecting/Connected/Reconnecting
- [x] C75 `connect()` 发送 CONNECT 等待 CONNACK
- [x] C76 `publish()` 实现 QoS 0/1/2
- [x] C77 `subscribe()` / `unsubscribe()` 实现
- [x] C78 `ping()` PINGREQ/PINGRESP
- [x] C79 `disconnect()` 发送 DISCONNECT
- [x] C80 `poll()` 接收并处理入站报文
- [x] C81 `try_reconnect()` 指数退避（初始 1s，最大 30s）+ 自动恢复订阅
- [x] C82 QoS 1 等待 PUBACK；QoS 2 四次握手（PUBREC/PUBREL/PUBCOMP）

## v0.53.1 MQTT 客户端 — 集成测试 + 文档
- [x] C83 T1 QoS 枚举值
- [x] C84 T2 LastWill 构造
- [x] C85 T3 TopicFilter 精确匹配
- [x] C86 T4 TopicFilter + 单层通配符
- [x] C87 T5 TopicFilter # 多层通配符
- [x] C88 T6 CONNECT 报文编码
- [x] C89 T7 CONNACK 报文解码
- [x] C90 T8 PUBLISH QoS 0 报文编解码
- [x] C91 T9 PUBLISH QoS 1 报文编解码
- [x] C92 T10 SUBSCRIBE 报文编码
- [x] C93 T11 PINGREQ/PINGRESP 报文编解码
- [x] C94 T12 MqttClient connect + publish QoS 0
- [x] C95 T13 MqttClient subscribe
- [x] C96 T14 MqttClient publish QoS 1 等待 PUBACK
- [x] C97 T15 MqttClient 指数退避重连
- [x] C98 `docs/protocols/mqtt-client-design.md` 存在
- [x] C99 文档包含 12 章节 + Mermaid 状态机图 + QoS 2 时序图
- [x] C100 文档位置在 `docs/protocols/` 下

## v0.53.2 告警管理 — Crate 骨架 + 类型
- [x] C101 `crates/agents/alarm/Cargo.toml` 存在，package name 为 `eneros-alarm`
- [x] C102 dependencies 包含 `eneros-upa-model`（D19~D25）
- [x] C103 `src/lib.rs` 包含 no_std + alloc
- [x] C104 模块声明完整：error / record / level / rule / suppression / escalation / manager
- [x] C105 D19~D25 偏差声明表存在
- [x] C106 `AlarmError` 枚举（NotFound/AlreadyAcknowledged/AlreadyCleared/AlreadyEscalated/InvalidLevel/Suppressed）
- [x] C107 `AlarmLevel` 枚举（Info/Warning/Critical/Emergency）派生 PartialOrd/Ord
- [x] C108 `AlarmId = u64` 类型别名
- [x] C109 `AlarmRecord` 结构体（id/level/source/description/raised_at_ms/acknowledged_at_ms/cleared_at_ms/escalated_from/state）
- [x] C110 `AlarmState` 枚举（Active/Acknowledged/Cleared）

## v0.53.2 告警管理 — 抑制 + 升级
- [x] C111 `SuppressionRule` 结构体（source_pattern/duration_ms/max_count）
- [x] C112 `SuppressionWindow` 使用 VecDeque<u64> 时间戳队列（D21）
- [x] C113 `SuppressionWindow::should_suppress()` 滑动窗口逻辑
- [x] C114 `EscalationPolicy` 结构体（from_level/to_level/timeout_ms）
- [x] C115 `EscalationPolicy::check_escalation()` 返回 Option<AlarmLevel>

## v0.53.2 告警管理 — AlarmManager
- [x] C116 `AlarmManager` 使用 `BTreeMap<AlarmId, AlarmRecord>` active 表（D20）
- [x] C117 `raise()` 先检查抑制再生成告警
- [x] C118 `acknowledge()` Active → Acknowledged
- [x] C119 `clear()` 任意状态 → Cleared，转入 history
- [x] C120 `escalate()` 升级级别，记录 escalated_from
- [x] C121 `check_auto_escalate()` 批量检查所有 Active 告警
- [x] C122 `query_active()` / `query_history()` 查询方法
- [x] C123 `stats()` 返回告警统计

## v0.53.2 告警管理 — 集成测试 + 文档
- [x] C124 T1 AlarmLevel 排序
- [x] C125 T2 AlarmRecord 构造
- [x] C126 T3 AlarmState 状态转换
- [x] C127 T4 AlarmManager raise + query_active
- [x] C128 T5 AlarmManager acknowledge
- [x] C129 T6 AlarmManager clear 后转入 history
- [x] C130 T7 未知 ID acknowledge 返回 NotFound
- [x] C131 T8 已 Cleared 再次 clear 返回 AlreadyCleared
- [x] C132 T9 SuppressionRule 同源 N 秒内第 2 次抑制
- [x] C133 T10 SuppressionRule 超时窗口后允许
- [x] C134 T11 EscalationPolicy 检测超时升级
- [x] C135 T12 AlarmManager escalate 手动升级
- [x] C136 T13 AlarmManager check_auto_escalate 批量升级
- [x] C137 T14 AlarmManager query_history 时间范围
- [x] C138 T15 AlarmManager stats 统计
- [x] C139 `docs/runtime/alarm-management-design.md` 存在
- [x] C140 文档包含 12 章节 + Mermaid 生命周期图 + 升级流程图
- [x] C141 文档位置在 `docs/runtime/` 下

## 版本号同步
- [x] C142 `Makefile` 版本号 0.52.0 → 0.53.0
- [x] C143 `.github/workflows/ci.yml` 版本号 0.52.0 → 0.53.0
- [x] C144 `ci/src/gate.rs` 补充 v0.53.0/v0.53.1/v0.53.2 三个 crate 注释（clippy + test 两段）

## 构建校验（§2.4.2 C6~C11）
- [x] C145 `cargo metadata --format-version 1` 成功
- [x] C146 `cargo test -p eneros-soe-engine -p eneros-mqtt -p eneros-alarm` 全部通过
- [x] C147 `cargo build -p eneros-soe-engine -p eneros-mqtt -p eneros-alarm --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C148 `cargo fmt -p eneros-soe-engine -p eneros-mqtt -p eneros-alarm -- --check` 格式通过
- [x] C149 `cargo clippy -p eneros-soe-engine -p eneros-mqtt -p eneros-alarm --all-targets -- -D warnings` lint 通过
- [x] C150 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C151 soe-engine 在 `crates/protocols/` 下
- [x] C152 mqtt 在 `crates/protocols/` 下
- [x] C153 alarm 在 `crates/agents/` 下（蓝图明确指定）
- [x] C154 3 个新文档分别在 `docs/protocols/` 和 `docs/runtime/` 下
- [x] C155 无根目录 crate
- [x] C156 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C157 3 个 crate 所有 Rust 代码无 `use std::*`
- [x] C158 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C159 不要求 `Send + Sync`（D7/D17/D24）
