# ADR-0048：决策有类型产物链与构造权收口（蓝图 61 v0.314.28）

- 状态：已接受（2026-08-30）
- 关联：蓝图 61 [02] §7、[06] §3、[16] §3.6、[17] .28 行；ADR-0047（观测事件）
- 影响面：decision-contract（新 crate）、control-dispatch（typed 入口 + 迁移 adapter）、
  decision-contract trybuild 门

## 背景

Phase 0 收口后，决策链仍以"裸 proposal"为合同：`power_kw + target_device:
String + 12 态链`。求解状态/输入摘要/单位没有类型约束，`ProposedControlPlan`
直达 dispatch 内部字段；候选、计划、认证、命令意图在类型上不可区分——
编译器无法阻止未认证计划进入执行面。

## 裁决

1. **新 crate `crates/ai/decision-contract`**（no_std + alloc；依赖仅
   telemetry-model 值对象 + sha2）：七类产物 IntentCandidate/ObjectiveSpec/
   DecisionRequest/CandidatePlan/CertifiedPlan/CommandIntent/CommandBatch，
   全部私有字段 + 校验构造器（缺字段/非有限/倒窗/越界/digest 零在构造即
   拒绝）。**不定义、不重导出 AuthorizedCommand**（源码审计测试锁定每行
   代码）。wire V1（CAND/CERT/CINT magic）先校后分配，未知版本显式
   `VersionUnsupported`。
2. **构造权收口**：`CertifiedPlan` 构造需要 `CertifierSeal`（零大小
   token，无公开构造器）。默认态（无 feature）外部**不可能**构造证书；
   外部合法面 = `decode_v1` + `verify_binding`。迁移窗口
   （`transition-certifier` feature，.28-.37）下 ControlChain 十二步复核
   （gateway 独立裁决路径——与生成器不同代码路径，非 SafetyValidator bool）
   作为过渡认证器发放 seal；真 certifier crate（.38）落地即删除 feature。
   `CommandBatch::new` 只接受 `&CertifiedPlan` + `&dyn ResourceRegistry`
   （.35 卷 resource-registry 落地前由 dispatch profile 单源充当视图）。
3. **trybuild 编译失败测试**（[06] §3.5.1"编译失败测试：CandidatePlan
   不能传给 execute；非 certifier 不能构造 CertifiedPlan"）：
   - 外部构造 CertifiedPlan → 编译失败；
   - 外部构造 CertifierSeal（`_priv` 私有）→ 编译失败；
   - CandidatePlan 作为 `&AuthorizedCommand` 传参 → 编译失败（类型不可
     互换——执行入口签名未变，仍只接受 Gateway 产物）。
   trybuild stderr 快照按 workspace feature 合一态（feature 恒开）生成
   并 cfg 门控；无 feature 态由 lib 源码审计 + decode 测试覆盖。
4. **typed 入口**：`DispatchEngine::run_typed/submit_typed`——CandidatePlan
   → 过渡认证证书（plan_sha256 = SHA-256(CandidatePlan V1 wire)）→
   CommandIntent（profile 绑定）→ CommandBatch（scope/绑定/窗口校验）→
   12 步合核。执行面**不变**：投递仍只经 AuthorizedCommand（唯一
   Gateway 构造）；typed 产物是决策侧前置链，不是第二执行真相。
   correlation 派生：`PlanTyped` 用 PlanId 字节 = facade 的
   `correlation_of(plan_id)` 同派生——跨路径幂等键一致（集成测试
   facade_and_typed_share_identity 锁定：同 proposal 两路径命中同一
   幂等记录，零重复投递）。
5. **迁移 adapter**（`control-dispatch::adapter::proposal_to_candidate`）：
   proposal 在 dispatch 边界进入 CandidatePlan——链内只消费 typed 产物。
   身份派生确定性规则：PlanId = UTF-8(plan_id) 前 16B 零填充（与
   correlation_of 同派生）；SnapshotId = input_digest 前 16B（provenance
   即快照绑定；None（测试形态）→ plan_id 派生伪快照）；非成功 solve
   status 拒绝（双保险）。facade reader 保留至 .32（[17] .32 删除期限）。

## 事实差异与序列裁决（如实）

- **后端原生产 CandidatePlan 推迟至 .29**：[06] §3.4"host runner/remote
  brain/heuristic 只能生成 IntentCandidate 或 CandidatePlan"。.28 建立链
  与 adapter（后端产物在 dispatch 边界 typed 化）；后端原生切换与 69B
  proposal wire → CandidatePlan wire 的传输切换在 .29 **一次协调完成**
  （避免同一生产者两次迁移——69B writer 删除期限 .29 不受影响）。
- **feature 加法性限制**：Cargo feature 任何 crate 皆可启用——
  `transition-certifier` 的收口执行面 = (a) 默认态无构造器（trybuild
  锁定）；(b) `rg 'issue_transition'` 门 = 仅 control-dispatch 裁决路径；
  (c) .38 删除 feature。这是 Rust 无 friend-crate 下的最大可达强度。
- **objective_digest 迁移期全零**：ObjectiveSpec 生产者（objective-engine）
  为 .36 卷；CandidatePlan 构造器允许零值并在文档标注"迁移期无
  ObjectiveSpec"。

## 结果

- 编译器现在阻止：未认证计划构造证书、CandidatePlan 直达执行签名、
  跨类型产物混用。
- workspace 7930 测试全绿；Phase 0 参考闭环经 typed 链（facade 内部
  adapter → run_typed → 证书/意图/批 → 12 步链）继续通过。
