# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.72.0` → `0.73.0`
  - [x] members 添加 `crates/agents/device-agent`（置于 `crates/agents/energy-market-agent` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 外科手术式变更 v0.72.0 `AgentRuntimeError`
  - [x] 修改 `crates/agents/energy-market-agent/src/error.rs`：在 `AgentRuntimeError` 枚举添加 `DeviceError(String)` 变体
  - [x] 验证：v0.72.0 测试仍通过（`cargo test -p eneros-energy-market-agent`）

- [x] Task 3: 创建 `eneros-device-agent` crate 骨架
  - [x] 新建 `crates/agents/device-agent/Cargo.toml`，package name = `eneros-device-agent`
  - [x] dependencies：`eneros-agent` / `eneros-energy-market-agent` + 无 serde（纯 Rust，无序列化需求）
  - [x] 无 `[features]` 段
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / device_type / registry / command / agent
  - [x] lib.rs 包含 D1~D12 偏差声明表

- [x] Task 4: 实现 `error.rs` — DeviceError
  - [x] `DeviceError` 枚举：`DeviceNotFound(String)` / `PointNotFound(String)` / `DeviceOffline(String)` / `WriteFailed(String)` / `ReadFailed(String)`
  - [x] 派生 `Debug`（D8）
  - [x] 实现 `fmt::Display`（便于错误转换）
  - [x] 实现 `From<DeviceError> for AgentRuntimeError`（映射到 `AgentRuntimeError::DeviceError(format!("{:?}", e))`）

- [x] Task 5: 实现 `device_type.rs` — DeviceType + DeviceInfo + DeviceState + DeviceSnapshot
  - [x] `DeviceType` 枚举：Pcs / Battery / Bms / Meter / Temperature，派生 `Debug + Clone + Copy + PartialEq + Eq + Hash`
  - [x] `DeviceState` 结构体：soc / voltage / current / temperature / power / online / last_update_ms，派生 `Debug + Clone + Default`
  - [x] `DeviceSnapshot` 结构体：`states: BTreeMap<String, DeviceState>`，派生 `Debug + Clone`
    - [x] `new() -> Self`
    - [x] `set(&mut self, name: &str, state: DeviceState)`
    - [x] `get(&self, name: &str) -> Option<&DeviceState>`
    - [x] `len(&self) -> usize` / `is_empty(&self) -> bool`

- [x] Task 6: 实现 `registry.rs` — DeviceAdapter trait + MockDevice + DeviceRegistry + DeviceInfo
  - [x] `DeviceAdapter` trait：`read_point(&mut self, name: &str) -> Result<f64, DeviceError>` / `write_point(&mut self, name: &str, value: f64) -> Result<(), DeviceError>` / `device_type(&self) -> DeviceType` / `is_online(&self) -> bool`（D6）
  - [x] `MockDevice` 结构体：`device_type` / `points: BTreeMap<String, f64>` / `online: bool`（D10）
    - [x] `new(device_type) -> Self`
    - [x] `with_point(name, value) -> Self`（链式）
    - [x] `set_point(&mut self, name, value)`
    - [x] `set_online(&mut self, online)`
    - [x] 实现 `DeviceAdapter` trait — `read_point` 查找点位返回值，不存在返回 `PointNotFound`；`is_online() == false` 返回 `DeviceOffline`；`write_point` 更新点位值
  - [x] `DeviceInfo` 结构体：`device_type: DeviceType` / `adapter: Box<dyn DeviceAdapter>`（D11）
  - [x] `DeviceRegistry` 结构体：`devices: BTreeMap<String, DeviceInfo>`（D12）
    - [x] `new() -> Self`
    - [x] `register(&mut self, name, device_type, adapter: Box<dyn DeviceAdapter>)`
    - [x] `get_mut(&mut self, name: &str) -> Option<&mut DeviceInfo>`
    - [x] `len(&self) -> usize` / `is_empty(&self) -> bool`
    - [x] `iter_mut(&mut self)` — 返回 `core::slice::IterMut` 或 BTreeMap iter_mut

- [x] Task 7: 实现 `command.rs` — DeviceCommand + CommandSource + MockCommandSource
  - [x] `DeviceCommand` 结构体：target_device: String / power_kw: f64 / ttl_ms: u64 / timestamp_ms: u64，派生 `Debug + Clone`（D7）
  - [x] `CommandSource` trait：`try_read(&mut self) -> Option<DeviceCommand>`（D4）
  - [x] `MockCommandSource` 结构体：`commands: VecDeque<DeviceCommand>`（D4）
    - [x] `new() -> Self`
    - [x] `with_commands(commands: Vec<DeviceCommand>) -> Self`
    - [x] `push(&mut self, cmd: DeviceCommand)`
    - [x] 实现 `CommandSource` trait — `try_read()` pop_front

