# EnerOS v0.2.0 — CI/CD 流水线 + 代码规范 Spec

## Why

v0.1.0 建立了基础工具链与一个最小 CI 骨架（fmt/clippy/test/交叉编译），但缺少质量护栏的完整闭环：无 clippy/rustfmt 配置文件、无 cargo-deny 许可证与漏洞双查、无提交规范 enforcement、无本地预检工具。v0.2.0 补齐这些短板，确保后续 20 个版本（v0.3.0 ~ v0.22.0）在持续演进中代码质量不退化，安全漏洞早发现，PR 必须全绿方可合并。

蓝图依据：`e:\eneros\蓝图\phase0.md` §v0.2.0。

## What Changes

- 新建 `clippy.toml`：clippy lint 严格度配置
- 新建 `rustfmt.toml`：代码格式化配置
- 新建 `deny.toml`：cargo-deny 配置（许可证 + 漏洞双查，替代 cargo-audit）
- 新建 `.commitlintrc.yml`：Conventional Commits 提交规范配置
- 新建 `ci/` crate（host-side std 工具，非 no_std）：质量门禁 Rust 实现，包含 `QualityGate` trait、`CheckResult`/`GateReport` 结构体、`DefaultGate` 默认实现、`GateError` 错误枚举
- 新建 `tools/pre-commit.sh`：本地 pre-commit 钩子脚本，调用 `ci` crate 执行本地预检
- 新建 `docs/ci-cd-manual.md`：CI/CD 使用手册
- 新建 `docs/code-conventions.md`：代码规范
- 新建 `docs/commit-conventions.md`：提交规范
- 修改 `.github/workflows/ci.yml`：增加 cargo-deny 步骤、sccache 缓存、commitlint 检查、build-std 交叉编译
- 修改 `Cargo.toml`（workspace 根）：添加 `ci` 成员
- 修改 `Makefile`：增加 `ci-local` 目标（本地运行质量门禁）
- 修改 `README.md`：增加 CI/CD 章节

### 关于 `ci/` crate 的 no_std 说明

蓝图 §43.1 要求"全项目所有 Rust 代码必须 no_std"，该规则覆盖三层：内核态/RTOS 态、Agent Runtime、LLM 封装层——均为目标侧（target）代码。`ci/` crate 是 host-side 开发工具（在 CI runner / 开发者 PC 上运行，非嵌入式目标），需要 `std::process::Command` 执行 cargo 子命令、`std::fs` 读取文件、`std::time` 计时。若强制 no_std 则无法实现其功能。因此 `ci/` crate 作为 host-side 工具例外使用 std，不纳入 no_std 合规范围，也不参与 `aarch64-unknown-none` 交叉编译（CI 中交叉编译步骤使用 `-p eneros-kernel` / `-p eneros-runtime` 精确指定包）。

## Impact

- **Affected specs**:
  - `develop-v010-toolchain`（v0.1.0）：ci.yml 将被增强，Makefile/Cargo.toml/README 将被修改
- **Affected code**:
  - `.github/workflows/ci.yml`（增强）
  - `Cargo.toml`（workspace 根，添加 ci 成员）
  `Makefile`（添加 ci-local 目标）
  - `README.md`（添加 CI/CD 章节）
  - 新建：`clippy.toml`、`rustfmt.toml`、`deny.toml`、`.commitlintrc.yml`、`ci/Cargo.toml`、`ci/src/gate.rs`、`ci/src/main.rs`、`ci/src/error.rs`、`tools/pre-commit.sh`、`docs/ci-cd-manual.md`、`docs/code-conventions.md`、`docs/commit-conventions.md`
- **依赖输入**:
  - `e:\eneros\蓝图\phase0.md` §v0.2.0（详细蓝图）
  - `e:\eneros\.trae\rules\记忆.md` §六（提交规范）、§七（CI/CD 规范）
  - `e:\eneros\.trae\specs\develop-v010-toolchain\spec.md`（v0.1.0 基线）
- **后续影响**: v0.3.0 起所有版本受本 CI 约束；CI 配置随工具链演进

## ADDED Requirements

