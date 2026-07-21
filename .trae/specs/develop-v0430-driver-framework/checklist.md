# Checklist — v0.43.0 用户态驱动框架

## C1-C12: Task 1 — Crate 骨架 + lib.rs trait/类型/错误

- [x] C1: 目录 `crates/drivers/framework/` 存在
- [x] C2: `crates/drivers/framework/Cargo.toml` 存在
- [x] C3: Cargo.toml `name = "eneros-driver-framework"`
- [x] C4: Cargo.toml `version.workspace = true`
- [x] C5: Cargo.toml 零外部依赖（仅 alloc/core）
- [x] C6: `crates/drivers/framework/src/lib.rs` 存在
- [x] C7: lib.rs 包含 `#![cfg_attr(not(test), no_std)]`
- [x] C8: lib.rs 包含 `extern crate alloc;`
- [x] C9: `DriverId(pub u64)` 定义存在（Derive Copy/Clone/Debug/PartialEq/Eq/PartialOrd/Ord/Hash）
- [x] C10: `DriverType` 枚举定义（Serial/Network/Can/Storage/Gpio/I2c/Spi/Custom(u16)）
- [x] C11: `DriverState` 枚举定义（Uninitialized/Ready/Running/Stopped/Error/Dead）
- [x] C12: `DriverHealth` 枚举定义（Healthy/Degraded/Unhealthy/Unknown）
- [x] C13: `DeviceDriver` trait 定义（Send + Sync，10 个方法）
- [x] C14: `DriverError` 枚举定义（AlreadyRegistered/NotFound/PermissionDenied/InvalidState/InitFailed/StartFailed/StopFailed/DeinitFailed/NotRegistered）
- [x] C15: `DriverError` 实现 `core::fmt::Display`
- [x] C16: `DriverError` 实现 `core::error::Error`
- [x] C17: lib.rs 声明 `pub mod registry;` / `pub mod handle;` / `pub mod mock;`
- [x] C18: lib.rs re-exports 核心类型（DriverId/DriverType/DriverState/DriverHealth/DeviceDriver/DriverError）
- [x] C19: 单元测试 ≥6 个（DriverId/DriverType/DriverState/DriverHealth/DriverError Display/DriverError Eq）
- [x] C20: 文件创建正确（无语法错误）

## C21-C32: Task 2 — DriverHandle + DriverCapability

- [x] C21: 文件 `crates/drivers/framework/src/handle.rs` 存在
- [x] C22: `DriverPermission(pub u32)` 定义存在（D1：自包含位集）
- [x] C23: 权限常量定义（OPEN=0x01 / CONFIG=0x02 / IRQ=0x04 / ALL=0xFF）
- [x] C24: `DriverPermission` 方法（bits/from_bits/contains/is_empty/is_all）
- [x] C25: `DriverPermission` 实现 `BitOr` + `BitOrAssign`
- [x] C26: `DriverCapability` 定义存在（字段 owner_id: u64 + permissions: DriverPermission）
- [x] C27: `DriverCapability` Derive Clone/Copy/Debug/PartialEq/Eq（D8：Copy）
- [x] C28: `DriverCapability::new(owner_id, permissions)` 方法存在
- [x] C29: `DriverCapability::can_access(required) -> bool` 方法存在
- [x] C30: `DriverCapability::new_full(owner_id)` + `new_empty(owner_id)` 辅助构造存在
- [x] C31: `DriverHandle` 定义存在（字段 id: DriverId + cap: DriverCapability）
- [x] C32: `DriverHandle::new(id, cap)` + `id()` + `cap()` 方法存在
- [x] C33: 单元测试 ≥8 个（权限位运算/can_access 授权/can_access 拒绝/new_full/new_empty/Handle 构造/Handle id()/cap()/Copy 语义）
- [x] C34: 文件编译正确

## C35-C50: Task 3 — DriverRegistry + DriverEntry + DriverStats

