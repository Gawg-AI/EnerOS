# Tasks — EnerOS v0.7.0 HAL ARM64 外设实现

> **变更ID**：develop-v070-hal-arm64-peripherals
> **蓝图依据**：`蓝图/phase0.md` §v0.7.0（第 1279–1440 行）
> **原则**：非瓶颈版本，trait/struct 签名必须可编译（蓝图 §43.2）

---

# Task 1: 模块骨架更新与版本升级

在 eneros-hal arm64 模块中新增外设子模块声明，升级工作区版本。

- [x] SubTask 1.1: 修改 `hal/src/arm64/mod.rs`
  - 添加 `pub mod uart_pl011;`
  - 添加 `pub mod gpio;`
  - 添加 `pub mod net_mmio;`
  - 更新模块文档注释说明新增子模块
- [x] SubTask 1.2: 修改 workspace 根 `Cargo.toml`
  - version `0.6.0` → `0.7.0`
- [x] SubTask 1.3: 创建空文件 `hal/src/arm64/uart_pl011.rs`、`gpio.rs`、`net_mmio.rs`（仅模块文档注释）
- [x] SubTask 1.4: 验证 `cargo build -p eneros-hal` 成功（host，arm64 被 cfg 排除）

---

# Task 2: 实现 Pl011Uart（hal/src/arm64/uart_pl011.rs）

实现 HalSerial trait，基于 ARM PL011 UART 硬件。

- [x] SubTask 2.1: 定义 PL011 寄存器偏移常量
  - PL011_DR=0x00, PL011_FR=0x18, PL011_IBRD=0x24, PL011_FBRD=0x28
  - PL011_LCRH=0x2C, PL011_CR=0x30, PL011_IMSC=0x38
- [x] SubTask 2.2: 定义 FR 位常量
  - FR_TXFF=1<<5, FR_RXFE=1<<4, FR_BUSY=1<<3
- [x] SubTask 2.3: 定义 `Pl011Uart` 结构体（base: u64 字段）
- [x] SubTask 2.4: 实现 `Pl011Uart::new(base: u64) -> Self`（const fn）
- [x] SubTask 2.5: 实现 `Pl011Uart::init(&self, baud: u32, clock_hz: u32)` — 配置波特率/帧格式/使能
- [x] SubTask 2.6: 实现 MMIO 辅助 `unsafe fn w32/r32`
- [x] SubTask 2.7: 实现 `HalSerial::write(&self, data: &[u8])` — 轮询 FR.TXFF，写 DR，返回 Ok(data.len())
- [x] SubTask 2.8: 实现 `HalSerial::read(&self, buf: &mut [u8])` — 轮询 FR.RXFE，读 DR，返回 Ok(已读字节数)
- [x] SubTask 2.9: 实现 `HalSerial::flush(&self)` — 轮询 FR.BUSY 直到清零，返回 Ok(())
- [x] SubTask 2.10: 添加 `static ARM64_UART: Pl011Uart = Pl011Uart::new(0x09000000)` 单例
- [x] SubTask 2.11: 添加 `pub fn serial() -> &'static dyn crate::HalSerial` 获取器
- [x] SubTask 2.12: 添加 `#[cfg(test)] mod tests` — 寄存器常量验证测试

---

# Task 3: 实现 Arm64Gpio（hal/src/arm64/gpio.rs）

实现 HalGpio trait，基于通用 GPIO 控制器寄存器接口。

- [x] SubTask 3.1: 定义 GPIO 寄存器偏移常量
  - GPIO_DIR=0x04, GPIO_DATA=0x40, GPIO_PUD=0x94
- [x] SubTask 3.2: 定义 `Arm64Gpio` 结构体（base: u64, pin_count: u32 字段）
- [x] SubTask 3.3: 实现 `Arm64Gpio::new(base: u64, pin_count: u32) -> Self`（const fn）
- [x] SubTask 3.4: 实现 MMIO 辅助 `unsafe fn w32/r32`
- [x] SubTask 3.5: 实现 `HalGpio::set_dir(&self, config: GpioConfig)` — 校验 pin < pin_count，读写 GPIO_DIR，配置 GPIO_PUD
- [x] SubTask 3.6: 实现 `HalGpio::set(&self, pin: u32, val: bool)` — 校验 pin，写 GPIO_DATA
- [x] SubTask 3.7: 实现 `HalGpio::get(&self, pin: u32) -> Result<bool, HalError>` — 校验 pin，读 GPIO_DATA
- [x] SubTask 3.8: 实现 `HalGpio::toggle(&self, pin: u32)` — 调用 get + set
- [x] SubTask 3.9: 实现 pin 越界保护 — 返回 `Err(HalError::InvalidParam)`
- [x] SubTask 3.10: 添加 `static ARM64_GPIO: Arm64Gpio = Arm64Gpio::new(0x09020000, 32)` 单例
- [x] SubTask 3.11: 添加 `pub fn gpio() -> &'static dyn crate::HalGpio` 获取器
- [x] SubTask 3.12: 添加 `#[cfg(test)] mod tests` — 寄存器常量验证测试

---

# Task 4: 实现 NetMmio（hal/src/arm64/net_mmio.rs）

网口寄存器级访问（无 trait 实现，仅 MMIO 读取）。

