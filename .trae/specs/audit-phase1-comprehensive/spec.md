# Phase 1 全面测试、验证与修复 Spec

## Why

Phase 1（v0.23.0~v0.74.0）共 52 个版本已全部开发完成，覆盖 12 个子模块（P1-A~P1-L），新增约 44 个 crate。在进入 Phase 2（多机联邦）之前，需要对 Phase 1 全部交付物执行一次系统性测试、验证与修复，确保：

- 所有 crate 编译通过（host + aarch64 交叉编译）
- 所有单元测试通过（无回归）
- 代码质量达标（fmt + clippy + no_std 合规）
- 目录结构与规范一致（§2.4 校验清单 C1~C15）
- **三大出口标准在主机侧全部达成**（autonomous 运行 / 双脑链路 < 2s / 比传统 EMS 收益 ≥ 10%）
- 发现的问题得到修复并通过回归验证

与 Phase 0 审计不同，本次为**审计 + 修复**：发现问题时直接修复源代码，并执行回归测试验证修复有效。

## What Changes

- 执行 workspace 全量构建与测试（Phase 0 + Phase 1 共 61 crate + ci）
- 执行全部 Phase 1 新增 crate 的 aarch64 交叉编译
- 验证 no_std 合规性（全项目无 `use std::*`、无 `panic!`/`todo!`/`unimplemented!`）
- 验证目录结构（§2.4 C1~C15）
- 运行 CI 质量门禁（eneros-ci）
- 验证文档分类（docs/<topic>/，特别关注 Phase 1 新增的 `docs/ai/`、`docs/agents/`、`docs/protocols/`、`docs/security/`）
- 验证 .gitignore 覆盖
- **修复**发现的编译错误、测试失败、fmt/clippy 违规、no_std 违规、目录结构违规
- 汇总 Phase 1 三大出口标准达成情况

## Impact

- Affected specs: 全部 Phase 1 spec（v0.23.0~v0.74.0）
- Affected code: 全部 61 crate + ci + docs/（修复模式：仅在发现问题时修改）
- 审计为只读 + 修复：发现问题 → 修复 → 回归验证

## ADDED Requirements

### Requirement: Phase 1 全面测试与验证

系统 SHALL 对 Phase 1 全部交付物执行系统性验证，覆盖构建、测试、代码质量、目录结构、文档分类、合规性六大维度。

#### Scenario: 全量构建通过
- **WHEN** 执行 `cargo build --workspace`
- **THEN** 全部 61 个 crate + ci 编译成功

#### Scenario: 全量测试通过
- **WHEN** 执行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
- **AND** 执行 `cargo test -p eneros-hal --features mock`
- **THEN** 全部单元测试通过，0 失败

#### Scenario: Phase 1 新增 crate 交叉编译通过
- **WHEN** 对每个 Phase 1 新增 crate 执行 `cargo build -p <crate> --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
- **THEN** 全部约 44 个 Phase 1 新增 crate 交叉编译成功

#### Scenario: 代码质量达标
- **WHEN** 执行 `cargo fmt --all -- --check`
- **AND** 执行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
- **THEN** 格式检查通过，clippy 无 warning

#### Scenario: no_std 合规
- **WHEN** 在 `crates/` 下搜索 `use std::` 模式
- **AND** 搜索 `panic!(` / `todo!(` / `unimplemented!(` 模式
- **THEN** `use std::` 零匹配；`panic!`/`todo!`/`unimplemented!` 仅在 test 模块或显式说明中存在

#### Scenario: 目录结构合规
- **WHEN** 执行 §2.4 校验清单 C1~C15
- **THEN** 全部 15 项通过

#### Scenario: 文档分类合规
- **WHEN** 检查 `docs/` 根目录
- **THEN** 无平面化 `.md` 文件（除 `README.md` 索引外），全部在 `docs/<topic>/` 子目录（含 Phase 1 新增 `docs/ai/`、`docs/agents/`、`docs/protocols/`、`docs/security/`）

#### Scenario: .gitignore 覆盖
- **WHEN** 检查 `.gitignore`
- **THEN** 覆盖 `target/`、`build/`、`*.elf`、`*.bin`、`*.dtb`、`*.img`、`qemu-output/`、IDE 缓存、密钥、`*.gguf`（模型文件）、`*.log`

### Requirement: Phase 1 出口标准验证

系统 SHALL 逐项验证 Phase 1 三大出口标准的主机侧达成情况。

#### Scenario: 出口标准 1 — autonomous 运行
- **WHEN** 检查 v0.74.0 MvpOrchestrator 端到端 24h 自治场景测试
- **THEN** 通过 MvpOrchestrator tick 24h 模拟测试（96 个 15min 时段），无 Panic、无 OOM、状态机正确流转

#### Scenario: 出口标准 2 — 双脑链路 < 2s
- **WHEN** 检查 v0.71.0 DualBrainEngine + v0.70.0 RealtimePathEngine 协作
- **THEN** 主机侧逻辑验证通过：LLM Intent → Solver 求解 → 控制命令下发完整链路在 Mock 环境下时序正确（QEMU 实测延后 Phase 2）

#### Scenario: 出口标准 3 — 比传统 EMS 收益 ≥ 10%
- **WHEN** 检查 v0.74.0 RevenueComparator 双脑 vs TraditionalEms 对比测试
- **THEN** 在典型峰谷电价场景下，双脑收益 ≥ 传统 EMS 收益 × 1.10（improvement_pct ≥ 10.0）

### Requirement: 问题修复与回归验证

系统 SHALL 对审计中发现的问题执行修复，并通过回归测试验证修复有效。

#### Scenario: 编译错误修复
- **WHEN** 发现 crate 编译失败
- **THEN** 修复源代码使编译通过，并重新执行 `cargo build --workspace` 确认无回归

#### Scenario: 测试失败修复
- **WHEN** 发现单元测试失败
- **THEN** 修复源代码使测试通过，并重新执行 `cargo test --workspace` 确认无回归

#### Scenario: fmt/clippy 违规修复
- **WHEN** 发现 `cargo fmt --check` 或 `cargo clippy` 违规
- **THEN** 执行 `cargo fmt` / 修复 clippy warning，并重新执行验证确认通过

#### Scenario: no_std 违规修复
- **WHEN** 发现 `use std::*` / `panic!` / `todo!` / `unimplemented!` 违规
- **THEN** 替换为 `alloc::*` / `core::*` / `heapless::*`，并重新执行搜索确认零匹配

#### Scenario: 目录结构违规修复
- **WHEN** 发现 crate 直接放根目录 / 文档平面化 / path 引用错误
- **THEN** 迁移至正确位置，更新 Cargo.toml path 引用，并重新执行 §2.4 校验确认通过

## MODIFIED Requirements

无修改。本次为审计 + 修复，不修改既有需求。

## REMOVED Requirements

无移除。