- [x] C35: 文件 `crates/drivers/framework/src/registry.rs` 存在
- [x] C36: `DriverStats` 定义存在（D5：open_count/error_count/last_error/irq_count）
- [x] C37: `DriverStats` 方法（record_open/record_error/record_irq）+ Default
- [x] C38: `DriverEntry` 定义存在（driver: Box<dyn DeviceDriver> + required_perms + created_at: u64 + stats）
- [x] C39: `DriverRegistry` 定义存在（D2：BTreeMap/BTreeSet）
- [x] C40: `drivers: BTreeMap<DriverId, DriverEntry>` 字段
- [x] C41: `type_index: BTreeMap<DriverType, Vec<DriverId>>` 字段
- [x] C42: `name_index: BTreeMap<String, DriverId>` 字段
- [x] C43: `new()` 方法返回空注册表
- [x] C44: `register(driver, required_perms, now)` 方法存在（D3：now 参数）
- [x] C45: 重复注册返回 `Err(AlreadyRegistered)`
- [x] C46: `find_by_id(id)` 方法存在
- [x] C47: `find_by_type(dtype)` 方法存在
- [x] C48: `find_by_name(name)` 方法存在
- [x] C49: `open(id, cap)` 方法存在（D1：cap.can_access 校验）
- [x] C50: `open` 权限不足返回 `Err(PermissionDenied)`
- [x] C51: `open` 不存在返回 `Err(NotFound)`
- [x] C52: `unregister(id)` 方法存在
- [x] C53: `list()` + `count()` + `stats(id)` 方法存在
- [x] C54: 单元测试 ≥12 个（空注册表/注册成功/重复注册/find_by_id 命中/未命中/find_by_type/find_by_name/open 成功/权限不足/不存在/unregister 成功/不存在/list/count/stats）
- [x] C55: 文件编译正确

## C56-C66: Task 4 — MockDriver 测试桩

- [x] C56: 文件 `crates/drivers/framework/src/mock.rs` 存在
- [x] C57: `MockDriver` 结构体定义（id/name/driver_type/state/irq_log/health/init_fails/start_fails）
- [x] C58: `MockDriver::new(id, name, driver_type)` 构造方法（初始 Uninitialized）
- [x] C59: 配置方法 set_health/set_init_fails/set_start_fails 存在
- [x] C60: 查询方法 irq_log()/state() 存在
- [x] C61: 实现 `DeviceDriver` trait 的 `id()`/`name()`/`driver_type()`/`state()`
- [x] C62: `init()` 成功转 Ready，失败返回 Err(InitFailed)
- [x] C63: `start()` 成功转 Running，失败返回 Err(StartFailed)
- [x] C64: `stop()` 转 Stopped
- [x] C65: `deinit()` 转 Dead
- [x] C66: `handle_irq(irq_id)` 记录到 irq_log
- [x] C67: `health_check()` 返回配置的 health
- [x] C68: 单元测试 ≥8 个（初始状态/init 成功/init 失败/start 成功/start 失败/stop/deinit/handle_irq/health_check）
- [x] C69: 文件编译正确

## C70-C78: Task 5 — workspace 集成

- [x] C70: 根 `Cargo.toml` workspace members 包含 `"crates/drivers/framework"`
- [x] C71: lib.rs re-exports `DriverRegistry`
- [x] C72: lib.rs re-exports `DriverHandle` / `DriverCapability` / `DriverPermission`
- [x] C73: lib.rs re-exports `DriverStats`
- [x] C74: lib.rs re-exports `MockDriver`
- [x] C75: `cargo build -p eneros-driver-framework` 通过
- [x] C76: `cargo test -p eneros-driver-framework --lib` 通过
- [x] C77: `cargo clippy -p eneros-driver-framework --all-targets -- -D warnings` 通过

## C78-C88: Task 6 — 集成测试

