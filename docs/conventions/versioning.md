# EnerOS 版本纪律与 bump 工具

> 版本：v1.0.0  
> 适用范围：EnerOS workspace 全部 145 个成员 crate  
> 蓝图依据：`蓝图/44-00` §3（owner 裁决 2026-08-18，取代 `蓝图/20` R1-1）

---

## 纪律

1. **未过 52 卷 GO 终验，版本不得挂 rc/1.0 标记**：预发布后缀（`-rc.N` 等）
   一律非法；`1.x` 及以上只有 `docs/v1.0.0-go-no-go.md` 全 PASS 才允许；
   冲刺期一律 `0.x.y`（`0.313` 延续历史 120 版本计划顶格 v0.312）。
2. **每 WP Exit 自证后递增一个 patch**：`0.313.0 → 0.313.1 → …`；
   52 卷 GO 裁决通过后才允许 tag `v1.0.0`。
3. 该纪律由双闸门 fail-closed 强制：`cargo xtask qa gng-version` 与
   `cargo xtask qa gng-version`（CI 显式步骤 + pre-commit 三快门之一；
   原 `eneros-ci version-gate` 已随蓝图 67 WP-67.1.4 退役——版本规则
   只改 xtask/qa.rs 一处）。
   非 `x.y.z` 数字格式同样拒绝。

## 工具

```bash
cargo xtask bump-version patch   # 0.313.0 -> 0.313.1（每 WP Exit 的标准动作）
cargo xtask bump-version minor   # 0.313.0 -> 0.314.0
cargo xtask bump-version major   # 0.313.0 -> 1.0.0（须过终验，闸门另守）
cargo xtask bump-version --set 0.313.5            # 显式指定
cargo xtask bump-version patch --dry-run          # 只打印计划
cargo xtask bump-version patch --from 0.313.0     # 根字段当前值不符即拒（幂等防护）
cargo xtask bump-version patch --manifest-path X  # 默认 <repo>/Cargo.toml
```

## 语义

- 改根 `Cargo.toml` 的 `[workspace.package].version`：所有
  `version.workspace = true` 成员随继承生效（当前 134 个）；
- 同时把成员 manifest 里**字面** `[package].version` 归一到同一目标
  （清偿历史 `0.<卷>.<patch>` 遗留字段，当前 11 个）；
- **不触碰**依赖版本，**不写** `Cargo.lock`（下次 cargo 构建自动同步）；
- 当前版本带预发布标记时禁止隐式 bump，须 `--set` 显式指定。

## 一次性归一脚本（历史存档）

WP-44.1 首次订正（`1.0.0-rc.0 → 0.313.0` + 11 个遗留字面归一）由会话脚本
`/tmp/wp441_set_version.py`（只改 `[package]`/`[workspace.package]` 段内裸
`version = "..."` 行）完成；日常维护一律用上述 xtask 子命令，不再用临时脚本。
