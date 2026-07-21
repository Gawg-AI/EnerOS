# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.58.0`
- [x] C2 members 列表已添加 `crates/kernel/rtos-watchdog-degrade`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/kernel/rtos-watchdog-degrade/Cargo.toml` 存在，package name 为 `eneros-rtos-watchdog-degrade`
- [x] C5 dependencies 包含 `eneros-protocol-abstract` + `eneros-upa-model` + `eneros-controlbus` + `eneros-rtos-cmd-exec` + `eneros-rtos-degrade` + `eneros-watchdog`（path 引用正确）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / state / config / heartbeat / recovery / stats / flow / mock
- [x] C8 D1~D12 偏差声明表存在于 lib.rs

## FlowError 错误类型
- [x] C9 `FlowError` 枚举包含 PointWriteFailed / HeartbeatNotRegistered / RecoveryNotInProgress
- [x] C10 实现 `Display` + `Debug`

## DegradeState 状态机
- [x] C11 `DegradeState` 枚举包含 Normal / Degrading / Degraded / Recovering / Emergency
- [x] C12 派生 `Debug / Clone / Copy / PartialEq / Eq`
- [x] C13 `is_degraded()` 方法（Normal=false，其余=true）
- [x] C14 单元测试 — is_degraded

## DegradeConfig
- [x] C15 `DegradeConfig` 结构体包含 6 个字段（heartbeat_period_ms / heartbeat_timeout_count / recovery_transition_ms / watchdog_hard_timeout_ms / power_setpoint_point / power_cmd_point）
- [x] C16 `Default` 实现（1s / 3 / 30s / 10s / PointId(0) / PointId(0)）
- [x] C17 单元测试 — 默认值

## HeartbeatWatcher + HeartbeatStatus
- [x] C18 `HeartbeatStatus` 枚举包含 Alive / Timeout(u8) / Dead
- [x] C19 `HeartbeatWatcher` 结构体包含 5 字段（heartbeat_period_ns / last_heartbeat_ns / timeout_count / max_timeout / agent_alive）
- [x] C20 `new(heartbeat_period_ns, max_timeout)` 构造
- [x] C21 `on_heartbeat(now_ns)` — D3 now_ns 注入
- [x] C22 `check(now_ns) -> HeartbeatStatus` — D3 now_ns 注入
- [x] C23 `is_alive()` 访问器
- [x] C24 单元测试 — Alive / Timeout / Dead / 恢复

## RecoveryManager（纯状态，D10）
- [x] C25 `RecoveryManager` 结构体包含 6 字段（saved_setpoint / transition_start_ns / transition_duration_ns / progress / degraded_setpoint / agent_setpoint）
- [x] C26 `new(transition_duration_ns)` 构造
- [x] C27 `save_setpoint(current_value)` 方法
- [x] C28 `start_transition(degraded_value, agent_value, now_ns)` 方法
- [x] C29 `transition_step(now_ns) -> Option<f64>` 方法（线性插值，完成返回 None）
- [x] C30 `is_complete() -> bool` 方法
- [x] C31 `complete()` 方法
- [x] C32 不持有 protocol（D10 纯状态）
- [x] C33 单元测试 — 保存/启动/插值/完成

## FlowStats + FlowReport
- [x] C34 `FlowStats` 结构体包含 6 字段（state_transitions / emergency_count / recovery_count / heartbeat_timeouts / degrade_evaluations / cmds_executed）
- [x] C35 `FlowReport` 结构体包含 6 字段（state / state_changed / heartbeat / cmd_report / degrade_report / watchdog）
- [x] C36 不使用 AtomicU64（D4）
- [x] C37 单元测试 — 累加

## 外科手术式扩展 v0.57.0（D10）
- [x] C38 `crates/kernel/rtos-degrade/src/engine.rs` 新增 `pub fn protocol_mut(&mut self) -> &mut P`
- [x] C39 v0.57.0 的 16 个测试全部保持通过（回归测试）
- [x] C40 `cargo test -p eneros-rtos-degrade` 通过

## WatchdogDegradeFlow（核心编排器）
- [x] C41 `WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider>` 泛型结构体（D6）
- [x] C42 字段完整：state / heartbeat / degrade_engine / cmd_executor / recovery / watchdog / config / stats
- [x] C43 `new(degrade_engine, cmd_executor, watchdog, config)` 构造（初始 Normal）
- [x] C44 `tick(&mut self, ctx: &DegradeContext) -> FlowReport`（D11 单步驱动）
- [x] C45 tick 中调用 `cmd_executor.tick(ctx.now_ns)`（D7，非 process_commands）
- [x] C46 tick 中调用 `degrade_engine.evaluate(ctx, ctx.now_ns)`（D8，非 evaluate(context)）
- [x] C47 tick 中通过 `degrade_engine.protocol_mut()` 写入插值结果（D9/D10）
- [x] C48 `evaluate_state_transition(heartbeat) -> DegradeState` 状态机转换逻辑
- [x] C49 Normal → Degrading（心跳 Dead）
- [x] C50 Degrading → Degraded（下一 tick）
- [x] C51 Degraded → Recovering（心跳 Alive）
- [x] C52 Recovering → Normal（过渡完成）
- [x] C53 Recovering → Degraded（恢复中再次崩溃，风险 8.4）
- [x] C54 任意 → Emergency（watchdog HardReset）
- [x] C55 `on_state_transition(from, to, now_ns)` 转换动作
- [x] C56 Normal → Degrading 时调用 `recovery.save_setpoint()`
- [x] C57 Degraded → Recovering 时调用 `recovery.start_transition()`
- [x] C58 Recovering → Normal 时调用 `recovery.complete()`
- [x] C59 `state()` / `stats()` 访问器
- [x] C60 单元测试 — 各状态转换

## 喂狗策略（D1 复用 v0.13.0 Watchdog）
- [x] C61 `new` 中注册 3 层（kernel / runtime / agent）
- [x] C62 Normal 状态喂 3 层（kernel + runtime + agent）
- [x] C63 Degrading/Degraded/Recovering 状态喂 2 层（kernel + runtime，跳过 agent）
- [x] C64 Emergency 状态不喂狗
- [x] C65 `watchdog.check(now_ns) -> WatchdogStatus` 返回 HardReset 时转 Emergency

## MockPointAccess + MockDeviceStateProvider
- [x] C66 `MockPointAccess` 结构体（BTreeMap<PointId, PointValue>）
- [x] C67 实现 `PointAccess` trait 全部 6 个方法
- [x] C68 `MockDeviceStateProvider` 实现 `DeviceStateProvider` trait
- [x] C69 编译通过（在测试中使用）

## 集成测试
- [x] C70 T1 DegradeState is_degraded
- [x] C71 T2 HeartbeatWatcher Alive
- [x] C72 T3 HeartbeatWatcher Timeout
- [x] C73 T4 HeartbeatWatcher Dead
- [x] C74 T5 HeartbeatWatcher 恢复
- [x] C75 T6 RecoveryManager 线性插值
- [x] C76 T7 RecoveryManager is_complete + complete
- [x] C77 T8 WatchdogDegradeFlow Normal 执行 cmd_executor.tick
- [x] C78 T9 WatchdogDegradeFlow Normal → Degrading → Degraded
- [x] C79 T10 WatchdogDegradeFlow Degraded → Recovering → Normal
- [x] C80 T11 WatchdogDegradeFlow Recovering → Degraded（恢复中再次崩溃）
- [x] C81 T12 WatchdogDegradeFlow Emergency
- [x] C82 T13 WatchdogDegradeFlow 喂狗层级
- [x] C83 T14 FlowStats 累加
- [x] C84 T15 DegradeConfig 默认值

## 设计文档
- [x] C85 `docs/kernel/watchdog-degrade-flow-design.md` 存在
- [x] C86 文档包含 12 章节
- [x] C87 文档包含 2 Mermaid 图（端到端降级状态机图 + tick 时序图）
- [x] C88 D1~D12 偏差声明表
- [x] C89 文档位置在 `docs/kernel/` 下

## 版本号同步
- [x] C90 `Makefile` 版本号 0.57.0 → 0.58.0
- [x] C91 `.github/workflows/ci.yml` 版本号 0.57.0 → 0.58.0
- [x] C92 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-watchdog-degrade` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C93 `cargo metadata --format-version 1` 成功
- [x] C94 `cargo test -p eneros-rtos-watchdog-degrade` 全部通过
- [x] C95 `cargo test -p eneros-rtos-degrade` 全部通过（v0.57.0 回归）
- [x] C96 `cargo build -p eneros-rtos-watchdog-degrade --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C97 `cargo fmt -p eneros-rtos-watchdog-degrade -- --check` 格式通过
- [x] C98 `cargo clippy -p eneros-rtos-watchdog-degrade --all-targets -- -D warnings` lint 通过
- [x] C99 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C100 rtos-watchdog-degrade 在 `crates/kernel/` 下（子系统归属正确）
- [x] C101 跨 crate path 引用使用相对路径
- [x] C102 设计文档在 `docs/kernel/` 下
- [x] C103 无根目录 crate
- [x] C104 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C105 所有 Rust 代码无 `use std::*`
- [x] C106 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C107 不要求 `Send + Sync`（D6 泛型，单线程）
- [x] C108 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`

## Karpathy 原则校验
- [x] C109 不新建 `WatchdogFeeder`（D1 复用 v0.13.0 Watchdog）
- [x] C110 不复用 v0.37.0 `HeartbeatMonitor`（D2 本地轻量 HeartbeatWatcher）
- [x] C111 不使用 `MonotonicTime::now()`（D3 now_ns 注入）
- [x] C112 不使用 `log_*!`（D4 stats 计数器）
- [x] C113 不使用 `Duration` 类型（D5 u64 毫秒/纳秒）
- [x] C114 不使用 `Box<dyn PointAccess>`（D6 泛型）
- [x] C115 不调用 `process_commands()`（D7 使用 tick(now_ns)）
- [x] C116 不调用 `evaluate(context)`（D8 使用 evaluate(ctx, now_ns)）
- [x] C117 不使用 `POWER_SETPOINT_ID`/`POWER_CMD_ID`（D9 DevicePointMap + config 字段）
- [x] C118 RecoveryManager 不持有 protocol（D10 纯状态）
- [x] C119 tick 签名为 `tick(&mut self, ctx: &DegradeContext) -> FlowReport`（D11）
- [x] C120 Emergency 不自动恢复（D12 对应蓝图风险 8.4/8.6）
