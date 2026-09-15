# 浸泡运行记录（`eneros.soak-run.v1`）填写说明

> 载体：`evidence/70/70.14/soak-run.json` + `<run_id>/` 原始文件目录。
> 判据实现单源：`xtask/src/soak.rs`（`cargo xtask soak run-check <manifest>`）。
> 本说明与 2026-09-13 复查收紧后的判据同步；数值口径变更必须同改本文件。

## 1. 字段语义与判据要点

| 字段 | 判据（违反即判负） |
|---|---|
| `artifacts.<role>` | 必须为 `"<sha256>@<仓库内相对路径>"`；判据**实算文件摘要并比对**（裸摘要 = `run-artifact-unbound`，路径缺失/逃逸/摘要不符各有一条 violation）。必需角色：`root_task`、`boot_chain` |
| `raw_files[]` | 仓库内相对路径；必须存在、**非空**、**可读**（读失败＝`run-fatal-scan-unreadable`，不得当"无可疑"）。绝对路径与 `..` = `run-raw-file-escapes-base` |
| `started_at`/`ended_at` | 实测墙钟（RFC3339，只接受 UTC `Z`，秒 0–59，小数位 1–9 位数字）。`measured_hours` 由两者算出，**不得**由参数推导 |
| `heartbeats[]` | ≥2、严格递增、覆盖全窗、相邻间隔 ≤900 s |
| `protocol_coverage[]` | 每协议窗口在运行区间内、跨度 ≥ `min_hours−1h`、`samples>0`，且样本数 ≥ 覆盖窗口每 30 分钟 1 个（下限 3） |
| `metrics[]` | ≥3 个样本；时刻**严格递增**且在运行区间内；样本跨度 ≥ 运行时长 75%；堆斜率按**真实时刻**最小二乘（值/小时）与阈值比对 |
| `device` | SKU/修订/序列号/固件非空（序列号不得为 `unknown`）；资格判据会核对 `device.sku_id`/`board_revision` 与被资格 SKU/修订一致 |
| `rig` | `rig_id`/`instruments`/`power_channel` 非空（独立观察面） |
| `interruptions[]` | 非空即判负（有中断不得当连续 72 h） |
| 阈值 | `--min-hours` 只能 ≥ 72、`--max-heap-slope` 只能 ≤ 65536（放宽属发布策略，不由调用方静默完成） |

## 2. 判据的边界（诚实声明）

判据核对的是**记录与样本的内部一致性**，不是物理世界。运行记录本身是执行器
自报的索引：真正的防伪要求执行器（试验台脚本）在运行期间写入原始文件并计算
摘要，判据再据实核对。当前仓内**没有**真实试验台执行器（`soak72.sh` 的
`run_executors` 为空并显式 PENDING），因此本模板在不接入真实执行器前必然判负。

## 3. 未闭合项

- `--min-hours` 等阈值的放宽仍需"策略签名 + 留痕"（登记
  `REG-70.14-JUDGE-POLICY-OVERRIDE`）；本次只做了"只能更严"的单向闸门。
- `qa soak-report` 仍按 MANIFEST 的存在性给 D1-PASS（登记待收口）。
