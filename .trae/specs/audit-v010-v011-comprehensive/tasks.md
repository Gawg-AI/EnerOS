# Tasks — EnerOS v0.1.0~v0.11.0 全面检查与修复

> **范围**：v0.1.0~v0.11.0 已交付的全部 10 个 crate
> **合规性**：蓝图 §43.1（no_std）、§七（CI/CD）、§1.3（收工检查）
> **原则**：Karpathy 四原则——先思考、简洁优先、外科手术式修改、目标驱动
> **审计日期**：2026-07-12

---

## 检查任务

- [x] Task 1: 代码格式检查（cargo fmt）
  - [x] SubTask 1.1: 执行 `cargo fmt --all -- --check`，记录结果 — **PASS**
  - [x] SubTask 1.2: 若失败，执行 `cargo fmt --all` 修复，再次验证 — **无需修复**

- [x] Task 2: Clippy lint 检查
  - [x] SubTask 2.1: 执行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` — **PASS**
  - [x] SubTask 2.2: 执行 `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` — **PASS**
  - [x] SubTask 2.3: 若有 warning，修复后重新验证 — **无需修复**

- [x] Task 3: 依赖安全与合规检查（cargo-deny）
  - [x] SubTask 3.1: 执行 `cargo deny check advisories licenses bans sources` — **本地未安装 cargo-deny，需在 CI 中验证**
  - [x] SubTask 3.2: 若有违规，更新 Cargo.lock 或 deny.toml 后重新验证 — **deny.toml 已配置，待 CI 执行**

> **注**：cargo-deny 本地编译安装超时（Windows 从源码编译耗时过长）。
> deny.toml 配置已就绪，CI 流水线会通过 `cargo install cargo-deny --locked || true` 安装并执行。

- [x] Task 4: 单元测试与集成测试
  - [x] SubTask 4.1: 执行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` — **120 个测试全部通过**
  - [x] SubTask 4.2: 执行 `cargo test -p eneros-hal --features mock` — **23 个测试全部通过**
  - [x] SubTask 4.3: 记录各 crate 测试数量，确认无失败 — **board:7, ci:5, hal:11+12, heap:27, mm:44, runtime:11, sel4-sys:6, user-heap:9**

- [x] Task 5: 交叉编译验证（aarch64-unknown-none）
  - [x] SubTask 5.1: 交叉编译 eneros-kernel — **PASS（修复后无 warning）**
  - [x] SubTask 5.2: 交叉编译 eneros-runtime — **PASS**
  - [x] SubTask 5.3: 交叉编译 eneros-board — **PASS**
  - [x] SubTask 5.4: 交叉编译 eneros-sel4-sys — **PASS**
  - [x] SubTask 5.5: 交叉编译 eneros-hello — **PASS**
  - [x] SubTask 5.6: 交叉编译 eneros-hal — **PASS**
  - [x] SubTask 5.7: 交叉编译 eneros-mm — **PASS**
  - [x] SubTask 5.8: 交叉编译 eneros-heap — **PASS**
  - [x] SubTask 5.9: 交叉编译 eneros-user-heap — **PASS**
  - [x] SubTask 5.10: 交叉编译 eneros-ci — **N/A（ci 是 host-side 工具，不交叉编译）**

- [x] Task 6: no_std 合规性检查
  - [x] SubTask 6.1: 搜索全工作区源码中的 `use std::` 引用 — **仅 ci crate（host 工具）和 test 模块内使用，合规**
  - [x] SubTask 6.2: 确认所有 crate 的 lib.rs 含 `#![cfg_attr(not(test), no_std)]` 或 `#![no_std]` — **全部确认**
  - [x] SubTask 6.3: 确认 kernel/hello 含 `#![no_std]` + `#![no_main]` — **kernel 是库 crate 无需 no_main；hello 已确认**

