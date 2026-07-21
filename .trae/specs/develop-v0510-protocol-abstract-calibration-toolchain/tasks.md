# Tasks

## v0.51.0 协议抽象层

- [x] Task 1: 同步 workspace 版本号与 members 列表
  - [x] 修改 `e:\eneros\Cargo.toml`：`version = "0.50.0"` → `version = "0.51.0"`
  - [x] 在 members 数组中 `"crates/protocols/upa-model"` 之后增加 `"crates/protocols/protocol-abstract"` 和 `"crates/drivers/calibration"`
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 protocol-abstract crate 骨架
  - [x] 创建 `e:\eneros\crates\protocols\protocol-abstract\Cargo.toml`
    - package name = `eneros-protocol-abstract`，workspace 继承
    - dependencies: `eneros-upa-model = { path = "../upa-model" }`
  - [x] 创建 `e:\eneros\crates\protocols\protocol-abstract\src\lib.rs`
    - `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
    - 模块声明：error / address / mapping / config / access / adapter / manager / mock
    - D1~D7 偏差声明表
    - 重导出公共 API
  - 验证：`cargo build -p eneros-protocol-abstract` 编译通过

- [x] Task 3: 实现 error 模块
  - [x] 创建 `src/error.rs`
    - `ProtocolError` 枚举（PointNotFound / AdapterNotFound / AddrTypeMismatch / ReadFailed / WriteFailed / ProtocolInit / ProtocolNotStarted / InvalidConfig / Unsupported）
  - 验证：编译通过

- [x] Task 4: 实现 address + mapping + config 模块
  - [x] 创建 `src/address.rs` — `ProtocolAddress` 枚举（Modbus{slave_addr,reg_addr,func_code} / Iec104{common_addr,ioa,type_id} / Can{can_id,start_byte,length}）
  - [x] 创建 `src/mapping.rs` — `ProtocolPointMapping` 结构体（point_id/device_id/protocol_addr/data_type/scale/offset）+ 转换方法
  - [x] 创建 `src/config.rs` — `ProtocolType` 枚举 + `AdapterConfig` 结构体 + `DeviceConfig` 结构体
  - 验证：编译通过

- [x] Task 5: 实现 access 模块（PointAccess trait）
  - [x] 创建 `src/access.rs`
    - `PointAccess` trait（不要求 Send+Sync，D2）：
      - `read_point(&mut self, point_id) -> Result<DataPoint, ProtocolError>`
      - `read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>>`
      - `write_point(&mut self, point_id, value: PointValue) -> Result<(), ProtocolError>`
      - `write_points(&mut self, cmds: &[(PointId, PointValue)]) -> Vec<Result<(), ProtocolError>>`
      - `read_device_points(&mut self, device_id) -> Result<Vec<DataPoint>, ProtocolError>`
      - `protocol_type(&self) -> ProtocolType`
    - **不实现** subscribe/unsubscribe（D3）
  - 验证：编译通过

- [x] Task 6: 实现 adapter 模块（ProtocolAdapter trait）
  - [x] 创建 `src/adapter.rs`
    - `ProtocolAdapter` trait（继承 PointAccess）：
      - `init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError>`
      - `start(&mut self) -> Result<(), ProtocolError>`
      - `stop(&mut self) -> Result<(), ProtocolError>`
      - `poll(&mut self, now_ms: u64) -> Result<(), ProtocolError>` — D5: now_ms 注入
    - `AdapterState` 枚举（Uninitialized / Initialized / Running / Stopped / Error）
  - 验证：编译通过

- [x] Task 7: 实现 manager 模块（ProtocolManager）
  - [x] 创建 `src/manager.rs`
    - `ProtocolManager` 结构体：
      - adapters: `BTreeMap<ProtocolType, Box<dyn ProtocolAdapter>>`
      - point_routes: `BTreeMap<PointId, ProtocolType>` — 点 ID 到协议类型路由
    - 方法：
      - `new() -> Self`
      - `register_adapter(&mut self, adapter: Box<dyn ProtocolAdapter>)`
      - `register_route(&mut self, point_id, protocol_type)` — 注册点 ID 路由
      - `read_point(&mut self, point_id) -> Result<DataPoint, ProtocolError>` — 按 route 路由到适配器
      - `read_points(&mut self, point_ids) -> Vec<Result<DataPoint, ProtocolError>>`
      - `write_point(&mut self, point_id, value) -> Result<(), ProtocolError>`
      - `poll_all(&mut self, now_ms: u64)` — 所有适配器 poll
      - `adapter_count(&self) -> usize`
      - `adapter_state(&self, protocol_type) -> Option<AdapterState>`
  - 验证：编译通过

- [x] Task 8: 实现 mock 模块（MockAdapter）
  - [x] 创建 `src/mock.rs`（`#[cfg(test)]`）
    - `MockAdapter` 实现 `ProtocolAdapter` trait
    - 内部状态：
      - points: `BTreeMap<PointId, DataPoint>` — 模拟点表
      - mappings: `BTreeMap<PointId, ProtocolPointMapping>`
      - state: `AdapterState`
      - protocol_type: `ProtocolType`
      - poll_count: `u32`（统计 poll 次数）
    - 方法：
      - `new(protocol_type) -> Self`
      - `set_point(point_id, point)` — 设置模拟点值
      - `poll_count(&self) -> u32`
      - 实现 trait 所有方法（read_point 从 points 读取，write_point 更新 points，poll 递增计数）
  - 验证：编译通过

