# EnerOS 贡献指南

感谢你对 EnerOS 项目的关注。EnerOS 是一个面向电力与能源领域的原生智能体操作系统（AgentOS），所有代码在 GitHub 上以 MIT 协议开源。本文档说明如何参与项目开发，包括环境搭建、代码规范、提交流程与版本发布规则。

## 1. 贡献方式

欢迎通过以下方式参与：

- **提交 Issue**：报告缺陷、提出功能建议、讨论架构问题
- **提交 Pull Request**：修复缺陷、实现新功能、完善文档
- **代码审查**：参与 PR review，帮助提升代码质量
- **文档完善**：补充用户手册、开发者指南、示例代码
- **测试覆盖**：增加单元测试、集成测试、E2E 测试用例

在开始较大改动前，建议先在 Issue 中讨论方案，避免重复劳动或方向偏差。

## 2. 开发环境搭建

### 2.1 Rust 工具链

EnerOS 使用 Rust 1.75+ 编译，推荐使用 rustup 管理工具链：

```bash
# 安装 rustup（Linux/macOS）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Windows 用户请访问 https://rustup.rs 下载安装器

# 确认版本
rustc --version    # 需要 1.75.0 以上
cargo --version
```

安装组件：

```bash
rustup component add rustfmt clippy
```

### 2.2 系统依赖

部分 crate 依赖系统库：

- **Linux 实时/硬件相关**（eneros-os 的 HAL、seccomp、AF_PACKET）：需要 Linux 内核头文件、libseccomp-dev
- **跨平台核心库**（eneros-core、eneros-powerflow、eneros-analysis 等）：无额外系统依赖

Ubuntu/Debian 示例：

```bash
sudo apt install -y build-essential pkg-config libseccomp-dev libssl-dev
```

### 2.3 克隆与编译

```bash
git clone <repo-url> eneros && cd eneros

# 编译整个 workspace
cargo build --workspace

# 仅编译跨平台核心库（Windows/macOS 开发时推荐）
cargo build --workspace --exclude eneros-installer

# 运行全部测试
cargo test --workspace

# 启动开发模式（Linux，带热重载）
./deploy/scripts/dev.sh
```

> **Windows 用户注意**：`deploy/scripts/` 下的 `.sh` 脚本需在 Git Bash 或 WSL 下运行。原生 PowerShell 可直接使用 `cargo run --package eneros-api -- run --config eneros.toml` 等命令。

### 2.4 IDE 推荐

- **VS Code** + rust-analyzer 扩展（推荐）
- **RustRover** / CLion with Rust 插件
- 配置 `rust-analyzer.checkOnSave.command` 为 `clippy` 以获得实时检查

## 3. 代码规范

### 3.1 格式化

所有代码必须通过 `rustfmt` 格式化：

```bash
cargo fmt --all
```

提交前执行 `cargo fmt --all -- --check` 确认无格式问题。

### 3.2 Lint 检查

`clippy` 必须 0 警告：

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

如遇误报，使用 `#[allow(clippy::xxx)]` 并在注释中说明理由。不允许为消除警告而引入 `#[allow]` 的滥用。

### 3.3 Commit 风格

遵循 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/v1.0.0/) 规范：

```
<type>(<scope>): <subject>

<body>

<footer>
```

**type** 取值：

| type | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | 缺陷修复 |
| `docs` | 文档变更 |
| `style` | 代码格式（不影响功能） |
| `refactor` | 重构（不新增功能、不修复缺陷） |
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建/工具链/依赖等杂项 |
| `ci` | CI 配置变更 |

**scope** 为受影响的 crate 或模块名，如 `plugin`、`agent`、`os`、`docs`。

示例：

```
feat(plugin): 新增 plugin-daemon 进程隔离模式

实现 plugin-daemon 独立进程加载插件，通过 IPC 通道与主进程通信，
崩溃隔离不影响主进程。Inline 模式保留向后兼容。

Closes #142
```

### 3.4 代码风格要点

- 公开 API 必须有文档注释（`///`），说明用途、参数、返回值
- 模块级注释（`//!`）说明模块职责
- 错误处理使用 `thiserror` 定义错误类型，避免 `unwrap()`/`expect()` 出现在非测试代码中
- 异步代码使用 `tokio` runtime，trait 异步方法使用 `async-trait`
- 并发数据结构优先使用 `parking_lot::RwLock` / `dashmap`，避免 `std::sync::Mutex` 长时间持锁

## 4. 分支与 PR 流程

### 4.1 分支命名

- `feature/<简短描述>` — 新功能，如 `feature/plugin-daemon`
- `fix/<简短描述>` — 缺陷修复，如 `fix/iec104-reconnect`
- `docs/<简短描述>` — 文档，如 `docs/user-manual`
- `refactor/<简短描述>` — 重构

### 4.2 提交流程

1. 从 `main` 拉取最新代码创建分支
2. 完成开发并确保本地通过全部检查（见 4.3）
3. 推送分支并向 `main` 提交 Pull Request
4. 等待 CI 检查通过
5. 至少一名维护者 review 通过后合并
6. 合并采用 GitHub API 方式（见项目规则）

