# 蓝图 70 修复复查报告

日期：2026-09-19。结论：**修复已取得实质进展，但尚不能按修复蓝图验收通过。仍确认 6 项问题，Standards 2 项、Spec 4 项；分别包含 P1 1 项和 P1 2 项。**

审查对象是当前未提交修复，相对基线 `9ed12d3ee5b182c81066eabf1ba9a783954c28f0`，包含未跟踪的新实现文件。验收依据为 [26-独立审查缺陷修复蓝图-AI执行版.md](/home/ahx/文档/EnerOS/蓝图/70-核心七方向全链路深化蓝图/26-独立审查缺陷修复蓝图-AI执行版.md)。本轮没有修改业务实现，也没有代替用户提交修复。

复查期间工作树仍有更新；已重新核对最新管理探针及 R10 父脚本，并在末轮重新执行 workspace 测试。受审文件摘要见 `reviewed-source-sha256.json`；其后继续修改代码时，本报告不自动代表新版本。

## Standards

本轴检查运行状态、持久状态和对外承诺是否遵守已有合同。没有把主观风格意见计入缺陷。

### RS1 · P1 · 终态未持久化，重放和查询仍返回 Completed

- 位置：[mgmt_rpc.rs:505](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:505)、[mgmt_rpc.rs:613](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:613)、[mgmt_rpc.rs:640](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:640)。
- 同步路径先在内存里 `finish(Completed)`。终态提交失败后只把第一次响应改为 Failed，并设置 `pending_persist`；重放和查询直接读取内存 Completed，没有检查终态是否落盘。
- 异步路径由 `observe_operation` 得到完成事实后，也只更新内存并生成 Completed；没有在该回复之前确认终态持久化。外层 handler 后续尝试持久化，不能改正已经生成的响应。
- 故障注入实测：`first=Failed replay=Completed query=Completed durable=Running pending=true`。异步实测：`first=Accepted query=Completed durable=Running pending=false persist_calls=1`。
- 影响：客户端被告知操作已经完成并可恢复，实际持久账本仍是 Running。崩溃后状态与此前承诺不一致，原 S2 仅修好了意图先于执行的部分。
- 合同：修复蓝图 §6.3 表格要求“终态持久成功……才可回复 Completed”；M06 要求终态提交失败不回复 Completed。
- 修复验收：首次回复、幂等重放、status 查询、异步观测收口必须共用持久确认边界。同步/异步终态写失败时全部出口不得返回 Completed；成功补写后才允许收敛。副作用执行次数仍为 1。
- 证据：`standards.txt`，独立测试工程 `standards-repro/`。该复现程序断言当前错误现象，因此退出 0 表示问题已复现，不能视为修复通过。

### RS2 · P2 · 管理终态重试依赖后续请求，异步写失败还会丢失重试标志

- 位置：[svc-mgmt.rs:1061](/home/ahx/文档/EnerOS/crates/agents/business-services/src/bin/svc-mgmt.rs:1061)、[mgmt_rpc.rs:278](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:278)。
- 实际重试仅挂在 `MgmtHandler::handle`。生产服务在 [service.rs:64](/home/ahx/文档/EnerOS/crates/agents/control-services/src/service.rs:64) 阻塞接收；[wiring.rs:454](/home/ahx/文档/EnerOS/crates/runtime/root-task/src/wiring.rs:454) 也明确登记 svc-mgmt 无周期驱动。QEMU 管理探针是测试装配且有结束点，不能作为生产持续重试来源。
- 异步观测后外层调用 `take_dirty()` 先清标志；若持久化失败，`note_persist(false)` 不会设为 pending。后续没有新变更时，待补写状态可能不再触发重试。
- 合同：修复蓝图 §6.3.4：“清 dirty……只能发生在相应提交被确认之后”“由周期驱动重试；不能依赖用户再发新请求触发”；M10。
- 修复验收：生产装配存在真实、有界的独立周期来源；所有失败路径保留待处理状态。故障恢复后不发送任何新管理请求，终态仍应自动持久收敛；故障持续时不得伪造成功或重复执行副作用。
- 证据等级：生产周期缺失来自调用链核对；异步 pending 未设置由 `standards.txt` 动态复现。没有把 host 手工调用重试等同于目标侧周期验证。

