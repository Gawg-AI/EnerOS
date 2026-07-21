# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.56.0`
- [x] C2 members 列表已添加 `crates/kernel/rtos-cmd-exec`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/kernel/rtos-cmd-exec/Cargo.toml` 存在，package name 为 `eneros-rtos-cmd-exec`
- [x] C5 dependencies 包含 `eneros-controlbus` + `eneros-protocol-abstract` + `eneros-upa-model`（path 引用正确）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / state_provider / device_map / stats / executor / mock
- [x] C8 D1~D12 偏差声明表存在于 lib.rs

## ExecutorError 错误类型
- [x] C9 `ExecutorError` 枚举包含 PointWriteFailed/DeviceNotMapped/StateUnavailable
- [x] C10 实现 `Display` + `Debug`

## DeviceStateProvider trait
- [x] C11 `DeviceStateProvider` trait 定义 `device_state(&self, device) -> DeviceState`
- [x] C12 复用 `eneros_controlbus::DeviceState`（不重新定义，D1）

## DevicePointMap
- [x] C13 `DevicePointMap` 结构体存在
- [x] C14 `new()` / `insert(device_id, point_id)` / `get(&self, device_id) -> Option<PointId>` 方法
- [x] C15 单元测试 — 插入/查询/未映射

## ExecutorStats + ExecutorReport
- [x] C16 `ExecutorStats` 结构体（success/failure/expired/rejected/truncated/unmapped/total_executed）
- [x] C17 `ExecutorReport` 结构体（total/success/failed/expired/rejected/truncated/unmapped）
- [x] C18 不使用 AtomicU64（D7）
- [x] C19 单元测试 — 累加

## CommandExecutor
- [x] C20 `CommandExecutor<P: PointAccess, S: DeviceStateProvider>` 泛型结构体（D6）
- [x] C21 字段：protocol / state_provider / device_map / stats
- [x] C22 `new(protocol, state_provider, device_map) -> Self`
- [x] C23 `tick(&mut self, now_ns: u64) -> ExecutorReport` 单步驱动（D5）
- [x] C24 复用 `eneros_controlbus::command_consume()`（D2，不用 ControlBusReader）
- [x] C25 复用 `eneros_controlbus::ttl_check()`（D1，不重新实现 TtlChecker）
- [x] C26 复用 `eneros_controlbus::constraint_check()`（D1，不重新实现 ConstraintChecker）
- [x] C27 Emergency 旁路：`ControlAction::Emergency` 下发 0.0，跳过 TTL + 约束（D8）
- [x] C28 Idle 处理：`ControlAction::Idle` 下发 0.0（D9）
- [x] C29 Charge/Discharge：下发截断后 setpoint（f32→f64→PointValue::Float，D10）
- [x] C30 `stats(&self) -> &ExecutorStats` 访问器

## MockPointAccess + MockDeviceStateProvider
- [x] C31 `MockPointAccess` 结构体（written_points: BTreeMap）
- [x] C32 实现 `PointAccess` trait 全部 6 个方法
- [x] C33 `last_write(point_id) -> Option<&PointValue>` 查询
- [x] C34 `fail_next_write(point_id)` 标记下次写入失败
- [x] C35 `MockDeviceStateProvider` 结构体
- [x] C36 实现 `DeviceStateProvider` trait
- [x] C37 `set_state(device_id, state)` 设置预设状态

## 集成测试
- [x] C38 T1 CommandExecutor 正常命令执行
- [x] C39 T2 CommandExecutor TTL 过期丢弃
- [x] C40 T3 CommandExecutor 约束超限截断
- [x] C41 T4 CommandExecutor 硬限制违反拒绝
- [x] C42 T5 CommandExecutor Emergency 旁路
- [x] C43 T6 CommandExecutor Idle 下发 0.0
- [x] C44 T7 CommandExecutor 写入失败统计
- [x] C45 T8 CommandExecutor 未映射设备跳过
- [x] C46 T9 CommandExecutor 多命令批量消费
- [x] C47 T10 CommandExecutor 空队列 tick
- [x] C48 T11 DevicePointMap 插入/查询/未映射
- [x] C49 T12 ExecutorStats 累加与多次 tick
- [x] C50 测试借用 controlbus `TEST_LOCK` 序列化（全局命令环）

## 设计文档
- [x] C51 `docs/kernel/cmd-executor-design.md` 存在
- [x] C52 文档包含 12 章节
- [x] C53 文档包含 2 Mermaid 图（执行链路流程图 + tick 时序图）
- [x] C54 D1~D12 偏差声明表
- [x] C55 文档位置在 `docs/kernel/` 下

## 版本号同步
- [x] C56 `Makefile` 版本号 0.55.0 → 0.56.0
- [x] C57 `.github/workflows/ci.yml` 版本号 0.55.0 → 0.56.0
- [x] C58 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-cmd-exec` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C59 `cargo metadata --format-version 1` 成功
- [x] C60 `cargo test -p eneros-rtos-cmd-exec` 全部通过
- [x] C61 `cargo build -p eneros-rtos-cmd-exec --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C62 `cargo fmt -p eneros-rtos-cmd-exec -- --check` 格式通过
- [x] C63 `cargo clippy -p eneros-rtos-cmd-exec --all-targets -- -D warnings` lint 通过
- [x] C64 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C65 rtos-cmd-exec 在 `crates/kernel/` 下（子系统归属正确）
- [x] C66 跨 crate path 引用使用相对路径
- [x] C67 设计文档在 `docs/kernel/` 下
- [x] C68 无根目录 crate
- [x] C69 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C70 所有 Rust 代码无 `use std::*`
- [x] C71 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C72 不要求 `Send + Sync`（D6 泛型，单线程）
- [x] C73 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`

## Karpathy 原则校验
- [x] C74 复用 v0.22.0 已有函数（ttl_check/constraint_check/command_consume），不重新实现（D1）
- [x] C75 未引入蓝图伪代码中不存在于实际 API 的类型（ControlBusReader/cmd.to_point_writes()）
- [x] C76 未过度抽象（TtlChecker/ConstraintChecker 结构体未创建，直接用函数）
