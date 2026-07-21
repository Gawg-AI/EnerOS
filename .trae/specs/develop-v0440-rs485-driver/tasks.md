# Tasks

## Task 1: 框架扩展 — DriverError 增加 Timeout 变体
- [x] SubTask 1.1: 修改 `crates/drivers/framework/src/lib.rs`，向 `DriverError` 枚举增加 `Timeout` 变体
- [x] SubTask 1.2: 在 `Display` 实现中增加 `Timeout => "operation timed out"` 分支
- [x] SubTask 1.3: 在 `tests` 模块中增加 `Timeout` 变体的 Display 与 Eq 测试

## Task 2: workspace 版本号与 members 同步
- [x] SubTask 2.1: 修改根 `Cargo.toml`，`version` 从 `0.43.0` → `0.44.0`
- [x] SubTask 2.2: 向 `members` 数组增加 `"crates/drivers/rs485"`

## Task 3: 创建 eneros-rs485 crate 骨架
- [x] SubTask 3.1: 创建 `crates/drivers/rs485/Cargo.toml`（workspace 继承，依赖 `eneros-driver-framework` + `eneros-hal`）
- [x] SubTask 3.2: 创建 `crates/drivers/rs485/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明）
- [x] SubTask 3.3: 创建 `crates/drivers/rs485/src/config.rs`（`UartPort`/`StopBits`/`Parity`/`GpioPin` 类型 + `Rs485Config` 结构 + `Default` 实现，D2）

## Task 4: 实现 UartHw trait 抽象（D1）
- [x] SubTask 4.1: 创建 `crates/drivers/rs485/src/uart_hw.rs`，定义 `UartHw` trait（configure/enable_rx_irq/disable_rx_irq/read_byte/write_bytes/wait_tx_done/rx_irq_id）

## Task 5: 实现环形缓冲 RingBuffer（D4）
- [x] SubTask 5.1: 创建 `crates/drivers/rs485/src/ring.rs`，实现 `RingBuffer<T, const N: usize>` 泛型环形缓冲（push/pop/len/is_empty/is_full/clear）
- [x] SubTask 5.2: 为 RingBuffer 编写单元测试（空/满/推入弹出/环绕）

## Task 6: 实现 Rs485Driver + Rs485Stats
- [x] SubTask 6.1: 创建 `crates/drivers/rs485/src/driver.rs`，定义 `Rs485Stats`（tx_count/rx_count/rx_error_count/last_rx_error + Default）
- [x] SubTask 6.2: 实现 `Rs485Driver` 结构（id/config/state/uart/gpio/de_re_pin/rx_buffer/stats/irq_rx 字段）
- [x] SubTask 6.3: 为 `Rs485Driver` 实现 `DeviceDriver` trait（init/start/stop/deinit/handle_irq/health_check，遵循蓝图 §4.2 逻辑）
- [x] SubTask 6.4: 实现 `Rs485Driver::send(&mut self, data: &[u8])` 发送方法（DE 切换 → write_bytes → wait_tx_done → DE 恢复，D8 时序由 UartHw 处理）
- [x] SubTask 6.5: 实现 `Rs485Driver::recv(&mut self, timeout_ms: u32, now_ns: u64)` 接收方法（帧间隔检测 + 超时，D3 注入 now_ns）

## Task 7: 实现 MockUartHw 测试桩
- [x] SubTask 7.1: 创建 `crates/drivers/rs485/src/mock.rs`，实现 `MockUartHw`（预填充 rx 缓冲、记录 written、可配置 tx 超时）
- [x] SubTask 7.2: 为 `MockUartHw` 实现 `UartHw` trait

## Task 8: 集成测试 — 收发环回与状态转换
- [x] SubTask 8.1: 在 `lib.rs` 的 `#[cfg(test)] mod tests` 中编写配置/状态转换测试（init→Ready, start→Running, stop→Stopped, deinit→Dead）
- [x] SubTask 8.2: 编写 `send()` 测试（成功发送 + DE 切换 + tx_count 递增）
- [x] SubTask 8.3: 编写 `send()` 超时测试（`wait_tx_done` 返回 Timeout → DE 恢复 → 返回 Err(Timeout)）
- [x] SubTask 8.4: 编写 `recv()` 测试（rx_buffer 预填充 → 帧间隔超时 → 返回帧数据）
- [x] SubTask 8.5: 编写 `recv()` 超时测试（rx_buffer 空 → 超过 deadline → 返回 Err(Timeout)）
- [x] SubTask 8.6: 编写 `handle_irq()` 测试（irq_id 匹配 → rx_buffer 填充 → irq_rx=true）
- [x] SubTask 8.7: 编写 `health_check()` 测试（rx_error_count 阈值 0/11/101 三档）

## Task 9: 设计文档
- [x] SubTask 9.1: 创建 `docs/drivers/rs485-driver-design.md`，包含：版本目标、前置依赖、交付物清单、详细设计（含偏差声明 D1~D10）、收发流程、测试计划、验收标准、风险、多角度要求

## Task 10: 构建校验（§2.4 清单）
- [x] SubTask 10.1: `cargo metadata --format-version 1 > /dev/null`（workspace 成员路径正确）
- [x] SubTask 10.2: `cargo test -p eneros-rs485`（单元 + 集成测试通过）
- [x] SubTask 10.3: `cargo test -p eneros-driver-framework`（框架 Timeout 变体不破坏既有测试）
- [x] SubTask 10.4: `cargo build -p eneros-rs485 --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（交叉编译通过）
- [x] SubTask 10.5: `cargo fmt --all -- --check`（格式检查）
- [x] SubTask 10.6: `cargo clippy -p eneros-rs485 -p eneros-driver-framework --all-targets -- -D warnings`（lint 无 warning）
- [x] SubTask 10.7: 确认 `.gitignore` 覆盖新产生的文件类型（无新增需忽略类型）

# Task Dependencies

- Task 1（框架扩展）→ 无前置，可独立执行
- Task 2（workspace 同步）→ 无前置，可独立执行
- Task 3（crate 骨架）→ 依赖 Task 2（members 需先添加）
- Task 4（UartHw trait）→ 依赖 Task 3
- Task 5（RingBuffer）→ 依赖 Task 3
- Task 6（Rs485Driver）→ 依赖 Task 1（Timeout 变体）、Task 4（UartHw）、Task 5（RingBuffer）
- Task 7（MockUartHw）→ 依赖 Task 4（UartHw trait）
- Task 8（集成测试）→ 依赖 Task 6（Rs485Driver）、Task 7（MockUartHw）
- Task 9（设计文档）→ 可与 Task 4~8 并行
- Task 10（构建校验）→ 依赖 Task 1~8 全部完成

# 可并行执行

- Task 1 + Task 2 可并行
- Task 4 + Task 5 可并行（均依赖 Task 3）
- Task 7 + Task 9 可并行（均依赖 Task 4）