Standards：2 项；本轴最严重问题为 P1 的未持久终态对外宣称完成。

## Spec

### RR1 · P1 · 真实存储写失败后，仍会解除其他在途计划的输入保留

- 位置：[held_engine.rs:575](/home/ahx/文档/EnerOS/crates/protocols/control-dispatch/src/held_engine.rs:575)、[held_engine.rs:216](/home/ahx/文档/EnerOS/crates/protocols/control-dispatch/src/held_engine.rs:216)。
- `live_snapshot_ref_count()` 将 `store.plans()` 错误经 `unwrap_or_default()` 转为空集合。已有计划 P 持有快照时，再提交共享快照的 Q；真实持久写失败使 store 不可正常读取，回滚查询被误解为 0 个引用，随后 release，P 的保留也被撤销。
- 独立测试注入介质 `write-failed`，真实返回 `IntentPersistFailed("held storage: write-failed")`，随后断言“第一份计划仍有 pin”失败。
- 新增树内 [typed_chain.rs:1741](/home/ahx/文档/EnerOS/crates/protocols/control-dispatch/tests/typed_chain.rs:1741) 的 H02 测试使用正常 MemIo，没有注入持久写失败；`.err()` 的结果又被 `let _ = err` 丢弃，所以它通过不能证明 H02。
- 合同：修复蓝图 §7.2.2“准入失败只撤回本次新取得的持有，不撤销先前计划的所有权”；H02；ADR-0054 对持有期间冻结输入保留的约束。
- 修复验收：读取失败/状态未知不能推断零引用并释放。真实 write/flush 等故障导致第二份拒绝后，第一份输入仍受保护；必须断言故障确实发生。原容量拒绝及空闲释放回归也必须继续通过。
- 证据：`held.txt`、`held-repro/repro.rs` 中 `recheck_h02_real_persist_failure_preserves_existing_pin`，退出 101。

### RR2 · P2 · R10 外部读回仍能借用另一计划的协议事务

- 位置：[run70-r10-held.sh:203](/home/ahx/文档/EnerOS/tools/run70-r10-held.sh:203)。
- 内部日志现已按完整 plan/intent ID 统计，这是有效修复。但 A4b 仍对整份 `$TA` 计算读回数量，再与选中计划的 `UNIQ` 比较；没有建立所选计划/intent 与协议事务的关联。A5 同样使用整份 transcript。
- 独立夹具：P 有完整 12 个 Applied、内部读回值为 1.0；外部 transcript 只含另一组 Q 的 12 个写入/读回，值为 99。守恒脚本输出 `12 0 12 0 0 12 12`，外部 oracle 输出 `12`，三资产跨度输出 `4 21 4 21 4 21`。因此 A4b 及 A5 的现有谓词仍通过，尽管不存在 P 的外部读回。
- 此复现证明父门的这些谓词可误通过，没有宣称该合成夹具跑通整套 QEMU R10。
- 合同：修复蓝图 §8.2.5～7 明确不同计划/窗口不得补数；缺协议 plan ID 时要绑定真实投递 trace 或证明窗口内只有指定计划；A04、A07。
- 修复验收：输出所选完整 plan/intent 集及对应协议事务；每个必要 READ 只能用于其归属的 intent。上述 Q-only 夹具必须拒绝，真实 P 正例必须继续通过。
- 证据：`P-serial.txt`、`Q-only-transcript.jsonl`、`cross-plan-readback.txt`。复查期间父脚本新增的退出码处理没有改变该关联缺口。

