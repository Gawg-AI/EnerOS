# ADR-052：saga 假 seam 冻结登记（蓝图 63 WP-63.1.4，v0.316.0）

- 状态：已接受（2026-09-03）
- 关联：蓝图 61 [10] §3（BatchSaga WP-5.2）、[09] §2.7；蓝图 62 卷 62.1
  WP-62.1.2（D-3 红测）；蓝图 63 [00] §3.3 ADR 保护清单（本 ADR 为蓝图 63
  唯一新增保护条目）、[00] §4 不做边界（product profile 驱动 root-task 接线、
  Saga 分片 leader）
- 裁决对象：`crates/protocols/control-dispatch/src/saga.rs`（BatchSaga，
  ~916 行）+ `engine.rs` `EngineSagaExecutor`（十参构造，含
  `legacy_single: bool` 开关）
- 验收映射：蓝图 63 [02] WP-63.1.4 判据 1（ADR 文件 + registry 登记 +
  模块头状态注释三件齐）

## 背景与事实

1. **零生产消费者**（2026-09-03 全仓 rg 实测）：`saga.rs` / `EngineSagaExecutor`
   的非测试消费者 = 0。仅 engine.rs 模块 doc 注释 + decision-contract/
   candidate.rs 注释提及；驱动者 = `tests/saga_e4e5.rs`（19 用例）+ 
   `tests/settlement_lock.rs` D-3 红测（`saga_executor_binds_plan_wire_sha_
   not_input_digest`）。
2. **删除测试结论**：删 `saga.rs` + `EngineSagaExecutor` + 相关测试约 −1100
   行，生产行为零变化（无消费者可验证）。
3. **62.1 D-3 红测存在本身有价值**：该红测锁「`EngineSagaExecutor` 落进
   CoreCommand 的 plan_sha 必须 = CandidatePlan wire SHA-256 且与 input_digest
   语义分离」——saga 存在时它守护的是 interface 语义；若 saga 删除，该红测
   守护的对象不复存在。
4. **为何不默认删除**：删除 ≈ −1100 行是纯负收益（无人要求清行数）；而
   BatchSaga 是蓝图 61 Phase 5 正式交付物（[10] §3），批提交入口
   （svc-model-harness binary 建壳 / product profile 驱动接线 / Saga 分片
   leader）均被 BLOCKED 前置项挡住（目标侧存储句柄移交等）。机制本身经 19
   用例 + Phase 5 实弹覆盖，是**冻结资产**而非债务。
5. **为何不默认继续投资**：62 前每卷都在给 saga 加参数/行为（十参构造、
   legacy_single 开关）——零消费者前提下任何新参数都是假 seam 收税。

## 裁决（默认 = 冻结登记，无需 owner 即可执行）

1. **冻结纪律**（本 ADR 生效起）：在第一个生产批提交入口出现前，不得向
   `BatchSaga` / `EngineSagaExecutor` 添加任何**新参数、新方法、新测试以外
   的行为**；不得为「将来接入」写新代码。代码级 bug 修复例外（走常规缺陷
   流程，修完仍受本纪律约束）。既有 19 用例 + D-3 红测保留为**冻结资产
   守卫**——它们证明冻结期间机制未腐化。
2. **激活前置**（满足任一即解冻，需另立修订/新 ADR）：
   a. 生产 profile 驱动接线落地（root-task 或 svc 装配真实驱动 BatchSaga）；
   b. 批提交入口裁决（多设备投递面 WP-5.2 Saga 卷 / svc-model-harness
      binary 建壳，均登记总册）；
   c. 显式 owner 裁决删除（本 ADR 的删除支：删 saga.rs + EngineSagaExecutor
      + saga_e4e5.rs 相关测试 + settlement_lock.rs D-3 红测，~ −1100 行）。
3. **D-3 红测语义澄清**：D-3 红测存在 = interface 管不住 plan_input_digest /
   plan_sha 同型互换（62.1 实证缺陷形态）——冻结期间继续守卫该教训。
4. **EngineSagaExecutor::new 的 `#[allow(clippy::too_many_arguments)]` 保留**
   （63.1.3 唯一未删 allow）：函数属冻结面，重构其构造收拢样板 = 冻结纪律
   禁止的「新行为外改动」，等激活前置触发时随重构一并删除。

## 不做边界（交叉引用）

- product profile 驱动 root-task 接线：维持蓝图 63 [00] §4 登记，不实施。
- Saga 分片 leader / svc-process / svc-enterprise binary：维持 BLOCKED 登记。

## 修订（2026-09-20）

1. **激活前置已满足**：§2b（批提交入口裁决）经 ADR-0054（2026-09-17
   Accepted，`docs/adr/ADR-0054-held-plan-production-entry.md`）触发——
   ADR-0054 明文「本决策满足 ADR-0052 的批入口裁决前置；只有接入这个
   真实入口所需的行为解冻，不能以本文件宣称生产已激活」。据此对 saga
   面 + engine 驱动路径做**定向解冻**（仅接入所需行为，冻结纪律对无关
   扩张面继续生效）。
2. **生产激活证据与时点状态并记**：R10 专属门 22/22 单次全绿（evidence/
   70/70.15/ 树，2026-09-18/19）+ 马拉松 v4 R01–R09+R12 PASS = 生产激活
   证据；候选链 R10 状态在 a72ae68 时点仍为 MISSING（候选重录未完成）——
   事实与时点并记，不单写「生产已激活」结论。
3. **解冻后新纪律**：不新增第二 saga 执行真相（ADR-0054 既有约束延续）。
4. **§4 allow 退役排期**：`EngineSagaExecutor::new` 的
   `#[allow(clippy::too_many_arguments)]` 于蓝图 72 修复批B B2（构造收拢
   `PlanBinding` + `DeliveryContext`，11 参 → 5 参）一并删除。

