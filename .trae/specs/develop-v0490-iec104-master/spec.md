# v0.49.0 IEC 104 主站 Spec

## Why

v0.48.0 完成了 IEC 60870-5-104 从站（被控设备侧）。电力调度控制中心需要主站侧主动发起通信：周期性总召唤读取全量遥测遥信、下发遥控命令、下发时钟同步命令统一全网时间。本版本实现主站侧，与 v0.48.0 从站构成完整的 IEC 104 主从互通链路，解锁 P1-F 协议栈第七层完整能力。

## What Changes

- **新增 crate** `eneros-iec104-master`，置于 `crates/protocols/iec104-master/`
- **依赖** `eneros-iec104-slave`（path = "../iec104-slave"），复用 APDU/ASDU/TypeId/Cot/InformationObject/IoValue/QualityDescriptor/Sco/Dco/TimeTag/SinglePointValue/DoublePointValue/Iec104Error 等类型
- **新增** `Iec104Master` 主站结构体，支持多设备并发连接（BTreeMap<u16, MasterConnection>）
- **新增** `MasterTransport` trait —— 传输层抽象（connect/send/recv/close/now_ms），解耦 smoltcp
- **新增** `RemoteDevice`、`ConnState`、`MasterConfig`、`MasterStats`、`PollTask` 类型
- **新增** `MasterConnection` 内部连接管理（send_seq/recv_seq/state/pending_acks）
- **实现** connect() / interrogation() / clock_sync() / send_single_command() / send_double_command() / poll() 方法
- **新增** mock 传输层 `MockMasterTransport` 用于测试
- **新增** 设计文档 `docs/protocols/iec104-master-design.md`
- **更新** 根 `Cargo.toml`：版本号 0.48.0 → 0.49.0，members 增加 `"crates/protocols/iec104-master"`
- **更新** `Makefile` / `ci.yml` / `gate.rs` 版本号同步

## Impact

- **Affected specs**: v0.48.0（iec104-slave，作为依赖被复用）
- **Affected code**:
  - `e:\eneros\Cargo.toml` — workspace 版本号 + members 列表
  - `e:\eneros\crates\protocols\iec104-master\` — 新 crate 全部源码
  - `e:\eneros\docs\protocols\iec104-master-design.md` — 设计文档
  - `e:\eneros\Makefile` / `e:\eneros\.github\workflows\ci.yml` / `e:\eneros\ci\src\gate.rs` — 版本号
- **依赖关系**: v0.49.0 完成后解锁后续 P1-F 协议栈版本及 Phase 1 末尾版本

## ADDED Requirements

### Requirement: IEC 104 主站协议栈

系统 SHALL 提供 IEC 60870-5-104 主站实现，主动发起通信，支持多设备并发连接、周期性总召唤、遥控命令下发、时钟同步命令下发。

#### Scenario: 主站连接从站并完成 STARTDT 握手

- **WHEN** 主站调用 `connect(device)` 连接远端设备
- **THEN** 传输层建立 TCP 连接，主站发送 STARTDT_ACT U 格式帧
- **WHEN** 主站收到 STARTDT_CON
- **THEN** 连接状态转为 Connected，可开始数据传输

#### Scenario: 周期性总召唤

- **WHEN** 连接处于 Connected 状态且距上次总召唤超过 poll_interval_ms
- **THEN** 主站发送 InterrogationCommand ASDU（COT=Activation，QOI=20 站召唤）
- **WHEN** 收到从站总召唤响应数据
- **THEN** 主站正确解析遥测遥信数据并更新统计

#### Scenario: 遥控命令下发

- **WHEN** 主站调用 `send_single_command(common_addr, ioa, value)`
- **THEN** 主站发送 SingleCommand ASDU（COT=Activation）
- **WHEN** 从站回复激活确认
- **THEN** 遥控流程完成

#### Scenario: 时钟同步

- **WHEN** 主站调用 `clock_sync(common_addr)`
- **THEN** 主站发送 ClockSyncCommand ASDU（带 CP56Time2a 时标）
- **WHEN** 从站回复确认
- **THEN** 时钟同步完成

#### Scenario: 多设备并发轮询

- **WHEN** 主站配置多个远端设备并调用 `poll(now_ms)`
- **THEN** 每个设备按各自 poll_interval_ms 独立触发总召唤
- **AND** 每个设备独立维护连接状态与序列号

#### Scenario: 连接保活

- **WHEN** 连续 t3 超时无数据收发
- **THEN** 主站发送 TESTFR_ACT U 格式帧
- **WHEN** 收到 TESTFR_CON
- **THEN** 连接保活成功

### Requirement: MasterTransport 传输层抽象

系统 SHALL 提供 `MasterTransport` trait 抽象传输层，使主站与底层网络栈解耦。

#### Scenario: 传输层注入

- **WHEN** 创建 Iec104Master 时注入 MasterTransport 实现
- **THEN** 主站通过 trait 方法 connect/send/recv/close/now_ms 操作网络
- **AND** 不直接依赖 smoltcp 或 std::net

## MODIFIED Requirements

### Requirement: IEC 104 协议族完整性

v0.48.0 仅实现从站侧。v0.49.0 补全主站侧，使 EnerOS 能同时扮演控制中心（主站）与被控设备（从站）角色。

## 偏差声明（D1~D11）

| 偏差 | 说明 |
|------|------|
| **D1** | 定义本地 `MasterTransport` trait（connect/send/recv/close/now_ms），解耦 smoltcp，类比 v0.48.0 `SlaveTransport` |
| **D2** | 时间通过 `now_ms: u64` 参数注入（无 `MonotonicTime` 类型，与 v0.48.0 D3 一致） |
| **D3** | 超时/间隔使用 `u32` 毫秒（无 `Duration` 类型，与 v0.48.0 D5 一致） |
| **D4** | 不依赖 `eneros-net`/smoltcp（传输层由 trait 抽象，与 v0.48.0 D8 一致） |
| **D5** | 复用 `eneros-iec104-slave` 的 APDU/ASDU/TypeId/Cot 等类型（path 依赖，类比 v0.46.0 modbus-tcp 复用 v0.45.0 modbus-rtu） |
| **D6** | crate 放入 `crates/protocols/iec104-master/`（与 iec104-slave/modbus-rtu/modbus-tcp 同级） |
| **D7** | 不实现 `DeviceDriver` trait（协议栈非设备驱动，与 v0.48.0 D9 一致） |
| **D8** | IP 地址用 `[u8; 4]` 表示 IPv4（无 `std::net::IpAddr`，与 v0.46.0 `TcpDevice` 一致） |
| **D9** | `SocketHandle` 抽象为 `ConnId = u32`（传输层 trait 返回连接 ID，主站按 ID 操作） |
| **D10** | `PollScheduler` 简化为基于 now_ms 的时间戳比较（无定时器对象，Simplicity First） |
| **D11** | 时钟同步时标由调用方通过 `now_ms` 参数注入并构造 `TimeTag`（不在主站内部获取系统时间） |