### RR3 · P1 · gate 模糊匹配仍可让错误子门证明父门成功

- 位置：[run70-candidate-evidence.py:144](/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py:144)、[evidence_chain.rs:344](/home/ahx/文档/EnerOS/xtask/src/evidence_chain.rs:344)。
- 两端使用 PASS 子串匹配及 gate 前缀/连字符后缀匹配。`VOL70.2-R03-POWER-CUT GATE: PASS` 被当成 `VOL70.2` 的成功，尽管 R02/R03 是不同 required case；`VOL70.1-GATE: PASSED_NOT` 也被当作 PASS。任意后缀还允许 `HELD-GATE: PASS` 证明 `R10-HELD`。
- 使用当前 CandidateFixture，先确认正常签名候选可通过，再将 R02 所属 run-record 终态改为上述 R03-only 行并重新生成一致的签名/摘要。真实 checker 输出 R02 PASS、违规集合为空。两项独立“必须拒绝”测试均失败。
- 合同：修复蓝图 §4.2.3～7、E05/E06 要求指定 gate 的明确终态，错误 gate/非终态必须失败。case_id 绑定不能替代 gate 真实性校验。
- 修复验收：为实际输出格式建立明确的 gate 别名/格式映射，完整匹配终态 token。不得通过任意前后缀推断归属。Python 写侧与 Rust 读侧均独立拒绝错误子门、截断或扩展 PASS token，并保留合法三种输出格式的正例。
- 证据：`candidate-adversarial.txt`、`candidate-redtests.rs`、`reproduce_candidate.py`。隔离副本运行，没有改工作树 xtask 源码。

### RR4 · P2 · 同 case 失败重跑会覆盖此前已索引的成功证据

- 位置：[run70-candidate-evidence.py:265](/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py:265)、[run70-candidate-evidence.py:304](/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py:304)。
- run-record 和 envelope 的路径固定为 `<candidate>/<case>.*.json`，且在失败返回分支之前写入。同一候选的同一 case 先 PASS、再 FAIL 时，索引字节确实不变，但索引引用的成功文件已经被失败文件覆盖，摘要失配且原运行证据丢失。
- 实测：`index_unchanged=True`、`success_envelope_unchanged=False`、`index_envelope_digest_matches=False`。
- 合同：修复蓝图 §4.2.8 要求完整写入后再切换索引，不允许失败/不完整运行破坏此前成功索引；当前注释承诺“不顶掉已有成功条目”也未满足。
- 修复验收：运行证据使用独立不可覆盖的 run 路径；确认记录和信封完整后再原子更新索引。先成功后失败、签名子进程失败、写入中断时，旧索引及其引用内容/摘要均保持有效，失败运行另行保存。
- 证据：`recorder-adversarial.txt`、`reproduce_recorder.py`。复现调用实际 `cmd_record`，临时目录隔离，lineage 与 envelope 子进程被替身实现，验证的是记录器路径和写入顺序；没有改真实候选文件。

Spec：4 项；本轴最严重为 P1 的在途输入误释放与错误 gate 误通过。

## 原问题逐项复验