### 4.3 CI 检查项

PR 必须通过以下检查：

- `cargo fmt --all -- --check` — 格式检查
- `cargo clippy --workspace --all-targets -- -D warnings` — Lint 检查
- `cargo build --workspace` — 编译通过
- `cargo test --workspace` — 全部测试通过
- `cargo doc --workspace --no-deps` — 文档构建无警告

CI 配置见 `.github/workflows/ci.yml`。

### 4.4 PR 描述模板

```
## 变更说明
<!-- 简述本次变更的目的和内容 -->

## 变更类型
- [ ] 新功能（feat）
- [ ] 缺陷修复（fix）
- [ ] 文档（docs）
- [ ] 重构（refactor）
- [ ] 其他

## 关联 Issue
<!-- 如 Closes #123 -->

## 验证方式
<!-- 说明如何验证本次变更 -->

## 检查清单
- [ ] 代码通过 cargo fmt
- [ ] clippy 0 警告
- [ ] 新增/修改的代码有对应测试
- [ ] CHANGELOG.md 已更新（如适用）
```

## 5. 测试要求

### 5.1 测试分层

| 层级 | 位置 | 运行命令 | 说明 |
|------|------|----------|------|
| 单元测试 | `src/*.rs` 内 `#[cfg(test)] mod tests` | `cargo test -p <crate>` | 测试单个函数/模块逻辑 |
| 集成测试 | `crates/<name>/tests/*.rs` | `cargo test -p <crate> --test <name>` | 测试 crate 间集成 |
| E2E 测试 | `crates/<name>/tests/e2e_*.rs` | `cargo test --test e2e_*` | 端到端场景验证 |
| OS 测试 | `os/tests/*.rs` | `cargo test -p eneros-os-tests` | 启动/引导等 OS 级测试 |

### 5.2 测试规范

- 新增功能必须附带单元测试
- Bug 修复必须附带回归测试
- 测试函数命名清晰，表达被测行为：`test_protocol_plugin_returns_custom_type`
- 使用 `#[tokio::test]` 运行异步测试
- 避免测试间共享状态，每个测试独立

### 5.3 运行测试

```bash
# 全部测试
cargo test --workspace

# 排除 Linux 专用 crate（跨平台开发时）
cargo test --workspace --exclude eneros-installer

# 单个 crate
cargo test -p eneros-plugin

# 显示输出
cargo test -- --nocapture

# 仅运行匹配名称的测试
cargo test -p eneros-plugin signature
```

## 6. 版本发布流程

EnerOS 遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)：

- **MAJOR**：不兼容的 API 变更
- **MINOR**：向后兼容的功能新增
- **PATCH**：向后兼容的缺陷修复

### 6.1 变更记录规则

每次代码变更提交前，必须在 `CHANGELOG.md` 中对应版本节记录：

1. 在 `CHANGELOG.md` 顶部新增版本节（如尚未存在）
2. 按 `Added` / `Changed` / `Fixed` / `Removed` 分类记录变更
3. 每条记录说明变更内容、涉及的 crate/文件、影响

### 6.2 路线图管理

- `ROADMAP.md` 记录未来版本规划
- 新版本发布时，将 `ROADMAP.md` 中已完成项移至 `CHANGELOG.md`
- 每个规划项标注代码现状依据和验收标准

### 6.3 发布步骤

1. 确认 `CHANGELOG.md` 已记录全部变更
2. 更新 `ROADMAP.md`，标记已完成项
3. 更新 `README.md` 中的版本号与变更说明
4. 创建版本 tag：`git tag v0.x.y`
5. 通过 GitHub API 创建 Release

## 7. Issue 报告指南

### 7.1 Bug Report

提交 Bug 时请包含：

- **环境信息**：OS 版本、EnerOS 版本、Rust 版本
- **复现步骤**：最小可复现的操作序列
- **预期行为**：期望发生什么
- **实际行为**：实际发生了什么，附带完整错误日志
- **相关配置**：脱敏后的配置文件片段

### 7.2 Feature Request

提交功能建议时请包含：

- **使用场景**：解决什么问题，在什么电力场景下使用
- **方案描述**：建议的实现方式
- **替代方案**：考虑过的其他方案
- **影响范围**：涉及哪些 crate，是否影响现有 API

## 8. 行为准则

参与 EnerOS 项目时请遵守以下原则：

- **尊重**：尊重每一位贡献者，不论经验水平
- **专业**：讨论聚焦技术与事实，避免人身攻击
- **协作**：积极响应 review 意见，提供建设性反馈
- **负责**：对自己的提交负责，及时修复 CI 失败和 review 指出的问题

## 9. 相关文档

- [部署运维指南](docs/deployment.md)
- [开发者指南](docs/developer-guide.md)
- [用户手册](docs/user-manual.md)
- [插件开发指南](docs/plugin-development.md)
- [架构决策记录](docs/adr/0001-record-architecture-decisions.md)
- [变更日志](CHANGELOG.md)
- [开发路线图](ROADMAP.md)
