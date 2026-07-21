# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.52.0`
- [x] C2 members 列表已添加 `crates/protocols/telemetry-model`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/protocols/telemetry-model/Cargo.toml` 存在，package name 为 `eneros-telemetry-model`
- [x] C5 dependencies 仅包含 `eneros-upa-model = { path = "../upa-model" }`（D3）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：quality / digital / command / telemetry / telesignaling / telecontrol / teleadjust / deadband
- [x] C8 D1~D7 偏差声明表存在于 lib.rs doc comment

## quality 模块
- [x] C9 `QualityFlag` 枚举包含 7 个变体（Good/Invalid/Questionable/Substituted/Blocked/Overflow/Outdated）
- [x] C10 `QualityFlag` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C11 `is_valid()` 方法：Good 返回 true，其余 false
- [x] C12 `is_error()` 方法：Invalid/Blocked/Overflow/Outdated 返回 true

## digital 模块
- [x] C13 `DigitalState` 枚举包含 4 个变体（Off/On/Intermediate/Bad）
- [x] C14 `DigitalState` 派生 Debug/Clone/Copy/PartialEq/Eq/Hash
- [x] C15 `is_on()` / `is_off()` / `is_valid()` 方法实现

## command 模块
- [x] C16 `SingleCommand` 枚举（Off/On）
- [x] C17 `DoubleCommand` 枚举（Off/On/Intermediate/Bad）
- [x] C18 `ControlCommand` 枚举（Single(SingleCommand)/Double(DoubleCommand)）
- [x] C19 `ControlExecState` 枚举（Idle/Selected/Executing/Done/Failed/Timeout）
- [x] C20 `ControlExecState::is_terminal()` — Done/Failed/Timeout 返回 true
- [x] C21 `ControlExecState::is_active()` — Selected/Executing 返回 true

## telemetry 模块（Telemetry 遥测）
- [x] C22 `Telemetry` 结构体包含 11 个字段（point_id/device_id/name/value/unit/quality/timestamp_ms/deadband/high_limit/low_limit/last_reported）
- [x] C23 `timestamp_ms` 为 `u64` 类型（D1 时间戳注入）
- [x] C24 `new()` 构造函数（默认 quality=Good, deadband=0.0, limits=None）
- [x] C25 `should_report()` 死区过滤逻辑正确（None 首次上报；|value-last| > deadband 上报）
- [x] C26 `check_quality()` 越限检测（value > high_limit 或 < low_limit 置 Questionable）
- [x] C27 `update()` 方法更新值和时间戳
- [x] C28 `force_report()` 强制上报方法

## telesignaling 模块（Telesignaling 遥信）
- [x] C29 `Telesignaling` 结构体包含 8 个字段（point_id/device_id/name/value/quality/timestamp_ms/double_point/last_reported）
- [x] C30 `new()` 构造函数
- [x] C31 `should_report()` 状态变化立即上报（无死区）
- [x] C32 `update()` 方法更新值和时间戳
- [x] C33 `force_report()` 强制上报方法

## telecontrol 模块（Telecontrol 遥控）
- [x] C34 `Telecontrol` 结构体包含 8 个字段（point_id/device_id/name/command/quality/timestamp_ms/select_before_operate/exec_state）
- [x] C35 `new()` 构造函数（默认 exec_state=Idle）
- [x] C36 `select()` SBO 第一步：Idle → Selected
- [x] C37 `execute()` SBO 第二步：Selected → Executing；非 SBO：Idle → Executing
- [x] C38 `complete()` / `fail()` / `timeout()` 状态转换
- [x] C39 `is_complete()` 查询方法
- [x] C40 状态机错误处理（非 Idle select 返回错误；非 Selected/Idle execute 返回错误）

## teleadjust 模块（Teleadjust 遥调）
- [x] C41 `Teleadjust` 结构体包含 10 个字段（point_id/device_id/name/setpoint/current_value/quality/timestamp_ms/min_value/max_value/ramp_rate）
- [x] C42 `new()` 构造函数
- [x] C43 `validate()` 范围验证
- [x] C44 `set()` 设置设定值（超出范围返回错误）
- [x] C45 `update_current()` 更新当前实际值
- [x] C46 `is_in_range()` / `deviation()` 查询方法

## deadband 模块（DeadbandFilter）
- [x] C47 `PointDeadband` 结构体（deadband/last_reported/report_count/skip_count）
- [x] C48 `DeadbandFilter` 使用 `BTreeMap<PointId, PointDeadband>`（D6 no_std 兼容）
- [x] C49 `new()` / `configure()` 方法
- [x] C50 `should_report()` 死区过滤逻辑正确
- [x] C51 `force_report()` 强制上报方法
- [x] C52 `get_stats()` 返回 (report_count, skip_count)
- [x] C53 `point_count()` / `remove()` 方法

## 集成测试
- [x] C54 测试 1：Telemetry 死区过滤（值变化 ≤ 死区 → 不上报；> 死区 → 上报）
- [x] C55 测试 2：Telemetry 首次上报（last_reported=None → 上报）
- [x] C56 测试 3：Telemetry 品质检查（越限 → Questionable）
- [x] C57 测试 4：Telemetry force_report 强制上报
- [x] C58 测试 5：Telesignaling 状态变化立即上报
- [x] C59 测试 6：Telesignaling 状态不变不上报
- [x] C60 测试 7：Telecontrol SBO 流程（select → execute → complete）
- [x] C61 测试 8：Telecontrol 非 SBO 流程（execute → complete）
- [x] C62 测试 9：Telecontrol 失败/超时（execute → fail / timeout）
- [x] C63 测试 10：Telecontrol 状态机错误（Idle 非 select 直接 execute 返回错误）
- [x] C64 测试 11：Teleadjust 设定值范围验证
- [x] C65 测试 12：Teleadjust 偏差计算
- [x] C66 测试 13：DeadbandFilter 批量死区过滤
- [x] C67 测试 14：DeadbandFilter force_report 强制上报
- [x] C68 测试 15：DeadbandFilter get_stats 统计查询
- [x] C69 测试 16：DeadbandFilter remove 移除点配置
- [x] C70 测试 17：DeadbandFilter 死区 0（全部上报）
- [x] C71 测试 18：QualityFlag is_valid/is_error 语义
- [x] C72 测试 19：DigitalState is_on/is_off/is_valid 语义
- [x] C73 测试 20：ControlExecState is_terminal/is_active 语义

## 设计文档
- [x] C74 `docs/protocols/telemetry-model-design.md` 存在
- [x] C75 文档包含 12 章节 + Mermaid 架构图 + SBO 状态机图
- [x] C76 文档位置在 `docs/protocols/` 下（非 docs/ 根）

## 版本号同步
- [x] C77 `Makefile` 版本号 0.51.0 → 0.52.0
- [x] C78 `.github/workflows/ci.yml` 版本号 0.51.0 → 0.52.0
- [x] C79 `ci/src/gate.rs` 补充 v0.52.0 telemetry-model 注释

## 构建校验（§2.4.2 C6~C11）
- [x] C80 `cargo metadata --format-version 1` 成功
- [x] C81 `cargo test -p eneros-telemetry-model` 全部通过（20 passed; 0 failed）
- [x] C82 `cargo build -p eneros-telemetry-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C83 `cargo fmt -p eneros-telemetry-model -- --check` 格式检查通过
- [x] C84 `cargo clippy -p eneros-telemetry-model --all-targets -- -D warnings` lint 通过
- [x] C85 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C86 新 crate 在 `crates/<subsystem>/` 下（telemetry-model 在 protocols/）
- [x] C87 新文档在 `docs/<topic>/` 下（protocols）
- [x] C88 无根目录 crate
- [x] C89 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C90 telemetry-model 所有 Rust 代码无 `use std::*`
- [x] C91 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C92 不要求 `Send + Sync`（D7）
