# Tasks — EnerOS v0.3.0 seL4 在 ARM64 硬件启动

- [x] Task 1: 创建 board crate 骨架
  - [x] SubTask 1.1: 创建 `board/Cargo.toml`（crate 名 `eneros-board`，版本 workspace，no_std，无外部依赖）
  - [x] SubTask 1.2: 创建 `board/src/lib.rs`（`#![no_std]`，声明 `pub mod boot_info;` + `pub mod mini_uart;`，不定义 panic_handler）
  - [x] SubTask 1.3: 修改 `Cargo.toml`（workspace 根）：members 添加 `"board"`，workspace.package.version 更新为 `"0.3.0"`

- [x] Task 2: 实现 boot_info 模块
  - [x] SubTask 2.1: 创建 `board/src/boot_info.rs`：定义 `BootStage` 枚举（RomInit/Bootloader/Sel4Loaded/Sel4Running）
  - [x] SubTask 2.2: 创建 `board/src/boot_info.rs`：定义 `BootInfo` 结构体（board_name/ram_base/ram_size/serial_base/cpu_count/freq_mhz）+ `BoardConfig` trait（fn boot_info() -> BootInfo）
  - [x] SubTask 2.3: 添加 `BootInfo` 与 `BootStage` 的单元测试（字段构造、枚举变体匹配，覆盖率 ≥ 80%）

- [x] Task 3: 实现 mini_uart 模块
  - [x] SubTask 3.1: 创建 `board/src/mini_uart.rs`：定义 `SerialOut` trait（putc/puts/hex）
  - [x] SubTask 3.2: 创建 `board/src/mini_uart.rs`：实现 `Pl011Serial` 结构体（new/read/write/putc/puts/hex），按蓝图 §4.5 代码实现

- [x] Task 4: 创建板级配置文件
  - [x] SubTask 4.1: 创建 `board/qemu-virt/boot.txt`（U-Boot 启动脚本：设置 bootcmd 加载 seL4 镜像到 0x40000000 并跳转，~60 行）
  - [x] SubTask 4.2: 创建 `board/qemu-virt/dts`（QEMU virt 设备树覆盖片段：引用 configs/qemu-virt.dts 的 PL011/内存/CPU 节点，~100 行）

- [x] Task 5: 创建烧录脚本
  - [x] SubTask 5.1: 创建 `tools/flash.sh`（支持 `--qemu` 模式启动 QEMU 验证；支持 SD 卡设备参数烧录镜像；含用法提示与安全检查，~80 行）

- [x] Task 6: 更新 Makefile 与 CI
  - [x] SubTask 6.1: 修改 `Makefile`：VERSION 更新为 `0.3.0`，添加 `board-build` 目标（`cargo build -p eneros-board --target $(TARGET) -Z build-std=core,alloc`），help 添加说明
  - [x] SubTask 6.2: 修改 `.github/workflows/ci.yml`：交叉编译步骤新增 `cargo build -p eneros-board`，版本标识更新为 v0.3.0

- [x] Task 7: 创建文档
  - [x] SubTask 7.1: 创建 `docs/hardware-boot-guide.md`（真机启动指南：前置条件、硬件连接、SD 卡烧录步骤、U-Boot 配置、预期串口输出）
  - [x] SubTask 7.2: 创建 `docs/serial-debug-manual.md`（串口调试手册：波特率 115200、USB 转串口接线、minicom/screen 使用、常见故障排查）
  - [x] SubTask 7.3: 创建 `docs/device-tree-spec.md`（设备树说明：DTS 结构、节点说明、与 board crate 的关系、如何适配新板）

- [x] Task 8: 更新 kernel/src/lib.rs 注释
  - [x] SubTask 8.1: 修改 `kernel/src/lib.rs`：更新 init() 注释从 "will be implemented in v0.3.0" 改为说明 board crate 已提供启动支持，kernel init 将在后续版本实现

- [x] Task 9: 验证与测试
  - [x] SubTask 9.1: `cargo fmt --all -- --check` 无差异 — PASSED (exit 0)
  - [x] SubTask 9.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-runtime --all-targets -- -D warnings` 无 warning — PASSED (修复 clone_on_copy 后 exit 0)
  - [x] SubTask 9.3: `cargo test -p eneros-board` 单元测试通过 — PASSED (7/7 tests passed)
  - [x] SubTask 9.4: `cargo run -p eneros-ci` 质量门禁全绿 — PASSED (Overall: PASS, fmt+clippy+audit+test 4 项全绿)
  - [x] SubTask 9.5: `cargo deny check advisories licenses bans sources` 通过 — PASSED (all 4 ok)
  - [x] SubTask 9.6: `cargo build -p eneros-board --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过 — PASSED (exit 0, 13.45s)

# Task Dependencies
- [Task 2] 依赖 [Task 1]（boot_info 模块需 board crate 存在）
- [Task 3] 依赖 [Task 1]（mini_uart 模块需 board crate 存在）
- [Task 2] 和 [Task 3] 可并行
- [Task 6] 依赖 [Task 1]（Makefile/CI 需 board crate 在 workspace）
- [Task 8] 依赖 [Task 1]（注释引用 board crate）
- [Task 9] 依赖 [Task 1]~[Task 8] 全部完成
- [Task 4]、[Task 5]、[Task 7] 可并行（无代码依赖）
