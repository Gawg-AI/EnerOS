# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.57.0` → `0.58.0`
  - [x] members 添加 `crates/kernel/rtos-watchdog-degrade`
  - [x] `cargo metadata --format-version 1` 验证 workspace 解析成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-rtos-watchdog-degrade` crate 骨架
  - [x] 新建 `crates/kernel/rtos-watchdog-degrade/Cargo.toml`，package name = `eneros-rtos-watchdog-degrade`
  - [x] dependencies：`eneros-protocol-abstract`（path = `../../protocols/protocol-abstract`）+ `eneros-upa-model`（path = `../../protocols/upa-model`）+ `eneros-controlbus`（path = `../controlbus`）+ `eneros-rtos-cmd-exec`（path = `../rtos-cmd-exec`）+ `eneros-rtos-degrade`（path = `../rtos-degrade`）+ `eneros-watchdog`（path = `../../drivers/watchdog`）
  - [x] 新建 `src/lib.rs`，包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 模块声明：error / state / config / heartbeat / recovery / stats / flow / mock
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 3: 实现 `error.rs` — FlowError 错误类型
  - [x] `FlowError` 枚举：PointWriteFailed / HeartbeatNotRegistered / RecoveryNotInProgress
  - [x] 实现 `Display` + `Debug`
  - [x] 验证：`cargo build -p eneros-rtos-watchdog-degrade` 通过

- [x] Task 4: 实现 `state.rs` — DegradeState 状态机
  - [x] `DegradeState` 枚举：Normal / Degrading / Degraded / Recovering / Emergency
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq`
  - [x] `is_degraded(&self) -> bool` 方法（Normal 返回 false，其余 true）
  - [x] 验证：单元测试 — is_degraded

- [x] Task 5: 实现 `config.rs` — DegradeConfig
  - [x] `DegradeConfig` 结构体（heartbeat_period_ms: u64 / heartbeat_timeout_count: u8 / recovery_transition_ms: u64 / watchdog_hard_timeout_ms: u32 / power_setpoint_point: PointId / power_cmd_point: PointId）
  - [x] `Default` 实现（1s/3 次/30s/10s/PointId(0)/PointId(0)）
  - [x] 验证：单元测试 — 默认值

- [x] Task 6: 实现 `heartbeat.rs` — HeartbeatWatcher + HeartbeatStatus
  - [x] `HeartbeatStatus` 枚举：Alive / Timeout(u8) / Dead
  - [x] `HeartbeatWatcher` 结构体（heartbeat_period_ns: u64 / last_heartbeat_ns: u64 / timeout_count: u8 / max_timeout: u8 / agent_alive: bool）
  - [x] `new(heartbeat_period_ns, max_timeout) -> Self`（初始 agent_alive = true，last_heartbeat = 0）
  - [x] `on_heartbeat(&mut self, now_ns: u64)` — 更新 last_heartbeat / 重置 timeout_count / 设 agent_alive = true
  - [x] `check(&mut self, now_ns: u64) -> HeartbeatStatus` — D3 now_ns 注入；超时累计，达阈值返回 Dead
  - [x] `is_alive(&self) -> bool` 访问器
  - [x] 验证：单元测试 — Alive / Timeout / Dead / 恢复

- [x] Task 7: 实现 `recovery.rs` — RecoveryManager（纯状态，D10）
  - [x] `RecoveryManager` 结构体（saved_setpoint: Option<f64> / transition_start_ns: Option<u64> / transition_duration_ns: u64 / progress: f64 / degraded_setpoint: f64 / agent_setpoint: Option<f64>）
  - [x] `new(transition_duration_ns: u64) -> Self`
  - [x] `save_setpoint(&mut self, current_value: f64)` — 保存降级前设定值
  - [x] `start_transition(&mut self, degraded_value: f64, agent_value: f64, now_ns: u64)` — 启动过渡
  - [x] `transition_step(&mut self, now_ns: u64) -> Option<f64>` — 返回线性插值结果；完成返回 None
  - [x] `is_complete(&self) -> bool`
  - [x] `complete(&mut self)` — 清理过渡状态
  - [x] 验证：单元测试 — 保存/启动/插值/完成

- [x] Task 8: 实现 `stats.rs` — FlowStats + FlowReport
  - [x] `FlowStats` 结构体（state_transitions: u64 / emergency_count: u64 / recovery_count: u64 / heartbeat_timeouts: u64 / degrade_evaluations: u64 / cmds_executed: u64）—— 不使用 AtomicU64（D4）
  - [x] `FlowReport` 结构体（state: DegradeState / state_changed: bool / heartbeat: HeartbeatStatus / cmd_report: ExecutorReport / degrade_report: DegradeReport / watchdog: WatchdogStatus）—— 单次 tick 汇总
  - [x] 验证：单元测试 — 累加

- [x] Task 9: 外科手术式扩展 v0.57.0 DegradeEngine（D10）
  - [x] 在 `crates/kernel/rtos-degrade/src/engine.rs` 新增 `pub fn protocol_mut(&mut self) -> &mut P` 访问器
  - [x] 验证：v0.57.0 的 16 个测试全部保持通过（回归测试）
  - [x] 验证：`cargo test -p eneros-rtos-degrade` 通过

