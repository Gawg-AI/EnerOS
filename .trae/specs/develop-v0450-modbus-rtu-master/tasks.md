# Tasks — v0.45.0 Modbus RTU 主站

## Task 1: workspace 版本号与 members 同步
- [x] SubTask 1.1: 修改根 `Cargo.toml`，`version` 从 `0.44.0` → `0.45.0`
- [x] SubTask 1.2: 向 `members` 数组增加 `"crates/protocols/modbus-rtu"`

## Task 2: 创建 eneros-modbus-rtu crate 骨架
- [x] SubTask 2.1: 创建 `crates/protocols/modbus-rtu/Cargo.toml`（workspace 继承，依赖 `eneros-driver-framework`，不直接依赖 `eneros-rs485` 以解耦）
- [x] SubTask 2.2: 创建 `crates/protocols/modbus-rtu/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + re-export）
- [x] SubTask 2.3: 创建 `crates/protocols/modbus-rtu/src/error.rs`（`ModbusError` 枚举 + Display 实现，D2）

## Task 3: 实现 CRC-16/MODBUS 算法
- [x] SubTask 3.1: 创建 `crates/protocols/modbus-rtu/src/crc.rs`，实现 `pub fn crc16_modbus(data: &[u8]) -> u16`（多项式 0xA001，初始值 0xFFFF）
- [x] SubTask 3.2: 编写 CRC16 单元测试（已知测试向量 + 空输入 + 全 0xFF 输入）

## Task 4: 实现 ModbusFrame 帧结构
- [x] SubTask 4.1: 创建 `crates/protocols/modbus-rtu/src/frame.rs`，定义 `ModbusFrame` 结构（slave_addr/func_code/data/crc）
- [x] SubTask 4.2: 实现 `encode(&self) -> Vec<u8>`（SlaveAddr + FuncCode + Data + CRC16 LE）
- [x] SubTask 4.3: 实现 `decode(buf: &[u8]) -> Result<Self, ModbusError>`（长度校验 + CRC 校验）
- [x] SubTask 4.4: 编写帧编解码单元测试（编码→解码环回、CRC 失败、帧过短）

## Task 5: 实现功能码/请求/响应类型
- [x] SubTask 5.1: 创建 `crates/protocols/modbus-rtu/src/request.rs`，定义 `FunctionCode` 枚举（6 变体，D9）
- [x] SubTask 5.2: 定义 `ModbusRequest` 枚举（ReadHoldingRegisters/WriteSingleRegister/WriteMultipleRegisters）
- [x] SubTask 5.3: 定义 `ModbusResponse` 枚举（含 Broadcast 变体，D10）+ `ExceptionCode` 枚举
- [x] SubTask 5.4: 编写请求/响应类型测试

## Task 6: 实现点表映射
- [x] SubTask 6.1: 创建 `crates/protocols/modbus-rtu/src/point.rs`，定义 `ModbusDataType`/`AccessMode`/`RegToPoint`/`PointMapping`
- [x] SubTask 6.2: 实现 `RegToPoint::word_count()`（D5：U16/I16/Bit=1, U32/F32=2）
- [x] SubTask 6.3: 实现 `RegToPoint::convert(&[u16]) -> Result<f64, ModbusError>`（D5：按 data_type 解码 + scale/offset）
- [x] SubTask 6.4: 实现 `group_by_slave()` 辅助函数（D6）
- [x] SubTask 6.5: 编写点表转换单元测试（U16/I16/U32/F32/Bit 各类型）

## Task 7: 实现 ModbusRtuMaster 主站
- [x] SubTask 7.1: 创建 `crates/protocols/modbus-rtu/src/master.rs`，定义 `RtuTransport` trait（D1）+ `ModbusStats`（D3）+ `ModbusRtuMaster`
- [x] SubTask 7.2: 实现 `build_frame(slave_addr, &request) -> Vec<u8>`（D7：请求帧编码 + CRC）
- [x] SubTask 7.3: 实现 `parse_response(&request, &frame) -> Result<ModbusResponse, ModbusError>`（D7：响应帧解码 + 异常码判断）
- [x] SubTask 7.4: 实现 `send_request_with_retry()`（含广播地址 0 处理，D10 + 超时重试）
- [x] SubTask 7.5: 实现 `read_holding_registers()` / `write_single_register()` / `write_multiple_registers()` 公开方法
- [x] SubTask 7.6: 实现 `poll_points(&PointMapping)` 轮询方法

## Task 8: 实现 MockRtuTransport 测试桩
- [x] SubTask 8.1: 创建 `crates/protocols/modbus-rtu/src/mock.rs`，实现 `MockRtuTransport`（预填充响应帧队列 + 记录发送帧 + 可配置超时）
- [x] SubTask 8.2: 为 `MockRtuTransport` 实现 `RtuTransport` trait

## Task 9: 集成测试 — 主站收发与点表轮询
- [x] SubTask 9.1: 在 `lib.rs` 的 `#[cfg(test)] mod tests` 中编写 CRC16 测试（已知向量）
- [x] SubTask 9.2: 编写帧编解码测试（编码→解码环回 + CRC 失败 + 帧过短）
- [x] SubTask 9.3: 编写 `read_holding_registers()` 成功测试（mock 响应 + 验证请求帧 + 返回寄存器值）
- [x] SubTask 9.4: 编写 `write_single_register()` / `write_multiple_registers()` 成功测试
- [x] SubTask 9.5: 编写超时重试测试（mock 超时 → MaxRetryExceeded）
- [x] SubTask 9.6: 编写异常码测试（mock 异常响应 → ModbusError::Exception）
- [x] SubTask 9.7: 编写广播写测试（slave_addr=0 → 不等待响应 → Broadcast）
- [x] SubTask 9.8: 编写点表转换测试（U16/I16/U32/F32/Bit + scale/offset）
- [x] SubTask 9.9: 编写 `poll_points()` 轮询测试（多点位 + 同从站分组）

