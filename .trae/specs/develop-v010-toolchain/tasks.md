# Tasks — EnerOS v0.1.0 开发环境与交叉编译工具链

- [x] Task 1: 创建 Cargo workspace 根配置与目录结构
  - [x] SubTask 1.1: 创建 `Cargo.toml`（workspace 根，包含 kernel 与 runtime 成员）
  - [x] SubTask 1.2: 创建 `kernel/Cargo.toml`（crate `eneros-kernel`，版本 0.1.0，no_std）
  - [x] SubTask 1.3: 创建 `runtime/Cargo.toml`（crate `eneros-runtime`，版本 0.1.0，no_std + alloc，依赖 sel4 v3.0.0）
  - [x] SubTask 1.4: 创建 `kernel/src/lib.rs`（no_std 占位入口）
  - [x] SubTask 1.5: 创建 `runtime/src/main.rs`（no_std + no_main 根任务，输出 "EnerOS boot: v0.1.0 (seL4 integrated)"）

- [x] Task 2: 配置 Rust 工具链与交叉编译
  - [x] SubTask 2.1: 创建 `rust-toolchain.toml`（锁定 nightly-2026-04-04 + build-std/custom_test_frameworks unstable feature）
  - [x] SubTask 2.2: 创建 `.cargo/config.toml`（target aarch64-unknown-none + build-std=core,alloc + linker 配置）

- [x] Task 3: 集成 seL4 构建系统
  - [x] SubTask 3.1: 创建 `configs/qemu-virt.dts`（QEMU virt 机器设备树源文件）
  - [x] SubTask 3.2: 创建 `Makefile`（包含 build/dtb/run/clean/gdb 目标，集成 seL4 14.0.0 构建与 rust-sel4 v3.0.0）

- [x] Task 4: 创建工具链安装脚本
  - [x] SubTask 4.1: 创建 `tools/setup-toolchain.sh`（WSL2 Ubuntu-22.04 一键安装：rust nightly + aarch64-linux-gnu-gcc + qemu + cmake + ninja + dtc + gdb-multiarch，幂等）

- [x] Task 5: 配置 CI 流水线
  - [x] SubTask 5.1: 创建 `.github/workflows/ci.yml`（fmt/clippy/test/交叉编译，使用 Makefile 目标）

- [x] Task 6: 创建 GDB 调试配置与文档
  - [x] SubTask 6.1: 创建 `.gdbinit`（连接 QEMU :1234，断点 eneros_runtime::_start）
  - [x] SubTask 6.2: 创建 `README.md`（项目简介、目录结构、环境要求、快速开始、构建步骤、QEMU 运行、GDB 调试）

- [x] Task 7: 验证 .gitignore 完整性
  - [x] SubTask 7.1: 检查现有 `.gitignore` 覆盖 target/、build/、*.elf、*.bin、*.dtb、qemu-output/ 等所有构建产物

- [ ] Task 8: WSL2 环境验证（需用户在 WSL2 中执行）
  - [ ] SubTask 8.1: 在 WSL2 中执行 `tools/setup-toolchain.sh` 验证工具链安装
  - [ ] SubTask 8.2: 在 WSL2 中执行 `make build` 验证交叉编译产出镜像
  - [ ] SubTask 8.3: 在 WSL2 中执行 `make dtb` 验证设备树编译
  - [ ] SubTask 8.4: 在 WSL2 中执行 `make run` 验证 QEMU 启动与串口输出

# Task Dependencies
- [Task 2] 依赖 [Task 1]（workspace 结构需先存在）
- [Task 3] 依赖 [Task 1] 和 [Task 2]（Makefile 依赖 workspace 与工具链配置）
- [Task 5] 依赖 [Task 3]（CI 使用 Makefile 目标）
- [Task 6] 依赖 [Task 3]（GDB 配置依赖 QEMU 参数）
- [Task 8] 依赖 [Task 1]~[Task 7] 全部完成
- [Task 1]、[Task 4]、[Task 7] 可并行
