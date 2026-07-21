# Checklist — v0.44.0 RS485 串口驱动

## 目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/drivers/rs485/` 已放入 `crates/<subsystem>/` 下，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 已添加 `"crates/drivers/rs485"`
- [x] **C3 跨 crate path 引用**：`crates/drivers/rs485/Cargo.toml` 中 `eneros-driver-framework` 与 `eneros-hal` 的 `path` 使用正确相对路径（同在 `crates/drivers/` 下 → `path = "../framework"`、跨子系统 `path = "../../hal/hal"`）
- [x] **C4 文档分类**：`docs/drivers/rs485-driver-design.md` 已放入 `docs/drivers/` 子目录，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 构建校验（§2.4.2，必须全部通过）

- [x] **C6 cargo metadata** 成功（workspace 成员路径全部正确）
- [x] **C7 cargo test** 通过（`cargo test -p eneros-rs485` 37 测试 + `cargo test -p eneros-driver-framework` 48 测试全绿）
- [x] **C8 cargo build --target aarch64-unknown-none** 通过（`eneros-rs485` 交叉编译成功）
- [x] **C9 cargo fmt --check** 通过
- [x] **C10 cargo clippy** 无 warning（`-p eneros-rs485 -p eneros-driver-framework -- -D warnings`）
- [x] **C11 cargo deny check** 已知问题：GitHub 网络无法访问 advisory-db（`Recv failure: Connection was reset`），与项目记忆中记录的已知问题一致，非代码问题

## 文档与规范校验

- [x] **C12 文档位置**：`rs485-driver-design.md` 在 `docs/drivers/` 下
- [x] **C13 无垃圾文件**：`git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] **C14 .gitignore 覆盖**：`.gitignore` 已覆盖 `target/`/`*.elf`/`*.bin`/`*.dtb`/`.idea/`/`.vscode/`/`.trae/cache/` 等，无新增需忽略文件类型
- [x] **C15 提交信息**：将遵循 Conventional Commits（待用户提交时执行）

## no_std 合规校验

- [x] **N1** `crates/drivers/rs485/src/lib.rs` 顶部有 `#![cfg_attr(not(test), no_std)]`
- [x] **N2** 子模块（config.rs/uart_hw.rs/ring.rs/driver.rs/mock.rs）未重复添加 `#![cfg_attr(not(test), no_std)]`（从 lib.rs 继承）
- [x] **N3** 无 `use std::*` / `panic!` / `todo!` / `unimplemented!`（clippy + 交叉编译验证）
- [x] **N4** 使用 `alloc::*`（Vec/String/Box）而非 `std::*`
- [x] **N5** 使用 `core::sync::atomic::AtomicBool`（D6 偏差）
- [x] **N6** 环形缓冲使用 const generics（`RingBuffer<T, const N: usize>`），无外部依赖（D4 偏差）

## 功能校验（对照蓝图 §3 交付物）

- [x] **F1** `Rs485Driver` 实现 `DeviceDriver` trait（id/name/driver_type/state/init/start/stop/deinit/handle_irq/health_check）
- [x] **F2** `Rs485Config` 含全部蓝图字段：port/baud_rate/data_bits/stop_bits/parity/local_addr/response_timeout_ms/frame_gap_ms/de_re_pin/pre_send_delay_us/post_send_delay_us
- [x] **F3** `Rs485Config::default()` 默认值：Uart0/9600/8/One/None/1/1000ms/4ms/None/100μs/100μs
- [x] **F4** `send()` 方法：DE 拉高 → write_bytes → wait_tx_done → DE 拉低 → tx_count++
- [x] **F5** `send()` 超时：wait_tx_done 返回 Timeout → DE 恢复 → 返回 Err(Timeout)
- [x] **F6** `recv(timeout_ms)` 方法：帧间隔检测 + 超时返回 Timeout（D3 修正：通过 `UartHw::now_ns()` 获取时间，非参数注入）
- [x] **F7** `handle_irq()`：irq_id 匹配 rx_irq_id → 读字节入 rx_buffer → irq_rx=true
- [x] **F8** `health_check()`：rx_error_count 阈值 0/11/101 对应 Healthy/Degraded/Unhealthy
- [x] **F9** `Rs485Stats` 含 tx_count/rx_count/rx_error_count/last_rx_error
- [x] **F10** `DriverError::Timeout` 变体已添加到框架（D5 偏差）+ Display + Clone 实现

