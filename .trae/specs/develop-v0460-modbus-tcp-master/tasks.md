# Tasks — v0.46.0 Modbus TCP 主站

## Task 1: workspace 版本号与 members 同步
- [x] SubTask 1.1: 修改根 `Cargo.toml`，`version` 从 `0.45.0` → `0.46.0`
- [x] SubTask 1.2: 向 `members` 数组增加 `"crates/protocols/modbus-tcp"`

## Task 2: 创建 eneros-modbus-tcp crate 骨架
- [x] SubTask 2.1: 创建 `crates/protocols/modbus-tcp/Cargo.toml`（workspace 继承，依赖 `eneros-modbus-rtu` 以复用应用层类型）
- [x] SubTask 2.2: 创建 `crates/protocols/modbus-tcp/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + re-export）
- [x] SubTask 2.3: 创建 `crates/protocols/modbus-tcp/src/error.rs`（`ModbusTcpError` 枚举 + Display + From<ModbusError>，D2）

## Task 3: 实现 MBAP 头结构
- [x] SubTask 3.1: 创建 `crates/protocols/modbus-tcp/src/mbap.rs`，定义 `MbapHeader` 结构（transaction_id/protocol_id/length/unit_id）
- [x] SubTask 3.2: 实现 `MbapHeader::new(transaction_id, unit_id, data_len)`（protocol_id=0，length=data_len+1）
- [x] SubTask 3.3: 实现 `MbapHeader::encode(&self) -> [u8; 7]`（大端编码）
- [x] SubTask 3.4: 实现 `MbapHeader::decode(buf: &[u8]) -> Result<Self, ModbusTcpError>`（长度校验 + 协议 ID 校验）
- [x] SubTask 3.5: 编写 MBAP 编解码单元测试（环回 + 帧过短 + 协议 ID 非 0）

## Task 4: 实现 TcpDevice 设备描述
- [x] SubTask 4.1: 创建 `crates/protocols/modbus-tcp/src/device.rs`，定义 `TcpDevice` 结构（ip: [u8;4], port: u16, unit_id: u8, timeout_ms: u32，D4）
- [x] SubTask 4.2: 实现 `TcpDevice::new(ip, port, unit_id)` 构造方法（默认 timeout_ms=3000）
- [x] SubTask 4.3: 实现 `TcpDevice::default_port()` 返回 502
- [x] SubTask 4.4: 编写 TcpDevice 单元测试

## Task 5: 实现 TcpTransport trait + TcpStats（D1/D5/D6）
- [x] SubTask 5.1: 创建 `crates/protocols/modbus-tcp/src/transport.rs`，定义 `TcpTransport` trait（`send`/`recv`/`connect` 三方法，D1/D6）
- [x] SubTask 5.2: 定义 `TcpStats` 结构（request_count/response_count/error_count/timeout_count/reconnect_count，D5）
- [x] SubTask 5.3: 编写 TcpStats 单元测试（Default + 字段访问）

## Task 6: 实现 ModbusTcpMaster 主站
- [x] SubTask 6.1: 创建 `crates/protocols/modbus-tcp/src/master.rs`，定义 `ModbusTcpMaster` 结构（transport: &mut dyn TcpTransport, next_txn_id: u16, stats: TcpStats, retry_count: u8）
- [x] SubTask 6.2: 实现 `next_txn_id()` 事务 ID 自增（u16 循环复用，D7 串行）
- [x] SubTask 6.3: 实现 `build_pdu(req: &ModbusRequest) -> Vec<u8>`（复用 v0.45.0 ModbusRequest::encode_data()，D3）
- [x] SubTask 6.4: 实现 `build_frame(txn_id, unit_id, pdu) -> Vec<u8>`（MBAP + PDU 拼装）
- [x] SubTask 6.5: 实现 `parse_response(req, resp_bytes) -> Result<ModbusResponse, ModbusTcpError>`（MBAP 解码 + 事务 ID 校验 + PDU 解析，复用 v0.45.0 逻辑）
- [x] SubTask 6.6: 实现 `send_request_with_retry(device, req) -> Result<ModbusResponse, ModbusTcpError>`（connect + send + recv + 重试）
- [x] SubTask 6.7: 实现 `read_holding_registers(device, start_addr, quantity)` 公开方法
- [x] SubTask 6.8: 实现 `write_single_register(device, reg_addr, value)` 公开方法
- [x] SubTask 6.9: 实现 `write_multiple_registers(device, start_addr, values)` 公开方法
- [x] SubTask 6.10: 实现 `poll_devices(devices, mapping)` 多设备批量轮询（D7 串行）

## Task 7: 实现 MockTcpTransport 测试桩
- [x] SubTask 7.1: 创建 `crates/protocols/modbus-tcp/src/mock.rs`（`#[cfg(test)]`），实现 `MockTcpTransport`（预置响应队列 + 记录发送帧 + 记录 connect 调用）
- [x] SubTask 7.2: 为 `MockTcpTransport` 实现 `TcpTransport` trait

