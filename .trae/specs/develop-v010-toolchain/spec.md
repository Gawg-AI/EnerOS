# EnerOS v0.1.0 — 开发环境与交叉编译工具链 Spec

## Why

EnerOS Phase 0 的零号交付物。后续 21 个版本（v0.2.0 ~ v0.22.0）都依赖此工具链才能落地。当前工作区为空白（仅有 `.gitignore` 与 `.trae/`），需要从零搭建可复现的交叉编译环境，产出可在 ARM64 上启动的 seL4 + Rust 镜像。

蓝图依据：`e:\eneros\蓝图\phase0.md` §v0.1.0。

## What Changes

- 新建 Cargo workspace 根配置（`Cargo.toml`）
- 新建 `rust-toolchain.toml`：锁定 Rust nightly-2026-04-04 + `build-std` unstable feature
- 新建 `.cargo/config.toml`：交叉编译目标 `aarch64-unknown-none` + linker 配置 + build-std
- 新建 `kernel/` crate（no_std，Phase 0 内核占位）
- 新建 `runtime/` crate（no_std + alloc，用户态根任务占位，使用 rust-sel4 v3.0.0）
- 新建 `Makefile`：统一构建入口（`make build` / `make run` / `make dtb` / `make clean`）
- 新建 `tools/setup-toolchain.sh`：工具链一键安装脚本（WSL2 Ubuntu-22.04）
- 新建 `configs/qemu-virt.dts`：QEMU virt 机器设备树源文件
- 新建 `.github/workflows/ci.yml`：CI 流水线（fmt/clippy/test/交叉编译）
- 新建 `README.md`：项目说明与快速开始
- 新建 `.gdbinit`：GDB 调试配置
- 版本锁定：seL4 14.0.0、rust-sel4 v3.0.0、Rust nightly-2026-04-04

## Impact

- **Affected specs**: 无（首个开发版本，无前序 spec）
- **Affected code**:
  - `Cargo.toml`（workspace 根）
  - `rust-toolchain.toml`
  - `.cargo/config.toml`
  - `kernel/Cargo.toml`、`kernel/src/lib.rs`
  - `runtime/Cargo.toml`、`runtime/src/main.rs`
  - `Makefile`
  - `tools/setup-toolchain.sh`
  - `configs/qemu-virt.dts`
  - `.github/workflows/ci.yml`
  - `README.md`
  - `.gdbinit`
- **依赖输入**:
  - `e:\eneros\蓝图\phase0.md`（v0.1.0 详细蓝图）
  - `e:\eneros\蓝图\Power_Native_Agent_OS_Version_Roadmap_v3.md`（版本路线图）
  - `e:\eneros\.trae\rules\记忆.md`（开发管理记忆，§三 垃圾文件治理、§四 开发流程、§五 版本号与依赖管理）
- **后续影响**: v0.2.0（CI/CD）将基于本工具链；v0.3.0（seL4 硬件启动）将使用本镜像

## ADDED Requirements

### Requirement: Rust 工具链锁定

系统 SHALL 通过 `rust-toolchain.toml` 锁定 Rust nightly-2026-04-04 版本，并启用 `build-std`、`custom_test_frameworks` unstable feature。

#### Scenario: 工具链版本一致
- **WHEN** 在项目根目录执行 `rustup show`
- **THEN** 显示 `nightly-2026-04-04` 为当前工具链
- **AND** `cargo --version` 可正常执行

#### Scenario: build-std 可用
- **WHEN** 执行 `cargo build -Z build-std=core,alloc --target aarch64-unknown-none`
- **THEN** 编译成功，无 build-std 相关错误

### Requirement: Cargo Workspace 结构

系统 SHALL 建立包含 `kernel` 与 `runtime` 两个 crate 的 Cargo workspace。

#### Scenario: workspace 结构正确
- **WHEN** 执行 `cargo metadata --no-deps`
- **THEN** 返回的 JSON 中 `packages` 数组包含 `eneros-kernel` 与 `eneros-runtime` 两个包
- **AND** 两个包的 `version` 均为 `0.1.0`

### Requirement: no_std 合规

系统 SHALL 确保所有 Rust 代码遵循 no_std 规范（蓝图 §43.1）。

#### Scenario: kernel crate no_std
- **WHEN** 检查 `kernel/src/lib.rs`
- **THEN** 文件首行为 `#![no_std]`
- **AND** 不存在 `use std::*` 语句