- [x] Task 9: 集成测试（protocol-abstract）
  - [x] 在 `src/lib.rs` 的 `#[cfg(test)] mod tests` 中编写集成测试（15 个测试全部通过）：
    - 测试 1：MockAdapter read_point 正常
    - 测试 2：MockAdapter read_point 点不存在返回 PointNotFound
    - 测试 3：MockAdapter write_point 更新点值
    - 测试 4：MockAdapter read_points 批量读取
    - 测试 5：MockAdapter read_device_points 按设备读取
    - 测试 6：ProtocolManager 注册适配器 + 路由
    - 测试 7：ProtocolManager read_point 按路由分发
    - 测试 8：ProtocolManager poll_all 所有适配器
    - 测试 9：ProtocolAdapter 生命周期（init→start→poll→stop）
    - 测试 10：ProtocolAddress 枚举构造与匹配
    - 测试 11：ProtocolPointMapping 转换（原始值→工程量）
    - 测试 12：多协议共存（两个 MockAdapter 不同 protocol_type）
  - 验证：`cargo test -p eneros-protocol-abstract` 全部通过（15 passed）

- [x] Task 10: 设计文档（protocol-abstract）
  - [x] 创建 `e:\eneros\docs\protocols\protocol-abstract-design.md`
    - 12 章节 + 2 个 Mermaid 图（协议栈分层架构图 + trait 类关系图）
  - 验证：文档位置在 `docs/protocols/` 下

## v0.51.1 计量校准

