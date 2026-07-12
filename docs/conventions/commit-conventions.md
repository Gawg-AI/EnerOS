# EnerOS 提交规范

> 版本：v0.2.0  
> 适用范围：EnerOS 仓库所有 git 提交  
> 蓝图依据：`记忆.md` §六、`.commitlintrc.yml`

---

## 概述

EnerOS 采用 [Conventional Commits](https://www.conventionalcommits.org/) 规范，通过 commitlint 在 CI 与 pre-commit 钩子中自动 enforce。统一的提交信息格式便于：

- 自动生成版本变更日志
- 追溯功能 / 修复的引入版本
- 关联 issue 与 PR

---

## 格式

```
<type>(<scope>): <subject>

<body>

<footer>
```

- `type`：必填，提交类型
- `scope`：选填，影响范围（crate 名或子系统名）
- `subject`：必填，简短描述
- `body`：选填，详细说明
- `footer`：选填，BREAKING CHANGE 或 issue 关联

---

## Type 列表

| type | 说明 | 示例 |
|------|------|------|
| `feat` | 新功能（对应版本交付物） | `feat(kernel/heap): v0.10.0 实现堆分配器` |
| `fix` | 修复缺陷 | `fix(runtime/boot): 修复启动崩溃` |
| `docs` | 文档变更 | `docs(ci): 更新 CI 手册` |
| `style` | 代码格式（不影响功能） | `style: cargo fmt` |
| `refactor` | 重构（非新功能非修复） | `refactor(kernel/mm): 重构页表` |
| `test` | 测试相关 | `test(kernel/heap): 添加堆测试` |
| `chore` | 构建 / 工具 / 配置变更 | `chore(deps): 升级依赖` |
| `ci` | CI 配置变更 | `ci: 添加 cargo-deny 步骤` |

---

## Scope 规范

`scope` 为可选字段，使用 crate 名或子系统名：

- crate 名：`kernel`、`runtime`、`ai`、`protocols`、`agents`、`ci`
- 子系统名：`heap`、`mm`、`boot`、`sched`、`net`、`fs`
- 文档类：`docs`

示例：`feat(kernel/heap): ...` 表示 kernel crate 的 heap 子系统新增功能。

---

## Subject 规则

- 使用**祈使语气**（英文）或动宾短语（中文）：`add` / `实现` / `修复`
- 首字母小写（英文）
- 不超过 **72 字符**
- 结尾**不加句号**
- 不重复 type 与 scope 已表达的信息

---

## Body 规则

- 可选，用于说明**为什么**（why）而非**做了什么**（what）
- 每行不超过 **100 字符**
- 多条说明用 `-` 列表
- 与 subject 之间空一行

---

## Footer 规则

- 可选，用于：
  - `BREAKING CHANGE:` 描述破坏性变更
  - `Closes #<issue>` 关闭 issue
- 与 body 之间空一行

### BREAKING CHANGE 示例

```
feat(runtime): 重构启动流程

将引导加载逻辑从 runtime 移入 kernel，统一入口。

BREAKING CHANGE: runtime 不再接受 boot_args 参数，改由 kernel 传递
```

---

## 完整示例

```
feat(kernel/heap): v0.10.0 实现内核态 buddy 堆分配器

- 实现 buddy 算法分配/释放/合并
- 添加碎片统计接口
- 通过分配/释放/碎片测试

Closes #10
```

```
fix(runtime/boot): 修复启动时页表未刷新导致的崩溃

tlb 刷新时序在多核场景下存在竞态，调整为刷新后再次 dsb 同步。

Closes #23
```

```
ci: 添加 cargo-deny 检查步骤

替代 cargo-audit，同时检查许可证合规与已知漏洞。
```

---

## 验证

### CI 自动检查

每次 push / PR，CI 的 commitlint 步骤自动校验提交信息。不合规的提交信息会导致 CI 失败。

### 本地预检

```bash
# 使用 pre-commit 钩子（安装后在 git commit 时自动触发）
tools/pre-commit.sh install

# 手动运行质量门禁
make ci-local
```

### 修正提交信息

若提交信息被 commitlint 拦截，且尚未推送：

```bash
git commit --amend -m "feat(kernel/heap): 实现堆分配器"
```

若已推送，修正后用 `git push --force-with-lease`（**禁止** force push 到 main / develop）。
