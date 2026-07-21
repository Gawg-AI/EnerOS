# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.57.0`
- [x] C2 members 列表已添加 `crates/kernel/rtos-degrade`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/kernel/rtos-degrade/Cargo.toml` 存在，package name 为 `eneros-rtos-degrade`
- [x] C5 dependencies 包含 `eneros-protocol-abstract` + `eneros-upa-model` + `eneros-rtos-cmd-exec`（path 引用正确）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / mode / context / rule / engine / stats / safe_defaults / builtin / mock
- [x] C8 D1~D12 偏差声明表存在于 lib.rs

## DegradeError 错误类型
- [x] C9 `DegradeError` 枚举包含 PointWriteFailed/NoDeviceMap/SafeDefaultMissing
- [x] C10 实现 `Display` + `Debug`

## DegradeMode 枚举
- [x] C11 `DegradeMode` 枚举包含 Normal/HoldOutput/StopCharge/SafeDefault/EmergencyStop
- [x] C12 派生 `Debug / Clone / Copy / PartialEq / Eq / PartialOrd / Ord`
- [x] C13 `is_degraded()` 方法（Normal=false，其余=true）
- [x] C14 单元测试 — Ord 比较、is_degraded

## DegradeContext
- [x] C15 `DegradeContext` 结构体包含 7 个字段（agent_alive/agent_last_heartbeat_ns/control_bus_active/device_comm_ok/battery_soc/grid_frequency/temperature）
- [x] C16 `new() -> Self` 默认构造
- [x] C17 builder 风格 setter 方法
- [x] C18 单元测试 — 构造与字段访问

## DegradeRule trait
- [x] C19 `DegradeRule` trait 定义 name/priority/evaluate 三方法
- [x] C20 **不要求 Send + Sync**（D6，no_std 单线程）
- [x] C21 编译通过

## DegradeStats + DegradeReport
- [x] C22 `DegradeStats` 结构体（mode_switch_count/evaluations_count/last_mode/last_mode_switch_ns）
- [x] C23 `DegradeReport` 结构体（new_mode/mode_changed/action_taken）
- [x] C24 不使用 AtomicU64（D7）
- [x] C25 单元测试 — 累加

## SafeDefaults
- [x] C26 `SafeDefaults` 结构体（BTreeMap<PointId, f64>）
- [x] C27 `new()` / `insert()` / `get()` / `iter()` 方法
- [x] C28 单元测试 — 插入/查询/迭代

## DegradeEngine
- [x] C29 `DegradeEngine<P: PointAccess>` 泛型结构体（D6，不用 Box<dyn PointAccess>）
- [x] C30 字段：rules / current_mode / previous_mode / safe_defaults / device_map / protocol / stats
- [x] C31 `new(protocol, device_map, safe_defaults) -> Self`（初始 Normal）
- [x] C32 `add_rule()` 插入时按 priority 降序排序（D8，不在 evaluate 中排序）
- [x] C33 `evaluate(&mut self, ctx: &DegradeContext) -> DegradeReport`
- [x] C34 evaluate 逻辑：按优先级降序 find_map → 无触发返回 Normal
- [x] C35 `on_mode_change()` 模式切换处理
- [x] C36 HoldOutput 无动作
- [x] C37 StopCharge 向 device_map 功率点下发 0.0
- [x] C38 SafeDefault 遍历 safe_defaults 下发
- [x] C39 EmergencyStop 向 device_map 紧急停机点下发 Bool(true)
- [x] C40 Normal 无动作（Agent 接管）
- [x] C41 `current_mode()` / `previous_mode()` / `stats()` 访问器
- [x] C42 单元测试 — 模式切换、不变、恢复回切

## 内置规则集
- [x] C43 `AgentDeadRule`（priority=100，触发条件：!agent_alive 或心跳超时 5s → SafeDefault）
- [x] C44 `ControlBusDownRule`（priority=90，触发条件：!control_bus_active → HoldOutput）
- [x] C45 `DeviceCommFailRule`（priority=80，触发条件：!device_comm_ok → SafeDefault）
- [x] C46 `LowBatteryRule`（priority=70，触发条件：battery_soc < 10.0 → StopCharge）
- [x] C47 `OverTempRule`（priority=60，触发条件：temperature > 80.0 → StopCharge）
- [x] C48 常量 `HEARTBEAT_TIMEOUT_NS: u64 = 5_000_000_000`
- [x] C49 单元测试 — 每个规则的触发与不触发

## MockPointAccess
- [x] C50 `MockPointAccess` 结构体（written_points: BTreeMap）
- [x] C51 实现 `PointAccess` trait 全部 6 个方法
- [x] C52 `last_write(point_id) -> Option<&PointValue>` 查询
- [x] C53 编译通过（在测试中使用）

## 集成测试
- [x] C54 T1 DegradeMode Ord 比较
- [x] C55 T2 DegradeMode is_degraded
- [x] C56 T3 AgentDeadRule 触发
- [x] C57 T4 AgentDeadRule 心跳超时
- [x] C58 T5 ControlBusDownRule 触发
- [x] C59 T6 DeviceCommFailRule 触发
- [x] C60 T7 LowBatteryRule 触发
- [x] C61 T8 OverTempRule 触发
- [x] C62 T9 多规则优先级仲裁
- [x] C63 T10 无规则触发返回 Normal
- [x] C64 T11 DegradeEngine StopCharge 下发 0.0
- [x] C65 T12 DegradeEngine 模式不变不执行动作
- [x] C66 T13 DegradeEngine SafeDefault 遍历下发
- [x] C67 T14 DegradeEngine EmergencyStop 下发 Bool(true)
- [x] C68 T15 DegradeEngine 恢复回切 Normal
- [x] C69 T16 DegradeStats mode_switch_count 累加

## 设计文档
- [x] C70 `docs/kernel/degrade-engine-design.md` 存在
- [x] C71 文档包含 12 章节
- [x] C72 文档包含 2 Mermaid 图（降级模式层级图 + evaluate 时序图）
- [x] C73 D1~D12 偏差声明表
- [x] C74 文档位置在 `docs/kernel/` 下

## 版本号同步
- [x] C75 `Makefile` 版本号 0.56.0 → 0.57.0
- [x] C76 `.github/workflows/ci.yml` 版本号 0.56.0 → 0.57.0
- [x] C77 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-degrade` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C78 `cargo metadata --format-version 1` 成功
- [x] C79 `cargo test -p eneros-rtos-degrade` 全部通过
- [x] C80 `cargo build -p eneros-rtos-degrade --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C81 `cargo fmt -p eneros-rtos-degrade -- --check` 格式通过
- [x] C82 `cargo clippy -p eneros-rtos-degrade --all-targets -- -D warnings` lint 通过
- [x] C83 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C84 rtos-degrade 在 `crates/kernel/` 下（子系统归属正确）
- [x] C85 跨 crate path 引用使用相对路径
- [x] C86 设计文档在 `docs/kernel/` 下
- [x] C87 无根目录 crate
- [x] C88 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C89 所有 Rust 代码无 `use std::*`
- [x] C90 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C91 不要求 `Send + Sync`（D6 泛型，单线程）
- [x] C92 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`

## Karpathy 原则校验
- [x] C93 不要求 `Send + Sync`（蓝图 DegradeRule: Send + Sync 被拒绝，D6）
- [x] C94 不使用 `Box<dyn PointAccess>`（改为泛型 <P: PointAccess>，D6）
- [x] C95 不使用 `log_warn!`（改为 stats 计数器，D7）
- [x] C96 不使用 `MonotonicTime`（改为 `now_ns: u64` 注入，D5）
- [x] C97 不使用 `EMERGENCY_STOP_ID`/`POWER_CMD_ID` 常量（复用 v0.56.0 DevicePointMap，D9）
- [x] C98 不在 evaluate 中 sort（插入时排序，D8 性能优化）
- [x] C99 未引入蓝图伪代码中不存在于实际 API 的类型
