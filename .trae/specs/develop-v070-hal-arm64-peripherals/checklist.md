# Checklist — EnerOS v0.7.0 HAL ARM64 外设实现

> **变更ID**：develop-v070-hal-arm64-peripherals
> **蓝图依据**：`蓝图/phase0.md` §v0.7.0（第 1279–1440 行）

---

## 一、模块骨架与版本升级

- [x] `hal/src/arm64/mod.rs` 含 `pub mod uart_pl011/gpio/net_mmio`
- [x] workspace `Cargo.toml` version = 0.7.0
- [x] Host 构建（x86_64）arm64 模块被 cfg 排除，`cargo build -p eneros-hal` 成功
- [x] v0.5.0/v0.6.0 的 mock 和核心模块不受影响

## 二、Pl011Uart 实现（uart_pl011.rs）

### 寄存器常量
- [x] PL011_DR=0x00, PL011_FR=0x18, PL011_IBRD=0x24, PL011_FBRD=0x28
- [x] PL011_LCRH=0x2C, PL011_CR=0x30, PL011_IMSC=0x38
- [x] FR_TXFF=1<<5 (32), FR_RXFE=1<<4 (16), FR_BUSY=1<<3 (8)

### 结构体与方法
- [x] `Pl011Uart` 结构体（base: u64 字段）
- [x] `Pl011Uart::new(base: u64)` const fn
- [x] `Pl011Uart::init(&self, baud: u32, clock_hz: u32)` — 波特率/帧格式/使能
- [x] MMIO 辅助 `unsafe fn w32/r32`

### HalSerial 实现
- [x] `write()` 轮询 FR.TXFF，写 DR，返回 Ok(data.len())
- [x] `read()` 轮询 FR.RXFE，读 DR，返回 Ok(已读字节数)
- [x] `flush()` 轮询 FR.BUSY 直到清零，返回 Ok(())
- [x] `static ARM64_UART` 单例（基址 0x09000000）+ `pub fn serial()` 获取器

## 三、Arm64Gpio 实现（gpio.rs）

### 寄存器常量
- [x] GPIO_DIR=0x04, GPIO_DATA=0x40, GPIO_PUD=0x94

### 结构体与方法
- [x] `Arm64Gpio` 结构体（base: u64, pin_count: u32 字段）
- [x] `Arm64Gpio::new(base: u64, pin_count: u32)` const fn
- [x] MMIO 辅助 `unsafe fn w32/r32`

### HalGpio 实现
- [x] `set_dir()` 校验 pin < pin_count，读写 GPIO_DIR，配置 GPIO_PUD
- [x] `set()` 校验 pin，写 GPIO_DATA
- [x] `get()` 校验 pin，读 GPIO_DATA
- [x] `toggle()` 调用 get + set
- [x] pin 越界返回 `Err(HalError::InvalidParam)`
- [x] `static ARM64_GPIO` 单例（基址 0x09020000, pin_count=32）+ `pub fn gpio()` 获取器

## 四、NetMmio 实现（net_mmio.rs）

- [x] 网口寄存器偏移常量定义
- [x] `NetMmio` 结构体（mac_base: u64, phy_base: u64 字段）
- [x] `NetMmio::new()` const fn
- [x] MMIO 辅助 `unsafe fn w32/r32`
- [x] `read_phy_id()` 通过 MDIO 读取 PHY ID，返回 (u16, u16)
- [x] `read_mac_addr()` 读取 MAC 地址，返回 [u8; 6]
- [x] `static ARM64_NET` 单例 + 获取器

## 五、Provider 更新（provider.rs）

- [x] `serial()` 返回 `crate::arm64::uart_pl011::serial()`
- [x] `gpio()` 返回 `crate::arm64::gpio::gpio()`
- [x] `mem()` panic 消息指向 v0.8.0
- [x] 模块文档注释更新

## 六、单元测试

- [x] uart_pl011.rs 测试：PL011 寄存器偏移常量正确性
- [x] uart_pl011.rs 测试：FR 位常量正确性
- [x] gpio.rs 测试：GPIO 寄存器偏移常量正确性
- [x] net_mmio.rs 测试：网口寄存器常量正确性
- [x] `cargo test -p eneros-hal` 通过（默认 feature）
- [x] `cargo test -p eneros-hal --features mock` 通过（v0.5.0/v0.6.0 回归）

## 七、no_std 合规

- [x] arm64 外设模块代码不使用 `std::*`
- [x] MMIO 使用 `core::ptr::read_volatile`/`write_volatile`
- [x] 交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 成功

## 八、CI / Makefile

- [x] `.github/workflows/ci.yml` 版本标识为 v0.7.0
- [x] `Makefile` VERSION = 0.7.0
- [x] `ci/src/gate.rs` 注释更新
- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [x] `cargo deny check advisories licenses bans sources` 通过

## 九、交叉编译验证

- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-board --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hal --target aarch64-unknown-none` 成功（含外设模块）

## 十、文档交付

- [x] `docs/uart-driver-guide.md` 存在
- [x] 《UART 驱动说明》含 PL011 寄存器表
- [x] 《UART 驱动说明》含初始化序列
- [x] 《UART 驱动说明》含收发流程
- [x] 《UART 驱动说明》含波特率配置公式
- [x] `docs/gpio-usage-guide.md` 存在
- [x] 《GPIO 使用》含寄存器表
- [x] 《GPIO 使用》含方向配置说明
- [x] 《GPIO 使用》含使用示例

## 十一、工作区整洁

- [x] `git status` 无 target/ build/ *.elf *.bin *.img *.dtb 被追踪
- [x] 无 IDE 缓存被追踪

## 十二、蓝图合规性

- [x] 蓝图 §43.1：所有 Rust 代码 no_std
- [x] 蓝图 §43.2：非瓶颈版本，签名可编译
- [x] 蓝图 §7.1：串口可收发数据（交叉编译验证）
- [x] 蓝图 §7.2：GPIO 可控制（交叉编译验证）
- [x] 蓝图 §7.3：网口寄存器可读 ID（结构定义）
- [x] 蓝图 §7.4：文档齐全
- [x] 蓝图 §7.5：出口判定：HAL 外设就绪
- [x] 蓝图 §8.5（v0.5.0）：无 async fn
- [x] v0.5.0 mock 回归兼容
- [x] v0.6.0 核心回归兼容
