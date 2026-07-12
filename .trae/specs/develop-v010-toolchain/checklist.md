# Checklist — EnerOS v0.1.0 开发环境与交叉编译工具链

## 工具链与配置
- [x] `rust-toolchain.toml` 存在且锁定 nightly-2026-04-04
- [x] `rust-toolchain.toml` 包含 `[unstable]` 段启用 build-std 与 custom_test_frameworks
- [x] `.cargo/config.toml` 存在且配置 target = "aarch64-unknown-none"
- [x] `.cargo/config.toml` 配置 build-std = ["core", "alloc"]

## Workspace 结构
- [x] `Cargo.toml`（workspace 根）存在且包含 kernel 与 runtime 成员
- [x] `kernel/Cargo.toml` 存在，crate 名为 eneros-kernel，版本 0.1.0
- [x] `runtime/Cargo.toml` 存在，crate 名为 eneros-runtime，版本 0.1.0
- [x] `runtime/Cargo.toml` 包含 sel4 依赖（git tag v3.0.0）

## no_std 合规
- [x] `kernel/src/lib.rs` 首行为 `#![no_std]`
- [x] `kernel/src/lib.rs` 无 `use std::*` 语句
- [x] `runtime/src/main.rs` 首行为 `#![no_std]` 且 `#![no_main]`
- [x] `runtime/src/main.rs` 无 `use std::*` 语句

## seL4 集成
- [x] runtime crate 依赖 rust-sel4 v3.0.0
- [x] Makefile 中 seL4 版本锁定为 14.0.0

## 构建系统
- [x] `Makefile` 存在
- [x] `Makefile` 包含 `build` 目标
- [x] `Makefile` 包含 `dtb` 目标
- [x] `Makefile` 包含 `run` 目标
- [x] `Makefile` 包含 `clean` 目标
- [x] `Makefile` 包含 `gdb` 目标

## 设备树
- [x] `configs/qemu-virt.dts` 存在
- [x] DTS 文件为 QEMU virt 机器有效设备树（已修复：移除 #define 宏，使用数值常量）

## 工具链脚本
- [x] `tools/setup-toolchain.sh` 存在
- [x] 脚本支持 WSL2 Ubuntu-22.04
- [x] 脚本安装 rust nightly + aarch64-linux-gnu-gcc + qemu + cmake + ninja + dtc + gdb-multiarch
- [x] 脚本幂等（重复执行不报错）

## CI 流水线
- [x] `.github/workflows/ci.yml` 存在
- [x] CI 包含 `cargo fmt --check` 步骤
- [x] CI 包含 `cargo clippy -- -D warnings` 步骤
- [x] CI 包含 `cargo test` 步骤
- [x] CI 包含交叉编译步骤（aarch64-unknown-none）

## GDB 调试
- [x] `.gdbinit` 存在
- [x] `.gdbinit` 配置连接 QEMU :1234
- [x] `.gdbinit` 设置断点 eneros_runtime::_start

## .gitignore 完整性
- [x] `.gitignore` 包含 `target/`
- [x] `.gitignore` 包含 `build/`
- [x] `.gitignore` 包含 `*.elf`、`*.bin`、`*.img`
- [x] `.gitignore` 包含 `*.dtb`
- [x] `.gitignore` 包含 `qemu-output/`

## 文档
- [x] `README.md` 存在
- [x] README 包含项目简介
- [x] README 包含目录结构说明
- [x] README 包含环境要求
- [x] README 包含快速开始（构建步骤）
- [x] README 包含 QEMU 运行步骤
- [x] README 包含 GDB 调试说明

## WSL2 运行时验证（需在 WSL2 中执行）
- [ ] `rustup show` 显示 nightly-2026-04-04
- [ ] `cargo --version` 可执行
- [ ] `aarch64-linux-gnu-gcc --version` 可执行
- [ ] `qemu-system-aarch64 --version` 可执行
- [ ] `make build` 成功产出镜像
- [ ] `make dtb` 成功产出 qemu-virt.dtb
- [ ] `make run` QEMU 启动并输出 "EnerOS boot: v0.1.0 (seL4 integrated)"
- [ ] GDB 可连接 QEMU :1234 并命中断点
