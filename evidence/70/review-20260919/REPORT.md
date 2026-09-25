# 蓝图 70 独立检查与缺陷报告（2026-09-19）

结论：确认 **8 项缺陷（P1×5、P2×3）**，现有测试全绿不足以证明蓝图 70 完成。另有文档校验失效。当前核心整组仍为 **BLOCKED**。

审查范围：蓝图 70.1–70.15 的规范、实施记录、生产调用链和验收工具；变更基线 `071f60629f73e71dd0c1351874bc82786ecf2ab7` → `9ed12d3ee5b182c81066eabf1ba9a783954c28f0`。本次只新增审查材料，没有修业务源码、修改既有候选证据或提交 commit。初始已有的未跟踪候选目录 `evidence/70/70.15/2587ce1b7aaccb74004f7b691d1d8ae11fce9cce3d8d329d1137a26561b7c294/` 保留。

## Standards

### S1 · P1 · 最后一次试启动将正在运行的槽开放为可写

- 位置：[boot_guard.rs:60](/home/ahx/文档/EnerOS/crates/drivers/drv-svc/src/boot_guard.rs:60)、[boot_install.rs:213](/home/ahx/文档/EnerOS/crates/drivers/drv-svc/src/boot_install.rs:213)。
- `boot-manager` 在真正引导前调用 `prepare_attempt`，持久扣减试启次数。当 A 的最后一次试启由 1 变成 0 时，本次仍引导 A，盘上状态的 `select()` 却已选择回退槽 B。守卫把下一次启动选择当成本次运行身份，于是允许安装写入 A；A 的健康确认也被 `confirm-slot-not-selected` 拒绝。
- 违反：`PROJECT.md` §2.3 安全默认拒绝；蓝图 70/02 I1“身份同源”。
- **动态复现**：`standards-repro.txt` 显示 `booting=A, guard.active_is_a=false, running_slot_write_allowed=true`，以及健康确认失败。这不同于已登记的“同一 boot 内第二次 trial”问题。
- 修复方向：从可信启动交接数据绑定本次实际运行槽，守卫和确认共同消费；补充 tries=1→0 的完整用例。

### S2 · P1 · 管理动作先执行副作用，持久账本失败仍返回成功

- 位置：[mgmt_rpc.rs:354](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:354)、[svc-mgmt.rs:1034](/home/ahx/文档/EnerOS/crates/agents/business-services/src/bin/svc-mgmt.rs:1034)、[svc-mgmt.rs:256](/home/ahx/文档/EnerOS/crates/agents/business-services/src/bin/svc-mgmt.rs:256)。
- `begin()` 只改内存，随后立刻 `execute()`。外层在整个请求完成后才 `persist_ops()`；写失败只打印，响应仍可为 Completed，dirty 已清。没有 storage 授权时也没有执行侧拒绝守卫，`ops_store_ok` 只用于展示。副作用之后、落盘之前的崩溃会丢失幂等记录，同请求恢复后可以再次执行。
- 违反：蓝图 70/02 I3“命令/升级/配置动作以可恢复意图和幂等标识约束副作用”，70/07 WP-70.6.1 持久幂等合同。
- **证据等级：生产调用链静态确证**，本次没有运行管理动作的目标掉电/坏盘注入，不能声称已观察到真实设备重复执行。
- 修复方向：在执行前提交 Running 意图；持久化失败阻断副作用，终态写失败需可恢复、可重试并给出诚实响应。

### S3 · P2 · 不同管理动作被当成相同请求，返回旧成功结果

- 位置：[mgmt_ops.rs:264](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_ops.rs:264)、[mgmt_rpc.rs:352](/home/ahx/文档/EnerOS/crates/agents/business-services/src/mgmt_rpc.rs:352)。
- 幂等键查到相同 principal/request_id 后仅比较 `arg_digest`，而摘要只包含 arg，不包含 action。以相同 ID、空参数依次调用 `maintenance-enter`、`maintenance-exit`，第二次返回 Completed replay，退出动作没有执行。
- 违反：70/07 WP-70.6.1“请求 ID 冲突……分别拒绝；旧结果不得串给新请求”，70/02 I3 实际终态合同。
- **动态复现**：`standards-repro.txt` 中两响应均 Completed，执行计数只有 1。
- 修复方向：action 纳入请求摘要或显式冲突比较，明确配置代次等字段的幂等语义。

Standards 共 3 项；最严重为 P1 的运行槽写保护和管理副作用持久顺序。未额外报告主观风格问题。

## Spec