## Task 10: 设计文档
- [x] SubTask 10.1: 创建 `docs/protocols/modbus-rtu-master-design.md`，包含：版本目标、前置依赖、交付物清单、详细设计（含偏差声明 D1~D10）、帧结构/收发流程、测试计划、验收标准、风险、多角度要求

## Task 11: 构建校验（§2.4 清单）
- [x] SubTask 11.1: `cargo metadata --format-version 1 > /dev/null`（workspace 成员路径正确）
- [x] SubTask 11.2: `cargo test -p eneros-modbus-rtu`（单元 + 集成测试通过）
- [x] SubTask 11.3: `cargo build -p eneros-modbus-rtu --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（交叉编译通过）
- [x] SubTask 11.4: `cargo fmt --all -- --check`（格式检查）
- [x] SubTask 11.5: `cargo clippy -p eneros-modbus-rtu --all-targets -- -D warnings`（lint 无 warning）
- [x] SubTask 11.6: 确认 `.gitignore` 覆盖新产生的文件类型（无新增需忽略类型）

# Task Dependencies

- Task 1（workspace 同步）→ 无前置，可独立执行
- Task 2（crate 骨架）→ 依赖 Task 1（members 需先添加）
- Task 3（CRC16）→ 依赖 Task 2（error.rs 的 ModbusError）
- Task 4（帧结构）→ 依赖 Task 3（CRC16）
- Task 5（功能码/请求/响应）→ 依赖 Task 2（error.rs）
- Task 6（点表映射）→ 依赖 Task 2（error.rs）+ Task 5（ModbusDataType）
- Task 7（主站）→ 依赖 Task 4（帧）+ Task 5（请求/响应）+ Task 6（点表）
- Task 8（mock）→ 依赖 Task 7（RtuTransport trait）
- Task 9（集成测试）→ 依赖 Task 7（主站）+ Task 8（mock）
- Task 10（设计文档）→ 可与 Task 3~9 并行
- Task 11（构建校验）→ 依赖 Task 1~9 全部完成

# 可并行执行

- Task 3（CRC16）+ Task 5（功能码）可并行（均依赖 Task 2）
- Task 6（点表）+ Task 8 可在 Task 7 完成后并行（Task 8 依赖 Task 7 的 RtuTransport trait）
- Task 10（设计文档）可与 Task 3~9 并行
