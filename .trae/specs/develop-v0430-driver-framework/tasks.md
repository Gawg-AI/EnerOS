# Tasks — v0.43.0 用户态驱动框架

## Wave 1: Crate 骨架 + 类型/trait/错误（lib.rs）

- [x] Task 1: 创建 eneros-driver-framework crate 骨架 + lib.rs 核心 trait/类型/错误
  - 新建目录 `crates/drivers/framework/`
  - 新建 `crates/drivers/framework/Cargo.toml`：
    - `name = "eneros-driver-framework"` / `version.workspace = true` / `edition.workspace = true` / `authors.workspace = true` / `license.workspace = true`
    - `description = "EnerOS user-space device driver framework — DeviceDriver trait, registry, capability"`
    - 零外部依赖（仅 `alloc`/`core`）
  - 新建 `crates/drivers/framework/src/lib.rs`：
    - `#![cfg_attr(not(test), no_std)]`
    - `extern crate alloc;`
    - 模块声明：`pub mod registry;` / `pub mod handle;` / `pub mod mock;`
    - 定义 `DriverId(pub u64)`（D6：Copy/Clone/Debug/PartialEq/Eq/PartialOrd/Ord/Hash）
    - 定义 `DriverType` 枚举（Serial/Network/Can/Storage/Gpio/I2c/Spi/Custom(u16)，Derive Debug/Clone/Copy/PartialEq/Eq/Hash）
    - 定义 `DriverState` 枚举（Uninitialized/Ready/Running/Stopped/Error/Dead，Derive Debug/Clone/Copy/PartialEq/Eq）
    - 定义 `DriverHealth` 枚举（Healthy/Degraded/Unhealthy/Unknown，Derive Debug/Clone/Copy/PartialEq/Eq）
    - 定义 `DeviceDriver` trait（Send + Sync，10 个方法：id/name/driver_type/state/init/start/stop/deinit/handle_irq/health_check）
    - 定义 `DriverError` 枚举（AlreadyRegistered/NotFound/PermissionDenied/InvalidState/InitFailed/StartFailed/StopFailed/DeinitFailed/NotRegistered，Derive Debug/Clone/PartialEq/Eq）
    - 为 `DriverError` 实现 `core::fmt::Display` + `core::error::Error`
    - re-exports：`DriverId` / `DriverType` / `DriverState` / `DriverHealth` / `DeviceDriver` / `DriverError`
    - 单元测试（≥6）：DriverId 构造与比较 / DriverType 变体 / DriverState 全变体 / DriverHealth 全变体 / DriverError Display 输出 / DriverError Eq 比较
  - 暂不加入 workspace Cargo.toml（Task 5 统一添加）
  - 验证：文件创建正确（无语法错误）

## Wave 2: 注册表 + 句柄 + 能力令牌（并行，依赖 Task 1）

- [x] Task 2: 创建 `framework/src/handle.rs` — DriverHandle + DriverCapability
  - 新建文件 `crates/drivers/framework/src/handle.rs`
  - 定义 `DriverPermission(pub u32)`（D1：自包含权限位集，手动 bitflags）
    - 常量：`OPEN: u32 = 0x01` / `CONFIG: u32 = 0x02` / `IRQ: u32 = 0x04` / `ALL: u32 = 0xFF`
    - 方法：`bits()` / `from_bits(bits)` / `contains(other)` / `is_empty()` / `is_all()`
    - 实现 `BitOr` / `BitOrAssign`（返回 DriverPermission）
  - 定义 `DriverCapability`（D1：自包含能力令牌，D8：Copy）
    - 字段：`owner_id: u64` / `permissions: DriverPermission`
    - Derive：Clone/Copy/Debug/PartialEq/Eq
    - 方法：`new(owner_id: u64, permissions: DriverPermission) -> Self` / `can_access(&self, required: DriverPermission) -> bool` / `owner(&self) -> u64` / `permissions(&self) -> DriverPermission`
    - 辅助构造：`new_full(owner_id) -> Self`（ALL 权限）/ `new_empty(owner_id) -> Self`（无权限）
  - 定义 `DriverHandle`（D8：持 DriverCapability）
    - 字段：`id: DriverId` / `cap: DriverCapability`
    - Derive：Clone/Copy/Debug/PartialEq/Eq
    - 方法：`new(id: DriverId, cap: DriverCapability) -> Self` / `id(&self) -> DriverId` / `cap(&self) -> DriverCapability`
  - 单元测试（≥8）：权限位集运算 / can_access 授权 / can_access 拒绝 / new_full / new_empty / DriverHandle 构造 / DriverHandle id()/cap() / Copy 语义
  - 验证：文件编译正确

