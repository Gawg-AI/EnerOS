# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.55.0` → `0.56.0`
  - [x] members 添加 `crates/kernel/rtos-cmd-exec`
  - [x] `cargo metadata --format-version 1` 验证 workspace 解析成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-rtos-cmd-exec` crate 骨架
  - [x] 新建 `crates/kernel/rtos-cmd-exec/Cargo.toml`，package name = `eneros-rtos-cmd-exec`
  - [x] dependencies：`eneros-controlbus`（path = `../controlbus`，同 kernel 子系统）+ `eneros-protocol-abstract`（path = `../../protocols/protocol-abstract`，跨子系统）+ `eneros-upa-model`（path = `../../protocols/upa-model`，跨子系统）
  - [x] 新建 `src/lib.rs`，包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 模块声明：error / state_provider / device_map / stats / executor / mock
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 3: 实现 `error.rs` — ExecutorError 错误类型
  - [x] `ExecutorError` 枚举：PointWriteFailed(ProtocolError) / DeviceNotMapped(DeviceId) / StateUnavailable(DeviceId)
  - [x] 实现 `Display` + `Debug`
  - [x] 验证：`cargo build -p eneros-rtos-cmd-exec` 通过

- [x] Task 4: 实现 `state_provider.rs` — DeviceStateProvider trait
  - [x] `DeviceStateProvider` trait：`fn device_state(&self, device: controlbus::DeviceId) -> DeviceState;`
  - [x] 复用 `eneros_controlbus::DeviceState`（不重新定义，D1）
  - [x] 验证：编译通过

- [x] Task 5: 实现 `device_map.rs` — DevicePointMap
  - [x] `DevicePointMap` 结构体（map: BTreeMap<u32, PointId>，内部用 u32 避免 controlbus::DeviceId 与 upa_model::PointId 混淆）
  - [x] `new() -> Self`
  - [x] `insert(device_id: controlbus::DeviceId, point_id: upa_model::PointId)`
  - [x] `get(&self, device_id: controlbus::DeviceId) -> Option<upa_model::PointId>`
  - [x] 验证：单元测试 — 插入/查询/未映射

- [x] Task 6: 实现 `stats.rs` — ExecutorStats + ExecutorReport
  - [x] `ExecutorStats` 结构体（success_count / failure_count / expired_count / rejected_count / truncated_count / unmapped_count / total_executed: u64）
  - [x] `ExecutorReport` 结构体（total / success / failed / expired / rejected / truncated / unmapped: usize）—— 单次 tick 汇总
  - [x] 不使用 AtomicU64（D7：单线程，匹配 v0.54.0/v0.55.0）
  - [x] 验证：单元测试 — 累加与重置

- [x] Task 7: 实现 `executor.rs` — CommandExecutor
  - [x] `CommandExecutor<P: PointAccess, S: DeviceStateProvider>` 泛型结构体（D6）
  - [x] 字段：protocol: P / state_provider: S / device_map: DevicePointMap / stats: ExecutorStats
  - [x] `new(protocol, state_provider, device_map) -> Self`
  - [x] `tick(&mut self, now_ns: u64) -> ExecutorReport`（D5 单步驱动，替代阻塞 `process_commands()`）
  - [x] tick 逻辑：循环 `command_consume()` → Emergency 旁路（D8）→ TTL 检查（复用 `ttl_check`，D1）→ 约束检查（复用 `constraint_check`，D1）→ `write_point` 下发
  - [x] Emergency 旁路：`ControlAction::Emergency` 直接下发 0.0，跳过 TTL + 约束（D8 安全优先）
  - [x] Idle 处理：`ControlAction::Idle` 下发 0.0（setpoint 被忽略，D9）
  - [x] Charge/Discharge：下发截断后的 setpoint（f32 → f64 → PointValue::Float，D10）
  - [x] `stats(&self) -> &ExecutorStats` 访问器
  - [x] 验证：单元测试 — 正常/过期/截断/拒绝/Emergency/Idle/写入失败/未映射

- [x] Task 8: 实现 `mock.rs` — MockPointAccess + MockDeviceStateProvider
  - [x] `MockPointAccess` 结构体（written_points: BTreeMap<PointId, PointValue>，复用 v0.55.0 模式）
  - [x] 实现 `PointAccess` trait 全部 6 个方法（write_point 记录写入，read_point 返回上次写入）
  - [x] `last_write(&self, point_id) -> Option<&PointValue>` 查询最后写入值
  - [x] `fail_next_write(point_id)` 标记下次 write_point 返回 Err
  - [x] `MockDeviceStateProvider` 结构体（states: BTreeMap<u32, DeviceState>）
  - [x] 实现 `DeviceStateProvider` trait
  - [x] `set_state(device_id, state)` 设置预设状态
  - [x] 验证：编译通过（在测试中使用）

- [x] Task 9: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 CommandExecutor 正常命令执行（Charge, setpoint=50.0, TTL 内, 约束通过）
  - [x] T2 CommandExecutor TTL 过期命令被丢弃
  - [x] T3 CommandExecutor 约束超限命令被截断（setpoint > max_power）
  - [x] T4 CommandExecutor 硬限制违反命令被拒绝（SOC 超限）
  - [x] T5 CommandExecutor Emergency 紧急停机旁路（跳过 TTL + 约束，下发 0.0）
  - [x] T6 CommandExecutor Idle 动作下发 0.0
  - [x] T7 CommandExecutor 写入失败统计（MockPointAccess.fail_next_write）
  - [x] T8 CommandExecutor 未映射设备跳过
  - [x] T9 CommandExecutor 多命令批量消费（3 条命令一次性 tick）
  - [x] T10 CommandExecutor 空队列（tick 无命令时 report 全零）
  - [x] T11 DevicePointMap 插入/查询/未映射
  - [x] T12 ExecutorStats 累加与多次 tick 统计
  - [x] 验证：`cargo test -p eneros-rtos-cmd-exec` 全部通过
  - [x] 注意：测试需借用 controlbus 的 `TEST_LOCK` 序列化（全局命令环）

- [x] Task 10: 设计文档 `docs/kernel/cmd-executor-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / 核心类型 / CommandExecutor / DeviceStateProvider / DevicePointMap / 执行流程（TTL→约束→下发）/ Emergency 旁路 / 错误处理 / 统计与可观测 / 与上下游关系 / 偏差声明
  - [x] 2 Mermaid 图：执行链路流程图 + tick 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/kernel/` 下（符合目录规范）

- [x] Task 11: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.55.0` → `0.56.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.55.0` → `0.56.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-cmd-exec` 说明
  - [x] 验证：`cargo build -p eneros-rtos-cmd-exec` 通过

- [x] Task 12: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-rtos-cmd-exec` 全部通过
  - [x] `cargo build -p eneros-rtos-cmd-exec --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-rtos-cmd-exec -- --check` 格式通过
  - [x] `cargo clippy -p eneros-rtos-cmd-exec --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~6 → Task 2（各模块依赖 crate 骨架）
- Task 7（executor）依赖 Task 3 + 4 + 5 + 6
- Task 8（mock）依赖 Task 4（DeviceStateProvider trait）
- Task 9 → Task 5, 6, 7, 8（集成测试依赖各模块）
- Task 10 → Task 9（文档在测试通过后撰写）
- Task 11 → Task 10（版本同步在功能完成后）
- Task 12 → Task 11（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（state_provider）+ Task 5（device_map）+ Task 6（stats）可并行
- Task 7（executor）依赖 Task 3 + 4 + 5 + 6
- Task 8（mock）依赖 Task 4