#### Scenario: runtime crate no_std
- **WHEN** 检查 `runtime/src/main.rs`
- **THEN** 文件首行为 `#![no_std]` 且 `#![no_main]`
- **AND** 不存在 `use std::*` 语句

### Requirement: seL4 集成

系统 SHALL 通过 rust-sel4 v3.0.0 集成 seL4 14.0.0 微内核。

#### Scenario: rust-sel4 依赖锁定
- **WHEN** 检查 `runtime/Cargo.toml`
- **THEN** `dependencies` 中包含 `sel4` 依赖
- **AND** git 源指向 rust-sel4 仓库且 tag 为 `v3.0.0`

### Requirement: 构建系统

系统 SHALL 提供 `Makefile` 作为统一构建入口，支持以下目标。

#### Scenario: make build
- **WHEN** 在 WSL2 中执行 `make build`
- **THEN** 交叉编译产出 seL4 + Rust 镜像
- **AND** 无编译错误

#### Scenario: make dtb
- **WHEN** 执行 `make dtb`
- **THEN** 从 `configs/qemu-virt.dts` 编译产出 `qemu-virt.dtb`

#### Scenario: make run
- **WHEN** 在 WSL2 中执行 `make run`
- **THEN** 启动 QEMU 加载镜像
- **AND** 串口输出包含 `EnerOS boot: v0.1.0 (seL4 integrated)`

#### Scenario: make clean
- **WHEN** 执行 `make clean`
- **THEN** 清理 `target/` 与 `build/` 目录

### Requirement: QEMU 验证

系统 SHALL 能在 QEMU virt 机器上启动 seL4 + Rust 镜像。

#### Scenario: QEMU 启动成功
- **WHEN** 执行 `make run`（在 WSL2 中）
- **THEN** QEMU 启动并加载 seL4 kernel
- **AND** 串口输出 seL4 boot log
- **AND** 输出 `EnerOS boot: v0.1.0 (seL4 integrated)`

### Requirement: CI 流水线

系统 SHALL 提供 GitHub Actions CI 配置，包含格式检查、lint、测试、交叉编译。

#### Scenario: CI 检查项完整
- **WHEN** 检查 `.github/workflows/ci.yml`
- **THEN** 包含以下步骤：
  - `cargo fmt --check`
  - `cargo clippy -- -D warnings`
  - `cargo test`
  - 交叉编译 `cargo build --target aarch64-unknown-none`

### Requirement: 工具链安装脚本

系统 SHALL 提供一键安装脚本 `tools/setup-toolchain.sh`。

#### Scenario: 脚本幂等
- **WHEN** 在 WSL2 Ubuntu-22.04 中执行 `tools/setup-toolchain.sh` 两次
- **THEN** 两次执行均成功
- **AND** 第二次执行不报错（幂等）

### Requirement: 设备树源文件

系统 SHALL 提供 QEMU virt 机器的设备树源文件。

#### Scenario: DTS 可编译为 DTB
- **WHEN** 执行 `make dtb`
- **THEN** 产出 `qemu-virt.dtb` 文件
- **AND** 文件大小 > 0

### Requirement: GDB 调试支持

系统 SHALL 提供 GDB 调试配置，支持通过 QEMU `-gdb` 参数远程调试。

#### Scenario: GDB 连接
- **WHEN** QEMU 以 `-gdb tcp::1234` 启动
- **AND** GDB 执行 `target remote :1234`
- **THEN** 连接成功
- **AND** 可在 `eneros_root_task::main` 设置断点

### Requirement: .gitignore 完整性

系统 SHALL 确保 `.gitignore` 覆盖所有构建产物与垃圾文件类型（蓝图 §三）。

#### Scenario: 构建产物被忽略
- **WHEN** 执行 `make build` 后运行 `git status`
- **THEN** `target/`、`build/`、`*.elf`、`*.bin`、`*.dtb` 均不出现在未追踪文件列表中

### Requirement: 文档交付

系统 SHALL 提供 README.md 文档，包含项目简介、目录结构、快速开始、构建说明。

#### Scenario: README 包含快速开始
- **WHEN** 阅读 `README.md`
- **THEN** 包含"快速开始"章节
- **AND** 包含环境要求、构建步骤、QEMU 运行步骤
