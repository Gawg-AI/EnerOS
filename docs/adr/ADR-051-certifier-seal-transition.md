# ADR-051：CertifierSeal 发放收口与 control-dispatch 过渡窗延长（蓝图 61 v0.314.38）

- 状态：已接受（2026-08-31）
- 关联：蓝图 61 [09] §4；ADR-0048（决策链/迁移窗口）；[16] §3.9
- 验收映射：[09] §4.7 退出 gate——只有 CertifierModule 生成
  CertifiedPlan；变异覆盖每类硬约束；key 生命周期与回滚

## 背景

ADR-0048 将 `transition-certifier` feature（control-dispatch 十二步链内
复核作为过渡认证器）的生命窗口定为 .28-.37，并约定"真 certifier crate
（.38）落地后删除本 feature"。

.38 实际落地 `eneros-certifier`（独立评估器 + proof records + key
绑定），但**目标侧 dispatch 仍无法切换**到真认证器：独立评估的输入
（TrustedSnapshot、CapabilityEnvelope、储能物理参数）依赖 Phase 2/3
已登记 BLOCKED 的目标侧存储句柄移交与 svc-snapshot/svc-resource-registry
接线；dispatch 目前在目标侧只有自身安全目录，不具备重算全部硬约束的
输入。

## 裁决

1. **真发放面**：`eneros-decision-contract` 新增 `certifier` feature，
   `CertifierSeal::issue()` + `CertifiedPlan::new_certified` 仅该 feature
   可达；仅 `eneros-certifier` 声明启用（rg 门 + compile-fail 锁定）。
2. **过渡窗延长（偏离登记）**：`transition-certifier` 保留至 Phase 5
   PlanCompiler 落地、目标侧认证输入齐备后删除。期间 CertifiedPlan 的
   **规范生产者 = eneros-certifier**（host/planner 路径）；dispatch 的
   过渡发放面仅服务目标侧单 PCS 迁移链，两个发放面在 ADR-0048 的
   "生成器不能给自己签证书"约束下各自独立（certifier 与 planner 不同
   crate、不同代码路径）。
3. **key 绑定**：证书以站内 certifier key 的 HMAC-SHA256 绑定
   （`CertificateEnvelope`：key_id + key_version + mac）；Gateway 验证
   key id/版本/有效期/plan digest——只证明认证产物来源，不替代
   Gateway 权限裁决。key 生命周期：rotate（前代 N-1 验证窗口）/
   retire（立即回滚）。
4. **净功率面的互斥语义**：充放/购售互斥在 Assignment（净功率）合同
   面不可观测；其保证在 planner 模型 binary 约束（模型侧性质），
   独立评估器覆盖能量守恒/包络/承诺/备用等可观测硬约束（[09] §4.2
   全清单中无物理危害的不可观测项如实登记）。

## 后果

- Phase 5 前置：目标侧快照/包络输入接线 + dispatch 切换 certifier +
  删除 transition-certifier（本 ADR 与 ADR-0048 一并收口）。
- certifier no_std + alloc，可移植目标侧 svc-certifier（无 writer、
  无 planner 依赖；shadow 测试为 dev-dependency 不入生产闭包）。