- [x] Task 3: 创建 `framework/src/registry.rs` — DriverRegistry + DriverEntry + DriverStats
  - 新建文件 `crates/drivers/framework/src/registry.rs`
  - 定义 `DriverStats`（D5：蓝图引用但未定义）
    - 字段：`open_count: u32` / `error_count: u32` / `last_error: Option<DriverError>` / `irq_count: u32`
    - Derive：Clone/Debug/Default
    - 方法：`record_open(&mut self)` / `record_error(&mut self, err)` / `record_irq(&mut self)`
  - 定义 `DriverEntry`（内部结构）
    - 字段：`driver: Box<dyn DeviceDriver>` / `required_perms: DriverPermission` / `created_at: u64`（D3：注入时间戳）/ `stats: DriverStats`
  - 定义 `DriverRegistry`（D2：BTreeMap/BTreeSet）
    - 字段：`drivers: BTreeMap<DriverId, DriverEntry>` / `type_index: BTreeMap<DriverType, Vec<DriverId>>` / `name_index: BTreeMap<String, DriverId>`
    - 方法：
      - `new() -> Self`
      - `register(&mut self, driver: Box<dyn DeviceDriver>, required_perms: DriverPermission, now: u64) -> Result<DriverId, DriverError>`（D3：now 参数；检查 ID 冲突→AlreadyRegistered；更新 type_index/name_index）
      - `find_by_id(&self, id: &DriverId) -> Option<DriverId>`
      - `find_by_type(&self, dtype: DriverType) -> Vec<DriverId>`
      - `find_by_name(&self, name: &str) -> Option<DriverId>`
      - `open(&self, id: &DriverId, cap: &DriverCapability) -> Result<DriverHandle, DriverError>`（D1：cap.can_access(required_perms)→PermissionDenied；NotFound；返回 DriverHandle）
      - `unregister(&mut self, id: &DriverId) -> Result<(), DriverError>`（移除并清理索引→NotRegistered）
      - `list(&self) -> Vec<DriverId>`（列出所有驱动 ID）
      - `count(&self) -> usize`
      - `stats(&self, id: &DriverId) -> Option<&DriverStats>`
  - 单元测试（≥12）：空注册表 / 注册成功 / 重复注册失败 / find_by_id 命中 / find_by_id 未命中 / find_by_type / find_by_name / open 成功 / open 权限不足 / open 不存在 / unregister 成功 / unregister 不存在 / list / count / stats 查询
  - 验证：文件编译正确

## Wave 3: MockDriver 测试桩（依赖 Task 1+2+3）

- [x] Task 4: 创建 `framework/src/mock.rs` — MockDriver 测试桩
  - 新建文件 `crates/drivers/framework/src/mock.rs`
  - 定义 `MockDriver` 结构体（实现 DeviceDriver）
    - 字段：`id: DriverId` / `name: String` / `driver_type: DriverType` / `state: DriverState` / `irq_log: Vec<u32>` / `health: DriverHealth` / `init_fails: bool` / `start_fails: bool`
    - Derive：Debug
    - 方法：`new(id: DriverId, name: &str, driver_type: DriverType) -> Self`（初始 Uninitialized）
    - 配置方法：`set_health(&mut self, h: DriverHealth)` / `set_init_fails(&mut self, v: bool)` / `set_start_fails(&mut self, v: bool)`
    - 查询方法：`irq_log(&self) -> &[u32]` / `state(&self) -> DriverState`（覆盖 trait 的 state()）
    - 实现 `DeviceDriver` trait：
      - `id()` / `name()` / `driver_type()` / `state()` 返回字段
      - `init()`：若 init_fails 返回 Err(InitFailed)，否则 state=Ready，Ok
      - `start()`：若 start_fails 返回 Err(StartFailed)，否则 state=Running，Ok
      - `stop()`：state=Stopped，Ok
      - `deinit()`：state=Dead，Ok
      - `handle_irq(irq_id)`：push 到 irq_log
      - `health_check()`：返回 health 字段
  - 单元测试（≥8）：构造初始状态 / init 成功转 Ready / init 失败 / start 成功转 Running / start 失败 / stop 转 Stopped / deinit 转 Dead / handle_irq 记录 / health_check 返回配置值
  - 验证：文件编译正确

## Wave 4: workspace 集成（依赖 Task 2+3+4）

