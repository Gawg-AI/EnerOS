# 蓝图 70 手动修复复查与交接（2026-09-18）

## 当前结论与工作区

**尚不能确认 R10 完整收官。** 工程回归通过，但当前提交的候选链验证失败，且最低多资源验收、输入保留、实际回读查询仍有缺口。

- 仓库：`/home/ahx/文档/EnerOS`，分支 `main`。
- 审查基线：`b9b88cb7bfa9441734973c80e31353c4b9e0fbd8`。
- 手动修复提交：`da9e46f9243c70a04b26be0f3ea3a0b997d7bb21`。
- 比较命令：`git diff b9b88cb...da9e46f`，139 文件，19748 插入、4434 删除。
- 本次开始时工作树完全干净，未运行 QEMU/门后台进程；`configs/protocol-mqtt.toml` 无修改。
- 本次只读审查生产代码，没有修改实现、配置、登记册或历史候选记录。新增本交接文档及 `target/r10-review/manual-*` 诊断结果。
- 用户当前请求是“已手动修复完毕，帮我检查”，不是授权本次自动改写其修复。后续修复应以本报告缺口为范围。

## 本次实际复核

| 检查 | 结果 | 本地证据 |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | `target/r10-review/manual-fmt.log`（无输出） |
| `cargo test -p eneros-control-dispatch -p eneros-protocol-services -p eneros-durable-state -p eneros-resource-registry` | 302 passed / 0 failed | `target/r10-review/manual-tests.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS，零告警 | `target/r10-review/manual-clippy.log` |
| `cargo xtask evidence42 candidate-check` | FAIL：当前提交找不到索引 | `target/r10-review/manual-candidate-check.log` |
| 显式指定 `47b0068b.../candidate-index.json` 的 candidate-check | FAIL：30 violations，均为 source_commit/dirty_tree 身份不匹配 | `target/r10-review/manual-explicit-check.log` |
| 11 个 run-record 对原始 batch 日志大小及 SHA-256 | 全部匹配（R11 是 BLOCKED，无运行日志） | `target/r10-review/manual-log-hashes.json` |
| R10 写后读回判据负例 | 错误放行：所有读均在写之前，仍计验证成功 2 次 | `target/r10-review/manual-readback-oracle-red.log` |

本轮没有重新跑全部 QEMU 长门，也没有重建镜像。登记册所称 9102 项全仓测试属于上一轮结果，不能冒充本轮重跑。

旧 `batch/vol5-held.log` 确为 FAIL；手动修复后的 `batch/vol5.log` 已 PASS（R6 拒绝标记4次，forger发送8次）。两份日志不是同一运行。`batch/vol1.log` 记录 3972 rounds / 3728 commits，确实越过原512压力线；继续保留该压力线，不要恢复“结构性不可达”的旧归因。

## 交付证据问题（独立实测）

**[P1] 提交后的候选身份没有闭环。**

`evidence/70/70.15/47b0068b1dfa6d37cdd924aff8821cabc63eeccc68d67ae9e9ee1b5cd0b6a15a/candidate-index.json:5–6` 仍绑定 `b9b88cb` 与提交前 dirty digest；当前 HEAD 是 `da9e46f` 且树干净。自动查找立即报 `candidate-index-unavailable`；传 `--index` 也不会解决身份不匹配，实际产生30项 violation。

这不说明原日志被伪造（11份摘要均核验通过），而是现行验证协议不认可当前提交。需在冻结实现后解决“提交前录制→提交后身份验证”的可核验关联，并复跑最终命令。不要只手工改索引 source_commit，也不要绕过验真后声称零 violation。

## Standards

以下按规范审查轴单独列出，不与 Spec 轴合并计数。

1. **[P1] 60秒控制窗与最新 ADR 冲突。** `crates/ai/decision-contract/src/lib.rs:63` 将生产控制窗改为60秒；`docs/adr/ADR-0054-held-plan-production-entry.md:34–35` 明确不能为4×15秒放宽30秒安全窗，`CONTEXT.md:4` 指定最新ADR优先。登记册引用较早DEC-70-13未消除冲突。应恢复既定窗口并缩短或重新认证计划，或完成有依据的新架构裁决；常量一致性测试不能证明扩大输入有效期的依据。
2. **[P1] 未保留完整认证输入与事件。** `held_engine.rs:124` 只保存 `input.encode()`，`held_plan.rs:423` 限定234B TrustedInput，内容为摘要/身份及精简遥测。持有计划没有接入保留与释放，`event_face.rs:723–734,768–770` 仍可在压力下删尽旧快照。违反 ADR-0054:29–30；掉电后可能无法复查原认证依据。
3. **[P2] 正常成功命令实际回读无法查询。** `engine.rs:3040–3042` 将 Committed 映射为 `Applied { readback: None }`；`dispatch_service.rs:134–135` 随之输出无回读。违反 ADR-0054:15–16。应沿真实回执持久化实际值，不得用请求设定值代填。

TCP新接收超时在生产装配前有 `time_init`，未把“目标时钟未初始化”列为故障。没有将命名/抽象偏好列为缺陷。

Standards：3项，最严重为生产有效期与最新ADR冲突，以及持有输入保留缺失。

## Spec

以下按需求审查轴单独保留，部分问题与 Standards 重叠。

1. **[P1] 控制窗从30秒扩大到60秒。** ADR-0054:34–35禁止因4×15秒计划放宽；`decision-contract/src/lib.rs:57–63` 的变更影响生产输入有效性，不能仅作为测试适配。
2. **[P1] R10门未证明最低三资源×四时段及步骤守恒。** 蓝图08:208、234、242、274要求至少3资源、4时段和全部步骤计数守恒；`tools/run70-r10-held.sh:169–176` 只检查两资产请求与累计打印 Applied≥2。当前peer wire仍限8 assignments，12项最低示例无法贯通。实际 A 窗唯一 PLAN-HELD 为6个意图，最终6个不同Applied；14行Applied包含周期重复打印。需要逐assignment关联执行或合法未动作原因，不能以请求里的steps=4替代验收。
3. **[P1] 计划输入保留没有生产接线。** ADR-0054:29–30和蓝图08:137–140要求引用保留；234B输入摘要不包含原始ObservationEvent，`hold_input_offset`没有生产调用者。需完整内容保留或可释放的持久引用，并验证压力删除/恢复路径。
4. **[P1] 正常成功命令回读不可查询。** ADR-0054:15–16、蓝图11:30要求逐意图可对账；`engine.rs:3039–3042` 固定回读None，A窗全部成功意图亦为None。抓包不能替代产品查询与持久记录。
5. **[P2] 写后读回门的时序判据错误。** 蓝图08:250要求逐命令关联；`tools/run70-r10-held.sh:192–204` 仅按地址和值集合匹配，忽略顺序和意图。取原判据输入两次READ后两次同址同值WRITE（没有任何写后读），输出2，达到≥2通过线。应按写入后的独立读取关联，加入旧读值/错误顺序负例。

Spec：5项；最低三资源完整计划与当前两资产门不符，另有安全窗、输入保留和回读查询缺口。

## 关键证据与复现

- A窗：`evidence/70/70.15/r10-held/win-a.qemu-serial.log:39901` 为 `PLAN-HELD ... batches=1 intents=6`；48703–48709为Completed与6条不同Applied，全部readback=None。
- 原门汇总：`evidence/70/70.15/batch/r10-held.log`。
- 最小读回假阳性复现（从当前门原样提取的Python判据）：

```bash
python3 target/r10-review/manual-a4b-oracle.py target/r10-review/readback-before-write.jsonl
# 输出 2；但fixture没有任何写后读取。
```

- 候选显式复核：

```bash
cargo xtask evidence42 candidate-check --index evidence/70/70.15/47b0068b1dfa6d37cdd924aff8821cabc63eeccc68d67ae9e9ee1b5cd0b6a15a/candidate-index.json --report
# candidate-check FAIL: 30 violation(s)
```

## 接续顺序

1. 先收敛30秒/60秒的架构合同，不把放宽生产安全窗口默认为门适配；实施方案与最新ADR保持一致。
2. 完成完整冻结输入/观测的持有与释放，以及真实回读进入Saga持久记录和查询。
3. 支持并实测至少3资源×4时段；覆盖单次提交后续tick、完整步骤守恒、部分失败及重启。合法跳过也需与原assignment可追溯。
4. 修复R10门：按唯一完整身份计数、跨步守恒、写后读取时序；负例必须能把错误证据判红，再运行目标长门。
5. 必要门与fmt/clippy通过后冻结树，重建所需产物并统一12case；明确最终提交和录制身份的验证协议。目标是当前交付树上只余R11 BLOCKED、零violation。
6. 据实更新登记册“终态”；当前3资源未验收与R10收官并存，不能当作已完成证明。

## 仍需遵守的操作约束

- 不使用git checkout/stash，也不丢弃用户提交/工作树；归因A/B使用独立worktree。
- QEMU门互斥，启动前查1883/5577/5582/2404；kill与启动分开，避免pgrep -f自匹配。
- 不在门运行中原地修改其bash脚本：解释器后续读取可能受文件偏移变化影响。
- vol1/vol2自身不构建镜像；必须核实镜像来源。UEFI冻结应据串口/READY诊断。
- vol5/R10门会改再恢复MQTT配置，跑后核实干净。
- 录制开始后禁止改影响树身份的tracked文件；evidence/target之外包括docs/configs。同profile产物录制期间不重建。
- 初始历史WIP备份仍在 `target/r10-review/initial.patch`；旧 `fs-baseline` worktree曾人为应用诊断变异，不是干净基线，不得误用。