- [x] C78: 文件 `crates/drivers/framework/tests/driver_framework_test.rs` 存在
- [x] C79: `test_mock_driver_lifecycle` 测试存在（init→start→stop→deinit 全流程）
- [x] C80: `test_registry_register_and_find_by_id` 测试存在
- [x] C81: `test_registry_find_by_type` 测试存在
- [x] C82: `test_registry_find_by_name` 测试存在
- [x] C83: `test_registry_duplicate_register` 测试存在
- [x] C84: `test_registry_open_with_capability` 测试存在
- [x] C85: `test_registry_open_permission_denied` 测试存在
- [x] C86: `test_registry_open_not_found` 测试存在
- [x] C87: `test_registry_unregister` 测试存在
- [x] C88: `test_registry_stats_and_list` 测试存在
- [x] C89: `cargo test -p eneros-driver-framework --test driver_framework_test` 通过

## C90-C96: Task 7 — 文档 + 配置

- [x] C90: 文件 `docs/drivers/driver-framework-design.md` 存在
- [x] C91: driver-framework-design.md ≥10 章
- [x] C92: 文档包含 mermaid 状态机图（DriverState 状态转换）
- [x] C93: 文档包含 mermaid 流程图（注册表数据流）
- [x] C94: 文档包含 D1-D9 偏差声明表
- [x] C95: 文件 `configs/driver-framework.toml` 存在（配置模板）
- [x] C96: 配置模板包含默认权限/日志级别/注册表容量配置项

## C97-C105: Task 8 — 版本同步

- [x] C97: 根 `Cargo.toml` version = "0.43.0"
- [x] C98: `Makefile` VERSION := 0.43.0
- [x] C99: `Makefile` 头部版本注释更新为 v0.43.0
- [x] C100: `.github/workflows/ci.yml` 版本注释更新为 v0.43.0
- [x] C101: `ci/src/gate.rs` clippy 注释更新为 v0.43.0（含 eneros-driver-framework 描述）
- [x] C102: `ci/src/gate.rs` test 注释更新为 v0.43.0（含 eneros-driver-framework 描述）
- [x] C103: `crates/agents/agent/src/lib.rs` VERSION = "0.43.0"
- [x] C104: `crates/agents/agent/src/lib.rs` 模块文档更新为 v0.43.0
- [x] C105: grep 无残留 0.42.1 版本号（历史引用除外）

## C106-C116: Task 9 — 构建验证

- [x] C106: `cargo fmt --all -- --check` 通过
- [x] C107: `cargo clippy -p eneros-driver-framework --all-targets -- -D warnings` 通过
- [x] C108: `cargo test -p eneros-driver-framework` 全部通过
- [x] C109: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [x] C110: WSL2 交叉编译 `cargo build -p eneros-driver-framework --target aarch64-unknown-none` 通过
- [x] C111: `cargo deny check licenses bans sources` 通过
- [x] C112: `cargo deny check advisories` 已知 GitHub 网络问题（环境限制）
- [x] C113: `cargo run -p eneros-ci` fmt+clippy 通过
- [x] C114: 无 `use std::*` 在非测试代码中
- [x] C115: 无 `panic!` / `todo!` / `unimplemented!` 在非测试代码中
- [x] C116: 无 `HashMap` / `HashSet` 在新代码中（使用 BTreeMap/BTreeSet）

## C117-C122: 偏差合规与目录结构

- [x] C117: D1 合规 — DriverCapability 自包含（owner_id + permissions），不依赖 eneros-agent CapabilityToken
- [x] C118: D2 合规 — DriverRegistry 使用 BTreeMap/BTreeSet
- [x] C119: D3 合规 — register() 接受 now: u64 参数注入时间戳
- [x] C120: D5 合规 — DriverStats 在框架内定义
- [x] C121: D6 合规 — DriverId 在框架内定义（pub struct DriverId(pub u64)）
- [x] C122: 目录结构合规 — framework crate 在 `crates/drivers/framework/`，文档在 `docs/drivers/`，配置在 `configs/`