- [x] Task 8: 实现 `agent.rs` — DeviceAgent
  - [x] `DeviceAgent` 结构体：descriptor / devices: DeviceRegistry / command_source: Box<dyn CommandSource> / last_snapshot: DeviceSnapshot / state: AgentState / tick_count: u64
  - [x] `new(name, command_source: Box<dyn CommandSource>, now_ms) -> Self`（D3）
  - [x] `new_default(now_ms) -> Self` — MockCommandSource::new() + 预注册 3 个 MockDevice（pcs/battery/meter）
  - [x] `registry_mut(&mut self) -> &mut DeviceRegistry`
  - [x] `last_snapshot(&self) -> &DeviceSnapshot`
  - [x] `poll_devices(&mut self, now_ms: u64) -> DeviceSnapshot`（D5）
    - [x] 遍历 devices.iter_mut()
    - [x] 对每个设备：检查 `is_online()`，若离线则 `DeviceState { online: false, ..Default::default() }`
    - [x] 若在线：`read_point("soc")` / `read_point("voltage")` / `read_point("current")` / `read_point("temperature")` / `read_point("power")`，失败时用 0.0 + online=false
    - [x] 构建 `DeviceState`，`snapshot.set(name, state)`
    - [x] 更新 `self.last_snapshot = snapshot.clone()`，返回 snapshot
  - [x] `execute_commands(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>`（D14）
    - [x] while let Some(cmd) = self.command_source.try_read()
    - [x] 查找 `self.devices.get_mut(&cmd.target_device)`
    - [x] 未找到：跳过（continue）
    - [x] 找到：`device.adapter.write_point("power_setpoint", cmd.power_kw)`，失败记录但不中断
  - [x] 实现 `AgentRuntime` trait（复用 v0.72.0）（D9）
    - [x] `descriptor()` → `&self.descriptor`
    - [x] `on_start(now_ms)` → `self.state = Running; Ok(())`
    - [x] `on_tick(now_ms)` → `poll_devices(now_ms)` + `execute_commands(now_ms)` + `tick_count += 1`，`Ok(())`
    - [x] `on_stop(now_ms)` → `self.state = Dead; Ok(())`
    - [x] `on_heartbeat(now_ms)` → `Running ? Alive : Dead`

- [x] Task 9: 集成测试（lib.rs）— 至少 22 个测试
  - [x] T1 DeviceType 变体构造
  - [x] T2 DeviceState::default 全零
  - [x] T3 DeviceSnapshot::new 空 + set + get
  - [x] T4 MockDevice::new 空 + read_point 未找到
  - [x] T5 MockDevice::with_point 链式 + read_point 成功
  - [x] T6 MockDevice set_point + write_point 成功
  - [x] T7 MockDevice set_online(false) + read_point 返回 DeviceOffline
  - [x] T8 DeviceRegistry::new 空 + register + len
  - [x] T9 DeviceRegistry get_mut 成功
  - [x] T10 DeviceRegistry get_mut 未找到返回 None
  - [x] T11 DeviceCommand 构造
  - [x] T12 MockCommandSource::new 空 + try_read None
  - [x] T13 MockCommandSource::with_commands + try_read 成功
  - [x] T14 MockCommandSource push + try_read
  - [x] T15 DeviceError 变体构造
  - [x] T16 From<DeviceError> for AgentRuntimeError 转换
  - [x] T17 DeviceAgent::new_default 构造 + 预注册 3 设备
  - [x] T18 DeviceAgent::on_start 状态转 Running
  - [x] T19 DeviceAgent::on_tick 采集设备状态（soc=0.65）
  - [x] T20 DeviceAgent::on_tick 执行命令（power_kw=50.0）
  - [x] T21 DeviceAgent::on_tick 设备离线标记
  - [x] T22 DeviceAgent::on_tick 命令目标设备不存在跳过
  - [x] T23 DeviceAgent::on_stop 状态转 Dead
  - [x] T24 DeviceAgent::on_heartbeat Running → Alive / Dead

- [x] Task 10: 创建设计文档 `docs/agents/device-agent-design.md`
  - [x] 12 章节完整
  - [x] 2 Mermaid 图（Device Agent tick 流程图 + 设备状态采集时序图）
  - [x] D1~D12 偏差声明表
  - [x] 文档位于 `docs/agents/` 下

- [x] Task 11: 版本同步
  - [x] `Makefile` 版本号 `0.73.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.73.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-device-agent`

- [x] Task 12: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-device-agent` 全部通过
  - [x] `cargo test -p eneros-energy-market-agent` 全部通过（回归验证 D8 变更）
  - [x] `cargo build -p eneros-device-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-device-agent -- --check` 通过
  - [x] `cargo clippy -p eneros-device-agent --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 可与 Task 1 并行
- Task 3 依赖 Task 1 + Task 2
- Task 4~8 依赖 Task 3（并行实现）
- Task 9 依赖 Task 4~8
- Task 10 可与 Task 4~9 并行
- Task 11 依赖 Task 3
- Task 12 依赖 Task 4~11 全部完成
