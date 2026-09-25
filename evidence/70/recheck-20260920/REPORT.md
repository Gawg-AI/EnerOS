# 蓝图 70 手工修复复验报告（2026-09-20）

**结论：多项原缺陷已修复，但当前仍不能全部验收通过。** 本次基于最新提交 `a72ae68cfd39bf1c13aa3be0f1dd5cdb30ca29ca` 及当前工作树核对，不沿用 9 月 19 日旧代码的失败结论。Standards 发现 2 项 P1，Spec 发现 3 项 P2；其中既有未闭环要求，也有本次修复新增的边界问题。

依据：[修复蓝图 26](/home/ahx/文档/EnerOS/蓝图/70-核心七方向全链路深化蓝图/26-独立审查缺陷修复蓝图-AI执行版.md)。相对最初基线 `9ed12d3` 的修复现已提交；当前尚有运行证据文件变化。受审源码摘要存于本目录 `reviewed-source-sha256.json`。本次只审查、测试、归档，没有修改业务代码或门禁判据，没有新启动 QEMU。

## 已确认修复

| 上次问题 | 本次结论 | 独立验证 |
|---|---|---|
| RS1 未落盘却返回 Completed | 原问题已修复 | 同步首次响应、重放、查询及异步收口在终态提交失败时均不返回 Completed；成功补写后查询 Completed，副作用只执行一次。 |
| RS2 缺生产重试来源 | 已新增生产入口，但不能关闭 | dirty 清理及失败 pending 已修正；新周期同步调用存在下面 S20-1 风险。 |
| RR1 真实持久写失败误释放共享快照 | 原问题已修复 | 真实 write-failed 故障反例转绿；旧容量拒绝、空闲释放等独立 held 工程共 22 项通过。 |
| RR2 外部 READ 借数 | 部分修复 | 异值和无绑定字段的旧夹具被拒；同地址同值的跨计划歧义仍见 R20-1。 |
| RR3 子门冒充父门、扩展 PASS token | 原问题已修复 | 最新允许有界括注的解析器下，两项独立候选红测仍通过；合法签名的 R03-only 记录不再证明 R02。 |
| RR4 FAIL 重跑覆盖成功证据 | 原场景已修复，完整不可覆盖保证仍有边界 | PASS→FAIL 后旧索引、旧信封及摘要均保持有效；同秒同内容重录并发生提交故障仍见 R20-3。 |

原 S1 最后试启保护、S3 不同 action 幂等冲突在独立管理复现中继续通过。这里的“已修复”针对相应反例及已核对路径，不等于蓝图全部目标侧验收完成。

## Standards

### S20-1 · P1 · 新管理补写 tick 会同步阻塞负责故障恢复的 root

位置：[service.rs:1235](/home/ahx/文档/EnerOS/crates/runtime/root-task/src/service.rs:1235)、[service.rs:2299](/home/ahx/文档/EnerOS/crates/runtime/root-task/src/service.rs:2299)。

新周期任务直接向 svc-mgmt 执行 `endpoint.call`。实际底层是 [ipc_sel4.rs:267](/home/ahx/文档/EnerOS/crates/agents/agent-sdk/src/ipc_sel4.rs:267) 的同步 `seL4_Call`；所谓超时只在调用返回后检查，不能让一个永不回应的服务解除阻塞。

若 svc-mgmt 在该周期前 fault、挂起或不再接收，root 会卡在这次调用，无法走到同一循环后面的 [故障轮询 service.rs:1351](/home/ahx/文档/EnerOS/crates/runtime/root-task/src/service.rs:1351)，因而无法执行它负责的恢复。当前 tick 只检查 handle 存在，没有处理服务状态或不可回应的情况。另有 root↔mgmt 同步调用的交错风险：管理动作通过 RootMgmtClient 等待 root，而 root 的管理轮询和 tick 位于同一主循环。

**证据等级：静态生产调用链确认；没有声称已在 QEMU 复现挂死，也没有将已有间歇冻结日志归因于此。** 合同为蓝图 §6.3.4 的可靠周期补写及既有 supervisor 故障恢复职责。

验收要求：周期触发必须不能无限阻塞 root 的管理/故障轮询。实现真实非阻塞通知或可恢复的有界异步状态机；仅补一个调用前状态判断不足以关闭竞态。注入 svc-mgmt 无响应、fault 以及管理请求交错，root 应继续处理故障和其他周期，恢复后补写收敛且不重复副作用。