- [x] Task 7: 版本号一致性检查
  - [x] SubTask 7.1: 确认 workspace.package.version = "0.11.0" — **PASS**
  - [x] SubTask 7.2: 确认 Makefile VERSION = 0.11.0 — **PASS**
  - [x] SubTask 7.3: 确认 ci.yml 版本标识 v0.11.0 — **PASS**
  - [x] SubTask 7.4: 确认 ci/src/gate.rs 注释含 v0.11.0 — **PASS（第 98、141 行）**
  - [x] SubTask 7.5: 记录各 crate 版本号 — **ci=0.2.0, mm=0.8.0, heap=0.10.0, user/heap=0.11.0, 其他=workspace 0.11.0**

- [x] Task 8: 文档完整性检查
  - [x] SubTask 8.1: 确认 docs/ 目录含各版本核心文档 — **全部存在（22 个文档）**
  - [x] SubTask 8.2: 确认各 spec 目录的 tasks.md/checklist.md 已勾选 — **v0.3.0~v0.11.0 全部勾选；v0.1.0 有 9 项 WSL2 验证未勾选；v0.2.0 有 1 项 CI 时间验证未勾选**

- [x] Task 9: 工作区整洁检查
  - [x] SubTask 9.1: 执行 `git status` — **无垃圾文件被追踪**
  - [x] SubTask 9.2: 确认 `.gitignore` 覆盖所有必要模式 — **覆盖完整（target/、build/、*.elf、*.dtb、.idea/、.vscode/ 等）**
  - [x] SubTask 9.3: 确认无 Cargo.lock 冲突或异常 — **正常**

- [x] Task 10: 修复发现的问题
  - [x] SubTask 10.1: 汇总所有检查中发现的问题 — **1 个问题：kernel lang_items warning**
  - [x] SubTask 10.2: 对每个问题进行外科手术式修复 — **已修复：添加 `#![allow(internal_features)]`**
  - [x] SubTask 10.3: 修复后重新运行相关检查验证 — **交叉编译重新验证通过，无 warning**

---

## 验证任务

- [x] Task 11: 最终回归验证
  - [x] SubTask 11.1: 重新运行 Task 1-4 的全部检查，确认全绿 — **fmt/clippy/test 全部 PASS**
  - [x] SubTask 11.2: 重新运行 Task 5 的交叉编译，确认全绿 — **9 个 crate 全部 PASS，无 warning**
  - [x] SubTask 11.3: 生成最终检查报告 — **见 checklist.md**

---

# Task Dependencies

- Task 1-9 可并行执行（互不依赖）
- Task 10（修复）依赖 Task 1-9 的检查结果
- Task 11（最终验证）依赖 Task 10 完成

**并行机会**：Task 1-9 应尽可能并行执行以加快审计速度。

---

# 审计总结

## 检查结果汇总

| 检查项 | 结果 | 备注 |
|--------|------|------|
| cargo fmt | ✅ PASS | 无格式违规 |
| cargo clippy（workspace） | ✅ PASS | 无 warning |
| cargo clippy（hal mock） | ✅ PASS | 无 warning |
| cargo-deny | ⏳ PENDING | 本地未安装，需 CI 验证 |
| cargo test（workspace） | ✅ PASS | 120 个测试通过 |
| cargo test（hal mock） | ✅ PASS | 23 个测试通过 |
| 交叉编译（9 crate） | ✅ PASS | 全部通过，无 warning |
| no_std 合规性 | ✅ PASS | 全部合规 |
| 版本号一致性 | ✅ PASS | 全部一致 |
| 文档完整性 | ✅ PASS | 22 个文档全部存在 |
| spec 目录勾选 | ✅ PASS | v0.3.0~v0.11.0 全部勾选 |
| 工作区整洁 | ✅ PASS | .gitignore 完整，无垃圾文件 |

## 修复清单

| 问题 | 修复方式 | 影响文件 |
|------|----------|----------|
| kernel `lang_items` warning | 添加 `#![allow(internal_features)]` | kernel/src/lib.rs（+1 行） |

## 待外部环境验证项

| 项目 | 原因 | 验证方式 |
|------|------|----------|
| cargo-deny 检查 | 本地未安装 cargo-deny | CI 流水线自动执行 |
| v0.1.0 WSL2 验证（9 项） | 需 WSL2 环境 | 用户在 WSL2 中手动执行 |
| v0.2.0 CI 运行时间 | 需 GitHub Actions | 推送代码后 CI 自动验证 |
