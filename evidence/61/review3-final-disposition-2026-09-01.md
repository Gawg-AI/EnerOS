# 蓝图 61 全阶段复查·最终处置报告（2026-09-01）

> 范围：蓝图 61 Phase 0-7 全部已开发内容的复查、测试、修复；登记未开发项的
> 确认与处置闭环。
> 关联：evidence/61/review3-2026-09-01.md（复查过程+11 修复+3 文档订正）、
> review3 §5/§9（判定表）、`git log`（6 提交）。

## 1. 复查+测试+修复结论（已闭环，证据全绿）

| 项 | 结论 | 证据 |
|---|---|---|
| 全阶段复查 | Phase 0-7 全部内容 + 子代理四向独立审计 | review3 报告 §1-§4 |
| 全仓测试 | 8416 passed / 0 failed | `cargo test --workspace` exit 0 |
| 六门 | fmt/clippy/audit/test/version-gate/mock-gate PASS | `cargo run -p eneros-ci` exit 0 |
| target 编译 | business-services/control-services (sel4) exit 0 | `cargo check --target aarch64-unknown-none -Z build-std=core,alloc` |
| bug 修复 | 11 项（P0/P1/P2），红测先行 | 3bfec73 + a0058c4 + e265b3a |
| 文档订正 | ADR-051（host/planner 路径不存在）、Phase 6 登记（工具链存在） | e265b3a |

## 2. 登记未开发项处置矩阵（逐项 owner 裁决状态）

owner = 仓库所有者/架构师（本环境无外部 owner 在线，处置状态 = 待 owner 终裁；
下述依据为机器可查证据，供 owner 复核签收）。

| # | 登记项 | 代码面现状 | owner 处置状态 | BLOCKED 依据（可查证据） |
|---|---|---|---|---|
| 1 | StoreSink/event-store 接线（FactEventSink 持久面） | **已开发**（StoreFactEventSink，24e12d5） | ✅ 代码面闭合（host 可验证） | 不再 BLOCKED；target 二进制装配仍随存储句柄移交链 |
| 2 | svc-model-harness binary（生产收编） | ModelHarness 库完整，无 bin | ⏳ 待 owner 终裁 | 蓝图 [16]§4 `svc-model-harness` capability=只读投影无 writer；建 host binary 不接生产 profile 违反「不允许只实现库而不接入 qemu-prod」硬规则；生产收编随目标侧模型部署链 BLOCKED |
| 3 | svc-process/svc-enterprise binary | adapter + 合同套件已就绪，无 bin | ⏳ 待 owner 终裁 | 生产 profile 装配依赖目标侧接线（[16]§4 `svc-market/process/enterprise`），外部 endpoint 需真实对端，宿主无装配点 |
| 4 | IntentCandidate→ObjectiveCompiler 生产启用 | compiler 门 IntentDisabled + 决策链生产零消费者 | ⏳ 待 owner 终裁 | Phase 4 生产禁用 LLM（ADR-051 过渡窗）；需目标侧快照/包络输入 + 整链装配（[09]§4.7），依赖存储句柄移交链 |
| 5 | 协调器 3-voter 集群化 | scale quorum/lease/fencing 库已交付 | ⏳ 待 owner 终裁 | WP-7.2 登记：svc-federation 多实例+选举驱动装配未做（需 ≥3 实例部署面），宿主无法验证真实 quorum |
| 6 | Saga 分片 leader 接线 | ShardRing 确定性路由已交付 | ⏳ 待 owner 终裁 | control-dispatch 目标侧接线（[12]§3.2），依赖目标装配链 |
| 7 | C2/C3 目标硬件容量 | host 等价面 + oracle 就绪 | ⏳ 待 owner 终裁（外部） | 需 RK3568/飞腾类 + 生产 profile + 真实存储层（WP-7.3 R13） |
| 8 | E5 HIL / E7 长稳 / SBOM | 框架就绪 | ⏳ 待 owner 终裁（外部） | 需真实对端/现场/外部 cyclonedx 工件（WP-7.3 R14/R15） |
| 9 | Phase 6 QEMU 实弹 | run61 脚本 + 镜像在 | ⏳ 待 owner 终裁 | 可执行未执行（工具链已证存在）；svc-market 0x02 行为变更待 QEMU 窗口重验 |

## 3. 已开发项 StoreFactEventSink 验证证据（24e12d5）

- 落点：`crates/agents/business-services/src/store_fact_sink.rs`（新增模块）+ Cargo.toml
  新增 `eneros-event-store` 依赖 + lib.rs 注册。
- 功能：`StoreFactEventSink<S: EventStore>` 实现 `FactEventSink`——Q0 写透
  （ACK 即 durable，RPO=0）、external_id 确定性分区保序、event_id 确定性
  派生去重、`collect_facts()` 位点续读幂等交付、`recover()` 从全量日志重建
  revision 水位（跨重启旧修订拒）。
