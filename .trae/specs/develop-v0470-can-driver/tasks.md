# Tasks — v0.47.0 CAN 驱动

## Task 1: workspace 版本号与 members 同步
- [x] SubTask 1.1: 修改根 `Cargo.toml`，`version` 从 `0.46.0` → `0.47.0`
- [x] SubTask 1.2: 向 `members` 数组增加 `"crates/drivers/can"`

## Task 2: 创建 eneros-can crate 骨架
- [x] SubTask 2.1: 创建 `crates/drivers/can/Cargo.toml`（workspace 继承，依赖 `eneros-driver-framework`，不依赖 eneros-hal，D9）
- [x] SubTask 2.2: 创建 `crates/drivers/can/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + re-export）
- [x] SubTask 2.3: 创建 `crates/drivers/can/src/ring.rs`（`RingBuffer<T, N>` 本地实现，D4；参考 v0.44.0 RS485 的 ring.rs 结构）

## Task 3: 实现 CanFrame/CanId/FrameType 帧结构
- [x] SubTask 3.1: 创建 `crates/drivers/can/src/frame.rs`，定义 `FrameType` 枚举（Data/Remote/Error/Overload）
- [x] SubTask 3.2: 定义 `CanId` 枚举（Standard(u16)/Extended(u32)，含掩码截断 0x7FF/0x1FFFFFFF）
- [x] SubTask 3.3: 定义 `CanFrame` 结构（id/frame_type/data: Vec<u8>/dlc: u8，无 timestamp 字段，D3）
- [x] SubTask 3.4: 实现 `CanFrame::new_standard(id, data)` 和 `CanFrame::new_extended(id, data)` 构造方法（含 ID 掩码 + dlc 自动计算）
- [x] SubTask 3.5: 实现 `CanFrame::new_remote(id, is_extended)` 构造方法（远程帧，data 为空，dlc=0）
- [x] SubTask 3.6: 编写帧结构单元测试（标准/扩展/远程帧 + ID 掩码 + 数据长度边界 0/8 字节）

## Task 4: 实现 CanFilter 过滤器
- [x] SubTask 4.1: 创建 `crates/drivers/can/src/filter.rs`，定义 `CanFilter` 结构（filter_id/filter_mask/extended）
- [x] SubTask 4.2: 实现 `CanFilter::accept_all()`（filter_mask=0）
- [x] SubTask 4.3: 实现 `CanFilter::match_exact(id, extended)`（mask=0x7FF 或 0x1FFFFFFF）
- [x] SubTask 4.4: 实现 `CanFilter::match_prefix(prefix, prefix_bits, extended)`（高位掩码匹配）
- [x] SubTask 4.5: 实现 `CanFilter::matches(&self, frame: &CanFrame) -> bool`（ID & mask == filter_id & mask，标准/扩展帧互斥）
- [x] SubTask 4.6: 编写过滤器单元测试（accept_all/match_exact/match_prefix + 标准/扩展互斥 + 边界值）

## Task 5: 实现 CanConfig/CanMode 配置（D2）
- [x] SubTask 5.1: 创建 `crates/drivers/can/src/config.rs`，定义 `CanMode` 枚举（Normal/ListenOnly/Loopback）
- [x] SubTask 5.2: 定义 `CanControllerType` 枚举（MCP2515/Internal/SJA1000，仅作配置标识，D2）
- [x] SubTask 5.3: 定义 `CanConfig` 结构（controller_type/baud_rate/mode/filters/auto_retransmit）+ Default（500kbps/Normal/空过滤器/true）
- [x] SubTask 5.4: 编写配置单元测试（Default 值 + 各字段访问）

## Task 6: 实现 CanController HAL 抽象（D1）
- [x] SubTask 6.1: 创建 `crates/drivers/can/src/controller.rs`，定义 `CanController` trait（D1/D9）
- [x] SubTask 6.2: 定义 trait 方法：`reset`/`set_baud_rate`/`set_mode`/`set_filter`/`enable_rx_irq`/`disable_rx_irq`/`read_rx_buffer`/`write_tx_buffer`/`now_ns`（D5）
- [x] SubTask 6.3: 定义 `CanStats` 结构（tx_count/rx_count/rx_error_count/tx_error_count/bus_off_count）+ Default
- [x] SubTask 6.4: 编写 CanStats 单元测试

## Task 7: 实现 CanDriver 驱动
- [x] SubTask 7.1: 创建 `crates/drivers/can/src/driver.rs`，定义 `CanDriver` 结构（id/config/state/controller: Box<dyn CanController>/rx_queue: RingBuffer<CanFrame, 64>/filters: Vec<CanFilter>/stats: CanStats/irq_rx: AtomicBool）
- [x] SubTask 7.2: 实现 `CanDriver::new(id, config, controller: Box<dyn CanController>)`（name 根据 controller_type 生成）
- [x] SubTask 7.3: 实现 `DeviceDriver::init`（controller.reset + set_baud_rate + set_filter 循环 + set_mode + enable_rx_irq）
- [x] SubTask 7.4: 实现 `DeviceDriver::start`/`stop`/`deinit`/`handle_irq`/`health_check`
- [x] SubTask 7.5: 实现 `send(&mut self, frame: &CanFrame)`（长度校验 >8 → InvalidState + controller.write_tx_buffer + tx_count++）
- [x] SubTask 7.6: 实现 `recv(&mut self, now_ns: u64, timeout_ms: u32)`（D5：从 rx_queue 弹出，超时返回 Timeout，rx_count++/rx_error_count++）
- [x] SubTask 7.7: 实现 `handle_irq`（读取 RX 缓冲，应用软件过滤器匹配后入队，设置 irq_rx 标志）
- [x] SubTask 7.8: 实现 `take_irq_rx`/`stats`/`config` 访问器
- [x] SubTask 7.9: 编写 CanDriver 单元测试（至少 15 个测试：状态转换/init 配置/send 成功/send 数据过长/recv 成功/recv 超时/handle_irq 匹配/handle_irq 不匹配/health_check 三级/trait object 兼容/stats 递增/多帧发送/多帧接收）

## Task 8: 实现 MockCanController 测试桩
- [x] SubTask 8.1: 创建 `crates/drivers/can/src/mock.rs`（`#[cfg(test)]`），实现 `MockCanController`（rx_queue: VecDeque<CanFrame>/tx_frames: Vec<CanFrame>/set_baud_rate_calls/now_ns 可推进）
- [x] SubTask 8.2: 为 `MockCanController` 实现 `CanController` trait（read_rx_buffer 弹出队列、write_tx_buffer 记录、now_ns 返回可配置值）
- [x] SubTask 8.3: 实现 MockCanController 辅助方法（push_rx_frame/set_now_ns/tx_frames/clear）