- [x] SubTask 4.1: 定义网口寄存器偏移常量（MAC_BASE, PHY_BASE, MDIO 等）
- [x] SubTask 4.2: 定义 `NetMmio` 结构体（mac_base: u64, phy_base: u64 字段）
- [x] SubTask 4.3: 实现 `NetMmio::new(mac_base: u64, phy_base: u64) -> Self`（const fn）
- [x] SubTask 4.4: 实现 MMIO 辅助 `unsafe fn w32/r32`
- [x] SubTask 4.5: 实现 `NetMmio::read_phy_id(&self) -> (u16, u16)` — 通过 MDIO 读取 PHY ID 寄存器
- [x] SubTask 4.6: 实现 `NetMmio::read_mac_addr(&self) -> [u8; 6]` — 读取 MAC 地址寄存器
- [x] SubTask 4.7: 添加 `static ARM64_NET: NetMmio` 单例（使用 QEMU virt 网口基址）
- [x] SubTask 4.8: 添加 `#[cfg(test)] mod tests` — 寄存器常量验证测试

---

# Task 5: 更新 Arm64HalCoreProvider（hal/src/arm64/provider.rs）

补全 serial() 和 gpio()，更新 mem() panic 消息。

- [x] SubTask 5.1: 修改 `serial()` — 从 panic 改为 `crate::arm64::uart_pl011::serial()`
- [x] SubTask 5.2: 修改 `gpio()` — 从 panic 改为 `crate::arm64::gpio::gpio()`
- [x] SubTask 5.3: 修改 `mem()` panic 消息 — 从 "v0.7.0" 改为 "v0.8.0"
- [x] SubTask 5.4: 更新模块文档注释说明 serial/gpio 已实现，mem 推迟到 v0.8.0
- [x] SubTask 5.5: 验证 `cargo build -p eneros-hal --target aarch64-unknown-none` 成功

---

# Task 6: 编写单元测试

Host 端可测试的部分（寄存器常量验证）；aarch64 代码通过交叉编译验证。

- [x] SubTask 6.1: 在 `uart_pl011.rs` 中添加 `#[cfg(test)] mod tests`
  - PL011 寄存器偏移常量正确性（PL011_DR==0x00, PL011_FR==0x18 等）
  - FR 位常量正确性（FR_TXFF==32, FR_RXFE==16, FR_BUSY==8）
- [x] SubTask 6.2: 在 `gpio.rs` 中添加 `#[cfg(test)] mod tests`
  - GPIO 寄存器偏移常量正确性（GPIO_DIR==0x04, GPIO_DATA==0x40, GPIO_PUD==0x94）
- [x] SubTask 6.3: 在 `net_mmio.rs` 中添加 `#[cfg(test)] mod tests`
  - 网口寄存器常量正确性
- [x] SubTask 6.4: 验证 `cargo test -p eneros-hal --features mock` 通过（v0.5.0/v0.6.0 回归）
- [x] SubTask 6.5: 验证 `cargo test -p eneros-hal` 通过（默认 feature）

---

# Task 7: 集成到 CI / Makefile

更新版本号与构建配置。

- [x] SubTask 7.1: 修改 `.github/workflows/ci.yml` — 版本标识 v0.6.0 → v0.7.0
- [x] SubTask 7.2: 修改 `Makefile` — VERSION 0.6.0 → 0.7.0
- [x] SubTask 7.3: 修改 `ci/src/gate.rs` — 注释更新说明 v0.7.0 外设模块
- [x] SubTask 7.4: 验证 `cargo fmt --all -- --check` 通过
- [x] SubTask 7.5: 验证 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 7.6: 验证 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [x] SubTask 7.7: 验证交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 通过（含外设模块）

---

# Task 8: 编写文档

交付两份技术文档。

- [x] SubTask 8.1: 创建 `docs/uart-driver-guide.md`《UART 驱动说明》
  - PL011 架构概述（ARM PrimeCell UART）
  - 寄存器表（DR/FR/IBRD/FBRD/LCRH/CR/IMSC）
  - FR 位定义表（TXFF/RXFE/BUSY 等）
  - 初始化序列（禁用→配置波特率→配置帧格式→使能）
  - 收发流程（write 轮询 TXFF→写 DR；read 轮询 RXFE→读 DR）
  - 波特率配置公式（baud = clock / (16 * (IBRD + FBRD/64))）
  - QEMU virt PL011 基址说明（0x09000000）
- [x] SubTask 8.2: 创建 `docs/gpio-usage-guide.md`《GPIO 使用》
  - GPIO 控制器概述
  - 寄存器表（DIR/DATA/PUD）
  - 方向配置（Output 置位 / Input 清零）
  - 上下拉配置（None/Up/Down）
  - 使用示例（配置 LED 输出、读取按键输入）
  - 越界保护说明
  - QEMU virt GPIO 基址说明

---

# Task 9: 验证与收尾

全量验证。

- [x] SubTask 9.1: `cargo fmt --all -- --check`
- [x] SubTask 9.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
- [x] SubTask 9.3: `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings`
- [x] SubTask 9.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
- [x] SubTask 9.5: `cargo test -p eneros-hal --features mock`
- [x] SubTask 9.6: `cargo deny check advisories licenses bans sources`
- [x] SubTask 9.7: 交叉编译全部 crate 到 aarch64-unknown-none（kernel/runtime/board/sel4-sys/hello/hal）
- [x] SubTask 9.8: 确认 `git status` 无垃圾文件
- [x] SubTask 9.9: 更新 checklist.md

---

# Task Dependencies

- Task 2/3/4 依赖 Task 1（模块骨架）
- Task 5 依赖 Task 2/3（provider 引用 uart/gpio 单例）
- Task 6 依赖 Task 5（测试）
- Task 7 依赖 Task 6（CI 集成）
- Task 8 可与 Task 5/6/7 并行（文档独立）
- Task 9 依赖全部前序