- [x] Task 5: 更新 workspace Cargo.toml + lib.rs 集成
  - 修改根 `Cargo.toml`：workspace members 新增 `"crates/drivers/framework"`
  - 修改 `crates/drivers/framework/src/lib.rs`：确认 re-exports 完整（DriverRegistry / DriverEntry / DriverStats / DriverHandle / DriverCapability / DriverPermission / MockDriver）
  - 验证：`cargo build -p eneros-driver-framework` 通过
  - 验证：`cargo test -p eneros-driver-framework --lib` 通过（单元测试）

## Wave 5: 集成测试（依赖 Task 5）

- [x] Task 6: 编写 v0.43.0 集成测试 `tests/driver_framework_test.rs`
  - 新建文件 `crates/drivers/framework/tests/driver_framework_test.rs`
  - 10 个集成测试：
    1. `test_mock_driver_lifecycle` — MockDriver init→start→stop→deinit 全流程状态转换
    2. `test_registry_register_and_find_by_id` — 注册后按 ID 查找命中
    3. `test_registry_find_by_type` — 按类型查找（注册多个 Serial 驱动）
    4. `test_registry_find_by_name` — 按名称查找
    5. `test_registry_duplicate_register` — 重复注册返回 AlreadyRegistered
    6. `test_registry_open_with_capability` — 有权限 cap open 成功
    7. `test_registry_open_permission_denied` — 无权限 cap 返回 PermissionDenied
    8. `test_registry_open_not_found` — 不存在 ID 返回 NotFound
    9. `test_registry_unregister` — 注销后 find_by_id 返回 None
    10. `test_registry_stats_and_list` — stats 查询 + list 列出所有 + count
  - 验证：`cargo test -p eneros-driver-framework --test driver_framework_test` 通过

## Wave 6: 文档 + 配置（并行，依赖 Task 5）

- [x] Task 7: 编写设计文档 + 配置模板
  - 新建 `docs/drivers/driver-framework-design.md`（≥10 章，含 mermaid 图，D1-D9 偏差表）：
    1. 概述与版本定位
    2. 架构定位（P1-F 基石）
    3. DeviceDriver trait 设计
    4. DriverType / DriverState / DriverHealth 类型
    5. DriverRegistry 注册表设计
    6. DriverHandle + DriverCapability 能力模型
    7. 驱动生命周期状态机（mermaid stateDiagram）
    8. 注册表数据流（mermaid flowchart）
    9. 测试策略
    10. 偏差声明（D1-D9 表）
    11. 未来演进（Phase 3 seL4 集成）
  - 新建 `configs/driver-framework.toml`（配置模板：默认权限/日志级别/注册表容量）
  - 验证：文档存在且 ≥10 章

## Wave 7: 版本同步 + 构建验证（依赖所有）

- [x] Task 8: 同步版本标识符 0.42.1 → 0.43.0
  - 根 `Cargo.toml`：`version = "0.43.0"`
  - `Makefile`：`VERSION := 0.43.0` + 头部版本注释
  - `.github/workflows/ci.yml`：版本注释更新为 v0.43.0
  - `ci/src/gate.rs`：2 处版本注释（clippy + test，描述新增 eneros-driver-framework）
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.43.0"` + 模块文档注释更新
  - 验证：grep 无残留 0.42.1 版本号（历史引用除外）

- [x] Task 9: 完整构建验证
  - `cargo fmt --all -- --check` — 格式检查
  - `cargo clippy -p eneros-driver-framework --all-targets -- -D warnings` — lint
  - `cargo test -p eneros-driver-framework` — 单元+集成测试
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` — workspace 回归
  - WSL2 交叉编译：`cargo build -p eneros-driver-framework --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - `cargo deny check licenses bans sources` — 许可证检查
  - `cargo run -p eneros-ci` — CI 质量门禁
  - no_std 合规检查：无 `use std::*` / 无 `panic!` / 无 `todo!` / 无 `unimplemented!` / 无 `HashMap`/`HashSet`

# Task Dependencies

- Task 1: 无依赖（Wave 1）
- Task 2, Task 3: 依赖 Task 1（Wave 2 并行）
- Task 4: 依赖 Task 1 + Task 2 + Task 3（MockDriver 实现 DeviceDriver trait，用 DriverId/DriverType 等）
- Task 5: 依赖 Task 2 + Task 3 + Task 4（workspace 集成需全部源文件就绪）
- Task 6: 依赖 Task 5（集成测试需 workspace 能解析 crate）
- Task 7: 依赖 Task 5（文档描述已实现的设计）
- Task 8: 依赖 Task 6 + Task 7（版本同步在测试通过后）
- Task 9: 依赖 Task 8（最终构建验证）
