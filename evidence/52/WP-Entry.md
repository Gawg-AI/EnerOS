# 蓝图 52 卷 Entry（锚点重验 + 上卷回归）

> 日期：2026-08-23 · 基线 HEAD=4521e83（workspace **0.313.44**）· 工作树 clean
> 执行模式：一卷完全体（52.1→52.2→52.3→52.4→Exit 连续执行）；
> 本卷红线：**零代码改动**（缺陷→登记→退回 owner 卷）；GO 仅当 52.2 全 PASS
> 且 52.3 全完成。

## 一、代码事实锚点重验（§4.5.1，三项全过无修订）

### 锚点① `docs/v1.0.0-go-no-go.md` 顶部裁决
- 实读全文：顶部裁决 =「**NO-GO / BLOCKED**（2026-08-18，蓝图 43 商业交付
  终验）」，历史记录全部标记 superseded ✅ 与卷蓝图登记一致，无需修订。

### 锚点② `evidence/42/next-steps.json` 8 项 ↔ 44-00 §5 映射逐项核对

| # | next-steps 条目（原文 owner） | 系列映射 | 清偿状态（本 Entry 核验） |
|---|---|---|---|
| 1 | Resolve closure crypto policy (36/41) | →47 | **已清偿**：47 卷 SecureChannelFactory+私钥出仓+crypto-policy D4 更新；closure gate crypto_violations=0（本日实跑） |
| 2 | Resolve orphan-check 146 files (36/41) | →44 | **已清偿（带登记残留）**：44 卷 frozen-crates 门禁落地；48 复验登记 HEAD 3 孤儿=审查裁决保留接缝、清偿归后续卷（非未决缺陷） |
| 3 | Resolve legacy G0/G1 evidence gate (36) | →44 | **已清偿**：42 收口 final-audit legacy FAIL 清零 + 44 卷门禁收口 |
| 4 | Configure independent protocol peers (39/40/42) | →46 | **大部分清偿**：46 卷 6 协议 L2 VERIFIED（l2-*-verified.json ×6 绑 4f7344c 在库核验）；modbus-rtu=BLOCKED（真机 RS485 对端归 51-B6/owner，非软件缺口） |
| 5 | Run RC 72h software soak (42) | →52 | **本卷执行**（WP-52.2；QEMU 软件侧真跑+真实起止登记） |
| 6 | Run Windows native xtask tools (42/CI) | →52 | **本卷处置**：wine ABSENT（2026-08-23 which 实测）→ Windows 原生实跑维持环境 BLOCKED 如实登记，不得伪装 |
| 7 | Run clean locked CI dual-build (42/CI) | →52 | **本卷执行**（WP-52.1 双构建 digest 比对；act ABSENT→远端 CI 维持外部项） |
| 8 | Run QEMU A/B update boot matrix (42) | →52 | **本卷执行**（WP-52.2 三态矩阵重跑，29/42 卷 harness） |

### 锚点③ 42 卷 BLOCKED-42 四项现状重验

| 项 | BLOCKED-42 登记现状 | 本 Entry 重验（2026-08-23） |
|---|---|---|
| Windows 本地实跑 | 环境 BLOCKED（CI matrix 已配置） | wine ABSENT 实测维持；闭合路径不变（windows-latest job 资源到位） |
| RC 72h soak | 资源 BLOCKED（5000-cycle 后台实跑中登记） | 42 卷 harness 在库；本卷 52.2 以 QEMU 全链路真跑并如实登记起止/中断 |
| CI 双构建远端 | 环境 BLOCKED（组件级 BIT-IDENTICAL 已证） | act ABSENT 实测维持；本卷 52.1 同机双目录+clean clone 构建覆盖软件可证面 |
| L2 独立协议对端 | 外部依赖 BLOCKED（句柄未移交） | **已被 46 卷实质清偿**：6 协议 L2 VERIFIED 证据在库（本 Entry ls 核验 ×6）；modbus-rtu 真机对端归 B6 |

## 二、上卷回归（§4.5.2，≥2 项照抄 51 Exit Gate 4 表，2026-08-23 实跑）

1. **closure gate**：`cargo xtask closure gate --profile qemu-prod` →
   `CLOSURE-GATE profile=qemu-prod binaries=32 reached_packages=122 denied=0
   crypto_violations=0 digest=bdd2332ed0a73788b7c3e05c67e58ba15d03ee1f403275e053f1159a3faf1884`
   + `CLOSURE-GATE PASS` EXIT=0 ——与 51/50 卷登记**逐字一致**（零代码面维持）✅
2. **CATALOG-VALIDATE**：`cargo xtask catalog validate` → `CATALOG-VALIDATE
   PASS` EXIT=0 ✅
3. **CF01 config-refs**：`cargo xtask qa gate config-refs` →
   `CONFIG-REFS-CF01 → OK（active 18，templates 74，deleted 3）` EXIT=0
   ——与 51 卷登记一致 ✅

## 三、本卷新建/修改文件清单（开工前列出，卷内未列不动）

- 新建：`evidence/52/WP-Entry.md`（本文件）、`candidate-lineage.json`、
  `WP-52.1.md`、`WP-52.2.md` 及终验矩阵各项证据文件、`blocked-register.json`、
  `WP-52.3.md`、`WP-52.4.md`、`exit-gate.{md,json}`；QEMU 串口日志若干
  （gitignore 需 add -f，49 卷补正先例）
- 修改：`docs/v1.0.0-go-no-go.md`（52.4 顶部唯一裁决更新）、
  `蓝图/44-00-交付冲刺总纲.md` §6 状态板 52 行、`PROJECT.md` §3/§4、
  workspace 版本 bump（Cargo.toml/Cargo.lock 机械行）
- tag：`v1.0.0-rc.1`（候选冻结，GO 后转 v1.0.0）
- **产品源码 crate：零改动**

## 四、冻结前置实测（WP-52.1 输入）

- pinned-tools.json 核对：rustc 1.96.0-nightly (2972b5e59 2026-04-03)
  与 rust-toolchain.toml nightly-2026-04-04 匹配 ✅
- wine ABSENT / act ABSENT（which 实测）→ Windows 原生与本地 CI runner
  双双不可行，如实 BLOCKED 登记（不伪装为已完成）
