# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.73.0`
- [x] C2 members 列表已添加 `crates/agents/device-agent`（置于 `crates/agents/energy-market-agent` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## v0.72.0 外科手术式变更（D8）
- [x] C4 `crates/agents/energy-market-agent/src/error.rs` 的 `AgentRuntimeError` 添加 `DeviceError(String)` 变体
- [x] C5 `cargo test -p eneros-energy-market-agent` 回归通过（v0.72.0 测试不受影响）

## Crate 骨架
- [x] C6 `crates/agents/device-agent/Cargo.toml` 存在，package name = `eneros-device-agent`
- [x] C7 dependencies 包含 `eneros-agent` + `eneros-energy-market-agent`
- [x] C8 无 `[features]` 段（纯 Rust）
- [x] C9 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C10 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C11 模块声明：error / device_type / registry / command / agent

## error.rs — DeviceError
- [x] C12 `DeviceError` 枚举：`DeviceNotFound(String)` / `PointNotFound(String)` / `DeviceOffline(String)` / `WriteFailed(String)` / `ReadFailed(String)`
- [x] C13 派生 `Debug`（D8）
- [x] C14 实现 `fmt::Display`
- [x] C15 实现 `From<DeviceError> for AgentRuntimeError`（映射到 `DeviceError(String)`）

## device_type.rs — DeviceType + DeviceState + DeviceSnapshot
- [x] C16 `DeviceType` 枚举：5 变体（Pcs/Battery/Bms/Meter/Temperature），派生 `Debug + Clone + Copy + PartialEq + Eq + Hash`
- [x] C17 `DeviceState` 结构体：7 字段（soc/voltage/current/temperature/power/online/last_update_ms），派生 `Debug + Clone + Default`
- [x] C18 `DeviceSnapshot` 结构体：`states: BTreeMap<String, DeviceState>`，派生 `Debug + Clone`
- [x] C19 `DeviceSnapshot::new()` 创建空快照
- [x] C20 `DeviceSnapshot::set(name, state)` 设置设备状态
- [x] C21 `DeviceSnapshot::get(name)` 返回 `Option<&DeviceState>`
- [x] C22 `DeviceSnapshot::len()` / `is_empty()`

## registry.rs — DeviceAdapter + MockDevice + DeviceInfo + DeviceRegistry
- [x] C23 `DeviceAdapter` trait：`read_point` / `write_point` / `device_type` / `is_online`（D6）
- [x] C24 `MockDevice` 结构体：device_type / points: BTreeMap<String, f64> / online: bool（D10）
- [x] C25 `MockDevice::new(device_type)` 创建空设备
- [x] C26 `MockDevice::with_point(name, value)` 链式添加点位
- [x] C27 `MockDevice::set_point` / `set_online`
- [x] C28 `MockDevice` 实现 `DeviceAdapter` — `read_point` 查找点位，不存在返回 `PointNotFound`，离线返回 `DeviceOffline`
- [x] C29 `MockDevice` 实现 `write_point` — 更新点位值
- [x] C30 `DeviceInfo` 结构体：device_type + adapter: Box<dyn DeviceAdapter>（D11）
- [x] C31 `DeviceRegistry` 结构体：devices: BTreeMap<String, DeviceInfo>（D12）
- [x] C32 `DeviceRegistry::new()` / `register()` / `get_mut()` / `len()` / `is_empty()` / `iter_mut()`

## command.rs — DeviceCommand + CommandSource + MockCommandSource
- [x] C33 `DeviceCommand` 结构体：target_device / power_kw / ttl_ms / timestamp_ms，派生 `Debug + Clone`（D7）
- [x] C34 `CommandSource` trait：`try_read(&mut self) -> Option<DeviceCommand>`（D4）
- [x] C35 `MockCommandSource` 结构体：commands: VecDeque<DeviceCommand>
- [x] C36 `MockCommandSource::new()` / `with_commands()` / `push()`
- [x] C37 `MockCommandSource` 实现 `CommandSource` — `try_read()` pop_front

## agent.rs — DeviceAgent
- [x] C38 `DeviceAgent` 结构体：6 字段（descriptor / devices / command_source / last_snapshot / state / tick_count）
- [x] C39 `new(name, command_source, now_ms)` — `AgentDescriptor::new(AgentType::Device, name, now_ms)`（D3）
- [x] C40 `new_default(now_ms)` — MockCommandSource + 预注册 3 MockDevice（pcs/battery/meter）
- [x] C41 `registry_mut()` 返回 `&mut DeviceRegistry`
- [x] C42 `last_snapshot()` 返回 `&DeviceSnapshot`
- [x] C43 `poll_devices(now_ms)` — 遍历设备，read_point 采集 soc/voltage/current/temperature/power（D5/D6）
- [x] C44 `poll_devices` 离线设备标记 `online: false`
- [x] C45 `poll_devices` read_point 失败用 0.0 + online=false（不中断）
- [x] C46 `execute_commands(now_ms)` — while try_read，查找设备，write_point("power_setpoint", power_kw)（D7）
- [x] C47 `execute_commands` 设备未找到跳过（D14，不中断）
- [x] C48 `execute_commands` 写入失败记录但不中断（D14）
- [x] C49 实现 `AgentRuntime::descriptor()`（D9 复用 v0.72.0）
- [x] C50 实现 `on_start(now_ms)` — `state = Running`
- [x] C51 实现 `on_tick(now_ms)` — `poll_devices` + `execute_commands` + `tick_count += 1`
- [x] C52 实现 `on_stop(now_ms)` — `state = Dead`
- [x] C53 实现 `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`