## 测试覆盖校验（对照蓝图 §6 测试计划）

- [x] **T1** 配置/状态转换测试（init→Ready, start→Running, stop→Stopped, deinit→Dead）
- [x] **T2** `send()` 成功测试（数据写入 + DE 切换 + tx_count 递增）
- [x] **T3** `send()` 超时测试（wait_tx_done 超时 → DE 恢复 → Err(Timeout)）
- [x] **T4** `recv()` 成功测试（rx_buffer 预填充 → 帧间隔超时 → 返回帧）
- [x] **T5** `recv()` 超时测试（空缓冲 → 超过 deadline → Err(Timeout)）
- [x] **T6** `handle_irq()` 测试（IRQ 匹配 → rx_buffer 填充 → irq_rx=true）
- [x] **T7** `health_check()` 三档测试（0/11/101 → Healthy/Degraded/Unhealthy）
- [x] **T8** RingBuffer 单元测试（空/满/推入弹出/环绕/零容量）
- [x] **T9** `UartHw` trait 可被 `MockUartHw` 实现（trait object 兼容 + DeviceDriver trait object 兼容）

## 验收标准（对照蓝图 §7）

- [x] **A1** RS485 驱动实现 DeviceDriver trait
- [x] **A2** 支持 9600/19200/38400/115200 波特率（配置参数化）
- [x] **A3** DE/RE 方向控制正确（发送时 DE=1，接收时 DE=0）
- [x] **A4** 收发逻辑完整（send + recv 路径）
- [x] **A5** 帧间隔检测逻辑实现（frame_gap_ms 静默判定）

## 偏差声明校验

- [x] **D1** `UartHw` trait 已定义（HAL 无 HalUart，本地抽象，含 `Send + Sync` 超级 trait）
- [x] **D2** `UartPort`/`StopBits`/`Parity`/`GpioPin` 在 config.rs 内定义
- [x] **D3 修正** `recv(timeout_ms)` 通过 `UartHw::now_ns()` 获取时间（原设计为参数注入，实现时改为 trait 方法，更简洁）
- [x] **D4** `RingBuffer<T, const N: usize>` 在 ring.rs 内实现（无外部依赖，使用 `MaybeUninit` + unsafe 块）
- [x] **D5** `DriverError::Timeout` 变体已添加到框架
- [x] **D6** 使用 `core::sync::atomic::AtomicBool`
- [x] **D7 修正** DE/RE 控制方法（`configure_de_re`/`set_de_re`）合并到 `UartHw` trait 中（原设计为 `&'static dyn HalGpio`，因 `HalGpio` 无 `Send + Sync` 导致 `Rs485Driver` 无法满足 `DeviceDriver: Send + Sync`，故修正）
- [x] **D8** 延时由 `UartHw` 实现负责（`Rs485Driver` 不直接调用 delay_us）
- [x] **D9** 无 `tx_buffer` 字段（同步发送，无需缓冲）
- [x] **D10** `recv()` 返回 `alloc::vec::Vec<u8>`

## 编译错误修复记录

实现过程中发现并修复以下编译错误：

1. **driver.rs:113** — `DriverError` 未实现 `Copy`，`Some(e)` 后 `return Err(e)` 使用已移动值。修复：`Some(e.clone())`。
2. **ring.rs:68** — `MaybeUninit::assume_init_read` 是 unsafe 函数，需 `unsafe` 块包裹。修复：添加 `unsafe { }` 块 + SAFETY 注释。
3. **lib.rs 测试模块** — 缺少 `DeviceDriver` trait 与 `DriverType` 导入，导致 54 个 "method not found" 错误。修复：补充 `use eneros_driver_framework::{DeviceDriver, ..., DriverType}`。
