# Checklist

## v0.51.0 协议抽象层

### Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.51.0`
- [x] C2 members 列表已添加 `crates/protocols/protocol-abstract` 和 `crates/drivers/calibration`
- [x] C3 `cargo metadata --format-version 1` 解析成功（METADATA_OK）

### Crate 骨架（protocol-abstract）
- [x] C4 `crates/protocols/protocol-abstract/Cargo.toml` 存在，package name 为 `eneros-protocol-abstract`
- [x] C5 dependencies 仅包含 `eneros-upa-model = { path = "../upa-model" }`（D1）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / address / mapping / config / access / adapter / manager / mock
- [x] C8 D1~D7 偏差声明表存在于 lib.rs doc comment

### error 模块
- [x] C9 `ProtocolError` 枚举包含 9 个变体（PointNotFound/AdapterNotFound/AddrTypeMismatch/ReadFailed/WriteFailed/ProtocolInit/ProtocolNotStarted/InvalidConfig/Unsupported）

### address + mapping + config 模块
- [x] C10 `ProtocolAddress` 枚举包含 Modbus/Iec104/Can 三类变体，字段完整
- [x] C11 `ProtocolPointMapping` 结构体包含 point_id/device_id/protocol_addr/data_type/scale/offset 字段 + 转换方法（原始值→工程量）
- [x] C12 `ProtocolType` 枚举包含 ModbusRtu/ModbusTcp/Iec104/Can/Internal
- [x] C13 `AdapterConfig` 结构体字段合理（name/protocol_type/device_configs 等）
- [x] C14 `DeviceConfig` 结构体字段合理

### access 模块（PointAccess trait）
- [x] C15 `PointAccess` trait 包含 6 个方法（read_point/read_points/write_point/write_points/read_device_points/protocol_type）
- [x] C16 `PointAccess` **不派生** `Send + Sync`（D2）
- [x] C17 `PointAccess` **不包含** subscribe/unsubscribe 方法（D3）

### adapter 模块（ProtocolAdapter trait）
- [x] C18 `ProtocolAdapter` trait 继承 `PointAccess`
- [x] C19 `ProtocolAdapter` 包含 init/start/stop/poll 4 个方法
- [x] C20 `poll` 方法签名包含 `now_ms: u64` 参数（D5 时间戳注入）
- [x] C21 `AdapterState` 枚举包含 5 个状态（Uninitialized/Initialized/Running/Stopped/Error）

### manager 模块（ProtocolManager）
- [x] C22 `ProtocolManager` 持有 `BTreeMap<ProtocolType, Box<dyn ProtocolAdapter>>`（D4 不使用 Arc<RwLock>）
- [x] C23 `ProtocolManager` 持有 `BTreeMap<PointId, ProtocolType>` 点 ID 路由表
- [x] C24 `register_adapter` / `register_route` / `read_point` / `read_points` / `write_point` / `poll_all` 方法实现
- [x] C25 `adapter_count` / `adapter_state` 查询方法实现

### mock 模块（MockAdapter）
- [x] C26 `MockAdapter` 实现 `ProtocolAdapter` trait
- [x] C27 `MockAdapter::new(protocol_type)` 构造函数
- [x] C28 `MockAdapter::set_point(point_id, point)` 设置模拟点值
- [x] C29 `MockAdapter::poll_count()` 返回 poll 次数

### 集成测试
- [x] C30 测试 1：MockAdapter read_point 正常
- [x] C31 测试 2：MockAdapter read_point 点不存在返回 PointNotFound
- [x] C32 测试 3：MockAdapter write_point 更新点值
- [x] C33 测试 4：MockAdapter read_points 批量读取
- [x] C34 测试 5：MockAdapter read_device_points 按设备读取
- [x] C35 测试 6：ProtocolManager 注册适配器 + 路由
- [x] C36 测试 7：ProtocolManager read_point 按路由分发
- [x] C37 测试 8：ProtocolManager poll_all 所有适配器
- [x] C38 测试 9：ProtocolAdapter 生命周期（init→start→poll→stop）
- [x] C39 测试 10：ProtocolAddress 枚举构造与匹配
- [x] C40 测试 11：ProtocolPointMapping 转换（原始值→工程量）
- [x] C41 测试 12：多协议共存（两个 MockAdapter 不同 protocol_type）

### 设计文档
- [x] C42 `docs/protocols/protocol-abstract-design.md` 存在
- [x] C43 文档包含 12 章节 + 2 个 Mermaid 图（协议栈分层架构图 + trait 类关系图）
- [x] C44 文档位置在 `docs/protocols/` 下（非 docs/ 根）

## v0.51.1 计量校准

