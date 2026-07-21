# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.53.0` → `0.54.0`
  - [x] members 添加 `crates/kernel/rtos-control`
  - [x] `cargo metadata --format-version 1` 验证 workspace 解析成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-rtos-control` crate 骨架
  - [x] 新建 `crates/kernel/rtos-control/Cargo.toml`，package name = `eneros-rtos-control`
  - [x] dependencies：`eneros-controlbus`（path = `../controlbus`，同 kernel 子系统）+ `eneros-protocol-abstract`（path = `../../protocols/protocol-abstract`，跨子系统）+ `eneros-upa-model`（path = `../../protocols/upa-model`，跨子系统）
  - [x] 新建 `src/lib.rs`，包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 模块声明：error / pid / setpoint / loop_trait / engine / power_loop / mock / stats
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 3: 实现 `error.rs` — ControlError 错误类型
  - [x] `ControlError` 枚举：SetpointInvalid / FeedbackReadFailed / OutputWriteFailed / ConstraintViolation / LoopPanic / EngineFull
  - [x] 实现 `Display` + `Debug`
  - [x] 验证：`cargo build -p eneros-rtos-control` 通过

- [x] Task 4: 实现 `pid.rs` — PidController PID 控制器
  - [x] `PidController` 结构体（kp/ki/kd/integral/last_error/integral_limit/output_limit/setpoint/process_variable）
  - [x] `new(kp, ki, kd) -> Self` 构造函数（默认 integral_limit = f64::MAX, output_limit = f64::MAX）
  - [x] `compute(&mut self, dt: f64) -> f64`：误差 + 积分限幅 + 微分 + 输出限幅
  - [x] `set_setpoint(sp)` / `set_process_variable(pv)` / `reset()`
  - [x] `set_integral_limit(l)` / `set_output_limit(l)`（在线调参，9.5 可维护要求）
  - [x] 验证：单元测试 — 阶跃响应、积分限幅、输出限幅、reset 清零、dt=0 不 panic

- [x] Task 5: 实现 `setpoint.rs` — SetpointTracker 设定值跟踪器
  - [x] `SetpointTracker` 结构体（current / target / max_rate_per_s）
  - [x] `new(initial, max_rate_per_s) -> Self`
  - [x] `set_target(target)` 设置目标值
  - [x] `update(&mut self, dt: f64) -> f64`：按 max_rate_per_s 限制单步变化
  - [x] `is_settled(&self) -> bool` 判断是否已收敛到目标
  - [x] `current(&self) -> f64` / `target(&self) -> f64`
  - [x] 验证：单元测试 — 斜率限制、收敛、无限制模式、负方向

- [x] Task 6: 实现 `loop_trait.rs` — ControlLoop trait + LoopStats
  - [x] `ControlLoop` trait（不要求 Send+Sync）：`name(&self) -> &str` / `period_us(&self) -> u64` / `init(&mut self) -> Result<(), ControlError>` / `execute(&mut self, elapsed_us: u64) -> Result<(), ControlError>` / `shutdown(&mut self)`
  - [x] `LoopStats` 结构体（name / period_us / last_exec_time_us / last_jitter_us / max_jitter_us / total_jitter_us / exec_count / error_count）
  - [x] 验证：编译通过

- [x] Task 7: 实现 `stats.rs` — EngineStats + JitterRecord
  - [x] `JitterStats` 结构体（last_jitter_us / max_jitter_us / total_jitter_us / exec_count / error_count / last_exec_time_us）
  - [x] `EngineStats` 结构体（per_loop: Vec<(String, JitterStats)> + total_ticks: u64）
  - [x] `update(name, jitter_us, exec_time_us)` 方法
  - [x] `get(&name) -> Option<&JitterStats>` 查询
  - [x] 验证：单元测试 — 更新与查询

- [x] Task 8: 实现 `engine.rs` — ControlLoopEngine 引擎
  - [x] `ControlLoopEngine` 结构体（loops: Vec<Box<dyn ControlLoop>> / loop_stats: Vec<LoopStats> / last_execute_us: Vec<u64> / stats: EngineStats）
  - [x] `new() -> Self`
  - [x] `register(&mut self, ctrl: Box<dyn ControlLoop>)`：追加循环 + 初始化 LoopStats
  - [x] `tick(&mut self, now_us: u64, elapsed_us: u64) -> EngineTickReport`：遍历所有循环，对到期的循环调用 execute，记录 jitter 与 exec_time
  - [x] `EngineTickReport` 结构体（executed_loops: usize / errors: usize）
  - [x] `stats(&self) -> &EngineStats`
  - [x] 验证：单元测试 — 多循环调度、错误隔离、抖动统计、最小周期驱动

- [x] Task 9: 实现 `power_loop.rs` — PowerControlLoop 示例
  - [x] `PowerControlLoop<P: PointAccess>` 泛型结构体（pid / setpoint_tracker / feedback_point_id / output_point_id / current_setpoint / protocol: P / name: &'static str）
  - [x] `new(pid, tracker, feedback_pid, output_pid, protocol, name) -> Self`
  - [x] 实现 `ControlLoop` trait：`period_us = 10000`（10ms）
  - [x] `execute`：调用 `command_consume()` 取设定值 → 用 tracker.update 跟踪 → 读反馈点 → PID 计算 → 写下发点
  - [x] `shutdown`：调用 `pid.reset()`
  - [x] 验证：单元测试 — 完整链路（用 MockPointAccess + 手动塞 ControlCommand）、无命令时保持、PID 反馈收敛

- [x] Task 10: 实现 `mock.rs` — MockPointAccess + 辅助测试工具
  - [x] `MockPointAccess` 结构体（points: BTreeMap<PointId, DataPoint>）
  - [x] 实现 `PointAccess` trait（read_point / write_point / read_points / write_points / read_device_points / protocol_type）
  - [x] `set_point(point_id, value)` 设置测试值
  - [x] 验证：编译通过（在测试中使用）

- [x] Task 11: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 PidController 阶跃响应数值正确
  - [x] T2 PidController 积分限幅生效
  - [x] T3 PidController 输出限幅生效
  - [x] T4 PidController reset 清零
  - [x] T5 PidController dt=0 不 panic（微分项为 0）
  - [x] T6 SetpointTracker 斜率限制（小步前进）
  - [x] T7 SetpointTracker 收敛到目标
  - [x] T8 SetpointTracker 无限制模式（直接返回 target）
  - [x] T9 SetpointTracker 负方向收敛
  - [x] T10 ControlLoopEngine 注册 + tick 调度
  - [x] T11 ControlLoopEngine 多循环不同周期
  - [x] T12 ControlLoopEngine 错误隔离
  - [x] T13 ControlLoopEngine 抖动统计（max/avg）
  - [x] T14 PowerControlLoop 完整链路（命令→反馈→PID→下发）
  - [x] T15 PowerControlLoop 无命令保持上次设定值
  - [x] T16 PowerControlLoop PID 反馈收敛（多次 tick 后输出趋稳）
  - [x] 验证：`cargo test -p eneros-rtos-control` 全部通过

- [x] Task 12: 设计文档 `docs/kernel/rtos-control-loop-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / 核心类型 / PidController / SetpointTracker / ControlLoop trait / ControlLoopEngine / PowerControlLoop / 错误处理 / 性能与抖动 / 测试策略 / 偏差声明
  - [x] 2 Mermaid 图：引擎架构图 + tick 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/kernel/` 下（符合目录规范）

- [x] Task 13: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.53.0` → `0.54.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.53.0` → `0.54.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-control` 说明
  - [x] 验证：`cargo build -p eneros-rtos-control` 通过

- [x] Task 14: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-rtos-control` 全部通过
  - [x] `cargo build -p eneros-rtos-control --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-rtos-control -- --check` 格式通过
  - [x] `cargo clippy -p eneros-rtos-control --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~10 → Task 2（各模块依赖 crate 骨架）
- Task 11 → Task 4, 5, 8, 9, 10（集成测试依赖各模块）
- Task 12 → Task 11（文档在测试通过后撰写）
- Task 13 → Task 12（版本同步在功能完成后）
- Task 14 → Task 13（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（pid）+ Task 5（setpoint）+ Task 6（loop_trait）+ Task 7（stats）+ Task 10（mock）可并行
- Task 8（engine）依赖 Task 6 + Task 7
- Task 9（power_loop）依赖 Task 4 + Task 5 + Task 6 + Task 10 + eneros-controlbus
