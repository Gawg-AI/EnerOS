# ADR-035：W5.1 AI 决策入审计链——AiDecision 事件 + DecisionRecord 五要素

- 状态：已接受（2026-08-09）
- 关联：蓝图 22 §2.W5.1；ADR-025（审计链 SM3）；红线7
- 验收映射：决策事件在审计链可查 ✓；篡改决策记录被审计验证检出 ✓

## 背景

审计链（SM3 哈希链）真实可用但 AI 决策不接入。要求每条决策
（意图、LP 解、安全校验结果、执行结果、偏差）可事后追溯，
篡改可检出。

## 裁决

1. `AuditEventType::AiDecision = 12` 新增事件类型（判别值连续 1~12）。
2. `dual-brain/src/audit.rs` 新增 `DecisionAudit`（包装
   `AuditLoggerImpl<MemStore>`）：
   - `DecisionRecord` 结构体承载五要素（actor/target/intent/
     lp_solution/safety/exec/deviation/now_ms）——避免 9 参数函数
     （clippy::too_many_arguments）；
   - detail 紧凑编码 `i=意图;p=LP解;s=安全;x=执行;d=偏差`（≤256B
     定容，`AuditEvent::new` 超长静默截断兜底）；
   - `now_ms` 写入 `timestamp_ms`（此前构造默认 0）。
3. 安全阻断的决策**同样入链**（拒绝可审计——"没做"也是事实）。
4. 篡改检出沿用审计链既有机制：`verify_chain` 重算组合哈希，
   翻转 `event_hash` 一字节 → `broken_at` 定位（与 INT20 同模式）。

## 理由

- 复用既有 SM3 哈希链，无新密码学依赖；`AiDecision` 判别值不与
  既有 11 类冲突（`as u8` 编码无 match 全枚举需改）。
- 五要素入 detail（而非分五条事件）保持单决策单条目、链长与
  决策数 1:1，事后检索/审计导出简单。

## 影响

- `security/audit`：+1 事件类型（无行为变化）；
- `dual-brain`：+`audit` 模块（4 测试：可查/篡改检出/链式衔接/
  阻断也入链）；+1 依赖（eneros-audit，workspace 内部 crate）。