### R1 · P1 · 拒绝第三份计划会解除前两份在途计划的输入保留

- 位置：[held_engine.rs:207](/home/ahx/文档/EnerOS/crates/protocols/control-dispatch/src/held_engine.rs:207)、[event_face.rs:438](/home/ahx/文档/EnerOS/crates/agents/control-services/src/event_face.rs:438)、[event_face.rs:459](/home/ahx/文档/EnerOS/crates/agents/control-services/src/event_face.rs:459)。
- 保留按 snapshot ID 做集合去重，不计引用。已有两份不同计划共享同一快照时，第三份先 hold，再因计划容量满而拒绝，失败回滚无条件 release 同一个 ID，解除仍在途计划依赖的保留。后续存储压力可回收该快照。
- 违反：ADR-0054:29“持有期间须保留证书引用的冻结输入与事件”。
- **动态复现**：`held-red-tests.txt` 的 `review_capacity_rejection_unpins_live_plans` 断言失败；同时确认已有非终态计划、第三份被拒、保留消失。
- 修复方向：以仍存活计划引用或可区分持有人令牌管理保留；失败回滚只能撤销本次新增的所有权。

### R2 · P2 · 最后一份计划释放失败后，空闲 tick 不再重试

- 位置：[held_engine.rs:247](/home/ahx/文档/EnerOS/crates/protocols/control-dispatch/src/held_engine.rs:247)。
- 计划终态首次解除保留若暂时失败，代码意图是在下一 tick 重试；但没有非终态计划时会提前 return，跳过 `finalize_held_plans()`。没有新计划的空闲周期内保留持续泄漏；累计失败可耗尽四个保留槽并阻断后续准入。
- 违反：ADR-0054:29–31“释放或到期后解除持有……驱动来自周期 tick”。
- **动态复现**：`review_release_failure_is_not_retried_on_idle_ticks` 两个 tick 后只调用一次 release，期望两次。相同夹具的原有正常保留/释放测试通过，见 `held-baseline.txt`。
- 修复方向：空闲路径也执行待释放项收尾，失败项持续重试且不依赖新提案。

### R3 · P1 · 一份合法签名信封可以冒充另一 required case

- 位置：[evidence_chain.rs:540](/home/ahx/文档/EnerOS/xtask/src/evidence_chain.rs:540)。
- 候选检查读取信封后校验 schema、产物身份和签名，却没有要求 `envelope.test_id == entry.case_id`。将 R02 索引指向 R01 的原始信封及其原始摘要，完全不修改签名，检查仍同时给 R01/R02 PASS、违规列表为空。
- 违反：70/10 WP-70.15.1 required 完整性；WP-70.15.2“release-required验证不能接受任意单个合法信封”。
- **动态复现**：`candidate_red_tests.rs` 中 `review70_reject_reused_signed_envelope_for_different_case` 在原实现上失败，见 `adversarial-tests.txt`。夹具使用真实签名与生产验证函数。
- 修复方向：绑定 case、信封 test ID 和运行记录 case ID，并拒绝不同 required 项复用同一运行证据。

### R4 · P1 · 失败运行可被记录成 PASS，候选检查也不核对运行终态

- 位置：[run70-candidate-evidence.py:174](/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py:174)、[evidence_chain.rs:695](/home/ahx/文档/EnerOS/xtask/src/evidence_chain.rs:695)。
- 记录器只要求抓到某个 `GATE/WINDOW: PASS` 且无 FAIL，不核对 `--gate`，也不要求 `--exit-code` 为 0。非零退出码+PASS 行、声明 VOL70.1 却只有 VOL70.2 的 PASS 行，两种输入都返回 0 并记录 PASS。
- 下游检查读到 `eneros.gate-run-record.v1` 后只核 `ran_artifact_sha256`，不核其中 `exit_code/verdict/verdict_lines`。因此只要外层信封写 PASS 并合法签名，即使附带的记录明确为 FAIL/exit=1，也会过门。
- 违反：70/10 WP-70.15.2“核对verdict……FAIL/PENDING/BLOCKED……全部给机器错误原因”，WP-70.15.4 可复核运行证据。
- **动态复现**：`recorder-repro.txt` 两种错误输入都记录 PASS；该 harness 替换 lineage 和签名子进程，专门验证真实记录器的判定逻辑。`adversarial-tests.txt` 另以真实签名、真实候选检查函数证明 FAIL 记录仍通过，弥补前一层 harness 的范围限制。
- 修复方向：在记录和读取两端核对指定 gate 的终态、退出码及 case 对应关系，缺字段拒绝；不能让外层 PASS 覆盖相矛盾的原始运行事实。