| 原编号 | 复查状态 | 判定依据与边界 |
|---|---|---|
| S1 最后试启运行槽身份 | 原复现已修复 | host 最后一次试启的写保护与确认通过；已审阅用户新增 OTA 三次启动日志，16 项通过。未在本轮重新启动 QEMU，也不把该历史镜像自动等同当前全部修改。 |
| S2 管理持久化顺序 | 部分修复 | 意图先落盘已有实现；终态真实性和自动补写仍见 RS1/RS2。 |
| S3 不同 action 误重放 | 原复现已修复 | 同键 enter→exit 被拒，执行计数保持 1。 |
| R1 共享快照误释放 | 部分修复 | 原“第三份容量拒绝”回归已过；真实持久故障仍见 RR1。 |
| R2 空闲不重试 release | 原 host 复现已修复 | 旧独立两项 held 测试通过；H08 单服务恢复、H09 目标侧故障及两轮容量周转仍需对应证据，不能由 host 通过代替。 |
| R3 信封跨 case 复用 | 原问题已修复 | 读侧有 envelope.test_id 与 case_id 独立绑定，迁入回归已过；与 RR3 gate 归属问题区分。 |
| R4 运行结果失真 | 部分修复 | 非零退出码、FAIL 载荷及普通跨门负例已有拒绝；RR3/RR4 仍未闭环。 |
| R5 跨计划计数借用 | 部分修复 | 内部完整 ID 分账有效，外部 READ 归属仍见 RR2。 |
| 文档校验重复误报 | 结构部分已修复 | 实跑 PASS，104 定义、33 执行引用、0 错误；53 个历史快照漂移文件仍如实告警。D06 最终状态核对不能提前视作完成。 |

## 本轮验证与验收边界

| 检查 | 结果 |
|---|---|
| `cargo test --workspace --locked` | 最后一次实跑退出 0；9147 passed、0 failed、0 ignored，466 个结果块。 |
| `cargo fmt --all -- --check` | 退出 0。 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过。 |
| `cargo xtask qa feature-matrix` | PASS，矩阵 9 项。 |
| 记录器 selftest / R10 accounting selftest / 蓝图校验 selftest | 通过；R10 为 12 项，蓝图校验为 8 项。它们没有覆盖本报告全部独立反例。 |
| 蓝图真实目录结构校验 | PASS，0 错误，历史漂移 53 个文件。 |
| 原 held 独立回归 | 2/2 通过。 |
| 新增真实持久故障 H02 反例 | 失败，退出 101；在途 pin 被解除。 |
| 新增 candidate 错误子门/错误 PASS token 反例 | 2/2 失败，退出 101；错误结果被接纳。 |
| 同 case 成功后失败记录器反例 | 失败，退出 1；旧成功文件被覆盖。 |
| 管理终态故障注入 / R10 外部借数夹具 | 复现错误行为，见上述输出。 |
| `cargo xtask evidence42 check-current --matrix core-70` | 退出 2：`candidate-index-unavailable`，没有与当前代码身份匹配的候选索引。 |

当前不能写成“只剩 R11 硬件证据”：本轮首先存在软件缺陷及当前候选缺失。R11 的外部硬件待证仍应单列保留，不能删除 required 项或降低 scope。现有 PASS 门日志可作为对应历史运行证据查看，不能替代针对最终代码身份重新生成候选。

## 复现命令

从 `/home/ahx/文档/EnerOS` 执行；隔离测试工程只引用实际工作树 crate，不修改生产源码。故障红测预期在本次受审实现上失败，修复后应改为要求正确行为并纳入长期回归。

```bash
cargo run --offline --manifest-path evidence/70/recheck-20260919/standards-repro/Cargo.toml
cargo test --offline --manifest-path evidence/70/recheck-20260919/held-repro/Cargo.toml recheck_h02_real_persist_failure_preserves_existing_pin -- --nocapture
cargo test --offline --manifest-path evidence/70/recheck-20260919/held-repro/Cargo.toml review_ -- --nocapture
python3 evidence/70/recheck-20260919/reproduce_candidate.py
python3 evidence/70/recheck-20260919/reproduce_recorder.py
python3 tools/r10_plan_accounting.py evidence/70/recheck-20260919/P-serial.txt
python3 tools/r10_readback_oracle.py evidence/70/recheck-20260919/Q-only-transcript.jsonl
python3 tools/r10_write_spread.py evidence/70/recheck-20260919/Q-only-transcript.jsonl
```

Standards 共 2 项、最高 P1；Spec 共 4 项、最高 P1。下一次验收须先关闭本报告反例，再补齐最终代码身份下的目标侧及候选证据。
