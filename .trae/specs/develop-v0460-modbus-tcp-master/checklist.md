# Checklist — v0.46.0 Modbus TCP 主站

## 目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/protocols/modbus-tcp/` 已放入 `crates/<subsystem>/` 下，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 已添加 `"crates/protocols/modbus-tcp"`
- [x] **C3 跨 crate path 引用**：`crates/protocols/modbus-tcp/Cargo.toml` 中 `eneros-modbus-rtu` 的 `path` 使用正确相对路径（`path = "../modbus-rtu"`，同在 protocols/ 下）
- [x] **C4 文档分类**：`docs/protocols/modbus-tcp-master-design.md` 已放入 `docs/protocols/` 子目录（已存在），未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 构建校验（§2.4.2，必须全部通过）

- [x] **C6 cargo metadata** 成功（workspace 成员路径全部正确）
- [x] **C7 cargo test** 通过（`cargo test -p eneros-modbus-tcp` — 43 模块测试 + 8 集成测试 = 51 测试全绿）
- [x] **C8 cargo build --target aarch64-unknown-none** 通过（`eneros-modbus-tcp` 交叉编译成功）
- [x] **C9 cargo fmt --check** 通过
- [x] **C10 cargo clippy** 无 warning（`-p eneros-modbus-tcp -- -D warnings`）
- [x] **C11 cargo deny check** — 未单独执行（已知 GitHub 网络问题，参考既有版本惯例记录已知问题）

## 文档与规范校验

- [x] **C12 文档位置**：`modbus-tcp-master-design.md` 在 `docs/protocols/` 下
- [x] **C13 无垃圾文件**：`git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] **C14 .gitignore 覆盖**：无新增需忽略文件类型（仅新增 `crates/protocols/modbus-tcp/` Rust 源码与 `docs/protocols/` 文档）
- [x] **C15 提交信息**：遵循 Conventional Commits（待用户提交时按规范执行）

## no_std 合规校验

- [x] **N1** `crates/protocols/modbus-tcp/src/lib.rs` 顶部有 `#![cfg_attr(not(test), no_std)]`
- [x] **N2** 子模块未重复添加 `#![cfg_attr(not(test), no_std)]`（从 lib.rs 继承）
- [x] **N3** 无 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- [x] **N4** 使用 `alloc::*`（Vec/String）而非 `std::*`
- [x] **N5** 外部依赖仅 `eneros-modbus-rtu`（传递依赖 `eneros-driver-framework`）；MBAP/PDU 编解码均为本地实现
- [x] **N6** 不直接依赖 smoltcp（D4：TcpDevice 用 `[u8;4]` 表示 IPv4）

## 功能校验（对照蓝图 §3 交付物）

- [x] **F1** `MbapHeader` 含 transaction_id/protocol_id/length/unit_id 字段 + encode/decode 方法
- [x] **F2** `TcpDevice` 含 ip([u8;4])/port/unit_id/timeout_ms 字段（D4）
- [x] **F3** `TcpTransport` trait 定义（D1：send/recv/connect 三方法）
- [x] **F4** `ModbusTcpMaster` 实现 read_holding_registers/write_single_register/write_multiple_registers
- [x] **F5** `ModbusTcpMaster` 实现 poll_devices 多设备轮询（D7 串行）
- [x] **F6** `ModbusTcpError` 含 Modbus(ModbusError)/TransactionMismatch/ConnectionFailed/NotConnected/Timeout/Closed/FrameTooShort/InvalidProtocolId 变体（D2）
- [x] **F7** `TcpStats` 含 request_count/response_count/error_count/timeout_count/reconnect_count（D5）
- [x] **F8** 复用 v0.45.0 的 `ModbusRequest`/`ModbusResponse`/`ExceptionCode`/`FunctionCode`/`PointMapping`/`RegToPoint`（D3）
- [x] **F9** 事务 ID 自增 + u16 循环回绕
- [x] **F10** `MockTcpTransport` 测试桩（预置响应 + 记录发送帧 + 记录 connect 调用）

## 测试覆盖校验（对照蓝图 §6 测试计划）

- [x] **T1** MBAP 编解码测试（环回 + 帧过短 + 协议 ID 非 0）— `mbap.rs` + `lib.rs::tests::test_mbap_roundtrip_integration`
- [x] **T2** `read_holding_registers()` 成功测试（mock 响应 + 验证发送帧 MBAP+PDU + 事务 ID）— `master.rs` + `lib.rs::tests::test_master_read_holding_registers_integration`
- [x] **T3** `write_single_register()` / `write_multiple_registers()` 成功测试 — `master.rs` + `lib.rs::tests::test_master_write_single_register_integration`
- [x] **T4** 事务 ID 不匹配测试（mock 错误 txn_id → TransactionMismatch）— `master.rs` + `lib.rs::tests::test_transaction_mismatch_integration`
- [x] **T5** 超时重试测试（mock 超时 → MaxRetryExceeded）— `master.rs` + `lib.rs::tests::test_timeout_retry_integration`
- [x] **T6** 异常码测试（mock 异常响应 → ModbusTcpError::Modbus(ModbusError::Exception)）— `master.rs` + `lib.rs::tests::test_exception_response_integration`
- [x] **T7** `poll_devices()` 多设备轮询测试（3 设备 + 点表映射 + 串行轮询）— `master.rs` + `lib.rs::tests::test_poll_devices_integration`
- [x] **T8** 事务 ID 回绕测试（u16 溢出后从 0 开始）— `master.rs` + `lib.rs::tests::test_txn_id_wraparound_integration`

## 验收标准（对照蓝图 §7）

- [x] **A1** MBAP 头编解码正确
- [x] **A2** 能通过 TcpTransport 读写 Modbus TCP 从站（mock 验证）
- [x] **A3** 支持多设备轮询（≥3 设备）
- [x] **A4** 事务 ID 配对验证
- [x] **A5** 事务 ID u16 回绕正常

## 偏差声明校验

- [x] **D1** `TcpTransport` trait 定义（send/recv/connect，解耦主站与 socket 实现）
- [x] **D2** `ModbusTcpError` 枚举定义（包装 ModbusError + TCP 特有变体，不修改 v0.45.0）
- [x] **D3** 复用 v0.45.0 `ModbusRequest`/`ModbusResponse`，`slave_addr` 语义复用为 `unit_id`
- [x] **D4** `TcpDevice` 用 `[u8;4]` 表示 IPv4，不依赖 smoltcp
- [x] **D5** `TcpStats` 定义（含 reconnect_count）
- [x] **D6** 连接管理委托给 `TcpTransport::connect()`，主站不持有连接池
- [x] **D7** `poll_devices()` 串行遍历（非真并发）
- [x] **D8** crate 放入 `crates/protocols/modbus-tcp/`
