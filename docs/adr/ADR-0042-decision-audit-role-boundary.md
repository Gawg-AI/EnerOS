# ADR-0042：dual-brain/audit.rs 保留——DecisionAudit 是 security/audit 的决策语义门面

> 状态：Accepted（2026-08-09）
> 决策者：架构深化实施（蓝图 23 W4.2）
> 前置：ADR-0035（AI 决策审计）；22 蓝图 §G-R5（决策纪律闸门）。
> 审计证据（实施时代码复核，纠正规格 §0.2 结论 2 的误判）：
> - `crates/ai/dual-brain/src/audit.rs`（229 行）的 `DecisionAudit` **不是**独立审计体系——
>   它包装 `security/audit` 的 `AuditLoggerImpl<MemStore>`（SM3 哈希链），
>   用 `AuditEventType::AiDecision`（=12）入链（audit.rs:15-19, 111）。
> - `DecisionAudit` 是 22 G-R5 验收测试 `dual-brain/tests/gr5_decision_discipline.rs` 的
>   核心组件（第 1/2 步：决策入链 + 篡改检出 broken_at 定位）。
> - 规格断言"dual-brain 之外零调用方"属实（外部无代码引用），但结论"双审计体系并存"
>   不成立——`DecisionAudit` 与 `security/audit` 是**门面 + 基础设施**的分层，非重复。

## 1. 背景

蓝图 23 §0.2 结论 2 原始表述："`dual-brain/audit.rs` 的 `DecisionAudit`（SM3 哈希链）在
dual-brain 之外零调用方……避免双审计体系并存"。但实施复核发现：`DecisionAudit` 内部
**直接使用** `security/audit` 的链实现（`AuditLoggerImpl`/`ChainVerifierImpl`/`MemStore`），
并为 AI 决策提供结构化语义（`DecisionRecord` 五要素 + `SafetyOutcome`/`ExecOutcome` 类型、
`deviation` 偏差指标、`i=意图;p=LP解;s=安全;x=执行;d=偏差` 紧凑编码）。这不是第二套审计体系，
是 `security/audit` 通用链的**决策专用门面**。

## 2. 决策

1. **保留 `dual-brain/audit.rs`**：`DecisionAudit` 是 `security/audit` 的决策语义门面，
   提供"决策可追溯"的领域入口（ADR-0035 能力由它向 AI 层暴露）。
2. **职责边界（本 ADR 固化，防下次重提）**：
   - `security/audit` = 通用审计基础设施：事件链（SM3）、存储、验证、事件类型。
   - `dual-brain/audit.rs` = 决策语义门面：把 AI 决策（意图/LP 解/安全/执行/偏差）
     编码为 `AiDecision` 事件入链，并提供 `SafetyOutcome`/`ExecOutcome`/`DecisionRecord`
     领域类型。**不复制链实现**（零哈希/存储/验证逻辑，全部委托 security/audit）。
3. **不接线到更多调用方**：当前唯一消费者是 22 G-R5 验收测试 + 内部单测。
   作为门面模块，保留 `pub mod audit`（生产可接线），doc 注释已标注用途。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 删除 audit.rs（规格选项 a） | 会破坏 22 G-R5 验收证据（gr5_decision_discipline.rs 直接用 DecisionAudit）；且"双体系并存"的定性不成立——它是门面非重复实现 |
| 保留但标 `#[doc(hidden)]` 等隐藏 | 它是决策审计的正式门面，GR5 闸门核心；隐藏反而误导"审计不可用" |

## 4. 影响面

- 无代码改动（裁决为保留 + 边界明确化）。
- `docs/adr/ADR-0042` 固化职责边界，供未来探索者检索。

## 5. 验收

- `cargo test -p eneros-dual-brain` → 全 PASS（含 audit.rs 4 单测 + gr5 集成测试）。
- ADR-0035 决策可追溯能力不退化（gr5 测试第 1/2 步：入链 + 篡改检出仍绿）。

## 6. 追踪

- 蓝图：23 §5 W4.2；22 蓝图 §G-R5。
- 纪律：ADR-0035（AI 决策审计）、锁定 1（interface 诚实——门面即真实在用）。