### S20-2 · P1 · 恢复后的 Unknown 未决记录可被逐出

位置：[mgmt_ops.rs:299](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_ops.rs:299)、[mgmt_ops.rs:358](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_ops.rs:358)。

容量满时只检查是否有 Running，否则直接删除最旧记录。恢复路径会把 Running 转为 Unknown；因此满账本重启后，原来受保护的未决操作全部失去容量保护。

独立测试通过真实 ManagementRpc 填满 15 条 Running，再执行实际恢复函数、提交第 16 条，结果为：`before=15 new_status=Accepted old_op_present=false calls=16`。要求背压的断言失败（退出 101）。丢失旧请求身份后，后续旧请求重放还可能被当作新请求执行。

这是本次进一步复查发现的既有恢复缺口，不宣称由最新提交新引入。它违反蓝图 §6.3.7 / M08 的“未决操作不得被逐出；满容量明确背压”，不能因 Running 满容量测试通过就关闭该要求。

验收要求：Unknown/待对账状态与 Running 一样受保护；只有确实完成清算、可淘汰的记录才允许淘汰。满容量恢复后，新请求必须背压、旧 operation_id 仍可查询、原请求身份仍防重。

证据：`standards.txt`；`standards-repro/` 的 `unknown-capacity` 模式。该模式前半部分同时验证 RS1/S1/S3 的正确行为，末尾 Unknown 容量断言才是本次失败点。

Standards：2 项，最高 P1；涉及 root 故障恢复可用性和未决管理操作防重。

## Spec

### R20-1 · P2 · R10 新绑定器仍无法区分同地址同值的不同计划

位置：[r10_intent_bind.py:133](/home/ahx/文档/EnerOS/tools/r10_intent_bind.py:133)。

新绑定器只用 `(addr,value)` 从整份 transcript 中选择 WRITE，再寻找后续同值 READ。它没有协议事务与 intent 的真实身份映射，没有把写入时间绑定到该 intent 的实际投递，也没有证明窗口只有一个计划。

独立夹具提供 P/Q 两个完整 ID 不同但设定值相同的计划，transcript 只有一套事务。无法判断该套事务属于 P 还是 Q，工具却为选中 P 输出 `12 12 12` 和 `4 21 4 21 4 21`，退出 0。因此“异值 Q-only 夹具被拒”不足以证明逐计划归属。

另一个同根边界是 `WRITE(1) → 覆盖 WRITE(9) → READ(1)`：待验证队列不会因覆盖写失效，仍可给原 intent 记证明。蓝图 §8.2.7 明确禁止单凭“整份日志同地址同值”建立归属，A05 要求拒绝覆盖歧义。

验收要求：引入可核对的 intent→真实投递→协议事务映射，或严格限定且证明单计划窗口；无法消歧时拒绝。增加同值 P/Q 混合、窗口错配、覆盖写三类反例，不能仅测试不同数值。

证据：`bind.txt`、`bind-repro/same-value-Q-only.*`、`bind-repro/overwrite.txt`。这些验证的是门禁判据，不是整套 QEMU gate。

### R20-2 · P2 · skipped 分支把多时段验收改成了天然成立的数量上界

位置：[run70-r10-held.sh:244](/home/ahx/文档/EnerOS/tools/run70-r10-held.sh:244)。

只要 `skipped>0`，A5b 就不再检查时间跨度，而改检查“绑定写数 ≤ intents”。绑定器每个 intent 最多绑定一次，这个上界由算法本身保证，无法识别提前集中发放。存在部分同值折叠不代表剩余不同设定值步骤都能同刻执行。

独立夹具每资产有两个不同设定值，共 6 intents＋6 skipped，全部同刻写入。绑定器输出 `6 6 6`、`2 0 2 0 2 0`；A5b 的 `6≤6` 仍为真。最新提交把非合并计划的下界改为按 2 秒步距计算，没有修复这个 skipped 分支。

违反蓝图 A07 的“原时序/三资产条件保留”。验收要求：按剩余实际调度步骤检查时间合同；全部折叠的计划可以验证合并行为，但不能充当多时段执行成功的唯一证据。上述剩余不同值步骤同刻发放的夹具必须失败。

