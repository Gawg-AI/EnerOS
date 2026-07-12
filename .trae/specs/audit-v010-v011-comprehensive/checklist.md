# Checklist — EnerOS v0.1.0~v0.11.0 全面检查与修复

> **范围**：v0.1.0~v0.11.0 已交付的全部 10 个 crate
> **合规性**：蓝图 §43.1（no_std）、§七（CI/CD）、§1.3（收工检查）
> **审计日期**：2026-07-12

---

## 1. 代码格式（cargo fmt）

- [x] `cargo fmt --all -- --check` 退出码为 0
- [x] 无格式违规输出

## 2. Clippy lint

- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 退出码为 0
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 退出码为 0
- [x] 无 clippy warning

## 3. 依赖安全与合规（cargo-deny）

- [ ] `cargo deny check advisories` 通过 — **本地未安装 cargo-deny，需在 CI 中验证**
- [ ] `cargo deny check licenses` 通过 — **同上**
- [ ] `cargo deny check bans` 通过 — **同上**
- [ ] `cargo deny check sources` 通过 — **同上**

> **注**：`deny.toml` 配置文件已存在且正确配置。cargo-deny 在本地编译安装超时，
> CI 流水线中通过 `cargo install cargo-deny --locked || true` 安装并执行。

## 4. 单元测试与集成测试

- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部通过
- [x] `cargo test -p eneros-hal --features mock` 全部通过
- [x] 各 crate 测试数量记录：
  - [x] eneros-board: 7 个测试
  - [x] eneros-ci: 5 个测试
  - [x] eneros-hal: 11 个测试（默认）+ 12 个测试（mock feature）= 23 个测试
  - [x] eneros-heap: 27 个测试
  - [x] eneros-mm: 44 个测试
  - [x] eneros-runtime: 11 个测试
  - [x] eneros-sel4-sys: 6 个测试
  - [x] eneros-user-heap: 9 个测试
  - **总计：120 个工作区测试 + 23 个 hal mock 测试 = 143 个测试全部通过**

## 5. 交叉编译验证（aarch64-unknown-none）

- [x] eneros-kernel 交叉编译通过（修复后无 warning）
- [x] eneros-runtime 交叉编译通过
- [x] eneros-board 交叉编译通过
- [x] eneros-sel4-sys 交叉编译通过
- [x] eneros-hello 交叉编译通过
- [x] eneros-hal 交叉编译通过
- [x] eneros-mm 交叉编译通过
- [x] eneros-heap 交叉编译通过
- [x] eneros-user-heap 交叉编译通过

## 6. no_std 合规性

- [x] 所有 crate 的 lib.rs/main.rs 含 `#![cfg_attr(not(test), no_std)]` 或 `#![no_std]`
- [x] kernel crate 含 `#![no_std]`（注：kernel 是库 crate，非二进制，不需要 `#![no_main]`）
- [x] hello crate 含 `#![no_std]` + `#![no_main]`
- [x] 源码中无 `use std::` 引用（除 `#[cfg(test)]` 模块内和 ci crate 外）
  - **注**：`eneros-ci` 是 host-side CI 工具，使用 `std` 是设计如此，非 OS 的一部分
  - `user/heap/src/lib.rs:211` 的 `use std::panic` 在 `#[cfg(test)]` 模块内 ✅
  - `ci/src/gate.rs` 和 `ci/src/error.rs` 的 `use std::` 属于 ci 工具 ✅
- [x] 源码中无 `std::collections::HashMap`、`std::sync::Mutex`、`std::net::*` 等

## 7. 版本号一致性

- [x] workspace.package.version = "0.11.0"
- [x] Makefile VERSION = 0.11.0（文件头 `Version: v0.11.0`）
- [x] ci.yml 版本标识 v0.11.0
- [x] ci/src/gate.rs 注释含 v0.11.0（第 98、141 行）
- [x] 各 crate 版本号记录：
  - [x] kernel: workspace (0.11.0)
  - [x] runtime: workspace (0.11.0)
  - [x] ci: 0.2.0（host-side 工具，独立版本号）
  - [x] board: workspace (0.11.0)
  - [x] sel4-sys: workspace (0.11.0)
  - [x] hello: workspace (0.11.0)
  - [x] hal: workspace (0.11.0)
  - [x] mm: 0.8.0（锁定引入版本）
  - [x] heap: 0.10.0（锁定引入版本）
  - [x] user/heap: 0.11.0

