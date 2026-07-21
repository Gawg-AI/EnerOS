# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.54.0`
- [x] C2 members 列表已添加 `crates/kernel/rtos-control`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/kernel/rtos-control/Cargo.toml` 存在，package name 为 `eneros-rtos-control`
- [x] C5 dependencies 包含 `eneros-controlbus` + `eneros-protocol-abstract` + `eneros-upa-model`（path 引用正确）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / pid / setpoint / loop_trait / engine / power_loop / mock / stats
- [x] C8 D1~D12 偏差声明表存在于 lib.rs

## ControlError 错误类型
- [x] C9 `ControlError` 枚举包含 SetpointInvalid/FeedbackReadFailed/OutputWriteFailed/ConstraintViolation/LoopPanic/EngineFull
- [x] C10 实现 `Display` + `Debug`

## PidController PID 控制器
- [x] C11 `PidController` 结构体包含 9 字段（kp/ki/kd/integral/last_error/integral_limit/output_limit/setpoint/process_variable）
- [x] C12 `new(kp, ki, kd)` 默认 integral_limit = f64::MAX, output_limit = f64::MAX
- [x] C13 `compute(dt)` 实现误差 + 积分限幅 + 微分 + 输出限幅
- [x] C14 `set_setpoint` / `set_process_variable` / `reset` 方法实现
- [x] C15 `set_integral_limit` / `set_output_limit` 在线调参方法（9.5 可维护要求）
- [x] C16 积分限幅使用 `core::cmp::min/max` 手写 clamp（D10）

## SetpointTracker 设定值跟踪
- [x] C17 `SetpointTracker` 结构体（current / target / max_rate_per_s）
- [x] C18 `new(initial, max_rate_per_s) -> Self`
- [x] C19 `set_target(target)` 设置目标值
- [x] C20 `update(dt) -> f64` 按 max_rate_per_s 限制单步变化（D11）
- [x] C21 `is_settled() -> bool` 判断是否已收敛
- [x] C22 `current()` / `target()` 访问器

## ControlLoop trait + LoopStats
- [x] C23 `ControlLoop` trait 不要求 Send+Sync（D5）
- [x] C24 trait 方法：name / period_us / init / execute(elapsed_us) / shutdown
- [x] C25 `LoopStats` 结构体包含 name/period_us/last_exec_time_us/last_jitter_us/max_jitter_us/total_jitter_us/exec_count/error_count

## EngineStats + JitterStats
- [x] C26 `JitterStats` 结构体（last_jitter_us/max_jitter_us/total_jitter_us/exec_count/error_count/last_exec_time_us）
- [x] C27 `EngineStats` 结构体（per_loop: Vec<(String, JitterStats)> + total_ticks: u64）（D9）
- [x] C28 `update(name, jitter_us, exec_time_us)` 方法
- [x] C29 `get(&name) -> Option<&JitterStats>` 查询方法

## ControlLoopEngine 引擎
- [x] C30 `ControlLoopEngine` 结构体（loops: Vec<Box<dyn ControlLoop>> / loop_stats / last_execute_us / stats）
- [x] C31 `new() -> Self`
- [x] C32 `register(ctrl: Box<dyn ControlLoop>)` 追加循环
- [x] C33 `tick(now_us, elapsed_us) -> EngineTickReport` 单步驱动（D3：非阻塞 run -> !）
- [x] C34 `tick` 遍历所有循环，对到期的循环调用 execute
- [x] C35 `tick` 记录 jitter 与 exec_time
- [x] C36 `EngineTickReport` 结构体（executed_loops / errors）
- [x] C37 `stats() -> &EngineStats` 访问器
- [x] C38 `stats` 不使用 AtomicU64（D8）

## PowerControlLoop 示例
- [x] C39 `PowerControlLoop<P: PointAccess>` 泛型结构体（D6：不用 Box<dyn PointAccess>）
- [x] C40 包含 pid / setpoint_tracker / feedback_point_id / output_point_id / current_setpoint / protocol / name
- [x] C41 实现 `ControlLoop` trait，`period_us = 10000`（10ms）
- [x] C42 `execute` 调用 `command_consume()` 取设定值（D7：直接调全局函数）
- [x] C43 `execute` 用 tracker.update 跟踪设定值
- [x] C44 `execute` 读反馈点 → PID 计算 → 写下发点
- [x] C45 `shutdown` 调用 `pid.reset()`（D12）
- [x] C46 无命令时保持上次设定值

## MockPointAccess
- [x] C47 `MockPointAccess` 结构体（points: BTreeMap<PointId, DataPoint>）
- [x] C48 实现 `PointAccess` trait 全部 6 个方法
- [x] C49 `set_point(point_id, value)` 设置测试值

## 集成测试
- [x] C50 T1 PidController 阶跃响应数值正确
- [x] C51 T2 PidController 积分限幅生效
- [x] C52 T3 PidController 输出限幅生效
- [x] C53 T4 PidController reset 清零
- [x] C54 T5 PidController dt=0 不 panic（微分项为 0）
- [x] C55 T6 SetpointTracker 斜率限制（小步前进）
- [x] C56 T7 SetpointTracker 收敛到目标
- [x] C57 T8 SetpointTracker 无限制模式（直接返回 target）
- [x] C58 T9 SetpointTracker 负方向收敛
- [x] C59 T10 ControlLoopEngine 注册 + tick 调度
- [x] C60 T11 ControlLoopEngine 多循环不同周期
- [x] C61 T12 ControlLoopEngine 错误隔离
- [x] C62 T13 ControlLoopEngine 抖动统计（max/avg）
- [x] C63 T14 PowerControlLoop 完整链路（命令→反馈→PID→下发）
- [x] C64 T15 PowerControlLoop 无命令保持上次设定值
- [x] C65 T16 PowerControlLoop PID 反馈收敛（多次 tick 后输出趋稳）

## 设计文档
- [x] C66 `docs/kernel/rtos-control-loop-design.md` 存在
- [x] C67 文档包含 12 章节
- [x] C68 文档包含 2 Mermaid 图（引擎架构图 + tick 时序图）
- [x] C69 D1~D12 偏差声明表
- [x] C70 文档位置在 `docs/kernel/` 下（符合目录规范）

## 版本号同步
- [x] C71 `Makefile` 版本号 0.53.0 → 0.54.0
- [x] C72 `.github/workflows/ci.yml` 版本号 0.53.0 → 0.54.0
- [x] C73 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-control` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C74 `cargo metadata --format-version 1` 成功
- [x] C75 `cargo test -p eneros-rtos-control` 全部通过
- [x] C76 `cargo build -p eneros-rtos-control --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C77 `cargo fmt -p eneros-rtos-control -- --check` 格式通过
- [x] C78 `cargo clippy -p eneros-rtos-control --all-targets -- -D warnings` lint 通过
- [x] C79 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C80 rtos-control 在 `crates/kernel/` 下（子系统归属正确）
- [x] C81 跨 crate path 引用使用相对路径（controlbus 同子系统 `../controlbus`；protocol-abstract/upa-model 跨子系统 `../../protocols/...`）
- [x] C82 设计文档在 `docs/kernel/` 下（符合 §2.3.3）
- [x] C83 无根目录 crate
- [x] C84 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C85 所有 Rust 代码无 `use std::*`
- [x] C86 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C87 不要求 `Send + Sync`（D5）
- [x] C88 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`（继承自 lib.rs）