- [x] Task 11: 创建 calibration crate 骨架
  - [x] 创建 `e:\eneros\crates\drivers\calibration\Cargo.toml`
    - package name = `eneros-calibration`，workspace 继承
    - **零 dependencies**（D10）
  - [x] 创建 `e:\eneros\crates\drivers\calibration\src\lib.rs`
    - `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
    - 模块声明：coeffs / accuracy / result / trait_def / store / func
    - D8~D10 偏差声明表
  - 验证：编译通过

- [x] Task 12: 实现 calibration 核心类型
  - [x] 创建 `src/coeffs.rs` — `CalibCoeffs` 结构体 + apply_voltage/apply_current
  - [x] 创建 `src/accuracy.rs` — `AccuracyClass` 枚举 + is_within_class
  - [x] 创建 `src/result.rs` — `CalibResult` 结构体 + `MeterReading` 结构体
  - [x] 创建 `src/trait_def.rs` — `MeterCalibration` trait（避开 trait 关键字）
  - [x] 创建 `src/store.rs` — `CalibStore` trait + `InMemoryCalibStore` 实现
  - [x] 创建 `src/func.rs` — `DefaultCalibrator` + `calibrate_meter` / `verify_accuracy` 函数
  - 验证：编译通过

- [x] Task 13: calibration 测试
  - [x] 单元测试 + 集成测试（12 个测试全部通过）：
    - 变比计算/误差分类/系数持久化/校准函数/精度验证
  - 验证：`cargo test -p eneros-calibration` 全部通过（12 passed）

- [x] Task 14: 设计文档（calibration）
  - [x] 创建 `e:\eneros\docs\drivers\meter-calibration-design.md`（12 章节 + Mermaid 校准流程图）
  - 验证：文档位置在 `docs/drivers/` 下

## v0.51.2 调试与工厂测试工具链

- [x] Task 15: 创建 tools/device_simulator
  - [x] 创建 `e:\eneros\tools\device_simulator\Cargo.toml`（独立 crate，**std**，D11，独立 workspace）
  - [x] 创建 `e:\eneros\tools\device_simulator\src\main.rs` — 设备模拟器入口
  - [x] 创建 `e:\eneros\tools\device_simulator\src\sim.rs` — `SimConfig`/`SimHandle`/`SimError` + 模拟逻辑
  - 验证：`cargo build` 编译通过 + 4 测试通过（独立编译，非 workspace 成员 D14）

- [x] Task 16: 创建 tools/protocol_analyzer
  - [x] 创建 `e:\eneros\tools\protocol_analyzer\Cargo.toml`（独立 crate，**std**）
  - [x] 创建 `e:\eneros\tools\protocol_analyzer\src\main.rs` — 分析器入口
  - [x] 创建 `e:\eneros\tools\protocol_analyzer\src\capture.rs` — `Packet` 类型 + 抓包逻辑
  - 验证：`cargo build` 编译通过 + 3 测试通过

- [x] Task 17: 创建 tools/batch_config
  - [x] 创建 `e:\eneros\tools\batch_config\Cargo.toml`（独立 crate，**std**）
  - [x] 创建 `e:\eneros\tools\batch_config\src\main.rs` — 批量配置入口
  - [x] 创建 `e:\eneros\tools\batch_config\src\runner.rs` — `TestSuite`/`TestItem`/`TestCategory`/`TestReport`/`TestFailure` + `FactoryTestRunner` trait + `DefaultTestRunner` 实现
  - 验证：`cargo build` 编译通过 + 4 测试通过

- [x] Task 18: 设计文档（factory-test-toolchain）
  - [x] 创建 `e:\eneros\docs\runtime\factory-test-toolchain.md`（12 章节 + Mermaid 架构图）
  - 验证：文档位置在 `docs/runtime/` 下

## 通用收尾

- [x] Task 19: 更新 Makefile / ci.yml / gate.rs 版本号
  - [x] `e:\eneros\Makefile`：0.50.0 → 0.51.0
  - [x] `e:\eneros\.github\workflows\ci.yml`：0.50.0 → 0.51.0
  - [x] `e:\eneros\ci\src\gate.rs`：补充 v0.51.x 新 crate 注释（protocol-abstract + calibration）
  - 验证：版本号已同步

- [x] Task 20: 构建校验（C6~C11）
  - [x] `cargo metadata --format-version 1` — workspace 解析成功（METADATA_OK）
  - [x] `cargo test -p eneros-protocol-abstract -p eneros-calibration` — 全部测试通过（27 passed: 15 protocol-abstract + 12 calibration）
  - [x] `cargo build -p eneros-protocol-abstract -p eneros-calibration --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` — 交叉编译通过（CROSS_COMPILE_OK）
  - [x] `cargo fmt -p eneros-protocol-abstract -p eneros-calibration -- --check` — 格式检查通过（FMT_OK）
  - [x] `cargo clippy -p eneros-protocol-abstract -p eneros-calibration --all-targets -- -D warnings` — lint 通过（CLIPPY_OK）
  - [x] `cargo deny check advisories licenses bans sources` — 安全扫描通过（DENY_OK, advisories/bans/licenses/sources all ok）

# Task Dependencies
## v0.51.0
- Task 1 独立（workspace 准备）
- Task 2 依赖 Task 1
- Task 3~6 依赖 Task 2，相互独立
- Task 7 依赖 Task 3~6
- Task 8 依赖 Task 5+6（mock 实现 trait）
- Task 9 依赖 Task 7+8
- Task 10 依赖 Task 7

## v0.51.1
- Task 11 依赖 Task 1（workspace members）
- Task 12 依赖 Task 11
- Task 13 依赖 Task 12
- Task 14 依赖 Task 12

## v0.51.2
- Task 15~17 相互独立，不依赖 workspace（独立 std 程序）
- Task 18 依赖 Task 15~17

## 通用
- Task 19 独立
- Task 20 依赖全部完成