### Crate 骨架（calibration）
- [x] C45 `crates/drivers/calibration/Cargo.toml` 存在，package name 为 `eneros-calibration`
- [x] C46 dependencies 为空（D10 零依赖）
- [x] C47 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C48 模块声明完整：coeffs / accuracy / result / trait_def / store / func
- [x] C49 D8~D10 偏差声明表存在

### 核心类型
- [x] C50 `CalibCoeffs` 结构体包含 ct_ratio/pt_ratio/phase_correction/offset_voltage/offset_current/calibrated_at 字段
- [x] C51 `AccuracyClass` 枚举包含 Class0_2S/Class0_5S/Class1_0/Class2_0
- [x] C52 `CalibResult` 结构体包含校准前后误差、精度等级、是否通过字段
- [x] C53 `MeterReading` 结构体定义
- [x] C54 `MeterCalibration` trait 包含 apply_coefficients/measure_error/classify_accuracy 方法

### 持久化
- [x] C55 `CalibStore` trait 包含 load/save 方法（D9 抽象持久化）
- [x] C56 `InMemoryCalibStore` 实现 `CalibStore` trait（测试用）

### 函数
- [x] C57 `calibrate_meter` 函数实现
- [x] C58 `verify_accuracy` 函数实现

### 测试
- [x] C59 变比计算测试通过
- [x] C60 误差分类测试通过
- [x] C61 系数持久化测试通过（InMemoryCalibStore save→load）

### 设计文档
- [x] C62 `docs/drivers/meter-calibration-design.md` 存在
- [x] C63 文档位置在 `docs/drivers/` 下

## v0.51.2 调试与工厂测试工具链

### device_simulator
- [x] C64 `tools/device_simulator/Cargo.toml` 存在（std，非 workspace 成员 D14，独立 workspace 根）
- [x] C65 `src/main.rs` 入口存在
- [x] C66 `src/sim.rs` 包含 `SimConfig`/`SimHandle`/`SimError` 类型
- [x] C67 独立 `cargo build` 编译通过（D11 std 程序）+ 4 测试通过

### protocol_analyzer
- [x] C68 `tools/protocol_analyzer/Cargo.toml` 存在（std）
- [x] C69 `src/main.rs` 入口存在
- [x] C70 `src/capture.rs` 包含 `Packet` 类型 + 抓包逻辑
- [x] C71 独立 `cargo build` 编译通过 + 3 测试通过

### batch_config
- [x] C72 `tools/batch_config/Cargo.toml` 存在（std）
- [x] C73 `src/main.rs` 入口存在
- [x] C74 `src/runner.rs` 包含 `TestSuite`/`TestItem`/`TestCategory`/`TestReport`/`TestFailure` 类型
- [x] C75 `FactoryTestRunner` trait + `DefaultTestRunner` 实现
- [x] C76 独立 `cargo build` 编译通过 + 4 测试通过

### 设计文档
- [x] C77 `docs/runtime/factory-test-toolchain.md` 存在
- [x] C78 文档位置在 `docs/runtime/` 下

## 通用收尾

### 版本号同步
- [x] C79 `Makefile` 版本号 0.50.0 → 0.51.0
- [x] C80 `.github/workflows/ci.yml` 版本号 0.50.0 → 0.51.0
- [x] C81 `ci/src/gate.rs` 补充 v0.51.x 新 crate 注释（protocol-abstract + calibration）

### 构建校验（§2.4.2 C6~C11）
- [x] C82 `cargo metadata --format-version 1` 成功（METADATA_OK）
- [x] C83 `cargo test -p eneros-protocol-abstract -p eneros-calibration` 全部通过（27 passed: 15 + 12）
- [x] C84 `cargo build --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过（CROSS_COMPILE_OK）
- [x] C85 `cargo fmt -- --check` 格式检查通过（FMT_OK）
- [x] C86 `cargo clippy --all-targets -- -D warnings` lint 通过（CLIPPY_OK）
- [x] C87 `cargo deny check advisories licenses bans sources` 安全扫描通过（DENY_OK, advisories/bans/licenses/sources all ok）

### 目录结构校验（§2.4.1）
- [x] C88 新 crate 在 `crates/<subsystem>/` 下（protocol-abstract 在 protocols/，calibration 在 drivers/）
- [x] C89 新文档在 `docs/<topic>/` 下（protocols/drivers/runtime）
- [x] C90 无根目录 crate（tools/ 例外，D12 工具非产品代码）
- [x] C91 .gitignore 覆盖新产生的文件类型
- [x] C92 提交信息遵循 Conventional Commits

### no_std 合规
- [x] C93 protocol-abstract 所有 Rust 代码无 `use std::*`
- [x] C94 calibration 所有 Rust 代码无 `use std::*`
- [x] C95 tools/ 例外（D11 std 程序）