### R5 · P2 · R10 守恒验收跨计划借用 Applied 和 readback 计数

- 位置：[r10_plan_accounting.py:28](/home/ahx/文档/EnerOS/tools/r10_plan_accounting.py:28)、[r10_plan_accounting.py:85](/home/ahx/文档/EnerOS/tools/r10_plan_accounting.py:85)。
- `PLAN-HELD`/`HELD-PLAN-DONE` 按 plan ID 关联，但 `applied_ids` 和 `readback_some` 在整份日志上累计，返回时没有筛选获选计划。一份计划只有自报的 DONE、没有逐命令证据时，可以借后续计划的 12 个 Applied 和 readback 通过 A4a/A4c。
- 违反：70/08 全计划逐命令验收、ADR-0054:15–16 实际回读可查；该脚本自身声明“对每个 PLAN-HELD 计划（按 plan id 关联）”。
- **动态复现**：执行 `python3 tools/r10_plan_accounting.py evidence/70/review-20260919/unrelated-intents.txt` 得到 `12 0 12 0 0 12 12`。第一份已 DONE 计划没有命令行，全部 Applied 行出现在另一计划之后。此复现证明 A4a/A4c 的假阳性，不代表仅这份日志就能通过整道 R10 的其他条件。
- 修复方向：输出携带完整 plan/batch/intent 身份的事件；守恒、Applied 集合与读回证据都按同一计划关联后重算。

Spec 共 5 项；最严重为 P1 的输入保留失效及 required/失败证据误放行。

## 本次实际验证

| 检查 | 结果 | 证据 |
|---|---|---|
| `cargo test --workspace` | 9,118 passed / 0 failed；466 个测试结果块 | `workspace-tests.txt` |
| `cargo fmt --all -- --check` | PASS | `fmt.txt`（空输出、退出0） |
| `cargo xtask qa feature-matrix` | PASS，9 个特征配置 | `feature-matrix.txt` |
| `python3 tools/run70_gate_judge.py --selftest` | PASS | `judge.txt` |
| `python3 tools/r10_readback_oracle.py --selftest` | 7/7 PASS | 本次执行；该检查不覆盖 R5 的跨计划关联 |
| 蓝图 `校验蓝图.py` | FAIL，33 条重复工作包标题错误 | `blueprint-validation.json` |
| 当前候选 `candidate-check --matrix core-70 --report` | 索引内部校验 PASS；11 项 PASS、R11 BLOCKED | `candidate-check.txt` |
| 当前候选 `check-current --matrix core-70` | 非零退出、核心整组 BLOCKED | `check-current.txt` |
| 新增对抗性断言 | 候选检查 2 个红测；计划保留 2 个红测 | `adversarial-tests.txt`、`held-red-tests.txt` |
| OTA/管理幂等与脚本复现 | 如上述结果复现缺陷 | `standards-repro.txt`、`recorder-repro.txt` |

蓝图校验器把执行卡中的同名 WP 标题也当作新的定义，导致结构检查失败；它还报告 53 个原始源码快照漂移。这是文档/校验器维护问题，不能等同于 53 个产品 bug，也不能把文档检查失败当作产品运行失败。

R11 的真实板卡、HIL、72h 压力及相关外部事实仍缺失；本次没有重新构建并运行 QEMU 多窗、真实硬件、72h 浸泡或换机重建。11 个 PASS 是现有候选工具的结果，鉴于 R3/R4，不应视为本次已重新证明全部行为合格。全仓测试/特征矩阵为 host 范围，不能替代目标实弹。

## 复现材料

从仓库根目录运行：

```bash
cargo run --offline --manifest-path evidence/70/review-20260919/standards-repro/Cargo.toml
cargo test --offline --manifest-path evidence/70/review-20260919/held-repro/Cargo.toml --test repro review_ -- --nocapture
python3 evidence/70/review-20260919/reproduce_candidate.py
python3 evidence/70/review-20260919/recorder_repro.py
python3 tools/r10_plan_accounting.py evidence/70/review-20260919/unrelated-intents.txt
```

Rust 红测是要求正确行为的断言，**当前实现应当失败**。候选复现器在临时目录复制 xtask、插入两项测试，业务源码不改动；依赖和 lock 从本 checkout 获取。standards/held 独立 Cargo 项目引用此工作区源码，只有测试替身、没有外部设备写入。详细机器结果见 `summary.json`。
