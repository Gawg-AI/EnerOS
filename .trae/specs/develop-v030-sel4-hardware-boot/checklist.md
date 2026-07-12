# Checklist — EnerOS v0.3.0 seL4 在 ARM64 硬件启动

## board Crate 骨架
- [x] `board/Cargo.toml` 存在，crate 名为 `eneros-board`，version.workspace = true
- [x] `board/src/lib.rs` 包含 `#![no_std]`，不定义 panic_handler
- [x] `board/src/lib.rs` 声明 `pub mod boot_info;` 和 `pub mod mini_uart;`
- [x] workspace `Cargo.toml` members 包含 `"board"`
- [x] workspace.package.version 为 `"0.3.0"`

## boot_info 模块
- [x] `BootStage` 枚举包含 RomInit/Bootloader/Sel4Loaded/Sel4Running 四个变体
- [x] `BootInfo` 结构体包含 board_name/ram_base/ram_size/serial_base/cpu_count/freq_mhz 六个字段
- [x] `BoardConfig` trait 定义 `fn boot_info() -> BootInfo`
- [x] 单元测试覆盖 BootInfo 字段构造与读取
- [x] 单元测试覆盖 BootStage 枚举变体匹配
- [x] 单元测试覆盖率 ≥ 80%

## mini_uart 模块
- [x] `SerialOut` trait 定义 putc/puts/hex 三个方法
- [x] `Pl011Serial` 结构体包含 `base: u64` 字段
- [x] `Pl011Serial::new(base: u64)` 构造函数为 `const fn`
- [x] `Pl011Serial` 实现 `SerialOut` trait
- [x] `putc` 等待 TX FIFO 不满（FR_TXFF 位）后写数据寄存器
- [x] `puts` 对 `\n` 自动补 `\r`
- [x] `hex` 输出 0x 前缀 + 16 位十六进制
- [x] read/write 寄存器方法使用 `core::ptr::read_volatile/write_volatile`

## 板级配置文件
- [x] `board/qemu-virt/boot.txt` 存在，包含 U-Boot bootcmd 配置
- [x] `board/qemu-virt/boot.txt` 设置加载地址为 0x40000000
- [x] `board/qemu-virt/dts` 存在，包含 PL011 UART 节点
- [x] `board/qemu-virt/dts` 包含内存与 CPU 节点定义

## 烧录脚本
- [x] `tools/flash.sh` 存在且可执行
- [x] 脚本支持 `--qemu` 参数启动 QEMU 验证
- [x] 脚本支持 SD 卡设备参数（如 `/dev/sdX`）
- [x] 脚本包含安全检查（防止误写系统盘）

## Makefile 与 CI
- [x] Makefile VERSION 为 `0.3.0`
- [x] Makefile 包含 `board-build` 目标
- [x] `make board-build` 运行 `cargo build -p eneros-board --target aarch64-unknown-none -Z build-std=core,alloc`
- [x] Makefile help 包含 board-build 说明
- [x] CI 交叉编译步骤包含 `cargo build -p eneros-board`
- [x] CI 版本标识更新为 v0.3.0
- [x] CI host 侧 clippy/test 包含 eneros-board（无需排除）

## 文档
- [x] `docs/hardware-boot-guide.md` 存在且包含前置条件
- [x] `docs/hardware-boot-guide.md` 包含 SD 卡烧录步骤
- [x] `docs/hardware-boot-guide.md` 包含 U-Boot 配置说明
- [x] `docs/hardware-boot-guide.md` 包含预期串口输出
- [x] `docs/serial-debug-manual.md` 存在且包含波特率配置（115200）
- [x] `docs/serial-debug-manual.md` 包含 USB 转串口接线说明
- [x] `docs/serial-debug-manual.md` 包含 minicom/screen 使用方法
- [x] `docs/serial-debug-manual.md` 包含常见故障排查
- [x] `docs/device-tree-spec.md` 存在且包含 DTS 结构说明
- [x] `docs/device-tree-spec.md` 包含节点说明（UART/内存/CPU/GIC）
- [x] `docs/device-tree-spec.md` 说明与 board crate 的关系
- [x] `docs/device-tree-spec.md` 包含新板适配指南

## kernel 注释更新
- [x] `kernel/src/lib.rs` init() 注释不再引用 "v0.3.0"
- [x] 注释说明 board crate 已提供启动支持

## 验证
- [x] `cargo fmt --all -- --check` 无差异
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-runtime --all-targets -- -D warnings` 无 warning
- [x] `cargo test -p eneros-board` 全部通过
- [x] `cargo run -p eneros-ci` 全绿
- [x] `cargo deny check advisories licenses bans sources` 通过
- [x] `cargo build -p eneros-board --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