### Requirement: Clippy 配置

系统 SHALL 提供 `clippy.toml` 配置文件，设定 clippy lint 严格度，确保 `-D warnings` 下的规则一致性。

#### Scenario: clippy 配置生效
- **WHEN** 执行 `cargo clippy --all-targets -- -D warnings`
- **THEN** clippy 读取 `clippy.toml` 配置
- **AND** 按配置的严格度执行 lint 检查

### Requirement: Rustfmt 配置

系统 SHALL 提供 `rustfmt.toml` 配置文件，统一代码格式化风格。

#### Scenario: 格式化一致
- **WHEN** 执行 `cargo fmt --all`
- **THEN** 所有 Rust 源文件按 `rustfmt.toml` 配置格式化
- **AND** `cargo fmt --all -- --check` 无差异

### Requirement: cargo-deny 配置

系统 SHALL 提供 `deny.toml` 配置文件，实现许可证合规与已知漏洞双查，替代 v0.1.0 的 cargo-audit。

#### Scenario: 许可证检查
- **WHEN** 执行 `cargo deny check licenses`
- **THEN** 检查所有依赖许可证是否在允许列表内
- **AND** 不允许的许可证导致检查失败

#### Scenario: 漏洞检查
- **WHEN** 执行 `cargo deny check advisories`
- **THEN** 检查依赖是否存在已知安全公告（RUSTSEC）
- **AND** 存在漏洞时检查失败

### Requirement: 提交规范配置

系统 SHALL 提供 `.commitlintrc.yml` 配置文件，基于 Conventional Commits 规范（蓝图 §六），enforce 提交信息格式。

#### Scenario: 合规提交
- **WHEN** 提交信息为 `feat(kernel/heap): v0.10.0 实现堆分配器`
- **THEN** commitlint 检查通过

#### Scenario: 不合规提交被拦截
- **WHEN** 提交信息为 `update code`
- **THEN** commitlint 检查失败
- **AND** 提示正确的提交格式

### Requirement: 质量门禁 Crate

系统 SHALL 提供 `ci/` crate（host-side std 工具），实现质量门禁接口，供本地预检和 CI 调用。

#### Scenario: QualityGate trait 可用
- **WHEN** 检查 `ci/src/gate.rs`
- **THEN** 定义 `QualityGate` trait，包含 `run_all`、`run_fmt_check`、`run_clippy`、`run_audit`、`run_tests` 方法
- **AND** 定义 `CheckResult` 结构体（name/passed/duration_ms/message）
- **AND** 定义 `GateReport` 结构体（results/overall_pass）
- **AND** 定义 `GateError` 枚举（FmtDirty/ClippyWarning/VulnFound/TestFailed/IoError）

#### Scenario: DefaultGate 实现
- **WHEN** 检查 `ci/src/gate.rs`
- **THEN** `DefaultGate` 实现 `QualityGate` trait
- **AND** `run_fmt_check` 调用 `cargo fmt -- --check`
- **AND** `run_clippy` 调用 `cargo clippy --all-targets -- -D warnings`
- **AND** `run_audit` 调用 `cargo deny check advisories licenses`
- **AND** `run_tests` 调用 `cargo test --workspace`

#### Scenario: 本地预检运行
- **WHEN** 执行 `cargo run -p eneros-ci`
- **THEN** 运行全部 4 项检查
- **AND** 输出 `GateReport`（每项检查名称/通过状态/耗时/失败信息）
- **AND** 全部通过时退出码 0，任一失败时退出码 1

#### Scenario: audit 网络降级
- **WHEN** `cargo deny check` 因网络不可用失败
- **THEN** 降级为 warning（不阻断）
- **AND** `GateReport` 中 audit 项标记为 passed=true，message 含降级提示

### Requirement: GateReport 聚合逻辑测试

系统 SHALL 为 `GateReport` 聚合逻辑提供单元测试，覆盖率 ≥ 80%。

#### Scenario: 全部通过
- **WHEN** 4 项 CheckResult 均为 passed=true
- **THEN** GateReport.overall_pass == true