## 8. 文档完整性

- [x] v0.1.0（工具链）：rust-toolchain.toml 存在
- [x] v0.2.0（CI/CD）：docs/ci-cd-manual.md、docs/code-conventions.md、docs/commit-conventions.md 存在
- [x] v0.3.0（硬件启动）：docs/hardware-boot-guide.md、docs/serial-debug-manual.md、docs/device-tree-spec.md 存在
- [x] v0.4.0（首个用户态）：docs/userland-runtime-design.md、docs/sel4-api-bindings.md 存在
- [x] v0.5.0（HAL 接口）：docs/hal-interface-spec.md、docs/hal-design-whitepaper.md 存在
- [x] v0.6.0（HAL ARM64 核心）：docs/gicv3-driver-guide.md、docs/arm-generic-timer-usage.md 存在
- [x] v0.7.0（HAL ARM64 外设）：docs/uart-driver-guide.md、docs/gpio-usage-guide.md 存在
- [x] v0.8.0（页表/vspace）：docs/arm64-page-table-design.md、docs/address-space-layout.md 存在
- [x] v0.9.0（分区隔离）：docs/partition-isolation-design.md、docs/dma-protection-guide.md 存在
- [x] v0.10.0（内核堆）：docs/kernel-heap-design.md、docs/slab-buddy-algorithm.md 存在
- [x] v0.11.0（用户态堆）：docs/user-heap-design.md、docs/oom-policy.md 存在

## 9. spec 目录完整性

- [ ] develop-v010-toolchain/ — **9 项未勾选（WSL2 环境验证，需用户在 WSL2 中手动执行）**
- [ ] develop-v020-cicd-pipeline/ — **1 项未勾选（CI 全流程 < 10 分钟，需 GitHub Actions 执行）**
- [x] develop-v030-sel4-hardware-boot/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v040-first-userland-component/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v050-hal-interface-spec/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v060-hal-arm64-core/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v070-hal-arm64-peripherals/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v080-page-table-vspace/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v090-partition-isolation/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v100-kernel-heap/ 的 tasks.md/checklist.md 已勾选
- [x] develop-v110-user-heap/ 的 tasks.md/checklist.md 已勾选

> **注**：v0.1.0 的未勾选项均为 WSL2 环境验证（rustup/cargo/gcc/qemu/make 等），
> 需在 WSL2 中手动执行。v0.2.0 的未勾选项为 CI 运行时间验证，需 GitHub Actions 执行。

## 10. 工作区整洁

- [x] `git status` 无 target/ 被追踪
- [x] `git status` 无 *.elf、*.bin、*.dtb 被追踪
- [x] `git status` 无 .idea/、.vscode/ 被追踪
- [x] `git status` 无 *.tmp、*.bak、*.swp 被追踪
- [x] `git status` 无 *.pem、*.key、.env 被追踪
- [x] `.gitignore` 含 target/
- [x] `.gitignore` 含 build/
- [x] `.gitignore` 含 *.elf
- [x] `.gitignore` 含 *.dtb
- [x] `.gitignore` 含 .idea/
- [x] `.gitignore` 含 .vscode/

## 11. 修复验证

- [x] 所有检查中发现的问题已修复（kernel `lang_items` warning 已修复）
- [x] 修复后重新运行交叉编译，确认全绿且无 warning
- [x] 修复未引入新的 warning 或测试失败
- [x] 修复符合"外科手术式修改"原则（仅添加一行 `#![allow(internal_features)]`）

### 修复详情

**问题**：`kernel/src/lib.rs` 使用 `#![feature(lang_items)]` 但缺少 `#![allow(internal_features)]`，
导致交叉编译时产生 `warning: the feature 'lang_items' is internal to the compiler or standard library`。

**修复**：在 `kernel/src/lib.rs` 第 8 行添加 `#![allow(internal_features)]`，与 `hello/src/main.rs` 
的模式保持一致（hello crate 已有此属性）。

**影响范围**：仅 `kernel/src/lib.rs` 一行新增，不影响任何功能逻辑。