## Task 8: 集成测试 — 主站收发与多设备轮询
- [x] SubTask 8.1: 在 `lib.rs` 的 `#[cfg(test)] mod tests` 中编写 MBAP 编解码测试（已知向量 + 环回）
- [x] SubTask 8.2: 编写 `read_holding_registers()` 成功测试（mock 响应 + 验证发送帧 MBAP+PDU + 事务 ID）
- [x] SubTask 8.3: 编写 `write_single_register()` / `write_multiple_registers()` 成功测试
- [x] SubTask 8.4: 编写事务 ID 不匹配测试（mock 错误 txn_id → TransactionMismatch）
- [x] SubTask 8.5: 编写超时重试测试（mock 超时 → MaxRetryExceeded）
- [x] SubTask 8.6: 编写异常码测试（mock 异常响应 → ModbusTcpError::Modbus(ModbusError::Exception)）
- [x] SubTask 8.7: 编写 `poll_devices()` 多设备轮询测试（3 设备 + 点表映射 + 串行轮询）
- [x] SubTask 8.8: 编写事务 ID 回绕测试（u16 溢出后从 0 开始）

## Task 9: 设计文档
- [x] SubTask 9.1: 创建 `docs/protocols/modbus-tcp-master-design.md`，包含：版本目标、前置依赖、交付物清单、详细设计（含偏差声明 D1~D8）、MBAP 结构/收发流程、测试计划、验收标准、风险、多角度要求

## Task 10: 构建校验（§2.4 清单）
- [x] SubTask 10.1: `cargo metadata --format-version 1 > /dev/null`（workspace 成员路径正确）
- [x] SubTask 10.2: `cargo test -p eneros-modbus-tcp`（单元 + 集成测试通过 — 51 测试全绿）
- [x] SubTask 10.3: `cargo build -p eneros-modbus-tcp --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（交叉编译通过）
- [x] SubTask 10.4: `cargo fmt --all -- --check`（格式检查通过）
- [x] SubTask 10.5: `cargo clippy -p eneros-modbus-tcp --all-targets -- -D warnings`（lint 无 warning）
- [x] SubTask 10.6: 确认 `.gitignore` 覆盖新产生的文件类型（无新增需忽略类型）

# Task Dependencies

- Task 1（workspace 同步）→ 无前置，可独立执行
- Task 2（crate 骨架）→ 依赖 Task 1（members 需先添加）
- Task 3（MBAP）→ 依赖 Task 2（error.rs 的 ModbusTcpError）
- Task 4（TcpDevice）→ 依赖 Task 2（error.rs）
- Task 5（Transport trait）→ 依赖 Task 2（error.rs）+ Task 4（TcpDevice）
- Task 6（主站）→ 依赖 Task 3（MBAP）+ Task 4（TcpDevice）+ Task 5（Transport）+ v0.45.0 应用层类型
- Task 7（mock）→ 依赖 Task 5（TcpTransport trait）+ Task 6（主站）
- Task 8（集成测试）→ 依赖 Task 6（主站）+ Task 7（mock）
- Task 9（设计文档）→ 可与 Task 3~7 并行
- Task 10（构建校验）→ 依赖 Task 1~8 全部完成

# 可并行执行

- Task 3（MBAP）+ Task 4（TcpDevice）可并行（均依赖 Task 2）
- Task 9（设计文档）可与 Task 3~7 并行