#### Scenario: 任一失败
- **WHEN** 任一 CheckResult.passed == false
- **THEN** GateReport.overall_pass == false

### Requirement: pre-commit 钩子

系统 SHALL 提供 `tools/pre-commit.sh` 脚本，安装为 git pre-commit 钩子后在提交前自动运行质量门禁。

#### Scenario: 钩子安装
- **WHEN** 执行 `tools/pre-commit.sh install`
- **THEN** 在 `.git/hooks/pre-commit` 创建可执行钩子

#### Scenario: 提交前自动检查
- **WHEN** 开发者执行 `git commit`
- **THEN** 自动运行 `cargo run -p eneros-ci`
- **AND** 检查失败时阻止提交

### Requirement: sccache 编译缓存

系统 SHALL 在 CI 中启用 sccache 缓存编译产物，加速 CI 流程。

#### Scenario: sccache 生效
- **WHEN** CI 运行编译步骤
- **THEN** 使用 sccache 缓存编译中间产物
- **AND** 二次构建命中缓存加速

### Requirement: CI/CD 文档

系统 SHALL 提供三份文档：CI/CD 使用手册、代码规范、提交规范。

#### Scenario: CI/CD 使用手册
- **WHEN** 阅读 `docs/ci-cd-manual.md`
- **THEN** 包含 CI 流水线说明、检查项说明、本地预检方法、缓存策略

#### Scenario: 代码规范
- **WHEN** 阅读 `docs/code-conventions.md`
- **THEN** 包含 no_std 规范、命名规范、模块组织、clippy/rustfmt 配置说明

#### Scenario: 提交规范
- **WHEN** 阅读 `docs/commit-conventions.md`
- **THEN** 包含 Conventional Commits 格式、type 列表、scope 规范、示例

## MODIFIED Requirements

### Requirement: CI 流水线（增强）

v0.1.0 的 CI 基础骨架 SHALL 被增强为完整的质量门禁流水线。

#### Scenario: CI 检查项完整
- **WHEN** 检查 `.github/workflows/ci.yml`
- **THEN** 包含以下步骤：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo deny check advisories licenses bans sources`（替代 cargo-audit）
  - `cargo test --all`
  - 交叉编译 `cargo build -p eneros-kernel --target aarch64-unknown-none -Z build-std=core,alloc`
  - 交叉编译 `cargo build -p eneros-runtime --target aarch64-unknown-none -Z build-std=core,alloc`
  - commitlint 提交信息检查
  - sccache 编译缓存
  - workspace 整洁检查

#### Scenario: CI 性能达标
- **WHEN** CI 全流程执行
- **THEN** 总耗时 < 10 分钟（含缓存命中）

#### Scenario: PR 阻断
- **WHEN** PR 触发 CI 且任一检查失败
- **THEN** PR 被标记为不可合并
- **AND** GitHub Branch Protection 规则 enforce 全绿方可合并

### Requirement: Workspace 结构（增强）

v0.1.0 的 workspace SHALL 新增 `ci` 成员。

#### Scenario: ci crate 在 workspace 中
- **WHEN** 执行 `cargo metadata --no-deps`
- **THEN** `packages` 数组包含 `eneros-ci` 包
- **AND** 版本为 `0.2.0`

#### Scenario: ci crate 不参与交叉编译
- **WHEN** 执行 `cargo build -p eneros-kernel --target aarch64-unknown-none`
- **THEN** 不编译 `eneros-ci` crate
- **AND** 无交叉编译错误

### Requirement: Makefile（增强）

v0.1.0 的 Makefile SHALL 新增 `ci-local` 目标。

#### Scenario: make ci-local
- **WHEN** 执行 `make ci-local`
- **THEN** 运行 `cargo run -p eneros-ci`
- **AND** 输出质量门禁报告
- **AND** 全绿时退出码 0

### Requirement: README（增强）

v0.1.0 的 README SHALL 新增 CI/CD 章节。

#### Scenario: README 包含 CI/CD 说明
- **WHEN** 阅读 `README.md`
- **THEN** 包含"CI/CD"章节
- **AND** 说明 CI 检查项、本地预检方法（`make ci-local`）、提交规范