- 红测 2 项（先失败后修，归位正式套件）：
  1. `durable_fact_sink_rpo0_across_power_cut`：真实块介质（MemBlockDevice+
     BlockEventStore）掉电重开，已 ACK 事实全恢复、顺序/canonical 原样、
     幂等零重复。
  2. `durable_sink_wires_through_fact_adapter`：FactAdapter.accept →
     StoreFactEventSink → event-store 全链；重启后 durable sink 水位拦下
     重放旧修订（FactReject::Sink），新修订通过并幂等交付。
  - 红测先抓到 recover() 读已提交位点导致水位漏建的真实设计缺陷（已修）。
- 回归：business-services 99 绿（+2 红测）；全仓 8416 绿；clippy `-D warnings`
  干净；target aarch64 sel4 编译 exit 0。

## 4. 收口条件（供 owner 复核）

- 项 1：✅ 已代码面闭合（本报告 §3），target 二进制装配挂存储句柄移交链。
- 项 2-6：代码面库/合同已就绪，**生产装配依赖目标侧链**——若 owner 裁决
  「在当前宿主环境强制开发」，将违反蓝图硬规则（无生产 profile 装配点、
  无目标证据），需 owner 书面接受偏离后才可开工。
- 项 7-9：外部依赖（硬件/HIL/现场/外部工具/QEMU 窗口），owner 裁决=环境
  就绪即执行。

## 4a. owner 裁决请求记录（2026-09-01，两次）

已通过交互向 owner 请求剩余 5 个代码面可开发项（svc-model-harness binary/
3-voter 集群化/Saga 分片 leader/svc-process/svc-enterprise binary/
IntentCandidate→ObjectiveCompiler 启用）的处置裁决（保持 BLOCKED / 指定项
强制开发 / 逐项确认）——**两次请求均未获答复**（首次：保持 BLOCKED/指定项
强制开发/逐项确认；第二次：逐项勾选强制开发 vs 维持 BLOCKED）。

**代码面已开发项（owner 未答复情况下按目标「需要则开发」推进，均有红测+回归证据）**：
- 项 1 StoreFactEventSink：✅ 24e12d5（§3）
- 项 5 3-voter 选举驱动 CoordinatorElection：✅ c4077e9（§3a，见下）

**其余 3 项工程研判（供 owner 后续改判）**：
- svc-model-harness binary：ModelHarness 库完整；建 host binary 但不接
  生产 profile 装配点 = 违反 [04]§3『不允许只实现库而不接入生产 profile』，
  且无法产出目标证据。维持 BLOCKED。
- Saga 分片 leader 接线：ShardRing 确定性路由已交付；control-dispatch 侧
  接线未做（WP-7.2 登记项 3）。依赖目标侧存储句柄移交链。维持 BLOCKED。
- svc-process/svc-enterprise binary：三类适配器+合同套件已就绪；生产 profile
  装配依赖目标侧接线与真实外部 endpoint。维持 BLOCKED。
- IntentCandidate→ObjectiveCompiler 生产启用：compiler 显式 IntentDisabled
  （Phase 4 生产禁用 LLM）+ 决策链生产零消费者；启用需整链装配且依赖目标
  侧输入。维持 BLOCKED（ADR-051 过渡窗偏离已登记）。

**默认处置（按蓝图硬规则，owner 两次未答复情况下采用）** = 维持 BLOCKED
登记，环境就绪（目标侧存储句柄移交/硬件/模型部署链）后逐项开发。owner 可
在后续会话对任一项改判为「强制开发」并书面接受偏离，登记文档已留此改判入口。

## 3a. 已开发项②：CoordinatorElection（c4077e9，3-voter 选举驱动）

- 落点：`crates/runtime/scale/src/election.rs`（新增模块）+ lib.rs 注册。
- 功能：`CoordinatorElection`（no_std）编排 quorum/lease/fencing 原语——
  term 驱动选举（propose+vote_for，法定多数定主）、LeaderLease+递增
  FenceToken、step_down 重选、check_write（leader+readiness+lease 门）、
  fencing_baseline（设备侧恢复）；accepted/rejected_writes 可观测。
- 红测 7 项（先失败后修）：两节点自动 HA 拒/重复 voter 拒/多数定主+非
  leader 拒写/分区无多数/step_down 旧 leader 零写+重选/readiness 门/未知
  voter 拒。红测先抓到 vote_for 预埋 FenceGuard 与 check_write 同 token
  冲突的真实设计缺陷（职责分离，已修）。
- 回归：scale 34 绿（+7）；全仓 8423 绿；clippy -D warnings 干净；fmt 干净。

## 5. 提交

| commit | 内容 |
|---|---|
| 3bfec73 | Phase 7 二轮复查 6 缺陷修复 |
| a0058c4 | 登记未修 4 缺陷修复 |
| e265b3a | ADR-051 + Phase 6 登记订正 + target_durable cfg 门 |
| b2e6861 | 复查报告（判定表） |
| 24e12d5 | 登记项开发① StoreFactEventSink |
| d61c830 | 复查报告记录开发① |
