# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.56.0` → `0.57.0`
  - [x] members 添加 `crates/kernel/rtos-degrade`
  - [x] `cargo metadata --format-version 1` 验证 workspace 解析成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-rtos-degrade` crate 骨架
  - [x] 新建 `crates/kernel/rtos-degrade/Cargo.toml`，package name = `eneros-rtos-degrade`
  - [x] dependencies：`eneros-protocol-abstract`（path = `../../protocols/protocol-abstract`，跨子系统）+ `eneros-upa-model`（path = `../../protocols/upa-model`，跨子系统）+ `eneros-rtos-cmd-exec`（path = `../rtos-cmd-exec`，同 kernel 子系统，复用 DevicePointMap）
  - [x] 新建 `src/lib.rs`，包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 模块声明：error / mode / context / rule / engine / stats / safe_defaults / builtin / mock
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 3: 实现 `error.rs` — DegradeError 错误类型
  - [x] `DegradeError` 枚举：PointWriteFailed(ProtocolError) / NoDeviceMap / SafeDefaultMissing(PointId)
  - [x] 实现 `Display` + `Debug`
  - [x] 验证：`cargo build -p eneros-rtos-degrade` 通过

- [x] Task 4: 实现 `mode.rs` — DegradeMode 枚举
  - [x] `DegradeMode` 枚举：Normal=0 / HoldOutput=1 / StopCharge=2 / SafeDefault=3 / EmergencyStop=4
  - [x] 派生 `Debug / Clone / Copy / PartialEq / Eq / PartialOrd / Ord`
  - [x] `is_degraded(&self) -> bool` 方法（Normal 返回 false，其余 true）
  - [x] 验证：单元测试 — Ord 比较、is_degraded

- [x] Task 5: 实现 `context.rs` — DegradeContext
  - [x] `DegradeContext` 结构体（agent_alive: bool / agent_last_heartbeat_ns: u64 / control_bus_active: bool / device_comm_ok: bool / battery_soc: f64 / grid_frequency: f64 / temperature: f64）
  - [x] `new() -> Self` 默认构造（全零/false）
  - [x] builder 风格 setter 方法（with_agent_alive / with_battery_soc 等）
  - [x] 验证：单元测试 — 构造与字段访问

- [x] Task 6: 实现 `rule.rs` — DegradeRule trait
  - [x] `DegradeRule` trait（**不要求 Send + Sync**，D6）
  - [x] `fn name(&self) -> &str`
  - [x] `fn priority(&self) -> u8`
  - [x] `fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode>`
  - [x] 验证：编译通过

- [x] Task 7: 实现 `stats.rs` — DegradeStats + DegradeReport
  - [x] `DegradeStats` 结构体（mode_switch_count: u64 / evaluations_count: u64 / last_mode: DegradeMode / last_mode_switch_ns: u64）
  - [x] `DegradeReport` 结构体（new_mode: DegradeMode / mode_changed: bool / action_taken: bool）—— 单次评估汇总
  - [x] 不使用 AtomicU64（D7：单线程，匹配 v0.54.0/v0.55.0/v0.56.0）
  - [x] 验证：单元测试 — 累加

- [x] Task 8: 实现 `safe_defaults.rs` — SafeDefaults
  - [x] `SafeDefaults` 结构体（map: BTreeMap<PointId, f64>）
  - [x] `new() -> Self`
  - [x] `insert(point_id, value)` / `get(&self, point_id) -> Option<f64>` / `iter(&self) -> impl Iterator`
  - [x] 验证：单元测试 — 插入/查询/迭代

- [x] Task 9: 实现 `engine.rs` — DegradeEngine
  - [x] `DegradeEngine<P: PointAccess>` 泛型结构体（D6，不用 Box<dyn PointAccess>）
  - [x] 字段：rules: Vec<Box<dyn DegradeRule>> / current_mode: DegradeMode / previous_mode: DegradeMode / safe_defaults: SafeDefaults / device_map: DevicePointMap / protocol: P / stats: DegradeStats
  - [x] `new(protocol, device_map, safe_defaults) -> Self`（初始 mode = Normal）
  - [x] `add_rule(&mut self, rule: Box<dyn DegradeRule>)` — 插入时按 priority 降序排序（不在 evaluate 中排序，D8 性能优化）
  - [x] `evaluate(&mut self, ctx: &DegradeContext) -> DegradeReport`
  - [x] evaluate 逻辑：按优先级降序遍历 rules → find_map → 无触发返回 Normal → 与 current_mode 比较 → 不同则 on_mode_change
  - [x] `on_mode_change(&mut self, from, to, now_ns) -> bool`（返回是否执行了下发动作）
  - [x] on_mode_change 逻辑：HoldOutput 无动作 / StopCharge 向 device_map 功率点下发 0.0 / SafeDefault 遍历 safe_defaults 下发 / EmergencyStop 向 device_map 紧急停机点下发 Bool(true) / Normal 无动作
  - [x] `current_mode(&self) -> DegradeMode` / `previous_mode(&self) -> DegradeMode` / `stats(&self) -> &DegradeStats` 访问器
  - [x] 验证：单元测试 — 模式切换、不变、恢复回切

- [x] Task 10: 实现 `builtin.rs` — 5 个内置规则
  - [x] `AgentDeadRule`（priority=100）：`!ctx.agent_alive` 或 `now_ns - ctx.agent_last_heartbeat_ns > HEARTBEAT_TIMEOUT_NS`（5s）→ SafeDefault
  - [x] `ControlBusDownRule`（priority=90）：`!ctx.control_bus_active` → HoldOutput
  - [x] `DeviceCommFailRule`（priority=80）：`!ctx.device_comm_ok` → SafeDefault
  - [x] `LowBatteryRule`（priority=70）：`ctx.battery_soc < 10.0` → StopCharge
  - [x] `OverTempRule`（priority=60）：`ctx.temperature > 80.0` → StopCharge
  - [x] 常量 `HEARTBEAT_TIMEOUT_NS: u64 = 5_000_000_000`
  - [x] 验证：单元测试 — 每个规则的触发与不触发

- [x] Task 11: 实现 `mock.rs` — MockPointAccess + 辅助测试工具
  - [x] `MockPointAccess` 结构体（written_points: BTreeMap<PointId, PointValue>）
  - [x] 实现 `PointAccess` trait 全部 6 个方法
  - [x] `last_write(&self, point_id) -> Option<&PointValue>` 查询
  - [x] 验证：编译通过（在测试中使用）

- [x] Task 12: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 DegradeMode Ord 比较（Normal < EmergencyStop）
  - [x] T2 DegradeMode is_degraded（Normal=false, 其余=true）
  - [x] T3 AgentDeadRule 触发（agent_alive=false → SafeDefault）
  - [x] T4 AgentDeadRule 心跳超时（now_ns - last_heartbeat > 5s → SafeDefault）
  - [x] T5 ControlBusDownRule 触发（control_bus_active=false → HoldOutput）
  - [x] T6 DeviceCommFailRule 触发（device_comm_ok=false → SafeDefault）
  - [x] T7 LowBatteryRule 触发（battery_soc=5.0 → StopCharge）
  - [x] T8 OverTempRule 触发（temperature=85.0 → StopCharge）
  - [x] T9 多规则优先级仲裁（AgentDead + LowBattery 同时触发 → SafeDefault）
  - [x] T10 无规则触发返回 Normal
  - [x] T11 DegradeEngine 模式切换 StopCharge 下发 0.0
  - [x] T12 DegradeEngine 模式不变不执行动作
  - [x] T13 DegradeEngine SafeDefault 遍历 safe_defaults 下发
  - [x] T14 DegradeEngine EmergencyStop 下发 Bool(true)
  - [x] T15 DegradeEngine 恢复回切到 Normal
  - [x] T16 DegradeStats mode_switch_count 累加
  - [x] 验证：`cargo test -p eneros-rtos-degrade` 全部通过

- [x] Task 13: 设计文档 `docs/kernel/degrade-engine-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / 核心类型 / DegradeMode / DegradeRule / DegradeContext / DegradeEngine / 内置规则集 / 模式切换流程 / 错误处理 / 统计与可观测 / 偏差声明
  - [x] 2 Mermaid 图：降级模式层级图 + evaluate 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/kernel/` 下（符合目录规范）

- [x] Task 14: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.56.0` → `0.57.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.56.0` → `0.57.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-degrade` 说明
  - [x] 验证：`cargo build -p eneros-rtos-degrade` 通过

- [x] Task 15: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-rtos-degrade` 全部通过
  - [x] `cargo build -p eneros-rtos-degrade --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-rtos-degrade -- --check` 格式通过
  - [x] `cargo clippy -p eneros-rtos-degrade --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~8 → Task 2（各模块依赖 crate 骨架）
- Task 9（engine）依赖 Task 3 + 4 + 5 + 6 + 7 + 8
- Task 10（builtin）依赖 Task 4 + 5 + 6
- Task 11（mock）依赖 v0.51.0 PointAccess
- Task 12 → Task 8, 9, 10, 11（集成测试依赖各模块）
- Task 13 → Task 12（文档在测试通过后撰写）
- Task 14 → Task 13（版本同步在功能完成后）
- Task 15 → Task 14（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（mode）+ Task 5（context）+ Task 6（rule trait）+ Task 7（stats）+ Task 8（safe_defaults）可并行
- Task 9（engine）依赖 Task 3 + 4 + 5 + 6 + 7 + 8
- Task 10（builtin）依赖 Task 4 + 5 + 6
- Task 11（mock）独立
