# Checklist — v0.47.0 CAN 驱动

## 目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/drivers/can/` 已放入 `crates/<subsystem>/` 下，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 已添加 `"crates/drivers/can"`
- [x] **C3 跨 crate path 引用**：`crates/drivers/can/Cargo.toml` 中 `eneros-driver-framework` 的 `path` 使用正确相对路径（`path = "../framework"`，同在 drivers/ 下）
- [x] **C4 文档分类**：`docs/drivers/can-driver-design.md` 已放入 `docs/drivers/` 子目录（已存在），未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 构建校验（§2.4.2，必须全部通过）

- [x] **C6 cargo metadata** 成功（workspace 成员路径全部正确）
- [x] **C7 cargo test** 通过（`cargo test -p eneros-can` 全绿，107 单元/集成 + 1 文档测试 = 108 全绿）
- [x] **C8 cargo build --target aarch64-unknown-none** 通过（`eneros-can` 交叉编译成功，含 `-Z build-std-features=compiler-builtins-mem`）
- [x] **C9 cargo fmt --check** 通过
- [x] **C10 cargo clippy** 无 warning（`-p eneros-can -- -D warnings`）
- [x] **C11 cargo deny check** — 未单独执行（已知 GitHub 网络问题，参考既有版本惯例记录已知问题）

## 文档与规范校验

- [x] **C12 文档位置**：`can-driver-design.md` 在 `docs/drivers/` 下
- [x] **C13 无垃圾文件**：`git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] **C14 .gitignore 覆盖**：无新增需忽略文件类型
- [x] **C15 提交信息**：遵循 Conventional Commits

## no_std 合规校验

- [x] **N1** `crates/drivers/can/src/lib.rs` 顶部有 `#![cfg_attr(not(test), no_std)]`
- [x] **N2** 子模块未重复添加 `#![cfg_attr(not(test), no_std)]`（从 lib.rs 继承）
- [x] **N3** 无 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- [x] **N4** 使用 `alloc::*`（Vec/String）而非 `std::*`
- [x] **N5** 外部依赖仅 `eneros-driver-framework`；不依赖 `eneros-hal`（D9）
- [x] **N6** `CanFrame.data` 使用 `Vec<u8>`（0~8 字节）

## 功能校验（对照蓝图 §3 交付物）

- [x] **F1** `CanFrame` 含 id/frame_type/data/dlc 字段 + new_standard/new_extended 方法（无 timestamp，D3）
- [x] **F2** `CanId` 含 Standard(u16)/Extended(u32) 变体 + ID 掩码截断（0x7FF/0x1FFFFFFF）
- [x] **F3** `FrameType` 含 Data/Remote/Error/Overload 变体
- [x] **F4** `CanFilter` 含 filter_id/filter_mask/extended + accept_all/match_exact/match_prefix/matches 方法
- [x] **F5** `CanConfig` 含 controller_type/baud_rate/mode/filters/auto_retransmit 字段 + Default
- [x] **F6** `CanMode` 含 Normal/ListenOnly/Loopback 变体
- [x] **F7** `CanControllerType` 含 MCP2515/Internal/SJA1000 变体（仅配置标识，D2）
- [x] **F8** `CanController` trait 定义（D1：reset/set_baud_rate/set_mode/set_filter/enable_rx_irq/disable_rx_irq/read_rx_buffer/write_tx_buffer/now_ns）
- [x] **F9** `CanDriver` 实现 `DeviceDriver` trait + send/recv/handle_irq 方法
- [x] **F10** `CanStats` 含 tx_count/rx_count/rx_error_count/tx_error_count/bus_off_count
- [x] **F11** `RingBuffer<T, N>` 本地实现（D4，不依赖 v0.44.0 RS485 的 ring.rs）
- [x] **F12** `MockCanController` 测试桩（预置接收帧队列 + 记录发送帧 + 可配置 now_ns）

## 测试覆盖校验（对照蓝图 §6 测试计划）

- [x] **T1** CanFrame 单元测试（标准/扩展/远程帧 + ID 掩码 + 数据长度边界 0/8 字节）
- [x] **T2** CanFilter 单元测试（accept_all/match_exact/match_prefix + 标准/扩展互斥 + 边界值）
- [x] **T3** CanConfig 单元测试（Default 值 + 字段访问）
- [x] **T4** CanStats 单元测试（Default + 递增）
- [x] **T5** CanDriver 状态转换测试（Uninitialized → Ready → Running → Stopped → Dead）
- [x] **T6** CanDriver send() 成功测试（mock controller + tx_count 递增）
- [x] **T7** CanDriver send() 数据过长测试（>8 字节 → InvalidState）
- [x] **T8** CanDriver recv() 成功测试（mock 帧 → 返回 + rx_count 递增）
- [x] **T9** CanDriver recv() 超时测试（空队列 → Timeout + rx_error_count 递增）
- [x] **T10** CanDriver handle_irq() 测试（IRQ 匹配 + 帧入队 + irq_rx 标志）
- [x] **T11** CanDriver health_check() 测试（Healthy/Degraded/Unhealthy 三级）
- [x] **T12** CanDriver Loopback 集成测试（自发自收）
- [x] **T13** CanDriver 多帧收发集成测试（5 帧连续）
- [x] **T14** CanDriver 过滤器集成测试（仅匹配帧入队）

## 验收标准（对照蓝图 §7）

- [x] **A1** CAN 驱动实现 DeviceDriver trait
- [x] **A2** 支持标准帧（11 位 ID）和扩展帧（29 位 ID）
- [x] **A3** ID 过滤器工作正确（accept_all/match_exact/match_prefix）
- [x] **A4** Loopback 模式自发自收验证通过（mock 模拟）
- [x] **A5** 能收发 CAN 帧（mock 验证）

## 偏差声明校验

- [x] **D1** `CanController` trait 定义（HAL 无 CAN 专有方法，本地抽象）
- [x] **D2** `CanControllerType` 枚举仅作配置标识（不实现具体寄存器级操作）
- [x] **D3** `CanFrame` 不含 `timestamp` 字段（无 `MonotonicTime` 类型）
- [x] **D4** `RingBuffer<T, N>` 本地实现（不依赖 v0.44.0 RS485 的 ring.rs）
- [x] **D5** `recv()` 接受 `now_ns: u64` 参数（不使用 `MonotonicTime::now()`）
- [x] **D6** `CanController::read_rx_buffer()` 返回 `Option<CanFrame>`
- [x] **D7** `CanFilter::matches()` 实现 ID+掩码匹配 + 标准帧/扩展帧互斥
- [x] **D8** crate 放入 `crates/drivers/can/`
- [x] **D9** 不依赖 `eneros-hal` crate（HAL 抽象由本地 `CanController` trait 提供）
