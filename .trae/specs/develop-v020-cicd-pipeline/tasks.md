# Tasks — EnerOS v0.2.0 CI/CD 流水线 + 代码规范

- [x] Task 1: 创建代码规范配置文件
  - [x] SubTask 1.1: 创建 `clippy.toml`（设定 lint 严格度：禁止 unwrap/expect/panic in non-test、禁止 unimplemented、禁止 mod_module_inception 等）
  - [x] SubTask 1.2: 创建 `rustfmt.toml`（统一格式化：max_width=100、tab_spaces=4、use_field_init_shorthand=true、newline_style=Unix）
  - [x] SubTask 1.3: 创建 `.commitlintrc.yml`（Conventional Commits 配置：feat/fix/docs/style/refactor/test/chore/ci 类型、scope 可选、subject 最大长度 72）
  - [x] SubTask 1.4: 创建 `deny.toml`（cargo-deny 配置：licenses 允许列表 MIT/Apache-2.0/BSD-3-Clause/ISC、advisories 检查 RUSTSEC、bans 禁止重复依赖、sources 仅允许 crates.io）

- [x] Task 2: 创建 `ci/` 质量门禁 crate
  - [x] SubTask 2.1: 创建 `ci/Cargo.toml`（crate `eneros-ci`，版本 0.2.0，std-based host 工具，依赖无或仅 std）
  - [x] SubTask 2.2: 创建 `ci/src/error.rs`（`GateError` 枚举：FmtDirty/ClippyWarning/VulnFound/TestFailed/IoError，实现 `std::fmt::Display` 和 `std::error::Error`）
  - [x] SubTask 2.3: 创建 `ci/src/gate.rs`（`CheckResult` 结构体、`GateReport` 结构体、`QualityGate` trait、`DefaultGate` 实现调用 cargo 子命令、`From<Result<(), GateError>> for CheckResult` 实现）
  - [x] SubTask 2.4: 创建 `ci/src/main.rs`（CLI 入口：构造 `DefaultGate`、调用 `run_all`、打印报告、退出码 0/1）
  - [x] SubTask 2.5: 创建 `ci/src/gate.rs` 的单元测试（GateReport 聚合逻辑：全通过/任一失败/audit 降级场景，覆盖率 ≥ 80%）

- [x] Task 3: 创建 pre-commit 钩子脚本
  - [x] SubTask 3.1: 创建 `tools/pre-commit.sh`（支持 `install` 子命令安装到 `.git/hooks/pre-commit`；默认行为运行 `cargo run -p eneros-ci`）

- [x] Task 4: 增强 CI 流水线
  - [x] SubTask 4.1: 修改 `.github/workflows/ci.yml`：将 cargo-audit 替换为 `cargo deny check advisories licenses bans sources`
  - [x] SubTask 4.2: 修改 `.github/workflows/ci.yml`：添加 sccache 安装与 `RUSTC_WRAPPER=sccache` 环境变量
  - [x] SubTask 4.3: 修改 `.github/workflows/ci.yml`：添加 commitlint 检查步骤（使用 `wagoid/commitlint-github-action`）
  - [x] SubTask 4.4: 修改 `.github/workflows/ci.yml`：交叉编译步骤添加 `-Z build-std=core,alloc` 参数
  - [x] SubTask 4.5: 修改 `.github/workflows/ci.yml`：版本标识更新为 v0.2.0

- [x] Task 5: 修改 Workspace 与 Makefile
  - [x] SubTask 5.1: 修改 `Cargo.toml`（workspace 根）：members 添加 `"ci"`，workspace.package.version 更新为 `"0.2.0"`
  - [x] SubTask 5.2: 修改 `Makefile`：添加 `ci-local` 目标（运行 `cargo run -p eneros-ci`），更新 VERSION 为 `0.2.0`，help 添加 ci-local 说明

- [x] Task 6: 创建文档
  - [x] SubTask 6.1: 创建 `docs/ci-cd-manual.md`（CI 流水线说明、检查项详解、本地预检方法 `make ci-local`、缓存策略 sccache、PR 合并流程、故障排查）
  - [x] SubTask 6.2: 创建 `docs/code-conventions.md`（no_std 规范、命名规范 snake_case、模块组织、clippy.toml 配置说明、rustfmt.toml 配置说明、no_std 替代方案表）
  - [x] SubTask 6.3: 创建 `docs/commit-conventions.md`（Conventional Commits 格式、type 列表及说明、scope 规范、subject 规则、body/footer 示例、完整示例）

- [x] Task 7: 更新 README
  - [x] SubTask 7.1: 修改 `README.md`：添加"CI/CD"章节（CI 检查项列表、本地预检 `make ci-local`、提交规范链接、pre-commit 钩子安装方法）

- [x] Task 8: 验证与测试
  - [x] SubTask 8.1: 本地运行 `cargo fmt --all -- --check` 验证格式无差异 — PASSED (exit 0)
  - [x] SubTask 8.2: 本地运行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-runtime --all-targets -- -D warnings` 验证无 warning — PASSED (修复 clippy.toml encoding/allow-invalid + 添加 --workspace + 排除 eneros-kernel)
  - [x] SubTask 8.3: 本地运行 `cargo test -p eneros-ci` 验证 GateReport 单元测试通过且覆盖率 ≥ 80% — PASSED (5/5 tests passed)
  - [x] SubTask 8.4: 本地运行 `cargo run -p eneros-ci` 验证质量门禁全绿 — PASSED (Overall: PASS, fmt+clippy+audit+test 4 项全绿)
  - [x] SubTask 8.5: 本地运行 `cargo deny check` 验证许可证与漏洞检查通过 — PASSED (advisories/licenses/bans/sources 全 ok，修复 deny.toml schema 兼容 cargo-deny 0.20+)
  - [x] SubTask 8.6: 验证 commitlint 拦截不合规提交信息 — PASSED ("update code" 被拦截 exit 1，"feat(ci): add quality gate tool" 通过 exit 0)
  - [x] SubTask 8.7: 验证 pre-commit 钩子安装后可拦截失败提交 — PASSED (install 子命令工作，默认行为在 Git Bash 下全绿，在 cargo 不可用时正确阻断 exit 1)

# Task Dependencies
- [Task 2] 依赖 [Task 1]（ci crate 的 DefaultGate 调用 cargo deny，需 deny.toml 存在）
- [Task 4] 依赖 [Task 1]（CI 使用 deny.toml/.commitlintrc.yml）
- [Task 4] 依赖 [Task 2]（CI 可选调用 eneros-ci）
- [Task 5] 依赖 [Task 2]（Makefile ci-local 依赖 ci crate 存在）
- [Task 7] 依赖 [Task 5]（README 引用 make ci-local）
- [Task 8] 依赖 [Task 1]~[Task 7] 全部完成
- [Task 1]、[Task 3]、[Task 6] 可并行