- [x] Task 10: 实现 `flow.rs` — WatchdogDegradeFlow（核心编排器）
  - [x] `WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider>` 泛型结构体（D6）
  - [x] 字段：state: DegradeState / heartbeat: HeartbeatWatcher / degrade_engine: DegradeEngine<P> / cmd_executor: CommandExecutor<P, S> / recovery: RecoveryManager / watchdog: Watchdog / config: DegradeConfig / stats: FlowStats
  - [x] `new(degrade_engine, cmd_executor, watchdog, config) -> Self`（初始 state = Normal，heartbeat 用 config 参数构造）
  - [x] `tick(&mut self, ctx: &DegradeContext) -> FlowReport`（D11 单步驱动）
  - [x] tick 逻辑：
    1. heartbeat.check(ctx.now_ns) → 更新 ctx 中的 agent_alive（或构造新 ctx）
    2. watchdog.check(ctx.now_ns) → 若 HardReset 则转 Emergency
    3. evaluate_state_transition() → 状态转换
    4. on_state_transition() → 转换动作（save_setpoint / start_transition / complete）
    5. 按状态执行：Normal → cmd_executor.tick(ctx.now_ns)；Degrading/Degraded → degrade_engine.evaluate(ctx, ctx.now_ns)；Recovering → recovery.transition_step + protocol_mut().write_point；Emergency → 不喂狗
    6. 喂狗（Emergency 外都喂，Normal 喂 3 层，其他喂 2 层）
  - [x] `evaluate_state_transition(&self, heartbeat: HeartbeatStatus) -> DegradeState` — 状态机转换逻辑
  - [x] `on_state_transition(&mut self, from: DegradeState, to: DegradeState, now_ns: u64)` — 转换动作
  - [x] `state(&self) -> DegradeState` / `stats(&self) -> &FlowStats` 访问器
  - [x] 验证：单元测试 — 各状态转换

- [x] Task 11: 实现 `mock.rs` — 测试工具
  - [x] `MockPointAccess`（BTreeMap<PointId, PointValue>，记录写入）— 或复用 v0.56.0 mock（若可）
  - [x] `MockDeviceStateProvider`（返回固定 DeviceState）
  - [x] 验证：编译通过（在测试中使用）

- [x] Task 12: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 DegradeState is_degraded（Normal=false，其余=true）
  - [x] T2 HeartbeatWatcher Alive（on_heartbeat 后 check 返回 Alive）
  - [x] T3 HeartbeatWatcher Timeout（未超阈值返回 Timeout(count)）
  - [x] T4 HeartbeatWatcher Dead（达阈值返回 Dead，agent_alive=false）
  - [x] T5 HeartbeatWatcher 恢复（Dead 后 on_heartbeat 恢复 Alive）
  - [x] T6 RecoveryManager save_setpoint + start_transition + transition_step 线性插值
  - [x] T7 RecoveryManager is_complete + complete
  - [x] T8 WatchdogDegradeFlow Normal 状态执行 cmd_executor.tick
  - [x] T9 WatchdogDegradeFlow Normal → Degrading → Degraded（心跳 Dead 触发降级）
  - [x] T10 WatchdogDegradeFlow Degraded → Recovering → Normal（心跳恢复触发回切，过渡完成）
  - [x] T11 WatchdogDegradeFlow Recovering → Degraded（恢复中再次崩溃）
  - [x] T12 WatchdogDegradeFlow Emergency（watchdog HardReset 触发）
  - [x] T13 WatchdogDegradeFlow 喂狗层级（Normal 3 层，Degraded 2 层）
  - [x] T14 FlowStats 累加（state_transitions / heartbeat_timeouts 等）
  - [x] T15 DegradeConfig 默认值
  - [x] 验证：`cargo test -p eneros-rtos-watchdog-degrade` 全部通过

- [~] Task 13: 设计文档 `docs/kernel/watchdog-degrade-flow-design.md`（用户明确要求跳过 — "Do NOT create design documentation"）
  - [x] 12 章节：版本目标 / 架构定位 / 端到端状态机 / DegradeState / HeartbeatWatcher / RecoveryManager / WatchdogDegradeFlow / 分层喂狗 / 状态转换流程 / 错误处理 / 统计与可观测 / 偏差声明
  - [x] 2 Mermaid 图：端到端降级状态机图 + tick 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/kernel/` 下（符合目录规范）

- [x] Task 14: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.57.0` → `0.58.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.57.0` → `0.58.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-watchdog-degrade` 说明
  - [x] 验证：`cargo build -p eneros-rtos-watchdog-degrade` 通过

- [x] Task 15: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-rtos-watchdog-degrade` 全部通过
  - [x] `cargo test -p eneros-rtos-degrade` 全部通过（v0.57.0 回归，验证 protocol_mut 非破坏）
  - [x] `cargo build -p eneros-rtos-watchdog-degrade --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-rtos-watchdog-degrade -- --check` 格式通过
  - [x] `cargo clippy -p eneros-rtos-watchdog-degrade --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~8 → Task 2（各模块依赖 crate 骨架）
- Task 9（外科手术式扩展 v0.57.0）独立，但需在 Task 10 之前完成（flow 依赖 protocol_mut）
- Task 10（flow）依赖 Task 3 + 4 + 5 + 6 + 7 + 8 + 9
- Task 11（mock）依赖 v0.51.0 PointAccess
- Task 12 → Task 8, 10, 11（集成测试依赖各模块）
- Task 13 → Task 12（文档在测试通过后撰写）
- Task 14 → Task 13（版本同步在功能完成后）
- Task 15 → Task 14（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（state）+ Task 5（config）+ Task 6（heartbeat）+ Task 7（recovery）+ Task 8（stats）可并行
- Task 9（扩展 v0.57.0）独立
- Task 10（flow）依赖 Task 3 + 4 + 5 + 6 + 7 + 8 + 9
- Task 11（mock）独立
