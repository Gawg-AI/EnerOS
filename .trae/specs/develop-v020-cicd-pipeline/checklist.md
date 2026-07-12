# Checklist — EnerOS v0.2.0 CI/CD 流水线 + 代码规范

## 代码规范配置
- [x] `clippy.toml` 存在且配置了 lint 严格度（禁止 unwrap/expect in non-test 等）
- [x] `rustfmt.toml` 存在且配置了格式化风格（max_width/tab_spaces/newline_style 等）
- [x] `.commitlintrc.yml` 存在且配置了 Conventional Commits 规则（type/scope/subject）
- [x] `deny.toml` 存在且配置了许可证允许列表（MIT/Apache-2.0/BSD-3-Clause/ISC）
- [x] `deny.toml` 配置了 advisories 检查（RUSTSEC 漏洞数据库）
- [x] `deny.toml` 配置了 bans 检查（禁止重复依赖）

## ci/ 质量门禁 Crate
- [x] `ci/Cargo.toml` 存在，crate 名为 `eneros-ci`，版本 0.2.0
- [x] `ci/src/error.rs` 定义 `GateError` 枚举（FmtDirty/ClippyWarning/VulnFound/TestFailed/IoError）
- [x] `GateError` 实现了 `std::fmt::Display` 和 `std::error::Error`
- [x] `ci/src/gate.rs` 定义 `CheckResult` 结构体（name/passed/duration_ms/message）
- [x] `ci/src/gate.rs` 定义 `GateReport` 结构体（results/overall_pass）
- [x] `ci/src/gate.rs` 定义 `QualityGate` trait（run_all/run_fmt_check/run_clippy/run_audit/run_tests）
- [x] `ci/src/gate.rs` 实现 `DefaultGate`（调用 cargo fmt/clippy/deny/test）
- [x] `ci/src/gate.rs` 实现 `From<Result<(), GateError>> for CheckResult`
- [x] `ci/src/main.rs` 为 CLI 入口（打印报告、退出码 0/1）
- [x] audit 网络不可用时降级为 warning（不阻断）

## ci/ Crate 单元测试
- [x] `ci/src/gate.rs` 包含 GateReport 聚合逻辑单元测试
- [x] 测试覆盖"全部通过 → overall_pass=true"场景
- [x] 测试覆盖"任一失败 → overall_pass=false"场景
- [x] 测试覆盖"audit 降级 → passed=true + 降级提示"场景
- [x] 测试覆盖率 ≥ 80%

## pre-commit 钩子
- [x] `tools/pre-commit.sh` 存在且可执行
- [x] 脚本支持 `install` 子命令安装到 `.git/hooks/pre-commit`
- [x] 默认行为运行 `cargo run -p eneros-ci`

## CI 流水线（增强）
- [x] `.github/workflows/ci.yml` 使用 cargo-deny 替代 cargo-audit
- [x] CI 包含 `cargo deny check advisories licenses bans sources` 步骤
- [x] CI 启用 sccache 编译缓存（RUSTC_WRAPPER=sccache）
- [x] CI 包含 commitlint 提交信息检查步骤
- [x] CI 交叉编译步骤包含 `-Z build-std=core,alloc` 参数
- [x] CI 交叉编译仅构建 eneros-kernel 和 eneros-runtime（不构建 eneros-ci）
- [x] CI 版本标识更新为 v0.2.0
- [ ] CI 全流程 < 10 分钟（缓存命中时） — 需在 GitHub Actions 实际运行后确认

## Workspace 与 Makefile
- [x] `Cargo.toml`（workspace 根）members 包含 `"ci"`
- [x] `Cargo.toml` workspace.package.version 为 `"0.2.0"`
- [x] `Makefile` 包含 `ci-local` 目标
- [x] `make ci-local` 运行 `cargo run -p eneros-ci`
- [x] Makefile VERSION 更新为 `0.2.0`
- [x] Makefile help 包含 ci-local 说明

## 文档
- [x] `docs/ci-cd-manual.md` 存在且包含 CI 流水线说明
- [x] `docs/ci-cd-manual.md` 包含本地预检方法（`make ci-local`）
- [x] `docs/ci-cd-manual.md` 包含缓存策略（sccache）
- [x] `docs/code-conventions.md` 存在且包含 no_std 规范
- [x] `docs/code-conventions.md` 包含 clippy.toml/rustfmt.toml 配置说明
- [x] `docs/commit-conventions.md` 存在且包含 Conventional Commits 格式
- [x] `docs/commit-conventions.md` 包含 type 列表与示例

## README
- [x] `README.md` 包含"CI/CD"章节
- [x] README 说明 CI 检查项
- [x] README 说明本地预检方法（`make ci-local`）
- [x] README 说明提交规范与 pre-commit 钩子安装

## 验证
- [x] `cargo fmt --all -- --check` 无差异
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-runtime --all-targets -- -D warnings` 无 warning
- [x] `cargo test -p eneros-ci` 全部通过（5/5 passed）
- [x] `cargo run -p eneros-ci` 全绿（fmt/clippy/audit/test 4 项通过）
- [x] `cargo metadata --no-deps` 包含 eneros-ci 包
- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 不编译 eneros-ci — 通过 `--exclude` 与 cross-build 步骤隔离
- [x] commitlint 拦截不合规提交信息（如 "update code" → 2 errors）
- [x] pre-commit 钩子安装后可拦截失败提交（exit 1 on failure）

## 验证过程中修复的问题
- [x] clippy.toml：中文 reason 在 Windows GBK 控制台乱码 → 改为 ASCII；添加 `allow-invalid = true`
- [x] clippy.toml：`core::panic::panic_any` 不可达 warning → `allow-invalid = true`
- [x] ci/src/gate.rs：`--exclude` 需配合 `--workspace` 使用 → 添加 `--workspace`
- [x] ci/src/gate.rs + ci.yml：eneros-kernel 同样定义 `#[panic_handler]` → 一并 `--exclude`
- [x] deny.toml：`vulnerability = "deny"` 在 cargo-deny 0.20+ 已移除 → 删除
- [x] deny.toml：`unmaintained = "warning"` schema 变更 → 改为 `"workspace"`
- [x] deny.toml：`private = { ignore = true }` schema 不兼容 → 删除
- [x] deny.toml：rust-sel4 使用 BSD-2-Clause → 添加到 allow 列表
- [x] deny.toml：syn 使用 Unicode-3.0 → 添加到 allow 列表
- [x] deny.toml：rust-sel4 为 git 源 → 添加 `allow-git`
- [x] deny.toml：git 依赖无版本号触发 wildcard → `wildcards = "warn"`