## Task 9: 集成测试
- [x] SubTask 9.1: 在 `lib.rs` 的 `#[cfg(test)] mod tests` 中编写 CanFrame 端到端测试（构造→发送→接收→校验）
- [x] SubTask 9.2: 编写 CanFilter 集成测试（accept_all + match_exact + match_prefix 组合验证）
- [x] SubTask 9.3: 编写 CanDriver Loopback 模式测试（mock 模拟自发自收）
- [x] SubTask 9.4: 编写 CanDriver 多帧收发测试（5 帧连续发送+接收，验证统计）
- [x] SubTask 9.5: 编写 CanDriver 过滤器集成测试（仅匹配帧入队，不匹配丢弃）

## Task 10: 设计文档
- [x] SubTask 10.1: 创建 `docs/drivers/can-driver-design.md`，包含：版本目标、前置依赖、交付物清单、详细设计（含偏差声明 D1~D9）、帧结构/过滤器/收发流程、测试计划、验收标准、风险、多角度要求

## Task 11: 构建校验（§2.4 清单）
- [x] SubTask 11.1: `cargo metadata --format-version 1 > /dev/null`（workspace 成员路径正确）
- [x] SubTask 11.2: `cargo test -p eneros-can`（单元 + 集成测试通过，107 单元/集成 + 1 文档测试 = 108 全绿）
- [x] SubTask 11.3: `cargo build -p eneros-can --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（交叉编译通过）
- [x] SubTask 11.4: `cargo fmt --all -- --check`（格式检查）
- [x] SubTask 11.5: `cargo clippy -p eneros-can --all-targets -- -D warnings`（lint 无 warning）
- [x] SubTask 11.6: 确认 `.gitignore` 覆盖新产生的文件类型（无新增需忽略类型）

# Task Dependencies

- Task 1（workspace 同步）→ 无前置，可独立执行
- Task 2（crate 骨架 + ring）→ 依赖 Task 1（members 需先添加）
- Task 3（帧结构）→ 依赖 Task 2
- Task 4（过滤器）→ 依赖 Task 3（CanFrame/CanId）
- Task 5（配置）→ 依赖 Task 4（CanFilter）
- Task 6（CanController trait + CanStats）→ 依赖 Task 3（CanFrame）
- Task 7（CanDriver）→ 依赖 Task 2~6 全部（frame/filter/config/controller/ring）
- Task 8（mock）→ 依赖 Task 6（CanController trait）
- Task 9（集成测试）→ 依赖 Task 7（CanDriver）+ Task 8（mock）
- Task 10（设计文档）→ 可与 Task 3~8 并行
- Task 11（构建校验）→ 依赖 Task 1~9 全部完成

# 可并行执行

- Task 3（帧结构）+ Task 6（CanController trait）可并行（均依赖 Task 2）
- Task 4（过滤器）+ Task 5（配置）可在 Task 3 完成后并行
- Task 10（设计文档）可与 Task 3~8 并行