证据：`bind-repro/skipped-burst.*`、`bind.txt`。

### R20-3 · P2 · 同秒同内容重录仍覆盖原信封，索引故障后旧成功证据失效

位置：[run70-candidate-evidence.py:304](/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py:304)。

run 路径取记录内容摘要，而 `recorded_at` 只有秒精度。同秒重录同一日志会得到相同路径，但 envelope 创建会生成新的 run_id/nonce，因此信封内容不是幂等的。重录先覆盖信封、后更新索引；如果后者失败，旧索引仍指向已被覆盖的文件及旧摘要。

确定性故障注入固定同一秒，令两次 envelope 具有不同 nonce，并在第二次 `save_index` 抛出写入错误。实际 cmd_record 的结果为：`index_unchanged=True`、`success_envelope_unchanged=False`、`index_envelope_digest_matches=False`。这不是之前 PASS→FAIL 的旧反例；该旧反例现在确实已通过。

证据等级：记录器流程动态复现，时钟/lineage/envelope 子进程为测试替身；新 nonce 行为另由真实 [evidence_chain.rs](/home/ahx/文档/EnerOS/xtask/src/evidence_chain.rs:1556) 核对。没有操作真实候选。该场景需要同秒重复记录和提交故障，影响范围小于原缺陷，但仍违反 §4.2.8 的不可覆盖及完整写入后切换要求。

验收要求：每次创建获得真正独立的路径；或命中旧内容时只读复用完整旧信封而不重写。故障发生在写信封后、索引切换前时，旧索引引用必须仍然有效。

证据：`recorder-edge.txt`、`reproduce_recorder_same_second.py`。

Spec：3 项，最高 P2；包括外部归属仍不充分、时序验收降级和证据不可覆盖边界。

## 最新实跑结果与边界

| 检查 | 结果 |
|---|---|
| `cargo test --workspace --locked` | **9156 passed、0 failed、0 ignored**，466 个结果块，退出 0。 |
| fmt / Clippy（`-D warnings`） | 均通过。 |
| `cargo xtask qa feature-matrix` | PASS，9 项。 |
| 旧独立 held 工程 | 22 项通过，包含真实写故障、容量拒绝、空闲重试。 |
| 独立候选 gate 反例 | 2 项通过。 |
| 普通 PASS→FAIL 记录器反例 | 通过，旧成功文件及摘要保留。 |
| 管理终态正确行为复验 | 通过；同一工程新增 Unknown 满容量断言失败。 |
| 记录器 selftest / bind selftest | 通过；不覆盖本报告全部独立反例。 |
| 蓝图结构校验 | PASS，0 错误；53 个历史源码快照漂移文件仍为告警。 |
| `cargo xtask evidence42 check-current --matrix core-70` | **失败**：`candidate-index-unavailable`，当前提交/工作树没有可匹配的索引。 |

用户已有新一轮 QEMU 和候选记录，不能忽略这些进展；但当前 `check-current` 的实测结果尚不能证明整组就绪。蓝图末尾也仍记录 R10 缺失及 R11 外部待证。软件缺陷、R10 目标窗口和 R11 硬件证据必须分别核对，不能以常规测试全绿代替。

## 复现入口

从 `/home/ahx/文档/EnerOS` 执行。独立工程引用当前工作树源码；只增加测试产物，不修改实现。

```bash
cargo run --offline --manifest-path evidence/70/recheck-20260920/standards-repro/Cargo.toml
cargo run --offline --manifest-path evidence/70/recheck-20260920/standards-repro/Cargo.toml -- unknown-capacity
python3 evidence/70/recheck-20260920/bind-repro/repro_bind.py
python3 evidence/70/recheck-20260920/reproduce_recorder_same_second.py
python3 evidence/70/recheck-20260919/reproduce_candidate.py
python3 evidence/70/recheck-20260919/reproduce_recorder.py
```

独立测试中的失败是“要求正确行为”的断言失败；bind 夹具则输出当前错误通过的结果，不应把其退出 0 解读为归属验证成功。S20-1 需另补目标侧故障/交错验证，不能从 host 测试推定目标已安全。

Standards 共 2 项、最高 P1；Spec 共 3 项、最高 P2。本轮确认了修复进展，也保留了仍未满足验收条件的明确证据。
