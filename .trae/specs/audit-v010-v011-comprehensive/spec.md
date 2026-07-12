# EnerOS v0.1.0~v0.11.0 全面检查与修复 Spec

## Why

EnerOS 已完成 v0.1.0~v0.11.0 共 11 个版本的增量开发（Phase 0 的 P0-A~P0-C）。
各版本开发时虽各自通过了局部验证，但尚未进行过一次统一的回归审计。随着 v0.11.0
修复 `#[global_allocator]` 冲突时对 `eneros-heap` 进行了改动，需要确认全工作区
仍然健康，并为后续 v0.12.0（RTC）开发建立干净基线。

## What Changes

- 运行全套 CI 检查（fmt / clippy / cargo-deny / test / cross-build / workspace-clean）
- 验证 10 个 crate 的版本号一致性与 no_std 合规性
- 验证 v0.1.0~v0.11.0 各版本交付文档完整性
- 修复检查中发现的任何回归或缺陷
- 不新增功能，不重构现有代码（除非修复必需）

## Impact

- **Affected specs**: 无（本 spec 是审计性任务，不产生新功能 spec）
- **Affected code**: 可能涉及以下 crate 的小幅修复：
  - `kernel/` `runtime/` `ci/` `board/` `sel4-sys/` `hello/` `hal/` `mm/` `heap/` `user/heap/`
- **Affected docs**: 可能补充缺失文档
- **Affected config**: 可能修正 `Cargo.toml` / `Makefile` / `ci.yml` 中的版本不一致

## ADDED Requirements

### Requirement: 全工作区代码格式合规

系统 SHALL 通过 `cargo fmt --all -- --check` 检查，无格式违规。

#### Scenario: 格式检查通过
- **WHEN** 执行 `cargo fmt --all -- --check`
- **THEN** 退出码为 0，无输出

### Requirement: 全工作区 Clippy 零警告

系统 SHALL 通过 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 检查。

#### Scenario: Clippy 检查通过
- **WHEN** 执行 clippy 检查
- **THEN** 退出码为 0，无 warning（`-D warnings` 将 warning 视为 error）

### Requirement: 依赖安全与合规

系统 SHALL 通过 `cargo deny check advisories licenses bans sources` 检查。

#### Scenario: cargo-deny 检查通过
- **WHEN** 执行 cargo-deny 检查
- **THEN** 无安全公告（advisories）违规、无许可证（licenses）违规、无禁用依赖（bans）、无源违规（sources）

### Requirement: 全工作区测试通过

系统 SHALL 通过 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 检查，所有单元测试和集成测试通过。

#### Scenario: 测试全部通过
- **WHEN** 执行工作区测试
- **THEN** 所有 crate 的测试退出码为 0，无失败用例

### Requirement: 全 crate 交叉编译通过

系统 SHALL 能将全部 10 个 crate 交叉编译到 `aarch64-unknown-none` 目标。

#### Scenario: 交叉编译成功
- **WHEN** 对每个 crate 执行 `cargo build -p <crate> --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
- **THEN** 退出码为 0，生成 `.rlib` 文件

### Requirement: no_std 合规性

所有 Rust 源码 SHALL 遵循蓝图 §43.1 no_std 要求：
- 正式构建（aarch64-unknown-none）为 no_std
- 禁止 `use std::*`，改用 `alloc::*` / `core::*` / `heapless::*` / `spin::*`

#### Scenario: 无 std 引用
- **WHEN** 搜索源码中的 `use std::` 引用
- **THEN** 仅出现在 `#[cfg(test)]` 模块内（测试构建允许链接 std）

### Requirement: 版本号一致性

workspace.package.version SHALL 为 `0.11.0`（当前开发版本）。
各 crate 的版本号应明确且合理（继承 workspace 或锁定引入版本）。

#### Scenario: 版本号检查
- **WHEN** 检查各 Cargo.toml 的 version 字段
- **THEN** workspace.package.version = "0.11.0"，各 crate 版本号符合预期

### Requirement: 文档完整性

v0.1.0~v0.11.0 各版本的核心交付文档 SHALL 存在于 `docs/` 目录。

#### Scenario: 文档存在
- **WHEN** 检查 docs/ 目录
- **THEN** 各版本关键文档存在（见 checklist 详细清单）

### Requirement: 工作区整洁

工作区 SHALL 不包含被 Git 追踪的垃圾文件（target/、*.elf、*.bin、*.dtb、IDE 缓存等）。
`.gitignore` SHALL 覆盖所有必要忽略模式。

#### Scenario: 工作区整洁
- **WHEN** 执行 `git status`
- **THEN** 无 target/、*.elf、*.bin、*.dtb、.idea/、.vscode/ 等被追踪

## MODIFIED Requirements

无。本 spec 为审计性任务，不修改现有功能要求。

## REMOVED Requirements

无。
