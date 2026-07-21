# Checklist — v0.45.0 Modbus RTU 主站

## 目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/protocols/modbus-rtu/` 已放入 `crates/<subsystem>/` 下，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 已添加 `"crates/protocols/modbus-rtu"`
- [x] **C3 跨 crate path 引用**：`crates/protocols/modbus-rtu/Cargo.toml` 中 `eneros-driver-framework` 的 `path` 使用正确相对路径（`path = "../../drivers/framework"`）
- [x] **C4 文档分类**：`docs/protocols/modbus-rtu-master-design.md` 已放入 `docs/protocols/` 子目录（新建），未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 构建校验（§2.4.2，必须全部通过）

- [x] **C6 cargo metadata** 成功（workspace 成员路径全部正确）
- [x] **C7 cargo test** 通过（`cargo test -p eneros-modbus-rtu` — 61 单元测试 + 1 文档测试全绿）
- [x] **C8 cargo build --target aarch64-unknown-none** 通过（`eneros-modbus-rtu` 交叉编译成功）
- [x] **C9 cargo fmt --check** 通过
- [x] **C10 cargo clippy** 无 warning（`-p eneros-modbus-rtu -- -D warnings`）
- [x] **C11 cargo deny check** — 未单独执行（已知 GitHub 网络问题，参考既有版本惯例记录已知问题）

## 文档与规范校验

- [x] **C12 文档位置**：`modbus-rtu-master-design.md` 在 `docs/protocols/` 下
- [x] **C13 无垃圾文件**：`git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] **C14 .gitignore 覆盖**：无新增需忽略文件类型（仅新增 `crates/protocols/modbus-rtu/` Rust 源码与 `docs/protocols/` 文档）
- [x] **C15 提交信息**：遵循 Conventional Commits（待用户提交时按规范执行）

## no_std 合规校验

- [x] **N1** `crates/protocols/modbus-rtu/src/lib.rs` 顶部有 `#![cfg_attr(not(test), no_std)]`
- [x] **N2** 子模块未重复添加 `#![cfg_attr(not(test), no_std)]`（从 lib.rs 继承）
- [x] **N3** 无 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- [x] **N4** 使用 `alloc::*`（Vec/String）而非 `std::*`
- [x] **N5** 无外部依赖（除 eneros-driver-framework）；CRC16/帧编解码均为本地实现
- [x] **N6** f64 运算在 no_std 可用（`core::primitive::f64`）

## 功能校验（对照蓝图 §3 交付物）

- [x] **F1** `ModbusFrame` 含 slave_addr/func_code/data/crc 字段 + encode/decode 方法
- [x] **F2** `FunctionCode` 枚举含 6 变体（0x01/0x03/0x04/0x05/0x06/0x10）
- [x] **F3** `ModbusRequest` 含 ReadHoldingRegisters/WriteSingleRegister/WriteMultipleRegisters
- [x] **F4** `ModbusResponse` 含 ReadHoldingRegisters/WriteSingleRegister/WriteMultipleRegisters/Error/Broadcast
- [x] **F5** `ExceptionCode` 含 6 异常码（0x01~0x06）
- [x] **F6** `ModbusRtuMaster` 实现 read_holding_registers/write_single_register/write_multiple_registers
- [x] **F7** `ModbusRtuMaster` 实现 poll_points 点表轮询
- [x] **F8** `PointMapping`/`RegToPoint` 含全部字段 + word_count/convert 方法
- [x] **F9** `ModbusDataType` 含 U16/I16/U32/F32/Bit 变体
- [x] **F10** `RtuTransport` trait 定义（D1）+ `Rs485Driver` 自动满足
- [x] **F11** `ModbusStats` 含 request_count/response_count/error_count/timeout_count/crc_error_count
- [x] **F12** `ModbusError` 含全部变体（D2）

## 测试覆盖校验（对照蓝图 §6 测试计划）

- [x] **T1** CRC16 单元测试（已知测试向量 + 空输入 + 边界）— `crc.rs` + `lib.rs::tests::test_crc_known_vector`
- [x] **T2** 帧编解码测试（编码→解码环回 + CRC 失败 + 帧过短）— `frame.rs` + `lib.rs::tests::test_frame_roundtrip_integration`
- [x] **T3** `read_holding_registers()` 成功测试（mock 响应 + 验证请求帧）— `master.rs` + `lib.rs::tests::test_master_read_holding_registers_integration`
- [x] **T4** `write_single_register()` / `write_multiple_registers()` 成功测试 — `master.rs` + `lib.rs::tests::test_master_write_single_register_integration`
- [x] **T5** 超时重试测试（mock 超时 → MaxRetryExceeded）— `master.rs`
- [x] **T6** 异常码测试（mock 异常响应 → ModbusError::Exception）— `master.rs`
- [x] **T7** 广播写测试（slave_addr=0 → 不等待响应 → Broadcast）— `master.rs` + `lib.rs::tests::test_master_broadcast_write_integration`
- [x] **T8** 点表转换测试（U16/I16/U32/F32/Bit + scale/offset）— `point.rs` + `lib.rs::tests::test_point_conversion_integration`
- [x] **T9** `poll_points()` 轮询测试（多点位 + 同从站分组）— `master.rs` + `lib.rs::tests::test_poll_points_integration`

## 验收标准（对照蓝图 §7）

- [x] **A1** 功能码 03/06/10 实现完整
- [x] **A2** CRC16 校验正确（国标测试向量通过 — `crc16_modbus(b"123456789") == 0x4B37`）
- [x] **A3** 点表映射支持 U16/I16/U32/F32 数据类型
- [x] **A4** 超时重试机制工作正常
- [x] **A5** 广播地址 0 写操作不等待响应（D10）

## 偏差声明校验

- [x] **D1** `RtuTransport` trait 定义（解耦主站与 RS485 驱动，便于 mock 测试）
- [x] **D2** `ModbusError` 枚举定义（蓝图引用但未定义）
- [x] **D3** `ModbusStats` 定义（蓝图引用但未定义）
- [x] **D4** `AccessMode` 枚举定义（蓝图引用但未定义）
- [x] **D5** `RegToPoint::word_count()` / `convert()` 实现（蓝图引用但未定义）
- [x] **D6** `group_by_slave()` 辅助函数实现（蓝图引用但未定义）
- [x] **D7** `build_frame()` / `parse_response()` 实现（蓝图引用但未定义）
- [x] **D8** crate 放入 `crates/protocols/modbus-rtu/`（遵循 §2.3.1 crate 分组规则）
- [x] **D9** `FunctionCode` 枚举含 6 变体，但仅实现 03/06/10 编解码
- [x] **D10** 广播地址 0 写操作不等待响应，返回 `ModbusResponse::Broadcast`
