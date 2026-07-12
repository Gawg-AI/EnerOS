# EnerOS CI/CD 使用手册

> 版本：v0.2.0  
> 适用范围：EnerOS 全 workspace（kernel / runtime / ci）  
> 蓝图依据：`蓝图/phase0.md` §v0.2.0、`记忆.md` §七 CI/CD 规范

---

## 概述

EnerOS 的 CI/CD 体系以"质量护栏"为核心目标：在每一次 push 与 PR 中自动执行格式、lint、安全、测试、交叉编译与提交规范检查，确保 v0.3.0 起的 20 个版本在持续演进中代码质量不退化、安全漏洞早发现、PR 必须全绿方可合并。

质量护栏由四部分组成：

1. **CI 流水线**（GitHub Actions）：远端自动检查
2. **本地预检工具**（`ci/` crate + `make ci-local`）：提交前本地预演
3. **pre-commit 钩子**（`tools/pre-commit.sh`）：提交时自动拦截
4. **配置文件**（`clippy.toml` / `rustfmt.toml` / `deny.toml` / `.commitlintrc.yml`）：规则单一来源

---

## CI 流水线

### 触发条件

| 事件 | 目标分支 | 说明 |
|------|---------|------|
| `push` | `main`、`develop` | 直接推送触发 |
| `pull_request` | `main`、`develop` | PR 触发，全绿方可合并 |

### 检查项

CI 流水线（`.github/workflows/ci.yml`）包含以下检查，任一失败即阻断合并：

| # | 检查项 | 命令 | 说明 |
|---|--------|------|------|
| 1 | 代码格式检查 | `cargo fmt --all -- --check` | 依据 `rustfmt.toml` |
| 2 | Clippy lint 检查 | `cargo clippy --all-targets -- -D warnings` | 依据 `clippy.toml`，warning 即失败 |
| 3 | 安全与许可证检查 | `cargo deny check advisories licenses bans sources` | 替代 cargo-audit，双查漏洞与许可证 |
| 4 | 单元测试 | `cargo test --all` | 全 workspace 测试 |
| 5 | 交叉编译（kernel） | `cargo build -p eneros-kernel --target aarch64-unknown-none -Z build-std=core,alloc` | 验证 no_std + build-std |
| 6 | 交叉编译（runtime） | `cargo build -p eneros-runtime --target aarch64-unknown-none -Z build-std=core,alloc` | 验证 no_std + build-std |
| 7 | 提交规范检查 | commitlint | 依据 `.commitlintrc.yml`，Conventional Commits |
| 8 | Workspace 整洁检查 | 校验 `.gitignore` 覆盖 `target/` `build/` `*.elf` `*.dtb` | 防止垃圾文件入仓 |

> **注意**：`ci/` crate 是 host-side 工具，使用 std，**不参与** `aarch64-unknown-none` 交叉编译。交叉编译步骤用 `-p eneros-kernel` / `-p eneros-runtime` 精确指定包。

### 编译缓存

CI 启用 **sccache** 缓存编译中间产物，二次构建命中缓存可显著加速。同时缓存 cargo registry（`~/.cargo/registry`、`~/.cargo/git`）。

---

## 本地预检

为避免推送后才发现 CI 失败，请在提交前运行本地预检。

### 方式一：make ci-local

```bash
make ci-local
```

等价于 `cargo run -p eneros-ci`，运行全部 4 项检查（fmt / clippy / deny / test），输出 `GateReport`：

- 每项检查的名称、通过状态、耗时、失败信息
- 全部通过退出码 0，任一失败退出码 1

### 方式二：直接调用 ci crate

```bash
cargo run -p eneros-ci
```

### 方式三：pre-commit 钩子

安装钩子后，每次 `git commit` 自动运行质量门禁，失败则阻止提交：

```bash
# 安装钩子（写入 .git/hooks/pre-commit）
tools/pre-commit.sh install

# 手动运行（不安装）
tools/pre-commit.sh
```

> `ci/` crate 的 `cargo deny check` 在网络不可用时降级为 warning（不阻断），`GateReport` 中 audit 项标记 `passed=true` 并附降级提示。

---

## 缓存策略

| 缓存类型 | 作用域 | 说明 |
|---------|--------|------|
| sccache | 编译中间产物 | CI 与本地均可启用，命中后跳过重编译 |
| cargo registry | `~/.cargo/registry`、`~/.cargo/git` | 依赖下载缓存，key 基于 `Cargo.lock` 哈希 |
| target 目录 | workspace `target/` | CI 按 job 隔离，key 区分交叉编译与普通构建 |

本地启用 sccache：

```bash
cargo install sccache
export RUSTC_WRAPPER=sccache
```

---

## PR 合并流程

```
1. 创建分支  feature/v0.XX.0-<简述>（禁止在 main 直接开发）
   ↓
2. 本地预检  make ci-local  （或 tools/pre-commit.sh）
   ↓
3. 推送 + 发起 PR  目标分支 develop
   ↓
4. CI 全绿  所有检查项通过
   ↓
5. Code Review  至少一人审查通过
   ↓
6. 合并到 develop  Squash / Rebase 合并
   ↓
7. 定期合并  develop → main（发版时）
```

**分支保护规则**：`main` / `develop` 受保护，必须 CI 全绿 + Review 通过方可合并，禁止直接 push。

---

## 故障排查

### CI 失败怎么办？

1. 在 PR 页面查看失败的 job 与日志
2. 本地复现：`make ci-local` 或单独运行失败项（如 `cargo clippy --all-targets -- -D warnings`）
3. 修复后重新推送，CI 自动重跑

### Clippy warning 如何修复？

- CI 使用 `-D warnings`，任何 warning 都会失败
- 按 clippy 提示的 lint 名称修复，必要时在 `clippy.toml` 调整阈值
- **禁止**用 `#[allow(...)]` 绕过，除非有充分理由并在 PR 中说明

### cargo-deny 许可证不合规如何处理？

- 查看失败日志中被拒的依赖及其许可证
- 若许可证本身可接受，在 `deny.toml` 的 `allow` 列表中补充
- 若许可证不可接受，替换该依赖

### commitlint 拦截如何修正提交信息？

- 提交信息必须符合 `<type>(<scope>): <subject>` 格式（详见 `docs/commit-conventions.md`）
- 若已提交但未推送，修正最近一次提交信息：

```bash
git commit --amend -m "feat(kernel/heap): 实现堆分配器"
```

- 若已推送，修正后需 `git push --force-with-lease`（**禁止** force push 到 main/develop）

### 交叉编译失败（build-std）？

- 确认使用 nightly 工具链（`rustup show`，应与 `rust-toolchain.toml` 一致）
- 确认安装了 `rust-src` component：`rustup component add rust-src --toolchain nightly`
- 确认目标代码无 `use std::*`（no_std 合规，见 `docs/code-conventions.md`）

---

## 性能要求

- CI 全流程总耗时 **< 10 分钟**（含缓存命中）
- 启用 sccache + cargo registry 缓存降低重复编译开销
- 交叉编译与单元测试 job 并行执行（均依赖 quality-check 通过后并发）
- 若 CI 持续超时，检查是否引入了重量级依赖或未利用缓存