## 集成测试（lib.rs）
- [x] C54 T1 DeviceType 变体构造
- [x] C55 T2 DeviceState::default 全零
- [x] C56 T3 DeviceSnapshot new/set/get
- [x] C57 T4 MockDevice::new 空 + read_point 未找到
- [x] C58 T5 MockDevice::with_point + read_point 成功
- [x] C59 T6 MockDevice set_point + write_point 成功
- [x] C60 T7 MockDevice 离线 + read_point 返回 DeviceOffline
- [x] C61 T8 DeviceRegistry::new 空 + register + len
- [x] C62 T9 DeviceRegistry get_mut 成功
- [x] C63 T10 DeviceRegistry get_mut 未找到 None
- [x] C64 T11 DeviceCommand 构造
- [x] C65 T12 MockCommandSource::new 空 + try_read None
- [x] C66 T13 MockCommandSource::with_commands + try_read 成功
- [x] C67 T14 MockCommandSource push + try_read
- [x] C68 T15 DeviceError 变体构造
- [x] C69 T16 From<DeviceError> for AgentRuntimeError 转换
- [x] C70 T17 DeviceAgent::new_default 构造 + 预注册 3 设备
- [x] C71 T18 DeviceAgent::on_start 状态转 Running
- [x] C72 T19 DeviceAgent::on_tick 采集设备状态（soc=0.65）
- [x] C73 T20 DeviceAgent::on_tick 执行命令（power_kw=50.0）
- [x] C74 T21 DeviceAgent::on_tick 设备离线标记
- [x] C75 T22 DeviceAgent::on_tick 命令目标不存在跳过
- [x] C76 T23 DeviceAgent::on_stop 状态转 Dead
- [x] C77 T24 DeviceAgent::on_heartbeat Running → Alive / Dead
- [x] C78 `cargo test -p eneros-device-agent` 全部通过

## 设计文档
- [x] C79 `docs/agents/device-agent-design.md` 存在
- [x] C80 12 章节完整
- [x] C81 2 Mermaid 图（Device Agent tick 流程图 + 设备状态采集时序图）
- [x] C82 D1~D12 偏差声明表
- [x] C83 文档在 `docs/agents/` 下

## 版本同步
- [x] C84 `Makefile` 版本号 `0.73.0`（header + VERSION 变量 2 处）
- [x] C85 `.github/workflows/ci.yml` 版本号 `0.73.0`
- [x] C86 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-device-agent`

## 构建校验（§2.4.2 C6~C11）
- [x] C87 `cargo metadata --format-version 1` 成功
- [x] C88 `cargo test -p eneros-device-agent` 全部通过
- [x] C89 `cargo test -p eneros-energy-market-agent` 回归通过（D8 变更）
- [x] C90 `cargo build -p eneros-device-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C91 `cargo fmt -p eneros-device-agent -- --check` 通过
- [x] C92 `cargo clippy -p eneros-device-agent --all-targets -- -D warnings` 无 warning
- [x] C93 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C94 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C95 无 `panic!` / `todo!` / `unimplemented!`
- [x] C96 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C97 无 `unsafe` 块
- [x] C98 无 `SystemTime::now()` / `uuid::Uuid::new_v4()`（D2）
- [x] C99 无 `log::warn!` / `log::info!` / `log::error!`（D1）
- [x] C100 无 `std::collections::HashMap` / `std::sync::Mutex`（D12，使用 BTreeMap）

## 目录规范
- [x] C101 crate 在 `crates/agents/device-agent/`
- [x] C102 跨 crate path 引用均为相对路径
- [x] C103 文档在 `docs/agents/` 下
- [x] C104 无根目录 crate（除 `ci/`）
- [x] C105 无垃圾文件

## 依赖复用
- [x] C106 复用 v0.72.0 `AgentRuntime` / `HeartbeatStatus` / `AgentRuntimeError`（D9）
- [x] C107 复用 v0.33.0 `AgentDescriptor` / `AgentType` / `AgentState` / `TrustLevel` / `AgentError` / `AgentId`

## 简化设计验证（Karpathy 原则）
- [x] C108 `DeviceAdapter` trait 本地定义（D6：不依赖复杂 PointAccess）
- [x] C109 `MockDevice` 简单 BTreeMap-backed（D10：无 PointMap 依赖）
- [x] C110 `CommandSource` trait + Mock（D4：无 ControlBusReader 依赖）
- [x] C111 `DeviceSnapshot` 直接返回（D5：无 SharedMemoryHandle 依赖）
- [x] C112 `DeviceCommand` 本地定义（D7：v0.55.0 ControlCommand 结构不同）
- [x] C113 `DeviceError` 本地定义（D8：AgentError 缺变体）
- [x] C114 `BTreeMap` 替代 `HashMap`（D12：no_std 合规）
- [x] C115 错误不中断执行（D14 延续：poll/execute 容错）
