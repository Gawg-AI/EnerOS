# Phase 0 全面审计 Spec

## Why

Phase 0（v0.1.0~v0.22.0）共 25 个版本已全部开发完成，但在进入 Phase 1 之前需要对全部 Phase 0 功能进行一次系统性审计与测试，确保：
- 所有 crate 编译通过（host + aarch64 交叉编译）
- 所有单元测试通过（无回归）
- 代码质量达标（fmt + clippy + no_std 合规）
- 目录结构与规范一致（§2.4 校验清单 C1~C15）
- 四大出口标准在主机侧全部达成

## What Changes

本次为**只读审计**，不新增功能代码，仅在发现问题时创建修复任务：

- 执行 workspace 全量构建与测试
- 执行全部 17 个 crate 的 aarch64 交叉编译
- 验证 no_std 合规性（全项目无 `use std::*`）
- 验证目录结构（§2.4 C1~C15）
- 运行 CI 质量门禁（eneros-ci）
- 验证文档分类（docs/<topic>/）
- 验证 .gitignore 覆盖
- 汇总 Phase 0 四大出口标准达成情况

## Impact

- Affected specs: 全部 Phase 0 spec（v0.1.0~v0.22.0）
- Affected code: 全部 17 个 crate + ci + docs/
- 审计为只读操作，不修改源代码（除非发现阻塞性问题）

## ADDED Requirements

### Requirement: Phase 0 全面审计

系统 SHALL 对 Phase 0 全部交付物执行系统性验证，覆盖构建、测试、代码质量、目录结构、文档分类、合规性六大维度。

#### Scenario: 全量构建通过
- **WHEN** 执行 `cargo build --workspace`
- **THEN** 全部 17 个 crate + ci 编译成功

#### Scenario: 全量测试通过
- **WHEN** 执行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
- **AND** 执行 `cargo test -p eneros-hal --features mock`
- **THEN** 全部单元测试通过，0 失败

#### Scenario: 交叉编译通过
- **WHEN** 对每个 crate 执行 `cargo build -p <crate> --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
- **THEN** 全部 17 个 crate 交叉编译成功

#### Scenario: 代码质量达标
- **WHEN** 执行 `cargo fmt --all -- --check`
- **AND** 执行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
- **THEN** 格式检查通过，clippy 无 warning

#### Scenario: no_std 合规
- **WHEN** 在 `crates/` 下搜索 `use std::` 模式
- **THEN** 零匹配（全项目 no_std，蓝图 §43.1）

#### Scenario: 目录结构合规
- **WHEN** 执行 §2.4 校验清单 C1~C15
- **THEN** 全部 15 项通过

#### Scenario: 文档分类合规
- **WHEN** 检查 `docs/` 根目录
- **THEN** 无平面化 `.md` 文件（除 `README.md` 索引外），全部在 `docs/<topic>/` 子目录

#### Scenario: .gitignore 覆盖
- **WHEN** 检查 `.gitignore`
- **THEN** 覆盖 `target/`、`build/`、`*.elf`、`*.bin`、`*.dtb`、`*.img`、`qemu-output/`、IDE 缓存、密钥

### Requirement: Phase 0 出口标准验证

系统 SHALL 逐项验证 Phase 0 四大出口标准的主机侧达成情况。

#### Scenario: 出口标准 1 — 双分区隔离
- **WHEN** 检查 v0.8.0 页表隔离 + v0.9.0 物理内存隔离 + v0.9.1 合规 + v0.21.0 共享内存授权
- **THEN** 全部通过主机测试

#### Scenario: 出口标准 2 — 实时性能
- **WHEN** 检查 v0.19.0 分区调度抖动 + v0.12.0 时钟精度 + v0.6.0 中断响应 + v0.22.0 命令通道延迟 + v0.18.0 线程切换
- **THEN** 主机侧逻辑验证通过（QEMU 实测延后）

#### Scenario: 出口标准 3 — 多核启动 + RTOS 核绑定
- **WHEN** 检查 v0.15.0 SMP 启动 + v0.16.0 核绑定 + v0.17.0 内存一致性
- **THEN** 全部通过主机测试

#### Scenario: 出口标准 4 — 基础 OS 服务就绪
- **WHEN** 检查 10 项 OS 服务（heap/user-heap/time/watchdog/panic/smp/sched/ipc/spsc-ring/controlbus）
- **THEN** 全部就绪，单元测试通过

## MODIFIED Requirements

无修改。本次为只读审计，不修改既有需求。

## REMOVED Requirements

无移除。
